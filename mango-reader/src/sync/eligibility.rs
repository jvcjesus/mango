//! Evaluates synchronization eligibility for a Komga series.
//!
//! Enforces:
//! - Untouched series (0 completed books, 0 read progress, no read date) are skipped (`SkippedUntouched`)
//!   to avoid expensive tracker queries and protect rate limits.
//! - Partial read progress within a book (e.g. page 50 of 200 without completion) does not mark a book completed.

use super::model::KomgaSeriesProgress;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEligibility {
    Eligible,
    Untouched,
}

impl SyncEligibility {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible)
    }
}

/// Evaluates whether a series has any completed reading progress eligible for tracker sync.
pub fn evaluate_eligibility(progress: &KomgaSeriesProgress) -> SyncEligibility {
    // A series is untouched if it has 0 completed books AND no completed volume/chapter progress
    let has_completed_books = progress.completed_books > 0;
    let has_completed_volume = progress.highest_completed_volume.is_some_and(|v| v > 0.0);
    let has_completed_chapter = progress.highest_completed_chapter.is_some_and(|c| c > 0.0);
    let has_read_date = progress.last_read_date.is_some();

    if !has_completed_books && !has_completed_volume && !has_completed_chapter && !has_read_date {
        SyncEligibility::Untouched
    } else {
        SyncEligibility::Eligible
    }
}
