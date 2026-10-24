//! Native asynchronous client for Komga library management, discovery, and scan triggers.
//!
//! Provides read-only connection verification, library discovery, conservative series/book queries,
//! and library rescan orchestration so that freshly acquired and published manga are immediately indexed.

use crate::error::{AppError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub const DEFAULT_KOMGA_URL: &str = "http://127.0.0.1:25600";

#[derive(Debug, Clone, Deserialize)]
pub struct KomgaUser {
    pub id: Option<String>,
    pub email: Option<String>,
    #[allow(dead_code)]
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaLibrary {
    pub id: String,
    pub name: String,
    pub root: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaBook {
    pub id: String,
    pub series_id: String,
    pub name: String,
    pub number: f64,
    pub metadata: Option<KomgaBookMetadata>,
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
            format!("{}/{}", self.base_url.trim_end_matches('/'), endpoint.trim_start_matches('/'))
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
    /// Never mutates Komga state or triggers scans.
    pub async fn test_connection(&self) -> Result<String> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/api/v1/users/me")
            .send()
            .await
            .map_err(|e| AppError::new(502, "komga_offline", format!("Cannot reach Komga at {}: {e}", self.base_url)))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(
                status.as_u16(),
                "komga_unauthorized",
                "Komga authentication failed: invalid API key or credentials",
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

        let user: Option<KomgaUser> = resp.json().await.ok();
        let user_id = user
            .as_ref()
            .and_then(|u| u.email.as_deref().or(u.id.as_deref()))
            .unwrap_or("authenticated user");

        Ok(format!("Successfully connected to Komga as {user_id}"))
    }

    /// Discovers existing libraries in Komga without triggering scans.
    pub async fn list_libraries(&self) -> Result<Vec<KomgaLibrary>> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/api/v1/libraries")
            .send()
            .await
            .map_err(|e| AppError::new(502, "komga_offline", format!("Cannot fetch Komga libraries: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(status.as_u16(), "komga_unauthorized", "Komga authentication failed"));
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(status.as_u16(), "komga_error", format!("Komga returned HTTP {status}: {body}")));
        }

        let libs: Vec<KomgaLibrary> = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
        Ok(libs)
    }

    /// Searches for series matching a title query in Komga.
    pub async fn search_series(&self, title: &str, library_id: Option<&str>) -> Result<Vec<KomgaSeries>> {
        let mut req = self.request_builder(reqwest::Method::GET, "/api/v1/series");
        req = req.query(&[("search", title)]);
        if let Some(lib_id) = library_id.filter(|s| !s.trim().is_empty()) {
            req = req.query(&[("library_id", lib_id)]);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::new(502, "komga_offline", format!("Cannot search Komga series: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(status.as_u16(), "komga_unauthorized", "Komga authentication failed"));
        }

        if !status.is_success() {
            return Err(AppError::new(status.as_u16(), "komga_error", format!("Komga series search returned HTTP {status}")));
        }

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            content: Vec<KomgaSeries>,
        }

        let page: Page = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
        Ok(page.content)
    }

    /// Fetches all books for a specific series.
    pub async fn get_books_in_series(&self, series_id: &str) -> Result<Vec<KomgaBook>> {
        let endpoint = format!("/api/v1/series/{series_id}/books?size=500");
        let resp = self
            .request_builder(reqwest::Method::GET, &endpoint)
            .send()
            .await
            .map_err(|e| AppError::new(502, "komga_offline", format!("Cannot fetch Komga books: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::new(status.as_u16(), "komga_unauthorized", "Komga authentication failed"));
        }

        if !status.is_success() {
            return Err(AppError::new(status.as_u16(), "komga_error", format!("Komga books fetch returned HTTP {status}")));
        }

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            content: Vec<KomgaBook>,
        }

        let page: Page = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
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
            .map_err(|e| AppError::new(502, "komga_offline", format!("Failed to trigger Komga scan: {e}")))?;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn komga_client_url_formatting() {
        let client = KomgaClient::new(Client::new(), Some("http://localhost:25600/".into()), Some("secret".into()));
        let req = client.request_builder(reqwest::Method::GET, "/api/v1/users/me").build().unwrap();
        assert_eq!(req.url().as_str(), "http://localhost:25600/api/v1/users/me");
        assert_eq!(req.headers().get("X-API-Key").unwrap(), "secret");
        assert_eq!(req.headers().get("X-Auth-Token").unwrap(), "secret");
    }

    #[tokio::test]
    async fn komga_client_mock_api_tests() {
        use axum::{routing::{get, post}, Json, Router};
        use serde_json::json;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let scan_triggered = Arc::new(AtomicBool::new(false));
        let scan_flag = scan_triggered.clone();

        let app = Router::new()
            .route(
                "/api/v1/users/me",
                get(|| async {
                    Json(json!({
                        "id": "user-1",
                        "email": "manga@example.com",
                        "roles": ["ROLE_ADMIN"]
                    }))
                }),
            )
            .route(
                "/api/v1/libraries",
                get(|| async {
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
                    ]))
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
                            axum::http::StatusCode::NO_CONTENT
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

        let client = KomgaClient::new(
            Client::new(),
            Some(format!("http://127.0.0.1:{port}")),
            Some("test-token".into()),
        );

        // 1. Read-only connection check
        let conn_res = client.test_connection().await;
        assert!(conn_res.is_ok());
        assert!(conn_res.unwrap().contains("manga@example.com"));
        assert!(!scan_triggered.load(Ordering::SeqCst), "Validation must NEVER trigger scan");

        // 2. Discover libraries
        let libs = client.list_libraries().await.unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].id, "lib-en");
        assert_eq!(libs[0].name, "Manga English");
        assert_eq!(libs[1].id, "lib-zh");
        assert_eq!(libs[1].name, "Manga Chinese");

        // 3. Query series & books
        let series = client.search_series("Blue Lock", Some("lib-en")).await.unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].id, "series-100");

        let books = client.get_books_in_series("series-100").await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "Blue Lock v01");
        assert_eq!(books[0].number, 1.0);

        // 4. Trigger scan
        assert!(client.trigger_scan(Some("lib-en")).await.is_ok());
        assert!(scan_triggered.load(Ordering::SeqCst), "Targeted scan must fire");
    }
}
