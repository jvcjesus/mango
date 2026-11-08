//! Read-only library-wide naming audit and approved metadata correction workflow.
//!
//! Enforces canonical reader-facing conventions in Komga while preserving physical safety:
//! - Read-only by default (no automatic modification of existing Komga books).
//! - Identifies inconsistent volume/chapter titles, duplicate numbers, and already-canonical items.
//! - Creates durable backup records prior to any metadata update.
//! - Provides full rollback capabilities.
//! - Verifies each book in Komga after correction.

use std::collections::{HashMap, HashSet};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::pipeline::komga::{KomgaBook, KomgaClient, KomgaSeries};
use crate::pipeline::naming::{normalize_book_naming, NamingConformity};

/// Individual book entry in a naming audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingAuditItem {
    pub series_id: String,
    pub series_title: String,
    pub book_id: String,
    pub book_name: String,
    pub existing_title: String,
    pub proposed_title: String,
    pub existing_number: String,
    pub proposed_number: String,
    pub proposed_number_sort: f64,
    pub is_volume: bool,
    pub conformity: NamingConformity,
    pub metadata_source: String,
    pub confidence: String,
    pub is_safe: bool,
    pub is_already_canonical: bool,
    pub reason: String,
    pub conflict_type: Option<String>,
}

/// Comprehensive audit report for a Komga library or series.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingAuditReport {
    pub audit_id: String,
    pub library_id: String,
    pub series_id_filter: Option<String>,
    pub total_series: usize,
    pub total_books: usize,
    pub canonical_count: usize,
    pub inconsistent_title_count: usize,
    pub inconsistent_number_count: usize,
    pub duplicate_number_count: usize,
    pub ambiguous_count: usize,
    pub safe_correction_count: usize,
    pub items: Vec<NamingAuditItem>,
    pub created_at: i64,
}

/// Durable backup record for rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingCorrectionBackup {
    pub id: i64,
    pub backup_id: String,
    pub book_id: String,
    pub series_id: String,
    pub series_title: String,
    pub original_title: Option<String>,
    pub original_number: Option<String>,
    pub original_number_sort: Option<f64>,
    pub new_title: String,
    pub new_number: String,
    pub new_number_sort: f64,
    pub status: String,
    pub cbz_updated: bool,
    pub created_at: i64,
    pub rolled_back_at: Option<i64>,
}

/// Request to run a library naming audit.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingAuditRequest {
    pub library_id: Option<String>,
    pub series_id: Option<String>,
}

/// Request to apply approved naming corrections.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingApplyRequest {
    pub audit_id: String,
    pub book_ids: Option<Vec<String>>,
    #[serde(default)]
    pub update_cbz: bool,
    #[serde(default)]
    pub dry_run: bool,
}

/// Request to rollback a naming correction batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingRollbackRequest {
    pub backup_id: String,
    pub book_ids: Option<Vec<String>>,
}

/// Result of applying approved naming corrections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingCorrectionResult {
    pub backup_id: String,
    pub is_dry_run: bool,
    pub total_requested: usize,
    pub applied_count: usize,
    pub verified_count: usize,
    pub failed_count: usize,
    pub errors: Vec<String>,
}

/// Result of rolling back a naming correction batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingRollbackResult {
    pub backup_id: String,
    pub total_requested: usize,
    pub rolled_back_count: usize,
    pub failed_count: usize,
    pub errors: Vec<String>,
}

fn generate_unique_id(prefix: &str) -> String {
    let mut random = [0u8; 16];
    let _ = getrandom::getrandom(&mut random);
    format!("{prefix}_{}", hex::encode(random))
}

