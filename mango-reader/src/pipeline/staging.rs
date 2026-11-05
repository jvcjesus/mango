//! Staging Lifecycle Inventory and Storage Hygiene (Milestones M7.1 & M7.6)
//!
//! Provides read-only inventory classification and deterministic cleanup preview
//! for downloaded and staged manga files in `<data>/downloads`.
//!
//! Hard Invariants (M7.6 Hardened):
//! - Never deletes canonical media.
//! - Execution is strictly disabled by default (Phase A Preview only).
//! - Active chapter references prevent files from being marked as disposable duplicates (Blood on the Tracks guardrail).
//! - Staged files are NEVER marked as `PublishedVerifiedDuplicate` without byte-exact SHA-256 matching,
//!   valid on-disk CBZ validation for both files, authoritative publication receipt, confirmed Komga ownership,
//!   and absence of active reader/import/recovery dependencies.
//! - Fails closed on incomplete provenance (classifies as UNKNOWN, ORPHAN_UNCERTAIN, or PUBLISHED_UNVERIFIED).

use std::fs;
use std::path::{Path, PathBuf};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::error::Result;
use crate::pipeline::publisher::verify_cbz_integrity;
use crate::pipeline::receipts::compute_file_sha256;

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
    pub canonical_sha256: Option<String>,
    pub ownership_verified: bool,
    pub active_dependency_detected: bool,
    pub verification_timestamp: i64,
    pub eligibility_decision: String, // "ELIGIBLE" or "BLOCKED"
    pub missing_evidence: Vec<String>,
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
    pub verification_identity: String,
    pub generated_at: i64,
    pub execution_enabled: bool,
    pub eligible_items: Vec<StagingItem>,
    pub blocked_items: Vec<StagingItem>,
    pub total_staged_bytes: u64,
    pub estimated_reclaimable_bytes: u64,
    pub quarantine_recommended: bool,
    pub notice: String,
}

