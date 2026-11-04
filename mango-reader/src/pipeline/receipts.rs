//! Durable Publication Receipts and Crash-Safe Recovery Boundaries (M6.9).
//!
//! Provides verifiable, immutable publication receipts for all canonically published manga archives:
//! - Once a volume is published, the receipt records the UUID, canonical path, size, sha256,
//!   Komga notification status, and metadata status.
//! - Prevents re-downloading or re-publishing existing archives across process restarts or crashes.
//! - Isolates post-publication metadata failures from file publication.

use crate::{
    db::now,
    error::{AppError, Result},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::Path,
};

/// Generates a unique, collision-resistant receipt ID.
pub fn new_receipt_id() -> String {
    let mut random = [0u8; 16];
    let _ = getrandom::getrandom(&mut random);
    format!(
        "rcpt-{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([random[0], random[1], random[2], random[3]]),
        u16::from_be_bytes([random[4], random[5]]),
        u16::from_be_bytes([random[6], random[7]]),
        u16::from_be_bytes([random[8], random[9]]),
        u64::from_be_bytes([
            0, 0, random[10], random[11], random[12], random[13], random[14], random[15]
        ])
    )
}

/// Computes the total byte size and hex SHA-256 digest of a file on disk.
pub fn compute_file_sha256(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)
        .map_err(|e| AppError::not_found(format!("Failed to open file for sha256: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total_bytes = 0u64;

    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| AppError::internal(format!("Failed reading file for sha256: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total_bytes += n as u64;
    }

    let hash_str = hex::encode(hasher.finalize());
    Ok((total_bytes, hash_str))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationReceipt {
    pub id: String,
    pub chapter_id: i64,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: Option<f64>,
    pub chapter_number: Option<f64>,
    pub is_volume: bool,
    pub canonical_path: String,
    pub file_size_bytes: u64,
    pub file_sha256: String,
    pub published_at: i64,
    pub komga_notified: bool,
    pub komga_library_id: Option<String>,
    pub metadata_status: String,
    pub metadata_error: Option<String>,
    pub verified_ownership: bool,
    pub verified_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationReceiptVerificationResult {
    pub receipt_id: String,
    pub canonical_path: String,
    pub file_exists: bool,
    pub cbz_valid: bool,
    pub size_matched: bool,
    pub sha256_matched: bool,
    pub is_valid: bool,
    pub error: Option<String>,
}

/// Atomically creates or updates a durable publication receipt upon successful publication.
///
/// Safety guarantees:
/// - Fails closed on zero file size or empty SHA256.
/// - Does not overwrite immutable provenance on `canonical_path` conflict unless the
///   stored receipt represents the identical verified publication (matching size and sha256).
pub fn record_publication_receipt(
    conn: &Connection,
    chapter_id: i64,
    manga_id: Option<i64>,
    series_title: &str,
    is_volume: bool,
    release_number: f64,
    canonical_path: &str,
    file_size_bytes: u64,
    file_sha256: &str,
    komga_notified: bool,
    komga_library_id: Option<&str>,
    metadata_status: &str,
    metadata_error: Option<&str>,
) -> Result<PublicationReceipt> {
    if file_size_bytes == 0 || file_sha256.trim().is_empty() {
        return Err(AppError::bad_request(
            "Cannot record publication receipt with zero file size or empty SHA256 hash",
        ));
    }

    let current_time = now();
    let vol_num = if is_volume { Some(release_number) } else { None };
    let ch_num = if !is_volume { Some(release_number) } else { None };

    // Check if a receipt already exists for this canonical path
    if let Some(existing) = get_publication_receipt_by_path(conn, canonical_path)? {
        // Prove that the stored receipt represents the identical publication
        if existing.file_sha256 != file_sha256 || (existing.file_size_bytes as u64) != file_size_bytes {
            return Err(AppError::conflict(format!(
                "Conflicting publication receipt for canonical path '{}': existing receipt {} has sha256={}, size={} bytes; cannot overwrite with new publication sha256={}, size={} bytes",
                canonical_path, existing.id, existing.file_sha256, existing.file_size_bytes, file_sha256, file_size_bytes
            )));
        }

        // Identical publication: safely update operational metadata while preserving immutable provenance
        conn.execute(
            "UPDATE publication_receipts SET
                chapter_id = ?1,
                manga_id = COALESCE(?2, manga_id),
                series_title = ?3,
                volume_number = ?4,
                chapter_number = ?5,
                is_volume = ?6,
                komga_notified = CASE WHEN ?7 = 1 THEN 1 ELSE komga_notified END,
                komga_library_id = COALESCE(?8, komga_library_id),
                metadata_status = ?9,
                metadata_error = ?10,
                updated_at = ?11
             WHERE canonical_path = ?12",
            params![
                chapter_id,
                manga_id,
                series_title,
                vol_num,
                ch_num,
                if is_volume { 1 } else { 0 },
                if komga_notified { 1 } else { 0 },
                komga_library_id,
                metadata_status,
                metadata_error,
                current_time,
                canonical_path,
            ],
        )?;

        return get_publication_receipt_by_path(conn, canonical_path)?
            .ok_or_else(|| AppError::internal("Failed to retrieve updated publication receipt"));
    }

    let receipt_id = new_receipt_id();
    conn.execute(
        "INSERT INTO publication_receipts (
            id, chapter_id, manga_id, series_title, volume_number, chapter_number,
            is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
            komga_notified, komga_library_id, metadata_status, metadata_error,
            verified_ownership, verified_at, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, NULL, ?11, ?11)",
        params![
            receipt_id,
            chapter_id,
            manga_id,
            series_title,
            vol_num,
            ch_num,
            if is_volume { 1 } else { 0 },
            canonical_path,
            file_size_bytes as i64,
            file_sha256,
            current_time,
            if komga_notified { 1 } else { 0 },
            komga_library_id,
            metadata_status,
            metadata_error,
        ],
    )?;

    get_publication_receipt_by_path(conn, canonical_path)?
        .ok_or_else(|| AppError::internal("Failed to retrieve recorded publication receipt"))
}

/// Retrieves a publication receipt by canonical path.
pub fn get_publication_receipt_by_path(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<PublicationReceipt>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_id, manga_id, series_title, volume_number, chapter_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
                komga_notified, komga_library_id, metadata_status, metadata_error,
                verified_ownership, verified_at, created_at, updated_at
         FROM publication_receipts
         WHERE canonical_path = ?1",
    )?;

    stmt.query_row([canonical_path], map_receipt_row).optional().map_err(|e| {
        AppError::internal(format!("Failed to query publication receipt by path: {e}"))
    })
}

/// Retrieves a publication receipt by chapter ID.
pub fn get_publication_receipt_by_chapter(
    conn: &Connection,
    chapter_id: i64,
) -> Result<Option<PublicationReceipt>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_id, manga_id, series_title, volume_number, chapter_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
                komga_notified, komga_library_id, metadata_status, metadata_error,
                verified_ownership, verified_at, created_at, updated_at
         FROM publication_receipts
         WHERE chapter_id = ?1
         ORDER BY id DESC LIMIT 1",
    )?;

    stmt.query_row([chapter_id], map_receipt_row).optional().map_err(|e| {
        AppError::internal(format!("Failed to query publication receipt by chapter: {e}"))
    })
}

/// Retrieves a publication receipt by receipt ID.
pub fn get_publication_receipt(
    conn: &Connection,
    receipt_id: &str,
) -> Result<Option<PublicationReceipt>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_id, manga_id, series_title, volume_number, chapter_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
                komga_notified, komga_library_id, metadata_status, metadata_error,
                verified_ownership, verified_at, created_at, updated_at
         FROM publication_receipts
         WHERE id = ?1",
    )?;

    stmt.query_row([receipt_id], map_receipt_row).optional().map_err(|e| {
        AppError::internal(format!("Failed to query publication receipt: {e}"))
    })
}

