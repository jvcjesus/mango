//! Canonical manga naming standardization and metadata normalization.
//!
//! Enforces predictable, library-wide reader-facing conventions across all
//! manga series and acquisition sources (Madokami, Nyaa, extensions, etc.).
//!
//! Reader-facing standards:
//! - Volume 1  -> `Volume 01` (minimum 2 digits, expands naturally: Volume 12, Volume 100)
//! - Volume 2  -> `Volume 02`
//! - Volume 12 -> `Volume 12`
//! - Chapter 1  -> `Chapter 001` (minimum 3 digits, expands naturally: Chapter 100, Chapter 1000)
//! - Chapter 25 -> `Chapter 025`
//! - Chapter 100 -> `Chapter 100`
//!
//! Meaningful chapter titles are preserved cleanly as `Chapter 001 - Title`.
//! Leaked source conventions (`(Digital)`, `[ScanGroup]`, `v01`, etc.) are stripped.

use serde::{Deserialize, Serialize};

/// Classification of reader-facing title conformity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NamingConformity {
    /// Title strictly adheres to canonical standard.
    Canonical,
    /// Inconsistent volume or chapter title formatting (e.g. "Vol. 01 (Digital)", "Volume 3").
    InconsistentTitle,
    /// Inconsistent or missing volume/chapter number.
    InconsistentNumber,
    /// Multi-volume or multi-chapter omnibus/pack range.
    Range,
    /// Special, Extra, or Omari content.
    Special,
    /// Standalone one-shot release.
    OneShot,
    /// Ambiguous title or number that cannot be deterministically resolved.
    Ambiguous,
}

impl std::fmt::Display for NamingConformity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canonical => write!(f, "CANONICAL"),
            Self::InconsistentTitle => write!(f, "INCONSISTENT_TITLE"),
            Self::InconsistentNumber => write!(f, "INCONSISTENT_NUMBER"),
            Self::Range => write!(f, "RANGE"),
            Self::Special => write!(f, "SPECIAL"),
            Self::OneShot => write!(f, "ONE_SHOT"),
            Self::Ambiguous => write!(f, "AMBIGUOUS"),
        }
    }
}

/// Resolved canonical metadata title and numbering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalNamingResult {
    /// Reader-facing title (e.g. "Volume 01", "Chapter 001 - The Beginning").
    pub title: String,
    /// Structured numeric string (e.g. "1", "1.5", "25").
    pub number_string: String,
    /// Numeric sort value for deterministic ordering in Komga/Mango.
    pub number_sort: f64,
    /// Conformity assessment of the input title.
    pub conformity: NamingConformity,
    /// Cleaned meaningful subtitle if present (e.g. "The Beginning").
    pub subtitle: Option<String>,
    /// Whether the original title already perfectly matched canonical conventions.
    pub is_already_canonical: bool,
    /// Reason or details of the transformation.
    pub reason: String,
}

/// Formats a canonical volume title according to fork standards.
///
/// Whole numbers are zero-padded to at least 2 digits, expanding naturally without hardcoding:
/// - 1.0   -> "Volume 01"
/// - 2.0   -> "Volume 02"
/// - 12.0  -> "Volume 12"
/// - 100.0 -> "Volume 100"
///
/// Fractional numbers retain decimal components:
/// - 1.5   -> "Volume 01.5"
/// - 10.5  -> "Volume 10.5"
pub fn format_canonical_volume_title(volume: f64) -> String {
    if volume < 0.0 {
        return "Volume".to_string();
    }
    if volume.fract() == 0.0 {
        let n = volume as i64;
        if n < 10 {
            format!("Volume {:02}", n)
        } else {
            format!("Volume {}", n)
        }
    } else {
        let whole = volume.trunc() as i64;
        let frac_str = format!("{:.3}", volume.fract());
        let frac_trimmed = frac_str.trim_start_matches("0.").trim_end_matches('0');
        if whole < 10 {
            format!("Volume {:02}.{}", whole, frac_trimmed)
        } else {
            format!("Volume {}.{}", whole, frac_trimmed)
        }
    }
}

/// Formats a canonical volume range title.
/// Example: (1.0, 3.0) -> "Volumes 01-03"
pub fn format_canonical_volume_range(start: f64, end: f64) -> String {
    let s_str = if start.fract() == 0.0 {
        let n = start as i64;
        if n < 10 { format!("{:02}", n) } else { format!("{}", n) }
    } else {
        format!("{start}")
    };
    let e_str = if end.fract() == 0.0 {
        let n = end as i64;
        if n < 10 { format!("{:02}", n) } else { format!("{}", n) }
    } else {
        format!("{end}")
    };
    format!("Volumes {s_str}-{e_str}")
}

