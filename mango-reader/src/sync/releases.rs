use crate::error::{AppError, Result};
use crate::opentome::catalog::OpenTomeSeriesSchedule;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Calculate days difference between two YYYY-MM-DD date strings (d1 - d2).
pub fn days_between_ymd(d1: &str, d2: &str) -> i64 {
    fn ymd_to_days(s: &str) -> Option<i64> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() < 3 {
            return None;
        }
        let y: i64 = parts[0].parse().ok()?;
        let m: i64 = parts[1].parse().ok()?;
        let d: i64 = parts[2].parse().ok()?;
        let mut days = y * 365 + y / 4 - y / 100 + y / 400;
        let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for i in 1..m as usize {
            if i < month_days.len() {
                days += month_days[i];
                if i == 2 && ((y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)) {
                    days += 1;
                }
            }
        }
        days += d;
        Some(days)
    }

    match (ymd_to_days(d1), ymd_to_days(d2)) {
        (Some(a), Some(b)) => a - b,
        _ => 0,
    }
}

/// A single upcoming or released volume/chapter release candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingReleaseItem {
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub mangabaka_id: Option<i64>,
    pub title: String,
    pub volume_number: Option<f64>,
    pub chapter_number: Option<f64>,
    pub release_date: String,
    pub release_date_raw: Option<String>,
    pub precision: String, // "day", "month", "year", "unknown"
    pub date_status: String, // "confirmed", "tentative", "estimated", "unknown"
    pub source: String, // "opentome", "mangabaka", "reconciled"
    pub language: String, // "en", "ja", etc.
    pub market: String,
    pub format: String, // "physical", "digital"
    pub publisher: Option<String>,
    pub is_owned: bool,
    pub days_until: i64,
    pub cover_url: Option<String>,
    pub scope: String, // "library", "tracked_lists", "available"
}

/// Diagnostic classification codes for missing release dates (Phase 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseDiagnosticCode {
    NoUpcomingVolumeAnnounced,
    VolumeAnnouncedDatePending,
    SeriesNotFoundInOpenTome,
    AmbiguousOpenTomeMatch,
    OpenTomeDatabaseOutdated,
    MetadataSyncPending,
    UpstreamUpdateFailed,
    EditionLanguageMismatch,
}

impl ReleaseDiagnosticCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoUpcomingVolumeAnnounced => "no_upcoming_volume_announced",
            Self::VolumeAnnouncedDatePending => "volume_announced_date_pending",
            Self::SeriesNotFoundInOpenTome => "series_not_found_in_opentome",
            Self::AmbiguousOpenTomeMatch => "ambiguous_opentome_match",
            Self::OpenTomeDatabaseOutdated => "opentome_database_outdated",
            Self::MetadataSyncPending => "metadata_sync_pending",
            Self::UpstreamUpdateFailed => "upstream_update_failed",
            Self::EditionLanguageMismatch => "edition_language_mismatch",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::NoUpcomingVolumeAnnounced => "Publication Complete / Caught Up",
            Self::VolumeAnnouncedDatePending => "Volume Announced (Date Pending)",
            Self::SeriesNotFoundInOpenTome => "Series Not Found in OpenTome",
            Self::AmbiguousOpenTomeMatch => "Ambiguous OpenTome Match",
            Self::OpenTomeDatabaseOutdated => "OpenTome Database Outdated",
            Self::MetadataSyncPending => "Metadata Sync Pending",
            Self::UpstreamUpdateFailed => "Upstream Update Failed",
            Self::EditionLanguageMismatch => "Language / Edition Mismatch",
        }
    }
}

