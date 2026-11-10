//! Unified Volume Manager subsystem (Milestone M8.2).
//!
//! Makes official volumes the primary unit of manga acquisition management:
//! - Complete official volume inventory anchored to MangaBaka & OpenTome
//! - Authoritative Komga ownership vs catalog tracking distinction
//! - Active acquisition and staging state per volume
//! - Provider candidate discovery overview (Madokami & Nyaa)
//! - Review-first batch acquisition plan preview with safe dry-run persistence
//! - Strict edition safeguards (fails closed on ambiguous edition mapping)

use std::collections::{BTreeSet, HashMap};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::service::AcquisitionService;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::pipeline::ownership::{OwnershipStatus, TitleOwnership};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerEntry {
    pub volume_number: f64,
    pub label: String,
    pub title: Option<String>,
    pub cover_url: Option<String>,
    pub release_date: Option<String>,
    pub release_date_precision: Option<String>,
    pub release_region: String,
    pub language: String,
    pub opentome_status: String, // "released", "upcoming", "unannounced", "unknown"
    pub is_owned: bool,
    pub canonical_path: Option<String>,
    pub file_size_bytes: Option<u64>,
    pub file_hash: Option<String>,
    pub publication_receipt_id: Option<String>,
    pub published_at: Option<i64>,
    pub verified_at: Option<i64>,
    pub in_progress: bool,
    pub active_job_id: Option<String>,
    pub active_stage: Option<String>,
    pub candidate_count: usize,
    pub top_provider: Option<String>,
    pub top_candidate_id: Option<String>,
    pub top_candidate_title: Option<String>,
    pub top_candidate_score: Option<f64>,
    pub top_candidate_size: Option<u64>,
    pub last_discovery_at: Option<i64>,
    pub edition: String,
    pub edition_id: Option<String>,
    pub is_authoritative: bool,
    pub uncertainty_reason: Option<String>,
    pub display_status: String, // "owned", "published", "downloading", "candidate_available", "missing_released", "unreleased", "uncertain"
    pub status_label: String,
    #[serde(default)]
    pub komga_book_id: Option<String>,
    #[serde(default)]
    pub komga_book_name: Option<String>,
}

fn default_inherit() -> String {
    "inherit".to_string()
}

