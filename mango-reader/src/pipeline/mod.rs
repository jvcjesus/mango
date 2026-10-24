//! Native Acquisition, Enrichment, and Publishing Pipeline.
//!
//! Orchestrates the transition of manga acquisitions from Mango into the canonical Komga library:
//! 1. MangaBaka metadata enrichment (publication date, volume summary, creators, genres)
//! 2. Non-destructive image optimization via RustyLibraryShrinker with page-count validation
//! 3. Safe, atomic publication into canonical folder structures with duplicate and conflict protection
//! 4. Automatic Komga library scan triggers
//! 5. Detailed in-memory audit trail of recent pipeline executions

pub mod komga;
pub mod mangabaka;
pub mod optimizer;
pub mod publisher;

use crate::db::Db;
use crate::error::{AppError, Result};
use komga::KomgaClient;
use mangabaka::MangaBakaClient;
use optimizer::optimize_cbz;
use publisher::{publish_cbz, PublishOutcome};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::SystemTime,
};

pub const DEFAULT_MIN_SAVINGS_PERCENT: u32 = 10;
const MAX_RECENT_EVENTS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub enabled: bool,
    pub library_dir: Option<String>,
    pub chinese_library_dir: Option<String>,
    pub mangabaka_enabled: bool,
    pub mangabaka_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mangabaka_token: Option<String>,
    pub has_mangabaka_token: bool,
    pub komga_enabled: bool,
    pub komga_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub komga_api_key: Option<String>,
    pub has_komga_api_key: bool,
    pub komga_library_id: Option<String>,
    pub optimize_enabled: bool,
    pub optimizer_bin: Option<String>,
    pub min_savings_percent: u32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            library_dir: None,
            chinese_library_dir: None,
            mangabaka_enabled: true,
            mangabaka_url: Some(mangabaka::DEFAULT_MANGABAKA_API.to_string()),
            mangabaka_token: None,
            has_mangabaka_token: false,
            komga_enabled: false,
            komga_url: Some(komga::DEFAULT_KOMGA_URL.to_string()),
            komga_api_key: None,
            has_komga_api_key: false,
            komga_library_id: None,
            optimize_enabled: false,
            optimizer_bin: None,
            min_savings_percent: DEFAULT_MIN_SAVINGS_PERCENT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineEvent {
    pub id: String,
    pub timestamp: u64,
    pub chapter_id: i64,
    pub series: String,
    pub release_name: String,
    pub is_volume: bool,
    pub number: f64,
    pub outcome: String, // "published", "already_published", "conflict", "skipped", "error"
    pub destination: Option<String>,
    pub mangabaka_status: String,
    pub optimizer_status: String,
    pub komga_status: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Pipeline {
    db: Db,
    http: Client,
    events: Arc<RwLock<VecDeque<PipelineEvent>>>,
}

impl Pipeline {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            events: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_RECENT_EVENTS))),
        }
    }

    /// Loads the active pipeline configuration from settings and state.
    pub async fn load_config(&self) -> Result<PipelineConfig> {
        self.db
            .call(|conn| {
                let enabled = crate::settings::get::<bool>(conn, "pipeline.enabled").unwrap_or(false);
                let library_dir = crate::settings::get::<String>(conn, "pipeline.library_dir");
                let chinese_library_dir = crate::settings::get::<String>(conn, "pipeline.chinese_library_dir");
                let mangabaka_enabled = crate::settings::get::<bool>(conn, "pipeline.mangabaka_enabled").unwrap_or(true);
                let mangabaka_url = crate::settings::get::<String>(conn, "pipeline.mangabaka_url")
                    .or_else(|| Some(mangabaka::DEFAULT_MANGABAKA_API.to_string()));
                let komga_enabled = crate::settings::get::<bool>(conn, "pipeline.komga_enabled").unwrap_or(false);
                let komga_url = crate::settings::get::<String>(conn, "pipeline.komga_url")
                    .or_else(|| Some(komga::DEFAULT_KOMGA_URL.to_string()));
                let komga_library_id = crate::settings::get::<String>(conn, "pipeline.komga_library_id");
                let optimize_enabled = crate::settings::get::<bool>(conn, "pipeline.optimize_enabled").unwrap_or(false);
                let optimizer_bin = crate::settings::get::<String>(conn, "pipeline.optimizer_bin");
                let min_savings_percent = crate::settings::get::<u32>(conn, "pipeline.min_savings_percent")
                    .unwrap_or(DEFAULT_MIN_SAVINGS_PERCENT);

                let mangabaka_token = crate::db::state(conn, "pipeline.mangabaka_token")?;
                let komga_api_key = crate::db::state(conn, "pipeline.komga_api_key")?;

                Ok(PipelineConfig {
                    enabled,
                    library_dir,
                    chinese_library_dir,
                    mangabaka_enabled,
                    mangabaka_url,
                    mangabaka_token: None, // Secret redacted in config view
                    has_mangabaka_token: mangabaka_token.is_some_and(|t| !t.trim().is_empty()),
                    komga_enabled,
                    komga_url,
                    komga_api_key: None, // Secret redacted in config view
                    has_komga_api_key: komga_api_key.is_some_and(|k| !k.trim().is_empty()),
                    komga_library_id,
                    optimize_enabled,
                    optimizer_bin,
                    min_savings_percent,
                })
            })
            .await
    }

    /// Internal helper to load configuration with secrets included for execution.
    async fn load_internal_config(&self) -> Result<(PipelineConfig, Option<String>, Option<String>)> {
        self.db
            .call(|conn| {
                let enabled = crate::settings::get::<bool>(conn, "pipeline.enabled").unwrap_or(false);
                let library_dir = crate::settings::get::<String>(conn, "pipeline.library_dir");
                let chinese_library_dir = crate::settings::get::<String>(conn, "pipeline.chinese_library_dir");
                let mangabaka_enabled = crate::settings::get::<bool>(conn, "pipeline.mangabaka_enabled").unwrap_or(true);
                let mangabaka_url = crate::settings::get::<String>(conn, "pipeline.mangabaka_url")
                    .or_else(|| Some(mangabaka::DEFAULT_MANGABAKA_API.to_string()));
                let komga_enabled = crate::settings::get::<bool>(conn, "pipeline.komga_enabled").unwrap_or(false);
                let komga_url = crate::settings::get::<String>(conn, "pipeline.komga_url")
                    .or_else(|| Some(komga::DEFAULT_KOMGA_URL.to_string()));
                let komga_library_id = crate::settings::get::<String>(conn, "pipeline.komga_library_id");
                let optimize_enabled = crate::settings::get::<bool>(conn, "pipeline.optimize_enabled").unwrap_or(false);
                let optimizer_bin = crate::settings::get::<String>(conn, "pipeline.optimizer_bin");
                let min_savings_percent = crate::settings::get::<u32>(conn, "pipeline.min_savings_percent")
                    .unwrap_or(DEFAULT_MIN_SAVINGS_PERCENT);

                let mangabaka_token = crate::db::state(conn, "pipeline.mangabaka_token")?;
                let komga_api_key = crate::db::state(conn, "pipeline.komga_api_key")?;

                let cfg = PipelineConfig {
                    enabled,
                    library_dir,
                    chinese_library_dir,
                    mangabaka_enabled,
                    mangabaka_url,
                    mangabaka_token: None,
                    has_mangabaka_token: mangabaka_token.is_some(),
                    komga_enabled,
                    komga_url,
                    komga_api_key: None,
                    has_komga_api_key: komga_api_key.is_some(),
                    komga_library_id,
                    optimize_enabled,
                    optimizer_bin,
                    min_savings_percent,
                };

                Ok((cfg, mangabaka_token, komga_api_key))
            })
            .await
    }

    /// Saves updated pipeline configuration and tokens.
    pub async fn save_config(&self, patch: PipelineConfig) -> Result<()> {
        self.db
            .call(move |conn| {
                let mut map = serde_json::Map::new();

                map.insert("pipeline.enabled".into(), serde_json::json!(patch.enabled));
                map.insert("pipeline.library_dir".into(), serde_json::json!(patch.library_dir));
                map.insert("pipeline.chinese_library_dir".into(), serde_json::json!(patch.chinese_library_dir));
                map.insert("pipeline.mangabaka_enabled".into(), serde_json::json!(patch.mangabaka_enabled));
                map.insert("pipeline.mangabaka_url".into(), serde_json::json!(patch.mangabaka_url));
                map.insert("pipeline.komga_enabled".into(), serde_json::json!(patch.komga_enabled));
                map.insert("pipeline.komga_url".into(), serde_json::json!(patch.komga_url));
                map.insert("pipeline.komga_library_id".into(), serde_json::json!(patch.komga_library_id));
                map.insert("pipeline.optimize_enabled".into(), serde_json::json!(patch.optimize_enabled));
                map.insert("pipeline.optimizer_bin".into(), serde_json::json!(patch.optimizer_bin));
                map.insert("pipeline.min_savings_percent".into(), serde_json::json!(patch.min_savings_percent));

                crate::settings::put(conn, &map)?;

                // Update secrets in state table if provided (non-empty and not masked placeholder)
                if let Some(token) = patch.mangabaka_token {
                    let trimmed = token.trim();
                    if trimmed == "CLEAR" {
                        crate::db::forget_state(conn, "pipeline.mangabaka_token")?;
                    } else if !trimmed.is_empty() && trimmed != "********" {
                        crate::db::set_state(conn, "pipeline.mangabaka_token", trimmed)?;
                    }
                }

                if let Some(key) = patch.komga_api_key {
                    let trimmed = key.trim();
                    if trimmed == "CLEAR" {
                        crate::db::forget_state(conn, "pipeline.komga_api_key")?;
                    } else if !trimmed.is_empty() && trimmed != "********" {
                        crate::db::set_state(conn, "pipeline.komga_api_key", trimmed)?;
                    }
                }

                Ok(())
            })
            .await
    }

    /// Tests connection to MangaBaka API.
    pub async fn test_mangabaka(&self, url: Option<String>, token: Option<String>) -> Result<String> {
        let actual_token = match token {
            Some(t) if t.trim() == "********" || t.trim().is_empty() => {
                self.db.call(|conn| crate::db::state(conn, "pipeline.mangabaka_token")).await?
            }
            Some(t) => Some(t),
            None => self.db.call(|conn| crate::db::state(conn, "pipeline.mangabaka_token")).await?,
        };

        let client = MangaBakaClient::new(self.http.clone(), url, actual_token);
        client.test_connection().await
    }

    /// Tests connection to Komga server.
    pub async fn test_komga(&self, url: Option<String>, api_key: Option<String>) -> Result<String> {
        let actual_key = match api_key {
            Some(k) if k.trim() == "********" || k.trim().is_empty() => {
                self.db.call(|conn| crate::db::state(conn, "pipeline.komga_api_key")).await?
            }
            Some(k) => Some(k),
            None => self.db.call(|conn| crate::db::state(conn, "pipeline.komga_api_key")).await?,
        };

        let client = KomgaClient::new(self.http.clone(), url, actual_key);
        client.test_connection().await
    }

    /// Returns the audit history of recent pipeline runs.
    pub fn recent_events(&self) -> Vec<PipelineEvent> {
        self.events
            .read()
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn record_event(&self, event: PipelineEvent) {
        if let Ok(mut q) = self.events.write() {
            if q.len() >= MAX_RECENT_EVENTS {
                q.pop_back();
            }
            q.push_front(event);
        }
    }

    /// Executes the full native acquisition post-processing pipeline for a downloaded chapter/volume.
    pub async fn process_download(&self, source_cbz: &Path, job: &crate::downloads::Job) -> Result<()> {
        let (config, mb_token, komga_key) = self.load_internal_config().await?;

        if !config.enabled {
            return Ok(());
        }

        let is_volume = job.release_type == crate::release::ReleaseType::Volume;
        let release_num = if is_volume {
            job.volume.unwrap_or(job.number)
        } else {
            job.chapter_number.unwrap_or(job.number)
        };

        let release_name = publisher::canonical_release_name(&job.title, is_volume, release_num);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 1. Determine destination permanent library root
        let is_chinese = job.lang.as_deref().map(|l| l.to_lowercase()).map(|l| l == "zh" || l.contains("chinese") || l.contains("mandarin")).unwrap_or(false);
        let dest_root = if is_chinese && config.chinese_library_dir.as_deref().is_some_and(|p| !p.trim().is_empty()) {
            PathBuf::from(config.chinese_library_dir.as_ref().unwrap())
        } else if let Some(dir) = &config.library_dir {
            PathBuf::from(dir)
        } else {
            let err = "Permanent library directory is not configured in Mango Pipeline settings";
            tracing::warn!("PIPELINE: {err}");
            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: "error".to_string(),
                destination: None,
                mangabaka_status: "skipped".to_string(),
                optimizer_status: "skipped".to_string(),
                komga_status: "skipped".to_string(),
                error: Some(err.to_string()),
            });
            return Err(AppError::bad_request(err));
        };

        if !dest_root.exists() {
            let err = format!("Configured library path does not exist: {}", dest_root.display());
            tracing::warn!("PIPELINE: {err}");
            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: "error".to_string(),
                destination: None,
                mangabaka_status: "skipped".to_string(),
                optimizer_status: "skipped".to_string(),
                komga_status: "skipped".to_string(),
                error: Some(err.clone()),
            });
            return Err(AppError::bad_request(err));
        }

        // 2. MangaBaka Enrichment (for volumes)
        let mut mangabaka_status = "skipped".to_string();
        if config.mangabaka_enabled && is_volume {
            let mb_client = MangaBakaClient::new(self.http.clone(), config.mangabaka_url.clone(), mb_token);
            match mb_client.fetch_volume_enrichment(&job.title, release_num).await {
                Ok(Some(enrichment)) => {
                    match MangaBakaClient::enrich_cbz_archive(source_cbz, &enrichment) {
                        Ok(true) => mangabaka_status = "enriched".to_string(),
                        Ok(false) => mangabaka_status = "enrichment skipped (no ComicInfo)".to_string(),
                        Err(e) => mangabaka_status = format!("enrichment failed: {e}"),
                    }
                }
                Ok(None) => mangabaka_status = "not found in catalog".to_string(),
                Err(e) => mangabaka_status = format!("catalog search failed: {e}"),
            }
        }

        // 3. Non-destructive Image Optimization
        let mut optimizer_status = "skipped".to_string();
        if config.optimize_enabled {
            match optimize_cbz(source_cbz, config.optimizer_bin.as_deref(), config.min_savings_percent).await {
                Ok(res) => optimizer_status = res.reason,
                Err(e) => optimizer_status = format!("optimization error: {e}"),
            }
        }

        // 4. Safe, Atomic Publication into Komga Library
        let pub_res = publish_cbz(source_cbz, &dest_root, &job.title, is_volume, release_num);
        let (outcome_str, dest_path_str, publish_err) = match &pub_res {
            Ok(PublishOutcome::Published(p)) => ("published", Some(p.to_string_lossy().to_string()), None),
            Ok(PublishOutcome::AlreadyPublished(p)) => ("already_published", Some(p.to_string_lossy().to_string()), None),
            Err(e) if e.code == "destination_conflict" => ("conflict", None, Some(e.message.clone())),
            Err(e) => ("error", None, Some(e.message.clone())),
        };

        // 5. Automatic Komga Library Rescan Trigger
        let mut komga_status = "skipped".to_string();
        if config.komga_enabled && pub_res.is_ok() {
            let komga_client = KomgaClient::new(self.http.clone(), config.komga_url.clone(), komga_key);
            match komga_client.trigger_scan(config.komga_library_id.as_deref()).await {
                Ok(()) => komga_status = "scan triggered".to_string(),
                Err(e) => komga_status = format!("scan trigger failed: {e}"),
            }
        }

        // 6. Record Audit Event
        self.record_event(PipelineEvent {
            id: format!("{}-{}", job.chapter, now),
            timestamp: now,
            chapter_id: job.chapter,
            series: job.title.clone(),
            release_name,
            is_volume,
            number: release_num,
            outcome: outcome_str.to_string(),
            destination: dest_path_str,
            mangabaka_status,
            optimizer_status,
            komga_status,
            error: publish_err,
        });

        // Fail-safe: if publication failed (e.g. conflict), return error so downloads worker knows,
        // but staging download remains preserved in Mango data directory.
        pub_res.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;

    use std::sync::atomic::{AtomicU64, Ordering};
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_test_cbz(path: &Path, content: &str) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(format!("<ComicInfo><Series>{content}</Series></ComicInfo>").as_bytes()).unwrap();
        zip.start_file("001.webp", options).unwrap();
        zip.write_all(b"fake image bytes").unwrap();
        zip.finish().unwrap();
    }

    fn test_db() -> Db {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mango-pipetest-db-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Db::open(&dir.join("test.db")).unwrap()
    }

    #[test]
    fn pipeline_config_defaults() {
        let cfg = PipelineConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.mangabaka_enabled);
        assert_eq!(cfg.min_savings_percent, 10);
    }

    #[tokio::test]
    async fn pipeline_config_round_trip_and_secret_masking() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some("/srv/media/manga".into());
        config.komga_enabled = true;
        config.komga_api_key = Some("super_secret_komga_key".into());

        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.library_dir.as_deref(), Some("/srv/media/manga"));
        assert!(loaded.komga_enabled);
        assert!(loaded.has_komga_api_key);
        // Secret must not be exposed in user-facing config
        assert_eq!(loaded.komga_api_key, None);

        // Internal loader has access to actual secret
        let (_, _, secret_key) = pipeline.load_internal_config().await.unwrap();
        assert_eq!(secret_key.as_deref(), Some("super_secret_komga_key"));
    }

    #[tokio::test]
    async fn pipeline_publication_and_chinese_routing() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let test_root = std::env::temp_dir().join(format!("mango-pipetest-routes-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&test_root);
        let eng_library = test_root.join("manga");
        let zh_library = test_root.join("manga-chinese");
        let staging_dir = test_root.join("staging");
        fs::create_dir_all(&eng_library).unwrap();
        fs::create_dir_all(&zh_library).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(eng_library.to_string_lossy().to_string());
        config.chinese_library_dir = Some(zh_library.to_string_lossy().to_string());
        config.mangabaka_enabled = false; // Disable network call in unit test
        config.komga_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        // 1. Process English Volume
        let eng_cbz = staging_dir.join("eng_vol1.cbz");
        make_test_cbz(&eng_cbz, "English Series");
        let eng_job = crate::downloads::Job {
            chapter: 101,
            attempts: 0,
            source: "MangaDot".into(),
            title: "English Series".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: Some("Stick".into()),
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
        };

        pipeline.process_download(&eng_cbz, &eng_job).await.unwrap();
        let expected_eng_path = eng_library.join("English Series").join("English Series v01.cbz");
        assert!(expected_eng_path.exists(), "English volume published to default library");

        // 2. Process Chinese Volume -> routes to chinese_library_dir
        let zh_cbz = staging_dir.join("zh_vol2.cbz");
        make_test_cbz(&zh_cbz, "Chinese Series");
        let zh_job = crate::downloads::Job {
            chapter: 102,
            attempts: 0,
            source: "Bilibili".into(),
            title: "Chinese Series".into(),
            name: "Volume 2".into(),
            number: 2.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("zh".into()),
            volume: Some(2.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
        };

        pipeline.process_download(&zh_cbz, &zh_job).await.unwrap();
        let expected_zh_path = zh_library.join("Chinese Series").join("Chinese Series v02.cbz");
        assert!(expected_zh_path.exists(), "Chinese volume published to chinese_library_dir");

        // 3. Duplicate protection: re-running identical file succeeds as a safe no-op
        pipeline.process_download(&eng_cbz, &eng_job).await.unwrap();

        // 4. Refuse silent overwrite: different file for same volume produces conflict
        let diff_cbz = staging_dir.join("diff_vol1.cbz");
        make_test_cbz(&diff_cbz, "Completely Different Release Content");
        let err = pipeline.process_download(&diff_cbz, &eng_job).await.unwrap_err();
        assert_eq!(err.code, "destination_conflict");

        // Verify events recorded in audit log
        let events = pipeline.recent_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].outcome, "conflict");
        assert_eq!(events[1].outcome, "already_published");

        let _ = fs::remove_dir_all(&test_root);
    }
}
