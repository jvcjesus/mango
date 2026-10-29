use super::types::{NtfyConfig, NotificationEventType};
use crate::db::{now, set_state, state};
use rusqlite::Connection;

pub const KEY_NTFY_ENABLED: &str = "notification.ntfy_enabled";
pub const KEY_NTFY_URL: &str = "notification.ntfy_url";
pub const KEY_NTFY_TOPIC: &str = "notification.ntfy_topic";
pub const KEY_NTFY_TOKEN: &str = "notification.ntfy_token";
pub const KEY_PREF_NEW_VOLUME: &str = "notification.pref_new_volume";
pub const KEY_PREF_RELEASE_AVAILABLE: &str = "notification.pref_release_available";
pub const KEY_PREF_ACQUISITION_COMPLETE: &str = "notification.pref_acquisition_complete";
pub const KEY_PREF_ACTION_REQUIRED: &str = "notification.pref_action_required";
pub const KEY_ACTIVATED_AT: &str = "notification.activated_at";

pub fn get_ntfy_config(conn: &Connection) -> NtfyConfig {
    let enabled = state(conn, KEY_NTFY_ENABLED)
        .ok()
        .flatten()
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false);

    let server_url = state(conn, KEY_NTFY_URL)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://ntfy.sh".to_string());

    let topic = state(conn, KEY_NTFY_TOPIC)
        .ok()
        .flatten()
        .unwrap_or_default();

    let token = state(conn, KEY_NTFY_TOKEN)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());

    let pref_new_volume = state(conn, KEY_PREF_NEW_VOLUME)
        .ok()
        .flatten()
        .map(|s| s != "false" && s != "0")
        .unwrap_or(true);

    let pref_release_available = state(conn, KEY_PREF_RELEASE_AVAILABLE)
        .ok()
        .flatten()
        .map(|s| s != "false" && s != "0")
        .unwrap_or(true);

    let pref_acquisition_complete = state(conn, KEY_PREF_ACQUISITION_COMPLETE)
        .ok()
        .flatten()
        .map(|s| s != "false" && s != "0")
        .unwrap_or(true);

    let pref_action_required = state(conn, KEY_PREF_ACTION_REQUIRED)
        .ok()
        .flatten()
        .map(|s| s != "false" && s != "0")
        .unwrap_or(true);

    let activated_at = state(conn, KEY_ACTIVATED_AT)
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok());

    NtfyConfig {
        enabled,
        server_url,
        topic,
        token,
        pref_new_volume,
        pref_release_available,
        pref_acquisition_complete,
        pref_action_required,
        activated_at,
    }
}

pub fn set_ntfy_config(conn: &Connection, cfg: &NtfyConfig) -> crate::error::Result<()> {
    let prev = get_ntfy_config(conn);

    set_state(conn, KEY_NTFY_ENABLED, if cfg.enabled { "true" } else { "false" })?;
    set_state(conn, KEY_NTFY_URL, &cfg.server_url)?;
    set_state(conn, KEY_NTFY_TOPIC, &cfg.topic)?;

    if let Some(ref t) = cfg.token {
        set_state(conn, KEY_NTFY_TOKEN, t)?;
    } else {
        // If explicitly set to None, we only keep or clear if needed.
        // Usually None means do not change if called via update,
        // but if someone passes None in pure config set, we can keep or delete.
    }

    set_state(conn, KEY_PREF_NEW_VOLUME, if cfg.pref_new_volume { "true" } else { "false" })?;
    set_state(conn, KEY_PREF_RELEASE_AVAILABLE, if cfg.pref_release_available { "true" } else { "false" })?;
    set_state(conn, KEY_PREF_ACQUISITION_COMPLETE, if cfg.pref_acquisition_complete { "true" } else { "false" })?;
    set_state(conn, KEY_PREF_ACTION_REQUIRED, if cfg.pref_action_required { "true" } else { "false" })?;

    // Watermark / activation flood protection:
    // If transitioning from disabled to enabled, or if activated_at is unset, update activated_at to current epoch seconds.
    if cfg.enabled {
        if !prev.enabled || prev.activated_at.is_none() {
            let act = now();
            set_state(conn, KEY_ACTIVATED_AT, &act.to_string())?;
        }
    }

    Ok(())
}

pub fn is_event_type_enabled(cfg: &NtfyConfig, event_type: NotificationEventType) -> bool {
    match event_type {
        NotificationEventType::NewVolume => cfg.pref_new_volume,
        NotificationEventType::ReleaseAvailable => cfg.pref_release_available,
        NotificationEventType::AcquisitionComplete => cfg.pref_acquisition_complete,
        NotificationEventType::ActionRequired => cfg.pref_action_required,
    }
}
