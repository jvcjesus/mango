//! Reading-progress synchronization adapter for AniList.
//!
//! Dimension-aware:
//! - Volume-based Komga series syncs `progressVolumes` directly to AniList.
//! - Chapter-based Komga series syncs `progress` (chapters) to AniList.
//! - Never equates Volume 12 = Chapter 12.
//! - Never decreases progress: `target = max(existing_anilist, komga)`.
//! - Only marks COMPLETED if authoritative total is known and reached.

use super::model::{
    KomgaSeriesProgress, SyncRecord, SyncResultCategory, SyncTarget, TrackerMapping,
};
use crate::{
    db::now,
    trackers::{Kind, Trackers, anilist},
};

pub async fn sync_series_to_anilist(
    trackers: &Trackers,
    progress: &KomgaSeriesProgress,
    mapping: Option<&TrackerMapping>,
    dry_run: bool,
) -> SyncRecord {
    let series_id = &progress.series_id;
    let series_title = &progress.series_title;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let record_id = format!("sync-{}-{}-{}", SyncTarget::AniList, series_id, nanos);

    // Determine dimension
    let is_volume =
        progress.highest_completed_volume.is_some() && progress.highest_completed_chapter.is_none();
    let dimension = if is_volume { "volume" } else { "chapter" };

    // 1. Mapping check
    let Some(map) = mapping else {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: None,
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::SkippedNoMapping,
            message: "Series has no AniList mapping".to_string(),
            is_dry_run: dry_run,
        };
    };

    let media_id = match map.tracker_id.parse::<i64>() {
        Ok(id) if id > 0 => id,
        _ => {
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::AniList,
                tracker_media_id: Some(map.tracker_id.clone()),
                old_progress: None,
                new_progress: None,
                dimension: Some(dimension.to_string()),
                status_change: None,
                category: SyncResultCategory::AmbiguousMapping,
                message: format!("Invalid or ambiguous AniList media ID: {}", map.tracker_id),
                is_dry_run: dry_run,
            };
        }
    };

    // 2. Untouched check
    let (komga_prog, dim_unit) = if is_volume {
        (progress.highest_completed_volume.unwrap_or(0.0), "Vol")
    } else {
        (progress.highest_completed_chapter.unwrap_or(0.0), "Ch")
    };

    if komga_prog == 0.0 && progress.completed_books == 0 {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: Some(media_id.to_string()),
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::SkippedUntouched,
            message: "Series has not been read in Komga; skipping tracker synchronization"
                .to_string(),
            is_dry_run: dry_run,
        };
    }

    // 3. Load AniList account
    let account = match trackers.account(Kind::AniList).await {
        Ok(acc) => acc,
        Err(e) => {
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::AniList,
                tracker_media_id: Some(media_id.to_string()),
                old_progress: None,
                new_progress: None,
                dimension: Some(dimension.to_string()),
                status_change: None,
                category: SyncResultCategory::AuthError,
                message: format!("AniList authentication error: {e}"),
                is_dry_run: dry_run,
            };
        }
    };

    // 4. Query current AniList entry
    let (old_progress, old_status, total_units) =
        match anilist::find(trackers.http(), &account, media_id).await {
            Ok(Some(entry)) => {
                if is_volume {
                    (entry.last_volume_read, entry.status, entry.total_volumes)
                } else {
                    (entry.last_chapter_read, entry.status, entry.total_chapters)
                }
            }
            Ok(None) => (0.0, 1, 0), // 1 = CURRENT in AniList
            Err(e) => {
                return SyncRecord {
                    id: record_id,
                    timestamp: now(),
                    komga_series_id: series_id.clone(),
                    komga_series_title: series_title.clone(),
                    tracker: SyncTarget::AniList,
                    tracker_media_id: Some(media_id.to_string()),
                    old_progress: None,
                    new_progress: None,
                    dimension: Some(dimension.to_string()),
                    status_change: None,
                    category: SyncResultCategory::NetworkError,
                    message: format!("Failed to fetch AniList entry: {e}"),
                    is_dry_run: dry_run,
                };
            }
        };

    // 5. Apply never-decrease rule: target = max(existing, komga)
    let target_prog = old_progress.max(komga_prog);

    // Completion rule: only mark series COMPLETED if authoritative total is known and reached
    let is_completed = if total_units > 0 && target_prog >= total_units as f64 {
        true
    } else if progress.is_series_completed && total_units > 0 {
        true
    } else {
        false
    };

    let new_status = if is_completed { 2 } else { 1 }; // 2 = COMPLETED, 1 = CURRENT
    let status_change = if new_status != old_status {
        Some(if new_status == 2 {
            "COMPLETED".to_string()
        } else {
            "CURRENT".to_string()
        })
    } else {
        None
    };

    let needs_progress_update = target_prog > old_progress;
    let needs_status_update = is_completed && old_status != 2;

    if !needs_progress_update && !needs_status_update {
        let (cat, msg) = if old_progress > komga_prog {
            (
                SyncResultCategory::TrackerAhead,
                format!(
                    "AniList is ahead ({dim_unit} {old_progress}) of Komga progress ({dim_unit} {komga_prog}); keeping {dim_unit} {old_progress}"
                ),
            )
        } else {
            (
                SyncResultCategory::NoChange,
                format!("AniList is already up to date at {dim_unit} {old_progress}"),
            )
        };

        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: Some(media_id.to_string()),
            old_progress: Some(old_progress),
            new_progress: Some(old_progress),
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: cat,
            message: msg,
            is_dry_run: dry_run,
        };
    }

    // 6. Dry run vs Live update
    if dry_run {
        let status_desc = status_change
            .as_ref()
            .map(|s| format!(" (status -> {s})"))
            .unwrap_or_default();
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: Some(media_id.to_string()),
            old_progress: Some(old_progress),
            new_progress: Some(target_prog),
            dimension: Some(dimension.to_string()),
            status_change,
            category: SyncResultCategory::WouldUpdate,
            message: format!(
                "Dry run: would update AniList progress from {dim_unit} {old_progress} to {dim_unit} {target_prog}{status_desc}"
            ),
            is_dry_run: true,
        };
    }

    // Live update with correct dimension arguments
    let save_res = if is_volume {
        anilist::save_progress(
            trackers.http(),
            &account,
            media_id,
            new_status,
            0.0,
            None,
            Some(target_prog as i64),
        )
        .await
    } else {
        anilist::save_progress(
            trackers.http(),
            &account,
            media_id,
            new_status,
            0.0,
            Some(target_prog as i64),
            None,
        )
        .await
    };

    match save_res {
        Ok(_) => {
            let status_desc = status_change
                .as_ref()
                .map(|s| format!(" (status -> {s})"))
                .unwrap_or_default();
            SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::AniList,
                tracker_media_id: Some(media_id.to_string()),
                old_progress: Some(old_progress),
                new_progress: Some(target_prog),
                dimension: Some(dimension.to_string()),
                status_change,
                category: SyncResultCategory::Updated,
                message: format!(
                    "Updated AniList progress from {dim_unit} {old_progress} to {dim_unit} {target_prog}{status_desc}"
                ),
                is_dry_run: false,
            }
        }
        Err(e) => SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: Some(media_id.to_string()),
            old_progress: Some(old_progress),
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::RemoteError,
            message: format!("Failed to update AniList: {e}"),
            is_dry_run: false,
        },
    }
}
