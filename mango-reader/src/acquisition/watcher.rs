//! Release Availability Watcher & Notification Subsystem (Milestone M6.8.4)
//!
//! Provides durable, database-backed availability monitoring for missing manga volumes across
//! providers (Madokami, Nyaa) without enabling automatic acquisition.
//!
//! Invariants:
//! - Monitoring != Auto-Acquisition: A title can be monitored and notify without authorizing auto-acquire.
//! - Canonical physical ownership (Komga) is verified before every provider search.
//! - MangaBaka remains authoritative for logical volume existence.
//! - Only exact-volume eligible candidates (meeting all M6.8 hard gates) trigger notifications.
//! - Durable deduplication: repeated checks or restarts never spam notifications.
//! - Transient provider errors (timeouts, rate limits) are tracked with backoff and NEVER marked as unavailable.
//! - Zero TorBox jobs, zero Madokami media downloads, zero canonical publications.

use std::str::FromStr;
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::auto_acquire::is_candidate_auto_eligible;
use crate::acquisition::discovery::discover_for_volume;
use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::release_discovery::is_volume_owned;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::sync::reconciliation::{get_reconciliation, record_event_full, ReconciliationStatus};

fn pseudo_jitter(base: i64, pct: f64) -> i64 {
    let mut buf = [0u8; 4];
    let _ = getrandom::getrandom(&mut buf);
    let u = u32::from_le_bytes(buf) as f64 / u32::MAX as f64;
    let factor = (1.0 - pct) + (2.0 * pct * u);
    ((base as f64) * factor).round() as i64
}

fn pseudo_stagger(min: i64, max: i64) -> i64 {
    let mut buf = [0u8; 4];
    let _ = getrandom::getrandom(&mut buf);
    let u = u32::from_le_bytes(buf) as f64 / u32::MAX as f64;
    min + ((max - min) as f64 * u).round() as i64
}

/// Structured lifecycle states for availability watchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatcherState {
    Upcoming,
    ReleasedAwaitingProvider,
    EligibleCandidateFound,
    Notified,
    Acquired,
    CollectionUnavailable,
    ReviewRequired,
    ProviderUnavailable,
    RateLimited,
    OwnershipDetected,
    MonitoringDisabled,
}

impl WatcherState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Upcoming => "UPCOMING",
            Self::ReleasedAwaitingProvider => "RELEASED_AWAITING_PROVIDER",
            Self::EligibleCandidateFound => "ELIGIBLE_CANDIDATE_FOUND",
            Self::Notified => "NOTIFIED",
            Self::Acquired => "ACQUIRED",
            Self::CollectionUnavailable => "COLLECTION_UNAVAILABLE",
            Self::ReviewRequired => "REVIEW_REQUIRED",
            Self::ProviderUnavailable => "PROVIDER_UNAVAILABLE",
            Self::RateLimited => "RATE_LIMITED",
            Self::OwnershipDetected => "OWNERSHIP_DETECTED",
            Self::MonitoringDisabled => "MONITORING_DISABLED",
        }
    }
}

impl FromStr for WatcherState {
    type Err = AppError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "UPCOMING" => Ok(Self::Upcoming),
            "RELEASED_AWAITING_PROVIDER" => Ok(Self::ReleasedAwaitingProvider),
            "ELIGIBLE_CANDIDATE_FOUND" => Ok(Self::EligibleCandidateFound),
            "NOTIFIED" => Ok(Self::Notified),
            "ACQUIRED" => Ok(Self::Acquired),
            "COLLECTION_UNAVAILABLE" => Ok(Self::CollectionUnavailable),
            "REVIEW_REQUIRED" => Ok(Self::ReviewRequired),
            "PROVIDER_UNAVAILABLE" => Ok(Self::ProviderUnavailable),
            "RATE_LIMITED" => Ok(Self::RateLimited),
            "OWNERSHIP_DETECTED" => Ok(Self::OwnershipDetected),
            "MONITORING_DISABLED" => Ok(Self::MonitoringDisabled),
            _ => Err(AppError::bad_request(format!("Unknown watcher state: {s}"))),
        }
    }
}

