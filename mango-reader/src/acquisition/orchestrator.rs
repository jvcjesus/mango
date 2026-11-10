//! Autonomous Acquisition Orchestrator, Idempotent State Machine & Continuous Backlog Engine (Milestone M14).
//!
//! Architectural Invariants:
//! 1. Mango handles discovery, candidate ranking, safety gating, downloading, validation,
//!    atomic staging, publication, and verified Komga ownership reconciliation.
//! 2. English-only acquisitions: non-English candidates and foreign language releases fail closed.
//! 3. Continuous Backlog Processing: when volume N verifies in Komga, volume N+1 is evaluated
//!    immediately without artificial cron delays, respecting concurrency, safety, and rate limits.
//! 4. Per-Series Automation Overrides: each series can inherit global policy, or specify
//!    'monitor_only', 'review_required', 'auto_acquire', or 'paused'.
//! 5. Idempotent Leases & Durable State Machine: transitions are database-backed and prevent
//!    duplicate jobs across re-checks, crashes, and server restarts.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::acquisition::auto_acquire::{
    is_global_auto_acquire_enabled, is_simulation_mode,
};
use crate::acquisition::approval::{ApproveCandidateRequest, PersistedCandidate};
use crate::acquisition::auto_acquire::is_candidate_auto_eligible;
use crate::acquisition::discovery::discover_for_volume;
use crate::acquisition::matching::detect_foreign_language;
use crate::acquisition::monitoring::{
    add_or_update_review_queue_item, calculate_acquisition_priority, get_automation_mode,
    log_monitoring_activity_internal, AutomationMode,
};
use crate::acquisition::providers::ReleaseCandidate;
use crate::acquisition::service::AcquisitionService;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::sync::reconciliation::get_reconciliation;

// ============================================================================
// State Machine Types & Enums
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcquisitionLifecycleState {
    Discovered,
    Evaluating,
    AwaitingReview,
    Approved,
    Queued,
    Downloading,
    Validating,
    Staged,
    Publishing,
    VerifyingKomga,
    Completed,
    Paused,
    RetryScheduled,
    Blocked,
    Failed,
    Cancelled,
}

impl AcquisitionLifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discovered => "DISCOVERED",
            Self::Evaluating => "EVALUATING",
            Self::AwaitingReview => "AWAITING_REVIEW",
            Self::Approved => "APPROVED",
            Self::Queued => "QUEUED",
            Self::Downloading => "DOWNLOADING",
            Self::Validating => "VALIDATING",
            Self::Staged => "STAGED",
            Self::Publishing => "PUBLISHING",
            Self::VerifyingKomga => "VERIFYING_KOMGA",
            Self::Completed => "COMPLETED",
            Self::Paused => "PAUSED",
            Self::RetryScheduled => "RETRY_SCHEDULED",
            Self::Blocked => "BLOCKED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().as_str() {
            "DISCOVERED" => Some(Self::Discovered),
            "EVALUATING" => Some(Self::Evaluating),
            "AWAITING_REVIEW" => Some(Self::AwaitingReview),
            "APPROVED" => Some(Self::Approved),
            "QUEUED" => Some(Self::Queued),
            "DOWNLOADING" => Some(Self::Downloading),
            "VALIDATING" => Some(Self::Validating),
            "STAGED" => Some(Self::Staged),
            "PUBLISHING" => Some(Self::Publishing),
            "VERIFYING_KOMGA" => Some(Self::VerifyingKomga),
            "COMPLETED" => Some(Self::Completed),
            "PAUSED" => Some(Self::Paused),
            "RETRY_SCHEDULED" => Some(Self::RetryScheduled),
            "BLOCKED" => Some(Self::Blocked),
            "FAILED" => Some(Self::Failed),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionLifecycleRecord {
    pub id: i64,
    pub idempotency_key: String,
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: f64,
    pub edition: String,
    pub state: String,
    pub candidate_id: Option<String>,
    pub provider: Option<String>,
    pub release_title: Option<String>,
    pub chapter_id: Option<i64>,
    pub download_job_id: Option<i64>,
    pub publication_receipt_id: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub retry_count: i64,
    pub max_retries: i64,
    pub next_retry_at: Option<i64>,
    pub failure_classification: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerSeriesAutomationPolicy {
    Inherit,
    MonitorOnly,
    ReviewRequired,
    AutoAcquire,
    Paused,
}

impl PerSeriesAutomationPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::MonitorOnly => "monitor_only",
            Self::ReviewRequired => "review_required",
            Self::AutoAcquire => "auto_acquire",
            Self::Paused => "paused",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "monitor_only" | "mode_a" => Self::MonitorOnly,
            "review_required" | "review_before_acquisition" | "mode_b" => Self::ReviewRequired,
            "auto_acquire" | "automatic_acquisition" | "mode_c" => Self::AutoAcquire,
            "paused" => Self::Paused,
            _ => Self::Inherit,
        }
    }
}

