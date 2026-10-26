//! Acquisition Coordination Service.
//!
//! Orchestrates Nyaa release discovery, TorBox external downloads, candidate ranking,
//! duplicate protection against canonical storage and download queues, and durable acquisition history.

use crate::acquisition::candidates::NormalizedAcquisitionCandidate;
use crate::acquisition::execution::{
    AcquisitionExecution, ApprovePlanRequest, ExecutionItemStatus, ExecutionStatus,
};
use crate::acquisition::intelligence::SourceHealthReport;
use crate::acquisition::missing::MissingContentSummary;
use crate::acquisition::nyaa::{DEFAULT_NYAA_URL, NyaaClient, rank_candidates_smart};
use crate::acquisition::planner::{AcquisitionPlan, GlobalPlanSummary};
use crate::acquisition::profiles::{AcquisitionProfile, ProfilePatch};
use crate::acquisition::torbox::{DEFAULT_TORBOX_URL, TorBoxClient};
use crate::acquisition::{
    AcquireCollectionRequest, AcquisitionCandidate, AcquisitionConfig, AcquisitionHistoryRecord,
    AcquisitionRelease, AcquisitionReleaseUnit, CollectionInspection, CollectionUnitEntry,
};
use crate::db::Db;
use crate::error::{AppError, Result};
use reqwest::Client;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Notify;

/// Snapshot accepted identity while the queue row still exists. The existing
/// publication history is the durable boundary; later worker completion updates
/// this row instead of trying to recover provenance from a deleted download.
pub(crate) fn persist_publication_provenance(
    conn: &rusqlite::Connection,
    chapter: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO acquisition_history (chapter_id, manga_id, series, release_name, source_type, uploader,
            external_job_id, backend_type, outcome, canonical_path, release_size, completed_at, retrieval_identity_json)
         SELECT d.chapter_id, c.manga_id, m.title, COALESCE(d.raw_title,c.name), d.source_type, d.uploader,
            d.external_job_id, d.backend_type, 'published', d.published_path, d.release_size, strftime('%s','now'), d.retrieval_identity_json
         FROM downloads d JOIN chapters c ON c.id=d.chapter_id JOIN manga m ON m.id=c.manga_id
         WHERE d.chapter_id=?1 AND d.published_path IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM acquisition_history h WHERE h.chapter_id=d.chapter_id AND h.canonical_path=d.published_path
            AND h.outcome IN ('published','completed','verification_pending','published_scan_failed'))",
        [chapter],
    )?;
    Ok(())
}

