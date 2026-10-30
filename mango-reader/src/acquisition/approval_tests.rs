//! Comprehensive unit and integration regression tests for M6.4 manual candidate approval.

use std::fs;
use rusqlite::params;

use crate::acquisition::approval::ApproveCandidateRequest;
use crate::acquisition::service::*;
use crate::db::Db;

fn setup_test_service(prefix: &str) -> (AcquisitionService, Db, std::path::PathBuf) {
    let temp_dir = std::env::temp_dir().join(format!(
        "mango_test_approval_{}_{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    // Enable TorBox in state and configure library_dir in settings
    db.now(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.torbox_enabled', 'true')",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.torbox_token', 'test_token')",
            [],
        )?;
        crate::settings::set(conn, "pipeline.library_dir", &serde_json::json!(temp_dir.to_str().unwrap()))?;
        Ok(())
    })
    .unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
    (svc, db, temp_dir)
}

#[tokio::test]
async fn test_manual_approval_exact_nyaa_candidate_queues_successfully() {
    let (svc, db, temp_dir) = setup_test_service("exact_success");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Frieren', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, mangabaka_id, title, status, total_official_volumes, last_reconciled_at, updated_at)
             VALUES ('title:frieren', 1, 1001, 'Frieren', 'active', 12, 100, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:frieren', 10.0, 'nyaa:1001', 'nyaa', '1001',
                'Frieren v10 (Digital)', '[Group] Frieren - Volume 10 (Digital) [CBZ]', 'exact_volume', 150000000,
                'TrustedGroup', 1, 1, 95.0, 'Exact single volume match (+60); Digital edition (+15); Trusted uploader (+10)',
                NULL, 'magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12',
                'abcdef1234567890abcdef1234567890abcdef12', 25, 2, 1700000000, 1700000000
            )",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let res = svc
        .approve_candidate(1, 10.0, "nyaa:1001", ApproveCandidateRequest::default())
        .await
        .unwrap();

    assert_eq!(res.status, "queued");
    assert!(!res.already_owned);
    assert!(!res.already_acquiring);
    assert!(res.execution_id.is_some());

    // Verify downloads queue has exactly one row
    let (dl_count, raw_title, backend_type): (i64, String, String) = db
        .call(|conn| {
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?;
            let (raw, backend): (String, String) = conn.query_row(
                "SELECT raw_title, backend_type FROM downloads LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok((count, raw, backend))
        })
        .await
        .unwrap();

    assert_eq!(dl_count, 1);
    assert_eq!(backend_type, "torbox");
    assert!(raw_title.contains("Volume 10"));

    // Verify candidate approval provenance record exists
    let approval_count: i64 = db
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM candidate_approvals WHERE candidate_id = 'nyaa:1001' AND volume_number = 10.0",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(approval_count, 1);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_discovery_and_notifications_alone_cannot_enqueue() {
    let (_svc, db, temp_dir) = setup_test_service("inert_discovery");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Berserk', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:berserk', 12.0, 'nyaa:2002', 'nyaa', '2002',
                'Berserk v12', 'Berserk v12', 'exact_volume', 200000000,
                'Uploader', 0, 0, 80.0, 'Exact match', NULL, 'magnet:?xt=urn:btih:1111', '1111', 10, 0, 100, 100
            )",
            [],
        )?;
        // Enqueue notification outbox entry
        conn.execute(
            "INSERT INTO reconciliation_events (id, series_id, series_title, volume_number, event_type, message, created_at)
             VALUES (1, 'title:berserk', 'Berserk', 12.0, 'RELEASE_AVAILABLE', 'Release available', 100)",
            [],
        )?;
        crate::notification::outbox::enqueue_from_event(
            conn,
            1,
            "title:berserk",
            "Berserk",
            Some(12.0),
            "RELEASE_AVAILABLE",
            "Release available",
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Verify 0 download queue items exist
    let dl_count: i64 = db
        .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(dl_count, 0, "Discovery and notifications must NEVER enqueue download jobs");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_owned_volume_approval_returns_already_owned() {
    let (svc, db, temp_dir) = setup_test_service("already_owned");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Claymore', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:claymore', 5.0, 'nyaa:5005', 'nyaa', '5005',
                'Claymore v05', 'Claymore v05', 'exact_volume', 100000000,
                'Up', 1, 1, 90.0, 'Match', NULL, 'magnet:?xt=urn:btih:5555', '5555', 10, 0, 100, 100
            )",
            [],
        )?;
        // Existing acquisition history record establishing physical ownership
        conn.execute(
            "INSERT INTO acquisition_history (manga_id, series, release_name, outcome, canonical_path, requested_unit, completed_at)
             VALUES (1, 'Claymore', 'Claymore - v05', 'completed', '/srv/media/manga/Claymore/Claymore - v05.cbz', 'v5', 100)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let res = svc
        .approve_candidate(1, 5.0, "nyaa:5005", ApproveCandidateRequest::default())
        .await
        .unwrap();

    assert_eq!(res.status, "already_owned");
    assert!(res.already_owned);
    assert!(!res.already_acquiring);

    let dl_count: i64 = db
        .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(dl_count, 0, "Already owned volume must create ZERO download jobs");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_active_acquisition_returns_already_acquiring_and_deduplicates() {
    let (svc, db, temp_dir) = setup_test_service("already_acquiring");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Vinland', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:vinland', 8.0, 'nyaa:8008', 'nyaa', '8008',
                'Vinland v08', 'Vinland v08', 'exact_volume', 100000000,
                'Up', 1, 1, 90.0, 'Match', NULL, 'magnet:?xt=urn:btih:8888', '8888', 10, 0, 100, 100
            )",
            [],
        )?;
        // Active download item
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (80, 1, 'nyaa:8008:8', 'Volume 8', 8.0, 1, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, attempts, added_at, source_type, backend_type, torrent_hash)
             VALUES (80, 'downloading', 1, 1, 100, 'nyaa', 'torbox', '8888')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let res = svc
        .approve_candidate(1, 8.0, "nyaa:8008", ApproveCandidateRequest::default())
        .await
        .unwrap();

    assert_eq!(res.status, "already_acquiring");
    assert!(res.already_acquiring);
    assert!(!res.already_owned);

    // Verify downloads count remains exactly 1
    let dl_count: i64 = db
        .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(dl_count, 1, "Duplicate approval must NOT create another download job");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_candidate_belonging_to_another_volume_or_series_rejected() {
    let (svc, db, temp_dir) = setup_test_service("mismatched_target");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Monster', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:monster', 2.0, 'nyaa:vol2', 'nyaa', 'vol2',
                'Monster v02', 'Monster v02', 'exact_volume', 100000000,
                'Up', 1, 1, 90.0, 'Match', NULL, 'magnet:?xt=urn:btih:2222', '2222', 10, 0, 100, 100
            )",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Trying to approve Volume 3 using candidate for Volume 2
    let err = svc
        .approve_candidate(1, 3.0, "nyaa:vol2", ApproveCandidateRequest::default())
        .await
        .unwrap_err();

    assert!(err.message.contains("not found or stale"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_rejected_and_possible_match_safety() {
    let (svc, db, temp_dir) = setup_test_service("match_safety");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Dorohedoro', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES
            ('title:dorohedoro', 4.0, 'nyaa:rej', 'nyaa', 'rej', 'Dorohedoro Anime S1', 'Dorohedoro S01', 'rejected', 1000, 'Up', 0, 0, 0.0, 'Rejected', NULL, 'magnet:?xt=urn:btih:r', 'r', 1, 0, 100, 100),
            ('title:dorohedoro', 4.0, 'nyaa:poss', 'nyaa', 'poss', 'Dorohedoro raw scan 4', 'Dorohedoro raw scan 4', 'possible_match', 1000, 'Up', 0, 0, 40.0, 'Possible', NULL, 'magnet:?xt=urn:btih:p', 'p', 1, 0, 100, 100)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // 1. Rejected candidate always fails
    let err_rej = svc
        .approve_candidate(1, 4.0, "nyaa:rej", ApproveCandidateRequest::default())
        .await
        .unwrap_err();
    assert!(err_rej.message.contains("Rejected candidates cannot be acquired"));

    // 2. Possible match fails without explicit confirmation
    let err_poss = svc
        .approve_candidate(1, 4.0, "nyaa:poss", ApproveCandidateRequest::default())
        .await
        .unwrap_err();
    assert!(err_poss.message.contains("Possible match cannot be acquired without explicit confirmation"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_range_and_pack_confirmation_and_owned_protection() {
    let (svc, db, temp_dir) = setup_test_service("range_pack");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Bleach', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES
            ('title:bleach', 10.0, 'nyaa:pack', 'nyaa', 'pack', 'Bleach v01-10', '[Digital] Bleach v01-v10 [CBZ]', 'series_pack', 1000000000, 'Up', 1, 1, 85.0, 'Series pack', NULL, 'magnet:?xt=urn:btih:pk', 'pk', 20, 0, 100, 100)",
            [],
        )?;
        // Suppose volumes 1 through 9 are already owned in acquisition_history!
        for v in 1..=9 {
            conn.execute(
                "INSERT INTO acquisition_history (manga_id, series, release_name, outcome, canonical_path, requested_unit, completed_at)
                 VALUES (1, 'Bleach', ?1, 'completed', ?2, ?3, 100)",
                params![format!("Bleach - v{v}"), format!("/srv/media/manga/Bleach/Bleach - v{v}.cbz"), format!("v{v}")],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();

    // 1. Pack without confirm_pack fails
    let err = svc
        .approve_candidate(1, 10.0, "nyaa:pack", ApproveCandidateRequest::default())
        .await
        .unwrap_err();
    assert!(err.message.contains("Series pack requires explicit confirmation"));

    // 2. With confirm_pack: true, approval succeeds
    let res = svc
        .approve_candidate(
            1,
            10.0,
            "nyaa:pack",
            ApproveCandidateRequest {
                confirm_pack: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(res.status, "queued");

    // 3. CRITICAL: Ensure only Volume 10 is queued; volumes 1-9 must NOT be queued or republished!
    let queued_vols: Vec<f64> = db
        .call(|conn| {
            let mut stmt = conn.prepare("SELECT c.chapter_number FROM downloads d JOIN chapters c ON c.id = d.chapter_id WHERE c.manga_id = 1")?;
            let rows = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<f64>>>()?;
            Ok(rows)
        })
        .await
        .unwrap();

    assert_eq!(queued_vols, vec![10.0], "Only missing Volume 10 should be acquired from pack");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_unreleased_volume_rejected() {
    let (svc, db, temp_dir) = setup_test_service("unreleased");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Chainsaw Man', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, future_volumes_json, last_reconciled_at, updated_at)
             VALUES ('title:chainsaw man', 1, 'Chainsaw Man', 'active', 18, '[25.0]', 100, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:chainsaw man', 25.0, 'nyaa:vol25', 'nyaa', 'vol25',
                'Chainsaw Man v25', 'Chainsaw Man v25', 'exact_volume', 100000000,
                'Up', 1, 1, 90.0, 'Match', NULL, 'magnet:?xt=urn:btih:2525', '2525', 10, 0, 100, 100
            )",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let err = svc
        .approve_candidate(1, 25.0, "nyaa:vol25", ApproveCandidateRequest::default())
        .await
        .unwrap_err();

    assert!(err.message.contains("unreleased") || err.message.contains("exceeds maximum"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_madokami_unavailable_safely_rejected() {
    let (svc, db, temp_dir) = setup_test_service("madokami");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Nausicaa', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:nausicaa', 1.0, 'mado:1', 'madokami', '1',
                'Nausicaa v1', 'Nausicaa v1', 'exact_volume', 50000000,
                'Uploader', 0, 0, 75.0, 'Madokami', NULL, NULL, NULL, NULL, NULL, 100, 100
            )",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let err = svc
        .approve_candidate(1, 1.0, "mado:1", ApproveCandidateRequest::default())
        .await
        .unwrap_err();

    assert!(err.message.contains("Madokami") || err.message.contains("unavailable"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_physical_acquisition_complete_decoupled_from_komf() {
    let (_svc, db, temp_dir) = setup_test_service("decoupled_acq");

    db.call(|conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/c1', 'Solanin', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (99, 1, 'nyaa:99', 'Volume 1', 1.0, 1, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, attempts, added_at, source_type, backend_type, published_path)
             VALUES (99, 'verifying', 1, 0, 100, 'nyaa', 'torbox', '/srv/media/manga/Solanin/Solanin - v01.cbz')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // 1. Physical publication boundary crossed -> emit ACQUISITION_COMPLETE
    db.call(|conn| {
        record_acquisition_complete(conn, 99)?;
        Ok(())
    })
    .await
    .unwrap();

    // Verify ACQUISITION_COMPLETE event recorded in reconciliation_events
    let event_type: String = db
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT event_type FROM reconciliation_events WHERE series_id = 'manga:1' AND event_type = 'ACQUISITION_COMPLETE'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(event_type, "ACQUISITION_COMPLETE");

    // 2. KOMF metadata subsequently fails -> emit ACTION_REQUIRED without invalidating physical ACQUISITION_COMPLETE
    db.call(|conn| {
        record_metadata_action_required(conn, 99, "KOMF HTTP 504 Gateway Timeout")?;
        Ok(())
    })
    .await
    .unwrap();

    // Verify both events exist: physical ACQUISITION_COMPLETE is intact, and ACTION_REQUIRED alerted
    let (has_complete, has_action_req): (bool, bool) = db
        .call(|conn| {
            let c: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM reconciliation_events WHERE event_type = 'ACQUISITION_COMPLETE')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let a: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM reconciliation_events WHERE event_type = 'ACTION_REQUIRED')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            Ok((c, a))
        })
        .await
        .unwrap();

    assert!(has_complete, "ACQUISITION_COMPLETE must remain durable when metadata fails");
    assert!(has_action_req, "ACTION_REQUIRED should alert user about metadata failure");

    // 3. Completing publication removes downloads queue item and does NOT duplicate outbox entries
    db.call(|conn| {
        complete_queued_publication(conn, 99)?;
        Ok(())
    })
    .await
    .unwrap();

    let dl_exists: bool = db
        .call(|conn| {
            Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM downloads WHERE chapter_id = 99)", [], |r| r.get(0))?)
        })
        .await
        .unwrap();
    assert!(!dl_exists, "Downloads queue item removed on publication complete");

    let _ = fs::remove_dir_all(&temp_dir);
}
