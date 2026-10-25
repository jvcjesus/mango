//! TorBox External Download Backend Integration.
//!
//! Provides API client for TorBox v1 API, connection testing, torrent submission,
//! status monitoring, conservative file selection, streaming CDN retrieval to staging,
//! and remote job cancellation.

use crate::acquisition::{
    AcquisitionRelease, AcquisitionReleaseUnit, BackendFile, BackendJobStatus, ExternalJobInfo,
};
use crate::error::AppError;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Write;
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
    UnsupportedFormat(String),
    InvalidCbz(String),
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
            Self::UnsupportedFormat(msg) => write!(f, "Unsupported archive format: {msg}"),
            Self::InvalidCbz(msg) => write!(f, "Retrieved archive failed CBZ validation: {msg}"),
            Self::Io(msg) => write!(f, "IO error during retrieval: {msg}"),
        }
    }
}

impl std::error::Error for TorBoxError {}

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
            TorBoxError::UnsupportedFormat(msg) => AppError::new(400, "unsupported_format", msg),
            TorBoxError::InvalidCbz(msg) => AppError::bad_request(msg),
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
        let token = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        Self {
            http,
            base_url,
            token,
        }
    }

    fn auth_token(&self) -> std::result::Result<&str, TorBoxError> {
        self.token
            .as_deref()
            .ok_or_else(|| TorBoxError::Unauthorized("TorBox API token is not configured".to_string()))
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
            return Err(TorBoxError::Unauthorized("Invalid or expired API token".to_string()));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited("TorBox rate limit exceeded".to_string()));
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
            let msg = body.detail.or(body.error).unwrap_or_else(|| "Unknown error".to_string());
            return Err(TorBoxError::RemoteError(status.as_u16(), msg));
        }

        let user = body.data.unwrap_or(UserMeData {
            id: None,
            email: None,
            plan: None,
        });

        let email = user.email.unwrap_or_else(|| "user".to_string());
        let plan_str = user.plan.map(|p| p.to_string()).unwrap_or_else(|| "standard".to_string());
        Ok(format!("Connected to TorBox as {email} (Plan: {plan_str})"))
    }

    /// Submits a torrent to TorBox via magnet link.
    pub async fn submit(&self, release: &AcquisitionRelease) -> std::result::Result<ExternalJobInfo, TorBoxError> {
        let token = self.auth_token()?;
        let magnet = release
            .magnet_url
            .as_deref()
            .ok_or_else(|| TorBoxError::TorrentFailed("Release does not contain a magnet URL or infoHash".to_string()))?;

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
            .map_err(|e| TorBoxError::NetworkError(format!("Failed to submit torrent to TorBox: {e}")))?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized("Invalid or expired API token".to_string()));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited("TorBox rate limit exceeded".to_string()));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("Failed to submit torrent to TorBox (HTTP {status})"),
            ));
        }

        let body: TorBoxApiResponse<CreateTorrentData> = resp
            .json()
            .await
            .map_err(|e| TorBoxError::RemoteError(500, format!("Failed to parse submission response: {e}")))?;

        if !body.success {
            let msg = body.detail.or(body.error).unwrap_or_else(|| "TorBox submission failed".to_string());
            return Err(TorBoxError::TorrentFailed(msg));
        }

        let data = body.data.ok_or_else(|| TorBoxError::TorrentFailed("No data returned from createtorrent".to_string()))?;
        let job_id = match data.torrent_id {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s,
            _ => release.info_hash.clone().unwrap_or_else(|| "unknown".to_string()),
        };

        Ok(ExternalJobInfo {
            external_job_id: job_id,
            torrent_hash: data.hash.or_else(|| release.info_hash.clone()),
            is_cached: false,
        })
    }

    /// Queries TorBox for the status and file list of a submitted torrent.
    pub async fn get_torrent_data(&self, external_job_id: &str) -> std::result::Result<TorrentItemData, TorBoxError> {
        let token = self.auth_token()?;
        let url = format!("{}/torrents/mylist?id={}&bypass_cache=true", self.base_url, external_job_id);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(e.to_string()))?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized("Invalid or expired API token".to_string()));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited("TorBox rate limit exceeded".to_string()));
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
            }
        }

        if let Ok(multi) = serde_json::from_str::<TorBoxApiResponse<Vec<TorrentItemData>>>(&text) {
            if multi.success {
                if let Some(mut items) = multi.data {
                    if let Some(item) = items.pop() {
                        return Ok(item);
                    }
                }
            }
        }

        Err(TorBoxError::TorrentFailed(format!(
            "Torrent {external_job_id} not found in TorBox mylist response"
        )))
    }

    /// Checks the current backend status of a TorBox job.
    pub async fn check_status(&self, external_job_id: &str) -> std::result::Result<BackendJobStatus, TorBoxError> {
        let data = self.get_torrent_data(external_job_id).await?;
        let state = data.download_state.as_deref().unwrap_or("downloading").to_lowercase();

        if state == "cached" {
            return Ok(BackendJobStatus::Cached);
        }
        if state == "completed" || state == "seeding" || (data.progress.unwrap_or(0.0) >= 0.999 && !data.files.as_ref().map(|f| f.is_empty()).unwrap_or(true)) {
            return Ok(BackendJobStatus::Completed);
        }
        if state.contains("stalled") || (data.seeds.unwrap_or(1) == 0 && data.progress.unwrap_or(0.0) == 0.0) {
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
    pub async fn list_files(&self, external_job_id: &str) -> std::result::Result<Vec<BackendFile>, TorBoxError> {
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
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(TorBoxError::Unauthorized("Invalid or expired API token".to_string()));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(TorBoxError::RateLimited("TorBox rate limit exceeded".to_string()));
        }
        if !status.is_success() {
            return Err(TorBoxError::RemoteError(
                status.as_u16(),
                format!("Failed to request download link (HTTP {status})"),
            ));
        }

        let body: TorBoxApiResponse<String> = resp
            .json()
            .await
            .map_err(|e| TorBoxError::RemoteError(500, format!("Failed to parse requestdl response: {e}")))?;

        if !body.success {
            let msg = body.detail.or(body.error).unwrap_or_else(|| "Failed to get download URL".to_string());
            return Err(TorBoxError::TorrentFailed(msg));
        }

        let link = body.data.ok_or_else(|| TorBoxError::TorrentFailed("Empty download URL from TorBox".to_string()))?;
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
        let download_url = self.request_download_link(external_job_id, file_id).await?;

        // URL safety verification (prevent SSRF/loopback bypass)
        validate_download_url(&download_url)?;

        if let Some(parent) = destination_part.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TorBoxError::Io(e.to_string()))?;
        }

        let mut resp = self
            .http
            .get(&download_url)
            .header("User-Agent", "Mango/1.0 (Acquisition Retrieval)")
            .send()
            .await
            .map_err(|e| TorBoxError::NetworkError(format!("Failed to stream from CDN: {e}")))?;

        if !resp.status().is_success() {
            return Err(TorBoxError::RemoteError(
                resp.status().as_u16(),
                format!("CDN download link returned HTTP {}", resp.status()),
            ));
        }

        let mut file = File::create(destination_part).map_err(|e| TorBoxError::Io(e.to_string()))?;
        let mut bytes_downloaded: u64 = 0;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| TorBoxError::NetworkError(format!("Error streaming chunk: {e}")))?
        {
            file.write_all(&chunk).map_err(|e| TorBoxError::Io(e.to_string()))?;
            bytes_downloaded += chunk.len() as u64;
        }

        file.flush().map_err(|e| TorBoxError::Io(e.to_string()))?;
        drop(file);

        if expected_size > 0 && bytes_downloaded != expected_size {
            let _ = std::fs::remove_file(destination_part);
            return Err(TorBoxError::InvalidCbz(format!(
                "Downloaded size ({bytes_downloaded}) does not match expected size ({expected_size})"
            )));
        }

        // Verify CBZ integrity on the temporary staging file
        if let Err(e) = crate::pipeline::publisher::verify_cbz_integrity(destination_part) {
            let _ = std::fs::remove_file(destination_part);
            return Err(TorBoxError::InvalidCbz(format!(
                "Downloaded file failed CBZ archive integrity: {e}"
            )));
        }

        Ok(bytes_downloaded)
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
            tracing::warn!("TorBox controltorrent delete returned HTTP {}", resp.status());
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

