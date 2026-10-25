//! Unified Tracked Lists Subsystem.
//!
//! Distinctly models, imports, caches, and unifies external tracker lists
//! (AniList and MangaBaka) into a first-class Tracked Catalog.
//!
//! Enforces:
//! - Complete separation from physical local ownership (Komga) and acquisition downloads.
//! - Non-destructive caching: API errors/rate-limits NEVER erase previously cached entries.
//! - Conservative identity resolution: manual > resolved mappings > exact normalized title.
//! - Fail-closed on ambiguity: ambiguous titles remain separate cards.
//! - Auto-acquire and download queues are strictly never touched on import.

use crate::{
    db::{Db, now},
    error::{AppError, Result},
    pipeline::{
        Pipeline,
        mangabaka::{MangaBakaClient, is_latin_script, is_safe_cover_url, normalize_title},
    },
    sync::{
        catalog::list_tracked,
        mapping::list_mappings,
        model::{KomgaSeriesProgress, MappingConfidence},
    },
    trackers::{Kind, Trackers, anilist},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Raw/cached entry from an external tracker list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerListEntry {
    pub tracker: String, // "anilist" | "mangabaka"
    pub tracker_id: String,
    pub title: String,
    pub english_title: Option<String>,
    pub romaji_title: Option<String>,
    pub native_title: Option<String>,
    pub aliases: Option<String>,
    pub cover_url: Option<String>,
    pub extra_large_cover_url: Option<String>,
    pub large_cover_url: Option<String>,
    pub medium_cover_url: Option<String>,
    pub status: String,            // native status string
    pub normalized_status: String, // "Reading", "Planning", "Completed", "Paused", "Dropped"
    pub progress_chapter: Option<f64>,
    pub progress_volume: Option<f64>,
    pub total_chapters: Option<i64>,
    pub total_volumes: Option<i64>,
    pub format: Option<String>,
    pub updated_at: i64,
    pub raw_data: Option<String>,
}

/// Status of tracker list refresh operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerListRefreshStatus {
    pub tracker: String,
    pub last_refreshed_at: i64,
    pub item_count: usize,
    pub status: String, // "ok", "error", "rate_limited", "unauthorized"
    pub error_message: Option<String>,
}

/// Provider-specific state preserved in unified catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerItemDetail {
    pub tracker_id: String,
    pub title: Option<String>,
    pub cover_url: Option<String>,
    pub status: String,
    pub normalized_status: String,
    pub progress_chapter: Option<f64>,
    pub progress_volume: Option<f64>,
    pub total_chapters: Option<i64>,
    pub total_volumes: Option<i64>,
    pub updated_at: i64,
}

/// Unified tracked title displayed on the Lists page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedTrackedItem {
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub normalized_status: String, // "Reading", "Planning", "Completed", "Paused", "Dropped"
    pub format: Option<String>,

    pub anilist: Option<TrackerItemDetail>,
    pub mangabaka: Option<TrackerItemDetail>,

    pub ownership_status: String, // "Owned", "Partially Owned", "Not Owned", "Unknown"
    pub owned_units: Option<usize>,
    pub total_units: Option<i64>,
    pub komga_series_id: Option<String>,

    pub acquisition_status: String, // "Idle", "Queued", "Acquiring", "Failed"
    pub auto_acquire: bool,

    pub updated_at: i64,
}

/// Deterministic display title resolver:
/// 1. English title (AniList english or MangaBaka english)
/// 2. AniList userPreferred (if Latin script)
/// 3. Romanized Latin script title (AniList romaji or MangaBaka romaji)
/// 4. Provider canonical title (if Latin script)
/// 5. AniList userPreferred (any script)
/// 6. Provider canonical title (any script)
/// 7. AniList romaji
/// 8. Native title (AniList native or MangaBaka native)
/// 9. Safe fallback
pub fn resolve_display_title(
    al_english: Option<&str>,
    al_user_preferred: Option<&str>,
    al_romaji: Option<&str>,
    al_native: Option<&str>,
    mb_english: Option<&str>,
    mb_romaji: Option<&str>,
    mb_canonical: Option<&str>,
    mb_native: Option<&str>,
    fallback: &str,
) -> String {
    // 1. English title
    if let Some(t) = al_english.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    if let Some(t) = mb_english.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    // 2. AniList userPreferred if Latin-script
    if let Some(t) = al_user_preferred.filter(|s| !s.trim().is_empty() && is_latin_script(s)) {
        return t.trim().to_string();
    }
    // 3. Romanized Latin-script title
    if let Some(t) = al_romaji.filter(|s| !s.trim().is_empty() && is_latin_script(s)) {
        return t.trim().to_string();
    }
    if let Some(t) = mb_romaji.filter(|s| !s.trim().is_empty() && is_latin_script(s)) {
        return t.trim().to_string();
    }
    // 4. Any provider title that is Latin-script
    if let Some(t) = mb_canonical.filter(|s| !s.trim().is_empty() && is_latin_script(s)) {
        return t.trim().to_string();
    }
    // 5. AniList userPreferred (even if non-Latin)
    if let Some(t) = al_user_preferred.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    // 6. MangaBaka canonical title
    if let Some(t) = mb_canonical.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    // 7. AniList romaji
    if let Some(t) = al_romaji.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    // 8. Native title
    if let Some(t) = al_native.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    if let Some(t) = mb_native.filter(|s| !s.trim().is_empty()) {
        return t.trim().to_string();
    }
    // 9. Fallback
    let fb = fallback.trim();
    if !fb.is_empty() {
        return fb.to_string();
    }
    "Unknown Title".to_string()
}

