//! Home Management Dashboard & System Overview (Milestone M8.7).
//!
//! Aggregates cross-system telemetry and intelligence across Library, Acquisition,
//! Release Intelligence, Provider Health, and Storage into an editorial control plane.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::db::Db;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardLibraryStats {
    pub total_tracked_titles: usize,
    pub total_owned_volumes: usize,
    pub missing_released_volumes: usize,
    pub titles_needing_review: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardAcquisitionStats {
    pub active_downloads: usize,
    pub pending_approvals: usize,
    pub failed_jobs: usize,
    pub recoverable_jobs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardReleaseStats {
    pub upcoming_releases_count: usize,
    pub released_awaiting_count: usize,
    pub candidate_found_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProviderHealthItem {
    pub name: String,
    pub category: String, // "acquisition", "canonical", "metadata", "tracker"
    pub status: String,   // "HEALTHY", "CONFIGURED", "AVAILABLE", "DISABLED", "ATTENTION_REQUIRED"
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStorageStats {
    pub staging_bytes: u64,
    pub reclaimable_bytes: u64,
    pub duplicate_groups_count: usize,
    pub optimizer_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummaryDto {
    pub library: DashboardLibraryStats,
    pub acquisition: DashboardAcquisitionStats,
    pub release_intelligence: DashboardReleaseStats,
    pub provider_health: Vec<DashboardProviderHealthItem>,
    pub storage: DashboardStorageStats,
    pub generated_at: i64,
}

/// Generates a comprehensive management dashboard summary across all subsystems.
pub async fn get_dashboard_summary(
    db: &Db,
    downloads_dir: &Path,
    optimizer_bin: Option<&str>,
) -> Result<DashboardSummaryDto> {
    let downloads_dir_buf = downloads_dir.to_path_buf();

    // 1. Storage Optimizer & Staging Probe
    let optimizer_status = crate::pipeline::duplicates::get_storage_optimizer_status(optimizer_bin).await;

    // 2. Database Aggregations
    let (library, acquisition, release_intel, dup_report, provider_health) = db
        .call(|conn| {
            let lib = compute_library_stats(conn)?;
            let acq = compute_acquisition_stats(conn)?;
            let rel = compute_release_stats(conn)?;
            let dups = crate::pipeline::duplicates::scan_library_duplicates(conn)?;
            let prov = compute_provider_health(conn);
            Ok((lib, acq, rel, dups, prov))
        })
        .await?;

    // 3. Staging directory size probe (bounded recursive walk)
    let staging_bytes = compute_directory_bytes(&downloads_dir_buf);

    let storage = DashboardStorageStats {
        staging_bytes,
        reclaimable_bytes: dup_report.potential_reclaimable_bytes,
        duplicate_groups_count: dup_report.total_duplicate_groups,
        optimizer_available: optimizer_status.is_available,
    };

    Ok(DashboardSummaryDto {
        library,
        acquisition,
        release_intelligence: release_intel,
        provider_health,
        storage,
        generated_at: crate::db::now(),
    })
}

fn compute_library_stats(conn: &Connection) -> Result<DashboardLibraryStats> {
    let total_tracked: i64 = conn
        .query_row("SELECT COUNT(*) FROM manga", [], |r| r.get(0))
        .unwrap_or(0);

    let receipts_owned: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT canonical_path) FROM publication_receipts WHERE canonical_path IS NOT NULL AND canonical_path != ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let recon_owned: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(owned_volumes_count), 0) FROM series_reconciliations",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let total_owned = std::cmp::max(receipts_owned, recon_owned);

    let missing_released: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM release_availability_watchers WHERE is_released = 1 AND is_owned = 0",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let needing_review: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM series_reconciliations WHERE status = 'REVIEW_REQUIRED' OR status = 'review_required'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(DashboardLibraryStats {
        total_tracked_titles: total_tracked.max(0) as usize,
        total_owned_volumes: total_owned.max(0) as usize,
        missing_released_volumes: missing_released.max(0) as usize,
        titles_needing_review: needing_review.max(0) as usize,
    })
}