/// Generates a structured naming audit report from fetched Komga series and books.
pub fn generate_naming_audit_report(
    library_id: &str,
    series_id_filter: Option<&str>,
    series_with_books: Vec<(KomgaSeries, Vec<KomgaBook>)>,
) -> NamingAuditReport {
    let audit_id = generate_unique_id("audit");
    let mut audit_items = Vec::new();

    let mut canonical_count = 0;
    let mut inconsistent_title_count = 0;
    let mut inconsistent_number_count = 0;
    let mut duplicate_number_count = 0;
    let mut ambiguous_count = 0;
    let mut safe_correction_count = 0;

    let total_series = series_with_books.len();

    for (series, books) in series_with_books {
        if books.is_empty() {
            continue;
        }

        // Determine whether this series predominantly contains volumes or chapters
        let volume_signals = books.iter().filter(|b| {
            let lower = b.name.to_lowercase();
            lower.contains("vol") || lower.contains(" v") || lower.starts_with('v')
        }).count();
        let is_volume_series = volume_signals * 2 >= books.len() || volume_signals > 0;

        // Collect proposed numbers to detect duplicates within the series
        let mut number_occurrences: HashMap<String, Vec<String>> = HashMap::new();
        let mut series_items = Vec::new();

        for book in &books {
            let existing_title = book.metadata.as_ref()
                .and_then(|m| m.title.as_deref())
                .unwrap_or(&book.name)
                .trim()
                .to_string();

            let existing_number_str = book.metadata.as_ref()
                .and_then(|m| m.number.as_deref())
                .map(str::trim)
                .unwrap_or("")
                .to_string();

            let num_hint = if !existing_number_str.is_empty() {
                existing_number_str.parse::<f64>().ok()
            } else if book.number >= 0.0 {
                Some(book.number)
            } else {
                None
            };

            let norm = normalize_book_naming(
                &existing_title,
                &series.name,
                is_volume_series,
                num_hint,
            );

            let metadata_source = if book.metadata.as_ref().and_then(|m| m.title.as_ref()).is_some() {
                "Komga Metadata".to_string()
            } else {
                "Filename Parsed".to_string()
            };

            let conformity = norm.conformity;
            let mut confidence = "HIGH".to_string();
            let mut is_safe = true;
            let mut conflict_type = None;

            if conformity == NamingConformity::Ambiguous {
                confidence = "LOW".to_string();
                is_safe = false;
                ambiguous_count += 1;
            } else if conformity == NamingConformity::Range {
                confidence = "MEDIUM".to_string();
                is_safe = false;
                conflict_type = Some("RANGE_OMNIBUS".to_string());
            }

            // Track number for duplicate detection
            number_occurrences.entry(norm.number_string.clone())
                .or_default()
                .push(book.id.clone());

            series_items.push((book.clone(), norm, existing_title, existing_number_str, metadata_source, conformity, confidence, is_safe, conflict_type));
        }

        // Duplicate number detection pass
        let duplicate_numbers: HashSet<String> = number_occurrences
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(num, _)| num)
            .collect();

        for (book, norm, existing_title, existing_number_str, metadata_source, conformity, mut confidence, mut is_safe, mut conflict_type) in series_items {
            if duplicate_numbers.contains(&norm.number_string) && norm.number_string != "0" {
                conflict_type = Some("DUPLICATE_NUMBER".to_string());
                confidence = "LOW".to_string();
                is_safe = false;
                duplicate_number_count += 1;
            }

            if norm.is_already_canonical {
                canonical_count += 1;
            } else if conformity == NamingConformity::InconsistentTitle {
                inconsistent_title_count += 1;
            } else if conformity == NamingConformity::InconsistentNumber {
                inconsistent_number_count += 1;
            }

            if is_safe && !norm.is_already_canonical {
                safe_correction_count += 1;
            }

            audit_items.push(NamingAuditItem {
                series_id: series.id.clone(),
                series_title: series.name.clone(),
                book_id: book.id.clone(),
                book_name: book.name.clone(),
                existing_title,
                proposed_title: norm.title,
                existing_number: existing_number_str,
                proposed_number: norm.number_string,
                proposed_number_sort: norm.number_sort,
                is_volume: is_volume_series,
                conformity,
                metadata_source,
                confidence,
                is_safe,
                is_already_canonical: norm.is_already_canonical,
                reason: norm.reason,
                conflict_type,
            });
        }
    }

    NamingAuditReport {
        audit_id,
        library_id: library_id.to_string(),
        series_id_filter: series_id_filter.map(|s| s.to_string()),
        total_series,
        total_books: audit_items.len(),
        canonical_count,
        inconsistent_title_count,
        inconsistent_number_count,
        duplicate_number_count,
        ambiguous_count,
        safe_correction_count,
        items: audit_items,
        created_at: crate::db::now(),
    }
}

