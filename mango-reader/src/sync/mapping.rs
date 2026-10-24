//! Durable mapping between Komga series and external trackers (AniList, MangaBaka).

use super::model::{MappingConfidence, MappingSource, SyncTarget, TrackerMapping};
use crate::{
    db::now,
    error::{AppError, Result},
    pipeline::mangabaka::normalize_title,
    trackers::Kind,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

fn map_mapping_row(r: &Row) -> rusqlite::Result<TrackerMapping> {
    let tracker_str: String = r.get(1)?;
    let trk = SyncTarget::parse(&tracker_str).unwrap_or(SyncTarget::AniList);
    let manual_override: i64 = r.get(4)?;
    let source_str: Option<String> = r.get(6).ok();
    let edition_id: Option<String> = r.get(7).ok();
    let conf_str: Option<String> = r.get(8).ok();
    let last_validated: Option<i64> = r.get(9).ok();

    Ok(TrackerMapping {
        komga_series_id: r.get(0)?,
        tracker: trk,
        tracker_id: r.get(2)?,
        tracker_title: r.get(3)?,
        edition_id,
        mapping_source: source_str
            .as_deref()
            .map(MappingSource::parse)
            .unwrap_or(MappingSource::Manual),
        confidence: conf_str
            .as_deref()
            .map(MappingConfidence::parse)
            .unwrap_or(MappingConfidence::Resolved),
        manual_override: manual_override != 0,
        last_validated,
        updated_at: r.get(5)?,
    })
}

/// Fetches a tracker mapping for a Komga series if present.
pub fn get_mapping(
    conn: &Connection,
    komga_series_id: &str,
    tracker: SyncTarget,
) -> Result<Option<TrackerMapping>> {
    let row = conn
        .query_row(
            "SELECT komga_series_id, tracker, tracker_id, tracker_title, manual_override, updated_at,
                    mapping_source, edition_id, confidence, last_validated
             FROM tracker_mappings
             WHERE komga_series_id = ?1 AND tracker = ?2",
            params![komga_series_id, tracker.as_str()],
            map_mapping_row,
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Database error querying tracker mapping: {e}")))?;

    Ok(row)
}

/// Persists or updates a tracker mapping.
pub fn save_mapping(conn: &Connection, mapping: &TrackerMapping) -> Result<()> {
    conn.execute(
        "INSERT INTO tracker_mappings (
            komga_series_id, tracker, tracker_id, tracker_title, manual_override, updated_at,
            mapping_source, edition_id, confidence, last_validated
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT (komga_series_id, tracker) DO UPDATE SET
            tracker_id = excluded.tracker_id,
            tracker_title = excluded.tracker_title,
            manual_override = excluded.manual_override,
            updated_at = excluded.updated_at,
            mapping_source = excluded.mapping_source,
            edition_id = excluded.edition_id,
            confidence = excluded.confidence,
            last_validated = excluded.last_validated",
        params![
            mapping.komga_series_id,
            mapping.tracker.as_str(),
            mapping.tracker_id,
            mapping.tracker_title,
            if mapping.manual_override { 1 } else { 0 },
            mapping.updated_at,
            mapping.mapping_source.as_str(),
            mapping.edition_id,
            mapping.confidence.as_str(),
            mapping.last_validated,
        ],
    )
    .map_err(|e| AppError::internal(format!("Failed to save tracker mapping: {e}")))?;

    Ok(())
}

/// Deletes a tracker mapping. Returns true if a mapping was deleted.
pub fn delete_mapping(
    conn: &Connection,
    komga_series_id: &str,
    tracker: SyncTarget,
) -> Result<bool> {
    let affected = conn
        .execute(
            "DELETE FROM tracker_mappings WHERE komga_series_id = ?1 AND tracker = ?2",
            params![komga_series_id, tracker.as_str()],
        )
        .map_err(|e| AppError::internal(format!("Failed to delete tracker mapping: {e}")))?;

    Ok(affected > 0)
}

/// Lists all configured tracker mappings.
pub fn list_mappings(
    conn: &Connection,
    tracker: Option<SyncTarget>,
) -> Result<Vec<TrackerMapping>> {
    let mut stmt = if tracker.is_some() {
        conn.prepare(
            "SELECT komga_series_id, tracker, tracker_id, tracker_title, manual_override, updated_at,
                    mapping_source, edition_id, confidence, last_validated
             FROM tracker_mappings WHERE tracker = ?1 ORDER BY komga_series_id ASC",
        )
    } else {
        conn.prepare(
            "SELECT komga_series_id, tracker, tracker_id, tracker_title, manual_override, updated_at,
                    mapping_source, edition_id, confidence, last_validated
             FROM tracker_mappings ORDER BY komga_series_id ASC",
        )
    }
    .map_err(|e| AppError::internal(e.to_string()))?;

    let rows: rusqlite::Result<Vec<TrackerMapping>> = if let Some(t) = tracker {
        stmt.query_map([t.as_str()], map_mapping_row)?.collect()
    } else {
        stmt.query_map([], map_mapping_row)?.collect()
    };

    rows.map_err(|e| AppError::internal(format!("Failed to read tracker mappings: {e}")))
}

/// Resolves a tracker mapping for a Komga series from local cached data.
///
/// Precedence:
/// 1. Explicit entry in `tracker_mappings` table (manual override always wins).
/// 2. Existing Mango `tracks` entry linked to a local `manga` whose title matches `komga_series_title`.
pub fn resolve_local_mapping(
    conn: &Connection,
    komga_series_id: &str,
    komga_series_title: &str,
    tracker: SyncTarget,
) -> Result<Option<TrackerMapping>> {
    // 1. Direct mapping check
    if let Some(mapping) = get_mapping(conn, komga_series_id, tracker)? {
        return Ok(Some(mapping));
    }

    // 2. Mango `tracks` table fallback (for AniList)
    if tracker == SyncTarget::AniList {
        let anilist_kind = Kind::AniList.id();
        let norm_komga_title = normalize_title(komga_series_title);

        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.title, t.media_id, t.title
                 FROM manga m
                 JOIN tracks t ON m.id = t.manga_id
                 WHERE t.tracker = ?1 AND t.media_id > 0",
            )
            .map_err(|e| AppError::internal(e.to_string()))?;

        let candidate_rows = stmt.query_map([anilist_kind], |r| {
            let manga_id: i64 = r.get(0)?;
            let manga_title: String = r.get(1)?;
            let media_id: i64 = r.get(2)?;
            let track_title: String = r.get(3)?;
            Ok((manga_id, manga_title, media_id, track_title))
        });

        if let Ok(candidates) = candidate_rows {
            let mut matches = Vec::new();
            for cand in candidates.flatten() {
                let (_, manga_title, media_id, track_title) = cand;
                if normalize_title(&manga_title) == norm_komga_title {
                    matches.push((media_id, track_title, manga_title));
                }
            }

            // Ambiguity protection: if multiple tracks match the normalized title, fail closed!
            if matches.len() == 1 {
                let (media_id, track_title, manga_title) = matches.remove(0);
                let inferred = TrackerMapping {
                    komga_series_id: komga_series_id.to_string(),
                    tracker: SyncTarget::AniList,
                    tracker_id: media_id.to_string(),
                    tracker_title: if track_title.is_empty() {
                        Some(manga_title)
                    } else {
                        Some(track_title)
                    },
                    edition_id: None,
                    mapping_source: MappingSource::MangoTracker,
                    confidence: MappingConfidence::Resolved,
                    manual_override: false,
                    last_validated: Some(now()),
                    updated_at: now(),
                };

                let _ = save_mapping(conn, &inferred);
                return Ok(Some(inferred));
            }
        }
    }

    Ok(None)
}

/// Resolves a tracker mapping for a Komga series from persistent storage or local links.
pub fn resolve_mapping(
    conn: &Connection,
    komga_series_id: &str,
    komga_series_title: &str,
    tracker: SyncTarget,
) -> Result<Option<TrackerMapping>> {
    if let Some(existing) = get_mapping(conn, komga_series_id, tracker)? {
        if existing.manual_override || existing.confidence == MappingConfidence::Resolved {
            return Ok(Some(existing));
        }
    }

    resolve_local_mapping(conn, komga_series_id, komga_series_title, tracker)
}

/// Sets an explicit manual mapping override, which takes precedence over all automatic resolution.
pub fn set_manual_override(
    conn: &Connection,
    komga_series_id: &str,
    tracker: SyncTarget,
    tracker_id: &str,
    edition_id: Option<&str>,
    tracker_title: Option<&str>,
) -> Result<TrackerMapping> {
    let mapping = TrackerMapping {
        komga_series_id: komga_series_id.to_string(),
        tracker,
        tracker_id: tracker_id.trim().to_string(),
        tracker_title: tracker_title
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        edition_id: edition_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        mapping_source: MappingSource::Manual,
        confidence: MappingConfidence::Resolved,
        manual_override: true,
        last_validated: Some(now()),
        updated_at: now(),
    };

    save_mapping(conn, &mapping)?;
    Ok(mapping)
}

/// Clears an explicit manual mapping override.
pub fn clear_manual_override(
    conn: &Connection,
    komga_series_id: &str,
    tracker: SyncTarget,
) -> Result<()> {
    delete_mapping(conn, komga_series_id, tracker)?;
    Ok(())
}

/// Lists all unresolved or ambiguous tracker mappings.
pub fn list_unresolved_mappings(conn: &Connection) -> Result<Vec<TrackerMapping>> {
    let mut stmt = conn
        .prepare(
            "SELECT komga_series_id, tracker, tracker_id, tracker_title, manual_override, updated_at,
                    mapping_source, edition_id, confidence, last_validated
             FROM tracker_mappings
             WHERE manual_override = 0 AND confidence != 'resolved'
             ORDER BY updated_at DESC",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows: rusqlite::Result<Vec<TrackerMapping>> =
        stmt.query_map([], map_mapping_row)?.collect();
    rows.map_err(|e| AppError::internal(format!("Failed to list unresolved mappings: {e}")))
}
