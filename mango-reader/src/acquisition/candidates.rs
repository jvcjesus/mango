//! Provider-independent normalized acquisition candidate model.
//!
//! Normalizes release candidates across sources (Nyaa, MangaDot, Keiyoushi, future archives)
//! so the missing-content planner can evaluate and rank them uniformly.

use super::{AcquisitionCandidate, AcquisitionReleaseUnit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitMappingProvenance {
    ExplicitVolumeMetadata,
    ParsedVolumeFromTitle,
    ChapterToVolumeInference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateHealth {
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub source_healthy: bool,
    pub uploader_reliability_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateQualitySignals {
    pub is_digital: bool,
    pub is_trusted: bool,
    pub is_preferred_uploader: bool,
    pub edition_recognized: bool,
    pub edition_penalty: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedAcquisitionCandidate {
    pub series_identity: String,
    pub provider: String,
    pub provider_release_id: String,
    pub title: String,
    pub release_unit: AcquisitionReleaseUnit,
    pub release_number: Option<f64>,
    pub release_range: Option<(f64, f64)>,
    pub language: String,
    pub edition: Option<String>,
    pub format: Option<String>,
    pub uploader: Option<String>,
    pub size: Option<u64>,
    pub published_at: Option<i64>,
    pub availability: String, // "available", "unhealthy", "unavailable"
    pub health: CandidateHealth,
    pub quality_signals: CandidateQualitySignals,
    pub owned_units: Vec<f64>,
    pub missing_units: Vec<f64>,
    pub confidence: String, // "high", "review_recommended"
    pub score: f64,
    pub mapping_provenance: UnitMappingProvenance,
    pub provider_metadata: serde_json::Value,
}

impl NormalizedAcquisitionCandidate {
    /// Creates a normalized candidate from a Nyaa AcquisitionCandidate.
    pub fn from_nyaa_candidate(
        series: &str,
        cand: &AcquisitionCandidate,
        preferred_uploaders: &[String],
        language: &str,
    ) -> Self {
        let r = &cand.release;
        let is_pref = r.uploader.as_ref().map_or(false, |u| {
            preferred_uploaders.iter().any(|p| p.eq_ignore_ascii_case(u))
        });

        let availability = if r.seeders == 0 {
            "unhealthy".to_string()
        } else {
            "available".to_string()
        };

        let raw_lower = r.raw_title.to_lowercase();
        let is_digital = raw_lower.contains("digital");

        let identity_score = cand.smart_score.as_ref().map_or(20.0, |s| s.identity);
        let confidence = if identity_score >= 18.0
            && !cand.is_variant
            && (r.seeders >= 1 || cand.score >= 70.0)
        {
            "high".to_string()
        } else {
            "review_recommended".to_string()
        };

        let variant_penalty = if cand.is_variant { 25.0 } else { 0.0 };

        Self {
            series_identity: series.to_string(),
            provider: "nyaa".to_string(),
            provider_release_id: r.source_item_id.clone(),
            title: r.title.clone(),
            release_unit: r.release_unit.clone(),
            release_number: r.release_number,
            release_range: r.release_range,
            language: language.to_string(),
            edition: super::edition::edition_from_title(&r.raw_title),
            format: if is_digital {
                Some("Digital CBZ".to_string())
            } else {
                Some("CBZ Archive".to_string())
            },
            uploader: r.uploader.clone(),
            size: Some(r.size_bytes),
            published_at: r.published_at,
            availability,
            health: CandidateHealth {
                seeders: Some(r.seeders),
                leechers: Some(r.leechers),
                source_healthy: true,
                uploader_reliability_percent: None,
            },
            quality_signals: CandidateQualitySignals {
                is_digital,
                is_trusted: r.is_trusted,
                is_preferred_uploader: is_pref,
                edition_recognized: cand.is_variant,
                edition_penalty: variant_penalty,
            },
            owned_units: cand.units_owned.clone(),
            missing_units: cand.units_missing.clone(),
            confidence,
            score: cand.score,
            mapping_provenance: UnitMappingProvenance::ParsedVolumeFromTitle,
            provider_metadata: serde_json::json!({
                "sourceUrl": r.source_url,
                "torrentHash": r.info_hash,
                "torrentUrl": r.torrent_url,
                "rawTitle": r.raw_title,
                "requestedEdition": super::edition::edition_from_title(series),
                "smartScore": cand.smart_score,
            }),
        }
    }

    /// Creates a normalized candidate from a MangaDot or extension release.
    pub fn from_mangadot_release(
        series: &str,
        volume_number: f64,
        release_title: &str,
        size_bytes: Option<u64>,
        language: &str,
        provenance: UnitMappingProvenance,
    ) -> Self {
        let confidence = if provenance == UnitMappingProvenance::ChapterToVolumeInference {
            "review_recommended".to_string()
        } else {
            "high".to_string()
        };

        Self {
            series_identity: series.to_string(),
            provider: "mangadot".to_string(),
            provider_release_id: format!("md_{volume_number}"),
            title: release_title.to_string(),
            release_unit: AcquisitionReleaseUnit::Volume(volume_number),
            release_number: Some(volume_number),
            release_range: None,
            language: language.to_string(),
            edition: None,
            format: Some("Page Stream".to_string()),
            uploader: Some("MangaDot".to_string()),
            size: size_bytes,
            published_at: None,
            availability: "available".to_string(),
            health: CandidateHealth {
                seeders: None,
                leechers: None,
                source_healthy: true,
                uploader_reliability_percent: Some(100.0),
            },
            quality_signals: CandidateQualitySignals {
                is_digital: true,
                is_trusted: true,
                is_preferred_uploader: false,
                edition_recognized: false,
                edition_penalty: 0.0,
            },
            owned_units: vec![],
            missing_units: vec![volume_number],
            confidence,
            score: if provenance == UnitMappingProvenance::ChapterToVolumeInference { 50.0 } else { 80.0 },
            mapping_provenance: provenance,
            provider_metadata: serde_json::json!({
                "sourceType": "reader",
                "directVolume": true,
                "inferred": provenance == UnitMappingProvenance::ChapterToVolumeInference,
            }),
        }
    }
}
