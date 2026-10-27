//! TorBox External Download Backend Integration.
//!
//! Provides API client for TorBox v1 API, connection testing, torrent submission,
//! status monitoring, conservative file selection, streaming CDN retrieval to staging,
//! and remote job cancellation.

pub use crate::acquisition::BackendFile;
use crate::acquisition::{
    AcquisitionRelease, AcquisitionReleaseUnit, BackendJobStatus, ExternalJobInfo,
};
use crate::error::AppError;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

pub const DEFAULT_TORBOX_URL: &str = "https://api.torbox.app/v1/api";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorBoxError {
    Unauthorized(String),
    RateLimited(String),
    RemoteError(u16, String),
    NetworkError(String),
    TorrentFailed(String),
    AmbiguousFiles(String),
    EditionMismatch(String),
    NoArchiveFound(String),
    RequestedUnitNotFound(String),
    UnsupportedFormat(String),
    InvalidCbz(String),
    JobExpired(String),
    Io(String),
}

impl fmt::Display for TorBoxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized(msg) => write!(f, "TorBox authentication failed: {msg}"),
            Self::RateLimited(msg) => write!(f, "TorBox rate limit exceeded: {msg}"),
            Self::RemoteError(code, msg) => write!(f, "TorBox remote error (HTTP {code}): {msg}"),
            Self::NetworkError(msg) => write!(f, "TorBox network error: {msg}"),
            Self::TorrentFailed(msg) => write!(f, "TorBox torrent failed: {msg}"),
            Self::EditionMismatch(msg) => write!(f, "Edition mismatch: {msg}"),
            Self::AmbiguousFiles(msg) => write!(f, "TorBox file selection ambiguous: {msg}"),
            Self::NoArchiveFound(msg) => write!(f, "TorBox no manga archive found: {msg}"),
            Self::RequestedUnitNotFound(msg) => {
                write!(f, "Requested unit not found in TorBox torrent: {msg}")
            }
            Self::UnsupportedFormat(msg) => write!(f, "Unsupported archive format: {msg}"),
            Self::InvalidCbz(msg) => write!(f, "Retrieved archive failed CBZ validation: {msg}"),
            Self::JobExpired(msg) => write!(f, "TorBox job expired or missing: {msg}"),
            Self::Io(msg) => write!(f, "IO error during retrieval: {msg}"),
        }
    }
}

impl std::error::Error for TorBoxError {}

impl TorBoxError {
    pub fn is_job_expired(&self) -> bool {
        match self {
            Self::JobExpired(_) => true,
            Self::RemoteError(404, _) => true,
            Self::TorrentFailed(msg) if msg.to_lowercase().contains("not found") => true,
            _ => false,
        }
    }
}

impl From<TorBoxError> for AppError {
    fn from(err: TorBoxError) -> Self {
        match err {
            TorBoxError::Unauthorized(msg) => AppError::new(401, "unauthorized", msg),
            TorBoxError::RateLimited(msg) => AppError::new(429, "rate_limited", msg),
            TorBoxError::RemoteError(status, msg) => AppError::new(status, "remote_error", msg),
            TorBoxError::NetworkError(msg) => AppError::new(502, "network_error", msg),
            TorBoxError::TorrentFailed(msg) => AppError::new(422, "torrent_failed", msg),
            TorBoxError::EditionMismatch(msg) => AppError::new(422, "edition_mismatch", msg),
            TorBoxError::AmbiguousFiles(msg) => AppError::new(422, "ambiguous_files", msg),
            TorBoxError::NoArchiveFound(msg) => AppError::new(404, "no_archive_found", msg),
            TorBoxError::RequestedUnitNotFound(msg) => {
                AppError::new(404, "requested_unit_not_found", msg)
            }
            TorBoxError::UnsupportedFormat(msg) => AppError::new(400, "unsupported_format", msg),
            TorBoxError::InvalidCbz(msg) => AppError::bad_request(msg),
            TorBoxError::JobExpired(msg) => AppError::new(404, "external_job_expired", msg),
            TorBoxError::Io(msg) => AppError::internal(msg),
        }
    }
}

pub struct TorBoxClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl fmt::Debug for TorBoxClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorBoxClient")
            .field("base_url", &self.base_url)
            .field("has_token", &self.token.is_some())
            .finish()
    }
}

#[derive(Deserialize)]
struct TorBoxApiResponse<T> {
    success: bool,
    error: Option<String>,
    detail: Option<String>,
    data: Option<T>,
}

#[derive(Deserialize)]
struct UserMeData {
    #[allow(dead_code)]
    id: Option<i64>,
    email: Option<String>,
    plan: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CreateTorrentData {
    torrent_id: Option<serde_json::Value>,
    hash: Option<String>,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
pub struct TorrentItemData {
    pub id: serde_json::Value,
    pub name: Option<String>,
    pub hash: Option<String>,
    pub download_state: Option<String>,
    pub progress: Option<f64>,
    pub download_speed: Option<u64>,
    pub seeds: Option<u32>,
    pub peers: Option<u32>,
    pub files: Option<Vec<TorrentFileData>>,
}

#[derive(Deserialize, Clone)]
pub struct TorrentFileData {
    pub id: i64,
    pub name: String,
    pub size: u64,
    pub absolute_path: Option<String>,
}

impl TorBoxClient {
    pub fn new(http: Client, base_url: Option<String>, token: Option<String>) -> Self {
        let base_url = base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_TORBOX_URL.to_string());
        let token = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        Self {
            http,
            base_url,
            token,
        }
    }

    fn auth_token(&self) -> std::result::Result<&str, TorBoxError> {
        self.token.as_deref().ok_or_else(|| {
            TorBoxError::Unauthorized("TorBox API token is not configured".to_string())
        })
    }

    /// Tests the connection to TorBox by fetching the authenticated user profile (`/user/me`).
    pub async fn test_connection(&self) -> std::result::Result<String, TorBoxError> {
        let token = self.auth_token()?;
        let url = format!("{}/user/me", self.base_url);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(token)
            .header("User-Agent", "Mango/1.0 (Acquisition)")
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized(
                "Invalid or expired API token".to_string(),
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited(
                "TorBox rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("TorBox returned HTTP {status}"),
            ));
        }

        let body: TorBoxApiResponse<UserMeData> = resp
            .json()
            .await
            .map_err(|e| TorBoxError::RemoteError(500, format!("Failed to parse response: {e}")))?;

        if !body.success {
            let msg = body
                .detail
                .or(body.error)
                .unwrap_or_else(|| "Unknown error".to_string());
            return Err(TorBoxError::RemoteError(status.as_u16(), msg));
        }

        let user = body.data.unwrap_or(UserMeData {
            id: None,
            email: None,
            plan: None,
        });

        let email = user.email.unwrap_or_else(|| "user".to_string());
        let plan_str = user
            .plan
            .map(|p| p.to_string())
            .unwrap_or_else(|| "standard".to_string());
        Ok(format!("Connected to TorBox as {email} (Plan: {plan_str})"))
    }

