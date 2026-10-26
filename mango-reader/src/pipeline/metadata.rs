//! Canonical MangaBaka metadata normalization policy and local series catalog cache.
//!
//! Provides a unified, deterministic metadata representation used consistently across:
//! - New acquisitions
//! - Administrative CBZ repair operations
//! - Existing-library Komga normalization
//! - Lists/catalog frontend presentation

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Provenance of the synopsis/summary field to distinguish volume-specific
/// descriptions from collection or series fallback descriptions.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryProvenance {
    VolumeSpecific,
    EditionFallback,
    SeriesFallback,
    None,
}

impl std::fmt::Display for SummaryProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VolumeSpecific => write!(f, "volume_specific"),
            Self::EditionFallback => write!(f, "edition_fallback"),
            Self::SeriesFallback => write!(f, "series_fallback"),
            Self::None => write!(f, "none"),
        }
    }
}

/// Normalizes a single genre string from raw/machine-like format to clean title-case human-readable form.
///
/// Examples:
/// - "Award_winning" -> "Award Winning"
/// - "Slice_of_life" -> "Slice of Life"
/// - "Sci_fi" or "Sci-fi" -> "Sci-Fi"
/// - "Post_apocalyptic" -> "Post-Apocalyptic"
pub fn normalize_genre(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase().replace('_', " ").replace('-', " ");
    let canonical = match lower.as_str() {
        "sci fi" | "science fiction" => "Sci-Fi",
        "slice of life" => "Slice of Life",
        "award winning" => "Award Winning",
        "post apocalyptic" => "Post-Apocalyptic",
        "martial arts" => "Martial Arts",
        "super power" | "super powers" => "Super Power",
        "shounen" | "shonen" => "Shounen",
        "seinen" => "Seinen",
        "shoujo" | "shojo" => "Shoujo",
        "josei" => "Josei",
        "boys love" | "bl" => "Boys Love",
        "girls love" | "gl" => "Girls Love",
        _ => {
            // Title-case words separated by spaces or hyphens
            let words: Vec<String> = trimmed
                .replace('_', " ")
                .split_whitespace()
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => {
                            let mut s = first.to_uppercase().collect::<String>();
                            s.push_str(&chars.as_str().to_lowercase());
                            s
                        }
                    }
                })
                .collect();
            return Some(words.join(" "));
        }
    };

    Some(canonical.to_string())
}

/// Normalizes, cleans, and deduplicates genre collections.
pub fn normalize_genres(raw_genres: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen_lower = std::collections::HashSet::new();

    for g in raw_genres {
        if let Some(norm) = normalize_genre(g) {
            let key = norm.to_lowercase();
            if !seen_lower.contains(&key) {
                seen_lower.insert(key);
                normalized.push(norm);
            }
        }
    }

    normalized
}

/// Normalizes tag strings and collapses obvious redundant aliases (e.g. Football -> Soccer).
pub fn normalize_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase().replace('_', " ").replace('-', " ");
    let canonical = match lower.as_str() {
        "football" | "soccer/football" | "association football" => "Soccer",
        "battle royale" => "Battle Royale",
        "time travel" => "Time Travel",
        "martial arts" => "Martial Arts",
        "school life" => "School Life",
        "video games" | "gaming" => "Video Games",
        "survival" => "Survival",
        "mind games" | "psychological warfare" => "Mind Games",
        "body swap" => "Body Swap",
        "award winning" => "Award Winning",
        _ => {
            let words: Vec<String> = trimmed
                .replace('_', " ")
                .split_whitespace()
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => {
                            let mut s = first.to_uppercase().collect::<String>();
                            s.push_str(&chars.as_str().to_lowercase());
                            s
                        }
                    }
                })
                .collect();
            return Some(words.join(" "));
        }
    };

    Some(canonical.to_string())
}

