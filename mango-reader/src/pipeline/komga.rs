//! Native asynchronous client for Komga library management and scan triggers.
//!
//! Provides connection verification and library rescan orchestration so that
//! freshly acquired and published manga are immediately indexed by Komga.

use crate::error::{AppError, Result};
use reqwest::Client;
use serde::Deserialize;

pub const DEFAULT_KOMGA_URL: &str = "http://127.0.0.1:25600";

#[derive(Debug, Clone, Deserialize)]
pub struct KomgaUser {
    pub id: Option<String>,
    pub email: Option<String>,
    #[allow(dead_code)]
    pub roles: Option<Vec<String>>,
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

    /// Tests whether Komga is reachable and authentication succeeds.
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
    }
}
