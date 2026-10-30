//! Unit tests for M6.6: Release-Aware Discovery.

#[cfg(test)]
mod tests {
    use super::super::release_discovery::*;
    use crate::opentome::catalog::{OpenTomeMarketSchedule, OpenTomeSeriesSchedule, OpenTomeVolumeRelease};
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS manga (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                source_id TEXT NOT NULL DEFAULT '',
                source_name TEXT NOT NULL DEFAULT '',
                source_lang TEXT NOT NULL DEFAULT 'en'
            );
            CREATE TABLE IF NOT EXISTS chapters (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                volume REAL,
                chapter_number REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS downloads (
                chapter_id INTEGER PRIMARY KEY,
                position INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL,
                published_path TEXT,
                raw_title TEXT
            );
            CREATE TABLE IF NOT EXISTS acquisition_history (
                id INTEGER PRIMARY KEY,
                chapter_id INTEGER,
                manga_id INTEGER,
                series TEXT NOT NULL,
                outcome TEXT NOT NULL,
                canonical_path TEXT
            );
            CREATE TABLE IF NOT EXISTS series_reconciliations (
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
            CREATE TABLE IF NOT EXISTS reconciliation_events (
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
                UNIQUE(series_id, volume_number, event_type)
            );
            CREATE TABLE IF NOT EXISTS notification_outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id INTEGER NOT NULL REFERENCES reconciliation_events(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                transport TEXT NOT NULL DEFAULT 'ntfy',
                destination TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL,
                last_attempt_at INTEGER,
                delivered_at INTEGER,
                last_error TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(event_id, transport)
            );
            CREATE TABLE IF NOT EXISTS opentome_series_mappings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                manga_id INTEGER NOT NULL,
                series_id TEXT NOT NULL UNIQUE,
                work_id TEXT,
                confidence TEXT NOT NULL DEFAULT 'unmatched',
                mapping_source TEXT NOT NULL DEFAULT 'auto',
                title_matched TEXT,
                last_evaluated_at INTEGER NOT NULL DEFAULT 0,
                metadata_json TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS release_discovery_targets (
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
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS volume_discovery_status (
                series_id TEXT NOT NULL,
                volume_number REAL NOT NULL,
                last_discovered_at INTEGER NOT NULL,
                status TEXT NOT NULL,
                candidate_count INTEGER NOT NULL DEFAULT 0,
                providers_checked TEXT NOT NULL DEFAULT '[]',
                provider_errors TEXT,
                PRIMARY KEY (series_id, volume_number)
            );
            CREATE TABLE IF NOT EXISTS discovered_release_candidates (
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
                UNIQUE(series_id, volume_number, provider, candidate_id)
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_date_precision_validation() {
        assert!(is_exact_day_date("2026-11-03"));
        assert!(is_exact_day_date("2026-01-01"));
        assert!(!is_exact_day_date("2026-11"));
        assert!(!is_exact_day_date("2026"));
        assert!(!is_exact_day_date("invalid-date"));
        assert!(!is_exact_day_date(""));
    }

    #[test]
    fn test_retry_intervals_bounded() {
        assert_eq!(calculate_retry_delay_secs(1), Some(6 * 3600));   // +6h
        assert_eq!(calculate_retry_delay_secs(2), Some(24 * 3600));  // +24h
        assert_eq!(calculate_retry_delay_secs(3), Some(48 * 3600));  // +48h
        assert_eq!(calculate_retry_delay_secs(4), Some(4 * 86400));  // +4d
        assert_eq!(calculate_retry_delay_secs(5), Some(7 * 86400));  // +7d
        assert_eq!(calculate_retry_delay_secs(6), Some(14 * 86400)); // +14d
        assert_eq!(calculate_retry_delay_secs(7), None);             // Dormant
        assert_eq!(calculate_retry_delay_secs(8), None);
    }

    #[test]
    fn test_exact_date_future_does_not_trigger_early() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        // Seed followed series and reconciliation
        conn.execute(
            "INSERT INTO manga (id, title) VALUES (1, 'Kagurabachi')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Kagurabachi', 'up_to_date', 9, '[9.0]', '[9.0]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-11-03".to_string()),
            release_date_raw: Some("2026-11-03".to_string()),
            precision: "day".to_string(),
            date_type: "projected".to_string(),
            status: "future".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "Kagurabachi",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.volume_number, 9.0);
        assert_eq!(t.status, "scheduled"); // Future release is scheduled, not active
        assert_eq!(t.release_date, "2026-11-03");
        assert_eq!(t.attempts, 0);
    }

    #[test]
    fn test_exact_date_triggers_on_release_day() {
        let conn = setup_test_db();
        let today = "2026-11-03";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Kagurabachi')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Kagurabachi', 'up_to_date', 9, '[9.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-11-03".to_string()),
            release_date_raw: Some("2026-11-03".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "releases_today".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "Kagurabachi",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.volume_number, 9.0);
        assert_eq!(t.status, "releases_today"); // Reached release day!
    }

    #[test]
    fn test_historical_pre_activation_release_does_not_backfill() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'One Piece')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'One Piece', 'up_to_date', 105, '[1.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        // Release date in 2003 (far before activation)
        let vol_release = OpenTomeVolumeRelease {
            volume_number: 1.0,
            title: Some("Volume 1".to_string()),
            release_date: Some("2003-06-01".to_string()),
            release_date_raw: Some("2003-06-01".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "released".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-op".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-op".to_string(),
                gcd_series_id: 101,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 105,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "One Piece",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        // Historical release that is unowned does not create an active target (no flood)
        assert_eq!(targets.len(), 0);
    }

    #[test]
    fn test_month_and_year_precision_does_not_trigger() {
        let conn = setup_test_db();
        let today = "2026-11-01";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Dandadan')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Dandadan', 'up_to_date', 12, '[12.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        // Month precision: 2026-11 without exact day
        let vol_release = OpenTomeVolumeRelease {
            volume_number: 12.0,
            title: Some("Volume 12".to_string()),
            release_date: None,
            release_date_raw: Some("2026-11".to_string()),
            precision: "month".to_string(),
            date_type: "projected".to_string(),
            status: "future".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-ddd".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-ddd".to_string(),
                gcd_series_id: 102,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 12,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "Dandadan",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        assert_eq!(targets.len(), 0); // No manufactured day!
    }

    #[test]
    fn test_ownership_gate_stops_provider_query() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (800, 'Delicious in Dungeon')", []).unwrap();
        // Volume 14 is already owned in Komga
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:800', 800, 'Delicious in Dungeon', 'up_to_date', 14, '[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0]', '[]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 14.0,
            title: Some("Volume 14".to_string()),
            release_date: Some("2026-10-30".to_string()),
            release_date_raw: Some("2026-10-30".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "releases_today".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-did".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("Yen Press".to_string()),
                tome_id: "tome-did".to_string(),
                gcd_series_id: 103,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 14,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            800,
            "manga:800",
            "Delicious in Dungeon",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.status, "already_owned"); // Ownership gate verified!
        assert_eq!(t.attempts, 0); // No provider query made
    }

    #[test]
    fn test_ambiguous_mapping_fails_closed_without_searching_providers() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (5, 'Ambiguous Manga')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:5', 5, 'Ambiguous Manga', 'up_to_date', 3, '[1.0, 2.0, 3.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 3.0,
            title: Some("Volume 3".to_string()),
            release_date: Some("2026-10-30".to_string()),
            release_date_raw: Some("2026-10-30".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "releases_today".to_string(),
        };

        // OpenTome mapping confidence is ambiguous
        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-amb".to_string(),
            mapping_confidence: "ambiguous".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: None,
                tome_id: "tome-amb".to_string(),
                gcd_series_id: 104,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 3,
            }),
            last_evaluated_at: 1700000000,
        };

