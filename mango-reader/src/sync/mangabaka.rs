//! Reading-progress synchronization adapter for MangaBaka.
//!
//! Synchronizes reading progress to MangaBaka user libraries with dimension-safe
//! volume and chapter progress updates (`progress_volume`, `progress_chapter`).
//!
//! PROVEN API ARCHITECTURE:
//! 1. AniList cross-reference resolution via `GET /v1/source/anilist/{anilist_id}`.
//! 2. Existing library check via `GET /v1/my/library/{mangabaka_id}`.
//! 3. Safe 404 skipping (`NotInTrackerLibrary`) without automatically creating entries.
//! 4. Non-decreasing monotonic progress validation (`komga <= current` -> `NoChange` or `TrackerAhead`).
//! 5. Zero-mutation dry-run execution (`WouldUpdate`).
//! 6. Live updates via `PATCH /v1/my/library/{mangabaka_id}` with dimension-specific payload.
//! 7. Immediate post-mutation verification readback (`verified == requested` -> `Updated` vs `VerificationFailed`).
//! 8. `x-api-key` header authentication without credential leakage.

use super::{
    eligibility::evaluate_completion_predicate,
    model::{
        KomgaSeriesProgress, MappingConfidence, SyncRecord, SyncResultCategory, SyncTarget,
        TrackerMapping,
    },
};
use crate::{db::now, pipeline::mangabaka::MangaBakaClient};

