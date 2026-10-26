//! Native asynchronous client for MangaBaka API enrichment.
//!
//! Enriches manga metadata with verified publication dates, volume summaries,
//! official English publishers, writers, pencillers, and genre tags.

use crate::error::{AppError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub const DEFAULT_MANGABAKA_API: &str = "https://api.mangabaka.org/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "details")]
pub enum MangaBakaSeriesMatch {
    Found { id: i64, title: String },
    Ambiguous(Vec<String>),
    NotFound,
    ApiError(String),
    RateLimited,
    AuthError(String),
    NetworkError(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "details")]
pub enum MangaBakaCollectionMatch {
    Found {
        id: String,
        edition: String,
        total_works: Option<u32>,
    },
    Ambiguous(Vec<String>),
    NotFound,
    AuthError(String),
    NetworkError(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MangaBakaCrossReference {
    pub mangabaka_id: i64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaBakaLibraryEntry {
    pub id: Option<i64>,
    pub series_id: Option<i64>,
    pub progress_volume: Option<f64>,
    pub progress_chapter: Option<f64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaBakaLibraryItem {
    pub series_id: i64,
    pub title: String,
    pub english_title: Option<String>,
    pub romaji_title: Option<String>,
    pub native_title: Option<String>,
    pub aliases: Vec<String>,
    pub cover_url: Option<String>,
    pub state: String, // "reading", "plan_to_read", "considering", "completed", "paused", "dropped", "rereading"
    pub progress_chapter: Option<f64>,
    pub progress_volume: Option<f64>,
    pub total_chapters: Option<i64>,
    pub total_volumes: Option<i64>,
    pub format: Option<String>,
    pub series_status: Option<String>,
    pub updated_at: i64,
}

pub fn is_safe_cover_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Ok(parsed) = reqwest::Url::parse(trimmed) {
        matches!(parsed.scheme(), "http" | "https")
    } else {
        false
    }
}

pub fn is_latin_script(s: &str) -> bool {
    let mut latin_count = 0;
    let mut non_latin_script_count = 0;

    for c in s.chars() {
        if (c >= 'a' && c <= 'z')
            || (c >= 'A' && c <= 'Z')
            || ('\u{00C0}'..='\u{024F}').contains(&c)
        {
            latin_count += 1;
        } else if is_non_latin_script_char(c) {
            non_latin_script_count += 1;
        }
    }

    latin_count > 0 && latin_count >= non_latin_script_count
}

fn is_non_latin_script_char(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' |
        '\u{AC00}'..='\u{D7AF}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}' |
        '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' |
        '\u{0400}'..='\u{04FF}' |
        '\u{0600}'..='\u{06FF}' |
        '\u{0590}'..='\u{05FF}' |
        '\u{0E00}'..='\u{0E7F}' |
        '\u{0900}'..='\u{097F}'
    )
}

pub fn extract_mb_series_title_info(
    series_node: &serde_json::Value,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<String>,
) {
    let mut english_title: Option<String> = None;
    let mut romaji_title: Option<String> = None;
    let mut native_title: Option<String> = None;
    let mut aliases: Vec<String> = Vec::new();

    let top_title = series_node
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref t) = top_title {
        aliases.push(t.clone());
    }

    if let Some(titles) = series_node.get("titles").and_then(|v| v.as_array()) {
        for item in titles {
            let lang = item.get("language").and_then(|v| v.as_str()).unwrap_or("");
            let traits = item.get("traits").and_then(|v| v.as_array());
            let is_official = traits.map_or(false, |tr| {
                tr.iter().any(|t| t.as_str() == Some("official"))
            });
            let is_native =
                traits.map_or(false, |tr| tr.iter().any(|t| t.as_str() == Some("native")));
            let is_primary = item
                .get("is_primary")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let t = item
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let Some(title_str) = t else {
                continue;
            };

            if !aliases.contains(&title_str) {
                aliases.push(title_str.clone());
            }

            if lang == "en" || lang == "english" {
                if english_title.is_none() || is_official || is_primary {
                    english_title = Some(title_str.clone());
                }
            } else if lang == "ja-Latn" || lang == "ko-Latn" || lang.ends_with("-Latn") {
                if romaji_title.is_none() || is_primary {
                    romaji_title = Some(title_str.clone());
                }
            } else if is_native {
                if native_title.is_none() || is_primary {
                    native_title = Some(title_str.clone());
                }
            }
        }
    }

    let series_id = series_node.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let display_title = english_title
        .as_deref()
        .or_else(|| romaji_title.as_deref().filter(|s| is_latin_script(s)))
        .or_else(|| top_title.as_deref().filter(|s| is_latin_script(s)))
        .or_else(|| {
            aliases
                .iter()
                .find(|s| is_latin_script(s))
                .map(|s| s.as_str())
        })
        .or_else(|| top_title.as_deref())
        .or_else(|| aliases.first().map(|s| s.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("MangaBaka Series {series_id}"));

    (
        display_title,
        english_title,
        romaji_title,
        native_title,
        aliases,
    )
}

#[allow(dead_code)]
pub fn extract_mb_series_title(series_node: &serde_json::Value) -> String {
    let (display, _, _, _, _) = extract_mb_series_title_info(series_node);
    display
}

fn extract_url_from_image_size(node: &serde_json::Value) -> Option<String> {
    if let Some(s) = node.as_str() {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(obj) = node.as_object() {
        for dpr in ["x2", "x1", "x3", "url"] {
            if let Some(s) = obj.get(dpr).and_then(|v| v.as_str()) {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

pub fn extract_mb_series_cover(series_node: &serde_json::Value) -> Option<String> {
    if let Some(cover) = series_node.get("cover") {
        if let Some(s) = cover.as_str() {
            let t = s.trim();
            if !t.is_empty() && is_safe_cover_url(t) {
                return Some(t.to_string());
            }
        }
        if let Some(obj) = cover.as_object() {
            for key in ["x350", "x250", "raw", "x150", "x500"] {
                if let Some(val) = obj.get(key) {
                    if let Some(url) = extract_url_from_image_size(val) {
                        if is_safe_cover_url(&url) {
                            return Some(url);
                        }
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MangaBakaEnrichment {
    pub series_id: i64,
    pub series_title: Option<String>,
    pub work_title: Option<String>,
    pub collection_id: String,
    pub volume: f64,
    pub release_date: Option<String>,
    pub summary: Option<String>,
    pub pages: Option<i64>,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub publisher: Option<String>,
    pub total_volumes: Option<u32>,
    pub web_url: Option<String>,
    pub language: Option<String>,
    pub work_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "details")]
pub enum MangaBakaOutcome {
    Success(MangaBakaEnrichment),
    NotConfigured,
    AuthError(String),
    NetworkError(String),
    SeriesNotFound(String),
    AmbiguousSeries(usize),
    CollectionNotFound(String),
    AmbiguousCollection(usize),
    VolumeNotFound(f64),
}

impl MangaBakaOutcome {
    pub fn description(&self) -> String {
        match self {
            Self::Success(e) => format!("Enriched volume {} (series {})", e.volume, e.series_id),
            Self::NotConfigured => "MangaBaka integration disabled or not configured".to_string(),
            Self::AuthError(msg) => format!("Authentication failed: {msg}"),
            Self::NetworkError(msg) => format!("Network error: {msg}"),
            Self::SeriesNotFound(series) => format!("No exact title match for '{series}'"),
            Self::AmbiguousSeries(n) => format!("Found {n} candidate series; match ambiguous"),
            Self::CollectionNotFound(reason) => format!("Collection not found: {reason}"),
            Self::AmbiguousCollection(n) => {
                format!("Found {n} candidate collections; match ambiguous")
            }
            Self::VolumeNotFound(v) => format!("Volume {v} not found in collection"),
        }
    }

    #[allow(dead_code)]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
}

#[derive(Clone)]
pub struct MangaBakaClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl MangaBakaClient {
    pub fn new(http: Client, base_url: Option<String>, token: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url.unwrap_or_else(|| DEFAULT_MANGABAKA_API.to_string()),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    fn resolve_url(&self, endpoint: &str) -> String {
        if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            let base = self.base_url.trim_end_matches('/');
            let clean = endpoint.trim_start_matches('/');
            if clean.starts_with("v2/") && base.ends_with("/v1") {
                let root = &base[..base.len() - 3];
                format!("{root}/{clean}")
            } else if base.ends_with("/v1") && clean.starts_with("v1/") {
                format!("{base}/{}", &clean[3..])
            } else {
                format!("{base}/{clean}")
            }
        }
    }

    /// Builds a public request with NO authentication headers.
    ///
    /// Per the official MangaBaka OpenAPI specification, catalog endpoints
    /// (`/series/search`, `/series/{id}`, `/collections/{id}/works`, `/source/anilist/{id}`)
    /// are public (`security: none`) and must not carry user authentication tokens.
    fn public_request(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = self.resolve_url(endpoint);
        self.http
            .request(method, url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mango/1.0")
    }

    /// Builds an authenticated request using the user's Personal Access Token (PAT).
    ///
    /// Per the official MangaBaka OpenAPI specification, authenticated user endpoints
    /// (`/my/profile`, `/my/library`, `/my/library/{id}`) require user credentials.
    /// In accordance with the OpenAPI `components.securitySchemes.x-api-key`,
    /// authentication is provided strictly via the `x-api-key: <token>` header.
    /// Never sends `Authorization` to prevent mutual exclusivity errors.
    fn authenticated_request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let token = self.token.as_ref().ok_or_else(|| {
            AppError::new(
                401,
                "mangabaka_unauthorized",
                "MangaBaka API token is not configured",
            )
        })?;

        let url = self.resolve_url(endpoint);
        Ok(self
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mango/1.0")
            .header("x-api-key", token))
    }

    /// Resolves MangaBaka series ID and title from an AniList media ID.
    ///
    /// Calls `GET /v1/source/anilist/{anilist_id}`.
    /// Returns `Ok(None)` if MangaBaka returns 404 (no cross-reference).
    pub async fn resolve_from_anilist(
        &self,
        anilist_id: i64,
    ) -> Result<Option<MangaBakaCrossReference>> {
        let endpoint = format!("/source/anilist/{anilist_id}");
        let resp = match self
            .public_request(reqwest::Method::GET, &endpoint)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Err(AppError::new(
                    502,
                    "mangabaka_network_error",
                    format!("Failed to query MangaBaka AniList cross-reference: {e}"),
                ));
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AppError::new(
                401,
                "mangabaka_unauthorized",
                "MangaBaka API token invalid or rejected",
            ));
        }

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::new(
                429,
                "mangabaka_rate_limited",
                "MangaBaka rate limited (HTTP 429); retry later",
            ));
        }

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                400,
                "mangabaka_bad_request",
                format!("MangaBaka cross-reference request rejected (HTTP 400): {body}"),
            ));
        }

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "mangabaka_error",
                format!(
                    "MangaBaka returned HTTP {} for AniList cross-reference",
                    resp.status()
                ),
            ));
        }

        let raw: serde_json::Value = resp.json().await.map_err(|e| {
            AppError::internal(format!(
                "Failed to parse MangaBaka cross-reference JSON: {e}"
            ))
        })?;

        // 1. Official MangaBaka response schema per OpenAPI spec:
        // { "status": 200, "data": { "series": [ { "id": 83, "state": "active", "title": "Blue Lock", ... } ] } }
        let series_array = raw
            .get("data")
            .and_then(|d| d.get("series"))
            .and_then(|s| s.as_array())
            .or_else(|| raw.get("series").and_then(|s| s.as_array()));

        if let Some(series_list) = series_array {
            if series_list.is_empty() {
                return Ok(None);
            }

            // Filter for active series if state property exists
            let active_series: Vec<&serde_json::Value> = series_list
                .iter()
                .filter(|s| {
                    s.get("state")
                        .and_then(|st| st.as_str())
                        .map(|st| st == "active")
                        .unwrap_or(true)
                })
                .collect();

            let target_series = if active_series.len() == 1 {
                active_series[0]
            } else if active_series.is_empty() && series_list.len() == 1 {
                &series_list[0]
            } else if active_series.is_empty() {
                &series_list[0]
            } else {
                return Err(AppError::new(
                    409,
                    "mangabaka_ambiguous",
                    format!(
                        "AniList cross-reference returned multiple active MangaBaka series (count: {})",
                        active_series.len()
                    ),
                ));
            };

            let id_val = target_series
                .get("id")
                .or_else(|| target_series.get("series_id"))
                .or_else(|| target_series.get("mangabaka_id"));

            let mb_id = match id_val {
                Some(v) => {
                    if let Some(n) = v.as_i64() {
                        n
                    } else if let Some(s) = v.as_str() {
                        s.parse::<i64>().map_err(|_| {
                            AppError::internal(format!(
                                "Invalid MangaBaka ID in cross-reference series item: {s}"
                            ))
                        })?
                    } else {
                        return Ok(None);
                    }
                }
                None => return Ok(None),
            };

            let title = target_series
                .get("title")
                .or_else(|| target_series.get("romanized_title"))
                .or_else(|| target_series.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            return Ok(Some(MangaBakaCrossReference {
                mangabaka_id: mb_id,
                title,
            }));
        }

        // 2. Fallback for flat / legacy mock format: { "data": { "id": 202, ... } } or { "id": 202, ... }
        let data_node = raw.get("data").unwrap_or(&raw);
        let id_val = data_node
            .get("id")
            .or_else(|| data_node.get("series_id"))
            .or_else(|| data_node.get("mangabaka_id"))
            .or_else(|| raw.get("id"))
            .or_else(|| raw.get("series_id"))
            .or_else(|| raw.get("mangabaka_id"));

        let mb_id = match id_val {
            Some(v) => {
                if let Some(n) = v.as_i64() {
                    n
                } else if let Some(s) = v.as_str() {
                    s.parse::<i64>().map_err(|_| {
                        AppError::internal(format!("Invalid MangaBaka ID in cross-reference: {s}"))
                    })?
                } else {
                    return Ok(None);
                }
            }
            None => return Ok(None),
        };

        let title = data_node
            .get("title")
            .or_else(|| data_node.get("name"))
            .or_else(|| raw.get("title"))
            .or_else(|| raw.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Some(MangaBakaCrossReference {
            mangabaka_id: mb_id,
            title,
        }))
    }

    /// Fetches the authenticated user's library entry for a specific MangaBaka series ID.
    ///
    /// Calls `GET /v1/my/library/{mangabaka_id}`.
    /// Returns `Ok(None)` if MangaBaka returns 404 (series is not in the user's library).
    pub async fn get_my_library_entry(
        &self,
        mangabaka_id: i64,
    ) -> Result<Option<MangaBakaLibraryEntry>> {
        let endpoint = format!("/my/library/{mangabaka_id}");
        let resp = match self
            .authenticated_request(reqwest::Method::GET, &endpoint)?
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Err(AppError::new(
                    502,
                    "mangabaka_network_error",
                    format!("Failed to reach MangaBaka library endpoint: {e}"),
                ));
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AppError::new(
                401,
                "mangabaka_unauthorized",
                "MangaBaka API token invalid or rejected",
            ));
        }

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::new(
                429,
                "mangabaka_rate_limited",
                "MangaBaka rate limited (HTTP 429); retry later",
            ));
        }

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                400,
                "mangabaka_bad_request",
                format!("MangaBaka library request rejected (HTTP 400): {body}"),
            ));
        }

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "mangabaka_error",
                format!(
                    "MangaBaka returned HTTP {} for library entry",
                    resp.status()
                ),
            ));
        }

        let raw: serde_json::Value = resp.json().await.map_err(|e| {
            AppError::internal(format!("Failed to parse MangaBaka library JSON: {e}"))
        })?;

        let data_node = raw.get("data").unwrap_or(&raw);

        let parse_f64_opt = |val: Option<&serde_json::Value>| -> Option<f64> {
            val.and_then(|v| {
                if let Some(f) = v.as_f64() {
                    Some(f)
                } else if let Some(i) = v.as_i64() {
                    Some(i as f64)
                } else if let Some(s) = v.as_str() {
                    s.parse::<f64>().ok()
                } else {
                    None
                }
            })
        };

        let progress_volume = parse_f64_opt(
            data_node
                .get("progress_volume")
                .or_else(|| raw.get("progress_volume")),
        );
        let progress_chapter = parse_f64_opt(
            data_node
                .get("progress_chapter")
                .or_else(|| raw.get("progress_chapter")),
        );
        let status = data_node
            .get("state")
            .or_else(|| data_node.get("status"))
            .or_else(|| raw.get("state"))
            .or_else(|| raw.get("status"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let id = data_node
            .get("id")
            .or_else(|| raw.get("id"))
            .and_then(|v| v.as_i64());

        let series_id = data_node
            .get("series_id")
            .or_else(|| raw.get("series_id"))
            .and_then(|v| v.as_i64());

        Ok(Some(MangaBakaLibraryEntry {
            id,
            series_id,
            progress_volume,
            progress_chapter,
            status,
        }))
    }

    /// Mutates the authenticated user's library progress and/or status on MangaBaka.
    ///
    /// Calls `PATCH /v1/my/library/{mangabaka_id}` with payload:
    /// `{"progress_volume": completed, "progress_chapter": completed, "state": state}`.
    pub async fn update_my_library_entry(
        &self,
        mangabaka_id: i64,
        progress_volume: Option<f64>,
        progress_chapter: Option<f64>,
        state: Option<&str>,
    ) -> Result<()> {
        let endpoint = format!("/my/library/{mangabaka_id}");
        let mut body = serde_json::Map::new();
        if let Some(vol) = progress_volume {
            body.insert("progress_volume".to_string(), serde_json::json!(vol));
        }
        if let Some(ch) = progress_chapter {
            body.insert("progress_chapter".to_string(), serde_json::json!(ch));
        }
        if let Some(st) = state {
            body.insert("state".to_string(), serde_json::json!(st));
        }

        let resp = match self
            .authenticated_request(reqwest::Method::PATCH, &endpoint)?
            .json(&serde_json::Value::Object(body))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Err(AppError::new(
                    502,
                    "mangabaka_network_error",
                    format!("Failed to send update to MangaBaka: {e}"),
                ));
            }
        };

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AppError::new(
                401,
                "mangabaka_unauthorized",
                "MangaBaka API token invalid or rejected",
            ));
        }

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::new(
                429,
                "mangabaka_rate_limited",
                "MangaBaka rate limited (HTTP 429); retry later",
            ));
        }

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                400,
                "mangabaka_bad_request",
                format!("MangaBaka library request rejected (HTTP 400): {body}"),
            ));
        }

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "mangabaka_error",
                format!("MangaBaka returned HTTP {}", resp.status()),
            ));
        }

        Ok(())
    }

    /// Mutates the authenticated user's library progress on MangaBaka.
    ///
    /// Calls `PATCH /v1/my/library/{mangabaka_id}` with dimension-specific payload.
    pub async fn update_my_library_progress(
        &self,
        mangabaka_id: i64,
        progress_volume: Option<f64>,
        progress_chapter: Option<f64>,
    ) -> Result<()> {
        self.update_my_library_entry(mangabaka_id, progress_volume, progress_chapter, None)
            .await
    }

    /// Fetches the authenticated user's complete personal library from MangaBaka.
    ///
    /// Calls `GET /v2/my/library` with pagination, using `x-api-key: <PAT>` only.
    pub async fn fetch_my_library(&self) -> Result<Vec<MangaBakaLibraryItem>> {
        let mut items = Vec::new();
        let mut page = 1;
        let limit = 50;

        loop {
            let endpoint = format!("/v2/my/library?page={page}&limit={limit}&schema=full");
            let resp = match self
                .authenticated_request(reqwest::Method::GET, &endpoint)?
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    return Err(AppError::new(
                        502,
                        "mangabaka_network_error",
                        format!("Failed to reach MangaBaka library endpoint: {e}"),
                    ));
                }
            };

            if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || resp.status() == reqwest::StatusCode::FORBIDDEN
            {
                return Err(AppError::new(
                    401,
                    "mangabaka_unauthorized",
                    "MangaBaka API token invalid or rejected",
                ));
            }

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(AppError::new(
                    429,
                    "mangabaka_rate_limited",
                    "MangaBaka rate limited (HTTP 429); retry later",
                ));
            }

            if !resp.status().is_success() {
                return Err(AppError::new(
                    resp.status().as_u16(),
                    "mangabaka_error",
                    format!(
                        "MangaBaka returned HTTP {} for personal library",
                        resp.status()
                    ),
                ));
            }

            let raw: serde_json::Value = resp.json().await.map_err(|e| {
                AppError::internal(format!(
                    "Failed to parse MangaBaka personal library JSON: {e}"
                ))
            })?;

            let data_array = raw
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            if data_array.is_empty() {
                break;
            }

            for node in &data_array {
                let entry_node = node.get("entry").unwrap_or(node);
                let series_node = node.get("series");

                let state = entry_node
                    .get("state")
                    .or_else(|| entry_node.get("status"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("reading")
                    .to_string();

                let progress_chapter = entry_node
                    .get("progress_chapter")
                    .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)));
                let progress_volume = entry_node
                    .get("progress_volume")
                    .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)));

                let series_id = series_node
                    .and_then(|s| s.get("id"))
                    .and_then(|v| v.as_i64())
                    .or_else(|| entry_node.get("series_id").and_then(|v| v.as_i64()))
                    .unwrap_or(0);

                if series_id == 0 {
                    continue;
                }

                let (title, english_title, romaji_title, native_title, aliases) = series_node
                    .map(extract_mb_series_title_info)
                    .unwrap_or_else(|| {
                        (
                            format!("MangaBaka Series {series_id}"),
                            None,
                            None,
                            None,
                            Vec::new(),
                        )
                    });

                let cover_url = series_node.and_then(extract_mb_series_cover);

                let total_chapters = series_node
                    .and_then(|s| s.get("total_chapters"))
                    .and_then(|v| v.as_i64());
                let total_volumes = series_node
                    .and_then(|s| s.get("final_volume"))
                    .and_then(|v| v.as_i64());
                let format = series_node
                    .and_then(|s| s.get("type"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let series_status = series_node
                    .and_then(|s| s.get("status"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                items.push(MangaBakaLibraryItem {
                    series_id,
                    title,
                    english_title,
                    romaji_title,
                    native_title,
                    aliases,
                    cover_url,
                    state,
                    progress_chapter,
                    progress_volume,
                    total_chapters,
                    total_volumes,
                    format,
                    series_status,
                    updated_at: crate::db::now(),
                });
            }

            // Pagination check
            let has_next = raw
                .get("pagination")
                .and_then(|p| p.get("next"))
                .is_some_and(|n| !n.is_null());

            if !has_next || data_array.len() < limit {
                break;
            }

            page += 1;
            if page > 100 {
                break;
            }
        }

        Ok(items)
    }

    /// Tests connection and validates credentials against MangaBaka using `GET /v1/my/profile`.
    ///
    /// Per the official MangaBaka API specification, `GET /v1/my/profile` is a stable, authenticated
    /// user endpoint that verifies PAT validity. It is read-only and performs no tracker mutation.
    pub async fn test_connection(&self) -> Result<String> {
        let req = self.authenticated_request(reqwest::Method::GET, "/my/profile")?;
        let resp = req.send().await.map_err(|e| {
            AppError::new(
                502,
                "mangabaka_offline",
                format!("Cannot reach MangaBaka: {e}"),
            )
        })?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AppError::new(
                401,
                "mangabaka_unauthorized",
                "MangaBaka API token invalid or rejected",
            ));
        }

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::new(
                429,
                "mangabaka_rate_limited",
                "MangaBaka rate limited (HTTP 429); retry later",
            ));
        }

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::new(
                404,
                "mangabaka_not_found",
                "MangaBaka user profile endpoint not found (HTTP 404)",
            ));
        }

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let body = resp.text().await.unwrap_or_default();
            let msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                json.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&body)
                    .to_string()
            } else if !body.trim().is_empty() {
                body.trim().to_string()
            } else {
                "Bad Request (HTTP 400)".to_string()
            };
            return Err(AppError::new(
                400,
                "mangabaka_bad_request",
                format!("MangaBaka request rejected (HTTP 400): {msg}"),
            ));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "mangabaka_error",
                format!("MangaBaka answered HTTP {status}: {body}"),
            ));
        }

        let body_text = resp.text().await.unwrap_or_default();
        let username = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| {
                v.get("data").and_then(|d| {
                    d.get("preferred_username")
                        .or_else(|| d.get("nickname"))
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string())
                })
            });

        if let Some(user) = username {
            Ok(format!(
                "MangaBaka API connection and authentication successful (authenticated as {user})"
            ))
        } else {
            Ok("MangaBaka API connection and authentication successful".to_string())
        }
    }

    /// Resolves an exact series match on MangaBaka, failing closed if ambiguous or missing.
    pub async fn match_series(&self, query: &str) -> Result<MangaBakaSeriesMatch> {
        let trimmed_series = query.trim();
        if trimmed_series.is_empty() {
            return Ok(MangaBakaSeriesMatch::NotFound);
        }

        let wanted_norm = normalize_title(trimmed_series);

        let resp = match self
            .public_request(reqwest::Method::GET, "/series/search")
            .query(&[("q", trimmed_series)])
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r)
                if r.status() == reqwest::StatusCode::UNAUTHORIZED
                    || r.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                return Ok(MangaBakaSeriesMatch::AuthError(format!(
                    "HTTP {}",
                    r.status()
                )));
            }
            Ok(r) if r.status() == reqwest::StatusCode::BAD_REQUEST => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                let sanitized_msg = if body.trim().is_empty() {
                    format!("HTTP {status} Bad Request")
                } else {
                    format!("HTTP {status} Bad Request: {body}")
                };
                tracing::warn!("MANGABAKA: search returned HTTP 400: {sanitized_msg}");
                return Ok(MangaBakaSeriesMatch::ApiError(sanitized_msg));
            }
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                tracing::warn!("MANGABAKA: rate limited (HTTP 429)");
                return Ok(MangaBakaSeriesMatch::RateLimited);
            }
            Ok(r) => {
                tracing::warn!("MANGABAKA: search returned HTTP {}", r.status());
                return Ok(MangaBakaSeriesMatch::NetworkError(format!(
                    "HTTP {}",
                    r.status()
                )));
            }
            Err(e) => {
                tracing::warn!("MANGABAKA: search failed: {e}");
                return Ok(MangaBakaSeriesMatch::NetworkError(e.to_string()));
            }
        };

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        let candidates = body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut exact_matches = Vec::new();
        for item in &candidates {
            let id = match item.get("id").and_then(|v| v.as_i64()) {
                Some(id) => id,
                None => continue,
            };

            let primary_title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut titles = Vec::new();
            for key in [
                "title",
                "title_english",
                "title_romaji",
                "title_native",
                "romanized_title",
                "native_title",
            ] {
                if let Some(t) = item.get(key).and_then(|v| v.as_str()) {
                    titles.push(t);
                }
            }
            if let Some(arr) = item.get("titles").and_then(|v| v.as_array()) {
                for t in arr {
                    if let Some(s) = t.as_str() {
                        titles.push(s);
                    } else if let Some(s) = t.get("title").and_then(|v| v.as_str()) {
                        titles.push(s);
                    }
                }
            }

            if titles.iter().any(|t| normalize_title(t) == wanted_norm) {
                exact_matches.push((
                    id,
                    if primary_title.is_empty() {
                        titles.first().copied().unwrap_or("").to_string()
                    } else {
                        primary_title
                    },
                ));
            }
        }

        if exact_matches.is_empty() {
            return Ok(MangaBakaSeriesMatch::NotFound);
        }

        if exact_matches.len() > 1 {
            let candidate_names: Vec<String> = exact_matches.into_iter().map(|(_, t)| t).collect();
            return Ok(MangaBakaSeriesMatch::Ambiguous(candidate_names));
        }

        let (id, title) = exact_matches.remove(0);
        Ok(MangaBakaSeriesMatch::Found { id, title })
    }

    /// Resolves an exact English standard digital collection for a series on MangaBaka.
    pub async fn match_collection(&self, series_id: i64) -> Result<MangaBakaCollectionMatch> {
        self.match_edition_collection(series_id, None).await
    }

    pub async fn match_edition_collection(
        &self,
        series_id: i64,
        requested_edition: Option<&str>,
    ) -> Result<MangaBakaCollectionMatch> {
        let requested = requested_edition
            .map(crate::acquisition::edition::normalize_edition)
            .unwrap_or_else(|| "standard".into());
        let collections_url = format!("/series/{series_id}/collections");
        let c_resp = match self
            .public_request(reqwest::Method::GET, &collections_url)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r)
                if r.status() == reqwest::StatusCode::UNAUTHORIZED
                    || r.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                return Ok(MangaBakaCollectionMatch::AuthError(format!(
                    "HTTP {}",
                    r.status()
                )));
            }
            Ok(r) => {
                return Ok(MangaBakaCollectionMatch::NetworkError(format!(
                    "HTTP {}",
                    r.status()
                )));
            }
            Err(e) => {
                return Ok(MangaBakaCollectionMatch::NetworkError(e.to_string()));
            }
        };

        let c_body: serde_json::Value = match c_resp.json().await {
            Ok(v) => v,
            Err(e) => return Ok(MangaBakaCollectionMatch::NetworkError(e.to_string())),
        };

        let collections = c_body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let mut collection_matches = Vec::new();
        for col in &collections {
            let language = col
                .get("language")
                .and_then(|l| {
                    l.get("iso")
                        .or_else(|| l.get("code"))
                        .or_else(|| l.get("language"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| col.get("language").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_lowercase();

            let edition = col
                .get("edition")
                .and_then(|e| {
                    e.get("name")
                        .or_else(|| e.get("title"))
                        .and_then(|v| v.as_str())
                        .or_else(|| e.as_str())
                })
                .or_else(|| col.get("edition").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_lowercase();

            let medium = col
                .get("medium")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_lowercase();
            let total_works = col
                .get("total_works")
                .or_else(|| col.get("works_count"))
                .or_else(|| col.get("count"))
                .and_then(|v| v.as_u64())
                .map(|u| u as u32);

            let is_en = language == "en" || language.starts_with("en");
            let is_standard = crate::acquisition::edition::normalize_edition(&edition) == requested
                || (edition.is_empty() && requested == "standard");
            let is_digital = medium == "digital";

            if is_en && is_standard && is_digital {
                if let Some(col_id) = col.get("id").and_then(|v| v.as_str()) {
                    collection_matches.push((col_id.to_string(), edition, total_works));
                }
            }
        }

        // Fallback: if no explicit digital edition matched, accept standard edition if not marked print
        if collection_matches.is_empty() {
            for col in &collections {
                let language = col
                    .get("language")
                    .and_then(|l| {
                        l.get("iso")
                            .or_else(|| l.get("code"))
                            .or_else(|| l.get("language"))
                            .and_then(|v| v.as_str())
                    })
                    .or_else(|| col.get("language").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_lowercase();

                let edition = col
                    .get("edition")
                    .and_then(|e| {
                        e.get("name")
                            .or_else(|| e.get("title"))
                            .and_then(|v| v.as_str())
                            .or_else(|| e.as_str())
                    })
                    .or_else(|| col.get("edition").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_lowercase();

                let medium = col
                    .get("medium")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let total_works = col
                    .get("total_works")
                    .or_else(|| col.get("works_count"))
                    .or_else(|| col.get("count"))
                    .and_then(|v| v.as_u64())
                    .map(|u| u as u32);

                if (language == "en" || language.starts_with("en"))
                    && (crate::acquisition::edition::normalize_edition(&edition) == requested
                        || (edition.is_empty() && requested == "standard"))
                    && medium != "print"
                {
                    if let Some(col_id) = col.get("id").and_then(|v| v.as_str()) {
                        collection_matches.push((col_id.to_string(), edition, total_works));
                    }
                }
            }
        }

        if collection_matches.is_empty() {
            return Ok(MangaBakaCollectionMatch::NotFound);
        }

        if collection_matches.len() > 1 {
            let candidates = collection_matches
                .into_iter()
                .map(|(id, ed, _)| format!("{id} ({ed})"))
                .collect();
            return Ok(MangaBakaCollectionMatch::Ambiguous(candidates));
        }

        let (id, edition, total_works) = collection_matches.remove(0);
        Ok(MangaBakaCollectionMatch::Found {
            id,
            edition,
            total_works,
        })
    }

    /// Fetches the authoritative total works/volumes count for a MangaBaka collection.
    pub async fn fetch_collection_total_works(&self, collection_id: &str) -> Result<Option<u32>> {
        let works_url = format!("/collections/{collection_id}/works?page=1&per_page=1");
        let resp = match self
            .public_request(reqwest::Method::GET, &works_url)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(None),
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        if let Some(total) = body
            .get("pagination")
            .and_then(|p| p.get("total"))
            .and_then(|t| t.as_u64())
        {
            return Ok(Some(total as u32));
        }
        if let Some(total) = body
            .get("meta")
            .and_then(|m| m.get("total"))
            .and_then(|t| t.as_u64())
        {
            return Ok(Some(total as u32));
        }
        if let Some(total) = body.get("total").and_then(|t| t.as_u64()) {
            return Ok(Some(total as u32));
        }
        if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
            if !arr.is_empty() && body.get("pagination").is_none() {
                return Ok(Some(arr.len() as u32));
            }
        }
        Ok(None)
    }

    /// Authoritatively resolves volume metadata from MangaBaka.
    ///
    /// Precedence order for series identity:
    /// 1. Stable MangaBaka series ID (`mangabaka_id`)
    /// 2. AniList media ID (`anilist_id`) via `resolve_from_anilist()`
    /// 3. Exact title match (`match_series()`)
    ///
    /// Distinguishes:
    /// - Success(MangaBakaEnrichment)
    /// - VolumeNotFound(volume) / SeriesNotFound / CollectionNotFound
    /// - AmbiguousSeries / AmbiguousCollection
    /// - AuthError / NetworkError
    pub async fn resolve_volume_metadata(
        &self,
        series: &str,
        anilist_id: Option<i64>,
        mangabaka_id: Option<i64>,
        volume: f64,
    ) -> Result<MangaBakaOutcome> {
        self.resolve_edition_volume_metadata(series, anilist_id, mangabaka_id, volume, None)
            .await
    }

    pub async fn resolve_edition_volume_metadata(
        &self,
        series: &str,
        anilist_id: Option<i64>,
        mangabaka_id: Option<i64>,
        volume: f64,
        requested_edition: Option<&str>,
    ) -> Result<MangaBakaOutcome> {
        let trimmed_series = series.trim();
        if trimmed_series.is_empty() && mangabaka_id.is_none() && anilist_id.is_none() {
            return Ok(MangaBakaOutcome::SeriesNotFound("empty".into()));
        }

        // 1. Resolve series ID with stable ID precedence
        let matched_series_id = if let Some(mb_id) = mangabaka_id {
            mb_id
        } else if let Some(aid) = anilist_id {
            match self.resolve_from_anilist(aid).await {
                Ok(Some(xref)) => xref.mangabaka_id,
                Ok(None) => match self.match_series(trimmed_series).await? {
                    MangaBakaSeriesMatch::Found { id, .. } => id,
                    MangaBakaSeriesMatch::Ambiguous(candidates) => {
                        return Ok(MangaBakaOutcome::AmbiguousSeries(candidates.len()));
                    }
                    MangaBakaSeriesMatch::NotFound => {
                        return Ok(MangaBakaOutcome::SeriesNotFound(series.to_string()));
                    }
                    MangaBakaSeriesMatch::AuthError(e) => {
                        return Ok(MangaBakaOutcome::AuthError(e));
                    }
                    MangaBakaSeriesMatch::ApiError(e) => {
                        return Ok(MangaBakaOutcome::NetworkError(e));
                    }
                    MangaBakaSeriesMatch::RateLimited => {
                        return Ok(MangaBakaOutcome::NetworkError(
                            "Rate limited (HTTP 429)".into(),
                        ));
                    }
                    MangaBakaSeriesMatch::NetworkError(e) => {
                        return Ok(MangaBakaOutcome::NetworkError(e));
                    }
                },
                Err(e) if e.status == 409 || e.code == "mangabaka_ambiguous" => {
                    return Ok(MangaBakaOutcome::AmbiguousSeries(0));
                }
                Err(e)
                    if e.status == 401 || e.status == 403 || e.code == "mangabaka_unauthorized" =>
                {
                    return Ok(MangaBakaOutcome::AuthError(e.message));
                }
                Err(e) if e.status == 429 || e.code == "mangabaka_rate_limited" => {
                    return Ok(MangaBakaOutcome::NetworkError(
                        "Rate limited (HTTP 429)".into(),
                    ));
                }
                Err(e) => return Ok(MangaBakaOutcome::NetworkError(e.message)),
            }
        } else {
            match self.match_series(trimmed_series).await? {
                MangaBakaSeriesMatch::Found { id, .. } => id,
                MangaBakaSeriesMatch::Ambiguous(candidates) => {
                    return Ok(MangaBakaOutcome::AmbiguousSeries(candidates.len()));
                }
                MangaBakaSeriesMatch::NotFound => {
                    return Ok(MangaBakaOutcome::SeriesNotFound(series.to_string()));
                }
                MangaBakaSeriesMatch::AuthError(e) => return Ok(MangaBakaOutcome::AuthError(e)),
                MangaBakaSeriesMatch::ApiError(e) => return Ok(MangaBakaOutcome::NetworkError(e)),
                MangaBakaSeriesMatch::RateLimited => {
                    return Ok(MangaBakaOutcome::NetworkError(
                        "Rate limited (HTTP 429)".into(),
                    ));
                }
                MangaBakaSeriesMatch::NetworkError(e) => {
                    return Ok(MangaBakaOutcome::NetworkError(e));
                }
            }
        };

        // 2. Resolve collection for standard digital edition
        let (matched_collection_id, matched_total_works) = match self
            .match_edition_collection(matched_series_id, requested_edition)
            .await?
        {
            MangaBakaCollectionMatch::Found {
                id, total_works, ..
            } => (id, total_works),
            MangaBakaCollectionMatch::Ambiguous(candidates) => {
                return Ok(MangaBakaOutcome::AmbiguousCollection(candidates.len()));
            }
            MangaBakaCollectionMatch::NotFound => {
                return Ok(MangaBakaOutcome::CollectionNotFound(format!(
                    "No English digital {} edition found",
                    requested_edition.unwrap_or("standard")
                )));
            }
            MangaBakaCollectionMatch::AuthError(e) => return Ok(MangaBakaOutcome::AuthError(e)),
            MangaBakaCollectionMatch::NetworkError(e) => {
                return Ok(MangaBakaOutcome::NetworkError(e));
            }
        };

        // 3. Fetch series metadata (authors, artists, genres, tags, publishers, description)
        let series_url = format!("/series/{matched_series_id}");
        let (series_title, series_desc, writers, pencillers, genres, tags, publisher, total_vols) =
            match self
                .public_request(reqwest::Method::GET, &series_url)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let s_body: serde_json::Value = r.json().await.unwrap_or_default();
                    let s_data = s_body.get("data").unwrap_or(&s_body);

                    let (disp_title, _, _, _, _) = extract_mb_series_title_info(s_data);

                    let desc = s_data
                        .get("description")
                        .and_then(|v| {
                            if let Some(s) = v.as_str() {
                                Some(s.trim().to_string())
                            } else if let Some(d) = v
                                .get("desc")
                                .or_else(|| v.get("description"))
                                .and_then(|s| s.as_str())
                            {
                                Some(d.trim().to_string())
                            } else {
                                None
                            }
                        })
                        .filter(|s| !s.is_empty());

                    let mut writers: Vec<String> = Vec::new();
                    let mut pencillers: Vec<String> = Vec::new();

                    if let Some(arr) = s_data.get("authors").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                let clean = s.trim().to_string();
                                if !clean.is_empty() && !writers.contains(&clean) {
                                    writers.push(clean);
                                }
                            } else if let Some(obj) = item.as_object() {
                                let name = obj
                                    .get("name")
                                    .or_else(|| obj.get("author"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim()
                                    .to_string();
                                let role = obj
                                    .get("role")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim()
                                    .to_lowercase();
                                if !name.is_empty() {
                                    if role.contains("art") && !pencillers.contains(&name) {
                                        pencillers.push(name.clone());
                                    }
                                    if (role.contains("story")
                                        || role.contains("author")
                                        || role.contains("writer")
                                        || role.is_empty())
                                        && !writers.contains(&name)
                                    {
                                        writers.push(name);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(arr) = s_data.get("artists").and_then(|v| v.as_array()) {
                        for item in arr {
                            let name = if let Some(s) = item.as_str() {
                                s.trim().to_string()
                            } else if let Some(obj) = item.as_object() {
                                obj.get("name")
                                    .or_else(|| obj.get("artist"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim()
                                    .to_string()
                            } else {
                                String::new()
                            };
                            if !name.is_empty() && !pencillers.contains(&name) {
                                pencillers.push(name);
                            }
                        }
                    }

                    let genres: Vec<String> = s_data
                        .get("genres")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| {
                                    x.as_str().map(|s| {
                                        let mut chars = s.trim().chars();
                                        match chars.next() {
                                            None => String::new(),
                                            Some(first) => {
                                                first.to_uppercase().collect::<String>()
                                                    + chars.as_str()
                                            }
                                        }
                                    })
                                })
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();

                    let tags: Vec<String> = s_data
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| {
                                    if let Some(s) = x.as_str() {
                                        Some(s.trim().to_string())
                                    } else if let Some(n) = x
                                        .get("name")
                                        .or_else(|| x.get("title"))
                                        .and_then(|v| v.as_str())
                                    {
                                        Some(n.trim().to_string())
                                    } else {
                                        None
                                    }
                                })
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();

                    let mut english_publishers = Vec::new();
                    if let Some(pubs) = s_data.get("publishers").and_then(|v| v.as_array()) {
                        for p in pubs {
                            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let pub_type = p
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_lowercase();
                            if !name.is_empty()
                                && (pub_type == "english" || pub_type.is_empty())
                                && !english_publishers.contains(&name.to_string())
                            {
                                english_publishers.push(name.to_string());
                            }
                        }
                    }
                    if let Some(p) = s_data.get("publisher").and_then(|v| v.as_str()) {
                        let clean = p.trim().to_string();
                        if !clean.is_empty() && !english_publishers.contains(&clean) {
                            english_publishers.push(clean);
                        }
                    }
                    let publisher =
                        (!english_publishers.is_empty()).then(|| english_publishers.join(", "));

                    let total_vols = s_data
                        .get("final_volume")
                        .or_else(|| s_data.get("total_volumes"))
                        .and_then(|v| v.as_u64())
                        .map(|u| u as u32);

                    (
                        disp_title, desc, writers, pencillers, genres, tags, publisher, total_vols,
                    )
                }
                Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    return Ok(MangaBakaOutcome::NetworkError(
                        "Rate limited (HTTP 429)".into(),
                    ));
                }
                Ok(r) => {
                    return Ok(MangaBakaOutcome::NetworkError(format!(
                        "Series request returned HTTP {}",
                        r.status()
                    )));
                }
                Err(e) => {
                    return Ok(MangaBakaOutcome::NetworkError(e.to_string()));
                }
            };

        // 4. Query works in collection to find matching exact volume number
        let mut works_url = Some(format!("/collections/{matched_collection_id}/works"));
        let mut pages_checked = 0;

        while let Some(url) = works_url {
            if pages_checked >= 20 {
                break;
            }
            pages_checked += 1;

            let w_resp = match self.public_request(reqwest::Method::GET, &url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    return Ok(MangaBakaOutcome::NetworkError(
                        "Rate limited (HTTP 429)".into(),
                    ));
                }
                Ok(r) => {
                    return Ok(MangaBakaOutcome::NetworkError(format!(
                        "Works request returned HTTP {}",
                        r.status()
                    )));
                }
                Err(e) => {
                    return Ok(MangaBakaOutcome::NetworkError(e.to_string()));
                }
            };

            let w_body: serde_json::Value = match w_resp.json().await {
                Ok(v) => v,
                Err(e) => return Ok(MangaBakaOutcome::NetworkError(e.to_string())),
            };

            let works = w_body
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            for work in works {
                let sequence = work
                    .get("sequence_numeric")
                    .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
                    .or_else(|| {
                        work.get("sequence")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                    })
                    .or_else(|| {
                        work.get("volume")
                            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
                    });

                if let Some(seq) = sequence {
                    if (seq - volume).abs() < 0.001 {
                        let release_date = work
                            .get("release_date")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string());
                        let work_summary = work.get("description").and_then(|v| {
                            if let Some(s) = v.as_str() {
                                Some(s.trim().to_string())
                            } else if let Some(desc) = v.get("desc").and_then(|d| d.as_str()) {
                                Some(desc.trim().to_string())
                            } else {
                                None
                            }
                        });
                        let summary = work_summary.or(series_desc);
                        let pages = work
                            .get("pages")
                            .and_then(|v| v.as_i64())
                            .filter(|p| *p > 0);
                        let work_id = work.get("id").and_then(|v| {
                            if let Some(s) = v.as_str() {
                                Some(s.to_string())
                            } else if let Some(n) = v.as_i64() {
                                Some(n.to_string())
                            } else {
                                None
                            }
                        });
                        let work_title = work
                            .get("title")
                            .or_else(|| work.get("name"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        let total_volumes = matched_total_works.or_else(|| {
                            if requested_edition.is_none_or(|e| {
                                crate::acquisition::edition::normalize_edition(e) == "standard"
                            }) {
                                total_vols
                            } else {
                                None
                            }
                        });
                        let web_url =
                            Some(format!("https://mangabaka.org/series/{matched_series_id}"));

                        return Ok(MangaBakaOutcome::Success(MangaBakaEnrichment {
                            series_id: matched_series_id,
                            series_title: Some(series_title),
                            work_title,
                            collection_id: matched_collection_id,
                            volume,
                            release_date,
                            summary,
                            pages,
                            writers,
                            pencillers,
                            genres,
                            tags,
                            publisher,
                            total_volumes,
                            web_url,
                            language: Some("en".to_string()),
                            work_id,
                        }));
                    }
                }
            }

            works_url = w_body
                .get("pagination")
                .and_then(|p| p.get("next"))
                .and_then(|n| n.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string());
        }

        tracing::info!("MANGABAKA: volume {volume} not found in collection for {series:?}");
        Ok(MangaBakaOutcome::VolumeNotFound(volume))
    }

    /// Fetches volume enrichment data for a series and volume number with fail-closed precision.
    pub async fn fetch_volume_enrichment(
        &self,
        series: &str,
        volume: f64,
    ) -> Result<MangaBakaOutcome> {
        self.resolve_volume_metadata(series, None, None, volume)
            .await
    }

    /// Enriches ComicInfo.xml inside an existing CBZ file with MangaBaka metadata.
    ///
    /// Case-insensitively removes all existing ComicInfo variants, writes exactly ONE
    /// standards-compliant `ComicInfo.xml`, verifies ZIP integrity, and atomically replaces the file.
    pub fn enrich_cbz_archive(cbz_path: &Path, enrichment: &MangaBakaEnrichment) -> Result<bool> {
        let series_title = enrichment.series_title.as_deref().unwrap_or("Manga");
        let params = ComicInfoParams {
            series: series_title,
            volume: Some(enrichment.volume),
            number: None,
            is_volume: true,
            title: enrichment.work_title.as_deref(),
            scanlator: None,
            notes: None,
        };
        write_comic_info_to_cbz(cbz_path, &params, Some(enrichment)).map(|_| true)
    }
}

pub fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
pub struct ComicInfoParams<'a> {
    pub series: &'a str,
    pub volume: Option<f64>,
    pub number: Option<f64>,
    pub is_volume: bool,
    pub title: Option<&'a str>,
    pub scanlator: Option<&'a str>,
    pub notes: Option<&'a str>,
}

/// Generates a standards-compatible ComicInfo.xml string.
///
/// Implements strict volume vs chapter numbering semantics:
/// - True Volume releases emit `<Volume>` and strictly omit `<Number>` to prevent polluting chapter indices.
/// - Chapter releases emit `<Number>` and only emit `<Volume>` if volume is known.
pub fn generate_comic_info_xml(
    params: &ComicInfoParams,
    enrichment: Option<&MangaBakaEnrichment>,
    existing_xml: Option<&str>,
    page_count: i64,
) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo>\n");
    let mut field = |tag: &str, value: Option<&str>| {
        if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
            xml.push_str(&format!(
                "  <{tag}>{}</{tag}>\n",
                crate::downloads::escape(v)
            ));
        }
    };

    // Series
    let series_str = enrichment
        .and_then(|e| e.series_title.as_deref())
        .unwrap_or(params.series);
    field("Series", Some(series_str));

    // Volume vs Number semantics
    if params.is_volume {
        if let Some(vol) = params.volume.or_else(|| enrichment.map(|e| e.volume)) {
            let vol_str = if vol.fract() == 0.0 {
                (vol as i64).to_string()
            } else {
                vol.to_string()
            };
            field("Volume", Some(&vol_str));
        }
        // Strict Volume Semantics: Omit Number for true volume acquisitions!
    } else {
        // For chapter releases only:
        if let Some(num) = params.number {
            let num_str = if num.fract() == 0.0 {
                (num as i64).to_string()
            } else {
                num.to_string()
            };
            field("Number", Some(&num_str));
        }
        if let Some(vol) = params.volume {
            let vol_str = if vol.fract() == 0.0 {
                (vol as i64).to_string()
            } else {
                vol.to_string()
            };
            field("Volume", Some(&vol_str));
        }
    }

    // Title
    let default_title = if params.is_volume {
        params
            .volume
            .or_else(|| enrichment.map(|e| e.volume))
            .map(|v| {
                if v.fract() == 0.0 {
                    format!("Volume {}", v as i64)
                } else {
                    format!("Volume {v}")
                }
            })
    } else {
        params.title.map(|s| s.to_string())
    };
    let title_str = enrichment
        .and_then(|e| e.work_title.as_deref())
        .or(params.title)
        .or(default_title.as_deref());
    field("Title", title_str);

    // Summary
    let summary = enrichment.and_then(|e| e.summary.as_deref());
    field("Summary", summary);

    // Publication Date (Year, Month, Day)
    if let Some(date) = enrichment.and_then(|e| e.release_date.as_deref()) {
        if let Some((y, rest)) = date.split_once('-') {
            if let Some((m, d)) = rest.split_once('-') {
                field("Year", Some(y.trim()));
                field("Month", Some(m.trim()));
                field("Day", Some(d.trim()));
            }
        }
    }

    // Authors & Artists
    if let Some(e) = enrichment {
        if !e.writers.is_empty() {
            field("Writer", Some(&e.writers.join(", ")));
        }
        if !e.pencillers.is_empty() {
            field("Penciller", Some(&e.pencillers.join(", ")));
        }
        if let Some(pub_name) = &e.publisher {
            field("Publisher", Some(pub_name));
        }
        if !e.genres.is_empty() {
            let normalized_genres = crate::pipeline::metadata::normalize_genres(&e.genres);
            if !normalized_genres.is_empty() {
                field("Genre", Some(&normalized_genres.join(", ")));
            }
        }
        if !e.tags.is_empty() {
            let normalized_tags = crate::pipeline::metadata::normalize_tags(&e.tags, &e.genres);
            if !normalized_tags.is_empty() {
                field("Tags", Some(&normalized_tags.join(", ")));
            }
        }
        if let Some(web) = &e.web_url {
            field("Web", Some(web));
        } else {
            let web_str = format!("https://mangabaka.org/series/{}", e.series_id);
            field("Web", Some(&web_str));
        }
        if let Some(total) = e.total_volumes {
            field("Count", Some(&total.to_string()));
        }
    }

    // PageCount
    if page_count > 0 {
        field("PageCount", Some(&page_count.to_string()));
    } else if let Some(pages) = enrichment.and_then(|e| e.pages) {
        field("PageCount", Some(&pages.to_string()));
    }

    // LanguageISO
    let lang_str = enrichment
        .and_then(|e| e.language.as_deref())
        .unwrap_or("en");
    field("LanguageISO", Some(lang_str));

    // Manga direction
    field("Manga", Some("YesAndRightToLeft"));

    // Translator & Notes existing check
    let mut has_existing_notes = false;
    if let Some(raw) = existing_xml {
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("<Notes>") {
                has_existing_notes = true;
                break;
            }
        }
    }

    if let Some(trans) = params.scanlator {
        field("Translator", Some(trans));
    }

    if !has_existing_notes {
        let notes = match (params.notes, enrichment) {
            (Some(n), _) => n.to_string(),
            (None, Some(e)) => format!("Saved by Mango · MangaBaka Series {}", e.series_id),
            (None, None) => "Saved by Mango".to_string(),
        };
        field("Notes", Some(&notes));
    } else if let Some(n) = params.notes {
        field("Notes", Some(n));
    }

    // Retain non-managed tags from existing_xml if present
    if let Some(raw) = existing_xml {
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("<ScanInformation>")
                || trimmed.starts_with("<Colorist>")
                || trimmed.starts_with("<Letterer>")
                || trimmed.starts_with("<Editor>")
                || trimmed.starts_with("<Imprint>")
                || (trimmed.starts_with("<Translator>") && params.scanlator.is_none())
                || (trimmed.starts_with("<Notes>") && params.notes.is_none())
            {
                xml.push_str(&format!("  {trimmed}\n"));
            }
        }
    }

    xml.push_str("</ComicInfo>\n");
    xml
}