/// Formats a canonical chapter title according to fork standards.
///
/// Whole numbers are zero-padded to at least 3 digits, expanding naturally:
/// - 1.0    -> "Chapter 001"
/// - 25.0   -> "Chapter 025"
/// - 100.0  -> "Chapter 100"
/// - 1000.0 -> "Chapter 1000"
///
/// Meaningful subtitles are preserved cleanly:
/// - (1.0, Some("The Beginning")) -> "Chapter 001 - The Beginning"
pub fn format_canonical_chapter_title(chapter: f64, subtitle: Option<&str>) -> String {
    if chapter < 0.0 {
        return if let Some(sub) = subtitle.map(str::trim).filter(|s| !s.is_empty()) {
            format!("Special - {sub}")
        } else {
            "Special".to_string()
        };
    }

    let prefix = if chapter.fract() == 0.0 {
        let n = chapter as i64;
        if n < 100 {
            format!("Chapter {:03}", n)
        } else {
            format!("Chapter {}", n)
        }
    } else {
        let whole = chapter.trunc() as i64;
        let frac_str = format!("{:.3}", chapter.fract());
        let frac_trimmed = frac_str.trim_start_matches("0.").trim_end_matches('0');
        if whole < 100 {
            format!("Chapter {:03}.{}", whole, frac_trimmed)
        } else {
            format!("Chapter {}.{}", whole, frac_trimmed)
        }
    };

    if let Some(sub) = subtitle.map(str::trim).filter(|s| !s.is_empty()) {
        format!("{prefix} - {sub}")
    } else {
        prefix
    }
}

/// Formats a canonical chapter range title.
/// Example: (1.0, 5.0) -> "Chapters 001-005"
pub fn format_canonical_chapter_range(start: f64, end: f64) -> String {
    let s_str = if start.fract() == 0.0 {
        let n = start as i64;
        if n < 100 { format!("{:03}", n) } else { format!("{}", n) }
    } else {
        format!("{start}")
    };
    let e_str = if end.fract() == 0.0 {
        let n = end as i64;
        if n < 100 { format!("{:03}", n) } else { format!("{}", n) }
    } else {
        format!("{end}")
    };
    format!("Chapters {s_str}-{e_str}")
}

/// Strips bracketed tokens like `[Digital]`, `(Digital)`, `(2016)`, `[danke-Empire]`.
fn strip_bracketed_noise(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    let mut current_tag = String::new();

    for c in text.chars() {
        if c == '(' || c == '[' {
            depth += 1;
            current_tag.clear();
        } else if (c == ')' || c == ']') && depth > 0 {
            depth -= 1;
            let tag_lower = current_tag.trim().to_lowercase();
            // Check if tag is known noise
            let is_noise = tag_lower.contains("digital")
                || tag_lower.contains("raw")
                || tag_lower.contains("web")
                || tag_lower.contains("official")
                || tag_lower.contains("empire")
                || tag_lower.contains("viz")
                || tag_lower.contains("kodansha")
                || tag_lower.contains("crunchyroll")
                || tag_lower.contains("mangaplus")
                || tag_lower.starts_with('v') && tag_lower[1..].chars().all(|d| d.is_ascii_digit())
                || tag_lower.len() == 4 && tag_lower.chars().all(|d| d.is_ascii_digit());
            if !is_noise {
                out.push('(');
                out.push_str(&current_tag);
                out.push(')');
            }
            current_tag.clear();
        } else if depth > 0 {
            current_tag.push(c);
        } else {
            out.push(c);
        }
    }

    out
}

