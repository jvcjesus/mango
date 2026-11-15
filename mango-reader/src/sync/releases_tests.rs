use super::*;
use crate::opentome::catalog::{OpenTomeMarketSchedule, OpenTomeSeriesSchedule, OpenTomeVolumeRelease};
use rusqlite::Connection;

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE manga (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL DEFAULT 0,
            url TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            in_library INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 1,
            auto_acquire INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE tracked_catalog (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            manga_id INTEGER,
            komga_series_id TEXT,
            title TEXT NOT NULL,
            origins TEXT NOT NULL DEFAULT '[\"lists\"]',
            auto_acquire INTEGER NOT NULL DEFAULT 0,
            tracked INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE series_reconciliations (
            series_id TEXT PRIMARY KEY,
            manga_id INTEGER,
            komga_series_id TEXT,
            mangabaka_id INTEGER,
            title TEXT NOT NULL,
            edition TEXT NOT NULL DEFAULT 'standard',
            status TEXT NOT NULL DEFAULT 'up_to_date',
            is_ongoing INTEGER NOT NULL DEFAULT 1,
            total_official_volumes INTEGER,
            owned_volumes_count INTEGER NOT NULL DEFAULT 0,
            missing_volumes_count INTEGER NOT NULL DEFAULT 0,
            owned_volumes_json TEXT NOT NULL DEFAULT '[]',
            missing_volumes_json TEXT NOT NULL DEFAULT '[]',
            future_volumes_json TEXT NOT NULL DEFAULT '[]',
            upcoming_releases_json TEXT NOT NULL DEFAULT '[]',
            last_schedule_refresh_at INTEGER NOT NULL DEFAULT 0,
            last_reconciled_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE opentome_series_mappings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            manga_id INTEGER,
            series_id TEXT NOT NULL UNIQUE,
            work_id TEXT,
            japanese_series_id INTEGER,
            english_series_id INTEGER,
            japanese_tome_id TEXT,
            english_tome_id TEXT,
            confidence TEXT NOT NULL DEFAULT 'unmatched',
            mapping_source TEXT NOT NULL DEFAULT 'auto',
            title_matched TEXT,
            last_evaluated_at INTEGER NOT NULL DEFAULT 0,
            metadata_json TEXT,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();
    conn
}

#[test]
fn test_days_between_ymd() {
    assert_eq!(days_between_ymd("2026-03-03", "2026-02-10"), 21);
    assert_eq!(days_between_ymd("2026-02-10", "2026-02-10"), 0);
    assert_eq!(days_between_ymd("2026-02-05", "2026-02-10"), -5);
    assert_eq!(days_between_ymd("2026-12-08", "2026-01-01"), 341);
}

#[test]
fn test_opentome_market_schedule_multi_volume() {
    let v1 = OpenTomeVolumeRelease {
        volume_number: 1.0,
        title: Some("Vol 1".to_string()),
        release_date: Some("2024-01-01".to_string()),
        release_date_raw: Some("2024-01-01".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "released".to_string(),
    };
    let v2 = OpenTomeVolumeRelease {
        volume_number: 2.0,
        title: Some("Vol 2".to_string()),
        release_date: Some("2024-06-01".to_string()),
        release_date_raw: Some("2024-06-01".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "released".to_string(),
    };
    let v3 = OpenTomeVolumeRelease {
        volume_number: 3.0,
        title: Some("Vol 3".to_string()),
        release_date: Some("2026-03-01".to_string()),
        release_date_raw: Some("2026-03-01".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };
    let v4 = OpenTomeVolumeRelease {
        volume_number: 4.0,
        title: Some("Vol 4".to_string()),
        release_date: Some("2026-07-01".to_string()),
        release_date_raw: Some("2026-07-01".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };

    let sched = OpenTomeMarketSchedule {
        market: "en".to_string(),
        line_name: "English Line".to_string(),
        publisher: Some("Viz Media".to_string()),
        tome_id: "t_1".to_string(),
        gcd_series_id: 100,
        latest_volume: Some(v2.clone()),
        next_volume: Some(v3.clone()),
        total_volumes: 4,
        volumes: vec![v1.clone(), v2.clone(), v3.clone(), v4.clone()],
    };

    let upcoming = sched.upcoming_volumes();
    assert_eq!(upcoming.len(), 2);
    assert_eq!(upcoming[0].volume_number, 3.0);
    assert_eq!(upcoming[1].volume_number, 4.0);

    let recent = sched.recent_volumes(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].volume_number, 2.0);
    assert_eq!(recent[1].volume_number, 1.0);

    assert_eq!(sched.volume_by_number(3.0).unwrap().title.as_deref(), Some("Vol 3"));
}

#[test]
fn test_query_upcoming_releases_scope_filtering() {
    let conn = setup_test_db();
    let today = "2026-02-01";

    // Manga 1: In Library
    conn.execute(
        "INSERT INTO manga (id, title, in_library) VALUES (1, 'Frieren', 1)",
        [],
    )
    .unwrap();

    // Create OpenTome schedule for Manga 1
    let v14 = OpenTomeVolumeRelease {
        volume_number: 14.0,
        title: Some("Vol 14".to_string()),
        release_date: Some("2026-02-10".to_string()),
        release_date_raw: Some("2026-02-10".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };
    let frieren_sched = OpenTomeSeriesSchedule {
        work_id: "w_frieren".to_string(),
        mapping_confidence: "exact".to_string(),
        japanese: None,
        english: Some(OpenTomeMarketSchedule {
            market: "en".to_string(),
            line_name: "Frieren (EN)".to_string(),
            publisher: Some("Viz Media".to_string()),
            tome_id: "t_frieren_en".to_string(),
            gcd_series_id: 101,
            latest_volume: None,
            next_volume: Some(v14.clone()),
            total_volumes: 1,
            volumes: vec![v14],
        }),
        last_evaluated_at: 1700000000,
    };
    conn.execute(
        "INSERT INTO opentome_series_mappings (manga_id, series_id, work_id, confidence, metadata_json)
         VALUES (1, 'manga:1', 'w_frieren', 'exact', ?1)",
        [serde_json::to_string(&frieren_sched).unwrap()],
    )
    .unwrap();

    // Manga 2: In Tracked Lists Only (in_library = 0)
    conn.execute(
        "INSERT INTO manga (id, title, in_library) VALUES (2, 'Kagurabachi', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracked_catalog (manga_id, title, tracked) VALUES (2, 'Kagurabachi', 1)",
        [],
    )
    .unwrap();

    let v6 = OpenTomeVolumeRelease {
        volume_number: 6.0,
        title: Some("Vol 6".to_string()),
        release_date: Some("2026-02-03".to_string()),
        release_date_raw: Some("2026-02-03".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };
    let kb_sched = OpenTomeSeriesSchedule {
        work_id: "w_kb".to_string(),
        mapping_confidence: "exact".to_string(),
        japanese: None,
        english: Some(OpenTomeMarketSchedule {
            market: "en".to_string(),
            line_name: "Kagurabachi (EN)".to_string(),
            publisher: Some("Viz Media".to_string()),
            tome_id: "t_kb_en".to_string(),
            gcd_series_id: 102,
            latest_volume: None,
            next_volume: Some(v6.clone()),
            total_volumes: 1,
            volumes: vec![v6],
        }),
        last_evaluated_at: 1700000000,
    };
    conn.execute(
        "INSERT INTO opentome_series_mappings (manga_id, series_id, work_id, confidence, metadata_json)
         VALUES (2, 'manga:2', 'w_kb', 'exact', ?1)",
        [serde_json::to_string(&kb_sched).unwrap()],
    )
    .unwrap();

    // 1. Mango Library Only (default)
    let res_lib = query_upcoming_releases(&conn, "mango_library_only", None, None, today).unwrap();
    assert_eq!(res_lib.active_scope, "mango_library_only");
    assert_eq!(res_lib.releases.len(), 1);
    assert_eq!(res_lib.releases[0].title, "Frieren");
    assert_eq!(res_lib.releases[0].volume_number, Some(14.0));
    assert_eq!(res_lib.releases[0].days_until, 9); // Feb 10 - Feb 01 = 9 days

    // 2. Tracked Lists scope
    let res_tracked = query_upcoming_releases(&conn, "tracked_lists", None, None, today).unwrap();
    assert_eq!(res_tracked.releases.len(), 2);
    // Chronological order: Vol 6 (Feb 03) then Vol 14 (Feb 10)
    assert_eq!(res_tracked.releases[0].title, "Kagurabachi");
    assert_eq!(res_tracked.releases[0].volume_number, Some(6.0));
    assert_eq!(res_tracked.releases[1].title, "Frieren");
}

#[test]
fn test_undated_releases_preserved_and_diagnosed() {
    let conn = setup_test_db();
    let today = "2026-02-01";

    // Manga 3: In Library, ongoing, but no upcoming release date announced
    conn.execute(
        "INSERT INTO manga (id, title, in_library, status) VALUES (3, 'Ongoing Without Dates', 1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO series_reconciliations (series_id, manga_id, title, status, is_ongoing, future_volumes_json)
         VALUES ('manga:3', 3, 'Ongoing Without Dates', 'up_to_date', 1, '[15.0]')",
        [],
    )
    .unwrap();

    let res = query_upcoming_releases(&conn, "mango_library_only", None, None, today).unwrap();
    assert_eq!(res.releases.len(), 0);
    assert_eq!(res.undated.len(), 1);
    assert_eq!(res.undated[0].title, "Ongoing Without Dates");
    assert_eq!(res.undated[0].expected_volume, Some(15.0));
    assert!(res.undated[0].reason.contains("MangaBaka catalogue"));
}

#[test]
fn test_get_title_release_schedule() {
    let conn = setup_test_db();
    let today = "2026-02-01";

    conn.execute(
        "INSERT INTO manga (id, title, in_library) VALUES (10, 'Chainsaw Man', 1)",
        [],
    )
    .unwrap();

    let v19 = OpenTomeVolumeRelease {
        volume_number: 19.0,
        title: Some("Vol 19".to_string()),
        release_date: Some("2025-10-07".to_string()),
        release_date_raw: Some("2025-10-07".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "released".to_string(),
    };
    let v20 = OpenTomeVolumeRelease {
        volume_number: 20.0,
        title: Some("Vol 20".to_string()),
        release_date: Some("2026-03-03".to_string()),
        release_date_raw: Some("2026-03-03".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };
    let v21 = OpenTomeVolumeRelease {
        volume_number: 21.0,
        title: Some("Vol 21".to_string()),
        release_date: Some("2026-06-02".to_string()),
        release_date_raw: Some("2026-06-02".to_string()),
        precision: "day".to_string(),
        date_type: "published".to_string(),
        status: "future".to_string(),
    };

    let sched = OpenTomeSeriesSchedule {
        work_id: "w_csm".to_string(),
        mapping_confidence: "exact".to_string(),
        japanese: None,
        english: Some(OpenTomeMarketSchedule {
            market: "en".to_string(),
            line_name: "Chainsaw Man (EN)".to_string(),
            publisher: Some("Viz Media".to_string()),
            tome_id: "t_csm_en".to_string(),
            gcd_series_id: 200,
            latest_volume: Some(v19.clone()),
            next_volume: Some(v20.clone()),
            total_volumes: 3,
            volumes: vec![v19, v20, v21],
        }),
        last_evaluated_at: 1700000000,
    };

    conn.execute(
        "INSERT INTO opentome_series_mappings (manga_id, series_id, work_id, confidence, metadata_json)
         VALUES (10, 'manga:10', 'w_csm', 'exact', ?1)",
        [serde_json::to_string(&sched).unwrap()],
    )
    .unwrap();

    let title_sched = get_title_release_schedule(&conn, 10, today).unwrap();
    assert_eq!(title_sched.manga_id, 10);
    assert_eq!(title_sched.title, "Chainsaw Man");
    assert_eq!(title_sched.confidence, "exact");
    assert_eq!(title_sched.next_release.as_ref().unwrap().volume_number, Some(20.0));
    assert_eq!(title_sched.upcoming_volumes.len(), 2);
    assert_eq!(title_sched.upcoming_volumes[0].volume_number, Some(20.0));
    assert_eq!(title_sched.upcoming_volumes[1].volume_number, Some(21.0));
    assert_eq!(title_sched.recent_releases.len(), 1);
    assert_eq!(title_sched.recent_releases[0].volume_number, Some(19.0));
}
