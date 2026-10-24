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
pub enum MappingSource {
    Manual,
    ExistingMapping,
    MangoTracker,
    ComicInfo,
    CrossReference,
    ExactTitle,
    Metadata,
    Unknown,
}

impl MappingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ExistingMapping => "existing_mapping",
            Self::MangoTracker => "mango_tracker",
            Self::ComicInfo => "comic_info",
            Self::CrossReference => "cross_reference",
            Self::ExactTitle => "exact_title",
            Self::Metadata => "metadata",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "manual" => Self::Manual,
            "existing_mapping" => Self::ExistingMapping,
            "mango_tracker" => Self::MangoTracker,
            "comic_info" => Self::ComicInfo,
            "cross_reference" => Self::CrossReference,
            "exact_title" => Self::ExactTitle,
            "metadata" => Self::Metadata,
            _ => Self::Unknown,
        }
    }
}

impl Default for MappingSource {
    fn default() -> Self {
        Self::Manual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingConfidence {
    Resolved,
    Ambiguous,
    NeedsMapping,
    NotFound,
}

impl MappingConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Ambiguous => "ambiguous",
            Self::NeedsMapping => "needs_mapping",
            Self::NotFound => "not_found",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "resolved" => Self::Resolved,
            "ambiguous" => Self::Ambiguous,
            "needs_mapping" => Self::NeedsMapping,
            _ => Self::NotFound,
        }
    }
}

impl Default for MappingConfidence {
    fn default() -> Self {
        Self::Resolved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncResultCategory {
    Updated,
    WouldUpdate,
    NoChange,
    TrackerAhead,
    SkippedUntouched,
    SkippedNoMapping,
    AmbiguousMapping,
    SkippedUnsafeProgress,
    AuthError,
    NetworkError,
    Unsupported,
    VerificationFailed,
    RateLimited,
    RemoteError,
}

impl SyncResultCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::WouldUpdate => "would_update",
            Self::NoChange => "no_change",
            Self::TrackerAhead => "tracker_ahead",
            Self::SkippedUntouched => "skipped_untouched",
            Self::SkippedNoMapping => "skipped_no_mapping",
            Self::AmbiguousMapping => "ambiguous_mapping",
            Self::SkippedUnsafeProgress => "skipped_unsafe_progress",
            Self::AuthError => "auth_error",
            Self::NetworkError => "network_error",
            Self::Unsupported => "unsupported",
            Self::VerificationFailed => "verification_failed",
            Self::RateLimited => "rate_limited",
            Self::RemoteError => "remote_error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "updated" => Some(Self::Updated),
            "would_update" | "dry_run_would_update" => Some(Self::WouldUpdate),
            "no_change" => Some(Self::NoChange),
            "tracker_ahead" => Some(Self::TrackerAhead),
            "skipped_untouched" => Some(Self::SkippedUntouched),
            "skipped_no_mapping" => Some(Self::SkippedNoMapping),
            "ambiguous_mapping" | "skipped_ambiguous_mapping" => Some(Self::AmbiguousMapping),
            "skipped_unsafe_progress" => Some(Self::SkippedUnsafeProgress),
            "auth_error" => Some(Self::AuthError),
            "network_error" => Some(Self::NetworkError),
            "unsupported" => Some(Self::Unsupported),
            "verification_failed" => Some(Self::VerificationFailed),
            "rate_limited" => Some(Self::RateLimited),
            "remote_error" | "error" => Some(Self::RemoteError),
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
    #[serde(default)]
    pub edition_id: Option<String>,
    #[serde(default)]
    pub mapping_source: MappingSource,
    #[serde(default)]
    pub confidence: MappingConfidence,
    #[serde(default)]
    pub manual_override: bool,
    #[serde(default)]
    pub last_validated: Option<i64>,
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
    pub dimension: Option<String>, // "volume" | "chapter"
    pub status_change: Option<String>,
    pub category: SyncResultCategory,
    pub message: String,
    pub is_dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub total_series: usize,
    pub inspected_series: usize,
    pub untouched_series: usize,
    pub eligible_series: usize,
    pub anilist_mapped: usize,
    pub mangabaka_mapped: usize,
    pub unresolved_mappings: usize,
    pub ambiguous_mappings: usize,
    pub updated: usize,
    pub no_change: usize,
    pub tracker_ahead: usize,
    pub dry_run_updates: usize,
    pub skipped: usize,
    pub errors: usize,
    pub is_dry_run: bool,
    pub records: Vec<SyncRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedTitleDiagnostic {
    pub komga_series_id: String,
    pub komga_title: String,
    pub tracker: SyncTarget,
    pub reason: String,
    pub candidates: Vec<String>,
    pub confidence: MappingConfidence,
    pub source: MappingSource,
}
