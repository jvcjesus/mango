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
pub mod ownership;
pub mod publisher;
pub mod validation;

use crate::db::Db;
use crate::error::{AppError, Result};
use komga::KomgaClient;
use mangabaka::MangaBakaClient;
use std::fs;
#[allow(unused_imports)]
pub use optimizer::{
    check_executable_invocable, count_cbz_pages, is_executable, optimize_cbz, resolve_optimizer_binary,
    validate_optimizer, BUNDLED_OPTIMIZER_PATH, DEFAULT_IMAGE_QUALITY, DEFAULT_MIN_SAVINGS_PERCENT,
    DEFAULT_TARGET_HEIGHT, LEGACY_DEFAULT_OPTIMIZER_BIN, MAX_IMAGE_QUALITY, MAX_MIN_SAVINGS_PERCENT,
    MAX_TARGET_HEIGHT, MIN_IMAGE_QUALITY, MIN_SAVINGS_PERCENT, MIN_TARGET_HEIGHT, OptimizationResult,
    OptimizerValidationCode, OptimizerValidationResult,
};
#[allow(unused_imports)]
pub use publisher::{canonical_release_name, publish_cbz, PublishOutcome};
#[allow(unused_imports)]
pub use validation::{
    probe_writable_directory, validate_destination_dir, PathValidationCode, PathValidationResult,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

const MAX_RECENT_EVENTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerificationPolicy {
    pub timeout: Duration,
    pub initial_delay: Duration,
    pub max_interval: Duration,
    pub backoff_factor: f64,
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            initial_delay: Duration::from_millis(1500),
            max_interval: Duration::from_secs(10),
            backoff_factor: 1.5,
        }
    }
}

