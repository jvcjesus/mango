//! Followed-Series Monitoring, Review Queue & Automation Policy Engine (Milestones M10.1 + M11).
//!
//! Invariants:
//! - Default Automation Mode is Mode A (Monitor Only).
//! - Automatic Acquisition (Mode C) requires:
//!   1. Global Master Switch (`acquisition.auto_acquire_enabled`) == true
//!   2. Title Opt-In (`manga.auto_acquire`) == true
//!   3. High Match Confidence (Medium, Low, Ambiguous strictly fail closed)
//!   4. English-only language verification
//!   5. All hard eligibility gates satisfied (`is_candidate_auto_eligible`)
//!   6. Non-mutating in Simulation Mode (records `WOULD_AUTO_ACQUIRE`)
//! - Review Queue (Mode B) allows operator batch-approval without automated queueing.
//! - Followed series monitoring synchronizes AniList tracked lists filtered by configured tracking statuses.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::auto_acquire::{
    is_global_auto_acquire_enabled, is_simulation_mode, set_auto_publish_manual_enabled,
    set_global_auto_acquire, set_operating_mode, SETTING_AUTO_PUBLISH_MANUAL,
    SETTING_OPERATING_MODE,
};
use crate::acquisition::approval::ApproveCandidateRequest;
use crate::acquisition::providers::ReleaseCandidate;
use crate::acquisition::service::AcquisitionService;
use crate::acquisition::volume_manager::BatchAcquireItemResult;
use crate::acquisition::watcher::sync_watchers_for_series;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::sync::reconciliation::get_reconciliation;

pub const SETTING_AUTOMATION_MODE: &str = "acquisition.automation_mode";
pub const SETTING_MONITORING_INTERVAL_HOURS: &str = "monitoring.interval_hours";
pub const SETTING_MONITORED_TRACKING_STATUSES: &str = "monitoring.tracked_statuses";
pub const SETTING_LAST_CYCLE_AT: &str = "monitoring.last_cycle_at";
pub const SETTING_RECOMMENDATION_SCOPE: &str = "acquisition.recommendation_scope";

/// Automation operating modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    /// Mode A: Monitor Only (Default, strictly non-mutating)
    MonitorOnly,
    /// Mode B: Review Before Acquisition (Populates review queue for approval)
    ReviewBeforeAcquisition,
    /// Mode C: Automatic Acquisition (Guarded, opt-in auto-queueing)
    AutomaticAcquisition,
}

impl AutomationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MonitorOnly => "monitor_only",
            Self::ReviewBeforeAcquisition => "review_before_acquisition",
            Self::AutomaticAcquisition => "automatic_acquisition",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "monitor_only" | "monitor" | "mode_a" => Some(Self::MonitorOnly),
            "review_before_acquisition" | "review" | "mode_b" => Some(Self::ReviewBeforeAcquisition),
            "automatic_acquisition" | "automatic" | "auto" | "mode_c" => Some(Self::AutomaticAcquisition),
            _ => None,
        }
    }

    pub fn from_str(s: &str) -> Self {
        Self::parse(s).unwrap_or(Self::MonitorOnly)
    }
}

impl Default for AutomationMode {
    fn default() -> Self {
        Self::MonitorOnly
    }
}

fn default_recommendation_scope() -> String {
    "mango_library_only".to_string()
}

/// System-wide monitoring and automation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringSettings {
    pub automation_mode: AutomationMode,
    pub interval_hours: i64,
    pub tracked_statuses: Vec<String>,
    pub last_cycle_at: Option<i64>,
    pub auto_acquire_enabled: bool,
    pub operating_mode: String,
    pub auto_publish_manual: bool,
    #[serde(default = "default_recommendation_scope")]
    pub recommendation_scope: String,
}

/// Partial update payload for monitoring and automation settings.
/// Every field is optional to allow selective mutation without overwriting
/// unrelated safety flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMonitoringSettingsRequest {
    #[serde(default, alias = "automation_mode")]
    pub automation_mode: Option<String>,
    #[serde(default, alias = "interval_hours")]
    pub interval_hours: Option<i64>,
    #[serde(default, alias = "tracked_statuses")]
    pub tracked_statuses: Option<Vec<String>>,
    #[serde(default, alias = "auto_acquire_enabled")]
    pub auto_acquire_enabled: Option<bool>,
    #[serde(default, alias = "operating_mode")]
    pub operating_mode: Option<String>,
    #[serde(default, alias = "auto_publish_manual")]
    pub auto_publish_manual: Option<bool>,
    #[serde(default, alias = "recommendation_scope")]
    pub recommendation_scope: Option<String>,
}

impl From<MonitoringSettings> for UpdateMonitoringSettingsRequest {
    fn from(s: MonitoringSettings) -> Self {
        Self {
            automation_mode: Some(s.automation_mode.as_str().to_string()),
            interval_hours: Some(s.interval_hours),
            tracked_statuses: Some(s.tracked_statuses),
            auto_acquire_enabled: Some(s.auto_acquire_enabled),
            operating_mode: Some(s.operating_mode),
            auto_publish_manual: Some(s.auto_publish_manual),
            recommendation_scope: Some(s.recommendation_scope),
        }
    }
}