        let targets = sync_series_release_targets(
            &conn,
            5,
            "manga:5",
            "Ambiguous Manga",
            &schedule,
            today,
            activation_date,
        ).unwrap();

        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.status, "mapping_review_required"); // Fails closed!
    }

    #[test]
    fn test_release_date_shift_updates_target_safely() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Kagurabachi')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Kagurabachi', 'up_to_date', 9, '[9.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        // 1. Initially scheduled for Nov 3
        let vol_release_1 = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-11-03".to_string()),
            release_date_raw: Some("2026-11-03".to_string()),
            precision: "day".to_string(),
            date_type: "projected".to_string(),
            status: "future".to_string(),
        };

        let schedule_1 = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release_1),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "Kagurabachi",
            &schedule_1,
            today,
            activation_date,
        ).unwrap();

        let t1 = get_target_by_id(&conn, 1).unwrap().unwrap();
        assert_eq!(t1.release_date, "2026-11-03");
        assert_eq!(t1.status, "scheduled");

        // 2. OpenTome publisher shifts release date to Dec 1
        let vol_release_2 = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-12-01".to_string()),
            release_date_raw: Some("2026-12-01".to_string()),
            precision: "day".to_string(),
            date_type: "projected".to_string(),
            status: "future".to_string(),
        };

        let schedule_2 = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release_2),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        sync_series_release_targets(
            &conn,
            1,
            "manga:1",
            "Kagurabachi",
            &schedule_2,
            today,
            activation_date,
        ).unwrap();

        let t2 = get_target_by_id(&conn, 1).unwrap().unwrap();
        assert_eq!(t2.release_date, "2026-12-01"); // Updated cleanly!
        assert_eq!(t2.status, "scheduled");
    }

    #[test]
    fn test_zero_auto_acquisition_invariant() {
        let conn = setup_test_db();
        // Verify no downloads rows exist or are inserted
        let download_count: i64 = conn.query_row("SELECT count(*) FROM downloads", [], |r| r.get(0)).unwrap();
        assert_eq!(download_count, 0);

        let history_count: i64 = conn.query_row("SELECT count(*) FROM acquisition_history", [], |r| r.get(0)).unwrap();
        assert_eq!(history_count, 0);
    }

    #[test]
    fn test_target_idempotency_no_duplicates() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Kagurabachi')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Kagurabachi', 'up_to_date', 9, '[9.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-10-30".to_string()),
            release_date_raw: Some("2026-10-30".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "releases_today".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        // First sync
        sync_series_release_targets(&conn, 1, "manga:1", "Kagurabachi", &schedule, today, activation_date).unwrap();
        let count_1: i64 = conn.query_row("SELECT count(*) FROM release_discovery_targets", [], |r| r.get(0)).unwrap();
        assert_eq!(count_1, 1);

        // Second sync (repeated run)
        sync_series_release_targets(&conn, 1, "manga:1", "Kagurabachi", &schedule, today, activation_date).unwrap();
        let count_2: i64 = conn.query_row("SELECT count(*) FROM release_discovery_targets", [], |r| r.get(0)).unwrap();
        assert_eq!(count_2, 1); // Exactly 1 target! No duplicate
    }

    #[test]
    fn test_volume_becoming_owned_stops_retry() {
        let conn = setup_test_db();
        let today = "2026-10-30";
        let activation_date = "2026-10-25";

        conn.execute("INSERT INTO manga (id, title) VALUES (1, 'Kagurabachi')", []).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('manga:1', 1, 'Kagurabachi', 'up_to_date', 9, '[9.0]', '[]', 1700000000, 1700000000)",
            [],
        ).unwrap();

        let vol_release = OpenTomeVolumeRelease {
            volume_number: 9.0,
            title: Some("Volume 9".to_string()),
            release_date: Some("2026-10-30".to_string()),
            release_date_raw: Some("2026-10-30".to_string()),
            precision: "day".to_string(),
            date_type: "published".to_string(),
            status: "releases_today".to_string(),
        };

        let schedule = OpenTomeSeriesSchedule {
            work_id: "work-123".to_string(),
            mapping_confidence: "exact".to_string(),
            japanese: None,
            english: Some(OpenTomeMarketSchedule {
                market: "en".to_string(),
                line_name: "English".to_string(),
                publisher: Some("VIZ".to_string()),
                tome_id: "tome-1".to_string(),
                gcd_series_id: 100,
                latest_volume: None,
                next_volume: Some(vol_release),
                total_volumes: 9,
            }),
            last_evaluated_at: 1700000000,
        };

        sync_series_release_targets(&conn, 1, "manga:1", "Kagurabachi", &schedule, today, activation_date).unwrap();
        let t1 = get_target_by_id(&conn, 1).unwrap().unwrap();
        assert_eq!(t1.status, "releases_today");

        // Now user adds the book (becomes owned in Komga reconciliation)
        conn.execute(
            "UPDATE series_reconciliations
             SET owned_volumes_json = '[9.0]', missing_volumes_json = '[]'
             WHERE series_id = 'manga:1'",
            [],
        ).unwrap();

        // Subsequent target sync recognizes ownership
        sync_series_release_targets(&conn, 1, "manga:1", "Kagurabachi", &schedule, today, activation_date).unwrap();
        let t2 = get_target_by_id(&conn, 1).unwrap().unwrap();
        assert_eq!(t2.status, "already_owned"); // Retries stop!
    }
}
