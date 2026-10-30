use super::catalog::*;
use super::service::OpenTomeService;
use super::updater::OpenTomeUpdater;
use crate::db::Db;
use rusqlite::Connection;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "mango-ot-test-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        TestDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Helper to create a valid minimal OpenTome SQLite catalogue for testing.
fn create_test_opentome_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("failed to open test sqlite db");
    conn.execute_batch(
        r#"
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT INTO meta (key, value) VALUES ('dump_date', '2026-10-20'), ('schema_version', '1');

        CREATE TABLE series (
            gcd_series_id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            language TEXT NOT NULL,
            is_main INTEGER NOT NULL DEFAULT 1,
            is_omnibus INTEGER NOT NULL DEFAULT 0,
            volume_count INTEGER NOT NULL DEFAULT 0,
            tome_id TEXT,
            tome_work_id TEXT,
            publisher TEXT,
            local_name TEXT,
            medium TEXT DEFAULT 'manga',
            anilist_id INTEGER,
            mal_id INTEGER
        );

        CREATE TABLE volumes (
            id TEXT PRIMARY KEY,
            gcd_series_id INTEGER NOT NULL,
            volume_number INTEGER NOT NULL,
            title TEXT,
            release_date TEXT,
            release_date_precision TEXT,
            release_date_type TEXT,
            release_date_raw TEXT,
            status TEXT DEFAULT 'published'
        );

        CREATE TABLE series_alias (
            gcd_series_id INTEGER NOT NULL,
            alias TEXT NOT NULL,
            language TEXT
        );
        CREATE INDEX idx_series_alias ON series_alias(alias);
        "#,
    )
    .expect("failed to init test opentome schema");
    conn
}

#[test]
fn test_catalog_anilist_lookup() {
    let dir = TestDir::new("anilist");
    let db_path = dir.path().join("manga-metadata.sqlite");
    let conn = create_test_opentome_db(&db_path);

    conn.execute(
        "INSERT INTO series (gcd_series_id, name, language, is_main, tome_work_id, anilist_id)
         VALUES (101, 'One Piece', 'en', 1, 'w_onepiece', 30013)",
        [],
    )
    .unwrap();

    let work_id = find_work_by_anilist_id(&conn, 30013).unwrap();
    assert_eq!(work_id.as_deref(), Some("w_onepiece"));

    let nonexistent = find_work_by_anilist_id(&conn, 999999).unwrap();
    assert!(nonexistent.is_none());
}

#[test]
fn test_catalog_title_and_alias_lookup() {
    let dir = TestDir::new("title");
    let db_path = dir.path().join("manga-metadata.sqlite");
    let conn = create_test_opentome_db(&db_path);

    conn.execute(
        "INSERT INTO series (gcd_series_id, name, language, is_main, tome_work_id)
         VALUES (201, 'Kagurabachi', 'en', 1, 'w_kagurabachi')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO series_alias (gcd_series_id, alias, language)
         VALUES (201, 'カグラバチ', 'ja')",
        [],
    )
    .unwrap();

    // Exact title match (case insensitive)
    let found = find_works_by_title_or_alias(&conn, "kagurabachi").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "w_kagurabachi");

    // Alias match
    let found_alias = find_works_by_title_or_alias(&conn, "カグラバチ").unwrap();
    assert_eq!(found_alias.len(), 1);
    assert_eq!(found_alias[0].0, "w_kagurabachi");

    // Unknown title
    let none = find_works_by_title_or_alias(&conn, "Unknown Nonexistent Title").unwrap();
    assert!(none.is_empty());
}

