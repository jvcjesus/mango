//! Approved Plan Execution and Verification Pipeline data models and state machine.
//!
//! Enforces critical safety guarantees:
//! - Strict User Authorization: Only explicitly approved plans are executed.
//! - Frozen Immutable Snapshot: Approved candidates, profile constraints, and missing-content states are preserved.
//! - Rigorous Preflight Checks: Verifies disk space, destination directories, ownership reconciliation, and profile validity.
//! - Strict Candidate Revalidation: Re-queries live provider immediately before queueing; NO SILENT SUBSTITUTION allowed.
//! - Idempotent Queueing: Prevents duplicate download jobs across retries, browser actions, or restarts.
//! - Partial Execution Support: Accurately represents partial success and allows retrying failed items only.
//! - Non-destructive Cancellation & Restart Recovery: Preserves valid published media and recovers in-flight state safely.

use crate::acquisition::candidates::{NormalizedAcquisitionCandidate, UnitMappingProvenance};
use crate::acquisition::missing::MissingContentSummary;
use crate::acquisition::planner::AcquisitionPlan;
use crate::acquisition::profiles::AcquisitionProfile;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::result::Result as StdResult;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStatus {
    Approved,
    Preflight,
    Revalidating,
    Queued,
    Downloading,
    Verifying,
    Publishing,
    Reconciling,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Stale,
}

