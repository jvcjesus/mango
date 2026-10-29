//! Acquisition Provider capabilities and registration.
//!
//! Exposes provider metadata, supported release units (volume, collection, chapter),
//! whether external backends are required, and operational health.

use crate::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCandidate {
    pub provider: String,
    pub release_id: String,
    pub title: String,
    pub raw_title: String,
    pub target_volume: Option<f64>,
    pub target_chapter: Option<f64>,
    pub download_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseQuery {
    pub series_title: String,
    pub target_volume: Option<f64>,
    pub target_chapter: Option<f64>,
    pub preferred_edition: Option<String>,
}

pub trait ReleaseProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn search_releases(
        &self,
        query: &ReleaseQuery,
    ) -> impl std::future::Future<Output = Result<Vec<ReleaseCandidate>>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub source_id: String,
    pub name: String,
    pub supports_search: bool,
    pub supports_volumes: bool,
    pub supports_native_volumes: bool,
    pub supports_inferred_volumes: bool,
    pub volume_support_mode: String, // "native", "parsed_title_only", "inferred_chapters", "none"
    pub supports_chapters: bool,
    pub supports_collections: bool,
    pub requires_external_backend: bool,
    pub status: String, // "healthy", "degraded", "unavailable"
    pub last_error: Option<String>,
    pub last_success_at: Option<i64>,
}

impl ProviderCapabilities {
    pub fn nyaa() -> Self {
        Self {
            source_id: "nyaa".to_string(),
            name: "Nyaa".to_string(),
            supports_search: true,
            supports_volumes: true,
            supports_native_volumes: false,
            supports_inferred_volumes: true,
            volume_support_mode: "parsed_title_only".to_string(),
            supports_chapters: true,
            supports_collections: true,
            requires_external_backend: true,
            status: "healthy".to_string(),
            last_error: None,
            last_success_at: None,
        }
    }

    pub fn mangadot() -> Self {
        Self {
            source_id: "mangadot".to_string(),
            name: "MangaDot".to_string(),
            supports_search: true,
            supports_volumes: false, // Does NOT provide native SVolume metadata over Tachiyomi/Mihon protocol
            supports_native_volumes: false,
            supports_inferred_volumes: true, // Only parses volume numbers when embedded in chapter title string
            volume_support_mode: "inferred_chapters".to_string(),
            supports_chapters: true,
            supports_collections: false,
            requires_external_backend: false,
            status: "healthy".to_string(),
            last_error: None,
            last_success_at: None,
        }
    }

    pub fn keiyoushi_generic() -> Self {
        Self {
            source_id: "keiyoushi".to_string(),
            name: "Keiyoushi Extensions".to_string(),
            supports_search: true,
            supports_volumes: false,
            supports_native_volumes: false,
            supports_inferred_volumes: false,
            volume_support_mode: "none".to_string(),
            supports_chapters: true,
            supports_collections: false,
            requires_external_backend: false,
            status: "healthy".to_string(),
            last_error: None,
            last_success_at: None,
        }
    }

    pub fn madokami() -> Self {
        Self {
            source_id: "madokami".to_string(),
            name: "Madokami".to_string(),
            supports_search: true,
            supports_volumes: true,
            supports_native_volumes: true,
            supports_inferred_volumes: false,
            volume_support_mode: "native".to_string(),
            supports_chapters: false,
            supports_collections: true,
            requires_external_backend: false,
            status: "unavailable".to_string(),
            last_error: Some("Provider integration pending upstream contract".to_string()),
            last_success_at: None,
        }
    }
}

pub fn get_known_providers() -> Vec<ProviderCapabilities> {
    vec![
        ProviderCapabilities::nyaa(),
        ProviderCapabilities::mangadot(),
        ProviderCapabilities::keiyoushi_generic(),
        ProviderCapabilities::madokami(),
    ]
}
