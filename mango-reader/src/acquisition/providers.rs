//! Source-Agnostic Acquisition Provider capabilities, health, and candidate ranking.
//!
//! Provides normalized abstractions for discovery providers (Nyaa, Madokami, Keiyoushi),
//! unified candidate modeling, conservative matching classification, and deterministic scoring.

use crate::db::now;
use crate::error::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Status of a release discovery provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Available,
    Disabled,
    AuthenticationRequired,
    TemporarilyUnavailable,
    RateLimited,
    ConfigurationError,
    Unsupported,
}

impl ProviderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Disabled => "disabled",
            Self::AuthenticationRequired => "authentication_required",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::RateLimited => "rate_limited",
            Self::ConfigurationError => "configuration_error",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Operational health and availability of a discovery provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider_id: String,
    pub name: String,
    pub status: ProviderStatus,
    pub message: Option<String>,
    pub last_checked_at: Option<i64>,
}

/// Conservative match classification for a release candidate relative to a logical target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateMatchType {
    /// Exact single volume matching target (e.g. Target 10 == Candidate Vol 10).
    ExactVolume,
    /// A small volume range or omnibus containing target (e.g. Target 10 in Vols 8-12).
    VolumeRangeContainingTarget,
    /// A full series batch or large pack containing target (e.g. Vols 1-20).
    SeriesPack,
    /// Plausible/fuzzy volume candidate lacking unambiguous markers.
    PossibleMatch,
    /// Anime, chapters, wrong volume numbers, or unparseable releases.
    Rejected,
}

impl CandidateMatchType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExactVolume => "exact_volume",
            Self::VolumeRangeContainingTarget => "volume_range_containing_target",
            Self::SeriesPack => "series_pack",
            Self::PossibleMatch => "possible_match",
            Self::Rejected => "rejected",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "exact_volume" => Self::ExactVolume,
            "volume_range_containing_target" => Self::VolumeRangeContainingTarget,
            "series_pack" => Self::SeriesPack,
            "possible_match" => Self::PossibleMatch,
            _ => Self::Rejected,
        }
    }
}

/// Unified, source-agnostic candidate discovered for a logical target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCandidate {
    pub candidate_id: String,
    pub provider: String,
    pub provider_item_id: String,
    pub title: String,
    pub raw_title: String,
    pub match_type: CandidateMatchType,
    pub target_volume: Option<f64>,
    pub target_chapter: Option<f64>,
    pub size_bytes: Option<u64>,
    pub uploader: Option<String>,
    pub is_trusted: bool,
    pub is_digital: bool,
    pub score: f64,
    pub score_explanation: String,
    pub download_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_hash: Option<String>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub published_at: Option<i64>,
}

/// Release query specification for a logical missing volume/chapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseQuery {
    pub series_title: String,
    pub target_volume: Option<f64>,
    pub target_chapter: Option<f64>,
    pub preferred_edition: Option<String>,
}

/// Source-agnostic release provider trait.
pub trait ReleaseProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn check_health<'a>(&'a self) -> BoxFuture<'a, ProviderHealth>;
    fn search_releases<'a>(
        &'a self,
        query: &'a ReleaseQuery,
    ) -> BoxFuture<'a, Result<Vec<ReleaseCandidate>>>;
}

const QUALITY_GROUPS: &[&str] = &[
    "danke-empire",
    "digital-empire",
    "empire",
    "luschious",
    "1r0n",
    "slug",
    "fudge",
    "playa",
    "kobo",
    "viz",
    "seven seas",
    "yen press",
    "kodansha",
];

pub fn is_known_quality_group(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    QUALITY_GROUPS.iter().any(|&g| lower.contains(g))
}