pub fn compute_provider_health(conn: &Connection) -> Vec<DashboardProviderHealthItem> {
    let komga_enabled = crate::settings::get::<bool>(conn, "pipeline.komga_enabled").unwrap_or(false);
    let komga_url = crate::settings::get::<String>(conn, "pipeline.komga_url");
    let komga_status = if komga_enabled && komga_url.is_some() {
        ("HEALTHY", "Canonical library authority and preferred reader active")
    } else if komga_enabled {
        ("CONFIGURED", "Komga enabled; URL pending verification")
    } else {
        ("DISABLED", "Komga canonical integration disabled in Settings")
    };

    let komf_enabled = crate::settings::get::<bool>(conn, "pipeline.komf_enabled").unwrap_or(false);
    let komf_status = if komf_enabled {
        ("HEALTHY", "Automated Komga metadata bridge active")
    } else {
        ("DISABLED", "KOMF metadata bridge disabled in Settings")
    };

    let opentome_enabled = crate::settings::get::<bool>(conn, "opentome.enabled").unwrap_or(true);
    let opentome_status = if opentome_enabled {
        ("HEALTHY", "Official release schedule and publication intelligence active")
    } else {
        ("DISABLED", "OpenTome release tracking disabled in Settings")
    };

    let mb_enabled = crate::settings::get::<bool>(conn, "pipeline.mangabaka_enabled").unwrap_or(true);
    let mb_status = if mb_enabled {
        ("HEALTHY", "Authoritative manga catalog and edition identity active")
    } else {
        ("DISABLED", "MangaBaka disabled in Settings")
    };

    let torbox_enabled = crate::db::state(conn, "acquisition.torbox_enabled")
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1" || v.parse::<bool>().unwrap_or(false))
        .unwrap_or(false);
    let torbox_has_token = crate::db::state(conn, "acquisition.torbox_token")
        .ok()
        .flatten()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    let torbox_validated = crate::db::state(conn, "acquisition.torbox_validated")
        .ok()
        .flatten()
        .map(|v| v == "valid")
        .unwrap_or(false);
    let torbox_status_msg = crate::db::state(conn, "acquisition.torbox_status_message")
        .ok()
        .flatten()
        .filter(|m| !m.trim().is_empty());

    let (torbox_status_str, torbox_details_str) = if !torbox_enabled {
        ("DISABLED", "TorBox external BitTorrent backend disabled in Settings".to_string())
    } else if !torbox_has_token {
        ("UNCONFIGURED", "TorBox API token not configured".to_string())
    } else if torbox_validated {
        (
            "HEALTHY",
            torbox_status_msg.unwrap_or_else(|| "External BitTorrent acquisition and caching backend active".to_string()),
        )
    } else if let Some(msg) = torbox_status_msg {
        if msg.to_lowercase().contains("error") || msg.to_lowercase().contains("invalid") || msg.to_lowercase().contains("failed") {
            ("ATTENTION_REQUIRED", msg)
        } else {
            ("CONFIGURED", msg)
        }
    } else {
        ("CONFIGURED", "TorBox enabled; API connection pending verification".to_string())
    };

    let madokami_enabled = crate::db::state(conn, "acquisition.madokami_enabled")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(true);
    let (mado_status, mado_details) = if !madokami_enabled {
        ("DISABLED", "Madokami direct acquisition disabled in Settings")
    } else {
        ("AVAILABLE", "Direct HTTP with byte-range resume enabled")
    };

    let nyaa_enabled = crate::db::state(conn, "acquisition.nyaa_enabled")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(true);
    let (nyaa_status, nyaa_details) = if !nyaa_enabled {
        ("DISABLED", "Nyaa release discovery indexer disabled in Settings")
    } else {
        ("AVAILABLE", "Torrent discovery indexer active")
    };

    vec![
        DashboardProviderHealthItem {
            name: "Madokami".to_string(),
            category: "acquisition".to_string(),
            status: mado_status.to_string(),
            details: mado_details.to_string(),
        },
        DashboardProviderHealthItem {
            name: "Nyaa".to_string(),
            category: "acquisition".to_string(),
            status: nyaa_status.to_string(),
            details: nyaa_details.to_string(),
        },
        DashboardProviderHealthItem {
            name: "TorBox".to_string(),
            category: "acquisition".to_string(),
            status: torbox_status_str.to_string(),
            details: torbox_details_str,
        },
        DashboardProviderHealthItem {
            name: "Komga".to_string(),
            category: "canonical".to_string(),
            status: komga_status.0.to_string(),
            details: komga_status.1.to_string(),
        },
        DashboardProviderHealthItem {
            name: "KOMF".to_string(),
            category: "metadata".to_string(),
            status: komf_status.0.to_string(),
            details: komf_status.1.to_string(),
        },
        DashboardProviderHealthItem {
            name: "OpenTome".to_string(),
            category: "metadata".to_string(),
            status: opentome_status.0.to_string(),
            details: opentome_status.1.to_string(),
        },
        DashboardProviderHealthItem {
            name: "MangaBaka".to_string(),
            category: "metadata".to_string(),
            status: mb_status.0.to_string(),
            details: mb_status.1.to_string(),
        },
    ]
}