/// Deterministic cover selection:
/// 1. AniList extraLarge
/// 2. AniList large
/// 3. AniList cover_url
/// 4. AniList medium
/// 5. MangaBaka cover_url
/// 6. None
pub fn resolve_cover_url(
    al_xl: Option<&str>,
    al_l: Option<&str>,
    al_cover: Option<&str>,
    al_m: Option<&str>,
    mb_cover: Option<&str>,
) -> Option<String> {
    for candidate in [al_xl, al_l, al_cover, al_m, mb_cover] {
        if let Some(u) = candidate {
            let tr = u.trim();
            if !tr.is_empty() && is_safe_cover_url(tr) {
                return Some(tr.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedCatalogSummary {
    pub total: usize,
    pub reading: usize,
    pub planning: usize,
    pub completed: usize,
    pub paused: usize,
    pub dropped: usize,
    pub owned: usize,
    pub partially_owned: usize,
    pub not_owned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedCatalogResponse {
    pub items: Vec<UnifiedTrackedItem>,
    pub refresh_statuses: Vec<TrackerListRefreshStatus>,
    pub summary: UnifiedCatalogSummary,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListsQuery {
    pub status: Option<String>,
    pub ownership: Option<String>,
    pub tracker: Option<String>,
    pub q: Option<String>,
    pub sort: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RefreshListsPayload {
    pub tracker: Option<String>, // "anilist" | "mangabaka" | null (all)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListsRefreshReport {
    pub refreshed_trackers: Vec<String>,
    pub errors: Vec<String>,
    pub statuses: Vec<TrackerListRefreshStatus>,
}

/// Normalizes provider-native statuses into standard Mango categories.
pub fn normalize_status(tracker: &str, raw_status: &str) -> &'static str {
    let lower = raw_status.trim().to_lowercase();
    match tracker {
        "anilist" => match lower.as_str() {
            "current" | "repeating" => "Reading",
            "planning" => "Planning",
            "completed" => "Completed",
            "paused" => "Paused",
            "dropped" => "Dropped",
            _ => "Reading",
        },
        "mangabaka" => match lower.as_str() {
            "reading" | "rereading" => "Reading",
            "plan_to_read" | "considering" => "Planning",
            "completed" => "Completed",
            "paused" => "Paused",
            "dropped" => "Dropped",
            _ => "Reading",
        },
        _ => "Reading",
    }
}

/// Upserts imported tracker list entries into the local database cache.
pub fn upsert_tracker_list_entries(conn: &Connection, entries: &[TrackerListEntry]) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "INSERT INTO tracker_list_entries (
                tracker, tracker_id, title, english_title, romaji_title, native_title, aliases,
                cover_url, extra_large_cover_url, large_cover_url, medium_cover_url,
                status, normalized_status, progress_chapter, progress_volume,
                total_chapters, total_volumes, format, updated_at, raw_data
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            ON CONFLICT(tracker, tracker_id) DO UPDATE SET
                title = excluded.title,
                english_title = COALESCE(excluded.english_title, tracker_list_entries.english_title),
                romaji_title = COALESCE(excluded.romaji_title, tracker_list_entries.romaji_title),
                native_title = COALESCE(excluded.native_title, tracker_list_entries.native_title),
                aliases = COALESCE(excluded.aliases, tracker_list_entries.aliases),
                cover_url = COALESCE(excluded.cover_url, tracker_list_entries.cover_url),
                extra_large_cover_url = COALESCE(excluded.extra_large_cover_url, tracker_list_entries.extra_large_cover_url),
                large_cover_url = COALESCE(excluded.large_cover_url, tracker_list_entries.large_cover_url),
                medium_cover_url = COALESCE(excluded.medium_cover_url, tracker_list_entries.medium_cover_url),
                status = excluded.status,
                normalized_status = excluded.normalized_status,
                progress_chapter = excluded.progress_chapter,
                progress_volume = excluded.progress_volume,
                total_chapters = COALESCE(excluded.total_chapters, tracker_list_entries.total_chapters),
                total_volumes = COALESCE(excluded.total_volumes, tracker_list_entries.total_volumes),
                format = COALESCE(excluded.format, tracker_list_entries.format),
                updated_at = excluded.updated_at,
                raw_data = COALESCE(excluded.raw_data, tracker_list_entries.raw_data)",
        )
        .map_err(|e| AppError::internal(format!("Failed to prepare upsert: {e}")))?;

    for entry in entries {
        stmt.execute(params![
            entry.tracker,
            entry.tracker_id,
            entry.title,
            entry.english_title,
            entry.romaji_title,
            entry.native_title,
            entry.aliases,
            entry.cover_url,
            entry.extra_large_cover_url,
            entry.large_cover_url,
            entry.medium_cover_url,
            entry.status,
            entry.normalized_status,
            entry.progress_chapter,
            entry.progress_volume,
            entry.total_chapters,
            entry.total_volumes,
            entry.format,
            entry.updated_at,
            entry.raw_data,
        ])
        .map_err(|e| AppError::internal(format!("Failed to execute upsert: {e}")))?;
    }

    Ok(())
}

/// Reads cached tracker list entries from the database.
pub fn get_tracker_list_entries(
    conn: &Connection,
    tracker_filter: Option<&str>,
) -> Result<Vec<TrackerListEntry>> {
    let query = if let Some(trk) = tracker_filter {
        format!(
            "SELECT tracker, tracker_id, title, english_title, romaji_title, native_title, aliases,
                    cover_url, extra_large_cover_url, large_cover_url, medium_cover_url,
                    status, normalized_status, progress_chapter, progress_volume,
                    total_chapters, total_volumes, format, updated_at, raw_data
             FROM tracker_list_entries
             WHERE tracker = '{}'
             ORDER BY updated_at DESC",
            trk.replace('\'', "''")
        )
    } else {
        "SELECT tracker, tracker_id, title, english_title, romaji_title, native_title, aliases,
                cover_url, extra_large_cover_url, large_cover_url, medium_cover_url,
                status, normalized_status, progress_chapter, progress_volume,
                total_chapters, total_volumes, format, updated_at, raw_data
         FROM tracker_list_entries
         ORDER BY updated_at DESC"
            .to_string()
    };

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows = stmt
        .query_map([], |r| {
            Ok(TrackerListEntry {
                tracker: r.get(0)?,
                tracker_id: r.get(1)?,
                title: r.get(2)?,
                english_title: r.get(3)?,
                romaji_title: r.get(4)?,
                native_title: r.get(5)?,
                aliases: r.get(6)?,
                cover_url: r.get(7)?,
                extra_large_cover_url: r.get(8)?,
                large_cover_url: r.get(9)?,
                medium_cover_url: r.get(10)?,
                status: r.get(11)?,
                normalized_status: r.get(12)?,
                progress_chapter: r.get(13)?,
                progress_volume: r.get(14)?,
                total_chapters: r.get(15)?,
                total_volumes: r.get(16)?,
                format: r.get(17)?,
                updated_at: r.get(18)?,
                raw_data: r.get(19)?,
            })
        })
        .map_err(|e| AppError::internal(e.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(rows)
}

/// Updates the refresh status for a tracker.
pub fn update_refresh_status(conn: &Connection, status: &TrackerListRefreshStatus) -> Result<()> {
    conn.execute(
        "INSERT INTO tracker_list_refresh_status (tracker, last_refreshed_at, item_count, status, error_message)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(tracker) DO UPDATE SET
            last_refreshed_at = excluded.last_refreshed_at,
            item_count = excluded.item_count,
            status = excluded.status,
            error_message = excluded.error_message",
        params![
            status.tracker,
            status.last_refreshed_at,
            status.item_count,
            status.status,
            status.error_message,
        ],
    )
    .map_err(|e| AppError::internal(format!("Failed to record refresh status: {e}")))?;

    Ok(())
}

/// Retrieves all tracker list refresh statuses.
pub fn get_refresh_statuses(conn: &Connection) -> Result<Vec<TrackerListRefreshStatus>> {
    let mut stmt = conn
        .prepare(
            "SELECT tracker, last_refreshed_at, item_count, status, error_message
             FROM tracker_list_refresh_status",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows = stmt
        .query_map([], |r| {
            Ok(TrackerListRefreshStatus {
                tracker: r.get(0)?,
                last_refreshed_at: r.get(1)?,
                item_count: r.get(2)?,
                status: r.get(3)?,
                error_message: r.get(4)?,
            })
        })
        .map_err(|e| AppError::internal(e.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(rows)
}

/// Assembles the unified catalog from cached tracker list entries, DB mappings,
/// tracked catalog entries, and Komga series ownership.
pub fn build_unified_catalog(
    conn: &Connection,
    komga_series_list: &[KomgaSeriesProgress],
    query: &ListsQuery,
) -> Result<UnifiedCatalogResponse> {
    let raw_entries = get_tracker_list_entries(conn, None)?;
    let refresh_statuses = get_refresh_statuses(conn)?;
    let mappings = list_mappings(conn, None)?;
    let tracked_rows = list_tracked(conn).unwrap_or_default();

    // Map komga_series_id -> KomgaSeriesProgress
    let komga_map: HashMap<&str, &KomgaSeriesProgress> = komga_series_list
        .iter()
        .map(|s| (s.series_id.as_str(), s))
        .collect();

    // Komga series title lookup by normalized title
    let mut komga_by_norm_title: HashMap<String, &KomgaSeriesProgress> = HashMap::new();
    for s in komga_series_list {
        let norm = normalize_title(&s.series_title);
        komga_by_norm_title.entry(norm).or_insert(s);
    }

    // Mappings: (tracker, tracker_id) -> komga_series_id
    let mut tracker_to_komga: HashMap<(String, String), String> = HashMap::new();
    for m in &mappings {
        if m.confidence != MappingConfidence::Ambiguous {
            let trk_str = m.tracker.to_string();
            tracker_to_komga.insert((trk_str, m.tracker_id.clone()), m.komga_series_id.clone());
        }
    }

    // Separate by tracker
    let mut anilist_entries: Vec<TrackerListEntry> = Vec::new();
    let mut mangabaka_entries: Vec<TrackerListEntry> = Vec::new();

    for entry in raw_entries {
        if entry.tracker == "anilist" {
            anilist_entries.push(entry);
        } else if entry.tracker == "mangabaka" {
            mangabaka_entries.push(entry);
        }
    }

    // Track which mangabaka entries got merged
    let mut merged_mb_ids: HashMap<String, bool> = HashMap::new();
    let mut unified_items: Vec<UnifiedTrackedItem> = Vec::new();

    // 1. Process AniList entries and attempt conservative merge with MangaBaka
    for al in &anilist_entries {
        let al_komga = tracker_to_komga.get(&("anilist".to_string(), al.tracker_id.clone()));
        let al_norm = normalize_title(&al.title);

        // Build normalized title/alias set for AniList entry
        let mut al_norm_set: Vec<String> = Vec::new();
        for t in [&Some(al.title.clone()), &al.english_title, &al.romaji_title, &al.native_title] {
            if let Some(s) = t {
                let n = normalize_title(s);
                if !n.is_empty() && !al_norm_set.contains(&n) {
                    al_norm_set.push(n);
                }
            }
        }
        if let Some(ref al_alias_json) = al.aliases {
            if let Ok(aliases) = serde_json::from_str::<Vec<String>>(al_alias_json) {
                for a in aliases {
                    let n = normalize_title(&a);
                    if !n.is_empty() && !al_norm_set.contains(&n) {
                        al_norm_set.push(n);
                    }
                }
            }
        }

        // Find candidate MangaBaka entry
        // Conservative criteria:
        // A. Both mapped to the same Komga series ID
        // B. Exact normalized title or alias match (only if unique candidate, fail-closed on ambiguity)
        let mut matched_mb: Option<&TrackerListEntry> = None;

        if let Some(k_id) = al_komga {
            for mb in &mangabaka_entries {
                if merged_mb_ids.contains_key(&mb.tracker_id) {
                    continue;
                }
                if let Some(mb_k) =
                    tracker_to_komga.get(&("mangabaka".to_string(), mb.tracker_id.clone()))
                {
                    if mb_k == k_id {
                        matched_mb = Some(mb);
                        break;
                    }
                }
            }
        }

        if matched_mb.is_none() {
            // Check exact normalized title or alias match across all known titles
            let matching_candidates: Vec<&TrackerListEntry> = mangabaka_entries
                .iter()
                .filter(|mb| {
                    if merged_mb_ids.contains_key(&mb.tracker_id) {
                        return false;
                    }
                    let mut mb_norm_set: Vec<String> = Vec::new();
                    for t in [&Some(mb.title.clone()), &mb.english_title, &mb.romaji_title, &mb.native_title] {
                        if let Some(s) = t {
                            let n = normalize_title(s);
                            if !n.is_empty() && !mb_norm_set.contains(&n) {
                                mb_norm_set.push(n);
                            }
                        }
                    }
                    if let Some(ref mb_alias_json) = mb.aliases {
                        if let Ok(aliases) = serde_json::from_str::<Vec<String>>(mb_alias_json) {
                            for a in aliases {
                                let n = normalize_title(&a);
                                if !n.is_empty() && !mb_norm_set.contains(&n) {
                                    mb_norm_set.push(n);
                                }
                            }
                        }
                    }
                    mb_norm_set.iter().any(|m_norm| al_norm_set.contains(m_norm))
                })
                .collect();

            // Fail-closed on ambiguity: exactly one candidate required
            if matching_candidates.len() == 1 {
                matched_mb = Some(matching_candidates[0]);
            }
        }

        if let Some(mb) = matched_mb {
            merged_mb_ids.insert(mb.tracker_id.clone(), true);
        }

        // Determine Komga match
        let komga_prog = al_komga
            .and_then(|k| komga_map.get(k.as_str()).copied())
            .or_else(|| komga_by_norm_title.get(&al_norm).copied());

        let komga_id = komga_prog.map(|p| p.series_id.clone());

        let total_units = al
            .total_volumes
            .or(al.total_chapters)
            .or_else(|| matched_mb.and_then(|m| m.total_volumes.or(m.total_chapters)));

        // Ownership calculation
        let (ownership_status, owned_units) = if let Some(kp) = komga_prog {
            let owned_count = kp.total_books;
            let status = if owned_count == 0 {
                "Not Owned"
            } else if let Some(tot) = total_units {
                if tot > 0 && owned_count >= tot as usize {
                    "Owned"
                } else {
                    "Partially Owned"
                }
            } else {
                "Owned"
            };
            (status.to_string(), Some(owned_count))
        } else {
            ("Not Owned".to_string(), None)
        };

        // Normalized status determination
        let norm_status = if let Some(kp) = komga_prog {
            if kp.completed_books > 0
                || kp.highest_completed_volume.unwrap_or(0.0) > 0.0
                || kp.highest_completed_chapter.unwrap_or(0.0) > 0.0
            {
                if al.normalized_status == "Completed"
                    && (matched_mb.is_none()
                        || matched_mb.map_or(false, |m| m.normalized_status == "Completed"))
                {
                    "Completed".to_string()
                } else if al.normalized_status == "Paused"
                    && matched_mb.map_or(true, |m| m.normalized_status == "Paused")
                {
                    "Paused".to_string()
                } else if al.normalized_status == "Dropped"
                    && matched_mb.map_or(true, |m| m.normalized_status == "Dropped")
                {
                    "Dropped".to_string()
                } else {
                    "Reading".to_string()
                }
            } else {
                pick_normalized_status(
                    &al.normalized_status,
                    matched_mb.map(|m| m.normalized_status.as_str()),
                )
            }
        } else {
            pick_normalized_status(
                &al.normalized_status,
                matched_mb.map(|m| m.normalized_status.as_str()),
            )
        };

        // Tracked catalog entry
        let tracked_entry = tracked_rows.iter().find(|t| {
            if let (Some(k1), Some(k2)) = (&komga_id, &t.komga_series_id) {
                if k1 == k2 {
                    return true;
                }
            }
            normalize_title(&t.title) == al_norm
        });

        let auto_acquire = tracked_entry.map_or(false, |t| t.auto_acquire);

        let al_detail = TrackerItemDetail {
            tracker_id: al.tracker_id.clone(),
            title: Some(al.title.clone()),
            cover_url: al.cover_url.clone(),
            status: al.status.clone(),
            normalized_status: al.normalized_status.clone(),
            progress_chapter: al.progress_chapter,
            progress_volume: al.progress_volume,
            total_chapters: al.total_chapters,
            total_volumes: al.total_volumes,
            updated_at: al.updated_at,
        };

        let mb_detail = matched_mb.map(|mb| TrackerItemDetail {
            tracker_id: mb.tracker_id.clone(),
            title: Some(mb.title.clone()),
            cover_url: mb.cover_url.clone(),
            status: mb.status.clone(),
            normalized_status: mb.normalized_status.clone(),
            progress_chapter: mb.progress_chapter,
            progress_volume: mb.progress_volume,
            total_chapters: mb.total_chapters,
            total_volumes: mb.total_volumes,
            updated_at: mb.updated_at,
        });

        let composite_id = if let Some(mb) = matched_mb {
            format!("al-{}_mb-{}", al.tracker_id, mb.tracker_id)
        } else {
            format!("al-{}", al.tracker_id)
        };

        let cover_url = resolve_cover_url(
            al.extra_large_cover_url.as_deref(),
            al.large_cover_url.as_deref(),
            al.cover_url.as_deref(),
            al.medium_cover_url.as_deref(),
            matched_mb.and_then(|m| m.cover_url.as_deref()),
        );

        let display_title = resolve_display_title(
            al.english_title.as_deref(),
            None,
            al.romaji_title.as_deref(),
            al.native_title.as_deref(),
            matched_mb.and_then(|m| m.english_title.as_deref()),
            matched_mb.and_then(|m| m.romaji_title.as_deref()),
            matched_mb.map(|m| m.title.as_str()),
            matched_mb.and_then(|m| m.native_title.as_deref()),
            &al.title,
        );

        let mut unified_aliases: Vec<String> = Vec::new();
        for t in [&Some(al.title.clone()), &al.english_title, &al.romaji_title, &al.native_title] {
            if let Some(s) = t {
                let tr = s.trim().to_string();
                if !tr.is_empty() && !unified_aliases.contains(&tr) {
                    unified_aliases.push(tr);
                }
            }
        }
        if let Some(ref al_alias_json) = al.aliases {
            if let Ok(aliases) = serde_json::from_str::<Vec<String>>(al_alias_json) {
                for a in aliases {
                    let tr = a.trim().to_string();
                    if !tr.is_empty() && !unified_aliases.contains(&tr) {
                        unified_aliases.push(tr);
                    }
                }
            }
        }
        if let Some(mb) = matched_mb {
            for t in [&Some(mb.title.clone()), &mb.english_title, &mb.romaji_title, &mb.native_title] {
                if let Some(s) = t {
                    let tr = s.trim().to_string();
                    if !tr.is_empty() && !unified_aliases.contains(&tr) {
                        unified_aliases.push(tr);
                    }
                }
            }
            if let Some(ref mb_alias_json) = mb.aliases {
                if let Ok(aliases) = serde_json::from_str::<Vec<String>>(mb_alias_json) {
                    for a in aliases {
                        let tr = a.trim().to_string();
                        if !tr.is_empty() && !unified_aliases.contains(&tr) {
                            unified_aliases.push(tr);
                        }
                    }
                }
            }
        }

        let format = al
            .format
            .clone()
            .or_else(|| matched_mb.and_then(|m| m.format.clone()));
        let updated_at = al.updated_at.max(matched_mb.map_or(0, |m| m.updated_at));

        unified_items.push(UnifiedTrackedItem {
            id: composite_id,
            title: display_title,
            cover_url,
            aliases: unified_aliases,
            normalized_status: norm_status,
            format,
            anilist: Some(al_detail),
            mangabaka: mb_detail,
            ownership_status,
            owned_units,
            total_units,
            komga_series_id: komga_id,
            acquisition_status: "Idle".to_string(),
            auto_acquire,
            updated_at,
        });
    }

    // 2. Process remaining unmerged MangaBaka entries
    for mb in &mangabaka_entries {
        if merged_mb_ids.contains_key(&mb.tracker_id) {
            continue;
        }

        let mb_komga = tracker_to_komga.get(&("mangabaka".to_string(), mb.tracker_id.clone()));
        let mb_norm = normalize_title(&mb.title);

        let komga_prog = mb_komga
            .and_then(|k| komga_map.get(k.as_str()).copied())
            .or_else(|| komga_by_norm_title.get(&mb_norm).copied());

        let komga_id = komga_prog.map(|p| p.series_id.clone());
        let total_units = mb.total_volumes.or(mb.total_chapters);

        let (ownership_status, owned_units) = if let Some(kp) = komga_prog {
            let owned_count = kp.total_books;
            let status = if owned_count == 0 {
                "Not Owned"
            } else if let Some(tot) = total_units {
                if tot > 0 && owned_count >= tot as usize {
                    "Owned"
                } else {
                    "Partially Owned"
                }
            } else {
                "Owned"
            };
            (status.to_string(), Some(owned_count))
        } else {
            ("Not Owned".to_string(), None)
        };

        let norm_status = if let Some(kp) = komga_prog {
            if kp.completed_books > 0
                || kp.highest_completed_volume.unwrap_or(0.0) > 0.0
                || kp.highest_completed_chapter.unwrap_or(0.0) > 0.0
            {
                if mb.normalized_status == "Completed" {
                    "Completed".to_string()
                } else if mb.normalized_status == "Paused" {
                    "Paused".to_string()
                } else if mb.normalized_status == "Dropped" {
                    "Dropped".to_string()
                } else {
                    "Reading".to_string()
                }
            } else {
                mb.normalized_status.clone()
            }
        } else {
            mb.normalized_status.clone()
        };

        let tracked_entry = tracked_rows.iter().find(|t| {
            if let (Some(k1), Some(k2)) = (&komga_id, &t.komga_series_id) {
                if k1 == k2 {
                    return true;
                }
            }
            normalize_title(&t.title) == mb_norm
        });

        let auto_acquire = tracked_entry.map_or(false, |t| t.auto_acquire);

        let mb_detail = TrackerItemDetail {
            tracker_id: mb.tracker_id.clone(),
            title: Some(mb.title.clone()),
            cover_url: mb.cover_url.clone(),
            status: mb.status.clone(),
            normalized_status: mb.normalized_status.clone(),
            progress_chapter: mb.progress_chapter,
            progress_volume: mb.progress_volume,
            total_chapters: mb.total_chapters,
            total_volumes: mb.total_volumes,
            updated_at: mb.updated_at,
        };

        let display_title = resolve_display_title(
            None,
            None,
            None,
            None,
            mb.english_title.as_deref(),
            mb.romaji_title.as_deref(),
            Some(&mb.title),
            mb.native_title.as_deref(),
            &mb.title,
        );

        let cover_url = resolve_cover_url(
            None,
            None,
            None,
            None,
            mb.cover_url.as_deref(),
        );

        let mut unified_aliases: Vec<String> = Vec::new();
        for t in [&Some(mb.title.clone()), &mb.english_title, &mb.romaji_title, &mb.native_title] {
            if let Some(s) = t {
                let tr = s.trim().to_string();
                if !tr.is_empty() && !unified_aliases.contains(&tr) {
                    unified_aliases.push(tr);
                }
            }
        }
        if let Some(ref mb_alias_json) = mb.aliases {
            if let Ok(aliases) = serde_json::from_str::<Vec<String>>(mb_alias_json) {
                for a in aliases {
                    let tr = a.trim().to_string();
                    if !tr.is_empty() && !unified_aliases.contains(&tr) {
                        unified_aliases.push(tr);
                    }
                }
            }
        }

        unified_items.push(UnifiedTrackedItem {
            id: format!("mb-{}", mb.tracker_id),
            title: display_title,
            cover_url,
            aliases: unified_aliases,
            normalized_status: norm_status,
            format: mb.format.clone(),
            anilist: None,
            mangabaka: Some(mb_detail),
            ownership_status,
            owned_units,
            total_units,
            komga_series_id: komga_id,
            acquisition_status: "Idle".to_string(),
            auto_acquire,
            updated_at: mb.updated_at,
        });
    }

    // Compute summary
    let mut summary = UnifiedCatalogSummary {
        total: unified_items.len(),
        ..Default::default()
    };

    for item in &unified_items {
        match item.normalized_status.as_str() {
            "Reading" => summary.reading += 1,
            "Planning" => summary.planning += 1,
            "Completed" => summary.completed += 1,
            "Paused" => summary.paused += 1,
            "Dropped" => summary.dropped += 1,
            _ => summary.reading += 1,
        }

        match item.ownership_status.as_str() {
            "Owned" => summary.owned += 1,
            "Partially Owned" => summary.partially_owned += 1,
            _ => summary.not_owned += 1,
        }
    }

    // Filter items based on query
    let filtered: Vec<UnifiedTrackedItem> = unified_items
        .into_iter()
        .filter(|item| {
            if let Some(ref st) = query.status {
                if !st.is_empty() && st != "all" && !item.normalized_status.eq_ignore_ascii_case(st)
                {
                    return false;
                }
            }
            if let Some(ref ow) = query.ownership {
                let ow_norm = ow.to_lowercase().replace('_', " ");
                if !ow.is_empty()
                    && ow != "all"
                    && !item.ownership_status.to_lowercase().eq(&ow_norm)
                {
                    return false;
                }
            }
            if let Some(ref tr) = query.tracker {
                match tr.to_lowercase().as_str() {
                    "anilist" => {
                        if item.anilist.is_none() {
                            return false;
                        }
                    }
                    "mangabaka" => {
                        if item.mangabaka.is_none() {
                            return false;
                        }
                    }
                    "both" => {
                        if item.anilist.is_none() || item.mangabaka.is_none() {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(ref q) = query.q {
                let trimmed = q.trim().to_lowercase();
                if !trimmed.is_empty() {
                    let match_title = item.title.to_lowercase().contains(&trimmed);
                    let match_alias = item.aliases.iter().any(|a| a.to_lowercase().contains(&trimmed));
                    if !match_title && !match_alias {
                        return false;
                    }
                }
            }
            true
        })
        .collect();

    // Sort items
    let mut sorted = filtered;
    let sort_key = query.sort.as_deref().unwrap_or("updated");
    match sort_key {
        "title" => {
            sorted.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        }
        "progress" => {
            sorted.sort_by(|a, b| {
                let a_p = a
                    .anilist
                    .as_ref()
                    .and_then(|x| x.progress_volume.or(x.progress_chapter))
                    .or_else(|| {
                        a.mangabaka
                            .as_ref()
                            .and_then(|x| x.progress_volume.or(x.progress_chapter))
                    })
                    .unwrap_or(0.0);
                let b_p = b
                    .anilist
                    .as_ref()
                    .and_then(|x| x.progress_volume.or(x.progress_chapter))
                    .or_else(|| {
                        b.mangabaka
                            .as_ref()
                            .and_then(|x| x.progress_volume.or(x.progress_chapter))
                    })
                    .unwrap_or(0.0);
                b_p.partial_cmp(&a_p).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        _ => {
            // Default "updated" DESC
            sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        }
    }

    Ok(UnifiedCatalogResponse {
        items: sorted,
        refresh_statuses,
        summary,
    })
}

fn pick_normalized_status(s1: &str, s2: Option<&str>) -> String {
    let Some(s2) = s2 else {
        return s1.to_string();
    };
    if s1 == "Reading" || s2 == "Reading" {
        "Reading".to_string()
    } else if s1 == "Planning" || s2 == "Planning" {
        "Planning".to_string()
    } else if s1 == "Paused" || s2 == "Paused" {
        "Paused".to_string()
    } else if s1 == "Completed" && s2 == "Completed" {
        "Completed".to_string()
    } else if s1 == "Dropped" && s2 == "Dropped" {
        "Dropped".to_string()
    } else {
        s1.to_string()
    }
}

/// Orchestrates on-demand refresh of AniList and/or MangaBaka lists.
///
/// Guaranteed safety invariants:
/// - Never deletes cached entries on remote failures.
/// - Never queues downloads or enables auto-acquire.
/// - Rate safe with appropriate status recording.
pub async fn refresh_tracker_lists(
    db: &Db,
    trackers: &Trackers,
    pipeline: &Pipeline,
    target_tracker: Option<&str>,
) -> Result<ListsRefreshReport> {
    let mut refreshed_trackers = Vec::new();
    let mut errors = Vec::new();

    let sync_al = target_tracker.is_none() || target_tracker == Some("anilist");
    let sync_mb = target_tracker.is_none() || target_tracker == Some("mangabaka");

    // 1. Refresh AniList
    if sync_al {
        match trackers.account(Kind::AniList).await {
            Ok(account) => match anilist::fetch_user_manga_list(trackers.http(), &account).await {
                Ok(entries) => {
                    let count = entries.len();
                    let mapped_entries: Vec<TrackerListEntry> = entries
                        .into_iter()
                        .map(|e| {
                            let norm = normalize_status("anilist", &e.status).to_string();
                            let aliases_json = if e.aliases.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&e.aliases).ok()
                            };
                            TrackerListEntry {
                                tracker: "anilist".to_string(),
                                tracker_id: e.media_id.to_string(),
                                title: e.title,
                                english_title: e.english_title,
                                romaji_title: e.romaji_title,
                                native_title: e.native_title,
                                aliases: aliases_json,
                                cover_url: e.cover_url,
                                extra_large_cover_url: e.extra_large_cover_url,
                                large_cover_url: e.large_cover_url,
                                medium_cover_url: e.medium_cover_url,
                                status: e.status,
                                normalized_status: norm,
                                progress_chapter: Some(e.progress_chapter),
                                progress_volume: Some(e.progress_volume),
                                total_chapters: e.total_chapters,
                                total_volumes: e.total_volumes,
                                format: e.format,
                                updated_at: e.updated_at,
                                raw_data: None,
                            }
                        })
                        .collect();

                    let db_clone = db.clone();
                    let save_res = db_clone
                        .call(move |conn| {
                            upsert_tracker_list_entries(conn, &mapped_entries)?;
                            update_refresh_status(
                                conn,
                                &TrackerListRefreshStatus {
                                    tracker: "anilist".to_string(),
                                    last_refreshed_at: now(),
                                    item_count: count,
                                    status: "ok".to_string(),
                                    error_message: None,
                                },
                            )?;
                            Ok(())
                        })
                        .await;

                    if let Err(e) = save_res {
                        errors.push(format!("AniList DB save error: {e}"));
                    } else {
                        refreshed_trackers.push("anilist".to_string());
                    }
                }
                Err(e) => {
                    let msg = format!("AniList refresh failed: {e}");
                    errors.push(msg.clone());
                    let db_clone = db.clone();
                    let _ = db_clone
                        .call(move |conn| {
                            update_refresh_status(
                                conn,
                                &TrackerListRefreshStatus {
                                    tracker: "anilist".to_string(),
                                    last_refreshed_at: now(),
                                    item_count: 0,
                                    status: "error".to_string(),
                                    error_message: Some(msg),
                                },
                            )
                        })
                        .await;
                }
            },
            Err(e) => {
                let msg = format!("AniList not configured: {e}");
                errors.push(msg.clone());
                let db_clone = db.clone();
                let _ = db_clone
                    .call(move |conn| {
                        update_refresh_status(
                            conn,
                            &TrackerListRefreshStatus {
                                tracker: "anilist".to_string(),
                                last_refreshed_at: now(),
                                item_count: 0,
                                status: "unauthorized".to_string(),
                                error_message: Some(msg),
                            },
                        )
                    })
                    .await;
            }
        }
    }

    // 2. Refresh MangaBaka
    if sync_mb {
        let (cfg, mangabaka_token, _) = pipeline.load_internal_config().await?;
        if let Some(token) = mangabaka_token.filter(|t| !t.trim().is_empty()) {
            let mb_client =
                MangaBakaClient::new(reqwest::Client::new(), cfg.mangabaka_url, Some(token));

            match mb_client.fetch_my_library().await {
                Ok(items) => {
                    let count = items.len();
                    let mapped_entries: Vec<TrackerListEntry> = items
                        .into_iter()
                        .map(|item| {
                            let norm = normalize_status("mangabaka", &item.state).to_string();
                            let aliases_json = if item.aliases.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&item.aliases).ok()
                            };
                            TrackerListEntry {
                                tracker: "mangabaka".to_string(),
                                tracker_id: item.series_id.to_string(),
                                title: item.title,
                                english_title: item.english_title,
                                romaji_title: item.romaji_title,
                                native_title: item.native_title,
                                aliases: aliases_json,
                                cover_url: item.cover_url,
                                extra_large_cover_url: None,
                                large_cover_url: None,
                                medium_cover_url: None,
                                status: item.state,
                                normalized_status: norm,
                                progress_chapter: item.progress_chapter,
                                progress_volume: item.progress_volume,
                                total_chapters: item.total_chapters,
                                total_volumes: item.total_volumes,
                                format: item.format,
                                updated_at: item.updated_at,
                                raw_data: None,
                            }
                        })
                        .collect();

                    let db_clone = db.clone();
                    let save_res = db_clone
                        .call(move |conn| {
                            upsert_tracker_list_entries(conn, &mapped_entries)?;
                            update_refresh_status(
                                conn,
                                &TrackerListRefreshStatus {
                                    tracker: "mangabaka".to_string(),
                                    last_refreshed_at: now(),
                                    item_count: count,
                                    status: "ok".to_string(),
                                    error_message: None,
                                },
                            )?;
                            Ok(())
                        })
                        .await;

                    if let Err(e) = save_res {
                        errors.push(format!("MangaBaka DB save error: {e}"));
                    } else {
                        refreshed_trackers.push("mangabaka".to_string());
                    }
                }
                Err(e) => {
                    let status_str = if e.code == "mangabaka_rate_limited" {
                        "rate_limited"
                    } else if e.code == "mangabaka_unauthorized" {
                        "unauthorized"
                    } else {
                        "error"
                    };
                    let msg = format!("MangaBaka refresh failed: {e}");
                    errors.push(msg.clone());
                    let db_clone = db.clone();
                    let _ = db_clone
                        .call(move |conn| {
                            update_refresh_status(
                                conn,
                                &TrackerListRefreshStatus {
                                    tracker: "mangabaka".to_string(),
                                    last_refreshed_at: now(),
                                    item_count: 0,
                                    status: status_str.to_string(),
                                    error_message: Some(msg),
                                },
                            )
                        })
                        .await;
                }
            }
        } else {
            let msg = "MangaBaka PAT token not configured".to_string();
            errors.push(msg.clone());
            let db_clone = db.clone();
            let _ = db_clone
                .call(move |conn| {
                    update_refresh_status(
                        conn,
                        &TrackerListRefreshStatus {
                            tracker: "mangabaka".to_string(),
                            last_refreshed_at: now(),
                            item_count: 0,
                            status: "unauthorized".to_string(),
                            error_message: Some(msg),
                        },
                    )
                })
                .await;
        }
    }

    let statuses = db
        .call(|conn| get_refresh_statuses(conn))
        .await
        .unwrap_or_default();

    Ok(ListsRefreshReport {
        refreshed_trackers,
        errors,
        statuses,
    })
}