/// Lists recent publication receipts across the system.
pub fn list_publication_receipts(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<PublicationReceipt>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_id, manga_id, series_title, volume_number, chapter_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
                komga_notified, komga_library_id, metadata_status, metadata_error,
                verified_ownership, verified_at, created_at, updated_at
         FROM publication_receipts
         ORDER BY published_at DESC
         LIMIT ?1",
    )?;

    let rows = stmt.query_map([limit as i64], map_receipt_row)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// Lists publication receipts for a specific manga title.
pub fn list_receipts_for_manga(
    conn: &Connection,
    manga_id: i64,
) -> Result<Vec<PublicationReceipt>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_id, manga_id, series_title, volume_number, chapter_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at,
                komga_notified, komga_library_id, metadata_status, metadata_error,
                verified_ownership, verified_at, created_at, updated_at
         FROM publication_receipts
         WHERE manga_id = ?1
         ORDER BY volume_number ASC, chapter_number ASC",
    )?;

    let rows = stmt.query_map([manga_id], map_receipt_row)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// Updates Komga scan notification status on a receipt.
pub fn update_receipt_komga_status(
    conn: &Connection,
    canonical_path: &str,
    komga_notified: bool,
    komga_library_id: Option<&str>,
) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE publication_receipts
         SET komga_notified = ?1, komga_library_id = COALESCE(?2, komga_library_id), updated_at = ?3
         WHERE canonical_path = ?4",
        params![
            if komga_notified { 1 } else { 0 },
            komga_library_id,
            current_time,
            canonical_path
        ],
    )?;
    Ok(())
}

