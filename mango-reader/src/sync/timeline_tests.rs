#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use crate::sync::timeline::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE manga (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                source_id INTEGER NOT NULL DEFAULT 0,
                source_manga_id INTEGER,
                catalog_manga_id INTEGER,
                mangabaka_id INTEGER,
                auto_acquire INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE series_reconciliations (
                series_id TEXT,
                manga_id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                mangabaka_id INTEGER,
                collection_id TEXT,
                market TEXT,
                edition TEXT,
                total_official_volumes INTEGER,
                total_volumes INTEGER,
                owned_volumes_json TEXT DEFAULT '[]',
                missing_volumes_json TEXT DEFAULT '[]',
                future_volumes_json TEXT DEFAULT '[]',
                review_reason TEXT
            );
            CREATE TABLE opentome_mappings (
                manga_id INTEGER PRIMARY KEY,
                work_id TEXT NOT NULL
            );
            CREATE TABLE release_discovery_targets (
                id INTEGER PRIMARY KEY,
                manga_id INTEGER,
                series_id TEXT,
                volume_number REAL NOT NULL,
                release_date TEXT NOT NULL,
                status TEXT NOT NULL
            );
            CREATE TABLE chapters (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                manga_id INTEGER NOT NULL,
                chapter_number REAL NOT NULL,
                download_path TEXT
            );
            CREATE TABLE publication_receipts (
                id TEXT PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                volume_number REAL,
                chapter_number REAL,
                canonical_path TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                published_at INTEGER NOT NULL,
                komga_notified INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE discovered_release_candidates (
                id TEXT PRIMARY KEY,
                series_id TEXT NOT NULL,
                volume_number REAL NOT NULL,
                candidate_id TEXT NOT NULL DEFAULT 'cand-1',
                provider TEXT NOT NULL,
                provider_item_id TEXT NOT NULL DEFAULT 'item-1',
                raw_title TEXT NOT NULL,
                title TEXT,
                match_type TEXT NOT NULL DEFAULT 'exact_volume',
                size_bytes INTEGER NULL DEFAULT 25000000,
                uploader TEXT NULL DEFAULT 'TrustedUploader',
                is_trusted INTEGER NOT NULL DEFAULT 1,
                is_digital INTEGER NOT NULL DEFAULT 1,
                score REAL NOT NULL,
                score_explanation TEXT NOT NULL DEFAULT 'Score',
                download_url TEXT NULL DEFAULT 'https://example.com/dl',
                magnet_url TEXT NULL DEFAULT 'magnet:?xt=urn:btih:test',
                info_hash TEXT NULL DEFAULT 'test_hash',
                seeders INTEGER NULL DEFAULT 10,
                leechers INTEGER NULL DEFAULT 2,
                published_at INTEGER NULL,
                discovered_at INTEGER NOT NULL
            );
            CREATE TABLE downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chapter_id INTEGER NOT NULL,
                state TEXT NOT NULL
            );
            CREATE TABLE backlog_acquisition_jobs (
                id TEXT PRIMARY KEY,
                manga_id INTEGER NOT NULL,
                scope TEXT NOT NULL,
                status TEXT NOT NULL,
                items_total INTEGER NOT NULL,
                items_completed INTEGER NOT NULL
            );
            CREATE TABLE reconciliation_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                series_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                message TEXT NOT NULL,
                volume_number REAL,
                created_at INTEGER NOT NULL,
                resolved_at INTEGER
            );"
        ).unwrap();
        conn
    }

    #[test]
    fn test_continuous_series_no_gaps() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (1, 'Continuous Manga', 101)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (1, 'Continuous Manga', 'up_to_date', 101, 'col-1', 5)",
            [],
        ).unwrap();

        for v in 1..=5 {
            conn.execute(
                "INSERT INTO chapters (manga_id, chapter_number, download_path)
                 VALUES (1, ?1, ?2)",
                rusqlite::params![v as f64, format!("/data/local/English/Continuous Manga/v{v}.cbz")],
            ).unwrap();
        }

        let timeline = get_series_timeline(&conn, 1).unwrap();
        assert_eq!(timeline.volumes.len(), 5);
        assert_eq!(timeline.gaps.owned_count, 5);
        assert_eq!(timeline.gaps.missing_count, 0);
        assert_eq!(timeline.gaps.interior_gap_count, 0);
        assert!(!timeline.gaps.has_interior_gaps);
        assert!(timeline.volumes.iter().all(|v| v.status == "OWNED" && v.is_owned && !v.is_gap));
    }

    #[test]
    fn test_interior_gap_detection() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (2, 'Gap Manga', 102)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (2, 'Gap Manga', 'missing_volume', 102, 'col-2', 5)",
            [],
        ).unwrap();

        // Own volumes 1, 2, 4, 5 (missing volume 3)
        for v in &[1, 2, 4, 5] {
            conn.execute(
                "INSERT INTO chapters (manga_id, chapter_number, download_path)
                 VALUES (2, ?1, ?2)",
                rusqlite::params![*v as f64, format!("/data/local/English/Gap Manga/v{v}.cbz")],
            ).unwrap();
        }

        let timeline = get_series_timeline(&conn, 2).unwrap();
        assert_eq!(timeline.volumes.len(), 5);
        assert_eq!(timeline.gaps.owned_count, 4);
        assert_eq!(timeline.gaps.missing_count, 1);
        assert_eq!(timeline.gaps.interior_gap_count, 1);
        assert!(timeline.gaps.has_interior_gaps);
        assert_eq!(timeline.gaps.gap_volume_numbers, vec![3.0]);

        let vol3 = timeline.volumes.iter().find(|v| (v.volume_number - 3.0).abs() < 0.001).unwrap();
        assert!(!vol3.is_owned);
        assert!(vol3.is_gap);
        assert_eq!(vol3.gap_type.as_deref(), Some("INTERIOR"));
        assert_eq!(vol3.status, "RELEASED_MISSING");
    }

    #[test]
    fn test_leading_and_trailing_gaps() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (3, 'Edges Manga', 103)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (3, 'Edges Manga', 'missing_volume', 103, 'col-3', 6)",
            [],
        ).unwrap();

        // Own only volumes 3 and 4
        for v in &[3, 4] {
            conn.execute(
                "INSERT INTO chapters (manga_id, chapter_number, download_path)
                 VALUES (3, ?1, ?2)",
                rusqlite::params![*v as f64, format!("/data/local/English/Edges Manga/v{v}.cbz")],
            ).unwrap();
        }

        let timeline = get_series_timeline(&conn, 3).unwrap();
        assert_eq!(timeline.volumes.len(), 6);
        assert_eq!(timeline.gaps.owned_count, 2);
        assert_eq!(timeline.gaps.missing_count, 4);
        assert_eq!(timeline.gaps.interior_gap_count, 0);
        assert!(!timeline.gaps.has_interior_gaps);
        assert_eq!(timeline.gaps.leading_gap_count, 2); // vols 1, 2
        assert_eq!(timeline.gaps.trailing_gap_count, 2); // vols 5, 6

        let vol1 = timeline.volumes.iter().find(|v| (v.volume_number - 1.0).abs() < 0.001).unwrap();
        assert_eq!(vol1.gap_type.as_deref(), Some("LEADING"));

        let vol6 = timeline.volumes.iter().find(|v| (v.volume_number - 6.0).abs() < 0.001).unwrap();
        assert_eq!(vol6.gap_type.as_deref(), Some("TRAILING"));
    }

    #[test]
    fn test_dual_storage_reading_source_separation() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (4, 'Blood on the Tracks', 25283)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (4, 'Blood on the Tracks', 'up_to_date', 25283, 'col-4', 2)",
            [],
        ).unwrap();

        // Dual storage: canonical copy AND local staging download
        conn.execute(
            "INSERT INTO chapters (manga_id, chapter_number, download_path)
             VALUES (4, 1.0, '/data/local/English/Blood on the Tracks/Blood on the Tracks v01.cbz')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chapters (manga_id, chapter_number, download_path)
             VALUES (4, 1.0, 'Local/Blood on the Tracks/Blood on the Tracks v01.cbz')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chapters (manga_id, chapter_number, download_path)
             VALUES (4, 2.0, '/data/local/English/Blood on the Tracks/Blood on the Tracks v02.cbz')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chapters (manga_id, chapter_number, download_path)
             VALUES (4, 2.0, 'Local/Blood on the Tracks/Blood on the Tracks v02.cbz')",
            [],
        ).unwrap();

        let timeline = get_series_timeline(&conn, 4).unwrap();
        assert!(timeline.reading_source_separation.has_duplicate_storage);
        assert_eq!(timeline.reading_source_separation.canonical_komga_count, 2);
        assert_eq!(timeline.reading_source_separation.local_staging_count, 2);
        assert!(timeline.reading_source_separation.storage_distinction_notes.iter().any(|n| n.contains("Dual-Storage Alignment")));
    }

    #[test]
    fn test_unresolved_collection_fail_closed() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (5, 'Blade of the Immortal', 5246)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (5, 'Blade of the Immortal', 'collection_unavailable', 5246, NULL, NULL)",
            [],
        ).unwrap();

        // Owned volume 1 only
        conn.execute(
            "INSERT INTO chapters (manga_id, chapter_number, download_path)
             VALUES (5, 1.0, '/data/local/English/Blade of the Immortal/v01.cbz')",
            [],
        ).unwrap();

        let timeline = get_series_timeline(&conn, 5).unwrap();
        assert_eq!(timeline.reconciliation_status, "collection_unavailable");
        // Because no collection is available, unowned volumes are NOT fabricated!
        assert_eq!(timeline.volumes.len(), 1);
        assert_eq!(timeline.volumes[0].volume_number, 1.0);
        assert_eq!(timeline.volumes[0].status, "OWNED");
    }

    #[test]
    fn test_candidate_attached_to_unowned_volume() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (6, 'Kagurabachi', 708)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (6, 'Kagurabachi', 'missing_volume', 708, 'col-6', 1)",
            [],
        ).unwrap();

        // Candidate discovered for volume 1
        conn.execute(
            "INSERT INTO discovered_release_candidates
             (id, series_id, volume_number, provider, raw_title, title, score, discovered_at)
             VALUES ('cand-1', 'manga:6', 1.0, 'madokami', 'Kagurabachi v01.cbz', 'Kagurabachi Vol. 1', 95.0, 1000)",
            [],
        ).unwrap();

        let timeline = get_series_timeline(&conn, 6).unwrap();
        assert_eq!(timeline.volumes.len(), 1);
        let vol1 = &timeline.volumes[0];
        assert_eq!(vol1.status, "CANDIDATE_AVAILABLE");
        assert_eq!(vol1.candidate_count, 1);
        assert_eq!(vol1.top_candidate_source.as_deref(), Some("madokami"));
        assert_eq!(vol1.top_candidate_score, Some(95.0));
        assert_eq!(vol1.top_candidate_auto_eligible, Some(true));
        assert_eq!(vol1.top_candidate_decision_code.as_deref(), Some("AUTO_ALLOWED"));
    }

    #[test]
    fn test_timeline_rejects_pre_release_candidate_from_auto_eligibility() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (7, 'Kagurabachi', 708)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (7, 'Kagurabachi', 'missing_volume', 708, 'col-7', 9)",
            [],
        ).unwrap();

        // Volume 9 is an unreleased future target (pre or date in future)
        conn.execute(
            "INSERT INTO release_discovery_targets (id, manga_id, series_id, volume_number, release_date, status)
             VALUES (101, 7, 'manga:7', 9.0, '2099-01-01', 'pre')",
            [],
        ).unwrap();

        // Candidate discovered for volume 9
        conn.execute(
            "INSERT INTO discovered_release_candidates
             (id, series_id, volume_number, provider, raw_title, title, score, discovered_at)
             VALUES ('cand-9', 'manga:7', 9.0, 'madokami', 'Kagurabachi v09.cbz', 'Kagurabachi Vol. 9', 95.0, 1000)",
            [],
        ).unwrap();

        let timeline = get_series_timeline(&conn, 7).unwrap();
        let vol9 = timeline.volumes.iter().find(|v| (v.volume_number - 9.0).abs() < 0.001).unwrap();
        // Candidate count is 1, but auto-eligible MUST be false for PRE release!
        assert_eq!(vol9.candidate_count, 1);
        assert_eq!(vol9.top_candidate_auto_eligible, Some(false));
        assert_eq!(vol9.top_candidate_decision_code.as_deref(), Some("PRE_RELEASE_REQUIRES_MANUAL"));
    }

    #[test]
    fn test_timeline_decision_code_for_ineligible_pack() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO manga (id, title, mangabaka_id) VALUES (8, 'Naruto', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (manga_id, title, status, mangabaka_id, collection_id, total_official_volumes)
             VALUES (8, 'Naruto', 'missing_volume', 1, 'col-8', 1)",
            [],
        ).unwrap();

        // Candidate is a series pack
        conn.execute(
            "INSERT INTO discovered_release_candidates
             (id, series_id, volume_number, provider, match_type, raw_title, title, score, discovered_at)
             VALUES ('cand-pack', 'manga:8', 1.0, 'nyaa', 'series_pack', 'Naruto Complete [Batch]', 'Naruto Batch', 90.0, 1000)",
            [],
        ).unwrap();

        let timeline = get_series_timeline(&conn, 8).unwrap();
        let vol1 = &timeline.volumes[0];
        assert_eq!(vol1.candidate_count, 1);
        assert_eq!(vol1.top_candidate_auto_eligible, Some(false));
        assert_eq!(vol1.top_candidate_decision_code.as_deref(), Some("PACK_REQUIRES_MANUAL"));
    }
}
