//! Persistent sync audit logging.
//!
//! Stores and queries detailed sync execution records in the SQLite database,
//! preserving diagnostic diffs while ensuring secrets are never persisted.

use super::model::{SyncRecord, SyncResultCategory, SyncTarget};
use crate::error::{AppError, Result};
use rusqlite::{Connection, params};

/// Inserts a sync execution record into the audit table.
pub fn record_sync(conn: &Connection, record: &SyncRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_records (
            id, timestamp, komga_series_id, komga_series_title, tracker,
            tracker_media_id, old_progress, new_progress, status_change,
            category, message, is_dry_run, dimension
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            record.id,
            record.timestamp,
            record.komga_series_id,
            record.komga_series_title,
            record.tracker.as_str(),
            record.tracker_media_id,
            record.old_progress,
            record.new_progress,
            record.status_change,
            record.category.as_str(),
            record.message,
            if record.is_dry_run { 1 } else { 0 },
            record.dimension,
        ],
    )
    .map_err(|e| AppError::internal(format!("Failed to record sync audit entry: {e}")))?;

    Ok(())
}

/// Lists recent sync records ordered by most recent first.
pub fn list_recent_records(conn: &Connection, limit: usize) -> Result<Vec<SyncRecord>> {
    let limit_i64 = limit.max(1).min(500) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, komga_series_id, komga_series_title, tracker,
                    tracker_media_id, old_progress, new_progress, status_change,
                    category, message, is_dry_run, dimension
             FROM sync_records
             ORDER BY timestamp DESC, id DESC
             LIMIT ?1",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let map_row = |r: &rusqlite::Row| {
        let tracker_str: String = r.get(4)?;
        let cat_str: String = r.get(9)?;
        let is_dry: i64 = r.get(11)?;
        let dimension: Option<String> = r.get(12).ok();

        let tracker = SyncTarget::parse(&tracker_str).unwrap_or(SyncTarget::AniList);
        let category =
            SyncResultCategory::parse(&cat_str).unwrap_or(SyncResultCategory::RemoteError);

        Ok(SyncRecord {
            id: r.get(0)?,
            timestamp: r.get(1)?,
            komga_series_id: r.get(2)?,
            komga_series_title: r.get(3)?,
            tracker,
            tracker_media_id: r.get(5)?,
            old_progress: r.get(6)?,
            new_progress: r.get(7)?,
            dimension,
            status_change: r.get(8)?,
            category,
            message: r.get(10)?,
            is_dry_run: is_dry != 0,
        })
    };

    let rows: rusqlite::Result<Vec<SyncRecord>> = stmt.query_map([limit_i64], map_row)?.collect();
    rows.map_err(|e| AppError::internal(format!("Failed to fetch sync audit records: {e}")))
}

/// Clears all sync records from the database.
pub fn clear_records(conn: &Connection) -> Result<usize> {
    let deleted = conn
        .execute("DELETE FROM sync_records", [])
        .map_err(|e| AppError::internal(format!("Failed to clear sync records: {e}")))?;
    Ok(deleted)
}