/// Safely writes or replaces ComicInfo.xml inside an existing CBZ file.
///
/// Properties:
/// - Operates via an isolated temporary file; never mutates the original in-place until validation passes.
/// - Case-insensitively removes all existing ComicInfo files (ComicInfo.xml, comicinfo.xml, COMICINFO.XML).
/// - Ensures the final archive contains exactly ONE authoritative `ComicInfo.xml` at root.
/// - Preserves exact image ordering and filenames.
/// - Verifies ZIP integrity and ComicInfo existence before atomic replacement.
pub fn write_comic_info_to_cbz(
    cbz_path: &Path,
    params: &ComicInfoParams,
    enrichment: Option<&MangaBakaEnrichment>,
) -> Result<usize> {
    let file = File::open(cbz_path).map_err(|e| {
        AppError::bad_request(format!(
            "Cannot open CBZ archive {}: {e}",
            cbz_path.display()
        ))
    })?;
    let mut source = zip::ZipArchive::new(file).map_err(|e| {
        AppError::bad_request(format!("Invalid CBZ archive {}: {e}", cbz_path.display()))
    })?;

    let mut existing_xml = None;
    let mut image_count = 0;
    let mut image_names = Vec::new();

    for i in 0..source.len() {
        let mut entry = source
            .by_index(i)
            .map_err(|e| AppError::bad_request(e.to_string()))?;
        let entry_name = entry.name().to_string();
        let is_file = entry.is_file();
        let basename = entry_name
            .rsplit_once('/')
            .map(|(_, b)| b)
            .unwrap_or(&entry_name);

        if basename.eq_ignore_ascii_case("comicinfo.xml") {
            let mut content = String::new();
            let _ = entry.read_to_string(&mut content);
            if !content.trim().is_empty() {
                existing_xml = Some(content);
            }
        } else if is_file
            && !entry_name.starts_with("__MACOSX")
            && !basename.starts_with('.')
            && crate::local::is_image(&entry_name)
        {
            image_count += 1;
            image_names.push(entry_name);
        }
    }

    if image_count == 0 {
        return Err(AppError::bad_request(format!(
            "Archive {} contains 0 manga images",
            cbz_path.display()
        )));
    }

    let final_xml =
        generate_comic_info_xml(params, enrichment, existing_xml.as_deref(), image_count);

    if params.is_volume && !final_xml.contains("<Volume>") {
        return Err(AppError::internal(
            "Generated ComicInfo.xml missing required Volume tag for volume release",
        ));
    }
    write_prepared_comic_info_to_cbz(cbz_path, &final_xml)
}

