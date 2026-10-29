use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationEventType {
    #[serde(rename = "NEW_VOLUME")]
    NewVolume,
    #[serde(rename = "RELEASE_AVAILABLE")]
    ReleaseAvailable,
    #[serde(rename = "ACQUISITION_COMPLETE")]
    AcquisitionComplete,
    #[serde(rename = "ACTION_REQUIRED")]
    ActionRequired,
}

impl NotificationEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewVolume => "NEW_VOLUME",
            Self::ReleaseAvailable => "RELEASE_AVAILABLE",
            Self::AcquisitionComplete => "ACQUISITION_COMPLETE",
            Self::ActionRequired => "ACTION_REQUIRED",
        }
    }
}

impl fmt::Display for NotificationEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for NotificationEventType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "NEW_VOLUME" => Ok(Self::NewVolume),
            "RELEASE_AVAILABLE" => Ok(Self::ReleaseAvailable),
            "ACQUISITION_COMPLETE" => Ok(Self::AcquisitionComplete),
            "ACTION_REQUIRED" => Ok(Self::ActionRequired),
            _ => Err(format!("Unknown event type: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationPayload {
    pub topic: String,
    pub title: String,
    pub message: String,
    pub tags: Vec<String>,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "sending")]
    Sending,
    #[serde(rename = "delivered")]
    Delivered,
    #[serde(rename = "retry_wait")]
    RetryWait,
    #[serde(rename = "permanently_failed")]
    PermanentlyFailed,
    #[serde(rename = "suppressed")]
    Suppressed,
}

impl OutboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Delivered => "delivered",
            Self::RetryWait => "retry_wait",
            Self::PermanentlyFailed => "permanently_failed",
            Self::Suppressed => "suppressed",
        }
    }
}

impl fmt::Display for OutboxStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for OutboxStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "sending" => Ok(Self::Sending),
            "delivered" => Ok(Self::Delivered),
            "retry_wait" => Ok(Self::RetryWait),
            "permanently_failed" => Ok(Self::PermanentlyFailed),
            "suppressed" => Ok(Self::Suppressed),
            _ => Err(format!("Unknown outbox status: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEntry {
    pub id: i64,
    pub event_id: i64,
    pub event_type: String,
    pub transport: String,
    pub destination: String,
    pub payload_json: String,
    pub status: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub last_attempt_at: Option<i64>,
    pub delivered_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtfyConfig {
    pub enabled: bool,
    pub server_url: String,
    pub topic: String,
    pub token: Option<String>,
    pub pref_new_volume: bool,
    pub pref_release_available: bool,
    pub pref_acquisition_complete: bool,
    pub pref_action_required: bool,
    pub activated_at: Option<i64>,
}

impl Default for NtfyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: "https://ntfy.sh".to_string(),
            topic: String::new(),
            token: None,
            pref_new_volume: true,
            pref_release_available: true,
            pref_acquisition_complete: true,
            pref_action_required: true,
            activated_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NtfyConfigDto {
    pub enabled: bool,
    pub server_url: String,
    pub topic: String,
    pub has_token: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub pref_new_volume: bool,
    pub pref_release_available: bool,
    pub pref_acquisition_complete: bool,
    pub pref_action_required: bool,
    pub activated_at: Option<i64>,
}

impl From<&NtfyConfig> for NtfyConfigDto {
    fn from(c: &NtfyConfig) -> Self {
        Self {
            enabled: c.enabled,
            server_url: c.server_url.clone(),
            topic: c.topic.clone(),
            has_token: c.token.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false),
            token: None, // Never expose secret token to frontend
            pref_new_volume: c.pref_new_volume,
            pref_release_available: c.pref_release_available,
            pref_acquisition_complete: c.pref_acquisition_complete,
            pref_action_required: c.pref_action_required,
            activated_at: c.activated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Success,
    Retryable(String),
    Permanent(String),
}
