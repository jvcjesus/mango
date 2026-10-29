use super::config::{get_ntfy_config, is_event_type_enabled};
use super::types::{NtfyConfig, NotificationEventType, NotificationPayload, OutboxEntry, OutboxStatus};
use crate::db::now;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::str::FromStr;

pub fn format_volume_num(vol: Option<f64>) -> Option<String> {
    vol.map(|v| {
        if (v.fract()).abs() < 0.001 {
            format!("{}", v as i64)
        } else {
            format!("{v}")
        }
    })
}

pub fn build_payload(
    cfg: &NtfyConfig,
    event_type: NotificationEventType,
    series_title: &str,
    vol_num: Option<f64>,
    message: &str,
    manga_id: Option<i64>,
) -> NotificationPayload {
    let vol_str = format_volume_num(vol_num);
    let click = manga_id.map(|id| format!("/title/{id}"));

    match event_type {
        NotificationEventType::NewVolume => {
            let vol_display = vol_str.map(|v| format!(" Vol. {v}")).unwrap_or_default();
            NotificationPayload {
                topic: cfg.topic.clone(),
                title: format!("📚 {series_title}{vol_display}"),
                message: message.to_string(),
                tags: vec!["books".to_string()],
                priority: 3,
                click,
            }
        }
        NotificationEventType::ReleaseAvailable => {
            let vol_display = vol_str.map(|v| format!(" Vol. {v}")).unwrap_or_default();
            NotificationPayload {
                topic: cfg.topic.clone(),
                title: format!("🔎 {series_title}{vol_display}"),
                message: message.to_string(),
                tags: vec!["mag".to_string()],
                priority: 3,
                click,
            }
        }
        NotificationEventType::AcquisitionComplete => {
            let vol_display = vol_str.map(|v| format!(" Vol. {v}")).unwrap_or_default();
            NotificationPayload {
                topic: cfg.topic.clone(),
                title: format!("✅ {series_title}{vol_display}"),
                message: message.to_string(),
                tags: vec!["white_check_mark".to_string()],
                priority: 3,
                click,
            }
        }
        NotificationEventType::ActionRequired => NotificationPayload {
            topic: cfg.topic.clone(),
            title: format!("⚠️ {series_title} needs attention"),
            message: message.to_string(),
            tags: vec!["warning".to_string()],
            priority: 4,
            click,
        },
    }
}

pub fn enqueue_from_event(
    conn: &Connection,
    event_id: i64,
    series_id: &str,
    series_title: &str,
    volume_number: Option<f64>,
    event_type_str: &str,
    message: &str,
) -> Result<Option<i64>> {
    let cfg = get_ntfy_config(conn);

    let event_type = match NotificationEventType::from_str(event_type_str) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };

    // If transport is not enabled, do not enqueue for delivery
    if !cfg.enabled {
        return Ok(None);
    }

    // Historical event flood protection:
    // If event creation predates ntfy activation watermark, suppress it.
    let created_at: i64 = conn
        .query_row(
            "SELECT created_at FROM reconciliation_events WHERE id = ?1",
            [event_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(now);

    let is_historical = match cfg.activated_at {
        Some(act) => created_at <= act,
        None => true,
    };

    let is_pref_enabled = is_event_type_enabled(&cfg, event_type);

    let status = if is_historical {
        OutboxStatus::Suppressed
    } else if !is_pref_enabled {
        OutboxStatus::Suppressed
    } else if cfg.topic.trim().is_empty() {
        OutboxStatus::PermanentlyFailed
    } else {
        OutboxStatus::Pending
    };

    // Resolve manga_id for browser navigation click link
    let manga_id: Option<i64> = if let Some(m_str) = series_id.strip_prefix("manga:") {
        m_str.parse().ok()
    } else {
        conn.query_row(
            "SELECT manga_id FROM series_reconciliations WHERE series_id = ?1",
            [series_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten()
    };

    let payload = build_payload(
        &cfg,
        event_type,
        series_title,
        volume_number,
        message,
        manga_id,
    );
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| AppError::internal(format!("Failed to serialize notification payload: {e}")))?;

    let last_error = if status == OutboxStatus::PermanentlyFailed {
        Some("ntfy topic not configured".to_string())
    } else {
        None
    };

    conn.execute(
        "INSERT OR IGNORE INTO notification_outbox (
            event_id, event_type, transport, destination, payload_json,
            status, attempts, next_attempt_at, last_error, created_at
        ) VALUES (?1, ?2, 'ntfy', ?3, ?4, ?5, 0, ?6, ?7, ?8)",
        params![
            event_id,
            event_type_str,
            &cfg.topic,
            payload_json,
            status.as_str(),
            now(),
            last_error,
            created_at,
        ],
    )?;

    let outbox_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM notification_outbox WHERE event_id = ?1 AND transport = 'ntfy'",
            [event_id],
            |r| r.get(0),
        )
        .optional()?;

    Ok(outbox_id)
}

