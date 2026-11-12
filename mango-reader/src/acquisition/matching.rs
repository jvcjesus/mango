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
pub const SPINOFF_MARKERS: &[&str] = &[
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
pub const FOREIGN_LANGUAGE_TAGS: &[(&str, &str)] = &[
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

/// Non-substantive format, edition, and noise tokens ignored during series title matching.
pub const NOISE_TOKENS: &[&str] = &[
    "the", "a", "an", "manga", "comic", "comics", "digital", "edition",
    "deluxe", "tankobon", "remastered", "hq", "web", "colored", "color",
    "master", "perfect", "collector", "collectors", "revised", "standard",
    "omnibus", "vol", "vols", "volume", "volumes", "ch", "chapter", "chapters",
    "tome", "tomes", "band",
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
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Extracts the candidate series title from a raw release title by:
/// 1. Stripping leading release group or uploader tags (e.g. `[Group]`, `(Uploader)`).
/// 2. Stripping trailing file extensions (e.g. `.cbz`, `.cbr`, `.zip`, `.rar`).
/// 3. Finding volume, chapter, collection, or trailing metadata boundaries.
/// 4. Trimming separators and noise.
pub fn extract_candidate_series_title(raw_title: &str) -> String {
    let mut s = raw_title.trim();

    // 1. Strip file extension
    for ext in &[".cbz", ".cbr", ".zip", ".rar", ".7z", ".pdf", ".epub"] {
        if let Some(pos) = s.to_ascii_lowercase().rfind(ext) {
            if pos + ext.len() == s.len() {
                s = &s[..pos];
            }
        }
    }

    // 2. Strip leading bracketed tags (e.g. [Uploader] or (Group))
    loop {
        s = s.trim_start();
        if (s.starts_with('[') && s.contains(']')) || (s.starts_with('(') && s.contains(')')) {
            let close_char = if s.starts_with('[') { ']' } else { ')' };
            if let Some(close_idx) = s.find(close_char) {
                // Only treat as leading uploader tag if it's reasonably short (<= 40 chars)
                // and doesn't span the entire string
                if close_idx <= 40 && close_idx + 1 < s.len() {
                    s = &s[close_idx + 1..];
                    continue;
                }
            }
        }
        break;
    }

    let trimmed = s.trim();
    if trimmed.is_empty() {
        return raw_title.trim().to_string();
    }

    // 3. Scan for volume/chapter boundaries or edition/year tags
    let lower = trimmed.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    let boundary_patterns = [
        " - c", " - v", " - ch", " - chapter", " - vol", " - volume", " - tome", " - band",
        " - #",
        " v.", " v ", " vol.", " vol ", " vols.", " vols ", " volume ", " volumes ", " tome ", " tomes ", " band ",
        " ch.", " ch ", " chapter ", " chapters ",
        " (digital", " [digital", " (20", " (19", " [20", " [19",
        " [complete", " (complete", " [batch", " (batch",
        " [remux", " (remux", " [web", " (web",
        " master edition", " deluxe edition", " perfect edition", " collector edition", " collectors edition",
    ];

    let mut earliest_boundary = trimmed.len();

    for pat in &boundary_patterns {
        if let Some(pos) = lower.find(pat) {
            if pos > 0 && pos < earliest_boundary {
                let after = pos + pat.len();
                if pat.starts_with(" v ") || pat.starts_with(" v.") {
                    if after < bytes.len() && (bytes[after].is_ascii_digit() || bytes[after] == b' ') {
                        earliest_boundary = pos;
                    }
                } else {
                    earliest_boundary = pos;
                }
            }
        }
    }

    // Scan words for volume or chapter markers:
    // e.g. "Haikyu!! v06" where word is "v06" or "#01"
    let mut current_offset = 0;
    for (i, word) in trimmed.split_whitespace().enumerate() {
        let w_offset = match trimmed[current_offset..].find(word) {
            Some(rel) => current_offset + rel,
            None => current_offset,
        };
        current_offset = w_offset + word.len();

        let w_lower = word.to_ascii_lowercase();

        // Check if word is a volume/chapter indicator
        let is_vol_pattern = (w_lower.starts_with('v')
            && w_lower.len() > 1
            && w_lower[1..].chars().next().map_or(false, |c| c.is_ascii_digit()))
            || (w_lower.starts_with("vol")
                && w_lower.len() > 3
                && w_lower[3..].chars().next().map_or(false, |c| c.is_ascii_digit()))
            || (w_lower.starts_with('c')
                && w_lower.len() > 1
                && w_lower[1..].chars().next().map_or(false, |c| c.is_ascii_digit()))
            || (w_lower.starts_with("ch")
                && w_lower.len() > 2
                && w_lower[2..].chars().next().map_or(false, |c| c.is_ascii_digit()))
            || (w_lower.starts_with('#')
                && w_lower.len() > 1
                && w_lower[1..].chars().next().map_or(false, |c| c.is_ascii_digit()));

        let is_keyword = i > 0
            && (w_lower == "v"
                || w_lower == "vol"
                || w_lower == "vols"
                || w_lower == "vol."
                || w_lower == "vols."
                || w_lower == "volume"
                || w_lower == "volumes"
                || w_lower == "ch"
                || w_lower == "ch."
                || w_lower == "chapter"
                || w_lower == "chapters"
                || w_lower == "tome"
                || w_lower == "tomes"
                || w_lower == "band");

        if (is_vol_pattern || is_keyword) && w_offset > 0 && w_offset < earliest_boundary {
            earliest_boundary = w_offset;
            break;
        }

        // Check for numeric chapter after hyphen: "Series - 01"
        if i > 1 && word.chars().all(|c| c.is_ascii_digit()) {
            if let Some(dash_pos) = trimmed[..w_offset].rfind('-') {
                if dash_pos > 0 && dash_pos < earliest_boundary {
                    earliest_boundary = dash_pos;
                    break;
                }
            }
        }
    }

    let mut candidate_series = trimmed[..earliest_boundary].trim();
    candidate_series = candidate_series
        .trim_end_matches(|c: char| c == '-' || c == ':' || c == '_' || c == ',' || c == '.')
        .trim();

    if candidate_series.is_empty() {
        trimmed.to_string()
    } else {
        candidate_series.to_string()
    }
}

/// Evaluates series title matching between candidate title and canonical title + aliases.
/// Returns `(matched, score, reason)`.
///
/// Guarantees:
/// - Rejects false positive substring matches (e.g. "Nana" will never match "Banana Fish").
/// - Rejects separate series sharing prefix words (e.g. "Berserk" will never match "Berserk of Gluttony").
/// - Rejects spinoff markers not present in canonical title (e.g. "Monster Musume" will never match "Monster").
/// - Preserves exact matches and alias matches with high confidence.
pub fn match_series_identity(
    candidate_title: &str,
    canonical_title: &str,
    aliases: &[String],
) -> (bool, f64, String) {
    // 1. Check spin-off markers across raw title
    if let Some(marker) = has_unmatched_spinoff_marker(candidate_title, canonical_title) {
        return (
            false,
            0.0,
            format!("Spin-off or subtitle marker detected: '{marker}'"),
        );
    }

    let cand_series = extract_candidate_series_title(candidate_title);
    if let Some(marker) = has_unmatched_spinoff_marker(&cand_series, canonical_title) {
        return (
            false,
            0.0,
            format!("Spin-off or subtitle marker detected in candidate series: '{marker}'"),
        );
    }

    let cand_tokens = tokenize(&cand_series);
    let canon_tokens = tokenize(canonical_title);

    let cand_substantive: Vec<String> = cand_tokens
        .iter()
        .filter(|t| !NOISE_TOKENS.contains(&t.as_str()))
        .filter(|t| {
            if t.chars().all(|c| c.is_ascii_digit()) {
                canon_tokens.contains(t)
            } else {
                true
            }
        })
        .cloned()
        .collect();

    let canon_substantive: Vec<String> = canon_tokens
        .iter()
        .filter(|t| !NOISE_TOKENS.contains(&t.as_str()))
        .cloned()
        .collect();

    if canon_substantive.is_empty() {
        return (
            false,
            0.0,
            "Canonical title has no substantive tokens".to_string(),
        );
    }

    // 2. Exact substantive token match for canonical title
    if cand_substantive == canon_substantive {
        return (
            true,
            100.0,
            format!("Exact series match for '{canonical_title}'"),
        );
    }

    // 3. Check for extra substantive tokens in candidate series that are NOT in canonical title
    let extra_tokens: Vec<&String> = cand_substantive
        .iter()
        .filter(|ct| !canon_substantive.contains(ct))
        .collect();

    // If candidate series title has extra substantive words not in canonical title,
    // check if they match an alias
    if !extra_tokens.is_empty() {
        for alias in aliases {
            let alias_tokens = tokenize(alias);
            let alias_substantive: Vec<String> = alias_tokens
                .iter()
                .filter(|t| !NOISE_TOKENS.contains(&t.as_str()))
                .cloned()
                .collect();

            if cand_substantive == alias_substantive {
                return (
                    true,
                    95.0,
                    format!("Exact alias match for '{alias}'"),
                );
            }

            let extra_alias_tokens: Vec<&String> = cand_substantive
                .iter()
                .filter(|ct| !alias_substantive.contains(ct))
                .collect();

            if extra_alias_tokens.is_empty()
                && alias_substantive
                    .iter()
                    .all(|at| cand_substantive.contains(at))
            {
                return (
                    true,
                    90.0,
                    format!("Series alias match for '{alias}'"),
                );
            }
        }

        // Extra substantive tokens exist and were NOT accounted for by canonical title or any alias!
        // This is a DIFFERENT series or unauthorized spinoff/variant (e.g. Berserk of Gluttony vs Berserk)
        return (
            false,
            0.0,
            format!(
                "Candidate series contains extra unmatched title tokens: {:?}",
                extra_tokens
            ),
        );
    }

    // 4. Candidate has no extra substantive tokens; check if all canonical tokens are present
    let missing_tokens: Vec<&String> = canon_substantive
        .iter()
        .filter(|ct| !cand_substantive.contains(ct))
        .collect();

    if missing_tokens.is_empty() {
        // All canonical tokens are present, and candidate has no extra substantive tokens!
        return (
            true,
            95.0,
            format!("Exact series token match for '{canonical_title}'"),
        );
    }

    // Candidate is missing some canonical tokens. Check aliases!
    for alias in aliases {
        let alias_tokens = tokenize(alias);
        let alias_substantive: Vec<String> = alias_tokens
            .iter()
            .filter(|t| !NOISE_TOKENS.contains(&t.as_str()))
            .cloned()
            .collect();

        if cand_substantive == alias_substantive {
            return (
                true,
                95.0,
                format!("Exact alias match for '{alias}'"),
            );
        }
    }

    // If candidate has at least 2 substantive tokens matching canonical prefix
    if cand_substantive.len() >= 2 && canon_substantive.starts_with(&cand_substantive) {
        return (
            true,
            75.0,
            format!(
                "Prefix token match for '{canonical_title}' (missing: {:?})",
                missing_tokens
            ),
        );
    }

    (
        false,
        0.0,
        format!(
            "Title does not match canonical series '{canonical_title}' (missing: {:?})",
            missing_tokens
        ),
    )
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

    #[test]
    fn test_extract_candidate_series_title() {
        assert_eq!(
            extract_candidate_series_title("[danke-Empire] Haikyu!! v06 (2016) (Digital) (danke-Empire).cbz"),
            "Haikyu!!"
        );
        assert_eq!(
            extract_candidate_series_title("[LuCaZ] Berserk of Gluttony v01 (2020) (Digital) [Empire]"),
            "Berserk of Gluttony"
        );
        assert_eq!(
            extract_candidate_series_title("Monster Musume - Chapter 01"),
            "Monster Musume"
        );
        assert_eq!(
            extract_candidate_series_title("Banana Fish - 01"),
            "Banana Fish"
        );
        assert_eq!(
            extract_candidate_series_title("Spy x Family - c001 (v01) [Digital]"),
            "Spy x Family"
        );
        assert_eq!(
            extract_candidate_series_title("Nana [Complete] [v01-v21]"),
            "Nana"
        );
    }

    #[test]
    fn test_strict_series_identity_disambiguation() {
        let (matched_berserk, score_b, _) = match_series_identity(
            "[danke-Empire] Berserk v01 (Digital)",
            "Berserk",
            &[],
        );
        assert!(matched_berserk);
        assert_eq!(score_b, 100.0);

        // Berserk of Gluttony must be REJECTED for Berserk!
        let (matched_gluttony, score_g, reason_g) = match_series_identity(
            "[danke-Empire] Berserk of Gluttony v01 (Digital)",
            "Berserk",
            &[],
        );
        assert!(!matched_gluttony);
        assert_eq!(score_g, 0.0);
        assert!(reason_g.contains("extra unmatched title tokens"), "Reason: {reason_g}");

        // Monster Musume must be REJECTED for Monster!
        let (matched_musume, score_m, reason_m) = match_series_identity(
            "Monster Musume v01",
            "Monster",
            &[],
        );
        assert!(!matched_musume);
        assert_eq!(score_m, 0.0);
        assert!(reason_m.contains("extra unmatched title tokens"), "Reason: {reason_m}");

        // Banana Fish must be REJECTED for Nana!
        let (matched_banana, score_banana, _) = match_series_identity(
            "Banana Fish v01",
            "Nana",
            &[],
        );
        assert!(!matched_banana);
        assert_eq!(score_banana, 0.0);

        // One Piece Digital Colored Comics matches One Piece!
        let (matched_op, score_op, _) = match_series_identity(
            "One Piece - Digital Colored Comics v01",
            "One Piece",
            &[],
        );
        assert!(matched_op);
        assert_eq!(score_op, 100.0);

        // Aliases match with high confidence
        let (matched_alias, score_alias, _) = match_series_identity(
            "[Group] HQ!! v01",
            "Haikyu!!",
            &["HQ!!".to_string()],
        );
        assert!(matched_alias);
        assert!(score_alias >= 90.0);
    }
}
