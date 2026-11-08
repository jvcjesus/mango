//! Intelligent Release Matching and Confidence Subsystem (Milestone M10).
//!
//! Provides deterministic, explainable matching between discovered releases
//! and canonical MangaBaka/Komga entries with strict disambiguation:
//! - Volume 4 vs Volume 40 strict boundary isolation
//! - Spin-off and subtitle rejection (e.g. "Haikyu-bu!!" vs "Haikyu!!")
//! - Explicit confidence levels: High, Medium, Low, Ambiguous
//! - Multi-language and edition gating
//! - Omnibus and volume range coverage detection

use super::providers::CandidateMatchType;
use super::AcquisitionReleaseUnit;
use serde::{Deserialize, Serialize};

/// Confidence classification for a release match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateConfidence {
    /// Exact canonical series + exact volume + matching language/edition.
    High,
    /// Verified series with volume range/omnibus or non-standard container.
    Medium,
    /// Fuzzy volume marker, single seeder, or chapter-only coverage.
    Low,
    /// Ambiguous title, spin-off marker, or conflicting edition/language; requires review.
    Ambiguous,
}

impl CandidateConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Ambiguous => "ambiguous",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            _ => Self::Ambiguous,
        }
    }

    pub fn requires_manual_review(&self) -> bool {
        matches!(self, Self::Ambiguous | Self::Low)
    }
}

impl std::fmt::Display for CandidateConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Known spin-off / subtitle markers that indicate a different work unless the canonical title includes them.
const SPINOFF_MARKERS: &[&str] = &[
    "spin-off",
    "spinoff",
    "-bu!!",
    "bu!!",
    "smash!!",
    "junior high",
    "official guidebook",
    "guidebook",
    "character book",
    "fanbook",
    "anthology",
    "the movie",
    "movie animation",
    "novel",
    "light novel",
    "side story",
    "gaiden",
    "databook",
    "artbook",
    "color walk",
];

/// Known language tags in release titles.
const FOREIGN_LANGUAGE_TAGS: &[(&str, &str)] = &[
    ("[fr]", "French"),
    ("(fr)", "French"),
    ("[french]", "French"),
    ("[de]", "German"),
    ("(de)", "German"),
    ("[german]", "German"),
    ("[ita]", "Italian"),
    ("(ita)", "Italian"),
    ("[italian]", "Italian"),
    ("[esp]", "Spanish"),
    ("(esp)", "Spanish"),
    ("[spanish]", "Spanish"),
    ("[pt]", "Portuguese"),
    ("(pt)", "Portuguese"),
    ("[raw]", "Japanese RAW"),
    ("(raw)", "Japanese RAW"),
    ("[jp]", "Japanese"),
    ("(jp)", "Japanese"),
    ("[rus]", "Russian"),
    ("(rus)", "Russian"),
    ("[pl]", "Polish"),
    ("(pl)", "Polish"),
];

/// Detects foreign language tags in a release title.
pub fn detect_foreign_language(title: &str, target_lang: &str) -> Option<String> {
    let lower = title.to_ascii_lowercase();
    let is_target_english = target_lang.eq_ignore_ascii_case("en") || target_lang.eq_ignore_ascii_case("eng");

    if is_target_english {
        for (tag, lang_name) in FOREIGN_LANGUAGE_TAGS {
            if lower.contains(tag) {
                return Some(lang_name.to_string());
            }
        }
    }
    None
}

/// Checks whether a release title has a spin-off or subtitle marker not present in the canonical title.
pub fn has_unmatched_spinoff_marker(candidate_title: &str, canonical_title: &str) -> Option<String> {
    let cand_lower = candidate_title.to_ascii_lowercase();
    let canon_lower = canonical_title.to_ascii_lowercase();

    for &marker in SPINOFF_MARKERS {
        if cand_lower.contains(marker) && !canon_lower.contains(marker) {
            return Some(marker.to_string());
        }
    }
    None
}