#[test]
fn test_catalog_line_picking_and_disambiguation() {
    let lines = vec![
        OpenTomeReleaseLine {
            gcd_series_id: 1,
            name: "One Piece (Omnibus)".to_string(),
            language: "en".to_string(),
            is_main: true,
            is_omnibus: true,
            volume_count: 30,
            tome_id: "t_en_omnibus".to_string(),
            tome_work_id: "w_op".to_string(),
            publisher: Some("VIZ Media".to_string()),
            local_name: None,
            medium: Some("manga".to_string()),
            anilist_id: None,
        },
        OpenTomeReleaseLine {
            gcd_series_id: 2,
            name: "One Piece".to_string(),
            language: "en".to_string(),
            is_main: true,
            is_omnibus: false,
            volume_count: 108,
            tome_id: "t_en_main".to_string(),
            tome_work_id: "w_op".to_string(),
            publisher: Some("VIZ Media".to_string()),
            local_name: None,
            medium: Some("manga".to_string()),
            anilist_id: Some(30013),
        },
        OpenTomeReleaseLine {
            gcd_series_id: 3,
            name: "ONE PIECE".to_string(),
            language: "ja".to_string(),
            is_main: true,
            is_omnibus: false,
            volume_count: 110,
            tome_id: "t_ja_main".to_string(),
            tome_work_id: "w_op".to_string(),
            publisher: Some("Shueisha".to_string()),
            local_name: None,
            medium: Some("manga".to_string()),
            anilist_id: None,
        },
    ];

    // English pick should prefer main non-omnibus
    let en = pick_main_line(&lines, "en").expect("should pick english line");
    assert_eq!(en.gcd_series_id, 2);
    assert_eq!(en.volume_count, 108);

    // Japanese pick
    let ja = pick_main_line(&lines, "ja").expect("should pick japanese line");
    assert_eq!(ja.gcd_series_id, 3);
    assert_eq!(ja.volume_count, 110);
}

#[test]
fn test_catalog_schedule_date_precision_and_status() {
    let dir = TestDir::new("schedule");
    let db_path = dir.path().join("manga-metadata.sqlite");
    let conn = create_test_opentome_db(&db_path);

    conn.execute(
        "INSERT INTO series (gcd_series_id, name, language, is_main, tome_work_id, publisher, volume_count)
         VALUES (301, 'Test Manga', 'en', 1, 'w_test', 'Viz', 6)",
        [],
    )
    .unwrap();

    // Volume 1: Day precision, past -> released
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision)
         VALUES ('v1', 301, 1, '2024-03-04', 'day')",
        [],
    )
    .unwrap();

    // Volume 2: Month precision, past -> released
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision, release_date_raw)
         VALUES ('v2', 301, 2, '2024-06-01', 'month', '2024-06')",
        [],
    )
    .unwrap();

    // Volume 3: Year precision, past -> released
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision, release_date_raw)
         VALUES ('v3', 301, 3, '2024-01-01', 'year', '2024')",
        [],
    )
    .unwrap();

    // Volume 4: Exact today -> releases_today
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision)
         VALUES ('v4', 301, 4, '2026-10-29', 'day')",
        [],
    )
    .unwrap();

    // Volume 5: Future exact -> future
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision)
         VALUES ('v5', 301, 5, '2026-12-01', 'day')",
        [],
    )
    .unwrap();

    // Volume 6: Unknown date -> future, precision unknown
    conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision)
         VALUES ('v6', 301, 6, NULL, NULL)",
        [],
    )
    .unwrap();

    let line = OpenTomeReleaseLine {
        gcd_series_id: 301,
        name: "Test Manga".to_string(),
        language: "en".to_string(),
        is_main: true,
        is_omnibus: false,
        volume_count: 6,
        tome_id: "t_test".to_string(),
        tome_work_id: "w_test".to_string(),
        publisher: Some("Viz".to_string()),
        local_name: None,
        medium: Some("manga".to_string()),
        anilist_id: None,
    };

    let schedule = get_line_schedule(&conn, &line, "2026-10-29").unwrap();
    assert_eq!(schedule.total_volumes, 6);

    // Latest volume released or releasing today should be Vol 4
    let latest = schedule.latest_volume.as_ref().expect("latest volume should exist");
    assert_eq!(latest.volume_number, 4.0);
    assert_eq!(latest.status, "releases_today");
    assert_eq!(latest.precision, "day");

    // Next volume upcoming should be Vol 4 (releases today) or subsequent future
    let next = schedule.next_volume.as_ref().expect("next volume should exist");
    assert_eq!(next.volume_number, 4.0);
    assert_eq!(next.status, "releases_today");
}

#[test]
fn test_updater_sqlite_validation_rules() {
    let dir = TestDir::new("validation");

    // 1. Corrupt non-sqlite file
    let corrupt_path = dir.path().join("corrupt.sqlite");
    {
        let mut f = File::create(&corrupt_path).unwrap();
        f.write_all(b"NOT A SQLITE DATABASE CORRUPT DATA").unwrap();
    }
    let res_corrupt = OpenTomeUpdater::validate_sqlite_file(&corrupt_path);
    assert!(res_corrupt.is_err(), "corrupt file must fail validation");

    // 2. Valid SQLite database missing required tables
    let missing_tables_path = dir.path().join("missing_tables.sqlite");
    {
        let empty_conn = Connection::open(&missing_tables_path).unwrap();
        empty_conn.execute("CREATE TABLE dummy (id INTEGER)", []).unwrap();
    }
    let res_missing = OpenTomeUpdater::validate_sqlite_file(&missing_tables_path);
    assert!(res_missing.is_err(), "missing tables must fail validation");

    // 3. Valid OpenTome SQLite database passes
    let valid_path = dir.path().join("valid.sqlite");
    create_test_opentome_db(&valid_path);
    let res_valid = OpenTomeUpdater::validate_sqlite_file(&valid_path);
    assert!(res_valid.is_ok(), "valid schema must pass validation");
}

