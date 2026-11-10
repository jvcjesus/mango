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
            "automatic_acquisition" | "auto" | "mode_c" => Some(Self::AutomaticAcquisition),
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
        }
    }
}

/// Proposed acquisition in the Review Queue (Mode B)
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

    Ok(MonitoringSettings {
        automation_mode,
        interval_hours,
        tracked_statuses,
        last_cycle_at,
        auto_acquire_enabled,
        operating_mode,
        auto_publish_manual,
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

    tx.commit()?;

    // 3. Return complete updated settings
    get_monitoring_settings(conn)
}

// ============================================================================
// Review Queue CRUD
// ============================================================================

pub fn list_review_queue(conn: &Connection, status: Option<&str>) -> Result<Vec<ReviewQueueItem>> {
    let mut sql = "SELECT id, manga_id, series_id, series_title, volume_number,
                          recommended_candidate_id, provider, title, raw_title, quality_score,
                          confidence, confidence_reason, file_size_bytes, publication_destination,
                          status, status_reason, alternatives_json, ranking_breakdown_json,
                          created_at, updated_at
                   FROM acquisition_review_queue".to_string();

    if let Some(st) = status {
        if !st.is_empty() && st != "all" {
            sql.push_str(&format!(" WHERE status = '{}'", st.replace('\'', "''")));
        }
    }
    sql.push_str(" ORDER BY updated_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_review_queue_row)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn get_review_queue_item(conn: &Connection, id: i64) -> Result<Option<ReviewQueueItem>> {
    conn.query_row(
        "SELECT id, manga_id, series_id, series_title, volume_number,
                recommended_candidate_id, provider, title, raw_title, quality_score,
                confidence, confidence_reason, file_size_bytes, publication_destination,
                status, status_reason, alternatives_json, ranking_breakdown_json,
                created_at, updated_at
         FROM acquisition_review_queue WHERE id = ?1",
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
    let current_time = now();
    let alt_json = if alternatives.is_empty() {
        None
    } else {
        serde_json::to_string(alternatives).ok()
    };
    let rb_json = cand.ranking_breakdown.as_ref().and_then(|rb| serde_json::to_string(rb).ok());

    conn.execute(
        "INSERT INTO acquisition_review_queue (
            manga_id, series_id, series_title, volume_number, recommended_candidate_id,
            provider, title, raw_title, quality_score, confidence, confidence_reason,
            file_size_bytes, publication_destination, status, status_reason,
            alternatives_json, ranking_breakdown_json, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', NULL, ?14, ?15, ?16, ?16)
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
            updated_at = excluded.updated_at
        WHERE status != 'approved'",
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
                let series_id = format!("manga:{manga_id}");
                results.push((manga_id, series_id, title));
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
    Ok(ReviewQueueItem {
        id: r.get(0)?,
        manga_id: r.get(1)?,
        series_id: r.get(2)?,
        series_title: r.get(3)?,
        volume_number: r.get(4)?,
        recommended_candidate_id: r.get(5)?,
        provider: r.get(6)?,
        title: r.get(7)?,
        raw_title: r.get(8)?,
        quality_score: r.get(9)?,
        confidence: r.get(10)?,
        confidence_reason: r.get(11)?,
        file_size_bytes: r.get(12)?,
        publication_destination: r.get(13)?,
        status: r.get(14)?,
        status_reason: r.get(15)?,
        alternatives_json: r.get(16)?,
        ranking_breakdown_json: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
    })
}