/// Tokenizes text into normalized lowercase alphanumeric words.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Evaluates series title matching between candidate title and canonical title + aliases.
pub fn match_series_identity(
    candidate_title: &str,
    canonical_title: &str,
    aliases: &[String],
) -> (bool, f64, String) {
    let cand_tokens = tokenize(candidate_title);
    let canon_tokens = tokenize(canonical_title);

    // 1. Check spin-off markers
    if let Some(marker) = has_unmatched_spinoff_marker(candidate_title, canonical_title) {
        return (false, 0.0, format!("Spin-off or subtitle marker detected: '{marker}'"));
    }

    // 2. Exact prefix or token inclusion for canonical title
    if !canon_tokens.is_empty() {
        let all_canon_present = canon_tokens.iter().all(|ct| cand_tokens.contains(ct));
        if all_canon_present {
            return (true, 100.0, format!("Exact series match for '{canonical_title}'"));
        }
    }

    // 3. Check aliases
    for alias in aliases {
        let alias_tokens = tokenize(alias);
        if !alias_tokens.is_empty() {
            let all_alias_present = alias_tokens.iter().all(|at| cand_tokens.contains(at));
            if all_alias_present {
                return (true, 90.0, format!("Series alias match for '{alias}'"));
            }
        }
    }

    // 4. Substring check as fallback
    let cand_clean = candidate_title.to_ascii_lowercase();
    let canon_clean = canonical_title.to_ascii_lowercase();
    if cand_clean.contains(&canon_clean) {
        return (true, 85.0, format!("Canonical substring match for '{canonical_title}'"));
    }

    (false, 0.0, format!("Title does not match canonical series '{canonical_title}'"))
}

/// Extracts a volume number from a release title with strict word-boundary matching.
///
/// Guaranteed invariants:
/// - Volume 4 does NOT match Volume 40, v4 does NOT match v40.
/// - Chapters (ch. 04, c04) are NOT parsed as volumes.
/// - Decimal volumes (v04.5) are preserved.
/// - Ranges (v01-03, Volumes 1-3) are parsed as collections.
pub fn match_volume_number(
    candidate_title: &str,
    target_volume: Option<f64>,
) -> (CandidateMatchType, Option<f64>, Option<(f64, f64)>, String) {
    let (unit, _, _) = crate::acquisition::nyaa::classify_release(candidate_title);

    match unit {
        AcquisitionReleaseUnit::Volume(v) => {
            if let Some(target) = target_volume {
                if (v - target).abs() < 0.001 {
                    (
                        CandidateMatchType::ExactVolume,
                        Some(v),
                        None,
                        format!("Exact volume match: Volume {v}"),
                    )
                } else {
                    (
                        CandidateMatchType::Rejected,
                        Some(v),
                        None,
                        format!("Volume mismatch: candidate is Volume {v}, requested Volume {target}"),
                    )
                }
            } else {
                (
                    CandidateMatchType::ExactVolume,
                    Some(v),
                    None,
                    format!("Single volume release: Volume {v}"),
                )
            }
        }
        AcquisitionReleaseUnit::Collection { start, end } => {
            if let Some(target) = target_volume {
                if target >= start && target <= end {
                    let span = end - start + 1.0;
                    if span >= 15.0 {
                        (
                            CandidateMatchType::SeriesPack,
                            None,
                            Some((start, end)),
                            format!("Series pack (Vols {start}-{end}) covers Volume {target}"),
                        )
                    } else {
                        (
                            CandidateMatchType::VolumeRangeContainingTarget,
                            None,
                            Some((start, end)),
                            format!("Volume range (Vols {start}-{end}) contains Volume {target}"),
                        )
                    }
                } else {
                    (
                        CandidateMatchType::Rejected,
                        None,
                        Some((start, end)),
                        format!("Volume range {start}-{end} does not contain Volume {target}"),
                    )
                }
            } else {
                (
                    CandidateMatchType::VolumeRangeContainingTarget,
                    None,
                    Some((start, end)),
                    format!("Volume range {start}-{end}"),
                )
            }
        }
        AcquisitionReleaseUnit::MultiRange(ranges) => {
            if let Some(target) = target_volume {
                if ranges.iter().any(|(s, e)| target >= *s && target <= *e) {
                    (
                        CandidateMatchType::VolumeRangeContainingTarget,
                        None,
                        None,
                        format!("Multi-range collection contains Volume {target}"),
                    )
                } else {
                    (
                        CandidateMatchType::Rejected,
                        None,
                        None,
                        format!("Multi-range collection does not contain Volume {target}"),
                    )
                }
            } else {
                (
                    CandidateMatchType::VolumeRangeContainingTarget,
                    None,
                    None,
                    "Multi-range collection".to_string(),
                )
            }
        }
        AcquisitionReleaseUnit::MixedCollection { volume_start, volume_end, .. } => {
            if let Some(target) = target_volume {
                if target >= volume_start && target <= volume_end {
                    (
                        CandidateMatchType::VolumeRangeContainingTarget,
                        None,
                        Some((volume_start, volume_end)),
                        format!("Mixed collection contains Volume {target}"),
                    )
                } else {
                    (
                        CandidateMatchType::Rejected,
                        None,
                        Some((volume_start, volume_end)),
                        format!("Mixed collection does not contain Volume {target}"),
                    )
                }
            } else {
                (
                    CandidateMatchType::VolumeRangeContainingTarget,
                    None,
                    Some((volume_start, volume_end)),
                    "Mixed collection".to_string(),
                )
            }
        }
        AcquisitionReleaseUnit::Chapter(c) => {
            if target_volume.is_some() {
                (
                    CandidateMatchType::Rejected,
                    None,
                    None,
                    format!("Chapter release (Ch. {c}) rejected when targeting volume"),
                )
            } else {
                (
                    CandidateMatchType::PossibleMatch,
                    None,
                    None,
                    format!("Chapter release: Ch. {c}"),
                )
            }
        }
        AcquisitionReleaseUnit::Unknown => {
            // Strict word-boundary fuzzy volume search
            if let Some(target) = target_volume {
                let v_int = target.round() as i64;
                if strict_fuzzy_volume_match(candidate_title, v_int) {
                    (
                        CandidateMatchType::PossibleMatch,
                        Some(target),
                        None,
                        format!("Fuzzy volume marker found for Volume {v_int}"),
                    )
                } else {
                    (
                        CandidateMatchType::Rejected,
                        None,
                        None,
                        "No unambiguous volume marker matching target".to_string(),
                    )
                }
            } else {
                (
                    CandidateMatchType::PossibleMatch,
                    None,
                    None,
                    "Unknown release unit".to_string(),
                )
            }
        }
    }
}