/// Normalizes, deduplicates, and filters tags to avoid redundancy with genres.
pub fn normalize_tags(raw_tags: &[String], genres: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen_lower: std::collections::HashSet<String> =
        genres.iter().map(|g| g.to_lowercase()).collect();

    for t in raw_tags {
        if let Some(norm) = normalize_tag(t) {
            let key = norm.to_lowercase();
            if !seen_lower.contains(&key) {
                seen_lower.insert(key);
                normalized.push(norm);
            }
        }
    }

    normalized
}

/// Selects the most specific publisher according to precedence rules:
/// 1. Exact work/release publisher
/// 2. Exact collection/edition publisher
/// 3. Series-level primary English publisher
///
/// Avoids blindly concatenating every series publisher (e.g. "Coolmic, K MANGA, Kodansha Manga").
#[allow(dead_code)]
pub fn select_publisher(
    work_publisher: Option<&str>,
    edition_publisher: Option<&str>,
    series_publishers: &[String],
) -> Option<String> {
    if let Some(p) = work_publisher.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(p.to_string());
    }
    if let Some(p) = edition_publisher.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(p.to_string());
    }

    // Series fallback: prefer standard English manga publishers if known
    if !series_publishers.is_empty() {
        for preferred in [
            "Kodansha",
            "VIZ",
            "Yen Press",
            "Square Enix",
            "Seven Seas",
            "Dark Horse",
        ] {
            if let Some(found) = series_publishers.iter().find(|p| p.contains(preferred)) {
                return Some(found.trim().to_string());
            }
        }
        return Some(series_publishers[0].trim().to_string());
    }

    None
}

/// Resolves summary with strict provenance tracking.
///
/// Precedence:
/// 1. Work/volume synopsis (VolumeSpecific)
/// 2. Collection/edition synopsis (EditionFallback)
/// 3. Series synopsis (SeriesFallback)
#[allow(dead_code)]
pub fn select_summary(
    work_desc: Option<&str>,
    edition_desc: Option<&str>,
    series_desc: Option<&str>,
) -> (Option<String>, SummaryProvenance) {
    if let Some(s) = work_desc.map(str::trim).filter(|s| !s.is_empty()) {
        return (Some(s.to_string()), SummaryProvenance::VolumeSpecific);
    }
    if let Some(s) = edition_desc.map(str::trim).filter(|s| !s.is_empty()) {
        return (Some(s.to_string()), SummaryProvenance::EditionFallback);
    }
    if let Some(s) = series_desc.map(str::trim).filter(|s| !s.is_empty()) {
        return (Some(s.to_string()), SummaryProvenance::SeriesFallback);
    }

    (None, SummaryProvenance::None)
}

/// Cached MangaBaka series metadata stored in SQLite for fast Lists rendering and dry runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedMangaBakaSeries {
    pub series_id: i64,
    pub title: String,
    pub english_title: Option<String>,
    pub romaji_title: Option<String>,
    pub native_title: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub artists: Vec<String>,
    pub publisher: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub total_volumes: Option<i64>,
    pub status: Option<String>,
    pub web_url: Option<String>,
    pub raw_json: Option<String>,
    pub updated_at: i64,
}

/// Retrieves a cached MangaBaka series by its MangaBaka series ID.
pub fn get_cached_series(
    conn: &Connection,
    series_id: i64,
) -> Result<Option<CachedMangaBakaSeries>> {
    let mut stmt = conn.prepare(
        "SELECT series_id, title, english_title, romaji_title, native_title,
                aliases, cover_url, authors, artists, publisher,
                genres, tags, summary, total_volumes, status, web_url, raw_json, updated_at
         FROM mangabaka_series_cache
         WHERE series_id = ?1",
    )?;

    let res = stmt
        .query_row([series_id], |r| {
            let aliases_json: String = r.get(5)?;
            let authors_json: String = r.get(7)?;
            let artists_json: String = r.get(8)?;
            let genres_json: String = r.get(10)?;
            let tags_json: String = r.get(11)?;

            Ok(CachedMangaBakaSeries {
                series_id: r.get(0)?,
                title: r.get(1)?,
                english_title: r.get(2)?,
                romaji_title: r.get(3)?,
                native_title: r.get(4)?,
                aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
                cover_url: r.get(6)?,
                authors: serde_json::from_str(&authors_json).unwrap_or_default(),
                artists: serde_json::from_str(&artists_json).unwrap_or_default(),
                publisher: r.get(9)?,
                genres: serde_json::from_str(&genres_json).unwrap_or_default(),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                summary: r.get(12)?,
                total_volumes: r.get(13)?,
                status: r.get(14)?,
                web_url: r.get(15)?,
                raw_json: r.get(16)?,
                updated_at: r.get(17)?,
            })
        })
        .optional()?;

    Ok(res)
}

