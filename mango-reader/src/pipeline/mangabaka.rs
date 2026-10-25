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
        if (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || ('\u{00C0}'..='\u{024F}').contains(&c) {
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
) -> (String, Option<String>, Option<String>, Option<String>, Vec<String>) {
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
            let is_official = traits.map_or(false, |tr| tr.iter().any(|t| t.as_str() == Some("official")));
            let is_native = traits.map_or(false, |tr| tr.iter().any(|t| t.as_str() == Some("native")));
            let is_primary = item.get("is_primary").and_then(|v| v.as_bool()).unwrap_or(false);
            let t = item.get("title").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

            let Some(title_str) = t else { continue; };

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
        .or_else(|| aliases.iter().find(|s| is_latin_script(s)).map(|s| s.as_str()))
        .or_else(|| top_title.as_deref())
        .or_else(|| aliases.first().map(|s| s.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("MangaBaka Series {series_id}"));

    (display_title, english_title, romaji_title, native_title, aliases)
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaBakaEnrichment {
    pub series_id: i64,
    pub collection_id: String,
    pub volume: f64,
    pub release_date: Option<String>,
    pub summary: Option<String>,
    pub pages: Option<i64>,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub genres: Vec<String>,
    pub publisher: Option<String>,
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
                    .unwrap_or_else(|| (format!("MangaBaka Series {series_id}"), None, None, None, Vec::new()));

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
        for col in collections {
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
                && edition == "standard edition"
                && medium == "digital"
            {
                if let Some(col_id) = col.get("id").and_then(|v| v.as_str()) {
                    collection_matches.push((col_id.to_string(), edition, total_works));
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

    /// Fetches volume enrichment data for a series and volume number with fail-closed precision.
    pub async fn fetch_volume_enrichment(
        &self,
        series: &str,
        volume: f64,
    ) -> Result<MangaBakaOutcome> {
        let trimmed_series = series.trim();
        if trimmed_series.is_empty() {
            return Ok(MangaBakaOutcome::SeriesNotFound("empty".into()));
        }

        // 1. Search series with fail-closed ambiguity check
        let (matched_series_id, matched_collection_id) = match self
            .match_series(trimmed_series)
            .await?
        {
            MangaBakaSeriesMatch::Found { id, .. } => match self.match_collection(id).await? {
                MangaBakaCollectionMatch::Found { id: col_id, .. } => (id, col_id),
                MangaBakaCollectionMatch::Ambiguous(candidates) => {
                    return Ok(MangaBakaOutcome::CollectionNotFound(format!(
                        "Found {} matching collections; ambiguous",
                        candidates.len()
                    )));
                }
                MangaBakaCollectionMatch::NotFound => {
                    return Ok(MangaBakaOutcome::CollectionNotFound(
                        "No English digital Standard Edition found".to_string(),
                    ));
                }
                MangaBakaCollectionMatch::AuthError(e) => {
                    return Ok(MangaBakaOutcome::AuthError(e));
                }
                MangaBakaCollectionMatch::NetworkError(e) => {
                    return Ok(MangaBakaOutcome::NetworkError(e));
                }
            },
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
            MangaBakaSeriesMatch::NetworkError(e) => return Ok(MangaBakaOutcome::NetworkError(e)),
        };

        // 2. Fetch normalized series metadata for authors/artists/genres/publishers
        let series_url = format!("/series/{matched_series_id}");
        let (writers, pencillers, genres, publisher) = match self
            .public_request(reqwest::Method::GET, &series_url)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                let s_body: serde_json::Value = r.json().await.unwrap_or_default();
                let s_data = s_body.get("data").unwrap_or(&s_body);

                let writers: Vec<String> = s_data
                    .get("authors")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let pencillers: Vec<String> = s_data
                    .get("artists")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

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
                let publisher =
                    (!english_publishers.is_empty()).then(|| english_publishers.join(", "));
                (writers, pencillers, genres, publisher)
            }
            _ => (Vec::new(), Vec::new(), Vec::new(), None),
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
                _ => break,
            };

            let w_body: serde_json::Value = match w_resp.json().await {
                Ok(v) => v,
                Err(_) => break,
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
                    });

                if let Some(seq) = sequence {
                    if (seq - volume).abs() < 0.001 {
                        let release_date = work
                            .get("release_date")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string());
                        let summary = work.get("description").and_then(|v| {
                            if let Some(s) = v.as_str() {
                                Some(s.trim().to_string())
                            } else if let Some(desc) = v.get("desc").and_then(|d| d.as_str()) {
                                Some(desc.trim().to_string())
                            } else {
                                None
                            }
                        });
                        let pages = work
                            .get("pages")
                            .and_then(|v| v.as_i64())
                            .filter(|p| *p > 0);

                        return Ok(MangaBakaOutcome::Success(MangaBakaEnrichment {
                            series_id: matched_series_id,
                            collection_id: matched_collection_id,
                            volume,
                            release_date,
                            summary,
                            pages,
                            writers,
                            pencillers,
                            genres,
                            publisher,
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

    /// Enriches ComicInfo.xml inside an existing CBZ file with MangaBaka metadata.
    pub fn enrich_cbz_archive(cbz_path: &Path, enrichment: &MangaBakaEnrichment) -> Result<bool> {
        let file = File::open(cbz_path)?;
        let mut source = zip::ZipArchive::new(file).map_err(|e| {
            AppError::bad_request(format!("Invalid CBZ {}: {e}", cbz_path.display()))
        })?;

        let mut comicinfo_idx = None;
        for i in 0..source.len() {
            let entry = source
                .by_index(i)
                .map_err(|e| AppError::bad_request(e.to_string()))?;
            if Path::new(entry.name())
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.eq_ignore_ascii_case("ComicInfo.xml"))
                .unwrap_or(false)
            {
                comicinfo_idx = Some(i);
                break;
            }
        }

        let comicinfo_idx = match comicinfo_idx {
            Some(idx) => idx,
            None => {
                tracing::warn!(
                    "MANGABAKA: ComicInfo.xml not found in {}",
                    cbz_path.display()
                );
                return Ok(false);
            }
        };

        let mut existing_xml = String::new();
        source
            .by_index(comicinfo_idx)?
            .read_to_string(&mut existing_xml)?;

        let updated_xml = enrich_comic_info_xml(&existing_xml, enrichment)?;

        // Write to temporary file and atomically replace
        let parent = cbz_path.parent().unwrap_or_else(|| Path::new("."));
        let pid = std::process::id();
        let temp_path = parent.join(format!(
            ".{}.mangabaka-{pid}.tmp",
            cbz_path.file_name().unwrap().to_string_lossy()
        ));

        struct CleanupGuard<'a>(&'a Path);
        impl<'a> Drop for CleanupGuard<'a> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }
        let _guard = CleanupGuard(&temp_path);

        let temp_file = File::create(&temp_path)?;
        let mut dest_zip = zip::ZipWriter::new(temp_file);

        for i in 0..source.len() {
            let mut entry = source.by_index(i)?;
            let options =
                zip::write::SimpleFileOptions::default().compression_method(entry.compression());
            dest_zip.start_file(entry.name(), options)?;
            if i == comicinfo_idx {
                dest_zip.write_all(updated_xml.as_bytes())?;
            } else {
                std::io::copy(&mut entry, &mut dest_zip)?;
            }
        }
        dest_zip.finish()?;

        // Verify temp zip integrity
        crate::pipeline::publisher::verify_cbz_integrity(&temp_path)?;

        fs::rename(&temp_path, cbz_path)?;
        tracing::info!("MANGABAKA: enriched {}", cbz_path.display());
        Ok(true)
    }
}

pub fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn enrich_comic_info_xml(xml: &str, enrichment: &MangaBakaEnrichment) -> Result<String> {
    let mut lines = Vec::new();
    let mut inside_comicinfo = false;

    // Tags that MangaBaka can safely enrich
    let managed_tags = [
        "Year",
        "Month",
        "Day",
        "Summary",
        "PageCount",
        "Writer",
        "Penciller",
        "Genre",
        "Publisher",
        "LanguageISO",
    ];

    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<ComicInfo") {
            inside_comicinfo = true;
            lines.push(line.to_string());
            continue;
        }
        if trimmed.starts_with("</ComicInfo>") {
            inside_comicinfo = false;

            // Append managed tags if not present
            if let Some(date) = &enrichment.release_date {
                if let Some((y, rest)) = date.split_once('-') {
                    if let Some((m, d)) = rest.split_once('-') {
                        lines.push(format!("  <Year>{}</Year>", y.trim()));
                        lines.push(format!("  <Month>{}</Month>", m.trim()));
                        lines.push(format!("  <Day>{}</Day>", d.trim()));
                    }
                }
            }
            if let Some(pages) = enrichment.pages {
                lines.push(format!("  <PageCount>{pages}</PageCount>"));
            }
            if let Some(summary) = &enrichment.summary {
                lines.push(format!(
                    "  <Summary>{}</Summary>",
                    crate::downloads::escape(summary)
                ));
            }
            if !enrichment.writers.is_empty() {
                lines.push(format!(
                    "  <Writer>{}</Writer>",
                    crate::downloads::escape(&enrichment.writers.join(", "))
                ));
            }
            if !enrichment.pencillers.is_empty() {
                lines.push(format!(
                    "  <Penciller>{}</Penciller>",
                    crate::downloads::escape(&enrichment.pencillers.join(", "))
                ));
            }
            if !enrichment.genres.is_empty() {
                lines.push(format!(
                    "  <Genre>{}</Genre>",
                    crate::downloads::escape(&enrichment.genres.join(", "))
                ));
            }
            if let Some(publisher) = &enrichment.publisher {
                lines.push(format!(
                    "  <Publisher>{}</Publisher>",
                    crate::downloads::escape(publisher)
                ));
            }
            lines.push("  <LanguageISO>en</LanguageISO>".to_string());

            lines.push(line.to_string());
            continue;
        }

        if inside_comicinfo {
            let is_managed = managed_tags
                .iter()
                .any(|tag| trimmed.starts_with(&format!("<{tag}>")));
            if is_managed {
                continue; // replaced by enriched version
            }
        }
        lines.push(line.to_string());
    }

    Ok(lines.join("\n"))
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
            collection_id: "col-123".into(),
            volume: 1.0,
            release_date: Some("2021-03-16".into()),
            summary: Some("Soccer high school thriller".into()),
            pages: Some(208),
            writers: vec!["Muneyuki Kaneshiro".into()],
            pencillers: vec!["Yuusuke Nomura".into()],
            genres: vec!["Sports".into(), "Shounen".into()],
            publisher: Some("Kodansha Manga".into()),
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
            release_date: None,
            summary: None,
            pages: None,
            writers: vec![],
            pencillers: vec![],
            genres: vec![],
            publisher: None,
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
}