/// Strips source-specific scanlation noise, digital tags, and redundant prefixes
/// while preserving genuine author-given story titles.
pub fn clean_meaningful_subtitle(
    raw: &str,
    series: &str,
    is_volume: bool,
    number: Option<f64>,
) -> Option<String> {
    let mut cleaned = raw.trim().to_string();
    if cleaned.is_empty() {
        return None;
    }

    // Strip series title prefix if present at start
    let s_trimmed = series.trim();
    if !s_trimmed.is_empty() && cleaned.to_lowercase().starts_with(&s_trimmed.to_lowercase()) {
        cleaned = cleaned[s_trimmed.len()..].trim().to_string();
    }

    // Strip bracketed noise: (Digital), [MangaPlus], (2016), etc.
    cleaned = strip_bracketed_noise(&cleaned);

    // Strip leading punctuation often left after series name
    cleaned = cleaned.trim_start_matches(['-', ':', '·', '_', ' ']).trim().to_string();

    // Strip volume / chapter prefix
    let lower = cleaned.to_lowercase();
    let prefix_len = if is_volume {
        if lower.starts_with("volume") {
            Some(6)
        } else if lower.starts_with("vol.") {
            Some(4)
        } else if lower.starts_with("vol") {
            Some(3)
        } else if lower.starts_with('v') && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit() || c.is_whitespace()) {
            Some(1)
        } else {
            None
        }
    } else {
        if lower.starts_with("chapter") {
            Some(7)
        } else if lower.starts_with("chap.") {
            Some(5)
        } else if lower.starts_with("chap") {
            Some(4)
        } else if lower.starts_with("ch.") {
            Some(3)
        } else if lower.starts_with("ch") {
            Some(2)
        } else if lower.starts_with('c') && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit() || c.is_whitespace()) {
            Some(1)
        } else {
            None
        }
    };

    if let Some(plen) = prefix_len {
        cleaned = cleaned[plen..].trim_start().to_string();
    }

    // Strip leading digits/number matching our volume/chapter
    let mut num_prefix_len = 0;
    let mut chars = cleaned.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' {
            num_prefix_len += c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    if num_prefix_len > 0 {
        let num_str = &cleaned[..num_prefix_len];
        if let Ok(parsed) = num_str.parse::<f64>() {
            if number.is_none_or(|n| (n - parsed).abs() < 0.001) {
                cleaned = cleaned[num_prefix_len..].to_string();
            }
        }
    }

    // Strip remaining punctuation separators
    cleaned = cleaned.trim_matches(['-', ':', '·', '_', '.', ',', ' ']).trim().to_string();

    // If only meaningless numbers, "Digital", or noise remain, discard
    let lower = cleaned.to_lowercase();
    if cleaned.is_empty()
        || lower == "digital"
        || lower == "raw"
        || lower == "cbr"
        || lower == "cbz"
        || lower.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '.')
    {
        None
    } else {
        Some(cleaned)
    }
}

/// Parses volume or chapter number from raw text if not provided structured.
pub fn parse_number_from_text(text: &str, is_volume: bool) -> Option<f64> {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '.').filter(|s| !s.is_empty()).collect();

    // Look for unit indicator prefix
    for (i, &t) in tokens.iter().enumerate() {
        let is_marker = if is_volume {
            t == "volume" || t == "vol" || t == "v"
        } else {
            t == "chapter" || t == "chap" || t == "ch" || t == "c"
        };

        if is_marker {
            if let Some(&next) = tokens.get(i + 1) {
                if let Ok(val) = next.parse::<f64>() {
                    return Some(val);
                }
            }
        } else if is_volume && t.starts_with('v') && t.len() > 1 {
            if let Ok(val) = t[1..].parse::<f64>() {
                return Some(val);
            }
        } else if !is_volume && (t.starts_with("ch") || t.starts_with('c')) {
            let offset = if t.starts_with("ch") { 2 } else { 1 };
            if offset < t.len() {
                if let Ok(val) = t[offset..].parse::<f64>() {
                    return Some(val);
                }
            }
        }
    }

    // Fallback: search for first parseable numeric token
    for &t in &tokens {
        if let Ok(val) = t.parse::<f64>() {
            return Some(val);
        }
    }

    None
}