    /// Submits a torrent to TorBox via magnet link.
    pub async fn submit(
        &self,
        release: &AcquisitionRelease,
    ) -> std::result::Result<ExternalJobInfo, TorBoxError> {
        let token = self.auth_token()?;
        let magnet = release.magnet_url.as_deref().ok_or_else(|| {
            TorBoxError::TorrentFailed(
                "Release does not contain a magnet URL or infoHash".to_string(),
            )
        })?;

        let url = format!("{}/torrents/createtorrent", self.base_url);
        let form = reqwest::multipart::Form::new()
            .text("magnet", magnet.to_string())
            .text("seed", "1")
            .text("allow_zip", "false");

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                TorBoxError::NetworkError(format!("Failed to submit torrent to TorBox: {e}"))
            })?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized(
                "Invalid or expired API token".to_string(),
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited(
                "TorBox rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("Failed to submit torrent to TorBox (HTTP {status})"),
            ));
        }

        let body: TorBoxApiResponse<CreateTorrentData> = resp.json().await.map_err(|e| {
            TorBoxError::RemoteError(500, format!("Failed to parse submission response: {e}"))
        })?;

        if !body.success {
            let msg = body
                .detail
                .or(body.error)
                .unwrap_or_else(|| "TorBox submission failed".to_string());
            return Err(TorBoxError::TorrentFailed(msg));
        }

        let data = body.data.ok_or_else(|| {
            TorBoxError::TorrentFailed("No data returned from createtorrent".to_string())
        })?;
        let job_id = match data.torrent_id {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s,
            _ => release
                .info_hash
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        };

        Ok(ExternalJobInfo {
            external_job_id: job_id,
            torrent_hash: data.hash.or_else(|| release.info_hash.clone()),
            is_cached: false,
        })
    }

    /// Queries TorBox for the status and file list of a submitted torrent.
    pub async fn get_torrent_data(
        &self,
        external_job_id: &str,
    ) -> std::result::Result<TorrentItemData, TorBoxError> {
        let token = self.auth_token()?;
        let url = format!(
            "{}/torrents/mylist?id={}&bypass_cache=true",
            self.base_url, external_job_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Err(TorBoxError::JobExpired(format!(
                "Failed to query torrent from TorBox (HTTP 404 Not Found for {external_job_id})"
            )));
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized(
                "Invalid or expired API token".to_string(),
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited(
                "TorBox rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("Failed to query torrent from TorBox (HTTP {status})"),
            ));
        }

        // TorBox /mylist with ?id= can return either a single TorrentItemData object or an array of items
        let text = resp
            .text()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        if let Ok(single) = serde_json::from_str::<TorBoxApiResponse<TorrentItemData>>(&text) {
            if single.success {
                if let Some(item) = single.data {
                    return Ok(item);
                }
            } else if let Some(ref detail) = single.detail {
                if detail.to_lowercase().contains("not found") {
                    return Err(TorBoxError::JobExpired(format!(
                        "Torrent {external_job_id} expired: {detail}"
                    )));
                }
            }
        }

        if let Ok(multi) = serde_json::from_str::<TorBoxApiResponse<Vec<TorrentItemData>>>(&text) {
            if multi.success {
                if let Some(mut items) = multi.data {
                    if let Some(item) = items.pop() {
                        return Ok(item);
                    }
                }
            } else if let Some(ref detail) = multi.detail {
                if detail.to_lowercase().contains("not found") {
                    return Err(TorBoxError::JobExpired(format!(
                        "Torrent {external_job_id} expired: {detail}"
                    )));
                }
            }
        }

        Err(TorBoxError::JobExpired(format!(
            "Torrent {external_job_id} not found in TorBox mylist response"
        )))
    }

    /// Checks the current backend status of a TorBox job.
    pub async fn check_status(
        &self,
        external_job_id: &str,
    ) -> std::result::Result<BackendJobStatus, TorBoxError> {
        let data = self.get_torrent_data(external_job_id).await?;
        let state = data
            .download_state
            .as_deref()
            .unwrap_or("downloading")
            .to_lowercase();

        if state == "cached" {
            return Ok(BackendJobStatus::Cached);
        }
        if state == "completed"
            || state == "seeding"
            || (data.progress.unwrap_or(0.0) >= 0.999
                && !data.files.as_ref().map(|f| f.is_empty()).unwrap_or(true))
        {
            return Ok(BackendJobStatus::Completed);
        }
        if state.contains("stalled")
            || (data.seeds.unwrap_or(1) == 0 && data.progress.unwrap_or(0.0) == 0.0)
        {
            return Ok(BackendJobStatus::Stalled {
                reason: "Swarm has 0 seeders or torrent is stalled on TorBox".to_string(),
            });
        }
        if state.contains("failed") || state.contains("error") {
            return Ok(BackendJobStatus::Failed {
                error: format!("TorBox job state is {state}"),
            });
        }

        Ok(BackendJobStatus::Downloading {
            progress: data.progress.unwrap_or(0.0),
            seeds: data.seeds.unwrap_or(0),
            peers: data.peers.unwrap_or(0),
            speed: data.download_speed.unwrap_or(0),
        })
    }

    /// Lists files contained within a completed TorBox torrent.
    pub async fn list_files(
        &self,
        external_job_id: &str,
    ) -> std::result::Result<Vec<BackendFile>, TorBoxError> {
        let data = self.get_torrent_data(external_job_id).await?;
        let raw_files = data.files.unwrap_or_default();

        let files = raw_files
            .into_iter()
            .map(|f| BackendFile {
                id: f.id,
                name: f.name.clone(),
                size: f.size,
                path: f.absolute_path.unwrap_or(f.name),
            })
            .collect();

        Ok(files)
    }

    /// Requests a direct CDN download link for a file in a TorBox torrent.
    pub async fn request_download_link(
        &self,
        external_job_id: &str,
        file_id: i64,
    ) -> std::result::Result<String, TorBoxError> {
        let token = self.auth_token()?;
        let url = format!(
            "{}/torrents/requestdl?token={}&torrent_id={}&file_id={}&zip=false",
            self.base_url, token, external_job_id, file_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Err(TorBoxError::JobExpired(format!(
                "Failed to request download link (HTTP 404 Not Found for torrent {external_job_id})"
            )));
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized(
                "Invalid or expired API token".to_string(),
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited(
                "TorBox rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("Failed to request download link (HTTP {status})"),
            ));
        }

        let body: TorBoxApiResponse<String> = resp.json().await.map_err(|e| {
            TorBoxError::RemoteError(500, format!("Failed to parse requestdl response: {e}"))
        })?;

        if !body.success {
            let msg = body
                .detail
                .or(body.error)
                .unwrap_or_else(|| "Failed to get download URL".to_string());
            if msg.to_lowercase().contains("not found") {
                return Err(TorBoxError::JobExpired(msg));
            }
            return Err(TorBoxError::TorrentFailed(msg));
        }

        let link = body.data.ok_or_else(|| {
            TorBoxError::TorrentFailed("Empty download URL from TorBox".to_string())
        })?;
        Ok(link)
    }

    /// Retrieves a file from TorBox CDN into a `.part` destination, validates archive integrity,
    /// and ensures bytes written match expectation.
    pub async fn retrieve_file(
        &self,
        external_job_id: &str,
        file_id: i64,
        expected_size: u64,
        destination_part: &Path,
    ) -> std::result::Result<u64, TorBoxError> {
        self.retrieve_file_with_tracker(
            external_job_id,
            file_id,
            expected_size,
            destination_part,
            file_id,
            &crate::transfer::TransferTracker::default(),
            &crate::transfer::TransferConfig::default(),
            |_, _, _| {},
        )
        .await
    }

    /// Extended retrieval method taking a tracker, config, and progress observer for stall
    /// detection, bounded retries, and Range resume.
    pub async fn retrieve_file_with_tracker<F>(
        &self,
        external_job_id: &str,
        file_id: i64,
        expected_size: u64,
        destination_part: &Path,
        chapter_id: i64,
        tracker: &crate::transfer::TransferTracker,
        config: &crate::transfer::TransferConfig,
        on_progress: F,
    ) -> std::result::Result<u64, TorBoxError>
    where
        F: FnMut(u64, Option<u64>, bool) + Send + 'static,
    {
        let download_url = self.request_download_link(external_job_id, file_id).await?;

        // URL safety verification (prevent SSRF/loopback bypass)
        validate_download_url(&download_url)?;

        let exp_opt = if expected_size > 0 {
            Some(expected_size)
        } else {
            None
        };

        let bytes = crate::transfer::stream_http_to_file(
            &self.http,
            &download_url,
            destination_part,
            exp_opt,
            config,
            chapter_id,
            tracker,
            on_progress,
        )
        .await
        .map_err(|e| {
            if e.code == "truncated_download" || e.code == "invalid_cbz" {
                TorBoxError::InvalidCbz(e.message)
            } else {
                TorBoxError::NetworkError(e.message)
            }
        })?;

        // Verify CBZ integrity on the temporary staging file
        if let Err(e) = crate::pipeline::publisher::verify_cbz_integrity(destination_part) {
            let _ = std::fs::remove_file(destination_part);
            return Err(TorBoxError::InvalidCbz(format!(
                "Downloaded file failed CBZ archive integrity: {e}"
            )));
        }

        Ok(bytes)
    }

    /// Cancels / deletes a torrent from TorBox remote storage.
    pub async fn cancel(&self, external_job_id: &str) -> std::result::Result<(), TorBoxError> {
        let token = self.auth_token()?;
        let url = format!("{}/torrents/controltorrent", self.base_url);

        let torrent_id_val = if let Ok(id_num) = external_job_id.parse::<i64>() {
            serde_json::Value::Number(id_num.into())
        } else {
            serde_json::Value::String(external_job_id.to_string())
        };

        let payload = serde_json::json!({
            "torrent_id": torrent_id_val,
            "operation": "delete"
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        if !resp.status().is_success() {
            // Cancellation is best-effort on delete
            tracing::warn!(
                "TorBox controltorrent delete returned HTTP {}",
                resp.status()
            );
        }

        Ok(())
    }
}

pub fn is_unsupported_format(lower: &str) -> bool {
    lower.ends_with(".cbr")
        || lower.ends_with(".rar")
        || lower.ends_with(".7z")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
        || lower.ends_with(".bz2")
        || lower.ends_with(".xz")
        || lower.ends_with(".pdf")
        || lower.ends_with(".epub")
        || lower.ends_with(".mobi")
        || lower.ends_with(".azw3")
}

/// Extracts the numeric volume number from an archive filename (e.g. "BLAME! v01.cbz" -> 1.0,
/// "Blue Lock v39 (2026) (Digital) (Stick).cbz" -> 39.0, "v001.cbz" -> 1.0, "vol. 1.cbz" -> 1.0).
///
/// Ensures years like "(2016)" or "(2026)" are never misinterpreted as volume numbers, and release
/// markers like "(f)", "(Digital)", "(Stick)" are safely ignored.
pub fn parse_archive_volume(filename: &str) -> Option<f64> {
    let clean = crate::release::strip_archive_extension(filename.trim());
    if clean.is_empty() {
        return None;
    }

    match crate::acquisition::nyaa::classify_release(clean).0 {
        AcquisitionReleaseUnit::Volume(v) => Some(v),
        _ => None,
    }
}

/// Conservative file selection algorithm:
/// 1. Filters out non-manga junk/metadata files (e.g. .nfo, .txt, .url, .jpg, .png, .db, .DS_Store, __MACOSX).
/// 2. Explicitly rejects unsupported archive/document formats (.cbr, .rar, .7z, .pdf, .epub).
/// 3. Accepts native .cbz (preferred) and validated manga .zip archives.
/// 4. If target_unit is Volume(N), searches for archive unambiguously matching Volume N.
///    - If exactly 1 matches (preferring .cbz over .zip if both exist), selects it.
///    - If 0 match, fails closed with TorBoxError::RequestedUnitNotFound.
///    - If >1 match with same format, fails closed with TorBoxError::AmbiguousFiles.
/// 5. If single archive exists and no target_unit specified, selects it.
/// 6. If multiple archives exist and no target_unit specified, fails closed with TorBoxError::AmbiguousFiles.
pub fn select_artifact<'a>(
    files: &'a [BackendFile],
    target_unit: Option<&AcquisitionReleaseUnit>,
) -> std::result::Result<&'a BackendFile, TorBoxError> {
    if files.is_empty() {
        return Err(TorBoxError::NoArchiveFound(
            "Torrent contains 0 files".to_string(),
        ));
    }

    // 1. Scan for unsupported archive/document formats
    let unsupported_files: Vec<&BackendFile> = files
        .iter()
        .filter(|f| {
            let lower = f.name.to_ascii_lowercase();
            !is_junk_file(&lower) && is_unsupported_format(&lower)
        })
        .collect();

    // 2. Filter to supported archive files (.cbz and .zip only; .cbz preferred native format)
    let archives: Vec<&BackendFile> = files
        .iter()
        .filter(|f| {
            let lower = f.name.to_ascii_lowercase();
            !is_junk_file(&lower) && (lower.ends_with(".cbz") || lower.ends_with(".zip"))
        })
        .collect();

    // 3. If target_unit is specified, verify if requested volume corresponds to an unsupported file
    if let Some(AcquisitionReleaseUnit::Volume(target_vol)) = target_unit {
        let unsupported_matching: Vec<&BackendFile> = unsupported_files
            .iter()
            .copied()
            .filter(|f| {
                parse_archive_volume(&f.name)
                    .map(|v| (v - target_vol).abs() < 0.001)
                    .unwrap_or(false)
            })
            .collect();

        if let Some(bad) = unsupported_matching.first() {
            return Err(TorBoxError::UnsupportedFormat(format!(
                "Requested volume {target_vol} matches unsupported format '{}'. Only native .cbz and validated .zip archives are supported.",
                bad.name
            )));
        }
    }

    // 4. If no supported archives exist, report unsupported format if any exist, or NoArchiveFound
    if archives.is_empty() {
        if let Some(bad) = unsupported_files.first() {
            return Err(TorBoxError::UnsupportedFormat(format!(
                "Torrent contains unsupported archive/document format '{}'. Only native .cbz and validated .zip archives are supported.",
                bad.name
            )));
        }
        return Err(TorBoxError::NoArchiveFound(
            "Torrent contains no .cbz or .zip archive files".to_string(),
        ));
    }

    // 5. Match against target_unit if specified
    if let Some(AcquisitionReleaseUnit::Volume(target_vol)) = target_unit {
        let mut parsed_vols = Vec::new();
        let mut matching: Vec<&BackendFile> = Vec::new();

        for arch in &archives {
            if let Some(vol) = parse_archive_volume(&arch.name) {
                parsed_vols.push(vol);
                if (vol - target_vol).abs() < 0.001 {
                    matching.push(arch);
                }
            }
        }

        if matching.len() == 1 {
            return Ok(matching[0]);
        }

        // Prefer native .cbz over .zip if both exist for the same target volume
        if matching.len() > 1 {
            let cbz_matching: Vec<&BackendFile> = matching
                .iter()
                .copied()
                .filter(|f| f.name.to_ascii_lowercase().ends_with(".cbz"))
                .collect();
            if cbz_matching.len() == 1 {
                return Ok(cbz_matching[0]);
            }
            return Err(TorBoxError::AmbiguousFiles(format!(
                "Batch torrent has {} archives matching requested Volume {target_vol}",
                matching.len()
            )));
        }

        // 0 matches -> RequestedUnitNotFound with useful non-sensitive diagnostics
        parsed_vols.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        parsed_vols.dedup();
        let found_summary = if parsed_vols.is_empty() {
            "none parsed".to_string()
        } else {
            parsed_vols
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        return Err(TorBoxError::RequestedUnitNotFound(format!(
            "Requested Volume {target_vol} not found among {} supported archive files (found volumes: [{}])",
            archives.len(),
            found_summary
        )));
    }

    if let Some(AcquisitionReleaseUnit::Chapter(target_ch)) = target_unit {
        let mut matching: Vec<&BackendFile> = Vec::new();
        for arch in &archives {
            let (unit, _, _) = crate::acquisition::nyaa::classify_release(&arch.name);
            let is_match = match unit {
                AcquisitionReleaseUnit::Chapter(c) => (c - target_ch).abs() < 0.001,
                _ => false,
            };
            if is_match {
                matching.push(arch);
            }
        }

        if matching.len() == 1 {
            return Ok(matching[0]);
        }

        if matching.len() > 1 {
            let cbz_matching: Vec<&BackendFile> = matching
                .iter()
                .copied()
                .filter(|f| f.name.to_ascii_lowercase().ends_with(".cbz"))
                .collect();
            if cbz_matching.len() == 1 {
                return Ok(cbz_matching[0]);
            }
            return Err(TorBoxError::AmbiguousFiles(format!(
                "Batch torrent has {} archives matching requested Chapter {target_ch}",
                matching.len()
            )));
        }

        return Err(TorBoxError::RequestedUnitNotFound(format!(
            "Requested Chapter {target_ch} not found among {} supported archive files",
            archives.len()
        )));
    }

    // 6. If exactly one supported archive exists and no specific target volume requested, select it
    if archives.len() == 1 {
        return Ok(archives[0]);
    }

    // 7. Ambiguous multiple archives with no exact target volume or unit specified
    Err(TorBoxError::AmbiguousFiles(format!(
        "Torrent contains {} archive files and no unambiguous volume match could be determined",
        archives.len()
    )))
}

