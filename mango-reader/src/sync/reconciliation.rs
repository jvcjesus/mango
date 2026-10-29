//! Ongoing series automation and reconciliation engine (M6.1).
//!
//! Provides periodic and manual reconciliation answering:
//! 1. Is the series up to date?
//! 2. Which official volumes exist?
//! 3. Which volumes are owned in Komga?
//! 4. Which official volumes are missing?
//! 5. Does anything require review?
//!
//! Absolute Physical-Media Invariant:
//! Published canonical media in Komga strictly overrules transient acquisition state.
//! This module is structurally read-only and incapable of enqueuing TorBox jobs or publishing media.

use super::{
    catalog::{add_or_update_tracked, set_tracked, TrackedCatalogEntry},
    identity::{resolve_mangabaka_identity, IdentityResolutionResult},
    mapping::get_mapping,
    model::SyncTarget,
};
use crate::{
    db::{now, Db},
    error::{AppError, Result},
    pipeline::{
        komga::KomgaClient,
        mangabaka::{
            normalize_title, MangaBakaClient, MangaBakaCollectionMatch,
            MangaBakaSeriesMatch, MangaBakaVolumesResult,
        },
    },
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    UpToDate,
    MissingVolume,
    ReviewRequired,
    SyncError,
}

impl std::fmt::Display for ReconciliationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UpToDate => write!(f, "up_to_date"),
            Self::MissingVolume => write!(f, "missing_volume"),
            Self::ReviewRequired => write!(f, "review_required"),
            Self::SyncError => write!(f, "sync_error"),
        }
    }
}