impl From<&MonitoringSettings> for UpdateMonitoringSettingsRequest {
    fn from(s: &MonitoringSettings) -> Self {
        Self {
            automation_mode: Some(s.automation_mode.as_str().to_string()),
            interval_hours: Some(s.interval_hours),
            tracked_statuses: Some(s.tracked_statuses.clone()),
            auto_acquire_enabled: Some(s.auto_acquire_enabled),
            operating_mode: Some(s.operating_mode.clone()),
            auto_publish_manual: Some(s.auto_publish_manual),
            recommendation_scope: Some(s.recommendation_scope.clone()),
        }
    }
}

/// Proposed acquisition in the Review Queue (Mode B)
fn default_priority() -> i64 {
    1
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQueueItem {
    pub id: i64,
    pub manga_id: Option<i64>,
    pub series_id: String,
    pub series_title: String,
    pub volume_number: f64,
    pub recommended_candidate_id: String,
    pub provider: String,
    pub title: String,
    pub raw_title: String,
    pub quality_score: f64,
    pub confidence: String,
    pub confidence_reason: Option<String>,
    pub file_size_bytes: Option<i64>,
    pub publication_destination: Option<String>,
    pub status: String, // "pending", "approved", "rejected", "skipped"
    pub status_reason: Option<String>,
    pub alternatives_json: Option<String>,
    pub ranking_breakdown_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default = "default_priority")]
    pub acquisition_priority: i64,
    #[serde(default)]
    pub earlier_missing_gaps: Option<String>,
    #[serde(default)]
    pub ordering_explanation: Option<String>,
    #[serde(default = "default_true")]
    pub is_safe_to_approve: bool,
    #[serde(default)]
    pub komga_series_id: Option<String>,
    #[serde(default)]
    pub last_reconciled_at: Option<i64>,
    #[serde(default)]
    pub owned_volumes_count: Option<i64>,
    #[serde(default)]
    pub missing_volumes_count: Option<i64>,
    #[serde(default)]
    pub is_komga_confirmed: Option<bool>,
    #[serde(default)]
    pub monitor_origins: Option<Vec<String>>,
    #[serde(default)]
    pub selection_reason: Option<String>,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default)]
    pub eligibility_reason: String,
    #[serde(default)]
    pub komga_mapping_status: String,
    #[serde(default)]
    pub ownership_status: String,
    #[serde(default)]
    pub confirmed_owned_volumes: Vec<f64>,
    #[serde(default)]
    pub confirmed_missing_volumes: Vec<f64>,
}

/// Audit history entry for monitoring decisions and executions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringActivityEntry {
    pub id: i64,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: f64,
    pub action: String, // "discovered", "plan_proposed", "approved", "rejected", "auto_acquired", "simulated", "failed"
    pub source: String,
    pub confidence: String,
    pub quality_score: Option<f64>,
    pub decision_code: Option<String>,
    pub details: String,
    pub created_at: i64,
}

// ============================================================================
// Settings Helpers
// ============================================================================

pub fn get_automation_mode(conn: &Connection) -> AutomationMode {
    let mode_str = crate::settings::get::<String>(conn, SETTING_AUTOMATION_MODE)
        .unwrap_or_else(|| "monitor_only".to_string());
    AutomationMode::from_str(&mode_str)
}

pub fn set_automation_mode(conn: &Connection, mode: AutomationMode) -> Result<()> {
    crate::settings::set(conn, SETTING_AUTOMATION_MODE, &serde_json::json!(mode.as_str()))
}

pub fn get_monitoring_interval_hours(conn: &Connection) -> i64 {
    crate::settings::get::<i64>(conn, SETTING_MONITORING_INTERVAL_HOURS).unwrap_or(12)
}

pub fn set_monitoring_interval_hours(conn: &Connection, hours: i64) -> Result<()> {
    let bounded = hours.clamp(1, 168); // 1 hour to 1 week
    crate::settings::set(conn, SETTING_MONITORING_INTERVAL_HOURS, &serde_json::json!(bounded))
}

pub fn get_monitored_tracking_statuses(conn: &Connection) -> Vec<String> {
    crate::settings::get::<Vec<String>>(conn, SETTING_MONITORED_TRACKING_STATUSES)
        .unwrap_or_else(|| vec!["reading".to_string(), "planning".to_string()])
}

pub fn set_monitored_tracking_statuses(conn: &Connection, statuses: &[String]) -> Result<()> {
    crate::settings::set(conn, SETTING_MONITORED_TRACKING_STATUSES, &serde_json::json!(statuses))
}

pub fn get_recommendation_scope(conn: &Connection) -> String {
    crate::settings::get::<String>(conn, SETTING_RECOMMENDATION_SCOPE)
        .unwrap_or_else(|| "mango_library_only".to_string())
}

pub fn set_recommendation_scope(conn: &Connection, scope: &str) -> Result<()> {
    let normalized = match scope.trim().to_lowercase().as_str() {
        "mango_library_only" | "library_only" | "mango_only" => "mango_library_only",
        "tracked_lists" | "lists" => "tracked_lists",
        "all_monitored" | "all" => "all_monitored",
        other => {
            return Err(AppError::bad_request(format!(
                "Invalid recommendation scope: '{other}'. Allowed: 'mango_library_only', 'tracked_lists', 'all_monitored'"
            )));
        }
    };
    crate::settings::set(conn, SETTING_RECOMMENDATION_SCOPE, &serde_json::json!(normalized))
}

