use super::config::get_ntfy_config;
use super::ntfy::NtfyClient;
use super::outbox::{claim_pending_batch, mark_delivered, mark_permanently_failed, mark_retry};
use super::types::{NotificationPayload, SendOutcome};
use crate::db::Db;
use crate::error::Result;
use reqwest::Client;
use std::time::Duration;

pub async fn process_outbox_batch(db: &Db, http: &Client) -> Result<usize> {
    let (cfg, batch) = db
        .call(|conn| {
            let config = get_ntfy_config(conn);
            if !config.enabled {
                return Ok((config, Vec::new()));
            }
            let claimed = claim_pending_batch(conn, 10)?;
            Ok((config, claimed))
        })
        .await?;

    if batch.is_empty() {
        return Ok(0);
    }

    let processed_count = batch.len();
    let client = NtfyClient::new(
        http.clone(),
        cfg.server_url.clone(),
        cfg.topic.clone(),
        cfg.token.clone(),
    );

    for entry in batch {
        let payload: NotificationPayload = match serde_json::from_str(&entry.payload_json) {
            Ok(p) => p,
            Err(e) => {
                let err_msg = format!("Malformed payload_json in outbox {}: {e}", entry.id);
                tracing::error!("{err_msg}");
                let id = entry.id;
                db.call(move |conn| mark_permanently_failed(conn, id, &err_msg))
                    .await?;
                continue;
            }
        };

        let outcome = client.send_payload(&payload).await.unwrap_or_else(|e| {
            SendOutcome::Retryable(format!("Internal client error: {e}"))
        });

        let id = entry.id;
        let event_id = entry.event_id;
        let attempts = entry.attempts;

        match outcome {
            SendOutcome::Success => {
                tracing::info!(
                    "Delivered notification outbox id {} (event {}) via ntfy",
                    id,
                    event_id
                );
                db.call(move |conn| mark_delivered(conn, id, event_id))
                    .await?;
            }
            SendOutcome::Retryable(err) => {
                tracing::warn!(
                    "Transient failure delivering notification outbox id {}: {err}",
                    id
                );
                db.call(move |conn| mark_retry(conn, id, attempts, &err))
                    .await?;
            }
            SendOutcome::Permanent(err) => {
                tracing::error!(
                    "Permanent failure delivering notification outbox id {}: {err}",
                    id
                );
                db.call(move |conn| mark_permanently_failed(conn, id, &err))
                    .await?;
            }
        }
    }

    Ok(processed_count)
}

pub async fn run_worker_forever(db: Db, http: Client) {
    tracing::info!("Starting notification outbox background worker");
    loop {
        if let Err(e) = process_outbox_batch(&db, &http).await {
            tracing::warn!("Error processing notification outbox batch: {e}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