#[tokio::test]
async fn test_service_fail_open_when_catalogue_missing() {
    let dir = TestDir::new("failopen");
    let db = Db::open(&dir.path().join("mango.db")).unwrap();
    let http = reqwest::Client::new();
    let service = OpenTomeService::new(db, dir.path(), http);

    // Status should report unavailable, no panic
    let status = service.status().await.unwrap();
    assert!(!status.available);

    // Fetching schedule for any manga returns Ok(None) fail-open
    let sched = service.get_series_schedule(1).await.unwrap();
    assert!(sched.is_none());
}

#[tokio::test]
async fn test_service_mapping_and_event_deduplication() {
    let dir = TestDir::new("dedup");
    let mango_db = Db::open(&dir.path().join("mango.db")).unwrap();

    // Setup OpenTome catalogue at expected location: <dir>/opentome/manga-metadata.sqlite
    let ot_dir = dir.path().join("opentome");
    std::fs::create_dir_all(&ot_dir).unwrap();
    let ot_db_path = ot_dir.join("manga-metadata.sqlite");
    let ot_conn = create_test_opentome_db(&ot_db_path);

    // Insert series into OpenTome
    ot_conn.execute(
        "INSERT INTO series (gcd_series_id, name, language, is_main, tome_work_id, volume_count, anilist_id)
         VALUES (501, 'Chainsaw Man', 'en', 1, 'w_csm', 15, 105778)",
        [],
    ).unwrap();

    // Today's release
    ot_conn.execute(
        "INSERT INTO volumes (id, gcd_series_id, volume_number, release_date, release_date_precision)
         VALUES ('csm_v15', 501, 15, date('now', 'localtime'), 'day')",
        [],
    ).unwrap();

    // Setup followed series in Mango DB
    mango_db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title) VALUES (10, 1, '/manga/csm', 'Chainsaw Man')",
            [],
        )?;
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, series_id, title, status, last_reconciled_at, updated_at)
             VALUES (10, 'series:csm', 'Chainsaw Man', 'following', 1000, 1000)",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracker_mappings (komga_series_id, tracker, tracker_id, updated_at)
             VALUES ('komga:csm', 'anilist', '105778', 1000)",
            [],
        )?;
        // link komga_series_id
        conn.execute(
            "UPDATE series_reconciliations SET komga_series_id = 'komga:csm' WHERE manga_id = 10",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    let http = reqwest::Client::new();
    let service = Arc::new(OpenTomeService::new(mango_db.clone(), dir.path(), http));

    // Evaluate schedule
    let schedule = service.get_series_schedule(10).await.unwrap().expect("schedule should resolve");
    assert_eq!(schedule.work_id, "w_csm");
    assert_eq!(schedule.mapping_confidence, "exact");
    let en_sched = schedule.english.expect("english schedule should exist");
    let next_vol = en_sched.next_volume.expect("next volume should exist");
    assert_eq!(next_vol.volume_number, 15.0);
    assert_eq!(next_vol.status, "releases_today");

    // Run release day event evaluation
    let count1 = service.evaluate_release_day_events().await.unwrap();
    assert_eq!(count1, 1, "first run should emit 1 event");

    // Second run should deduplicate and emit 0 additional new events
    let count2 = service.evaluate_release_day_events().await.unwrap();
    assert_eq!(count2, 1, "second run updates existing event without adding duplicate");

    // Verify exactly 1 event exists in reconciliation_events
    let total_events: i64 = mango_db.call(|conn| {
        let count = conn.query_row(
            "SELECT COUNT(*) FROM reconciliation_events WHERE event_type = 'VOLUME_RELEASES_TODAY'",
            [],
            |r| r.get(0),
        ).unwrap();
        Ok(count)
    }).await.unwrap();
    assert_eq!(total_events, 1);
}
