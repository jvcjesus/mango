//! Release-Aware Discovery Subsystem (Milestone M6.6)
//!
//! Orchestrates automated release-triggered candidate discovery by connecting
//! OpenTome physical release timing, MangaBaka canonical volume identity,
//! Komga ownership verification, and Nyaa/Madokami discovery providers.
//!
//! Strict Invariants:
//! - Pure discovery: M6.6 MUST NOT perform automatic acquisition. Guarded manual Acquire remains required.
//! - Canonical physical ownership (Komga) is verified before every provider search.
//! - MangaBaka remains authoritative for logical volume existence; OpenTome is timing intelligence only.
//! - Day-precision release date required (no manufactured dates).
//! - Historical activation watermark prevents flood backfills.
//! - Bounded exponential retries survive restart and stop once candidates or ownership are established.
//! - Notifications use durable outbox (M6.3) idempotently without spam.

use crate::acquisition::discovery::discover_for_volume;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::opentome::catalog::{OpenTomeSeriesSchedule, OpenTomeVolumeRelease};
use crate::sync::reconciliation::{
    gather_local_owned_volumes, get_reconciliation, record_event_full, ReconciliationStatus,
};
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_ATTEMPTS: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDiscoveryTarget {
    pub id: i64,
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: f64,
    pub market: String,
    pub edition: String,
    pub release_date: String,
    pub release_date_precision: String,
    pub date_type: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub next_attempt_at: i64,
    pub last_attempt_at: Option<i64>,
    pub candidates_count: i64,
    pub top_candidate_id: Option<String>,
    pub top_candidate_title: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Calculate retry delay in seconds for bounded backoff.
/// - Attempt 1 done -> +6 hours (21,600s)
/// - Attempt 2 done -> +24 hours (86,400s)
/// - Attempt 3 done -> +48 hours (172,800s)
/// - Attempt 4 done -> +4 days (345,600s)
/// - Attempt 5 done -> +7 days (604,800s)
/// - Attempt 6 done -> +14 days (1,209,600s)
/// - Attempt 7 done -> None (dormant)
pub fn calculate_retry_delay_secs(attempt: i64) -> Option<i64> {
    match attempt {
        1 => Some(6 * 3600),
        2 => Some(24 * 3600),
        3 => Some(48 * 3600),
        4 => Some(4 * 86400),
        5 => Some(7 * 86400),
        6 => Some(14 * 86400),
        _ => None,
    }
}

/// Verify if a date string has exact day precision (YYYY-MM-DD).
pub fn is_exact_day_date(date_str: &str) -> bool {
    if date_str.len() != 10 {
        return false;
    }
    let parts: Vec<&str> = date_str.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[2].chars().all(|c| c.is_ascii_digit())
}

/// Get or initialize the historical activation watermark date.
pub fn get_or_init_activation_date(conn: &Connection) -> Result<String> {
    let existing: Option<String> = crate::settings::get::<String>(conn, "acquisition.release_discovery_activation_date");
    if let Some(date) = existing {
        return Ok(date);
    }

    let today: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| "2026-10-30".to_string());

    let _ = crate::settings::set(conn, "acquisition.release_discovery_activation_date", &serde_json::json!(today));
    let _ = crate::settings::set(conn, "acquisition.release_discovery_activated_at", &serde_json::json!(now()));
    Ok(today)
}

/// Synchronize and evaluate release targets for a followed manga given an OpenTome schedule.
pub fn sync_series_release_targets(
    conn: &Connection,
    manga_id: i64,
    series_id: &str,
    series_title: &str,
    schedule: &OpenTomeSeriesSchedule,
    today_str: &str,
    activation_date: &str,
) -> Result<Vec<ReleaseDiscoveryTarget>> {
    let mut targets = Vec::new();

    // Check English market line
    if let Some(ref en) = schedule.english {
        if let Some(ref next_vol) = en.next_volume {
            if let Some(target) = evaluate_and_upsert_target(
                conn,
                manga_id,
                series_id,
                series_title,
                "en",
                "standard",
                next_vol,
                &schedule.mapping_confidence,
                today_str,
                activation_date,
            )? {
                targets.push(target);
            }
        }
    }

    // Check Japanese market line
    if let Some(ref ja) = schedule.japanese {
        if let Some(ref next_vol) = ja.next_volume {
            if let Some(target) = evaluate_and_upsert_target(
                conn,
                manga_id,
                series_id,
                series_title,
                "ja",
                "standard",
                next_vol,
                &schedule.mapping_confidence,
                today_str,
                activation_date,
            )? {
                targets.push(target);
            }
        }
    }

    Ok(targets)
}

