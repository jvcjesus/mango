//! Title diagnostics and candidate inspection subsystem (Milestone M6.8.3)
//!
//! Provides transparent, read-only inspection into Mango's internal state:
//! - Title identity resolution, provenance, and conflict detection
//! - MangaBaka collection and edition mapping
//! - Komga vs local Mango ownership accounting
//! - OpenTome release intelligence and schedule targets
//! - Guarded auto-acquisition gate evaluation with explicit blocking reasons
//! - Sanitized provider candidate inspection across Nyaa and Madokami

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::auto_acquire::{
    is_auto_acquire_dry_run, is_candidate_auto_eligible, is_global_auto_acquire_enabled,
    AutoDecisionCode,
};
use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::release_discovery::is_volume_owned;
use crate::error::{AppError, Result};
use crate::sync::reconciliation::{
    find_authoritative_tracker_info, gather_local_owned_volumes,
    get_reconciliation_for_manga, ReconciliationStatus,
};

/// Identity resolution and provenance diagnostics for a title.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleIdentityDiagnostics {
    pub manga_id: i64,
    pub title: String,
    pub source_id: i64,
    pub source_manga_id: Option<i64>,
    pub catalog_manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub anilist_id: Option<i64>,
    pub mangabaka_id: Option<i64>,
    pub mangabaka_title: Option<String>,
    pub identity_source: String,
    pub manual_override: bool,
    pub conflicts: Vec<String>,
}

/// MangaBaka collection and edition diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleCollectionDiagnostics {
    pub resolved_collection_id: Option<String>,
    pub market: String,
    pub edition: String,
    pub total_volumes: Option<u32>,
    pub collection_status: String,
    pub reason: Option<String>,
}

/// Detailed physical ownership state diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleOwnershipDiagnostics {
    pub komga_owned_volumes: Vec<f64>,
    pub mango_local_owned_volumes: Vec<f64>,
    pub local_query_skipped: bool,
    pub total_owned_count: usize,
    pub missing_volumes: Vec<f64>,
    pub future_volumes: Vec<f64>,
}

/// Summary of an OpenTome release target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseTargetSummaryDto {
    pub target_id: i64,
    pub volume_number: f64,
    pub release_date: String,
    pub release_date_precision: String,
    pub date_type: String,
    pub status: String,
    pub is_owned: bool,
    pub candidates_count: usize,
    pub top_candidate_title: Option<String>,
}

/// OpenTome release intelligence diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleReleaseIntelligenceDiagnostics {
    pub opentome_tracked: bool,
    pub work_id: Option<String>,
    pub mapping_confidence: Option<String>,
    pub mapping_source: Option<String>,
    pub market: String,
    pub edition: String,
    pub next_release_date: Option<String>,
    pub next_release_volume: Option<f64>,
    pub total_release_targets: usize,
    pub targets: Vec<ReleaseTargetSummaryDto>,
}

/// Reconciliation event state for diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationEventDto {
    pub id: i64,
    pub event_type: String,
    pub message: String,
    pub volume_number: Option<f64>,
    pub severity: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub outbox_status: Option<String>,
    pub outbox_attempts: Option<i64>,
    pub outbox_last_error: Option<String>,
}

/// Series reconciliation state diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleReconciliationDiagnostics {
    pub series_id: String,
    pub status: String,
    pub display_status: String,
    pub total_official_volumes: Option<u32>,
    pub last_reconciled_at: Option<i64>,
    pub last_error: Option<String>,
    pub review_reason: Option<String>,
    pub active_event: Option<ReconciliationEventDto>,
}

/// Evaluation of guarded auto-acquisition gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleAutoAcquireGateEvaluation {
    pub global_enabled: bool,
    pub title_opt_in: bool,
    pub activated_at: Option<i64>,
    pub dry_run: bool,
    pub ownership_gate_passed: bool,
    pub ownership_gate_reason: String,
    pub release_gate_passed: bool,
    pub release_gate_reason: String,
    pub correspondence_gate_passed: bool,
    pub correspondence_gate_reason: String,
    pub final_code: String,
    pub final_reason: String,
}

