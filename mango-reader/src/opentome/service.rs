use super::catalog::{
    self, OpenTomeSeriesSchedule,
};
use super::updater::{OpenTomeStatus, OpenTomeUpdater, DEFAULT_MANIFEST_URL};
use crate::acquisition::release_discovery::ReleaseDiscoveryTarget;
use crate::db::Db;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::Path;
use std::sync::Arc;

pub struct OpenTomeService {
    db: Db,
    updater: Arc<OpenTomeUpdater>,
    http: reqwest::Client,
}

impl OpenTomeService {
    pub fn new(db: Db, data_dir: &Path, http: reqwest::Client) -> Self {
        let updater = Arc::new(OpenTomeUpdater::new(data_dir, http.clone()));
        Self { db, updater, http }
    }

    pub fn updater(&self) -> &Arc<OpenTomeUpdater> {
        &self.updater
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub async fn is_enabled(&self) -> bool {
        self.db
            .call(|conn| {
                Ok(crate::settings::get::<bool>(conn, "opentome.enabled").unwrap_or(true))
            })
            .await
            .unwrap_or(true)
    }

    pub async fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.db
            .call(move |conn| {
                crate::settings::set(conn, "opentome.enabled", &serde_json::json!(enabled))?;
                Ok(())
            })
            .await
    }

    pub async fn status(&self) -> Result<OpenTomeStatus> {
        let (enabled, last_checked, last_error) = self
            .db
            .call(|conn| {
                let enabled = crate::settings::get::<bool>(conn, "opentome.enabled").unwrap_or(true);
                let last_checked = crate::settings::get::<i64>(conn, "opentome.last_checked_at");
                let last_error = crate::settings::get::<String>(conn, "opentome.last_error");
                Ok((enabled, last_checked, last_error))
            })
            .await?;

        Ok(self.updater.status(enabled, last_checked, last_error))
    }