fn evaluate_and_upsert_target(
    conn: &Connection,
    manga_id: i64,
    series_id: &str,
    series_title: &str,
    market: &str,
    edition: &str,
    vol: &OpenTomeVolumeRelease,
    mapping_confidence: &str,
    today_str: &str,
    activation_date: &str,
) -> Result<Option<ReleaseDiscoveryTarget>> {
    let vnum = vol.volume_number;
    let rdate = match vol.release_date.as_deref() {
        Some(d) if is_exact_day_date(d) => d,
        _ => return Ok(None), // Must have exact day date
    };

    if vol.precision != "day" {
        return Ok(None); // Day precision required
    }

    // Historical flood protection:
    // If release date is in the past before activation date, do not automatically create an active target.
    if rdate < activation_date {
        // Only record if already owned, otherwise skip backfill
        let is_owned = is_volume_owned(conn, manga_id, series_id, vnum)?;
        if is_owned {
            return upsert_target_record(
                conn,
                series_id,
                Some(manga_id),
                series_title,
                vnum,
                market,
                edition,
                rdate,
                &vol.precision,
                &vol.date_type,
                "already_owned",
                0,
                0,
                0,
                None,
                None,
                None,
            );
        }
        return Ok(None);
    }

    // Check if volume is already owned in Komga / local library
    let is_owned = is_volume_owned(conn, manga_id, series_id, vnum)?;
    if is_owned {
        return upsert_target_record(
            conn,
            series_id,
            Some(manga_id),
            series_title,
            vnum,
            market,
            edition,
            rdate,
            &vol.precision,
            &vol.date_type,
            "already_owned",
            0,
            0,
            0,
            None,
            None,
            None,
        );
    }

    // Check mapping gate
    let is_mapping_confident = matches!(
        mapping_confidence,
        "exact" | "high" | "user_confirmed"
    );

    let is_vol_corresponded = check_volume_correspondence(conn, series_id, vnum)?;

    let initial_status = if !is_mapping_confident || !is_vol_corresponded {
        "mapping_review_required"
    } else if rdate > today_str {
        "scheduled"
    } else if rdate == today_str {
        "releases_today"
    } else {
        // rdate < today_str but >= activation_date: active release
        "releases_today"
    };

    upsert_target_record(
        conn,
        series_id,
        Some(manga_id),
        series_title,
        vnum,
        market,
        edition,
        rdate,
        &vol.precision,
        &vol.date_type,
        initial_status,
        0,
        0,
        0,
        None,
        None,
        None,
    )
}