impl ExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::Preflight => "PREFLIGHT",
            Self::Revalidating => "REVALIDATING",
            Self::Queued => "QUEUED",
            Self::Downloading => "DOWNLOADING",
            Self::Verifying => "VERIFYING",
            Self::Publishing => "PUBLISHING",
            Self::Reconciling => "RECONCILING",
            Self::Completed => "COMPLETED",
            Self::Partial => "PARTIAL",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::Stale => "STALE",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "APPROVED" => Some(Self::Approved),
            "PREFLIGHT" => Some(Self::Preflight),
            "REVALIDATING" => Some(Self::Revalidating),
            "QUEUED" => Some(Self::Queued),
            "DOWNLOADING" => Some(Self::Downloading),
            "VERIFYING" => Some(Self::Verifying),
            "PUBLISHING" => Some(Self::Publishing),
            "RECONCILING" => Some(Self::Reconciling),
            "COMPLETED" => Some(Self::Completed),
            "PARTIAL" => Some(Self::Partial),
            "FAILED" => Some(Self::Failed),
            "CANCELLED" => Some(Self::Cancelled),
            "STALE" => Some(Self::Stale),
            _ => None,
        }
    }

    /// Validates legal state transitions. Illegal transitions must fail closed.
    pub fn can_transition_to(&self, next: ExecutionStatus) -> bool {
        match (self, next) {
            // Self-transitions are idempotent
            (a, b) if *a == b => true,

            // Terminal states cannot transition to anything
            (Self::Completed, _) => false,
            (Self::Cancelled, _) => false,
            (Self::Stale, _) => false,

            // Approved can advance through phases or reach terminal/partial outcomes
            (Self::Approved, Self::Preflight | Self::Revalidating | Self::Queued | Self::Completed | Self::Partial | Self::Failed | Self::Cancelled | Self::Stale) => true,

            // Preflight can move to Revalidating, Queued, Fail, Cancel, or become Stale
            (Self::Preflight, Self::Revalidating | Self::Queued | Self::Failed | Self::Cancelled | Self::Stale) => true,

            // Revalidating can move to Queued, Fail, Cancel, or become Stale
            (Self::Revalidating, Self::Queued | Self::Failed | Self::Cancelled | Self::Stale) => true,

            // Queued can advance through execution stages or reach terminal outcomes
            (Self::Queued, Self::Downloading | Self::Verifying | Self::Publishing | Self::Reconciling | Self::Completed | Self::Partial | Self::Failed | Self::Cancelled) => true,

            // Downloading can advance through remaining stages or reach terminal outcomes
            (Self::Downloading, Self::Verifying | Self::Publishing | Self::Reconciling | Self::Completed | Self::Partial | Self::Failed | Self::Cancelled) => true,

            // Verifying can advance or reach terminal outcomes
            (Self::Verifying, Self::Publishing | Self::Reconciling | Self::Completed | Self::Partial | Self::Failed | Self::Cancelled) => true,

            // Publishing can advance or reach terminal outcomes
            (Self::Publishing, Self::Reconciling | Self::Completed | Self::Partial | Self::Failed | Self::Cancelled) => true,

            // Reconciling can reach Completed, Partial, Failed, or Cancelled
            (Self::Reconciling, Self::Completed | Self::Partial | Self::Failed | Self::Cancelled) => true,

            // Partial can retry into Preflight or Revalidating, or be Cancelled
            (Self::Partial, Self::Preflight | Self::Revalidating | Self::Cancelled) => true,

            // Failed can retry into Preflight or Revalidating
            (Self::Failed, Self::Preflight | Self::Revalidating | Self::Cancelled) => true,

            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionItemStatus {
    Pending,
    Preflight,
    Revalidating,
    Queued,
    Downloading,
    Verifying,
    Publishing,
    Reconciling,
    Completed,
    Partial,
    Failed,
    Skipped,
    Cancelled,
}

impl ExecutionItemStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Preflight => "PREFLIGHT",
            Self::Revalidating => "REVALIDATING",
            Self::Queued => "QUEUED",
            Self::Downloading => "DOWNLOADING",
            Self::Verifying => "VERIFYING",
            Self::Publishing => "PUBLISHING",
            Self::Reconciling => "RECONCILING",
            Self::Completed => "COMPLETED",
            Self::Partial => "PARTIAL",
            Self::Failed => "FAILED",
            Self::Skipped => "SKIPPED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "PENDING" => Some(Self::Pending),
            "PREFLIGHT" => Some(Self::Preflight),
            "REVALIDATING" => Some(Self::Revalidating),
            "QUEUED" => Some(Self::Queued),
            "DOWNLOADING" => Some(Self::Downloading),
            "VERIFYING" => Some(Self::Verifying),
            "PUBLISHING" => Some(Self::Publishing),
            "RECONCILING" => Some(Self::Reconciling),
            "COMPLETED" => Some(Self::Completed),
            "PARTIAL" => Some(Self::Partial),
            "FAILED" => Some(Self::Failed),
            "SKIPPED" => Some(Self::Skipped),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionExecutionItem {
    pub id: String,
    pub execution_id: String,
    pub provider: String,
    pub provider_candidate_id: String,
    pub frozen_candidate: NormalizedAcquisitionCandidate,
    pub expected_units: Vec<f64>,
    pub expected_size: u64,
    pub status: ExecutionItemStatus,
    pub download_job_id: Option<i64>,
    pub verification_state: Option<String>,
    pub publication_state: Option<String>,
    pub retry_count: u32,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionExecution {
    pub id: String,
    pub plan_id: String,
    pub manga_id: i64,
    pub profile_id: String,
    pub title: String,
    pub status: ExecutionStatus,
    pub plan_snapshot: AcquisitionPlan,
    pub missing_content_snapshot: MissingContentSummary,
    pub candidates_snapshot: Vec<NormalizedAcquisitionCandidate>,
    pub expected_units: Vec<f64>,
    pub expected_total_bytes: u64,
    pub actual_downloaded_bytes: Option<u64>,
    pub failure_reason: Option<String>,
    pub cancellation_reason: Option<String>,
    pub summary: Option<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub updated_at: i64,
    pub items: Vec<AcquisitionExecutionItem>,
}

/// Request to create an execution from an approved plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovePlanRequest {
    pub plan_id: String,
    #[serde(default = "crate::acquisition::service::default_true")]
    pub approved: bool,
    #[serde(default)]
    pub override_confirmed: bool,
}

// ---------------------------------------------------------------------------
// Preflight & Validation Logic
// ---------------------------------------------------------------------------

/// Validates disk space sufficiency conservatively.
/// Required space = download bytes + workspace (1.5x) + publication (1.0x) + safety buffer (500MB).
pub fn validate_preflight_disk_space(
    required_bytes: u64,
    dest_dir: &Path,
    temp_dir: &Path,
) -> StdResult<(), String> {
    let safety_buffer = 500 * 1024 * 1024; // 500 MB minimum buffer
    let estimated_total_needed = (required_bytes as f64 * 2.5) as u64 + safety_buffer;

    let dest_free = crate::downloads::free_space(dest_dir);
    let temp_free = crate::downloads::free_space(temp_dir);

    match (dest_free, temp_free) {
        (Some(df), Some(tf)) => {
            if df < estimated_total_needed {
                return Err(format!(
                    "Insufficient destination disk space: required ~{} MB, available {} MB",
                    estimated_total_needed >> 20,
                    df >> 20
                ));
            }
            if tf < (required_bytes + safety_buffer) {
                return Err(format!(
                    "Insufficient temporary workspace disk space: required ~{} MB, available {} MB",
                    (required_bytes + safety_buffer) >> 20,
                    tf >> 20
                ));
            }
            Ok(())
        }
        _ => Err("Cannot confidently determine filesystem free space; failing closed".to_string()),
    }
}

/// Validates canonical destination directory and prevents language routing mismatch.
pub fn validate_preflight_destination(
    candidate_lang: &str,
    library_dir: Option<&str>,
    chinese_library_dir: Option<&str>,
) -> StdResult<std::path::PathBuf, String> {
    let is_chinese = candidate_lang.contains("zh") || candidate_lang.contains("chinese") || candidate_lang.contains("mandarin");

    let dest_path_str = if is_chinese {
        chinese_library_dir
            .ok_or_else(|| "Chinese manga library directory is not configured in Mango Pipeline settings".to_string())?
    } else {
        library_dir
            .ok_or_else(|| "Permanent library directory is not configured in Mango Pipeline settings".to_string())?
    };

    let p = Path::new(dest_path_str);
    if !p.exists() {
        return Err(format!("Destination directory does not exist: {}", dest_path_str));
    }
    if !p.is_dir() {
        return Err(format!("Destination path is not a directory: {}", dest_path_str));
    }

    if let Err(e) = crate::pipeline::validation::probe_writable_directory(p) {
        return Err(format!("Destination directory is not writable: {e}"));
    }

    Ok(p.to_path_buf())
}

/// Reconciles live ownership against expected units.
/// Returns (still_needed_units, already_owned_units).
/// Invariant: May REMOVE work, NEVER adds work.
pub fn reconcile_preflight_ownership(
    expected_units: &[f64],
    current_owned: &[f64],
) -> (Vec<f64>, Vec<f64>) {
    let mut still_needed = Vec::new();
    let mut already_owned = Vec::new();

    for &u in expected_units {
        if current_owned.contains(&u) {
            already_owned.push(u);
        } else {
            still_needed.push(u);
        }
    }

    (still_needed, already_owned)
}

/// Validates that live profile still accepts candidate.
pub fn validate_preflight_profile(
    profile: &AcquisitionProfile,
    cand: &NormalizedAcquisitionCandidate,
) -> StdResult<(), String> {
    if let (Some(limit), Some(size)) = (profile.max_acquisition_size_bytes, cand.size) {
        if size > limit {
            return Err("Candidate exceeds the acquisition size limit; identity override does not bypass profile limits".into());
        }
    }
    if let Some(seeders) = cand.health.seeders {
        if seeders < profile.min_seeders {
            return Err(format!(
                "Candidate seeders ({}) below profile minimum ({})",
                seeders, profile.min_seeders
            ));
        }
    }

    let identity = super::edition::RetrievalIdentity::new(
        &cand.series_identity,
        cand.provider_metadata
            .get("rawTitle")
            .and_then(|v| v.as_str())
            .unwrap_or(&cand.title),
        cand.release_unit.clone(),
        cand.provider_metadata
            .get("requestedEdition")
            .and_then(|v| v.as_str()),
    );
    super::torbox::validate_edition_intent(&identity).map_err(|e| e.to_string())?;

    if cand.provider_metadata["requiresIdentityOverride"] == true
        && !super::interactive::is_override(cand)
    {
        return Err(
            "Unverified source identity requires explicit Use Release Anyway approval".into(),
        );
    }

    if cand.mapping_provenance == UnitMappingProvenance::ChapterToVolumeInference {
        return Err(
            "Candidate volume mapping is inferred from chapter sequences; automated volume execution prohibited"
                .to_string(),
        );
    }

    if !profile.allow_unsupported_editions
        && cand.quality_signals.edition_recognized
        && !super::interactive::is_override(cand)
    {
        return Err("Candidate has unrecognized edition variant forbidden by profile".to_string());
    }

    Ok(())
}

/// Strict Candidate Revalidation immediately before queueing.
/// NO SILENT CANDIDATE SUBSTITUTION.
pub fn revalidate_candidate(
    frozen: &NormalizedAcquisitionCandidate,
    live: Option<&NormalizedAcquisitionCandidate>,
    profile: &AcquisitionProfile,
) -> StdResult<(), String> {
    let live_cand = live.ok_or_else(|| {
        format!(
            "Approved candidate '{}' (id: {}) is no longer available from provider. Replanning required.",
            frozen.title, frozen.provider_release_id
        )
    })?;

    if live_cand.provider_release_id != frozen.provider_release_id {
        return Err(format!(
            "Candidate identifier mismatch (approved: {}, live: {}). Replanning required.",
            frozen.provider_release_id, live_cand.provider_release_id
        ));
    }

    if live_cand
        .edition
        .as_deref()
        .map(super::edition::normalize_edition)
        != frozen
            .edition
            .as_deref()
            .map(super::edition::normalize_edition)
    {
        return Err("Candidate edition changed. Replanning required.".into());
    }

    if live_cand.uploader != frozen.uploader {
        return Err(format!(
            "Candidate uploader changed from '{:?}' to '{:?}'. Replanning required.",
            frozen.uploader, live_cand.uploader
        ));
    }

    if live_cand.quality_signals.is_digital != frozen.quality_signals.is_digital {
        return Err("Candidate digital edition signal changed. Replanning required.".to_string());
    }

    if live_cand.language != frozen.language {
        return Err(format!(
            "Candidate language changed from '{}' to '{}'. Replanning required.",
            frozen.language, live_cand.language
        ));
    }

    if let (Some(fz), Some(lz)) = (frozen.size, live_cand.size) {
        let diff = (fz as i64 - lz as i64).abs();
        if diff > (fz as i64 / 2) {
            return Err(format!(
                "Candidate file size changed drastically (approved: {} bytes, live: {} bytes). Replanning required.",
                fz, lz
            ));
        }
    }

    if let Some(s) = live_cand.health.seeders {
        if s < profile.min_seeders {
            return Err(format!(
                "Live seeders ({}) fell below profile minimum ({}). Replanning required.",
                s, profile.min_seeders
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// DB Persistence Functions
// ---------------------------------------------------------------------------

pub fn create_execution(
    conn: &Connection,
    plan: &AcquisitionPlan,
    missing_summary: &MissingContentSummary,
) -> Result<AcquisitionExecution> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM acquisition_executions WHERE plan_id = ?1 ORDER BY created_at LIMIT 1",
            [&plan.id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return get_execution(conn, &id)?
            .ok_or_else(|| AppError::not_found("Existing approved execution unavailable"));
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let execution_id = format!("exec-{}", nanoid());
    let title = plan.series_title.clone();
    let profile_id = plan.profile_id.clone();
    let status = ExecutionStatus::Approved;

    let plan_snapshot_json = serde_json::to_string(plan)?;
    let missing_content_snapshot_json = serde_json::to_string(missing_summary)?;

    let candidates: Vec<NormalizedAcquisitionCandidate> = plan
        .selected_items
        .iter()
        .map(|item| item.candidate.clone())
        .collect();
    let candidates_snapshot_json = serde_json::to_string(&candidates)?;

    let expected_units_json = serde_json::to_string(&plan.missing_units)?;
    let expected_total_bytes = plan.total_estimated_size as i64;

    conn.execute(
        "INSERT INTO acquisition_executions (
            id, plan_id, manga_id, profile_id, title, status,
            plan_snapshot_json, missing_content_snapshot_json, candidates_snapshot_json,
            expected_units_json, expected_total_bytes, summary,
            created_at, approved_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
        )",
        params![
            &execution_id,
            &plan.id,
            plan.manga_id,
            &profile_id,
            &title,
            status.as_str(),
            &plan_snapshot_json,
            &missing_content_snapshot_json,
            &candidates_snapshot_json,
            &expected_units_json,
            expected_total_bytes,
            format!("Approved acquisition of {} missing units", plan.missing_units.len()),
            now,
            now,
            now
        ],
    )?;

    let mut items = Vec::new();
    for original in &plan.selected_items {
        let groups = if original.candidate.provider_metadata["interactive"] == true {
            original
                .units_covered
                .iter()
                .map(|unit| vec![*unit])
                .collect::<Vec<_>>()
        } else {
            vec![original.units_covered.clone()]
        };
        for units in groups {
            let mut plan_item = original.clone();
            plan_item.units_covered = units;
            let item_id = format!("item-{}", nanoid());
            let frozen_json = serde_json::to_string(&plan_item.candidate)?;
            let units_json = serde_json::to_string(&plan_item.units_covered)?;
            let expected_size = plan_item.candidate.size.unwrap_or(0) as i64;
            let item_status = ExecutionItemStatus::Pending;

        conn.execute(
            "INSERT INTO acquisition_execution_items (
                id, execution_id, provider, provider_candidate_id, frozen_candidate_json,
                expected_units_json, expected_size, status, retry_count,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10
            )",
                params![
                    &item_id,
                    &execution_id,
                    &plan_item.candidate.provider,
                    &plan_item.candidate.provider_release_id,
                    &frozen_json,
                    &units_json,
                    expected_size,
                    item_status.as_str(),
                    now,
                    now
                ],
            )?;

            items.push(AcquisitionExecutionItem {
                id: item_id,
                execution_id: execution_id.clone(),
                provider: plan_item.candidate.provider.clone(),
                provider_candidate_id: plan_item.candidate.provider_release_id.clone(),
                frozen_candidate: plan_item.candidate.clone(),
                expected_units: plan_item.units_covered.clone(),
                expected_size: expected_size as u64,
                status: item_status,
                download_job_id: None,
                verification_state: None,
                publication_state: None,
                retry_count: 0,
                error_message: None,
                created_at: now,
                updated_at: now,
            });
        }
    }

    Ok(AcquisitionExecution {
        id: execution_id,
        plan_id: plan.id.clone(),
        manga_id: plan.manga_id,
        profile_id,
        title,
        status,
        plan_snapshot: plan.clone(),
        missing_content_snapshot: missing_summary.clone(),
        candidates_snapshot: candidates,
        expected_units: plan.missing_units.clone(),
        expected_total_bytes: plan.total_estimated_size,
        actual_downloaded_bytes: None,
        failure_reason: None,
        cancellation_reason: None,
        summary: Some(format!("Approved acquisition of {} missing units", plan.missing_units.len())),
        created_at: now,
        approved_at: now,
        started_at: None,
        completed_at: None,
        updated_at: now,
        items,
    })
}

pub fn get_execution(conn: &Connection, execution_id: &str) -> Result<Option<AcquisitionExecution>> {
    let row = conn
        .query_row(
            "SELECT id, plan_id, manga_id, profile_id, title, status,
                    plan_snapshot_json, missing_content_snapshot_json, candidates_snapshot_json,
                    expected_units_json, expected_total_bytes, actual_downloaded_bytes,
                    failure_reason, cancellation_reason, summary,
                    created_at, approved_at, started_at, completed_at, updated_at
             FROM acquisition_executions
             WHERE id = ?1",
            [execution_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, i64>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, Option<String>>(13)?,
                    r.get::<_, Option<String>>(14)?,
                    r.get::<_, i64>(15)?,
                    r.get::<_, i64>(16)?,
                    r.get::<_, Option<i64>>(17)?,
                    r.get::<_, Option<i64>>(18)?,
                    r.get::<_, i64>(19)?,
                ))
            },
        )
        .optional()?;

    let (
        id,
        plan_id,
        manga_id,
        profile_id,
        title,
        status_str,
        plan_json,
        missing_json,
        cands_json,
        units_json,
        expected_bytes,
        actual_bytes,
        failure_reason,
        cancellation_reason,
        summary,
        created_at,
        approved_at,
        started_at,
        completed_at,
        updated_at,
    ) = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let status = ExecutionStatus::from_str(&status_str).unwrap_or(ExecutionStatus::Failed);
    let plan_snapshot: AcquisitionPlan = serde_json::from_str(&plan_json).unwrap_or_else(|_| dummy_plan());
    let missing_content_snapshot: MissingContentSummary = serde_json::from_str(&missing_json).unwrap_or_else(|_| dummy_missing());
    let candidates_snapshot: Vec<NormalizedAcquisitionCandidate> = serde_json::from_str(&cands_json).unwrap_or_default();
    let expected_units: Vec<f64> = serde_json::from_str(&units_json).unwrap_or_default();

    // Load items
    let mut stmt = conn.prepare(
        "SELECT id, execution_id, provider, provider_candidate_id, frozen_candidate_json,
                expected_units_json, expected_size, status, download_job_id,
                verification_state, publication_state, retry_count, error_message,
                created_at, updated_at
         FROM acquisition_execution_items
         WHERE execution_id = ?1
         ORDER BY created_at ASC",
    )?;

    let item_rows = stmt.query_map([execution_id], |r| {
        let f_json: String = r.get(4)?;
        let u_json: String = r.get(5)?;
        let s_str: String = r.get(7)?;
        let frozen_candidate: NormalizedAcquisitionCandidate = serde_json::from_str(&f_json).unwrap_or_else(|_| dummy_cand());
        let units: Vec<f64> = serde_json::from_str(&u_json).unwrap_or_default();
        let item_status = ExecutionItemStatus::from_str(&s_str).unwrap_or(ExecutionItemStatus::Failed);

        Ok(AcquisitionExecutionItem {
            id: r.get(0)?,
            execution_id: r.get(1)?,
            provider: r.get(2)?,
            provider_candidate_id: r.get(3)?,
            frozen_candidate,
            expected_units: units,
            expected_size: r.get::<_, i64>(6)? as u64,
            status: item_status,
            download_job_id: r.get(8)?,
            verification_state: r.get(9)?,
            publication_state: r.get(10)?,
            retry_count: r.get::<_, i64>(11)? as u32,
            error_message: r.get(12)?,
            created_at: r.get(13)?,
            updated_at: r.get(14)?,
        })
    })?;

    let mut items = Vec::new();
    for it in item_rows {
        items.push(it?);
    }

    Ok(Some(AcquisitionExecution {
        id,
        plan_id,
        manga_id,
        profile_id,
        title,
        status,
        plan_snapshot,
        missing_content_snapshot,
        candidates_snapshot,
        expected_units,
        expected_total_bytes: expected_bytes as u64,
        actual_downloaded_bytes: actual_bytes.map(|b| b as u64),
        failure_reason,
        cancellation_reason,
        summary,
        created_at,
        approved_at,
        started_at,
        completed_at,
        updated_at,
        items,
    }))
}

pub fn get_executions_for_manga(conn: &Connection, manga_id: i64) -> Result<Vec<AcquisitionExecution>> {
    let mut stmt = conn.prepare("SELECT id FROM acquisition_executions WHERE manga_id = ?1 ORDER BY created_at DESC")?;
    let ids = stmt
        .query_map([manga_id], |r| r.get::<_, String>(0))?
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();

    let mut list = Vec::new();
    for id in ids {
        if let Some(exec) = get_execution(conn, &id)? {
            list.push(exec);
        }
    }
    Ok(list)
}

pub fn update_execution_status(
    conn: &Connection,
    execution_id: &str,
    new_status: ExecutionStatus,
    failure_reason: Option<&str>,
    cancellation_reason: Option<&str>,
    summary: Option<&str>,
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // Validate state transition
    let current_status_str: String = conn.query_row(
        "SELECT status FROM acquisition_executions WHERE id = ?1",
        [execution_id],
        |r| r.get(0),
    )?;
    let current_status = ExecutionStatus::from_str(&current_status_str)
        .ok_or_else(|| AppError::bad_request("Unknown current execution status"))?;

    if !current_status.can_transition_to(new_status) {
        return Err(AppError::bad_request(format!(
            "Illegal state transition from {} to {}",
            current_status.as_str(),
            new_status.as_str()
        )));
    }

    let started_at_update = if new_status == ExecutionStatus::Preflight || new_status == ExecutionStatus::Queued {
        Some(now)
    } else {
        None
    };

    let completed_at_update = match new_status {
        ExecutionStatus::Completed | ExecutionStatus::Partial | ExecutionStatus::Failed | ExecutionStatus::Cancelled => Some(now),
        _ => None,
    };

    conn.execute(
        "UPDATE acquisition_executions
         SET status = ?2,
             failure_reason = COALESCE(?3, failure_reason),
             cancellation_reason = COALESCE(?4, cancellation_reason),
             summary = COALESCE(?5, summary),
             started_at = COALESCE(started_at, ?6),
             completed_at = COALESCE(?7, completed_at),
             updated_at = ?8
         WHERE id = ?1",
        params![
            execution_id,
            new_status.as_str(),
            failure_reason,
            cancellation_reason,
            summary,
            started_at_update,
            completed_at_update,
            now
        ],
    )?;

    Ok(())
}

pub fn update_item_status(
    conn: &Connection,
    item_id: &str,
    status: ExecutionItemStatus,
    download_job_id: Option<i64>,
    error_message: Option<&str>,
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    conn.execute(
        "UPDATE acquisition_execution_items
         SET status = ?2,
             download_job_id = COALESCE(?3, download_job_id),
             error_message = COALESCE(?4, error_message),
             updated_at = ?5
         WHERE id = ?1",
        params![item_id, status.as_str(), download_job_id, error_message, now],
    )?;

    Ok(())
}

pub fn cancel_execution(conn: &Connection, execution_id: &str, reason: &str) -> Result<()> {
    update_execution_status(
        conn,
        execution_id,
        ExecutionStatus::Cancelled,
        None,
        Some(reason),
        Some("Cancelled by user"),
    )?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    conn.execute(
        "UPDATE acquisition_execution_items
         SET status = 'CANCELLED', updated_at = ?2
         WHERE execution_id = ?1 AND status NOT IN ('COMPLETED', 'SKIPPED')",
        params![execution_id, now],
    )?;

    Ok(())
}

pub fn retry_execution(conn: &Connection, execution_id: &str) -> Result<Vec<String>> {
    let current_status_str: String = conn.query_row(
        "SELECT status FROM acquisition_executions WHERE id = ?1",
        [execution_id],
        |r| r.get(0),
    )?;
    let current_status = ExecutionStatus::from_str(&current_status_str)
        .ok_or_else(|| AppError::bad_request("Unknown execution status"))?;

    if current_status != ExecutionStatus::Failed && current_status != ExecutionStatus::Partial {
        return Err(AppError::bad_request(format!(
            "Only Failed or Partial executions can be retried (current status: {})",
            current_status.as_str()
        )));
    }

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // Reset failed/cancelled items to Pending and increment retry_count
    let mut stmt = conn.prepare(
        "SELECT id FROM acquisition_execution_items
         WHERE execution_id = ?1 AND status IN ('FAILED', 'CANCELLED')",
    )?;
    let item_ids = stmt
        .query_map([execution_id], |r| r.get::<_, String>(0))?
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();

    for id in &item_ids {
        conn.execute(
            "UPDATE acquisition_execution_items
             SET status = 'PENDING', retry_count = retry_count + 1, error_message = NULL, updated_at = ?2
             WHERE id = ?1",
            params![id, now],
        )?;
    }

    update_execution_status(
        conn,
        execution_id,
        ExecutionStatus::Preflight,
        None,
        None,
        Some("Retrying failed execution items"),
    )?;

    Ok(item_ids)
}

fn nanoid() -> String {
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", t)
}

fn dummy_plan() -> AcquisitionPlan {
    AcquisitionPlan {
        id: "empty".into(),
        manga_id: 0,
        series_title: "".into(),
        profile_id: "".into(),
        profile_name: "".into(),
        missing_units: vec![],
        selected_items: vec![],
        alternatives: vec![],
        confidence: "high".into(),
        state: "draft".into(),
        stale_reason: None,
        total_estimated_size: 0,
        total_acquisitions_count: 0,
        missing_units_covered_count: 0,
        is_dry_run: true,
        created_at: 0,
        refreshed_at: 0,
    }
}

fn dummy_missing() -> MissingContentSummary {
    MissingContentSummary {
        manga_id: 0,
        title: "".into(),
        total_known_volumes: 0,
        owned_volumes: vec![],
        missing_volumes: vec![],
        acquiring_volumes: vec![],
        unavailable_volumes: vec![],
        unknown_volumes: vec![],
        volume_entries: vec![],
        total_known_chapters: 0,
        owned_chapters: vec![],
        missing_chapters: vec![],
        acquiring_chapters: vec![],
        is_fully_owned: true,
        has_missing: false,
        explanation: "".into(),
    }
}

fn dummy_cand() -> NormalizedAcquisitionCandidate {
    NormalizedAcquisitionCandidate {
        series_identity: "".into(),
        provider: "".into(),
        provider_release_id: "".into(),
        title: "".into(),
        release_unit: crate::acquisition::AcquisitionReleaseUnit::Unknown,
        release_number: None,
        release_range: None,
        language: "en".into(),
        edition: None,
        format: None,
        uploader: None,
        size: None,
        published_at: None,
        availability: "available".into(),
        health: crate::acquisition::candidates::CandidateHealth {
            seeders: None,
            leechers: None,
            source_healthy: true,
            uploader_reliability_percent: None,
        },
        quality_signals: crate::acquisition::candidates::CandidateQualitySignals {
            is_digital: true,
            is_trusted: true,
            is_preferred_uploader: false,
            edition_recognized: false,
            edition_penalty: 0.0,
        },
        owned_units: vec![],
        missing_units: vec![],
        confidence: "high".into(),
        score: 0.0,
        mapping_provenance: UnitMappingProvenance::ExplicitVolumeMetadata,
        provider_metadata: serde_json::json!({}),
    }
}
