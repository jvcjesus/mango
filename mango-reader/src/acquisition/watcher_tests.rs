//! Comprehensive Unit Tests for Release Availability Watcher Subsystem (Milestone M6.8.4)

use rusqlite::Connection;

use super::watcher::*;
use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::auto_acquire::{is_candidate_auto_eligible, AutoDecisionCode};
use crate::acquisition::profiles::AcquisitionProfile;

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE manga (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            source_id INTEGER NOT NULL DEFAULT 0,
            url TEXT NOT NULL DEFAULT '',
            auto_acquire INTEGER NOT NULL DEFAULT 0,
            auto_acquire_activated_at INTEGER,
            release_monitoring_enabled INTEGER NOT NULL DEFAULT 1,
            release_notify_enabled INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE series_reconciliations (
            series_id TEXT PRIMARY KEY,
            manga_id INTEGER,
            komga_series_id TEXT,
            mangabaka_id INTEGER,
            collection_id TEXT,
            title TEXT NOT NULL,
            edition TEXT NOT NULL DEFAULT 'standard',
            status TEXT NOT NULL,
            is_ongoing INTEGER NOT NULL DEFAULT 1,
            total_official_volumes INTEGER,
            owned_volumes_count INTEGER NOT NULL DEFAULT 0,
            missing_volumes_count INTEGER NOT NULL DEFAULT 0,
            owned_volumes_json TEXT NOT NULL DEFAULT '[]',
            missing_volumes_json TEXT NOT NULL DEFAULT '[]',
            future_volumes_json TEXT NOT NULL DEFAULT '[]',
            last_reconciled_at INTEGER NOT NULL,
            last_error TEXT,
            review_reason TEXT,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE release_discovery_targets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            series_id TEXT NOT NULL,
            manga_id INTEGER,
            series_title TEXT NOT NULL,
            volume_number REAL NOT NULL,
            market TEXT NOT NULL DEFAULT 'en',
            edition TEXT NOT NULL DEFAULT 'standard',
            release_date TEXT NOT NULL,
            release_date_precision TEXT NOT NULL DEFAULT 'day',
            date_type TEXT NOT NULL DEFAULT 'published',
            status TEXT NOT NULL DEFAULT 'scheduled',
            attempts INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 7,
            next_attempt_at INTEGER NOT NULL DEFAULT 0,
            last_attempt_at INTEGER,
            candidates_count INTEGER NOT NULL DEFAULT 0,
            top_candidate_id TEXT,
            top_candidate_title TEXT,
            last_error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(series_id, volume_number, market, edition, release_date)
        );

        CREATE TABLE discovered_release_candidates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            series_id TEXT NOT NULL,
            volume_number REAL NOT NULL,
            candidate_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            provider_item_id TEXT NOT NULL,
            title TEXT NOT NULL,
            raw_title TEXT NOT NULL,
            match_type TEXT NOT NULL,
            size_bytes INTEGER,
            uploader TEXT,
            is_trusted INTEGER NOT NULL DEFAULT 0,
            is_digital INTEGER NOT NULL DEFAULT 0,
            score REAL NOT NULL,
            score_explanation TEXT NOT NULL,
            download_url TEXT,
            magnet_url TEXT,
            info_hash TEXT,
            seeders INTEGER,
            leechers INTEGER,
            published_at INTEGER,
            discovered_at INTEGER NOT NULL,
            UNIQUE(series_id, volume_number, candidate_id)
        );

        CREATE TABLE volume_discovery_status (
            series_id TEXT NOT NULL,
            volume_number REAL NOT NULL,
            last_discovered_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            candidate_count INTEGER NOT NULL DEFAULT 0,
            top_candidate_id TEXT,
            providers_checked TEXT NOT NULL DEFAULT '[]',
            provider_errors TEXT,
            PRIMARY KEY (series_id, volume_number)
        );

        CREATE TABLE reconciliation_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            series_id TEXT NOT NULL,
            mangabaka_id INTEGER,
            mangabaka_volume_id TEXT,
            series_title TEXT NOT NULL,
            edition TEXT NOT NULL DEFAULT 'standard',
            volume_number REAL,
            event_type TEXT NOT NULL,
            top_candidate_id TEXT,
            candidate_count INTEGER NOT NULL DEFAULT 0,
            message TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            notified_at INTEGER,
            resolved_at INTEGER,
            UNIQUE(series_id, volume_number, event_type)
        );

        CREATE TABLE notification_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            destination TEXT NOT NULL,
            transport TEXT NOT NULL DEFAULT 'ntfy',
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 5,
            next_attempt_at INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            delivered_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(event_id, transport)
        );

        CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE release_availability_watchers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            series_id TEXT NOT NULL,
            manga_id INTEGER,
            series_title TEXT NOT NULL,
            volume_number REAL NOT NULL,
            mangabaka_id INTEGER,
            collection_id TEXT,
            work_id TEXT,
            market TEXT NOT NULL DEFAULT 'en',
            edition TEXT NOT NULL DEFAULT 'standard',
            release_date TEXT,
            is_released INTEGER NOT NULL DEFAULT 0,
            is_owned INTEGER NOT NULL DEFAULT 0,
            monitoring_enabled INTEGER NOT NULL DEFAULT 1,
            notify_on_available INTEGER NOT NULL DEFAULT 1,
            state TEXT NOT NULL DEFAULT 'UPCOMING',
            last_attempt_at INTEGER,
            last_checked_at INTEGER,
            next_check_at INTEGER NOT NULL DEFAULT 0,
            check_interval_secs INTEGER NOT NULL DEFAULT 14400,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            last_provider_checked TEXT,
            last_check_status TEXT,
            last_error TEXT,
            top_candidate_id TEXT,
            top_candidate_provider TEXT,
            top_candidate_title TEXT,
            top_candidate_fingerprint TEXT,
            candidate_count INTEGER NOT NULL DEFAULT 0,
            notified_at INTEGER,
            notification_event_id INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(series_id, volume_number, market, edition)
        );
        ",
    )
    .unwrap();
    conn
}

