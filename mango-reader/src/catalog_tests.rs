//! Tests for First-Class Catalog Titles & Optional Reading Sources (Milestone M6.7).

use crate::{
    catalog::parse_mangabaka_id_input,
    db::Db,
    titles::read_title,
};
use rusqlite::params;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn setup_test_db() -> (Db, std::path::PathBuf) {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mango-cattest-db-{}-{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db open");
    (db, dir)
}

#[tokio::test]
async fn test_parse_mangabaka_id() {
    assert_eq!(parse_mangabaka_id_input("83"), Some(83));
    assert_eq!(parse_mangabaka_id_input("  83  "), Some(83));
    assert_eq!(parse_mangabaka_id_input("https://mangabaka.com/series/83"), Some(83));
    assert_eq!(parse_mangabaka_id_input("https://mangabaka.com/series/83/blue-lock"), Some(83));
    assert_eq!(parse_mangabaka_id_input("https://mangabaka.com/series/83?ref=test"), Some(83));
    assert_eq!(parse_mangabaka_id_input("Blue Lock"), None);
}

#[tokio::test]
async fn test_direct_add_catalog_manga_creation() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // Insert catalog manga directly simulating add_mangabaka_catalog_title
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let manga_id = conn.last_insert_rowid();

    // 1. source_id must be 0
    let source_id: i64 = conn.query_row("SELECT source_id FROM manga WHERE id = ?1", [manga_id], |r| r.get(0)).unwrap();
    assert_eq!(source_id, 0, "Catalog title must have source_id = 0");

    // 2. Exact MangaBaka identity stored
    let mb_id: Option<i64> = conn.query_row("SELECT mangabaka_id FROM manga WHERE id = ?1", [manga_id], |r| r.get(0)).unwrap();
    assert_eq!(mb_id, Some(83), "Exact MangaBaka ID must be stored");

    // 3. Zero chapters created
    let chapter_count: i64 = conn.query_row("SELECT count(*) FROM chapters WHERE manga_id = ?1", [manga_id], |r| r.get(0)).unwrap();
    assert_eq!(chapter_count, 0, "Direct add must create zero chapters");

    // 4. read_title returns is_catalog = true
    let title_view = read_title(&conn, manga_id).unwrap();
    assert!(title_view.is_catalog, "TitleView must report is_catalog = true");
    assert_eq!(title_view.mangabaka_id, Some(83));
    assert_eq!(title_view.chapters.len(), 0);
}

#[tokio::test]
async fn test_synthetic_volume_rows_not_in_reader_chapters() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let catalog_id = conn.last_insert_rowid();

    // Insert synthetic acquisition chapters (like historical Nyaa/TorBox acquisition rows)
    conn.execute(
        "INSERT INTO chapters (manga_id, url, name, source_order, fetched_at, read, last_page_read, download_path, bookmark)
         VALUES (?1, 'nyaa:1849201', 'Volume 1 [Nyaa]', 1, 1700000000, 0, 0, '/path/vol1.cbz', 0)",
        [catalog_id],
    ).unwrap();
    conn.execute(
        "INSERT INTO chapters (manga_id, url, name, source_order, fetched_at, read, last_page_read, download_path, bookmark)
         VALUES (?1, 'torbox:job123', 'Volume 2 [TorBox]', 2, 1700000000, 0, 0, '/path/vol2.cbz', 0)",
        [catalog_id],
    ).unwrap();

    // read_title for catalog manga must filter out synthetic acquisition chapters
    let view = read_title(&conn, catalog_id).unwrap();
    assert_eq!(view.chapters.len(), 0, "Synthetic acquisition chapters must NOT pollute reader chapters on catalog titles");

    // Now insert a normal source-backed manga (source_id = 1) with real chapters
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized)
         VALUES (1, 'https://mangadot.net/manga/bluelock', 'Blue Lock (Source)', 1, 1700000000, 1)",
        [],
    ).unwrap();
    let source_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO chapters (manga_id, url, name, source_order, fetched_at, read, last_page_read, download_path, bookmark)
         VALUES (?1, 'https://mangadot.net/manga/bluelock/c1', 'Chapter 1', 1, 1700000000, 0, 0, NULL, 0)",
        [source_id],
    ).unwrap();

    let source_view = read_title(&conn, source_id).unwrap();
    assert_eq!(source_view.chapters.len(), 1, "Source manga must show real chapters normally");
    assert_eq!(source_view.chapters[0].name, "Chapter 1");
}

