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

/// Evaluates whether a series strictly satisfies the conservative completion predicate.
///
/// Strict invariants:
/// 1. Authoritative total units must be known and > 0.
/// 2. Target progress must reach or exceed total units.
/// 3. Cannot be inferred from partial local ownership (e.g. read 10 of 10 owned when total is 20).
/// 4. If local books are present, completed books must equal total books and total books must be >= total_units.
/// 5. Contiguous reading: no missing intermediate units from 1 to total_units.
pub fn evaluate_completion_predicate(
    progress: &KomgaSeriesProgress,
    total_units: i64,
    target_prog: f64,
) -> bool {
    if total_units <= 0 {
        return false;
    }

    if target_prog < total_units as f64 {
        return false;
    }

    // If local books are present, verify that local ownership is not merely partial
    if progress.total_books > 0 {
        // If user owns fewer books than the known total, they do NOT own the full work
        if (progress.total_books as i64) < total_units {
            return false;
        }

        // All owned books must be completed
        if progress.completed_books < progress.total_books {
            return false;
        }

        // Check for missing intermediate books if book progress details are present
        if !progress.books.is_empty() {
            let mut book_nums: Vec<i64> = progress
                .books
                .iter()
                .filter(|b| b.completed)
                .map(|b| b.number.floor() as i64)
                .collect();
            book_nums.sort_unstable();
            book_nums.dedup();

            for expected in 1..=total_units {
                if !book_nums.contains(&expected) {
                    return false;
                }
            }
        }
    } else if !progress.is_series_completed {
        // If no local books present, requires explicit completed status from Komga
        return false;
    }

    true
}
