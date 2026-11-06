//! Library Duplicate Detection and Storage Quality Intelligence (Milestone M8.6).
//!
//! Detects duplicate volumes across and within series:
//! - Exact duplicates: identical SHA-256 hash.
//! - Semantic duplicates: same series and volume number, different files/hashes.
//! - Chapter vs volume overlap: individual chapters covered by an owned canonical volume.
//!
//! Invariants:
//! - Strictly non-destructive preview: zero auto-deletion.
//! - Recommends keeping highest quality archive (digital, quality group, canonical receipt).

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DuplicateType {
    ExactHash,
    SemanticVolume,
    ChapterVolumeOverlap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateItem {
    pub item_id: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub is_canonical_receipt_match: bool,
    pub is_digital: bool,
    pub resolution_label: Option<String>,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub group_type: DuplicateType,
    pub series_id: String,
    pub manga_id: Option<i64>,
    pub series_title: String,
    pub volume_number: Option<f64>,
    pub items: Vec<DuplicateItem>,
    pub recommended_keep_id: Option<String>,
    pub resolution_recommendation: String,
    pub potential_reclaimable_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDuplicateReport {
    pub total_duplicate_groups: usize,
    pub exact_duplicates_count: usize,
    pub semantic_duplicates_count: usize,
    pub chapter_volume_overlaps_count: usize,
    pub potential_reclaimable_bytes: u64,
    pub groups: Vec<DuplicateGroup>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOptimizerStatus {
    pub is_available: bool,
    pub configured_path: Option<String>,
    pub resolved_path: Option<String>,
    pub source: String,
    pub version: Option<String>,
    pub message: String,
    pub default_target_height: u32,
    pub default_image_quality: u32,
    pub default_min_savings_percent: u32,
}

/// Scans the database and publication records for exact and semantic duplicates.
pub fn scan_library_duplicates(conn: &Connection) -> Result<LibraryDuplicateReport> {
    let mut groups = Vec::new();
    let mut exact_count = 0;
    let mut semantic_count = 0;
    let mut overlap_count = 0;
    let mut total_reclaimable = 0u64;

    // 1. Exact Duplicates by identical file_sha256 across publication receipts
    let mut stmt_exact = conn.prepare(
        "SELECT file_sha256, COUNT(*)
         FROM publication_receipts
         WHERE file_sha256 IS NOT NULL AND file_sha256 != ''
         GROUP BY file_sha256
         HAVING COUNT(*) > 1",
    )?;

    let exact_hashes: Vec<String> = stmt_exact
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    for hash in exact_hashes {
        let mut stmt_items = conn.prepare(
            "SELECT r.id, r.series_title, r.canonical_path, r.file_size_bytes, r.volume_number,
                    r.manga_id, r.created_at
             FROM publication_receipts r
             WHERE r.file_sha256 = ?1
             ORDER BY r.created_at DESC",
        )?;

        let items_res = stmt_items.query_map([&hash], |r| {
            let id: String = r.get(0)?;
            let series_title: String = r.get(1)?;
            let canonical_path: String = r.get(2)?;
            let file_size_bytes: i64 = r.get(3)?;
            let volume_number: Option<f64> = r.get(4)?;
            let manga_id: Option<i64> = r.get(5)?;
            let file_name = std::path::Path::new(&canonical_path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown.cbz".to_string());

            Ok((
                id,
                series_title,
                canonical_path,
                file_name,
                file_size_bytes.max(0) as u64,
                volume_number,
                manga_id,
            ))
        })?;

        let mut group_items = Vec::new();
        let mut series_title = "Unknown Series".to_string();
        let mut vol_num = None;
        let mut m_id = None;

        for res in items_res {
            if let Ok((id, title, path, name, size, v, mid)) = res {
                series_title = title;
                vol_num = v;
                m_id = mid;
                group_items.push(DuplicateItem {
                    item_id: id,
                    file_name: name,
                    path,
                    size_bytes: size,
                    sha256: Some(hash.clone()),
                    is_canonical_receipt_match: true,
                    is_digital: true,
                    resolution_label: None,
                    format: "CBZ".to_string(),
                });
            }
        }

        if group_items.len() > 1 {
            exact_count += 1;
            let keep_id = group_items.first().map(|i| i.item_id.clone());
            let group_size = group_items[0].size_bytes;
            let reclaimable = group_size * (group_items.len() as u64 - 1);
            total_reclaimable += reclaimable;

            groups.push(DuplicateGroup {
                group_type: DuplicateType::ExactHash,
                series_id: m_id.map(|id| format!("manga:{id}")).unwrap_or_default(),
                manga_id: m_id,
                series_title: series_title.clone(),
                volume_number: vol_num,
                items: group_items,
                recommended_keep_id: keep_id,
                resolution_recommendation: format!(
                    "Exact duplicate hash ({:.8}…): keep primary canonical publication receipt, review duplicate copies for removal.",
                    hash
                ),
                potential_reclaimable_bytes: reclaimable,
            });
        }
    }

    // 2. Semantic Duplicates: Same series and volume number with different receipts/hashes
    let mut stmt_semantic = conn.prepare(
        "SELECT r.manga_id, r.volume_number, COUNT(DISTINCT r.file_sha256)
         FROM publication_receipts r
         WHERE r.manga_id IS NOT NULL AND r.volume_number IS NOT NULL
         GROUP BY r.manga_id, r.volume_number
         HAVING COUNT(DISTINCT r.file_sha256) > 1",
    )?;

    let semantic_pairs: Vec<(i64, f64)> = stmt_semantic
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (m_id, v_num) in semantic_pairs {
        let mut stmt_items = conn.prepare(
            "SELECT r.id, r.series_title, r.canonical_path, r.file_size_bytes, r.file_sha256, r.created_at
             FROM publication_receipts r
             WHERE r.manga_id = ?1 AND ABS(r.volume_number - ?2) < 0.001
             ORDER BY r.file_size_bytes DESC, r.created_at DESC",
        )?;

        let items_res = stmt_items.query_map(params![m_id, v_num], |r| {
            let id: String = r.get(0)?;
            let series_title: String = r.get(1)?;
            let canonical_path: String = r.get(2)?;
            let file_size_bytes: i64 = r.get(3)?;
            let file_hash: Option<String> = r.get(4)?;
            let file_name = std::path::Path::new(&canonical_path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown.cbz".to_string());

            Ok((id, series_title, canonical_path, file_name, file_size_bytes.max(0) as u64, file_hash))
        })?;

        let mut group_items = Vec::new();
        let mut series_title = "Unknown Series".to_string();

        for res in items_res {
            if let Ok((id, title, path, name, size, hash)) = res {
                series_title = title;
                let is_digital = name.to_ascii_lowercase().contains("digital");
                let resolution = crate::acquisition::quality::detect_resolution_label(&name);
                let format = crate::acquisition::quality::detect_format(&name);

                group_items.push(DuplicateItem {
                    item_id: id,
                    file_name: name,
                    path,
                    size_bytes: size,
                    sha256: hash,
                    is_canonical_receipt_match: true,
                    is_digital,
                    resolution_label: resolution,
                    format,
                });
            }
        }

        if group_items.len() > 1 {
            semantic_count += 1;
            // Best item: Digital > Non-digital, then highest size/resolution
            group_items.sort_by(|a, b| {
                b.is_digital
                    .cmp(&a.is_digital)
                    .then_with(|| b.size_bytes.cmp(&a.size_bytes))
            });

            let keep_id = group_items.first().map(|i| i.item_id.clone());
            let reclaimable: u64 = group_items.iter().skip(1).map(|i| i.size_bytes).sum();
            total_reclaimable += reclaimable;

            groups.push(DuplicateGroup {
                group_type: DuplicateType::SemanticVolume,
                series_id: format!("manga:{m_id}"),
                manga_id: Some(m_id),
                series_title: series_title.clone(),
                volume_number: Some(v_num),
                items: group_items,
                recommended_keep_id: keep_id,
                resolution_recommendation: format!(
                    "Semantic volume duplicate for Vol {:.1}: recommend keeping higher quality digital release; review alternate version.",
                    v_num
                ),
                potential_reclaimable_bytes: reclaimable,
            });
        }
    }

    // 3. Chapter vs Volume Overlap
    // When volume receipts exist with is_volume = 1 and loose chapter downloads exist for the same manga
    let mut stmt_overlap = conn.prepare(
        "SELECT r.manga_id, r.volume_number, COUNT(c.id), r.series_title
         FROM publication_receipts r
         JOIN chapters c ON c.manga_id = r.manga_id AND c.download_path IS NOT NULL
         WHERE r.is_volume = 1 AND r.volume_number IS NOT NULL
         GROUP BY r.manga_id, r.volume_number, r.series_title
         HAVING COUNT(c.id) > 0
         LIMIT 25",
    )?;

    if let Ok(overlap_rows) = stmt_overlap.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    }) {
        for row in overlap_rows.flatten() {
            let (m_id, vol_num, ch_count, title) = row;
            overlap_count += 1;
            groups.push(DuplicateGroup {
                group_type: DuplicateType::ChapterVolumeOverlap,
                series_id: format!("manga:{m_id}"),
                manga_id: Some(m_id),
                series_title: title,
                volume_number: Some(vol_num),
                items: Vec::new(),
                recommended_keep_id: None,
                resolution_recommendation: format!(
                    "Volume {:.1} is owned and published, but {ch_count} loose chapter download(s) remain in library. Review for cleanup.",
                    vol_num
                ),
                potential_reclaimable_bytes: 0,
            });
        }
    }

    Ok(LibraryDuplicateReport {
        total_duplicate_groups: groups.len(),
        exact_duplicates_count: exact_count,
        semantic_duplicates_count: semantic_count,
        chapter_volume_overlaps_count: overlap_count,
        potential_reclaimable_bytes: total_reclaimable,
        groups,
        generated_at: crate::db::now(),
    })
}

/// Inspects optimizer binary status and capabilities.
pub async fn get_storage_optimizer_status(configured_bin: Option<&str>) -> StorageOptimizerStatus {
    let validation = crate::pipeline::optimizer::validate_optimizer(configured_bin).await;
    let is_available = validation.code == crate::pipeline::optimizer::OptimizerValidationCode::Valid;

    let message = if is_available {
        "RustyLibraryShrinker is available and verified for CBZ optimization.".to_string()
    } else {
        validation.message.clone()
    };

    StorageOptimizerStatus {
        is_available,
        configured_path: validation.configured_path,
        resolved_path: validation.resolved_path,
        source: validation.source,
        version: validation.version,
        message,
        default_target_height: crate::pipeline::optimizer::DEFAULT_TARGET_HEIGHT,
        default_image_quality: crate::pipeline::optimizer::DEFAULT_IMAGE_QUALITY,
        default_min_savings_percent: crate::pipeline::optimizer::DEFAULT_MIN_SAVINGS_PERCENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE manga (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                source_id INTEGER NOT NULL DEFAULT 0,
                url TEXT NOT NULL DEFAULT '',
                auto_acquire INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE chapters (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                download_path TEXT
            );

            CREATE TABLE publication_receipts (
                id TEXT PRIMARY KEY,
                chapter_id INTEGER NOT NULL,
                manga_id INTEGER,
                series_id TEXT NOT NULL,
                series_title TEXT NOT NULL,
                volume_number REAL,
                is_volume INTEGER NOT NULL DEFAULT 1,
                canonical_path TEXT NOT NULL,
                file_size_bytes INTEGER NOT NULL,
                file_sha256 TEXT,
                published_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_scan_library_duplicates_exact_hash() {
        let conn = setup_test_db();
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_id, series_title, volume_number,
                canonical_path, file_size_bytes, file_sha256, published_at, created_at
             ) VALUES
             ('rcpt-1', 101, 1, 'manga:1', 'Haikyu!!', 1.0, '/manga/Haikyu!!/Haikyu!! v01.cbz', 85000000, ?1, 1000, 1000),
             ('rcpt-2', 102, 1, 'manga:1', 'Haikyu!!', 1.0, '/manga/Haikyu!!/Haikyu!! v01 (Copy).cbz', 85000000, ?1, 2000, 2000)",
            [hash],
        )
        .unwrap();

        let report = scan_library_duplicates(&conn).unwrap();
        assert_eq!(report.exact_duplicates_count, 1);
        assert_eq!(report.semantic_duplicates_count, 0);
        assert_eq!(report.potential_reclaimable_bytes, 85000000);

        let group = &report.groups[0];
        assert_eq!(group.group_type, DuplicateType::ExactHash);
        assert_eq!(group.items.len(), 2);
        assert!(group.resolution_recommendation.contains("Exact duplicate hash"));
    }

    #[test]
    fn test_scan_library_duplicates_semantic_quality_preference() {
        let conn = setup_test_db();

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_id, series_title, volume_number,
                canonical_path, file_size_bytes, file_sha256, published_at, created_at
             ) VALUES
             ('rcpt-scan', 201, 2, 'manga:2', 'Blue Lock', 1.0, '/manga/Blue Lock/Blue Lock v01 [Scanlation].cbz', 45000000, 'hash-scan', 1000, 1000),
             ('rcpt-digital', 202, 2, 'manga:2', 'Blue Lock', 1.0, '/manga/Blue Lock/Blue Lock v01 [Digital].cbz', 95000000, 'hash-digital', 2000, 2000)",
            [],
        )
        .unwrap();

        let report = scan_library_duplicates(&conn).unwrap();
        assert_eq!(report.semantic_duplicates_count, 1);

        let group = &report.groups[0];
        assert_eq!(group.group_type, DuplicateType::SemanticVolume);
        assert_eq!(group.recommended_keep_id, Some("rcpt-digital".to_string()));
        assert!(group.resolution_recommendation.contains("recommend keeping higher quality digital release"));
    }

    #[test]
    fn test_scan_library_duplicates_chapter_volume_overlap() {
        let conn = setup_test_db();

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_id, series_title, volume_number,
                is_volume, canonical_path, file_size_bytes, file_sha256, published_at, created_at
             ) VALUES
             ('rcpt-vol1', 301, 3, 'manga:3', 'Jujutsu Kaisen', 1.0, 1, '/manga/Jujutsu Kaisen/v01.cbz', 60000000, 'hash-vol1', 1000, 1000)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO chapters (id, manga_id, download_path) VALUES
             (401, 3, '/downloads/manga_3/c1.cbz'),
             (402, 3, '/downloads/manga_3/c2.cbz')",
            [],
        )
        .unwrap();

        let report = scan_library_duplicates(&conn).unwrap();
        assert_eq!(report.chapter_volume_overlaps_count, 1);
        let group = &report.groups[0];
        assert_eq!(group.group_type, DuplicateType::ChapterVolumeOverlap);
        assert_eq!(group.volume_number, Some(1.0));
        assert!(group.resolution_recommendation.contains("2 loose chapter download(s)"));
    }

    #[tokio::test]
    async fn test_storage_optimizer_status_inspection() {
        let status = get_storage_optimizer_status(None).await;
        assert_eq!(status.default_target_height, 2400);
        assert_eq!(status.default_image_quality, 90);
        assert_eq!(status.default_min_savings_percent, 10);
    }
}