pub fn claim_pending_batch(conn: &Connection, batch_size: usize) -> Result<Vec<OutboxEntry>> {
    let current_time = now();

    // Reclaim stale claims where worker died/restarted (> 5 minutes ago)
    let stale_threshold = current_time - 300;
    conn.execute(
        "UPDATE notification_outbox
         SET status = 'retry_wait', next_attempt_at = ?1
         WHERE status = 'sending' AND last_attempt_at IS NOT NULL AND last_attempt_at < ?2",
        params![current_time, stale_threshold],
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, event_id, event_type, transport, destination, payload_json,
                status, attempts, next_attempt_at, last_attempt_at, delivered_at,
                last_error, created_at
         FROM notification_outbox
         WHERE status IN ('pending', 'retry_wait') AND next_attempt_at <= ?1
         ORDER BY id ASC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![current_time, batch_size as i64], |r| {
            Ok(OutboxEntry {
                id: r.get(0)?,
                event_id: r.get(1)?,
                event_type: r.get(2)?,
                transport: r.get(3)?,
                destination: r.get(4)?,
                payload_json: r.get(5)?,
                status: r.get(6)?,
                attempts: r.get(7)?,
                next_attempt_at: r.get(8)?,
                last_attempt_at: r.get(9)?,
                delivered_at: r.get(10)?,
                last_error: r.get(11)?,
                created_at: r.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut claimed = Vec::new();
    for mut item in rows {
        item.attempts += 1;
        item.last_attempt_at = Some(current_time);
        item.status = "sending".to_string();

        let updated = conn.execute(
            "UPDATE notification_outbox
             SET status = 'sending', last_attempt_at = ?1, attempts = attempts + 1
             WHERE id = ?2 AND status IN ('pending', 'retry_wait')",
            params![current_time, item.id],
        )?;

        if updated > 0 {
            claimed.push(item);
        }
    }

    Ok(claimed)
}

pub fn mark_delivered(conn: &Connection, id: i64, event_id: i64) -> Result<()> {
    let t = now();
    conn.execute(
        "UPDATE notification_outbox
         SET status = 'delivered', delivered_at = ?1, last_error = NULL
         WHERE id = ?2",
        params![t, id],
    )?;

    conn.execute(
        "UPDATE reconciliation_events
         SET notified_at = ?1
         WHERE id = ?2",
        params![t, event_id],
    )?;

    Ok(())
}

pub fn mark_retry(conn: &Connection, id: i64, attempts: i64, error_msg: &str) -> Result<()> {
    let t = now();
    // Max attempts is 5; if exceeded, permanently fail
    if attempts >= 5 {
        mark_permanently_failed(
            conn,
            id,
            &format!("Max retry attempts reached ({}): {}", attempts, error_msg),
        )
    } else {
        // Exponential backoff: 30s, 60s, 120s, 240s
        let shift = (attempts.saturating_sub(1)).min(6) as u32;
        let delay_secs = 30 * (1 << shift);
        let next_at = t + delay_secs;

        conn.execute(
            "UPDATE notification_outbox
             SET status = 'retry_wait', next_attempt_at = ?1, last_error = ?2
             WHERE id = ?3",
            params![next_at, error_msg, id],
        )?;

        Ok(())
    }
}

pub fn mark_permanently_failed(conn: &Connection, id: i64, error_msg: &str) -> Result<()> {
    conn.execute(
        "UPDATE notification_outbox
         SET status = 'permanently_failed', last_error = ?1
         WHERE id = ?2",
        params![error_msg, id],
    )?;
    Ok(())
}

pub fn list_recent_outbox(conn: &Connection, limit: usize) -> Result<Vec<OutboxEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_id, event_type, transport, destination, payload_json,
                status, attempts, next_attempt_at, last_attempt_at, delivered_at,
                last_error, created_at
         FROM notification_outbox
         ORDER BY id DESC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(OutboxEntry {
                id: r.get(0)?,
                event_id: r.get(1)?,
                event_type: r.get(2)?,
                transport: r.get(3)?,
                destination: r.get(4)?,
                payload_json: r.get(5)?,
                status: r.get(6)?,
                attempts: r.get(7)?,
                next_attempt_at: r.get(8)?,
                last_attempt_at: r.get(9)?,
                delivered_at: r.get(10)?,
                last_error: r.get(11)?,
                created_at: r.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(rows)
}