/// Conservative collection file mapper:
/// Maps torrent files to volume numbers (e.g. Volume 1 -> BackendFile).
///
/// Rules:
/// 1. Filters out junk and metadata files.
/// 2. If any archive is an unsupported format (.cbr, .rar, etc.), fails closed with TorBoxError::UnsupportedFormat.
/// 3. Filters to supported archives (.cbz, .zip).
/// 4. Classifies volume number using parse_archive_volume.
/// 5. If multiple files map to the same volume:
///    - If one is .cbz and another is .zip, prefers .cbz.
///    - If both have identical extensions or duplicate archives exist, fails closed with TorBoxError::AmbiguousFiles.
/// 6. If an archive file has an unparseable volume number, fails closed with TorBoxError::AmbiguousFiles.
/// 7. If no archives found, fails with TorBoxError::NoArchiveFound.
pub fn map_torrent_files_to_volumes(
    files: &[BackendFile],
) -> std::result::Result<BTreeMap<i64, BackendFile>, TorBoxError> {
    if files.is_empty() {
        return Err(TorBoxError::NoArchiveFound(
            "Torrent contains 0 files".to_string(),
        ));
    }

    // 1. Scan for unsupported formats
    for f in files {
        let lower = f.name.to_ascii_lowercase();
        if !is_junk_file(&lower) && is_unsupported_format(&lower) {
            return Err(TorBoxError::UnsupportedFormat(format!(
                "Collection torrent contains unsupported archive format '{}'",
                f.name
            )));
        }
    }

    // 2. Scan supported archives
    let archives: Vec<&BackendFile> = files
        .iter()
        .filter(|f| {
            let lower = f.name.to_ascii_lowercase();
            !is_junk_file(&lower) && (lower.ends_with(".cbz") || lower.ends_with(".zip"))
        })
        .collect();

    if archives.is_empty() {
        return Err(TorBoxError::NoArchiveFound(
            "Torrent contains no .cbz or .zip volume archives".to_string(),
        ));
    }

    let mut map: BTreeMap<i64, BackendFile> = BTreeMap::new();

    for arch in archives {
        let vol_opt = parse_archive_volume(&arch.name).map(|v| v.round() as i64);

        let Some(vol) = vol_opt else {
            return Err(TorBoxError::AmbiguousFiles(format!(
                "Cannot unambiguously determine volume number for file '{}'",
                arch.name
            )));
        };

        if let Some(existing) = map.get(&vol) {
            let existing_is_cbz = existing.name.to_ascii_lowercase().ends_with(".cbz");
            let new_is_cbz = arch.name.to_ascii_lowercase().ends_with(".cbz");

            if existing_is_cbz && !new_is_cbz {
                // Keep existing .cbz, ignore duplicate .zip
                continue;
            } else if !existing_is_cbz && new_is_cbz {
                // Replace .zip with preferred .cbz
                map.insert(vol, (*arch).clone());
            } else {
                // Duplicate archives with same format
                return Err(TorBoxError::AmbiguousFiles(format!(
                    "Duplicate archives found for Volume {vol}: '{}' and '{}'",
                    existing.name, arch.name
                )));
            }
        } else {
            map.insert(vol, (*arch).clone());
        }
    }

    Ok(map)
}