/// Normalizes any raw book title, volume/chapter numbering, and series context into
/// a structured canonical representation.
pub fn normalize_book_naming(
    raw_title: &str,
    series: &str,
    is_volume: bool,
    number: Option<f64>,
) -> CanonicalNamingResult {
    let trimmed_title = raw_title.trim();
    let parsed_num = parse_number_from_text(trimmed_title, is_volume);
    let num_val = number.or(parsed_num);
    let subtitle = clean_meaningful_subtitle(trimmed_title, series, is_volume, num_val);

    let lower_title = trimmed_title.to_lowercase();
    let has_unit_marker = parsed_num.is_some()
        || lower_title.contains("vol")
        || (lower_title.starts_with('v') && lower_title.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
        || lower_title.contains("chap")
        || lower_title.contains("ch.");

    if let Some(num) = num_val {
        let number_str = if num.fract() == 0.0 {
            (num as i64).to_string()
        } else {
            num.to_string()
        };

        if is_volume {
            if !has_unit_marker && subtitle.is_some() {
                // Preserved custom title that doesn't reference a volume number
                return CanonicalNamingResult {
                    title: trimmed_title.to_string(),
                    number_string: number_str,
                    number_sort: num,
                    conformity: NamingConformity::Special,
                    subtitle,
                    is_already_canonical: false,
                    reason: "Preserved custom volume story title".to_string(),
                };
            }

            let canonical_title = format_canonical_volume_title(num);
            let is_already_canonical = trimmed_title == canonical_title;
            let conformity = if is_already_canonical {
                NamingConformity::Canonical
            } else {
                NamingConformity::InconsistentTitle
            };
            let reason = if is_already_canonical {
                "Title already conforms to canonical Volume format".to_string()
            } else {
                format!("Normalized to canonical standard '{}'", canonical_title)
            };

            CanonicalNamingResult {
                title: canonical_title,
                number_string: number_str,
                number_sort: num,
                conformity,
                subtitle: None,
                is_already_canonical,
                reason,
            }
        } else {
            let canonical_title = format_canonical_chapter_title(num, subtitle.as_deref());
            let is_already_canonical = trimmed_title == canonical_title;
            let conformity = if is_already_canonical {
                NamingConformity::Canonical
            } else {
                NamingConformity::InconsistentTitle
            };
            let reason = if is_already_canonical {
                "Title already conforms to canonical Chapter format".to_string()
            } else {
                format!("Normalized to canonical standard '{}'", canonical_title)
            };

            CanonicalNamingResult {
                title: canonical_title,
                number_string: number_str,
                number_sort: num,
                conformity,
                subtitle,
                is_already_canonical,
                reason,
            }
        }
    } else {
        // Special or Unnumbered release
        let lower = trimmed_title.to_lowercase();
        let is_oneshot = lower.contains("oneshot") || lower.contains("one-shot");
        let is_special = lower.contains("special") || lower.contains("extra") || lower.contains("omake");

        let (title, conformity) = if is_oneshot {
            (trimmed_title.to_string(), NamingConformity::OneShot)
        } else if is_special {
            (trimmed_title.to_string(), NamingConformity::Special)
        } else {
            (trimmed_title.to_string(), NamingConformity::Ambiguous)
        };

        CanonicalNamingResult {
            title,
            number_string: "0".to_string(),
            number_sort: 0.0,
            conformity,
            subtitle: None,
            is_already_canonical: false,
            reason: "Non-standard or unnumbered entry".to_string(),
        }
    }
}

/// Generates the canonical physical CBZ filename for new publications.
///
/// Specification:
/// `Series Title - v03 (Year) (Digital) (Release Group).cbz`
///
/// Omits missing attributes without inventing metadata.
pub fn format_canonical_filename(
    series: &str,
    is_volume: bool,
    number: f64,
    year: Option<i32>,
    is_digital: bool,
    release_group: Option<&str>,
) -> String {
    let clean_series = series.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    
    let unit_tag = if is_volume {
        if number.fract() == 0.0 && number >= 0.0 {
            format!("v{:02}", number as i64)
        } else {
            format!("v{number}")
        }
    } else {
        if number.fract() == 0.0 && number >= 0.0 {
            format!("c{:03}", number as i64)
        } else {
            format!("c{number}")
        }
    };

    let mut parts = vec![format!("{clean_series} - {unit_tag}")];

    if let Some(y) = year {
        if y > 1900 && y < 2100 {
            parts.push(format!("({y})"));
        }
    }

    if is_digital {
        parts.push("(Digital)".to_string());
    }

    if let Some(grp) = release_group.map(str::trim).filter(|g| !g.is_empty()) {
        parts.push(format!("({grp})"));
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_canonical_volume_title() {
        assert_eq!(format_canonical_volume_title(1.0), "Volume 01");
        assert_eq!(format_canonical_volume_title(2.0), "Volume 02");
        assert_eq!(format_canonical_volume_title(9.0), "Volume 09");
        assert_eq!(format_canonical_volume_title(10.0), "Volume 10");
        assert_eq!(format_canonical_volume_title(12.0), "Volume 12");
        assert_eq!(format_canonical_volume_title(100.0), "Volume 100");
        assert_eq!(format_canonical_volume_title(1.5), "Volume 01.5");
        assert_eq!(format_canonical_volume_title(10.5), "Volume 10.5");
    }

    #[test]
    fn test_format_canonical_chapter_title() {
        assert_eq!(format_canonical_chapter_title(1.0, None), "Chapter 001");
        assert_eq!(format_canonical_chapter_title(25.0, None), "Chapter 025");
        assert_eq!(format_canonical_chapter_title(99.0, None), "Chapter 099");
        assert_eq!(format_canonical_chapter_title(100.0, None), "Chapter 100");
        assert_eq!(format_canonical_chapter_title(1000.0, None), "Chapter 1000");
        assert_eq!(format_canonical_chapter_title(1.5, None), "Chapter 001.5");
        assert_eq!(
            format_canonical_chapter_title(1.0, Some("The Beginning")),
            "Chapter 001 - The Beginning"
        );
    }

    #[test]
    fn test_normalize_haikyu_volume_titles() {
        // Volume 1 legacy Madokami format
        let r1 = normalize_book_naming("Vol. 01 (Digital)", "Haikyu!!", true, Some(1.0));
        assert_eq!(r1.title, "Volume 01");
        assert_eq!(r1.number_string, "1");
        assert_eq!(r1.number_sort, 1.0);
        assert!(!r1.is_already_canonical);
        assert_eq!(r1.conformity, NamingConformity::InconsistentTitle);

        // Volume 2 legacy Madokami format
        let r2 = normalize_book_naming("Vol. 02 (Digital)", "Haikyu!!", true, Some(2.0));
        assert_eq!(r2.title, "Volume 02");
        assert_eq!(r2.number_string, "2");
        assert_eq!(r2.number_sort, 2.0);

        // Volume 3 unpadded format
        let r3 = normalize_book_naming("Volume 3", "Haikyu!!", true, Some(3.0));
        assert_eq!(r3.title, "Volume 03");
        assert_eq!(r3.number_string, "3");
        assert_eq!(r3.number_sort, 3.0);
        assert!(!r3.is_already_canonical);

        // Volume 3 already normalized
        let r3_norm = normalize_book_naming("Volume 03", "Haikyu!!", true, Some(3.0));
        assert_eq!(r3_norm.title, "Volume 03");
        assert!(r3_norm.is_already_canonical);
        assert_eq!(r3_norm.conformity, NamingConformity::Canonical);
    }

    #[test]
    fn test_normalize_blue_lock_and_chainsaw_man_titles() {
        // Redundant series prefix + unpadded volume
        let bl1 = normalize_book_naming("Blue Lock Volume 1", "Blue Lock", true, Some(1.0));
        assert_eq!(bl1.title, "Volume 01");
        assert_eq!(bl1.number_string, "1");

        let csm5 = normalize_book_naming("Chainsaw Man Volume 5", "Chainsaw Man", true, Some(5.0));
        assert_eq!(csm5.title, "Volume 05");
    }

    #[test]
    fn test_normalize_meaningful_chapter_titles() {
        // Chapter with colon and meaningful story title
        let ch1 = normalize_book_naming("Ch. 1: The Beginning", "Bleach", false, Some(1.0));
        assert_eq!(ch1.title, "Chapter 001 - The Beginning");
        assert_eq!(ch1.subtitle, Some("The Beginning".into()));

        // Chapter with hyphen and scanlator tag
        let ch2 = normalize_book_naming("Chapter 2 - Death & Strawberry [MangaPlus]", "Bleach", false, Some(2.0));
        assert_eq!(ch2.title, "Chapter 002 - Death & Strawberry");
        assert_eq!(ch2.subtitle, Some("Death & Strawberry".into()));

        // Tower of God series prefix
        let tog1 = normalize_book_naming("Tower of God Chapter 1", "Tower of God", false, Some(1.0));
        assert_eq!(tog1.title, "Chapter 001");
        assert_eq!(tog1.subtitle, None);
    }

    #[test]
    fn test_format_canonical_filename() {
        assert_eq!(
            format_canonical_filename("Haikyu!!", true, 3.0, Some(2016), true, Some("danke-Empire")),
            "Haikyu!! - v03 (2016) (Digital) (danke-Empire)"
        );
        assert_eq!(
            format_canonical_filename("Blue Lock", true, 1.0, None, false, None),
            "Blue Lock - v01"
        );
    }
}