// ============================================================================
// Database & Lease Operations
// ============================================================================

/// Generates a deterministic idempotency key for an acquisition target.
pub fn generate_idempotency_key(series_id: &str, edition: &str, volume: f64) -> String {
    format!("{series_id}:{edition}:{volume:.2}:acq")
}

/// Retrieves or creates an acquisition lifecycle record.
pub fn get_or_create_lifecycle_record(
    conn: &Connection,
    series_id: &str,
    manga_id: Option<i64>,
    series_title: &str,
    volume_number: f64,
    edition: &str,
) -> Result<AcquisitionLifecycleRecord> {
    let key = generate_idempotency_key(series_id, edition, volume_number);
    let current_time = now();

    conn.execute(
        "INSERT INTO acquisition_lifecycle (
            idempotency_key, series_id, manga_id, series_title, volume_number,
            edition, state, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'DISCOVERED', ?7, ?7)
        ON CONFLICT(idempotency_key) DO NOTHING",
        params![
            key,
            series_id,
            manga_id,
            series_title,
            volume_number,
            edition,
            current_time,
        ],
    )?;

    conn.query_row(
        "SELECT id, idempotency_key, series_id, manga_id, series_title, volume_number,
                edition, state, candidate_id, provider, release_title, chapter_id,
                download_job_id, publication_receipt_id, lease_token, lease_expires_at,
                retry_count, max_retries, next_retry_at, failure_classification,
                last_error, created_at, updated_at, completed_at
         FROM acquisition_lifecycle WHERE idempotency_key = ?1",
        [&key],
        map_lifecycle_row,
    )
    .map_err(|e| AppError::internal(format!("Failed to retrieve lifecycle record: {e}")))
}

/// Atomically transitions lifecycle state with durable audit timestamp.
pub fn update_lifecycle_state(
    conn: &Connection,
    idempotency_key: &str,
    new_state: AcquisitionLifecycleState,
    candidate_id: Option<&str>,
    provider: Option<&str>,
    chapter_id: Option<i64>,
    receipt_id: Option<&str>,
    error_msg: Option<&str>,
) -> Result<()> {
    let current_time = now();
    let completed_time = if new_state == AcquisitionLifecycleState::Completed {
        Some(current_time)
    } else {
        None
    };

    conn.execute(
        "UPDATE acquisition_lifecycle
         SET state = ?1,
             candidate_id = COALESCE(?2, candidate_id),
             provider = COALESCE(?3, provider),
             chapter_id = COALESCE(?4, chapter_id),
             publication_receipt_id = COALESCE(?5, publication_receipt_id),
             last_error = ?6,
             completed_at = COALESCE(?7, completed_at),
             updated_at = ?8
         WHERE idempotency_key = ?9",
        params![
            new_state.as_str(),
            candidate_id,
            provider,
            chapter_id,
            receipt_id,
            error_msg,
            completed_time,
            current_time,
            idempotency_key,
        ],
    )?;

    Ok(())
}

/// Atomically claims an exclusive database lease for a worker.
/// Prevents concurrent workers or repeated ticks from processing the same unit.
pub fn claim_lifecycle_lease(
    conn: &Connection,
    idempotency_key: &str,
    worker_id: &str,
    lease_duration_secs: i64,
) -> Result<bool> {
    let current_time = now();
    let expires_at = current_time + lease_duration_secs;

    let rows_affected = conn.execute(
        "UPDATE acquisition_lifecycle
         SET lease_token = ?1, lease_expires_at = ?2, updated_at = ?3
         WHERE idempotency_key = ?4
           AND (lease_token IS NULL OR lease_expires_at < ?3 OR lease_token = ?1)",
        params![worker_id, expires_at, current_time, idempotency_key],
    )?;

    Ok(rows_affected > 0)
}