/// Executes an async fetch against Komga and computes the full library naming audit.
pub async fn execute_naming_audit(
    komga_client: &KomgaClient,
    library_id: &str,
    series_id_filter: Option<&str>,
) -> Result<NamingAuditReport> {
    let series_list = if let Some(s_id) = series_id_filter {
        let all = komga_client.list_series(Some(library_id)).await?;
        all.into_iter().filter(|s| s.id == s_id).collect()
    } else {
        komga_client.list_series(Some(library_id)).await?
    };

    let mut series_with_books = Vec::new();
    for series in series_list {
        let books = komga_client.get_books_in_series(&series.id).await?;
        series_with_books.push((series, books));
    }

    Ok(generate_naming_audit_report(library_id, series_id_filter, series_with_books))
}

/// Durably stores an audit snapshot in SQLite.
pub fn save_naming_audit(conn: &Connection, report: &NamingAuditReport) -> Result<()> {
    let details_json = serde_json::to_string(report)
        .map_err(|e| AppError::internal(format!("Failed to serialize naming audit: {e}")))?;

    conn.execute(
        "INSERT INTO naming_audit_logs (
            audit_id, library_id, series_id, total_series, total_books,
            inconsistent_titles, inconsistent_numbers, canonical_count, details_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            report.audit_id,
            report.library_id,
            report.series_id_filter,
            report.total_series as i64,
            report.total_books as i64,
            report.inconsistent_title_count as i64,
            report.inconsistent_number_count as i64,
            report.canonical_count as i64,
            details_json,
            report.created_at,
        ],
    ).map_err(|e| AppError::internal(format!("Failed to persist naming audit: {e}")))?;

    Ok(())
}

/// Fetches the latest naming audit report from SQLite.
pub fn get_latest_naming_audit(conn: &Connection) -> Result<Option<NamingAuditReport>> {
    let mut stmt = conn.prepare(
        "SELECT details_json FROM naming_audit_logs ORDER BY created_at DESC LIMIT 1"
    ).map_err(|e| AppError::internal(e.to_string()))?;

    let mut rows = stmt.query([]).map_err(|e| AppError::internal(e.to_string()))?;
    if let Some(row) = rows.next().map_err(|e| AppError::internal(e.to_string()))? {
        let json: String = row.get(0).map_err(|e| AppError::internal(e.to_string()))?;
        let report: NamingAuditReport = serde_json::from_str(&json)
            .map_err(|e| AppError::internal(format!("Failed to deserialize audit: {e}")))?;
        Ok(Some(report))
    } else {
        Ok(None)
    }
}

/// Fetches a specific audit report by its UUID from SQLite.
pub fn get_naming_audit_by_id(conn: &Connection, audit_id: &str) -> Result<Option<NamingAuditReport>> {
    let mut stmt = conn.prepare(
        "SELECT details_json FROM naming_audit_logs WHERE audit_id = ?1"
    ).map_err(|e| AppError::internal(e.to_string()))?;

    let mut rows = stmt.query([audit_id]).map_err(|e| AppError::internal(e.to_string()))?;
    if let Some(row) = rows.next().map_err(|e| AppError::internal(e.to_string()))? {
        let json: String = row.get(0).map_err(|e| AppError::internal(e.to_string()))?;
        let report: NamingAuditReport = serde_json::from_str(&json)
            .map_err(|e| AppError::internal(format!("Failed to deserialize audit: {e}")))?;
        Ok(Some(report))
    } else {
        Ok(None)
    }
}

