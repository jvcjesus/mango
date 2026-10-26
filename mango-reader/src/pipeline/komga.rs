//! Native asynchronous client for Komga library management, discovery, and scan triggers.
//!
//! Provides read-only connection verification, library discovery, conservative series/book queries,
//! and library rescan orchestration so that freshly acquired and published manga are immediately indexed.

use crate::error::{AppError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub const DEFAULT_KOMGA_URL: &str = "http://127.0.0.1:25600";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaLibrary {
    pub id: String,
    pub name: String,
    pub root: String,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaSeries {
    pub id: String,
    pub name: String,
    pub library_id: String,
    pub books_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaBookMetadata {
    pub title: Option<String>,
    pub number: Option<String>,
    pub number_sort: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KomgaReadProgress {
    pub page: i64,
    pub completed: bool,
    #[serde(default)]
    pub read_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaBook {
    pub id: String,
    pub series_id: String,
    pub name: String,
    pub number: f64,
    pub metadata: Option<KomgaBookMetadata>,
    #[serde(default)]
    pub read_progress: Option<KomgaReadProgress>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct KomgaClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl KomgaClient {
    pub fn new(http: Client, base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_KOMGA_URL.to_string()),
            api_key: api_key.filter(|t| !t.trim().is_empty()),
        }
    }

    fn request_builder(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            format!(
                "{}/{}",
                self.base_url.trim_end_matches('/'),
                endpoint.trim_start_matches('/')
            )
        };

        let mut req = self
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mango/1.0");

        if let Some(key) = &self.api_key {
            req = req.header("X-Auth-Token", key).header("X-API-Key", key);
        }

        req
    }

    /// Read-only connection verification.
    ///
    /// Never mutates Komga state or triggers scans. Validates connectivity and credentials
    /// against `/api/v1/libraries`, ensuring compatibility with API key authentication.
    pub async fn test_connection(&self) -> Result<String> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/api/v1/libraries")
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Cannot reach Komga at {}: {e}", self.base_url),
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AppError::new(
                401,
                "komga_unauthorized",
                "Komga authentication failed: invalid or missing API key",
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                403,
                "komga_forbidden",
                "Komga authorization failed: API key does not have permission",
            ));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::new(
                404,
                "komga_not_found",
                format!(
                    "Komga endpoint /api/v1/libraries not found at {}",
                    self.base_url
                ),
            ));
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga returned HTTP {status}: {body}"),
            ));
        }

        let library_count = resp.json::<Vec<KomgaLibrary>>().await.map(|l| l.len()).ok();

        let msg = match library_count {
            Some(count) => format!(
                "Successfully connected to Komga ({} {} accessible)",
                count,
                if count == 1 { "library" } else { "libraries" }
            ),
            None => "Successfully connected to Komga".to_string(),
        };

        Ok(msg)
    }

    /// Discovers existing libraries in Komga without triggering scans.
    pub async fn list_libraries(&self) -> Result<Vec<KomgaLibrary>> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/api/v1/libraries")
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Cannot fetch Komga libraries: {e}"),
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AppError::new(
                401,
                "komga_unauthorized",
                "Komga authentication failed: invalid or missing API key",
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                403,
                "komga_forbidden",
                "Komga authorization failed: API key does not have permission",
            ));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::new(
                404,
                "komga_not_found",
                format!(
                    "Komga endpoint /api/v1/libraries not found at {}",
                    self.base_url
                ),
            ));
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga returned HTTP {status}: {body}"),
            ));
        }

        let libs: Vec<KomgaLibrary> = resp
            .json()
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(libs)
    }

    /// Searches for series matching a title query in Komga.
    pub async fn search_series(
        &self,
        title: &str,
        library_id: Option<&str>,
    ) -> Result<Vec<KomgaSeries>> {
        let mut req = self.request_builder(reqwest::Method::GET, "/api/v1/series");
        req = req.query(&[("search", title)]);
        if let Some(lib_id) = library_id.filter(|s| !s.trim().is_empty()) {
            req = req.query(&[("library_id", lib_id)]);
        }

        let resp = req.send().await.map_err(|e| {
            AppError::new(
                502,
                "komga_offline",
                format!("Cannot search Komga series: {e}"),
            )
        })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                status.as_u16(),
                "komga_unauthorized",
                "Komga authentication failed",
            ));
        }

        if !status.is_success() {
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga series search returned HTTP {status}"),
            ));
        }

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            content: Vec<KomgaSeries>,
        }

        let page: Page = resp
            .json()
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(page.content)
    }

    /// Lists series in Komga, optionally filtered by library.
    pub async fn list_series(&self, library_id: Option<&str>) -> Result<Vec<KomgaSeries>> {
        let mut endpoint = "/api/v1/series?size=1000".to_string();
        if let Some(lib_id) = library_id.filter(|s| !s.trim().is_empty()) {
            endpoint.push_str(&format!("&library_id={lib_id}"));
        }
        let resp = self
            .request_builder(reqwest::Method::GET, &endpoint)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Cannot fetch Komga series: {e}"),
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                status.as_u16(),
                "komga_unauthorized",
                "Komga authentication failed",
            ));
        }

        if !status.is_success() {
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga series fetch returned HTTP {status}"),
            ));
        }

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            content: Vec<KomgaSeries>,
        }

        let page: Page = resp
            .json()
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(page.content)
    }

    /// Fetches all books for a specific series.
    pub async fn get_books_in_series(&self, series_id: &str) -> Result<Vec<KomgaBook>> {
        let endpoint = format!("/api/v1/series/{series_id}/books?size=500");
        let resp = self
            .request_builder(reqwest::Method::GET, &endpoint)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Cannot fetch Komga books: {e}"),
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                status.as_u16(),
                "komga_unauthorized",
                "Komga authentication failed",
            ));
        }

        if !status.is_success() {
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga books fetch returned HTTP {status}"),
            ));
        }

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            content: Vec<KomgaBook>,
        }

        let page: Page = resp
            .json()
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(page.content)
    }

    /// Triggers a library scan in Komga.
    ///
    /// If `library_id` is provided, scans that specific library; otherwise scans all libraries.
    pub async fn trigger_scan(&self, library_id: Option<&str>) -> Result<()> {
        let endpoint = match library_id.filter(|s| !s.trim().is_empty()) {
            Some(id) => format!("/api/v1/libraries/{id}/scan"),
            None => "/api/v1/libraries/scan".to_string(),
        };

        let resp = self
            .request_builder(reqwest::Method::POST, &endpoint)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Failed to trigger Komga scan: {e}"),
                )
            })?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NO_CONTENT {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga scan trigger failed with HTTP {status}: {body}"),
            ));
        }

        tracing::info!("KOMGA: library scan triggered successfully ({endpoint})");
        Ok(())
    }

    /// Triggers metadata analysis and refresh for a specific book in Komga.
    pub async fn refresh_book_metadata(&self, book_id: &str) -> Result<()> {
        let endpoint = format!("/api/v1/books/{book_id}/analyze");
        let resp = self
            .request_builder(reqwest::Method::POST, &endpoint)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Failed to refresh Komga book metadata: {e}"),
                )
            })?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NO_CONTENT {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga book analyze failed with HTTP {status}: {body}"),
            ));
        }

        tracing::info!("KOMGA: book metadata refresh triggered ({book_id})");
        Ok(())
    }

    /// Triggers metadata analysis and refresh for a specific series in Komga.
    pub async fn refresh_series_metadata(&self, series_id: &str) -> Result<()> {
        let endpoint = format!("/api/v1/series/{series_id}/analyze");
        let resp = self
            .request_builder(reqwest::Method::POST, &endpoint)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    502,
                    "komga_offline",
                    format!("Failed to refresh Komga series metadata: {e}"),
                )
            })?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NO_CONTENT {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                status.as_u16(),
                "komga_error",
                format!("Komga series analyze failed with HTTP {status}: {body}"),
            ));
        }

        tracing::info!("KOMGA: series metadata refresh triggered ({series_id})");
        Ok(())
    }

    /// Verifies whether a specific canonical release (Volume N or Chapter N) of a series
    /// has been indexed and is visible in Komga.
    pub async fn verify_release_indexed(
        &self,
        series_title: &str,
        library_id: Option<&str>,
        is_volume: bool,
        release_number: f64,
    ) -> Result<bool> {
        let trimmed_title = series_title.trim();
        let wanted_norm = crate::pipeline::mangabaka::normalize_title(trimmed_title);

        let series_candidates = match self.search_series(trimmed_title, library_id).await {
            Ok(s) => s,
            Err(e) => return Err(e),
        };

        let exact_matches: Vec<_> = series_candidates
            .into_iter()
            .filter(|s| {
                s.name.eq_ignore_ascii_case(trimmed_title)
                    || crate::pipeline::mangabaka::normalize_title(&s.name) == wanted_norm
            })
            .collect();

        let matched_series = match exact_matches.first() {
            Some(s) => s,
            None => return Ok(false), // Series folder not yet indexed by Komga
        };

        let books = match self.get_books_in_series(&matched_series.id).await {
            Ok(b) => b,
            Err(e) => return Err(e),
        };

        let series_context = crate::release::determine_series_context(books.iter().map(|b| {
            (
                b.name.as_str(),
                b.metadata.as_ref().and_then(|m| m.title.as_deref()),
            )
        }));

        for b in &books {
            let meta_title = b.metadata.as_ref().and_then(|m| m.title.as_deref());
            let meta_num = b.metadata.as_ref().and_then(|m| m.number.as_deref());

            let (unit, num_opt) = crate::release::classify_komga_book(
                &b.name,
                meta_title,
                meta_num,
                b.number,
                series_context,
            );

            let matched = if is_volume {
                if unit == crate::release::ReleaseUnit::Volume {
                    let vol = num_opt
                        .or_else(|| {
                            b.metadata.as_ref().and_then(|m| {
                                m.number_sort.or_else(|| {
                                    m.number.as_deref().and_then(|n| n.parse::<f64>().ok())
                                })
                            })
                        })
                        .unwrap_or(b.number);
                    (vol - release_number).abs() < 0.001
                } else {
                    false
                }
            } else {
                if unit == crate::release::ReleaseUnit::Chapter {
                    let ch = num_opt
                        .or_else(|| {
                            b.metadata.as_ref().and_then(|m| {
                                m.number_sort.or_else(|| {
                                    m.number.as_deref().and_then(|n| n.parse::<f64>().ok())
                                })
                            })
                        })
                        .unwrap_or(b.number);
                    (ch - release_number).abs() < 0.001
                } else {
                    false
                }
            };

            if matched {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn komga_client_url_formatting() {
        let client = KomgaClient::new(
            Client::new(),
            Some("http://localhost:25600/".into()),
            Some("secret".into()),
        );
        let req = client
            .request_builder(reqwest::Method::GET, "/api/v1/libraries")
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "http://localhost:25600/api/v1/libraries"
        );
        assert_eq!(req.headers().get("X-API-Key").unwrap(), "secret");
        assert_eq!(req.headers().get("X-Auth-Token").unwrap(), "secret");
    }

    #[tokio::test]
    async fn komga_client_mock_api_tests() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::{get, post},
        };
        use serde_json::json;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let scan_triggered = Arc::new(AtomicBool::new(false));
        let scan_flag = scan_triggered.clone();

        let app = Router::new()
            .route(
                "/api/v1/libraries",
                get(|headers: HeaderMap| async move {
                    match headers.get("X-API-Key").and_then(|v| v.to_str().ok()) {
                        Some("test-token") => (
                            StatusCode::OK,
                            Json(json!([
                                {
                                    "id": "lib-en",
                                    "name": "Manga English",
                                    "root": "/data/manga",
                                    "unavailable": false
                                },
                                {
                                    "id": "lib-zh",
                                    "name": "Manga Chinese",
                                    "root": "/data/manga-chinese",
                                    "unavailable": false
                                }
                            ])),
                        )
                            .into_response(),
                        Some("forbidden-token") => (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "message": "Forbidden access" })),
                        )
                            .into_response(),
                        _ => (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({ "message": "Authentication required" })),
                        )
                            .into_response(),
                    }
                }),
            )
            .route(
                "/api/v1/libraries/lib-en/scan",
                post({
                    let scan_flag = scan_flag.clone();
                    move || {
                        let scan_flag = scan_flag.clone();
                        async move {
                            scan_flag.store(true, Ordering::SeqCst);
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-100",
                                "name": "Blue Lock",
                                "libraryId": "lib-en",
                                "booksCount": 10
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-100/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "book-1",
                                "seriesId": "series-100",
                                "name": "Blue Lock v01",
                                "number": 1.0,
                                "metadata": {
                                    "title": "Blue Lock v01",
                                    "number": "1",
                                    "numberSort": 1.0
                                }
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

        let base_url = format!("http://127.0.0.1:{port}");

        // 1. Valid API key + 200 validation
        let valid_client = KomgaClient::new(
            Client::new(),
            Some(base_url.clone()),
            Some("test-token".into()),
        );
        let conn_res = valid_client.test_connection().await;
        assert!(conn_res.is_ok());
        assert_eq!(
            conn_res.unwrap(),
            "Successfully connected to Komga (2 libraries accessible)"
        );
        assert!(
            !scan_triggered.load(Ordering::SeqCst),
            "Validation must NEVER trigger scan"
        );

        // 2. Missing credentials + 401
        let no_auth_client = KomgaClient::new(Client::new(), Some(base_url.clone()), None);
        let err_no_auth = no_auth_client.test_connection().await.unwrap_err();
        assert_eq!(err_no_auth.status, 401);
        assert_eq!(err_no_auth.code, "komga_unauthorized");

        // 3. Invalid credentials + 401
        let invalid_client = KomgaClient::new(
            Client::new(),
            Some(base_url.clone()),
            Some("wrong-key".into()),
        );
        let err_invalid = invalid_client.test_connection().await.unwrap_err();
        assert_eq!(err_invalid.status, 401);
        assert_eq!(err_invalid.code, "komga_unauthorized");

        // 4. Forbidden + 403
        let forbidden_client = KomgaClient::new(
            Client::new(),
            Some(base_url.clone()),
            Some("forbidden-token".into()),
        );
        let err_forbidden = forbidden_client.test_connection().await.unwrap_err();
        assert_eq!(err_forbidden.status, 403);
        assert_eq!(err_forbidden.code, "komga_forbidden");

        // 5. Unsupported / 404 behavior (must not report as invalid credentials)
        let not_found_client = KomgaClient::new(
            Client::new(),
            Some(format!("{base_url}/invalid-subpath")),
            Some("test-token".into()),
        );
        let err_not_found = not_found_client.test_connection().await.unwrap_err();
        assert_eq!(err_not_found.status, 404);
        assert_eq!(err_not_found.code, "komga_not_found");
        assert_ne!(err_not_found.code, "komga_unauthorized");

        // 6. Network failure / offline port
        let dead_client = KomgaClient::new(
            Client::new(),
            Some("http://127.0.0.1:1".into()),
            Some("test-token".into()),
        );
        let err_dead = dead_client.test_connection().await.unwrap_err();
        assert_eq!(err_dead.status, 502);
        assert_eq!(err_dead.code, "komga_offline");

        // 7. Verify functional API behavior (libraries, series, books, scan)
        let libs = valid_client.list_libraries().await.unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].id, "lib-en");
        assert_eq!(libs[0].name, "Manga English");
        assert_eq!(libs[1].id, "lib-zh");
        assert_eq!(libs[1].name, "Manga Chinese");

        let series = valid_client
            .search_series("Blue Lock", Some("lib-en"))
            .await
            .unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].id, "series-100");

        let books = valid_client
            .get_books_in_series("series-100")
            .await
            .unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "Blue Lock v01");
        assert_eq!(books[0].number, 1.0);

        assert!(valid_client.trigger_scan(Some("lib-en")).await.is_ok());
        assert!(
            scan_triggered.load(Ordering::SeqCst),
            "Targeted scan must fire"
        );
    }

    #[tokio::test]
    async fn test_volume_ownership_verification_checks_exact_volume() {
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-blue-lock",
                                "name": "Blue Lock",
                                "libraryId": "lib-en",
                                "booksCount": 2
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-blue-lock/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "book-v1",
                                "seriesId": "series-blue-lock",
                                "name": "Blue Lock v01",
                                "number": 1.0,
                                "metadata": {
                                    "title": "Blue Lock v01",
                                    "number": "1",
                                    "numberSort": 1.0
                                }
                            },
                            {
                                "id": "book-v2",
                                "seriesId": "series-blue-lock",
                                "name": "Blue Lock v02",
                                "number": 2.0,
                                "metadata": {
                                    "title": "Blue Lock v02",
                                    "number": "2",
                                    "numberSort": 2.0
                                }
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

        let client = KomgaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("key".into()),
        );

        // Exact volume 1 exists
        assert!(
            client
                .verify_release_indexed("Blue Lock", Some("lib-en"), true, 1.0)
                .await
                .unwrap()
        );
        // Exact volume 2 exists
        assert!(
            client
                .verify_release_indexed("Blue Lock", Some("lib-en"), true, 2.0)
                .await
                .unwrap()
        );
        // Volume 3 does NOT exist
        assert!(
            !client
                .verify_release_indexed("Blue Lock", Some("lib-en"), true, 3.0)
                .await
                .unwrap()
        );
        // Volume 1.5 does NOT exist
        assert!(
            !client
                .verify_release_indexed("Blue Lock", Some("lib-en"), true, 1.5)
                .await
                .unwrap()
        );
        // Chapter 1 does NOT match Volume 1 (unit mismatch)
        assert!(
            !client
                .verify_release_indexed("Blue Lock", Some("lib-en"), false, 1.0)
                .await
                .unwrap()
        );
        // Other series does NOT exist
        assert!(
            !client
                .verify_release_indexed("Chainsaw Man", Some("lib-en"), true, 1.0)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_chapter_ownership_verification_checks_exact_chapter() {
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "series-dandadan",
                                "name": "Dandadan",
                                "libraryId": "lib-en",
                                "booksCount": 1
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/series-dandadan/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "book-c10",
                                "seriesId": "series-dandadan",
                                "name": "Dandadan ch010",
                                "number": 10.0,
                                "metadata": {
                                    "title": "Dandadan ch010",
                                    "number": "10",
                                    "numberSort": 10.0
                                }
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

        let client = KomgaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("key".into()),
        );

        // Exact chapter 10 exists
        assert!(
            client
                .verify_release_indexed("Dandadan", Some("lib-en"), false, 10.0)
                .await
                .unwrap()
        );
        // Chapter 11 does NOT exist
        assert!(
            !client
                .verify_release_indexed("Dandadan", Some("lib-en"), false, 11.0)
                .await
                .unwrap()
        );
        // Chapter 9 does NOT exist
        assert!(
            !client
                .verify_release_indexed("Dandadan", Some("lib-en"), false, 9.0)
                .await
                .unwrap()
        );
        // Volume 10 does NOT match Chapter 10 (unit mismatch)
        assert!(
            !client
                .verify_release_indexed("Dandadan", Some("lib-en"), true, 10.0)
                .await
                .unwrap()
        );
    }
}
