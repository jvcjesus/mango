//! Acquisition Coordination Service.
//!
//! Orchestrates Nyaa release discovery, TorBox external downloads, candidate ranking,
//! duplicate protection against canonical storage and download queues, and durable acquisition history.

use crate::acquisition::nyaa::{rank_candidates, NyaaClient, DEFAULT_NYAA_URL};
use crate::acquisition::torbox::{TorBoxClient, DEFAULT_TORBOX_URL};
use crate::acquisition::{
    AcquisitionCandidate, AcquisitionConfig, AcquisitionHistoryRecord, AcquisitionRelease,
    AcquisitionReleaseUnit,
};
use crate::db::Db;
use crate::error::{AppError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionConfigPatch {
    pub nyaa_enabled: Option<bool>,
    pub nyaa_url: Option<String>,
    pub nyaa_category: Option<String>,
    pub torbox_enabled: Option<bool>,
    pub torbox_url: Option<String>,
    pub torbox_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquireReleaseRequest {
    pub release: AcquisitionRelease,
    pub target_unit: Option<AcquisitionReleaseUnit>,
}

#[derive(Clone)]
pub struct AcquisitionService {
    pub db: Db,
    pub http: Client,
    pub pipeline: Option<Arc<crate::pipeline::Pipeline>>,
    pub wake_downloads: Option<Arc<Notify>>,
}

impl AcquisitionService {
    pub fn new(
        db: Db,
        http: Client,
        pipeline: Option<Arc<crate::pipeline::Pipeline>>,
        wake_downloads: Option<Arc<Notify>>,
    ) -> Self {
        Self {
            db,
            http,
            pipeline,
            wake_downloads,
        }
    }

    /// Loads public acquisition configuration, securely masking the TorBox API token.
    pub async fn load_config(&self) -> Result<AcquisitionConfig> {
        let (config, has_token) = self.load_internal_config().await?;
        Ok(AcquisitionConfig {
            has_torbox_token: has_token,
            ..config
        })
    }

    /// Loads internal acquisition configuration along with secret token status and value.
    pub async fn load_internal_config(&self) -> Result<(AcquisitionConfig, bool)> {
        self.db
            .call(|conn| {
                let nyaa_enabled = crate::db::state(conn, "acquisition.nyaa_enabled")?
                    .and_then(|v| v.parse::<bool>().ok())
                    .unwrap_or(true);
                let nyaa_url = crate::db::state(conn, "acquisition.nyaa_url")?
                    .or_else(|| Some(DEFAULT_NYAA_URL.to_string()));
                let nyaa_category = crate::db::state(conn, "acquisition.nyaa_category")?
                    .or_else(|| Some("3_1".to_string()));

                let torbox_enabled = crate::db::state(conn, "acquisition.torbox_enabled")?
                    .and_then(|v| v.parse::<bool>().ok())
                    .unwrap_or(false);
                let torbox_url = crate::db::state(conn, "acquisition.torbox_url")?
                    .or_else(|| Some(DEFAULT_TORBOX_URL.to_string()));

                let token_opt = crate::db::state(conn, "acquisition.torbox_token")?;
                let has_token = token_opt.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false);

                let torbox_validated = crate::db::state(conn, "acquisition.torbox_validated")?
                    .map(|v| v == "valid");
                let torbox_status_message = crate::db::state(conn, "acquisition.torbox_status_message")?;

                Ok((
                    AcquisitionConfig {
                        nyaa_enabled,
                        nyaa_url,
                        nyaa_category,
                        torbox_enabled,
                        torbox_url,
                        has_torbox_token: has_token,
                        torbox_validated,
                        torbox_status_message,
                    },
                    has_token,
                ))
            })
            .await
    }

    /// Fetches secret TorBox API token from state table without exposing it to public views.
    pub async fn get_torbox_token(&self) -> Result<Option<String>> {
        self.db
            .call(|conn| crate::db::state(conn, "acquisition.torbox_token"))
            .await
    }

    /// Updates acquisition configuration and stores secrets securely in the state table.
    pub async fn save_config(&self, patch: AcquisitionConfigPatch) -> Result<()> {
        self.db
            .call(move |conn| {
                if let Some(nyaa_enabled) = patch.nyaa_enabled {
                    crate::db::set_state(conn, "acquisition.nyaa_enabled", &nyaa_enabled.to_string())?;
                }
                if let Some(nyaa_url) = patch.nyaa_url {
                    crate::db::set_state(conn, "acquisition.nyaa_url", nyaa_url.trim())?;
                }
                if let Some(nyaa_cat) = patch.nyaa_category {
                    crate::db::set_state(conn, "acquisition.nyaa_category", nyaa_cat.trim())?;
                }
                if let Some(torbox_enabled) = patch.torbox_enabled {
                    crate::db::set_state(conn, "acquisition.torbox_enabled", &torbox_enabled.to_string())?;
                }
                if let Some(torbox_url) = patch.torbox_url {
                    crate::db::set_state(conn, "acquisition.torbox_url", torbox_url.trim())?;
                }
                if let Some(token) = patch.torbox_token {
                    let trimmed = token.trim();
                    if trimmed.is_empty() {
                        crate::db::forget_state(conn, "acquisition.torbox_token")?;
                        crate::db::forget_state(conn, "acquisition.torbox_validated")?;
                        crate::db::forget_state(conn, "acquisition.torbox_status_message")?;
                    } else {
                        crate::db::set_state(conn, "acquisition.torbox_token", trimmed)?;
                    }
                }
                Ok(())
            })
            .await
    }

    /// Tests connection to TorBox and records validation status.
    pub async fn test_torbox(&self, url: Option<String>, token: Option<String>) -> Result<String> {
        let actual_token = match token {
            Some(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => self.get_torbox_token().await?,
        };

        let client = TorBoxClient::new(self.http.clone(), url, actual_token);
        match client.test_connection().await {
            Ok(msg) => {
                let msg_clone = msg.clone();
                self.db
                    .call(move |conn| {
                        crate::db::set_state(conn, "acquisition.torbox_validated", "valid")?;
                        crate::db::set_state(conn, "acquisition.torbox_status_message", &msg_clone)?;
                        Ok(())
                    })
                    .await?;
                Ok(msg)
            }
            Err(e) => {
                let err_msg = e.to_string();
                let err_clone = err_msg.clone();
                let _ = self
                    .db
                    .call(move |conn| {
                        crate::db::set_state(conn, "acquisition.torbox_validated", "invalid")?;
                        crate::db::set_state(conn, "acquisition.torbox_status_message", &err_clone)?;
                        Ok(())
                    })
                    .await;
                Err(e.into())
            }
        }
    }

    /// Searches Nyaa for candidates for a given title, matching aliases and ranking candidates.
    pub async fn search_candidates(
        &self,
        manga_id: i64,
        target_unit: Option<AcquisitionReleaseUnit>,
    ) -> Result<Vec<AcquisitionCandidate>> {
        let (config, _) = self.load_internal_config().await?;
        if !config.nyaa_enabled {
            return Err(AppError::bad_request("Nyaa release discovery is disabled in Settings"));
        }

        // Fetch title and known aliases from database
        let (title, aliases) = self
            .db
            .call(move |conn| {
                let title: String = conn.query_row(
                    "SELECT title FROM manga WHERE id = ?1",
                    [manga_id],
                    |r| r.get(0),
                )?;

                let mut aliases = Vec::new();
                let mut stmt = conn.prepare(
                    "SELECT aliases FROM tracker_list_entries WHERE title = ?1 OR english_title = ?1 OR romaji_title = ?1",
                )?;
                let alias_rows = stmt
                    .query_map([&title], |r| r.get::<_, Option<String>>(0))?
                    .filter_map(std::result::Result::ok)
                    .flatten();

                for json_str in alias_rows {
                    if let Ok(list) = serde_json::from_str::<Vec<String>>(&json_str) {
                        for a in list {
                            if !aliases.contains(&a) {
                                aliases.push(a);
                            }
                        }
                    }
                }

                Ok((title, aliases))
            })
            .await?;

        let nyaa = NyaaClient::new(self.http.clone(), config.nyaa_url);
        let releases = nyaa
            .search(&title, config.nyaa_category.as_deref())
            .await
            .unwrap_or_default();

        let mut candidates = rank_candidates(releases, &title, &aliases, target_unit.as_ref());

        // Check ownership status for each candidate against DB and canonical library
        let (owned_units, acquiring_units) = self.get_title_acquisition_state(manga_id, &title).await?;

        for cand in &mut candidates {
            match &cand.release.release_unit {
                AcquisitionReleaseUnit::Volume(v) => {
                    if owned_units.contains(&format!("v{v}")) {
                        cand.ownership_status = "owned".to_string();
                    } else if acquiring_units.contains(&format!("v{v}")) {
                        cand.ownership_status = "acquiring".to_string();
                    }
                }
                AcquisitionReleaseUnit::Chapter(c) => {
                    if owned_units.contains(&format!("c{c}")) {
                        cand.ownership_status = "owned".to_string();
                    } else if acquiring_units.contains(&format!("c{c}")) {
                        cand.ownership_status = "acquiring".to_string();
                    }
                }
                _ => {}
            }
        }

        Ok(candidates)
    }

    /// Helper to get sets of owned and acquiring unit identifiers (e.g. "v1", "c12").
    async fn get_title_acquisition_state(
        &self,
        manga_id: i64,
        title: &str,
    ) -> Result<(std::collections::HashSet<String>, std::collections::HashSet<String>)> {
        let title_owned = if let Some(pipeline) = &self.pipeline {
            Some(pipeline.ownership.get_title_ownership(manga_id, title).await)
        } else {
            None
        };

        let mut owned_set = std::collections::HashSet::new();
        let mut acquiring_set = std::collections::HashSet::new();

        if let Some(own) = title_owned {
            for v in own.volumes {
                if v.status == crate::pipeline::ownership::OwnershipStatus::Owned {
                    owned_set.insert(format!("v{}", v.volume));
                } else if v.status == crate::pipeline::ownership::OwnershipStatus::Acquiring {
                    acquiring_set.insert(format!("v{}", v.volume));
                }
            }
        }

        // Also check downloads queue
        let queued_items: Vec<(String, f64, String)> = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.name, c.chapter_number, d.state FROM downloads d JOIN chapters c ON c.id = d.chapter_id WHERE c.manga_id = ?1",
                )?;
                let rows = stmt
                    .query_map([manga_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .filter_map(std::result::Result::ok)
                    .collect();
                Ok(rows)
            })
            .await?;

        for (name, num, state) in queued_items {
            if state != "failed" {
                let rel = crate::release::parse_release(&name, num);
                if rel.release_type == crate::release::ReleaseType::Volume {
                    acquiring_set.insert(format!("v{}", rel.volume.unwrap_or(num)));
                } else {
                    acquiring_set.insert(format!("c{}", rel.chapter_number.unwrap_or(num)));
                }
            }
        }

        Ok((owned_set, acquiring_set))
    }

    /// Enqueues an external release for acquisition via TorBox.
    ///
    /// Performs strict PRE-SUBMISSION duplicate checks against canonical library and download queue (Requirement 11, Scenario W).
    pub async fn acquire_release(
        &self,
        manga_id: i64,
        release: AcquisitionRelease,
        target_unit: Option<AcquisitionReleaseUnit>,
    ) -> Result<i64> {
        let (config, has_token) = self.load_internal_config().await?;
        if !config.torbox_enabled {
            return Err(AppError::bad_request("TorBox download backend is disabled in Settings"));
        }
        if !has_token {
            return Err(AppError::bad_request("TorBox API token is not configured. Add it in Settings."));
        }

        let title: String = self
            .db
            .call(move |conn| Ok(conn.query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |r| r.get(0))?))
            .await?;

        // 1. DUPLICATE CHECK: Verify canonical filesystem storage (Scenario W)
        if let Some(pipeline) = &self.pipeline {
            let (pipe_cfg, _, _) = pipeline.load_internal_config().await?;
            if let Some(ref lib_dir) = pipe_cfg.library_dir {
                let canonical_target = Path::new(lib_dir).join(&title);
                if canonical_target.is_dir() {
                    // Check if unit already exists on canonical filesystem
                    if let Some(ref unit) = target_unit.as_ref().or(Some(&release.release_unit)) {
                        if self.canonical_file_exists_for_unit(&canonical_target, unit) {
                            return Err(AppError::bad_request(format!(
                                "Cannot acquire: {unit} already exists in canonical library at {}",
                                canonical_target.display()
                            )));
                        }
                    }
                }
            }
        }

        // 2. DUPLICATE CHECK: Verify download queue (Scenario W)
        let info_hash = release.info_hash.clone();
        let release_unit_check = target_unit.clone().unwrap_or_else(|| release.release_unit.clone());

        let is_duplicate_download: bool = self
            .db
            .call(move |conn| {
                // Check matching torrent hash
                if let Some(ref hash) = info_hash {
                    let exists: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM downloads WHERE torrent_hash = ?1 AND state != 'failed')",
                            [hash],
                            |r| r.get(0),
                        )
                        .unwrap_or(false);
                    if exists {
                        return Ok(true);
                    }
                }

                // Check active unit acquisition for this manga
                let mut stmt = conn.prepare(
                    "SELECT c.name, c.chapter_number FROM downloads d JOIN chapters c ON c.id = d.chapter_id WHERE c.manga_id = ?1 AND d.state != 'failed'",
                )?;
                let active = stmt
                    .query_map([manga_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))?
                    .filter_map(std::result::Result::ok);

                for (name, num) in active {
                    let rel = crate::release::parse_release(&name, num);
                    match &release_unit_check {
                        AcquisitionReleaseUnit::Volume(target_v) => {
                            if rel.release_type == crate::release::ReleaseType::Volume
                                && (rel.volume.unwrap_or(num) - target_v).abs() < 0.001
                            {
                                return Ok(true);
                            }
                        }
                        AcquisitionReleaseUnit::Chapter(target_c) => {
                            if rel.release_type == crate::release::ReleaseType::Chapter
                                && (rel.chapter_number.unwrap_or(num) - target_c).abs() < 0.001
                            {
                                return Ok(true);
                            }
                        }
                        _ => {}
                    }
                }

                Ok(false)
            })
            .await?;

        if is_duplicate_download {
            return Err(AppError::bad_request(
                "Cannot acquire: release or unit is already in the download queue",
            ));
        }

        // 3. Create or link chapter record in database
        let chapter_unit = target_unit.unwrap_or_else(|| release.release_unit.clone());
        let (ch_name, ch_number) = match &chapter_unit {
            AcquisitionReleaseUnit::Volume(v) => (format!("Volume {v}"), *v),
            AcquisitionReleaseUnit::Chapter(c) => (format!("Chapter {c}"), *c),
            AcquisitionReleaseUnit::Collection { start, end } => (format!("Volumes {start}-{end}"), *start),
            AcquisitionReleaseUnit::Unknown => (release.title.clone(), -1.0),
        };

        let ch_url = format!("nyaa:{}", release.source_item_id);
        let uploader = release.uploader.clone();
        let raw_title = release.raw_title.clone();
        let release_size = release.size_bytes;
        let torrent_hash = release.info_hash.clone();

        let chapter_id: i64 = self
            .db
            .call(move |conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                // Try to find existing chapter with this URL or unit
                let existing_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM chapters WHERE manga_id = ?1 AND url = ?2",
                        rusqlite::params![manga_id, &ch_url],
                        |r| r.get(0),
                    )
                    .ok();

                let id = if let Some(id) = existing_id {
                    id
                } else {
                    conn.execute(
                        "INSERT INTO chapters (manga_id, url, name, chapter_number, scanlator, source_order, fetched_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                        rusqlite::params![manga_id, &ch_url, &ch_name, ch_number, &uploader, now],
                    )?;
                    conn.last_insert_rowid()
                };

                // Insert into downloads queue with external backend metadata
                conn.execute(
                    "INSERT INTO downloads (
                        chapter_id, state, position, attempts, added_at, source_type, backend_type,
                        raw_title, uploader, release_size, torrent_hash
                    ) VALUES (
                        ?1, 'queued', (SELECT COALESCE(MAX(position), 0) + 1 FROM downloads), 0, ?2,
                        'nyaa', 'torbox', ?3, ?4, ?5, ?6
                    ) ON CONFLICT(chapter_id) DO UPDATE SET
                        state = 'queued', attempts = 0, retry_at = NULL, error = NULL,
                        source_type = 'nyaa', backend_type = 'torbox',
                        raw_title = excluded.raw_title, uploader = excluded.uploader,
                        release_size = excluded.release_size, torrent_hash = excluded.torrent_hash",
                    rusqlite::params![id, now, &raw_title, &uploader, release_size as i64, &torrent_hash],
                )?;

                Ok(id)
            })
            .await?;

        // Wake downloads worker
        if let Some(wake) = &self.wake_downloads {
            wake.notify_one();
        }

        Ok(chapter_id)
    }

    fn canonical_file_exists_for_unit(&self, series_dir: &Path, unit: &AcquisitionReleaseUnit) -> bool {
        let entries = match std::fs::read_dir(series_dir) {
            Ok(e) => e,
            Err(_) => return false,
        };

        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".cbz") {
                continue;
            }

            let (file_unit, _, _) = crate::acquisition::nyaa::classify_release(name);
            match (unit, file_unit) {
                (AcquisitionReleaseUnit::Volume(target_v), AcquisitionReleaseUnit::Volume(v)) => {
                    if (target_v - v).abs() < 0.001 {
                        return true;
                    }
                }
                (AcquisitionReleaseUnit::Chapter(target_c), AcquisitionReleaseUnit::Chapter(c)) => {
                    if (target_c - c).abs() < 0.001 {
                        return true;
                    }
                }
                _ => {}
            }
        }

        false
    }

    /// Records a durable acquisition history entry (Requirement 11, Scenario AB).
    pub async fn record_history(
        &self,
        chapter_id: Option<i64>,
        manga_id: i64,
        series: &str,
        release_name: &str,
        source_type: &str,
        uploader: Option<&str>,
        external_job_id: Option<&str>,
        backend_type: &str,
        outcome: &str,
        canonical_path: Option<&str>,
        release_size: Option<u64>,
        error: Option<&str>,
    ) -> Result<()> {
        let series = series.to_string();
        let release_name = release_name.to_string();
        let source_type = source_type.to_string();
        let uploader = uploader.map(|s| s.to_string());
        let external_job_id = external_job_id.map(|s| s.to_string());
        let backend_type = backend_type.to_string();
        let outcome = outcome.to_string();
        let canonical_path = canonical_path.map(|s| s.to_string());
        let error = error.map(|s| s.to_string());

        self.db
            .call(move |conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                conn.execute(
                    "INSERT INTO acquisition_history (
                        chapter_id, manga_id, series, release_name, source_type, uploader,
                        external_job_id, backend_type, outcome, canonical_path, release_size,
                        completed_at, error
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    rusqlite::params![
                        chapter_id,
                        manga_id,
                        &series,
                        &release_name,
                        &source_type,
                        uploader.as_deref(),
                        external_job_id.as_deref(),
                        &backend_type,
                        &outcome,
                        canonical_path.as_deref(),
                        release_size.map(|s| s as i64),
                        now,
                        error.as_deref(),
                    ],
                )?;
                Ok(())
            })
            .await
    }

    /// Lists acquisition history records (Scenario AB).
    pub async fn list_history(
        &self,
        manga_id: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Vec<AcquisitionHistoryRecord>> {
        let max_rows = limit.unwrap_or(50).max(1).min(500);

        self.db
            .call(move |conn| {
                let mut sql = "SELECT id, chapter_id, manga_id, series, release_name, source_type, uploader,
                                      external_job_id, backend_type, outcome, canonical_path, release_size,
                                      completed_at, error
                               FROM acquisition_history".to_string();
                if manga_id.is_some() {
                    sql.push_str(" WHERE manga_id = ?1");
                }
                sql.push_str(" ORDER BY completed_at DESC LIMIT ?2");

                let mut stmt = conn.prepare(&sql)?;
                let rows = if let Some(m_id) = manga_id {
                    stmt.query_map(rusqlite::params![m_id, max_rows], |r| {
                        Ok(AcquisitionHistoryRecord {
                            id: r.get(0)?,
                            chapter_id: r.get(1)?,
                            manga_id: r.get(2)?,
                            series: r.get(3)?,
                            release_name: r.get(4)?,
                            source_type: r.get(5)?,
                            uploader: r.get(6)?,
                            external_job_id: r.get(7)?,
                            backend_type: r.get(8)?,
                            outcome: r.get(9)?,
                            canonical_path: r.get(10)?,
                            release_size: r.get::<_, Option<i64>>(11)?.map(|s| s as u64),
                            completed_at: r.get(12)?,
                            error: r.get(13)?,
                        })
                    })?
                    .filter_map(std::result::Result::ok)
                    .collect()
                } else {
                    stmt.query_map(rusqlite::params![max_rows, max_rows], |r| {
                        Ok(AcquisitionHistoryRecord {
                            id: r.get(0)?,
                            chapter_id: r.get(1)?,
                            manga_id: r.get(2)?,
                            series: r.get(3)?,
                            release_name: r.get(4)?,
                            source_type: r.get(5)?,
                            uploader: r.get(6)?,
                            external_job_id: r.get(7)?,
                            backend_type: r.get(8)?,
                            outcome: r.get(9)?,
                            canonical_path: r.get(10)?,
                            release_size: r.get::<_, Option<i64>>(11)?.map(|s| s as u64),
                            completed_at: r.get(12)?,
                            error: r.get(13)?,
                        })
                    })?
                    .filter_map(std::result::Result::ok)
                    .collect()
                };

                Ok(rows)
            })
            .await
    }
}