fn compute_acquisition_stats(conn: &Connection) -> Result<DashboardAcquisitionStats> {
    let active_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state IN ('queued', 'downloading', 'processing', 'validating')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let failed_jobs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state = 'failed'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let recoverable: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state = 'failed' AND error IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(DashboardAcquisitionStats {
        active_downloads: active_downloads.max(0) as usize,
        pending_approvals: 0,
        failed_jobs: failed_jobs.max(0) as usize,
        recoverable_jobs: recoverable.max(0) as usize,
    })
}

fn compute_release_stats(conn: &Connection) -> Result<DashboardReleaseStats> {
    let upcoming: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM release_availability_watchers WHERE state = 'UPCOMING'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let awaiting: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM release_availability_watchers WHERE state = 'RELEASED_AWAITING_PROVIDER'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let candidate_found: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM release_availability_watchers WHERE state IN ('ELIGIBLE_CANDIDATE_FOUND', 'NOTIFIED')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(DashboardReleaseStats {
        upcoming_releases_count: upcoming.max(0) as usize,
        released_awaiting_count: awaiting.max(0) as usize,
        candidate_found_count: candidate_found.max(0) as usize,
    })
}

fn compute_directory_bytes(dir: &Path) -> u64 {
    compute_directory_bytes_bounded(dir, 0, 10)
}

fn compute_directory_bytes_bounded(dir: &Path, depth: usize, max_depth: usize) -> u64 {
    if depth > max_depth || !dir.exists() {
        return 0;
    }
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += compute_directory_bytes_bounded(&entry.path(), depth + 1, max_depth);
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE manga (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
            CREATE TABLE publication_receipts (
                id TEXT PRIMARY KEY,
                chapter_id INTEGER NOT NULL,
                manga_id INTEGER,
                series_id TEXT NOT NULL,
                series_title TEXT NOT NULL,
                volume_number REAL,
                canonical_path TEXT NOT NULL,
                file_size_bytes INTEGER NOT NULL,
                file_sha256 TEXT,
                published_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE release_availability_watchers (
                id INTEGER PRIMARY KEY,
                state TEXT NOT NULL,
                is_released INTEGER NOT NULL,
                is_owned INTEGER NOT NULL
            );
            CREATE TABLE series_reconciliations (
                series_id TEXT PRIMARY KEY,
                status TEXT NOT NULL
            );
            CREATE TABLE downloads (
                id INTEGER PRIMARY KEY,
                chapter_id INTEGER NOT NULL,
                state TEXT NOT NULL,
                error TEXT
            );
            CREATE TABLE chapters (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                volume_number REAL,
                chapter_number REAL,
                is_downloaded INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_compute_dashboard_stats() {
        let conn = setup_test_db();
        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Haikyu!!')", []).unwrap();
        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_id, series_title, volume_number,
                canonical_path, file_size_bytes, file_sha256, published_at, created_at
             ) VALUES ('rcpt-1', 101, 1, 'manga:1', 'Haikyu!!', 1.0, '/manga/h.cbz', 500, 'h1', 1, 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO release_availability_watchers (id, state, is_released, is_owned)
             VALUES (1, 'RELEASED_AWAITING_PROVIDER', 1, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, status) VALUES ('manga:1', 'REVIEW_REQUIRED')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO downloads (id, chapter_id, state, error) VALUES (1, 102, 'failed', 'Network timeout')",
            [],
        ).unwrap();

        let lib = compute_library_stats(&conn).unwrap();
        assert_eq!(lib.total_tracked_titles, 1);
        assert_eq!(lib.total_owned_volumes, 1);
        assert_eq!(lib.missing_released_volumes, 1);
        assert_eq!(lib.titles_needing_review, 1);

        let acq = compute_acquisition_stats(&conn).unwrap();
        assert_eq!(acq.failed_jobs, 1);
        assert_eq!(acq.recoverable_jobs, 1);

        let rel = compute_release_stats(&conn).unwrap();
        assert_eq!(rel.released_awaiting_count, 1);
    }
}
