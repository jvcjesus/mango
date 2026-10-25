//! Native Manga Acquisition, Discovery, and External Download Integration.
//!
//! Provides generic abstractions for:
//! 1. Release discovery providers (e.g., Nyaa, future Madokami/archives)
//! 2. External download backends (e.g., TorBox)
//! 3. Candidate normalization, conservative volume/chapter classification, and deterministic ranking
//! 4. Seamless handoff into the existing native Mango downstream acquisition pipeline

pub mod nyaa;
pub mod service;
pub mod torbox;
#[cfg(test)]
pub mod tests;

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AcquisitionReleaseUnit {
    Volume(f64),
    Chapter(f64),
    Collection { start: f64, end: f64 },
    Unknown,
}

impl std::fmt::Display for AcquisitionReleaseUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Volume(v) => write!(f, "Volume {v}"),
            Self::Chapter(c) => write!(f, "Chapter {c}"),
            Self::Collection { start, end } => write!(f, "Volumes {start}-{end}"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A normalized release candidate from an acquisition source (e.g. Nyaa).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionRelease {
    pub source: String,
    pub source_item_id: String,
    pub title: String,
    pub raw_title: String,
    pub uploader: Option<String>,
    pub source_url: String,
    pub torrent_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_hash: Option<String>,
    pub category_id: Option<String>,
    pub category_name: Option<String>,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub seeders: u32,
    pub leechers: u32,
    pub published_at: Option<i64>,
    pub is_trusted: bool,
    pub is_remake: bool,
    pub release_unit: AcquisitionReleaseUnit,
    pub release_number: Option<f64>,
    pub release_range: Option<(f64, f64)>,
    pub language: Option<String>,
}

/// A release candidate evaluated and ranked for a specific Mango title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionCandidate {
    pub release: AcquisitionRelease,
    pub score: f64,
    pub match_type: String, // exact_volume, exact_chapter, collection, related, ambiguous
    pub ownership_status: String, // owned, acquiring, not_owned, unknown
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalJobInfo {
    pub external_job_id: String,
    pub torrent_hash: Option<String>,
    pub is_cached: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "details", rename_all = "snake_case")]
pub enum BackendJobStatus {
    Downloading {
        progress: f64,
        seeds: u32,
        peers: u32,
        speed: u64,
    },
    Cached,
    Completed,
    Stalled {
        reason: String,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendFile {
    pub id: i64,
    pub name: String,
    pub size: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionHistoryRecord {
    pub id: i64,
    pub chapter_id: Option<i64>,
    pub manga_id: i64,
    pub series: String,
    pub release_name: String,
    pub source_type: String,
    pub uploader: Option<String>,
    pub external_job_id: Option<String>,
    pub backend_type: String,
    pub outcome: String,
    pub canonical_path: Option<String>,
    pub release_size: Option<u64>,
    pub completed_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionConfig {
    pub nyaa_enabled: bool,
    pub nyaa_url: Option<String>,
    pub nyaa_category: Option<String>,
    pub torbox_enabled: bool,
    pub torbox_url: Option<String>,
    pub has_torbox_token: bool,
    pub torbox_validated: Option<bool>,
    pub torbox_status_message: Option<String>,
}

impl Default for AcquisitionConfig {
    fn default() -> Self {
        Self {
            nyaa_enabled: true,
            nyaa_url: Some(nyaa::DEFAULT_NYAA_URL.to_string()),
            nyaa_category: Some("3_1".to_string()),
            torbox_enabled: false,
            torbox_url: Some(torbox::DEFAULT_TORBOX_URL.to_string()),
            has_torbox_token: false,
            torbox_validated: None,
            torbox_status_message: None,
        }
    }
}

/// Generic release discovery abstraction.
///
/// Designed so Nyaa, future Madokami, or other archive sources implement the same interface.
#[allow(dead_code)]
pub trait AcquisitionSource: Send + Sync {
    fn source_id(&self) -> &'static str;
    fn source_name(&self) -> &'static str;
    fn search_releases(
        &self,
        query: &str,
        title_aliases: &[String],
        target_unit: Option<AcquisitionReleaseUnit>,
    ) -> impl std::future::Future<Output = Result<Vec<AcquisitionRelease>>> + Send;
}

/// Generic download backend abstraction.
///
/// Designed so TorBox or future external remote acquisition backends implement the same interface.
#[allow(dead_code)]
pub trait DownloadBackend: Send + Sync {
    fn backend_id(&self) -> &'static str;
    fn backend_name(&self) -> &'static str;
    fn submit(
        &self,
        release: &AcquisitionRelease,
    ) -> impl std::future::Future<Output = Result<ExternalJobInfo>> + Send;
    fn check_status(
        &self,
        external_job_id: &str,
    ) -> impl std::future::Future<Output = Result<BackendJobStatus>> + Send;
    fn list_files(
        &self,
        external_job_id: &str,
    ) -> impl std::future::Future<Output = Result<Vec<BackendFile>>> + Send;
    fn retrieve_file(
        &self,
        external_job_id: &str,
        file_id: i64,
        destination_part: &Path,
    ) -> impl std::future::Future<Output = Result<u64>> + Send;
    fn cancel(
        &self,
        external_job_id: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
    fn test_connection(&self) -> impl std::future::Future<Output = Result<String>> + Send;
}
