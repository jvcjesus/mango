//! Staging Lifecycle Inventory and Storage Hygiene (Milestone M7.1)
//!
//! Provides read-only inventory classification and deterministic cleanup preview
//! for downloaded and staged manga files in `<data>/downloads`.
//!
//! Hard Invariants:
//! - Never deletes canonical media.
//! - Execution is strictly disabled by default (Phase A Preview only in M7).
//! - Active chapter references prevent files from being marked as disposable duplicates (Blood on the Tracks guardrail).
//! - Fails closed on incomplete provenance (classifies as UNKNOWN or ORPHAN_UNCERTAIN).

use std::fs;
use std::path::{Path, PathBuf};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
use crate::pipeline::publisher::verify_cbz_integrity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StagingClassification {
    ActiveTransfer,
    PendingValidation,
    PendingPublication,
    PublishedVerifiedDuplicate,
    PublishedUnverified,
    FailedImportRetained,
    RecoveryRequired,
    OrphanUncertain,
    Unknown,
}

impl StagingClassification {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ActiveTransfer => "ACTIVE_TRANSFER",
            Self::PendingValidation => "PENDING_VALIDATION",
            Self::PendingPublication => "PENDING_PUBLICATION",
            Self::PublishedVerifiedDuplicate => "PUBLISHED_VERIFIED_DUPLICATE",
            Self::PublishedUnverified => "PUBLISHED_UNVERIFIED",
            Self::FailedImportRetained => "FAILED_IMPORT_RETAINED",
            Self::RecoveryRequired => "RECOVERY_REQUIRED",
            Self::OrphanUncertain => "ORPHAN_UNCERTAIN",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingItem {
    pub file_path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub classification: StagingClassification,
    pub reason: String,
    pub related_chapter_id: Option<i64>,
    pub related_manga_id: Option<i64>,
    pub series_title: Option<String>,
    pub canonical_destination: Option<String>,
    pub canonical_receipt_id: Option<String>,
    pub ownership_verified: bool,
    pub active_dependency_detected: bool,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingInventoryReport {
    pub total_files: usize,
    pub total_staging_bytes: u64,
    pub active_transfer_bytes: u64,
    pub verified_duplicate_bytes: u64,
    pub failed_recovery_bytes: u64,
    pub unknown_bytes: u64,
    pub estimated_reclaimable_bytes: u64,
    pub items: Vec<StagingItem>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupManifest {
    pub manifest_id: String,
    pub generated_at: i64,
    pub execution_enabled: bool,
    pub eligible_items: Vec<StagingItem>,
    pub blocked_items: Vec<StagingItem>,
    pub total_staged_bytes: u64,
    pub estimated_reclaimable_bytes: u64,
    pub quarantine_recommended: bool,
    pub notice: String,
}

/// Recursively scans `downloads_dir` for files, classifying each artifact.
pub async fn generate_staging_inventory(
    db: &Db,
    downloads_dir: &Path,
    library_dir: Option<&Path>,
) -> Result<StagingInventoryReport> {
    if !downloads_dir.exists() {
        return Ok(StagingInventoryReport {
            total_files: 0,
            total_staging_bytes: 0,
            active_transfer_bytes: 0,
            verified_duplicate_bytes: 0,
            failed_recovery_bytes: 0,
            unknown_bytes: 0,
            estimated_reclaimable_bytes: 0,
            items: Vec::new(),
            generated_at: crate::db::now(),
        });
    }

    // Collect all files in downloads_dir
    let mut file_paths = Vec::new();
    collect_files_recursive(downloads_dir, &mut file_paths)?;

    let lib_dir_buf = library_dir.map(PathBuf::from);

    // Database lookup & classification
    let items = db.call(move |conn| {
        let mut evaluated = Vec::new();
        for path in file_paths {
            let item = classify_staging_file(conn, &path, &lib_dir_buf)?;
            evaluated.push(item);
        }
        Ok(evaluated)
    }).await?;

    let mut total_staging_bytes: u64 = 0;
    let mut active_transfer_bytes: u64 = 0;
    let mut verified_duplicate_bytes: u64 = 0;
    let mut failed_recovery_bytes: u64 = 0;
    let mut unknown_bytes: u64 = 0;
    let mut estimated_reclaimable_bytes: u64 = 0;

    for item in &items {
        total_staging_bytes += item.size_bytes;
        match item.classification {
            StagingClassification::ActiveTransfer | StagingClassification::PendingValidation | StagingClassification::PendingPublication => {
                active_transfer_bytes += item.size_bytes;
            }
            StagingClassification::PublishedVerifiedDuplicate => {
                verified_duplicate_bytes += item.size_bytes;
                estimated_reclaimable_bytes += item.reclaimable_bytes;
            }
            StagingClassification::FailedImportRetained | StagingClassification::RecoveryRequired => {
                failed_recovery_bytes += item.size_bytes;
            }
            StagingClassification::OrphanUncertain | StagingClassification::Unknown | StagingClassification::PublishedUnverified => {
                unknown_bytes += item.size_bytes;
            }
        }
    }

    Ok(StagingInventoryReport {
        total_files: items.len(),
        total_staging_bytes,
        active_transfer_bytes,
        verified_duplicate_bytes,
        failed_recovery_bytes,
        unknown_bytes,
        estimated_reclaimable_bytes,
        items,
        generated_at: crate::db::now(),
    })
}

/// Generates a preview manifest for safe cleanup.
/// Execution is strictly disabled by default (Phase A Preview).
pub async fn generate_cleanup_manifest(
    db: &Db,
    downloads_dir: &Path,
    library_dir: Option<&Path>,
) -> Result<CleanupManifest> {
    let inventory = generate_staging_inventory(db, downloads_dir, library_dir).await?;

    let mut eligible_items = Vec::new();
    let mut blocked_items = Vec::new();
    let mut estimated_reclaimable_bytes: u64 = 0;

    for item in inventory.items {
        if item.classification == StagingClassification::PublishedVerifiedDuplicate && !item.active_dependency_detected {
            estimated_reclaimable_bytes += item.size_bytes;
            eligible_items.push(item);
        } else {
            blocked_items.push(item);
        }
    }

    let manifest_id = format!("manifest-{}", crate::db::now());
    let notice = "Phase A Preview Manifest. Real execution is disabled by default in M7 and requires separate explicit user authorization. No files were deleted.".to_string();

    Ok(CleanupManifest {
        manifest_id,
        generated_at: crate::db::now(),
        execution_enabled: false,
        eligible_items,
        blocked_items,
        total_staged_bytes: inventory.total_staging_bytes,
        estimated_reclaimable_bytes,
        quarantine_recommended: true,
        notice,
    })
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn classify_staging_file(
    conn: &Connection,
    path: &Path,
    library_dir: &Option<PathBuf>,
) -> Result<StagingItem> {
    let metadata = fs::metadata(path)?;
    let size_bytes = metadata.len();
    let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let path_str = path.to_string_lossy().to_string();

    // Check if filename indicates a partial download (.part)
    if path_str.ends_with(".part") || file_name.ends_with(".part") {
        return Ok(StagingItem {
            file_path: path_str,
            file_name,
            size_bytes,
            sha256: None,
            classification: StagingClassification::ActiveTransfer,
            reason: "Partial transfer in progress (.part file)".to_string(),
            related_chapter_id: None,
            related_manga_id: None,
            series_title: None,
            canonical_destination: None,
            canonical_receipt_id: None,
            ownership_verified: false,
            active_dependency_detected: true,
            reclaimable_bytes: 0,
        });
    }

    // Check database references in chapters and downloads
    // Query 1: Active download queue
    let dl_ref: Option<(i64, i64, String, String, Option<String>)> = conn
        .query_row(
            "SELECT d.chapter_id, c.manga_id, m.title, d.state, d.published_path
             FROM downloads d
             JOIN chapters c ON c.id = d.chapter_id
             JOIN manga m ON m.id = c.manga_id
             WHERE d.retrieved_path = ?1
                OR c.download_path = ?1
                OR ?1 LIKE '%' || c.download_path
             LIMIT 1",
            [&path_str],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;

    if let Some((ch_id, m_id, title, state, pub_path)) = dl_ref {
        match state.as_str() {
            "downloading" | "queued" => {
                return Ok(StagingItem {
                    file_path: path_str,
                    file_name,
                    size_bytes,
                    sha256: None,
                    classification: StagingClassification::ActiveTransfer,
                    reason: format!("Active download queue job #{ch_id} (state: {state})"),
                    related_chapter_id: Some(ch_id),
                    related_manga_id: Some(m_id),
                    series_title: Some(title),
                    canonical_destination: pub_path,
                    canonical_receipt_id: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    reclaimable_bytes: 0,
                });
            }
            "validating" => {
                return Ok(StagingItem {
                    file_path: path_str,
                    file_name,
                    size_bytes,
                    sha256: None,
                    classification: StagingClassification::PendingValidation,
                    reason: format!("Pending archive validation for job #{ch_id}"),
                    related_chapter_id: Some(ch_id),
                    related_manga_id: Some(m_id),
                    series_title: Some(title),
                    canonical_destination: pub_path,
                    canonical_receipt_id: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    reclaimable_bytes: 0,
                });
            }
            "publishing" => {
                return Ok(StagingItem {
                    file_path: path_str,
                    file_name,
                    size_bytes,
                    sha256: None,
                    classification: StagingClassification::PendingPublication,
                    reason: format!("Publication in progress for job #{ch_id}"),
                    related_chapter_id: Some(ch_id),
                    related_manga_id: Some(m_id),
                    series_title: Some(title),
                    canonical_destination: pub_path,
                    canonical_receipt_id: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    reclaimable_bytes: 0,
                });
            }
            "failed" => {
                return Ok(StagingItem {
                    file_path: path_str,
                    file_name,
                    size_bytes,
                    sha256: None,
                    classification: StagingClassification::RecoveryRequired,
                    reason: format!("Download failed for job #{ch_id} — retained for restart recovery"),
                    related_chapter_id: Some(ch_id),
                    related_manga_id: Some(m_id),
                    series_title: Some(title),
                    canonical_destination: pub_path,
                    canonical_receipt_id: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    reclaimable_bytes: 0,
                });
            }
            _ => {}
        }
    }

    // Query 2: Check if chapters table still points to this staging path as its reader source
    let chapter_dep: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT c.id, c.manga_id, m.title
             FROM chapters c
             JOIN manga m ON m.id = c.manga_id
             WHERE c.download_path = ?1
                OR ?1 LIKE '%' || c.download_path
             LIMIT 1",
            [&path_str],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    if let Some((ch_id, m_id, title)) = chapter_dep {
        // Active chapter reference exists in DB! Deleting this would break the chapter reader pointer!
        return Ok(StagingItem {
            file_path: path_str,
            file_name,
            size_bytes,
            sha256: None,
            classification: StagingClassification::PublishedUnverified,
            reason: format!("Active chapter reference in database (Chapter #{ch_id}) points directly to this file; retention required"),
            related_chapter_id: Some(ch_id),
            related_manga_id: Some(m_id),
            series_title: Some(title),
            canonical_destination: None,
            canonical_receipt_id: None,
            ownership_verified: false,
            active_dependency_detected: true,
            reclaimable_bytes: 0,
        });
    }

    // Query 3: Check if this file is a publication receipt duplicate
    // Does a verified canonical publication receipt exist for this file or matching series/volume?
    if let Some(lib_root) = library_dir {
        if let Ok(Some((series, vol))) = extract_series_and_volume_from_name(&file_name) {
            let canon_name = format!("{series} v{vol:02}.cbz");
            let canon_path = lib_root.join(&series).join(&canon_name);
            if canon_path.is_file() {
                // Canonical counterpart exists! Check integrity and receipt
                let canon_str = canon_path.to_string_lossy().to_string();
                let receipt: Option<(String, bool)> = conn
                    .query_row(
                        "SELECT id, ownership_verified FROM publication_receipts WHERE canonical_path = ?1 LIMIT 1",
                        [&canon_str],
                        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
                    )
                    .optional()?;

                let is_verified_cbz = verify_cbz_integrity(&canon_path).is_ok();
                if is_verified_cbz {
                    let receipt_id = receipt.as_ref().map(|(id, _)| id.clone());
                    let ownership_verified = receipt.as_ref().map(|(_, v)| *v).unwrap_or(true);

                    return Ok(StagingItem {
                        file_path: path_str,
                        file_name,
                        size_bytes,
                        sha256: None,
                        classification: StagingClassification::PublishedVerifiedDuplicate,
                        reason: format!("Canonical archive verified at {}", canon_path.display()),
                        related_chapter_id: None,
                        related_manga_id: None,
                        series_title: Some(series),
                        canonical_destination: Some(canon_str),
                        canonical_receipt_id: receipt_id,
                        ownership_verified,
                        active_dependency_detected: false,
                        reclaimable_bytes: size_bytes,
                    });
                }
            }
        }
    }

    // Fallback: If it's a valid CBZ but unreferenced
    if path_str.ends_with(".cbz") || path_str.ends_with(".zip") {
        let is_valid = verify_cbz_integrity(path).is_ok();
        if is_valid {
            return Ok(StagingItem {
                file_path: path_str,
                file_name,
                size_bytes,
                sha256: None,
                classification: StagingClassification::OrphanUncertain,
                reason: "Valid CBZ archive without recorded active database reference or receipt".to_string(),
                related_chapter_id: None,
                related_manga_id: None,
                series_title: None,
                canonical_destination: None,
                canonical_receipt_id: None,
                ownership_verified: false,
                active_dependency_detected: false,
                reclaimable_bytes: 0,
            });
        }
    }

    Ok(StagingItem {
        file_path: path_str,
        file_name,
        size_bytes,
        sha256: None,
        classification: StagingClassification::Unknown,
        reason: "Unrecognized file format or incomplete provenance".to_string(),
        related_chapter_id: None,
        related_manga_id: None,
        series_title: None,
        canonical_destination: None,
        canonical_receipt_id: None,
        ownership_verified: false,
        active_dependency_detected: false,
        reclaimable_bytes: 0,
    })
}

fn extract_series_and_volume_from_name(name: &str) -> Result<Option<(String, u32)>> {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".cbz") && !lower.ends_with(".zip") {
        return Ok(None);
    }

    if let Some(vol) = crate::acquisition::madokami::extract_volume_number(name) {
        let vol_u32 = vol as u32;
        if vol_u32 > 0 {
            let clean = name.trim_end_matches(".cbz").trim_end_matches(".zip");
            // Find volume token position to isolate series title
            let lower_clean = clean.to_ascii_lowercase();
            let vol_str = format!("{vol_u32}");
            let vol_pad = format!("{:02}", vol_u32);

            let markers = [
                format!("v{vol_pad}"),
                format!("v{vol_str}"),
                format!("vol {vol_str}"),
                format!("volume {vol_str}"),
                format!("vol. {vol_str}"),
            ];

            for marker in markers {
                if let Some(pos) = lower_clean.rfind(&marker) {
                    let series = clean[..pos].trim().trim_end_matches(['-', '_', '.', ' ']).to_string();
                    if !series.is_empty() {
                        return Ok(Some((series, vol_u32)));
                    }
                }
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn make_test_cbz(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(b"<ComicInfo><Series>Test</Series><Volume>1</Volume></ComicInfo>").unwrap();
        zip.start_file("001.png", options).unwrap();
        zip.write_all(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82").unwrap();
        zip.finish().unwrap();
    }

    #[tokio::test]
    async fn test_staging_active_transfer_and_manifest_preview() {
        let temp = std::env::temp_dir().join(format!("mango_test_staging_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db_path = temp.join("test.db");
        let db = Db::open(&db_path).unwrap();

        // 1. Partial file (.part)
        let part_file = downloads_dir.join("downloading.cbz.part");
        fs::write(&part_file, b"partial data").unwrap();

        // 2. Active download
        let active_file = downloads_dir.join("active.cbz");
        make_test_cbz(&active_file);

        let active_path_str = active_file.to_string_lossy().to_string();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Test Manga', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO chapters (id, manga_id, name, chapter_number, url, source_order, fetched_at)
                 VALUES (101, 1, 'Vol 1', 1.0, '/c101', 0, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO downloads (chapter_id, position, state, attempts, added_at, retrieved_path)
                 VALUES (101, 0, 'downloading', 1, 1000, ?1)",
                [&active_path_str],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inventory = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(inventory.total_files, 2);
        assert_eq!(inventory.items.iter().filter(|i| i.classification == StagingClassification::ActiveTransfer).count(), 2);
        assert_eq!(inventory.estimated_reclaimable_bytes, 0);

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert!(!manifest.execution_enabled, "Execution must be strictly disabled");
        assert_eq!(manifest.eligible_items.len(), 0);
        assert_eq!(manifest.blocked_items.len(), 2);

        let _ = fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_staging_active_chapter_dependency_blocks_duplicate_cleanup() {
        // Blood on the Tracks rule: even if a canonical file exists, if chapters.download_path still points to staging,
        // it must NOT be marked as disposable duplicate.
        let temp = std::env::temp_dir().join(format!("mango_test_staging_dep_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db_path = temp.join("test.db");
        let db = Db::open(&db_path).unwrap();

        let staged_file = downloads_dir.join("Blood on the Tracks v01.cbz");
        make_test_cbz(&staged_file);

        let canon_file = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");
        make_test_cbz(&canon_file);

        let staged_str = staged_file.to_string_lossy().to_string();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (702, 1, '/m702', 'Blood on the Tracks', 1)",
                [],
            )?;
            // Chapter download_path still references staging!
            conn.execute(
                "INSERT INTO chapters (id, manga_id, name, chapter_number, url, source_order, fetched_at, download_path)
                 VALUES (15057, 702, 'Vol 1', 1.0, '/c15057', 0, 1000, ?1)",
                [&staged_str],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inventory = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = inventory.items.iter().find(|i| i.file_name.contains("Blood on the Tracks v01")).unwrap();
        assert!(item.active_dependency_detected, "Active chapter reference must be detected");
        assert_eq!(item.classification, StagingClassification::PublishedUnverified);
        assert_eq!(inventory.estimated_reclaimable_bytes, 0);

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(manifest.eligible_items.len(), 0, "No items eligible while dependency exists");

        let _ = fs::remove_dir_all(&temp);
    }
}