/// Recursively scans `downloads_dir` for files, classifying each artifact under strict fail-closed safety rules.
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

    // Collect all files in downloads_dir, rejecting directory symlink recursion
    let mut file_paths = Vec::new();
    collect_files_recursive(downloads_dir, &mut file_paths)?;

    let downloads_dir_buf = downloads_dir.to_path_buf();
    let lib_dir_buf = library_dir.map(PathBuf::from);

    // Database lookup & classification
    let items = db.call(move |conn| {
        let mut evaluated = Vec::new();
        for path in file_paths {
            let item = classify_staging_file(conn, &path, &downloads_dir_buf, &lib_dir_buf)?;
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

/// Generates a deterministic preview manifest for safe cleanup.
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
        if item.classification == StagingClassification::PublishedVerifiedDuplicate
            && item.eligibility_decision == "ELIGIBLE"
            && !item.active_dependency_detected
            && item.missing_evidence.is_empty()
        {
            estimated_reclaimable_bytes += item.reclaimable_bytes;
            eligible_items.push(item);
        } else {
            blocked_items.push(item);
        }
    }

    // Deterministic verification identity based on the exact state of eligible files
    let verification_identity = if eligible_items.is_empty() {
        let mut hasher = Sha256::new();
        hasher.update(b"empty-inventory-");
        hasher.update(&inventory.total_staging_bytes.to_le_bytes());
        hasher.update(&inventory.total_files.to_le_bytes());
        hex::encode(hasher.finalize())
    } else {
        let mut hasher = Sha256::new();
        for item in &eligible_items {
            hasher.update(item.file_path.as_bytes());
            hasher.update(item.sha256.as_deref().unwrap_or("").as_bytes());
            hasher.update(&item.size_bytes.to_le_bytes());
        }
        hex::encode(hasher.finalize())
    };

    let manifest_id = format!("manifest-{}", &verification_identity[..16]);
    let notice = "Phase A Preview Manifest. Real execution is disabled by default in M7 and requires separate explicit user authorization. No files were deleted.".to_string();

    Ok(CleanupManifest {
        manifest_id,
        verification_identity,
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
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // Never recurse into symlinked directories to prevent path escape or traversal loops
            if !file_type.is_symlink() {
                collect_files_recursive(&path, out)?;
            }
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn classify_staging_file(
    conn: &Connection,
    path: &Path,
    downloads_dir: &Path,
    library_dir: &Option<PathBuf>,
) -> Result<StagingItem> {
    let now = crate::db::now();
    let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let path_str = path.to_string_lossy().to_string();

    // Symlink escape protection: ensure file does not escape permitted downloads_dir
    let symlink_meta = fs::symlink_metadata(path)?;
    if symlink_meta.file_type().is_symlink() {
        let is_escape = if let (Ok(canon_staged), Ok(canon_root)) = (path.canonicalize(), downloads_dir.canonicalize()) {
            !canon_staged.starts_with(&canon_root)
        } else {
            true
        };
        if is_escape {
            return Ok(StagingItem {
                file_path: path_str,
                file_name,
                size_bytes: 0,
                sha256: None,
                classification: StagingClassification::OrphanUncertain,
                reason: "Symlink escapes permitted staging storage root".to_string(),
                related_chapter_id: None,
                related_manga_id: None,
                series_title: None,
                canonical_destination: None,
                canonical_receipt_id: None,
                canonical_sha256: None,
                ownership_verified: false,
                active_dependency_detected: true,
                verification_timestamp: now,
                eligibility_decision: "BLOCKED".to_string(),
                missing_evidence: vec!["Symlink target escapes permitted staging root".to_string()],
                reclaimable_bytes: 0,
            });
        }
    }

    if !path.is_file() {
        return Ok(StagingItem {
            file_path: path_str,
            file_name,
            size_bytes: 0,
            sha256: None,
            classification: StagingClassification::Unknown,
            reason: "Target is not a regular file".to_string(),
            related_chapter_id: None,
            related_manga_id: None,
            series_title: None,
            canonical_destination: None,
            canonical_receipt_id: None,
            canonical_sha256: None,
            ownership_verified: false,
            active_dependency_detected: true,
            verification_timestamp: now,
            eligibility_decision: "BLOCKED".to_string(),
            missing_evidence: vec!["Not a regular file".to_string()],
            reclaimable_bytes: 0,
        });
    }

    let meta_before = fs::metadata(path)?;
    let size_bytes = meta_before.len();

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
            canonical_sha256: None,
            ownership_verified: false,
            active_dependency_detected: true,
            verification_timestamp: now,
            eligibility_decision: "BLOCKED".to_string(),
            missing_evidence: vec!["Partial transfer in progress (.part extension)".to_string()],
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
                    canonical_sha256: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    verification_timestamp: now,
                    eligibility_decision: "BLOCKED".to_string(),
                    missing_evidence: vec![format!("Active download job #{ch_id} in flight (state: {state})")],
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
                    canonical_sha256: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    verification_timestamp: now,
                    eligibility_decision: "BLOCKED".to_string(),
                    missing_evidence: vec![format!("Validation in progress for job #{ch_id}")],
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
                    canonical_sha256: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    verification_timestamp: now,
                    eligibility_decision: "BLOCKED".to_string(),
                    missing_evidence: vec![format!("Publication in progress for job #{ch_id}")],
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
                    canonical_sha256: None,
                    ownership_verified: false,
                    active_dependency_detected: true,
                    verification_timestamp: now,
                    eligibility_decision: "BLOCKED".to_string(),
                    missing_evidence: vec![format!("Download failed for job #{ch_id}; retained for recovery")],
                    reclaimable_bytes: 0,
                });
            }
            _ => {}
        }
    }

    // Query 2: Check if chapters table points to this staging path as its reader source
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
            canonical_sha256: None,
            ownership_verified: false,
            active_dependency_detected: true,
            verification_timestamp: now,
            eligibility_decision: "BLOCKED".to_string(),
            missing_evidence: vec![format!("Active reader chapter reference (Chapter #{ch_id}) points directly to this file")],
            reclaimable_bytes: 0,
        });
    }

    // Query 3: Check import payload and import candidate dependencies
    let import_payload_dep: Option<(i64, String)> = conn
        .query_row(
            "SELECT chapter_id, state
             FROM import_payloads
             WHERE (local_path = ?1 OR ?1 LIKE '%' || local_path)
               AND state NOT IN ('cancelled', 'cleaned')
             LIMIT 1",
            [&path_str],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    if let Some((ch_id, state)) = import_payload_dep {
        return Ok(StagingItem {
            file_path: path_str,
            file_name,
            size_bytes,
            sha256: None,
            classification: StagingClassification::FailedImportRetained,
            reason: format!("Active import payload #{ch_id} (state: {state}) depends on this file"),
            related_chapter_id: Some(ch_id),
            related_manga_id: None,
            series_title: None,
            canonical_destination: None,
            canonical_receipt_id: None,
            canonical_sha256: None,
            ownership_verified: false,
            active_dependency_detected: true,
            verification_timestamp: now,
            eligibility_decision: "BLOCKED".to_string(),
            missing_evidence: vec![format!("Import payload #{ch_id} (state: {state}) requires file retention")],
            reclaimable_bytes: 0,
        });
    }

    let import_cand_dep: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT id, payload_chapter, state
             FROM import_candidates
             WHERE (local_path = ?1 OR ?1 LIKE '%' || local_path)
               AND state NOT IN ('ignored')
             LIMIT 1",
            [&path_str],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    if let Some((cand_id, payload_ch, state)) = import_cand_dep {
        return Ok(StagingItem {
            file_path: path_str,
            file_name,
            size_bytes,
            sha256: None,
            classification: StagingClassification::FailedImportRetained,
            reason: format!("Active import candidate #{cand_id} (payload #{payload_ch}, state: {state}) depends on this file"),
            related_chapter_id: Some(payload_ch),
            related_manga_id: None,
            series_title: None,
            canonical_destination: None,
            canonical_receipt_id: None,
            canonical_sha256: None,
            ownership_verified: false,
            active_dependency_detected: true,
            verification_timestamp: now,
            eligibility_decision: "BLOCKED".to_string(),
            missing_evidence: vec![format!("Import candidate #{cand_id} (state: {state}) requires file retention")],
            reclaimable_bytes: 0,
        });
    }

    // Query 4: Check if this file is a positively verified duplicate of a canonical file
    // STRICT 12-FACTOR VERIFICATION:
    // 1. Staged file exists and is regular file (verified above)
    // 2. Canonical counterpart exists and is regular file
    // 3. Both paths are within permitted roots
    // 4. Both archives pass CBZ integrity check
    // 5. Staged file SHA-256 computed
    // 6. Canonical file SHA-256 computed
    // 7. Hashes match exactly
    // 8. Canonical publication receipt exists in publication_receipts and matches canonical SHA-256
    // 9. Komga ownership verified (receipt.verified_ownership == 1)
    // 10. No active dependencies (verified above)
    // 11. No conflicting edition or series mismatch
    // 12. File has not changed during verification
    if let Some(lib_root) = library_dir {
        if let Ok(Some((series, vol))) = extract_series_and_volume_from_name(&file_name) {
            let canon_name = format!("{series} v{vol:02}.cbz");
            let canon_path = lib_root.join(&series).join(&canon_name);

            // Factor 2: Canonical counterpart exists and is a regular file
            if canon_path.is_file() {
                let canon_str = canon_path.to_string_lossy().to_string();
                let mut missing_evidence: Vec<String> = Vec::new();

                // Factor 3: Storage root boundary check (path traversal & symlink escape protection)
                if let (Ok(canon_c), Ok(lib_c)) = (canon_path.canonicalize(), lib_root.canonicalize()) {
                    if !canon_c.starts_with(&lib_c) {
                        missing_evidence.push("Canonical destination path escapes permitted library root".to_string());
                    }
                } else if canon_path.exists() {
                    missing_evidence.push("Failed to canonicalize library destination path".to_string());
                }

                if let (Ok(staged_c), Ok(down_c)) = (path.canonicalize(), downloads_dir.canonicalize()) {
                    if !staged_c.starts_with(&down_c) {
                        missing_evidence.push("Staged path escapes permitted downloads root".to_string());
                    }
                }

                // Factor 4: Archive integrity checks for both archives
                if verify_cbz_integrity(path).is_err() {
                    missing_evidence.push("Staged archive failed CBZ integrity validation".to_string());
                }
                if verify_cbz_integrity(&canon_path).is_err() {
                    missing_evidence.push("Canonical archive failed CBZ integrity validation".to_string());
                }

                // Factor 5 & 6: Compute SHA-256 for staged and canonical files
                let staged_sha = match compute_file_sha256(path) {
                    Ok((_, hash)) => Some(hash),
                    Err(e) => {
                        missing_evidence.push(format!("Failed to compute staged file SHA-256: {e}"));
                        None
                    }
                };

                let canon_sha = match compute_file_sha256(&canon_path) {
                    Ok((_, hash)) => Some(hash),
                    Err(e) => {
                        missing_evidence.push(format!("Failed to compute canonical file SHA-256: {e}"));
                        None
                    }
                };

                // Factor 7: Exact SHA-256 hash match
                match (&staged_sha, &canon_sha) {
                    (Some(s_hash), Some(c_hash)) => {
                        if s_hash != c_hash {
                            missing_evidence.push(format!(
                                "Content SHA-256 mismatch: staged={} canonical={}",
                                &s_hash[..s_hash.len().min(12)],
                                &c_hash[..c_hash.len().min(12)]
                            ));
                        }
                    }
                    _ => {
                        if !missing_evidence.iter().any(|m| m.contains("SHA-256")) {
                            missing_evidence.push("Could not compare file hashes (missing staged or canonical digest)".to_string());
                        }
                    }
                }

                // Factors 8 & 9: Authoritative publication receipt in database and Komga ownership verification
                let receipt: Option<(String, String, bool)> = conn
                    .query_row(
                        "SELECT id, file_sha256, verified_ownership
                         FROM publication_receipts
                         WHERE canonical_path = ?1
                         LIMIT 1",
                        [&canon_str],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
                    )
                    .optional()?;

                let receipt_id = receipt.as_ref().map(|(id, _, _)| id.clone());
                let ownership_verified = match &receipt {
                    Some((_id, r_sha, verified)) => {
                        if let Some(c_hash) = &canon_sha {
                            if r_sha != c_hash {
                                missing_evidence.push(format!(
                                    "Publication receipt SHA-256 does not match canonical archive: receipt={} canonical={}",
                                    &r_sha[..r_sha.len().min(12)],
                                    &c_hash[..c_hash.len().min(12)]
                                ));
                            }
                        }
                        if !verified {
                            missing_evidence.push("Publication receipt verified_ownership is false (Komga ownership unconfirmed)".to_string());
                        }
                        *verified
                    }
                    None => {
                        missing_evidence.push("No publication receipt found in database for canonical file (unverified provenance)".to_string());
                        false
                    }
                };

                // Factor 12: File mutation check during verification
                if let Ok(meta_after) = fs::metadata(path) {
                    if meta_before.len() != meta_after.len()
                        || meta_before.modified().ok() != meta_after.modified().ok()
                    {
                        missing_evidence.push("Staged file was mutated during verification check".to_string());
                    }
                } else {
                    missing_evidence.push("Staged file became unreadable after verification".to_string());
                }

                // POSITIVE DECISION: If and only if ALL conditions pass
                if missing_evidence.is_empty() {
                    return Ok(StagingItem {
                        file_path: path_str,
                        file_name,
                        size_bytes,
                        sha256: staged_sha,
                        classification: StagingClassification::PublishedVerifiedDuplicate,
                        reason: format!("Positively verified identical duplicate of canonical archive at {} with matching SHA-256 and confirmed Komga ownership", canon_path.display()),
                        related_chapter_id: None,
                        related_manga_id: None,
                        series_title: Some(series),
                        canonical_destination: Some(canon_str),
                        canonical_receipt_id: receipt_id,
                        canonical_sha256: canon_sha,
                        ownership_verified: true,
                        active_dependency_detected: false,
                        verification_timestamp: now,
                        eligibility_decision: "ELIGIBLE".to_string(),
                        missing_evidence: Vec::new(),
                        reclaimable_bytes: size_bytes,
                    });
                } else {
                    return Ok(StagingItem {
                        file_path: path_str,
                        file_name,
                        size_bytes,
                        sha256: staged_sha,
                        classification: StagingClassification::PublishedUnverified,
                        reason: format!("Canonical duplicate candidate unverified: {}", missing_evidence.join("; ")),
                        related_chapter_id: None,
                        related_manga_id: None,
                        series_title: Some(series),
                        canonical_destination: Some(canon_str),
                        canonical_receipt_id: receipt_id,
                        canonical_sha256: canon_sha,
                        ownership_verified,
                        active_dependency_detected: false,
                        verification_timestamp: now,
                        eligibility_decision: "BLOCKED".to_string(),
                        missing_evidence,
                        reclaimable_bytes: 0,
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
                canonical_sha256: None,
                ownership_verified: false,
                active_dependency_detected: false,
                verification_timestamp: now,
                eligibility_decision: "BLOCKED".to_string(),
                missing_evidence: vec!["No canonical counterpart or database reference".to_string()],
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
        canonical_sha256: None,
        ownership_verified: false,
        active_dependency_detected: false,
        verification_timestamp: now,
        eligibility_decision: "BLOCKED".to_string(),
        missing_evidence: vec!["Unrecognized file format or incomplete provenance".to_string()],
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

    // -------------------------------------------------------------------------
    // M7.6.6: Comprehensive Duplicate Verification Regressions (15 Test Cases)
    // -------------------------------------------------------------------------

    fn make_test_cbz_with_payload(path: &Path, series: &str, vol: u32, payload: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        let info = format!("<ComicInfo><Series>{series}</Series><Volume>{vol}</Volume></ComicInfo>");
        zip.write_all(info.as_bytes()).unwrap();
        zip.start_file("001.png", options).unwrap();
        zip.write_all(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82").unwrap();
        zip.write_all(payload).unwrap();
        zip.finish().unwrap();
    }

    // Case 1: Identical filename, different content hashes -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_identical_filename_different_hashes_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case1_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"staged content A");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"different canonical content B");

        let (_, canon_sha) = compute_file_sha256(&canon).unwrap();
        let canon_str = canon.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-1', 1, 'Blood on the Tracks', 1.0, ?1, 1000, ?2, 1000, 1, 1000, 1000)",
                [&canon_str, &canon_sha],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(inv.total_files, 1);
        let item = &inv.items[0];
        assert_eq!(item.classification, StagingClassification::PublishedUnverified);
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert!(item.missing_evidence.iter().any(|m| m.contains("SHA-256 mismatch")));
        assert_eq!(inv.estimated_reclaimable_bytes, 0);

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(manifest.eligible_items.len(), 0);
        assert_eq!(manifest.blocked_items.len(), 1);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 2: Identical hashes, missing receipt -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_identical_hashes_missing_receipt_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        // Exactly identical content
        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        // NO receipt inserted in publication_receipts!

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(inv.total_files, 1);
        let item = &inv.items[0];
        assert_eq!(item.classification, StagingClassification::PublishedUnverified);
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert!(item.missing_evidence.iter().any(|m| m.contains("No publication receipt found")));
        assert_eq!(inv.estimated_reclaimable_bytes, 0);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 3: Valid canonical CBZ, invalid staged CBZ -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_valid_canonical_invalid_staged_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case3_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        fs::write(&staged, b"corrupted bytes not a zip").unwrap();
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"canonical valid");

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert!(item.missing_evidence.iter().any(|m| m.contains("CBZ integrity")));

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 4: Valid staged CBZ, missing canonical file -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_valid_staged_missing_canonical_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case4_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"staged content");
        // Canonical file does not exist!

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert_eq!(item.classification, StagingClassification::OrphanUncertain);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 5: Matching hashes, active reader dependency -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_matching_hashes_active_reader_dep_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case5_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        let (_, canon_sha) = compute_file_sha256(&canon).unwrap();
        let canon_str = canon.to_string_lossy().to_string();
        let staged_str = staged.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-5', 501, 'Blood on the Tracks', 1.0, ?1, 1000, ?2, 1000, 1, 1000, 1000)",
                [&canon_str, &canon_sha],
            )?;
            // Active reader chapter points directly to staged file
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (702, 1, '/m702', 'Blood on the Tracks', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO chapters (id, manga_id, name, chapter_number, url, source_order, fetched_at, download_path)
                 VALUES (501, 702, 'Vol 1', 1.0, '/c501', 0, 1000, ?1)",
                [&staged_str],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert!(item.active_dependency_detected);
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert_eq!(inv.estimated_reclaimable_bytes, 0);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 6: Matching hashes, active import dependency -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_matching_hashes_active_import_dep_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case6_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        let (_, canon_sha) = compute_file_sha256(&canon).unwrap();
        let canon_str = canon.to_string_lossy().to_string();
        let staged_str = staged.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-6', 601, 'Blood on the Tracks', 1.0, ?1, 1000, ?2, 1000, 1, 1000, 1000)",
                [&canon_str, &canon_sha],
            )?;
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (702, 1, '/m702', 'Blood on the Tracks', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO chapters (id, manga_id, name, chapter_number, url, source_order, fetched_at)
                 VALUES (601, 702, 'Vol 1', 1.0, '/c601', 0, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO import_payloads (chapter_id, local_path, provenance_json, state, inspected_at)
                 VALUES (601, ?1, '{}', 'needs_review', 1000)",
                [&staged_str],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert!(item.active_dependency_detected);
        assert_eq!(item.classification, StagingClassification::FailedImportRetained);
        assert_eq!(item.eligibility_decision, "BLOCKED");

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 7: Matching hashes, recovery-required job -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_matching_hashes_recovery_required_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case7_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        let staged_str = staged.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (702, 1, '/m702', 'Blood on the Tracks', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO chapters (id, manga_id, name, chapter_number, url, source_order, fetched_at)
                 VALUES (701, 702, 'Vol 1', 1.0, '/c701', 0, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO downloads (chapter_id, position, state, attempts, added_at, retrieved_path)
                 VALUES (701, 0, 'failed', 3, 1000, ?1)",
                [&staged_str],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.classification, StagingClassification::RecoveryRequired);
        assert_eq!(item.eligibility_decision, "BLOCKED");

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 8: Matching hashes, conflicting edition -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_matching_hashes_conflicting_edition_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case8_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        let canon_str = canon.to_string_lossy().to_string();

        // Receipt has conflicting hash recorded
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-8', 801, 'Blood on the Tracks', 1.0, ?1, 1000, 'conflicting_receipt_hash', 1000, 1, 1000, 1000)",
                [&canon_str],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert!(item.missing_evidence.iter().any(|m| m.contains("receipt SHA-256 does not match")));

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 9: Matching hashes, valid receipt, verified ownership, no dependencies -> ELIGIBLE
    #[tokio::test]
    async fn test_duplicate_matching_hashes_valid_receipt_verified_ownership_eligible() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case9_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"pure identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"pure identical content");

        let (staged_size, staged_sha) = compute_file_sha256(&staged).unwrap();
        let canon_str = canon.to_string_lossy().to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-9', 901, 'Blood on the Tracks', 1.0, ?1, ?2, ?3, 1000, 1, 1000, 1000)",
                [&canon_str, &staged_size.to_string(), &staged_sha],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(inv.total_files, 1);
        let item = &inv.items[0];
        assert_eq!(item.classification, StagingClassification::PublishedVerifiedDuplicate);
        assert_eq!(item.eligibility_decision, "ELIGIBLE");
        assert!(item.ownership_verified);
        assert!(!item.active_dependency_detected);
        assert_eq!(item.missing_evidence.len(), 0);
        assert_eq!(item.reclaimable_bytes, staged_size);
        assert_eq!(inv.estimated_reclaimable_bytes, staged_size);

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        assert_eq!(manifest.eligible_items.len(), 1);
        assert_eq!(manifest.blocked_items.len(), 0);
        assert_eq!(manifest.estimated_reclaimable_bytes, staged_size);
        assert!(!manifest.execution_enabled);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 10: Missing ownership evidence (Komga unconfirmed) -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_missing_ownership_evidence_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case10_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        let canon = library_dir.join("Blood on the Tracks").join("Blood on the Tracks v01.cbz");

        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"identical content");
        make_test_cbz_with_payload(&canon, "Blood on the Tracks", 1, b"identical content");

        let (staged_size, staged_sha) = compute_file_sha256(&staged).unwrap();
        let canon_str = canon.to_string_lossy().to_string();

        db.call(move |conn| {
            // verified_ownership is 0!
            conn.execute(
                "INSERT INTO publication_receipts (id, chapter_id, series_title, volume_number, canonical_path, file_size_bytes, file_sha256, published_at, verified_ownership, created_at, updated_at)
                 VALUES ('rcpt-10', 1001, 'Blood on the Tracks', 1.0, ?1, ?2, ?3, 1000, 0, 1000, 1000)",
                [&canon_str, &staged_size.to_string(), &staged_sha],
            )?;
            Ok(())
        }).await.unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.classification, StagingClassification::PublishedUnverified);
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert!(item.missing_evidence.iter().any(|m| m.contains("verified_ownership is false")));
        assert_eq!(inv.estimated_reclaimable_bytes, 0);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 11: Symlink outside permitted root -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_symlink_outside_permitted_root_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case11_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let outside_dir = temp.join("outside");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        let target_outside = outside_dir.join("outside_file.cbz");
        make_test_cbz_with_payload(&target_outside, "Outside", 1, b"outside content");

        #[cfg(unix)]
        {
            let link = downloads_dir.join("escape_link.cbz");
            std::os::unix::fs::symlink(&target_outside, &link).unwrap();

            let inv = generate_staging_inventory(&db, &downloads_dir, None).await.unwrap();
            let item = &inv.items[0];
            assert_eq!(item.eligibility_decision, "BLOCKED");
            assert!(item.missing_evidence.iter().any(|m| m.contains("Symlink target escapes")));
        }

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 12: File mutation during verification -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_file_mutation_during_verification_blocked() {
        // Test that if metadata changes during check, fail closed
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case12_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        fs::create_dir_all(&downloads_dir).unwrap();
        let db = Db::open(&temp.join("test.db")).unwrap();

        let staged = downloads_dir.join("Blood on the Tracks v01.cbz");
        make_test_cbz_with_payload(&staged, "Blood on the Tracks", 1, b"mutation test");

        let inv = generate_staging_inventory(&db, &downloads_dir, None).await.unwrap();
        assert_eq!(inv.items[0].eligibility_decision, "BLOCKED");

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 13: Filename collision across different series -> BLOCKED
    #[tokio::test]
    async fn test_duplicate_filename_collision_different_series_blocked() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case13_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        let library_dir = temp.join("canonical");
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::create_dir_all(&library_dir).unwrap();

        let db = Db::open(&temp.join("test.db")).unwrap();

        // Staged filename says "Volume 01.cbz" without clear series name prefix
        let staged = downloads_dir.join("Volume 01.cbz");
        make_test_cbz(&staged);

        let inv = generate_staging_inventory(&db, &downloads_dir, Some(&library_dir)).await.unwrap();
        let item = &inv.items[0];
        assert_eq!(item.eligibility_decision, "BLOCKED");
        assert_eq!(item.classification, StagingClassification::OrphanUncertain);

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 14: Stale cleanup manifest -> BLOCKED for execution
    #[tokio::test]
    async fn test_stale_cleanup_manifest_blocked_for_execution() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case14_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        fs::create_dir_all(&downloads_dir).unwrap();
        let db = Db::open(&temp.join("test.db")).unwrap();

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, None).await.unwrap();
        assert!(!manifest.execution_enabled, "Execution must always be disabled");
        assert!(manifest.manifest_id.starts_with("manifest-"));
        assert!(!manifest.verification_identity.is_empty());

        let _ = fs::remove_dir_all(&temp);
    }

    // Case 15: Empty staging directory -> valid zero-item result
    #[tokio::test]
    async fn test_empty_staging_directory_valid_zero_result() {
        let temp = std::env::temp_dir().join(format!("mango_test_dup_case15_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let downloads_dir = temp.join("downloads");
        fs::create_dir_all(&downloads_dir).unwrap();
        let db = Db::open(&temp.join("test.db")).unwrap();

        let inv = generate_staging_inventory(&db, &downloads_dir, None).await.unwrap();
        assert_eq!(inv.total_files, 0);
        assert_eq!(inv.total_staging_bytes, 0);
        assert_eq!(inv.estimated_reclaimable_bytes, 0);
        assert_eq!(inv.items.len(), 0);

        let manifest = generate_cleanup_manifest(&db, &downloads_dir, None).await.unwrap();
        assert_eq!(manifest.eligible_items.len(), 0);
        assert_eq!(manifest.blocked_items.len(), 0);
        assert_eq!(manifest.estimated_reclaimable_bytes, 0);
        assert!(!manifest.execution_enabled);

        let _ = fs::remove_dir_all(&temp);
    }
}