/// Classify missing release dates into 8 distinct, actionable categories.
pub fn classify_missing_release_diagnostic(
    ot_confidence: Option<&str>,
    has_work_id: bool,
    ot_schedule: Option<&OpenTomeSeriesSchedule>,
    future_volumes: &[f64],
    is_ongoing: bool,
    expected_vol: Option<f64>,
    is_database_outdated: bool,
    upstream_error: Option<&str>,
) -> (ReleaseDiagnosticCode, String) {
    // 1. Upstream update failed
    if let Some(err) = upstream_error {
        if !err.is_empty() {
            return (
                ReleaseDiagnosticCode::UpstreamUpdateFailed,
                format!("Upstream OpenTome catalogue update failed ({err}); schedules may be incomplete or outdated."),
            );
        }
    }

    // 2. Metadata sync pending
    if ot_confidence.is_none() && future_volumes.is_empty() && !is_ongoing {
        return (
            ReleaseDiagnosticCode::MetadataSyncPending,
            "Metadata synchronization pending; awaiting background reconciliation or manual title refresh.".to_string(),
        );
    }

    // 3. Ambiguous OpenTome match
    if ot_confidence == Some("ambiguous") {
        return (
            ReleaseDiagnosticCode::AmbiguousOpenTomeMatch,
            "Multiple matching works found in OpenTome; manual line selection required in Metadata Review.".to_string(),
        );
    }

    // 4. Series not found in OpenTome
    if ot_confidence == Some("unmatched") || (ot_confidence.is_some() && !has_work_id) {
        return (
            ReleaseDiagnosticCode::SeriesNotFoundInOpenTome,
            "Series could not be identified in the OpenTome database. Manual mapping or catalog update may be required.".to_string(),
        );
    }

    // 5. Edition or language mismatch (JA schedule exists, EN does not)
    if let Some(sched) = ot_schedule {
        if sched.japanese.is_some() && sched.english.is_none() {
            return (
                ReleaseDiagnosticCode::EditionLanguageMismatch,
                "Release schedule is available for Japanese print, but no English physical publication line exists in OpenTome.".to_string(),
            );
        }
    }

    // 6. Volume announced, date pending
    if let Some(vol) = expected_vol {
        return (
            ReleaseDiagnosticCode::VolumeAnnouncedDatePending,
            format!("Volume {} announced in MangaBaka catalogue; official publication date pending publisher announcement.", vol),
        );
    }
    if let Some(&first_future) = future_volumes.first() {
        return (
            ReleaseDiagnosticCode::VolumeAnnouncedDatePending,
            format!("Volume {} announced in MangaBaka catalogue; official publication date pending publisher announcement.", first_future),
        );
    }

    // 7. OpenTome database outdated
    if is_database_outdated {
        return (
            ReleaseDiagnosticCode::OpenTomeDatabaseOutdated,
            "Installed OpenTome database is older than the latest upstream release; update catalogue in Settings.".to_string(),
        );
    }

    // 8. No upcoming volume announced (completed or caught up)
    if !is_ongoing {
        (
            ReleaseDiagnosticCode::NoUpcomingVolumeAnnounced,
            "Official publication of this series is complete. No upcoming releases scheduled.".to_string(),
        )
    } else {
        (
            ReleaseDiagnosticCode::NoUpcomingVolumeAnnounced,
            "No upcoming physical volume release date announced yet by the publisher. Tracker catalog is synchronized.".to_string(),
        )
    }
}

/// A monitored or tracked title that currently has no confirmed future release date.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndatedReleaseItem {
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub komga_series_id: Option<String>,
    pub mangabaka_id: Option<i64>,
    pub title: String,
    pub expected_volume: Option<f64>,
    pub diagnostic_code: String,
    pub diagnostic_label: String,
    pub reason: String,
    pub source: String,
    pub is_owned: bool,
    pub cover_url: Option<String>,
    pub scope: String,
}

/// Payload returned to the releases calendar and chronological views.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingReleasesResponse {
    pub releases: Vec<UpcomingReleaseItem>,
    pub undated: Vec<UndatedReleaseItem>,
    pub total_upcoming: usize,
    pub next_7_days_count: usize,
    pub next_30_days_count: usize,
    pub next_90_days_count: usize,
    pub undated_count: usize,
    pub active_scope: String,
    pub last_refreshed_at: i64,
}