fn default_monitor_only() -> String {
    "monitor_only".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerSummary {
    pub manga_id: i64,
    pub series_id: String,
    #[serde(default)]
    pub komga_series_id: Option<String>,
    pub title: String,
    pub mangabaka_id: Option<i64>,
    pub edition: String,
    pub edition_id: Option<String>,
    pub market: String,
    pub total_official_volumes: usize,
    pub owned_count: usize,
    pub published_count: usize,
    pub in_progress_count: usize,
    pub candidate_available_count: usize,
    pub missing_released_count: usize,
    pub unreleased_count: usize,
    pub is_authoritative: bool,
    pub uncertainty_reason: Option<String>,
    #[serde(default = "default_inherit")]
    pub automation_override: String,
    #[serde(default = "default_monitor_only")]
    pub effective_automation_mode: String,
    pub volumes: Vec<VolumeManagerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerBatchPlanRequest {
    pub selected_volumes: Vec<f64>,
    #[allow(dead_code)]
    pub profile_id: Option<String>,
    pub is_dry_run: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchPlanItemDto {
    pub volume_number: f64,
    pub is_owned: bool,
    pub is_eligible: bool,
    pub status: String, // "ELIGIBLE", "ALREADY_OWNED", "IN_FLIGHT", "UNAVAILABLE", "UNRELEASED", "UNCERTAIN_EDITION"
    pub reason: String,
    pub proposed_backend: Option<String>,
    pub provider: Option<String>,
    pub candidate_id: Option<String>,
    pub candidate_title: Option<String>,
    pub candidate_score: Option<f64>,
    pub size_bytes: Option<u64>,
    pub is_digital: Option<bool>,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub recommended_release: Option<String>,
    #[serde(default)]
    pub alternative_releases: Vec<String>,
    #[serde(default)]
    pub publication_destination: Option<String>,
    #[serde(default)]
    pub ranking_breakdown: Option<crate::acquisition::quality::RankingBreakdown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerBatchPlanResponse {
    pub manga_id: i64,
    pub title: String,
    pub edition: String,
    pub edition_id: Option<String>,
    pub total_selected: usize,
    pub eligible_count: usize,
    pub blocked_count: usize,
    pub estimated_total_bytes: u64,
    pub items: Vec<BatchPlanItemDto>,
    pub draft_job_id: Option<String>,
    pub can_execute: bool,
    pub execution_blocked_reason: Option<String>,
    pub is_dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerBatchAcquireRequest {
    pub selected_volumes: Vec<f64>,
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAcquireItemResult {
    pub volume_number: f64,
    pub status: String,
    pub message: String,
    pub execution_id: Option<String>,
    pub candidate_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeManagerBatchAcquireResponse {
    pub manga_id: i64,
    pub queued_count: usize,
    pub simulated_count: usize,
    pub skipped_count: usize,
    pub results: Vec<BatchAcquireItemResult>,
    pub message: String,
}

fn format_volume_label(vol: f64) -> String {
    if (vol - vol.floor()).abs() < 0.001 {
        format!("Volume {:02}", vol as i64)
    } else {
        format!("Volume {:.1}", vol)
    }
}

fn parse_json_f64_list(raw: Option<&str>) -> Vec<f64> {
    raw.and_then(|s| serde_json::from_str::<Vec<f64>>(s).ok())
        .unwrap_or_default()
}

/// Retrieves comprehensive Volume Manager state for a manga title.
pub fn get_volume_manager_summary(
    conn: &Connection,
    manga_id: i64,
) -> Result<VolumeManagerSummary> {
    get_volume_manager_summary_with_ownership(conn, manga_id, None)
}

/// Retrieves comprehensive Volume Manager state with optional real-time Komga ownership.
pub fn get_volume_manager_summary_with_ownership(
    conn: &Connection,
    manga_id: i64,
    ownership_opt: Option<&TitleOwnership>,
) -> Result<VolumeManagerSummary> {
    // 1. Fetch title record
    let manga_row: Option<(String, i64, Option<i64>, Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT title, source_id, mangabaka_id, catalog_manga_id, source_manga_id
             FROM manga
             WHERE id = ?1",
            [manga_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;

    let (title, source_id, manga_mb_id, catalog_manga_id, _source_manga_id) = match manga_row {
        Some(r) => r,
        None => return Err(AppError::not_found(format!("Manga {manga_id} not found"))),
    };

    let effective_catalog_id = if source_id == 0 {
        manga_id
    } else {
        catalog_manga_id.unwrap_or(manga_id)
    };

    // 2. Query series reconciliation state
    let recon_row: Option<(
        String,
        Option<i64>,
        Option<String>,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = conn
        .query_row(
            "SELECT series_id, mangabaka_id, collection_id, edition, status,
                    total_official_volumes, owned_volumes_json, missing_volumes_json,
                    future_volumes_json, review_reason, komga_series_id
             FROM series_reconciliations
             WHERE manga_id = ?1 OR manga_id = ?2 OR (title = ?3 AND ?3 != '')
             ORDER BY (manga_id = ?1) DESC, (manga_id = ?2) DESC, updated_at DESC
             LIMIT 1",
            params![manga_id, effective_catalog_id, title],
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
                ))
            },
        )
        .optional()?;

    let (
        series_id,
        recon_mb_id,
        collection_id,
        edition,
        recon_status,
        total_official_vols,
        owned_vols_json,
        missing_vols_json,
        future_vols_json,
        review_reason,
        recon_komga_id,
    ) = match recon_row {
        Some((sid, mb, col, ed, st, tot, own, mis, fut, rev, kid)) => (
            sid,
            mb,
            col,
            ed,
            st,
            tot.map(|t| t as usize),
            own,
            mis,
            fut,
            rev,
            kid,
        ),
        None => (
            format!("manga:{effective_catalog_id}"),
            manga_mb_id,
            None,
            "standard".to_string(),
            "up_to_date".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
    };

    let komga_series_id = ownership_opt
        .and_then(|o| o.series_id.clone())
        .or(recon_komga_id);

    let mb_series_id = recon_mb_id.or(manga_mb_id);

    let is_authoritative = recon_status != "review_required"
        && recon_status != "collection_unavailable"
        && recon_status != "sync_error"
        && collection_id.is_some();

    let uncertainty_reason = if !is_authoritative {
        review_reason.clone().or_else(|| {
            if collection_id.is_none() {
                Some("MangaBaka collection requires manual review or edition selection".to_string())
            } else {
                Some(format!("Reconciliation status: {recon_status}"))
            }
        })
    } else {
        None
    };

    // Parse volume number lists
    let owned_list = parse_json_f64_list(owned_vols_json.as_deref());
    let missing_list = parse_json_f64_list(missing_vols_json.as_deref());
    let future_list = parse_json_f64_list(future_vols_json.as_deref());

    // Build Komga ownership map if ownership is provided
    let mut komga_ownership_map: HashMap<i64, (Option<String>, Option<String>)> = HashMap::new();
    if let Some(ownership) = ownership_opt {
        for vo in &ownership.volumes {
            if vo.status == OwnershipStatus::Owned {
                let k = (vo.volume * 1000.0).round() as i64;
                komga_ownership_map.insert(k, (vo.book_id.clone(), vo.book_name.clone()));
            }
        }
    }

    // 3. MangaBaka series cache (cover, genres, summary)
    let cover_url: Option<String> = if let Some(mb_id) = mb_series_id {
        conn.query_row(
            "SELECT cover_url FROM mangabaka_series_cache WHERE series_id = ?1",
            [mb_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None)
    } else {
        None
    };

    // 4. Gather all volume numbers
    let mut vol_set = BTreeSet::new();

    if let Some(total) = total_official_vols {
        if total > 0 && total <= 500 {
            for v in 1..=total {
                vol_set.insert((v as f64 * 1000.0).round() as i64);
            }
        }
    }

    for &v in &owned_list {
        vol_set.insert((v * 1000.0).round() as i64);
    }
    for &v in &missing_list {
        vol_set.insert((v * 1000.0).round() as i64);
    }
    for &v in &future_list {
        vol_set.insert((v * 1000.0).round() as i64);
    }
    for &k in komga_ownership_map.keys() {
        vol_set.insert(k);
    }

    // Include any release targets recorded
    let target_vols: Vec<f64> = conn
        .prepare(
            "SELECT volume_number FROM release_discovery_targets
             WHERE manga_id = ?1 OR manga_id = ?2 OR series_id = ?3 OR series_title = ?4",
        )
        .and_then(|mut stmt| {
            let rows = stmt.query_map(
                params![manga_id, effective_catalog_id, series_id, title],
                |r| r.get(0),
            )?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();

    for v in target_vols {
        vol_set.insert((v * 1000.0).round() as i64);
    }

    // Include publication receipts
    let receipt_vols: Vec<f64> = conn
        .prepare(
            "SELECT volume_number FROM publication_receipts
             WHERE (manga_id = ?1 OR manga_id = ?2 OR series_title = ?3) AND volume_number IS NOT NULL",
        )
        .and_then(|mut stmt| {
            let rows = stmt.query_map(params![manga_id, effective_catalog_id, title], |r| r.get(0))?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();

    for v in receipt_vols {
        vol_set.insert((v * 1000.0).round() as i64);
    }

    // If still empty (e.g. non-catalog title without reconciliation), check chapters
    if vol_set.is_empty() {
        let ch_vols: Vec<f64> = conn
            .prepare(
                "SELECT DISTINCT chapter_number FROM chapters
                 WHERE manga_id = ?1 AND chapter_number IS NOT NULL",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map([manga_id], |r| r.get(0))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        for v in ch_vols {
            if v > 0.0 && v <= 200.0 {
                vol_set.insert((v * 1000.0).round() as i64);
            }
        }
    }

    // 5. Build lookup maps for volume details

    // (A) Publication receipts map
    struct ReceiptRow {
        id: String,
        canonical_path: String,
        file_size_bytes: u64,
        file_sha256: String,
        published_at: i64,
        verified_ownership: bool,
        verified_at: Option<i64>,
    }

    let mut receipts_map: HashMap<i64, ReceiptRow> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, volume_number, canonical_path, file_size_bytes, file_sha256,
                published_at, verified_ownership, verified_at
         FROM publication_receipts
         WHERE (manga_id = ?1 OR manga_id = ?2 OR series_title = ?3
                OR manga_id IN (SELECT id FROM manga WHERE catalog_manga_id = ?2))
           AND volume_number IS NOT NULL
         ORDER BY published_at DESC",
    ) {
        if let Ok(rows) = stmt.query_map(
            params![manga_id, effective_catalog_id, title],
            |r| {
                let vol: f64 = r.get(1)?;
                let k = (vol * 1000.0).round() as i64;
                Ok((
                    k,
                    ReceiptRow {
                        id: r.get(0)?,
                        canonical_path: r.get(2)?,
                        file_size_bytes: r.get::<_, i64>(3)? as u64,
                        file_sha256: r.get(4)?,
                        published_at: r.get(5)?,
                        verified_ownership: r.get::<_, i64>(6)? != 0,
                        verified_at: r.get(7)?,
                    },
                ))
            },
        ) {
            for item in rows.flatten() {
                receipts_map.entry(item.0).or_insert(item.1);
            }
        }
    }

    // (B) Active candidate approvals / downloads map
    struct ActiveJobRow {
        job_id: String,
        stage: String,
    }
    let mut active_jobs_map: HashMap<i64, ActiveJobRow> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT ca.id, ca.volume_number, ca.status, d.state, d.external_job_id
         FROM candidate_approvals ca
         LEFT JOIN downloads d ON d.chapter_id = ca.chapter_id
         WHERE (ca.manga_id = ?1 OR ca.manga_id = ?2
                OR ca.manga_id IN (SELECT id FROM manga WHERE catalog_manga_id = ?2))
           AND ca.status IN ('approved', 'queued', 'acquiring', 'importing', 'validating')
         ORDER BY ca.id DESC",
    ) {
        if let Ok(rows) = stmt.query_map(params![manga_id, effective_catalog_id], |r| {
            let ca_id: i64 = r.get(0)?;
            let vol: f64 = r.get(1)?;
            let ca_status: String = r.get(2)?;
            let d_state: Option<String> = r.get(3)?;
            let ext_job: Option<String> = r.get(4)?;
            let k = (vol * 1000.0).round() as i64;
            let stage = d_state.unwrap_or(ca_status);
            let job_id = ext_job.unwrap_or_else(|| format!("approval-{}", ca_id));
            Ok((k, ActiveJobRow { job_id, stage }))
        }) {
            for item in rows.flatten() {
                active_jobs_map.entry(item.0).or_insert(item.1);
            }
        }
    }

    // (C) Volume discovery status map
    struct DiscoveryRow {
        candidate_count: usize,
        top_candidate_id: Option<String>,
        top_candidate_title: Option<String>,
        top_candidate_provider: Option<String>,
        top_candidate_score: Option<f64>,
        last_discovered_at: Option<i64>,
    }
    let mut discovery_map: HashMap<i64, DiscoveryRow> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT volume_number, candidate_count, top_candidate_id, top_candidate_title,
                top_candidate_provider, top_candidate_score, last_discovered_at
         FROM volume_discovery_status
         WHERE series_id = ?1 OR series_id = ?2",
    ) {
        if let Ok(rows) = stmt.query_map(params![series_id, format!("manga:{manga_id}")], |r| {
            let vol: f64 = r.get(0)?;
            let k = (vol * 1000.0).round() as i64;
            Ok((
                k,
                DiscoveryRow {
                    candidate_count: r.get::<_, i64>(1)? as usize,
                    top_candidate_id: r.get(2)?,
                    top_candidate_title: r.get(3)?,
                    top_candidate_provider: r.get(4)?,
                    top_candidate_score: r.get(5)?,
                    last_discovered_at: r.get(6)?,
                },
            ))
        }) {
            for item in rows.flatten() {
                discovery_map.insert(item.0, item.1);
            }
        }
    }

    // (D) Discovered release candidates (for size / score refinement)
    struct CandInfo {
        size_bytes: Option<u64>,
    }
    let mut cand_info_map: HashMap<i64, CandInfo> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT volume_number, size_bytes
         FROM discovered_release_candidates
         WHERE series_id = ?1 OR series_id = ?2
         ORDER BY score DESC",
    ) {
        if let Ok(rows) = stmt.query_map(params![series_id, format!("manga:{manga_id}")], |r| {
            let vol: f64 = r.get(0)?;
            let k = (vol * 1000.0).round() as i64;
            let sz = r.get::<_, Option<i64>>(1)?.map(|s| s as u64);
            Ok((k, CandInfo { size_bytes: sz }))
        }) {
            for item in rows.flatten() {
                cand_info_map.entry(item.0).or_insert(item.1);
            }
        }
    }

    // (E) Release discovery targets map
    #[allow(dead_code)]
    struct TargetRow {
        release_date: Option<String>,
        release_date_precision: Option<String>,
        date_type: Option<String>,
        target_status: Option<String>,
    }
    let mut targets_map: HashMap<i64, TargetRow> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT volume_number, release_date, release_date_precision, date_type, status
         FROM release_discovery_targets
         WHERE manga_id = ?1 OR manga_id = ?2 OR series_id = ?3 OR series_title = ?4",
    ) {
        if let Ok(rows) = stmt.query_map(
            params![manga_id, effective_catalog_id, series_id, title],
            |r| {
                let vol: f64 = r.get(0)?;
                let k = (vol * 1000.0).round() as i64;
                Ok((
                    k,
                    TargetRow {
                        release_date: r.get(1)?,
                        release_date_precision: r.get(2)?,
                        date_type: r.get(3)?,
                        target_status: r.get(4)?,
                    },
                ))
            },
        ) {
            for item in rows.flatten() {
                targets_map.entry(item.0).or_insert(item.1);
            }
        }
    }

    let today_str: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| "1970-01-01".to_string());

    // 6. Build VolumeManagerEntry for each volume
    let mut volumes = Vec::new();
    let mut owned_count = 0;
    let mut published_count = 0;
    let mut in_progress_count = 0;
    let mut candidate_available_count = 0;
    let mut missing_released_count = 0;
    let mut unreleased_count = 0;

    for &k in &vol_set {
        let vol_num = k as f64 / 1000.0;
        let label = format_volume_label(vol_num);

        let receipt = receipts_map.get(&k);
        let active_job = active_jobs_map.get(&k);
        let disc = discovery_map.get(&k);
        let cand_info = cand_info_map.get(&k);
        let target = targets_map.get(&k);

        // Ownership
        let in_owned_list = owned_list.iter().any(|&ov| (ov - vol_num).abs() < 0.001);
        let receipt_verified = receipt.map(|r| r.verified_ownership).unwrap_or(false);
        let komga_entry = komga_ownership_map.get(&k);
        let is_owned = in_owned_list || receipt_verified || komga_entry.is_some();
        let komga_book_id = komga_entry.and_then(|(bid, _)| bid.clone());
        let komga_book_name = komga_entry.and_then(|(_, bname)| bname.clone());

        // In progress
        let in_progress = active_job.is_some();

        // Release dates and OpenTome status
        let release_date = target.and_then(|t| t.release_date.clone());
        let release_date_precision = target.and_then(|t| t.release_date_precision.clone());
        let in_future_list = future_list.iter().any(|&fv| (fv - vol_num).abs() < 0.001);

        let is_future = in_future_list
            || release_date
                .as_ref()
                .map(|d| d.as_str() > today_str.as_str())
                .unwrap_or(false);

        let opentome_status = if is_future {
            "upcoming".to_string()
        } else if release_date.is_some() {
            "released".to_string()
        } else if in_future_list {
            "upcoming".to_string()
        } else {
            "unannounced".to_string()
        };

        // Candidates
        let candidate_count = disc.map(|d| d.candidate_count).unwrap_or(0);
        let top_provider = disc.and_then(|d| d.top_candidate_provider.clone());
        let top_candidate_id = disc.and_then(|d| d.top_candidate_id.clone());
        let top_candidate_title = disc.and_then(|d| d.top_candidate_title.clone());
        let top_candidate_score = disc.and_then(|d| d.top_candidate_score);
        let top_candidate_size = cand_info.and_then(|c| c.size_bytes);
        let last_discovery_at = disc.and_then(|d| d.last_discovered_at);

        // Counter calculations:
        // Candidate availability is an attribute of missing released volumes, not a mutually exclusive state!
        if is_owned {
            owned_count += 1;
        } else {
            if let Some(r) = receipt {
                if !r.verified_ownership {
                    published_count += 1;
                }
            }
            if in_progress {
                in_progress_count += 1;
            }
            if is_future {
                unreleased_count += 1;
            } else {
                // Every released unowned volume is missing released
                missing_released_count += 1;
            }
            if candidate_count > 0 {
                candidate_available_count += 1;
            }
        }

        // Display status & status label
        let (display_status, status_label) = if is_owned {
            ("owned".to_string(), "Owned in Komga".to_string())
        } else if in_progress {
            let st = active_job.map(|j| j.stage.as_str()).unwrap_or("queued");
            ("downloading".to_string(), format!("Acquiring · {st}"))
        } else if let Some(r) = receipt {
            (
                "published".to_string(),
                if r.verified_ownership {
                    "Published · Komga Verified".to_string()
                } else {
                    "Published · Scan Pending".to_string()
                },
            )
        } else if candidate_count > 0 {
            let prov_display = top_provider.as_deref().unwrap_or("Verified");
            (
                "candidate_available".to_string(),
                format!("Missing · {prov_display} candidate available"),
            )
        } else if is_future {
            ("unreleased".to_string(), "Unreleased / Upcoming".to_string())
        } else if !is_authoritative {
            ("uncertain".to_string(), "Uncertain Authority".to_string())
        } else {
            ("missing_released".to_string(), "Released · Missing".to_string())
        };

        volumes.push(VolumeManagerEntry {
            volume_number: vol_num,
            label,
            title: None,
            cover_url: cover_url.clone(),
            release_date,
            release_date_precision,
            release_region: "en".to_string(),
            language: "en".to_string(),
            opentome_status,
            is_owned,
            canonical_path: receipt.map(|r| r.canonical_path.clone()),
            file_size_bytes: receipt.map(|r| r.file_size_bytes),
            file_hash: receipt.map(|r| r.file_sha256.clone()),
            publication_receipt_id: receipt.map(|r| r.id.clone()),
            published_at: receipt.map(|r| r.published_at),
            verified_at: receipt.and_then(|r| r.verified_at),
            in_progress,
            active_job_id: active_job.map(|j| j.job_id.clone()),
            active_stage: active_job.map(|j| j.stage.clone()),
            candidate_count,
            top_provider,
            top_candidate_id,
            top_candidate_title,
            top_candidate_score,
            top_candidate_size,
            last_discovery_at,
            edition: edition.clone(),
            edition_id: collection_id.clone(),
            is_authoritative,
            uncertainty_reason: uncertainty_reason.clone(),
            display_status,
            status_label,
            komga_book_id,
            komga_book_name,
        });
    }

    let total_official_volumes = if let Some(tot) = total_official_vols {
        tot
    } else {
        volumes.len()
    };

    let (auto_override, global_mode) = (
        conn.query_row(
            "SELECT COALESCE(automation_override, 'inherit') FROM manga WHERE id = ?1",
            [manga_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "inherit".to_string()),
        crate::acquisition::monitoring::get_automation_mode(conn),
    );

    let effective_automation_mode = match auto_override.as_str() {
        "paused" => "paused".to_string(),
        "monitor_only" => "monitor_only".to_string(),
        "review_required" => "review_before_acquisition".to_string(),
        "auto_acquire" => "automatic_acquisition".to_string(),
        _ => global_mode.as_str().to_string(),
    };

    Ok(VolumeManagerSummary {
        manga_id,
        series_id,
        komga_series_id,
        title,
        mangabaka_id: mb_series_id,
        edition,
        edition_id: collection_id,
        market: "en".to_string(),
        total_official_volumes,
        owned_count,
        published_count,
        in_progress_count,
        candidate_available_count,
        missing_released_count,
        unreleased_count,
        is_authoritative,
        uncertainty_reason,
        automation_override: auto_override,
        effective_automation_mode,
        volumes,
    })
}

/// Evaluates a review-first batch acquisition plan preview for selected volumes.
/// Safe and dry-run by default: does not enqueue downloads or TorBox jobs.
pub fn plan_volume_manager_batch(
    conn: &Connection,
    manga_id: i64,
    req: &VolumeManagerBatchPlanRequest,
    _profile: &AcquisitionProfile,
    ownership_opt: Option<&TitleOwnership>,
) -> Result<VolumeManagerBatchPlanResponse> {
    let summary = get_volume_manager_summary_with_ownership(conn, manga_id, ownership_opt)?;

    let is_dry_run = req.is_dry_run.unwrap_or(true);
    let mut items = Vec::new();
    let mut eligible_count = 0;
    let mut blocked_count = 0;
    let mut estimated_total_bytes = 0u64;

    for &vol in &req.selected_volumes {
        let entry = summary
            .volumes
            .iter()
            .find(|v| (v.volume_number - vol).abs() < 0.001);

        let (
            is_owned,
            is_eligible,
            status,
            reason,
            proposed_backend,
            provider,
            candidate_id,
            candidate_title,
            candidate_score,
            size_bytes,
            is_digital,
        ) = match entry {
            Some(e) if e.is_owned => (
                true,
                false,
                "ALREADY_OWNED".to_string(),
                "Volume is already owned in Komga library".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            Some(e) if e.in_progress => (
                false,
                false,
                "IN_FLIGHT".to_string(),
                format!(
                    "Acquisition is already active ({})",
                    e.active_stage.as_deref().unwrap_or("in progress")
                ),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            Some(e) if !e.is_authoritative => (
                false,
                false,
                "UNCERTAIN_EDITION".to_string(),
                e.uncertainty_reason
                    .clone()
                    .unwrap_or_else(|| "Authority review required".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            Some(e) if e.opentome_status == "upcoming" => (
                false,
                false,
                "UNRELEASED".to_string(),
                format!(
                    "Volume release date is in the future ({})",
                    e.release_date.as_deref().unwrap_or("upcoming")
                ),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            Some(e) if e.candidate_count == 0 => (
                false,
                false,
                "UNAVAILABLE".to_string(),
                "No release candidates discovered for this volume across providers".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            Some(e) => {
                let prov = e.top_provider.clone();
                let backend = match prov.as_deref() {
                    Some("madokami") => "Madokami (Direct HTTP with Safe Resume)".to_string(),
                    Some("nyaa") => "TorBox (External Torrent Cache)".to_string(),
                    Some(other) => format!("External Provider ({other})"),
                    None => "Acquisition Provider".to_string(),
                };
                let sz = e.top_candidate_size.unwrap_or(80 * 1024 * 1024);
                estimated_total_bytes += sz;
                (
                    false,
                    true,
                    "ELIGIBLE".to_string(),
                    "Verified candidate available for review".to_string(),
                    Some(backend),
                    prov,
                    e.top_candidate_id.clone(),
                    e.top_candidate_title.clone(),
                    e.top_candidate_score,
                    Some(sz),
                    Some(true),
                )
            }
            None => (
                false,
                false,
                "UNAVAILABLE".to_string(),
                format!("Volume {} not found in official volume inventory", vol),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
        };

        if is_eligible {
            eligible_count += 1;
        } else {
            blocked_count += 1;
        }

        let mut confidence = None;
        let mut recommended_release = None;
        let mut alternative_releases = Vec::new();
        let mut publication_destination = None;
        let mut ranking_breakdown = None;

        if is_eligible {
            publication_destination = Some(format!("{}/Volume {:02}.cbz", summary.title, vol.round() as i64));

            if let Ok(mut stmt) = conn.prepare(
                "SELECT title, score, confidence, ranking_breakdown_json
                 FROM discovered_release_candidates
                 WHERE (series_id = ?1 OR series_id = 'manga:' || ?2) AND ABS(volume_number - ?3) < 0.001
                 ORDER BY score DESC",
            ) {
                if let Ok(rows) = stmt.query_map(params![summary.series_id, manga_id, vol], |r| {
                    let t: String = r.get(0)?;
                    let sc: f64 = r.get(1)?;
                    let conf: Option<String> = r.get(2)?;
                    let rb_j: Option<String> = r.get(3)?;
                    Ok((t, sc, conf, rb_j))
                }) {
                    let mut first = true;
                    for row in rows.flatten() {
                        if first {
                            first = false;
                            recommended_release = Some(row.0.clone());
                            confidence = row.2.or_else(|| Some("high".to_string()));
                            if let Some(j) = row.3 {
                                ranking_breakdown = serde_json::from_str(&j).ok();
                            }
                        } else {
                            alternative_releases.push(format!("{} (Score: {:.0})", row.0, row.1));
                        }
                    }
                }
            }
        }

        items.push(BatchPlanItemDto {
            volume_number: vol,
            is_owned,
            is_eligible,
            status,
            reason,
            proposed_backend,
            provider,
            candidate_id,
            candidate_title,
            candidate_score,
            size_bytes,
            is_digital,
            confidence,
            recommended_release,
            alternative_releases,
            publication_destination,
            ranking_breakdown,
        });
    }

    let can_execute = eligible_count > 0 && summary.is_authoritative;
    let execution_blocked_reason = if !summary.is_authoritative {
        Some("MangaBaka collection requires review before batch acquisition can execute".to_string())
    } else if eligible_count == 0 {
        Some("No selected volumes are currently eligible for acquisition".to_string())
    } else {
        None
    };

    let draft_job_id = Some(format!("batch-plan-{}-{}", manga_id, now()));

    Ok(VolumeManagerBatchPlanResponse {
        manga_id,
        title: summary.title,
        edition: summary.edition,
        edition_id: summary.edition_id,
        total_selected: req.selected_volumes.len(),
        eligible_count,
        blocked_count,
        estimated_total_bytes,
        items,
        draft_job_id,
        can_execute,
        execution_blocked_reason,
        is_dry_run,
    })
}

/// Executes one-click batch acquisition for selected volumes.
/// Skips already-owned or in-flight volumes and queues/simulates eligible releases.
pub async fn acquire_volume_manager_batch(
    db: &Db,
    acquisition: &AcquisitionService,
    titles: &crate::titles::Titles,
    manga_id: i64,
    req: VolumeManagerBatchAcquireRequest,
    user_id: Option<i64>,
) -> Result<VolumeManagerBatchAcquireResponse> {
    let title_view = titles.get(manga_id).await?;
    let ownership = if let Some(pipeline) = &acquisition.pipeline {
        Some(pipeline.ownership.get_title_ownership(manga_id, &title_view.title).await)
    } else {
        None
    };
    let profile = acquisition.get_manga_profile(manga_id).await.unwrap_or_default();

    let plan_req = VolumeManagerBatchPlanRequest {
        selected_volumes: req.selected_volumes.clone(),
        profile_id: None,
        is_dry_run: req.dry_run,
    };

    let plan = db
        .call(move |conn| {
            plan_volume_manager_batch(
                conn,
                manga_id,
                &plan_req,
                &profile,
                ownership.as_ref(),
            )
        })
        .await?;

    let mut results = Vec::new();
    let mut queued_count = 0;
    let mut simulated_count = 0;
    let mut skipped_count = 0;

    for item in plan.items {
        if !item.is_eligible || item.candidate_id.is_none() {
            skipped_count += 1;
            results.push(BatchAcquireItemResult {
                volume_number: item.volume_number,
                status: item.status,
                message: item.reason,
                execution_id: None,
                candidate_id: item.candidate_id,
            });
            continue;
        }

        let cand_id = item.candidate_id.as_ref().unwrap();
        let approve_req = crate::acquisition::approval::ApproveCandidateRequest {
            dry_run: req.dry_run,
            authorization_token: None, // one-click invisible authorization!
            user_id,
            confirm_review: Some(true),
            confirm_range: Some(true),
            confirm_pack: Some(true),
        };

        match acquisition
            .approve_candidate(manga_id, item.volume_number, cand_id, approve_req)
            .await
        {
            Ok(resp) => {
                let status = resp.status.clone();
                if status == "queued" {
                    queued_count += 1;
                } else if status == "simulated" || status == "dry_run_preview" {
                    simulated_count += 1;
                } else {
                    skipped_count += 1;
                }
                results.push(BatchAcquireItemResult {
                    volume_number: item.volume_number,
                    status: resp.status,
                    message: resp.message,
                    execution_id: resp.execution_id,
                    candidate_id: item.candidate_id,
                });
            }
            Err(e) => {
                skipped_count += 1;
                results.push(BatchAcquireItemResult {
                    volume_number: item.volume_number,
                    status: "error".to_string(),
                    message: e.to_string(),
                    execution_id: None,
                    candidate_id: item.candidate_id,
                });
            }
        }
    }

    let message = if simulated_count > 0 {
        format!(
            "Simulated batch acquisition for {} volume(s) ({} skipped/ineligible)",
            simulated_count, skipped_count
        )
    } else if queued_count > 0 {
        format!(
            "Queued {} volume(s) for acquisition ({} skipped/ineligible)",
            queued_count, skipped_count
        )
    } else {
        format!("No volumes were queued ({} skipped/ineligible)", skipped_count)
    };

    Ok(VolumeManagerBatchAcquireResponse {
        manga_id,
        queued_count,
        simulated_count,
        skipped_count,
        results,
        message,
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
                title TEXT NOT NULL,
                source_id INTEGER NOT NULL DEFAULT 0,
                mangabaka_id INTEGER NULL,
                catalog_manga_id INTEGER NULL,
                source_manga_id INTEGER NULL
            );
            CREATE TABLE series_reconciliations (
                series_id TEXT PRIMARY KEY,
                manga_id INTEGER NULL,
                komga_series_id TEXT NULL,
                mangabaka_id INTEGER NULL,
                collection_id TEXT NULL,
                title TEXT NOT NULL,
                edition TEXT NOT NULL DEFAULT 'standard',
                status TEXT NOT NULL DEFAULT 'up_to_date',
                is_ongoing INTEGER NOT NULL DEFAULT 1,
                total_official_volumes INTEGER NULL,
                owned_volumes_count INTEGER NOT NULL DEFAULT 0,
                missing_volumes_count INTEGER NOT NULL DEFAULT 0,
                owned_volumes_json TEXT NOT NULL DEFAULT '[]',
                missing_volumes_json TEXT NOT NULL DEFAULT '[]',
                future_volumes_json TEXT NOT NULL DEFAULT '[]',
                last_reconciled_at INTEGER NOT NULL DEFAULT 0,
                last_error TEXT NULL,
                review_reason TEXT NULL,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE mangabaka_series_cache (
                series_id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                english_title TEXT NULL,
                romaji_title TEXT NULL,
                native_title TEXT NULL,
                aliases TEXT NOT NULL DEFAULT '[]',
                cover_url TEXT NULL,
                authors TEXT NOT NULL DEFAULT '[]',
                artists TEXT NOT NULL DEFAULT '[]',
                publisher TEXT NULL,
                genres TEXT NOT NULL DEFAULT '[]',
                tags TEXT NOT NULL DEFAULT '[]',
                summary TEXT NULL,
                total_volumes INTEGER NULL,
                status TEXT NULL,
                web_url TEXT NULL,
                raw_json TEXT NULL,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE release_discovery_targets (
                id INTEGER PRIMARY KEY,
                series_id TEXT NOT NULL,
                manga_id INTEGER NULL,
                series_title TEXT NOT NULL,
                volume_number REAL NOT NULL,
                market TEXT NOT NULL DEFAULT 'en',
                edition TEXT NOT NULL DEFAULT 'standard',
                release_date TEXT NOT NULL,
                release_date_precision TEXT NOT NULL DEFAULT 'day',
                date_type TEXT NOT NULL DEFAULT 'published',
                status TEXT NOT NULL DEFAULT 'scheduled',
                attempts INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 7,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                last_attempt_at INTEGER NULL,
                candidates_count INTEGER NOT NULL DEFAULT 0,
                top_candidate_id TEXT NULL,
                top_candidate_title TEXT NULL,
                last_error TEXT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                auto_decision_code TEXT NULL,
                auto_decision_reason TEXT NULL,
                auto_decision_at INTEGER NULL
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
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE candidate_approvals (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                series_id TEXT NOT NULL,
                volume_number REAL NOT NULL,
                candidate_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                provider_release_id TEXT NOT NULL,
                match_type TEXT NOT NULL,
                approved_at INTEGER NOT NULL,
                execution_id TEXT NULL,
                chapter_id INTEGER NULL,
                status TEXT NOT NULL DEFAULT 'approved'
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
                magnet_url TEXT NULL,
                retrieval_identity_json TEXT NULL
            );
            CREATE TABLE volume_discovery_status (
                series_id TEXT NOT NULL,
                volume_number REAL NOT NULL,
                last_discovered_at INTEGER NOT NULL,
                status TEXT NOT NULL,
                candidate_count INTEGER NOT NULL DEFAULT 0,
                top_candidate_id TEXT NULL,
                top_candidate_title TEXT NULL,
                top_candidate_provider TEXT NULL,
                top_candidate_score REAL NULL,
                providers_checked TEXT NOT NULL,
                provider_errors TEXT NULL,
                PRIMARY KEY (series_id, volume_number)
            );
            CREATE TABLE discovered_release_candidates (
                id INTEGER PRIMARY KEY,
                series_id TEXT NOT NULL,
                volume_number REAL NOT NULL,
                candidate_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                provider_item_id TEXT NOT NULL,
                title TEXT NOT NULL,
                raw_title TEXT NOT NULL,
                match_type TEXT NOT NULL,
                size_bytes INTEGER NULL,
                uploader TEXT NULL,
                is_trusted INTEGER NOT NULL DEFAULT 0,
                is_digital INTEGER NOT NULL DEFAULT 0,
                score REAL NOT NULL,
                score_explanation TEXT NOT NULL,
                download_url TEXT NULL,
                magnet_url TEXT NULL,
                info_hash TEXT NULL,
                seeders INTEGER NULL,
                leechers INTEGER NULL,
                published_at INTEGER NULL,
                discovered_at INTEGER NOT NULL
            );
            CREATE TABLE chapters (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                chapter_number REAL NULL
            );"
        ).unwrap();
        conn
    }

    #[test]
    fn test_volume_manager_haikyu_45_volumes() {
        let conn = create_test_db();

        conn.execute(
            "INSERT INTO manga (id, title, source_id, mangabaka_id) VALUES (1092, 'Haikyu!!', 0, 8718)",
            [],
        ).unwrap();

        let missing_json = serde_json::to_string(
            &(1..=45).map(|v| v as f64).collect::<Vec<f64>>()
        ).unwrap();

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, mangabaka_id, collection_id, title, edition,
                status, is_ongoing, total_official_volumes, owned_volumes_count,
                missing_volumes_count, owned_volumes_json, missing_volumes_json,
                future_volumes_json, last_reconciled_at, updated_at
            ) VALUES (
                'manga:1092', 1092, 8718, '019e1d1c-8b74-79d5-911a-a1997aa899a7',
                'Haikyu!!', 'standard', 'missing_volume', 0, 45, 0, 45, '[]', ?1, '[]', 1000, 1000
            )",
            [&missing_json],
        ).unwrap();

        // Volume 1 candidate discovered
        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider, top_candidate_score, providers_checked
            ) VALUES (
                'manga:1092', 1.0, 1000, 'candidates_found', 3, 'cand-1', 'Haikyu!! v01 [Digital]', 'madokami', 95.0, '[\"madokami\",\"nyaa\"]'
            )",
            [],
        ).unwrap();

        let summary = get_volume_manager_summary(&conn, 1092).unwrap();
        assert_eq!(summary.manga_id, 1092);
        assert_eq!(summary.title, "Haikyu!!");
        assert_eq!(summary.total_official_volumes, 45);
        assert_eq!(summary.volumes.len(), 45);
        assert_eq!(summary.owned_count, 0);
        assert_eq!(summary.candidate_available_count, 1);
        assert_eq!(summary.missing_released_count, 45);
        assert!(summary.is_authoritative);

        let v1 = &summary.volumes[0];
        assert_eq!(v1.volume_number, 1.0);
        assert_eq!(v1.display_status, "candidate_available");
        assert_eq!(v1.top_provider.as_deref(), Some("madokami"));
        assert_eq!(v1.candidate_count, 3);
        assert!(!v1.is_owned);

        let v2 = &summary.volumes[1];
        assert_eq!(v2.volume_number, 2.0);
        assert_eq!(v2.display_status, "missing_released");
        assert!(!v2.is_owned);
    }

    #[test]
    fn test_volume_manager_batch_planning_preview() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, source_id, mangabaka_id) VALUES (1092, 'Haikyu!!', 0, 8718)",
            [],
        ).unwrap();

        let missing_json = serde_json::to_string(
            &(1..=45).map(|v| v as f64).collect::<Vec<f64>>()
        ).unwrap();

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, mangabaka_id, collection_id, title, edition,
                status, is_ongoing, total_official_volumes, owned_volumes_count,
                missing_volumes_count, owned_volumes_json, missing_volumes_json,
                future_volumes_json, last_reconciled_at, updated_at
            ) VALUES (
                'manga:1092', 1092, 8718, '019e1d1c-8b74-79d5-911a-a1997aa899a7',
                'Haikyu!!', 'standard', 'missing_volume', 0, 45, 0, 45, '[]', ?1, '[]', 1000, 1000
            )",
            [&missing_json],
        ).unwrap();

        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider, top_candidate_score, providers_checked
            ) VALUES (
                'manga:1092', 1.0, 1000, 'candidates_found', 1, 'cand-1', 'Haikyu!! v01', 'madokami', 95.0, '[\"madokami\"]'
            )",
            [],
        ).unwrap();

        let req = VolumeManagerBatchPlanRequest {
            selected_volumes: vec![1.0, 2.0],
            profile_id: None,
            is_dry_run: Some(true),
        };

        let profile = AcquisitionProfile::default();
        let plan = plan_volume_manager_batch(&conn, 1092, &req, &profile, None).unwrap();

        assert_eq!(plan.total_selected, 2);
        assert_eq!(plan.eligible_count, 1);
        assert_eq!(plan.blocked_count, 1);
        assert!(plan.can_execute);
        assert!(plan.is_dry_run);

        let item1 = &plan.items[0];
        assert_eq!(item1.volume_number, 1.0);
        assert_eq!(item1.status, "ELIGIBLE");
        assert_eq!(item1.provider.as_deref(), Some("madokami"));

        let item2 = &plan.items[1];
        assert_eq!(item2.volume_number, 2.0);
        assert_eq!(item2.status, "UNAVAILABLE");
    }

    #[test]
    fn test_volume_manager_edition_safeguard_blocks_batch() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, source_id, mangabaka_id) VALUES (700, 'Bungo Stray Dogs', 0, 3251)",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, mangabaka_id, collection_id, title, edition,
                status, is_ongoing, total_official_volumes, owned_volumes_count,
                missing_volumes_count, owned_volumes_json, missing_volumes_json,
                future_volumes_json, last_reconciled_at, review_reason, updated_at
            ) VALUES (
                'manga:700', 700, 3251, NULL,
                'Bungo Stray Dogs', 'standard', 'review_required', 1, 24, 0, 24, '[]', '[]', '[]', 1000,
                'Multiple ambiguous MangaBaka collections found', 1000
            )",
            [],
        ).unwrap();

        let summary = get_volume_manager_summary(&conn, 700).unwrap();
        assert!(!summary.is_authoritative);
        assert!(summary.uncertainty_reason.is_some());

        let req = VolumeManagerBatchPlanRequest {
            selected_volumes: vec![1.0],
            profile_id: None,
            is_dry_run: Some(true),
        };
        let profile = AcquisitionProfile::default();
        let plan = plan_volume_manager_batch(&conn, 700, &req, &profile, None).unwrap();
        assert!(!plan.can_execute);
        assert!(plan.execution_blocked_reason.is_some());
    }

    #[test]
    fn test_volume_manager_komga_ownership_overlay_and_batch_block() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, source_id, mangabaka_id) VALUES (1092, 'Haikyu!!', 0, 8718)",
            [],
        ).unwrap();

        let missing_json = serde_json::to_string(
            &(1..=45).map(|v| v as f64).collect::<Vec<f64>>()
        ).unwrap();

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, mangabaka_id, collection_id, title, edition,
                status, is_ongoing, total_official_volumes, owned_volumes_count,
                missing_volumes_count, owned_volumes_json, missing_volumes_json,
                future_volumes_json, last_reconciled_at, updated_at
            ) VALUES (
                'manga:1092', 1092, 8718, '019e1d1c-8b74-79d5-911a-a1997aa899a7',
                'Haikyu!!', 'standard', 'missing_volume', 0, 45, 0, 45, '[]', ?1, '[]', 1000, 1000
            )",
            [&missing_json],
        ).unwrap();

        // Synthetic candidate for volume 3
        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider, top_candidate_score, providers_checked
            ) VALUES (
                'manga:1092', 3.0, 1000, 'candidates_found', 1, 'cand-3', 'Haikyu!! v03', 'madokami', 95.0, '[\"madokami\"]'
            )",
            [],
        ).unwrap();

        // Synthetic real-time Komga ownership overlay for volumes 1 and 2
        let ownership = TitleOwnership {
            komga_connected: true,
            error: None,
            active_error: None,
            series_id: Some("0S4XY0P4ZM30Y".to_string()),
            series_name: Some("Haikyu!!".to_string()),
            library_id: Some("0RMWEXC3T10TY".to_string()),
            library_name: Some("English Manga".to_string()),
            volumes: vec![
                crate::pipeline::ownership::VolumeOwnership {
                    volume: 1.0,
                    status: OwnershipStatus::Owned,
                    book_id: Some("0S4XY0P4ZM30Z".to_string()),
                    book_name: Some("Haikyu!! v01".to_string()),
                    error: None,
                },
                crate::pipeline::ownership::VolumeOwnership {
                    volume: 2.0,
                    status: OwnershipStatus::Owned,
                    book_id: Some("0S584GXYQMFBZ".to_string()),
                    book_name: Some("Haikyu!! v02".to_string()),
                    error: None,
                },
            ],
            chapters: vec![],
            summary: crate::pipeline::ownership::OwnershipSummary {
                owned_volumes_count: 2,
                total_source_volumes: Some(45),
                owned_chapters_count: 0,
                total_source_chapters: None,
                unknown_books_count: 0,
                source_release_type: "volume".to_string(),
                authoritative_total_volumes: Some(45),
                authoritative_total_chapters: None,
                explanation: None,
            },
            last_refreshed: 1000,
        };

        let summary = get_volume_manager_summary_with_ownership(&conn, 1092, Some(&ownership)).unwrap();
        assert_eq!(summary.owned_count, 2);
        assert_eq!(summary.missing_released_count, 43);
        assert_eq!(summary.candidate_available_count, 1);
        assert_eq!(summary.komga_series_id.as_deref(), Some("0S4XY0P4ZM30Y"));

        let v1 = &summary.volumes[0];
        assert_eq!(v1.volume_number, 1.0);
        assert!(v1.is_owned);
        assert_eq!(v1.display_status, "owned");
        assert_eq!(v1.komga_book_id.as_deref(), Some("0S4XY0P4ZM30Z"));
        assert_eq!(v1.komga_book_name.as_deref(), Some("Haikyu!! v01"));

        let v2 = &summary.volumes[1];
        assert_eq!(v2.volume_number, 2.0);
        assert!(v2.is_owned);
        assert_eq!(v2.display_status, "owned");
        assert_eq!(v2.komga_book_id.as_deref(), Some("0S584GXYQMFBZ"));

        // Batch plan selecting Vol 1 (owned), Vol 2 (owned), and Vol 3 (missing with candidate)
        let req = VolumeManagerBatchPlanRequest {
            selected_volumes: vec![1.0, 2.0, 3.0],
            profile_id: None,
            is_dry_run: Some(true),
        };
        let profile = AcquisitionProfile::default();
        let plan = plan_volume_manager_batch(&conn, 1092, &req, &profile, Some(&ownership)).unwrap();

        assert_eq!(plan.total_selected, 3);
        assert_eq!(plan.eligible_count, 1); // Only Vol 3 is eligible!
        assert_eq!(plan.blocked_count, 2);  // Vol 1 and 2 are blocked because already owned!

        let item1 = &plan.items[0];
        assert_eq!(item1.volume_number, 1.0);
        assert_eq!(item1.status, "ALREADY_OWNED");
        assert!(!item1.is_eligible);
        assert!(item1.is_owned);

        let item2 = &plan.items[1];
        assert_eq!(item2.volume_number, 2.0);
        assert_eq!(item2.status, "ALREADY_OWNED");
        assert!(!item2.is_eligible);
        assert!(item2.is_owned);

        let item3 = &plan.items[2];
        assert_eq!(item3.volume_number, 3.0);
        assert_eq!(item3.status, "ELIGIBLE");
        assert!(item3.is_eligible);
        assert!(!item3.is_owned);
    }
}