#[test]
fn test_sync_watchers_creates_and_updates_states() {
    let conn = setup_test_db();

    conn.execute(
        "INSERT INTO manga (id, title, release_monitoring_enabled, release_notify_enabled)
         VALUES (101, 'Kagurabachi', 1, 1)",
        [],
    )
    .unwrap();

    let missing = vec![8.0, 9.0];
    let future = vec![10.0];
    let owned = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

    let watchers = sync_watchers_for_series(
        &conn,
        101,
        "manga:101",
        "Kagurabachi",
        Some(708),
        Some("col_kb_standard"),
        &missing,
        &future,
        &owned,
    )
    .unwrap();

    assert_eq!(watchers.len(), 3);

    let w8 = watchers.iter().find(|w| (w.volume_number - 8.0).abs() < 0.001).unwrap();
    assert_eq!(w8.state, WatcherState::ReleasedAwaitingProvider);
    assert!(w8.is_released);
    assert!(!w8.is_owned);
    assert!(w8.monitoring_enabled);
    assert!(w8.notify_on_available);

    let w10 = watchers.iter().find(|w| (w.volume_number - 10.0).abs() < 0.001).unwrap();
    assert_eq!(w10.state, WatcherState::Upcoming);
    assert!(!w10.is_released);

    // If volume 8 is later acquired/owned, re-sync marks it OWNERSHIP_DETECTED
    let owned_updated = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let missing_updated = vec![9.0];

    let watchers_after = sync_watchers_for_series(
        &conn,
        101,
        "manga:101",
        "Kagurabachi",
        Some(708),
        Some("col_kb_standard"),
        &missing_updated,
        &future,
        &owned_updated,
    )
    .unwrap();

    let w8_after = watchers_after.iter().find(|w| (w.volume_number - 8.0).abs() < 0.001).unwrap();
    assert_eq!(w8_after.state, WatcherState::OwnershipDetected);
    assert!(w8_after.is_owned);
    assert_eq!(w8_after.next_check_at, 0); // Stops future polling
}