/// Checks whether a title matches a specific integer volume number using strict word boundaries.
/// Prevents "v4" from matching "v40".
pub fn strict_fuzzy_volume_match(text: &str, target_vol: i64) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let patterns = [
        format!("v{target_vol}"),
        format!("v{:02}", target_vol),
        format!("vol {target_vol}"),
        format!("vol {:02}", target_vol),
        format!("vol. {target_vol}"),
        format!("vol. {:02}", target_vol),
        format!("vol.{target_vol}"),
        format!("vol.{:02}", target_vol),
        format!("volume {target_vol}"),
        format!("volume {:02}", target_vol),
        format!("tome {target_vol}"),
        format!("tome {:02}", target_vol),
        format!("band {target_vol}"),
        format!("band {:02}", target_vol),
    ];

    for pat in &patterns {
        let pat_bytes = pat.as_bytes();
        let mut start_idx = 0;
        while let Some(rel_pos) = lower[start_idx..].find(pat.as_str()) {
            let match_pos = start_idx + rel_pos;
            let after_pos = match_pos + pat_bytes.len();

            // Check following character: must NOT be an ASCII digit
            let following_is_digit = if after_pos < bytes.len() {
                bytes[after_pos].is_ascii_digit()
            } else {
                false
            };

            // Check preceding character: must NOT be an ASCII letter or digit
            let preceding_is_alphanumeric = if match_pos > 0 {
                bytes[match_pos - 1].is_ascii_alphanumeric()
            } else {
                false
            };

            if !following_is_digit && !preceding_is_alphanumeric {
                return true;
            }

            start_idx = match_pos + 1;
            if start_idx >= lower.len() {
                break;
            }
        }
    }
    false
}