/// Durable database-backed availability watcher record for a specific volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAvailabilityWatcher {
    pub id: i64,
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: f64,
    pub mangabaka_id: Option<i64>,
    pub collection_id: Option<String>,
    pub work_id: Option<String>,
    pub market: String,
    pub edition: String,
    pub release_date: Option<String>,
    pub is_released: bool,
    pub is_owned: bool,
    pub monitoring_enabled: bool,
    pub notify_on_available: bool,
    pub state: WatcherState,
    pub last_attempt_at: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub next_check_at: i64,
    pub check_interval_secs: i64,
    pub consecutive_failures: i64,
    pub last_provider_checked: Option<String>,
    pub last_check_status: Option<String>,
    pub last_error: Option<String>,
    pub top_candidate_id: Option<String>,
    pub top_candidate_provider: Option<String>,
    pub top_candidate_title: Option<String>,
    pub top_candidate_fingerprint: Option<String>,
    pub candidate_count: i64,
    pub notified_at: Option<i64>,
    pub notification_event_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Title-level monitoring and notification preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleMonitoringSettings {
    pub manga_id: i64,
    pub release_monitoring_enabled: bool,
    pub release_notify_enabled: bool,
    pub auto_acquire_enabled: bool,
}

/// Computes next polling interval with jitter based on release date proximity and failure count.
pub fn calculate_watcher_interval_secs(
    release_date_opt: Option<&str>,
    consecutive_failures: i64,
    today_str: &str,
) -> i64 {
    // 1. If experiencing consecutive failures, apply exponential backoff
    if consecutive_failures > 0 {
        let base_backoff = match consecutive_failures {
            1 => 1800,       // 30 min
            2 => 3600,       // 1 hour
            3 => 7200,       // 2 hours
            4 => 14400,      // 4 hours
            5 => 28800,      // 8 hours
            _ => 86400,      // 24 hours max
        };
        return pseudo_jitter(base_backoff, 0.10);
    }

    // 2. Proximity to release date
    let base_interval = if let Some(rdate) = release_date_opt {
        if rdate.len() == 10 && today_str.len() == 10 {
            // Check days difference
            if rdate > today_str {
                // Upcoming release: wait until release date
                86400
            } else {
                // Check if within 7 days of release date
                // Approximate: if same month/year or recent
                14400 // 4 hours for newly released
            }
        } else {
            86400
        }
    } else {
        // Unknown release date: default 24 hours
        86400
    };

    pseudo_jitter(base_interval, 0.15)
}

