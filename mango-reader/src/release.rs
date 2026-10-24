//! Release classification and volume parsing.
//!
//! Distinguishes first-class acquisition units (Chapter vs Volume) and conservatively
//! detects trustworthy volume information without guessing or fabricating volume boundaries.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseType {
    Chapter,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub release_type: ReleaseType,
    pub volume: Option<f64>,
    pub chapter_number: Option<f64>,
}

fn leading_number(text: &str) -> Option<f64> {
    if text.is_empty() || !text.chars().next()?.is_ascii_digit() {
        return None;
    }
    let end = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let candidate = text[..end].trim_end_matches('.');
    candidate.parse::<f64>().ok()
}

/// Conservatively parses a volume number from a release name like "Vol. 1 Ch. 5",
/// "Volume 2 - Chapter 14", "v3 c20", "Volume 12", "Vol 12", "Volume 0", etc.
///
/// Returns None if no explicit volume indicator is found or if the number is an unrelated
/// revision/version (e.g. "Chapter 5 v2").
pub fn parse_volume(name: &str) -> Option<f64> {
    if name.is_empty() {
        return None;
    }
    let lower = name.to_lowercase();
    let bytes = lower.as_bytes();

    // 1. Look for explicit "vol." / "volume" / "vol " / "vol_"
    for (i, _) in lower.match_indices("vol") {
        let ok_before = if i == 0 {
            true
        } else {
            let prev = bytes[i - 1] as char;
            !prev.is_alphanumeric()
        };
        if !ok_before {
            continue;
        }

        let mut rest = &lower[i + 3..];
        if rest.starts_with("ume") {
            rest = &rest[3..];
        }

        let trimmed = rest.trim_start_matches(|c: char| c == '.' || c == ':' || c == '-' || c == '_' || c.is_whitespace());
        if trimmed.is_empty() {
            continue;
        }

        if let Some(val) = leading_number(trimmed) {
            return Some(val);
        }
    }

    // 2. Check for short "v<number>" pattern (e.g. "v3 c20", "v01")
    for (i, _) in lower.match_indices('v') {
        let ok_before = if i == 0 {
            true
        } else {
            let prev = bytes[i - 1] as char;
            prev.is_whitespace() || ['[', '(', '{', '<', '-', '_', '.', ','].contains(&prev)
        };
        if !ok_before {
            continue;
        }

        // Reject revision/version strings like "Chapter 5 v2" where chapter marker precedes "v"
        let before_text = &lower[..i];
        let has_prior_chapter = ["chapter", "ch.", "ch ", "ch-", "ch_", "episode", "ep.", "ep ", "#"]
            .iter()
            .any(|mark| before_text.contains(mark));
        if has_prior_chapter {
            continue;
        }

        let mut rest = &lower[i + 1..];
        rest = rest.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
        if rest.is_empty() {
            continue;
        }

        if let Some(val) = leading_number(rest) {
            let num_len = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
            let num_str = &rest[..num_len];
            if num_str.matches('.').count() > 1 {
                continue;
            }

            let after_num = &rest[num_len..];
            let after_trimmed = after_num.trim_start();
            let followed_by_chapter = after_trimmed.starts_with('c')
                || after_trimmed.starts_with("ch")
                || after_trimmed.starts_with('#')
                || after_trimmed.starts_with('-');
            let in_brackets = i > 0 && ['[', '(', '{', '<'].contains(&(bytes[i - 1] as char));
            let is_end_or_meta = after_trimmed.is_empty()
                || after_trimmed.starts_with('[')
                || after_trimmed.starts_with('(')
                || after_trimmed.starts_with('{');

            if i == 0 || in_brackets || followed_by_chapter || is_end_or_meta {
                return Some(val);
            }
        }
    }

    None
}

