use super::types::{NotificationPayload, SendOutcome};
use crate::error::{AppError, Result};
use reqwest::Client;
use std::time::Duration;

#[derive(Clone)]
pub struct NtfyClient {
    http: Client,
    server_url: String,
    topic: String,
    token: Option<String>,
}

impl NtfyClient {
    pub fn new(http: Client, server_url: String, topic: String, token: Option<String>) -> Self {
        Self {
            http,
            server_url: server_url.trim_end_matches('/').to_string(),
            topic: topic.trim().to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    pub async fn send_payload(&self, payload: &NotificationPayload) -> Result<SendOutcome> {
        let topic = if !payload.topic.is_empty() {
            &payload.topic
        } else {
            &self.topic
        };

        if topic.is_empty() {
            return Ok(SendOutcome::Permanent(
                "ntfy topic is not configured".to_string(),
            ));
        }

        let base_url = if self.server_url.is_empty() {
            "https://ntfy.sh"
        } else {
            &self.server_url
        };

        // Post JSON to server root; ntfy routes by the "topic" field in the body.
        let url = format!("{base_url}/");

        let mut req = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(10))
            .json(&serde_json::json!({
                "topic": topic,
                "title": payload.title,
                "message": payload.message,
                "tags": payload.tags,
                "priority": payload.priority,
                "click": payload.click,
            }));

        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("Network error sending to ntfy: {e}");
                tracing::warn!("{err_msg}");
                return Ok(SendOutcome::Retryable(err_msg));
            }
        };

        let status = resp.status();
        if status.is_success() {
            Ok(SendOutcome::Success)
        } else if status.as_u16() == 429 {
            let msg = "Rate limited by ntfy (HTTP 429)".to_string();
            tracing::warn!("{msg}");
            Ok(SendOutcome::Retryable(msg))
        } else if status.is_server_error() {
            let msg = format!("ntfy server error: HTTP {status}");
            tracing::warn!("{msg}");
            Ok(SendOutcome::Retryable(msg))
        } else {
            let body = resp.text().await.unwrap_or_default();
            let msg = format!("ntfy rejected notification: HTTP {status} - {body}");
            tracing::warn!("{msg}");
            Ok(SendOutcome::Permanent(msg))
        }
    }

    pub async fn send_test(&self) -> Result<()> {
        if self.topic.is_empty() {
            return Err(AppError::bad_request("ntfy topic cannot be empty"));
        }

        let payload = NotificationPayload {
            topic: self.topic.clone(),
            title: "🥭 Mango Test Notification".to_string(),
            message: "ntfy integration is working correctly.".to_string(),
            tags: vec!["mango".to_string(), "white_check_mark".to_string()],
            priority: 3,
            click: None,
        };

        match self.send_payload(&payload).await? {
            SendOutcome::Success => Ok(()),
            SendOutcome::Retryable(e) | SendOutcome::Permanent(e) => {
                Err(AppError::internal(format!("Failed to send test notification: {e}")))
            }
        }
    }
}