fn is_junk_file(lower: &str) -> bool {
    lower.starts_with("__macosx")
        || lower.starts_with('.')
        || lower.ends_with(".nfo")
        || lower.ends_with(".txt")
        || lower.ends_with(".url")
        || lower.ends_with(".db")
        || lower.ends_with("thumbs.db")
        || lower.ends_with(".ds_store")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
}

/// Validates download URL safety (Scenario AF).
pub fn validate_download_url(url_str: &str) -> std::result::Result<(), TorBoxError> {
    if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
        return Err(TorBoxError::NetworkError("Invalid URL scheme".to_string()));
    }

    let without_scheme = url_str
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = without_scheme
        .split(&['/', ':', '?'][..])
        .next()
        .unwrap_or("");

    if host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host.starts_with("127.")
    {
        return Err(TorBoxError::NetworkError(
            "Rejected private/loopback URL host in CDN response".to_string(),
        ));
    }

    Ok(())
}

/// Compatibility entrypoint for callers without durable edition provenance.
/// Exact paths and unit evidence must corroborate selection; job-local IDs and
/// aggregate torrent size never override ambiguity or a requested-unit failure.
#[cfg(test)]
pub fn resolve_selected_file<'a>(
    files: &'a [BackendFile],
    raw_title: Option<&str>,
    target_unit: Option<&AcquisitionReleaseUnit>,
    release_size: Option<u64>,
    persisted_file_id: Option<i64>,
) -> std::result::Result<&'a BackendFile, TorBoxError> {
    if files.is_empty() {
        return Err(TorBoxError::NoArchiveFound(
            "Torrent contains 0 files".to_string(),
        ));
    }

    resolve_file_with_identity(
        files,
        raw_title,
        target_unit,
        release_size,
        persisted_file_id,
        None,
    )
}

/// Safe, bounded provider text for diagnostics. Never include URLs or magnet data.
pub fn diagnostic_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            if word.contains("://")
                || word.contains("magnet:")
                || word.contains('?')
                || word.contains("token=")
            {
                "[redacted]".to_string()
            } else {
                word.chars().filter(|c| !c.is_control()).take(160).collect()
            }
        })
        .take(30)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn validate_edition_intent(
    identity: &super::edition::RetrievalIdentity,
) -> std::result::Result<(), TorBoxError> {
    use super::edition::{edition_from_title, normalize_edition};
    if identity.selected_archive.is_some() {
        return Ok(());
    }
    let requested = identity.requested_edition.as_deref().map(normalize_edition);
    if let Some(release) = edition_from_title(&identity.source_release) {
        if requested.as_ref().is_some_and(|ed| ed != &release)
            || (requested.is_none() && release != "standard")
        {
            return Err(TorBoxError::EditionMismatch(format!(
                "Release is {release} edition; requested edition is {}. Confirm edition-local numbering; no standard/combined-volume conversion is inferred.",
                requested.as_deref().unwrap_or("unspecified")
            )));
        }
    }
    Ok(())
}