/// Synchronizes availability watcher rows for a series based on current reconciliation state.
pub fn sync_watchers_for_series(
    conn: &Connection,
    manga_id: i64,
    series_id: &str,
    series_title: &str,
    mangabaka_id: Option<i64>,
    collection_id: Option<&str>,
    missing_volumes: &[f64],
    future_volumes: &[f64],
    owned_volumes: &[f64],
) -> Result<Vec<ReleaseAvailabilityWatcher>> {
    let current_time = now();
    let today_str: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| "2026-11-04".to_string());

    // Fetch title-level monitoring preferences
    let (monitoring_enabled, notify_enabled): (bool, bool) = conn
        .query_row(
            "SELECT release_monitoring_enabled, release_notify_enabled FROM manga WHERE id = ?1",
            [manga_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
        )
        .unwrap_or((true, true));

    // 1. Process owned volumes: ensure any existing watcher is marked OWNERSHIP_DETECTED
    for &vol in owned_volumes {
        let _ = conn.execute(
            "UPDATE release_availability_watchers
             SET is_owned = 1, state = 'OWNERSHIP_DETECTED', next_check_at = 0, updated_at = ?1
             WHERE series_id = ?2 AND ABS(volume_number - ?3) < 0.001 AND state != 'OWNERSHIP_DETECTED'",
            params![current_time, series_id, vol],
        );
    }

    // 2. Process missing volumes (released)
    for &vol in missing_volumes {
        let is_owned = owned_volumes.iter().any(|&o| (o - vol).abs() < 0.001);
        if is_owned {
            continue;
        }

        // Try to fetch OpenTome release date if available
        let rdate: Option<String> = conn
            .query_row(
                "SELECT release_date FROM release_discovery_targets
                 WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
                params![series_id, vol],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);

        let initial_state = if collection_id.is_none() {
            WatcherState::CollectionUnavailable
        } else {
            WatcherState::ReleasedAwaitingProvider
        };

        let interval = calculate_watcher_interval_secs(rdate.as_deref(), 0, &today_str);
        let next_check = current_time + pseudo_stagger(60, 300); // initial check staggered

        conn.execute(
            "INSERT INTO release_availability_watchers (
                series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                market, edition, release_date, is_released, is_owned, monitoring_enabled,
                notify_on_available, state, next_check_at, check_interval_secs, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'en', 'standard', ?7, 1, 0, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
            ON CONFLICT(series_id, volume_number, market, edition) DO UPDATE SET
                manga_id = excluded.manga_id,
                series_title = excluded.series_title,
                mangabaka_id = excluded.mangabaka_id,
                collection_id = excluded.collection_id,
                release_date = COALESCE(excluded.release_date, release_availability_watchers.release_date),
                is_released = 1,
                monitoring_enabled = excluded.monitoring_enabled,
                notify_on_available = excluded.notify_on_available,
                updated_at = excluded.updated_at
            WHERE state NOT IN ('OWNERSHIP_DETECTED', 'ACQUIRED', 'NOTIFIED')",
            params![
                series_id,
                manga_id,
                series_title,
                vol,
                mangabaka_id,
                collection_id,
                rdate,
                if monitoring_enabled { 1 } else { 0 },
                if notify_enabled { 1 } else { 0 },
                initial_state.as_str(),
                next_check,
                interval,
                current_time,
            ],
        )?;
    }

    // 3. Process future volumes (upcoming)
    for &vol in future_volumes {
        let is_owned = owned_volumes.iter().any(|&o| (o - vol).abs() < 0.001);
        if is_owned {
            continue;
        }

        let rdate: Option<String> = conn
            .query_row(
                "SELECT release_date FROM release_discovery_targets
                 WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
                params![series_id, vol],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);

        let interval = calculate_watcher_interval_secs(rdate.as_deref(), 0, &today_str);
        let next_check = current_time + interval;

        conn.execute(
            "INSERT INTO release_availability_watchers (
                series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                market, edition, release_date, is_released, is_owned, monitoring_enabled,
                notify_on_available, state, next_check_at, check_interval_secs, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'en', 'standard', ?7, 0, 0, ?8, ?9, 'UPCOMING', ?10, ?11, ?12, ?12)
            ON CONFLICT(series_id, volume_number, market, edition) DO UPDATE SET
                manga_id = excluded.manga_id,
                series_title = excluded.series_title,
                mangabaka_id = excluded.mangabaka_id,
                collection_id = excluded.collection_id,
                release_date = COALESCE(excluded.release_date, release_availability_watchers.release_date),
                updated_at = excluded.updated_at
            WHERE state NOT IN ('OWNERSHIP_DETECTED', 'ACQUIRED', 'NOTIFIED')",
            params![
                series_id,
                manga_id,
                series_title,
                vol,
                mangabaka_id,
                collection_id,
                rdate,
                if monitoring_enabled { 1 } else { 0 },
                if notify_enabled { 1 } else { 0 },
                next_check,
                interval,
                current_time,
            ],
        )?;
    }

    list_watchers_for_series(conn, series_id)
}

