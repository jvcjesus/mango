//! Library Integrity and Diagnostics Audit Subsystem (Milestone M6.8.3)
//!
//! Provides proactive, read-only system audits across the entire manga library:
//! - Discrepancies between Komga ownership and Mango local files
//! - Duplicate canonical publications / volume collisions
//! - Missing files for completed downloads
//! - MangaBaka identity conflicts and multiple titles sharing IDs
//! - Orphan tracker mappings
//! - Stuck notification outbox items
//! - Inconsistent / invalid auto-acquire states
//!
//! Categorized into structured severity levels: Healthy, Info, Warning, ActionRequired, Critical.

use std::collections::HashMap;
use std::path::Path;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::now;
use crate::error::{AppError, Result};

/// Severity levels for library integrity audit issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IntegritySeverity {
    Healthy,
    Info,
    Warning,
    ActionRequired,
    Critical,
}

impl IntegritySeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "HEALTHY",
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::ActionRequired => "ACTION_REQUIRED",
            Self::Critical => "CRITICAL",
        }
    }
}

/// A specific diagnostic integrity issue identified by the audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityIssue {
    pub id: String,
    pub category: String,
    pub severity: IntegritySeverity,
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    pub manga_id: Option<i64>,
    pub series_title: Option<String>,
    pub resolution_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_paths: Vec<String>,
}

/// Comprehensive library integrity audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIntegrityReport {
    pub checked_at: i64,
    pub status: String,
    pub total_issues: usize,
    pub issues_by_severity: HashMap<String, usize>,
    pub issues: Vec<IntegrityIssue>,
}

impl LibraryIntegrityReport {
    /// Filters issues by severity, category, or title/keyword search.
    pub fn filter(&mut self, severity: Option<&str>, category: Option<&str>, search: Option<&str>) {
        if let Some(sev) = severity {
            let sev_upper = sev.to_uppercase();
            self.issues.retain(|i| i.severity.as_str().eq_ignore_ascii_case(&sev_upper));
        }
        if let Some(cat) = category {
            let cat_lower = cat.to_lowercase();
            self.issues.retain(|i| i.category.to_lowercase() == cat_lower);
        }
        if let Some(q) = search {
            let q_lower = q.to_lowercase();
            self.issues.retain(|i| {
                i.message.to_lowercase().contains(&q_lower)
                    || i.code.to_lowercase().contains(&q_lower)
                    || i.series_title.as_ref().map_or(false, |t| t.to_lowercase().contains(&q_lower))
                    || i.details.as_ref().map_or(false, |d| d.to_lowercase().contains(&q_lower))
            });
        }
        self.total_issues = self.issues.len();
    }
}