pub fn resolve_file_with_identity<'a>(
    files: &'a [BackendFile],
    raw_title: Option<&str>,
    target_unit: Option<&AcquisitionReleaseUnit>,
    _release_size: Option<u64>,
    persisted_file_id: Option<i64>,
    identity: Option<&super::edition::RetrievalIdentity>,
) -> std::result::Result<&'a BackendFile, TorBoxError> {
    use super::edition::{edition_from_title, normalize_edition};
    // Approval freezes source identity. Disposable backend IDs and later catalog
    // parsing cannot override an exact, unique path/size selection.
    if let Some(selection) = identity.and_then(|i| i.selected_archive.as_ref()) {
        if !super::source_units::safe_path(
            selection.path.strip_prefix('/').unwrap_or(&selection.path),
        ) || selection.size == 0
        {
            return Err(TorBoxError::AmbiguousFiles(
                "Unsafe approved archive path/size".into(),
            ));
        }
        let matched = files
            .iter()
            .filter(|f| f.path == selection.path && f.size == selection.size)
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            return Err(TorBoxError::AmbiguousFiles(
                "The approved file changed or is no longer uniquely available. Select it again."
                    .into(),
            ));
        }
        let file = matched[0];
        let name = file.name.to_ascii_lowercase();
        if !(name.ends_with(".cbz") || name.ends_with(".zip")) || is_junk_file(&name) {
            return Err(TorBoxError::UnsupportedFormat(
                "Approved file is not a supported archive".into(),
            ));
        }
        return Ok(file);
    }
    let target = identity.map(|i| &i.requested_unit).or(target_unit);
    let source = identity
        .map(|i| i.source_release.as_str())
        .or(raw_title)
        .unwrap_or("");
    let requested_edition = identity
        .and_then(|i| i.requested_edition.as_deref())
        .map(normalize_edition);
    let release_edition = edition_from_title(source);
    let fallback_identity = super::edition::RetrievalIdentity::new(
        "",
        source,
        target.cloned().unwrap_or(AcquisitionReleaseUnit::Unknown),
        None,
    );
    validate_edition_intent(identity.unwrap_or(&fallback_identity))?;
    if let Err(error @ TorBoxError::UnsupportedFormat(_)) = select_artifact(files, target) {
        return Err(error);
    }
    let archives = files
        .iter()
        .filter(|f| {
            let lower = f.name.to_ascii_lowercase();
            !is_junk_file(&lower) && (lower.ends_with(".cbz") || lower.ends_with(".zip"))
        })
        .collect::<Vec<_>>();
    if archives.is_empty() {
        return select_artifact(files, target);
    }
    let mut eligible = Vec::new();
    let mut wrong_edition = 0;
    let mut unknown_units = 0;
    for file in &archives {
        let file_edition = edition_from_title(&file.path)
            .or_else(|| edition_from_title(&file.name))
            .or_else(|| release_edition.clone());
        let parsed = super::source_units::archive_unit(file);
        let edition_ok = match (&requested_edition, &file_edition) {
            (Some(req), Some(actual)) => {
                req == actual
                    || (req.starts_with("source ")
                        && identity.is_some_and(|i| {
                            i.identity_override && i.override_approved_at.is_some()
                        }))
            }
            (Some(req), None) => {
                req == "standard"
                    || identity
                        .is_some_and(|i| i.identity_override && i.override_approved_at.is_some())
            }
            (None, Some(actual)) => actual == "standard",
            (None, None) => true,
        };
        let unit_ok = match (target, &parsed) {
            (Some(AcquisitionReleaseUnit::Volume(a)), AcquisitionReleaseUnit::Volume(b))
            | (Some(AcquisitionReleaseUnit::Chapter(a)), AcquisitionReleaseUnit::Chapter(b)) => {
                (a - b).abs() < 0.001
            }
            (None | Some(AcquisitionReleaseUnit::Unknown), _) => true,
            _ => false,
        };
        if !edition_ok {
            wrong_edition += 1;
        }
        if matches!(parsed, AcquisitionReleaseUnit::Unknown) {
            unknown_units += 1;
        }
        let selected = identity.and_then(|i| i.selected_archive.as_ref());
        let exact_selection =
            selected.is_some_and(|s| s.path == file.path && s.size > 0 && s.size == file.size);
        let source_unit = crate::acquisition::nyaa::classify_release(source).0;
        let source_unit_matches = match (target, &source_unit) {
            (Some(AcquisitionReleaseUnit::Volume(a)), AcquisitionReleaseUnit::Volume(b))
            | (Some(AcquisitionReleaseUnit::Chapter(a)), AcquisitionReleaseUnit::Chapter(b)) => {
                (a - b).abs() < 0.001
            }
            _ => false,
        };
        let unknown_single = matches!(parsed, AcquisitionReleaseUnit::Unknown)
            && archives.len() == 1
            && source_unit_matches;
        let accepted =
            edition_ok && (unit_ok || unknown_single) && (selected.is_none() || exact_selection);
        tracing::debug!(
            file_id = file.id, filename = %diagnostic_text(&file.name), path = %diagnostic_text(&file.path),
            parsed_unit = ?parsed, edition = ?file_edition, edition_ok, unit_ok,
            persisted_id_matches = persisted_file_id == Some(file.id), exact_selection, accepted,
            reason = if !edition_ok { "edition_mismatch_or_unproven" } else if selected.is_some() && !exact_selection { "persisted_path_size_mismatch" } else if !unit_ok && !unknown_single { "unit_mismatch_or_unknown" } else { "unit_and_edition_corroborated" },
            "TorBox archive resolution candidate"
        );
        if accepted {
            eligible.push(*file);
        }
    }
    // IDs are local to a remote job. They never override unit, edition or exact path evidence.
    // Release size is a torrent aggregate, never a per-volume tie breaker.
    if eligible.len() == 1 {
        return Ok(eligible[0]);
    }
    if eligible.len() > 1 {
        if let Some(raw) = raw_title {
            let exact = eligible
                .iter()
                .copied()
                .filter(|f| f.path == raw || f.name == raw)
                .collect::<Vec<_>>();
            if exact.len() == 1 {
                return Ok(exact[0]);
            }
        }
        return Err(TorBoxError::AmbiguousFiles(format!(
            "Requested {} {}, but {} of {} archives match; explicit file selection is required.",
            requested_edition
                .as_deref()
                .unwrap_or("unspecified edition"),
            target
                .map(ToString::to_string)
                .unwrap_or_else(|| "unit".into()),
            eligible.len(),
            archives.len()
        )));
    }
    if wrong_edition == archives.len() {
        return Err(TorBoxError::EditionMismatch(format!(
            "No archive matches requested {} edition ({} supported archives).",
            requested_edition.as_deref().unwrap_or("unspecified"),
            archives.len()
        )));
    }
    let summary = format!("Requested {} {}, but no archive uniquely matched among {} supported archives ({} unlabelled).{}",
        requested_edition.as_deref().unwrap_or("unspecified edition"),
        target.map(ToString::to_string).unwrap_or_else(|| "unit".into()), archives.len(), unknown_units,
        if identity.and_then(|i| i.selected_archive.as_ref()).is_some() { " Persisted path/size selection could not be corroborated." } else { "" });
    if unknown_units > 0 {
        Err(TorBoxError::AmbiguousFiles(summary))
    } else {
        Err(TorBoxError::RequestedUnitNotFound(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scenario_m_conservative_file_selection() {
        // Case 1: Single CBZ file
        let files1 = vec![BackendFile {
            id: 1,
            name: "Berserk v01.cbz".to_string(),
            size: 150_000_000,
            path: "Berserk v01.cbz".to_string(),
        }];
        let sel1 = select_artifact(&files1, None).unwrap();
        assert_eq!(sel1.id, 1);

        // Case 2: CBZ + covers and txt files
        let files2 = vec![
            BackendFile {
                id: 1,
                name: "cover.jpg".to_string(),
                size: 500_000,
                path: "cover.jpg".to_string(),
            },
            BackendFile {
                id: 2,
                name: "info.txt".to_string(),
                size: 1_200,
                path: "info.txt".to_string(),
            },
            BackendFile {
                id: 3,
                name: "Chainsaw Man v01.cbz".to_string(),
                size: 120_000_000,
                path: "Chainsaw Man v01.cbz".to_string(),
            },
        ];
        let sel2 = select_artifact(&files2, None).unwrap();
        assert_eq!(sel2.id, 3);

        // Case 3: Multi-volume batch with target volume requested
        let files3 = vec![
            BackendFile {
                id: 10,
                name: "Berserk v01.cbz".to_string(),
                size: 150_000_000,
                path: "Berserk v01.cbz".to_string(),
            },
            BackendFile {
                id: 11,
                name: "Berserk v02.cbz".to_string(),
                size: 155_000_000,
                path: "Berserk v02.cbz".to_string(),
            },
            BackendFile {
                id: 12,
                name: "Berserk v03.cbz".to_string(),
                size: 160_000_000,
                path: "Berserk v03.cbz".to_string(),
            },
        ];
        let sel3 = select_artifact(&files3, Some(&AcquisitionReleaseUnit::Volume(2.0))).unwrap();
        assert_eq!(sel3.id, 11);

        // Case 4: Ambiguous multiple archives with no target or missing target
        let err = select_artifact(&files3, None).unwrap_err();
        assert!(matches!(err, TorBoxError::AmbiguousFiles(_)));

        let err2 =
            select_artifact(&files3, Some(&AcquisitionReleaseUnit::Volume(99.0))).unwrap_err();
        assert!(matches!(err2, TorBoxError::RequestedUnitNotFound(_)));

        // Case 5: Unsupported formats rejected as UnsupportedFormat
        let files_cbr = vec![BackendFile {
            id: 20,
            name: "Berserk v01.cbr".to_string(),
            size: 150_000_000,
            path: "Berserk v01.cbr".to_string(),
        }];
        let err_cbr = select_artifact(&files_cbr, None).unwrap_err();
        assert!(matches!(err_cbr, TorBoxError::UnsupportedFormat(_)));

        let files_7z = vec![BackendFile {
            id: 21,
            name: "Berserk v01.7z".to_string(),
            size: 150_000_000,
            path: "Berserk v01.7z".to_string(),
        }];
        let err_7z = select_artifact(&files_7z, None).unwrap_err();
        assert!(matches!(err_7z, TorBoxError::UnsupportedFormat(_)));

        let files_pdf = vec![BackendFile {
            id: 22,
            name: "Berserk v01.pdf".to_string(),
            size: 150_000_000,
            path: "Berserk v01.pdf".to_string(),
        }];
        let err_pdf = select_artifact(&files_pdf, None).unwrap_err();
        assert!(matches!(err_pdf, TorBoxError::UnsupportedFormat(_)));

        let files_epub = vec![BackendFile {
            id: 23,
            name: "Berserk v01.epub".to_string(),
            size: 150_000_000,
            path: "Berserk v01.epub".to_string(),
        }];
        let err_epub = select_artifact(&files_epub, None).unwrap_err();
        assert!(matches!(err_epub, TorBoxError::UnsupportedFormat(_)));

        // Case 6: Target volume matching an unsupported file fails closed with UnsupportedFormat
        let files_mixed = vec![
            BackendFile {
                id: 30,
                name: "Berserk v01.cbz".to_string(),
                size: 150_000_000,
                path: "Berserk v01.cbz".to_string(),
            },
            BackendFile {
                id: 31,
                name: "Berserk v02.cbr".to_string(),
                size: 155_000_000,
                path: "Berserk v02.cbr".to_string(),
            },
        ];
        let err_target_cbr =
            select_artifact(&files_mixed, Some(&AcquisitionReleaseUnit::Volume(2.0))).unwrap_err();
        assert!(matches!(err_target_cbr, TorBoxError::UnsupportedFormat(_)));

        // Case 7: Prefer native .cbz over .zip if both exist for the same target volume
        let files_cbz_and_zip = vec![
            BackendFile {
                id: 40,
                name: "Berserk v01.zip".to_string(),
                size: 150_000_000,
                path: "Berserk v01.zip".to_string(),
            },
            BackendFile {
                id: 41,
                name: "Berserk v01.cbz".to_string(),
                size: 150_000_000,
                path: "Berserk v01.cbz".to_string(),
            },
        ];
        let sel_pref = select_artifact(
            &files_cbz_and_zip,
            Some(&AcquisitionReleaseUnit::Volume(1.0)),
        )
        .unwrap();
        assert_eq!(sel_pref.id, 41); // Selected .cbz!
    }

    #[test]
    fn test_scenario_af_url_safety_check() {
        assert!(validate_download_url("https://cdn.torbox.app/files/123/manga.cbz").is_ok());
        assert!(validate_download_url("http://127.0.0.1/evil").is_err());
        assert!(validate_download_url("http://localhost:8080/evil").is_err());
        assert!(validate_download_url("ftp://cdn.torbox.app/manga.cbz").is_err());
    }

    #[test]
    fn test_scenario_ag_stalled_zero_seeders() {
        let item = TorrentItemData {
            id: serde_json::Value::Number(1.into()),
            name: Some("Berserk".into()),
            hash: Some("abc".into()),
            download_state: Some("downloading".into()),
            progress: Some(0.0),
            download_speed: Some(0),
            seeds: Some(0),
            peers: Some(0),
            files: None,
        };

        // If seeds is 0 and progress is 0.0 -> stalled
        let status = if item.seeds.unwrap_or(0) == 0 && item.progress.unwrap_or(0.0) == 0.0 {
            BackendJobStatus::Stalled {
                reason: "Swarm has 0 seeders or torrent is stalled on TorBox".to_string(),
            }
        } else {
            BackendJobStatus::Completed
        };
        assert!(matches!(status, BackendJobStatus::Stalled { .. }));
    }

    #[test]
    fn test_blame_filename_parsing_and_selection() {
        let blame_files = vec![
            BackendFile {
                id: 101,
                name: "BLAME! Master Edition v01 (2016) (Digital) (danke-Empire).cbz".to_string(),
                size: 500_000_000,
                path: "BLAME! Master Edition v01 (2016) (Digital) (danke-Empire).cbz".to_string(),
            },
            BackendFile {
                id: 102,
                name: "BLAME! Master Edition v02 (2016) (Digital) (danke-Empire).cbz".to_string(),
                size: 510_000_000,
                path: "BLAME! Master Edition v02 (2016) (Digital) (danke-Empire).cbz".to_string(),
            },
            BackendFile {
                id: 103,
                name: "BLAME! Master Edition v03 (2017) (Digital) (danke-Empire).cbz".to_string(),
                size: 520_000_000,
                path: "BLAME! Master Edition v03 (2017) (Digital) (danke-Empire).cbz".to_string(),
            },
            BackendFile {
                id: 104,
                name: "BLAME! Master Edition v04 (2017) (Digital) (danke-Empire).cbz".to_string(),
                size: 530_000_000,
                path: "BLAME! Master Edition v04 (2017) (Digital) (danke-Empire).cbz".to_string(),
            },
            BackendFile {
                id: 105,
                name: "BLAME! Master Edition v05 (2017) (Digital) (danke-Empire).cbz".to_string(),
                size: 540_000_000,
                path: "BLAME! Master Edition v05 (2017) (Digital) (danke-Empire).cbz".to_string(),
            },
            BackendFile {
                id: 106,
                name: "BLAME! Master Edition v06 (2017) (F) (Digital) (danke-Empire).cbz"
                    .to_string(),
                size: 550_000_000,
                path: "BLAME! Master Edition v06 (2017) (F) (Digital) (danke-Empire).cbz"
                    .to_string(),
            },
        ];

        // 1. Unique mapping to Volumes 1-6
        let mapped = map_torrent_files_to_volumes(&blame_files).expect("mapping failed");
        assert_eq!(mapped.len(), 6);
        for i in 1..=6 {
            assert!(mapped.contains_key(&i));
            assert_eq!(mapped.get(&i).unwrap().id, 100 + i);
        }

        // 2. parse_archive_volume handles (F) marker and rejects publication years (2016, 2017)
        assert_eq!(parse_archive_volume(&blame_files[0].name), Some(1.0));
        assert_eq!(parse_archive_volume(&blame_files[5].name), Some(6.0));

        // 3. Selection of individual volumes
        let sel1 =
            select_artifact(&blame_files, Some(&AcquisitionReleaseUnit::Volume(1.0))).unwrap();
        assert_eq!(sel1.id, 101);

        let sel6 =
            select_artifact(&blame_files, Some(&AcquisitionReleaseUnit::Volume(6.0))).unwrap();
        assert_eq!(sel6.id, 106);

        // Missing volume 7 fails with RequestedUnitNotFound
        let err7 =
            select_artifact(&blame_files, Some(&AcquisitionReleaseUnit::Volume(7.0))).unwrap_err();
        assert!(matches!(err7, TorBoxError::RequestedUnitNotFound(_)));
    }

    #[test]
    fn test_blue_lock_v01_39_fixture_and_selective_retrieval() {
        let mut blue_lock_files = Vec::new();
        for i in 1..=39 {
            let year = 2021 + (i / 8);
            let f_marker = if i % 5 == 0 { " (f)" } else { "" };
            let filename = format!(
                "Blue Lock v{:02} ({year}){f_marker} (Digital) (Stick).cbz",
                i
            );
            blue_lock_files.push(BackendFile {
                id: 1000 + i as i64,
                name: filename.clone(),
                size: 80_000_000 + (i as u64 * 1_000_000),
                path: filename,
            });
        }

        // 1. Requested Volume 39 -> exactly v39 selected (ID 1039)
        let sel39 = select_artifact(
            &blue_lock_files,
            Some(&AcquisitionReleaseUnit::Volume(39.0)),
        )
        .unwrap();
        assert_eq!(sel39.id, 1039);
        assert_eq!(sel39.name, "Blue Lock v39 (2025) (Digital) (Stick).cbz");

        // 2. Requested Volume 26 -> exactly v26 selected (ID 1026)
        let sel26 = select_artifact(
            &blue_lock_files,
            Some(&AcquisitionReleaseUnit::Volume(26.0)),
        )
        .unwrap();
        assert_eq!(sel26.id, 1026);

        // 3. Requested Volume 40 -> RequestedUnitNotFound
        let err40 = select_artifact(
            &blue_lock_files,
            Some(&AcquisitionReleaseUnit::Volume(40.0)),
        )
        .unwrap_err();
        assert!(matches!(err40, TorBoxError::RequestedUnitNotFound(_)));
        let err_msg = err40.to_string();
        assert!(err_msg.contains("Volume 40"));
        assert!(err_msg.contains("found volumes"));

        // 4. Multi-select: units [31, 32, 35, 39]
        let selected_units = vec![31.0, 32.0, 35.0, 39.0];
        let mut chosen_ids = Vec::new();
        for u in selected_units {
            let f = select_artifact(&blue_lock_files, Some(&AcquisitionReleaseUnit::Volume(u)))
                .unwrap();
            chosen_ids.push(f.id);
        }
        assert_eq!(chosen_ids, vec![1031, 1032, 1035, 1039]);
    }

    #[test]
    fn test_genuine_ambiguity_fails_closed() {
        // Case 1: Two files parsing as the exact same volume (same format)
        let dup_files = vec![
            BackendFile {
                id: 201,
                name: "Blue Lock v39 (Digital) (GroupA).cbz".to_string(),
                size: 80_000_000,
                path: "Blue Lock v39 (Digital) (GroupA).cbz".to_string(),
            },
            BackendFile {
                id: 202,
                name: "Blue Lock v39 (Digital) (GroupB).cbz".to_string(),
                size: 85_000_000,
                path: "Blue Lock v39 (Digital) (GroupB).cbz".to_string(),
            },
        ];
        let err_dup =
            select_artifact(&dup_files, Some(&AcquisitionReleaseUnit::Volume(39.0))).unwrap_err();
        assert!(matches!(err_dup, TorBoxError::AmbiguousFiles(_)));

        // Case 2: Multiple archives with no target unit
        let err_no_target = select_artifact(&dup_files, None).unwrap_err();
        assert!(matches!(err_no_target, TorBoxError::AmbiguousFiles(_)));
    }
}

#[cfg(test)]
mod identity_regressions {
    use super::*;
    use crate::acquisition::edition::{RetrievalIdentity, SelectedArchive};

    fn archive(id: i64, name: &str) -> BackendFile {
        BackendFile {
            id,
            name: name.into(),
            path: format!("collection/{name}"),
            size: 100,
        }
    }
    fn request(source: &str, edition: Option<&str>, volume: f64) -> RetrievalIdentity {
        RetrievalIdentity::new(
            "Series X",
            source,
            AcquisitionReleaseUnit::Volume(volume),
            edition,
        )
    }
    fn resolve<'a>(
        files: &'a [BackendFile],
        identity: &RetrievalIdentity,
        id: Option<i64>,
    ) -> std::result::Result<&'a BackendFile, TorBoxError> {
        resolve_file_with_identity(files, None, None, Some(600), id, Some(identity))
    }

    #[test]
    fn ordinary_and_standard_collections() {
        let files = vec![
            archive(1, "Series X v01.cbz"),
            archive(2, "Series X v02.cbz"),
        ];
        assert_eq!(
            resolve(&files[..1], &request("Series X v01", None, 1.0), None)
                .unwrap()
                .id,
            1
        );
        assert_eq!(
            resolve(
                &files,
                &request("Series X v01-02", Some("Standard Edition"), 2.0),
                None
            )
            .unwrap()
            .id,
            2
        );
    }

    #[test]
    fn master_collection_matches_edition_local_volume_in_any_order() {
        let mut files = (1..=6)
            .map(|n| archive(n, &format!("Series X Master Edition v{n:02}.cbz")))
            .collect::<Vec<_>>();
        let identity = request(
            "Series X Master Edition (Digital)",
            Some("Master Edition"),
            4.0,
        );
        assert_eq!(resolve(&files, &identity, None).unwrap().id, 4);
        files.reverse();
        assert_eq!(resolve(&files, &identity, Some(1)).unwrap().id, 4);
    }

    #[test]
    fn edition_mismatch_and_unknown_combined_mapping_fail_closed() {
        let files = vec![archive(1, "Series X Master Edition v04.cbz")];
        for edition in [Some("Standard Edition"), Some("Deluxe Edition"), None] {
            assert!(matches!(
                resolve(
                    &files,
                    &request("Series X Master Edition", edition, 4.0),
                    Some(1)
                ),
                Err(TorBoxError::EditionMismatch(_))
            ));
        }
        let files = vec![archive(1, "Series X 3-in-1 v02.cbz")];
        assert!(matches!(
            resolve(
                &files,
                &request("Series X 3-in-1", Some("Standard Edition"), 4.0),
                None
            ),
            Err(TorBoxError::EditionMismatch(_))
        ));
    }

    #[test]
    fn same_edition_duplicates_fail_closed_even_with_id_or_torrent_size() {
        let files = vec![
            archive(1, "Series X Master Edition v04 A.cbz"),
            archive(2, "Series X Master Edition v04 B.cbz"),
        ];
        assert!(matches!(
            resolve(
                &files,
                &request("Series X Master Edition", Some("master"), 4.0),
                Some(1)
            ),
            Err(TorBoxError::AmbiguousFiles(_))
        ));
    }

    #[test]
    fn absent_unit_and_no_order_guessing() {
        let files = vec![
            archive(4, "Series X v01.cbz"),
            archive(1, "Series X v02.cbz"),
        ];
        assert!(matches!(
            resolve(&files, &request("Series X v01-02", None, 4.0), Some(4)),
            Err(TorBoxError::RequestedUnitNotFound(_))
        ));
        let files = vec![archive(4, "a.cbz"), archive(1, "b.cbz")];
        assert!(matches!(
            resolve(&files, &request("Series X", None, 1.0), Some(1)),
            Err(TorBoxError::AmbiguousFiles(_))
        ));
    }

    #[test]
    fn exact_selection_roundtrip_and_recreated_job_ids() {
        let files = vec![
            archive(7, "Series X v04 A.cbz"),
            archive(8, "Series X v04 B.cbz"),
        ];
        let mut identity = request("Series X v01-06", None, 4.0);
        identity.selected_archive = Some(SelectedArchive {
            path: files[1].path.clone(),
            size: 100,
        });
        let identity: RetrievalIdentity =
            serde_json::from_str(&serde_json::to_string(&identity).unwrap()).unwrap();
        assert_eq!(resolve(&files, &identity, Some(8)).unwrap().id, 8);
        let recreated = vec![
            archive(8, "Series X v04 A.cbz"),
            archive(99, "Series X v04 B.cbz"),
        ];
        assert_eq!(resolve(&recreated, &identity, Some(8)).unwrap().id, 99);
        let changed = vec![archive(8, "Series X v04 A.cbz")];
        assert!(resolve(&changed, &identity, Some(8)).is_err());
    }

    #[test]
    fn exact_name_never_overrides_wrong_unit_or_edition() {
        let files = vec![archive(1, "Series X v02.cbz")];
        assert!(resolve_file_with_identity(
            &files,
            Some(&files[0].name),
            Some(&AcquisitionReleaseUnit::Volume(4.0)),
            None,
            Some(1),
            None
        )
        .is_err());
    }

    #[test]
    fn collection_controls_and_chapter_volume_distinction() {
        for (series, total, requested) in [
            ("Witch Hat Atelier", 14, 4),
            ("Blue Lock", 39, 39),
            ("Made in Abyss", 13, 4),
        ] {
            let mut files = (1..=total)
                .map(|n| archive(n, &format!("{series} v{n:02} (Digital).cbz")))
                .collect::<Vec<_>>();
            files.reverse();
            assert_eq!(
                resolve(
                    &files,
                    &request(&format!("{series} v01-{total}"), None, requested as f64),
                    None
                )
                .unwrap()
                .id,
                requested
            );
        }
        assert_eq!(parse_archive_volume("Series X c04.cbz"), None);
        assert_eq!(parse_archive_volume("Series X v04.5.cbz"), Some(4.5));
        assert_eq!(parse_archive_volume("Series X v01-06.cbz"), None);
    }

    #[test]
    fn diagnostics_are_bounded_unicode_safe_and_redact_urls() {
        let safe = diagnostic_text(
            "Series 나 v04.cbz https://cdn.example/file?token=SECRET magnet:?xt=SECRET",
        );
        assert!(safe.contains("나 v04.cbz"));
        assert!(!safe.contains("SECRET"));
        assert!(diagnostic_text(&"📚".repeat(500)).chars().count() <= 160);
    }
}