/// Shared archive rewrite path for an already reviewed metadata document.
pub fn write_prepared_comic_info_to_cbz(cbz_path: &Path, final_xml: &str) -> Result<usize> {
    let parsed = roxmltree::Document::parse(final_xml)
        .map_err(|_| AppError::bad_request("Invalid prepared ComicInfo XML"))?;
    if parsed.root_element().tag_name().name() != "ComicInfo" {
        return Err(AppError::bad_request("Invalid ComicInfo root"));
    }
    let mut source = zip::ZipArchive::new(File::open(cbz_path)?)?;
    let image_count = source
        .file_names()
        .filter(|name| {
            crate::local::is_image(name)
                && !name.starts_with("__MACOSX")
                && !name.rsplit('/').next().unwrap_or("").starts_with('.')
        })
        .count();
    if image_count == 0 || !final_xml.starts_with("<?xml") || !final_xml.ends_with("</ComicInfo>\n")
    {
        return Err(AppError::bad_request(
            "Invalid prepared ComicInfo or empty archive",
        ));
    }
    // Write to temporary staging file in same directory
    let parent = cbz_path.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let temp_name = format!(
        ".{}.comicinfo-{}-{}.tmp",
        cbz_path.file_name().unwrap_or_default().to_string_lossy(),
        pid,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let temp_path = parent.join(temp_name);

    struct CleanupGuard<'a>(&'a Path);
    impl<'a> Drop for CleanupGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let guard = CleanupGuard(&temp_path);

    let temp_file = File::create(&temp_path)?;
    let mut dest_zip = zip::ZipWriter::new(temp_file);

    // Copy all non-ComicInfo entries with exact names and compression
    for i in 0..source.len() {
        let mut entry = source.by_index(i)?;
        let name = entry.name().to_string();
        let basename = name.rsplit_once('/').map(|(_, b)| b).unwrap_or(&name);

        if basename.eq_ignore_ascii_case("comicinfo.xml") {
            continue; // Skip any existing case-insensitive ComicInfo variants
        }

        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        dest_zip.start_file(&name, options)?;
        std::io::copy(&mut entry, &mut dest_zip)?;
    }

    // Add exactly ONE ComicInfo.xml at root
    let xml_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    dest_zip.start_file("ComicInfo.xml", xml_options)?;
    dest_zip.write_all(final_xml.as_bytes())?;
    dest_zip.finish()?;

    // Validate the temporary file
    crate::pipeline::publisher::verify_cbz_integrity(&temp_path)?;

    // Assert exactly ONE ComicInfo.xml exists case-insensitively
    {
        let check_file = File::open(&temp_path)?;
        let mut check_zip = zip::ZipArchive::new(check_file)?;
        let mut comicinfo_matches = Vec::new();
        let mut new_images = 0;
        for i in 0..check_zip.len() {
            let entry = check_zip.by_index(i)?;
            let name = entry.name().to_string();
            let basename = name.rsplit_once('/').map(|(_, b)| b).unwrap_or(&name);
            if basename.eq_ignore_ascii_case("comicinfo.xml") {
                comicinfo_matches.push(name.clone());
            }
            if entry.is_file()
                && !name.starts_with("__MACOSX")
                && !basename.starts_with('.')
                && crate::local::is_image(&name)
            {
                new_images += 1;
            }
        }
        if comicinfo_matches.len() != 1 || comicinfo_matches[0] != "ComicInfo.xml" {
            return Err(AppError::internal(format!(
                "CBZ rewrite failed: expected exactly 1 'ComicInfo.xml', found {:?}",
                comicinfo_matches
            )));
        }
        if new_images != image_count {
            return Err(AppError::internal(format!(
                "CBZ rewrite page count mismatch: expected {image_count}, got {new_images}"
            )));
        }
    }

    // Atomic replacement
    fs::rename(&temp_path, cbz_path)?;
    std::mem::forget(guard); // Disarm guard on success

    tracing::info!(
        "CBZ: successfully wrote ComicInfo.xml to {}",
        cbz_path.display()
    );
    Ok(image_count)
}