/// Distinguishes whether a release represents a complete Volume artifact or a Chapter artifact
/// (which may or may not belong to a known volume).
pub fn parse_release(name: &str, known_chapter_number: f64) -> ReleaseInfo {
    let volume = parse_volume(name);
    let lower = name.to_lowercase();

    let has_chapter_word = ["chapter", "ch.", "ch ", "ch-", "ch_", "episode", "ep.", "ep ", "act", "#"]
        .iter()
        .any(|mark| lower.contains(mark));

    let has_c_chapter = if volume.is_some() {
        lower.split_whitespace().any(|word| {
            let w = word.trim_matches(|c: char| !c.is_alphanumeric());
            w.starts_with('c') && w.len() > 1 && w[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
        })
    } else {
        false
    };

    let ch_from_name = crate::titles::number_from_name(name);

    let is_volume = if let Some(vol) = volume {
        if has_chapter_word || has_c_chapter {
            false
        } else if ch_from_name >= 0.0 && (ch_from_name - vol).abs() > 0.001 {
            // Distinct chapter number found after volume (e.g. "Volume 3 - 41")
            false
        } else {
            true
        }
    } else {
        false
    };

    if is_volume {
        ReleaseInfo {
            release_type: ReleaseType::Volume,
            volume,
            chapter_number: None,
        }
    } else {
        let chapter_num = if ch_from_name >= 0.0 {
            Some(ch_from_name)
        } else if known_chapter_number >= 0.0 {
            Some(known_chapter_number)
        } else {
            None
        };

        ReleaseInfo {
            release_type: ReleaseType::Chapter,
            volume,
            chapter_number: chapter_num,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_volume_parsing() {
        assert_eq!(parse_volume("Vol. 1 Ch. 5"), Some(1.0));
        assert_eq!(parse_volume("Volume 2 - Chapter 14"), Some(2.0));
        assert_eq!(parse_volume("v3 c20"), Some(3.0));
        assert_eq!(parse_volume("Volume 12"), Some(12.0));
        assert_eq!(parse_volume("Vol 12"), Some(12.0));
        assert_eq!(parse_volume("Volume 0"), Some(0.0));
        assert_eq!(parse_volume("Vol. 0 Ch. 0.5"), Some(0.0));
        assert_eq!(parse_volume("Vol. 2.5 Ch. 10"), Some(2.5));
        assert_eq!(parse_volume("[Group] Series Vol. 5 Chapter 20"), Some(5.0));
        assert_eq!(parse_volume("Series v01"), Some(1.0));
        assert_eq!(parse_volume("[Group] Series v02"), Some(2.0));
        assert_eq!(parse_volume("Series v12.5"), Some(12.5));
        assert_eq!(parse_volume("[Group] Series v01 [Digital]"), Some(1.0));

        // Rejections: unrelated numbers, revisions, or pure chapter strings
        assert_eq!(parse_volume("Chapter 1"), None);
        assert_eq!(parse_volume("Chapter 5 v2"), None, "v2 revision must not be parsed as volume");
        assert_eq!(parse_volume("Ch. 12 (v2)"), None, "revision must not be volume");
        assert_eq!(parse_volume("Season 2 14"), None, "season must not be volume");
        assert_eq!(parse_volume("104."), None);
        assert_eq!(parse_volume("Oneshot"), None);
        assert_eq!(parse_volume(""), None);
    }

    #[test]
    fn release_semantics_and_classification() {
        // Pure chapter with no volume
        let rel1 = parse_release("Chapter 1", 1.0);
        assert_eq!(rel1.release_type, ReleaseType::Chapter);
        assert_eq!(rel1.volume, None);
        assert_eq!(rel1.chapter_number, Some(1.0));

        // Explicit volume + chapter
        let rel2 = parse_release("Vol. 1 Ch. 5", 5.0);
        assert_eq!(rel2.release_type, ReleaseType::Chapter);
        assert_eq!(rel2.volume, Some(1.0));
        assert_eq!(rel2.chapter_number, Some(5.0));

        // Source-provided volume artifact
        let rel3 = parse_release("Volume 12", 12.0);
        assert_eq!(rel3.release_type, ReleaseType::Volume);
        assert_eq!(rel3.volume, Some(12.0));
        assert_eq!(rel3.chapter_number, None, "Volume artifact should not have fake chapter number");

        // Short notation v3 c20
        let rel4 = parse_release("v3 c20", -1.0);
        assert_eq!(rel4.release_type, ReleaseType::Chapter);
        assert_eq!(rel4.volume, Some(3.0));
        assert_eq!(rel4.chapter_number, Some(20.0));

        // Revision Ch 5 v2
        let rel5 = parse_release("Chapter 5 v2", 5.0);
        assert_eq!(rel5.release_type, ReleaseType::Chapter);
        assert_eq!(rel5.volume, None);
        assert_eq!(rel5.chapter_number, Some(5.0));

        // Volume 3 - 41
        let rel6 = parse_release("Volume 3 - 41 A Title", -1.0);
        assert_eq!(rel6.release_type, ReleaseType::Chapter);
        assert_eq!(rel6.volume, Some(3.0));
        assert_eq!(rel6.chapter_number, Some(41.0));

        // Volume 0
        let rel7 = parse_release("Volume 0", 0.0);
        assert_eq!(rel7.release_type, ReleaseType::Volume);
        assert_eq!(rel7.volume, Some(0.0));
        assert_eq!(rel7.chapter_number, None);

        // Canonical format Series v01
        let rel8 = parse_release("Blue Lock v01", 1.0);
        assert_eq!(rel8.release_type, ReleaseType::Volume);
        assert_eq!(rel8.volume, Some(1.0));
        assert_eq!(rel8.chapter_number, None, "True volume must never receive fake chapter number");

        // Fractional volume Series v12.5
        let rel9 = parse_release("Series v12.5", 12.5);
        assert_eq!(rel9.release_type, ReleaseType::Volume);
        assert_eq!(rel9.volume, Some(12.5));
        assert_eq!(rel9.chapter_number, None);

        // Group release with tags
        let rel10 = parse_release("[Group] Series v02 [Digital]", 2.0);
        assert_eq!(rel10.release_type, ReleaseType::Volume);
        assert_eq!(rel10.volume, Some(2.0));
        assert_eq!(rel10.chapter_number, None);
    }
}