impl VerificationPolicy {
    #[allow(dead_code)]
    pub fn fast_test() -> Self {
        Self {
            timeout: Duration::from_millis(600),
            initial_delay: Duration::from_millis(20),
            max_interval: Duration::from_millis(60),
            backoff_factor: 1.2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    Complete(PathBuf),
    PublishedPendingKomga(PathBuf),
    PublishedScanFailed { destination: PathBuf, error: String },
}

fn default_target_height() -> u32 {
    optimizer::DEFAULT_TARGET_HEIGHT
}

fn default_image_quality() -> u32 {
    optimizer::DEFAULT_IMAGE_QUALITY
}

fn default_min_savings_percent() -> u32 {
    optimizer::DEFAULT_MIN_SAVINGS_PERCENT
}

fn default_true() -> bool {
    true
}

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
    pub komga_chinese_library_id: Option<String>,
    #[serde(default = "default_true")]
    pub komga_scan_enabled: bool,
    pub optimize_enabled: bool,
    pub optimizer_bin: Option<String>,
    #[serde(default = "default_target_height")]
    pub target_height: u32,
    #[serde(default = "default_image_quality")]
    pub image_quality: u32,
    #[serde(default = "default_min_savings_percent")]
    pub min_savings_percent: u32,

    // Container-aware validation status fields (populated on load/validate)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_status: Option<PathValidationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chinese_library_status: Option<PathValidationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimizer_status: Option<OptimizerValidationResult>,
    #[serde(default)]
    pub bundled_optimizer_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_optimizer_bin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mangabaka_validated: Option<bool>,
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
            komga_chinese_library_id: None,
            komga_scan_enabled: true,
            optimize_enabled: false,
            optimizer_bin: None,
            target_height: optimizer::DEFAULT_TARGET_HEIGHT,
            image_quality: optimizer::DEFAULT_IMAGE_QUALITY,
            min_savings_percent: optimizer::DEFAULT_MIN_SAVINGS_PERCENT,
            library_status: None,
            chinese_library_status: None,
            optimizer_status: None,
            bundled_optimizer_present: false,
            resolved_optimizer_bin: None,
            mangabaka_validated: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatePathsResponse {
    pub library: PathValidationResult,
    pub chinese_library: PathValidationResult,
    pub optimizer: OptimizerValidationResult,
    pub bundled_optimizer_present: bool,
    pub resolved_optimizer_bin: Option<String>,
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
    pub(crate) http: Client,
    pub ownership: ownership::OwnershipService,
    events: Arc<RwLock<VecDeque<PipelineEvent>>>,
    verification_policy: Arc<RwLock<VerificationPolicy>>,
}

impl Pipeline {
    pub fn new(db: Db) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let ownership = ownership::OwnershipService::new(http.clone(), db.clone());
        Self {
            db,
            http,
            ownership,
            events: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_RECENT_EVENTS))),
            verification_policy: Arc::new(RwLock::new(VerificationPolicy::default())),
        }
    }

    #[allow(dead_code)]
    pub fn set_verification_policy(&self, policy: VerificationPolicy) {
        if let Ok(mut lock) = self.verification_policy.write() {
            *lock = policy;
        }
    }

    pub fn verification_policy(&self) -> VerificationPolicy {
        self.verification_policy.read().map(|p| *p).unwrap_or_default()
    }

    /// Loads the active pipeline configuration from settings and state, enriched with live validation statuses.
    pub async fn load_config(&self) -> Result<PipelineConfig> {
        let (raw_cfg, mangabaka_token, komga_api_key) = self.load_internal_config().await?;

        // Authoritative server-side runtime validation of library destinations and optimizer
        let library_status = validate_destination_dir(raw_cfg.library_dir.as_deref(), raw_cfg.enabled);
        let chinese_library_status =
            validate_destination_dir(raw_cfg.chinese_library_dir.as_deref(), false);
        let optimizer_status = validate_optimizer(raw_cfg.optimizer_bin.as_deref()).await;
        let (resolved_bin, _src) = resolve_optimizer_binary(raw_cfg.optimizer_bin.as_deref());
        let bundled_present = Path::new(BUNDLED_OPTIMIZER_PATH).is_file();

        Ok(PipelineConfig {
            enabled: raw_cfg.enabled,
            library_dir: raw_cfg.library_dir,
            chinese_library_dir: raw_cfg.chinese_library_dir,
            mangabaka_enabled: raw_cfg.mangabaka_enabled,
            mangabaka_url: raw_cfg.mangabaka_url,
            mangabaka_token: None, // Secret redacted in config view
            has_mangabaka_token: mangabaka_token.is_some_and(|t| !t.trim().is_empty()),
            komga_enabled: raw_cfg.komga_enabled,
            komga_url: raw_cfg.komga_url,
            komga_api_key: None, // Secret redacted in config view
            has_komga_api_key: komga_api_key.is_some_and(|k| !k.trim().is_empty()),
            komga_library_id: raw_cfg.komga_library_id,
            komga_chinese_library_id: raw_cfg.komga_chinese_library_id,
            komga_scan_enabled: raw_cfg.komga_scan_enabled,
            optimize_enabled: raw_cfg.optimize_enabled,
            optimizer_bin: raw_cfg.optimizer_bin,
            target_height: raw_cfg.target_height,
            image_quality: raw_cfg.image_quality,
            min_savings_percent: raw_cfg.min_savings_percent,
            library_status: Some(library_status),
            chinese_library_status: Some(chinese_library_status),
            optimizer_status: Some(optimizer_status),
            bundled_optimizer_present: bundled_present,
            resolved_optimizer_bin: resolved_bin.map(|p| p.to_string_lossy().to_string()),
            mangabaka_validated: raw_cfg.mangabaka_validated,
        })
    }

    /// Evaluates candidate paths on-demand without modifying saved configuration.
    pub async fn validate_paths(
        &self,
        library_dir: Option<&str>,
        chinese_library_dir: Option<&str>,
        optimizer_bin: Option<&str>,
    ) -> ValidatePathsResponse {
        let library = validate_destination_dir(library_dir, true);
        let chinese_library = validate_destination_dir(chinese_library_dir, false);
        let optimizer = validate_optimizer(optimizer_bin).await;
        let (resolved_bin, _src) = resolve_optimizer_binary(optimizer_bin);
        let bundled_present = Path::new(BUNDLED_OPTIMIZER_PATH).is_file();

        ValidatePathsResponse {
            library,
            chinese_library,
            optimizer,
            bundled_optimizer_present: bundled_present,
            resolved_optimizer_bin: resolved_bin.map(|p| p.to_string_lossy().to_string()),
        }
    }

    /// Internal helper to load configuration with secrets included for execution.
    pub(crate) async fn load_internal_config(&self) -> Result<(PipelineConfig, Option<String>, Option<String>)> {
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
                let komga_chinese_library_id = crate::settings::get::<String>(conn, "pipeline.komga_chinese_library_id");
                let komga_scan_enabled = crate::settings::get::<bool>(conn, "pipeline.komga_scan_enabled").unwrap_or(true);
                let optimize_enabled = crate::settings::get::<bool>(conn, "pipeline.optimize_enabled").unwrap_or(false);

                // Migration semantics: untouched legacy default path is migrated to None (using bundled path)
                let raw_bin = crate::settings::get::<String>(conn, "pipeline.optimizer_bin");
                let optimizer_bin = match raw_bin.as_deref().map(str::trim) {
                    Some(s) if s == LEGACY_DEFAULT_OPTIMIZER_BIN || s.is_empty() => None,
                    Some(s) => Some(s.to_string()),
                    None => None,
                };

                let target_height = crate::settings::get::<u32>(conn, "pipeline.target_height")
                    .unwrap_or(optimizer::DEFAULT_TARGET_HEIGHT);
                let image_quality = crate::settings::get::<u32>(conn, "pipeline.image_quality")
                    .unwrap_or(optimizer::DEFAULT_IMAGE_QUALITY);
                let min_savings_percent = crate::settings::get::<u32>(conn, "pipeline.min_savings_percent")
                    .unwrap_or(optimizer::DEFAULT_MIN_SAVINGS_PERCENT);

                let mangabaka_token = crate::db::state(conn, "pipeline.mangabaka_token")?;
                let komga_api_key = crate::db::state(conn, "pipeline.komga_api_key")?;
                let mangabaka_val_state = crate::db::state(conn, "pipeline.mangabaka_validated")?;
                let mangabaka_validated = if mangabaka_token.is_some() {
                    match mangabaka_val_state.as_deref() {
                        Some("valid") => Some(true),
                        Some(s) if s.starts_with("error") => Some(false),
                        _ => None,
                    }
                } else {
                    None
                };

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
                    komga_chinese_library_id,
                    komga_scan_enabled,
                    optimize_enabled,
                    optimizer_bin,
                    target_height,
                    image_quality,
                    min_savings_percent,
                    library_status: None,
                    chinese_library_status: None,
                    optimizer_status: None,
                    bundled_optimizer_present: false,
                    resolved_optimizer_bin: None,
                    mangabaka_validated,
                };

                Ok((cfg, mangabaka_token, komga_api_key))
            })
            .await
    }

    /// Saves updated pipeline configuration and tokens.
    ///
    /// Authoritative server-side safety checks ensure the pipeline cannot be enabled
    /// with invalid, missing, or non-writable destination directories.
    pub async fn save_config(&self, mut patch: PipelineConfig) -> Result<()> {
        // Normalize legacy optimizer path: replace untouched legacy default with None (bundled default)
        patch.optimizer_bin = match patch.optimizer_bin.as_deref().map(str::trim) {
            Some(s) if s == LEGACY_DEFAULT_OPTIMIZER_BIN || s.is_empty() => None,
            Some(s) => Some(s.to_string()),
            None => None,
        };

        // Range validation for optimizer quality settings
        if patch.target_height < optimizer::MIN_TARGET_HEIGHT || patch.target_height > optimizer::MAX_TARGET_HEIGHT {
            return Err(AppError::bad_request(format!(
                "Target page height must be between {} and {} px",
                optimizer::MIN_TARGET_HEIGHT,
                optimizer::MAX_TARGET_HEIGHT
            )));
        }
        if patch.image_quality < optimizer::MIN_IMAGE_QUALITY || patch.image_quality > optimizer::MAX_IMAGE_QUALITY {
            return Err(AppError::bad_request(format!(
                "Image quality must be between {} and {}",
                optimizer::MIN_IMAGE_QUALITY,
                optimizer::MAX_IMAGE_QUALITY
            )));
        }
        if patch.min_savings_percent > optimizer::MAX_MIN_SAVINGS_PERCENT {
            return Err(AppError::bad_request(format!(
                "Minimum size savings percentage must be between {} and {}%",
                optimizer::MIN_SAVINGS_PERCENT,
                optimizer::MAX_MIN_SAVINGS_PERCENT
            )));
        }

        // Strict authoritative server-side validation when enabling the pipeline
        if patch.enabled {
            let lib_status = validate_destination_dir(patch.library_dir.as_deref(), true);
            if !lib_status.is_valid() {
                return Err(AppError::bad_request(format!(
                    "Cannot enable acquisition pipeline: Permanent manga library error: {}",
                    lib_status.message
                )));
            }

            if patch.chinese_library_dir.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                let zh_status = validate_destination_dir(patch.chinese_library_dir.as_deref(), false);
                if !zh_status.is_valid() {
                    return Err(AppError::bad_request(format!(
                        "Cannot enable acquisition pipeline: Chinese manga library error: {}",
                        zh_status.message
                    )));
                }
            }

            if patch.optimize_enabled {
                let opt_status = validate_optimizer(patch.optimizer_bin.as_deref()).await;
                if !opt_status.is_valid() {
                    return Err(AppError::bad_request(format!(
                        "Cannot enable image optimization: {}",
                        opt_status.message
                    )));
                }
            }
        }

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
                map.insert("pipeline.komga_chinese_library_id".into(), serde_json::json!(patch.komga_chinese_library_id));
                map.insert("pipeline.komga_scan_enabled".into(), serde_json::json!(patch.komga_scan_enabled));
                map.insert("pipeline.optimize_enabled".into(), serde_json::json!(patch.optimize_enabled));
                map.insert("pipeline.optimizer_bin".into(), serde_json::json!(patch.optimizer_bin));
                map.insert("pipeline.target_height".into(), serde_json::json!(patch.target_height));
                map.insert("pipeline.image_quality".into(), serde_json::json!(patch.image_quality));
                map.insert("pipeline.min_savings_percent".into(), serde_json::json!(patch.min_savings_percent));

                crate::settings::put(conn, &map)?;

                // Update secrets in state table if provided (non-empty and not masked placeholder)
                if let Some(token) = patch.mangabaka_token {
                    let trimmed = token.trim();
                    if trimmed == "CLEAR" {
                        crate::db::forget_state(conn, "pipeline.mangabaka_token")?;
                        crate::db::forget_state(conn, "pipeline.mangabaka_validated")?;
                    } else if !trimmed.is_empty() && trimmed != "********" {
                        crate::db::set_state(conn, "pipeline.mangabaka_token", trimmed)?;
                        crate::db::forget_state(conn, "pipeline.mangabaka_validated")?;
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

    /// Discovers libraries configured in Komga without triggering scans.
    pub async fn discover_komga_libraries(
        &self,
        url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Vec<komga::KomgaLibrary>> {
        let (raw_cfg, _, saved_key) = self.load_internal_config().await?;
        let target_url = url.filter(|s| !s.trim().is_empty()).or(raw_cfg.komga_url);
        let actual_key = match api_key {
            Some(k) if k.trim() == "********" || k.trim().is_empty() => saved_key,
            Some(k) => Some(k),
            None => saved_key,
        };

        let client = KomgaClient::new(self.http.clone(), target_url, actual_key);
        client.list_libraries().await
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

    /// Polls Komga with bounded backoff until the release is indexed or timeout expires.
    pub async fn poll_verify_release(
        &self,
        komga_client: &KomgaClient,
        series_title: &str,
        library_id: Option<&str>,
        is_volume: bool,
        release_number: f64,
        policy: VerificationPolicy,
    ) -> Result<bool> {
        let start = std::time::Instant::now();
        let mut interval = policy.initial_delay;

        // Initial delay to allow Komga to receive and queue the scan
        tokio::time::sleep(interval).await;

        while start.elapsed() < policy.timeout {
            match komga_client
                .verify_release_indexed(series_title, library_id, is_volume, release_number)
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        "KOMGA: ownership verified for {} ({} {})",
                        series_title,
                        if is_volume { "volume" } else { "chapter" },
                        release_number
                    );
                    return Ok(true);
                }
                Ok(false) => {
                    tracing::debug!(
                        "KOMGA: release {} ({} {}) not yet visible, waiting...",
                        series_title,
                        if is_volume { "volume" } else { "chapter" },
                        release_number
                    );
                }
                Err(e) => {
                    tracing::warn!("KOMGA: error checking release indexing during verification: {e}");
                }
            }

            interval = Duration::from_secs_f64(
                (interval.as_secs_f64() * policy.backoff_factor)
                    .min(policy.max_interval.as_secs_f64()),
            );
            let remaining = policy.timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep(interval.min(remaining)).await;
        }

        // Final check
        komga_client
            .verify_release_indexed(series_title, library_id, is_volume, release_number)
            .await
    }

    /// Executes the full native acquisition post-processing pipeline for a downloaded chapter/volume.
    #[allow(dead_code)]
    pub async fn process_download(
        &self,
        source_cbz: &Path,
        job: &crate::downloads::Job,
    ) -> Result<AcquisitionOutcome> {
        self.process_download_with_progress(source_cbz, job, |_, _, _| {}).await
    }

    /// Executes the pipeline reporting lifecycle stages to the provided callback.
    pub async fn process_download_with_progress<F>(
        &self,
        source_cbz: &Path,
        job: &crate::downloads::Job,
        mut on_stage: F,
    ) -> Result<AcquisitionOutcome>
    where
        F: FnMut(&str, &str, Option<String>),
    {
        let (config, mb_token, komga_key) = self.load_internal_config().await?;

        if !config.enabled {
            return Ok(AcquisitionOutcome::Complete(source_cbz.to_path_buf()));
        }

        on_stage("identifying", "Classifying release format...", None);

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

        // Safety verification: Authoritatively verify destination is valid and writable before proceeding
        let dest_val = validate_destination_dir(Some(&dest_root.to_string_lossy()), true);
        if !dest_val.is_valid() {
            let err = format!("Configured library path cannot safely be used: {}", dest_val.message);
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
            on_stage("metadata", "Querying MangaBaka catalog...", None);
            let mb_client = MangaBakaClient::new(self.http.clone(), config.mangabaka_url.clone(), mb_token);
            match mb_client.fetch_volume_enrichment(&job.title, release_num).await {
                Ok(mangabaka::MangaBakaOutcome::Success(enrichment)) => {
                    match MangaBakaClient::enrich_cbz_archive(source_cbz, &enrichment) {
                        Ok(true) => mangabaka_status = format!("enriched: vol {}", enrichment.volume),
                        Ok(false) => mangabaka_status = "enrichment skipped (no ComicInfo)".to_string(),
                        Err(e) => mangabaka_status = format!("enrichment failed: {e}"),
                    }
                }
                Ok(outcome) => mangabaka_status = outcome.description(),
                Err(e) => mangabaka_status = format!("enrichment query error: {e}"),
            }
        }

        // 3. Non-destructive Image Optimization
        let mut optimizer_status = "skipped: disabled".to_string();
        if !config.optimize_enabled {
            tracing::info!("OPTIMIZER: skipped: disabled");
        } else if is_chinese {
            optimizer_status = "skipped: Chinese edition (already optimized)".to_string();
            tracing::info!("OPTIMIZER: skipped: Chinese edition (already optimized)");
        } else {
            on_stage("optimizing", "Optimizing image resolution and compression...", None);
            let (resolved_bin, bin_source) = optimizer::resolve_optimizer_binary(config.optimizer_bin.as_deref());
            match resolved_bin {
                None => {
                    optimizer_status = "failed: optimizer unavailable".to_string();
                    tracing::warn!("OPTIMIZER: failed: optimizer binary unavailable or not found — original retained");
                }
                Some(ref bin) => {
                    let orig_size = fs::metadata(source_cbz).map(|m| m.len()).unwrap_or(0);
                    tracing::info!(
                        "OPTIMIZER: starting {} ({bin_source}) for candidate {} ({} bytes, target height {}px, quality {}, min savings {}%)",
                        bin.display(),
                        source_cbz.display(),
                        orig_size,
                        config.target_height,
                        config.image_quality,
                        config.min_savings_percent
                    );
                    match optimizer::optimize_cbz(
                        source_cbz,
                        config.optimizer_bin.as_deref(),
                        config.target_height,
                        config.image_quality,
                        config.min_savings_percent,
                    ).await {
                        Ok(res) => {
                            optimizer_status = res.reason.clone();
                            if res.applied {
                                tracing::info!(
                                    "OPTIMIZER: candidate {} bytes -> optimized {} bytes (savings {:.1}% >= {}%) — accepted",
                                    res.original_bytes,
                                    res.optimized_bytes,
                                    res.savings_percent,
                                    config.min_savings_percent
                                );
                            } else {
                                tracing::info!(
                                    "OPTIMIZER: {} — original retained",
                                    res.reason
                                );
                            }
                        }
                        Err(e) => {
                            optimizer_status = format!("failed: {e}");
                            tracing::warn!("OPTIMIZER: failed: {e} — original retained");
                        }
                    }
                }
            }
        }

        // 4. Safe, Atomic Publication into Komga Library
        on_stage("publishing", "Publishing to canonical Komga library...", None);
        let pub_res = publish_cbz(source_cbz, &dest_root, &job.title, is_volume, release_num);
        let outcome_str = match &pub_res {
            Ok(PublishOutcome::Published(_)) => "published",
            Ok(PublishOutcome::AlreadyPublished(_)) => "already_published",
            Err(e) if e.code == "destination_conflict" => "conflict",
            Err(_) => "error",
        };

        // Fail-safe: if publication failed (e.g. conflict, directory error), fail closed now
        // Staging download remains preserved in Mango data directory.
        if let Err(e) = pub_res {
            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: outcome_str.to_string(),
                destination: None,
                mangabaka_status,
                optimizer_status,
                komga_status: "skipped".to_string(),
                error: Some(e.message.clone()),
            });
            return Err(e);
        }

        let canonical_dest = match pub_res.unwrap() {
            PublishOutcome::Published(p) | PublishOutcome::AlreadyPublished(p) => p,
        };
        let canonical_dest_str = canonical_dest.to_string_lossy().to_string();

        // 5. Durably persist publication success BEFORE Komga scan or verification
        let chapter_id = job.chapter;
        let p_str = canonical_dest_str.clone();
        let target_library_id = if is_chinese {
            config.komga_chinese_library_id.as_deref().or(config.komga_library_id.as_deref())
        } else {
            config.komga_library_id.as_deref()
        };
        let t_lib = target_library_id.map(|s| s.to_string());

        let _ = self.db.call(move |conn| {
            conn.execute(
                "UPDATE chapters SET download_path = ?2 WHERE id = ?1",
                rusqlite::params![chapter_id, p_str],
            )?;
            conn.execute(
                "UPDATE downloads SET published_path = ?2, published_at = ?3, target_library_id = ?4, state = 'verifying', error = 'Published — waiting for Komga' WHERE chapter_id = ?1",
                rusqlite::params![chapter_id, p_str, now as i64, t_lib],
            )?;
            Ok(())
        }).await;

        if !config.komga_enabled || !config.komga_scan_enabled {
            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: outcome_str.to_string(),
                destination: Some(canonical_dest_str.clone()),
                mangabaka_status,
                optimizer_status,
                komga_status: "skipped: komga disabled".to_string(),
                error: None,
            });

            let _ = self.db.call(move |conn| {
                conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter_id])?;
                Ok(())
            }).await;

            on_stage("complete", "Acquisition complete", Some(canonical_dest_str));
            return Ok(AcquisitionOutcome::Complete(canonical_dest));
        }

        // 6. Trigger only the configured destination Komga library scan
        on_stage("komga", "Triggering targeted Komga library scan...", Some(canonical_dest_str.clone()));
        let komga_client = KomgaClient::new(self.http.clone(), config.komga_url.clone(), komga_key);

        if let Err(e) = komga_client.trigger_scan(target_library_id).await {
            let err_msg = format!("Published to {}, but Komga scan trigger failed: {}", canonical_dest_str, e.message);
            tracing::warn!("PIPELINE: {err_msg}");

            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: outcome_str.to_string(),
                destination: Some(canonical_dest_str.clone()),
                mangabaka_status,
                optimizer_status,
                komga_status: format!("scan trigger failed: {}", e.message),
                error: Some(err_msg.clone()),
            });

            let err_clone = err_msg.clone();
            let _ = self.db.call(move |conn| {
                conn.execute(
                    "UPDATE downloads SET state = 'failed', error = ?2, retry_at = ?3 WHERE chapter_id = ?1",
                    rusqlite::params![chapter_id, err_clone, (now + 60) as i64],
                )?;
                Ok(())
            }).await;

            return Ok(AcquisitionOutcome::PublishedScanFailed {
                destination: canonical_dest,
                error: err_msg,
            });
        }

        // 7. Bounded Asynchronous Ownership Verification
        on_stage("verifying", "Ownership verification pending in Komga...", Some(canonical_dest_str.clone()));
        let policy = self.verification_policy();

        let verified = self.poll_verify_release(
            &komga_client,
            &job.title,
            target_library_id,
            is_volume,
            release_num,
            policy,
        ).await.unwrap_or(false);

        if verified {
            self.ownership.invalidate(job.manga_id);
            let _ = self.ownership.get_title_ownership(job.manga_id, &job.title).await;

            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: outcome_str.to_string(),
                destination: Some(canonical_dest_str.clone()),
                mangabaka_status,
                optimizer_status,
                komga_status: "verified in Komga".to_string(),
                error: None,
            });

            let _ = self.db.call(move |conn| {
                conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [chapter_id])?;
                Ok(())
            }).await;

            on_stage("complete", "Acquisition complete — verified in Komga", Some(canonical_dest_str));
            Ok(AcquisitionOutcome::Complete(canonical_dest))
        } else {
            // Indexing not visible yet within bounded timeout:
            // Persist pending verification state (NOT corruption, NOT download failure)
            self.record_event(PipelineEvent {
                id: format!("{}-{}", job.chapter, now),
                timestamp: now,
                chapter_id: job.chapter,
                series: job.title.clone(),
                release_name,
                is_volume,
                number: release_num,
                outcome: outcome_str.to_string(),
                destination: Some(canonical_dest_str.clone()),
                mangabaka_status,
                optimizer_status,
                komga_status: "pending verification (timeout)".to_string(),
                error: Some("Published — waiting for Komga".to_string()),
            });

            let _ = self.db.call(move |conn| {
                conn.execute(
                    "UPDATE downloads SET state = 'verifying', error = 'Published — waiting for Komga', attempts = attempts + 1, retry_at = ?2 WHERE chapter_id = ?1",
                    rusqlite::params![chapter_id, (now + 60) as i64],
                )?;
                Ok(())
            }).await;

            on_stage("verifying", "Published — waiting for Komga", Some(canonical_dest_str));
            Ok(AcquisitionOutcome::PublishedPendingKomga(canonical_dest))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;

    use rusqlite::OptionalExtension;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

    pub(crate) fn test_db_instance() -> Db {
        test_db()
    }

    #[test]
    fn pipeline_config_defaults() {
        let cfg = PipelineConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.mangabaka_enabled);
        assert_eq!(cfg.min_savings_percent, 10);
        assert_eq!(cfg.optimizer_bin, None);
    }

    #[tokio::test]
    async fn pipeline_cannot_enable_optimizer_with_invalid_binary() {
        let db = test_db();
        let pipeline = Pipeline::new(db);
        let temp_lib = TempDir::new("pipetest-opt-err");

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(temp_lib.path().to_string_lossy().to_string());
        config.optimize_enabled = true;
        config.optimizer_bin = Some("/nonexistent/custom_shrinker".into());

        let err = pipeline.save_config(config).await.unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert!(err.message.contains("Cannot enable image optimization"));
        assert!(err.message.contains("Binary not found"));
    }

    #[tokio::test]
    async fn pipeline_validate_paths_method() {
        let db = test_db();
        let pipeline = Pipeline::new(db);
        let temp_lib = TempDir::new("pipetest-val-resp");

        let resp = pipeline
            .validate_paths(
                Some(temp_lib.path().to_str().unwrap()),
                Some("/nonexistent/zh"),
                Some(LEGACY_DEFAULT_OPTIMIZER_BIN),
            )
            .await;

        assert_eq!(resp.library.code, PathValidationCode::Valid);
        assert!(resp.library.writable);
        assert_eq!(resp.chinese_library.code, PathValidationCode::Missing);
        // Legacy default should not be reported as a custom missing path
        assert_ne!(resp.optimizer.source, "custom");
    }

    #[tokio::test]
    async fn pipeline_disabled_saves_paths_without_enabling_validation() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let mut config = PipelineConfig::default();
        config.enabled = false;
        config.library_dir = Some("/unmounted/future/library".into());

        // Saving while disabled should succeed so users can prepare settings
        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        assert!(!loaded.enabled);
        assert_eq!(loaded.library_dir.as_deref(), Some("/unmounted/future/library"));
        assert_eq!(loaded.library_status.unwrap().code, PathValidationCode::Missing);
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(prefix: &str) -> Self {
            let pid = std::process::id();
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn pipeline_migrates_legacy_untouched_optimizer_path() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let temp_dir = TempDir::new("opt-mig");

        let mut config = PipelineConfig::default();
        config.enabled = false;
        config.library_dir = Some(temp_dir.path().to_string_lossy().to_string());
        // Simulating the legacy default untouched path
        config.optimizer_bin = Some(LEGACY_DEFAULT_OPTIMIZER_BIN.into());

        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        // Untouched legacy path must be normalized to None
        assert_eq!(loaded.optimizer_bin, None);
    }

    #[tokio::test]
    async fn pipeline_preserves_custom_optimizer_path() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let temp_dir = TempDir::new("opt-cust");

        let mut config = PipelineConfig::default();
        config.enabled = false;
        config.library_dir = Some(temp_dir.path().to_string_lossy().to_string());
        config.optimizer_bin = Some("/custom/tools/my_shrinker".into());

        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        assert_eq!(loaded.optimizer_bin.as_deref(), Some("/custom/tools/my_shrinker"));
    }

    #[tokio::test]
    async fn pipeline_config_round_trip_and_secret_masking() {
        let db = test_db();
        let pipeline = Pipeline::new(db);
        let temp_lib = TempDir::new("pipetest-lib");

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(temp_lib.path().to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_library_id = Some("lib-en-123".into());
        config.komga_chinese_library_id = Some("lib-zh-456".into());
        config.komga_scan_enabled = true;
        config.komga_api_key = Some("super_secret_komga_key".into());

        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.library_dir.as_deref(), Some(temp_lib.path().to_str().unwrap()));
        assert!(loaded.komga_enabled);
        assert_eq!(loaded.komga_library_id.as_deref(), Some("lib-en-123"));
        assert_eq!(loaded.komga_chinese_library_id.as_deref(), Some("lib-zh-456"));
        assert!(loaded.komga_scan_enabled);
        assert!(loaded.has_komga_api_key);
        // Secret must not be exposed in user-facing config
        assert_eq!(loaded.komga_api_key, None);
        // Live validation status is populated
        assert!(loaded.library_status.unwrap().is_valid());

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
            manga_id: 1,
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
            published_path: None,
            target_library_id: None,
        };

        pipeline.process_download(&eng_cbz, &eng_job).await.unwrap();
        let expected_eng_path = eng_library.join("English Series").join("English Series v01.cbz");
        assert!(expected_eng_path.exists(), "English volume published to default library");

        // 2. Process Chinese Volume -> routes to chinese_library_dir
        let zh_cbz = staging_dir.join("zh_vol2.cbz");
        make_test_cbz(&zh_cbz, "Chinese Series");
        let zh_job = crate::downloads::Job {
            chapter: 102,
            manga_id: 1,
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
            published_path: None,
            target_library_id: None,
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

    #[tokio::test]
    async fn pipeline_process_download_with_optimizer_end_to_end() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let test_root = std::env::temp_dir().join(format!("mango-pipetest-opt-e2e-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&test_root);
        let library_dir = test_root.join("manga");
        let staging_dir = test_root.join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        // 1. Create a mock shrinker that simulates a successful 25% reduction
        let script = test_root.join("mock_shrinker.sh");
        let script_content = r#"#!/bin/sh
# Finds the target archive in arguments
for arg in "$@"; do
    case "$arg" in
        *.cbz) target="$arg" ;;
    esac
done
if [ -n "$target" ] && [ -f "$target" ]; then
    # Overwrite target with a smaller valid CBZ containing same page count and ComicInfo
    tmp="$target.small.zip"
    python3 -c "
import zipfile, sys
with zipfile.ZipFile('$tmp', 'w') as z:
    z.writestr('ComicInfo.xml', '<ComicInfo><Series>Test</Series></ComicInfo>')
    z.writestr('001.webp', b'small')
" 2>/dev/null || true
    if [ -f "$tmp" ]; then
        mv "$tmp" "$target"
    fi
fi
echo "MockShrinker 1.0.0"
exit 0
"#;
        fs::write(&script, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        // 2. Save configuration with optimize_enabled = true
        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(library_dir.to_string_lossy().to_string());
        config.optimize_enabled = true;
        config.optimizer_bin = Some(script.to_string_lossy().to_string());
        config.target_height = 2400;
        config.image_quality = 90;
        config.min_savings_percent = 5;
        config.mangabaka_enabled = false;
        config.komga_enabled = false;

        pipeline.save_config(config).await.unwrap();

        // Verify config loaded dynamically from DB
        let loaded = pipeline.load_config().await.unwrap();
        assert!(loaded.optimize_enabled);
        assert_eq!(loaded.target_height, 2400);
        assert_eq!(loaded.image_quality, 90);
        assert_eq!(loaded.min_savings_percent, 5);

        // 3. Process download
        let cbz = staging_dir.join("chap1.cbz");
        make_test_cbz(&cbz, "Series With Opt");
        let _orig_size = fs::metadata(&cbz).unwrap().len();

        let job = crate::downloads::Job {
            chapter: 201,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Series With Opt".into(),
            name: "Chapter 1".into(),
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
        };

        pipeline.process_download(&cbz, &job).await.unwrap();

        let events = pipeline.recent_events();
        assert_eq!(events[0].chapter_id, 201);
        // Optimizer status must not be "skipped" when enabled
        assert!(!events[0].optimizer_status.contains("skipped: disabled"));

        // 4. Save with optimize_enabled = false and verify dynamic update without restarting
        let mut disabled_config = pipeline.load_config().await.unwrap();
        disabled_config.optimize_enabled = false;
        pipeline.save_config(disabled_config).await.unwrap();

        let cbz2 = staging_dir.join("chap2.cbz");
        make_test_cbz(&cbz2, "Series With Opt");
        let job2 = crate::downloads::Job {
            chapter: 202,
            manga_id: 1,
            attempts: 0,
            source: "MangaDot".into(),
            title: "Series With Opt".into(),
            name: "Chapter 2".into(),
            number: 2.0,
            scanlator: Some("Stick".into()),
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: None,
            release_type: crate::release::ReleaseType::Chapter,
            chapter_number: Some(2.0),
            published_path: None,
            target_library_id: None,
        };

        pipeline.process_download(&cbz2, &job2).await.unwrap();
        let events2 = pipeline.recent_events();
        assert_eq!(events2[0].chapter_id, 202);
        assert_eq!(events2[0].optimizer_status, "skipped: disabled");

        let _ = fs::remove_dir_all(&test_root);
    }

    #[tokio::test]
    async fn pipeline_optimizer_config_persists_round_trip() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        let mut config = PipelineConfig::default();
        config.enabled = false;
        config.optimize_enabled = true;
        config.target_height = 2800;
        config.image_quality = 85;
        config.min_savings_percent = 15;

        pipeline.save_config(config).await.unwrap();

        let loaded = pipeline.load_config().await.unwrap();
        assert_eq!(loaded.target_height, 2800);
        assert_eq!(loaded.image_quality, 85);
        assert_eq!(loaded.min_savings_percent, 15);
    }

    #[tokio::test]
    async fn pipeline_rejects_invalid_optimizer_values() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        // Height too low (< 500)
        let mut cfg = PipelineConfig::default();
        cfg.target_height = 400;
        let err = pipeline.save_config(cfg).await.unwrap_err();
        assert!(err.message.contains("Target page height must be between 500 and 10000 px"));

        // Height too high (> 10000)
        let mut cfg = PipelineConfig::default();
        cfg.target_height = 15000;
        let err = pipeline.save_config(cfg).await.unwrap_err();
        assert!(err.message.contains("Target page height must be between 500 and 10000 px"));

        // Quality too low (< 1)
        let mut cfg = PipelineConfig::default();
        cfg.image_quality = 0;
        let err = pipeline.save_config(cfg).await.unwrap_err();
        assert!(err.message.contains("Image quality must be between 1 and 100"));

        // Quality too high (> 100)
        let mut cfg = PipelineConfig::default();
        cfg.image_quality = 101;
        let err = pipeline.save_config(cfg).await.unwrap_err();
        assert!(err.message.contains("Image quality must be between 1 and 100"));

        // Min savings too high (> 99)
        let mut cfg = PipelineConfig::default();
        cfg.min_savings_percent = 100;
        let err = pipeline.save_config(cfg).await.unwrap_err();
        assert!(err.message.contains("Minimum size savings percentage must be between 0 and 99%"));
    }

    #[tokio::test]
    async fn pipeline_migrates_missing_optimizer_config_to_defaults() {
        let db = test_db();
        let pipeline = Pipeline::new(db);

        // Fresh database with no settings produces default values
        let loaded = pipeline.load_config().await.unwrap();
        assert_eq!(loaded.target_height, 2400);
        assert_eq!(loaded.image_quality, 90);
        assert_eq!(loaded.min_savings_percent, 10);

        // Serde deserialization from JSON without new fields (representing older config payload) receives defaults safely
        let old_json = r#"{
            "enabled": false,
            "library_dir": null,
            "chinese_library_dir": null,
            "mangabaka_enabled": true,
            "mangabaka_url": "https://api.mangabaka.org/v1",
            "has_mangabaka_token": false,
            "komga_enabled": false,
            "komga_url": "http://127.0.0.1:25600",
            "has_komga_api_key": false,
            "komga_library_id": null,
            "optimize_enabled": false,
            "optimizer_bin": null,
            "min_savings_percent": 10
        }"#;
        let parsed: PipelineConfig = serde_json::from_str(old_json).unwrap();
        assert_eq!(parsed.target_height, 2400);
        assert_eq!(parsed.image_quality, 90);
        assert_eq!(parsed.min_savings_percent, 10);
    }

    async fn insert_test_manga_and_chapter(
        db: &Db,
        chapter_id: i64,
        manga_id: i64,
        title: &str,
        name: &str,
        number: f64,
    ) {
        let title = title.to_string();
        let name = name.to_string();
        db.call(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO manga (id, source_id, url, title, in_library) VALUES (?1, 1, '/test', ?2, 1)",
                rusqlite::params![manga_id, title],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at, read) VALUES (?1, ?2, '/ch', ?3, ?4, 1, 0, 0)",
                rusqlite::params![chapter_id, manga_id, name, number],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO downloads (chapter_id, state, position, added_at) VALUES (?1, 'downloading', 1, 1000)",
                rusqlite::params![chapter_id],
            )?;
            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_scenario_a_publish_scan_immediate_ownership_visibility() {
        use axum::{http::StatusCode, routing::{get, post}, Json, Router};
        use serde_json::json;

        let db = test_db();
        let pipeline = Pipeline::new(db.clone());
        pipeline.set_verification_policy(VerificationPolicy::fast_test());

        let test_root = TempDir::new("scenario-a");
        let library_dir = test_root.path().join("manga");
        let staging_dir = test_root.path().join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let app = Router::new()
            .route(
                "/api/v1/libraries/lib-en/scan",
                post(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-1",
                                "name": "Solo Leveling",
                                "libraryId": "lib-en",
                                "booksCount": 1
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-1/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "book-1",
                                "seriesId": "series-1",
                                "name": "Solo Leveling v01",
                                "number": 1.0,
                                "metadata": {
                                    "title": "Solo Leveling v01",
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

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(library_dir.to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_scan_enabled = true;
        config.komga_library_id = Some("lib-en".into());
        config.komga_url = Some(format!("http://127.0.0.1:{port}"));
        config.mangabaka_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        insert_test_manga_and_chapter(&db, 301, 1, "Solo Leveling", "Volume 1", 1.0).await;

        let cbz = staging_dir.join("sl_vol1.cbz");
        make_test_cbz(&cbz, "Solo Leveling");

        let job = crate::downloads::Job {
            chapter: 301,
            manga_id: 1,
            attempts: 0,
            source: "TestSrc".into(),
            title: "Solo Leveling".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
        };

        let outcome = pipeline.process_download(&cbz, &job).await.unwrap();
        let canonical = match outcome {
            AcquisitionOutcome::Complete(p) => p,
            other => panic!("Expected Complete, got {:?}", other),
        };

        assert!(canonical.is_file(), "Canonical published artifact exists on disk");
        assert_eq!(
            canonical,
            library_dir.join("Solo Leveling").join("Solo Leveling v01.cbz")
        );

        let (ch_path, dl_count): (Option<String>, i64) = db
            .call(|conn| {
                let p: Option<String> = conn
                    .query_row("SELECT download_path FROM chapters WHERE id = 301", [], |r| r.get(0))
                    .optional()?
                    .flatten();
                let c: i64 = conn.query_row("SELECT COUNT(*) FROM downloads WHERE chapter_id = 301", [], |r| r.get(0))?;
                Ok((p, c))
            })
            .await
            .unwrap();

        assert_eq!(ch_path.as_deref(), Some(canonical.to_str().unwrap()));
        assert_eq!(dl_count, 0, "Download queue entry deleted upon Komga verification");

        let events = pipeline.recent_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].komga_status, "verified in Komga");
        assert_eq!(events[0].outcome, "published");
    }

    #[tokio::test]
    async fn test_scenario_b_publish_scan_delayed_ownership_visibility() {
        use axum::{http::StatusCode, routing::{get, post}, Json, Router};
        use serde_json::json;

        let db = test_db();
        let pipeline = Pipeline::new(db.clone());
        pipeline.set_verification_policy(VerificationPolicy::fast_test());

        let test_root = TempDir::new("scenario-b");
        let library_dir = test_root.path().join("manga");
        let staging_dir = test_root.path().join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let poll_count = Arc::new(AtomicUsize::new(0));
        let poll_flag = poll_count.clone();

        let app = Router::new()
            .route(
                "/api/v1/libraries/lib-en/scan",
                post(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-delayed",
                                "name": "Delayed Series",
                                "libraryId": "lib-en",
                                "booksCount": 1
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-delayed/books",
                get(move || {
                    let poll_flag = poll_flag.clone();
                    async move {
                        let c = poll_flag.fetch_add(1, Ordering::SeqCst);
                        if c == 0 {
                            Json(json!({ "content": [] }))
                        } else {
                            Json(json!({
                                "content": [
                                    {
                                        "id": "book-1",
                                        "seriesId": "series-delayed",
                                        "name": "Delayed Series v01",
                                        "number": 1.0,
                                        "metadata": {
                                            "title": "Delayed Series v01",
                                            "number": "1",
                                            "numberSort": 1.0
                                        }
                                    }
                                ]
                            }))
                        }
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(library_dir.to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_scan_enabled = true;
        config.komga_library_id = Some("lib-en".into());
        config.komga_url = Some(format!("http://127.0.0.1:{port}"));
        config.mangabaka_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        insert_test_manga_and_chapter(&db, 302, 1, "Delayed Series", "Volume 1", 1.0).await;

        let cbz = staging_dir.join("delayed_vol1.cbz");
        make_test_cbz(&cbz, "Delayed Series");

        let job = crate::downloads::Job {
            chapter: 302,
            manga_id: 1,
            attempts: 0,
            source: "TestSrc".into(),
            title: "Delayed Series".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
        };

        let outcome = pipeline.process_download(&cbz, &job).await.unwrap();
        assert!(matches!(outcome, AcquisitionOutcome::Complete(_)));
        assert!(poll_count.load(Ordering::SeqCst) >= 2, "Poll loop ran multiple iterations before Komga observed book");

        let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads WHERE chapter_id = 302", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(dl_count, 0, "Download queue cleared after delayed visibility");
    }

    #[tokio::test]
    async fn test_scenario_c_publish_scan_failure_preserves_artifact_and_retryable() {
        use axum::{http::StatusCode, routing::post, Router};

        let db = test_db();
        let pipeline = Pipeline::new(db.clone());

        let test_root = TempDir::new("scenario-c");
        let library_dir = test_root.path().join("manga");
        let staging_dir = test_root.path().join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let app = Router::new()
            .route(
                "/api/v1/libraries/lib-en/scan",
                post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "Komga internal crash") }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(library_dir.to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_scan_enabled = true;
        config.komga_library_id = Some("lib-en".into());
        config.komga_url = Some(format!("http://127.0.0.1:{port}"));
        config.mangabaka_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        insert_test_manga_and_chapter(&db, 303, 1, "Failing Scan Series", "Volume 1", 1.0).await;

        let cbz = staging_dir.join("failing_vol1.cbz");
        make_test_cbz(&cbz, "Failing Scan Series");

        let job = crate::downloads::Job {
            chapter: 303,
            manga_id: 1,
            attempts: 0,
            source: "TestSrc".into(),
            title: "Failing Scan Series".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
        };

        let outcome = pipeline.process_download(&cbz, &job).await.unwrap();
        let (dest, err) = match outcome {
            AcquisitionOutcome::PublishedScanFailed { destination, error } => (destination, error),
            other => panic!("Expected PublishedScanFailed, got {:?}", other),
        };

        assert!(dest.is_file(), "Canonical artifact preserved on disk");
        assert!(crate::pipeline::publisher::verify_cbz_integrity(&dest).is_ok());
        assert!(err.contains("Komga scan trigger failed"));

        let (state, pub_path, err_msg): (String, Option<String>, Option<String>) = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT state, published_path, error FROM downloads WHERE chapter_id = 303",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .await
            .unwrap();

        assert_eq!(state, "failed");
        assert_eq!(pub_path.as_deref(), Some(dest.to_str().unwrap()));
        assert!(err_msg.unwrap().contains("Komga scan trigger failed"));

        let (can_retry_komga, failure_stage) = db
            .call(|conn| {
                let qv = crate::downloads::read_queue(conn).unwrap();
                let item = qv.queue.into_iter().find(|q| q.chapter_id == 303).unwrap();
                Ok((item.can_retry_komga, item.failure_stage))
            })
            .await
            .unwrap();
        assert!(can_retry_komga, "Exposes can_retry_komga = true");
        assert_eq!(failure_stage.as_deref(), Some("komga"));
    }

    #[tokio::test]
    async fn test_scenario_d_publish_scan_timeout_preserves_artifact_and_pending() {
        use axum::{http::StatusCode, routing::{get, post}, Json, Router};
        use serde_json::json;

        let db = test_db();
        let pipeline = Pipeline::new(db.clone());
        pipeline.set_verification_policy(VerificationPolicy::fast_test());

        let test_root = TempDir::new("scenario-d");
        let library_dir = test_root.path().join("manga");
        let staging_dir = test_root.path().join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let app = Router::new()
            .route(
                "/api/v1/libraries/lib-en/scan",
                post(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-slow",
                                "name": "Slow Komga Series",
                                "libraryId": "lib-en",
                                "booksCount": 0
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-slow/books",
                get(|| async { Json(json!({ "content": [] })) }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = PipelineConfig::default();
        config.enabled = true;
        config.library_dir = Some(library_dir.to_string_lossy().to_string());
        config.komga_enabled = true;
        config.komga_scan_enabled = true;
        config.komga_library_id = Some("lib-en".into());
        config.komga_url = Some(format!("http://127.0.0.1:{port}"));
        config.mangabaka_enabled = false;
        config.optimize_enabled = false;
        pipeline.save_config(config).await.unwrap();

        insert_test_manga_and_chapter(&db, 304, 1, "Slow Komga Series", "Volume 1", 1.0).await;

        let cbz = staging_dir.join("slow_vol1.cbz");
        make_test_cbz(&cbz, "Slow Komga Series");

        let job = crate::downloads::Job {
            chapter: 304,
            manga_id: 1,
            attempts: 0,
            source: "TestSrc".into(),
            title: "Slow Komga Series".into(),
            name: "Volume 1".into(),
            number: 1.0,
            scanlator: None,
            author: None,
            artist: None,
            genre: None,
            summary: None,
            lang: Some("en".into()),
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            chapter_number: None,
            published_path: None,
            target_library_id: None,
        };

        let outcome = pipeline.process_download(&cbz, &job).await.unwrap();
        let canonical = match outcome {
            AcquisitionOutcome::PublishedPendingKomga(p) => p,
            other => panic!("Expected PublishedPendingKomga, got {:?}", other),
        };

        assert!(canonical.is_file());
        assert!(crate::pipeline::publisher::verify_cbz_integrity(&canonical).is_ok());

        let (state, pub_path, err_msg): (String, Option<String>, Option<String>) = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT state, published_path, error FROM downloads WHERE chapter_id = 304",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .await
            .unwrap();

        assert_eq!(state, "verifying");
        assert_eq!(pub_path.as_deref(), Some(canonical.to_str().unwrap()));
        assert_eq!(err_msg.as_deref(), Some("Published — waiting for Komga"));

        let (status_message, can_retry_komga) = db
            .call(|conn| {
                let qv = crate::downloads::read_queue(conn).unwrap();
                let item = qv.queue.into_iter().find(|q| q.chapter_id == 304).unwrap();
                Ok((item.status_message, item.can_retry_komga))
            })
            .await
            .unwrap();

        assert_eq!(status_message.as_deref(), Some("Published — waiting for Komga"));
        assert!(can_retry_komga);
    }

    #[tokio::test]
    async fn test_scenario_g_duplicate_acquisition_blocked_while_komga_pending() {
        use crate::pipeline::publisher::{publish_cbz, PublishOutcome};

        let db = test_db();
        let test_root = TempDir::new("scenario-g");
        let library_dir = test_root.path().join("manga");
        let staging_dir = test_root.path().join("staging");
        fs::create_dir_all(&library_dir).unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        insert_test_manga_and_chapter(&db, 305, 1, "Duplicate Guard", "Volume 1", 1.0).await;

        let cbz1 = staging_dir.join("dup_vol1.cbz");
        make_test_cbz(&cbz1, "Duplicate Guard Content 1");

        let res = publish_cbz(&cbz1, &library_dir, "Duplicate Guard", true, 1.0).unwrap();
        let canonical = match res {
            PublishOutcome::Published(p) => p,
            _ => unreachable!(),
        };

        let can_str = canonical.to_string_lossy().to_string();
        db.call(move |conn| {
            conn.execute("UPDATE chapters SET download_path = ?2 WHERE id = ?1", rusqlite::params![305, can_str])?;
            conn.execute(
                "UPDATE downloads SET published_path = ?2, state = 'verifying', error = 'Published — waiting for Komga' WHERE chapter_id = ?1",
                rusqlite::params![305, can_str],
            )?;
            Ok(())
        }).await.unwrap();

        // 1. Database-level duplicate protection
        let added = db.call(|conn| crate::downloads::add(conn, &[305])).await.unwrap();
        assert_eq!(added, 0, "Second acquisition blocked while ownership verification is pending");

        // 2. Filesystem-level duplicate protection
        let dup_same = publish_cbz(&cbz1, &library_dir, "Duplicate Guard", true, 1.0).unwrap();
        assert!(matches!(dup_same, PublishOutcome::AlreadyPublished(_)));

        let cbz2 = staging_dir.join("dup_vol1_different.cbz");
        make_test_cbz(&cbz2, "Different Unrelated Scanlator Content");
        let dup_conflict = publish_cbz(&cbz2, &library_dir, "Duplicate Guard", true, 1.0).unwrap_err();
        assert_eq!(dup_conflict.code, "destination_conflict");
    }
}