pub fn enrich_comic_info_xml(xml: &str, enrichment: &MangaBakaEnrichment) -> Result<String> {
    let series = enrichment.series_title.as_deref().unwrap_or("Manga");
    let params = ComicInfoParams {
        series,
        volume: Some(enrichment.volume),
        number: None,
        is_volume: true,
        title: enrichment.work_title.as_deref(),
        scanlator: None,
        notes: None,
    };
    Ok(generate_comic_info_xml(
        &params,
        Some(enrichment),
        Some(xml),
        enrichment.pages.unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_normalization() {
        assert_eq!(normalize_title("  Blue   Lock  "), "blue lock");
        assert_eq!(normalize_title("ONE PIECE"), "one piece");
    }

    #[test]
    fn comic_info_xml_enrichment() {
        let base_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Title>Volume 1</Title>
  <Series>Blue Lock</Series>
  <Volume>1</Volume>
  <Translator>Stick</Translator>
  <Notes>Saved by Mango</Notes>
</ComicInfo>"#;

        let enrichment = MangaBakaEnrichment {
            series_id: 83,
            series_title: Some("Blue Lock".into()),
            collection_id: "col-123".into(),
            volume: 1.0,
            release_date: Some("2021-03-16".into()),
            summary: Some("Soccer high school thriller".into()),
            pages: Some(208),
            writers: vec!["Muneyuki Kaneshiro".into()],
            pencillers: vec!["Yuusuke Nomura".into()],
            genres: vec!["Sports".into(), "Shounen".into()],
            publisher: Some("Kodansha Manga".into()),
            ..Default::default()
        };

        let enriched = enrich_comic_info_xml(base_xml, &enrichment).unwrap();
        assert!(enriched.contains("<Series>Blue Lock</Series>"));
        assert!(enriched.contains("<Volume>1</Volume>"));
        assert!(
            enriched.contains("<Translator>Stick</Translator>"),
            "Preserves existing Translator"
        );
        assert!(
            enriched.contains("<Notes>Saved by Mango</Notes>"),
            "Preserves existing Notes"
        );
        assert!(enriched.contains("<Year>2021</Year>"));
        assert!(enriched.contains("<Month>03</Month>"));
        assert!(enriched.contains("<Day>16</Day>"));
        assert!(enriched.contains("<PageCount>208</PageCount>"));
        assert!(enriched.contains("<Writer>Muneyuki Kaneshiro</Writer>"));
        assert!(enriched.contains("<Penciller>Yuusuke Nomura</Penciller>"));
        assert!(enriched.contains("<Publisher>Kodansha Manga</Publisher>"));
    }

    #[test]
    fn outcome_descriptions_and_variants() {
        let success = MangaBakaOutcome::Success(MangaBakaEnrichment {
            series_id: 42,
            collection_id: "c1".into(),
            volume: 2.0,
            ..Default::default()
        });
        assert!(success.is_success());
        assert!(success.description().contains("Enriched volume 2"));

        let not_found = MangaBakaOutcome::SeriesNotFound("Unknown Manga".into());
        assert!(!not_found.is_success());
        assert_eq!(
            not_found.description(),
            "No exact title match for 'Unknown Manga'"
        );

        let ambiguous = MangaBakaOutcome::AmbiguousSeries(3);
        assert!(!ambiguous.is_success());
        assert!(ambiguous.description().contains("Found 3 candidate series"));

        let vol_not_found = MangaBakaOutcome::VolumeNotFound(99.0);
        assert!(!vol_not_found.is_success());
        assert_eq!(
            vol_not_found.description(),
            "Volume 99 not found in collection"
        );
    }

    #[tokio::test]
    async fn mangabaka_precision_matching_and_enrichment_flow() {
        use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/series/search",
                get(
                    |axum::extract::Query(params): axum::extract::Query<
                        std::collections::HashMap<String, String>,
                    >| async move {
                        if params.contains_key("query") {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "message": "Validation error: Unrecognized key: \"query\"",
                                    "status": 400,
                                    "code": null
                                })),
                            )
                                .into_response();
                        }
                        let q = params.get("q").cloned().unwrap_or_default();
                        if q == "Blue Lock" {
                            Json(json!({
                                "data": [
                                    {
                                        "id": 101,
                                        "title": "Blue Lock",
                                        "title_english": "Blue Lock",
                                        "title_romaji": "Blue Lock"
                                    },
                                    {
                                        "id": 102,
                                        "title": "Blue Lock: Episode Nagi",
                                        "title_english": "Blue Lock: Episode Nagi"
                                    }
                                ]
                            }))
                            .into_response()
                        } else if q == "Ambiguous Title" {
                            Json(json!({
                                "data": [
                                    { "id": 201, "title": "Ambiguous Title" },
                                    { "id": 202, "title": "Ambiguous Title" }
                                ]
                            }))
                            .into_response()
                        } else {
                            Json(json!({ "data": [] })).into_response()
                        }
                    },
                ),
            )
            .route(
                "/series/101/collections",
                get(|| async {
                    Json(json!({
                        "data": [
                            {
                                "id": "col-standard-en",
                                "language": { "iso": "en" },
                                "edition": { "name": "Standard Edition" },
                                "medium": "digital"
                            },
                            {
                                "id": "col-print-en",
                                "language": { "iso": "en" },
                                "edition": { "name": "Standard Edition" },
                                "medium": "print"
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/series/101",
                get(|| async {
                    Json(json!({
                        "data": {
                            "authors": ["Muneyuki Kaneshiro"],
                            "artists": ["Yuusuke Nomura"],
                            "genres": ["sports", "thriller"],
                            "publishers": [{ "name": "Kodansha", "type": "english" }]
                        }
                    }))
                }),
            )
            .route(
                "/collections/col-standard-en/works",
                get(|| async {
                    Json(json!({
                        "data": [
                            {
                                "sequence_numeric": 1.0,
                                "release_date": "2021-03-16",
                                "description": "Vol 1 summary",
                                "pages": 208
                            },
                            {
                                "sequence_numeric": 2.0,
                                "release_date": "2021-05-18",
                                "description": "Vol 2 summary",
                                "pages": 192
                            }
                        ],
                        "pagination": {}
                    }))
                }),
            )
            .route(
                "/my/profile",
                get(|headers: axum::http::HeaderMap| async move {
                    if headers.contains_key("x-api-key") {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "status": 200,
                                "data": {
                                    "id": "usr_test",
                                    "preferred_username": "tester"
                                }
                            })),
                        )
                            .into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = MangaBakaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("fake-token".into()),
        );

        // 1. Connection test
        assert!(client.test_connection().await.is_ok());

        // 2. Exact match success for Volume 1
        let outcome1 = client
            .fetch_volume_enrichment("Blue Lock", 1.0)
            .await
            .unwrap();
        match outcome1 {
            MangaBakaOutcome::Success(e) => {
                assert_eq!(e.series_id, 101);
                assert_eq!(e.volume, 1.0);
                assert_eq!(e.release_date.as_deref(), Some("2021-03-16"));
                assert_eq!(e.pages, Some(208));
                assert_eq!(e.writers, vec!["Muneyuki Kaneshiro"]);
                assert_eq!(e.publisher.as_deref(), Some("Kodansha"));
            }
            other => panic!("Expected Success, got {other:?}"),
        }

        // 3. Exact match for non-existent volume (Volume 3) -> VolumeNotFound, no substitution!
        let outcome3 = client
            .fetch_volume_enrichment("Blue Lock", 3.0)
            .await
            .unwrap();
        assert_eq!(outcome3, MangaBakaOutcome::VolumeNotFound(3.0));

        // 4. Ambiguous match rejection
        let outcome_ambig = client
            .fetch_volume_enrichment("Ambiguous Title", 1.0)
            .await
            .unwrap();
        assert_eq!(outcome_ambig, MangaBakaOutcome::AmbiguousSeries(2));

        // 5. Unknown series rejection
        let outcome_unknown = client
            .fetch_volume_enrichment("Nonexistent Title", 1.0)
            .await
            .unwrap();
        assert_eq!(
            outcome_unknown,
            MangaBakaOutcome::SeriesNotFound("Nonexistent Title".into())
        );
    }

    #[test]
    fn test_chapter_vs_volume_numbering_rules() {
        let enrichment = MangaBakaEnrichment {
            series_id: 83,
            series_title: Some("Blue Lock".into()),
            collection_id: "col-123".into(),
            volume: 39.0,
            ..Default::default()
        };

        // 1. Volume acquisition: <Volume> must be present, <Number> must be omitted!
        let vol_params = ComicInfoParams {
            series: "Blue Lock",
            volume: Some(39.0),
            number: None,
            is_volume: true,
            title: Some("Volume 39"),
            scanlator: None,
            notes: None,
        };
        let xml_vol = generate_comic_info_xml(&vol_params, Some(&enrichment), None, 192);
        assert!(xml_vol.contains("<Volume>39</Volume>"));
        assert!(
            !xml_vol.contains("<Number>"),
            "Number tag must be omitted for volume acquisitions!"
        );

        // 2. Fractional volume acquisition
        let frac_vol_params = ComicInfoParams {
            series: "Blue Lock",
            volume: Some(39.5),
            number: None,
            is_volume: true,
            title: Some("Volume 39.5"),
            scanlator: None,
            notes: None,
        };
        let xml_frac_vol = generate_comic_info_xml(&frac_vol_params, Some(&enrichment), None, 100);
        assert!(xml_frac_vol.contains("<Volume>39.5</Volume>"));
        assert!(
            !xml_frac_vol.contains("<Number>"),
            "Number tag must be omitted for volume acquisitions!"
        );

        // 3. Chapter acquisition: <Number> must be present, <Volume> omitted if not known
        let ch_params = ComicInfoParams {
            series: "Blue Lock",
            volume: None,
            number: Some(15.0),
            is_volume: false,
            title: Some("Chapter 15"),
            scanlator: None,
            notes: None,
        };
        let xml_ch = generate_comic_info_xml(&ch_params, None, None, 20);
        assert!(xml_ch.contains("<Number>15</Number>"));
        assert!(
            !xml_ch.contains("<Volume>"),
            "Volume tag must be omitted for chapter acquisitions when unknown!"
        );

        // 4. Fractional chapter acquisition
        let frac_ch_params = ComicInfoParams {
            series: "Blue Lock",
            volume: None,
            number: Some(15.5),
            is_volume: false,
            title: Some("Chapter 15.5"),
            scanlator: None,
            notes: None,
        };
        let xml_frac_ch = generate_comic_info_xml(&frac_ch_params, None, None, 25);
        assert!(xml_frac_ch.contains("<Number>15.5</Number>"));
        assert!(!xml_frac_ch.contains("<Volume>"));
    }

    #[test]
    fn test_stale_comic_info_and_case_insensitive_cleanup() {
        let temp_dir =
            std::env::temp_dir().join(format!("mango-test-cbz-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let cbz_path = temp_dir.join("test_cleanup.cbz");

        // Create a CBZ with:
        // - comicinfo.xml (lowercase)
        // - sub/ComicInfo.xml (nested)
        // - ComicInfo.xml (root with stale metadata)
        // - 001.png, 002.png
        {
            let file = File::create(&cbz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opt = zip::write::SimpleFileOptions::default();

            zip.start_file("comicinfo.xml", opt).unwrap();
            zip.write_all(b"<ComicInfo><Series>Stale Lowercase</Series></ComicInfo>")
                .unwrap();

            zip.start_file("sub/ComicInfo.xml", opt).unwrap();
            zip.write_all(b"<ComicInfo><Series>Stale Nested</Series></ComicInfo>")
                .unwrap();

            zip.start_file("ComicInfo.xml", opt).unwrap();
            zip.write_all(
                b"<ComicInfo><Series>Stale Root</Series><Volume>999</Volume></ComicInfo>",
            )
            .unwrap();

            zip.start_file("001.png", opt).unwrap();
            zip.write_all(b"fake image 1").unwrap();

            zip.start_file("002.png", opt).unwrap();
            zip.write_all(b"fake image 2").unwrap();

            zip.finish().unwrap();
        }

        let enrichment = MangaBakaEnrichment {
            series_id: 83,
            series_title: Some("Blue Lock".into()),
            collection_id: "col-std".into(),
            volume: 39.0,
            release_date: Some("2026-01-20".into()),
            summary: Some("Egoist battle continues".into()),
            pages: Some(2),
            writers: vec!["Muneyuki Kaneshiro".into()],
            pencillers: vec!["Yusuke Nomura".into()],
            genres: vec!["Sports".into()],
            publisher: Some("Kodansha".into()),
            tags: vec!["Soccer".into()],
            web_url: Some("https://mangabaka.org/series/83".into()),
            language: Some("en".into()),
            ..Default::default()
        };

        let params = ComicInfoParams {
            series: "Blue Lock",
            volume: Some(39.0),
            number: None,
            is_volume: true,
            title: Some("Volume 39"),
            scanlator: None,
            notes: None,
        };

        let pages = write_comic_info_to_cbz(&cbz_path, &params, Some(&enrichment)).unwrap();
        assert_eq!(pages, 2);

        // Verify resulting CBZ
        let file = File::open(&cbz_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        let mut comic_info_entries = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i).unwrap();
            let name = entry.name().to_string();
            if name.to_lowercase().ends_with("comicinfo.xml") {
                comic_info_entries.push(name);
            }
        }

        assert_eq!(
            comic_info_entries.len(),
            1,
            "Must have exactly ONE ComicInfo.xml case-insensitively"
        );
        assert_eq!(
            comic_info_entries[0], "ComicInfo.xml",
            "Name must be exactly ComicInfo.xml at root"
        );

        let mut root_xml_entry = archive.by_name("ComicInfo.xml").unwrap();
        let mut xml_content = String::new();
        root_xml_entry.read_to_string(&mut xml_content).unwrap();

        assert!(xml_content.contains("<Series>Blue Lock</Series>"));
        assert!(xml_content.contains("<Volume>39</Volume>"));
        assert!(!xml_content.contains("Stale Lowercase"));
        assert!(!xml_content.contains("Stale Nested"));
        assert!(!xml_content.contains("Stale Root"));
        assert!(!xml_content.contains("<Volume>999</Volume>"));
        assert!(!xml_content.contains("<Number>"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_blue_lock_fixture_authoritative_resolution_and_comic_info() {
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/source/anilist/106130",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": {
                            "series_id": 83
                        }
                    }))
                }),
            )
            .route(
                "/series/83",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": {
                            "id": 83,
                            "title": "Blue Lock",
                            "synopsis": "After a disastrous defeat at the 2018 World Cup, Japan's team struggles to regroup.",
                            "authors": [
                                {"name": "Muneyuki Kaneshiro", "role": "Story"},
                                {"name": "Yusuke Nomura", "role": "Art"}
                            ],
                            "genres": ["Shounen", "Sports"],
                            "tags": ["Soccer", "Battle Royale"],
                            "publisher": "Kodansha"
                        }
                    }))
                }),
            )
            .route(
                "/series/83/collections",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": [
                            {
                                "id": "col_jp",
                                "name": "Standard Edition",
                                "language": "Japanese",
                                "format": "Print",
                                "work_count": 39
                            },
                            {
                                "id": "col_en_std",
                                "name": "Standard Edition",
                                "language": "English",
                                "format": "Digital",
                                "work_count": 39
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/collections/col_en_std/works",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": [
                            {
                                "id": "work_39",
                                "name": "Volume 39",
                                "volume": 39.0,
                                "sequence_numeric": 39.0,
                                "sequence": "39",
                                "release_date": "2026-01-20",
                                "pages": 192,
                                "synopsis": "The climax of the Neo Egoist League!"
                            }
                        ]
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = MangaBakaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("test-token".into()),
        );

        // Resolve via AniList xref
        let outcome = client
            .resolve_volume_metadata("Blue Lock", Some(106130), None, 39.0)
            .await
            .expect("MangaBaka query succeeds");

        let enrichment = match outcome {
            MangaBakaOutcome::Success(e) => e,
            other => panic!("Expected Success, got {other:?}"),
        };

        assert_eq!(enrichment.series_id, 83);
        assert_eq!(enrichment.series_title.as_deref(), Some("Blue Lock"));
        assert_eq!(enrichment.work_title.as_deref(), Some("Volume 39"));
        assert_eq!(enrichment.volume, 39.0);
        assert_eq!(enrichment.release_date.as_deref(), Some("2026-01-20"));
        assert_eq!(enrichment.writers, vec!["Muneyuki Kaneshiro"]);
        assert_eq!(enrichment.pencillers, vec!["Yusuke Nomura"]);
        assert_eq!(enrichment.publisher.as_deref(), Some("Kodansha"));
        assert_eq!(enrichment.genres, vec!["Shounen", "Sports"]);
        assert_eq!(enrichment.tags, vec!["Soccer", "Battle Royale"]);
        assert_eq!(enrichment.language.as_deref(), Some("en"));
        assert_eq!(
            enrichment.web_url.as_deref(),
            Some("https://mangabaka.org/series/83")
        );

        // Create CBZ fixture
        let temp_dir =
            std::env::temp_dir().join(format!("mango-bluelock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let cbz_path = temp_dir.join("Blue Lock v39.cbz");

        {
            let file = File::create(&cbz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opt = zip::write::SimpleFileOptions::default();
            zip.start_file("001.jpg", opt).unwrap();
            zip.write_all(b"jpeg image 1 data").unwrap();
            zip.start_file("002.jpg", opt).unwrap();
            zip.write_all(b"jpeg image 2 data").unwrap();
            zip.finish().unwrap();
        }

        let params = ComicInfoParams {
            series: "Blue Lock",
            volume: Some(39.0),
            number: None,
            is_volume: true,
            title: enrichment.work_title.as_deref(),
            scanlator: None,
            notes: None,
        };

        let page_count = write_comic_info_to_cbz(&cbz_path, &params, Some(&enrichment)).unwrap();
        assert_eq!(page_count, 2);

        // Verify CBZ integrity
        crate::pipeline::publisher::verify_cbz_integrity(&cbz_path).unwrap();

        // Verify XML content
        let file = File::open(&cbz_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.len(), 3); // 2 images + 1 ComicInfo.xml

        let mut xml_file = archive.by_name("ComicInfo.xml").unwrap();
        let mut xml_str = String::new();
        xml_file.read_to_string(&mut xml_str).unwrap();

        assert!(xml_str.contains("<Series>Blue Lock</Series>"));
        assert!(xml_str.contains("<Volume>39</Volume>"));
        assert!(
            !xml_str.contains("<Number>"),
            "Number must not be emitted for volume"
        );
        assert!(xml_str.contains("<LanguageISO>en</LanguageISO>"));
        assert!(xml_str.contains("<Writer>Muneyuki Kaneshiro</Writer>"));
        assert!(xml_str.contains("<Penciller>Yusuke Nomura</Penciller>"));
        assert!(xml_str.contains("<Publisher>Kodansha</Publisher>"));
        assert!(xml_str.contains("<Genre>Shounen, Sports</Genre>"));
        assert!(xml_str.contains("<Tags>Soccer, Battle Royale</Tags>"));
        assert!(xml_str.contains("<Web>https://mangabaka.org/series/83</Web>"));
        assert!(xml_str.contains("<Manga>YesAndRightToLeft</Manga>"));
        assert!(xml_str.contains("<PageCount>2</PageCount>"));
        assert!(xml_str.contains("<Year>2026</Year>"));
        assert!(xml_str.contains("<Month>01</Month>"));
        assert!(xml_str.contains("<Day>20</Day>"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_ambiguous_collection_fails_closed() {
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/series/83",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": {
                            "id": 83,
                            "title": "Blue Lock",
                            "authors": [],
                            "genres": [],
                            "tags": []
                        }
                    }))
                }),
            )
            .route(
                "/series/83/collections",
                get(|| async move {
                    Json(json!({
                        "status": 200,
                        "data": [
                            {
                                "id": "col_1",
                                "name": "Standard Edition",
                                "language": "English",
                                "format": "Digital"
                            },
                            {
                                "id": "col_2",
                                "name": "Standard Edition",
                                "language": "English",
                                "format": "Digital"
                            }
                        ]
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = MangaBakaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("test-token".into()),
        );

        let outcome = client
            .resolve_volume_metadata("Blue Lock", None, Some(83), 39.0)
            .await
            .expect("Call succeeded");

        assert_eq!(outcome, MangaBakaOutcome::AmbiguousCollection(2));
    }

    #[tokio::test]
    async fn test_network_error_failure_handling() {
        use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};

        let app = Router::new().route(
            "/series/83/collections",
            get(|| async move { (StatusCode::BAD_GATEWAY, "Bad Gateway").into_response() }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = MangaBakaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("test-token".into()),
        );

        let outcome = client
            .resolve_volume_metadata("Blue Lock", None, Some(83), 39.0)
            .await
            .expect("Call returns NetworkError outcome");

        match outcome {
            MangaBakaOutcome::NetworkError(msg) => {
                assert!(
                    msg.contains("502") || msg.contains("Bad Gateway"),
                    "Message was: {msg}"
                );
            }
            other => panic!("Expected NetworkError, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod edition_collection_regressions {
    use super::*;
    use axum::{routing::get, Json, Router};
    use serde_json::json;

    #[tokio::test]
    async fn authoritative_collection_edition_is_respected() {
        let app = Router::new()
            .route("/series/101/collections", get(|| async { Json(json!({"data": [
                {"id":"standard", "language":"en", "edition":{"name":"Standard Edition"}, "medium":"digital", "total_works":12},
                {"id":"master", "language":"en", "edition":{"name":"Master Edition"}, "medium":"digital", "total_works":6},
                {"id":"print-master", "language":"en", "edition":{"name":"Master Edition"}, "medium":"print"}
            ]})) }))
            .route("/series/101", get(|| async { Json(json!({"data": {"title":"Series X", "total_volumes":12}})) }))
            .route("/collections/master/works", get(|| async { Json(json!({"data":[{"id":"work4", "sequence_numeric":4, "title":"Master volume four"}]})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = MangaBakaClient::new(
            reqwest::Client::new(),
            Some(format!("http://{address}")),
            None,
        );
        assert!(
            matches!(client.match_collection(101).await.unwrap(), MangaBakaCollectionMatch::Found { id, .. } if id == "standard")
        );
        assert!(
            matches!(client.match_edition_collection(101, Some("Master Edition")).await.unwrap(), MangaBakaCollectionMatch::Found { id, total_works: Some(6), .. } if id == "master")
        );
        assert!(matches!(
            client
                .match_edition_collection(101, Some("Deluxe Edition"))
                .await
                .unwrap(),
            MangaBakaCollectionMatch::NotFound
        ));
        let result = client
            .resolve_edition_volume_metadata(
                "Series X",
                None,
                Some(101),
                4.0,
                Some("Master Edition"),
            )
            .await
            .unwrap();
        assert!(
            matches!(result, MangaBakaOutcome::Success(e) if e.collection_id == "master" && e.work_id.as_deref() == Some("work4") && e.total_volumes == Some(6))
        );
        server.abort();
    }
}