/// Finds a cached MangaBaka series by exact or normalized title match.
pub fn find_cached_series_by_title(
    conn: &Connection,
    title: &str,
) -> Result<Option<CachedMangaBakaSeries>> {
    let norm = crate::pipeline::mangabaka::normalize_title(title);
    let mut stmt = conn.prepare(
        "SELECT series_id, title, english_title, romaji_title, native_title,
                aliases, cover_url, authors, artists, publisher,
                genres, tags, summary, total_volumes, status, web_url, raw_json, updated_at
         FROM mangabaka_series_cache",
    )?;

    let rows: Vec<CachedMangaBakaSeries> = stmt
        .query_map([], |r| {
            let aliases_json: String = r.get(5)?;
            let authors_json: String = r.get(7)?;
            let artists_json: String = r.get(8)?;
            let genres_json: String = r.get(10)?;
            let tags_json: String = r.get(11)?;

            Ok(CachedMangaBakaSeries {
                series_id: r.get(0)?,
                title: r.get(1)?,
                english_title: r.get(2)?,
                romaji_title: r.get(3)?,
                native_title: r.get(4)?,
                aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
                cover_url: r.get(6)?,
                authors: serde_json::from_str(&authors_json).unwrap_or_default(),
                artists: serde_json::from_str(&artists_json).unwrap_or_default(),
                publisher: r.get(9)?,
                genres: serde_json::from_str(&genres_json).unwrap_or_default(),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                summary: r.get(12)?,
                total_volumes: r.get(13)?,
                status: r.get(14)?,
                web_url: r.get(15)?,
                raw_json: r.get(16)?,
                updated_at: r.get(17)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    for s in rows {
        if crate::pipeline::mangabaka::normalize_title(&s.title) == norm
            || s.english_title
                .as_deref()
                .map(crate::pipeline::mangabaka::normalize_title)
                == Some(norm.clone())
            || s.romaji_title
                .as_deref()
                .map(crate::pipeline::mangabaka::normalize_title)
                == Some(norm.clone())
            || s.aliases
                .iter()
                .any(|a| crate::pipeline::mangabaka::normalize_title(a) == norm)
        {
            return Ok(Some(s));
        }
    }

    Ok(None)
}

/// Upserts a cached series into SQLite.
#[allow(dead_code)]
pub fn upsert_cached_series(conn: &Connection, s: &CachedMangaBakaSeries) -> Result<()> {
    let aliases_json = serde_json::to_string(&s.aliases).unwrap_or_else(|_| "[]".to_string());
    let authors_json = serde_json::to_string(&s.authors).unwrap_or_else(|_| "[]".to_string());
    let artists_json = serde_json::to_string(&s.artists).unwrap_or_else(|_| "[]".to_string());
    let genres_json = serde_json::to_string(&s.genres).unwrap_or_else(|_| "[]".to_string());
    let tags_json = serde_json::to_string(&s.tags).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO mangabaka_series_cache (
            series_id, title, english_title, romaji_title, native_title,
            aliases, cover_url, authors, artists, publisher,
            genres, tags, summary, total_volumes, status, web_url, raw_json, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
        ON CONFLICT(series_id) DO UPDATE SET
            title = excluded.title,
            english_title = excluded.english_title,
            romaji_title = excluded.romaji_title,
            native_title = excluded.native_title,
            aliases = excluded.aliases,
            cover_url = excluded.cover_url,
            authors = excluded.authors,
            artists = excluded.artists,
            publisher = excluded.publisher,
            genres = excluded.genres,
            tags = excluded.tags,
            summary = excluded.summary,
            total_volumes = excluded.total_volumes,
            status = excluded.status,
            web_url = excluded.web_url,
            raw_json = excluded.raw_json,
            updated_at = excluded.updated_at",
        params![
            s.series_id,
            s.title,
            s.english_title,
            s.romaji_title,
            s.native_title,
            aliases_json,
            s.cover_url,
            authors_json,
            artists_json,
            s.publisher,
            genres_json,
            tags_json,
            s.summary,
            s.total_volumes,
            s.status,
            s.web_url,
            s.raw_json,
            s.updated_at,
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genre_normalization() {
        assert_eq!(
            normalize_genre("Award_winning"),
            Some("Award Winning".into())
        );
        assert_eq!(
            normalize_genre("Slice_of_life"),
            Some("Slice of Life".into())
        );
        assert_eq!(normalize_genre("sci-fi"), Some("Sci-Fi".into()));
        assert_eq!(normalize_genre("SCI_FI"), Some("Sci-Fi".into()));
        assert_eq!(normalize_genre("action"), Some("Action".into()));
        assert_eq!(
            normalize_genre("post_apocalyptic"),
            Some("Post-Apocalyptic".into())
        );

        let raw = vec![
            "Award_winning".into(),
            "Sports".into(),
            "sports".into(),
            "Shounen".into(),
            "SHOUNEN".into(),
        ];
        let normalized = normalize_genres(&raw);
        assert_eq!(normalized, vec!["Award Winning", "Sports", "Shounen"]);
    }

    #[test]
    fn test_tag_normalization_and_redundancy_removal() {
        assert_eq!(normalize_tag("football"), Some("Soccer".into()));
        assert_eq!(normalize_tag("Soccer/Football"), Some("Soccer".into()));
        assert_eq!(normalize_tag("battle_royale"), Some("Battle Royale".into()));
        assert_eq!(normalize_tag("time_travel"), Some("Time Travel".into()));

        let genres = vec!["Sports".into(), "Shounen".into()];
        let raw_tags = vec![
            "Football".into(),
            "Soccer".into(),
            "Sports".into(), // Duplicates genre
            "Battle_Royale".into(),
        ];
        let normalized_tags = normalize_tags(&raw_tags, &genres);
        assert_eq!(normalized_tags, vec!["Soccer", "Battle Royale"]);
    }

    #[test]
    fn test_publisher_precedence() {
        let series_pubs = vec!["Coolmic".into(), "K MANGA".into(), "Kodansha Manga".into()];
        // 1. Work publisher overrides all
        assert_eq!(
            select_publisher(Some("Kodansha USA"), Some("Kodansha Manga"), &series_pubs),
            Some("Kodansha USA".into())
        );
        // 2. Edition publisher overrides series fallback
        assert_eq!(
            select_publisher(None, Some("Kodansha Manga"), &series_pubs),
            Some("Kodansha Manga".into())
        );
        // 3. Series publisher fallback prefers standard English publisher over Coolmic
        assert_eq!(
            select_publisher(None, None, &series_pubs),
            Some("Kodansha Manga".into())
        );
    }

    #[test]
    fn test_summary_provenance() {
        let (s1, p1) = select_summary(
            Some("Vol 39 synopsis"),
            Some("Edition synopsis"),
            Some("Series synopsis"),
        );
        assert_eq!(s1.as_deref(), Some("Vol 39 synopsis"));
        assert_eq!(p1, SummaryProvenance::VolumeSpecific);

        let (s2, p2) = select_summary(None, Some("Edition synopsis"), Some("Series synopsis"));
        assert_eq!(s2.as_deref(), Some("Edition synopsis"));
        assert_eq!(p2, SummaryProvenance::EditionFallback);

        let (s3, p3) = select_summary(None, None, Some("Series synopsis"));
        assert_eq!(s3.as_deref(), Some("Series synopsis"));
        assert_eq!(p3, SummaryProvenance::SeriesFallback);
    }
}

/// Descriptive fallback only. No edition work, dates, page count or standard totals
/// are fabricated. Images/covers are handled by the existing lossless CBZ rewrite.
pub fn acquisition_series_fallback(
    conn: &Connection,
    title: &str,
    volume: f64,
) -> Result<(Option<super::mangabaka::MangaBakaEnrichment>, String)> {
    use super::mangabaka::MangaBakaEnrichment;
    if let Some(series) = find_cached_series_by_title(conn, title)? {
        return Ok((
            Some(MangaBakaEnrichment {
                series_id: series.series_id,
                volume,
                summary: series.summary,
                writers: series.authors,
                pencillers: series.artists,
                genres: normalize_genres(&series.genres),
                tags: normalize_tags(&series.tags, &series.genres),
                web_url: series.web_url,
                ..Default::default()
            }),
            "series_fallback:mangabaka".into(),
        ));
    }
    let mut stmt = conn.prepare("SELECT raw_data FROM tracker_list_entries WHERE tracker = 'anilist' AND (title = ?1 OR english_title = ?1 OR romaji_title = ?1)")?;
    let raw = stmt
        .query_map([title], |r| r.get::<_, Option<String>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if raw.len() == 1 {
        if let Some(value) = raw[0]
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            let media = value
                .get("media")
                .or_else(|| value.get("Media"))
                .unwrap_or(&value);
            let summary = media
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let genres = media
                .get("genres")
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|g| g.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if summary.is_some() || !genres.is_empty() {
                return Ok((
                    Some(MangaBakaEnrichment {
                        volume,
                        summary,
                        genres: normalize_genres(&genres),
                        ..Default::default()
                    }),
                    "series_fallback:anilist".into(),
                ));
            }
        }
    }
    Ok((None, "source_identity:user_override".into()))
}

#[cfg(test)]
mod acquisition_fallback_tests {
    use super::*;
    #[test]
    fn series_metadata_never_implies_edition_facts() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/022_metadata_normalization_and_maintenance.sql"
        ))
        .unwrap();
        let series = CachedMangaBakaSeries {
            series_id: 83,
            title: "Series X".into(),
            english_title: None,
            romaji_title: None,
            native_title: None,
            aliases: vec![],
            cover_url: Some("https://example.test/generic.jpg".into()),
            authors: vec!["Author".into()],
            artists: vec![],
            publisher: Some("General Publisher".into()),
            genres: vec!["Sci_fi".into()],
            tags: vec![],
            summary: Some("General summary".into()),
            total_volumes: Some(12),
            status: None,
            web_url: None,
            raw_json: None,
            updated_at: 0,
        };
        upsert_cached_series(&conn, &series).unwrap();
        let (data, scope) = acquisition_series_fallback(&conn, "Series X", 4.0).unwrap();
        assert_eq!(scope, "series_fallback:mangabaka");
        let data = data.unwrap();
        assert_eq!(data.summary.as_deref(), Some("General summary"));
        assert!(data.work_id.is_none());
        assert!(data.collection_id.is_empty());
        assert!(data.total_volumes.is_none());
        assert!(data.release_date.is_none());
        assert!(data.pages.is_none());
    }
    #[test]
    fn anilist_fallback_is_general_and_source_is_last_resort() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/022_metadata_normalization_and_maintenance.sql"
        ))
        .unwrap();
        conn.execute_batch("CREATE TABLE tracker_list_entries (tracker TEXT, title TEXT, english_title TEXT, romaji_title TEXT, raw_data TEXT);").unwrap();
        conn.execute(
            "INSERT INTO tracker_list_entries VALUES ('anilist','Series X',NULL,NULL,?1)",
            [r#"{"media":{"description":"General AniList description","genres":["Action"]}}"#],
        )
        .unwrap();
        let (data, scope) = acquisition_series_fallback(&conn, "Series X", 4.0).unwrap();
        assert_eq!(scope, "series_fallback:anilist");
        assert_eq!(data.unwrap().series_id, 0);
        assert_eq!(
            acquisition_series_fallback(&conn, "No metadata", 1.0)
                .unwrap()
                .1,
            "source_identity:user_override"
        );
    }
}