/// Retrieves all watchers for a given series_id.
pub fn list_watchers_for_series(
    conn: &Connection,
    series_id: &str,
) -> Result<Vec<ReleaseAvailabilityWatcher>> {
    let mut stmt = conn.prepare(
        "SELECT id, series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                work_id, market, edition, release_date, is_released, is_owned, monitoring_enabled,
                notify_on_available, state, last_attempt_at, last_checked_at, next_check_at,
                check_interval_secs, consecutive_failures, last_provider_checked, last_check_status,
                last_error, top_candidate_id, top_candidate_provider, top_candidate_title,
                top_candidate_fingerprint, candidate_count, notified_at, notification_event_id,
                created_at, updated_at
         FROM release_availability_watchers
         WHERE series_id = ?1
         ORDER BY volume_number ASC",
    ).map_err(|e| AppError::internal(format!("Prepare list watchers error: {e}")))?;

    let rows = stmt.query_map([series_id], map_watcher_row)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// Retrieves a watcher by primary key ID.
pub fn get_watcher_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<ReleaseAvailabilityWatcher>> {
    conn.query_row(
        "SELECT id, series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                work_id, market, edition, release_date, is_released, is_owned, monitoring_enabled,
                notify_on_available, state, last_attempt_at, last_checked_at, next_check_at,
                check_interval_secs, consecutive_failures, last_provider_checked, last_check_status,
                last_error, top_candidate_id, top_candidate_provider, top_candidate_title,
                top_candidate_fingerprint, candidate_count, notified_at, notification_event_id,
                created_at, updated_at
         FROM release_availability_watchers
         WHERE id = ?1",
        [id],
        map_watcher_row,
    )
    .optional()
    .map_err(|e| AppError::internal(format!("Get watcher error: {e}")))
}

fn map_watcher_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseAvailabilityWatcher> {
    let state_str: String = r.get(15)?;
    let state = WatcherState::from_str(&state_str).unwrap_or(WatcherState::ReleasedAwaitingProvider);

    Ok(ReleaseAvailabilityWatcher {
        id: r.get(0)?,
        series_id: r.get(1)?,
        manga_id: r.get(2)?,
        series_title: r.get(3)?,
        volume_number: r.get(4)?,
        mangabaka_id: r.get(5)?,
        collection_id: r.get(6)?,
        work_id: r.get(7)?,
        market: r.get(8)?,
        edition: r.get(9)?,
        release_date: r.get(10)?,
        is_released: r.get::<_, i64>(11)? != 0,
        is_owned: r.get::<_, i64>(12)? != 0,
        monitoring_enabled: r.get::<_, i64>(13)? != 0,
        notify_on_available: r.get::<_, i64>(14)? != 0,
        state,
        last_attempt_at: r.get(16)?,
        last_checked_at: r.get(17)?,
        next_check_at: r.get(18)?,
        check_interval_secs: r.get(19)?,
        consecutive_failures: r.get(20)?,
        last_provider_checked: r.get(21)?,
        last_check_status: r.get(22)?,
        last_error: r.get(23)?,
        top_candidate_id: r.get(24)?,
        top_candidate_provider: r.get(25)?,
        top_candidate_title: r.get(26)?,
        top_candidate_fingerprint: r.get(27)?,
        candidate_count: r.get(28)?,
        notified_at: r.get(29)?,
        notification_event_id: r.get(30)?,
        created_at: r.get(31)?,
        updated_at: r.get(32)?,
    })
}

/// Evaluates a single watcher record against discovery providers and updates state.
///
/// Strict Guarantees:
/// - Verifies canonical physical ownership first.
/// - Uses existing candidate eligibility hard gates (`is_candidate_auto_eligible`).
/// - Emits deduplicated ntfy alert via `record_event_full`.
/// - Never triggers downloads, TorBox jobs, or file mutations.
pub async fn evaluate_watcher_check(
    db: &Db,
    http: Client,
    watcher_id: i64,
) -> Result<Option<ReleaseAvailabilityWatcher>> {
    let watcher = db
        .call(move |conn| get_watcher_by_id(conn, watcher_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Watcher #{watcher_id} not found")))?;

    // If monitoring is disabled, skip check
    if !watcher.monitoring_enabled {
        return Ok(Some(watcher));
    }

    let current_time = now();
    let series_id = watcher.series_id.clone();
    let manga_id = watcher.manga_id;
    let volume_number = watcher.volume_number;
    let series_title = watcher.series_title.clone();

    // 1. Pre-flight Check: Canonical Komga Ownership
    let is_owned = db
        .call(move |conn| {
            if let Some(mid) = manga_id {
                is_volume_owned(conn, mid, &series_id, volume_number)
            } else {
                Ok(false)
            }
        })
        .await?;

    if is_owned {
        let updated = db
            .call(move |conn| {
                conn.execute(
                    "UPDATE release_availability_watchers
                     SET is_owned = 1, state = 'OWNERSHIP_DETECTED', next_check_at = 0,
                         last_checked_at = ?1, updated_at = ?1
                     WHERE id = ?2",
                    params![current_time, watcher_id],
                )?;
                get_watcher_by_id(conn, watcher_id)
            })
            .await?;
        return Ok(updated);
    }

    // 2. Pre-flight Check: MangaBaka reconciliation status
    let s_id = watcher.series_id.clone();
    let reconciliation = db.call(move |conn| get_reconciliation(conn, &s_id)).await?;

    if let Some(ref recon) = reconciliation {
        if recon.status == ReconciliationStatus::CollectionUnavailable {
            let updated = db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE release_availability_watchers
                         SET state = 'COLLECTION_UNAVAILABLE', last_checked_at = ?1,
                             last_error = 'MangaBaka collection unavailable', updated_at = ?1
                         WHERE id = ?2",
                        params![current_time, watcher_id],
                    )?;
                    get_watcher_by_id(conn, watcher_id)
                })
                .await?;
            return Ok(updated);
        } else if recon.status == ReconciliationStatus::ReviewRequired {
            let updated = db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE release_availability_watchers
                         SET state = 'REVIEW_REQUIRED', last_checked_at = ?1,
                             last_error = 'Reconciliation requires review', updated_at = ?1
                         WHERE id = ?2",
                        params![current_time, watcher_id],
                    )?;
                    get_watcher_by_id(conn, watcher_id)
                })
                .await?;
            return Ok(updated);
        }
    }

    // 3. Pre-flight Check: Future release date gate
    let today_str: String = db
        .call(|conn| {
            conn.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
                .map_err(AppError::from)
        })
        .await
        .unwrap_or_else(|_| "2026-11-04".to_string());

    if let Some(ref rdate) = watcher.release_date {
        if rdate > &today_str {
            // Unreleased future target
            let updated = db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE release_availability_watchers
                         SET state = 'UPCOMING', is_released = 0, last_checked_at = ?1, updated_at = ?1
                         WHERE id = ?2",
                        params![current_time, watcher_id],
                    )?;
                    get_watcher_by_id(conn, watcher_id)
                })
                .await?;
            return Ok(updated);
        }
    }

    // 4. Record attempt timestamp
    db.call(move |conn| {
        conn.execute(
            "UPDATE release_availability_watchers SET last_attempt_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![current_time, watcher_id],
        )?;
        Ok(())
    })
    .await?;

    // 5. Query Discovery Layer
    let discovery_res = discover_for_volume(db, http, &watcher.series_id, volume_number).await?;

    let candidate_count = discovery_res.candidate_count;
    let profile = AcquisitionProfile::default();

    // 6. Filter candidates using strict M6.8 hard gates
    let mut eligible_candidate: Option<crate::acquisition::providers::ReleaseCandidate> = None;
    for cand in &discovery_res.candidates {
        let persisted = PersistedCandidate {
            series_id: watcher.series_id.clone(),
            volume_number,
            candidate_id: cand.candidate_id.clone(),
            provider: cand.provider.clone(),
            provider_item_id: cand.provider_item_id.clone(),
            title: cand.title.clone(),
            raw_title: cand.raw_title.clone(),
            match_type: cand.match_type.as_str().to_string(),
            size_bytes: cand.size_bytes.map(|s| s as u64),
            uploader: cand.uploader.clone(),
            is_trusted: cand.is_trusted,
            is_digital: cand.is_digital,
            score: cand.score,
            score_explanation: cand.score_explanation.clone(),
            download_url: cand.download_url.clone(),
            magnet_url: cand.magnet_url.clone(),
            info_hash: cand.info_hash.clone(),
            seeders: cand.seeders.map(|s| s as u32),
            leechers: cand.leechers.map(|s| s as u32),
            published_at: cand.published_at,
        };

        if is_candidate_auto_eligible(&persisted, &profile).is_ok() {
            eligible_candidate = Some(cand.clone());
            break;
        }
    }

    // 7. Determine state transition and schedule
    let has_provider_errors = !discovery_res.provider_errors.is_empty();
    let (new_state, last_status, err_msg, consec_fail) = if let Some(ref _elig) = eligible_candidate {
        (WatcherState::EligibleCandidateFound, "eligible_candidate_found".to_string(), None, 0)
    } else if candidate_count > 0 {
        // Candidates discovered, but none satisfied hard eligibility gates (e.g. pack, range, pre)
        (WatcherState::ReleasedAwaitingProvider, "ineligible_candidates_only".to_string(), None, 0)
    } else if has_provider_errors {
        let errs = discovery_res
            .provider_errors
            .iter()
            .map(|(p, e)| format!("{p}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        let is_rate_limit = errs.contains("429") || errs.contains("rate");
        let st = if is_rate_limit { WatcherState::RateLimited } else { WatcherState::ProviderUnavailable };
        (st, "provider_error".to_string(), Some(errs), watcher.consecutive_failures + 1)
    } else {
        (WatcherState::ReleasedAwaitingProvider, "no_candidates_found".to_string(), None, 0)
    };

    let interval = calculate_watcher_interval_secs(watcher.release_date.as_deref(), consec_fail, &today_str);
    let next_check = current_time + interval;

    let (top_id, top_prov, top_title, top_fp) = if let Some(ref elig) = eligible_candidate {
        let fp = format!("{}:{}:{}", elig.provider, elig.candidate_id, elig.score);
        (Some(elig.candidate_id.clone()), Some(elig.provider.clone()), Some(elig.title.clone()), Some(fp))
    } else if let Some(ref top) = discovery_res.top_candidate {
        let fp = format!("{}:{}:{}", top.provider, top.candidate_id, top.score);
        (Some(top.candidate_id.clone()), Some(top.provider.clone()), Some(top.title.clone()), Some(fp))
    } else {
        (None, None, None, None)
    };

    let prov_checked_str = discovery_res.providers_checked.join(", ");

    // 8. Handle notification trigger if an eligible candidate is found
    let should_notify = eligible_candidate.is_some()
        && watcher.notify_on_available
        && watcher.state != WatcherState::Notified
        && watcher.state != WatcherState::EligibleCandidateFound;

    let final_state = if should_notify || (eligible_candidate.is_some() && watcher.state == WatcherState::Notified) {
        WatcherState::Notified
    } else {
        new_state
    };

    let series_id_c = watcher.series_id.clone();
    let edition_c = watcher.edition.clone();
    let top_id_c = top_id.clone();
    let top_prov_display = top_prov.clone().unwrap_or_else(|| "provider".to_string());

    let updated = db
        .call(move |conn| {
            let mut notif_event_id = watcher.notification_event_id;
            let mut notif_at = watcher.notified_at;

            if should_notify {
                let msg = format!("{series_title} Volume {volume_number} is available on {top_prov_display}.");
                let mb_id: Option<i64> = conn
                    .query_row(
                        "SELECT mangabaka_id FROM series_reconciliations WHERE series_id = ?1",
                        params![&series_id_c],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();

                let _ = record_event_full(
                    conn,
                    &series_id_c,
                    &series_title,
                    mb_id,
                    None,
                    &edition_c,
                    Some(volume_number),
                    "RELEASE_AVAILABLE",
                    top_id_c.as_deref(),
                    candidate_count as i64,
                    &msg,
                );

                notif_at = Some(current_time);

                // Fetch event ID for linkage
                notif_event_id = conn
                    .query_row(
                        "SELECT id FROM reconciliation_events
                         WHERE series_id = ?1 AND event_type = 'RELEASE_AVAILABLE' AND ABS(volume_number - ?2) < 0.001
                         ORDER BY id DESC LIMIT 1",
                        params![&series_id_c, volume_number],
                        |r| r.get(0),
                    )
                    .optional()?;
            }

            conn.execute(
                "UPDATE release_availability_watchers
                 SET state = ?1, is_released = 1, last_checked_at = ?2, next_check_at = ?3,
                     check_interval_secs = ?4, consecutive_failures = ?5, last_provider_checked = ?6,
                     last_check_status = ?7, last_error = ?8, top_candidate_id = ?9,
                     top_candidate_provider = ?10, top_candidate_title = ?11,
                     top_candidate_fingerprint = ?12, candidate_count = ?13,
                     notified_at = ?14, notification_event_id = ?15, updated_at = ?2
                 WHERE id = ?16",
                params![
                    final_state.as_str(),
                    current_time,
                    next_check,
                    interval,
                    consec_fail,
                    prov_checked_str,
                    last_status,
                    err_msg,
                    top_id,
                    top_prov,
                    top_title,
                    top_fp,
                    candidate_count as i64,
                    notif_at,
                    notif_event_id,
                    watcher_id,
                ],
            )?;

            get_watcher_by_id(conn, watcher_id)
        })
        .await?;

    Ok(updated)
}

/// Runs a single polling tick across due availability watchers.
pub async fn run_watcher_tick(db: &Db, http: Client) -> Result<usize> {
    let current_time = now();

    let due_ids: Vec<i64> = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM release_availability_watchers
                 WHERE monitoring_enabled = 1
                   AND is_owned = 0
                   AND state NOT IN ('OWNERSHIP_DETECTED', 'ACQUIRED', 'MONITORING_DISABLED')
                   AND next_check_at <= ?1
                 ORDER BY next_check_at ASC
                 LIMIT 10",
            )?;
            let rows = stmt.query_map([current_time], |r| r.get(0))?;
            let mut ids = Vec::new();
            for r in rows {
                ids.push(r?);
            }
            Ok(ids)
        })
        .await?;

    let mut processed = 0;
    for wid in due_ids {
        if let Ok(Some(_)) = evaluate_watcher_check(db, http.clone(), wid).await {
            processed += 1;
        }
    }

    Ok(processed)
}

