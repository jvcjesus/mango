//! Acquisition Job Recovery & Lifecycle Analysis (Milestone M8.3).
//!
//! Provides granular lifecycle inspection, crash recovery analysis, and safety-guarded retry actions:
//! - Full lifecycle stage tracking: Discovered -> Submitted -> Downloading -> Validating -> Publishing -> Published -> Komga Verified -> Metadata Completed
//! - Komga reconciliation state: not_published, published_awaiting_scan, scan_pending, komga_verified, komga_verification_failed, ownership_conflict
//! - KOMF metadata enrichment state: not_requested, pending, processing, completed, failed, retry_available
//! - Crash recovery analysis: inspects existing publication receipts and disk artifacts to recommend safe idempotency-preserving next actions

use std::path::Path;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::pipeline::publisher::verify_cbz_integrity;
use crate::pipeline::receipts::{get_publication_receipt_by_chapter, get_publication_receipt_by_path, PublicationReceipt};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecoveryInfo {
    pub chapter_id: i64,
    pub title: String,
    pub volume_number: Option<f64>,
    pub chapter_name: String,
    pub state: String,
    pub stage: String,
    pub failed_stage: String,
    pub error_category: String, // "network", "corrupt_archive", "komga_unreachable", "komf_metadata_error", "timeout", "filesystem", "none"
    pub error_message: Option<String>,
    pub last_successful_stage: String,
    pub artifact_exists: bool,
    pub artifact_path: Option<String>,
    pub canonical_publication_occurred: bool,
    pub canonical_receipt_id: Option<String>,
    pub canonical_path: Option<String>,
    pub komga_status: String, // "not_published", "published_awaiting_scan", "scan_pending", "komga_verified", "komga_verification_failed"
    pub metadata_status: String, // "not_requested", "pending", "processing", "completed", "failed", "retry_available"
    pub retry_is_safe: bool,
    pub recommended_action: String, // "verify_komga", "republish_staged_archive", "safe_restart_download", "already_verified_complete", "safe_retry_download"
    pub can_recover: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecoverySummary {
    pub total_failed_downloads: usize,
    pub total_stalled_downloads: usize,
    pub recoverable_downloads_count: usize,
    pub items: Vec<DownloadRecoveryInfo>,
}

/// Analyzes an acquisition/download job to determine its precise recovery boundary and next safe action.
pub fn analyze_download_recovery(
    conn: &Connection,
    chapter_id: i64,
) -> Result<DownloadRecoveryInfo> {
    // 1. Fetch download row
    let row: Option<(
        i64,
        String,
        Option<f64>,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    )> = conn
        .query_row(
            "SELECT d.chapter_id, m.title, c.volume, c.name, d.state, d.error,
                    d.failure_stage, d.published_path, d.retrieved_path, d.source_type,
                    d.backend_type, d.external_job_id, d.attempts
             FROM downloads d
             JOIN chapters c ON c.id = d.chapter_id
             JOIN manga m ON m.id = c.manga_id
             WHERE d.chapter_id = ?1",
            [chapter_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            },
        )
        .optional()?;

    let (
        ch_id,
        title,
        volume_number,
        chapter_name,
        state,
        error_msg,
        failure_stage,
        published_path,
        retrieved_path,
        _source_type,
        _backend_type,
        _ext_job_id,
        _attempts,
    ) = match row {
        Some(r) => r,
        None => return Err(AppError::not_found(format!("Download for chapter {chapter_id} not found"))),
    };

    // 2. Check publication receipt
    let receipt: Option<PublicationReceipt> = get_publication_receipt_by_chapter(conn, ch_id)?
        .or_else(|| {
            if let Some(ref p) = published_path {
                get_publication_receipt_by_path(conn, p).ok().flatten()
            } else {
                None
            }
        });

    let mut canonical_publication_occurred = false;
    let mut canonical_receipt_id = None;
    let mut canonical_path_res = None;
    let mut komga_status = "not_published".to_string();
    let mut metadata_status = "not_requested".to_string();
    let mut artifact_exists = false;
    let mut artifact_path_res = None;
    let mut last_successful_stage = "submitted".to_string();
    let mut failed_stage = failure_stage.unwrap_or_else(|| state.clone());
    let mut retry_is_safe = true;
    let mut recommended_action = "safe_retry_download".to_string();
    let mut can_recover = true;

    // Error categorization
    let error_category = if let Some(ref err) = error_msg {
        let lower = err.to_lowercase();
        if lower.contains("timeout") || lower.contains("timed out") || lower.contains("429") || lower.contains("503") || lower.contains("connection") {
            "network".to_string()
        } else if lower.contains("komga") {
            "komga_unreachable".to_string()
        } else if lower.contains("metadata") || lower.contains("komf") {
            "komf_metadata_error".to_string()
        } else if lower.contains("corrupt") || lower.contains("invalid zip") || lower.contains("cbz") {
            "corrupt_archive".to_string()
        } else {
            "acquisition_failed".to_string()
        }
    } else {
        "none".to_string()
    };

    if let Some(r) = receipt {
        canonical_publication_occurred = true;
        canonical_receipt_id = Some(r.id.clone());
        canonical_path_res = Some(r.canonical_path.clone());

        komga_status = if r.verified_ownership {
            "komga_verified".to_string()
        } else if r.komga_notified {
            "published_awaiting_scan".to_string()
        } else {
            "scan_pending".to_string()
        };

        metadata_status = match r.metadata_status.as_str() {
            "complete" => "completed".to_string(),
            "failed" => "failed".to_string(),
            "pending" | "submitting" | "in_progress" => "processing".to_string(),
            _ => "not_requested".to_string(),
        };

        // Check canonical file on disk
        let canon_p = Path::new(&r.canonical_path);
        if canon_p.is_file() && verify_cbz_integrity(canon_p).is_ok() {
            artifact_exists = true;
            artifact_path_res = Some(r.canonical_path.clone());
            last_successful_stage = "published".to_string();
            failed_stage = if !r.verified_ownership {
                "komga_verification".to_string()
            } else if r.metadata_status == "failed" {
                "metadata_enrichment".to_string()
            } else {
                "none".to_string()
            };

            if r.verified_ownership && r.metadata_status == "complete" {
                recommended_action = "already_verified_complete".to_string();
                can_recover = false;
                retry_is_safe = false;
            } else if !r.verified_ownership {
                recommended_action = "verify_komga".to_string();
                can_recover = true;
                retry_is_safe = true;
            } else {
                recommended_action = "retry_metadata_only".to_string();
                can_recover = true;
                retry_is_safe = true;
            }
        } else {
            artifact_exists = false;
            recommended_action = "republish_media".to_string();
            can_recover = true;
            retry_is_safe = true;
        }
    } else {
        // No publication receipt: check staging artifact
        if let Some(ref rp) = retrieved_path {
            let sp = Path::new(rp);
            if sp.is_file() {
                artifact_exists = true;
                artifact_path_res = Some(rp.clone());
                last_successful_stage = "downloaded_to_staging".to_string();

                if verify_cbz_integrity(sp).is_ok() {
                    failed_stage = "validation_or_publication".to_string();
                    recommended_action = "republish_staged_archive".to_string();
                    can_recover = true;
                    retry_is_safe = true;
                } else {
                    failed_stage = "archive_integrity_validation".to_string();
                    recommended_action = "safe_restart_download".to_string();
                    can_recover = true;
                    retry_is_safe = true;
                }
            }
        }
    }

    Ok(DownloadRecoveryInfo {
        chapter_id: ch_id,
        title,
        volume_number,
        chapter_name,
        state,
        stage: failed_stage.clone(),
        failed_stage,
        error_category,
        error_message: error_msg,
        last_successful_stage,
        artifact_exists,
        artifact_path: artifact_path_res,
        canonical_publication_occurred,
        canonical_receipt_id,
        canonical_path: canonical_path_res,
        komga_status,
        metadata_status,
        retry_is_safe,
        recommended_action,
        can_recover,
    })
}

/// Lists recovery analysis for all failed, stalled, or error-state downloads.
pub fn list_all_recovery_info(conn: &Connection) -> Result<DownloadRecoverySummary> {
    let mut stmt = conn.prepare(
        "SELECT chapter_id FROM downloads
         WHERE state = 'failed' OR is_stalled = 1 OR error IS NOT NULL
         ORDER BY added_at DESC
         LIMIT 100",
    )?;

    let ch_ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut items = Vec::new();
    let mut recoverable_count = 0;
    let mut failed_count = 0;

    for cid in ch_ids {
        if let Ok(info) = analyze_download_recovery(conn, cid) {
            if info.can_recover {
                recoverable_count += 1;
            }
            if info.state == "failed" {
                failed_count += 1;
            }
            items.push(info);
        }
    }

    let stalled_res: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE is_stalled = 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let stalled_count = stalled_res as usize;

    Ok(DownloadRecoverySummary {
        total_failed_downloads: failed_count,
        total_stalled_downloads: stalled_count,
        recoverable_downloads_count: recoverable_count,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE manga (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL
            );
            CREATE TABLE chapters (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                volume REAL NULL,
                name TEXT NOT NULL
            );
            CREATE TABLE downloads (
                chapter_id INTEGER PRIMARY KEY,
                position INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL DEFAULT 'queued',
                attempts INTEGER NOT NULL DEFAULT 0,
                retry_at INTEGER NULL,
                error TEXT NULL,
                added_at INTEGER NOT NULL,
                published_path TEXT NULL,
                published_at INTEGER NULL,
                target_library_id TEXT NULL,
                source_type TEXT NOT NULL DEFAULT 'reader',
                backend_type TEXT NOT NULL DEFAULT 'internal',
                external_job_id TEXT NULL,
                external_file_id INTEGER NULL,
                torrent_hash TEXT NULL,
                raw_title TEXT NULL,
                uploader TEXT NULL,
                release_size INTEGER NULL,
                retrieved_path TEXT NULL,
                collection_id TEXT NULL,
                collection_name TEXT NULL,
                execution_id TEXT NULL,
                execution_item_id TEXT NULL,
                bytes_downloaded INTEGER NULL DEFAULT 0,
                total_bytes INTEGER NULL,
                last_progress_at INTEGER NULL,
                is_stalled INTEGER NOT NULL DEFAULT 0,
                failure_stage TEXT NULL,
                magnet_url TEXT NULL,
                retrieval_identity_json TEXT NULL
            );
            CREATE TABLE publication_receipts (
                id TEXT PRIMARY KEY,
                chapter_id INTEGER NOT NULL DEFAULT 0,
                manga_id INTEGER NULL,
                series_title TEXT NOT NULL,
                volume_number REAL NULL,
                chapter_number REAL NULL,
                is_volume INTEGER NOT NULL DEFAULT 1,
                canonical_path TEXT NOT NULL,
                file_size_bytes INTEGER NOT NULL,
                file_sha256 TEXT NOT NULL,
                published_at INTEGER NOT NULL,
                komga_notified INTEGER NOT NULL DEFAULT 0,
                komga_library_id TEXT NULL,
                metadata_status TEXT NOT NULL DEFAULT 'complete',
                metadata_error TEXT NULL,
                verified_ownership INTEGER NOT NULL DEFAULT 0,
                verified_at INTEGER NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                komga_book_id TEXT NULL,
                komga_title TEXT NULL,
                verification_status TEXT NOT NULL DEFAULT 'verified',
                verification_details TEXT NULL
            );"
        ).unwrap();
        conn
    }

    #[test]
    fn test_recovery_analysis_with_existing_publication_receipt() {
        let conn = create_test_db();
        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Haikyu!!')", []).unwrap();
        conn.execute("INSERT INTO chapters (id, manga_id, volume, name) VALUES (101, 1, 1.0, 'Volume 1')", []).unwrap();
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, error, added_at, published_path)
             VALUES (101, 'failed', 'Komga notification timed out', 1000, '/data/manga/Haikyu!!/Haikyu!! v01.cbz')",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_title, volume_number, canonical_path,
                file_size_bytes, file_sha256, published_at, komga_notified, metadata_status,
                verified_ownership, created_at, updated_at
            ) VALUES (
                'rcpt-1234', 101, 1, 'Haikyu!!', 1.0, '/data/manga/Haikyu!!/Haikyu!! v01.cbz',
                50000000, 'abc123hash', 1000, 1, 'complete', 0, 1000, 1000
            )",
            [],
        ).unwrap();

        let info = analyze_download_recovery(&conn, 101).unwrap();
        assert_eq!(info.chapter_id, 101);
        assert!(info.canonical_publication_occurred);
        assert_eq!(info.canonical_receipt_id.as_deref(), Some("rcpt-1234"));
        assert_eq!(info.komga_status, "published_awaiting_scan");
        assert_eq!(info.metadata_status, "completed");
        assert_eq!(info.error_category, "network");
        assert!(info.retry_is_safe);
    }

    #[test]
    fn test_recovery_analysis_download_failure_categorization() {
        let conn = create_test_db();
        conn.execute("INSERT INTO manga (id, title) VALUES (2, 'Blue Lock')", []).unwrap();
        conn.execute("INSERT INTO chapters (id, manga_id, volume, name) VALUES (201, 2, 5.0, 'Volume 5')", []).unwrap();
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, error, added_at)
             VALUES (201, 'failed', 'Connection reset by peer: HTTP 503 from Madokami', 1000)",
            [],
        ).unwrap();

        let info = analyze_download_recovery(&conn, 201).unwrap();
        assert_eq!(info.chapter_id, 201);
        assert!(!info.canonical_publication_occurred);
        assert_eq!(info.error_category, "network");
        assert_eq!(info.recommended_action, "safe_retry_download");
        assert!(info.retry_is_safe);
        assert!(info.can_recover);
    }
}
