//! The download queue. A chapter is fetched page by page and saved as one CBZ at
//! `<data>/downloads/<source>/<title>/<chapter>.cbz`, with a `ComicInfo.xml` inside.
//! The queue lives in the database, so it survives a restart.

use crate::{
    db::{now, Db},
    error::{AppError, Result},
    images::Images,
    settings,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::sync::Notify;

pub const PAUSED: &str = "downloads.paused";
pub const DELETE_READ: &str = "downloads.deleteAfterReading";
/// Tries a chapter gets before it is parked as failed.
const TRIES: i64 = 5;
/// Seconds to wait before the next try, by how many have failed so far.
const WAITS: [i64; 4] = [30, 120, 600, 1800];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionLiveState {
    pub chapter_id: i64,
    pub stage: String,
    pub status_message: String,
    pub destination: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    pub chapter_id: i64,
    pub chapter_name: String,
    pub manga_id: i64,
    pub title: String,
    /// queued, downloading, identifying, metadata, optimizing, validating, publishing, komga, or failed.
    pub state: String,
    pub stage: String,
    pub status_message: Option<String>,
    pub destination: Option<String>,
    pub source: Option<String>,
    pub scanlator: Option<String>,
    pub release_type: Option<String>,
    pub volume: Option<f64>,
    pub done: i64,
    pub total: Option<i64>,
    pub attempts: i64,
    pub retry_at: Option<i64>,
    pub error: Option<String>,
    pub can_retry_komga: bool,
    pub failure_stage: Option<String>,
    pub published_path: Option<String>,
    pub source_type: Option<String>,
    pub backend_type: Option<String>,
    pub external_job_id: Option<String>,
    pub raw_title: Option<String>,
    pub uploader: Option<String>,
    pub release_size: Option<u64>,
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
}

#[derive(Serialize)]
pub struct QueueView {
    pub paused: bool,
    pub queue: Vec<Queued>,
}

/// A chapter picked from the queue, with what its file name and ComicInfo need.
#[derive(Clone)]
pub(crate) struct Job {
    pub(crate) chapter: i64,
    pub(crate) manga_id: i64,
    pub(crate) attempts: i64,
    pub(crate) source: String,
    pub(crate) title: String,
    pub(crate) name: String,
    pub(crate) number: f64,
    pub(crate) scanlator: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) genre: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) lang: Option<String>,
    pub(crate) volume: Option<f64>,
    pub(crate) release_type: crate::release::ReleaseType,
    pub(crate) chapter_number: Option<f64>,
    pub(crate) published_path: Option<String>,
    pub(crate) target_library_id: Option<String>,
    pub(crate) source_type: String,
    pub(crate) backend_type: String,
    pub(crate) external_job_id: Option<String>,
    #[allow(dead_code)]
    pub(crate) external_file_id: Option<i64>,
    pub(crate) torrent_hash: Option<String>,
    pub(crate) raw_title: Option<String>,
    pub(crate) uploader: Option<String>,
    pub(crate) release_size: Option<u64>,
    pub(crate) retrieved_path: Option<String>,
    #[allow(dead_code)]
    pub(crate) collection_id: Option<String>,
    #[allow(dead_code)]
    pub(crate) collection_name: Option<String>,
}

impl Default for Job {
    fn default() -> Self {
        Self {
            chapter: 0,
            manga_id: 0,
            attempts: 0,
            source: "Unknown source".to_string(),
            title: String::new(),
            name: String::new(),
            number: 0.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: None,
            volume: None,
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
            source_type: "reader".to_string(),
            backend_type: "internal".to_string(),
            external_job_id: None,
            external_file_id: None,
            torrent_hash: None,
            raw_title: None,
            uploader: None,
            release_size: None,
            retrieved_path: None,
            collection_id: None,
            collection_name: None,
        }
    }
}

#[derive(Clone)]
pub struct Downloads {
    db: Db,
    images: Images,
    dir: PathBuf,
    data_dir: PathBuf,
    /// A chapter is not fetched while the disk has less than this free, in bytes. 0: always.
    keep_free: u64,
    wake: Arc<Notify>,
    /// The chapter being fetched: its id, pages done, pages in all.
    progress: Arc<StdMutex<Option<(i64, i64, i64)>>>,
    live_stage: Arc<StdMutex<Option<AcquisitionLiveState>>>,
    pipeline: Option<Arc<crate::pipeline::Pipeline>>,
    pub(crate) acquisition: Option<Arc<crate::acquisition::service::AcquisitionService>>,
}

impl Downloads {
    pub fn new(db: Db, images: Images, data_dir: &Path, keep_free_mb: u64) -> Self {
        Self {
            db,
            images,
            dir: data_dir.join("downloads"),
            data_dir: data_dir.to_owned(),
            keep_free: keep_free_mb << 20,
            wake: Arc::default(),
            progress: Arc::default(),
            live_stage: Arc::default(),
            pipeline: None,
            acquisition: None,
        }
    }

    pub fn set_pipeline(&mut self, pipeline: Arc<crate::pipeline::Pipeline>) {
        self.pipeline = Some(pipeline);
    }

    pub fn set_acquisition(&mut self, acquisition: Arc<crate::acquisition::service::AcquisitionService>) {
        self.acquisition = Some(acquisition);
    }

    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    /// The folder chapters are saved in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Adds chapters to the end of the queue. Ones already saved or queued are skipped.
    pub async fn enqueue(&self, chapters: Vec<i64>) -> Result<usize> {
        let added = self.db.call(move |conn| add(conn, &chapters)).await?;
        self.wake.notify_one();
        Ok(added)
    }

    /// Queues the unread chapters of these titles, oldest first.
    pub async fn enqueue_unread(&self, titles: Vec<i64>) -> Result<usize> {
        let added = self
            .db
            .call(move |conn| {
                let chapters = unread_of(conn, &titles)?;
                add(conn, &chapters)
            })
            .await?;
        self.wake.notify_one();
        Ok(added)
    }

    pub async fn list(&self) -> Result<QueueView> {
        let mut view = self.db.call(|conn| read_queue(conn)).await?;
        let live = *self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((chapter, done, total)) = live {
            if let Some(entry) = view.queue.iter_mut().find(|q| q.chapter_id == chapter) {
                entry.done = done;
                entry.total = Some(total);
            }
        }
        let active = self.live_stage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(st) = active {
            if let Some(entry) = view.queue.iter_mut().find(|q| q.chapter_id == st.chapter_id) {
                entry.stage = st.stage;
                entry.status_message = Some(st.status_message);
                if st.destination.is_some() {
                    entry.destination = st.destination;
                }
            }
        }
        Ok(view)
    }

