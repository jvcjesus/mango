#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::pipeline::receipts::{
        compute_file_sha256, get_publication_receipt, get_publication_receipt_by_chapter,
        get_publication_receipt_by_path, list_publication_receipts,
        record_publication_receipt, update_receipt_metadata_status,
        verify_publication_receipt_on_disk,
    };
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let pid = std::process::id();
            let count = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("mango-receipt-test-{pid}-{count}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_test_cbz(path: &Path, content: &str) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(format!("<ComicInfo><Series>{content}</Series></ComicInfo>").as_bytes())
            .unwrap();
        zip.start_file("001.webp", options).unwrap();
        zip.write_all(format!("fake image bytes: {content}").as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }

    fn insert_test_manga(conn: &rusqlite::Connection, id: i64, title: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO manga (id, source_id, url, title) VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![id, format!("url-{id}"), title],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_record_and_retrieve_publication_receipt() {
        let db_dir = TempDir::new();
        let db = Db::open(&db_dir.path().join("test.db")).unwrap();
        let media_dir = TempDir::new();
        let cbz_path = media_dir.path().join("Test Series v01.cbz");
        make_test_cbz(&cbz_path, "Test Series");

        let (size, sha) = compute_file_sha256(&cbz_path).unwrap();
        assert!(size > 0);
        assert_eq!(sha.len(), 64);

        let receipt = db
            .call({
                let p = cbz_path.to_string_lossy().to_string();
                let sha = sha.clone();
                move |conn| {
                    insert_test_manga(conn, 1, "Test Series");
                    record_publication_receipt(
                        conn,
                        101,
                        Some(1),
                        "Test Series",
                        true,
                        1.0,
                        &p,
                        size,
                        &sha,
                        false,
                        Some("lib-123"),
                        "complete",
                        None,
                    )
                }
            })
            .await
            .unwrap();

        assert_eq!(receipt.chapter_id, 101);
        assert_eq!(receipt.manga_id, Some(1));
        assert_eq!(receipt.series_title, "Test Series");
        assert_eq!(receipt.volume_number, Some(1.0));
        assert!(receipt.is_volume);
        assert_eq!(receipt.file_size_bytes, size);
        assert_eq!(receipt.file_sha256, sha);
        assert_eq!(receipt.metadata_status, "complete");

        // Retrieve by path
        let by_path = db
            .call({
                let p = cbz_path.to_string_lossy().to_string();
                move |conn| get_publication_receipt_by_path(conn, &p)
            })
            .await
            .unwrap();
        assert!(by_path.is_some());
        assert_eq!(by_path.unwrap().id, receipt.id);

        // Retrieve by chapter
        let by_ch = db
            .call(move |conn| get_publication_receipt_by_chapter(conn, 101))
            .await
            .unwrap();
        assert!(by_ch.is_some());
        assert_eq!(by_ch.unwrap().id, receipt.id);
    }

    #[tokio::test]
    async fn test_publication_receipt_verification_on_disk() {
        let db_dir = TempDir::new();
        let db = Db::open(&db_dir.path().join("test.db")).unwrap();
        let media_dir = TempDir::new();
        let cbz_path = media_dir.path().join("Kagurabachi v01.cbz");
        make_test_cbz(&cbz_path, "Kagurabachi");

        let (size, sha) = compute_file_sha256(&cbz_path).unwrap();

        let receipt = db
            .call({
                let p = cbz_path.to_string_lossy().to_string();
                let sha = sha.clone();
                move |conn| {
                    insert_test_manga(conn, 850, "Kagurabachi");
                    record_publication_receipt(
                        conn,
                        201,
                        Some(850),
                        "Kagurabachi",
                        true,
                        1.0,
                        &p,
                        size,
                        &sha,
                        true,
                        Some("lib-manga"),
                        "complete",
                        None,
                    )
                }
            })
            .await
            .unwrap();

        // 1. Verify valid file on disk
        let res = db
            .call({
                let rid = receipt.id.clone();
                move |conn| verify_publication_receipt_on_disk(conn, &rid)
            })
            .await
            .unwrap();

        assert!(res.file_exists);
        assert!(res.cbz_valid);
        assert!(res.size_matched);
        assert!(res.sha256_matched);
        assert!(res.is_valid);
        assert!(res.error.is_none());

        // 2. Corrupt file content and re-verify -> sha256 mismatch
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .append(true)
                .open(&cbz_path)
                .unwrap();
            f.write_all(b"extra garbage bytes").unwrap();
        }

        let res_corrupted = db
            .call({
                let rid = receipt.id.clone();
                move |conn| verify_publication_receipt_on_disk(conn, &rid)
            })
            .await
            .unwrap();

        assert!(res_corrupted.file_exists);
        assert!(!res_corrupted.size_matched || !res_corrupted.sha256_matched);
        assert!(!res_corrupted.is_valid);
    }

    #[tokio::test]
    async fn test_post_publication_metadata_status_isolation() {
        let db_dir = TempDir::new();
        let db = Db::open(&db_dir.path().join("test.db")).unwrap();
        let media_dir = TempDir::new();
        let cbz_path = media_dir.path().join("Isolated Series v02.cbz");
        make_test_cbz(&cbz_path, "Isolated Series");

        let (size, sha) = compute_file_sha256(&cbz_path).unwrap();

        let receipt = db
            .call({
                let p = cbz_path.to_string_lossy().to_string();
                let sha = sha.clone();
                move |conn| {
                    insert_test_manga(conn, 3, "Isolated Series");
                    record_publication_receipt(
                        conn,
                        301,
                        Some(3),
                        "Isolated Series",
                        true,
                        2.0,
                        &p,
                        size,
                        &sha,
                        false,
                        None,
                        "pending",
                        None,
                    )
                }
            })
            .await
            .unwrap();

        assert_eq!(receipt.metadata_status, "pending");

        // Update metadata status to failed with error message
        db.call({
            let p = cbz_path.to_string_lossy().to_string();
            move |conn| {
                update_receipt_metadata_status(
                    conn,
                    &p,
                    "failed",
                    Some("MangaBaka API rate limit during metadata enrichment"),
                )
            }
        })
        .await
        .unwrap();

        let updated = db
            .call({
                let rid = receipt.id.clone();
                move |conn| get_publication_receipt(conn, &rid)
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(updated.metadata_status, "failed");
        assert_eq!(
            updated.metadata_error.as_deref(),
            Some("MangaBaka API rate limit during metadata enrichment")
        );
        // Canonical publication path and file data remain completely intact!
        assert_eq!(updated.file_size_bytes, size);
        assert_eq!(updated.file_sha256, sha);
        assert_eq!(updated.canonical_path, cbz_path.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn test_duplicate_publication_receipt_upsert_idempotent() {
        let db_dir = TempDir::new();
        let db = Db::open(&db_dir.path().join("test.db")).unwrap();
        let media_dir = TempDir::new();
        let cbz_path = media_dir.path().join("Idempotent Series v01.cbz");
        make_test_cbz(&cbz_path, "Idempotent Series");

        let (size, sha) = compute_file_sha256(&cbz_path).unwrap();
        let p = cbz_path.to_string_lossy().to_string();

        let _r1 = db
            .call({
                let p = p.clone();
                let sha = sha.clone();
                move |conn| {
                    insert_test_manga(conn, 4, "Idempotent Series");
                    record_publication_receipt(
                        conn, 401, Some(4), "Idempotent Series", true, 1.0, &p, size, &sha, false, None, "complete", None,
                    )
                }
            })
            .await
            .unwrap();

        // Second call with same canonical path updates without inserting duplicate
        let r2 = db
            .call({
                let p = p.clone();
                let sha = sha.clone();
                move |conn| {
                    record_publication_receipt(
                        conn, 401, Some(4), "Idempotent Series", true, 1.0, &p, size, &sha, true, Some("lib-2"), "complete", None,
                    )
                }
            })
            .await
            .unwrap();

        let list = db
            .call(|conn| list_publication_receipts(conn, 10))
            .await
            .unwrap();
        assert_eq!(list.len(), 1, "Only 1 receipt row must exist for canonical path");
        assert!(r2.komga_notified);
        assert_eq!(r2.komga_library_id.as_deref(), Some("lib-2"));
    }
}