#[cfg(test)]
mod structured_diagnostic_tests {
    use super::*;
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn candidate_trace_contains_evidence_without_signed_urls() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Capture(bytes.clone());
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        let files = vec![BackendFile {
            id: 1,
            name: "Series X 나 v04.cbz".into(),
            size: 100,
            path: "https://cdn.example/archive?token=TOKEN_SHOULD_NOT_APPEAR".into(),
        }];
        tracing::subscriber::with_default(subscriber, || {
            resolve_selected_file(
                &files,
                None,
                Some(&AcquisitionReleaseUnit::Volume(4.0)),
                None,
                None,
            )
            .unwrap();
        });
        let logged = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        for evidence in [
            "Series X 나 v04.cbz",
            "parsed_unit",
            "edition_ok",
            "unit_ok",
            "reason",
            "accepted",
            "[redacted]",
        ] {
            assert!(logged.contains(evidence), "Missing {evidence} in {logged}");
        }
        assert!(!logged.contains("TOKEN_SHOULD_NOT_APPEAR"));
        assert!(!logged.contains("https://cdn.example"));
    }
}

#[cfg(test)]
mod override_identity_tests {
    use super::*;
    use crate::acquisition::edition::{RetrievalIdentity, SelectedArchive};
    #[test]
    fn override_requires_approval_and_never_guesses_a_file() {
        let mut identity = RetrievalIdentity::new(
            "Series X",
            "Unverified release",
            AcquisitionReleaseUnit::Volume(4.0),
            Some("source release4"),
        );
        identity.identity_override = true;
        let files = vec![BackendFile {
            id: 1,
            name: "Series X v04.cbz".into(),
            path: "Series X v04.cbz".into(),
            size: 100,
        }];
        assert!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity)).is_err()
        );
        identity.override_approved_at = Some(1);
        assert_eq!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity))
                .unwrap()
                .id,
            1
        );
        let mut ambiguous = files.clone();
        ambiguous.push(BackendFile {
            id: 2,
            name: "Series X v04 B.cbz".into(),
            path: "Series X v04 B.cbz".into(),
            size: 100,
        });
        assert!(matches!(
            resolve_file_with_identity(&ambiguous, None, None, None, None, Some(&identity)),
            Err(TorBoxError::AmbiguousFiles(_))
        ));
        identity.selected_archive = Some(SelectedArchive {
            path: files[0].path.clone(),
            size: 100,
        });
        assert_eq!(
            resolve_file_with_identity(&ambiguous, None, None, None, None, Some(&identity))
                .unwrap()
                .id,
            1
        );
        let changed = vec![BackendFile {
            id: 1,
            name: "Series X v04.cbz".into(),
            path: "Series X v04.cbz".into(),
            size: 200,
        }];
        assert!(
            resolve_file_with_identity(&changed, None, None, None, None, Some(&identity)).is_err()
        );
    }
    #[test]
    fn override_cannot_bypass_unsupported_archive_or_known_edition_conflict() {
        let mut identity = RetrievalIdentity::new(
            "Series X",
            "Series X Master Edition",
            AcquisitionReleaseUnit::Volume(4.0),
            Some("Standard Edition"),
        );
        identity.identity_override = true;
        identity.override_approved_at = Some(1);
        let files = vec![BackendFile {
            id: 1,
            name: "Series X Master Edition v04.cbz".into(),
            path: "Series X Master Edition v04.cbz".into(),
            size: 100,
        }];
        assert!(matches!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity)),
            Err(TorBoxError::EditionMismatch(_))
        ));
        identity.requested_edition = Some("master".into());
        let files = vec![BackendFile {
            id: 1,
            name: "Series X Master Edition v04.cbr".into(),
            path: "Series X Master Edition v04.cbr".into(),
            size: 100,
        }];
        assert!(matches!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity)),
            Err(TorBoxError::UnsupportedFormat(_))
        ));
    }
}

#[cfg(test)]
mod approved_source_tests {
    use super::*;
    #[test]
    fn exact_approval_survives_metadata_disagreement_and_changed_ids_but_not_changed_files() {
        let mut identity = crate::acquisition::edition::RetrievalIdentity::new(
            "Series",
            "Master Edition",
            AcquisitionReleaseUnit::Volume(4.),
            Some("master"),
        );
        identity.selected_archive = Some(crate::acquisition::edition::SelectedArchive {
            path: "release/unknown.zip".into(),
            size: 100,
        });
        let mut files = vec![BackendFile {
            id: 900,
            name: "unknown.zip".into(),
            path: "release/unknown.zip".into(),
            size: 100,
        }];
        assert_eq!(
            resolve_file_with_identity(&files, None, None, None, Some(1), Some(&identity))
                .unwrap()
                .id,
            900
        );
        files[0].id = 901;
        assert!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity)).is_ok()
        );
        files[0].size = 101;
        assert!(
            resolve_file_with_identity(&files, None, None, None, None, Some(&identity)).is_err()
        );
    }
}
