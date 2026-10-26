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

    // First attempt using crate::release::parse_volume which handles "v01", "v1", "v001",
    // "vol 1", "vol. 1", "volume 1", "Volume 01", etc. and guards against chapter revisions
    if let Some(vol) = crate::release::parse_volume(clean) {
        return Some((vol.round() * 1000.0).round() / 1000.0);
    }

    // Fall back to classify_release if parse_volume did not identify it
    let (unit, num, _) = crate::acquisition::nyaa::classify_release(clean);
    match unit {
        AcquisitionReleaseUnit::Volume(v) => Some((v.round() * 1000.0).round() / 1000.0),
        _ => num.map(|n| (n.round() * 1000.0).round() / 1000.0),
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
            let (unit, num, _) = crate::acquisition::nyaa::classify_release(&arch.name);
            let is_match = match unit {
                AcquisitionReleaseUnit::Chapter(c) => (c - target_ch).abs() < 0.001,
                _ => num.map(|n| (n - target_ch).abs() < 0.001).unwrap_or(false),
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

/// Deterministic file resolution algorithm for TorBox torrents:
/// 1. Filters out junk and unsupported formats using the conservative selection rules.
/// 2. If exact normalized raw_title matches an archive in files, selects it.
/// 3. If target_unit is specified, selects the archive matching the requested volume/chapter via `select_artifact`.
///    If release_size is known (> 0), corroborates that the selected file size matches or is within reasonable range.
/// 4. If persisted_file_id is specified and matches a supported archive in the current torrent that is consistent with unit/title, selects it.
/// 5. If multiple files match without resolution or 0 files match, fails closed with explicit error.
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

    // 1. Scan and filter supported archives (.cbz / .zip)
    let supported_archives: Vec<&BackendFile> = files
        .iter()
        .filter(|f| {
            let lower = f.name.to_ascii_lowercase();
            !is_junk_file(&lower) && (lower.ends_with(".cbz") || lower.ends_with(".zip"))
        })
        .collect();

    if supported_archives.is_empty() {
        // Run select_artifact to get the precise unsupported format error if applicable
        return select_artifact(files, target_unit);
    }

    // 2. Exact normalized raw_title / file name matching
    if let Some(raw) = raw_title.map(str::trim).filter(|s| !s.is_empty()) {
        let clean_target = Path::new(raw)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(raw)
            .trim();

        let exact_matches: Vec<&BackendFile> = supported_archives
            .iter()
            .copied()
            .filter(|f| {
                let f_clean = Path::new(&f.name)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&f.name)
                    .trim();
                let f_path_clean = Path::new(&f.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&f.path)
                    .trim();
                f_clean.eq_ignore_ascii_case(clean_target)
                    || f_path_clean.eq_ignore_ascii_case(clean_target)
            })
            .collect();

        if exact_matches.len() == 1 {
            let selected = exact_matches[0];
            if let Some(expected_sz) = release_size.filter(|&sz| sz > 0) {
                if selected.size > 0 && selected.size != expected_sz {
                    tracing::info!(
                        "[TorBox] File '{}' matched raw title '{}' (size {} vs corroborated expected size {})",
                        selected.name,
                        clean_target,
                        selected.size,
                        expected_sz
                    );
                }
            }
            return Ok(selected);
        }

        if exact_matches.len() > 1 {
            // If multiple exact name matches, attempt to disambiguate by release_size
            if let Some(expected_sz) = release_size.filter(|&sz| sz > 0) {
                let size_matches: Vec<&BackendFile> = exact_matches
                    .iter()
                    .copied()
                    .filter(|f| f.size == expected_sz)
                    .collect();
                if size_matches.len() == 1 {
                    return Ok(size_matches[0]);
                }
            }
            return Err(TorBoxError::AmbiguousFiles(format!(
                "Multiple files ({}) match exact raw title '{clean_target}'",
                exact_matches.len()
            )));
        }
    }

    // 3. Requested unit semantics matching
    if let Some(unit) = target_unit {
        match select_artifact(files, Some(unit)) {
            Ok(file) => {
                if let Some(expected_sz) = release_size.filter(|&sz| sz > 0) {
                    if file.size > 0 && file.size != expected_sz {
                        tracing::info!(
                            "[TorBox] File '{}' matched target unit {:?} (size {} vs corroborated expected size {})",
                            file.name,
                            unit,
                            file.size,
                            expected_sz
                        );
                    }
                }
                return Ok(file);
            }
            Err(e) => {
                if target_unit.is_some() && raw_title.is_none() {
                    return Err(e);
                }
            }
        }
    }

    // 4. Corroborated persisted file ID (only if still matching and no other match found)
    if let Some(fid) = persisted_file_id {
        if let Some(file) = supported_archives.iter().copied().find(|f| f.id == fid) {
            return Ok(file);
        }
    }

    // 5. Fall back to select_artifact without unit (single archive in torrent)
    select_artifact(files, None)
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