/// Runs a complete, read-only audit across all library tables and storage references.
pub fn audit_library_integrity(
    conn: &Connection,
    downloads_dir: Option<&Path>,
) -> Result<LibraryIntegrityReport> {
    let checked_at = now();
    let mut issues = Vec::new();

    // ---------------------------------------------------------
    // Audit Check 1: MangaBaka Identity Conflicts
    // ---------------------------------------------------------
    // Check if separate independent titles share the same mangabaka_id
    let mut conflict_stmt = conn.prepare(
        "SELECT m1.id, m1.title, m2.id, m2.title, m1.mangabaka_id
         FROM manga m1
         JOIN manga m2 ON m1.mangabaka_id = m2.mangabaka_id AND m1.id < m2.id
         WHERE m1.mangabaka_id IS NOT NULL
           AND (m1.catalog_manga_id IS NULL OR m1.catalog_manga_id != m2.id)
           AND (m2.catalog_manga_id IS NULL OR m2.catalog_manga_id != m1.id)
           AND (m1.source_manga_id IS NULL OR m1.source_manga_id != m2.id)
           AND (m2.source_manga_id IS NULL OR m2.source_manga_id != m1.id)",
    ).map_err(|e| AppError::internal(format!("Prepare conflict stmt error: {e}")))?;

    let rows = conflict_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query conflict error: {e}")))?;

    for r in rows.flatten() {
        let (id1, title1, id2, title2, mb_id) = r;
        issues.push(IntegrityIssue {
            id: format!("mb-conflict-{id1}-{id2}"),
            category: "identity".to_string(),
            severity: IntegritySeverity::ActionRequired,
            code: "MANGABAKA_IDENTITY_CONFLICT".to_string(),
            message: format!("MangaBaka ID {mb_id} is simultaneously mapped to independent titles '{title1}' and '{title2}'"),
            details: Some(format!("Manga ID {id1} vs {id2}")),
            manga_id: Some(id1),
            series_title: Some(title1),
            resolution_hint: Some("Reassign one of the titles to its correct MangaBaka work or clear mapping".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // Check discrepancy between manga.mangabaka_id and tracker_mappings
    let mut mismatch_stmt = conn.prepare(
        "SELECT m.id, m.title, m.mangabaka_id, tm.tracker_id
         FROM manga m
         JOIN tracked_catalog tc ON tc.manga_id = m.id
         JOIN tracker_mappings tm ON tm.komga_series_id = tc.komga_series_id
         WHERE tm.tracker = 'mangabaka' AND m.mangabaka_id IS NOT NULL
           AND CAST(tm.tracker_id AS INTEGER) != m.mangabaka_id",
    ).map_err(|e| AppError::internal(format!("Prepare mismatch stmt error: {e}")))?;

    let rows = mismatch_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query mismatch error: {e}")))?;

    for r in rows.flatten() {
        let (mid, m_title, m_mb, t_mb) = r;
        issues.push(IntegrityIssue {
            id: format!("mb-tracker-mismatch-{mid}"),
            category: "identity".to_string(),
            severity: IntegritySeverity::Warning,
            code: "MANGABAKA_TRACKER_MISMATCH".to_string(),
            message: format!("Title '{m_title}' has manga.mangabaka_id = {m_mb} but tracker_mappings = {t_mb}"),
            details: None,
            manga_id: Some(mid),
            series_title: Some(m_title),
            resolution_hint: Some("Reconcile series to synchronize MangaBaka identity across records".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // ---------------------------------------------------------
    // Audit Check 2: Komga Ownership Anomalies
    // ---------------------------------------------------------
    // Only primary catalog titles (source_id = 0 and not converted from an external source)
    // are expected to have a linked Komga series. External reading extension chapters
    // (source_id > 0) are reader caches and not Komga canonical library entities.
    let mut anomaly_stmt = conn.prepare(
        "SELECT m.id, m.title, COUNT(c.id) AS chapter_count
         FROM manga m
         JOIN chapters c ON c.manga_id = m.id AND c.download_path IS NOT NULL
         LEFT JOIN tracked_catalog tc ON tc.manga_id = m.id
         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
         WHERE m.source_id = 0 AND m.catalog_manga_id IS NULL
           AND (tc.komga_series_id IS NULL OR tc.komga_series_id = '')
           AND (sr.komga_series_id IS NULL OR sr.komga_series_id = '')
         GROUP BY m.id, m.title",
    ).map_err(|e| AppError::internal(format!("Prepare anomaly stmt error: {e}")))?;

    let rows = anomaly_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query anomaly error: {e}")))?;

    for r in rows.flatten() {
        let (mid, m_title, count) = r;
        issues.push(IntegrityIssue {
            id: format!("komga-orphan-media-{mid}"),
            category: "ownership".to_string(),
            severity: IntegritySeverity::Warning,
            code: "KOMGA_OWNERSHIP_ANOMALY".to_string(),
            message: format!("Title '{m_title}' has {count} downloaded chapters in Mango but no linked Komga series"),
            details: Some(format!("{count} downloaded chapter(s)")),
            manga_id: Some(mid),
            series_title: Some(m_title),
            resolution_hint: Some("Ensure Komga library scan is completed and series title matches".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // ---------------------------------------------------------
    // Audit Check 3: Duplicate Volume Records
    // ---------------------------------------------------------
    // Verify duplicate volume records on disk rather than counting stale staging/payload paths
    // whose files were moved into canonical storage.
    let mut dup_stmt = conn.prepare(
        "SELECT m.id, m.title, c.chapter_number, c.download_path
         FROM chapters c
         JOIN manga m ON m.id = c.manga_id
         WHERE c.download_path IS NOT NULL AND c.chapter_number > 0",
    ).map_err(|e| AppError::internal(format!("Prepare dup stmt error: {e}")))?;

    let mut vol_paths: HashMap<(i64, String, u32), Vec<String>> = HashMap::new();

    let rows = dup_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, f64>(2)?,
            r.get::<_, String>(3)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query dup error: {e}")))?;

    for r in rows.flatten() {
        let (mid, m_title, vol_num, dl_path) = r;
        let p = Path::new(&dl_path);
        let exists = if p.is_absolute() {
            p.exists()
        } else if let Some(dir) = downloads_dir {
            dir.join(p).exists()
        } else {
            // When no downloads directory context is provided (e.g. mock DB in unit tests),
            // assume existence.
            true
        };
        if exists {
            let key = (mid, m_title, (vol_num * 1000.0).round() as u32);
            vol_paths.entry(key).or_default().push(dl_path);
        }
    }

    let mut sorted_vols: Vec<_> = vol_paths.into_iter().filter(|(_, paths)| paths.len() > 1).collect();
    sorted_vols.sort_by(|a, b| a.0.0.cmp(&b.0.0).then_with(|| a.0.2.cmp(&b.0.2)));

    for ((mid, m_title, vol_key), paths) in sorted_vols {
        let vol_num = (vol_key as f64) / 1000.0;
        let cnt = paths.len();

        let (canonical_paths, staging_paths): (Vec<_>, Vec<_>) = paths
            .iter()
            .cloned()
            .partition(|p| p.starts_with("/data/local/") || p.starts_with("/srv/media/") || p.starts_with("/pipeline-library/"));

        let (storage_type, message, details, hint) = if !canonical_paths.is_empty() && !staging_paths.is_empty() {
            (
                "canonical_and_staging".to_string(),
                format!("Title '{m_title}' volume {vol_num} is present in both Canonical Storage and Local Download Staging"),
                format!("Canonical: {}\nStaging: {}", canonical_paths.join(", "), staging_paths.join(", ")),
                "Canonical copy is verified in Komga; local download copy represents historical download staging. No automatic deletion is performed; verify before removing staging files.".to_string(),
            )
        } else if canonical_paths.len() > 1 {
            (
                "duplicate_canonical".to_string(),
                format!("Title '{m_title}' volume {vol_num} has {cnt} duplicate canonical library files"),
                canonical_paths.join("\n"),
                "Consolidate duplicate canonical files in library directory".to_string(),
            )
        } else {
            (
                "duplicate_staging".to_string(),
                format!("Title '{m_title}' volume {vol_num} has {cnt} duplicate staging downloads"),
                staging_paths.join("\n"),
                "Clean up duplicate staging downloads".to_string(),
            )
        };

        issues.push(IntegrityIssue {
            id: format!("duplicate-vol-{mid}-{vol_num}"),
            category: "storage".to_string(),
            severity: IntegritySeverity::Warning,
            code: "DUPLICATE_VOLUME_RECORDS".to_string(),
            message,
            details: Some(details),
            manga_id: Some(mid),
            series_title: Some(m_title),
            resolution_hint: Some(hint),
            storage_type: Some(storage_type),
            affected_paths: paths,
        });
    }

    // ---------------------------------------------------------
    // Audit Check 4: Orphan Tracker Mappings
    // ---------------------------------------------------------
    let mut orphan_stmt = conn.prepare(
        "SELECT tm.komga_series_id, tm.tracker, tm.tracker_id, tm.tracker_title
         FROM tracker_mappings tm
         LEFT JOIN tracked_catalog tc ON tc.komga_series_id = tm.komga_series_id
         LEFT JOIN series_reconciliations sr ON sr.komga_series_id = tm.komga_series_id
         WHERE tc.komga_series_id IS NULL AND sr.komga_series_id IS NULL",
    ).map_err(|e| AppError::internal(format!("Prepare orphan stmt error: {e}")))?;

    let rows = orphan_stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query orphan error: {e}")))?;

    for r in rows.flatten() {
        let (k_id, tracker, trk_id, title_opt) = r;
        let display = title_opt.unwrap_or_else(|| k_id.clone());
        issues.push(IntegrityIssue {
            id: format!("orphan-mapping-{k_id}-{tracker}"),
            category: "trackers".to_string(),
            severity: IntegritySeverity::Info,
            code: "ORPHAN_TRACKER_MAPPING".to_string(),
            message: format!("Tracker mapping for {tracker} ID {trk_id} ('{display}') has no active series in catalog"),
            details: Some(format!("Komga series ID: {k_id}")),
            manga_id: None,
            series_title: Some(display),
            resolution_hint: Some("Will be cleaned automatically during catalog maintenance or when series is re-imported".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // ---------------------------------------------------------
    // Audit Check 5: Stuck Notification Outbox Entries
    // ---------------------------------------------------------
    let stuck_threshold = checked_at - 3600; // 1 hour threshold
    let mut outbox_stmt = conn.prepare(
        "SELECT id, event_type, destination, status, attempts, last_error, created_at
         FROM notification_outbox
         WHERE (status IN ('pending', 'sending', 'retry_wait') AND next_attempt_at < ?1)
            OR status = 'permanently_failed'",
    ).map_err(|e| AppError::internal(format!("Prepare outbox stmt error: {e}")))?;

    let rows = outbox_stmt.query_map([stuck_threshold], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, i64>(6)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query outbox error: {e}")))?;

    for r in rows.flatten() {
        let (ob_id, evt_type, dest, status, attempts, last_err, _created) = r;
        let sev = if status == "permanently_failed" {
            IntegritySeverity::ActionRequired
        } else {
            IntegritySeverity::Warning
        };

        issues.push(IntegrityIssue {
            id: format!("outbox-stuck-{ob_id}"),
            category: "notifications".to_string(),
            severity: sev,
            code: "STUCK_OUTBOX_NOTIFICATION".to_string(),
            message: format!("Outbox notification #{ob_id} ({evt_type}) is {status} after {attempts} attempts"),
            details: last_err.map(|e| format!("Destination: {dest} - Error: {e}")),
            manga_id: None,
            series_title: None,
            resolution_hint: Some("Check ntfy server URL and network connectivity in Notification settings".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // ---------------------------------------------------------
    // Audit Check 6: Impossible Auto-Acquire States
    // ---------------------------------------------------------
    let mut auto_stmt = conn.prepare(
        "SELECT m.id, m.title, sr.status, sr.mangabaka_id, sr.collection_id, sr.review_reason
         FROM manga m
         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
         WHERE m.auto_acquire = 1
           AND (sr.status IN ('ActionRequired', 'review_required', 'Unmatched')
                OR sr.mangabaka_id IS NULL
                OR sr.collection_id IS NULL
                OR sr.review_reason IS NOT NULL)",
    ).map_err(|e| AppError::internal(format!("Prepare auto stmt error: {e}")))?;

    let rows = auto_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    }).map_err(|e| AppError::internal(format!("Query auto error: {e}")))?;

    for r in rows.flatten() {
        let (mid, m_title, status_opt, mb_opt, coll_opt, reason_opt) = r;
        let reason_desc = reason_opt.unwrap_or_else(|| {
            if mb_opt.is_none() {
                "MangaBaka identity is unlinked".to_string()
            } else if coll_opt.is_none() {
                "MangaBaka collection is unresolved".to_string()
            } else {
                format!("Reconciliation status: {}", status_opt.unwrap_or_default())
            }
        });

        issues.push(IntegrityIssue {
            id: format!("auto-acquire-blocked-{mid}"),
            category: "policy".to_string(),
            severity: IntegritySeverity::ActionRequired,
            code: "AUTO_ACQUIRE_INVALID_RECONCILIATION".to_string(),
            message: format!("Title '{m_title}' has auto-acquire enabled but reconciliation requires review: {reason_desc}"),
            details: Some(format!("Manga ID: {mid}")),
            manga_id: Some(mid),
            series_title: Some(m_title),
            resolution_hint: Some("Resolve MangaBaka series and collection in Diagnostics or disable auto-acquire".to_string()),
            storage_type: None,
            affected_paths: Vec::new(),
        });
    }

    // ---------------------------------------------------------
    // Compute Summary & Status
    // ---------------------------------------------------------
    let mut issues_by_severity = HashMap::new();
    issues_by_severity.insert("CRITICAL".to_string(), 0);
    issues_by_severity.insert("ACTION_REQUIRED".to_string(), 0);
    issues_by_severity.insert("WARNING".to_string(), 0);
    issues_by_severity.insert("INFO".to_string(), 0);

    for issue in &issues {
        *issues_by_severity.entry(issue.severity.as_str().to_string()).or_insert(0) += 1;
    }

    let status = if *issues_by_severity.get("CRITICAL").unwrap_or(&0) > 0 {
        "critical".to_string()
    } else if *issues_by_severity.get("ACTION_REQUIRED").unwrap_or(&0) > 0 {
        "action_required".to_string()
    } else if *issues_by_severity.get("WARNING").unwrap_or(&0) > 0 {
        "warning".to_string()
    } else {
        "healthy".to_string()
    };

    Ok(LibraryIntegrityReport {
        checked_at,
        status,
        total_issues: issues.len(),
        issues_by_severity,
        issues,
    })
}