    /// Trigger manual or scheduled update check.
    pub async fn check_for_updates(&self, force: bool) -> Result<bool> {
        let manifest_url = self
            .db
            .call(|conn| {
                Ok(crate::settings::get::<String>(conn, "opentome.manifest_url")
                    .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string()))
            })
            .await
            .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string());

        let now_epoch = crate::db::now();
        let result = self.updater.check_and_update(&manifest_url, force).await;

        let err_msg = result.as_ref().err().map(|e| e.to_string());
        self.db
            .call(move |conn| {
                let _ = crate::settings::set(conn, "opentome.last_checked_at", &serde_json::json!(now_epoch));
                if let Some(ref err) = err_msg {
                    let _ = crate::settings::set(conn, "opentome.last_error", &serde_json::json!(err));
                } else {
                    let _ = conn.execute("DELETE FROM settings WHERE key = 'opentome.last_error'", []);
                }
                Ok(())
            })
            .await?;

        result
    }

    pub fn open_catalogue_connection(&self) -> Result<Option<Connection>> {
        if !self.updater.is_available() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            self.updater.db_path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| AppError::internal(format!("Failed to open OpenTome catalogue: {e}")))?;
        Ok(Some(conn))
    }

    /// Get current local date string (YYYY-MM-DD) formatted according to server/container timezone.
    pub async fn current_date_str(&self) -> String {
        self.db
            .call(|conn| {
                let d: String = conn
                    .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
                    .unwrap_or_else(|_| {
                        let s = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        format!("{:04}-{:02}-{:02}", 1970 + s / 31557600, 1, 1)
                    });
                Ok(d)
            })
            .await
            .unwrap_or_else(|_| "1970-01-01".to_string())
    }

    /// Retrieve or resolve OpenTome schedule for a specific manga series.
    pub async fn get_series_schedule(
        &self,
        manga_id: i64,
    ) -> Result<Option<OpenTomeSeriesSchedule>> {
        if !self.is_enabled().await {
            return Ok(None);
        }

        let ot_conn_opt = self.open_catalogue_connection()?;
        let Some(ot_conn) = ot_conn_opt else {
            return Ok(None);
        };

        let today = self.current_date_str().await;

        self.db
            .call(move |conn| {
                Self::resolve_or_refresh_schedule_internal(conn, &ot_conn, manga_id, &today)
            })
            .await
    }

    /// Internal synchronous function to resolve/refresh schedule within a DB transaction.
    fn resolve_or_refresh_schedule_internal(
        conn: &Connection,
        ot_conn: &Connection,
        manga_id: i64,
        today_str: &str,
    ) -> Result<Option<OpenTomeSeriesSchedule>> {
        let (series_title, series_id, komga_series_id, _mb_id): (String, String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT m.title, 
                        COALESCE(sr.series_id, 'manga:' || m.id), 
                        sr.komga_series_id, 
                        sr.mangabaka_id
                 FROM manga m
                 LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                 WHERE m.id = ?1",
                params![manga_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| AppError::internal(format!("Query failed: {e}")))?
            .ok_or_else(|| AppError::not_found(format!("Manga {manga_id} not found")))?;

        let anilist_id: Option<i64> = if let Some(ref ksid) = komga_series_id {
            conn.query_row(
                "SELECT tracker_id FROM tracker_mappings WHERE komga_series_id = ?1 AND tracker = 'anilist'",
                params![ksid],
                |r| {
                    let tid: String = r.get(0)?;
                    Ok(tid.parse::<i64>().ok())
                },
            )
            .optional()
            .unwrap_or(None)
            .flatten()
        } else {
            None
        };

        // Check existing mapping in `opentome_series_mappings`
        let existing = conn
            .query_row(
                "SELECT work_id, confidence, mapping_source, metadata_json, last_evaluated_at
                 FROM opentome_series_mappings
                 WHERE series_id = ?1",
                params![&series_id],
                |r| {
                    let wid: Option<String> = r.get(0)?;
                    let conf: String = r.get(1)?;
                    let src: String = r.get(2)?;
                    let meta: Option<String> = r.get(3)?;
                    let eval: i64 = r.get(4)?;
                    Ok((wid, conf, src, meta, eval))
                },
            )
            .optional()
            .unwrap_or(None);

        if let Some((work_id_opt, confidence, _src, _meta_json_opt, _last_eval)) = existing {
            if confidence == "rejected" {
                return Ok(None);
            }
            if let Some(work_id) = work_id_opt {
                // Re-evaluate schedule from catalogue with today's date
                let schedule = catalog::evaluate_work_schedule(ot_conn, &work_id, &confidence, today_str)
                    .map_err(|e| AppError::internal(format!("OpenTome schedule evaluation failed: {e}")))?;

                let schedule_json = serde_json::to_string(&schedule).unwrap_or_default();
                let now = crate::db::now();
                let _ = conn.execute(
                    "UPDATE opentome_series_mappings 
                     SET metadata_json = ?1, last_evaluated_at = ?2, updated_at = ?2
                     WHERE series_id = ?3",
                    params![schedule_json, now, &series_id],
                );
                return Ok(Some(schedule));
            } else if confidence == "ambiguous" || confidence == "unmatched" {
                return Ok(None);
            }
        }

        // No existing mapping: perform resolution
        let (resolved_confidence, resolved_work_id, mapping_source, title_matched): (&str, Option<String>, &str, String) = {
            let mut resolved: Option<(&str, Option<String>, &str, String)> = None;

            // Step 1: AniList ID match (Tier 1)
            if let Some(aid) = anilist_id {
                if let Ok(Some(wid)) = catalog::find_work_by_anilist_id(ot_conn, aid) {
                    resolved = Some(("exact", Some(wid), "anilist_id", series_title.clone()));
                }
            }

            // Step 2: Title or alias search (Tier 2)
            if resolved.is_none() {
                let candidates = catalog::find_works_by_title_or_alias(ot_conn, &series_title)
                    .unwrap_or_default();
                if candidates.len() == 1 {
                    resolved = Some(("high", Some(candidates[0].0.clone()), "title_exact", candidates[0].1.clone()));
                } else if candidates.len() > 1 {
                    resolved = Some(("ambiguous", None, "title_multiple", series_title.clone()));
                } else {
                    resolved = Some(("unmatched", None, "auto", series_title.clone()));
                }
            }

            resolved.unwrap_or(("unmatched", None, "auto", series_title.clone()))
        };

        let now = crate::db::now();

        if let Some(ref work_id) = resolved_work_id {
            let lines = catalog::get_work_lines(ot_conn, work_id)
                .map_err(|e| AppError::internal(format!("Failed to query work lines: {e}")))?;
            let ja = catalog::pick_main_line(&lines, "ja");
            let en = catalog::pick_main_line(&lines, "en");

            let schedule = catalog::evaluate_work_schedule(ot_conn, work_id, resolved_confidence, today_str)
                .map_err(|e| AppError::internal(format!("OpenTome schedule evaluation failed: {e}")))?;

            let schedule_json = serde_json::to_string(&schedule).unwrap_or_default();

            conn.execute(
                "INSERT OR REPLACE INTO opentome_series_mappings
                 (manga_id, series_id, work_id, japanese_series_id, english_series_id,
                  japanese_tome_id, english_tome_id, confidence, mapping_source,
                  title_matched, last_evaluated_at, metadata_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?11, ?11)",
                params![
                    manga_id,
                    &series_id,
                    work_id,
                    ja.as_ref().map(|l| l.gcd_series_id),
                    en.as_ref().map(|l| l.gcd_series_id),
                    ja.as_ref().map(|l| l.tome_id.clone()),
                    en.as_ref().map(|l| l.tome_id.clone()),
                    resolved_confidence,
                    mapping_source,
                    &title_matched,
                    now,
                    schedule_json,
                ],
            )
            .map_err(|e| AppError::internal(format!("Failed to persist OpenTome mapping: {e}")))?;

            Ok(Some(schedule))
        } else {
            conn.execute(
                "INSERT OR REPLACE INTO opentome_series_mappings
                 (manga_id, series_id, work_id, confidence, mapping_source,
                  title_matched, last_evaluated_at, metadata_json, created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, NULL, ?6, ?6)",
                params![
                    manga_id,
                    &series_id,
                    resolved_confidence,
                    mapping_source,
                    &title_matched,
                    now,
                ],
            )
            .map_err(|e| AppError::internal(format!("Failed to persist OpenTome mapping: {e}")))?;

            Ok(None)
        }
    }

    /// User action to confirm, reject, or reset a mapping.
    pub async fn remap_manga(&self, manga_id: i64, work_id: Option<String>, action: &str) -> Result<()> {
        let ot_conn_opt = self.open_catalogue_connection()?;
        let today = self.current_date_str().await;
        let act = action.to_string();

        self.db
            .call(move |conn| {
                let (series_title, series_id) = conn
                    .query_row(
                        "SELECT m.title, COALESCE(sr.series_id, 'manga:' || m.id)
                         FROM manga m
                         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                         WHERE m.id = ?1",
                        params![manga_id],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .map_err(|e| AppError::not_found(format!("Manga not found: {e}")))?;

                let now = crate::db::now();

                match act.as_str() {
                    "reject" => {
                        conn.execute(
                            "INSERT OR REPLACE INTO opentome_series_mappings
                             (manga_id, series_id, work_id, confidence, mapping_source,
                              title_matched, last_evaluated_at, metadata_json, created_at, updated_at)
                             VALUES (?1, ?2, NULL, 'rejected', 'manual', ?3, ?4, NULL, ?4, ?4)",
                            params![manga_id, &series_id, &series_title, now],
                        )?;
                    }
                    "confirm" => {
                        let wid = work_id.ok_or_else(|| AppError::bad_request("workId is required to confirm mapping"))?;
                        let Some(ref ot_conn) = ot_conn_opt else {
                            return Err(AppError::bad_request("OpenTome catalogue is not available"));
                        };

                        let lines = catalog::get_work_lines(ot_conn, &wid)
                            .map_err(|e| AppError::internal(format!("Failed to query work lines: {e}")))?;
                        let ja = catalog::pick_main_line(&lines, "ja");
                        let en = catalog::pick_main_line(&lines, "en");

                        let schedule = catalog::evaluate_work_schedule(ot_conn, &wid, "user_confirmed", &today)
                            .map_err(|e| AppError::internal(format!("Schedule evaluation failed: {e}")))?;
                        let schedule_json = serde_json::to_string(&schedule).unwrap_or_default();

                        conn.execute(
                            "INSERT OR REPLACE INTO opentome_series_mappings
                             (manga_id, series_id, work_id, japanese_series_id, english_series_id,
                              japanese_tome_id, english_tome_id, confidence, mapping_source,
                              title_matched, last_evaluated_at, metadata_json, created_at, updated_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'user_confirmed', 'manual', ?8, ?9, ?10, ?9, ?9)",
                            params![
                                manga_id,
                                &series_id,
                                &wid,
                                ja.as_ref().map(|l| l.gcd_series_id),
                                en.as_ref().map(|l| l.gcd_series_id),
                                ja.as_ref().map(|l| l.tome_id.clone()),
                                en.as_ref().map(|l| l.tome_id.clone()),
                                &series_title,
                                now,
                                schedule_json,
                            ],
                        )?;
                    }
                    "auto" => {
                        conn.execute(
                            "DELETE FROM opentome_series_mappings WHERE series_id = ?1",
                            params![&series_id],
                        )?;
                    }
                    _ => return Err(AppError::bad_request("Invalid action; must be 'confirm', 'reject', or 'auto'")),
                }
                Ok(())
            })
            .await
    }

    /// Check release-day transitions for followed titles and record domain events.
    pub async fn evaluate_release_day_events(&self) -> Result<usize> {
        if !self.is_enabled().await {
            return Ok(0);
        }

        let ot_conn_opt = self.open_catalogue_connection()?;
        let Some(ot_conn) = ot_conn_opt else {
            return Ok(0);
        };

        let today = self.current_date_str().await;

        let http = self.http.clone();

        let count = self.db
            .call(move |conn| {
                let activation_date = crate::acquisition::release_discovery::get_or_init_activation_date(conn)?;

                let mut stmt = conn.prepare(
                    "SELECT sr.manga_id, sr.series_id, sr.title, sr.mangabaka_id
                     FROM series_reconciliations sr
                     WHERE sr.status != 'unfollowed' AND sr.manga_id IS NOT NULL",
                )?;

                let rows = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?))
                })?;

                let mut event_count = 0;

                for row_res in rows {
                    let (manga_id, series_id, series_title, mb_id) = match row_res {
                        Ok(t) => t,
                        Err(_) => continue,
                    };

                    let schedule_opt = Self::resolve_or_refresh_schedule_internal(conn, &ot_conn, manga_id, &today)
                        .unwrap_or(None);

                    if let Some(schedule) = schedule_opt {
                        // Sync release discovery targets
                        let _ = crate::acquisition::release_discovery::sync_series_release_targets(
                            conn,
                            manga_id,
                            &series_id,
                            &series_title,
                            &schedule,
                            &today,
                            &activation_date,
                        );

                        // Check English next volume
                        if let Some(ref en) = schedule.english {
                            if let Some(ref next_vol) = en.next_volume {
                                if next_vol.status == "releases_today" {
                                    let msg = format!(
                                        "{} Vol. {} (English) releases today ({})",
                                        series_title,
                                        next_vol.volume_number,
                                        next_vol.release_date.as_deref().unwrap_or(&today)
                                    );
                                    let _ = crate::sync::reconciliation::record_event_full(
                                        conn,
                                        &series_id,
                                        &series_title,
                                        mb_id,
                                        None,
                                        "standard",
                                        Some(next_vol.volume_number),
                                        "VOLUME_RELEASES_TODAY",
                                        None,
                                        0,
                                        &msg,
                                    );
                                    event_count += 1;
                                }
                            }
                        }

                        // Check Japanese next volume
                        if let Some(ref ja) = schedule.japanese {
                            if let Some(ref next_vol) = ja.next_volume {
                                if next_vol.status == "releases_today" {
                                    let msg = format!(
                                        "{} Vol. {} (Japanese) releases today ({})",
                                        series_title,
                                        next_vol.volume_number,
                                        next_vol.release_date.as_deref().unwrap_or(&today)
                                    );
                                    let _ = crate::sync::reconciliation::record_event_full(
                                        conn,
                                        &series_id,
                                        &series_title,
                                        mb_id,
                                        None,
                                        "standard",
                                        Some(next_vol.volume_number),
                                        "VOLUME_RELEASES_TODAY_JA",
                                        None,
                                        0,
                                        &msg,
                                    );
                                    event_count += 1;
                                }
                            }
                        }
                    }
                }

                Ok(event_count)
            })
            .await?;

        // Evaluate all pending release discovery targets
        let _ = crate::acquisition::release_discovery::evaluate_all_pending_targets(&self.db, http).await;

        Ok(count)
    }

    /// Evaluate and trigger release-aware discovery targets for a specific manga.
    pub async fn evaluate_manga_release_discovery(
        &self,
        manga_id: i64,
    ) -> Result<Vec<ReleaseDiscoveryTarget>> {
        if !self.is_enabled().await {
            return self.db.call(move |conn| crate::acquisition::release_discovery::list_targets_for_manga(conn, manga_id)).await;
        }

        let ot_conn_opt = self.open_catalogue_connection()?;
        let Some(ot_conn) = ot_conn_opt else {
            return self.db.call(move |conn| crate::acquisition::release_discovery::list_targets_for_manga(conn, manga_id)).await;
        };

        let today = self.current_date_str().await;
        let http = self.http.clone();

        // 1. Resolve schedule and sync targets
        let due_target_ids: Vec<i64> = self.db
            .call(move |conn| {
                let activation_date = crate::acquisition::release_discovery::get_or_init_activation_date(conn)?;

                let (series_title, series_id): (String, String) = conn
                    .query_row(
                        "SELECT m.title, COALESCE(sr.series_id, 'manga:' || m.id)
                         FROM manga m
                         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                         WHERE m.id = ?1",
                        params![manga_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|e| AppError::not_found(format!("Manga not found: {e}")))?;

                if let Some(schedule) = Self::resolve_or_refresh_schedule_internal(conn, &ot_conn, manga_id, &today)? {
                    let _ = crate::acquisition::release_discovery::sync_series_release_targets(
                        conn,
                        manga_id,
                        &series_id,
                        &series_title,
                        &schedule,
                        &today,
                        &activation_date,
                    );
                }

                let cur_time = crate::db::now();
                let mut stmt = conn.prepare(
                    "SELECT id FROM release_discovery_targets
                     WHERE manga_id = ?1
                       AND (status = 'releases_today' OR (status = 'not_found_retry_wait' AND next_attempt_at <= ?2))
                     ORDER BY id ASC",
                )?;
                let rows = stmt.query_map(params![manga_id, cur_time], |r| r.get(0))?;
                let mut ids = Vec::new();
                for r in rows {
                    ids.push(r?);
                }
                Ok(ids)
            })
            .await?;

        // 2. Process due targets for this manga
        for tid in due_target_ids {
            let _ = crate::acquisition::release_discovery::process_target_discovery(&self.db, http.clone(), tid).await;
        }

        // 3. Return all targets for this manga
        self.db
            .call(move |conn| crate::acquisition::release_discovery::list_targets_for_manga(conn, manga_id))
            .await
    }
}