/// Inserts a durable backup record for an individual book correction.
pub fn record_backup_entry(
    conn: &Connection,
    backup_id: &str,
    book_id: &str,
    series_id: &str,
    series_title: &str,
    original_title: Option<&str>,
    original_number: Option<&str>,
    original_number_sort: Option<f64>,
    new_title: &str,
    new_number: &str,
    new_number_sort: f64,
    cbz_updated: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO naming_correction_backups (
            backup_id, book_id, series_id, series_title,
            original_title, original_number, original_number_sort,
            new_title, new_number, new_number_sort,
            status, cbz_updated, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'applied', ?11, ?12)",
        rusqlite::params![
            backup_id,
            book_id,
            series_id,
            series_title,
            original_title,
            original_number,
            original_number_sort,
            new_title,
            new_number,
            new_number_sort,
            cbz_updated as i64,
            crate::db::now(),
        ],
    ).map_err(|e| AppError::internal(format!("Failed to store backup: {e}")))?;

    Ok(())
}

/// Updates the status of a backup record (e.g. 'verified', 'failed').
pub fn update_backup_status(conn: &Connection, backup_id: &str, book_id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE naming_correction_backups SET status = ?1 WHERE backup_id = ?2 AND book_id = ?3",
        rusqlite::params![status, backup_id, book_id],
    ).map_err(|e| AppError::internal(format!("Failed to update backup status: {e}")))?;

    Ok(())
}

/// Fetches active backup records for a batch that can be rolled back.
pub fn get_backups_for_rollback(
    conn: &Connection,
    backup_id: &str,
    book_ids_filter: Option<&[String]>,
) -> Result<Vec<NamingCorrectionBackup>> {
    let mut stmt = conn.prepare(
        "SELECT id, backup_id, book_id, series_id, series_title,
                original_title, original_number, original_number_sort,
                new_title, new_number, new_number_sort,
                status, cbz_updated, created_at, rolled_back_at
         FROM naming_correction_backups
         WHERE backup_id = ?1 AND status IN ('applied', 'verified')"
    ).map_err(|e| AppError::internal(e.to_string()))?;

    let filter_set: Option<HashSet<&str>> = book_ids_filter.map(|ids| ids.iter().map(String::as_str).collect());

    let rows = stmt.query_map([backup_id], |row| {
        Ok(NamingCorrectionBackup {
            id: row.get(0)?,
            backup_id: row.get(1)?,
            book_id: row.get(2)?,
            series_id: row.get(3)?,
            series_title: row.get(4)?,
            original_title: row.get(5)?,
            original_number: row.get(6)?,
            original_number_sort: row.get(7)?,
            new_title: row.get(8)?,
            new_number: row.get(9)?,
            new_number_sort: row.get(10)?,
            status: row.get(11)?,
            cbz_updated: row.get::<_, i64>(12)? != 0,
            created_at: row.get(13)?,
            rolled_back_at: row.get(14)?,
        })
    })
    .map_err(|e| AppError::internal(e.to_string()))?
    .filter_map(|r| r.ok())
    .filter(|b| filter_set.as_ref().is_none_or(|s| s.contains(b.book_id.as_str())))
    .collect();

    Ok(rows)
}

/// Marks a backup record as rolled back in SQLite.
pub fn mark_backup_rolled_back(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE naming_correction_backups SET status = 'rolled_back', rolled_back_at = ?1 WHERE id = ?2",
        rusqlite::params![crate::db::now(), id],
    ).map_err(|e| AppError::internal(format!("Failed to mark rollback: {e}")))?;

    Ok(())
}