#[test]
fn test_collection_unavailable_fails_closed() {
    let conn = setup_test_db();

    conn.execute(
        "INSERT INTO manga (id, title) VALUES (201, 'Blade of the Immortal')",
        [],
    )
    .unwrap();

    let missing = vec![1.0];
    let watchers = sync_watchers_for_series(
        &conn,
        201,
        "manga:201",
        "Blade of the Immortal",
        Some(5246),
        None, // Collection unavailable
        &missing,
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(watchers.len(), 1);
    assert_eq!(watchers[0].state, WatcherState::CollectionUnavailable);
}

#[test]
fn test_candidate_eligibility_hard_gates_enforced() {
    let profile = AcquisitionProfile::default();

    // 1. Exact volume release -> Eligible
    let valid_cand = PersistedCandidate {
        series_id: "manga:101".into(),
        volume_number: 9.0,
        candidate_id: "cand_valid".into(),
        provider: "nyaa".into(),
        provider_item_id: "12345".into(),
        title: "Kagurabachi v09 (2026) (Digital) (Empire)".into(),
        raw_title: "Kagurabachi v09 (2026) (Digital) (Empire).cbz".into(),
        match_type: "exact_volume".into(),
        size_bytes: Some(150 * 1024 * 1024),
        uploader: Some("danke-Empire".into()),
        is_trusted: true,
        is_digital: true,
        score: 95.0,
        score_explanation: "Exact volume".into(),
        download_url: None,
        magnet_url: Some("magnet:?xt=urn:btih:abc".into()),
        info_hash: Some("abc".into()),
        seeders: Some(15),
        leechers: Some(2),
        published_at: Some(1793700000),
    };
    assert!(is_candidate_auto_eligible(&valid_cand, &profile).is_ok());

    // 2. Multi-volume batch/pack -> Rejected
    let pack_cand = PersistedCandidate {
        raw_title: "Kagurabachi v01-v09 Complete Batch.cbz".into(),
        match_type: "series_pack".into(),
        ..valid_cand.clone()
    };
    assert_eq!(
        is_candidate_auto_eligible(&pack_cand, &profile).unwrap_err(),
        AutoDecisionCode::PackRequiresManual
    );

    // 3. Madokami PRE release -> Rejected
    let pre_cand = PersistedCandidate {
        provider: "madokami".into(),
        download_url: Some("http://madokami.example/file.cbz".into()),
        raw_title: "Kagurabachi v09 (Pre).cbz".into(),
        match_type: "exact_volume".into(),
        ..valid_cand.clone()
    };
    assert_eq!(
        is_candidate_auto_eligible(&pre_cand, &profile).unwrap_err(),
        AutoDecisionCode::PreReleaseRequiresManual
    );

    // 4. Non-English release -> Rejected
    let fr_cand = PersistedCandidate {
        raw_title: "Kagurabachi Tome 09 French.cbz".into(),
        match_type: "exact_volume".into(),
        ..valid_cand.clone()
    };
    assert_eq!(
        is_candidate_auto_eligible(&fr_cand, &profile).unwrap_err(),
        AutoDecisionCode::LanguageUnknown
    );

    // 5. PDF format -> Rejected
    let pdf_cand = PersistedCandidate {
        raw_title: "Kagurabachi v09.pdf".into(),
        match_type: "exact_volume".into(),
        ..valid_cand.clone()
    };
    assert_eq!(
        is_candidate_auto_eligible(&pdf_cand, &profile).unwrap_err(),
        AutoDecisionCode::FormatNotAllowed
    );
}

#[test]
fn test_title_monitoring_settings_toggles() {
    let conn = setup_test_db();

    conn.execute(
        "INSERT INTO manga (id, title, auto_acquire, release_monitoring_enabled, release_notify_enabled)
         VALUES (301, 'Title Test', 0, 1, 1)",
        [],
    )
    .unwrap();

    let settings = get_title_monitoring_settings(&conn, 301).unwrap();
    assert!(settings.release_monitoring_enabled);
    assert!(settings.release_notify_enabled);
    assert!(!settings.auto_acquire_enabled); // Independent setting!

    let updated = set_title_monitoring_settings(&conn, 301, false, false).unwrap();
    assert!(!updated.release_monitoring_enabled);
    assert!(!updated.release_notify_enabled);
    assert!(!updated.auto_acquire_enabled);
}

#[test]
fn test_interval_calculation_and_jitter() {
    let today = "2026-11-04";

    // Consecutive failures apply exponential backoff
    let backoff_1 = calculate_watcher_interval_secs(None, 1, today);
    assert!(backoff_1 >= 1500 && backoff_1 <= 2100); // ~1800s with jitter

    let backoff_3 = calculate_watcher_interval_secs(None, 3, today);
    assert!(backoff_3 >= 6000 && backoff_3 <= 8500); // ~7200s with jitter

    // Released recently (e.g. today or within 7 days) has frequent check interval (~4h)
    let interval_recent = calculate_watcher_interval_secs(Some("2026-11-03"), 0, today);
    assert!(interval_recent >= 12000 && interval_recent <= 17000); // ~14400s with jitter
}

#[tokio::test]
async fn test_run_watcher_tick_atomic_lease() {
    let temp_dir = std::env::temp_dir().join(format!("mango_wtick_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);
    let db = crate::db::Db::open(&temp_dir.join("test.db")).unwrap();

    let cur_time = crate::db::now();

    // Insert title and a due watcher
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, update_strategy)
             VALUES (401, 1, 'url:401', 'Watch Manga', 'ALWAYS_UPDATE')",
            [],
        )?;
        conn.execute(
            "INSERT INTO release_availability_watchers (
                series_id, manga_id, series_title, volume_number, edition,
                is_released, is_owned, monitoring_enabled, notify_on_available,
                state, last_attempt_at, last_checked_at, next_check_at,
                check_interval_secs, consecutive_failures, created_at, updated_at
             ) VALUES (
                'manga:401', 401, 'Watch Manga', 1.0, 'standard',
                1, 0, 1, 1,
                'RELEASED_AWAITING_PROVIDER', 0, 0, ?1,
                3600, 0, ?1, ?1
             )",
            [cur_time - 10], // Due 10 seconds ago
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Call run_watcher_tick
    let http = reqwest::Client::new();
    let _ = run_watcher_tick(&db, http).await;

    // Check that the watcher was leased: next_check_at must be leased forward
    db.call(move |conn| {
        let (last_attempt, next_check): (i64, i64) = conn.query_row(
            "SELECT last_attempt_at, next_check_at FROM release_availability_watchers WHERE manga_id = 401",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert!(last_attempt >= cur_time);
        assert!(next_check >= cur_time + 290);
        Ok(())
    })
    .await
    .unwrap();

    let _ = std::fs::remove_dir_all(&temp_dir);
}
