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

    fn request_builder(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            let base = self.base_url.trim_end_matches('/');
            let clean = endpoint.trim_start_matches('/');
            if base.ends_with("/v1") && clean.starts_with("v1/") {
                format!("{base}/{}", &clean[3..])
            } else {
                format!("{base}/{clean}")
            }
        };

        let mut req = self
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mango/1.0");

        if let Some(token) = &self.token {
            // MangaBaka API strictly forbids sending both `Authorization` and `x-api-key` simultaneously
            // (returning HTTP 400 `Cannot use both Authorization and x-api-key headers simultaneously`).
            // Production MangaBaka API authentication uses solely the `x-api-key` header.
            req = req.header("x-api-key", token);
        }

        req
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
            .request_builder(reqwest::Method::GET, &endpoint)
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

        // Support top-level and data-nested structures
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
            .request_builder(reqwest::Method::GET, &endpoint)
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
            .get("status")
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

    /// Mutates the authenticated user's library progress on MangaBaka.
    ///
    /// Calls `PATCH /v1/my/library/{mangabaka_id}` with dimension-specific payload:
    /// `{"progress_volume": completed}` or `{"progress_chapter": completed}`.
    pub async fn update_my_library_progress(
        &self,
        mangabaka_id: i64,
        progress_volume: Option<f64>,
        progress_chapter: Option<f64>,
    ) -> Result<()> {
        let endpoint = format!("/my/library/{mangabaka_id}");
        let mut body = serde_json::Map::new();
        if let Some(vol) = progress_volume {
            body.insert("progress_volume".to_string(), serde_json::json!(vol));
        }
        if let Some(ch) = progress_chapter {
            body.insert("progress_chapter".to_string(), serde_json::json!(ch));
        }

        let resp = match self
            .request_builder(reqwest::Method::PATCH, &endpoint)
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

    /// Tests whether MangaBaka is reachable and answering.
    pub async fn test_connection(&self) -> Result<String> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/series/search")
            .query(&[("query", "test"), ("q", "test")])
            .send()
            .await
            .map_err(|e| {
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

        Ok("MangaBaka API connection and authentication successful".to_string())
    }

    /// Resolves an exact series match on MangaBaka, failing closed if ambiguous or missing.
    pub async fn match_series(&self, query: &str) -> Result<MangaBakaSeriesMatch> {
        let trimmed_series = query.trim();
        if trimmed_series.is_empty() {
            return Ok(MangaBakaSeriesMatch::NotFound);
        }

        let wanted_norm = normalize_title(trimmed_series);

        let resp = match self
            .request_builder(reqwest::Method::GET, "/series/search")
            .query(&[("query", trimmed_series), ("q", trimmed_series)])
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
            .request_builder(reqwest::Method::GET, &collections_url)
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
            .request_builder(reqwest::Method::GET, &works_url)
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
            .request_builder(reqwest::Method::GET, &series_url)
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

            let w_resp = match self
                .request_builder(reqwest::Method::GET, &url)
                .send()
                .await
            {
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
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/series/search",
                get(
                    |axum::extract::Query(params): axum::extract::Query<
                        std::collections::HashMap<String, String>,
                    >| async move {
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
                        } else if q == "Ambiguous Title" {
                            Json(json!({
                                "data": [
                                    { "id": 201, "title": "Ambiguous Title" },
                                    { "id": 202, "title": "Ambiguous Title" }
                                ]
                            }))
                        } else {
                            Json(json!({ "data": [] }))
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
