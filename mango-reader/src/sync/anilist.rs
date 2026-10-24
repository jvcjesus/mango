//! Reading-progress synchronization adapter for AniList.
//!
//! AniList tracks manga by read chapter numbers.
//! Volume progress in Komga without explicit chapter mapping will safely skip (`SkippedUnsafeProgress`).
//! Progress never decreases unless overridden: `target = max(existing_anilist, komga_ch)`.

use super::model::{KomgaSeriesProgress, SyncRecord, SyncResultCategory, SyncTarget, TrackerMapping};
use crate::{
    db::now,
    trackers::{anilist, Kind, Track, Trackers},
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
                status_change: None,
                category: SyncResultCategory::SkippedAmbiguousMapping,
                message: format!("Invalid AniList media ID: {}", map.tracker_id),
                is_dry_run: dry_run,
            };
        }
    };

    // 2. Safety check: AniList is chapter-oriented. Volume-only Komga series without chapter counts must fail closed.
    let komga_ch = match progress.highest_completed_chapter {
        Some(ch) => ch,
        None => {
            if let Some(vol) = progress.highest_completed_volume {
                return SyncRecord {
                    id: record_id,
                    timestamp: now(),
                    komga_series_id: series_id.clone(),
                    komga_series_title: series_title.clone(),
                    tracker: SyncTarget::AniList,
                    tracker_media_id: Some(media_id.to_string()),
                    old_progress: None,
                    new_progress: None,
                    status_change: None,
                    category: SyncResultCategory::SkippedUnsafeProgress,
                    message: format!(
                        "Komga series is volume-based (Vol {vol}); cannot safely derive chapter progress for AniList without chapter mapping"
                    ),
                    is_dry_run: dry_run,
                };
            }
            0.0
        }
    };

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
                status_change: None,
                category: SyncResultCategory::AuthError,
                message: format!("AniList authentication error: {e}"),
                is_dry_run: dry_run,
            };
        }
    };

    // 4. Query current AniList entry
    let (old_progress, old_status, total_chapters) = match anilist::find(trackers.http(), &account, media_id).await {
        Ok(Some(entry)) => (entry.last_chapter_read, entry.status, entry.total_chapters),
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
                status_change: None,
                category: SyncResultCategory::NetworkError,
                message: format!("Failed to fetch AniList entry: {e}"),
                is_dry_run: dry_run,
            };
        }
    };

    // 5. Apply never-decrease rule: target = max(existing, komga_ch)
    let target_ch = old_progress.max(komga_ch);
    let new_status = if progress.is_series_completed { 2 } else { 1 }; // 2 = COMPLETED
    let status_change = if new_status != old_status {
        Some(if new_status == 2 { "COMPLETED".to_string() } else { "CURRENT".to_string() })
    } else {
        None
    };

    let needs_progress_update = target_ch > old_progress;
    let needs_status_update = progress.is_series_completed && old_status != 2;

    if !needs_progress_update && !needs_status_update {
        let (cat, msg) = if old_progress > komga_ch {
            (
                SyncResultCategory::TrackerAhead,
                format!("AniList is ahead (Ch {old_progress}) of Komga progress (Ch {komga_ch}); keeping Ch {old_progress}"),
            )
        } else {
            (
                SyncResultCategory::NoChange,
                format!("AniList is already up to date at Ch {old_progress}"),
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
            status_change: None,
            category: cat,
            message: msg,
            is_dry_run: dry_run,
        };
    }

    // 6. Dry run vs Live update
    if dry_run {
        let status_desc = status_change.as_ref().map(|s| format!(" (status -> {s})")).unwrap_or_default();
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::AniList,
            tracker_media_id: Some(media_id.to_string()),
            old_progress: Some(old_progress),
            new_progress: Some(target_ch),
            status_change,
            category: SyncResultCategory::DryRunWouldUpdate,
            message: format!("Dry run: would update AniList progress from Ch {old_progress} to Ch {target_ch}{status_desc}"),
            is_dry_run: true,
        };
    }

    let track = Track {
        tracker: Kind::AniList.id(),
        library_id: 0,
        media_id,
        url: format!("https://anilist.co/manga/{media_id}"),
        title: series_title.clone(),
        last_chapter_read: target_ch,
        total_chapters,
        score: 0.0,
        status: new_status,
        error: None,
    };

    match anilist::save(trackers.http(), &account, &track).await {
        Ok(_) => {
            let status_desc = status_change.as_ref().map(|s| format!(" (status -> {s})")).unwrap_or_default();
            SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::AniList,
                tracker_media_id: Some(media_id.to_string()),
                old_progress: Some(old_progress),
                new_progress: Some(target_ch),
                status_change,
                category: SyncResultCategory::Updated,
                message: format!("Updated AniList progress from Ch {old_progress} to Ch {target_ch}{status_desc}"),
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
            status_change: None,
            category: SyncResultCategory::RemoteError,
            message: format!("Failed to update AniList: {e}"),
            is_dry_run: false,
        },
    }
}