/// Classifies a release title against a query target volume.
pub fn classify_candidate_release(
    _title: &str,
    raw_title: &str,
    target_volume: Option<f64>,
    _target_chapter: Option<f64>,
) -> (CandidateMatchType, Option<f64>, bool) {
    let lower = raw_title.to_ascii_lowercase();

    // 1. Anime / media rejection
    if super::nyaa::is_anime_false_positive(raw_title, None) {
        return (CandidateMatchType::Rejected, None, false);
    }

    // 2. Digital release detection
    let is_digital = lower.contains("digital")
        || lower.contains("(digital)")
        || lower.contains("[digital]")
        || lower.contains("web-dl")
        || lower.contains("official")
        || lower.contains("kobo")
        || lower.contains("kindle");

    // 3. Classification against target volume
    if let Some(target_v) = target_volume {
        let (unit, _, _) = super::nyaa::classify_release(raw_title);

        match unit {
            super::AcquisitionReleaseUnit::Volume(v) => {
                if (v - target_v).abs() < 0.001 {
                    (CandidateMatchType::ExactVolume, Some(v), is_digital)
                } else {
                    (CandidateMatchType::Rejected, Some(v), is_digital)
                }
            }
            super::AcquisitionReleaseUnit::Collection { start, end } => {
                if target_v >= start && target_v <= end {
                    let span = end - start + 1.0;
                    if span >= 15.0 || lower.contains("complete") || lower.contains("batch") {
                        (CandidateMatchType::SeriesPack, None, is_digital)
                    } else {
                        (CandidateMatchType::VolumeRangeContainingTarget, None, is_digital)
                    }
                } else {
                    (CandidateMatchType::Rejected, None, is_digital)
                }
            }
            super::AcquisitionReleaseUnit::MultiRange(ranges) => {
                if ranges.iter().any(|(s, e)| target_v >= *s && target_v <= *e) {
                    (CandidateMatchType::VolumeRangeContainingTarget, None, is_digital)
                } else {
                    (CandidateMatchType::Rejected, None, is_digital)
                }
            }
            super::AcquisitionReleaseUnit::MixedCollection {
                volume_start,
                volume_end,
                ..
            } => {
                if target_v >= volume_start && target_v <= volume_end {
                    (CandidateMatchType::VolumeRangeContainingTarget, None, is_digital)
                } else {
                    (CandidateMatchType::Rejected, None, is_digital)
                }
            }
            super::AcquisitionReleaseUnit::Chapter(_) => {
                // Looking for volume; individual chapter releases are rejected
                (CandidateMatchType::Rejected, None, is_digital)
            }
            super::AcquisitionReleaseUnit::Unknown => {
                // Check if title has fuzzy volume marker matching target
                let v_int = target_v.round() as i64;
                let target_patterns = [
                    format!("v{v_int}"),
                    format!("v{:02}", v_int),
                    format!("vol {v_int}"),
                    format!("vol. {v_int}"),
                    format!("volume {v_int}"),
                    format!("volume {:02}", v_int),
                ];
                let has_match = target_patterns.iter().any(|pat| lower.contains(pat));
                if has_match {
                    (CandidateMatchType::PossibleMatch, Some(target_v), is_digital)
                } else {
                    (CandidateMatchType::Rejected, None, is_digital)
                }
            }
        }
    } else {
        // No specific volume targeted; classify generically
        let (unit, _, _) = super::nyaa::classify_release(raw_title);
        match unit {
            super::AcquisitionReleaseUnit::Volume(v) => {
                (CandidateMatchType::ExactVolume, Some(v), is_digital)
            }
            super::AcquisitionReleaseUnit::Collection { .. } => {
                (CandidateMatchType::VolumeRangeContainingTarget, None, is_digital)
            }
            super::AcquisitionReleaseUnit::MultiRange(_)
            | super::AcquisitionReleaseUnit::MixedCollection { .. } => {
                (CandidateMatchType::VolumeRangeContainingTarget, None, is_digital)
            }
            super::AcquisitionReleaseUnit::Chapter(_) => (CandidateMatchType::Rejected, None, is_digital),
            super::AcquisitionReleaseUnit::Unknown => (CandidateMatchType::PossibleMatch, None, is_digital),
        }
    }
}

