//! Tracked library catalog foundation.
//!
//! Distinctly models and separates:
//! - Tracked (followed in catalog)
//! - Owned (physically present in Komga or downloads)
//! - Acquiring (currently downloading in queue)
//! - Reading Progress (chapters/volumes read)
//! - Auto-Acquire (permission to automatically download future releases)
//!
//! Enforces that adding or tracking a manga NEVER implies downloading or auto-acquiring it.

use crate::{
    db::now,
    error::{AppError, Result},
    pipeline::mangabaka::normalize_title,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedCatalogEntry {
    pub id: i64,
    pub manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub title: String,
    pub origins: Vec<String>,
    pub auto_acquire: bool,
    pub tracked: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Unified title status model proving separation of concerns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedTitleStatus {
    pub title: String,
    pub komga_series_id: Option<String>,
    pub is_tracked: bool,
    pub is_owned: bool,
    pub owned_volumes_count: usize,
    pub is_acquiring: bool,
    pub auto_acquire_enabled: bool,
    pub reading_progress: Option<f64>,
    pub origins: Vec<String>,
}

fn parse_origins(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|_| vec!["mango".to_string()])
}

fn serialize_origins(origins: &[String]) -> String {
    serde_json::to_string(origins).unwrap_or_else(|_| "[\"mango\"]".to_string())
}

/// Adds or updates a tracked catalog entry.
///
/// If an entry already exists for the title/series, appends the new origin if not present.
/// Auto-acquire always strictly defaults to false.
pub fn add_or_update_tracked(
    conn: &Connection,
    title: &str,
    origin: &str,
    komga_series_id: Option<&str>,
    manga_id: Option<i64>,
) -> Result<TrackedCatalogEntry> {
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return Err(AppError::bad_request("Title cannot be empty"));
    }

    let norm_target = normalize_title(trimmed_title);

    // Look for existing row by komga_series_id or matching normalized title
    let mut stmt = conn
        .prepare(
            "SELECT id, manga_id, komga_series_id, title, origins, auto_acquire, tracked, created_at, updated_at
             FROM tracked_catalog",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let existing = stmt
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let m_id: Option<i64> = r.get(1)?;
            let k_id: Option<String> = r.get(2)?;
            let t: String = r.get(3)?;
            let origs_raw: String = r.get(4)?;
            let auto_acq: i64 = r.get(5)?;
            let trk: i64 = r.get(6)?;
            let created: i64 = r.get(7)?;
            let updated: i64 = r.get(8)?;

            Ok(TrackedCatalogEntry {
                id,
                manga_id: m_id,
                komga_series_id: k_id,
                title: t,
                origins: parse_origins(&origs_raw),
                auto_acquire: auto_acq != 0,
                tracked: trk != 0,
                created_at: created,
                updated_at: updated,
            })
        })
        .map_err(|e| AppError::internal(e.to_string()))?
        .flatten()
        .find(|item| {
            if let (Some(wanted_k), Some(existing_k)) = (komga_series_id, &item.komga_series_id) {
                if wanted_k == existing_k {
                    return true;
                }
            }
            normalize_title(&item.title) == norm_target
        });

    let current_time = now();

    if let Some(mut entry) = existing {
        if !entry.origins.contains(&origin.to_string()) {
            entry.origins.push(origin.to_string());
        }
        let orig_str = serialize_origins(&entry.origins);

        conn.execute(
            "UPDATE tracked_catalog SET
                origins = ?1,
                komga_series_id = COALESCE(komga_series_id, ?2),
                manga_id = COALESCE(manga_id, ?3),
                tracked = 1,
                updated_at = ?4
             WHERE id = ?5",
            params![orig_str, komga_series_id, manga_id, current_time, entry.id],
        )
        .map_err(|e| AppError::internal(format!("Failed to update tracked catalog entry: {e}")))?;

        entry.updated_at = current_time;
        entry.tracked = true;
        if entry.komga_series_id.is_none() {
            entry.komga_series_id = komga_series_id.map(ToString::to_string);
        }
        if entry.manga_id.is_none() {
            entry.manga_id = manga_id;
        }

        return Ok(entry);
    }

    // Insert new tracked entry with auto_acquire strictly set to 0 (false)
    let origins = vec![origin.to_string()];
    let orig_str = serialize_origins(&origins);

    conn.execute(
        "INSERT INTO tracked_catalog (manga_id, komga_series_id, title, origins, auto_acquire, tracked, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, 1, ?5, ?5)",
        params![manga_id, komga_series_id, trimmed_title, orig_str, current_time],
    )
    .map_err(|e| AppError::internal(format!("Failed to insert tracked catalog entry: {e}")))?;

    let new_id = conn.last_insert_rowid();

    Ok(TrackedCatalogEntry {
        id: new_id,
        manga_id,
        komga_series_id: komga_series_id.map(ToString::to_string),
        title: trimmed_title.to_string(),
        origins,
        auto_acquire: false, // Strictly false on creation
        tracked: true,
        created_at: current_time,
        updated_at: current_time,
    })
}