/// Triggers safe on-demand availability refresh for a specific title.
pub async fn refresh_title_watchers(
    db: &Db,
    http: Client,
    manga_id: i64,
) -> Result<Vec<ReleaseAvailabilityWatcher>> {
    // Fetch watcher IDs for this manga
    let watcher_ids: Vec<i64> = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM release_availability_watchers
                 WHERE manga_id = ?1 AND is_owned = 0
                 ORDER BY volume_number ASC",
            )?;
            let rows = stmt.query_map([manga_id], |r| r.get(0))?;
            let mut ids = Vec::new();
            for r in rows {
                ids.push(r?);
            }
            Ok(ids)
        })
        .await?;

    for wid in watcher_ids {
        let _ = evaluate_watcher_check(db, http.clone(), wid).await?;
    }

    let result = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                        work_id, market, edition, release_date, is_released, is_owned, monitoring_enabled,
                        notify_on_available, state, last_attempt_at, last_checked_at, next_check_at,
                        check_interval_secs, consecutive_failures, last_provider_checked, last_check_status,
                        last_error, top_candidate_id, top_candidate_provider, top_candidate_title,
                        top_candidate_fingerprint, candidate_count, notified_at, notification_event_id,
                        created_at, updated_at
                 FROM release_availability_watchers
                 WHERE manga_id = ?1
                 ORDER BY volume_number ASC",
            )?;
            let rows = stmt.query_map([manga_id], map_watcher_row)?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r?);
            }
            Ok(list)
        })
        .await?;

    Ok(result)
}