/// Evaluates and assigns scores with deterministic explanations.
pub fn score_candidate(candidate: &mut ReleaseCandidate, _target_volume: Option<f64>) {
    let mut score: f64 = 0.0;
    let mut explanations = Vec::new();

    match candidate.match_type {
        CandidateMatchType::ExactVolume => {
            score += 60.0;
            explanations.push("Exact single volume match (+60)".to_string());
        }
        CandidateMatchType::VolumeRangeContainingTarget => {
            score += 35.0;
            explanations.push("Volume range contains target (+35)".to_string());
        }
        CandidateMatchType::SeriesPack => {
            score += 20.0;
            explanations.push("Series pack / complete batch (+20)".to_string());
        }
        CandidateMatchType::PossibleMatch => {
            score += 10.0;
            explanations.push("Possible / fuzzy volume match (+10)".to_string());
        }
        CandidateMatchType::Rejected => {
            candidate.score = 0.0;
            candidate.score_explanation = "Rejected match".to_string();
            return;
        }
    }

    if candidate.is_digital {
        score += 15.0;
        explanations.push("Digital edition (+15)".to_string());
    }

    if candidate.is_trusted {
        score += 10.0;
        explanations.push("Trusted uploader (+10)".to_string());
    }

    if is_known_quality_group(&candidate.raw_title) {
        score += 5.0;
        explanations.push("Recognized quality group (+5)".to_string());
    }

    let title_lower = candidate.raw_title.to_ascii_lowercase();
    if title_lower.contains(" v2")
        || title_lower.contains("[v2]")
        || title_lower.contains("(v2)")
        || title_lower.contains("remake")
    {
        score += 2.0;
        explanations.push("Revised edition / v2 (+2)".to_string());
    }

    match candidate.seeders {
        Some(s) if s >= 20 => {
            score += 10.0;
            explanations.push(format!("Well-seeded: {s} seeds (+10)"));
        }
        Some(s) if s >= 5 => {
            score += 5.0;
            explanations.push(format!("Active seeds: {s} seeds (+5)"));
        }
        Some(s) if s >= 1 => {
            score += 2.0;
            explanations.push(format!("Low seeds: {s} seeds (+2)"));
        }
        Some(0) => {
            score -= 15.0;
            explanations.push("Dead torrent: 0 seeders (-15)".to_string());
        }
        _ => {}
    }

    candidate.score = score.max(0.0);
    candidate.score_explanation = explanations.join("; ");
}

/// Scores and ranks candidates descending by score.
pub fn score_and_rank_candidates(
    candidates: &mut [ReleaseCandidate],
    target_volume: Option<f64>,
) {
    for c in candidates.iter_mut() {
        score_candidate(c, target_volume);
    }
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
            .then_with(|| b.published_at.unwrap_or(0).cmp(&a.published_at.unwrap_or(0)))
    });
}

// -----------------------------------------------------------------------------
// Nyaa Release Provider Implementation
// -----------------------------------------------------------------------------

pub struct NyaaReleaseProvider {
    client: super::nyaa::NyaaClient,
    category: Option<String>,
    enabled: bool,
}

impl NyaaReleaseProvider {
    pub fn new(http: Client, base_url: Option<String>, category: Option<String>, enabled: bool) -> Self {
        let client = super::nyaa::NyaaClient::new(http, base_url);
        Self {
            client,
            category,
            enabled,
        }
    }
}