/// Checks whether a series is eligible under the given recommendation scope.
/// Fails closed to false if identity or membership is ambiguous.
pub fn is_series_eligible_for_scope(
    conn: &Connection,
    manga_id: Option<i64>,
    series_title: &str,
    scope: &str,
) -> bool {
    match scope {
        "all_monitored" => true,
        "tracked_lists" => {
            if let Some(m_id) = manga_id {
                let canonical_id = crate::sync::reconciliation::resolve_canonical_manga_id(conn, m_id);
                let in_lib: bool = conn
                    .query_row("SELECT in_library FROM manga WHERE id = ?1", [canonical_id], |r| r.get(0))
                    .unwrap_or(false);
                if in_lib {
                    return true;
                }
            }
            let title_in_lib: bool = conn
                .query_row(
                    "SELECT 1 FROM manga WHERE title = ?1 COLLATE NOCASE AND in_library = 1 LIMIT 1",
                    [series_title],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if title_in_lib {
                return true;
            }

            let is_in_lists: bool = conn
                .query_row(
                    "SELECT 1 FROM tracked_catalog WHERE (manga_id = ?1 OR title = ?2 COLLATE NOCASE) AND origins LIKE '%lists%' LIMIT 1",
                    params![manga_id, series_title],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            is_in_lists
        }
        _ => {
            // Default "mango_library_only"
            if let Some(m_id) = manga_id {
                let canonical_id = crate::sync::reconciliation::resolve_canonical_manga_id(conn, m_id);
                let in_lib: bool = conn
                    .query_row(
                        "SELECT in_library FROM manga WHERE id = ?1",
                        [canonical_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);
                if in_lib {
                    return true;
                }
            }
            conn.query_row(
                "SELECT 1 FROM manga WHERE title = ?1 COLLATE NOCASE AND in_library = 1 LIMIT 1",
                [series_title],
                |_| Ok(true),
            )
            .unwrap_or(false)
        }
    }
}

pub fn get_monitoring_settings(conn: &Connection) -> Result<MonitoringSettings> {
    let automation_mode = get_automation_mode(conn);
    let interval_hours = get_monitoring_interval_hours(conn);
    let tracked_statuses = get_monitored_tracking_statuses(conn);
    let last_cycle_at = crate::settings::get::<i64>(conn, SETTING_LAST_CYCLE_AT);
    let auto_acquire_enabled = is_global_auto_acquire_enabled(conn);
    let operating_mode = crate::settings::get::<String>(conn, SETTING_OPERATING_MODE)
        .unwrap_or_else(|| "simulation".to_string());
    let auto_publish_manual = crate::settings::get::<bool>(conn, SETTING_AUTO_PUBLISH_MANUAL)
        .unwrap_or(false);
    let recommendation_scope = get_recommendation_scope(conn);

    Ok(MonitoringSettings {
        automation_mode,
        interval_hours,
        tracked_statuses,
        last_cycle_at,
        auto_acquire_enabled,
        operating_mode,
        auto_publish_manual,
        recommendation_scope,
    })
}

/// Atomically applies a partial monitoring settings update.
/// Only explicitly provided fields in `req` are modified; unspecified fields
/// are strictly preserved to prevent unintended state transitions.
/// All inputs are validated beforehand to fail closed.
pub fn update_monitoring_settings(
    conn: &mut Connection,
    req: &UpdateMonitoringSettingsRequest,
) -> Result<MonitoringSettings> {
    // 1. Validation phase (fails closed before any mutation occurs)
    let validated_mode = match &req.automation_mode {
        Some(s) => {
            let mode = AutomationMode::parse(s).ok_or_else(|| {
                AppError::bad_request(format!(
                    "Invalid automation mode: '{s}'. Allowed: 'monitor_only', 'review_before_acquisition', 'automatic_acquisition'"
                ))
            })?;
            Some(mode)
        }
        None => None,
    };

    let validated_interval = match req.interval_hours {
        Some(hours) => {
            if !(1..=168).contains(&hours) {
                return Err(AppError::bad_request(format!(
                    "Monitoring interval must be between 1 and 168 hours (got {hours})"
                )));
            }
            Some(hours)
        }
        None => None,
    };

    let validated_statuses = match &req.tracked_statuses {
        Some(statuses) => {
            let mut norm_list = Vec::with_capacity(statuses.len());
            for st in statuses {
                let trimmed = st.trim().to_lowercase();
                match trimmed.as_str() {
                    "reading" | "planning" | "completed" | "paused" | "dropped" => {
                        if !norm_list.contains(&trimmed) {
                            norm_list.push(trimmed);
                        }
                    }
                    _ => {
                        return Err(AppError::bad_request(format!(
                            "Invalid tracking status: '{st}'. Allowed: 'reading', 'planning', 'completed', 'paused', 'dropped'"
                        )));
                    }
                }
            }
            Some(norm_list)
        }
        None => None,
    };

    let validated_operating_mode = match &req.operating_mode {
        Some(m) => {
            let trimmed = m.trim().to_lowercase();
            if trimmed != "simulation" && trimmed != "normal" {
                return Err(AppError::bad_request(format!(
                    "Invalid operating mode: '{m}'. Allowed: 'simulation', 'normal'"
                )));
            }
            Some(trimmed)
        }
        None => None,
    };

    let validated_scope = match &req.recommendation_scope {
        Some(s) => {
            let trimmed = s.trim().to_lowercase();
            match trimmed.as_str() {
                "mango_library_only" | "library_only" | "mango_only" => Some("mango_library_only".to_string()),
                "tracked_lists" | "lists" => Some("tracked_lists".to_string()),
                "all_monitored" | "all" => Some("all_monitored".to_string()),
                _ => {
                    return Err(AppError::bad_request(format!(
                        "Invalid recommendation scope: '{s}'. Allowed: 'mango_library_only', 'tracked_lists', 'all_monitored'"
                    )));
                }
            }
        }
        None => None,
    };

    // 2. Atomic mutation phase (all-or-nothing transaction)
    let tx = conn.transaction()?;

    if let Some(mode) = validated_mode {
        set_automation_mode(&tx, mode)?;
    }
    if let Some(interval) = validated_interval {
        set_monitoring_interval_hours(&tx, interval)?;
    }
    if let Some(statuses) = validated_statuses {
        set_monitored_tracking_statuses(&tx, &statuses)?;
    }
    if let Some(auto_acquire) = req.auto_acquire_enabled {
        set_global_auto_acquire(&tx, auto_acquire)?;
    }
    if let Some(op_mode) = validated_operating_mode {
        set_operating_mode(&tx, &op_mode)?;
    }
    if let Some(auto_publish) = req.auto_publish_manual {
        set_auto_publish_manual_enabled(&tx, auto_publish)?;
    }
    if let Some(scope) = validated_scope {
        set_recommendation_scope(&tx, &scope)?;
    }

    tx.commit()?;

    // 3. Return complete updated settings
    get_monitoring_settings(conn)
}

// ============================================================================
// Review Queue CRUD & Acquisition Ordering Engine
// ============================================================================

/// Calculates the sequential acquisition priority and gaps for a volume proposal.
pub fn calculate_acquisition_priority(
    conn: &Connection,
    series_title: &str,
    target_volume: f64,
) -> (i64, Option<String>, Option<String>, bool) {
    let policy = crate::settings::get::<String>(conn, "acquisition.volume_ordering_policy")
        .unwrap_or_else(|| "sequential_priority".to_string());

    let mut missing_earlier = Vec::new();
    let mut missing_earlier_with_candidates = Vec::new();

    if let Ok(mut stmt) = conn.prepare(
        "SELECT volume_number, top_candidate_id, state FROM release_availability_watchers
         WHERE series_title = ?1 AND is_owned = 0 AND volume_number < ?2
         ORDER BY volume_number ASC"
    ) {
        if let Ok(rows) = stmt.query_map(params![series_title, target_volume], |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        }) {
            for (vol, cand_opt, state) in rows.flatten() {
                // If earlier volume is already acquiring, downloading, or completed, skip it
                let is_in_progress: bool = conn.query_row(
                    "SELECT 1 FROM downloads d
                     JOIN chapters c ON c.id = d.chapter_id
                     JOIN manga m ON m.id = c.manga_id
                     WHERE m.title = ?1 AND ABS(c.chapter_number - ?2) < 0.001
                       AND d.state IN ('queued', 'downloading', 'processing', 'validating', 'completed')
                     LIMIT 1",
                    params![series_title, vol],
                    |_| Ok(true)
                ).unwrap_or(false);

                if !is_in_progress {
                    missing_earlier.push(vol);
                    if cand_opt.is_some() || state == "NOTIFIED" || state == "ELIGIBLE_CANDIDATE_FOUND" {
                        missing_earlier_with_candidates.push(vol);
                    }
                }
            }
        }
    }

    if missing_earlier.is_empty() {
        (
            1,
            None,
            Some("Lowest missing volume: prioritized first in sequential acquisition order.".to_string()),
            true,
        )
    } else if !missing_earlier_with_candidates.is_empty() {
        let gaps_json = serde_json::to_string(&missing_earlier).ok();
        let gaps_str = missing_earlier
            .iter()
            .map(|v| format!("Vol. {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        let priority = 1 + missing_earlier.len() as i64;
        let is_safe = policy == "any_available";
        let explanation = format!(
            "Out-of-order proposal (earlier missing {gaps_str} available with active releases); sequential priority ranks earlier volumes first."
        );
        (priority, gaps_json, Some(explanation), is_safe)
    } else {
        let gaps_json = serde_json::to_string(&missing_earlier).ok();
        let gaps_str = missing_earlier
            .iter()
            .map(|v| format!("Vol. {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        let priority = 1 + missing_earlier.len() as i64;
        let explanation = format!(
            "Earlier missing volumes ({gaps_str}) currently have no candidate releases; volume permitted with release gap."
        );
        (priority, gaps_json, Some(explanation), true)
    }
}

pub fn list_review_queue(conn: &Connection, status: Option<&str>) -> Result<Vec<ReviewQueueItem>> {
    let scope = get_recommendation_scope(conn);
    let mut sql = "SELECT q.id, q.manga_id, q.series_id, q.series_title, q.volume_number,
                          q.recommended_candidate_id, q.provider, q.title, q.raw_title, q.quality_score,
                          q.confidence, q.confidence_reason, q.file_size_bytes, q.publication_destination,
                          q.status, q.status_reason, q.alternatives_json, q.ranking_breakdown_json,
                          q.created_at, q.updated_at, q.acquisition_priority, q.earlier_missing_gaps,
                          q.ordering_explanation, q.is_safe_to_approve,
                          sr.komga_series_id, sr.last_reconciled_at, sr.owned_volumes_count, sr.missing_volumes_count,
                          tc.origins,
                          COALESCE(m.in_library, (SELECT m2.in_library FROM manga m2 WHERE m2.title = q.series_title COLLATE NOCASE LIMIT 1), 0) AS in_library,
                          sr.status AS reconciliation_status,
                          sr.owned_volumes_json,
                          sr.missing_volumes_json
                   FROM acquisition_review_queue q
                   LEFT JOIN manga m ON m.id = q.manga_id
                   LEFT JOIN series_reconciliations sr ON (sr.series_id = q.series_id OR sr.title = q.series_title COLLATE NOCASE)
                   LEFT JOIN tracked_catalog tc ON (tc.manga_id = q.manga_id OR tc.title = q.series_title COLLATE NOCASE)".to_string();

    let mut conditions = Vec::new();

    if let Some(st) = status {
        if !st.is_empty() && st != "all" {
            conditions.push(format!("q.status = '{}'", st.replace('\'', "''")));
        }
    }

    match scope.as_str() {
        "all_monitored" => {},
        "tracked_lists" => {
            conditions.push("(COALESCE(m.in_library, (SELECT m2.in_library FROM manga m2 WHERE m2.title = q.series_title COLLATE NOCASE LIMIT 1), 0) = 1 OR tc.origins LIKE '%lists%')".to_string());
        },
        _ => {
            // "mango_library_only" (default)
            conditions.push("COALESCE(m.in_library, (SELECT m2.in_library FROM manga m2 WHERE m2.title = q.series_title COLLATE NOCASE LIMIT 1), 0) = 1".to_string());
        }
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" ORDER BY q.acquisition_priority ASC, q.quality_score DESC, q.volume_number ASC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_review_queue_row)?;
    let mut list = Vec::new();
    for r in rows {
        let item = r?;
        // Post-filter: If volume is confirmed owned in Komga reconciliation, do not return as pending proposal
        if item.status == "pending" && item.confirmed_owned_volumes.iter().any(|&v| (v - item.volume_number).abs() < 0.001) {
            continue;
        }
        list.push(item);
    }
    Ok(list)
}

pub fn get_review_queue_item(conn: &Connection, id: i64) -> Result<Option<ReviewQueueItem>> {
    conn.query_row(
        "SELECT q.id, q.manga_id, q.series_id, q.series_title, q.volume_number,
                q.recommended_candidate_id, q.provider, q.title, q.raw_title, q.quality_score,
                q.confidence, q.confidence_reason, q.file_size_bytes, q.publication_destination,
                q.status, q.status_reason, q.alternatives_json, q.ranking_breakdown_json,
                q.created_at, q.updated_at, q.acquisition_priority, q.earlier_missing_gaps,
                q.ordering_explanation, q.is_safe_to_approve,
                sr.komga_series_id, sr.last_reconciled_at, sr.owned_volumes_count, sr.missing_volumes_count,
                tc.origins,
                COALESCE(m.in_library, (SELECT m2.in_library FROM manga m2 WHERE m2.title = q.series_title COLLATE NOCASE LIMIT 1), 0) AS in_library,
                sr.status AS reconciliation_status,
                sr.owned_volumes_json,
                sr.missing_volumes_json
         FROM acquisition_review_queue q
         LEFT JOIN manga m ON m.id = q.manga_id
         LEFT JOIN series_reconciliations sr ON (sr.series_id = q.series_id OR sr.title = q.series_title COLLATE NOCASE)
         LEFT JOIN tracked_catalog tc ON (tc.manga_id = q.manga_id OR tc.title = q.series_title COLLATE NOCASE)
         WHERE q.id = ?1",
        [id],
        map_review_queue_row,
    )
    .optional()
    .map_err(|e| AppError::internal(format!("Failed to get review queue item: {e}")))
}

pub fn add_or_update_review_queue_item(
    conn: &Connection,
    manga_id: Option<i64>,
    series_id: &str,
    series_title: &str,
    volume_number: f64,
    cand: &ReleaseCandidate,
    alternatives: &[ReleaseCandidate],
    destination: Option<&str>,
) -> Result<i64> {
    // 0. Scope eligibility check (fails closed to no-op if outside configured scope)
    let scope = get_recommendation_scope(conn);
    if !is_series_eligible_for_scope(conn, manga_id, series_title, &scope) {
        tracing::info!(
            "Refusing review queue insertion for {} Vol {}: series is not eligible under recommendation scope '{}'",
            series_title, volume_number, scope
        );
        return Ok(0);
    }

    // 1. Never add candidates with ambiguous confidence (e.g. uncertain title identity)
    if cand.confidence.as_deref() == Some("ambiguous") {
        tracing::info!(
            "Refusing review queue insertion for {} Vol {}: candidate confidence is ambiguous",
            series_title, volume_number
        );
        return Ok(0);
    }

    // 2. Never queue proposals for volumes already owned (check watchers, local files, and Komga reconciliation)
    let is_owned_watcher: bool = conn
        .query_row(
            "SELECT 1 FROM release_availability_watchers
             WHERE (series_id = ?1 OR series_title = ?2 COLLATE NOCASE)
               AND ABS(volume_number - ?3) < 0.001
               AND is_owned = 1
             LIMIT 1",
            params![series_id, series_title, volume_number],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);

    let is_owned_local = if let Some(mid) = manga_id {
        let canonical_mid = crate::sync::reconciliation::resolve_canonical_manga_id(conn, mid);
        crate::acquisition::release_discovery::is_volume_owned(conn, canonical_mid, series_id, volume_number).unwrap_or(false)
    } else {
        false
    };

    let in_recon_owned: bool = conn
        .query_row(
            "SELECT owned_volumes_json FROM series_reconciliations WHERE series_id = ?1 OR title = ?2 COLLATE NOCASE LIMIT 1",
            params![series_id, series_title],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|json| serde_json::from_str::<Vec<f64>>(&json).ok())
        .map_or(false, |vols| vols.iter().any(|&v| (v - volume_number).abs() < 0.001));

    if is_owned_watcher || is_owned_local || in_recon_owned {
        tracing::info!(
            "Refusing review queue insertion for {} Vol {}: volume is already owned",
            series_title, volume_number
        );
        return Ok(0);
    }
    let current_time = now();
    let alt_json = if alternatives.is_empty() {
        None
    } else {
        serde_json::to_string(alternatives).ok()
    };
    let rb_json = cand.ranking_breakdown.as_ref().and_then(|rb| serde_json::to_string(rb).ok());

    let (priority, gaps_json, ordering_expl, is_safe) =
        calculate_acquisition_priority(conn, series_title, volume_number);

    conn.execute(
        "INSERT INTO acquisition_review_queue (
            manga_id, series_id, series_title, volume_number, recommended_candidate_id,
            provider, title, raw_title, quality_score, confidence, confidence_reason,
            file_size_bytes, publication_destination, status, status_reason,
            alternatives_json, ranking_breakdown_json, created_at, updated_at,
            acquisition_priority, earlier_missing_gaps, ordering_explanation, is_safe_to_approve
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', NULL, ?14, ?15, ?16, ?16, ?17, ?18, ?19, ?20)
        ON CONFLICT(series_id, volume_number) DO UPDATE SET
            manga_id = excluded.manga_id,
            recommended_candidate_id = excluded.recommended_candidate_id,
            provider = excluded.provider,
            title = excluded.title,
            raw_title = excluded.raw_title,
            quality_score = excluded.quality_score,
            confidence = excluded.confidence,
            confidence_reason = excluded.confidence_reason,
            file_size_bytes = excluded.file_size_bytes,
            publication_destination = excluded.publication_destination,
            alternatives_json = excluded.alternatives_json,
            ranking_breakdown_json = excluded.ranking_breakdown_json,
            updated_at = excluded.updated_at,
            acquisition_priority = excluded.acquisition_priority,
            earlier_missing_gaps = excluded.earlier_missing_gaps,
            ordering_explanation = excluded.ordering_explanation,
            is_safe_to_approve = excluded.is_safe_to_approve
        WHERE status NOT IN ('approved', 'rejected', 'skipped')",
        params![
            manga_id,
            series_id,
            series_title,
            volume_number,
            cand.candidate_id,
            cand.provider,
            cand.title,
            cand.raw_title,
            cand.score,
            cand.confidence.as_deref().unwrap_or("high"),
            cand.confidence_reason,
            cand.size_bytes.map(|s| s as i64),
            destination,
            alt_json,
            rb_json,
            current_time,
            priority,
            gaps_json,
            ordering_expl,
            if is_safe { 1 } else { 0 },
        ],
    )?;

    conn.query_row(
        "SELECT id FROM acquisition_review_queue WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
        params![series_id, volume_number],
        |r| r.get(0),
    )
    .map_err(|e| AppError::internal(format!("Failed to retrieve review queue item id: {e}")))
}

pub fn reject_review_queue_item(conn: &Connection, id: i64, reason: Option<&str>) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE acquisition_review_queue
         SET status = 'rejected', status_reason = ?1, updated_at = ?2
         WHERE id = ?3",
        params![reason, current_time, id],
    )?;
    Ok(())
}

pub fn skip_review_queue_item(conn: &Connection, id: i64, reason: Option<&str>) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE acquisition_review_queue
         SET status = 'skipped', status_reason = ?1, updated_at = ?2
         WHERE id = ?3",
        params![reason, current_time, id],
    )?;
    Ok(())
}

pub async fn approve_review_queue_item(
    db: &Db,
    acquisition: &AcquisitionService,
    id: i64,
    user_id: Option<i64>,
) -> Result<BatchAcquireItemResult> {
    let item = db.call(move |conn| get_review_queue_item(conn, id)).await?
        .ok_or_else(|| AppError::not_found(format!("Review item #{id} not found")))?;

    let manga_id = item.manga_id.ok_or_else(|| {
        AppError::bad_request("Cannot approve item without associated manga ID")
    })?;

    let is_simulation = db.call(|conn| Ok(is_simulation_mode(conn))).await?;

    let approve_req = ApproveCandidateRequest {
        dry_run: Some(is_simulation),
        authorization_token: None,
        user_id,
        confirm_review: Some(true),
        confirm_range: Some(true),
        confirm_pack: Some(true),
    };

    let resp = acquisition
        .approve_candidate(manga_id, item.volume_number, &item.recommended_candidate_id, approve_req)
        .await?;

    let current_time = now();
    let status_str = if is_simulation { "simulated" } else { "approved" };

    db.call(move |conn| {
        conn.execute(
            "UPDATE acquisition_review_queue
             SET status = ?1, updated_at = ?2
             WHERE id = ?3",
            params![status_str, current_time, id],
        )?;

        // Update watcher if exists
        conn.execute(
            "UPDATE release_availability_watchers
             SET state = 'ACQUIRED', updated_at = ?1
             WHERE series_id = ?2 AND ABS(volume_number - ?3) < 0.001",
            params![current_time, item.series_id, item.volume_number],
        )?;

        // Log to activity
        log_monitoring_activity_internal(
            conn,
            item.manga_id,
            &item.series_title,
            item.volume_number,
            if is_simulation { "simulated" } else { "approved" },
            &item.provider,
            &item.confidence,
            Some(item.quality_score),
            Some("OPERATOR_APPROVED"),
            &format!("Operator approved acquisition of {} Vol. {}", item.series_title, item.volume_number),
            current_time,
        )?;

        Ok(())
    }).await?;

    Ok(BatchAcquireItemResult {
        volume_number: item.volume_number,
        status: resp.status,
        message: resp.message,
        execution_id: resp.execution_id,
        candidate_id: Some(item.recommended_candidate_id),
    })
}

// ============================================================================
// Activity History
// ============================================================================

pub fn log_monitoring_activity_internal(
    conn: &Connection,
    manga_id: Option<i64>,
    series_title: &str,
    volume_number: f64,
    action: &str,
    source: &str,
    confidence: &str,
    quality_score: Option<f64>,
    decision_code: Option<&str>,
    details: &str,
    created_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO monitoring_activity_history (
            manga_id, series_title, volume_number, action, source,
            confidence, quality_score, decision_code, details, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            manga_id,
            series_title,
            volume_number,
            action,
            source,
            confidence,
            quality_score,
            decision_code,
            details,
            created_at,
        ],
    )?;
    Ok(())
}

pub fn list_monitoring_activity(conn: &Connection, limit: usize) -> Result<Vec<MonitoringActivityEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, manga_id, series_title, volume_number, action, source,
                confidence, quality_score, decision_code, details, created_at
         FROM monitoring_activity_history
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;

    let rows = stmt.query_map([limit as i64], |r| {
        Ok(MonitoringActivityEntry {
            id: r.get(0)?,
            manga_id: r.get(1)?,
            series_title: r.get(2)?,
            volume_number: r.get(3)?,
            action: r.get(4)?,
            source: r.get(5)?,
            confidence: r.get(6)?,
            quality_score: r.get(7)?,
            decision_code: r.get(8)?,
            details: r.get(9)?,
            created_at: r.get(10)?,
        })
    })?;

    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

// ============================================================================
// Followed-Series Synchronization & Scheduler
// ============================================================================

/// Synchronizes all followed series matching configured tracking statuses to availability watchers.
pub async fn sync_followed_series_to_watchers(db: &Db, _http: reqwest::Client) -> Result<usize> {
    let tracked_statuses = db.call(|conn| Ok(get_monitored_tracking_statuses(conn))).await?;
    let scope = db.call(|conn| Ok(get_recommendation_scope(conn))).await?;

    // 1. Gather all followed series from tracked_catalog and tracker_list_entries
    let series_to_monitor: Vec<(i64, String, String)> = db.call(move |conn| {
        let mut results = Vec::new();

        // Query tracked_catalog
        let mut stmt = conn.prepare(
            "SELECT tc.manga_id, tc.title, tc.komga_series_id
             FROM tracked_catalog tc
             WHERE tc.tracked = 1 AND tc.manga_id IS NOT NULL",
        )?;

        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
        })?;

        for r in rows {
            let (manga_id, title, komga_series_id) = r?;

            // Check recommendation scope eligibility
            if !is_series_eligible_for_scope(conn, Some(manga_id), &title, &scope) {
                continue;
            }

            // Check if series has an entry in tracker_list_entries matching tracked_statuses
            let norm_status: Option<String> = conn
                .query_row(
                    "SELECT normalized_status FROM tracker_list_entries
                     WHERE title = ?1 COLLATE NOCASE
                        OR (?2 IS NOT NULL AND tracker_id = (SELECT tracker_id FROM tracker_mappings WHERE komga_series_id = ?2 LIMIT 1))
                     LIMIT 1",
                    params![title, komga_series_id],
                    |row| row.get(0),
                )
                .optional()?;

            let is_eligible = match norm_status {
                Some(st) => tracked_statuses.iter().any(|ts| ts.eq_ignore_ascii_case(&st)),
                None => true, // default include if tracked in mango catalog
            };

            if is_eligible {
                let canonical_id = crate::sync::reconciliation::resolve_canonical_manga_id(conn, manga_id);
                let series_id = format!("manga:{canonical_id}");
                if !results.iter().any(|(mid, sid, _)| *mid == canonical_id || sid == &series_id) {
                    results.push((canonical_id, series_id, title));
                }
            }
        }

        Ok(results)
    }).await?;

    let mut total_synced_watchers = 0;

    for (manga_id, series_id, _title) in series_to_monitor {
        let sid = series_id.clone();
        let recon_opt = db.call(move |conn| get_reconciliation(conn, &sid)).await?;

        if let Some(recon) = recon_opt {
            let missing = recon.missing_volumes.clone();
            let future = recon.future_volumes.clone();
            let owned = recon.owned_volumes.clone();
            let mangabaka_id = recon.mangabaka_id;
            let collection_id = recon.collection_id.clone();
            let s_title = recon.title.clone();

            let watchers = db.call(move |conn| {
                sync_watchers_for_series(
                    conn,
                    manga_id,
                    &series_id,
                    &s_title,
                    mangabaka_id,
                    collection_id.as_deref(),
                    &missing,
                    &future,
                    &owned,
                )
            }).await?;

            total_synced_watchers += watchers.len();
        }
    }

    Ok(total_synced_watchers)
}