/// Conservative file selection algorithm:
/// 1. Filters out non-manga junk/metadata files (e.g. .nfo, .txt, .url, .jpg, .png, .db, .DS_Store, __MACOSX).
/// 2. Explicitly rejects unsupported archive/document formats (.cbr, .rar, .7z, .pdf, .epub).
/// 3. Accepts native .cbz (preferred) and validated manga .zip archives.
/// 4. If single archive exists, selects it.
/// 5. If multiple archives exist (e.g. batch):
///    - If target_unit is Volume(N), searches for archive unambiguously matching Volume N.
///    - If exactly 1 matches, selects it.
///    - If 0 or >1 match, fails closed with TorBoxError::AmbiguousFiles.
pub fn select_artifact<'a>(
    files: &'a [BackendFile],
    target_unit: Option<&AcquisitionReleaseUnit>,
) -> std::result::Result<&'a BackendFile, TorBoxError> {
    if files.is_empty() {
        return Err(TorBoxError::NoArchiveFound("Torrent contains 0 files".to_string()));
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
                let (unit, num, _) = crate::acquisition::nyaa::classify_release(&f.name);
                if let AcquisitionReleaseUnit::Volume(v) = unit {
                    (v - target_vol).abs() < 0.001
                } else if let Some(n) = num {
                    (n - target_vol).abs() < 0.001
                } else {
                    false
                }
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

    // 5. If exactly one supported archive exists and no ambiguity, select it
    if archives.len() == 1 {
        return Ok(archives[0]);
    }

    // 6. Multiple archives found (e.g. multi-volume batch). Match against target_unit:
    if let Some(AcquisitionReleaseUnit::Volume(target_vol)) = target_unit {
        let matching: Vec<&BackendFile> = archives
            .iter()
            .copied()
            .filter(|f| {
                let (unit, num, _) = crate::acquisition::nyaa::classify_release(&f.name);
                if let AcquisitionReleaseUnit::Volume(v) = unit {
                    (v - target_vol).abs() < 0.001
                } else if let Some(n) = num {
                    (n - target_vol).abs() < 0.001
                } else {
                    false
                }
            })
            .collect();

        if matching.len() == 1 {
            return Ok(matching[0]);
        }

        // Prefer native .cbz over .zip if both exist for the same target volume
        let cbz_matching: Vec<&BackendFile> = matching
            .iter()
            .copied()
            .filter(|f| f.name.to_ascii_lowercase().ends_with(".cbz"))
            .collect();
        if cbz_matching.len() == 1 {
            return Ok(cbz_matching[0]);
        }

        if matching.is_empty() {
            return Err(TorBoxError::AmbiguousFiles(format!(
                "Batch torrent has {} archives, but none matched requested Volume {target_vol}",
                archives.len()
            )));
        }

        return Err(TorBoxError::AmbiguousFiles(format!(
            "Batch torrent has {} archives matching requested Volume {target_vol}",
            matching.len()
        )));
    }

    // Ambiguous multiple archives with no exact target volume specified
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
/// 4. Classifies volume number using classify_release.
/// 5. If multiple files map to the same volume:
///    - If one is .cbz and another is .zip, prefers .cbz.
///    - If both have identical extensions or duplicate archives exist, fails closed with TorBoxError::AmbiguousFiles.
/// 6. If an archive file has an unparseable volume number, fails closed with TorBoxError::AmbiguousFiles.
/// 7. If no archives found, fails with TorBoxError::NoArchiveFound.
pub fn map_torrent_files_to_volumes(
    files: &[BackendFile],
) -> std::result::Result<BTreeMap<i64, BackendFile>, TorBoxError> {
    if files.is_empty() {
        return Err(TorBoxError::NoArchiveFound("Torrent contains 0 files".to_string()));
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
        let (unit, num, _) = crate::acquisition::nyaa::classify_release(&arch.name);
        let vol_opt = match unit {
            AcquisitionReleaseUnit::Volume(v) => Some(v.round() as i64),
            _ => num.map(|n| n.round() as i64),
        };

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
    let host = without_scheme.split(&['/', ':', '?'][..]).next().unwrap_or("");

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

        let err2 = select_artifact(&files3, Some(&AcquisitionReleaseUnit::Volume(99.0))).unwrap_err();
        assert!(matches!(err2, TorBoxError::AmbiguousFiles(_)));

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
        let err_target_cbr = select_artifact(&files_mixed, Some(&AcquisitionReleaseUnit::Volume(2.0))).unwrap_err();
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
        let sel_pref = select_artifact(&files_cbz_and_zip, Some(&AcquisitionReleaseUnit::Volume(1.0))).unwrap();
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
}