pub(crate) fn complete_queued_publication(conn: &rusqlite::Connection, chapter: i64) -> Result<()> {
    persist_publication_provenance(conn, chapter)?;
    conn.execute("UPDATE acquisition_history SET outcome='completed', completed_at=strftime('%s','now')
        WHERE chapter_id=?1 AND canonical_path=(SELECT published_path FROM downloads WHERE chapter_id=?1)
        AND outcome IN ('published','verification_pending','published_scan_failed')", [chapter])?;
    conn.execute("DELETE FROM downloads WHERE chapter_id=?1", [chapter])?;
    Ok(())
}

fn preserve_queued_identity(
    conn: &rusqlite::Connection,
    chapter_id: i64,
    next_json: &str,
    torrent_hash: Option<&str>,
) -> Result<String> {
    let previous: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT retrieval_identity_json, torrent_hash FROM downloads WHERE chapter_id = ?1",
            [chapter_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let mut next: super::edition::RetrievalIdentity = serde_json::from_str(next_json)
        .map_err(|_| AppError::bad_request("Invalid retrieval identity"))?;
    if let Some((Some(json), old_hash)) = previous {
        let previous: super::edition::RetrievalIdentity =
            serde_json::from_str(&json).map_err(|_| {
                AppError::bad_request("Invalid persisted retrieval identity; replan acquisition")
            })?;
        if previous.selected_archive.is_some() && old_hash.as_deref() != torrent_hash {
            return Err(AppError::new(
                409,
                "selection_conflict",
                "Selected archive torrent identity changed; preserve the existing partial transfer",
            ));
        }
        next.preserve_selection_from(&previous)
            .map_err(|error| AppError::new(409, "selection_conflict", error))?;
    }
    serde_json::to_string(&next).map_err(|e| AppError::internal(e.to_string()))
}

pub fn default_true() -> bool {
    true
}

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
    pub requested_edition: Option<String>,
    pub selected_archive: Option<super::edition::SelectedArchive>,
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
                let has_token = token_opt
                    .as_deref()
                    .map(|t| !t.trim().is_empty())
                    .unwrap_or(false);

                let torbox_validated =
                    crate::db::state(conn, "acquisition.torbox_validated")?.map(|v| v == "valid");
                let torbox_status_message =
                    crate::db::state(conn, "acquisition.torbox_status_message")?;

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
                    crate::db::set_state(
                        conn,
                        "acquisition.nyaa_enabled",
                        &nyaa_enabled.to_string(),
                    )?;
                }
                if let Some(nyaa_url) = patch.nyaa_url {
                    crate::db::set_state(conn, "acquisition.nyaa_url", nyaa_url.trim())?;
                }
                if let Some(nyaa_cat) = patch.nyaa_category {
                    crate::db::set_state(conn, "acquisition.nyaa_category", nyaa_cat.trim())?;
                }
                if let Some(torbox_enabled) = patch.torbox_enabled {
                    crate::db::set_state(
                        conn,
                        "acquisition.torbox_enabled",
                        &torbox_enabled.to_string(),
                    )?;
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
                        crate::db::set_state(
                            conn,
                            "acquisition.torbox_status_message",
                            &msg_clone,
                        )?;
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
                        crate::db::set_state(
                            conn,
                            "acquisition.torbox_status_message",
                            &err_clone,
                        )?;
                        Ok(())
                    })
                    .await;
                Err(e.into())
            }
        }
    }

    /// Fetches title and known aliases for a given manga from DB.
    pub async fn get_title_and_aliases(&self, manga_id: i64) -> Result<(String, Vec<String>)> {
        self.db
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
            .await
    }

    /// Searches Nyaa for candidates for a given title, matching aliases and ranking candidates.
    pub async fn search_candidates(
        &self,
        manga_id: i64,
        target_unit: Option<AcquisitionReleaseUnit>,
        sort_by: Option<String>,
        filter_type: Option<String>,
        filter_ownership: Option<String>,
        filter_uploader: Option<String>,
        min_seeders: Option<u32>,
    ) -> Result<Vec<AcquisitionCandidate>> {
        let (config, _) = self.load_internal_config().await?;
        if !config.nyaa_enabled {
            return Err(AppError::bad_request(
                "Nyaa release discovery is disabled in Settings",
            ));
        }

        let (title, aliases) = self.get_title_and_aliases(manga_id).await?;

        let nyaa = NyaaClient::new(self.http.clone(), config.nyaa_url);
        let mut releases = Vec::new();
        let mut seen_keys = std::collections::HashSet::new();

        if let Some(AcquisitionReleaseUnit::Volume(v)) = &target_unit {
            let v_int = v.round() as i64;
            let queries = [
                format!("{title} v{v_int}"),
                format!("{title} vol {v_int}"),
                format!("{title} volume {v_int}"),
                title.clone(),
            ];
            for q in queries {
                if let Ok(res) = nyaa.search(&q, config.nyaa_category.as_deref()).await {
                    for r in res {
                        let key = r
                            .info_hash
                            .clone()
                            .unwrap_or_else(|| r.source_item_id.clone());
                        if seen_keys.insert(key) {
                            releases.push(r);
                        }
                    }
                }
            }
        } else {
            releases = nyaa
                .search(&title, config.nyaa_category.as_deref())
                .await
                .unwrap_or_default();
        }

        // Fetch ownership state and volumes for library-aware smart ranking
        let (owned_set, _) = self.get_title_acquisition_state(manga_id, &title).await?;

        let mut owned_volumes = Vec::new();
        let mut all_series_missing_volumes = Vec::new();

        if let Some(pipeline) = &self.pipeline {
            let title_owned = pipeline
                .ownership
                .get_title_ownership(manga_id, &title)
                .await;
            for v in &title_owned.volumes {
                if v.status == crate::pipeline::ownership::OwnershipStatus::Owned {
                    owned_volumes.push(v.volume);
                } else {
                    all_series_missing_volumes.push(v.volume);
                }
            }
        }

        if owned_volumes.is_empty() {
            for s in &owned_set {
                if s.starts_with('v') {
                    if let Ok(val) = s[1..].parse::<f64>() {
                        owned_volumes.push(val);
                    }
                }
            }
        }

        let candidates = rank_candidates_smart(
            releases,
            &title,
            &aliases,
            target_unit.as_ref(),
            &owned_volumes,
            &all_series_missing_volumes,
            sort_by.as_deref(),
            filter_type.as_deref(),
            filter_ownership.as_deref(),
            filter_uploader.as_deref(),
            min_seeders,
        );

        Ok(candidates)
    }

    /// Inspects a collection release, breaking it down into unit entries with owned/missing status.
    pub async fn inspect_collection(
        &self,
        manga_id: i64,
        release: AcquisitionRelease,
    ) -> Result<CollectionInspection> {
        let (title, _) = self.get_title_and_aliases(manga_id).await?;
        let (owned_set, acquiring_set) = self.get_title_acquisition_state(manga_id, &title).await?;

        let mut vols = release.release_unit.included_volumes();

        // Optional TorBox file mapping if torrent files are already available
        let mut file_map = std::collections::BTreeMap::new();
        if let Some(ref hash) = release.info_hash {
            if let Ok(Some(token)) = self.get_torbox_token().await {
                let (config, _) = self.load_internal_config().await.unwrap_or_default();
                let client = TorBoxClient::new(self.http.clone(), config.torbox_url, Some(token));
                if let Ok(files) = client.list_files(hash).await {
                    if let Ok(mapped) =
                        crate::acquisition::torbox::map_torrent_files_to_volumes(&files)
                    {
                        file_map = mapped;
                    }
                }
            }
        }

        if vols.is_empty() && !file_map.is_empty() {
            vols = file_map.keys().map(|&k| k as f64).collect();
        }

        let mut units = Vec::new();
        for &v in &vols {
            let v_idx = v.round() as i64;
            let is_owned =
                owned_set.contains(&format!("v{v}")) || owned_set.contains(&format!("v{v_idx}"));
            let is_acquiring = acquiring_set.contains(&format!("v{v}"))
                || acquiring_set.contains(&format!("v{v_idx}"));

            let (file_id, filename, file_size) = if let Some(bf) = file_map.get(&v_idx) {
                (Some(bf.id), Some(bf.name.clone()), Some(bf.size))
            } else {
                (None, Some(format!("Volume {:02}", v_idx)), None)
            };

            units.push(CollectionUnitEntry {
                volume: v,
                file_id,
                filename,
                file_size,
                is_owned,
                is_acquiring,
            });
        }

        let total_units = units.len();
        let owned_count = units.iter().filter(|u| u.is_owned).count();
        let missing_count = total_units.saturating_sub(owned_count);
        let recommended_mode = if owned_count > 0 && missing_count > 0 {
            "missing_only".to_string()
        } else {
            "entire".to_string()
        };

        Ok(CollectionInspection {
            release,
            units,
            total_units,
            owned_count,
            missing_count,
            recommended_mode,
        })
    }

    /// Enqueues selected volumes of a collection release as independent child jobs in downloads queue.
    pub async fn acquire_collection(
        &self,
        manga_id: i64,
        req: AcquireCollectionRequest,
    ) -> Result<Vec<i64>> {
        let (config, has_token) = self.load_internal_config().await?;
        if !config.torbox_enabled {
            return Err(AppError::bad_request(
                "TorBox download backend is disabled in Settings",
            ));
        }
        if !has_token {
            return Err(AppError::bad_request(
                "TorBox API token is not configured. Add it in Settings.",
            ));
        }
        let actual_token = self.get_torbox_token().await?.unwrap_or_default();

        let title: String = self
            .db
            .call(move |conn| {
                Ok(
                    conn.query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await?;

        // Filter out volumes already owned or acquiring
        let (owned_set, acquiring_set) = self.get_title_acquisition_state(manga_id, &title).await?;

        let mut volumes_to_acquire = Vec::new();
        for &v in &req.selected_volumes {
            let v_idx = v.round() as i64;
            let is_owned =
                owned_set.contains(&format!("v{v}")) || owned_set.contains(&format!("v{v_idx}"));
            let is_acquiring = acquiring_set.contains(&format!("v{v}"))
                || acquiring_set.contains(&format!("v{v_idx}"));

            if req.mode == "missing_only" && is_owned {
                continue;
            }
            if is_acquiring {
                continue;
            }
            volumes_to_acquire.push(v);
        }

        if volumes_to_acquire.is_empty() {
            return Err(AppError::bad_request(
                "All selected volumes are already owned or currently acquiring",
            ));
        }

        crate::acquisition::torbox::validate_edition_intent(
            &super::edition::RetrievalIdentity::new(
                &title,
                &req.release.raw_title,
                req.release.release_unit.clone(),
                req.requested_edition.as_deref(),
            ),
        )?;

        // Submit collection torrent to TorBox
        let client = TorBoxClient::new(self.http.clone(), config.torbox_url, Some(actual_token));
        let job_info = client.submit(&req.release).await?;
        let external_job_id = job_info.external_job_id.clone();
        let torrent_hash = job_info
            .torrent_hash
            .clone()
            .or_else(|| req.release.info_hash.clone());
        let collection_id = format!("col-{}", external_job_id);
        let collection_name = req.release.title.clone();

        // Attempt file mapping if files are listed
        let mut file_map = std::collections::BTreeMap::new();
        if let Ok(files) = client.list_files(&external_job_id).await {
            if let Ok(mapped) = crate::acquisition::torbox::map_torrent_files_to_volumes(&files) {
                file_map = mapped;
            }
        }

        let uploader = req.release.uploader.clone();
        let total_size = req.release.size_bytes;
        let per_vol_size = if !volumes_to_acquire.is_empty() {
            total_size / volumes_to_acquire.len() as u64
        } else {
            0
        };

        let mut chapter_ids = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        for v in volumes_to_acquire {
            let v_idx = v.round() as i64;
            let (ext_file_id, raw_name, f_size) = if let Some(bf) = file_map.get(&v_idx) {
                (Some(bf.id), bf.name.clone(), bf.size)
            } else {
                (
                    None,
                    format!("{} v{:02}", req.release.title, v_idx),
                    per_vol_size,
                )
            };

            let identity_json = serde_json::to_string(&super::edition::RetrievalIdentity::new(
                &title,
                &req.release.raw_title,
                AcquisitionReleaseUnit::Volume(v),
                req.requested_edition.as_deref(),
            ))
            .map_err(|e| AppError::internal(e.to_string()))?;
            let ch_url = format!("nyaa:{}:{v_idx}", req.release.source_item_id);
            let ch_name = format!("Volume {v_idx}");
            let up_clone = uploader.clone();
            let ext_jid = external_job_id.clone();
            let col_id = collection_id.clone();
            let col_name = collection_name.clone();
            let thash = torrent_hash.clone();

            let ch_id: i64 = self.db.call(move |conn| {
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
                        rusqlite::params![manga_id, &ch_url, &ch_name, v, &up_clone, now],
                    )?;
                    conn.last_insert_rowid()
                };

                let identity_json = preserve_queued_identity(conn, id, &identity_json, thash.as_deref())?;
                conn.execute(
                    "INSERT INTO downloads (
                        chapter_id, state, position, attempts, added_at, source_type, backend_type,
                        external_job_id, external_file_id, raw_title, uploader, release_size, torrent_hash,
                        collection_id, collection_name, retrieval_identity_json
                    ) VALUES (
                        ?1, 'queued', (SELECT COALESCE(MAX(position), 0) + 1 FROM downloads), 0, ?2,
                        'nyaa', 'torbox', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
                    ) ON CONFLICT(chapter_id) DO UPDATE SET
                        state = 'queued', attempts = 0, retry_at = NULL, error = NULL,
                        source_type = 'nyaa', backend_type = 'torbox',
                        external_job_id = excluded.external_job_id,
                        external_file_id = excluded.external_file_id,
                        raw_title = excluded.raw_title, uploader = excluded.uploader,
                        release_size = excluded.release_size, torrent_hash = excluded.torrent_hash,
                        collection_id = excluded.collection_id, collection_name = excluded.collection_name,
                        retrieval_identity_json = excluded.retrieval_identity_json",
                    rusqlite::params![
                        id, now, &ext_jid, ext_file_id, &raw_name, &up_clone, f_size as i64, &thash,
                        &col_id, &col_name, &identity_json
                    ],
                )?;

                Ok(id)
            }).await?;

            chapter_ids.push(ch_id);
        }

        if let Some(wake) = &self.wake_downloads {
            wake.notify_one();
        }

        Ok(chapter_ids)
    }

    /// Helper to get sets of owned and acquiring unit identifiers (e.g. "v1", "c12").
    async fn get_title_acquisition_state(
        &self,
        manga_id: i64,
        title: &str,
    ) -> Result<(
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    )> {
        let title_owned = if let Some(pipeline) = &self.pipeline {
            Some(
                pipeline
                    .ownership
                    .get_title_ownership(manga_id, title)
                    .await,
            )
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

        // Query downloaded chapters from DB (useful when pipeline is offline or in tests)
        let downloaded_chapters: Vec<(String, f64)> = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT name, chapter_number FROM chapters WHERE manga_id = ?1 AND download_path IS NOT NULL",
                )?;
                let rows = stmt
                    .query_map([manga_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .filter_map(std::result::Result::ok)
                    .collect();
                Ok(rows)
            })
            .await?;

        for (name, num) in downloaded_chapters {
            let rel = crate::release::parse_release(&name, num);
            if rel.release_type == crate::release::ReleaseType::Volume {
                owned_set.insert(format!("v{}", rel.volume.unwrap_or(num)));
            } else {
                owned_set.insert(format!("c{}", rel.chapter_number.unwrap_or(num)));
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
            let rel = crate::release::parse_release(&name, num);
            let key = if rel.release_type == crate::release::ReleaseType::Volume {
                format!("v{}", rel.volume.unwrap_or(num))
            } else {
                format!("c{}", rel.chapter_number.unwrap_or(num))
            };

            if state == "completed" {
                owned_set.insert(key);
            } else if state != "failed" {
                acquiring_set.insert(key);
            }
        }

        Ok((owned_set, acquiring_set))
    }

    /// Enqueues an external release for acquisition via TorBox.
    ///
    /// Performs strict PRE-SUBMISSION duplicate checks against canonical library and download queue (Requirement 11, Scenario W).
    #[cfg(test)]
    pub async fn acquire_release(
        &self,
        manga_id: i64,
        release: AcquisitionRelease,
        target_unit: Option<AcquisitionReleaseUnit>,
    ) -> Result<i64> {
        self.acquire_release_with_identity(manga_id, release, target_unit, None, None)
            .await
    }

    pub async fn acquire_release_with_identity(
        &self,
        manga_id: i64,
        release: AcquisitionRelease,
        target_unit: Option<AcquisitionReleaseUnit>,
        requested_edition: Option<String>,
        selected_archive: Option<super::edition::SelectedArchive>,
    ) -> Result<i64> {
        let (config, has_token) = self.load_internal_config().await?;
        if !config.torbox_enabled {
            return Err(AppError::bad_request(
                "TorBox download backend is disabled in Settings",
            ));
        }
        if !has_token {
            return Err(AppError::bad_request(
                "TorBox API token is not configured. Add it in Settings.",
            ));
        }

        let title: String = self
            .db
            .call(move |conn| {
                Ok(
                    conn.query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |r| {
                        r.get(0)
                    })?,
                )
            })
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
        let release_unit_check = target_unit
            .clone()
            .unwrap_or_else(|| release.release_unit.clone());

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
        if !matches!(&chapter_unit, AcquisitionReleaseUnit::Volume(v) | AcquisitionReleaseUnit::Chapter(v) if v.is_finite() && *v >= 0.0)
        {
            return Err(AppError::bad_request("Select an explicit edition-local volume or chapter; collection/unknown release numbering cannot identify one acquisition unit"));
        }
        let mut identity = super::edition::RetrievalIdentity::new(
            &title,
            &release.raw_title,
            chapter_unit.clone(),
            requested_edition.as_deref(),
        );
        identity.selected_archive = selected_archive;
        crate::acquisition::torbox::validate_edition_intent(&identity)?;
        let identity_json =
            serde_json::to_string(&identity).map_err(|e| AppError::internal(e.to_string()))?;
        let (ch_name, ch_number) = match &chapter_unit {
            AcquisitionReleaseUnit::Volume(v) => (format!("Volume {v}"), *v),
            AcquisitionReleaseUnit::Chapter(c) => (format!("Chapter {c}"), *c),
            AcquisitionReleaseUnit::Collection { start, end } => {
                (format!("Volumes {start}-{end}"), *start)
            }
            AcquisitionReleaseUnit::MultiRange(ranges) => {
                let s = ranges
                    .iter()
                    .map(|(s, e)| format!("{s}-{e}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!("Volumes {s}"),
                    ranges.first().map(|(s, _)| *s).unwrap_or(1.0),
                )
            }
            AcquisitionReleaseUnit::MixedCollection {
                volume_start,
                volume_end,
                ..
            } => (
                format!("Volumes {volume_start}-{volume_end}"),
                *volume_start,
            ),
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
                let identity_json = preserve_queued_identity(conn, id, &identity_json, torrent_hash.as_deref())?;
                conn.execute(
                    "INSERT INTO downloads (
                        chapter_id, state, position, attempts, added_at, source_type, backend_type,
                        raw_title, uploader, release_size, torrent_hash, retrieval_identity_json
                    ) VALUES (
                        ?1, 'queued', (SELECT COALESCE(MAX(position), 0) + 1 FROM downloads), 0, ?2,
                        'nyaa', 'torbox', ?3, ?4, ?5, ?6, ?7
                    ) ON CONFLICT(chapter_id) DO UPDATE SET
                        state = 'queued', attempts = 0, retry_at = NULL, error = NULL,
                        source_type = 'nyaa', backend_type = 'torbox',
                        raw_title = excluded.raw_title, uploader = excluded.uploader,
                        release_size = excluded.release_size, torrent_hash = excluded.torrent_hash,
                        retrieval_identity_json = excluded.retrieval_identity_json",
                    rusqlite::params![id, now, &raw_title, &uploader, release_size as i64, &torrent_hash, &identity_json],
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

    fn canonical_file_exists_for_unit(
        &self,
        series_dir: &Path,
        unit: &AcquisitionReleaseUnit,
    ) -> bool {
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
            match (unit, &file_unit) {
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
                (AcquisitionReleaseUnit::Volume(target_v), rel_u) if rel_u.is_collection() => {
                    if rel_u
                        .included_volumes()
                        .iter()
                        .any(|v| (v - target_v).abs() < 0.001)
                    {
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
        self.record_history_with_provenance(
            chapter_id,
            manga_id,
            series,
            release_name,
            source_type,
            uploader,
            external_job_id,
            backend_type,
            outcome,
            canonical_path,
            release_size,
            error,
            None,
            None,
            None,
            None,
            None,
        )
        .await
    }

    pub async fn record_history_with_provenance(
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
        mangabaka_id: Option<i64>,
        mangabaka_release_id: Option<&str>,
        anilist_id: Option<i64>,
        requested_unit: Option<&str>,
        metadata_status: Option<&str>,
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
        let mangabaka_release_id = mangabaka_release_id.map(|s| s.to_string());
        let requested_unit = requested_unit.map(|s| s.to_string());
        let metadata_status = metadata_status.map(|s| s.to_string());

        self.db
            .call(move |conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                // Publication may already have removed the operational queue row.
                // Preserve its captured JSON/provider provenance while settling history.
                let updated = conn.execute(
                    "UPDATE acquisition_history SET outcome=?1, error=?2, completed_at=?3,
                        mangabaka_id=COALESCE(?4,mangabaka_id), mangabaka_release_id=COALESCE(?5,mangabaka_release_id),
                        anilist_id=COALESCE(?6,anilist_id), requested_unit=COALESCE(?7,requested_unit), metadata_status=COALESCE(?8,metadata_status)
                     WHERE chapter_id=?9 AND canonical_path IS ?10 AND outcome IN ('published','completed','verification_pending','published_scan_failed')",
                    rusqlite::params![&outcome,error.as_deref(),now,mangabaka_id,mangabaka_release_id.as_deref(),anilist_id,requested_unit.as_deref(),metadata_status.as_deref(),chapter_id,canonical_path.as_deref()],
                )?;
                if updated > 0 { return Ok(()); }
                conn.execute(
                    "INSERT INTO acquisition_history (
                        chapter_id, manga_id, series, release_name, source_type, uploader,
                        external_job_id, backend_type, outcome, canonical_path, release_size,
                        completed_at, error, mangabaka_id, mangabaka_release_id, anilist_id,
                        requested_unit, metadata_status, retrieval_identity_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, (SELECT retrieval_identity_json FROM downloads WHERE chapter_id = ?1))",
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
                        mangabaka_id,
                        mangabaka_release_id.as_deref(),
                        anilist_id,
                        requested_unit.as_deref(),
                        metadata_status.as_deref(),
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
                                      completed_at, error, mangabaka_id, mangabaka_release_id, anilist_id,
                                      requested_unit, metadata_status
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
                            mangabaka_id: r.get(14)?,
                            mangabaka_release_id: r.get(15)?,
                            anilist_id: r.get(16)?,
                            requested_unit: r.get(17)?,
                            metadata_status: r.get(18)?,
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
                            mangabaka_id: r.get(14)?,
                            mangabaka_release_id: r.get(15)?,
                            anilist_id: r.get(16)?,
                            requested_unit: r.get(17)?,
                            metadata_status: r.get(18)?,
                        })
                    })?
                    .filter_map(std::result::Result::ok)
                    .collect()
                };

                Ok(rows)
            })
            .await
    }

    // Profile Management
    pub async fn list_profiles(&self) -> Result<Vec<AcquisitionProfile>> {
        self.db
            .call(|conn| crate::acquisition::profiles::list_profiles(conn))
            .await
    }

    pub async fn get_profile(&self, id: String) -> Result<Option<AcquisitionProfile>> {
        self.db
            .call(move |conn| crate::acquisition::profiles::get_profile_by_id(conn, &id))
            .await
    }

    pub async fn create_profile(&self, profile: AcquisitionProfile) -> Result<AcquisitionProfile> {
        self.db
            .call(move |conn| crate::acquisition::profiles::create_profile(conn, profile))
            .await
    }

    pub async fn update_profile(
        &self,
        id: String,
        patch: ProfilePatch,
    ) -> Result<AcquisitionProfile> {
        self.db
            .call(move |conn| crate::acquisition::profiles::update_profile(conn, &id, patch))
            .await
    }

    pub async fn delete_profile(&self, id: String) -> Result<()> {
        self.db
            .call(move |conn| crate::acquisition::profiles::delete_profile(conn, &id))
            .await
    }

    pub async fn duplicate_profile(
        &self,
        id: String,
        new_name: Option<String>,
    ) -> Result<AcquisitionProfile> {
        self.db
            .call(move |conn| crate::acquisition::profiles::duplicate_profile(conn, &id, new_name))
            .await
    }

    pub async fn get_manga_profile(&self, manga_id: i64) -> Result<AcquisitionProfile> {
        self.db
            .call(move |conn| crate::acquisition::profiles::get_profile_for_manga(conn, manga_id))
            .await
    }

    pub async fn set_manga_profile(&self, manga_id: i64, profile_id: Option<String>) -> Result<()> {
        self.db
            .call(move |conn| {
                crate::acquisition::profiles::set_manga_profile(
                    conn,
                    manga_id,
                    profile_id.as_deref(),
                )
            })
            .await
    }

    // Missing Content Engine
    pub async fn get_title_missing_content(&self, manga_id: i64) -> Result<MissingContentSummary> {
        let (title, _) = self.get_title_and_aliases(manga_id).await?;
        let ownership_opt = if let Some(pipeline) = &self.pipeline {
            Some(
                pipeline
                    .ownership
                    .get_title_ownership(manga_id, &title)
                    .await,
            )
        } else {
            None
        };
        self.db
            .call(move |conn| {
                Ok(
                    crate::acquisition::missing::resolve_missing_content_for_title(
                        conn,
                        ownership_opt.as_ref(),
                        manga_id,
                        &title,
                    ),
                )
            })
            .await
    }

    // Acquisition Planner
    pub async fn plan_title_missing_content(
        &self,
        manga_id: i64,
        dry_run: bool,
    ) -> Result<AcquisitionPlan> {
        let missing_summary = self.get_title_missing_content(manga_id).await?;
        let profile = self.get_manga_profile(manga_id).await?;

        if missing_summary.missing_volumes.is_empty() {
            return Ok(crate::acquisition::planner::generate_plan(
                manga_id,
                &missing_summary.title,
                &profile,
                &missing_summary,
                &[],
                &std::collections::HashMap::new(),
                dry_run,
            ));
        }

        // Query candidates from discovery sources
        let (config, _) = self.load_internal_config().await?;
        let mut normalized_candidates = Vec::new();

        if config.nyaa_enabled {
            let start = std::time::Instant::now();
            match self
                .search_candidates(manga_id, None, None, None, None, None, None)
                .await
            {
                Ok(cands) => {
                    let dur_ms = start.elapsed().as_millis() as u64;
                    let count = cands.len();
                    let exact = cands
                        .iter()
                        .filter(|c| c.smart_score.as_ref().map_or(false, |s| s.identity >= 20.0))
                        .count();
                    let _ = self
                        .db
                        .call(move |conn| {
                            crate::acquisition::intelligence::record_discovery_success(
                                conn, "nyaa", count, exact, dur_ms,
                            )
                        })
                        .await;

                    for c in cands {
                        normalized_candidates.push(
                            NormalizedAcquisitionCandidate::from_nyaa_candidate(
                                &missing_summary.title,
                                &c,
                                &profile.preferred_uploaders,
                                &profile.language,
                            ),
                        );
                    }
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let _ = self
                        .db
                        .call(move |conn| {
                            crate::acquisition::intelligence::record_discovery_failure(
                                conn, "nyaa", &err_str,
                            )
                        })
                        .await;
                }
            }
        }

        // Check if there are local reader/MangaDot volume-labelled releases
        let title_for_db = missing_summary.title.clone();
        let lang_for_db = profile.language.clone();
        let md_cands = self.db.call(move |conn| {
            let mut list = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT name FROM chapters WHERE manga_id = ?1") {
                if let Ok(rows) = stmt.query_map([manga_id], |r| r.get::<_, String>(0)) {
                    for name in rows.flatten() {
                        if let Some(v) = crate::release::parse_volume(&name) {
                            list.push(NormalizedAcquisitionCandidate::from_mangadot_release(
                                &title_for_db,
                                v,
                                &name,
                                None,
                                &lang_for_db,
                                crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
                            ));
                        }
                    }
                }
            }
            Ok(list)
        }).await.unwrap_or_default();
        normalized_candidates.extend(md_cands);

        // Fetch source health map
        let health_reports = self.get_source_health_reports().await.unwrap_or_default();
        let mut source_health_map = std::collections::HashMap::new();
        for h in health_reports {
            source_health_map.insert(h.source_id, h.status);
        }

        let plan = crate::acquisition::planner::generate_plan(
            manga_id,
            &missing_summary.title,
            &profile,
            &missing_summary,
            &normalized_candidates,
            &source_health_map,
            dry_run,
        );

        if !dry_run {
            let plan_to_save = plan.clone();
            self.db
                .call(move |conn| crate::acquisition::planner::save_plan(conn, &plan_to_save))
                .await?;
        }

        Ok(plan)
    }

    pub async fn get_title_plan(&self, manga_id: i64) -> Result<Option<AcquisitionPlan>> {
        let (title, _) = self.get_title_and_aliases(manga_id).await?;
        let mut current_owned_volumes = Vec::new();
        if let Some(pipeline) = &self.pipeline {
            let own = pipeline
                .ownership
                .get_title_ownership(manga_id, &title)
                .await;
            for v in own.volumes {
                if v.status == crate::pipeline::ownership::OwnershipStatus::Owned {
                    current_owned_volumes.push(v.volume);
                }
            }
        }
        self.db
            .call(move |conn| {
                crate::acquisition::planner::get_plan_for_manga(
                    conn,
                    manga_id,
                    &current_owned_volumes,
                )
            })
            .await
    }

    pub async fn get_global_missing_content(&self) -> Result<Vec<MissingContentSummary>> {
        let library_titles = self
            .db
            .call(|conn| {
                let mut stmt =
                    conn.prepare("SELECT id FROM manga WHERE in_library = 1 ORDER BY title ASC")?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .filter_map(std::result::Result::ok)
                    .collect::<Vec<_>>();
                Ok(ids)
            })
            .await?;

        let mut summaries = Vec::new();
        for id in library_titles {
            if let Ok(summary) = self.get_title_missing_content(id).await {
                summaries.push(summary);
            }
        }
        Ok(summaries)
    }

    pub async fn plan_all_missing_content(&self) -> Result<GlobalPlanSummary> {
        let library_titles = self
            .db
            .call(|conn| {
                let mut stmt =
                    conn.prepare("SELECT id FROM manga WHERE in_library = 1 ORDER BY title ASC")?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .filter_map(std::result::Result::ok)
                    .collect::<Vec<_>>();
                Ok(ids)
            })
            .await?;

        let mut planned = Vec::new();
        let mut total_vols = 0;
        let mut total_colls = 0;
        let mut total_indiv = 0;
        let mut total_bytes = 0;
        let mut unavail = 0;
        let mut needs_rev = 0;
        let mut satisfied = 0;

        for id in &library_titles {
            if let Ok(plan) = self.plan_title_missing_content(*id, true).await {
                if plan.state == "completed" {
                    satisfied += 1;
                } else {
                    if plan.confidence != "high" {
                        needs_rev += 1;
                    }
                    if plan.selected_items.is_empty() {
                        unavail += plan.missing_units.len();
                    } else {
                        for item in &plan.selected_items {
                            if item.candidate.release_unit.is_collection() {
                                total_colls += 1;
                            } else {
                                total_indiv += 1;
                            }
                            total_vols += item.units_covered.len();
                            total_bytes += item.candidate.size.unwrap_or(0);
                        }
                    }
                    planned.push(plan);
                }
            }
        }

        Ok(GlobalPlanSummary {
            total_series_analyzed: library_titles.len(),
            series_planned: planned,
            total_volumes_to_acquire: total_vols,
            total_collections_count: total_colls,
            total_individual_count: total_indiv,
            estimated_total_bytes: total_bytes,
            unavailable_volumes_count: unavail,
            needs_review_count: needs_rev,
            already_satisfied_count: satisfied,
        })
    }

    pub async fn get_source_health_reports(&self) -> Result<Vec<SourceHealthReport>> {
        self.db
            .call(|conn| crate::acquisition::intelligence::get_all_source_health(conn))
            .await
    }

    /// Explicit user approval and creation of an immutable execution snapshot.
    pub async fn approve_and_create_execution(
        &self,
        manga_id: i64,
        req: ApprovePlanRequest,
    ) -> Result<AcquisitionExecution> {
        if !req.approved {
            return Err(AppError::bad_request(
                "Explicit approval is required to create an acquisition execution",
            ));
        }

        let pid = req.plan_id.clone();
        let plan_opt = self.db.call(move |conn| {
            let row = conn.query_row(
                "SELECT id, manga_id, profile_id, missing_units_json, selected_candidates_json, alternative_candidates_json, explanations_json, confidence, state, stale_reason, created_at, refreshed_at
                 FROM acquisition_plans WHERE id = ?1",
                [&pid],
                |r| {
                    let id: String = r.get(0)?;
                    let manga_id: i64 = r.get(1)?;
                    let profile_id: String = r.get(2)?;
                    let missing_units_json: String = r.get(3)?;
                    let selected_candidates_json: String = r.get(4)?;
                    let alternative_candidates_json: String = r.get(5)?;
                    let _explanations_json: String = r.get(6)?;
                    let confidence: String = r.get(7)?;
                    let state: String = r.get(8)?;
                    let stale_reason: Option<String> = r.get(9)?;
                    let created_at: i64 = r.get(10)?;
                    let refreshed_at: i64 = r.get(11)?;

                    let missing_units: Vec<f64> = serde_json::from_str(&missing_units_json).unwrap_or_default();
                    let selected_items: Vec<crate::acquisition::planner::AcquisitionPlanItem> =
                        serde_json::from_str(&selected_candidates_json).unwrap_or_default();
                    let alternatives: Vec<crate::acquisition::candidates::NormalizedAcquisitionCandidate> =
                        serde_json::from_str(&alternative_candidates_json).unwrap_or_default();

                    let total_estimated_size = selected_items
                        .iter()
                        .map(|item| item.candidate.size.unwrap_or(0))
                        .sum();
                    let total_acquisitions_count = selected_items.len();
                    let missing_units_covered_count = selected_items
                        .iter()
                        .flat_map(|item| &item.units_covered)
                        .map(|&u| (u * 100.0).round() as i64)
                        .collect::<std::collections::HashSet<_>>()
                        .len();

                    let series_title: String = conn
                        .query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |row| row.get(0))
                        .unwrap_or_default();
                    let profile_name: String = conn
                        .query_row("SELECT name FROM acquisition_profiles WHERE id = ?1", [&profile_id], |row| row.get(0))
                        .unwrap_or_default();

                    Ok(AcquisitionPlan {
                        id,
                        manga_id,
                        series_title,
                        profile_id,
                        profile_name,
                        missing_units,
                        selected_items,
                        alternatives,
                        confidence,
                        state,
                        stale_reason,
                        total_estimated_size,
                        total_acquisitions_count,
                        missing_units_covered_count,
                        is_dry_run: false,
                        created_at,
                        refreshed_at,
                    })
                },
            ).optional()?;
            Ok(row)
        }).await?;

        let plan = plan_opt.ok_or_else(|| AppError::not_found("Acquisition plan not found"))?;

        if plan
            .selected_items
            .iter()
            .any(|item| super::interactive::is_override(&item.candidate))
            && !req.override_confirmed
        {
            return Err(AppError::bad_request(
                "Use Release Anyway requires explicit confirmation before approval",
            ));
        }
        if plan.selected_items.is_empty() {
            return Err(AppError::bad_request("No release has been selected"));
        }
        if plan.manga_id != manga_id {
            return Err(AppError::bad_request(
                "Plan does not belong to the specified title",
            ));
        }

        // Check plan staleness at approval time
        let current_missing = self.get_title_missing_content(manga_id).await?;
        let mut plan_missing_sorted = plan.missing_units.clone();
        plan_missing_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let mut live_missing_sorted = current_missing.missing_volumes.clone();
        live_missing_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let interactive = plan
            .selected_items
            .iter()
            .all(|item| item.candidate.provider_metadata["interactive"] == true);
        if !interactive && plan_missing_sorted != live_missing_sorted {
            return Err(AppError::bad_request(
                "Plan is stale: library missing units have changed since plan generation. Please regenerate and review.",
            ));
        }

        let profile = self.get_manga_profile(manga_id).await?;
        if plan.profile_id != profile.id {
            return Err(AppError::bad_request(
                "Plan is stale: title acquisition profile has changed since plan generation. Please regenerate and review.",
            ));
        }

        let execution = self
            .db
            .call(move |conn| {
                crate::acquisition::execution::create_execution(conn, &plan, &current_missing)
            })
            .await?;

        Ok(execution)
    }

    /// Executes an approved plan through preflight, revalidation, and queueing.
    pub async fn start_execution(&self, execution_id: String) -> Result<AcquisitionExecution> {
        let eid = execution_id.clone();
        let exec_opt = self.get_execution(&execution_id).await?;
        let mut exec = exec_opt.ok_or_else(|| AppError::not_found("Execution not found"))?;

        // Idempotency: if already queued or beyond, return current state
        if matches!(
            exec.status,
            ExecutionStatus::Queued
                | ExecutionStatus::Downloading
                | ExecutionStatus::Verifying
                | ExecutionStatus::Publishing
                | ExecutionStatus::Reconciling
                | ExecutionStatus::Completed
        ) {
            return Ok(exec);
        }

        if !matches!(
            exec.status,
            ExecutionStatus::Approved
                | ExecutionStatus::Preflight
                | ExecutionStatus::Partial
                | ExecutionStatus::Failed
        ) {
            return Err(AppError::bad_request(format!(
                "Execution in status {} cannot be started",
                exec.status.as_str()
            )));
        }

        // 1. PREFLIGHT
        self.db
            .call({
                let eid = eid.clone();
                move |conn| {
                    crate::acquisition::execution::update_execution_status(
                        conn,
                        &eid,
                        ExecutionStatus::Preflight,
                        None,
                        None,
                        Some("Running preflight checks"),
                    )
                }
            })
            .await?;
        exec.status = ExecutionStatus::Preflight;

        // A. Profile check
        let profile = self.get_manga_profile(exec.manga_id).await?;
        for item in &exec.items {
            if let Err(e) = crate::acquisition::execution::validate_preflight_profile(
                &profile,
                &item.frozen_candidate,
            ) {
                let err_msg = format!("Preflight profile check failed: {e}");
                self.fail_execution(&eid, &err_msg).await?;
                return Err(AppError::bad_request(err_msg));
            }
        }

        // B. Destination check
        let (lib_dir, zh_lib_dir) = self
            .db
            .call(|conn| {
                let l = crate::settings::get::<String>(conn, "pipeline.library_dir");
                let zh = crate::settings::get::<String>(conn, "pipeline.chinese_library_dir");
                Ok((l, zh))
            })
            .await?;

        for item in &exec.items {
            if let Err(e) = crate::acquisition::execution::validate_preflight_destination(
                &item.frozen_candidate.language,
                lib_dir.as_deref(),
                zh_lib_dir.as_deref(),
            ) {
                let err_msg = format!("Preflight destination check failed: {e}");
                self.fail_execution(&eid, &err_msg).await?;
                return Err(AppError::bad_request(err_msg));
            }
        }

        // C. Disk Space check
        let target_dest = if exec
            .items
            .iter()
            .any(|i| i.frozen_candidate.language.contains("zh"))
        {
            zh_lib_dir.as_deref().unwrap_or("/srv/media/manga-chinese")
        } else {
            lib_dir.as_deref().unwrap_or("/srv/media/manga")
        };
        let dest_path = Path::new(target_dest);
        let temp_path = Path::new("/tmp");

        if let Err(e) = crate::acquisition::execution::validate_preflight_disk_space(
            exec.expected_total_bytes,
            dest_path,
            temp_path,
        ) {
            let err_msg = format!("Preflight disk check failed: {e}");
            self.fail_execution(&eid, &err_msg).await?;
            return Err(AppError::bad_request(err_msg));
        }

        // D. Existing Ownership reconciliation
        let (owned_set, _) = self
            .get_title_acquisition_state(exec.manga_id, &exec.title)
            .await?;
        let owned_vols: Vec<f64> = owned_set
            .iter()
            .filter_map(|s| s.strip_prefix('v').and_then(|v| v.parse::<f64>().ok()))
            .collect();

        let mut all_skipped = true;
        for item in &mut exec.items {
            let (still_needed, _) = crate::acquisition::execution::reconcile_preflight_ownership(
                &item.expected_units,
                if item.frozen_candidate.provider_metadata["requestedEdition"]
                    .as_str()
                    .is_some_and(|e| e != "standard")
                {
                    &[]
                } else {
                    &owned_vols
                },
            );
            item.expected_units = still_needed;
            if item.expected_units.is_empty() {
                item.status = ExecutionItemStatus::Skipped;
                self.db
                    .call({
                        let iid = item.id.clone();
                        move |conn| {
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &iid,
                                ExecutionItemStatus::Skipped,
                                None,
                                Some("All expected units are already owned"),
                            )
                        }
                    })
                    .await?;
            } else {
                all_skipped = false;
            }
        }

        if all_skipped {
            self.db
                .call({
                    let eid = eid.clone();
                    move |conn| {
                        crate::acquisition::execution::update_execution_status(
                            conn,
                            &eid,
                            ExecutionStatus::Completed,
                            None,
                            None,
                            Some("All units already owned; nothing to download"),
                        )
                    }
                })
                .await?;
            exec.status = ExecutionStatus::Completed;
            return Ok(exec);
        }

        // 2. CANDIDATE REVALIDATION
        self.db
            .call({
                let eid = eid.clone();
                move |conn| {
                    crate::acquisition::execution::update_execution_status(
                        conn,
                        &eid,
                        ExecutionStatus::Revalidating,
                        None,
                        None,
                        Some("Revalidating candidates against provider"),
                    )
                }
            })
            .await?;
        exec.status = ExecutionStatus::Revalidating;

        let needs_torbox = exec
            .items
            .iter()
            .any(|i| i.frozen_candidate.provider == "nyaa");
        if needs_torbox {
            let (config, has_token) = self.load_internal_config().await?;
            if !config.torbox_enabled {
                let err_msg = "TorBox backend is disabled in settings".to_string();
                self.fail_execution(&eid, &err_msg).await?;
                return Err(AppError::bad_request(err_msg));
            }
            if !has_token {
                let err_msg = "TorBox token is not configured".to_string();
                self.fail_execution(&eid, &err_msg).await?;
                return Err(AppError::bad_request(err_msg));
            }
        }

        for item in &exec.items {
            if item.status == ExecutionItemStatus::Skipped {
                continue;
            }

            let live_match = Some(&item.frozen_candidate);
            if let Err(e) = crate::acquisition::execution::revalidate_candidate(
                &item.frozen_candidate,
                live_match,
                &profile,
            ) {
                let err_msg = format!("Candidate revalidation failed: {e}");
                self.fail_execution(&eid, &err_msg).await?;
                return Err(AppError::bad_request(err_msg));
            }
        }

        // 3. QUEUED
        self.db
            .call({
                let eid = eid.clone();
                move |conn| {
                    crate::acquisition::execution::update_execution_status(
                        conn,
                        &eid,
                        ExecutionStatus::Queued,
                        None,
                        None,
                        Some("Queueing acquisition jobs"),
                    )
                }
            })
            .await?;
        exec.status = ExecutionStatus::Queued;

        for item in &mut exec.items {
            if item.status == ExecutionItemStatus::Skipped {
                continue;
            }

            if let Some(existing_job_id) = item.download_job_id {
                let exists: bool = self
                    .db
                    .call(move |conn| {
                        let count: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM downloads WHERE chapter_id = ?1",
                            [existing_job_id],
                            |r| r.get(0),
                        )?;
                        Ok(count > 0)
                    })
                    .await?;
                if exists {
                    continue;
                }
            }

            let cand = &item.frozen_candidate;
            let manga_id = exec.manga_id;
            let item_id = item.id.clone();
            let exec_id = eid.clone();
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let ch_url = format!(
                "{}:{}:{}",
                cand.provider,
                cand.provider_release_id,
                item.expected_units.first().copied().unwrap_or(1.0)
            );
            let ch_name = format!(
                "Volume {}",
                item.expected_units.first().copied().unwrap_or(1.0)
            );
            let scanlator = cand.uploader.clone().unwrap_or_default();
            let release_size = cand.size.unwrap_or(0) as i64;
            let first_vol = item.expected_units.first().copied().unwrap_or(1.0);
            let src_type = if cand.provider == "nyaa" {
                "nyaa"
            } else {
                "reader"
            };
            let bk_type = if cand.provider == "nyaa" {
                "torbox"
            } else {
                "internal"
            };
            let raw_t = cand
                .provider_metadata
                .get("rawTitle")
                .and_then(|v| v.as_str())
                .unwrap_or(&cand.title)
                .to_string();
            let mut retrieval_identity = super::edition::RetrievalIdentity::new(
                &exec.title,
                &raw_t,
                AcquisitionReleaseUnit::Volume(first_vol),
                cand.provider_metadata
                    .get("requestedEdition")
                    .and_then(|v| v.as_str()),
            );
            if let Some(selected) = cand
                .provider_metadata
                .get("selectedArchive")
                .filter(|value| !value.is_null())
            {
                retrieval_identity.selected_archive = Some(
                    serde_json::from_value(selected.clone())
                        .map_err(|_| AppError::bad_request("Invalid frozen archive selection"))?,
                );
            }
            retrieval_identity.language = Some(cand.language.clone());
            retrieval_identity.identity_override = super::interactive::is_override(cand);
            retrieval_identity.override_approved_at = if retrieval_identity.identity_override {
                Some(exec.approved_at)
            } else {
                None
            };
            retrieval_identity.metadata_confidence = Some(
                if retrieval_identity.identity_override {
                    "user_override"
                } else {
                    "source_identity"
                }
                .into(),
            );
            let identity_json = serde_json::to_string(&retrieval_identity)
                .map_err(|e| AppError::internal(e.to_string()))?;
            let thash = cand
                .provider_metadata
                .get("torrentHash")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let chapter_id: i64 = self.db.call(move |conn| {
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
                        rusqlite::params![manga_id, &ch_url, &ch_name, first_vol, &scanlator, now],
                    )?;
                    conn.last_insert_rowid()
                };

                let identity_json = preserve_queued_identity(conn, id, &identity_json, thash.as_deref())?;
                conn.execute(
                    "INSERT INTO downloads (
                        chapter_id, state, position, attempts, added_at, source_type, backend_type,
                        raw_title, uploader, release_size, torrent_hash, execution_id, execution_item_id, retrieval_identity_json
                    ) VALUES (
                        ?1, 'queued', (SELECT COALESCE(MAX(position), 0) + 1 FROM downloads), 0, ?2,
                        ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
                    ) ON CONFLICT(chapter_id) DO UPDATE SET
                        state = 'queued', attempts = 0, retry_at = NULL, error = NULL,
                        source_type = excluded.source_type, backend_type = excluded.backend_type,
                        raw_title = excluded.raw_title, uploader = excluded.uploader,
                        release_size = excluded.release_size, torrent_hash = excluded.torrent_hash,
                        execution_id = excluded.execution_id, execution_item_id = excluded.execution_item_id,
                        retrieval_identity_json = excluded.retrieval_identity_json",
                    rusqlite::params![
                        id, now, src_type, bk_type, &raw_t, &scanlator, release_size, &thash,
                        &exec_id, &item_id, &identity_json
                    ],
                )?;

                Ok(id)
            }).await?;

            item.download_job_id = Some(chapter_id);
            item.status = ExecutionItemStatus::Queued;
            self.db
                .call({
                    let iid = item.id.clone();
                    move |conn| {
                        crate::acquisition::execution::update_item_status(
                            conn,
                            &iid,
                            ExecutionItemStatus::Queued,
                            Some(chapter_id),
                            None,
                        )
                    }
                })
                .await?;
        }

        if let Some(wake) = &self.wake_downloads {
            wake.notify_waiters();
        }

        Ok(exec)
    }

    pub async fn fail_execution(&self, execution_id: &str, reason: &str) -> Result<()> {
        self.db
            .call({
                let eid = execution_id.to_string();
                let r = reason.to_string();
                move |conn| {
                    crate::acquisition::execution::update_execution_status(
                        conn,
                        &eid,
                        ExecutionStatus::Failed,
                        Some(&r),
                        None,
                        Some(&r),
                    )
                }
            })
            .await
    }

    /// Fetches execution snapshot, synchronizing live status with downloads and canonical publication.
    pub async fn get_execution(&self, execution_id: &str) -> Result<Option<AcquisitionExecution>> {
        let exec_opt = self
            .db
            .call({
                let eid = execution_id.to_string();
                move |conn| crate::acquisition::execution::get_execution(conn, &eid)
            })
            .await?;

        let mut exec = match exec_opt {
            Some(e) => e,
            None => return Ok(None),
        };

        if matches!(
            exec.status,
            ExecutionStatus::Completed | ExecutionStatus::Cancelled | ExecutionStatus::Stale
        ) {
            return Ok(Some(exec));
        }

        let eid = exec.id.clone();
        let updated = self.db.call(move |conn| {
            let mut any_active = false;
            let mut active_state = ExecutionStatus::Queued;

            for item in &mut exec.items {
                if matches!(
                    item.status,
                    ExecutionItemStatus::Completed
                        | ExecutionItemStatus::Skipped
                        | ExecutionItemStatus::Cancelled
                ) {
                    continue;
                }

                let dl_row: Option<(String, Option<String>)> = conn.query_row(
                    "SELECT state, error FROM downloads WHERE execution_item_id = ?1 OR chapter_id = ?2",
                    rusqlite::params![&item.id, item.download_job_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional()?;

                if let Some((dl_state, dl_error)) = dl_row {
                    match dl_state.as_str() {
                        "failed" => {
                            item.status = ExecutionItemStatus::Failed;
                            item.error_message = dl_error;
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &item.id,
                                item.status,
                                item.download_job_id,
                                item.error_message.as_deref(),
                            )?;
                        }
                        "downloading" => {
                            item.status = ExecutionItemStatus::Downloading;
                            any_active = true;
                            active_state = ExecutionStatus::Downloading;
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &item.id,
                                item.status,
                                item.download_job_id,
                                None,
                            )?;
                        }
                        "validating" => {
                            item.status = ExecutionItemStatus::Verifying;
                            item.verification_state = Some("validating".to_string());
                            any_active = true;
                            active_state = ExecutionStatus::Verifying;
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &item.id,
                                item.status,
                                item.download_job_id,
                                None,
                            )?;
                        }
                        "publishing" => {
                            item.status = ExecutionItemStatus::Publishing;
                            item.publication_state = Some("publishing".to_string());
                            any_active = true;
                            active_state = ExecutionStatus::Publishing;
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &item.id,
                                item.status,
                                item.download_job_id,
                                None,
                            )?;
                        }
                        "komga" => {
                            item.status = ExecutionItemStatus::Reconciling;
                            any_active = true;
                            active_state = ExecutionStatus::Reconciling;
                            crate::acquisition::execution::update_item_status(
                                conn,
                                &item.id,
                                item.status,
                                item.download_job_id,
                                None,
                            )?;
                        }
                        _ => {
                            item.status = ExecutionItemStatus::Queued;
                            any_active = true;
                        }
                    }
                } else if let Some(job_id) = item.download_job_id {
                    let ch_path: Option<String> = conn.query_row(
                        "SELECT download_path FROM chapters WHERE id = ?1",
                        [job_id],
                        |r| r.get(0),
                    ).optional()?.flatten();

                    if ch_path.is_some() {
                        item.status = ExecutionItemStatus::Completed;
                        item.publication_state = Some("published".to_string());
                        item.verification_state = Some("verified".to_string());
                        crate::acquisition::execution::update_item_status(
                            conn,
                            &item.id,
                            item.status,
                            item.download_job_id,
                            None,
                        )?;
                    } else if item.status == ExecutionItemStatus::Queued
                        || item.status == ExecutionItemStatus::Downloading
                    {
                        item.status = ExecutionItemStatus::Failed;
                        item.error_message = Some("Job disappeared from downloads queue before publication".to_string());
                        crate::acquisition::execution::update_item_status(
                            conn,
                            &item.id,
                            item.status,
                            item.download_job_id,
                            item.error_message.as_deref(),
                        )?;
                    }
                }
            }

            let all_completed = !exec.items.is_empty() && exec.items.iter().all(|i| {
                matches!(i.status, ExecutionItemStatus::Completed | ExecutionItemStatus::Skipped)
            });
            let any_failed = exec.items.iter().any(|i| i.status == ExecutionItemStatus::Failed);

            let new_exec_status = if any_active {
                active_state
            } else if any_failed {
                let has_completed = exec.items.iter().any(|i| i.status == ExecutionItemStatus::Completed);
                if has_completed {
                    ExecutionStatus::Partial
                } else {
                    ExecutionStatus::Failed
                }
            } else if all_completed {
                ExecutionStatus::Completed
            } else {
                exec.status
            };

            if new_exec_status != exec.status {
                crate::acquisition::execution::update_execution_status(
                    conn,
                    &eid,
                    new_exec_status,
                    None,
                    None,
                    None,
                )?;
                exec.status = new_exec_status;
            }

            Ok(exec)
        }).await?;

        Ok(Some(updated))
    }

    pub async fn get_title_executions(&self, manga_id: i64) -> Result<Vec<AcquisitionExecution>> {
        self.db
            .call(move |conn| {
                crate::acquisition::execution::get_executions_for_manga(conn, manga_id)
            })
            .await
    }

    pub async fn cancel_execution(&self, execution_id: &str, reason: &str) -> Result<()> {
        let eid = execution_id.to_string();
        let r = reason.to_string();

        self.db.call({
            let eid = eid.clone();
            let r = r.clone();
            move |conn| {
                crate::acquisition::execution::cancel_execution(conn, &eid, &r)?;
                conn.execute(
                    "DELETE FROM downloads WHERE execution_id = ?1 AND state IN ('queued', 'downloading')",
                    [&eid],
                )?;
                Ok(())
            }
        }).await
    }

    pub async fn retry_execution(&self, execution_id: &str) -> Result<AcquisitionExecution> {
        let eid = execution_id.to_string();
        self.db
            .call({
                let eid = eid.clone();
                move |conn| {
                    crate::acquisition::execution::retry_execution(conn, &eid)?;
                    Ok(())
                }
            })
            .await?;

        self.start_execution(eid).await
    }

    pub async fn reconcile_executions_on_startup(&self) -> Result<()> {
        let pending_ids = self.db.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM acquisition_executions
                 WHERE status IN ('PREFLIGHT', 'REVALIDATING', 'QUEUED', 'DOWNLOADING', 'VERIFYING', 'PUBLISHING', 'RECONCILING')",
            )?;
            let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.filter_map(std::result::Result::ok).collect::<Vec<_>>();
            Ok(ids)
        }).await?;

        for id in pending_ids {
            let _ = self.get_execution(&id).await;
        }
        Ok(())
    }
}
