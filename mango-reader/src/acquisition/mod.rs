//! Native Manga Acquisition, Discovery, and External Download Integration.
//!
//! Provides generic abstractions for:
//! 1. Release discovery providers (e.g., Nyaa, future Madokami/archives)
//! 2. External download backends (e.g., TorBox)
//! 3. Candidate normalization, conservative volume/chapter classification, and deterministic ranking
//! 4. Seamless handoff into the existing native Mango downstream acquisition pipeline

pub mod candidates;
pub mod edition;
pub mod execution;
pub mod intelligence;
pub mod interactive;
pub mod missing;
pub mod nyaa;
pub mod planner;
pub mod profiles;
pub mod providers;
pub mod service;
pub mod source_units;
#[cfg(test)]
pub mod tests;
pub mod torbox;

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AcquisitionReleaseUnit {
    Volume(f64),
    Chapter(f64),
    Collection {
        start: f64,
        end: f64,
    },
    MultiRange(Vec<(f64, f64)>),
    MixedCollection {
        volume_start: f64,
        volume_end: f64,
        chapter_start: f64,
        chapter_end: f64,
    },
    Unknown,
}

impl AcquisitionReleaseUnit {
    pub fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::Collection { .. } | Self::MultiRange(_) | Self::MixedCollection { .. }
        )
    }

    pub fn included_volumes(&self) -> Vec<f64> {
        match self {
            Self::Volume(v) => vec![*v],
            Self::Collection { start, end } => {
                let s = *start as i64;
                let e = *end as i64;
                if s <= e {
                    (s..=e).map(|n| n as f64).collect()
                } else {
                    vec![]
                }
            }
            Self::MultiRange(ranges) => {
                let mut vols = Vec::new();
                for (s, e) in ranges {
                    let s_i = *s as i64;
                    let e_i = *e as i64;
                    if s_i <= e_i {
                        for n in s_i..=e_i {
                            vols.push(n as f64);
                        }
                    }
                }
                vols.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                vols.dedup();
                vols
            }
            Self::MixedCollection {
                volume_start,
                volume_end,
                ..
            } => {
                let s = *volume_start as i64;
                let e = *volume_end as i64;
                if s <= e {
                    (s..=e).map(|n| n as f64).collect()
                } else {
                    vec![]
                }
            }
            Self::Chapter(_) | Self::Unknown => vec![],
        }
    }

    pub fn included_chapters(&self) -> Vec<f64> {
        match self {
            Self::Chapter(c) => vec![*c],
            Self::MixedCollection {
                chapter_start,
                chapter_end,
                ..
            } => {
                let s = *chapter_start as i64;
                let e = *chapter_end as i64;
                if s <= e {
                    (s..=e).map(|n| n as f64).collect()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }
}

impl std::fmt::Display for AcquisitionReleaseUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Volume(v) => write!(f, "Volume {v}"),
            Self::Chapter(c) => write!(f, "Chapter {c}"),
            Self::Collection { start, end } => write!(f, "Volumes {start}-{end}"),
            Self::MultiRange(ranges) => {
                let s = ranges
                    .iter()
                    .map(|(s, e)| format!("{s}-{e}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "Volumes {s}")
            }
            Self::MixedCollection {
                volume_start,
                volume_end,
                chapter_start,
                chapter_end,
            } => {
                write!(
                    f,
                    "Volumes {volume_start}-{volume_end} + Chapters {chapter_start}-{chapter_end}"
                )
            }
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detailed, explainable components of the deterministic Smart ranking score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartScore {
    pub identity: f64,       // 0..25: Title/alias match confidence and variant penalty
    pub coverage: f64,       // 0..25: Useful missing units supplied vs duplicate overhead
    pub quality: f64,        // 0..15: Digital format, trusted uploader, preferred uploader
    pub health: f64,         // 0..15: Saturating seeders curve
    pub freshness: f64,      // 0..10: Recency of release
    pub efficiency: f64,     // 0..10: Download bytes per useful missing unit
    pub penalties: f64,      // Negative penalties (e.g. dead torrent, remake, mixed collection)
    pub total: f64,          // Final normalized score 0..100
    pub explanation: String, // Concise human explanation e.g. "★ BEST MATCH: Covers all 7 missing volumes · Digital · 12 seeders"
    pub summary_tag: String, // "BEST MATCH", "GOOD ALTERNATIVE", "LOWER MATCH", "COLLECTION", "MIXED COLLECTION"
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
    pub is_variant: bool,
    pub variant_name: Option<String>,
    pub is_mixed: bool,
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
    pub smart_score: Option<SmartScore>,
    pub match_type: String, // exact_volume, exact_chapter, collection, mixed_collection, related, ambiguous
    pub ownership_status: String, // owned, acquiring, not_owned, partial, unknown
    pub units_included: Vec<f64>,
    pub units_owned: Vec<f64>,
    pub units_missing: Vec<f64>,
    pub coverage_summary: String,
    pub useful_efficiency_bytes: Option<u64>,
    pub is_variant: bool,
    pub variant_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionUnitEntry {
    pub volume: f64,
    pub file_id: Option<i64>,
    pub filename: Option<String>,
    pub file_size: Option<u64>,
    pub is_owned: bool,
    pub is_acquiring: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionInspection {
    pub release: AcquisitionRelease,
    pub units: Vec<CollectionUnitEntry>,
    pub total_units: usize,
    pub owned_count: usize,
    pub missing_count: usize,
    pub recommended_mode: String, // "missing_only" | "entire"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquireCollectionRequest {
    pub release: AcquisitionRelease,
    pub selected_volumes: Vec<f64>,
    pub requested_edition: Option<String>,
    pub mode: String, // "missing_only" | "entire" | "manual"
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
    pub mangabaka_id: Option<i64>,
    pub mangabaka_release_id: Option<String>,
    pub anilist_id: Option<i64>,
    pub requested_unit: Option<String>,
    pub metadata_status: Option<String>,
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
    fn cancel(&self, external_job_id: &str)
    -> impl std::future::Future<Output = Result<()>> + Send;
    fn test_connection(&self) -> impl std::future::Future<Output = Result<String>> + Send;
}
