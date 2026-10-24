//! Reading-progress synchronization adapter for MangaBaka.
//!
//! MangaBaka manages edition/volume collections.
//! Evaluates volume-level reading progress derived from Komga.
//!
//! AUDITED API REALITY:
//! MangaBaka REST API v1 provides public catalog/edition/metadata queries,
//! but does NOT provide a remote user reading progress mutation endpoint.
//! Live progress sync safely fails closed with `SyncResultCategory::Unsupported`.
//! MangaBaka is actively utilized for exact edition resolution and authoritative totals.

use super::model::{
    KomgaSeriesProgress, SyncRecord, SyncResultCategory, SyncTarget, TrackerMapping,
};
use crate::{db::now, pipeline::mangabaka::MangaBakaClient};

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
    let dimension = Some("volume".to_string());

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
            dimension,
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
            dimension,
            status_change: None,
            category: SyncResultCategory::SkippedNoMapping,
            message: "Series has no MangaBaka mapping".to_string(),
            is_dry_run: dry_run,
        };
    };

    if progress.completed_books == 0 && progress.highest_completed_volume.unwrap_or(0.0) == 0.0 {
        return SyncRecord {
            id: record_id,
            timestamp: now(),
            komga_series_id: series_id.clone(),
            komga_series_title: series_title.clone(),
            tracker: SyncTarget::MangaBaka,
            tracker_media_id: Some(map.tracker_id.clone()),
            old_progress: None,
            new_progress: None,
            dimension,
            status_change: None,
            category: SyncResultCategory::SkippedUntouched,
            message: "Series has not been read in Komga; skipping MangaBaka sync".to_string(),
            is_dry_run: dry_run,
        };
    }

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
            dimension,
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
            dimension,
            status_change: if progress.is_series_completed {
                Some("COMPLETED".to_string())
            } else {
                None
            },
            category: SyncResultCategory::WouldUpdate,
            message: format!("Dry run: would record MangaBaka volume progress: Vol {komga_vol}"),
            is_dry_run: true,
        };
    }

    // Live update fails closed honestly: MangaBaka API v1 does not support remote user progress mutations
    SyncRecord {
        id: record_id,
        timestamp: now(),
        komga_series_id: series_id.clone(),
        komga_series_title: series_title.clone(),
        tracker: SyncTarget::MangaBaka,
        tracker_media_id: Some(map.tracker_id.clone()),
        old_progress: None,
        new_progress: Some(komga_vol),
        dimension,
        status_change: None,
        category: SyncResultCategory::Unsupported,
        message: "MangaBaka REST API v1 does not support user progress mutations; edition and totals resolution active".to_string(),
        is_dry_run: false,
    }
}