/// Updates title monitoring and notification settings.
pub fn set_title_monitoring_settings(
    conn: &Connection,
    manga_id: i64,
    monitoring_enabled: bool,
    notify_enabled: bool,
) -> Result<TitleMonitoringSettings> {
    let current_time = now();

    conn.execute(
        "UPDATE manga
         SET release_monitoring_enabled = ?1, release_notify_enabled = ?2
         WHERE id = ?3",
        params![
            if monitoring_enabled { 1 } else { 0 },
            if notify_enabled { 1 } else { 0 },
            manga_id
        ],
    )?;

    // Synchronize existing watcher rows
    conn.execute(
        "UPDATE release_availability_watchers
         SET monitoring_enabled = ?1, notify_on_available = ?2, updated_at = ?3
         WHERE manga_id = ?4",
        params![
            if monitoring_enabled { 1 } else { 0 },
            if notify_enabled { 1 } else { 0 },
            current_time,
            manga_id
        ],
    )?;

    get_title_monitoring_settings(conn, manga_id)
}

/// Retrieves title monitoring and notification settings.
pub fn get_title_monitoring_settings(
    conn: &Connection,
    manga_id: i64,
) -> Result<TitleMonitoringSettings> {
    conn.query_row(
        "SELECT id, release_monitoring_enabled, release_notify_enabled, auto_acquire
         FROM manga
         WHERE id = ?1",
        [manga_id],
        |r| {
            Ok(TitleMonitoringSettings {
                manga_id: r.get(0)?,
                release_monitoring_enabled: r.get::<_, i64>(1)? != 0,
                release_notify_enabled: r.get::<_, i64>(2)? != 0,
                auto_acquire_enabled: r.get::<_, i64>(3)? != 0,
            })
        },
    )
    .map_err(|e| AppError::internal(format!("Get title monitoring settings error: {e}")))
}

/// Background worker loop for processing due availability watchers periodically.
pub async fn run_watcher_worker_forever(db: Db, http: Client) {
    loop {
        if let Err(e) = run_watcher_tick(&db, http.clone()).await {
            tracing::warn!("Availability watcher tick encountered error: {e}");
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    }
}