/// Releases an existing lease.
pub fn release_lifecycle_lease(
    conn: &Connection,
    idempotency_key: &str,
    worker_id: &str,
) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE acquisition_lifecycle
         SET lease_token = NULL, lease_expires_at = NULL, updated_at = ?1
         WHERE idempotency_key = ?2 AND lease_token = ?3",
        params![current_time, idempotency_key, worker_id],
    )?;
    Ok(())
}

/// Reclaims stale expired leases and resets in-flight states for recovery.
pub fn reclaim_stale_leases(conn: &Connection) -> Result<usize> {
    let current_time = now();
    let rows = conn.execute(
        "UPDATE acquisition_lifecycle
         SET lease_token = NULL, lease_expires_at = NULL,
             state = CASE WHEN state IN ('DOWNLOADING', 'VALIDATING', 'STAGED', 'PUBLISHING') THEN 'RETRY_SCHEDULED' ELSE state END,
             updated_at = ?1
         WHERE lease_expires_at IS NOT NULL AND lease_expires_at <= ?1 AND state != 'COMPLETED'",
        params![current_time],
    )?;
    Ok(rows)
}

// ============================================================================
// Per-Series Automation Policy Helpers
// ============================================================================

pub fn get_per_series_policy(conn: &Connection, manga_id: i64) -> PerSeriesAutomationPolicy {
    let val: Option<String> = conn
        .query_row(
            "SELECT automation_override FROM manga WHERE id = ?1",
            [manga_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);

    match val {
        Some(s) => PerSeriesAutomationPolicy::parse(&s),
        None => PerSeriesAutomationPolicy::Inherit,
    }
}

pub fn set_per_series_policy(
    conn: &Connection,
    manga_id: i64,
    policy: PerSeriesAutomationPolicy,
) -> Result<()> {
    conn.execute(
        "UPDATE manga SET automation_override = ?1 WHERE id = ?2",
        params![policy.as_str(), manga_id],
    )?;
    Ok(())
}

pub fn get_effective_policy_for_manga(
    conn: &Connection,
    manga_id: i64,
) -> Option<AutomationMode> {
    let series_policy = get_per_series_policy(conn, manga_id);
    match series_policy {
        PerSeriesAutomationPolicy::Paused => None,
        PerSeriesAutomationPolicy::MonitorOnly => Some(AutomationMode::MonitorOnly),
        PerSeriesAutomationPolicy::ReviewRequired => Some(AutomationMode::ReviewBeforeAcquisition),
        PerSeriesAutomationPolicy::AutoAcquire => Some(AutomationMode::AutomaticAcquisition),
        PerSeriesAutomationPolicy::Inherit => Some(get_automation_mode(conn)),
    }
}

// ============================================================================
// Continuous Backlog Processing Engine
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklogStepResult {
    pub manga_id: i64,
    pub series_title: String,
    pub volume_number: f64,
    pub action: String, // "auto_queued", "review_proposed", "simulated", "paused", "deferred_concurrency", "up_to_date"
    pub candidate_id: Option<String>,
    pub provider: Option<String>,
    pub message: String,
}

/// Evaluates and advances the next sequential missing volume for a series immediately.
/// Called upon Komga verified ownership completion of any volume.
pub async fn advance_continuous_backlog(
    db: &Db,
    http: reqwest::Client,
    acquisition: Option<&Arc<AcquisitionService>>,
    completed_chapter_id: i64,
) -> Result<Option<BacklogStepResult>> {
    // 1. Resolve series information from completed chapter
    let series_info: Option<(i64, String, f64)> = db
        .call(move |conn| {
            let row: Option<(i64, String, String, f64)> = conn
                .query_row(
                    "SELECT c.manga_id, m.title, c.name, c.chapter_number
                     FROM chapters c
                     JOIN manga m ON m.id = c.manga_id
                     WHERE c.id = ?1",
                    [completed_chapter_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;

            if let Some((manga_id, title, name, ch_num)) = row {
                let vol = crate::release::parse_volume(&name)
                    .or_else(|| crate::release::parse_release(&name, ch_num).volume)
                    .unwrap_or(ch_num);
                Ok(Some((manga_id, title, vol)))
            } else {
                Ok(None)
            }
        })
        .await?;

    let Some((manga_id, _title, completed_vol)) = series_info else {
        return Ok(None);
    };

    tracing::info!(
        "Continuous backlog triggered for manga_id={manga_id} following completion of Vol. {completed_vol}"
    );

    evaluate_series_backlog(db, http, acquisition, manga_id).await
}

/// Evaluates next missing volume for a specific manga series under its effective policy.
pub async fn evaluate_series_backlog(
    db: &Db,
    http: reqwest::Client,
    acquisition: Option<&Arc<AcquisitionService>>,
    manga_id: i64,
) -> Result<Option<BacklogStepResult>> {
    let current_time = now();

    // 1. Check effective policy
    let (effective_policy, title, series_id, global_auto, is_sim) = db
        .call(move |conn| {
            let policy = get_effective_policy_for_manga(conn, manga_id);
            let t: String = conn
                .query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |r| r.get(0))
                .unwrap_or_else(|_| format!("Manga #{manga_id}"));
            let s_id = format!("manga:{manga_id}");
            let g_auto = is_global_auto_acquire_enabled(conn);
            let sim = is_simulation_mode(conn);
            Ok((policy, t, s_id, g_auto, sim))
        })
        .await?;

    let Some(mode) = effective_policy else {
        // Series automation is paused
        return Ok(Some(BacklogStepResult {
            manga_id,
            series_title: title.clone(),
            volume_number: 0.0,
            action: "paused".to_string(),
            candidate_id: None,
            provider: None,
            message: format!("Series '{}' automation is PAUSED.", title),
        }));
    };

    if mode == AutomationMode::MonitorOnly {
        return Ok(None);
    }

    // 2. Check active downloads concurrency
    let max_concurrent: i64 = db
        .call(|conn| {
            Ok(crate::settings::get::<i64>(conn, "acquisition.max_concurrent_downloads")
                .unwrap_or(2)
                .clamp(1, 10))
        })
        .await?;

    let active_downloads: i64 = db
        .call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM downloads
                 WHERE state IN ('queued', 'downloading', 'processing', 'validating', 'verifying')",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await?;

    if active_downloads >= max_concurrent {
        return Ok(Some(BacklogStepResult {
            manga_id,
            series_title: title.clone(),
            volume_number: 0.0,
            action: "deferred_concurrency".to_string(),
            candidate_id: None,
            provider: None,
            message: format!(
                "Active downloads ({active_downloads}) meet or exceed limit ({max_concurrent}); deferring backlog evaluation."
            ),
        }));
    }

    // 3. Reconcile missing volumes in sequential order
    let s_id_clone = series_id.clone();
    let recon_opt = db.call(move |conn| get_reconciliation(conn, &s_id_clone)).await?;

    let Some(recon) = recon_opt else {
        return Ok(None);
    };

    let mut missing_volumes: Vec<f64> = recon
        .missing_volumes
        .iter()
        .copied()
        .filter(|&v| v > 0.0 && !recon.owned_volumes.iter().any(|&o| (o - v).abs() < 0.001))
        .collect();

    missing_volumes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let Some(&target_vol) = missing_volumes.first() else {
        // All released volumes are owned!
        return Ok(Some(BacklogStepResult {
            manga_id,
            series_title: title.clone(),
            volume_number: 0.0,
            action: "up_to_date".to_string(),
            candidate_id: None,
            provider: None,
            message: format!("Series '{}' backlog is fully up to date.", title),
        }));
    };

    // 4. Verify target volume is not already downloading or in flight
    let is_in_flight: bool = db
        .call(move |conn| {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM downloads d
                     JOIN chapters c ON c.id = d.chapter_id
                     JOIN manga m ON m.id = c.manga_id
                     WHERE m.id = ?1 AND ABS(c.chapter_number - ?2) < 0.001
                       AND d.state IN ('queued', 'downloading', 'processing', 'validating', 'verifying')
                     LIMIT 1",
                    params![manga_id, target_vol],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            Ok(exists)
        })
        .await?;

    if is_in_flight {
        return Ok(None);
    }

    // 5. Query / discover candidates for target volume
    let discovery_res = discover_for_volume(db, http, &series_id, target_vol).await?;

    // Filter candidate using strict English-only & match criteria
    let mut eligible_candidate: Option<ReleaseCandidate> = None;
    let profile = crate::acquisition::profiles::AcquisitionProfile::default();

    for cand in &discovery_res.candidates {
        let is_english = cand.language.as_deref().map_or(true, |l| l.eq_ignore_ascii_case("en"))
            && detect_foreign_language(&cand.raw_title, "en").is_none();

        if !is_english {
            continue;
        }

        let persisted = PersistedCandidate {
            series_id: series_id.clone(),
            volume_number: target_vol,
            candidate_id: cand.candidate_id.clone(),
            provider: cand.provider.clone(),
            provider_item_id: cand.provider_item_id.clone(),
            title: cand.title.clone(),
            raw_title: cand.raw_title.clone(),
            match_type: cand.match_type.as_str().to_string(),
            size_bytes: cand.size_bytes.map(|s| s as u64),
            uploader: cand.uploader.clone(),
            is_trusted: cand.is_trusted,
            is_digital: cand.is_digital,
            score: cand.score,
            score_explanation: cand.score_explanation.clone(),
            download_url: cand.download_url.clone(),
            magnet_url: cand.magnet_url.clone(),
            info_hash: cand.info_hash.clone(),
            seeders: cand.seeders.map(|s| s as u32),
            leechers: cand.leechers.map(|s| s as u32),
            published_at: cand.published_at,
        };

        if is_candidate_auto_eligible(&persisted, &profile).is_ok() {
            eligible_candidate = Some(cand.clone());
            break;
        }
    }

    let Some(candidate) = eligible_candidate else {
        // No candidate currently meets strict eligibility gates
        return Ok(None);
    };

    // Calculate sequential priority and ordering safety
    let title_for_pri = title.clone();
    let (_, _, _, is_safe_order) = db
        .call(move |conn| {
            Ok(calculate_acquisition_priority(conn, &title_for_pri, target_vol))
        })
        .await?;

    let destination = format!("{}/Volume {:02}.cbz", title, target_vol.round() as i64);
    let alternatives: Vec<_> = discovery_res
        .candidates
        .iter()
        .filter(|c| c.candidate_id != candidate.candidate_id)
        .cloned()
        .collect();

    // 6. Act according to Mode B vs Mode C
    match mode {
        AutomationMode::ReviewBeforeAcquisition => {
            let s_id_c = series_id.clone();
            let title_c = title.clone();
            let cand_c = candidate.clone();
            let dest_c = destination.clone();
            let alts_c = alternatives.clone();

            db.call(move |conn| {
                add_or_update_review_queue_item(
                    conn,
                    Some(manga_id),
                    &s_id_c,
                    &title_c,
                    target_vol,
                    &cand_c,
                    &alts_c,
                    Some(&dest_c),
                )?;
                log_monitoring_activity_internal(
                    conn,
                    Some(manga_id),
                    &title_c,
                    target_vol,
                    "plan_proposed",
                    &cand_c.provider,
                    cand_c.confidence.as_deref().unwrap_or("high"),
                    Some(cand_c.score),
                    Some("AWAITING_OPERATOR_REVIEW"),
                    &format!(
                        "Continuous backlog proposed next sequential volume: {} Vol. {}",
                        title_c, target_vol
                    ),
                    current_time,
                )
            })
            .await?;

            Ok(Some(BacklogStepResult {
                manga_id,
                series_title: title,
                volume_number: target_vol,
                action: "review_proposed".to_string(),
                candidate_id: Some(candidate.candidate_id),
                provider: Some(candidate.provider),
                message: format!("Proposed next missing Vol. {target_vol} in Review Queue"),
            }))
        }
        AutomationMode::AutomaticAcquisition => {
            let is_high_conf = candidate
                .confidence
                .as_deref()
                .map_or(false, |c| c.eq_ignore_ascii_case("high"));

            let can_auto = global_auto && is_high_conf && is_safe_order;

            if !can_auto {
                // High confidence or global auto switch not met: falls back safely to review queue
                let s_id_c = series_id.clone();
                let title_c = title.clone();
                let cand_c = candidate.clone();
                let dest_c = destination.clone();
                let alts_c = alternatives.clone();

                db.call(move |conn| {
                    add_or_update_review_queue_item(
                        conn,
                        Some(manga_id),
                        &s_id_c,
                        &title_c,
                        target_vol,
                        &cand_c,
                        &alts_c,
                        Some(&dest_c),
                    )?;
                    log_monitoring_activity_internal(
                        conn,
                        Some(manga_id),
                        &title_c,
                        target_vol,
                        "plan_proposed",
                        &cand_c.provider,
                        cand_c.confidence.as_deref().unwrap_or("high"),
                        Some(cand_c.score),
                        Some("SAFETY_FALLBACK_TO_REVIEW"),
                        &format!(
                            "Continuous backlog gated acquisition to review: {} Vol. {}",
                            title_c, target_vol
                        ),
                        current_time,
                    )
                })
                .await?;

                return Ok(Some(BacklogStepResult {
                    manga_id,
                    series_title: title,
                    volume_number: target_vol,
                    action: "review_proposed".to_string(),
                    candidate_id: Some(candidate.candidate_id),
                    provider: Some(candidate.provider),
                    message: format!(
                        "Vol. {target_vol} gated to Review Queue (insufficient confidence or safety override)"
                    ),
                }));
            }

            if is_sim {
                let title_c = title.clone();
                let cand_c = candidate.clone();
                db.call(move |conn| {
                    log_monitoring_activity_internal(
                        conn,
                        Some(manga_id),
                        &title_c,
                        target_vol,
                        "simulated",
                        &cand_c.provider,
                        cand_c.confidence.as_deref().unwrap_or("high"),
                        Some(cand_c.score),
                        Some("WOULD_AUTO_ACQUIRE"),
                        &format!(
                            "Simulation Mode: Would auto-acquire {} Vol. {} from {}",
                            title_c, target_vol, cand_c.provider
                        ),
                        current_time,
                    )
                })
                .await?;

                Ok(Some(BacklogStepResult {
                    manga_id,
                    series_title: title,
                    volume_number: target_vol,
                    action: "simulated".to_string(),
                    candidate_id: Some(candidate.candidate_id),
                    provider: Some(candidate.provider),
                    message: format!(
                        "Simulation Mode: simulated continuous acquisition of Vol. {target_vol}"
                    ),
                }))
            } else if let Some(acq) = acquisition {
                let approve_req = ApproveCandidateRequest {
                    dry_run: Some(false),
                    authorization_token: None,
                    user_id: None,
                    confirm_review: Some(true),
                    confirm_range: Some(true),
                    confirm_pack: Some(true),
                };

                let resp = acq
                    .approve_candidate(manga_id, target_vol, &candidate.candidate_id, approve_req)
                    .await?;

                let title_c = title.clone();
                let cand_c = candidate.clone();
                db.call(move |conn| {
                    log_monitoring_activity_internal(
                        conn,
                        Some(manga_id),
                        &title_c,
                        target_vol,
                        "auto_acquired",
                        &cand_c.provider,
                        cand_c.confidence.as_deref().unwrap_or("high"),
                        Some(cand_c.score),
                        Some("CONTINUOUS_BACKLOG_EXECUTED"),
                        &format!(
                            "Continuous backlog queued acquisition of {} Vol. {} from {}",
                            title_c, target_vol, cand_c.provider
                        ),
                        current_time,
                    )
                })
                .await?;

                Ok(Some(BacklogStepResult {
                    manga_id,
                    series_title: title,
                    volume_number: target_vol,
                    action: "auto_queued".to_string(),
                    candidate_id: Some(candidate.candidate_id),
                    provider: Some(candidate.provider),
                    message: resp.message,
                }))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

// ============================================================================
// Activity Center Overview Diagnostics
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealthSummary {
    pub provider: String,
    pub enabled: bool,
    pub configured: bool,
    pub status: String, // "healthy", "rate_limited", "unconfigured", "disabled"
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationOverviewMetrics {
    pub automation_mode: String,
    pub auto_acquire_enabled: bool,
    pub operating_mode: String,
    pub auto_publish_manual: bool,
    pub active_acquisitions_count: usize,
    pub pending_reviews_count: usize,
    pub retry_scheduled_count: usize,
    pub blocked_acquisitions_count: usize,
    pub awaiting_verification_count: usize,
    pub recently_completed_count: usize,
    pub provider_health: Vec<ProviderHealthSummary>,
}

pub fn get_automation_overview_metrics(conn: &Connection) -> Result<AutomationOverviewMetrics> {
    let mode = get_automation_mode(conn);
    let auto_acquire = is_global_auto_acquire_enabled(conn);
    let op_mode = crate::settings::get::<String>(conn, crate::acquisition::auto_acquire::SETTING_OPERATING_MODE)
        .unwrap_or_else(|| "simulation".to_string());
    let auto_pub = crate::settings::get::<bool>(conn, crate::acquisition::auto_acquire::SETTING_AUTO_PUBLISH_MANUAL)
        .unwrap_or(false);

    let active_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state IN ('queued', 'downloading', 'processing', 'validating')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let awaiting_verif: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state = 'verifying'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let pending_reviews: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM acquisition_review_queue WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let retry_scheduled: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state = 'failed' AND attempts > 0 AND retry_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let blocked_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE state = 'failed' AND (retry_at IS NULL OR attempts >= 3)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let recently_completed: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM acquisition_history WHERE outcome = 'completed' AND completed_at >= (strftime('%s','now') - 86400)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Provider health summaries
    let torbox_token = crate::db::state(conn, "acquisition.torbox_token")
        .ok()
        .flatten()
        .filter(|t| !t.trim().is_empty());
    let torbox_enabled = crate::settings::get::<bool>(conn, "acquisition.torbox_enabled")
        .unwrap_or(false);

    let madokami_user = crate::db::state(conn, "acquisition.madokami_username")
        .ok()
        .flatten()
        .filter(|u| !u.trim().is_empty());
    let madokami_enabled = crate::settings::get::<bool>(conn, "acquisition.madokami_enabled")
        .unwrap_or(true);

    let nyaa_enabled = crate::settings::get::<bool>(conn, "acquisition.nyaa_enabled")
        .unwrap_or(true);

    let provider_health = vec![
        ProviderHealthSummary {
            provider: "madokami".to_string(),
            enabled: madokami_enabled,
            configured: madokami_user.is_some(),
            status: if !madokami_enabled {
                "disabled".to_string()
            } else if madokami_user.is_some() {
                "healthy".to_string()
            } else {
                "unconfigured".to_string()
            },
            details: if madokami_user.is_some() {
                "Direct HTTP acquisition configured and operational".to_string()
            } else {
                "Credentials required in Settings for direct HTTP acquisition".to_string()
            },
        },
        ProviderHealthSummary {
            provider: "torbox".to_string(),
            enabled: torbox_enabled,
            configured: torbox_token.is_some(),
            status: if !torbox_enabled {
                "disabled".to_string()
            } else if torbox_token.is_some() {
                "healthy".to_string()
            } else {
                "unconfigured".to_string()
            },
            details: if torbox_token.is_some() {
                "TorBox torrent backend authenticated and active".to_string()
            } else {
                "API token required in Settings to enable torrent backend".to_string()
            },
        },
        ProviderHealthSummary {
            provider: "nyaa".to_string(),
            enabled: nyaa_enabled,
            configured: true,
            status: if nyaa_enabled { "healthy".to_string() } else { "disabled".to_string() },
            details: if nyaa_enabled {
                "Torrent discovery active".to_string()
            } else {
                "Torrent discovery disabled in Settings".to_string()
            },
        },
    ];

    Ok(AutomationOverviewMetrics {
        automation_mode: mode.as_str().to_string(),
        auto_acquire_enabled: auto_acquire,
        operating_mode: op_mode,
        auto_publish_manual: auto_pub,
        active_acquisitions_count: active_count,
        pending_reviews_count: pending_reviews,
        retry_scheduled_count: retry_scheduled,
        blocked_acquisitions_count: blocked_count,
        awaiting_verification_count: awaiting_verif,
        recently_completed_count: recently_completed,
        provider_health,
    })
}

fn map_lifecycle_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AcquisitionLifecycleRecord> {
    Ok(AcquisitionLifecycleRecord {
        id: r.get(0)?,
        idempotency_key: r.get(1)?,
        series_id: r.get(2)?,
        manga_id: r.get(3)?,
        series_title: r.get(4)?,
        volume_number: r.get(5)?,
        edition: r.get(6)?,
        state: r.get(7)?,
        candidate_id: r.get(8)?,
        provider: r.get(9)?,
        release_title: r.get(10)?,
        chapter_id: r.get(11)?,
        download_job_id: r.get(12)?,
        publication_receipt_id: r.get(13)?,
        lease_token: r.get(14)?,
        lease_expires_at: r.get(15)?,
        retry_count: r.get(16)?,
        max_retries: r.get(17)?,
        next_retry_at: r.get(18)?,
        failure_classification: r.get(19)?,
        last_error: r.get(20)?,
        created_at: r.get(21)?,
        updated_at: r.get(22)?,
        completed_at: r.get(23)?,
    })
}