/// Explicitly configures auto-acquire permission for a tracked series.
pub fn set_auto_acquire(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    let affected = conn
        .execute(
            "UPDATE tracked_catalog SET auto_acquire = ?1, updated_at = ?2 WHERE id = ?3",
            params![if enabled { 1 } else { 0 }, now(), id],
        )
        .map_err(|e| AppError::internal(format!("Failed to set auto-acquire: {e}")))?;

    if affected == 0 {
        return Err(AppError::not_found("Tracked catalog entry not found"));
    }

    Ok(())
}

/// Toggles whether a series is actively tracked in the catalog.
pub fn set_tracked(conn: &Connection, id: i64, tracked: bool) -> Result<()> {
    let affected = conn
        .execute(
            "UPDATE tracked_catalog SET tracked = ?1, updated_at = ?2 WHERE id = ?3",
            params![if tracked { 1 } else { 0 }, now(), id],
        )
        .map_err(|e| AppError::internal(format!("Failed to set tracked: {e}")))?;

    if affected == 0 {
        return Err(AppError::not_found("Tracked catalog entry not found"));
    }

    Ok(())
}

/// Lists all tracked catalog entries.
pub fn list_tracked(conn: &Connection) -> Result<Vec<TrackedCatalogEntry>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, manga_id, komga_series_id, title, origins, auto_acquire, tracked, created_at, updated_at
             FROM tracked_catalog
             ORDER BY updated_at DESC, id DESC",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows: rusqlite::Result<Vec<TrackedCatalogEntry>> = stmt
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let m_id: Option<i64> = r.get(1)?;
            let k_id: Option<String> = r.get(2)?;
            let t: String = r.get(3)?;
            let origs_raw: String = r.get(4)?;
            let auto_acq: i64 = r.get(5)?;
            let trk: i64 = r.get(6)?;
            let created: i64 = r.get(7)?;
            let updated: i64 = r.get(8)?;

            Ok(TrackedCatalogEntry {
                id,
                manga_id: m_id,
                komga_series_id: k_id,
                title: t,
                origins: parse_origins(&origs_raw),
                auto_acquire: auto_acq != 0,
                tracked: trk != 0,
                created_at: created,
                updated_at: updated,
            })
        })?
        .collect();

    rows.map_err(|e| AppError::internal(format!("Failed to read tracked catalog: {e}")))
}

/// Fetches a single tracked catalog entry by ID.
pub fn get_tracked_by_id(conn: &Connection, id: i64) -> Result<Option<TrackedCatalogEntry>> {
    let entry = conn
        .query_row(
            "SELECT id, manga_id, komga_series_id, title, origins, auto_acquire, tracked, created_at, updated_at
             FROM tracked_catalog WHERE id = ?1",
            [id],
            |r| {
                let id: i64 = r.get(0)?;
                let m_id: Option<i64> = r.get(1)?;
                let k_id: Option<String> = r.get(2)?;
                let t: String = r.get(3)?;
                let origs_raw: String = r.get(4)?;
                let auto_acq: i64 = r.get(5)?;
                let trk: i64 = r.get(6)?;
                let created: i64 = r.get(7)?;
                let updated: i64 = r.get(8)?;

                Ok(TrackedCatalogEntry {
                    id,
                    manga_id: m_id,
                    komga_series_id: k_id,
                    title: t,
                    origins: parse_origins(&origs_raw),
                    auto_acquire: auto_acq != 0,
                    tracked: trk != 0,
                    created_at: created,
                    updated_at: updated,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(entry)
}