/// Unified title diagnostics payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleDiagnostics {
    pub identity: TitleIdentityDiagnostics,
    pub collection: TitleCollectionDiagnostics,
    pub ownership: TitleOwnershipDiagnostics,
    pub release_intelligence: TitleReleaseIntelligenceDiagnostics,
    pub reconciliation: TitleReconciliationDiagnostics,
    pub auto_acquire_gates: TitleAutoAcquireGateEvaluation,
}

/// Gathers comprehensive diagnostics for a manga title.
pub fn get_title_diagnostics(conn: &Connection, manga_id: i64) -> Result<TitleDiagnostics> {
    // 1. Fetch title record
    let manga_row = conn
        .query_row(
            "SELECT id, title, source_id, source_manga_id, catalog_manga_id, mangabaka_id,
                    auto_acquire, auto_acquire_activated_at
             FROM manga WHERE id = ?1",
            [manga_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Database error reading manga: {e}")))?
        .ok_or_else(|| AppError::not_found(format!("Manga {manga_id} not found")))?;

    let (
        m_id,
        title,
        source_id,
        source_manga_id,
        catalog_manga_id,
        manga_mb_id,
        title_opt_in,
        activated_at,
    ) = manga_row;

    // 2. Query tracked_catalog
    let tc_row: Option<(Option<String>, i64)> = conn
        .query_row(
            "SELECT komga_series_id, auto_acquire FROM tracked_catalog WHERE manga_id = ?1 LIMIT 1",
            [m_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap_or(None);

    let komga_series_id = tc_row.as_ref().and_then(|r| r.0.clone());

    // 3. Query tracker_mappings
    let mut anilist_id: Option<i64> = None;
    let mut mapping_mb_id: Option<i64> = None;
    let mut mapping_mb_title: Option<String> = None;
    let mut manual_override = false;
    let mut identity_source = "unresolved".to_string();

    if let Some(ref k_id) = komga_series_id {
        let mut stmt = conn
            .prepare(
                "SELECT tracker, tracker_id, tracker_title, manual_override, mapping_source, edition_id
                 FROM tracker_mappings WHERE komga_series_id = ?1",
            )
            .map_err(|e| AppError::internal(format!("Failed to prepare tracker query: {e}")))?;

        let rows = stmt
            .query_map([k_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)? != 0,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|e| AppError::internal(format!("Failed to query tracker mappings: {e}")))?;

        for r in rows.flatten() {
            let (tracker, t_id, t_title, m_over, m_src, _ed) = r;
            if tracker == "anilist" {
                anilist_id = t_id.parse::<i64>().ok();
            } else if tracker == "mangabaka" {
                mapping_mb_id = t_id.parse::<i64>().ok();
                mapping_mb_title = t_title;
                manual_override = m_over;
                identity_source = m_src;
            }
        }
    }

    // Check cached tracker info if mapping was not in DB
    if mapping_mb_id.is_none() {
        let cached = find_authoritative_tracker_info(conn, &title).unwrap_or_default();
        if let Some(mb_id) = cached.mangabaka_id {
            mapping_mb_id = Some(mb_id);
            identity_source = "tracker_cache".to_string();
        }
        if anilist_id.is_none() {
            anilist_id = cached.anilist_id;
        }
    }

    let final_mb_id = manga_mb_id.or(mapping_mb_id);
    if manga_mb_id.is_some() {
        identity_source = "manga_record".to_string();
    }

    // Detect identity conflicts
    let mut conflicts = Vec::new();
    if let (Some(m_id_val), Some(map_id_val)) = (manga_mb_id, mapping_mb_id) {
        if m_id_val != map_id_val {
            conflicts.push(format!(
                "Manga table MangaBaka ID ({m_id_val}) differs from tracker mapping ID ({map_id_val})"
            ));
        }
    }

    // Check if other non-linked titles share the same mangabaka_id
    if let Some(mb_id) = final_mb_id {
        let other_titles: Vec<(i64, String)> = conn
            .prepare(
                "SELECT id, title FROM manga
                 WHERE mangabaka_id = ?1 AND id != ?2
                   AND (catalog_manga_id IS NULL OR catalog_manga_id != ?2)
                   AND (source_manga_id IS NULL OR source_manga_id != ?2)",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map(params![mb_id, m_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        for (other_id, other_name) in other_titles {
            conflicts.push(format!(
                "MangaBaka ID {mb_id} is also assigned to manga '{other_name}' (ID {other_id})"
            ));
        }
    }

    let identity = TitleIdentityDiagnostics {
        manga_id: m_id,
        title: title.clone(),
        source_id,
        source_manga_id,
        catalog_manga_id,
        komga_series_id: komga_series_id.clone(),
        anilist_id,
        mangabaka_id: final_mb_id,
        mangabaka_title: mapping_mb_title,
        identity_source,
        manual_override,
        conflicts,
    };

    // 4. Reconciliation state
    let recon_opt = get_reconciliation_for_manga(conn, m_id).ok().flatten();
    let series_id = recon_opt
        .as_ref()
        .map(|r| r.series_id.clone())
        .unwrap_or_else(|| format!("title:{}", title.to_ascii_lowercase()));

    // Active event from reconciliation_events
    let active_event: Option<ReconciliationEventDto> = conn
        .query_row(
            "SELECT re.id, re.event_type, re.message, re.volume_number, re.created_at, re.resolved_at,
                    no.status, no.attempts, no.last_error
             FROM reconciliation_events re
             LEFT JOIN notification_outbox no ON no.event_id = re.id
             WHERE (re.series_id = ?1 OR re.series_title = ?2) AND re.resolved_at IS NULL
             ORDER BY re.created_at DESC LIMIT 1",
            params![series_id, title],
            |r| {
                let evt_type: String = r.get(1)?;
                let severity = match evt_type.as_str() {
                    "ACTION_REQUIRED" => "action_required",
                    "ACQUISITION_FAILED" => "warning",
                    "CANDIDATE_FOUND" | "AUTO_ACQUIRED" | "VOLUME_RELEASED" => "info",
                    _ => "info",
                }
                .to_string();

                Ok(ReconciliationEventDto {
                    id: r.get(0)?,
                    event_type: evt_type,
                    message: r.get(2)?,
                    volume_number: r.get(3)?,
                    severity,
                    created_at: r.get(4)?,
                    resolved_at: r.get(5)?,
                    outbox_status: r.get(6)?,
                    outbox_attempts: r.get(7)?,
                    outbox_last_error: r.get(8)?,
                })
            },
        )
        .optional()
        .unwrap_or(None);

    let reconciliation = if let Some(ref r) = recon_opt {
        TitleReconciliationDiagnostics {
            series_id: r.series_id.clone(),
            status: r.status.to_string(),
            display_status: r.display_status.clone(),
            total_official_volumes: r.total_official_volumes,
            last_reconciled_at: Some(r.last_reconciled_at),
            last_error: r.last_error.clone(),
            review_reason: r.review_reason.clone(),
            active_event: active_event.clone(),
        }
    } else {
        TitleReconciliationDiagnostics {
            series_id: series_id.clone(),
            status: "Unmatched".to_string(),
            display_status: "Unmatched".to_string(),
            total_official_volumes: None,
            last_reconciled_at: None,
            last_error: None,
            review_reason: None,
            active_event: active_event.clone(),
        }
    };

    // 5. MangaBaka collection diagnostics
    let collection = if let Some(ref r) = recon_opt {
        let coll_status = if r.collection_id.is_some() {
            "resolved".to_string()
        } else if r.status == ReconciliationStatus::ReviewRequired
            || r.status == ReconciliationStatus::CollectionUnavailable
            || r.review_reason.is_some()
        {
            "collection_unavailable".to_string()
        } else {
            "unmatched".to_string()
        };

        TitleCollectionDiagnostics {
            resolved_collection_id: r.collection_id.clone(),
            market: "en".to_string(),
            edition: r.edition.clone(),
            total_volumes: r.total_official_volumes,
            collection_status: coll_status,
            reason: r.review_reason.clone(),
        }
    } else {
        TitleCollectionDiagnostics {
            resolved_collection_id: None,
            market: "en".to_string(),
            edition: "standard".to_string(),
            total_volumes: None,
            collection_status: "unmatched".to_string(),
            reason: Some("Title has not been reconciled against MangaBaka".to_string()),
        }
    };

    // 6. Ownership diagnostics
    let komga_owned_volumes = recon_opt
        .as_ref()
        .map(|r| r.owned_volumes.clone())
        .unwrap_or_default();
    let mango_local_owned_volumes = gather_local_owned_volumes(conn, Some(m_id)).unwrap_or_default();
    let local_query_skipped = false;

    // Deduplicated union of owned volumes
    let mut all_owned = komga_owned_volumes.clone();
    for v in &mango_local_owned_volumes {
        if !all_owned.iter().any(|&o| (o - v).abs() < 0.001) {
            all_owned.push(*v);
        }
    }
    all_owned.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let missing_volumes = recon_opt
        .as_ref()
        .map(|r| r.missing_volumes.clone())
        .unwrap_or_default();
    let future_volumes = recon_opt
        .as_ref()
        .map(|r| r.future_volumes.clone())
        .unwrap_or_default();

    let ownership = TitleOwnershipDiagnostics {
        komga_owned_volumes,
        mango_local_owned_volumes,
        local_query_skipped,
        total_owned_count: all_owned.len(),
        missing_volumes,
        future_volumes,
    };

    // 7. OpenTome Release Intelligence
    let ot_mapping: Option<(String, String, String)> = conn
        .query_row(
            "SELECT work_id, confidence, mapping_source
             FROM opentome_series_mappings WHERE manga_id = ?1",
            [m_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .unwrap_or(None);

    let mut target_rows: Vec<ReleaseTargetSummaryDto> = Vec::new();
    let mut next_release_date: Option<String> = None;
    let mut next_release_volume: Option<f64> = None;

    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, volume_number, release_date, release_date_precision, date_type, status,
                candidates_count, top_candidate_title
         FROM release_discovery_targets
         WHERE manga_id = ?1 OR series_id = ?2
         ORDER BY volume_number ASC",
    ) {
        let rows = stmt.query_map(params![m_id, series_id], |r| {
            let v_num: f64 = r.get(1)?;
            let rel_date: String = r.get(2)?;
            let is_owned = is_volume_owned(conn, m_id, &series_id, v_num).unwrap_or(false);
            Ok(ReleaseTargetSummaryDto {
                target_id: r.get(0)?,
                volume_number: v_num,
                release_date: rel_date,
                release_date_precision: r.get(3)?,
                date_type: r.get(4)?,
                status: r.get(5)?,
                is_owned,
                candidates_count: r.get(6)?,
                top_candidate_title: r.get(7)?,
            })
        });

        if let Ok(mapped) = rows {
            for t in mapped.flatten() {
                if !t.is_owned && next_release_date.is_none() {
                    next_release_date = Some(t.release_date.clone());
                    next_release_volume = Some(t.volume_number);
                }
                target_rows.push(t);
            }
        }
    }

    let release_intelligence = TitleReleaseIntelligenceDiagnostics {
        opentome_tracked: ot_mapping.is_some(),
        work_id: ot_mapping.as_ref().map(|m| m.0.clone()),
        mapping_confidence: ot_mapping.as_ref().map(|m| m.1.clone()),
        mapping_source: ot_mapping.as_ref().map(|m| m.2.clone()),
        market: "en".to_string(),
        edition: collection.edition.clone(),
        next_release_date,
        next_release_volume,
        total_release_targets: target_rows.len(),
        targets: target_rows,
    };

    // 8. Auto-Acquire Gate Evaluation
    let global_enabled = is_global_auto_acquire_enabled(conn);
    let dry_run = is_auto_acquire_dry_run(conn);

    let (ownership_gate_passed, ownership_gate_reason) = if ownership.missing_volumes.is_empty() {
        (
            false,
            "All official volumes are already owned in Komga/Mango".to_string(),
        )
    } else {
        (
            true,
            format!("{} missing volume(s) eligible for acquisition", ownership.missing_volumes.len()),
        )
    };

    let today_str: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| "1970-01-01".to_string());

    let (release_gate_passed, release_gate_reason) = if let Some(ref date) = release_intelligence.next_release_date {
        if date <= &today_str {
            (true, format!("Target volume release date {date} is published (local today: {today_str})"))
        } else {
            (false, format!("Target volume release date {date} is in the future (local today: {today_str})"))
        }
    } else if !ownership.missing_volumes.is_empty() {
        (true, "Backlog volume without pending future release schedule".to_string())
    } else {
        (false, "No unowned release scheduled".to_string())
    };

    let (corr_passed, corr_reason) = if collection.collection_status == "resolved" {
        (true, format!("MangaBaka collection {} is verified", collection.resolved_collection_id.as_deref().unwrap_or("")))
    } else {
        (false, collection.reason.clone().unwrap_or_else(|| "Collection unverified".to_string()))
    };

    let (final_code, final_reason) = if !global_enabled {
        (
            "GLOBAL_AUTO_DISABLED".to_string(),
            "Global automatic acquisition master switch is OFF in system settings".to_string(),
        )
    } else if !title_opt_in {
        (
            "TITLE_AUTO_DISABLED".to_string(),
            "Title has not opted into automatic acquisition (disabled by default)".to_string(),
        )
    } else if !corr_passed {
        (
            "MAPPING_REVIEW_REQUIRED".to_string(),
            format!("MangaBaka collection requires review: {corr_reason}"),
        )
    } else if !ownership_gate_passed {
        (
            "OWNED".to_string(),
            "All known volumes are verified owned; no acquisition needed".to_string(),
        )
    } else if !release_gate_passed {
        (
            "NOT_RELEASED".to_string(),
            release_gate_reason.clone(),
        )
    } else if dry_run {
        (
            "WOULD_AUTO_ACQUIRE".to_string(),
            "Gates passed; dry-run mode is ON so job will be logged without download execution".to_string(),
        )
    } else {
        (
            "AUTO_ALLOWED".to_string(),
            "All policy gates passed and automatic acquisition is authorized".to_string(),
        )
    };

    let auto_acquire_gates = TitleAutoAcquireGateEvaluation {
        global_enabled,
        title_opt_in,
        activated_at,
        dry_run,
        ownership_gate_passed,
        ownership_gate_reason,
        release_gate_passed,
        release_gate_reason,
        correspondence_gate_passed: corr_passed,
        correspondence_gate_reason: corr_reason,
        final_code,
        final_reason,
    };

    Ok(TitleDiagnostics {
        identity,
        collection,
        ownership,
        release_intelligence,
        reconciliation,
        auto_acquire_gates,
    })
}

/// Sanitized candidate inspection record for read-only inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateInspectionDto {
    pub candidate_id: String,
    pub provider: String,
    pub transport: String,
    pub title: String,
    pub raw_title: String,
    pub match_type: String,
    pub is_pre: bool,
    pub size_bytes: Option<u64>,
    pub uploader: Option<String>,
    pub is_trusted: bool,
    pub is_digital: bool,
    pub score: f64,
    pub score_explanation: String,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub published_at: Option<i64>,
    pub auto_acquire_eligible: bool,
    pub rejection_code: Option<String>,
    pub rejection_reason: Option<String>,
    pub sanitized_reference: Option<String>,
    pub quality_evaluation: Option<crate::acquisition::quality::CandidateQualityEvaluation>,
}

/// Result of volume candidate inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeCandidateInspectionResult {
    pub series_id: String,
    pub volume_number: f64,
    pub target_status: Option<String>,
    pub target_release_date: Option<String>,
    pub is_currently_owned: bool,
    pub silent_upgrade_blocked: bool,
    pub candidates_count: usize,
    pub top_candidate: Option<CandidateInspectionDto>,
    pub candidates: Vec<CandidateInspectionDto>,
}

/// Sanitizes a URL or download reference to prevent credential leakage.
fn sanitize_reference(provider: &str, download_url: Option<&str>, magnet_url: Option<&str>, info_hash: Option<&str>) -> Option<String> {
    if provider == "madokami" {
        download_url.map(|u| {
            // Remove basic auth (user:pass@) if present in URL
            if let Ok(mut parsed) = reqwest::Url::parse(u) {
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
                parsed.to_string()
            } else {
                "https://manga.madokami.al/...".to_string()
            }
        })
    } else if provider == "nyaa" {
        info_hash.map(|h| format!("urn:btih:{h}"))
            .or_else(|| magnet_url.map(|m| {
                if let Some(idx) = m.find("&") {
                    m[..idx].to_string() // Keep only base urn:btih without potential passkeys
                } else {
                    m.to_string()
                }
            }))
    } else {
        None
    }
}

/// Inspects discovered candidates for a logical target volume safely.
pub fn inspect_volume_candidates(
    conn: &Connection,
    series_id: &str,
    volume_number: f64,
    profile: &AcquisitionProfile,
) -> Result<VolumeCandidateInspectionResult> {
    // 1. Fetch target info if recorded
    let target_row: Option<(String, String)> = conn
        .query_row(
            "SELECT status, release_date FROM release_discovery_targets
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
            params![series_id, volume_number],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap_or(None);

    let (target_status, target_release_date) = match target_row {
        Some((s, d)) => (Some(s), Some(d)),
        None => (None, None),
    };

    let today_str: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| "1970-01-01".to_string());

    // 2. Check ownership status (Komga / local reconciliation)
    let is_currently_owned: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM series_reconciliations
                WHERE series_id = ?1 AND owned_volumes_json LIKE '%' || ?2 || '%'
            )",
            params![series_id, volume_number],
            |r| r.get(0),
        )
        .unwrap_or(false)
        || is_volume_owned(conn, 0, series_id, volume_number).unwrap_or(false);

    // 3. Query discovered candidates
    let mut stmt = conn
        .prepare(
            "SELECT candidate_id, provider, provider_item_id, title, raw_title, match_type,
                    size_bytes, uploader, is_trusted, is_digital, score, score_explanation,
                    download_url, magnet_url, info_hash, seeders, leechers, published_at
             FROM discovered_release_candidates
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001
             ORDER BY score DESC",
        )
        .map_err(|e| AppError::internal(format!("Failed to prepare candidates query: {e}")))?;

    let rows = stmt
        .query_map(params![series_id, volume_number], |r| {
            Ok(PersistedCandidate {
                series_id: series_id.to_string(),
                volume_number,
                candidate_id: r.get(0)?,
                provider: r.get(1)?,
                provider_item_id: r.get(2)?,
                title: r.get(3)?,
                raw_title: r.get(4)?,
                match_type: r.get(5)?,
                size_bytes: r.get(6)?,
                uploader: r.get(7)?,
                is_trusted: r.get::<_, i64>(8)? != 0,
                is_digital: r.get::<_, i64>(9)? != 0,
                score: r.get(10)?,
                score_explanation: r.get(11)?,
                download_url: r.get(12)?,
                magnet_url: r.get(13)?,
                info_hash: r.get(14)?,
                seeders: r.get(15)?,
                leechers: r.get(16)?,
                published_at: r.get(17)?,
            })
        })
        .map_err(|e| AppError::internal(format!("Failed to query candidates: {e}")))?;

    let mut inspected_list = Vec::new();
    let mut quality_evals = Vec::new();

    for cand_res in rows {
        let cand = cand_res.map_err(|e| AppError::internal(format!("Row error: {e}")))?;

        let transport = match cand.provider.as_str() {
            "madokami" => "Direct HTTP (Range resume)".to_string(),
            "nyaa" => "TorBox BitTorrent (Debrid)".to_string(),
            other => format!("Provider: {other}"),
        };

        // PRE check: if target release date is in the future
        let is_pre = if let Some(ref rel_date) = target_release_date {
            rel_date > &today_str
        } else {
            false
        };

        // Evaluate eligibility via shared auto-acquire gate logic
        let (auto_acquire_eligible, rejection_code, rejection_reason) =
            match is_candidate_auto_eligible(&cand, profile) {
                Ok(()) => (true, None, None),
                Err(code) => {
                    let reason = match code {
                        AutoDecisionCode::ProviderUnavailable => "Provider is not enabled for automation",
                        AutoDecisionCode::RangeRequiresManual => "Multi-volume range candidate requires manual confirmation",
                        AutoDecisionCode::PackRequiresManual => "Series pack candidate requires manual confirmation",
                        AutoDecisionCode::PossibleMatchRequiresManual => "Ambiguous title candidate requires manual confirmation",
                        AutoDecisionCode::CandidateNotExact => "Candidate match type is not ExactVolume",
                        AutoDecisionCode::AmbiguousTitleTokens => "Title contains batch/omnibus/complete keywords",
                        AutoDecisionCode::LanguageUnknown => "Language could not be verified as English",
                        AutoDecisionCode::FormatNotAllowed => "Format/file type is not permitted",
                        AutoDecisionCode::SeedersBelowPolicy => "Torrent seeders below policy minimum",
                        AutoDecisionCode::SizeOutOfBounds => "Archive size is outside permitted byte boundaries",
                        AutoDecisionCode::ScoreBelowThreshold => "Quality score is below minimum threshold (50.0)",
                        _ => "Candidate failed hard policy eligibility gate",
                    };
                    (false, Some(code.as_str().to_string()), Some(reason.to_string()))
                }
            };

        let quality_eval = crate::acquisition::quality::evaluate_candidate_quality(
            &cand,
            is_currently_owned,
            rejection_reason.clone(),
        );
        quality_evals.push(quality_eval);

        let sanitized_reference = sanitize_reference(
            &cand.provider,
            cand.download_url.as_deref(),
            cand.magnet_url.as_deref(),
            cand.info_hash.as_deref(),
        );

        inspected_list.push(CandidateInspectionDto {
            candidate_id: cand.candidate_id,
            provider: cand.provider,
            transport,
            title: cand.title,
            raw_title: cand.raw_title,
            match_type: cand.match_type,
            is_pre,
            size_bytes: cand.size_bytes,
            uploader: cand.uploader,
            is_trusted: cand.is_trusted,
            is_digital: cand.is_digital,
            score: cand.score,
            score_explanation: cand.score_explanation,
            seeders: cand.seeders,
            leechers: cand.leechers,
            published_at: cand.published_at,
            auto_acquire_eligible,
            rejection_code,
            rejection_reason,
            sanitized_reference,
            quality_evaluation: None,
        });
    }

    // Rank quality evaluations and re-attach
    crate::acquisition::quality::rank_quality_evaluations(&mut quality_evals);
    for (i, eval) in quality_evals.into_iter().enumerate() {
        if let Some(dto) = inspected_list.get_mut(i) {
            dto.quality_evaluation = Some(eval);
        }
    }

    // Sort inspected candidates by quality rank
    inspected_list.sort_by_key(|c| c.quality_evaluation.as_ref().map_or(999, |q| q.rank));

    let top_candidate = inspected_list.first().cloned();

    Ok(VolumeCandidateInspectionResult {
        series_id: series_id.to_string(),
        volume_number,
        target_status,
        target_release_date,
        is_currently_owned,
        silent_upgrade_blocked: is_currently_owned,
        candidates_count: inspected_list.len(),
        top_candidate,
        candidates: inspected_list,
    })
}
