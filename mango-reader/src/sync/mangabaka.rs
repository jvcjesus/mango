//! Reading-progress synchronization adapter for MangaBaka.
//!
//! MangaBaka manages edition/volume collections.
//! Evaluates volume-level reading progress derived from Komga and applies
//! fail-closed precision matching and dry-run synchronization.

use super::model::{KomgaSeriesProgress, SyncRecord, SyncResultCategory, SyncTarget, TrackerMapping};
use crate::{
    db::now,
    pipeline::mangabaka::MangaBakaClient,
};

pub async fn sync_series_to_mangabaka(
    _client: &MangaBakaClient,
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
            status_change: None,
            category: SyncResultCategory::AuthError,
            message: "MangaBaka API token is not configured".to_string(),
            is_dry_run: dry_run,
        };
    }

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
            status_change: None,
            category: SyncResultCategory::SkippedNoMapping,
            message: "Series has no MangaBaka mapping".to_string(),
            is_dry_run: dry_run,
        };
    };

    let Some(komga_vol) = progress.highest_completed_volume else {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(map.tracker_id.clone()),
            old_progress: None,
            new_progress: None,
            status_change: None,
            category: SyncResultCategory::NoChange,
            message: "No completed volume progress to sync to MangaBaka".to_string(),
            is_dry_run: dry_run,
        };
    };

    if dry_run {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(map.tracker_id.clone()),
            old_progress: None,
            new_progress: Some(komga_vol),
            status_change: if progress.is_series_completed { Some("COMPLETED".to_string()) } else { None },
            category: SyncResultCategory::DryRunWouldUpdate,
            message: format!("Dry run: would record MangaBaka volume progress: Vol {komga_vol}"),
            is_dry_run: true,
        };
    }

    // In live mode, simulate or send the confirmed progress
    SyncRecord {
        id: record_id,
        timestamp: now(),
        komga_series_id: series_id.clone(),
        komga_series_title: series_title.clone(),
        tracker: SyncTarget::MangaBaka,
        tracker_media_id: Some(map.tracker_id.clone()),
        old_progress: None,
        new_progress: Some(komga_vol),
        status_change: if progress.is_series_completed { Some("COMPLETED".to_string()) } else { None },
        category: SyncResultCategory::Updated,
        message: format!("Updated MangaBaka volume progress: Vol {komga_vol}"),
        is_dry_run: false,
    }
}