/// Runs a full monitoring cycle: synchronizes followed series to watchers and updates timestamp.
pub async fn run_monitored_series_cycle(db: &Db, http: reqwest::Client) -> Result<usize> {
    let synced = sync_followed_series_to_watchers(db, http).await?;
    let current_time = now();

    db.call(move |conn| {
        crate::settings::set(conn, SETTING_LAST_CYCLE_AT, &serde_json::json!(current_time))?;
        Ok(())
    }).await?;

    Ok(synced)
}

fn map_review_queue_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewQueueItem> {
    let komga_series_id: Option<String> = r.get(24).ok();
    let last_reconciled_at: Option<i64> = r.get(25).ok();
    let owned_volumes_count: Option<i64> = r.get(26).ok();
    let missing_volumes_count: Option<i64> = r.get(27).ok();
    let origins_raw: Option<String> = r.get(28).ok();
    let monitor_origins = origins_raw.and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok());
    let is_komga_confirmed = Some(komga_series_id.is_some());
    let ordering_explanation: Option<String> = r.get(22).ok();
    let confidence_reason: Option<String> = r.get(11).ok();
    let provider: String = r.get(6)?;
    let score: f64 = r.get(9)?;

    let in_library: bool = r.get::<_, i64>(29).unwrap_or(0) != 0;
    let reconciliation_status: Option<String> = r.get(30).ok();
    let owned_volumes_json: Option<String> = r.get(31).ok();
    let missing_volumes_json: Option<String> = r.get(32).ok();

    let komga_mapping_status = if komga_series_id.is_some() {
        "mapped".to_string()
    } else if reconciliation_status.as_deref() == Some("review_required") {
        "ambiguous".to_string()
    } else {
        "unmapped".to_string()
    };

    let ownership_status = if komga_series_id.is_none() {
        "unmapped_to_komga".to_string()
    } else if reconciliation_status.as_deref() == Some("review_required")
        || reconciliation_status.as_deref() == Some("sync_error")
        || last_reconciled_at.is_none()
    {
        "unknown_ownership".to_string()
    } else if missing_volumes_count == Some(0) {
        "confirmed_owned".to_string()
    } else {
        "confirmed_missing".to_string()
    };

    let eligibility_reason = if in_library {
        "Explicit Mango Library Member".to_string()
    } else if monitor_origins.as_ref().map_or(false, |o| o.iter().any(|s| s.contains("lists"))) {
        "Tracked List Entry".to_string()
    } else {
        "Monitored Title".to_string()
    };

    let confirmed_owned_volumes: Vec<f64> = owned_volumes_json
        .and_then(|raw| serde_json::from_str::<Vec<f64>>(&raw).ok())
        .unwrap_or_default();

    let confirmed_missing_volumes: Vec<f64> = missing_volumes_json
        .and_then(|raw| serde_json::from_str::<Vec<f64>>(&raw).ok())
        .unwrap_or_default();

    let raw_safe: bool = r.get::<_, i64>(23).map(|v| v != 0).unwrap_or(true);
    let is_safe_to_approve = raw_safe && ownership_status != "unknown_ownership" && ownership_status != "unmapped_to_komga";

    let selection_reason = Some(format!(
        "Discovered by {provider} (Score: {score:.0}) - {}",
        ordering_explanation
            .as_deref()
            .or(confidence_reason.as_deref())
            .unwrap_or("Optimal candidate according to acquisition profile")
    ));

    Ok(ReviewQueueItem {
        id: r.get(0)?,
        manga_id: r.get(1)?,
        series_id: r.get(2)?,
        series_title: r.get(3)?,
        volume_number: r.get(4)?,
        recommended_candidate_id: r.get(5)?,
        provider,
        title: r.get(7)?,
        raw_title: r.get(8)?,
        quality_score: score,
        confidence: r.get(10)?,
        confidence_reason,
        file_size_bytes: r.get(12)?,
        publication_destination: r.get(13)?,
        status: r.get(14)?,
        status_reason: r.get(15)?,
        alternatives_json: r.get(16)?,
        ranking_breakdown_json: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
        acquisition_priority: r.get(20).unwrap_or(1),
        earlier_missing_gaps: r.get(21).ok(),
        ordering_explanation,
        is_safe_to_approve,
        komga_series_id,
        last_reconciled_at,
        owned_volumes_count,
        missing_volumes_count,
        is_komga_confirmed,
        monitor_origins,
        selection_reason,
        in_library,
        eligibility_reason,
        komga_mapping_status,
        ownership_status,
        confirmed_owned_volumes,
        confirmed_missing_volumes,
    })
}