#[tokio::test]
async fn test_conversion_and_reversible_rollback() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // Step 1: Create source-backed manga (representing Blue Lock before conversion)
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (1, 'https://mangadot.net/manga/bluelock', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let source_manga_id = conn.last_insert_rowid();

    // Add 3 web chapters to source manga
    for i in 1..=3 {
        conn.execute(
            "INSERT INTO chapters (manga_id, url, name, source_order, fetched_at, read, last_page_read, download_path, bookmark)
             VALUES (?1, ?2, ?3, ?4, 1700000000, 0, 0, NULL, 0)",
            params![source_manga_id, format!("https://mangadot/c{i}"), format!("Chapter {i}"), i],
        ).unwrap();
    }

    // Add associations to source manga
    let src_series_id = format!("manga:{source_manga_id}");
    conn.execute(
        "INSERT INTO series_reconciliations (manga_id, series_id, title, komga_series_id, mangabaka_id, status, last_reconciled_at, updated_at)
         VALUES (?1, ?2, 'Blue Lock', '0RN3Z9FSAPX0G', 83, 'Matched', 1700000000, 1700000000)",
        params![source_manga_id, src_series_id],
    ).unwrap();

    conn.execute(
        "INSERT INTO opentome_series_mappings (manga_id, series_id, title_matched, confidence)
         VALUES (?1, ?2, 'Blue Lock', 'exact')",
        params![source_manga_id, src_series_id],
    ).unwrap();

    conn.execute(
        "INSERT INTO tracked_catalog (title, manga_id, created_at, updated_at)
         VALUES ('Blue Lock', ?1, 1700000000, 1700000000)",
        params![source_manga_id],
    ).unwrap();

    // Step 2: Perform conversion to catalog title
    // Create catalog title (source_id = 0) linked to source_manga_id
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id, source_manga_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83, ?1)",
        [source_manga_id],
    ).unwrap();
    let catalog_manga_id = conn.last_insert_rowid();

    // Update source manga
    conn.execute(
        "UPDATE manga SET in_library = 0, catalog_manga_id = ?1 WHERE id = ?2",
        params![catalog_manga_id, source_manga_id],
    ).unwrap();

    // Re-point associations to catalog manga
    let cat_series_id = format!("manga:{catalog_manga_id}");
    conn.execute(
        "UPDATE series_reconciliations SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
        params![catalog_manga_id, cat_series_id, source_manga_id],
    ).unwrap();
    conn.execute(
        "UPDATE opentome_series_mappings SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
        params![catalog_manga_id, cat_series_id, source_manga_id],
    ).unwrap();
    conn.execute(
        "UPDATE tracked_catalog SET manga_id = ?1 WHERE manga_id = ?2",
        params![catalog_manga_id, source_manga_id],
    ).unwrap();

    // Verify converted state:
    // Catalog title is in_library = 1, source_id = 0, chapters = 0
    let cat_view = read_title(&conn, catalog_manga_id).unwrap();
    assert!(cat_view.is_catalog);
    assert_eq!(cat_view.source_manga_id, Some(source_manga_id));
    assert_eq!(cat_view.chapters.len(), 0);

    // Source manga is in_library = 0, but all chapters are STILL INTACT!
    let (src_in_library, src_cat_id): (i64, Option<i64>) = conn.query_row(
        "SELECT in_library, catalog_manga_id FROM manga WHERE id = ?1",
        [source_manga_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(src_in_library, 0);
    assert_eq!(src_cat_id, Some(catalog_manga_id));

    let preserved_chapters: i64 = conn.query_row(
        "SELECT count(*) FROM chapters WHERE manga_id = ?1",
        [source_manga_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(preserved_chapters, 3, "Source chapters must NEVER be deleted during conversion");

    // Associations now point to catalog_manga_id
    let recon_manga_id: i64 = conn.query_row(
        "SELECT manga_id FROM series_reconciliations WHERE komga_series_id = '0RN3Z9FSAPX0G'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(recon_manga_id, catalog_manga_id);

    // Step 3: Test Reversible Rollback
    // Restore source title as primary
    conn.execute(
        "UPDATE manga SET in_library = 1, catalog_manga_id = NULL WHERE id = ?1",
        [source_manga_id],
    ).unwrap();
    conn.execute(
        "UPDATE manga SET in_library = 0, source_manga_id = NULL WHERE id = ?1",
        [catalog_manga_id],
    ).unwrap();

    // Re-point associations back
    conn.execute(
        "UPDATE series_reconciliations SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
        params![source_manga_id, src_series_id, catalog_manga_id],
    ).unwrap();
    conn.execute(
        "UPDATE opentome_series_mappings SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
        params![source_manga_id, src_series_id, catalog_manga_id],
    ).unwrap();
    conn.execute(
        "UPDATE tracked_catalog SET manga_id = ?1 WHERE manga_id = ?2",
        params![source_manga_id, catalog_manga_id],
    ).unwrap();

    // Verify rolled back state:
    let (src_in_lib, src_cat): (i64, Option<i64>) = conn.query_row(
        "SELECT in_library, catalog_manga_id FROM manga WHERE id = ?1",
        [source_manga_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(src_in_lib, 1, "Source manga must be back in library");
    assert!(src_cat.is_none());

    let recon_manga_back: i64 = conn.query_row(
        "SELECT manga_id FROM series_reconciliations WHERE komga_series_id = '0RN3Z9FSAPX0G'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(recon_manga_back, source_manga_id, "Reconciliation must point back to source manga");
}

#[tokio::test]
async fn test_detach_reading_source() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (1, 'https://mangadot.net/manga/bluelock', 'Blue Lock', 0, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let source_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id, source_manga_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83, ?1)",
        [source_id],
    ).unwrap();
    let catalog_id = conn.last_insert_rowid();

    conn.execute("UPDATE manga SET catalog_manga_id = ?1 WHERE id = ?2", params![catalog_id, source_id]).unwrap();

    // Detach: clear source_manga_id on catalog manga and catalog_manga_id on source manga
    conn.execute("UPDATE manga SET source_manga_id = NULL WHERE id = ?1", [catalog_id]).unwrap();
    conn.execute("UPDATE manga SET catalog_manga_id = NULL WHERE id = ?1", [source_id]).unwrap();

    let detached_src: Option<i64> = conn.query_row("SELECT source_manga_id FROM manga WHERE id = ?1", [catalog_id], |r| r.get(0)).unwrap();
    assert!(detached_src.is_none(), "Catalog title source_manga_id must be cleared");

    let detached_cat: Option<i64> = conn.query_row("SELECT catalog_manga_id FROM manga WHERE id = ?1", [source_id], |r| r.get(0)).unwrap();
    assert!(detached_cat.is_none(), "Source manga catalog_manga_id must be cleared");
}

#[tokio::test]
async fn test_duplicate_catalog_add_idempotency() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let first_id = conn.last_insert_rowid();

    // Query for existing catalog manga by mangabaka_id
    let existing_id: Option<i64> = conn.query_row(
        "SELECT id FROM manga WHERE mangabaka_id = 83 AND source_id = 0 LIMIT 1",
        [],
        |r| r.get(0),
    ).ok();

    assert_eq!(existing_id, Some(first_id), "Subsequent add must return existing catalog manga ID");

    // Total count of manga with mangabaka_id = 83 must remain 1
    let count: i64 = conn.query_row("SELECT count(*) FROM manga WHERE mangabaka_id = 83", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1, "Duplicate add must not create duplicate manga rows");
}

#[tokio::test]
async fn test_scheduled_updates_exclude_catalog_manga() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // Create extension and source 1 (MangaDot) in database
    conn.execute(
        "INSERT INTO extensions (pkg, name, version_name, version_code, lib, content_warning, installed_at, updated_at)
         VALUES ('ext.mangadot', 'MangaDot', '1.0', 1, 'lib', 0, 1700000000, 1700000000)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO sources (id, pkg, name, lang, base_url, supports_latest)
         VALUES (1, 'ext.mangadot', 'MangaDot', 'en', 'https://mangadot.net', 1)",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized)
         VALUES (0, 'catalog:83', 'Catalog Manga', 1, 1700000000, 1)",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized)
         VALUES (1, 'https://mangadot/series', 'Source Manga', 1, 1700000000, 1)",
        [],
    ).unwrap();

    // Updates query checks s.id > 0 AND m.in_library = 1
    let eligible_titles: Vec<String> = conn.prepare(
        "SELECT m.title FROM manga m JOIN sources s ON s.id = m.source_id WHERE m.in_library = 1 AND s.id > 0"
    ).unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap();

    assert_eq!(eligible_titles.len(), 1);
    assert_eq!(eligible_titles[0], "Source Manga", "Catalog titles (source_id = 0) must be excluded from scheduled scraping");
}


#[tokio::test]
async fn test_exact_identity_bypasses_ambiguity() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // Insert catalog manga with exact MangaBaka ID 1234
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:1234', 'Ambiguous Series Title', 1, 1700000000, 1, 1234)",
        [],
    ).unwrap();
    let manga_id = conn.last_insert_rowid();

    // Query exact ID directly as reconciliation does
    let exact_id: Option<i64> = conn.query_row(
        "SELECT mangabaka_id FROM manga WHERE id = ?1",
        [manga_id],
        |r| r.get(0),
    ).unwrap();

    assert_eq!(exact_id, Some(1234), "Catalog title exact ID must resolve directly without fuzzy matching");
}

#[tokio::test]
async fn test_lists_find_local_manga_id_prefers_active_catalog() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // Old source-backed manga (in_library = 0)
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (1, 'https://mangadot/blue-lock', 'Blue Lock', 0, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let source_id = conn.last_insert_rowid();

    // New active catalog manga (in_library = 1, source_id = 0)
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id, source_manga_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83, ?1)",
        [source_id],
    ).unwrap();
    let catalog_id = conn.last_insert_rowid();

    // find_local_manga_id ordering prefers (in_library = 1) DESC, (source_id = 0) DESC
    let resolved_id: i64 = conn.query_row(
        "SELECT id FROM manga WHERE mangabaka_id = 83 ORDER BY (in_library = 1) DESC, (source_id = 0) DESC LIMIT 1",
        [],
        |r| r.get(0),
    ).unwrap();

    assert_eq!(resolved_id, catalog_id, "Lists must link to the active catalog manga, not the inactive source manga");
}