    /// Retries the Komga library scan stage without re-downloading or re-optimizing.
    pub async fn retry_komga(&self, chapter: i64) -> Result<String> {
        let (manga_id, title, lang, pub_path_opt, target_lib_opt, is_vol, rel_num) = self
            .db
            .call(move |conn| {
                let (manga_id, title, lang, pub_path, target_lib, ch_name, ch_num): (
                    i64,
                    String,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    String,
                    f64,
                ) = conn.query_row(
                    "SELECT m.id, m.title, s.lang, d.published_path, d.target_library_id, c.name, c.chapter_number
                     FROM chapters c
                     JOIN manga m ON m.id = c.manga_id
                     JOIN downloads d ON d.chapter_id = c.id
                     LEFT JOIN sources s ON s.id = m.source_id
                     WHERE c.id = ?1",
                    [chapter],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
                )?;
                let rel = crate::release::parse_release(&ch_name, ch_num);
                let is_vol = rel.release_type == crate::release::ReleaseType::Volume;
                let rel_num = if is_vol {
                    rel.volume.unwrap_or(ch_num)
                } else {
                    rel.chapter_number.unwrap_or(ch_num)
                };
                Ok((manga_id, title, lang, pub_path, target_lib, is_vol, rel_num))
            })
            .await?;

        if let Some(pipeline) = &self.pipeline {
            let (config, _, komga_key) = pipeline.load_internal_config().await?;
            if !config.komga_enabled {
                return Err(AppError::bad_request("Komga integration is disabled in Pipeline settings"));
            }

            // Verify the published canonical artifact exists and has valid archive integrity
            let pub_path = match pub_path_opt {
                Some(p) => p,
                None => return Err(AppError::bad_request("No canonical published artifact recorded for this chapter")),
            };

            let path = PathBuf::from(&pub_path);
            if !path.is_file() {
                return Err(AppError::bad_request(format!(
                    "Canonical published artifact does not exist at {}",
                    path.display()
                )));
            }
            crate::pipeline::publisher::verify_cbz_integrity(&path)?;

            let is_chinese = lang.as_deref().map(|l| l.to_lowercase()).map(|l| l == "zh" || l.contains("chinese") || l.contains("mandarin")).unwrap_or(false);
            let target_library_id = target_lib_opt.or_else(|| {
                if is_chinese {
                    config.komga_chinese_library_id.or(config.komga_library_id)
                } else {
                    config.komga_library_id
                }
            });

            let komga_client = crate::pipeline::komga::KomgaClient::new(pipeline.http.clone(), config.komga_url, komga_key);
            komga_client.trigger_scan(target_library_id.as_deref()).await?;

            // Bounded quick check before returning (does not hang HTTP request)
            let policy = crate::pipeline::VerificationPolicy {
                timeout: Duration::from_secs(3),
                initial_delay: Duration::from_millis(500),
                max_interval: Duration::from_secs(1),
                backoff_factor: 1.5,
            };

            let verified = pipeline
                .poll_verify_release(
                    &komga_client,
                    &title,
                    target_library_id.as_deref(),
                    is_vol,
                    rel_num,
                    policy,
                )
                .await
                .unwrap_or(false);

            if verified {
                pipeline.ownership.invalidate(manga_id);
                let _ = pipeline.ownership.get_title_ownership(manga_id, &title).await;
                self.db
                    .call(move |conn| {
                        conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter])?;
                        Ok(())
                    })
                    .await?;
                Ok(format!("Ownership verified in Komga — acquisition complete for {title}"))
            } else {
                self.db
                    .call(move |conn| {
                        conn.execute(
                            "UPDATE downloads SET state = 'verifying', error = 'Published — waiting for Komga' WHERE chapter_id = ?1",
                            [chapter],
                        )?;
                        Ok(())
                    })
                    .await?;
                Ok(format!("Komga library scan triggered — ownership verification pending for {title}"))
            }
        } else {
            Err(AppError::bad_request("Pipeline not configured"))
        }
    }

    /// Takes one chapter out of the queue, or with None all of them. A fetch in progress stops at its next page.
    pub async fn cancel(&self, chapter: Option<i64>) -> Result<()> {
        let external_jobs: Vec<(String, String)> = self
            .db
            .call(move |conn| {
                let sql = match chapter {
                    Some(id) => format!("SELECT backend_type, external_job_id FROM downloads WHERE chapter_id = {id} AND external_job_id IS NOT NULL"),
                    None => "SELECT backend_type, external_job_id FROM downloads WHERE external_job_id IS NOT NULL".to_string(),
                };
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .filter_map(std::result::Result::ok)
                    .collect();
                Ok(rows)
            })
            .await?;

        for (backend, job_id) in external_jobs {
            if backend == "torbox" {
                let token = if let Some(acq) = &self.acquisition {
                    acq.get_torbox_token().await.ok().flatten()
                } else {
                    self.db.call(|conn| crate::db::state(conn, "acquisition.torbox_token")).await.ok().flatten()
                };
                let (acq_cfg, _) = if let Some(acq) = &self.acquisition {
                    acq.load_internal_config().await.unwrap_or_default()
                } else {
                    (crate::acquisition::AcquisitionConfig::default(), false)
                };
                let client = crate::acquisition::torbox::TorBoxClient::new(
                    self.pipeline.as_ref().map(|p| p.http.clone()).unwrap_or_default(),
                    acq_cfg.torbox_url,
                    token,
                );
                let jid = job_id.clone();
                let has_siblings = self.db.call(move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT count(*) FROM downloads WHERE external_job_id = ?1 AND chapter_id != COALESCE(?2, -1)",
                        rusqlite::params![&jid, chapter],
                        |r| r.get(0),
                    ).unwrap_or(0);
                    Ok(count > 0)
                }).await.unwrap_or(false);

                if !has_siblings {
                    let _ = client.cancel(&job_id).await;
                }
            }
        }

        self.db
            .call(move |conn| {
                match chapter {
                    Some(id) => conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [id])?,
                    None => conn.execute("DELETE FROM downloads", [])?,
                };
                Ok(())
            })
            .await
    }

    /// Gives a parked chapter a fresh set of tries.
    pub async fn retry(&self, chapter: i64) -> Result<()> {
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE downloads SET state = 'queued', attempts = 0, retry_at = NULL, error = NULL WHERE chapter_id = ?1 AND state != 'downloading'",
                    [chapter],
                )?;
                Ok(())
            })
            .await?;
        self.wake.notify_one();
        Ok(())
    }

    pub async fn set_paused(&self, paused: bool) -> Result<()> {
        self.db.call(move |conn| settings::set(conn, PAUSED, &paused.into())).await?;
        self.wake.notify_one();
        Ok(())
    }

    /// Puts the queue in the order given. Chapters left out keep their order, after the others.
    pub async fn reorder(&self, chapters: Vec<i64>) -> Result<()> {
        self.db
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute("UPDATE downloads SET position = position + ?1", [chapters.len() as i64 + 1_000_000_000])?;
                for (position, id) in chapters.iter().enumerate() {
                    tx.execute("UPDATE downloads SET position = ?2 WHERE chapter_id = ?1", params![id, position as i64])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    /// Deletes a chapter's staging file. The chapter and its progress stay.
    /// Canonical library artifacts are strictly preserved.
    pub async fn delete(&self, chapter: i64) -> Result<()> {
        let (path, is_published) = self
            .db
            .call(move |conn| {
                let (path, pub_path): (Option<String>, Option<String>) = conn
                    .query_row(
                        "SELECT c.download_path, d.published_path FROM chapters c LEFT JOIN downloads d ON d.chapter_id = c.id WHERE c.id = ?1",
                        [chapter],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .unwrap_or((None, None));
                let is_canonical = path.as_deref().map(|p| Path::new(p).is_absolute()).unwrap_or(false) || pub_path.is_some();
                if !is_canonical {
                    conn.execute("UPDATE chapters SET download_path = NULL WHERE id = ?1", [chapter])?;
                }
                Ok((path, is_canonical))
            })
            .await?;
        if let Some(relative) = path {
            // Requirement 12: Cancellation/delete must NEVER delete the valid canonical library artifact
            let is_abs_or_canonical = Path::new(&relative).is_absolute() || is_published;
            if !is_abs_or_canonical {
                let file = self.dir.join(relative);
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = fs::remove_file(&file);
                    // The title's folder and the source's go too, when this was the last file in them.
                    for folder in file.ancestors().skip(1).take(2) {
                        let _ = fs::remove_dir(folder);
                    }
                })
                .await;
            }
        }
        Ok(())
    }

    /// Called when a chapter is finished in the reader: deletes its file if the owner asked for that.
    pub async fn finished_reading(&self, chapter: i64) {
        let wanted = self.db.call(|conn| Ok(settings::get::<bool>(conn, DELETE_READ).unwrap_or(false))).await.unwrap_or(false);
        if wanted {
            if let Err(e) = self.delete(chapter).await {
                tracing::warn!("could not delete the file of a chapter that was read: {e}");
            }
        }
    }

    /// Works through the queue, one chapter at a time. Never returns.
    pub async fn run(self) {
        // Whatever was being fetched when Mango stopped starts over,
        // but downloads that were already published durably remain published (do NOT redownload!).
        let _ = self
            .db
            .call(|conn| {
                // 1. Verify any downloads marked with published_path
                let mut stmt = conn.prepare("SELECT chapter_id, published_path FROM downloads WHERE published_path IS NOT NULL")?;
                let published_items = stmt
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .filter_map(std::result::Result::ok)
                    .collect::<Vec<_>>();

                for (ch_id, pub_path) in published_items {
                    let path = PathBuf::from(&pub_path);
                    if path.is_file() && crate::pipeline::publisher::verify_cbz_integrity(&path).is_ok() {
                        conn.execute(
                            "UPDATE downloads SET state = 'verifying', error = 'Published — waiting for Komga' WHERE chapter_id = ?1",
                            [ch_id],
                        )?;
                    } else {
                        conn.execute(
                            "UPDATE downloads SET published_path = NULL, state = 'queued', error = NULL WHERE chapter_id = ?1",
                            [ch_id],
                        )?;
                    }
                }

                // 2. Interrupted staging downloads reset to queued
                conn.execute("UPDATE downloads SET state = 'queued' WHERE state = 'downloading' AND published_path IS NULL", [])?;
                Ok(())
            })
            .await;

        loop {
            let Ok(Some(job)) = self.db.call(|conn| next_job(conn)).await else {
                // Nothing due. Wait for a nudge, and look again before long in case a retry has come due.
                let _ = tokio::time::timeout(Duration::from_secs(30), self.wake.notified()).await;
                continue;
            };

            // If this job was already published (e.g. restart recovery or retry from verification timeout):
            if let Some(ref pub_path) = job.published_path {
                let canonical_path = PathBuf::from(pub_path);
                if canonical_path.is_file() && crate::pipeline::publisher::verify_cbz_integrity(&canonical_path).is_ok() {
                    // Do NOT redownload! Proceed directly to Komga verification
                    if let Some(pipeline) = &self.pipeline {
                        let live_stage = self.live_stage.clone();
                        let chapter_id = job.chapter;
                        let update_stage = move |stage: &str, msg: &str, dest: Option<String>| {
                            if let Ok(mut lock) = live_stage.lock() {
                                *lock = Some(AcquisitionLiveState {
                                    chapter_id,
                                    stage: stage.to_string(),
                                    status_message: msg.to_string(),
                                    destination: dest,
                                });
                            }
                        };

                        let (config, _, komga_key) = pipeline.load_internal_config().await.unwrap_or_default();
                        if config.komga_enabled && config.komga_scan_enabled {
                            let komga_client = crate::pipeline::komga::KomgaClient::new(pipeline.http.clone(), config.komga_url, komga_key);
                            update_stage("komga", "Triggering targeted Komga library scan...", Some(pub_path.clone()));
                            let target_lib = job.target_library_id.as_deref().or(config.komga_library_id.as_deref());
                            if let Ok(()) = komga_client.trigger_scan(target_lib).await {
                                update_stage("verifying", "Ownership verification pending in Komga...", Some(pub_path.clone()));
                                let policy = pipeline.verification_policy();
                                let is_vol = job.release_type == crate::release::ReleaseType::Volume;
                                let rel_num = if is_vol {
                                    job.volume.unwrap_or(job.number)
                                } else {
                                    job.chapter_number.unwrap_or(job.number)
                                };
                                let verified = pipeline
                                    .poll_verify_release(
                                        &komga_client,
                                        &job.title,
                                        target_lib,
                                        is_vol,
                                        rel_num,
                                        policy,
                                    )
                                    .await
                                    .unwrap_or(false);
                                if verified {
                                    pipeline.ownership.invalidate(job.manga_id);
                                    let _ = pipeline.ownership.get_title_ownership(job.manga_id, &job.title).await;
                                    let _ = self
                                        .db
                                        .call(move |conn| {
                                            conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter_id])?;
                                            Ok(())
                                        })
                                        .await;
                                    update_stage("complete", "Acquisition complete — verified in Komga", Some(pub_path.clone()));
                                } else {
                                    let _ = self
                                        .db
                                        .call(move |conn| {
                                            conn.execute(
                                                "UPDATE downloads SET state = 'verifying', error = 'Published — waiting for Komga', attempts = attempts + 1 WHERE chapter_id = ?1",
                                                [chapter_id],
                                            )?;
                                            Ok(())
                                        })
                                        .await;
                                }
                            } else {
                                let _ = self
                                    .db
                                    .call(move |conn| {
                                        conn.execute(
                                            "UPDATE downloads SET state = 'failed', error = 'Published, but Komga scan trigger failed' WHERE chapter_id = ?1",
                                            [chapter_id],
                                        )?;
                                        Ok(())
                                    })
                                    .await;
                            }
                        } else {
                            let _ = self
                                .db
                                .call(move |conn| {
                                    conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter_id])?;
                                    Ok(())
                                })
                                .await;
                        }
                    }
                    *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    continue;
                }
            }

            let outcome = if job.backend_type == "torbox" {
                match self.handle_torbox_job(&job).await {
                    Ok(Some(res)) => Ok(Some(res)),
                    Ok(None) => continue, // in progress or stalled waiting for retry
                    Err(e) => {
                        let ch_id = job.chapter;
                        let err_msg = e.to_string();
                        let err_msg_db = err_msg.clone();
                        let _ = self
                            .db
                            .call(move |conn| {
                                conn.execute(
                                    "UPDATE downloads SET state = 'failed', error = ?1 WHERE chapter_id = ?2",
                                    rusqlite::params![&err_msg_db, ch_id],
                                )?;
                                Ok(())
                            })
                            .await;
                        if let Some(acq) = &self.acquisition {
                            let _ = acq
                                .record_history(
                                    Some(job.chapter),
                                    job.manga_id,
                                    &job.title,
                                    job.raw_title.as_deref().unwrap_or(&job.name),
                                    &job.source_type,
                                    job.uploader.as_deref(),
                                    job.external_job_id.as_deref(),
                                    &job.backend_type,
                                    "failed",
                                    None,
                                    job.release_size,
                                    Some(&err_msg),
                                )
                                .await;
                        }
                        *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        continue;
                    }
                }
            } else {
                self.save(&job).await
            };
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = None;

            let mut final_outcome = outcome.clone();
            if let Ok(Some((relative_path, _page_count))) = &outcome {
                if let Some(pipeline) = &self.pipeline {
                    let cbz_file = self.dir.join(relative_path);
                    let live_stage = self.live_stage.clone();
                    let chapter_id = job.chapter;

                    let update_stage = move |stage: &str, msg: &str, dest: Option<String>| {
                        if let Ok(mut lock) = live_stage.lock() {
                            *lock = Some(AcquisitionLiveState {
                                chapter_id,
                                stage: stage.to_string(),
                                status_message: msg.to_string(),
                                destination: dest,
                            });
                        }
                    };

                    match pipeline.process_download_with_progress(&cbz_file, &job, update_stage).await {
                        Ok(crate::pipeline::AcquisitionOutcome::Complete(_)) => {
                            if let Some(acq) = &self.acquisition {
                                let _ = acq
                                    .record_history(
                                        Some(job.chapter),
                                        job.manga_id,
                                        &job.title,
                                        job.raw_title.as_deref().unwrap_or(&job.name),
                                        &job.source_type,
                                        job.uploader.as_deref(),
                                        job.external_job_id.as_deref(),
                                        &job.backend_type,
                                        "completed",
                                        Some(&cbz_file.display().to_string()),
                                        job.release_size,
                                        None,
                                    )
                                    .await;
                            }
                            *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
                            continue;
                        }
                        Ok(crate::pipeline::AcquisitionOutcome::PublishedPendingKomga(_)) => {
                            if let Some(acq) = &self.acquisition {
                                let _ = acq
                                    .record_history(
                                        Some(job.chapter),
                                        job.manga_id,
                                        &job.title,
                                        job.raw_title.as_deref().unwrap_or(&job.name),
                                        &job.source_type,
                                        job.uploader.as_deref(),
                                        job.external_job_id.as_deref(),
                                        &job.backend_type,
                                        "verification_pending",
                                        Some(&cbz_file.display().to_string()),
                                        job.release_size,
                                        None,
                                    )
                                    .await;
                            }
                            *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
                            continue;
                        }
                        Ok(crate::pipeline::AcquisitionOutcome::PublishedScanFailed { error, .. }) => {
                            tracing::warn!("acquisition published for {} / {}, but Komga scan failed: {error}", job.title, job.name);
                            if let Some(acq) = &self.acquisition {
                                let _ = acq
                                    .record_history(
                                        Some(job.chapter),
                                        job.manga_id,
                                        &job.title,
                                        job.raw_title.as_deref().unwrap_or(&job.name),
                                        &job.source_type,
                                        job.uploader.as_deref(),
                                        job.external_job_id.as_deref(),
                                        &job.backend_type,
                                        "published_scan_failed",
                                        Some(&cbz_file.display().to_string()),
                                        job.release_size,
                                        Some(&error),
                                    )
                                    .await;
                            }
                            *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!("acquisition pipeline processing failed for {} / {}: {e}", job.title, job.name);
                            if let Some(acq) = &self.acquisition {
                                let _ = acq
                                    .record_history(
                                        Some(job.chapter),
                                        job.manga_id,
                                        &job.title,
                                        job.raw_title.as_deref().unwrap_or(&job.name),
                                        &job.source_type,
                                        job.uploader.as_deref(),
                                        job.external_job_id.as_deref(),
                                        &job.backend_type,
                                        "pipeline_failed",
                                        None,
                                        job.release_size,
                                        Some(&e.to_string()),
                                    )
                                    .await;
                            }
                            final_outcome = Err(e);
                        }
                    }
                }
            }

            *self.live_stage.lock().unwrap_or_else(|e| e.into_inner()) = None;

            if let Err(e) = &final_outcome {
                tracing::warn!("acquisition of {} / {} failed: {e}", job.title, job.name);
            }
            let (chapter, attempts) = (job.chapter, job.attempts);
            if let Err(e) = self.db.call(move |conn| record(conn, chapter, attempts, final_outcome)).await {
                tracing::warn!("could not record a download: {e}");
            }
        }
    }

    /// Handles external remote torrent acquisition via TorBox.
    async fn handle_torbox_job(&self, job: &Job) -> Result<Option<(String, i64)>> {
        // 1. If already retrieved, verify staging CBZ
        if let Some(ref rel) = job.retrieved_path {
            let path = self.dir.join(rel);
            if path.is_file() && crate::pipeline::publisher::verify_cbz_integrity(&path).is_ok() {
                return Ok(Some((rel.clone(), 1)));
            }
        }

        let token = if let Some(acq) = &self.acquisition {
            acq.get_torbox_token().await.ok().flatten()
        } else {
            self.db.call(|conn| crate::db::state(conn, "acquisition.torbox_token")).await.ok().flatten()
        };
        let (acq_cfg, _) = if let Some(acq) = &self.acquisition {
            acq.load_internal_config().await.unwrap_or_default()
        } else {
            (crate::acquisition::AcquisitionConfig::default(), false)
        };
        let client = crate::acquisition::torbox::TorBoxClient::new(
            self.pipeline.as_ref().map(|p| p.http.clone()).unwrap_or_default(),
            acq_cfg.torbox_url,
            token,
        );

        let live_stage = self.live_stage.clone();
        let chapter_id = job.chapter;
        let update_stage = move |stage: &str, msg: &str| {
            if let Ok(mut lock) = live_stage.lock() {
                *lock = Some(AcquisitionLiveState {
                    chapter_id,
                    stage: stage.to_string(),
                    status_message: msg.to_string(),
                    destination: None,
                });
            }
        };

        // 2. Submit if external_job_id is missing
        let external_job_id = match &job.external_job_id {
            Some(id) if !id.trim().is_empty() => id.clone(),
            _ => {
                update_stage("torbox_submitting", "Submitting torrent to TorBox remote swarm...");
                let magnet = if let Some(ref hash) = job.torrent_hash {
                    crate::acquisition::nyaa::build_magnet(hash, job.raw_title.as_deref().unwrap_or(&job.name))
                } else {
                    return Err(AppError::bad_request("TorBox job has no torrent hash or magnet"));
                };

                let rel = crate::acquisition::AcquisitionRelease {
                    source: "nyaa".to_string(),
                    source_item_id: job.chapter.to_string(),
                    title: job.name.clone(),
                    raw_title: job.raw_title.clone().unwrap_or_else(|| job.name.clone()),
                    uploader: job.uploader.clone(),
                    source_url: String::new(),
                    torrent_url: None,
                    magnet_url: Some(magnet),
                    info_hash: job.torrent_hash.clone(),
                    category_id: None,
                    category_name: None,
                    size_bytes: job.release_size.unwrap_or(0),
                    size_formatted: String::new(),
                    seeders: 1,
                    leechers: 0,
                    published_at: None,
                    is_trusted: false,
                    is_remake: false,
                    is_variant: false,
                    variant_name: None,
                    is_mixed: false,
                    release_unit: match job.release_type {
                        crate::release::ReleaseType::Volume => crate::acquisition::AcquisitionReleaseUnit::Volume(job.volume.unwrap_or(job.number)),
                        crate::release::ReleaseType::Chapter => crate::acquisition::AcquisitionReleaseUnit::Chapter(job.chapter_number.unwrap_or(job.number)),
                    },
                    release_number: Some(job.number),
                    release_range: None,
                    language: job.lang.clone(),
                };

                let info = client.submit(&rel).await?;
                let jid = info.external_job_id.clone();
                let jid_db = jid.clone();
                let thash = info.torrent_hash.clone();
                let ch_id = job.chapter;
                self.db.call(move |conn| {
                    conn.execute(
                        "UPDATE downloads SET external_job_id = ?1, torrent_hash = COALESCE(torrent_hash, ?2) WHERE chapter_id = ?3",
                        rusqlite::params![&jid_db, &thash, ch_id],
                    )?;
                    Ok(())
                }).await?;
                jid
            }
        };

        // 3. Check status
        update_stage("torbox_checking", "Checking TorBox download status...");
        let status = client.check_status(&external_job_id).await?;
        match status {
            crate::acquisition::BackendJobStatus::Downloading { progress, seeds, .. } => {
                let pct = (progress * 100.0).clamp(0.0, 99.0);
                update_stage("torbox_downloading", &format!("Downloading in TorBox: {pct:.0}% ({seeds} seeds)"));
                let ch_id = job.chapter;
                let status_msg = format!("Downloading in TorBox ({pct:.0}%)");
                self.db.call(move |conn| {
                    conn.execute(
                        "UPDATE downloads SET state = 'queued', error = ?1, retry_at = strftime('%s','now') + 5 WHERE chapter_id = ?2",
                        rusqlite::params![&status_msg, ch_id],
                    )?;
                    Ok(())
                }).await?;
                if let Ok(mut p) = self.progress.lock() {
                    *p = Some((job.chapter, pct as i64, 100));
                }
                return Ok(None);
            }
            crate::acquisition::BackendJobStatus::Stalled { reason } => {
                update_stage("torbox_stalled", &format!("TorBox stalled: {reason}"));
                let ch_id = job.chapter;
                let err_msg = reason.clone();
                self.db.call(move |conn| {
                    conn.execute(
                        "UPDATE downloads SET state = 'queued', error = ?1, retry_at = strftime('%s','now') + 15 WHERE chapter_id = ?2",
                        rusqlite::params![&err_msg, ch_id],
                    )?;
                    Ok(())
                }).await?;
                return Ok(None);
            }
            crate::acquisition::BackendJobStatus::Failed { error } => {
                return Err(AppError::bad_request(format!("TorBox download failed: {error}")));
            }
            crate::acquisition::BackendJobStatus::Cached | crate::acquisition::BackendJobStatus::Completed => {
                // Done on TorBox remote swarm
            }
        }

        // 4. File selection
        update_stage("torbox_selecting", "Selecting manga archive file...");
        let files = client.list_files(&external_job_id).await?;
        let target_unit = match job.release_type {
            crate::release::ReleaseType::Volume => Some(crate::acquisition::AcquisitionReleaseUnit::Volume(job.volume.unwrap_or(job.number))),
            crate::release::ReleaseType::Chapter => Some(crate::acquisition::AcquisitionReleaseUnit::Chapter(job.chapter_number.unwrap_or(job.number))),
        };
        let selected_file = if let Some(fid) = job.external_file_id {
            files.iter().find(|f| f.id == fid).ok_or_else(|| {
                crate::acquisition::torbox::TorBoxError::NoArchiveFound(format!(
                    "File id {fid} not found in TorBox torrent"
                ))
            })?
        } else {
            crate::acquisition::torbox::select_artifact(&files, target_unit.as_ref())?
        };

        // 5. Staging path
        let relative = self.place(job);
        let full_cbz_path = self.dir.join(&relative);
        let part_path = full_cbz_path.with_extension("cbz.part");

        if let Some(parent) = full_cbz_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // 6. Stream retrieve to staging .part
        update_stage("torbox_retrieving", &format!("Retrieving {} from TorBox CDN...", selected_file.name));
        client.retrieve_file(&external_job_id, selected_file.id, selected_file.size, &part_path).await?;

        // Double-check CBZ archive integrity & plausible manga images before atomic promotion
        if let Err(e) = crate::pipeline::publisher::verify_cbz_integrity(&part_path) {
            let _ = std::fs::remove_file(&part_path);
            return Err(AppError::bad_request(format!("Retrieved archive failed integrity verification: {e}")));
        }

        // Atomic promotion
        std::fs::rename(&part_path, &full_cbz_path)?;

        let rel_str = relative.to_string_lossy().replace('\\', "/");
        let ch_id = job.chapter;
        let rel_clone = rel_str.clone();
        self.db.call(move |conn| {
            conn.execute("UPDATE chapters SET download_path = ?1 WHERE id = ?2", rusqlite::params![&rel_clone, ch_id])?;
            conn.execute("UPDATE downloads SET retrieved_path = ?1 WHERE chapter_id = ?2", rusqlite::params![&rel_clone, ch_id])?;
            Ok(())
        }).await?;

        // Best effort remote delete on TorBox only if no other sibling downloads remain active
        let ext_jid = external_job_id.clone();
        let is_last_active = self.db.call(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM downloads WHERE external_job_id = ?1 AND chapter_id != ?2 AND state NOT IN ('failed', 'completed')",
                rusqlite::params![&ext_jid, ch_id],
                |r| r.get(0),
            ).unwrap_or(0);
            Ok(count == 0)
        }).await.unwrap_or(true);

        if is_last_active {
            let _ = client.cancel(&external_job_id).await;
        }

        Ok(Some((rel_str, 1)))
    }

    /// Fetches a chapter and writes its CBZ. None when it was cancelled or paused part way.
    async fn save(&self, job: &Job) -> Result<Option<(String, i64)>> {
        // A disk that fills up takes the database's next write with it.
        if self.keep_free > 0 && free_space(&self.data_dir).is_some_and(|free| free < self.keep_free) {
            let why = format!("Less than {} MB is free on the disk. Delete some downloads to make room.", self.keep_free >> 20);
            return Err(AppError::new(507, "disk_full", why));
        }
        let total = self.images.page_count(job.chapter).await?;
        let relative = self.place(job);
        let file = self.dir.join(&relative);
        let part = file.with_extension("cbz.part");
        let outcome = self.write(job, total, &part).await;
        if !matches!(outcome, Ok(true)) {
            let _ = fs::remove_file(&part);
            return outcome.map(|_| None);
        }
        fs::rename(&part, &file)?;
        Ok(Some((relative.to_string_lossy().replace('\\', "/"), total)))
    }

    /// Writes the pages and the ComicInfo into `part`. False when the chapter stopped being wanted.
    async fn write(&self, job: &Job, total: i64, part: &Path) -> Result<bool> {
        let blocking = |e: tokio::task::JoinError| AppError::internal(e.to_string());
        let stored = || zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        if let Some(folder) = part.parent() {
            fs::create_dir_all(folder)?;
        }
        let archive = Arc::new(StdMutex::new(zip::ZipWriter::new(fs::File::create(part)?)));

        // Two pages are fetched at a time and written in order, so memory holds two pages at most.
        let mut next = 0;
        while next < total {
            if !self.wanted(job.chapter).await {
                return Ok(false);
            }
            let second = (next + 1 < total).then_some(next + 1);
            let (first, other) = tokio::join!(self.images.page(job.chapter, next), async {
                match second {
                    Some(index) => Some(self.images.page(job.chapter, index).await),
                    None => None,
                }
            });
            let mut pages = vec![first?];
            if let Some(page) = other {
                pages.push(page?);
            }
            let (writer, from) = (archive.clone(), next);
            next += pages.len() as i64;
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut zip = writer.lock().unwrap_or_else(|e| e.into_inner());
                for (offset, page) in pages.iter().enumerate() {
                    zip.start_file(format!("{:03}.{}", from + offset as i64 + 1, extension(page.content_type)), stored())?;
                    zip.write_all(&page.bytes)?;
                }
                Ok(())
            })
            .await
            .map_err(blocking)??;
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((job.chapter, next, total));
        }

        let info = comic_info(job, total);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut zip = Arc::try_unwrap(archive)
                .ok()
                .and_then(|lock| lock.into_inner().ok())
                .ok_or_else(|| AppError::internal("the archive is still in use"))?;
            zip.start_file("ComicInfo.xml", stored())?;
            zip.write_all(info.as_bytes())?;
            zip.finish()?.sync_all()?;
            Ok(())
        })
        .await
        .map_err(blocking)??;
        Ok(true)
    }

    /// Is this chapter still to be fetched? Not when it was cancelled, or the queue was paused.
    async fn wanted(&self, chapter: i64) -> bool {
        self.db
            .call(move |conn| {
                let queued = conn.query_row("SELECT 1 FROM downloads WHERE chapter_id = ?1", [chapter], |_| Ok(())).optional()?.is_some();
                Ok(queued && !settings::get::<bool>(conn, PAUSED).unwrap_or(false))
            })
            .await
            .unwrap_or(false)
    }

    /// Where a chapter's file goes, relative to the downloads folder.
    fn place(&self, job: &Job) -> PathBuf {
        place_in(&self.dir, job)
    }
}

