use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncTarget {
    AniList,
    MangaBaka,
}

impl SyncTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AniList => "anilist",
            Self::MangaBaka => "mangabaka",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "anilist" => Some(Self::AniList),
            "mangabaka" => Some(Self::MangaBaka),
            _ => None,
        }
    }
}

impl std::fmt::Display for SyncTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncResultCategory {
    Updated,
    NoChange,
    DryRunWouldUpdate,
    SkippedNoMapping,
    SkippedAmbiguousMapping,
    SkippedUnsafeProgress,
    TrackerAhead,
    AuthError,
    NetworkError,
    RateLimited,
    RemoteError,
}

impl SyncResultCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::NoChange => "no_change",
            Self::DryRunWouldUpdate => "dry_run_would_update",
            Self::SkippedNoMapping => "skipped_no_mapping",
            Self::SkippedAmbiguousMapping => "skipped_ambiguous_mapping",
            Self::SkippedUnsafeProgress => "skipped_unsafe_progress",
            Self::TrackerAhead => "tracker_ahead",
            Self::AuthError => "auth_error",
            Self::NetworkError => "network_error",
            Self::RateLimited => "rate_limited",
            Self::RemoteError => "remote_error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "updated" => Some(Self::Updated),
            "no_change" => Some(Self::NoChange),
            "dry_run_would_update" => Some(Self::DryRunWouldUpdate),
            "skipped_no_mapping" => Some(Self::SkippedNoMapping),
            "skipped_ambiguous_mapping" => Some(Self::SkippedAmbiguousMapping),
            "skipped_unsafe_progress" => Some(Self::SkippedUnsafeProgress),
            "tracker_ahead" => Some(Self::TrackerAhead),
            "auth_error" => Some(Self::AuthError),
            "network_error" => Some(Self::NetworkError),
            "rate_limited" => Some(Self::RateLimited),
            "remote_error" => Some(Self::RemoteError),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaBookProgress {
    pub id: String,
    pub name: String,
    pub number: f64,
    pub volume: Option<f64>,
    pub chapter: Option<f64>,
    pub completed: bool,
    pub page: i64,
    pub read_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaSeriesProgress {
    pub series_id: String,
    pub series_title: String,
    pub library_id: String,
    pub total_books: usize,
    pub completed_books: usize,
    pub highest_completed_book: Option<f64>,
    pub highest_completed_volume: Option<f64>,
    pub highest_completed_chapter: Option<f64>,
    pub is_series_completed: bool,
    pub last_read_date: Option<String>,
    pub books: Vec<KomgaBookProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerMapping {
    pub komga_series_id: String,
    pub tracker: SyncTarget,
    pub tracker_id: String,
    pub tracker_title: Option<String>,
    pub manual_override: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    pub id: String,
    pub timestamp: i64,
    pub komga_series_id: String,
    pub komga_series_title: String,
    pub tracker: SyncTarget,
    pub tracker_media_id: Option<String>,
    pub old_progress: Option<f64>,
    pub new_progress: Option<f64>,
    pub status_change: Option<String>,
    pub category: SyncResultCategory,
    pub message: String,
    pub is_dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub total_series: usize,
    pub updated: usize,
    pub no_change: usize,
    pub dry_run_updates: usize,
    pub skipped: usize,
    pub errors: usize,
    pub is_dry_run: bool,
    pub records: Vec<SyncRecord>,
}