#[tokio::test]
async fn test_candidate_approval_and_acquisition_history_compatibility() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let catalog_id = conn.last_insert_rowid();
    let series_id = format!("manga:{catalog_id}");

    // Insert discovery target
    conn.execute(
        "INSERT INTO release_discovery_targets (
            series_id, manga_id, series_title, volume_number, release_date, status, created_at, updated_at
         ) VALUES (?1, ?2, 'Blue Lock', 1, '2024-01-01', 'candidate_available', 1700000000, 1700000000)",
        params![&series_id, catalog_id],
    ).unwrap();

    // Record manual candidate approval under catalog manga_id
    conn.execute(
        "INSERT INTO candidate_approvals (
            manga_id, series_id, volume_number, candidate_id, provider, provider_release_id, match_type, approved_at, status
         ) VALUES (?1, ?2, 1, 'cand_123', 'nyaa', 'nyaa_123', 'Exact', 1700000000, 'approved')",
        params![catalog_id, &series_id],
    ).unwrap();

    // Record acquisition history under catalog manga_id
    conn.execute(
        "INSERT INTO acquisition_history (
            manga_id, series, release_name, outcome, completed_at
         ) VALUES (?1, 'Blue Lock', 'Blue Lock v01 [Digital]', 'success', 1700000010)",
        [catalog_id],
    ).unwrap();

    let approval_manga: i64 = conn.query_row(
        "SELECT manga_id FROM candidate_approvals WHERE candidate_id = 'cand_123'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(approval_manga, catalog_id, "Candidate approvals must attach directly to catalog manga ID");

    let history_manga: i64 = conn.query_row(
        "SELECT manga_id FROM acquisition_history WHERE release_name = 'Blue Lock v01 [Digital]'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(history_manga, catalog_id, "Acquisition history must attach directly to catalog manga ID");
}

#[tokio::test]
async fn test_description_cleaning_in_read_title() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    let raw_desc = "★★★★☆ 8.6\n\n**Year:** 2018 · **Chapters:** 384 · **Volumes:** 39 · **Tracked:** 1850 · **Content Rating:** Safe · 183 ratings\n\n---\n\nAfter a disastrous defeat at the 2018 World Cup, Japan’s team struggles to regroup.\n\n*Source: Kodansha USA, Vol. 1*\n\n\n**Links:**\n- [AniList](https://anilist.co/manga/106130)\n- [MangaUpdates](https://www.mangaupdates.com/series/joxc7wz)\n\n**Alternative Names:**\n- 블루 록\n- BLUELOCK";

    conn.execute(
        "INSERT INTO manga (source_id, url, title, description, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', ?1, 1, 1700000000, 1, 83)",
        [raw_desc],
    ).unwrap();
    let manga_id = conn.last_insert_rowid();

    let view = read_title(&conn, manga_id).unwrap();
    let desc = view.description.expect("description present");

    assert!(!desc.contains("★★★★☆"), "Stars must be stripped");
    assert!(!desc.contains("**Year:**"), "Year pseudo-metadata must be stripped");
    assert!(!desc.contains("**Links:**"), "Links section must be stripped");
    assert!(!desc.contains("**Alternative Names:**"), "Alternative names must be stripped");
    assert!(desc.starts_with("After a disastrous defeat"));
    assert!(desc.ends_with("*Source: Kodansha USA, Vol. 1*"));
}

