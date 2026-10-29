use super::config::{get_ntfy_config, set_ntfy_config};
use super::ntfy::NtfyClient;
use super::outbox::list_recent_outbox;
use super::types::{NtfyConfigDto, OutboxEntry};
use crate::db::Db;
use crate::error::{AppError, Result};
use reqwest::Client;
use serde::Deserialize;

#[derive(Clone)]
pub struct NotificationService {
    db: Db,
    http: Client,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNtfyConfigRequest {
    pub enabled: bool,
    pub server_url: String,
    pub topic: String,
    pub token: Option<String>,
    pub pref_new_volume: bool,
    pub pref_release_available: bool,
    pub pref_acquisition_complete: bool,
    pub pref_action_required: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestNotificationRequest {
    pub server_url: Option<String>,
    pub topic: Option<String>,
    pub token: Option<String>,
}

impl NotificationService {
    pub fn new(db: Db, http: Client) -> Self {
        Self { db, http }
    }

    pub async fn get_config(&self) -> Result<NtfyConfigDto> {
        self.db
            .call(|conn| {
                let cfg = get_ntfy_config(conn);
                Ok(NtfyConfigDto::from(&cfg))
            })
            .await
    }

    pub async fn save_config(&self, req: UpdateNtfyConfigRequest) -> Result<NtfyConfigDto> {
        let server_url = if req.server_url.trim().is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            req.server_url.trim().to_string()
        };

        self.db
            .call(move |conn| {
                let mut current = get_ntfy_config(conn);
                current.enabled = req.enabled;
                current.server_url = server_url;
                current.topic = req.topic.trim().to_string();

                if let Some(t) = req.token {
                    let trimmed = t.trim();
                    if trimmed.is_empty() {
                        current.token = None;
                    } else if trimmed != "****" {
                        current.token = Some(trimmed.to_string());
                    }
                }

                current.pref_new_volume = req.pref_new_volume;
                current.pref_release_available = req.pref_release_available;
                current.pref_acquisition_complete = req.pref_acquisition_complete;
                current.pref_action_required = req.pref_action_required;

                set_ntfy_config(conn, &current)?;
                let updated = get_ntfy_config(conn);
                Ok(NtfyConfigDto::from(&updated))
            })
            .await
    }

    pub async fn send_test(&self, req: Option<TestNotificationRequest>) -> Result<()> {
        let saved_cfg = self
            .db
            .call(|conn| Ok(get_ntfy_config(conn)))
            .await?;

        let (server_url, topic, token) = if let Some(r) = req {
            let s_url = r
                .server_url
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(saved_cfg.server_url);
            let top = r
                .topic
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(saved_cfg.topic);
            let tok = match r.token {
                Some(ref t) if t == "****" => saved_cfg.token,
                Some(ref t) if t.trim().is_empty() => None,
                Some(t) => Some(t),
                None => saved_cfg.token,
            };
            (s_url, top, tok)
        } else {
            (saved_cfg.server_url, saved_cfg.topic, saved_cfg.token)
        };

        if topic.trim().is_empty() {
            return Err(AppError::bad_request("ntfy topic cannot be empty"));
        }

        let client = NtfyClient::new(self.http.clone(), server_url, topic, token);
        client.send_test().await
    }

    pub async fn get_outbox(&self, limit: usize) -> Result<Vec<OutboxEntry>> {
        let lim = limit.clamp(1, 200);
        self.db
            .call(move |conn| list_recent_outbox(conn, lim))
            .await
    }
}
