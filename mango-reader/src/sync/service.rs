//! Synchronization service orchestration.
//!
//! Coordinates reading progress synchronization between Komga and external trackers
//! (AniList, MangaBaka), guaranteeing single-execution concurrency locking,
//! partial failure tolerance, dry-run safety, and complete audit logging.

use super::{
    anilist::sync_series_to_anilist,
    audit::{list_recent_records, record_sync},
    komga_reader::fetch_all_series_progress,
    mangabaka::sync_series_to_mangabaka,
    mapping::resolve_mapping,
    model::{SyncRecord, SyncResultCategory, SyncSummary, SyncTarget},
};
use crate::{
    db::Db,
    error::{AppError, Result},
    pipeline::{komga::KomgaClient, mangabaka::MangaBakaClient, Pipeline},
    trackers::Trackers,
};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct SyncService {
    db: Db,
    trackers: Trackers,
    pipeline: Arc<Pipeline>,
    http: Client,
    running: Arc<Mutex<()>>,
}

impl SyncService {
    pub fn new(db: Db, trackers: Trackers, pipeline: Arc<Pipeline>) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            db,
            trackers,
            pipeline,
            http,
            running: Arc::new(Mutex::new(())),
        }
    }

    /// Returns whether a sync operation is currently in progress.
    pub fn is_running(&self) -> bool {
        self.running.try_lock().is_err()
    }

    /// Fetches the recent audit history.
    pub async fn history(&self, limit: usize) -> Result<Vec<SyncRecord>> {
        self.db.call(move |conn| list_recent_records(conn, limit)).await
    }

    /// Executes a progress synchronization run across all accessible Komga series.
    ///
    /// If `dry_run` is true, evaluates all changes and records diagnostic diffs
    /// with strictly zero writes to external trackers or Komga.
    pub async fn run_sync(
        &self,
        dry_run: bool,
        target_tracker: Option<SyncTarget>,
        library_id: Option<String>,
    ) -> Result<SyncSummary> {
        // Enforce concurrency safety: only one sync operation runs at a time
        let _guard = self
            .running
            .try_lock()
            .map_err(|_| AppError::new(409, "sync_conflict", "A progress synchronization operation is already in progress"))?;

        let (cfg, mangabaka_token, komga_api_key) = self.pipeline.load_internal_config().await?;

        let komga_client = KomgaClient::new(self.http.clone(), cfg.komga_url.clone(), komga_api_key);
        let has_mb_token = mangabaka_token.as_ref().is_some_and(|t| !t.trim().is_empty());
        let mb_client = MangaBakaClient::new(self.http.clone(), cfg.mangabaka_url.clone(), mangabaka_token);

        let target_lib = library_id.or(cfg.komga_library_id);
        let progress_list = match fetch_all_series_progress(&komga_client, target_lib.as_deref()).await {
            Ok(p) => p,
            Err(e) => {
                return Err(AppError::new(
                    502,
                    "komga_sync_error",
                    format!("Failed to query Komga series for progress sync: {e}"),
                ));
            }
        };

        let mut summary = SyncSummary {
            total_series: progress_list.len(),
            is_dry_run: dry_run,
            ..Default::default()
        };

        let sync_anilist = target_tracker.is_none() || target_tracker == Some(SyncTarget::AniList);
        let sync_mangabaka = target_tracker.is_none() || target_tracker == Some(SyncTarget::MangaBaka);

        for series_prog in &progress_list {
            // 1. AniList synchronization
            if sync_anilist {
                let s_id = series_prog.series_id.clone();
                let s_title = series_prog.series_title.clone();
                let mapping = self
                    .db
                    .call(move |conn| resolve_mapping(conn, &s_id, &s_title, SyncTarget::AniList))
                    .await
                    .ok()
                    .flatten();

                let record = sync_series_to_anilist(&self.trackers, series_prog, mapping.as_ref(), dry_run).await;
                self.record_and_tally(&mut summary, record).await;
            }

            // 2. MangaBaka synchronization
            if sync_mangabaka {
                let s_id = series_prog.series_id.clone();
                let s_title = series_prog.series_title.clone();
                let mapping = self
                    .db
                    .call(move |conn| resolve_mapping(conn, &s_id, &s_title, SyncTarget::MangaBaka))
                    .await
                    .ok()
                    .flatten();

                let record =
                    sync_series_to_mangabaka(&mb_client, has_mb_token, series_prog, mapping.as_ref(), dry_run).await;
                self.record_and_tally(&mut summary, record).await;
            }
        }

        Ok(summary)
    }

    async fn record_and_tally(&self, summary: &mut SyncSummary, record: SyncRecord) {
        match record.category {
            SyncResultCategory::Updated => summary.updated += 1,
            SyncResultCategory::DryRunWouldUpdate => summary.dry_run_updates += 1,
            SyncResultCategory::NoChange | SyncResultCategory::TrackerAhead => summary.no_change += 1,
            SyncResultCategory::SkippedNoMapping
            | SyncResultCategory::SkippedAmbiguousMapping
            | SyncResultCategory::SkippedUnsafeProgress => summary.skipped += 1,
            SyncResultCategory::AuthError
            | SyncResultCategory::NetworkError
            | SyncResultCategory::RateLimited
            | SyncResultCategory::RemoteError => summary.errors += 1,
        }

        // Persist to sync_records audit log
        let rec = record.clone();
        let _ = self.db.call(move |conn| record_sync(conn, &rec)).await;
        summary.records.push(record);
    }
}