/// Evaluates confidence level and explanation for a matched release candidate.
pub fn evaluate_candidate_confidence(
    candidate_title: &str,
    canonical_series: &str,
    aliases: &[String],
    _target_volume: Option<f64>,
    match_type: CandidateMatchType,
    is_digital: bool,
    is_trusted: bool,
    provider: &str,
    seeders: Option<u32>,
    target_language: &str,
) -> (CandidateConfidence, String) {
    // 1. Language mismatch check
    if let Some(foreign_lang) = detect_foreign_language(candidate_title, target_language) {
        return (
            CandidateConfidence::Ambiguous,
            format!("Foreign language detected ({foreign_lang}); does not match target language '{target_language}'"),
        );
    }

    // 2. Series identity check
    let (series_matched, _, series_reason) = match_series_identity(candidate_title, canonical_series, aliases);
    if !series_matched {
        return (
            CandidateConfidence::Ambiguous,
            format!("Series identity uncertain: {series_reason}"),
        );
    }

    // 3. Rejection
    if match_type == CandidateMatchType::Rejected {
        return (
            CandidateConfidence::Low,
            "Release does not match target volume or edition".to_string(),
        );
    }

    // 4. Exact volume matching
    if match_type == CandidateMatchType::ExactVolume {
        if is_digital || is_trusted || provider == "madokami" || seeders.unwrap_or(0) >= 5 {
            return (
                CandidateConfidence::High,
                format!("High confidence exact volume match for '{canonical_series}' from {provider}"),
            );
        } else {
            return (
                CandidateConfidence::Medium,
                format!("Exact volume match, but low seed count or unverified uploader from {provider}"),
            );
        }
    }

    // 5. Volume ranges / omnibuses
    if match_type == CandidateMatchType::VolumeRangeContainingTarget {
        return (
            CandidateConfidence::Medium,
            "Volume range or omnibus contains target volume; review recommended before acquisition".to_string(),
        );
    }

    // 6. Series packs
    if match_type == CandidateMatchType::SeriesPack {
        return (
            CandidateConfidence::Medium,
            "Series pack or complete collection covers target volume".to_string(),
        );
    }

    // 7. Possible / fuzzy match
    if match_type == CandidateMatchType::PossibleMatch {
        return (
            CandidateConfidence::Low,
            "Fuzzy volume markers detected; manual inspection required to verify contents".to_string(),
        );
    }

    (
        CandidateConfidence::Ambiguous,
        "Ambiguous release classification; requires manual review".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_4_vs_40_strict_isolation() {
        assert!(strict_fuzzy_volume_match("Haikyu!! v04", 4));
        assert!(strict_fuzzy_volume_match("Haikyu!! v4", 4));
        assert!(strict_fuzzy_volume_match("Haikyu!! Vol 4", 4));
        assert!(strict_fuzzy_volume_match("Haikyu!! Volume 04 (Digital)", 4));

        // Critical requirement: Volume 4 must NOT match Volume 40!
        assert!(!strict_fuzzy_volume_match("Haikyu!! v40", 4));
        assert!(!strict_fuzzy_volume_match("Haikyu!! v40.cbz", 4));
        assert!(!strict_fuzzy_volume_match("Haikyu!! Vol 40", 4));
        assert!(!strict_fuzzy_volume_match("Haikyu!! Volume 40", 4));

        // Volume 40 matches 40
        assert!(strict_fuzzy_volume_match("Haikyu!! v40", 40));
        assert!(strict_fuzzy_volume_match("Haikyu!! Volume 40", 40));
    }

    #[test]
    fn test_spinoff_rejection() {
        // Spin-off works must be rejected or flagged
        assert_eq!(
            has_unmatched_spinoff_marker("Haikyu-bu!! v01", "Haikyu!!"),
            Some("-bu!!".to_string())
        );
        assert_eq!(
            has_unmatched_spinoff_marker("Attack on Titan: Junior High v01", "Attack on Titan"),
            Some("junior high".to_string())
        );
        assert_eq!(
            has_unmatched_spinoff_marker("Berserk Official Guidebook", "Berserk"),
            Some("official guidebook".to_string())
        );

        // Canonical series that naturally have those words are not rejected
        assert_eq!(
            has_unmatched_spinoff_marker("Attack on Titan: Junior High v01", "Attack on Titan: Junior High"),
            None
        );
    }

    #[test]
    fn test_foreign_language_detection() {
        assert_eq!(
            detect_foreign_language("[FR] Haikyu!! v04.cbz", "en"),
            Some("French".to_string())
        );
        assert_eq!(
            detect_foreign_language("Haikyu!! v04 [German].cbz", "en"),
            Some("German".to_string())
        );
        assert_eq!(
            detect_foreign_language("Haikyu!! v04 [RAW].cbz", "en"),
            Some("Japanese RAW".to_string())
        );
        assert_eq!(
            detect_foreign_language("Haikyu!! v04 (Digital) (Danke-Empire).cbz", "en"),
            None
        );
    }

    #[test]
    fn test_confidence_classification() {
        let (conf_high, _) = evaluate_candidate_confidence(
            "Haikyu!! v04 (Digital) (Danke-Empire)",
            "Haikyu!!",
            &[],
            Some(4.0),
            CandidateMatchType::ExactVolume,
            true,
            true,
            "nyaa",
            Some(25),
            "en",
        );
        assert_eq!(conf_high, CandidateConfidence::High);

        let (conf_omnibus, _) = evaluate_candidate_confidence(
            "Haikyu!! v01-05 (Digital)",
            "Haikyu!!",
            &[],
            Some(4.0),
            CandidateMatchType::VolumeRangeContainingTarget,
            true,
            true,
            "nyaa",
            Some(25),
            "en",
        );
        assert_eq!(conf_omnibus, CandidateConfidence::Medium);

        let (conf_foreign, _) = evaluate_candidate_confidence(
            "[FR] Haikyu!! v04",
            "Haikyu!!",
            &[],
            Some(4.0),
            CandidateMatchType::ExactVolume,
            true,
            true,
            "nyaa",
            Some(25),
            "en",
        );
        assert_eq!(conf_foreign, CandidateConfidence::Ambiguous);
    }
}