pub(crate) fn place_in(dir: &Path, job: &Job) -> PathBuf {
    let folder = Path::new(&clean(&job.source)).join(clean(&job.title));
    let base_name = sanitize_release_name(&job.name);
    let name = match job.scanlator.as_deref().filter(|s| !s.is_empty()) {
        Some(scanlator) => clean(&format!("{base_name} [{scanlator}]")),
        None => clean(&base_name),
    };
    let path = folder.join(format!("{name}.cbz"));
    // Two chapters of one title can share a name; the second gets its id added.
    if dir.join(&path).exists() {
        return folder.join(format!("{name} ({}).cbz", job.chapter));
    }
    path
}

pub(crate) fn add(conn: &mut Connection, chapters: &[i64]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut added = 0;
    for chapter in chapters {
        added += tx.execute(
            "INSERT OR IGNORE INTO downloads (chapter_id, position, added_at)
             SELECT id, (SELECT COALESCE(MAX(position), 0) + 1 FROM downloads), ?2 FROM chapters WHERE id = ?1 AND download_path IS NULL",
            params![chapter, now()],
        )?;
    }
    tx.commit()?;
    Ok(added)
}

fn unread_of(conn: &Connection, titles: &[i64]) -> Result<Vec<i64>> {
    let list = titles.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM chapters WHERE manga_id IN ({list}) AND read = 0 AND download_path IS NULL ORDER BY manga_id, source_order DESC"
    ))?;
    let chapters = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(chapters)
}