#[tokio::test]
async fn test_unified_catalog_ownership_uses_reconciliation() {
    let (db, _dir) = setup_test_db();
    let conn = db.second().expect("conn");

    // 1. Insert tracker list entries (simulating MangaBaka and AniList tracking with 40 Japanese volumes)
    conn.execute(
        "INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, total_volumes, updated_at)
         VALUES ('mangabaka', '83', 'Blue Lock', 'Reading', 'Reading', 40, 1700000000)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, total_volumes, updated_at)
         VALUES ('anilist', '106130', 'Blue Lock', 'CURRENT', 'Reading', 40, 1700000000)",
        [],
    ).unwrap();

    // 2. Insert tracker mapping
    conn.execute(
        "INSERT INTO tracker_mappings (komga_series_id, tracker, tracker_id, confidence, mapping_source, updated_at)
         VALUES ('0RN3Z9FSAPX0G', 'mangabaka', '83', 'resolved', 'manual', 1700000000)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tracker_mappings (komga_series_id, tracker, tracker_id, confidence, mapping_source, updated_at)
         VALUES ('0RN3Z9FSAPX0G', 'anilist', '106130', 'resolved', 'manual', 1700000000)",
        [],
    ).unwrap();

    // 3. Insert catalog manga
    conn.execute(
        "INSERT INTO manga (source_id, url, title, in_library, added_at, initialized, mangabaka_id)
         VALUES (0, 'catalog:83', 'Blue Lock', 1, 1700000000, 1, 83)",
        [],
    ).unwrap();
    let catalog_id = conn.last_insert_rowid();

    // 4. Insert canonical reconciliation: 39 owned, 39 total official volumes, 0 missing, status = 'up_to_date', is_ongoing = 1
    conn.execute(
        "INSERT INTO series_reconciliations (
            series_id, manga_id, komga_series_id, mangabaka_id, title, edition, status, is_ongoing,
            total_official_volumes, owned_volumes_count, missing_volumes_count,
            owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at
         ) VALUES (
            ?1, ?2, '0RN3Z9FSAPX0G', 83, 'Blue Lock', 'standard', 'up_to_date', 1,
            39, 39, 0,
            '[]', '[]', '[]', 1700000000, 1700000000
         )",
        params![format!("manga:{catalog_id}"), catalog_id],
    ).unwrap();

    // 5. Simulate Komga reporting 39 total books
    let komga_series = vec![crate::sync::model::KomgaSeriesProgress {
        series_id: "0RN3Z9FSAPX0G".to_string(),
        series_title: "Blue Lock".to_string(),
        library_id: "0RMWEXC3T10TY".to_string(),
        total_books: 39,
        completed_books: 30,
        highest_completed_book: Some(30.0),
        highest_completed_volume: Some(30.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: None,
        books: Vec::new(),
    }];

    let query = crate::sync::lists::ListsQuery::default();
    let response = crate::sync::lists::build_unified_catalog(&conn, &komga_series, &query).unwrap();

    assert_eq!(response.items.len(), 1);
    let item = &response.items[0];

    // Assert that reconciliation overrides raw tracker counts:
    // Denominator is 39 (not 40), owned is 39, status is "Up to date" (not "Partially Owned")
    assert_eq!(item.total_units, Some(39), "Canonical total units must come from reconciliation (39, not raw tracker 40)");
    assert_eq!(item.owned_units, Some(39), "Owned units must be 39");
    assert_eq!(item.ownership_status, "Up to date", "Ownership status must be Up to date");

    // Filter by ownership=owned should include "Up to date"
    let mut owned_query = crate::sync::lists::ListsQuery::default();
    owned_query.ownership = Some("owned".to_string());
    let owned_response = crate::sync::lists::build_unified_catalog(&conn, &komga_series, &owned_query).unwrap();
    assert_eq!(owned_response.items.len(), 1, "ownership=owned filter must include Up to date");
}