/// Lists recent backup batches.
pub fn list_naming_backups(conn: &Connection) -> Result<Vec<NamingCorrectionBackup>> {
    let mut stmt = conn.prepare(
        "SELECT id, backup_id, book_id, series_id, series_title,
                original_title, original_number, original_number_sort,
                new_title, new_number, new_number_sort,
                status, cbz_updated, created_at, rolled_back_at
         FROM naming_correction_backups
         ORDER BY created_at DESC LIMIT 100"
    ).map_err(|e| AppError::internal(e.to_string()))?;

    let backups = stmt.query_map([], |row| {
        Ok(NamingCorrectionBackup {
            id: row.get(0)?,
            backup_id: row.get(1)?,
            book_id: row.get(2)?,
            series_id: row.get(3)?,
            series_title: row.get(4)?,
            original_title: row.get(5)?,
            original_number: row.get(6)?,
            original_number_sort: row.get(7)?,
            new_title: row.get(8)?,
            new_number: row.get(9)?,
            new_number_sort: row.get(10)?,
            status: row.get(11)?,
            cbz_updated: row.get::<_, i64>(12)? != 0,
            created_at: row.get(13)?,
            rolled_back_at: row.get(14)?,
        })
    })
    .map_err(|e| AppError::internal(e.to_string()))?
    .filter_map(|r| r.ok())
    .collect();

    Ok(backups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::komga::KomgaBookMetadata;

    #[test]
    fn test_generate_naming_audit_report_detects_haikyu_inconsistencies() {
        let series = KomgaSeries {
            id: "0S4XY0P4ZM30Y".to_string(),
            name: "Haikyu!!".to_string(),
            library_id: "0RMWEXC3T10TY".to_string(),
            books_count: 3,
        };

        let books = vec![
            KomgaBook {
                id: "b1".to_string(),
                series_id: series.id.clone(),
                library_id: Some(series.library_id.clone()),
                name: "Haikyu!! v01".to_string(),
                number: 1.0,
                metadata: Some(KomgaBookMetadata {
                    title: Some("Vol. 01 (Digital)".to_string()),
                    number: Some("1".to_string()),
                    number_sort: Some(1.0),
                }),
                read_progress: None,
                media: None,
                url: None,
            },
            KomgaBook {
                id: "b2".to_string(),
                series_id: series.id.clone(),
                library_id: Some(series.library_id.clone()),
                name: "Haikyu!! v02".to_string(),
                number: 2.0,
                metadata: Some(KomgaBookMetadata {
                    title: Some("Vol. 02 (Digital)".to_string()),
                    number: Some("2".to_string()),
                    number_sort: Some(2.0),
                }),
                read_progress: None,
                media: None,
                url: None,
            },
            KomgaBook {
                id: "b3".to_string(),
                series_id: series.id.clone(),
                library_id: Some(series.library_id.clone()),
                name: "Haikyu!! v03".to_string(),
                number: 3.0,
                metadata: Some(KomgaBookMetadata {
                    title: Some("Volume 3".to_string()),
                    number: Some("3".to_string()),
                    number_sort: Some(3.0),
                }),
                read_progress: None,
                media: None,
                url: None,
            },
        ];

        let report = generate_naming_audit_report(
            "0RMWEXC3T10TY",
            Some("0S4XY0P4ZM30Y"),
            vec![(series, books)],
        );

        assert_eq!(report.total_series, 1);
        assert_eq!(report.total_books, 3);
        assert_eq!(report.canonical_count, 0); // None of them were canonical initially!
        assert_eq!(report.inconsistent_title_count, 3);
        assert_eq!(report.safe_correction_count, 3);

        // Verify proposed titles
        assert_eq!(report.items[0].proposed_title, "Volume 01");
        assert_eq!(report.items[1].proposed_title, "Volume 02");
        assert_eq!(report.items[2].proposed_title, "Volume 03");
        assert!(report.items.iter().all(|i| i.is_safe));
    }
}