pub(crate) fn read_queue(conn: &Connection) -> Result<QueueView> {
    let mut stmt = conn.prepare(
        "SELECT d.chapter_id, c.name, m.id, m.title, d.state, c.page_count, d.attempts, d.retry_at, d.error,
                COALESCE(s.name, 'Unknown source'), c.scanlator, c.chapter_number, d.published_path,
                d.source_type, d.backend_type, d.external_job_id, d.raw_title, d.uploader, d.release_size,
                d.collection_id, d.collection_name
         FROM downloads d
         JOIN chapters c ON c.id = d.chapter_id
         JOIN manga m ON m.id = c.manga_id
         LEFT JOIN sources s ON s.id = m.source_id
         ORDER BY d.state != 'downloading' AND d.state != 'verifying', d.position, d.chapter_id",
    )?;
    let queue = stmt
        .query_map([], |r| {
            let chapter_name: String = r.get(1)?;
            let chapter_number: f64 = r.get(11)?;
            let release_info = crate::release::parse_release(&chapter_name, chapter_number);
            let state: String = r.get(4)?;
            let error: Option<String> = r.get(8)?;
            let published_path: Option<String> = r.get(12)?;
            let can_retry_komga = (state == "failed" || state == "verifying")
                && (published_path.is_some() || error.as_deref().map(|e| e.to_lowercase().contains("komga")).unwrap_or(false));
            let failure_stage = if can_retry_komga {
                Some("komga".to_string())
            } else if state == "failed" {
                Some("download".to_string())
            } else {
                None
            };
            let status_message = if state == "verifying" {
                Some(error.clone().unwrap_or_else(|| "Published — waiting for Komga".to_string()))
            } else {
                None
            };
            Ok(Queued {
                chapter_id: r.get(0)?,
                chapter_name,
                manga_id: r.get(2)?,
                title: r.get(3)?,
                state: state.clone(),
                stage: state,
                status_message,
                destination: published_path.clone(),
                source: r.get(9)?,
                scanlator: r.get(10)?,
                release_type: Some(match release_info.release_type {
                    crate::release::ReleaseType::Volume => "volume".to_string(),
                    crate::release::ReleaseType::Chapter => "chapter".to_string(),
                }),
                volume: release_info.volume,
                done: 0,
                total: r.get(5)?,
                attempts: r.get(6)?,
                retry_at: r.get(7)?,
                error,
                can_retry_komga,
                failure_stage,
                published_path,
                source_type: r.get(13)?,
                backend_type: r.get(14)?,
                external_job_id: r.get(15)?,
                raw_title: r.get(16)?,
                uploader: r.get(17)?,
                release_size: r.get::<_, Option<i64>>(18)?.map(|s| s as u64),
                collection_id: r.get(19)?,
                collection_name: r.get(20)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(QueueView { paused: settings::get::<bool>(conn, PAUSED).unwrap_or(false), queue })
}

/// The next chapter that is due, marked as being fetched. None while paused or when nothing is due.
fn next_job(conn: &Connection) -> Result<Option<Job>> {
    if settings::get::<bool>(conn, PAUSED).unwrap_or(false) {
        return Ok(None);
    }
    let job_row = conn
        .query_row(
            "SELECT d.chapter_id, d.attempts, COALESCE(s.name, 'Unknown source'), m.title, c.name, c.chapter_number, c.scanlator,
                    m.author, m.artist, m.genre, m.description, s.lang, m.id, d.published_path, d.target_library_id, d.state,
                    d.source_type, d.backend_type, d.external_job_id, d.external_file_id, d.torrent_hash, d.raw_title,
                    d.uploader, d.release_size, d.retrieved_path, d.collection_id, d.collection_name
             FROM downloads d JOIN chapters c ON c.id = d.chapter_id JOIN manga m ON m.id = c.manga_id
             LEFT JOIN sources s ON s.id = m.source_id
             WHERE (d.state = 'queued' OR d.state = 'verifying') AND (d.retry_at IS NULL OR d.retry_at <= ?1)
             ORDER BY d.position, d.chapter_id LIMIT 1",
            [now()],
            |r| {
                let name: String = r.get(4)?;
                let number: f64 = r.get(5)?;
                let release_info = crate::release::parse_release(&name, number);
                let state: String = r.get(15)?;
                Ok((
                    Job {
                        chapter: r.get(0)?,
                        manga_id: r.get(12)?,
                        attempts: r.get(1)?,
                        source: r.get(2)?,
                        title: r.get(3)?,
                        name,
                        number,
                        scanlator: r.get(6)?,
                        author: r.get(7)?,
                        artist: r.get(8)?,
                        genre: r.get(9)?,
                        summary: r.get(10)?,
                        lang: r.get(11)?,
                        volume: release_info.volume,
                        release_type: release_info.release_type,
                        chapter_number: release_info.chapter_number,
                        published_path: r.get(13)?,
                        target_library_id: r.get(14)?,
                        source_type: r.get(16)?,
                        backend_type: r.get(17)?,
                        external_job_id: r.get(18)?,
                        external_file_id: r.get(19)?,
                        torrent_hash: r.get(20)?,
                        raw_title: r.get(21)?,
                        uploader: r.get(22)?,
                        release_size: r.get::<_, Option<i64>>(23)?.map(|s| s as u64),
                        retrieved_path: r.get(24)?,
                        collection_id: r.get(25)?,
                        collection_name: r.get(26)?,
                    },
                    state,
                ))
            },
        )
        .optional()?;

    if let Some((job, state)) = job_row {
        if state == "queued" && job.published_path.is_none() {
            conn.execute("UPDATE downloads SET state = 'downloading' WHERE chapter_id = ?1", [job.chapter])?;
        }
        return Ok(Some(job));
    }
    Ok(None)
}

/// Writes down how a fetch ended: saved, put back, or failed with a later try or none.
fn record(conn: &Connection, chapter: i64, attempts: i64, outcome: Result<Option<(String, i64)>>) -> Result<()> {
    // If the chapter was already published, do NOT overwrite its publication state or reset to download!
    let published_path: Option<String> = conn
        .query_row("SELECT published_path FROM downloads WHERE chapter_id = ?1", [chapter], |r| r.get(0))
        .optional()?
        .flatten();

    match outcome {
        Ok(Some((path, pages))) => {
            conn.execute("UPDATE chapters SET download_path = ?2, page_count = ?3 WHERE id = ?1", params![chapter, path, pages])?;
            conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter])?;
        }
        Ok(None) => {
            if published_path.is_none() {
                conn.execute("UPDATE downloads SET state = 'queued' WHERE chapter_id = ?1", [chapter])?;
            }
        }
        Err(e) => {
            let tried = attempts + 1;
            let (state, retry_at) = match WAITS.get(tried as usize - 1) {
                Some(wait) if tried < TRIES => {
                    if published_path.is_some() {
                        ("verifying", Some(now() + wait))
                    } else {
                        ("queued", Some(now() + wait))
                    }
                }
                _ => ("failed", None),
            };
            conn.execute(
                "UPDATE downloads SET state = ?2, attempts = ?3, retry_at = ?4, error = ?5 WHERE chapter_id = ?1",
                params![chapter, state, tried, retry_at, e.message],
            )?;
        }
    }
    Ok(())
}

/// Bytes the disk that holds `dir` has free for Mango. None where that is not known.
#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct it is given, which is plain numbers, and reads a
    // path that lives until it returns.
    let mut seen: libc::statvfs = unsafe { std::mem::zeroed() };
    let done = unsafe { libc::statvfs(path.as_ptr(), &mut seen) };
    #[allow(clippy::unnecessary_cast)]
    (done == 0).then(|| seen.f_bavail as u64 * seen.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_space(_: &Path) -> Option<u64> {
    None
}

/// Trims trailing colons, hyphens, underscores and extra whitespace from release names before
/// appending uploader brackets to avoid artifacts like "Chapter 1_  [Stick].cbz".
pub(crate) fn sanitize_release_name(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches([':', '-', '_']).trim();
    if trimmed.is_empty() {
        name.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// A name that is safe as a file or folder on Linux, macOS and Windows.
pub(crate) fn clean(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let safe: String = safe.trim().trim_end_matches('.').trim().chars().take(120).collect();
    let reserved = ["CON", "PRN", "AUX", "NUL"].iter().any(|word| safe.eq_ignore_ascii_case(word));
    match safe.as_str() {
        "" | "." | ".." => "_".into(),
        _ if reserved => format!("_{safe}"),
        _ => safe.trim_end().to_owned(),
    }
}

pub(crate) fn extension(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/jxl" => "jxl",
        "image/bmp" => "bmp",
        _ => "jpg",
    }
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The ComicInfo.xml other readers look for inside a CBZ.
fn comic_info(job: &Job, pages: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo>\n");
    let mut field = |tag: &str, value: Option<&str>| {
        if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
            xml.push_str(&format!("  <{tag}>{}</{tag}>\n", escape(value)));
        }
    };
    field("Title", Some(&job.name));
    field("Series", Some(&job.title));

    if let Some(vol) = job.volume {
        let vol_str = if vol.fract() == 0.0 {
            (vol as i64).to_string()
        } else {
            vol.to_string()
        };
        field("Volume", Some(&vol_str));
    }

    if job.release_type == crate::release::ReleaseType::Chapter {
        if let Some(num) = job.chapter_number {
            let num_str = if num.fract() == 0.0 {
                (num as i64).to_string()
            } else {
                num.to_string()
            };
            field("Number", Some(&num_str));
        } else if job.number >= 0.0 {
            let num_str = if job.number.fract() == 0.0 {
                (job.number as i64).to_string()
            } else {
                job.number.to_string()
            };
            field("Number", Some(&num_str));
        }
    }

    field("Summary", job.summary.as_deref());
    field("Writer", job.author.as_deref());
    field("Penciller", job.artist.as_deref());
    field("Genre", job.genre.as_deref());
    field("Translator", job.scanlator.as_deref());
    field("LanguageISO", job.lang.as_deref().filter(|lang| !matches!(*lang, "all" | "other")));
    field("PageCount", Some(&pages.to_string()));

    if !job.source.is_empty() && job.source != "Unknown source" {
        field("ScanInformation", Some(&job.source));
    }

    let notes = match &job.source {
        s if !s.is_empty() && s != "Unknown source" => format!("Saved by Mango · Source: {s}"),
        _ => "Saved by Mango".to_string(),
    };
    field("Notes", Some(&notes));

    xml.push_str("</ComicInfo>\n");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 7, '/a', 'Alpha: A/B?', 1)", []).unwrap();
        // Listed newest first: 12 is the oldest chapter.
        for (id, order, read) in [(10, 0, 0), (11, 1, 1), (12, 2, 0)] {
            conn.execute(
                "INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read) VALUES (?1, 1, ?1, 'Ch ' || ?1, ?2, 0, ?3)",
                params![id, order, read],
            )
            .unwrap();
        }
        conn
    }

    fn states(conn: &Connection) -> Vec<(i64, String, i64)> {
        read_queue(conn).unwrap().queue.into_iter().map(|q| (q.chapter_id, q.state, q.attempts)).collect()
    }

    #[test]
    fn queue_from_add_to_saved() {
        let mut conn = database();
        let unread = unread_of(&conn, &[1]).unwrap();
        assert_eq!(unread, [12, 10], "unread chapters, oldest first");
        assert_eq!(add(&mut conn, &unread).unwrap(), 2);
        assert_eq!(add(&mut conn, &[12, 999]).unwrap(), 0, "queued and unknown chapters are skipped");

        let job = next_job(&conn).unwrap().unwrap();
        assert_eq!((job.chapter, job.source.as_str()), (12, "Unknown source"));
        assert_eq!(states(&conn)[0], (12, "downloading".into(), 0));
        assert_eq!(next_job(&conn).unwrap().unwrap().chapter, 10, "the one being fetched is not handed out twice");

        // A failure waits before its next try; five in a row park the chapter.
        record(&conn, 12, 0, Err(AppError::new(502, "source_error", "boom"))).unwrap();
        let queue = read_queue(&conn).unwrap().queue;
        let failed = queue.iter().find(|q| q.chapter_id == 12).unwrap();
        assert_eq!((failed.state.as_str(), failed.attempts, failed.error.as_deref()), ("queued", 1, Some("boom")));
        assert!(failed.retry_at.unwrap() > now());
        record(&conn, 12, 4, Err(AppError::internal("still boom"))).unwrap();
        assert!(states(&conn).contains(&(12, "failed".into(), 5)));

        record(&conn, 10, 0, Ok(None)).unwrap();
        assert!(states(&conn).contains(&(10, "queued".into(), 0)), "a cancelled fetch goes back in line");
        record(&conn, 10, 0, Ok(Some(("Src/Alpha/Ch 10.cbz".into(), 30)))).unwrap();
        let saved: (Option<String>, Option<i64>) =
            conn.query_row("SELECT download_path, page_count FROM chapters WHERE id = 10", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(saved, (Some("Src/Alpha/Ch 10.cbz".into()), Some(30)));
        assert_eq!(states(&conn).len(), 1, "a saved chapter leaves the queue");
        assert_eq!(add(&mut conn, &[10]).unwrap(), 0, "and is not queued again");
    }

    #[test]
    fn pause_holds_the_queue() {
        let mut conn = database();
        add(&mut conn, &[10]).unwrap();
        settings::set(&conn, PAUSED, &true.into()).unwrap();
        assert!(next_job(&conn).unwrap().is_none());
        assert!(read_queue(&conn).unwrap().paused);
        settings::set(&conn, PAUSED, &false.into()).unwrap();
        assert!(next_job(&conn).unwrap().is_some());
    }

    #[test]
    fn names_and_comic_info() {
        assert_eq!(clean("Alpha: A/B?"), "Alpha_ A_B_");
        assert_eq!(clean("  dots... "), "dots");
        assert_eq!(clean(".."), "_");
        assert_eq!(clean("nul"), "_nul");
        assert_eq!(clean(&"x".repeat(300)).len(), 120);

        let job = Job {
            chapter: 1,
            manga_id: 1,
            attempts: 0,
            source: "Src".into(),
            title: "Tom & Jerry <3".into(),
            name: "Ch \"1\"".into(),
            number: 1.5,
            scanlator: Some(String::new()),
            author: Some("A".into()),
            artist: None,
            genre: None,
            summary: None,
            lang: Some("all".into()),
            volume: None,
            release_type: crate::release::ReleaseType::Chapter,
            chapter_number: Some(1.5),
            published_path: None,
            target_library_id: None,
            ..Default::default()
        };
        let xml = comic_info(&job, 12);
        assert!(xml.contains("<Series>Tom &amp; Jerry &lt;3</Series>"));
        assert!(xml.contains("<Title>Ch &quot;1&quot;</Title>"));
        assert!(xml.contains("<Number>1.5</Number>") && xml.contains("<PageCount>12</PageCount>"));
        assert!(!xml.contains("Translator") && !xml.contains("LanguageISO"), "empty fields are left out");
        assert!(xml.contains("<ScanInformation>Src</ScanInformation>"));
        assert!(xml.contains("<Notes>Saved by Mango · Source: Src</Notes>"));
    }

    #[test]
    fn volume_and_chapter_comic_info_and_clean_naming() {
        assert_eq!(sanitize_release_name("Chapter 1: "), "Chapter 1");
        assert_eq!(sanitize_release_name("Chapter 1 - "), "Chapter 1");
        assert_eq!(sanitize_release_name("Chapter 1: The Beginning"), "Chapter 1: The Beginning");

        // Case A: Pure Volume artifact
        let vol_job = Job {
            chapter: 10,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Blue Lock".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: Some("Stick".into()),
            author: Some("Muneyuki Kaneshiro".into()),
            artist: Some("Yusuke Nomura".into()),
            genre: Some("Sports, Shounen".into()),
            summary: Some("High school soccer".into()),
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
            ..Default::default()
        };
        let vol_xml = comic_info(&vol_job, 192);
        assert!(vol_xml.contains("<Volume>1</Volume>"));
        assert!(!vol_xml.contains("<Number>"), "Volume artifact must not emit fake chapter Number");
        assert!(vol_xml.contains("<Translator>Stick</Translator>"));
        assert!(vol_xml.contains("<ScanInformation>MangaDot</ScanInformation>"));
        assert!(vol_xml.contains("<Notes>Saved by Mango · Source: MangaDot</Notes>"));

        // Case B: Chapter belonging to a volume
        let ch_vol_job = Job {
            chapter: 11,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Blue Lock".into(),
            name: "Vol. 1 Ch. 5".into(),
            number: 5.0,
            scanlator: Some("Stick".into()),
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Chapter,
            chapter_number: Some(5.0),
            published_path: None,
            target_library_id: None,
            ..Default::default()
        };
        let ch_vol_xml = comic_info(&ch_vol_job, 24);
        assert!(ch_vol_xml.contains("<Volume>1</Volume>"));
        assert!(ch_vol_xml.contains("<Number>5</Number>"));
        assert!(ch_vol_xml.contains("<Title>Vol. 1 Ch. 5</Title>"));

        // Case C: Pure chapter (MangaDot style with colon)
        let ch_job = Job {
            chapter: 12,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Blue Lock".into(),
            name: "Chapter 1: ".into(),
            number: 1.0,
            scanlator: Some("Stick".into()),
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: None,
            release_type: crate::release::ReleaseType::Chapter,
            chapter_number: Some(1.0),
            published_path: None,
            target_library_id: None,
            ..Default::default()
        };
        let ch_xml = comic_info(&ch_job, 75);
        assert!(ch_xml.contains("<Number>1</Number>"));
        assert!(!ch_xml.contains("<Volume>"), "Pure chapter must not emit Volume");

        // Filename verification for clean naming
        let place = place_in(Path::new("/tmp"), &ch_job);
        assert_eq!(place.file_name().unwrap().to_str().unwrap(), "Chapter 1 [Stick].cbz");

        let vol_place = place_in(Path::new("/tmp"), &vol_job);
        assert_eq!(vol_place.file_name().unwrap().to_str().unwrap(), "Volume 1 [Stick].cbz");

        // Case D: Fractional Volume artifact (e.g. v12.5)
        let frac_job = Job {
            chapter: 13,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Blue Lock".into(),
            name: "Series v12.5".into(),
            number: 12.5,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(12.5),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
            ..Default::default()
        };
        let frac_xml = comic_info(&frac_job, 100);
        assert!(frac_xml.contains("<Volume>12.5</Volume>"));
        assert!(!frac_xml.contains("<Number>"), "Fractional volume must not emit fake chapter Number");
        assert_eq!(
            crate::pipeline::publisher::canonical_release_name(&frac_job.title, true, 12.5),
            "Blue Lock v12.5"
        );
        assert_eq!(
            crate::pipeline::publisher::canonical_release_name(&vol_job.title, true, 1.0),
            "Blue Lock v01"
        );
    }

    #[test]
    fn queue_lifecycle_fields_and_komga_retry_detection() {
        let mut conn = database();
        add(&mut conn, &[10, 11, 12]).unwrap();

        // 1. Mark chapter 10 as downloading
        conn.execute("UPDATE downloads SET state = 'downloading' WHERE chapter_id = 10", []).unwrap();

        // 2. Mark chapter 11 as failed with generic error
        conn.execute(
            "UPDATE downloads SET state = 'failed', attempts = 5, error = 'Network connection timed out' WHERE chapter_id = 11",
            [],
        )
        .unwrap();

        // 3. Mark chapter 12 as failed specifically with Komga scan error
        conn.execute(
            "UPDATE downloads SET state = 'failed', attempts = 1, error = 'Published to /srv/media/manga, but Komga scan trigger failed: connection refused' WHERE chapter_id = 12",
            [],
        )
        .unwrap();

        let queue_view = read_queue(&conn).unwrap();
        assert_eq!(queue_view.queue.len(), 3);

        let q10 = queue_view.queue.iter().find(|q| q.chapter_id == 10).unwrap();
        assert_eq!(q10.state, "downloading");
        assert_eq!(q10.can_retry_komga, false);
        assert_eq!(q10.failure_stage, None);

        let q11 = queue_view.queue.iter().find(|q| q.chapter_id == 11).unwrap();
        assert_eq!(q11.state, "failed");
        assert_eq!(q11.can_retry_komga, false);
        assert_eq!(q11.failure_stage.as_deref(), Some("download"));

        let q12 = queue_view.queue.iter().find(|q| q.chapter_id == 12).unwrap();
        assert_eq!(q12.state, "failed");
        assert_eq!(q12.can_retry_komga, true);
        assert_eq!(q12.failure_stage.as_deref(), Some("komga"));
    }

    fn make_test_cbz(path: &Path, content: &str) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(format!("<ComicInfo><Series>{content}</Series></ComicInfo>").as_bytes()).unwrap();
        zip.start_file("001.webp", options).unwrap();
        zip.write_all(b"fake image bytes").unwrap();
        zip.finish().unwrap();
    }

    fn test_images(db: Db, dir: &Path) -> Images {
        let host = crate::host::Host::new(crate::config::HostConfig::default(), dir.join("host_bin"), dir).unwrap();
        let local = crate::local::Local::new(dir.join("local"));
        Images::new(db, host, local, dir, 10)
    }

    #[tokio::test]
    async fn test_scenario_e_retry_komga_does_not_redownload_or_republish() {
        use axum::{routing::{get, post}, Json, Router};
        use serde_json::json;

        let pid = std::process::id();
        let nanos = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("mango-test-retry-e-{pid}-{nanos}"));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let db = Db::open(&temp_dir.join("test.db")).unwrap();
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/manga', 'Retry Series', 1)", [])?;
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at, read) VALUES (10, 1, '/c10', 'Volume 1', 1.0, 1, 0, 0)", [])?;
            Ok(())
        }).await.unwrap();

        let lib_dir = temp_dir.join("manga");
        let series_dir = lib_dir.join("Retry Series");
        fs::create_dir_all(&series_dir).unwrap();
        let published_cbz = series_dir.join("Retry Series v01.cbz");
        make_test_cbz(&published_cbz, "Retry Series");
        let original_meta = fs::metadata(&published_cbz).unwrap();

        let app = Router::new()
            .route(
                "/api/v1/libraries/lib-en/scan",
                post(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-ret",
                                "name": "Retry Series",
                                "libraryId": "lib-en",
                                "booksCount": 1
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-ret/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "book-ret",
                                "seriesId": "series-ret",
                                "name": "Retry Series v01",
                                "number": 1.0,
                                "metadata": {
                                    "title": "Retry Series v01",
                                    "number": "1",
                                    "numberSort": 1.0
                                }
                            }
                        ]
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let pipeline = Arc::new(crate::pipeline::Pipeline::new(db.clone()));
        let mut config = crate::pipeline::PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(lib_dir.to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_scan_enabled = true;
        config.komga_library_id = Some("lib-en".into());
        config.komga_url = Some(format!("http://127.0.0.1:{port}"));
        config.mangabaka_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        let images = test_images(db.clone(), &temp_dir);
        let mut downloads = Downloads::new(db.clone(), images, &temp_dir, 0);
        downloads.set_pipeline(pipeline);

        let pub_str = published_cbz.to_string_lossy().to_string();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, published_path, target_library_id, error)
                 VALUES (10, 'failed', 1, 1000, ?1, 'lib-en', 'Komga scan failed previously')",
                rusqlite::params![pub_str],
            )?;
            conn.execute("UPDATE chapters SET download_path = ?1 WHERE id = 10", rusqlite::params![pub_str])?;
            Ok(())
        }).await.unwrap();

        let res = downloads.retry_komga(10).await.unwrap();
        assert!(res.contains("Ownership verified in Komga — acquisition complete"));

        let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads WHERE chapter_id = 10", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(dl_count, 0);

        assert!(published_cbz.exists());
        let final_meta = fs::metadata(&published_cbz).unwrap();
        assert_eq!(final_meta.len(), original_meta.len());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_scenario_f_process_restart_during_ownership_verification_resumes() {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("mango-test-restart-f-{pid}-{nanos}"));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let db = Db::open(&temp_dir.join("test.db")).unwrap();
        let lib_dir = temp_dir.join("manga");
        fs::create_dir_all(&lib_dir).unwrap();
        let published_cbz = lib_dir.join("Existing Series v01.cbz");
        make_test_cbz(&published_cbz, "Existing Series");

        let pub_str = published_cbz.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m', 'Series', 1)", [])?;
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at, read) VALUES (1, 1, '/c1', 'Vol 1', 1.0, 1, 0, 0)", [])?;
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at, read) VALUES (2, 1, '/c2', 'Vol 2', 2.0, 2, 0, 0)", [])?;

            // Item 1: Already published, interrupted in downloading
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, published_path, target_library_id)
                 VALUES (1, 'downloading', 1, 1000, ?1, 'lib-en')",
                rusqlite::params![pub_str],
            )?;

            // Item 2: Staging download, interrupted during download (no published_path)
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, published_path)
                 VALUES (2, 'downloading', 2, 1000, NULL)",
                [],
            )?;
            Ok(())
        }).await.unwrap();

        // Run the startup recovery logic as defined in Downloads::run()
        db.call(|conn| {
            let mut stmt = conn.prepare("SELECT chapter_id, published_path FROM downloads WHERE published_path IS NOT NULL")?;
            let published_items = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .filter_map(std::result::Result::ok)
                .collect::<Vec<_>>();

            for (ch_id, pub_path) in published_items {
                let path = PathBuf::from(&pub_path);
                if path.is_file() && crate::pipeline::publisher::verify_cbz_integrity(&path).is_ok() {
                    conn.execute(
                        "UPDATE downloads SET state = 'verifying', error = 'Published — waiting for Komga' WHERE chapter_id = ?1",
                        [ch_id],
                    )?;
                } else {
                    conn.execute(
                        "UPDATE downloads SET published_path = NULL, state = 'queued', error = NULL WHERE chapter_id = ?1",
                        [ch_id],
                    )?;
                }
            }

            conn.execute("UPDATE downloads SET state = 'queued' WHERE state = 'downloading' AND published_path IS NULL", [])?;
            Ok(())
        }).await.unwrap();

        let (state1, pub1, state2, pub2): (String, Option<String>, String, Option<String>) = db.call(|conn| {
            let (s1, p1) = conn.query_row("SELECT state, published_path FROM downloads WHERE chapter_id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let (s2, p2) = conn.query_row("SELECT state, published_path FROM downloads WHERE chapter_id = 2", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok((s1, p1, s2, p2))
        }).await.unwrap();

        assert_eq!(state1, "verifying");
        assert_eq!(pub1.as_deref(), Some(published_cbz.to_str().unwrap()));

        assert_eq!(state2, "queued");
        assert_eq!(pub2, None);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_scenario_j_cancellation_after_publication_preserves_canonical_artifact() {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("mango-test-cancel-j-{pid}-{nanos}"));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let db = Db::open(&temp_dir.join("test.db")).unwrap();
        let lib_dir = temp_dir.join("manga");
        fs::create_dir_all(&lib_dir).unwrap();
        let published_cbz = lib_dir.join("Preserved Series v01.cbz");
        make_test_cbz(&published_cbz, "Preserved Series");

        let pub_str = published_cbz.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m', 'Preserved Series', 1)", [])?;
            conn.execute(
                "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at, read, download_path)
                 VALUES (10, 1, '/c10', 'Volume 1', 1.0, 1, 0, 0, ?1)",
                rusqlite::params![pub_str],
            )?;
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, published_path, target_library_id)
                 VALUES (10, 'verifying', 1, 1000, ?1, 'lib-en')",
                rusqlite::params![pub_str],
            )?;
            Ok(())
        }).await.unwrap();

        let images = test_images(db.clone(), &temp_dir);
        let downloads = Downloads::new(db.clone(), images, &temp_dir, 0);

        downloads.cancel(Some(10)).await.unwrap();

        let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads WHERE chapter_id = 10", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(dl_count, 0);

        downloads.delete(10).await.unwrap();

        assert!(published_cbz.is_file(), "Canonical library artifact is never deleted on cancellation");
        assert!(crate::pipeline::publisher::verify_cbz_integrity(&published_cbz).is_ok());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