impl ReleaseProvider for NyaaReleaseProvider {
    fn id(&self) -> &'static str {
        "nyaa"
    }

    fn name(&self) -> &'static str {
        "Nyaa"
    }

    fn check_health<'a>(&'a self) -> BoxFuture<'a, ProviderHealth> {
        Box::pin(async move {
            if !self.enabled {
                return ProviderHealth {
                    provider_id: "nyaa".to_string(),
                    name: "Nyaa".to_string(),
                    status: ProviderStatus::Disabled,
                    message: Some("Disabled in settings".to_string()),
                    last_checked_at: Some(now()),
                };
            }
            ProviderHealth {
                provider_id: "nyaa".to_string(),
                name: "Nyaa".to_string(),
                status: ProviderStatus::Available,
                message: None,
                last_checked_at: Some(now()),
            }
        })
    }

    fn search_releases<'a>(
        &'a self,
        query: &'a ReleaseQuery,
    ) -> BoxFuture<'a, Result<Vec<ReleaseCandidate>>> {
        Box::pin(async move {
            if !self.enabled {
                return Ok(Vec::new());
            }

            let mut raw_releases = Vec::new();
            let mut seen_hashes = std::collections::HashSet::new();

            if let Some(target_v) = query.target_volume {
                let v_int = target_v.round() as i64;
                let queries = [
                    format!("{} v{}", query.series_title, v_int),
                    format!("{} vol {}", query.series_title, v_int),
                    format!("{} volume {}", query.series_title, v_int),
                    query.series_title.clone(),
                ];
                for q in queries {
                    if let Ok(results) = self.client.search(&q, self.category.as_deref()).await {
                        for r in results {
                            let key = r.info_hash.clone().unwrap_or_else(|| r.source_item_id.clone());
                            if seen_hashes.insert(key) {
                                raw_releases.push(r);
                            }
                        }
                    }
                }
            } else {
                let results = self
                    .client
                    .search(&query.series_title, self.category.as_deref())
                    .await
                    .unwrap_or_default();
                for r in results {
                    let key = r.info_hash.clone().unwrap_or_else(|| r.source_item_id.clone());
                    if seen_hashes.insert(key) {
                        raw_releases.push(r);
                    }
                }
            }

            let mut candidates = Vec::new();
            for r in raw_releases {
                let (match_type, parsed_vol, is_digital) = classify_candidate_release(
                    &r.title,
                    &r.raw_title,
                    query.target_volume,
                    query.target_chapter,
                );

                if match_type == CandidateMatchType::Rejected {
                    continue;
                }

                let candidate = ReleaseCandidate {
                    candidate_id: format!("nyaa:{}", r.source_item_id),
                    provider: "nyaa".to_string(),
                    provider_item_id: r.source_item_id,
                    title: r.title,
                    raw_title: r.raw_title,
                    match_type,
                    target_volume: parsed_vol.or(query.target_volume),
                    target_chapter: query.target_chapter,
                    size_bytes: Some(r.size_bytes),
                    uploader: r.uploader,
                    is_trusted: r.is_trusted,
                    is_digital,
                    score: 0.0,
                    score_explanation: String::new(),
                    download_url: r.torrent_url,
                    magnet_url: r.magnet_url,
                    info_hash: r.info_hash,
                    seeders: Some(r.seeders),
                    leechers: Some(r.leechers),
                    published_at: r.published_at,
                };
                candidates.push(candidate);
            }

            score_and_rank_candidates(&mut candidates, query.target_volume);
            Ok(candidates)
        })
    }
}

// -----------------------------------------------------------------------------
// Madokami Release Provider Implementation (Offline / Seam)
// -----------------------------------------------------------------------------

pub struct MadokamiReleaseProvider {
    _http: Client,
    _base_url: Option<String>,
}

impl MadokamiReleaseProvider {
    pub fn new(http: Client, base_url: Option<String>) -> Self {
        Self {
            _http: http,
            _base_url: base_url,
        }
    }
}

impl ReleaseProvider for MadokamiReleaseProvider {
    fn id(&self) -> &'static str {
        "madokami"
    }

    fn name(&self) -> &'static str {
        "Madokami"
    }

    fn check_health<'a>(&'a self) -> BoxFuture<'a, ProviderHealth> {
        Box::pin(async move {
            ProviderHealth {
                provider_id: "madokami".to_string(),
                name: "Madokami".to_string(),
                status: ProviderStatus::TemporarilyUnavailable,
                message: Some("Madokami is currently offline / unavailable".to_string()),
                last_checked_at: Some(now()),
            }
        })
    }

    fn search_releases<'a>(
        &'a self,
        _query: &'a ReleaseQuery,
    ) -> BoxFuture<'a, Result<Vec<ReleaseCandidate>>> {
        Box::pin(async move {
            // Madokami is currently offline; return empty result safely without breaking discovery
            tracing::warn!("Madokami provider queried while temporarily unavailable; returning empty candidates");
            Ok(Vec::new())
        })
    }
}

// -----------------------------------------------------------------------------
// Legacy capabilities model for UI Settings inspection
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub source_id: String,
    pub name: String,
    pub supports_search: bool,
    pub supports_volumes: bool,
    pub supports_native_volumes: bool,
    pub supports_inferred_volumes: bool,
    pub volume_support_mode: String,
    pub supports_chapters: bool,
    pub supports_collections: bool,
    pub requires_external_backend: bool,
    pub status: String,
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
            supports_volumes: false,
            supports_native_volumes: false,
            supports_inferred_volumes: true,
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
            last_error: Some("Madokami is currently offline / unavailable".to_string()),
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