/// Updates post-publication metadata status on a receipt without touching media.
pub fn update_receipt_metadata_status(
    conn: &Connection,
    canonical_path: &str,
    metadata_status: &str,
    metadata_error: Option<&str>,
) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE publication_receipts
         SET metadata_status = ?1, metadata_error = ?2, updated_at = ?3
         WHERE canonical_path = ?4",
        params![metadata_status, metadata_error, current_time, canonical_path],
    )?;
    Ok(())
}

/// Updates verified ownership status on a receipt once confirmed in Komga.
pub fn update_receipt_ownership_verified(
    conn: &Connection,
    canonical_path: &str,
    verified: bool,
) -> Result<()> {
    let current_time = now();
    conn.execute(
        "UPDATE publication_receipts
         SET verified_ownership = ?1, verified_at = CASE WHEN ?1 = 1 THEN ?2 ELSE verified_at END, updated_at = ?2
         WHERE canonical_path = ?3",
        params![if verified { 1 } else { 0 }, current_time, canonical_path],
    )?;
    Ok(())
}

/// Re-verifies a publication receipt against the physical filesystem.
/// Checks that the file exists, has valid CBZ structure, matches recorded size, and matches recorded sha256.
pub fn verify_publication_receipt_on_disk(
    conn: &Connection,
    receipt_id: &str,
) -> Result<PublicationReceiptVerificationResult> {
    let receipt = get_publication_receipt(conn, receipt_id)?
        .ok_or_else(|| AppError::not_found(format!("Receipt {receipt_id} not found")))?;

    let path = Path::new(&receipt.canonical_path);
    if !path.is_file() {
        return Ok(PublicationReceiptVerificationResult {
            receipt_id: receipt.id,
            canonical_path: receipt.canonical_path,
            file_exists: false,
            cbz_valid: false,
            size_matched: false,
            sha256_matched: false,
            is_valid: false,
            error: Some("Canonical file does not exist on disk".to_string()),
        });
    }

    let cbz_valid = crate::pipeline::publisher::verify_cbz_integrity(path).is_ok();
    let (actual_size, actual_sha256) = match compute_file_sha256(path) {
        Ok(res) => res,
        Err(e) => {
            return Ok(PublicationReceiptVerificationResult {
                receipt_id: receipt.id,
                canonical_path: receipt.canonical_path,
                file_exists: true,
                cbz_valid,
                size_matched: false,
                sha256_matched: false,
                is_valid: false,
                error: Some(format!("Failed reading canonical file: {e}")),
            });
        }
    };

    let size_matched = actual_size == receipt.file_size_bytes;
    let sha256_matched = actual_sha256.eq_ignore_ascii_case(&receipt.file_sha256);
    let is_valid = cbz_valid && size_matched && sha256_matched;

    let error = if !cbz_valid {
        Some("Archive integrity verification failed".to_string())
    } else if !size_matched {
        Some(format!(
            "File size mismatch (actual: {actual_size}, recorded: {})",
            receipt.file_size_bytes
        ))
    } else if !sha256_matched {
        Some("SHA-256 digest mismatch".to_string())
    } else {
        None
    };

    Ok(PublicationReceiptVerificationResult {
        receipt_id: receipt.id,
        canonical_path: receipt.canonical_path,
        file_exists: true,
        cbz_valid,
        size_matched,
        sha256_matched,
        is_valid,
        error,
    })
}

fn map_receipt_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PublicationReceipt> {
    Ok(PublicationReceipt {
        id: r.get(0)?,
        chapter_id: r.get(1)?,
        manga_id: r.get(2)?,
        series_title: r.get(3)?,
        volume_number: r.get(4)?,
        chapter_number: r.get(5)?,
        is_volume: r.get::<_, i64>(6)? != 0,
        canonical_path: r.get(7)?,
        file_size_bytes: r.get::<_, i64>(8)? as u64,
        file_sha256: r.get(9)?,
        published_at: r.get(10)?,
        komga_notified: r.get::<_, i64>(11)? != 0,
        komga_library_id: r.get(12)?,
        metadata_status: r.get(13)?,
        metadata_error: r.get(14)?,
        verified_ownership: r.get::<_, i64>(15)? != 0,
        verified_at: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}
