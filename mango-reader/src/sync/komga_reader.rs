//! Read-only Komga reading-progress evaluator.
//!
//! Queries Komga series and books to inspect read status (`completed`, `page`, `readDate`)
//! and computes normalized series reading progress without mutating Komga state or triggering scans.

use super::model::{KomgaBookProgress, KomgaSeriesProgress};
use crate::{
    error::Result,
    pipeline::komga::{KomgaBook, KomgaClient, KomgaSeries},
    release::parse_volume,
};

/// Parses a chapter number from a book name (e.g. "c001", "ch. 12", "Chapter 45.5").
pub fn parse_chapter_number(name: &str) -> Option<f64> {
    let lower = name.to_lowercase();
    let is_volume = parse_volume(name).is_some();

    for mark in ["chapter", "ch.", "ch ", "c."] {
        if let Some(at) = lower.find(mark) {
            let candidate = lower[at + mark.len()..].trim_start();
            let end = candidate.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(candidate.len());
            if let Ok(num) = candidate[..end].trim_end_matches('.').parse::<f64>() {
                return Some(num);
            }
        }
    }

    // Check for word starting with 'c' followed strictly by digits (e.g. c001, c128)
    for part in lower.split(|c: char| !c.is_alphanumeric() && c != '.') {
        if part.starts_with('c') && part.len() > 1 && part[1..].chars().all(|c| c.is_ascii_digit() || c == '.') {
            if let Ok(num) = part[1..].trim_end_matches('.').parse::<f64>() {
                return Some(num);
            }
        }
    }

    if is_volume {
        return None;
    }

    let num = crate::titles::number_from_name(name);
    if num >= 0.0 {
        Some(num)
    } else {
        None
    }
}

/// Evaluates normalized series reading progress from Komga series metadata and its books.
pub fn evaluate_series_progress(series: &KomgaSeries, books: &[KomgaBook]) -> KomgaSeriesProgress {
    let mut book_progresses = Vec::with_capacity(books.len());
    let mut completed_books = 0;
    let mut highest_completed_book: Option<f64> = None;
    let mut highest_completed_volume: Option<f64> = None;
    let mut highest_completed_chapter: Option<f64> = None;
    let mut last_read_date: Option<String> = None;

    // Determine if the series is predominantly volume-based or chapter-based
    let volume_candidates = books.iter().filter_map(|b| parse_volume(&b.name)).count();
    let chapter_candidates = books.iter().filter_map(|b| parse_chapter_number(&b.name)).count();
    let is_volume_series = volume_candidates > 0 && volume_candidates >= chapter_candidates;

    for book in books {
        let (completed, page, read_date) = match &book.read_progress {
            Some(rp) => (rp.completed, rp.page, rp.read_date.clone()),
            None => (false, 0, None),
        };

        let mut vol = parse_volume(&book.name);
        let mut ch = parse_chapter_number(&book.name);

        // Fallback: if conservative parser did not identify volume or chapter, but Komga book number is present
        if vol.is_none() && ch.is_none() {
            if is_volume_series {
                vol = Some(book.number);
            } else {
                ch = Some(book.number);
            }
        }

        if completed {
            completed_books += 1;
            let current_book_num = book.metadata.as_ref().and_then(|m| m.number_sort).unwrap_or(book.number);
            highest_completed_book = Some(highest_completed_book.map_or(current_book_num, |h| h.max(current_book_num)));

            if let Some(v) = vol {
                highest_completed_volume = Some(highest_completed_volume.map_or(v, |h| h.max(v)));
            }
            if let Some(c) = ch {
                highest_completed_chapter = Some(highest_completed_chapter.map_or(c, |h| h.max(c)));
            }

            if let Some(date) = &read_date {
                if last_read_date.as_ref().map_or(true, |d| date > d) {
                    last_read_date = Some(date.clone());
                }
            }
        }

        book_progresses.push(KomgaBookProgress {
            id: book.id.clone(),
            name: book.name.clone(),
            number: book.number,
            volume: vol,
            chapter: ch,
            completed,
            page,
            read_date,
        });
    }

    let total_books = books.len();
    let is_series_completed = total_books > 0 && completed_books == total_books;

    KomgaSeriesProgress {
        series_id: series.id.clone(),
        series_title: series.name.clone(),
        library_id: series.library_id.clone(),
        total_books,
        completed_books,
        highest_completed_book,
        highest_completed_volume,
        highest_completed_chapter,
        is_series_completed,
        last_read_date,
        books: book_progresses,
    }
}

/// Reads all series from Komga and evaluates reading progress for each.
pub async fn fetch_all_series_progress(
    client: &KomgaClient,
    library_id: Option<&str>,
) -> Result<Vec<KomgaSeriesProgress>> {
    let series_list = client.list_series(library_id).await?;
    let mut progress_list = Vec::with_capacity(series_list.len());

    for series in series_list {
        let books = client.get_books_in_series(&series.id).await?;
        let progress = evaluate_series_progress(&series, &books);
        progress_list.push(progress);
    }

    Ok(progress_list)
}

/// Reads a single series and evaluates its reading progress.
pub async fn fetch_single_series_progress(
    client: &KomgaClient,
    series_id: &str,
    series_title: &str,
    library_id: &str,
) -> Result<KomgaSeriesProgress> {
    let books = client.get_books_in_series(series_id).await?;
    let dummy_series = KomgaSeries {
        id: series_id.to_string(),
        name: series_title.to_string(),
        library_id: library_id.to_string(),
        books_count: books.len() as i64,
    };
    Ok(evaluate_series_progress(&dummy_series, &books))
}