fn upsert_target_record(
    conn: &Connection,
    series_id: &str,
    manga_id: Option<i64>,
    series_title: &str,
    volume_number: f64,
    market: &str,
    edition: &str,
    release_date: &str,
    precision: &str,
    date_type: &str,
    initial_status: &str,
    attempts: i64,
    next_attempt_at: i64,
    candidates_count: i64,
    top_candidate_id: Option<&str>,
    top_candidate_title: Option<&str>,
    last_error: Option<&str>,
) -> Result<Option<ReleaseDiscoveryTarget>> {
    let current_time = now();

    // Check existing target for this volume and market
    let existing: Option<(i64, String, String, i64, i64)> = conn
        .query_row(
            "SELECT id, release_date, status, attempts, next_attempt_at
             FROM release_discovery_targets
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001 AND market = ?3 AND edition = ?4",
            params![series_id, volume_number, market, edition],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Query failed: {e}")))?;

    if let Some((id, old_date, old_status, _old_attempts, _old_next)) = existing {
        // Date changed handling:
        if old_date != release_date {
            let new_status = if initial_status == "already_owned" {
                "already_owned"
            } else if release_date > conn.query_row("SELECT date('now','localtime')", [], |r| r.get::<_, String>(0)).unwrap_or_default().as_str() {
                "scheduled"
            } else {
                "releases_today"
            };

            conn.execute(
                "UPDATE release_discovery_targets
                 SET release_date = ?1, status = ?2, next_attempt_at = 0, updated_at = ?3
                 WHERE id = ?4",
                params![release_date, new_status, current_time, id],
            )?;
        } else if old_status == "scheduled" && initial_status == "releases_today" {
            // Reached release day!
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'releases_today', next_attempt_at = 0, updated_at = ?1
                 WHERE id = ?2",
                params![current_time, id],
            )?;
        } else if initial_status == "already_owned" && old_status != "already_owned" {
            // Volume became owned! Stop retries
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'already_owned', updated_at = ?1
                 WHERE id = ?2",
                params![current_time, id],
            )?;
        }

        return get_target_by_id(conn, id);
    }

    // Insert brand new target
    conn.execute(
        "INSERT INTO release_discovery_targets (
            series_id, manga_id, series_title, volume_number, market, edition,
            release_date, release_date_precision, date_type, status, attempts,
            max_attempts, next_attempt_at, candidates_count, top_candidate_id,
            top_candidate_title, last_error, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)",
        params![
            series_id,
            manga_id,
            series_title,
            volume_number,
            market,
            edition,
            release_date,
            precision,
            date_type,
            initial_status,
            attempts,
            DEFAULT_MAX_ATTEMPTS,
            next_attempt_at,
            candidates_count,
            top_candidate_id,
            top_candidate_title,
            last_error,
            current_time,
        ],
    )?;

    let new_id = conn.last_insert_rowid();
    get_target_by_id(conn, new_id)
}