impl std::str::FromStr for ReconciliationStatus {
    type Err = AppError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "up_to_date" => Ok(Self::UpToDate),
            "missing_volume" => Ok(Self::MissingVolume),
            "review_required" => Ok(Self::ReviewRequired),
            "sync_error" => Ok(Self::SyncError),
            other => Err(AppError::bad_request(format!(
                "Invalid reconciliation status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationSummary {
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub mangabaka_id: Option<i64>,
    pub collection_id: Option<String>,
    pub title: String,
    pub edition: String,
    pub status: ReconciliationStatus,
    pub is_ongoing: bool,
    pub total_official_volumes: Option<u32>,
    pub owned_volumes_count: usize,
    pub missing_volumes_count: usize,
    pub owned_volumes: Vec<f64>,
    pub missing_volumes: Vec<f64>,
    pub future_volumes: Vec<f64>,
    pub last_reconciled_at: i64,
    pub last_error: Option<String>,
    pub review_reason: Option<String>,
    pub display_status: String,
}

impl ReconciliationSummary {
    pub fn compute_display_status(&self) -> String {
        match self.status {
            ReconciliationStatus::UpToDate => {
                if self.is_ongoing {
                    format!("{} volumes owned · Up to date · Ongoing", self.owned_volumes_count)
                } else {
                    format!("{} volumes owned · Up to date · Completed", self.owned_volumes_count)
                }
            }
            ReconciliationStatus::MissingVolume => {
                let tot = self.total_official_volumes.unwrap_or((self.owned_volumes_count + self.missing_volumes_count) as u32);
                format!(
                    "{} / {} volumes owned · {} missing",
                    self.owned_volumes_count, tot, self.missing_volumes_count
                )
            }
            ReconciliationStatus::ReviewRequired => {
                format!(
                    "Review required: {}",
                    self.review_reason.as_deref().unwrap_or("Ambiguous metadata identity")
                )
            }
            ReconciliationStatus::SyncError => {
                format!(
                    "Sync error: {}",
                    self.last_error.as_deref().unwrap_or("Communication failure")
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReconciliationTarget {
    pub manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub title: String,
    pub edition: Option<String>,
}

/// Converts epoch seconds into (Year, Month, Day) UTC.
pub fn epoch_to_ymd(epoch_sec: i64) -> (i32, u32, u32) {
    let z = (epoch_sec / 86400) + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1029 + doe / 1461 - doe / 36524) / 365;
    let y = (yoe as i32) + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

pub fn today_ymd() -> String {
    let (y, m, d) = epoch_to_ymd(now());
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn is_future_release(release_date: Option<&str>, today: &str) -> bool {
    if let Some(date_str) = release_date {
        let trimmed = date_str.trim();
        if trimmed.len() >= 10 {
            let date_part = &trimmed[..10];
            return date_part > today;
        }
    }
    false
}

/// Gathers local physically owned volume numbers from downloads and chapters tables.
pub fn gather_local_owned_volumes(conn: &Connection, manga_id: Option<i64>) -> Result<Vec<f64>> {
    let mut owned = Vec::new();

    // Canonical media published locally in downloads table
    let sql = "SELECT d.published_path, c.name, d.raw_title
               FROM downloads d
               JOIN chapters c ON c.id = d.chapter_id
               WHERE (c.manga_id = ?1 OR ?1 IS NULL)
                 AND (d.published_path IS NOT NULL OR d.state = 'completed')";

    if let Ok(mut stmt) = conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map([manga_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        }) {
            for (pub_path, c_name, raw_title) in rows.flatten() {
                let vol_opt = pub_path
                    .as_deref()
                    .and_then(crate::release::parse_volume)
                    .or_else(|| crate::release::parse_volume(&c_name))
                    .or_else(|| raw_title.as_deref().and_then(crate::release::parse_volume));

                if let Some(v) = vol_opt {
                    if !owned.iter().any(|&ov: &f64| (ov - v).abs() < 0.001) {
                        owned.push(v);
                    }
                }
            }
        }
    }

    // Existing chapter downloads
    if let Some(m_id) = manga_id {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT name, download_path FROM chapters WHERE manga_id = ?1 AND download_path IS NOT NULL",
        ) {
            if let Ok(rows) = stmt.query_map([m_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            }) {
                for (name, dl_path) in rows.flatten() {
                    let vol_opt = crate::release::parse_volume(&name)
                        .or_else(|| dl_path.as_deref().and_then(crate::release::parse_volume));
                    if let Some(v) = vol_opt {
                        if !owned.iter().any(|&ov: &f64| (ov - v).abs() < 0.001) {
                            owned.push(v);
                        }
                    }
                }
            }
        }
    }

    owned.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(owned)
}

/// Executes reconciliation for a series with fail-closed precision.
pub async fn reconcile_series(
    db: &Db,
    komga_client: Option<&KomgaClient>,
    mb_client: &MangaBakaClient,
    target: &ReconciliationTarget,
) -> Result<ReconciliationSummary> {
    let edition = target
        .edition
        .as_deref()
        .map(crate::acquisition::edition::normalize_edition)
        .unwrap_or_else(|| "standard".to_string());

    let series_id = if let Some(m_id) = target.manga_id {
        format!("manga:{m_id}")
    } else if let Some(ref k_id) = target.komga_series_id {
        format!("komga:{k_id}")
    } else {
        format!("title:{}", normalize_title(&target.title))
    };

    let series_id_for_prev = series_id.clone();
    let previous = db
        .call(move |conn| get_reconciliation(conn, &series_id_for_prev))
        .await?;

    let current_time = now();

    // 1. Ownership: strictly gather canonical and Komga ownership
    let m_id_opt = target.manga_id;
    let mut owned_volumes = db
        .call(move |conn| gather_local_owned_volumes(conn, m_id_opt))
        .await?;

    if let (Some(kc), Some(ref k_id)) = (komga_client, &target.komga_series_id) {
        match kc.get_books_in_series(k_id).await {
            Ok(books) => {
                for b in books {
                    let vol_num = if b.number > 0.0 {
                        Some(b.number)
                    } else {
                        crate::release::parse_volume(&b.name)
                    };
                    if let Some(v) = vol_num {
                        if !owned_volumes.iter().any(|&ov: &f64| (ov - v).abs() < 0.001) {
                            owned_volumes.push(v);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Komga books for series {k_id}: {e}");
            }
        }
    }
    owned_volumes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // 2. Resolve MangaBaka Identity
    let mut resolved_mb_id: Option<i64> = None;

    if let Some(ref k_id) = target.komga_series_id {
        let k_id_clone = k_id.clone();
        let title_clone = target.title.clone();
        let mb_client_clone = mb_client.clone();

        let resolution = db
            .call(move |conn| {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        resolve_mangabaka_identity(conn, &mb_client_clone, &k_id_clone, &title_clone)
                            .await
                    })
                })
            })
            .await?;

        match resolution {
            IdentityResolutionResult::Resolved(mapping) => {
                if let Ok(parsed_id) = mapping.tracker_id.parse::<i64>() {
                    resolved_mb_id = Some(parsed_id);
                }
            }
            IdentityResolutionResult::Ambiguous(candidates) => {
                let reason = format!(
                    "Ambiguous MangaBaka identity: {} candidates found",
                    candidates.len()
                );
                let title_c = target.title.clone();
                let reason_c = reason.clone();
                let mut sum = build_review_required(
                    &series_id,
                    target,
                    &edition,
                    owned_volumes,
                    reason,
                    current_time,
                );
                sum.display_status = sum.compute_display_status();
                let sum_c = sum.clone();
                db.call(move |conn| {
                    record_event(conn, &title_c, None, None, "ACTION_REQUIRED", &reason_c)?;
                    save_reconciliation(conn, &sum_c)
                })
                .await?;
                return Ok(sum);
            }
            IdentityResolutionResult::RateLimited => {
                return handle_sync_error(
                    db,
                    &series_id,
                    target,
                    &edition,
                    previous,
                    owned_volumes,
                    "Rate limited by MangaBaka (HTTP 429)".to_string(),
                    current_time,
                )
                .await;
            }
            IdentityResolutionResult::NetworkError(msg)
            | IdentityResolutionResult::ApiError(msg)
            | IdentityResolutionResult::AuthError(msg) => {
                return handle_sync_error(
                    db,
                    &series_id,
                    target,
                    &edition,
                    previous,
                    owned_volumes,
                    msg,
                    current_time,
                )
                .await;
            }
            IdentityResolutionResult::NotFound => {
                // Fall through to title search
            }
        }
    }

    if resolved_mb_id.is_none() {
        if let Some(ref k_id) = target.komga_series_id {
            let k_id_c = k_id.clone();
            if let Ok(Some(mapping)) = db
                .call(move |conn| get_mapping(conn, &k_id_c, SyncTarget::MangaBaka))
                .await
            {
                if let Ok(id) = mapping.tracker_id.parse::<i64>() {
                    resolved_mb_id = Some(id);
                }
            }
        }
    }

    if resolved_mb_id.is_none() {
        match mb_client.match_series(&target.title).await {
            Ok(MangaBakaSeriesMatch::Found { id, .. }) => {
                resolved_mb_id = Some(id);
            }
            Ok(MangaBakaSeriesMatch::Ambiguous(candidates)) => {
                let reason = format!(
                    "Ambiguous MangaBaka title match: {} candidate series",
                    candidates.len()
                );
                let title_c = target.title.clone();
                let reason_c = reason.clone();
                let mut sum = build_review_required(
                    &series_id,
                    target,
                    &edition,
                    owned_volumes,
                    reason,
                    current_time,
                );
                sum.display_status = sum.compute_display_status();
                let sum_c = sum.clone();
                db.call(move |conn| {
                    record_event(conn, &title_c, None, None, "ACTION_REQUIRED", &reason_c)?;
                    save_reconciliation(conn, &sum_c)
                })
                .await?;
                return Ok(sum);
            }
            Ok(MangaBakaSeriesMatch::NotFound) => {
                let reason = format!("No matching series found on MangaBaka for '{}'", target.title);
                let title_c = target.title.clone();
                let reason_c = reason.clone();
                let mut sum = build_review_required(
                    &series_id,
                    target,
                    &edition,
                    owned_volumes,
                    reason,
                    current_time,
                );
                sum.display_status = sum.compute_display_status();
                let sum_c = sum.clone();
                db.call(move |conn| {
                    record_event(conn, &title_c, None, None, "ACTION_REQUIRED", &reason_c)?;
                    save_reconciliation(conn, &sum_c)
                })
                .await?;
                return Ok(sum);
            }
            Ok(MangaBakaSeriesMatch::RateLimited) => {
                return handle_sync_error(
                    db,
                    &series_id,
                    target,
                    &edition,
                    previous,
                    owned_volumes,
                    "Rate limited by MangaBaka (HTTP 429)".to_string(),
                    current_time,
                )
                .await;
            }
            Ok(MangaBakaSeriesMatch::NetworkError(msg))
            | Ok(MangaBakaSeriesMatch::AuthError(msg))
            | Ok(MangaBakaSeriesMatch::ApiError(msg)) => {
                return handle_sync_error(
                    db,
                    &series_id,
                    target,
                    &edition,
                    previous,
                    owned_volumes,
                    msg,
                    current_time,
                )
                .await;
            }
            Err(e) => {
                return handle_sync_error(
                    db,
                    &series_id,
                    target,
                    &edition,
                    previous,
                    owned_volumes,
                    e.to_string(),
                    current_time,
                )
                .await;
            }
        }
    }

    let mb_series_id = match resolved_mb_id {
        Some(id) => id,
        None => {
            let reason = "Could not resolve MangaBaka series identity".to_string();
            let mut sum = build_review_required(
                &series_id,
                target,
                &edition,
                owned_volumes,
                reason,
                current_time,
            );
            sum.display_status = sum.compute_display_status();
            let sum_c = sum.clone();
            db.call(move |conn| save_reconciliation(conn, &sum_c)).await?;
            return Ok(sum);
        }
    };

    // 3. Match Edition Collection
    let collection_id = match mb_client.match_edition_collection(mb_series_id, Some(&edition)).await {
        Ok(MangaBakaCollectionMatch::Found { id, .. }) => id,
        Ok(MangaBakaCollectionMatch::Ambiguous(candidates)) => {
            let reason = format!(
                "Ambiguous collections on MangaBaka for edition '{}': {} matches",
                edition,
                candidates.len()
            );
            let title_c = target.title.clone();
            let reason_c = reason.clone();
            let mut sum = build_review_required(
                &series_id,
                target,
                &edition,
                owned_volumes,
                reason,
                current_time,
            );
            sum.mangabaka_id = Some(mb_series_id);
            sum.display_status = sum.compute_display_status();
            let sum_c = sum.clone();
            db.call(move |conn| {
                record_event(conn, &title_c, Some(mb_series_id), None, "ACTION_REQUIRED", &reason_c)?;
                save_reconciliation(conn, &sum_c)
            })
            .await?;
            return Ok(sum);
        }
        Ok(MangaBakaCollectionMatch::NotFound) => {
            let reason = format!(
                "No collection found on MangaBaka for edition '{}'",
                edition
            );
            let title_c = target.title.clone();
            let reason_c = reason.clone();
            let mut sum = build_review_required(
                &series_id,
                target,
                &edition,
                owned_volumes,
                reason,
                current_time,
            );
            sum.mangabaka_id = Some(mb_series_id);
            sum.display_status = sum.compute_display_status();
            let sum_c = sum.clone();
            db.call(move |conn| {
                record_event(conn, &title_c, Some(mb_series_id), None, "ACTION_REQUIRED", &reason_c)?;
                save_reconciliation(conn, &sum_c)
            })
            .await?;
            return Ok(sum);
        }
        Ok(MangaBakaCollectionMatch::NetworkError(msg))
        | Ok(MangaBakaCollectionMatch::AuthError(msg)) => {
            return handle_sync_error(
                db,
                &series_id,
                target,
                &edition,
                previous,
                owned_volumes,
                msg,
                current_time,
            )
            .await;
        }
        Err(e) => {
            return handle_sync_error(
                db,
                &series_id,
                target,
                &edition,
                previous,
                owned_volumes,
                e.to_string(),
                current_time,
            )
            .await;
        }
    };

    // 4. Fetch Official Works / Volumes
    let official_volumes = match mb_client.fetch_collection_volumes(&collection_id).await {
        Ok(MangaBakaVolumesResult::Found(vols)) => vols,
        Ok(MangaBakaVolumesResult::Empty) => {
            let reason = format!(
                "Matched collection '{}' contains no works on MangaBaka",
                collection_id
            );
            let title_c = target.title.clone();
            let reason_c = reason.clone();
            let mut sum = build_review_required(
                &series_id,
                target,
                &edition,
                owned_volumes,
                reason,
                current_time,
            );
            sum.mangabaka_id = Some(mb_series_id);
            sum.collection_id = Some(collection_id);
            sum.display_status = sum.compute_display_status();
            let sum_c = sum.clone();
            db.call(move |conn| {
                record_event(conn, &title_c, Some(mb_series_id), None, "ACTION_REQUIRED", &reason_c)?;
                save_reconciliation(conn, &sum_c)
            })
            .await?;
            return Ok(sum);
        }
        Ok(MangaBakaVolumesResult::RateLimited) => {
            return handle_sync_error(
                db,
                &series_id,
                target,
                &edition,
                previous,
                owned_volumes,
                "Rate limited by MangaBaka (HTTP 429)".to_string(),
                current_time,
            )
            .await;
        }
        Ok(MangaBakaVolumesResult::NetworkError(msg))
        | Ok(MangaBakaVolumesResult::AuthError(msg)) => {
            return handle_sync_error(
                db,
                &series_id,
                target,
                &edition,
                previous,
                owned_volumes,
                msg,
                current_time,
            )
            .await;
        }
        Err(e) => {
            return handle_sync_error(
                db,
                &series_id,
                target,
                &edition,
                previous,
                owned_volumes,
                e.to_string(),
                current_time,
            )
            .await;
        }
    };

    // 5. Check Series Status (Ongoing vs Completed)
    let is_ongoing = match mb_client.fetch_series_status(mb_series_id).await {
        Ok(Some(st)) => {
            let lower = st.trim().to_lowercase();
            lower != "completed" && lower != "finished" && lower != "cancelled"
        }
        _ => true,
    };

    // 6. Partition Volumes: Released vs Future Announced
    let today = today_ymd();
    let mut released_vols = Vec::new();
    let mut future_vols = Vec::new();

    for v in &official_volumes {
        if is_future_release(v.release_date.as_deref(), &today) {
            future_vols.push(v.volume_number);
        } else {
            released_vols.push(v.volume_number);
        }
    }

    // 7. Calculate Missing Content
    let mut missing_vols = Vec::new();
    let mut owned_count = 0;

    for &rv in &released_vols {
        if owned_volumes.iter().any(|&ov| (ov - rv).abs() < 0.001) {
            owned_count += 1;
        } else {
            missing_vols.push(rv);
        }
    }

    let status = if missing_vols.is_empty() {
        ReconciliationStatus::UpToDate
    } else {
        ReconciliationStatus::MissingVolume
    };

    let mut summary = ReconciliationSummary {
        series_id,
        manga_id: target.manga_id,
        komga_series_id: target.komga_series_id.clone(),
        mangabaka_id: Some(mb_series_id),
        collection_id: Some(collection_id),
        title: target.title.clone(),
        edition,
        status,
        is_ongoing,
        total_official_volumes: Some(released_vols.len() as u32),
        owned_volumes_count: owned_count,
        missing_volumes_count: missing_vols.len(),
        owned_volumes,
        missing_volumes: missing_vols.clone(),
        future_volumes: future_vols,
        last_reconciled_at: current_time,
        last_error: None,
        review_reason: None,
        display_status: String::new(),
    };

    summary.display_status = summary.compute_display_status();

    // 8. Log Events for Notifications (M6.3 Foundation) and save summary
    let sum_clone = summary.clone();
    let title_c = target.title.clone();
    let prev_c = previous;
    let released_vols_c = released_vols.clone();

    db.call(move |conn| {
        for &mv in &missing_vols {
            record_event(
                conn,
                &title_c,
                Some(mb_series_id),
                Some(mv),
                "MISSING_TARGET",
                &format!("Missing volume {mv} detected for {title_c}"),
            )?;
        }

        for &rv in &released_vols_c {
            let is_new = prev_c.as_ref().is_some_and(|p| {
                !p.owned_volumes.iter().any(|&ov| (ov - rv).abs() < 0.001)
                    && !p.missing_volumes.iter().any(|&mv| (mv - rv).abs() < 0.001)
                    && !p.future_volumes.iter().any(|&fv| (fv - rv).abs() < 0.001)
            });
            if is_new {
                record_event(
                    conn,
                    &title_c,
                    Some(mb_series_id),
                    Some(rv),
                    "NEW_VOLUME",
                    &format!("New official volume {rv} released for {title_c}"),
                )?;
            }
        }

        save_reconciliation(conn, &sum_clone)
    })
    .await?;

    Ok(summary)
}

fn build_review_required(
    series_id: &str,
    target: &ReconciliationTarget,
    edition: &str,
    owned_volumes: Vec<f64>,
    reason: String,
    current_time: i64,
) -> ReconciliationSummary {
    let owned_count = owned_volumes.len();
    ReconciliationSummary {
        series_id: series_id.to_string(),
        manga_id: target.manga_id,
        komga_series_id: target.komga_series_id.clone(),
        mangabaka_id: None,
        collection_id: None,
        title: target.title.clone(),
        edition: edition.to_string(),
        status: ReconciliationStatus::ReviewRequired,
        is_ongoing: true,
        total_official_volumes: None,
        owned_volumes_count: owned_count,
        missing_volumes_count: 0,
        owned_volumes,
        missing_volumes: vec![],
        future_volumes: vec![],
        last_reconciled_at: current_time,
        last_error: None,
        review_reason: Some(reason),
        display_status: String::new(),
    }
}

async fn handle_sync_error(
    db: &Db,
    series_id: &str,
    target: &ReconciliationTarget,
    edition: &str,
    previous: Option<ReconciliationSummary>,
    owned_volumes: Vec<f64>,
    error_msg: String,
    current_time: i64,
) -> Result<ReconciliationSummary> {
    let summary = if let Some(mut prev) = previous {
        prev.status = ReconciliationStatus::SyncError;
        prev.last_error = Some(error_msg);
        prev.last_reconciled_at = current_time;
        if !owned_volumes.is_empty() {
            prev.owned_volumes = owned_volumes;
            prev.owned_volumes_count = prev.owned_volumes.len();
        }
        prev.display_status = prev.compute_display_status();
        prev
    } else {
        let owned_count = owned_volumes.len();
        let mut s = ReconciliationSummary {
            series_id: series_id.to_string(),
            manga_id: target.manga_id,
            komga_series_id: target.komga_series_id.clone(),
            mangabaka_id: None,
            collection_id: None,
            title: target.title.clone(),
            edition: edition.to_string(),
            status: ReconciliationStatus::SyncError,
            is_ongoing: true,
            total_official_volumes: None,
            owned_volumes_count: owned_count,
            missing_volumes_count: 0,
            owned_volumes,
            missing_volumes: vec![],
            future_volumes: vec![],
            last_reconciled_at: current_time,
            last_error: Some(error_msg),
            review_reason: None,
            display_status: String::new(),
        };
        s.display_status = s.compute_display_status();
        s
    };

    let s_c = summary.clone();
    db.call(move |conn| save_reconciliation(conn, &s_c)).await?;
    Ok(summary)
}

/// Idempotently stores reconciliation summary in SQLite.
pub fn save_reconciliation(conn: &Connection, s: &ReconciliationSummary) -> Result<()> {
    let owned_json = serde_json::to_string(&s.owned_volumes).unwrap_or_else(|_| "[]".to_string());
    let missing_json = serde_json::to_string(&s.missing_volumes).unwrap_or_else(|_| "[]".to_string());
    let future_json = serde_json::to_string(&s.future_volumes).unwrap_or_else(|_| "[]".to_string());
    let status_str = s.status.to_string();

    conn.execute(
        "INSERT INTO series_reconciliations (
            series_id, manga_id, komga_series_id, mangabaka_id, collection_id,
            title, edition, status, is_ongoing, total_official_volumes,
            owned_volumes_count, missing_volumes_count, owned_volumes_json,
            missing_volumes_json, future_volumes_json, last_reconciled_at,
            last_error, review_reason, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
        ON CONFLICT(series_id) DO UPDATE SET
            manga_id = excluded.manga_id,
            komga_series_id = excluded.komga_series_id,
            mangabaka_id = excluded.mangabaka_id,
            collection_id = excluded.collection_id,
            title = excluded.title,
            edition = excluded.edition,
            status = excluded.status,
            is_ongoing = excluded.is_ongoing,
            total_official_volumes = excluded.total_official_volumes,
            owned_volumes_count = excluded.owned_volumes_count,
            missing_volumes_count = excluded.missing_volumes_count,
            owned_volumes_json = excluded.owned_volumes_json,
            missing_volumes_json = excluded.missing_volumes_json,
            future_volumes_json = excluded.future_volumes_json,
            last_reconciled_at = excluded.last_reconciled_at,
            last_error = excluded.last_error,
            review_reason = excluded.review_reason,
            updated_at = excluded.updated_at",
        params![
            s.series_id,
            s.manga_id,
            s.komga_series_id,
            s.mangabaka_id,
            s.collection_id,
            s.title,
            s.edition,
            status_str,
            if s.is_ongoing { 1 } else { 0 },
            s.total_official_volumes,
            s.owned_volumes_count as i64,
            s.missing_volumes_count as i64,
            owned_json,
            missing_json,
            future_json,
            s.last_reconciled_at,
            s.last_error,
            s.review_reason,
            now(),
        ],
    )
    .map_err(|e| AppError::internal(format!("Failed to save series reconciliation: {e}")))?;

    Ok(())
}

/// Retrieves durable reconciliation summary by series ID.
pub fn get_reconciliation(conn: &Connection, series_id: &str) -> Result<Option<ReconciliationSummary>> {
    let sql = "SELECT series_id, manga_id, komga_series_id, mangabaka_id, collection_id,
                      title, edition, status, is_ongoing, total_official_volumes,
                      owned_volumes_count, missing_volumes_count, owned_volumes_json,
                      missing_volumes_json, future_volumes_json, last_reconciled_at,
                      last_error, review_reason
               FROM series_reconciliations
               WHERE series_id = ?1";

    let res = conn
        .query_row(sql, [series_id], |r| {
            let s_id: String = r.get(0)?;
            let m_id: Option<i64> = r.get(1)?;
            let k_id: Option<String> = r.get(2)?;
            let mb_id: Option<i64> = r.get(3)?;
            let col_id: Option<String> = r.get(4)?;
            let t: String = r.get(5)?;
            let ed: String = r.get(6)?;
            let st_raw: String = r.get(7)?;
            let is_ong: i64 = r.get(8)?;
            let tot_vols: Option<u32> = r.get(9)?;
            let owned_cnt: i64 = r.get(10)?;
            let missing_cnt: i64 = r.get(11)?;
            let owned_raw: String = r.get(12)?;
            let missing_raw: String = r.get(13)?;
            let future_raw: String = r.get(14)?;
            let last_rec: i64 = r.get(15)?;
            let last_err: Option<String> = r.get(16)?;
            let rev_reason: Option<String> = r.get(17)?;

            let status = st_raw.parse::<ReconciliationStatus>().unwrap_or(ReconciliationStatus::SyncError);
            let owned_volumes: Vec<f64> = serde_json::from_str(&owned_raw).unwrap_or_default();
            let missing_volumes: Vec<f64> = serde_json::from_str(&missing_raw).unwrap_or_default();
            let future_volumes: Vec<f64> = serde_json::from_str(&future_raw).unwrap_or_default();

            let mut summary = ReconciliationSummary {
                series_id: s_id,
                manga_id: m_id,
                komga_series_id: k_id,
                mangabaka_id: mb_id,
                collection_id: col_id,
                title: t,
                edition: ed,
                status,
                is_ongoing: is_ong != 0,
                total_official_volumes: tot_vols,
                owned_volumes_count: owned_cnt as usize,
                missing_volumes_count: missing_cnt as usize,
                owned_volumes,
                missing_volumes,
                future_volumes,
                last_reconciled_at: last_rec,
                last_error: last_err,
                review_reason: rev_reason,
                display_status: String::new(),
            };
            summary.display_status = summary.compute_display_status();

            Ok(summary)
        })
        .optional()
        .map_err(|e| AppError::internal(format!("Failed to get series reconciliation: {e}")))?;

    Ok(res)
}

/// Retrieves durable reconciliation summary by manga ID.
pub fn get_reconciliation_for_manga(conn: &Connection, manga_id: i64) -> Result<Option<ReconciliationSummary>> {
    let key = format!("manga:{manga_id}");
    if let Some(sum) = get_reconciliation(conn, &key)? {
        return Ok(Some(sum));
    }

    let sql = "SELECT series_id FROM series_reconciliations WHERE manga_id = ?1 LIMIT 1";
    if let Ok(Some(s_id)) = conn.query_row(sql, [manga_id], |r| r.get::<_, String>(0)).optional() {
        return get_reconciliation(conn, &s_id);
    }

    Ok(None)
}

/// Lists all reconciliations stored in the database.
pub fn list_reconciliations(conn: &Connection) -> Result<Vec<ReconciliationSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT series_id, manga_id, komga_series_id, mangabaka_id, collection_id,
                    title, edition, status, is_ongoing, total_official_volumes,
                    owned_volumes_count, missing_volumes_count, owned_volumes_json,
                    missing_volumes_json, future_volumes_json, last_reconciled_at,
                    last_error, review_reason
             FROM series_reconciliations
             ORDER BY updated_at DESC",
        )
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows: rusqlite::Result<Vec<ReconciliationSummary>> = stmt
        .query_map([], |r| {
            let s_id: String = r.get(0)?;
            let m_id: Option<i64> = r.get(1)?;
            let k_id: Option<String> = r.get(2)?;
            let mb_id: Option<i64> = r.get(3)?;
            let col_id: Option<String> = r.get(4)?;
            let t: String = r.get(5)?;
            let ed: String = r.get(6)?;
            let st_raw: String = r.get(7)?;
            let is_ong: i64 = r.get(8)?;
            let tot_vols: Option<u32> = r.get(9)?;
            let owned_cnt: i64 = r.get(10)?;
            let missing_cnt: i64 = r.get(11)?;
            let owned_raw: String = r.get(12)?;
            let missing_raw: String = r.get(13)?;
            let future_raw: String = r.get(14)?;
            let last_rec: i64 = r.get(15)?;
            let last_err: Option<String> = r.get(16)?;
            let rev_reason: Option<String> = r.get(17)?;

            let status = st_raw.parse::<ReconciliationStatus>().unwrap_or(ReconciliationStatus::SyncError);
            let owned_volumes: Vec<f64> = serde_json::from_str(&owned_raw).unwrap_or_default();
            let missing_volumes: Vec<f64> = serde_json::from_str(&missing_raw).unwrap_or_default();
            let future_volumes: Vec<f64> = serde_json::from_str(&future_raw).unwrap_or_default();

            let mut summary = ReconciliationSummary {
                series_id: s_id,
                manga_id: m_id,
                komga_series_id: k_id,
                mangabaka_id: mb_id,
                collection_id: col_id,
                title: t,
                edition: ed,
                status,
                is_ongoing: is_ong != 0,
                total_official_volumes: tot_vols,
                owned_volumes_count: owned_cnt as usize,
                missing_volumes_count: missing_cnt as usize,
                owned_volumes,
                missing_volumes,
                future_volumes,
                last_reconciled_at: last_rec,
                last_error: last_err,
                review_reason: rev_reason,
                display_status: String::new(),
            };
            summary.display_status = summary.compute_display_status();

            Ok(summary)
        })?
        .collect();

    rows.map_err(|e| AppError::internal(format!("Failed to list reconciliations: {e}")))
}

pub fn record_event_full(
    conn: &Connection,
    series_id: &str,
    series_title: &str,
    mangabaka_id: Option<i64>,
    mangabaka_volume_id: Option<&str>,
    edition: &str,
    volume_number: Option<f64>,
    event_type: &str,
    top_candidate_id: Option<&str>,
    candidate_count: i64,
    message: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO reconciliation_events (
            series_id, series_title, mangabaka_id, mangabaka_volume_id, edition,
            volume_number, event_type, top_candidate_id, candidate_count, message, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(series_id, volume_number, event_type) DO UPDATE SET
            top_candidate_id = excluded.top_candidate_id,
            candidate_count = excluded.candidate_count,
            message = excluded.message,
            created_at = excluded.created_at",
        params![
            series_id,
            series_title,
            mangabaka_id,
            mangabaka_volume_id,
            edition,
            volume_number,
            event_type,
            top_candidate_id,
            candidate_count,
            message,
            now(),
        ],
    )
    .map_err(|e| AppError::internal(format!("Failed to record reconciliation event: {e}")))?;

    let event_id: Option<i64> = if let Some(v) = volume_number {
        conn.query_row(
            "SELECT id FROM reconciliation_events
             WHERE series_id = ?1 AND event_type = ?2 AND ABS(volume_number - ?3) < 0.001
             ORDER BY id DESC LIMIT 1",
            params![series_id, event_type, v],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None)
    } else {
        conn.query_row(
            "SELECT id FROM reconciliation_events
             WHERE series_id = ?1 AND event_type = ?2 AND volume_number IS NULL
             ORDER BY id DESC LIMIT 1",
            params![series_id, event_type],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None)
    };

    if let Some(id) = event_id {
        let _ = crate::notification::outbox::enqueue_from_event(
            conn,
            id,
            series_id,
            series_title,
            volume_number,
            event_type,
            message,
        );
    }

    Ok(())
}

/// Records a deduplicated event in `reconciliation_events`.
pub fn record_event(
    conn: &Connection,
    series_title: &str,
    mangabaka_id: Option<i64>,
    volume_number: Option<f64>,
    event_type: &str,
    message: &str,
) -> Result<()> {
    let series_id = format!("title:{}", normalize_title(series_title));
    record_event_full(
        conn,
        &series_id,
        series_title,
        mangabaka_id,
        None,
        "standard",
        volume_number,
        event_type,
        None,
        0,
        message,
    )
}

/// Checks whether a series is followed (tracked = 1 in tracked_catalog).
pub fn is_series_followed(conn: &Connection, manga_id: Option<i64>, title: &str) -> Result<bool> {
    if let Some(m_id) = manga_id {
        if let Ok(Some(tracked)) = conn
            .query_row(
                "SELECT tracked FROM tracked_catalog WHERE manga_id = ?1",
                [m_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        {
            return Ok(tracked != 0);
        }
    }

    let norm = normalize_title(title);
    let mut stmt = conn
        .prepare("SELECT title, tracked FROM tracked_catalog")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| AppError::internal(e.to_string()))?;

    while let Some(row) = rows.next().map_err(|e| AppError::internal(e.to_string()))? {
        let t: String = row.get(0).map_err(|e| AppError::internal(e.to_string()))?;
        if normalize_title(&t) == norm {
            let trk: i64 = row.get(1).map_err(|e| AppError::internal(e.to_string()))?;
            return Ok(trk != 0);
        }
    }

    Ok(false)
}

/// Follows a series in the tracked catalog (reversible, non-destructive, auto_acquire = false).
pub fn follow_series(
    conn: &Connection,
    title: &str,
    komga_series_id: Option<&str>,
    manga_id: Option<i64>,
) -> Result<TrackedCatalogEntry> {
    add_or_update_tracked(conn, title, "mango", komga_series_id, manga_id)
}

/// Unfollows a series in the tracked catalog.
/// Strictly leaves canonical files and Komga books untouched.
pub fn unfollow_series(conn: &Connection, manga_id: Option<i64>, title: &str) -> Result<()> {
    if let Some(m_id) = manga_id {
        let affected = conn
            .execute(
                "UPDATE tracked_catalog SET tracked = 0, updated_at = ?1 WHERE manga_id = ?2",
                params![now(), m_id],
            )
            .map_err(|e| AppError::internal(e.to_string()))?;

        if affected > 0 {
            return Ok(());
        }
    }

    let norm = normalize_title(title);
    let mut stmt = conn
        .prepare("SELECT id, title FROM tracked_catalog")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| AppError::internal(e.to_string()))?;

    for row in rows.flatten() {
        if normalize_title(&row.1) == norm {
            set_tracked(conn, row.0, false)?;
            return Ok(());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};
    use serde_json::json;

    fn setup_test_db() -> (Db, rusqlite::Connection) {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut conn).unwrap();
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        (db, conn)
    }

    #[tokio::test]
    async fn test_ongoing_series_all_volumes_owned_up_to_date() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/101", get(|| async { Json(json!({"data": {"id": 101, "title": "Blood on the Tracks", "status": "ongoing"}})) }))
            .route("/series/101/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 11}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": (1..=11).map(|i| {
                json!({"id": format!("w{i}"), "sequence_numeric": i as f64, "release_date": "2021-01-01"})
            }).collect::<Vec<_>>()})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        // Pre-populate owned volumes 1..=11 in Komga books mock
        let komga_app = Router::new()
            .route("/api/v1/series/k101/books", get(|| async { Json(json!({
                "content": (1..=11).map(|i| {
                    json!({"id": format!("b{i}"), "seriesId": "k101", "name": format!("Volume {i}"), "number": i as f64})
                }).collect::<Vec<_>>(),
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k101".to_string()),
            title: "Blood on the Tracks".to_string(),
            edition: Some("standard".to_string()),
        };

        // Record mapping so identity resolution finds mb:101
        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k101".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "101".to_string(),
                tracker_title: Some("Blood on the Tracks".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::UpToDate);
        assert!(summary.is_ongoing);
        assert_eq!(summary.owned_volumes_count, 11);
        assert_eq!(summary.missing_volumes_count, 0);
        assert!(summary.missing_volumes.is_empty());
        assert_eq!(summary.display_status, "11 volumes owned · Up to date · Ongoing");

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_completed_series_all_volumes_owned_up_to_date() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/102", get(|| async { Json(json!({"data": {"id": 102, "title": "Completed Series", "status": "completed"}})) }))
            .route("/series/102/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 10}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": (1..=10).map(|i| {
                json!({"id": format!("w{i}"), "sequence_numeric": i as f64, "release_date": "2020-01-01"})
            }).collect::<Vec<_>>()})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        let komga_app = Router::new()
            .route("/api/v1/series/k102/books", get(|| async { Json(json!({
                "content": (1..=10).map(|i| {
                    json!({"id": format!("b{i}"), "seriesId": "k102", "name": format!("Volume {i}"), "number": i as f64})
                }).collect::<Vec<_>>(),
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k102".to_string()),
            title: "Completed Series".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k102".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "102".to_string(),
                tracker_title: Some("Completed Series".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::UpToDate);
        assert!(!summary.is_ongoing);
        assert_eq!(summary.owned_volumes_count, 10);
        assert_eq!(summary.missing_volumes_count, 0);
        assert_eq!(summary.display_status, "10 volumes owned · Up to date · Completed");

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_ongoing_series_missing_published_volume() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/103", get(|| async { Json(json!({"data": {"id": 103, "title": "Ongoing Missing", "status": "ongoing"}})) }))
            .route("/series/103/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 11}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": (1..=11).map(|i| {
                json!({"id": format!("w{i}"), "sequence_numeric": i as f64, "release_date": "2021-01-01"})
            }).collect::<Vec<_>>()})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        // Own volumes 1..=10 only, missing volume 11
        let komga_app = Router::new()
            .route("/api/v1/series/k103/books", get(|| async { Json(json!({
                "content": (1..=10).map(|i| {
                    json!({"id": format!("b{i}"), "seriesId": "k103", "name": format!("Volume {i}"), "number": i as f64})
                }).collect::<Vec<_>>(),
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k103".to_string()),
            title: "Ongoing Missing".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k103".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "103".to_string(),
                tracker_title: Some("Ongoing Missing".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::MissingVolume);
        assert!(summary.is_ongoing);
        assert_eq!(summary.owned_volumes_count, 10);
        assert_eq!(summary.missing_volumes_count, 1);
        assert_eq!(summary.missing_volumes, vec![11.0]);
        assert_eq!(summary.display_status, "10 / 11 volumes owned · 1 missing");

        // Verify event logged in reconciliation_events
        let events: Vec<(String, f64, String)> = db.call(|conn| {
            let mut stmt = conn.prepare("SELECT series_title, volume_number, event_type FROM reconciliation_events")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.flatten().collect())
        }).await.unwrap();

        assert!(events.iter().any(|(t, v, e)| t == "Ongoing Missing" && (*v - 11.0).abs() < 0.001 && e == "MISSING_TARGET"));

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_multiple_volumes_missing_with_gap() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/104", get(|| async { Json(json!({"data": {"id": 104, "title": "Gap Series", "status": "completed"}})) }))
            .route("/series/104/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 5}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": (1..=5).map(|i| {
                json!({"id": format!("w{i}"), "sequence_numeric": i as f64, "release_date": "2020-01-01"})
            }).collect::<Vec<_>>()})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        // Own volumes 1, 3, 5 only (gap on 2 and 4)
        let komga_app = Router::new()
            .route("/api/v1/series/k104/books", get(|| async { Json(json!({
                "content": [
                    {"id": "b1", "seriesId": "k104", "name": "Volume 1", "number": 1.0},
                    {"id": "b3", "seriesId": "k104", "name": "Volume 3", "number": 3.0},
                    {"id": "b5", "seriesId": "k104", "name": "Volume 5", "number": 5.0},
                ],
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k104".to_string()),
            title: "Gap Series".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k104".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "104".to_string(),
                tracker_title: Some("Gap Series".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::MissingVolume);
        assert_eq!(summary.owned_volumes_count, 3);
        assert_eq!(summary.missing_volumes_count, 2);
        assert_eq!(summary.missing_volumes, vec![2.0, 4.0]);
        assert_eq!(summary.display_status, "3 / 5 volumes owned · 2 missing");

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_future_announced_unreleased_volume_not_missing() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/105", get(|| async { Json(json!({"data": {"id": 105, "title": "Future Series", "status": "ongoing"}})) }))
            .route("/series/105/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 6}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": [
                {"id": "w1", "sequence_numeric": 1.0, "release_date": "2021-01-01"},
                {"id": "w2", "sequence_numeric": 2.0, "release_date": "2021-06-01"},
                {"id": "w3", "sequence_numeric": 3.0, "release_date": "2022-01-01"},
                {"id": "w4", "sequence_numeric": 4.0, "release_date": "2022-06-01"},
                {"id": "w5", "sequence_numeric": 5.0, "release_date": "2023-01-01"},
                {"id": "w6", "sequence_numeric": 6.0, "release_date": "2029-12-31"}, // Future announced!
            ]})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        // Own volumes 1..=5 in Komga
        let komga_app = Router::new()
            .route("/api/v1/series/k105/books", get(|| async { Json(json!({
                "content": (1..=5).map(|i| {
                    json!({"id": format!("b{i}"), "seriesId": "k105", "name": format!("Volume {i}"), "number": i as f64})
                }).collect::<Vec<_>>(),
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k105".to_string()),
            title: "Future Series".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k105".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "105".to_string(),
                tracker_title: Some("Future Series".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        // Volume 6 is unreleased in future, so series is UpToDate!
        assert_eq!(summary.status, ReconciliationStatus::UpToDate);
        assert!(summary.is_ongoing);
        assert_eq!(summary.owned_volumes_count, 5);
        assert_eq!(summary.missing_volumes_count, 0);
        assert!(summary.missing_volumes.is_empty());
        assert_eq!(summary.future_volumes, vec![6.0]);
        assert_eq!(summary.display_status, "5 volumes owned · Up to date · Ongoing");

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_highest_durable_canonical_ownership_wins() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/106", get(|| async { Json(json!({"data": {"id": 106, "title": "Published Wins", "status": "ongoing"}})) }))
            .route("/series/106/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 2}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": [
                {"id": "w1", "sequence_numeric": 1.0, "release_date": "2021-01-01"},
                {"id": "w2", "sequence_numeric": 2.0, "release_date": "2021-06-01"},
            ]})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        // Komga only has volume 1 (e.g. Komga hasn't indexed volume 2 yet)
        let komga_app = Router::new()
            .route("/api/v1/series/k106/books", get(|| async { Json(json!({
                "content": [
                    {"id": "b1", "seriesId": "k106", "name": "Volume 1", "number": 1.0}
                ],
                "totalPages": 1,
                "last": true
            })) }));
        let k_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let k_addr = k_listener.local_addr().unwrap();
        let k_server = tokio::spawn(async move { axum::serve(k_listener, komga_app).await.unwrap(); });

        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{k_addr}")), None);

        // BUT volume 2 was already canonically published in downloads table!
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (42, 1, '/m/42', 'Published Wins')", []).unwrap();
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at) VALUES (901, 42, '/c/901', 'Published Wins - Volume 02.cbz', 2.0, 0, 0)", []).unwrap();
            conn.execute("INSERT INTO downloads (chapter_id, state, position, added_at, published_path) VALUES (901, 'completed', 0, 0, '/srv/media/manga/Published Wins/Published Wins - Volume 02.cbz')", []).unwrap();
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k106".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "106".to_string(),
                tracker_title: Some("Published Wins".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let target = ReconciliationTarget {
            manga_id: Some(42),
            komga_series_id: Some("k106".to_string()),
            title: "Published Wins".to_string(),
            edition: None,
        };

        let summary = reconcile_series(&db, Some(&kc), &mb, &target).await.unwrap();

        // Volume 2 must be owned because canonical publication wins over Komga delay!
        assert_eq!(summary.status, ReconciliationStatus::UpToDate);
        assert_eq!(summary.owned_volumes_count, 2);
        assert_eq!(summary.missing_volumes_count, 0);

        server.abort();
        k_server.abort();
    }

    #[tokio::test]
    async fn test_follow_and_unfollow_reversible_non_destructive() {
        let (db, _) = setup_test_db();
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (10, 1, '/m/10', 'Vinland Saga')", []).unwrap();
            Ok(())
        }).await.unwrap();

        let is_fol = db.call(|conn| is_series_followed(conn, Some(10), "Vinland Saga")).await.unwrap();
        assert!(!is_fol);

        // Follow series
        db.call(|conn| follow_series(conn, "Vinland Saga", Some("k_vinland"), Some(10))).await.unwrap();
        let is_fol_after = db.call(|conn| is_series_followed(conn, Some(10), "Vinland Saga")).await.unwrap();
        assert!(is_fol_after);

        // Unfollow series
        db.call(|conn| unfollow_series(conn, Some(10), "Vinland Saga")).await.unwrap();
        let is_fol_unfollowed = db.call(|conn| is_series_followed(conn, Some(10), "Vinland Saga")).await.unwrap();
        assert!(!is_fol_unfollowed);

        // Re-follow works seamlessly
        db.call(|conn| follow_series(conn, "Vinland Saga", Some("k_vinland"), Some(10))).await.unwrap();
        let is_fol_refollowed = db.call(|conn| is_series_followed(conn, Some(10), "Vinland Saga")).await.unwrap();
        assert!(is_fol_refollowed);
    }

    #[tokio::test]
    async fn test_transient_error_preserves_last_known_good_state() {
        let (db, _) = setup_test_db();
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (50, 1, '/m/50', 'Good Series')", []).unwrap();
            Ok(())
        }).await.unwrap();

        // Seed previous successful reconciliation
        let prev = ReconciliationSummary {
            series_id: "manga:50".to_string(),
            manga_id: Some(50),
            komga_series_id: Some("k50".to_string()),
            mangabaka_id: Some(200),
            collection_id: Some("col-200".to_string()),
            title: "Good Series".to_string(),
            edition: "standard".to_string(),
            status: ReconciliationStatus::UpToDate,
            is_ongoing: true,
            total_official_volumes: Some(8),
            owned_volumes_count: 8,
            missing_volumes_count: 0,
            owned_volumes: (1..=8).map(|i| i as f64).collect(),
            missing_volumes: vec![],
            future_volumes: vec![],
            last_reconciled_at: 1000,
            last_error: None,
            review_reason: None,
            display_status: "8 volumes owned · Up to date · Ongoing".to_string(),
        };
        db.call(move |conn| save_reconciliation(conn, &prev)).await.unwrap();

        // Mock server returning HTTP 500 error
        let app = Router::new()
            .route("/series/200", get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Error") }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        let target = ReconciliationTarget {
            manga_id: Some(50),
            komga_series_id: Some("k50".to_string()),
            title: "Good Series".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k50".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "200".to_string(),
                tracker_title: Some("Good Series".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, None, &mb, &target).await.unwrap();

        // Must report SyncError, but PRESERVE the owned volumes and previous known counts!
        assert_eq!(summary.status, ReconciliationStatus::SyncError);
        assert!(summary.last_error.is_some());
        assert_eq!(summary.owned_volumes_count, 8);
        assert_eq!(summary.owned_volumes.len(), 8);

        server.abort();
    }

    #[tokio::test]
    async fn test_ambiguous_collection_triggers_review_required() {
        let (db, _) = setup_test_db();
        // Return 2 ambiguous digital collections for the same edition/language
        let app = Router::new()
            .route("/series/300", get(|| async { Json(json!({"data": {"id": 300, "title": "Ambiguous Series", "status": "ongoing"}})) }))
            .route("/series/300/collections", get(|| async { Json(json!({"data": [
                {"id": "col-1", "language": "en", "edition": "Special Edition", "medium": "digital", "total_works": 5},
                {"id": "col-2", "language": "en", "edition": "Special Edition", "medium": "digital", "total_works": 5}
            ]})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k300".to_string()),
            title: "Ambiguous Series".to_string(),
            edition: Some("Special Edition".to_string()),
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k300".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "300".to_string(),
                tracker_title: Some("Ambiguous Series".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        let summary = reconcile_series(&db, None, &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::ReviewRequired);
        assert!(summary.review_reason.is_some());
        assert!(summary.display_status.contains("Review required"));

        // Check action required event
        let events: Vec<(String, String)> = db.call(|conn| {
            let mut stmt = conn.prepare("SELECT series_title, event_type FROM reconciliation_events")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.flatten().collect())
        }).await.unwrap();
        assert!(events.iter().any(|(t, e)| t == "Ambiguous Series" && e == "ACTION_REQUIRED"));

        server.abort();
    }

    #[tokio::test]
    async fn test_reconciliation_is_strictly_read_only_and_never_acquires() {
        let (db, _) = setup_test_db();
        let app = Router::new()
            .route("/series/400", get(|| async { Json(json!({"data": {"id": 400, "title": "ReadOnly Test", "status": "ongoing"}})) }))
            .route("/series/400/collections", get(|| async { Json(json!({"data": [
                {"id": "col-std", "language": "en", "edition": "Standard Edition", "medium": "digital", "total_works": 5}
            ]})) }))
            .route("/collections/col-std/works", get(|| async { Json(json!({"data": (1..=5).map(|i| {
                json!({"id": format!("w{i}"), "sequence_numeric": i as f64, "release_date": "2020-01-01"})
            }).collect::<Vec<_>>()})) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let mb = MangaBakaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);

        let target = ReconciliationTarget {
            manga_id: None,
            komga_series_id: Some("k400".to_string()),
            title: "ReadOnly Test".to_string(),
            edition: None,
        };

        db.call(|conn| {
            super::super::mapping::save_mapping(conn, &super::super::model::TrackerMapping {
                komga_series_id: "k400".to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: "400".to_string(),
                tracker_title: Some("ReadOnly Test".to_string()),
                edition_id: None,
                mapping_source: super::super::model::MappingSource::Manual,
                confidence: super::super::model::MappingConfidence::Resolved,
                manual_override: true,
                last_validated: Some(crate::db::now()),
                updated_at: crate::db::now(),
            })
        }).await.unwrap();

        // Count downloads before reconciliation
        let initial_dl_count: i64 = db.call(|conn| {
            conn.query_row("SELECT count(*) FROM downloads", [], |r| r.get(0)).map_err(AppError::from)
        }).await.unwrap();

        let summary = reconcile_series(&db, None, &mb, &target).await.unwrap();

        // 5 volumes exist, 0 owned -> MissingVolume status
        assert_eq!(summary.status, ReconciliationStatus::MissingVolume);
        assert_eq!(summary.missing_volumes_count, 5);

        // Verification: download queue remains completely untouched!
        let final_dl_count: i64 = db.call(|conn| {
            conn.query_row("SELECT count(*) FROM downloads", [], |r| r.get(0)).map_err(AppError::from)
        }).await.unwrap();
        assert_eq!(initial_dl_count, final_dl_count);

        server.abort();
    }
}