/// Detailed release schedule for an individual series page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleReleaseScheduleDto {
    pub manga_id: i64,
    pub series_id: String,
    pub title: String,
    pub next_release: Option<UpcomingReleaseItem>,
    pub upcoming_volumes: Vec<UpcomingReleaseItem>,
    pub recent_releases: Vec<UpcomingReleaseItem>,
    pub primary_source: String,
    pub confidence: String,
    pub last_evaluated_at: i64,
    pub diagnostic_code: Option<String>,
    pub diagnostic_label: Option<String>,
    pub explanation: Option<String>,
}

/// Query upcoming releases across library and tracked titles.
pub fn query_upcoming_releases(
    conn: &Connection,
    scope: &str,
    days_limit: Option<i64>,
    format_filter: Option<&str>,
    today_str: &str,
) -> Result<UpcomingReleasesResponse> {
    let active_scope = match scope {
        "tracked_lists" => "tracked_lists",
        "all_available" => "all_available",
        _ => "mango_library_only",
    };

    // 1. Gather series eligible under the requested scope
    // We collect (series_id, Option<manga_id>, Option<komga_id>, Option<mb_id>, title, scope_tag)
    struct EligibleSeries {
        series_id: String,
        manga_id: Option<i64>,
        komga_series_id: Option<String>,
        mangabaka_id: Option<i64>,
        title: String,
        scope: String,
    }

    let mut eligible: Vec<EligibleSeries> = Vec::new();
    let mut seen_keys = HashSet::new();

    // Mango library titles
    let mut stmt_lib = conn.prepare(
        "SELECT COALESCE(sr.series_id, 'manga:' || m.id), m.id, sr.komga_series_id, sr.mangabaka_id, m.title
         FROM manga m
         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
         WHERE m.in_library = 1
         ORDER BY m.title ASC",
    ).map_err(|e| AppError::internal(format!("Prepare library query failed: {e}")))?;

    let lib_rows = stmt_lib.query_map([], |r| {
        Ok(EligibleSeries {
            series_id: r.get(0)?,
            manga_id: Some(r.get(1)?),
            komga_series_id: r.get(2)?,
            mangabaka_id: r.get(3)?,
            title: r.get(4)?,
            scope: "library".to_string(),
        })
    }).map_err(|e| AppError::internal(format!("Query library failed: {e}")))?;

    for s in lib_rows.flatten() {
        if seen_keys.insert(s.series_id.clone()) {
            eligible.push(s);
        }
    }

    // Physical Komga owned series in reconciliations
    let mut stmt_owned = conn.prepare(
        "SELECT series_id, manga_id, komga_series_id, mangabaka_id, title
         FROM series_reconciliations
         WHERE owned_volumes_count > 0",
    ).map_err(|e| AppError::internal(format!("Prepare owned query failed: {e}")))?;

    let owned_rows = stmt_owned.query_map([], |r| {
        Ok(EligibleSeries {
            series_id: r.get(0)?,
            manga_id: r.get(1)?,
            komga_series_id: r.get(2)?,
            mangabaka_id: r.get(3)?,
            title: r.get(4)?,
            scope: "library".to_string(),
        })
    }).map_err(|e| AppError::internal(format!("Query owned failed: {e}")))?;

    for s in owned_rows.flatten() {
        if seen_keys.insert(s.series_id.clone()) {
            eligible.push(s);
        }
    }

    if active_scope != "mango_library_only" {
        // Include tracked_catalog titles
        let mut stmt_tracked = conn.prepare(
            "SELECT tc.id, tc.manga_id, tc.komga_series_id, tc.title, tc.origins
             FROM tracked_catalog tc
             WHERE tc.tracked = 1",
        ).map_err(|e| AppError::internal(format!("Prepare tracked query failed: {e}")))?;

        let tracked_rows = stmt_tracked.query_map([], |r| {
            let tc_id: i64 = r.get(0)?;
            let m_id: Option<i64> = r.get(1)?;
            let k_id: Option<String> = r.get(2)?;
            let title: String = r.get(3)?;
            let s_id = if let Some(mid) = m_id {
                format!("manga:{mid}")
            } else if let Some(ref kid) = k_id {
                format!("komga:{kid}")
            } else {
                format!("tracked:{tc_id}")
            };
            Ok(EligibleSeries {
                series_id: s_id,
                manga_id: m_id,
                komga_series_id: k_id,
                mangabaka_id: None,
                title,
                scope: "tracked_lists".to_string(),
            })
        }).map_err(|e| AppError::internal(format!("Query tracked failed: {e}")))?;

        for s in tracked_rows.flatten() {
            if seen_keys.insert(s.series_id.clone()) {
                eligible.push(s);
            }
        }
    }

    if active_scope == "all_available" {
        // Include all other reconciliations
        let mut stmt_all = conn.prepare(
            "SELECT series_id, manga_id, komga_series_id, mangabaka_id, title
             FROM series_reconciliations",
        ).map_err(|e| AppError::internal(format!("Prepare all reconciliations query failed: {e}")))?;

        let all_rows = stmt_all.query_map([], |r| {
            Ok(EligibleSeries {
                series_id: r.get(0)?,
                manga_id: r.get(1)?,
                komga_series_id: r.get(2)?,
                mangabaka_id: r.get(3)?,
                title: r.get(4)?,
                scope: "available".to_string(),
            })
        }).map_err(|e| AppError::internal(format!("Query all failed: {e}")))?;

        for s in all_rows.flatten() {
            if seen_keys.insert(s.series_id.clone()) {
                eligible.push(s);
            }
        }
    }

    // 2. Load cached OpenTome mappings and reconciliations
    let mut opentome_cached: HashMap<String, OpenTomeSeriesSchedule> = HashMap::new();
    let mut stmt_ot = conn.prepare(
        "SELECT series_id, metadata_json FROM opentome_series_mappings WHERE metadata_json IS NOT NULL",
    ).map_err(|e| AppError::internal(format!("Prepare opentome cache query failed: {e}")))?;

    let ot_rows = stmt_ot.query_map([], |r| {
        let s_id: String = r.get(0)?;
        let json_str: String = r.get(1)?;
        Ok((s_id, json_str))
    }).map_err(|e| AppError::internal(format!("Query opentome cache failed: {e}")))?;

    for r in ot_rows.flatten() {
        if let Ok(sched) = serde_json::from_str::<OpenTomeSeriesSchedule>(&r.1) {
            opentome_cached.insert(r.0, sched);
        }
    }

    let mut mappings_meta: HashMap<String, (String, bool)> = HashMap::new();
    let mut stmt_meta = conn.prepare(
        "SELECT series_id, confidence, work_id IS NOT NULL FROM opentome_series_mappings",
    ).map_err(|e| AppError::internal(format!("Prepare opentome mappings meta query failed: {e}")))?;
    let meta_rows = stmt_meta.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))
    }).map_err(|e| AppError::internal(format!("Query opentome mappings meta failed: {e}")))?;
    for r in meta_rows.flatten() {
        mappings_meta.insert(r.0, r.1);
    }

    let (is_database_outdated, upstream_error): (bool, Option<String>) = {
        let last_error: Option<String> = crate::settings::get(conn, "opentome.last_error");
        let installed: Option<String> = crate::settings::get(conn, "opentome.installed_version");
        let upstream: Option<String> = crate::settings::get(conn, "opentome.latest_upstream_version");
        let outdated = match (installed, upstream) {
            (Some(i), Some(u)) => i != u,
            _ => false,
        };
        (outdated, last_error)
    };

    let mut reconciliations_map: HashMap<String, (Vec<f64>, Vec<f64>, bool, String)> = HashMap::new();
    let mut stmt_rec = conn.prepare(
        "SELECT series_id, owned_volumes_json, future_volumes_json, is_ongoing, status
         FROM series_reconciliations",
    ).map_err(|e| AppError::internal(format!("Prepare recon map query failed: {e}")))?;

    let rec_rows = stmt_rec.query_map([], |r| {
        let s_id: String = r.get(0)?;
        let owned_json: String = r.get(1)?;
        let future_json: String = r.get(2)?;
        let ongoing: bool = r.get::<_, i64>(3)? != 0;
        let stat: String = r.get(4)?;
        let owned = serde_json::from_str::<Vec<f64>>(&owned_json).unwrap_or_default();
        let future = serde_json::from_str::<Vec<f64>>(&future_json).unwrap_or_default();
        Ok((s_id, (owned, future, ongoing, stat)))
    }).map_err(|e| AppError::internal(format!("Query recon map failed: {e}")))?;

    for r in rec_rows.flatten() {
        reconciliations_map.insert(r.0, r.1);
    }

    // 3. Process each eligible series into upcoming releases or undated items
    let mut all_releases: Vec<UpcomingReleaseItem> = Vec::new();
    let mut undated_items: Vec<UndatedReleaseItem> = Vec::new();

    let now_epoch = crate::db::now();

    for s in eligible {
        let recon_info = reconciliations_map.get(&s.series_id);
        let owned_set: HashSet<i64> = recon_info
            .map(|(o, _, _, _)| o.iter().map(|v| (v * 1000.0).round() as i64).collect())
            .unwrap_or_default();

        let is_ongoing = recon_info.map(|(_, _, ong, _)| *ong).unwrap_or(true);
        let mut series_has_upcoming = false;

        // Check OpenTome schedule
        let ot_sched_opt = opentome_cached.get(&s.series_id);

        if let Some(ot_sched) = ot_sched_opt {
            // Check English line first, then Japanese line
            let mut lines_to_inspect = Vec::new();
            if let Some(ref en) = ot_sched.english {
                lines_to_inspect.push(("en", en));
            }
            if let Some(ref ja) = ot_sched.japanese {
                lines_to_inspect.push(("ja", ja));
            }

            for (mkt, line) in lines_to_inspect {
                for vol in &line.volumes {
                    let is_upcoming = vol.status == "releases_today"
                        || vol.status == "future"
                        || vol.release_date.as_deref().map_or(false, |d| d >= today_str);

                    if !is_upcoming {
                        continue;
                    }

                    let r_date = vol.release_date.clone().or_else(|| vol.release_date_raw.clone()).unwrap_or_default();
                    if r_date.is_empty() {
                        continue;
                    }

                    let days_diff = days_between_ymd(&r_date, today_str);
                    if days_diff < 0 {
                        continue;
                    }

                    if let Some(lim) = days_limit {
                        if days_diff > lim {
                            continue;
                        }
                    }

                    if let Some(fmt) = format_filter {
                        if !fmt.is_empty() && fmt != "all" && fmt != "physical" {
                            continue;
                        }
                    }

                    let is_owned = owned_set.contains(&((vol.volume_number * 1000.0).round() as i64));

                    let date_status = match vol.precision.as_str() {
                        "day" => {
                            if vol.date_type == "projected" {
                                "tentative".to_string()
                            } else {
                                "confirmed".to_string()
                            }
                        }
                        "month" | "year" => "tentative".to_string(),
                        _ => "unknown".to_string(),
                    };

                    let cover = s.manga_id.map(|mid| format!("/api/manga/{mid}/cover"));

                    all_releases.push(UpcomingReleaseItem {
                        series_id: s.series_id.clone(),
                        manga_id: s.manga_id,
                        komga_series_id: s.komga_series_id.clone(),
                        mangabaka_id: s.mangabaka_id,
                        title: s.title.clone(),
                        volume_number: Some(vol.volume_number),
                        chapter_number: None,
                        release_date: r_date.clone(),
                        release_date_raw: vol.release_date_raw.clone(),
                        precision: vol.precision.clone(),
                        date_status,
                        source: "opentome".to_string(),
                        language: line.market.clone(),
                        market: mkt.to_string(),
                        format: "physical".to_string(),
                        publisher: line.publisher.clone(),
                        is_owned,
                        days_until: days_diff,
                        cover_url: cover,
                        scope: s.scope.clone(),
                    });

                    series_has_upcoming = true;
                }
            }
        }

        // If no upcoming volumes found from OpenTome, diagnose and add to undated
        if !series_has_upcoming {
            let (conf_opt, has_wid) = mappings_meta.get(&s.series_id).map(|(c, w)| (Some(c.as_str()), *w)).unwrap_or((None, false));
            let future_vols = recon_info.map(|(_, fv, _, _)| fv.as_slice()).unwrap_or(&[]);

            if !future_vols.is_empty() {
                for &fv in future_vols {
                    let is_owned = owned_set.contains(&((fv * 1000.0).round() as i64));
                    let cover = s.manga_id.map(|mid| format!("/api/manga/{mid}/cover"));
                    let (diag_code, reason) = classify_missing_release_diagnostic(
                        conf_opt,
                        has_wid,
                        ot_sched_opt,
                        future_vols,
                        is_ongoing,
                        Some(fv),
                        is_database_outdated,
                        upstream_error.as_deref(),
                    );

                    undated_items.push(UndatedReleaseItem {
                        series_id: s.series_id.clone(),
                        manga_id: s.manga_id,
                        komga_series_id: s.komga_series_id.clone(),
                        mangabaka_id: s.mangabaka_id,
                        title: s.title.clone(),
                        expected_volume: Some(fv),
                        diagnostic_code: diag_code.as_str().to_string(),
                        diagnostic_label: diag_code.label().to_string(),
                        reason,
                        source: "mangabaka".to_string(),
                        is_owned,
                        cover_url: cover,
                        scope: s.scope.clone(),
                    });
                    series_has_upcoming = true;
                }
            } else if is_ongoing {
                let cover = s.manga_id.map(|mid| format!("/api/manga/{mid}/cover"));
                let (diag_code, reason) = classify_missing_release_diagnostic(
                    conf_opt,
                    has_wid,
                    ot_sched_opt,
                    future_vols,
                    is_ongoing,
                    None,
                    is_database_outdated,
                    upstream_error.as_deref(),
                );

                undated_items.push(UndatedReleaseItem {
                    series_id: s.series_id.clone(),
                    manga_id: s.manga_id,
                    komga_series_id: s.komga_series_id.clone(),
                    mangabaka_id: s.mangabaka_id,
                    title: s.title.clone(),
                    expected_volume: None,
                    diagnostic_code: diag_code.as_str().to_string(),
                    diagnostic_label: diag_code.label().to_string(),
                    reason,
                    source: if ot_sched_opt.is_some() { "opentome".to_string() } else { "reconciled".to_string() },
                    is_owned: false,
                    cover_url: cover,
                    scope: s.scope.clone(),
                });
                series_has_upcoming = true;
            }
        }

        // If tracked and ongoing with no records at all, add to undated
        if !series_has_upcoming && is_ongoing {
            let cover = s.manga_id.map(|mid| format!("/api/manga/{mid}/cover"));
            let (diag_code, reason) = classify_missing_release_diagnostic(
                None,
                false,
                None,
                &[],
                is_ongoing,
                None,
                is_database_outdated,
                upstream_error.as_deref(),
            );
            undated_items.push(UndatedReleaseItem {
                series_id: s.series_id.clone(),
                manga_id: s.manga_id,
                komga_series_id: s.komga_series_id.clone(),
                mangabaka_id: s.mangabaka_id,
                title: s.title.clone(),
                expected_volume: None,
                diagnostic_code: diag_code.as_str().to_string(),
                diagnostic_label: diag_code.label().to_string(),
                reason,
                source: "catalog".to_string(),
                is_owned: false,
                cover_url: cover,
                scope: s.scope.clone(),
            });
        }
    }

    // Sort releases chronologically: earliest date first, then volume number
    all_releases.sort_by(|a, b| {
        a.release_date
            .cmp(&b.release_date)
            .then_with(|| {
                a.volume_number
                    .partial_cmp(&b.volume_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Deduplicate any identical (series_id, volume_number) releases (preferring 'en' over 'ja' if both)
    let mut deduplicated_releases: Vec<UpcomingReleaseItem> = Vec::new();
    let mut seen_release_keys = HashSet::new();
    for r in all_releases {
        let key = format!("{}:{}:{:?}", r.series_id, r.language, r.volume_number);
        if seen_release_keys.insert(key) {
            deduplicated_releases.push(r);
        }
    }

    // Compute metrics
    let next_7 = deduplicated_releases.iter().filter(|r| r.days_until <= 7).count();
    let next_30 = deduplicated_releases.iter().filter(|r| r.days_until <= 30).count();
    let next_90 = deduplicated_releases.iter().filter(|r| r.days_until <= 90).count();
    let total = deduplicated_releases.len();
    let undated_count = undated_items.len();

    Ok(UpcomingReleasesResponse {
        releases: deduplicated_releases,
        undated: undated_items,
        total_upcoming: total,
        next_7_days_count: next_7,
        next_30_days_count: next_30,
        next_90_days_count: next_90,
        undated_count,
        active_scope: active_scope.to_string(),
        last_refreshed_at: now_epoch,
    })
}

/// Retrieve the comprehensive release schedule for an individual series page.
pub fn get_title_release_schedule(
    conn: &Connection,
    manga_id: i64,
    today_str: &str,
) -> Result<TitleReleaseScheduleDto> {
    let (series_title, series_id): (String, String) = conn
        .query_row(
            "SELECT m.title, COALESCE(sr.series_id, 'manga:' || m.id)
             FROM manga m
             LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
             WHERE m.id = ?1",
            params![manga_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Query failed: {e}")))?
        .ok_or_else(|| AppError::not_found(format!("Manga {manga_id} not found")))?;

    // Check OpenTome mapping
    let ot_row = conn
        .query_row(
            "SELECT confidence, mapping_source, last_evaluated_at, metadata_json
             FROM opentome_series_mappings
             WHERE series_id = ?1",
            params![&series_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Query opentome mapping failed: {e}")))?;

    // Gather owned volumes
    let owned_volumes: HashSet<i64> = conn
        .query_row(
            "SELECT owned_volumes_json FROM series_reconciliations WHERE series_id = ?1",
            params![&series_id],
            |r| {
                let json_str: String = r.get(0)?;
                let list = serde_json::from_str::<Vec<f64>>(&json_str).unwrap_or_default();
                Ok(list.into_iter().map(|v| (v * 1000.0).round() as i64).collect())
            },
        )
        .optional()
        .unwrap_or(None)
        .unwrap_or_default();

    let mut upcoming_releases: Vec<UpcomingReleaseItem> = Vec::new();
    let mut recent_releases: Vec<UpcomingReleaseItem> = Vec::new();
    let primary_source = "OpenTome".to_string();
    let mut confidence = "none".to_string();
    let mut last_eval: i64 = 0;
    let cover = Some(format!("/api/manga/{manga_id}/cover"));

    if let Some((ref conf, ref _src, eval, ref meta_json_opt)) = ot_row {
        confidence = conf.clone();
        last_eval = eval;

        if let Some(ref json_str) = meta_json_opt {
            if let Ok(sched) = serde_json::from_str::<OpenTomeSeriesSchedule>(&json_str) {
                let en_line = sched.english.as_ref();
                let ja_line = sched.japanese.as_ref();
                let line_opt = en_line.or(ja_line);

                if let Some(line) = line_opt {
                    for vol in &line.volumes {
                        let is_upcoming = vol.status == "releases_today"
                            || vol.status == "future"
                            || vol.release_date.as_deref().map_or(false, |d| d >= today_str);

                        let r_date = vol.release_date.clone().or_else(|| vol.release_date_raw.clone()).unwrap_or_default();
                        let days_diff = if !r_date.is_empty() {
                            days_between_ymd(&r_date, today_str)
                        } else {
                            0
                        };

                        let is_owned = owned_volumes.contains(&((vol.volume_number * 1000.0).round() as i64));

                        let date_status = match vol.precision.as_str() {
                            "day" => {
                                if vol.date_type == "projected" {
                                    "tentative".to_string()
                                } else {
                                    "confirmed".to_string()
                                }
                            }
                            "month" | "year" => "tentative".to_string(),
                            _ => "unknown".to_string(),
                        };

                        let item = UpcomingReleaseItem {
                            series_id: series_id.clone(),
                            manga_id: Some(manga_id),
                            komga_series_id: None,
                            mangabaka_id: None,
                            title: series_title.clone(),
                            volume_number: Some(vol.volume_number),
                            chapter_number: None,
                            release_date: r_date.clone(),
                            release_date_raw: vol.release_date_raw.clone(),
                            precision: vol.precision.clone(),
                            date_status,
                            source: "opentome".to_string(),
                            language: line.market.clone(),
                            market: line.market.clone(),
                            format: "physical".to_string(),
                            publisher: line.publisher.clone(),
                            is_owned,
                            days_until: days_diff,
                            cover_url: cover.clone(),
                            scope: "library".to_string(),
                        };

                        if is_upcoming && !r_date.is_empty() && days_diff >= 0 {
                            upcoming_releases.push(item);
                        } else if vol.status == "released" || days_diff < 0 {
                            recent_releases.push(item);
                        }
                    }
                }
            }
        }
    }

    // Sort upcoming chronologically
    upcoming_releases.sort_by(|a, b| {
        a.release_date
            .cmp(&b.release_date)
            .then_with(|| a.volume_number.partial_cmp(&b.volume_number).unwrap_or(std::cmp::Ordering::Equal))
    });

    // Sort recent reverse chronologically (most recent published volume first)
    recent_releases.sort_by(|a, b| {
        b.volume_number
            .partial_cmp(&a.volume_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    recent_releases.truncate(5);

    let next_release = upcoming_releases.first().cloned();

    let (diagnostic_code, diagnostic_label, explanation) = if next_release.is_none() {
        let is_ongoing = conn
            .query_row(
                "SELECT is_ongoing FROM series_reconciliations WHERE series_id = ?1",
                params![&series_id],
                |r| Ok(r.get::<_, i64>(0)? != 0),
            )
            .optional()
            .unwrap_or(None)
            .unwrap_or(true);

        let future_volumes: Vec<f64> = conn
            .query_row(
                "SELECT future_volumes_json FROM series_reconciliations WHERE series_id = ?1",
                params![&series_id],
                |r| {
                    let s: String = r.get(0)?;
                    Ok(serde_json::from_str(&s).unwrap_or_default())
                },
            )
            .optional()
            .unwrap_or(None)
            .unwrap_or_default();

        let last_error: Option<String> = crate::settings::get(conn, "opentome.last_error");
        let installed: Option<String> = crate::settings::get(conn, "opentome.installed_version");
        let upstream: Option<String> = crate::settings::get(conn, "opentome.latest_upstream_version");
        let is_database_outdated = match (installed, upstream) {
            (Some(i), Some(u)) => i != u,
            _ => false,
        };

        let ot_conf = ot_row.as_ref().map(|(c, _, _, _)| c.as_str());
        let has_wid = ot_row.as_ref().map(|(_, _, _, m)| m.is_some()).unwrap_or(false);
        let ot_sched = ot_row.as_ref().and_then(|(_, _, _, m)| m.as_ref()).and_then(|j| serde_json::from_str::<OpenTomeSeriesSchedule>(j).ok());

        let (diag_code, reason) = classify_missing_release_diagnostic(
            ot_conf,
            has_wid,
            ot_sched.as_ref(),
            &future_volumes,
            is_ongoing,
            None,
            is_database_outdated,
            last_error.as_deref(),
        );

        (Some(diag_code.as_str().to_string()), Some(diag_code.label().to_string()), Some(reason))
    } else {
        (None, None, None)
    };

    Ok(TitleReleaseScheduleDto {
        manga_id,
        series_id,
        title: series_title,
        next_release,
        upcoming_volumes: upcoming_releases,
        recent_releases,
        primary_source,
        confidence,
        last_evaluated_at: last_eval,
        diagnostic_code,
        diagnostic_label,
        explanation,
    })
}

#[cfg(test)]
#[path = "releases_tests.rs"]
mod releases_tests;