pub fn get_target_by_id(conn: &Connection, id: i64) -> Result<Option<ReleaseDiscoveryTarget>> {
    conn.query_row(
        "SELECT id, series_id, manga_id, series_title, volume_number, market, edition,
                release_date, release_date_precision, date_type, status, attempts,
                max_attempts, next_attempt_at, last_attempt_at, candidates_count,
                top_candidate_id, top_candidate_title, last_error, created_at, updated_at
         FROM release_discovery_targets
         WHERE id = ?1",
        params![id],
        |r| {
            Ok(ReleaseDiscoveryTarget {
                id: r.get(0)?,
                series_id: r.get(1)?,
                manga_id: r.get(2)?,
                series_title: r.get(3)?,
                volume_number: r.get(4)?,
                market: r.get(5)?,
                edition: r.get(6)?,
                release_date: r.get(7)?,
                release_date_precision: r.get(8)?,
                date_type: r.get(9)?,
                status: r.get(10)?,
                attempts: r.get(11)?,
                max_attempts: r.get(12)?,
                next_attempt_at: r.get(13)?,
                last_attempt_at: r.get(14)?,
                candidates_count: r.get(15)?,
                top_candidate_id: r.get(16)?,
                top_candidate_title: r.get(17)?,
                last_error: r.get(18)?,
                created_at: r.get(19)?,
                updated_at: r.get(20)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::internal(format!("Query failed: {e}")))
}

pub fn list_targets_for_manga(conn: &Connection, manga_id: i64) -> Result<Vec<ReleaseDiscoveryTarget>> {
    let mut stmt = conn.prepare(
        "SELECT id, series_id, manga_id, series_title, volume_number, market, edition,
                release_date, release_date_precision, date_type, status, attempts,
                max_attempts, next_attempt_at, last_attempt_at, candidates_count,
                top_candidate_id, top_candidate_title, last_error, created_at, updated_at
         FROM release_discovery_targets
         WHERE manga_id = ?1
         ORDER BY volume_number ASC, market ASC",
    )?;

    let rows = stmt.query_map(params![manga_id], |r| {
        Ok(ReleaseDiscoveryTarget {
            id: r.get(0)?,
            series_id: r.get(1)?,
            manga_id: r.get(2)?,
            series_title: r.get(3)?,
            volume_number: r.get(4)?,
            market: r.get(5)?,
            edition: r.get(6)?,
            release_date: r.get(7)?,
            release_date_precision: r.get(8)?,
            date_type: r.get(9)?,
            status: r.get(10)?,
            attempts: r.get(11)?,
            max_attempts: r.get(12)?,
            next_attempt_at: r.get(13)?,
            last_attempt_at: r.get(14)?,
            candidates_count: r.get(15)?,
            top_candidate_id: r.get(16)?,
            top_candidate_title: r.get(17)?,
            last_error: r.get(18)?,
            created_at: r.get(19)?,
            updated_at: r.get(20)?,
        })
    })?;

    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// Check if volume is owned in local library or Komga reconciliation.
pub fn is_volume_owned(
    conn: &Connection,
    manga_id: i64,
    series_id: &str,
    volume_number: f64,
) -> Result<bool> {
    // 1. Check local owned volumes
    let local_vols = gather_local_owned_volumes(conn, Some(manga_id))?;
    if local_vols.iter().any(|&v| (v - volume_number).abs() < 0.001) {
        return Ok(true);
    }

    // 2. Check reconciliation owned volumes
    if let Some(recon) = get_reconciliation(conn, series_id)? {
        if recon.owned_volumes.iter().any(|&v| (v - volume_number).abs() < 0.001) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Check if volume corresponds to a known MangaBaka official volume.
pub fn check_volume_correspondence(
    conn: &Connection,
    series_id: &str,
    volume_number: f64,
) -> Result<bool> {
    let Some(recon) = get_reconciliation(conn, series_id)? else {
        return Ok(false);
    };

    if recon.status == ReconciliationStatus::ReviewRequired || recon.status == ReconciliationStatus::SyncError {
        return Ok(false);
    }

    // Is it in owned, missing, or future volumes?
    if recon.owned_volumes.iter().any(|&v| (v - volume_number).abs() < 0.001) {
        return Ok(true);
    }
    if recon.missing_volumes.iter().any(|&v| (v - volume_number).abs() < 0.001) {
        return Ok(true);
    }
    if recon.future_volumes.iter().any(|&v| (v - volume_number).abs() < 0.001) {
        return Ok(true);
    }

    // Or within total official volumes
    if let Some(total) = recon.total_official_volumes {
        if volume_number >= 1.0 && volume_number <= (total as f64) + 0.001 {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Process a single target through release-aware discovery.
/// Pure discovery: never initiates downloads or mutates media.
pub async fn process_target_discovery(
    db: &Db,
    http: Client,
    target_id: i64,
) -> Result<Option<ReleaseDiscoveryTarget>> {
    // 1. Fetch current target state
    let target = db
        .call(move |conn| get_target_by_id(conn, target_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Target {target_id} not found")))?;

    // Only process eligible active states
    if target.status != "releases_today" && target.status != "not_found_retry_wait" {
        return Ok(Some(target));
    }

    let series_id = target.series_id.clone();
    let manga_id_opt = target.manga_id;
    let volume_number = target.volume_number;
    let series_title = target.series_title.clone();

    // 2. Ownership Gate (BEFORE provider query)
    let s_id = series_id.clone();
    let is_owned = db
        .call(move |conn| {
            if let Some(mid) = manga_id_opt {
                is_volume_owned(conn, mid, &s_id, volume_number)
            } else {
                Ok(false)
            }
        })
        .await?;

    if is_owned {
        let cur_time = now();
        db.call(move |conn| {
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'already_owned', updated_at = ?1
                 WHERE id = ?2",
                params![cur_time, target_id],
            )?;
            Ok(())
        })
        .await?;
        return db.call(move |conn| get_target_by_id(conn, target_id)).await;
    }

    // 3. Mapping Gate
    let s_id2 = series_id.clone();
    let is_corresponded = db
        .call(move |conn| check_volume_correspondence(conn, &s_id2, volume_number))
        .await?;

    if !is_corresponded {
        let cur_time = now();
        db.call(move |conn| {
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'mapping_review_required', last_error = 'Volume correspondence or MangaBaka identity unresolved', updated_at = ?1
                 WHERE id = ?2",
                params![cur_time, target_id],
            )?;
            Ok(())
        })
        .await?;
        return db.call(move |conn| get_target_by_id(conn, target_id)).await;
    }

    // 4. Mark status: discovering
    let cur_time = now();
    db.call(move |conn| {
        conn.execute(
            "UPDATE release_discovery_targets
             SET status = 'discovering', last_attempt_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![cur_time, target_id],
        )?;
        Ok(())
    })
    .await?;

    // 5. Execute M6.2 Discovery
    let discovery_res = discover_for_volume(db, http, &series_id, volume_number).await?;

    let new_attempts = target.attempts + 1;
    let candidate_count = discovery_res.candidate_count;
    let top_cand = discovery_res.top_candidate.clone();
    let top_cand_id = top_cand.as_ref().map(|c| c.candidate_id.clone());
    let top_cand_title = top_cand.as_ref().map(|c| c.title.clone());

    let has_useful_candidate = candidate_count > 0 && top_cand.as_ref().map_or(false, |c| {
        use crate::acquisition::providers::CandidateMatchType;
        matches!(
            c.match_type,
            CandidateMatchType::ExactVolume
                | CandidateMatchType::VolumeRangeContainingTarget
                | CandidateMatchType::SeriesPack
                | CandidateMatchType::PossibleMatch
        )
    });

    let (final_status, next_attempt_at) = if discovery_res.status == "owned" {
        ("already_owned", 0)
    } else if has_useful_candidate {
        // Candidate found! Stop aggressive retries
        ("candidate_available", 0)
    } else if new_attempts >= target.max_attempts {
        // Exceeded bounded attempts
        ("dormant", 0)
    } else {
        // Schedule next bounded retry
        let delay = calculate_retry_delay_secs(new_attempts).unwrap_or(86400);
        ("not_found_retry_wait", cur_time + delay)
    };

    let prov_err_msg = if !discovery_res.provider_errors.is_empty() {
        Some(
            discovery_res
                .provider_errors
                .iter()
                .map(|(p, e)| format!("{p}: {e}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    } else {
        None
    };

    let title_for_notif = series_title.clone();
    let top_id_for_notif = top_cand_id.clone();

    // 6. Persist results
    db.call(move |conn| {
        conn.execute(
            "UPDATE release_discovery_targets
             SET status = ?1, attempts = ?2, next_attempt_at = ?3, candidates_count = ?4,
                 top_candidate_id = ?5, top_candidate_title = ?6, last_error = ?7, updated_at = ?8
             WHERE id = ?9",
            params![
                final_status,
                new_attempts,
                next_attempt_at,
                candidate_count as i64,
                top_cand_id,
                top_cand_title,
                prov_err_msg,
                cur_time,
                target_id,
            ],
        )?;

        // 7. Emit RELEASE_AVAILABLE notification if candidate found
        if final_status == "candidate_available" {
            let msg = format!("{title_for_notif} Vol. {volume_number} is out — a release was found");
            let mb_id: Option<i64> = conn
                .query_row(
                    "SELECT mangabaka_id FROM series_reconciliations WHERE series_id = ?1",
                    params![&series_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();

            let _ = record_event_full(
                conn,
                &series_id,
                &title_for_notif,
                mb_id,
                None,
                &target.edition,
                Some(volume_number),
                "RELEASE_AVAILABLE",
                top_id_for_notif.as_deref(),
                candidate_count as i64,
                &msg,
            );
        }

        Ok(())
    })
    .await?;

    db.call(move |conn| get_target_by_id(conn, target_id)).await
}

/// Evaluates all pending release discovery targets due for attempt.
pub async fn evaluate_all_pending_targets(db: &Db, http: Client) -> Result<usize> {
    let cur_time = now();

    let target_ids: Vec<i64> = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM release_discovery_targets
                 WHERE (status = 'releases_today' AND (last_attempt_at IS NULL OR last_attempt_at < ?1 - 300))
                    OR (status = 'not_found_retry_wait' AND next_attempt_at <= ?1)
                 ORDER BY id ASC
                 LIMIT 50",
            )?;
            let rows = stmt.query_map(params![cur_time], |r| r.get(0))?;
            let mut ids = Vec::new();
            for r in rows {
                ids.push(r?);
            }
            Ok(ids)
        })
        .await?;

    let mut count = 0;
    for tid in target_ids {
        if let Ok(Some(_)) = process_target_discovery(db, http.clone(), tid).await {
            count += 1;
        }
    }

    Ok(count)
}