pub async fn sync_series_to_mangabaka(
    client: &MangaBakaClient,
    has_token: bool,
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
    let record_id = format!("sync-{}-{}-{}", SyncTarget::MangaBaka, series_id, nanos);

    // Determine dimension: volume-based vs chapter-based
    let is_volume =
        progress.highest_completed_volume.is_some() && progress.highest_completed_chapter.is_none();
    let dimension = if is_volume { "volume" } else { "chapter" };
    let (komga_prog, dim_unit) = if is_volume {
        (progress.highest_completed_volume.unwrap_or(0.0), "Vol")
    } else {
        (progress.highest_completed_chapter.unwrap_or(0.0), "Ch")
    };

    // 1. Untouched check: untouched series in Komga produce SkippedUntouched without tracker mutations
    if komga_prog == 0.0 && progress.completed_books == 0 {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: mapping.map(|m| m.tracker_id.clone()),
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::SkippedUntouched,
            message: "Series has not been read in Komga; skipping MangaBaka sync".to_string(),
            is_dry_run: dry_run,
        };
    }

    // 2. Auth check
    if !has_token {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: None,
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::AuthError,
            message: "MangaBaka API token is not configured".to_string(),
            is_dry_run: dry_run,
        };
    }

    // 3. Mapping check
    let Some(map) = mapping else {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: None,
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::SkippedNoMapping,
            message: "Series has no MangaBaka mapping".to_string(),
            is_dry_run: dry_run,
        };
    };

    let clean_id = map
        .tracker_id
        .trim_start_matches("mb-")
        .trim_start_matches("mb_");
    let mangabaka_id = match clean_id.parse::<i64>() {
        Ok(id) if id > 0 && map.confidence != MappingConfidence::Ambiguous => id,
        _ => {
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::MangaBaka,
                tracker_media_id: Some(map.tracker_id.clone()),
                old_progress: None,
                new_progress: None,
                dimension: Some(dimension.to_string()),
                status_change: None,
                category: SyncResultCategory::AmbiguousMapping,
                message: format!(
                    "Invalid or ambiguous MangaBaka series ID: {}",
                    map.tracker_id
                ),
                is_dry_run: dry_run,
            };
        }
    };

    // 4. Query current user library entry on MangaBaka
    let library_entry = match client.get_my_library_entry(mangabaka_id).await {
        Ok(entry) => entry,
        Err(err) => {
            let status = err.status.as_u16();
            let category = if status == 401 || status == 403 || err.code == "mangabaka_unauthorized"
            {
                SyncResultCategory::AuthError
            } else if status == 429 || err.code == "mangabaka_rate_limited" {
                SyncResultCategory::RateLimited
            } else if status == 502 || err.code == "mangabaka_network_error" {
                SyncResultCategory::NetworkError
            } else {
                SyncResultCategory::RemoteError
            };
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::MangaBaka,
                tracker_media_id: Some(mangabaka_id.to_string()),
                old_progress: None,
                new_progress: None,
                dimension: Some(dimension.to_string()),
                status_change: None,
                category,
                message: format!("Failed to read MangaBaka library entry: {}", err.message),
                is_dry_run: dry_run,
            };
        }
    };

    // If entry returns 404 (None): do NOT automatically add the title to MangaBaka
    let Some(entry) = library_entry else {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: None,
            new_progress: None,
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::NotInTrackerLibrary,
            message:
                "Series is not in user's MangaBaka library; skipping without automatically adding"
                    .to_string(),
            is_dry_run: dry_run,
        };
    };

    // 5. Read current progress for dimension and status
    let current_mb_prog = if is_volume {
        entry.progress_volume.unwrap_or(0.0)
    } else {
        entry.progress_chapter.unwrap_or(0.0)
    };

    let old_state = entry.status.as_deref().unwrap_or("reading");

    let total_units: Option<i64> = if let Some(map) = mapping {
        if let Some(col_id) = &map.edition_id {
            client
                .fetch_collection_total_works(col_id)
                .await
                .ok()
                .flatten()
                .map(|t| t as i64)
        } else {
            None
        }
    } else {
        None
    };

    let is_completed = if let Some(tot) = total_units {
        evaluate_completion_predicate(progress, tot, komga_prog)
    } else {
        false
    };

    // Status transition rules:
    // - PAUSED or DROPPED: NEVER automatically override
    // - COMPLETED: conservative predicate only; never regress
    // - PLAN_TO_READ / CONSIDERING: transition to READING when reading begins
    let new_state = if old_state == "paused" || old_state == "dropped" {
        old_state
    } else if is_completed {
        "completed"
    } else if old_state == "completed" {
        "completed"
    } else if (old_state == "plan_to_read" || old_state == "considering")
        && (komga_prog > 0.0 || progress.completed_books > 0)
    {
        "reading"
    } else {
        old_state
    };

    let status_change = if new_state != old_state {
        Some(format!("{old_state} -> {new_state}"))
    } else {
        None
    };

    let needs_progress_update = komga_prog > current_mb_prog;
    let needs_status_update = new_state != old_state;

    // 6. Check if no changes are required
    if !needs_progress_update && !needs_status_update {
        if komga_prog == current_mb_prog && current_mb_prog > 0.0 {
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::MangaBaka,
                tracker_media_id: Some(mangabaka_id.to_string()),
                old_progress: Some(current_mb_prog),
                new_progress: Some(current_mb_prog),
                dimension: Some(dimension.to_string()),
                status_change: None,
                category: SyncResultCategory::NoChange,
                message: format!("MangaBaka is already up to date ({dim_unit} {current_mb_prog})"),
                is_dry_run: dry_run,
            };
        }

        if komga_prog < current_mb_prog {
            return SyncRecord {
                id: record_id,
                timestamp: now(),
                komga_series_id: series_id.clone(),
                komga_series_title: series_title.clone(),
                tracker: SyncTarget::MangaBaka,
                tracker_media_id: Some(mangabaka_id.to_string()),
                old_progress: Some(current_mb_prog),
                new_progress: Some(komga_prog),
                dimension: Some(dimension.to_string()),
                status_change: None,
                category: SyncResultCategory::TrackerAhead,
                message: format!(
                    "MangaBaka is ahead of Komga ({dim_unit} {current_mb_prog} > {dim_unit} {komga_prog}); keeping current progress"
                ),
                is_dry_run: dry_run,
            };
        }

        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: Some(0.0),
            new_progress: Some(0.0),
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::NoChange,
            message: format!("No reading progress to update on MangaBaka ({dim_unit} 0)"),
            is_dry_run: dry_run,
        };
    }

    // 7. Dry-run mode: return WouldUpdate with ZERO PATCH requests
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
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: Some(current_mb_prog),
            new_progress: Some(komga_prog),
            dimension: Some(dimension.to_string()),
            status_change,
            category: SyncResultCategory::WouldUpdate,
            message: format!(
                "Dry run: would update MangaBaka {dimension} progress from {dim_unit} {current_mb_prog} to {dim_unit} {komga_prog}{status_desc}"
            ),
            is_dry_run: true,
        };
    }

    // 8. Live mutation: PATCH dimension-specific field and optional state
    let (vol_arg, ch_arg) = if is_volume {
        (Some(komga_prog), None)
    } else {
        (None, Some(komga_prog))
    };

    let state_arg = if needs_status_update {
        Some(new_state)
    } else {
        None
    };

    if let Err(err) = client
        .update_my_library_entry(mangabaka_id, vol_arg, ch_arg, state_arg)
        .await
    {
        let status = err.status.as_u16();
        let category = if status == 401 || status == 403 || err.code == "mangabaka_unauthorized" {
            SyncResultCategory::AuthError
        } else if status == 429 || err.code == "mangabaka_rate_limited" {
            SyncResultCategory::RateLimited
        } else if status == 502 || err.code == "mangabaka_network_error" {
            SyncResultCategory::NetworkError
        } else {
            SyncResultCategory::RemoteError
        };
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: Some(current_mb_prog),
            new_progress: Some(komga_prog),
            dimension: Some(dimension.to_string()),
            status_change,
            category,
            message: format!("Failed to update MangaBaka progress: {}", err.message),
            is_dry_run: false,
        };
    }

    // 9. Immediate post-update verification
    match client.get_my_library_entry(mangabaka_id).await {
        Ok(Some(verified_entry)) => {
            let verified_prog = if is_volume {
                verified_entry.progress_volume.unwrap_or(0.0)
            } else {
                verified_entry.progress_chapter.unwrap_or(0.0)
            };

            let prog_matches = (verified_prog - komga_prog).abs() < 0.001;
            let status_matches = if needs_status_update {
                verified_entry.status.as_deref() == Some(new_state)
            } else {
                true
            };

            if prog_matches && status_matches {
                let status_desc = status_change
                    .as_ref()
                    .map(|s| format!(" (status: {s})"))
                    .unwrap_or_default();
                SyncRecord {
                    id: record_id,
                    timestamp: now(),
                    komga_series_id: series_id.clone(),
                    komga_series_title: series_title.clone(),
                    tracker: SyncTarget::MangaBaka,
                    tracker_media_id: Some(mangabaka_id.to_string()),
                    old_progress: Some(current_mb_prog),
                    new_progress: Some(komga_prog),
                    dimension: Some(dimension.to_string()),
                    status_change,
                    category: SyncResultCategory::Updated,
                    message: format!(
                        "Successfully updated MangaBaka {dimension} progress from {dim_unit} {current_mb_prog} to {dim_unit} {komga_prog}{status_desc}"
                    ),
                    is_dry_run: false,
                }
            } else {
                SyncRecord {
                    id: record_id,
                    timestamp: now(),
                    komga_series_id: series_id.clone(),
                    komga_series_title: series_title.clone(),
                    tracker: SyncTarget::MangaBaka,
                    tracker_media_id: Some(mangabaka_id.to_string()),
                    old_progress: Some(current_mb_prog),
                    new_progress: Some(komga_prog),
                    dimension: Some(dimension.to_string()),
                    status_change: None,
                    category: SyncResultCategory::VerificationFailed,
                    message: format!(
                        "Post-update verification mismatch: expected {dim_unit} {komga_prog}, got {dim_unit} {verified_prog}"
                    ),
                    is_dry_run: false,
                }
            }
        }
        Ok(None) => SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: Some(current_mb_prog),
            new_progress: Some(komga_prog),
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::VerificationFailed,
            message: "Post-update verification failed: library entry disappeared after update"
                .to_string(),
            is_dry_run: false,
        },
        Err(e) => SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(mangabaka_id.to_string()),
            old_progress: Some(current_mb_prog),
            new_progress: Some(komga_prog),
            dimension: Some(dimension.to_string()),
            status_change: None,
            category: SyncResultCategory::VerificationFailed,
            message: format!("Post-update verification failed: {}", e.message),
            is_dry_run: false,
        },
    }
}
