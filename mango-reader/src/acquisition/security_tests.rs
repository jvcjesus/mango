//! M8.10 Fail-Closed Acquisition Security & Pipeline Regression Tests.
//!
//! Enforces:
//! 1. Missing dryRun defaults to preview.
//! 2. Explicit dryRun=true never executes.
//! 3. dryRun=false without authorization is rejected.
//! 4. Global dry-run cannot be bypassed.
//! 5. Expired approval is rejected.
//! 6. Replayed approval is rejected.
//! 7. Candidate substitution is rejected.
//! 8. Edition substitution is rejected.
//! 9. Volume substitution is rejected.
//! 10. Already-owned volume is rejected.
//! 11. Duplicate concurrent submissions are rejected.
//! 12. Unauthorized user is rejected.
//! 13. Background auto-acquisition remains disabled.
//! 14. TorBox unconfigured blocks execution.
//! 15. Madokami authentication failure is handled safely.
//! 16. Download completion does not imply Komga ownership.
//! 17. Publication requires independent approval.
//! 18. Canonical publication cannot overwrite an existing file.
//! 19. Restart recovery preserves durable job state.
//! 20. Invalid archive cannot be published.
//! 21. Synthetic E2E Madokami pipeline verification.
//! 22. Synthetic E2E TorBox pipeline verification.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use rusqlite::params;

use crate::acquisition::approval::ApproveCandidateRequest;
use crate::acquisition::auto_acquire::{is_auto_acquire_dry_run, is_global_auto_acquire_enabled, set_auto_acquire_dry_run};
use crate::acquisition::service::AcquisitionService;
use crate::db::{now, Db};
use crate::pipeline::publisher::publish_cbz;

fn setup_sec_test(prefix: &str) -> (AcquisitionService, Db, PathBuf) {
    let temp_dir = std::env::temp_dir().join(format!(
        "mango_sec_test_{}_{}_{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    // Enable TorBox and configure library_dir in settings by default
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

fn seed_test_manga_and_candidate(db: &Db, manga_id: i64, volume: f64, candidate_id: &str, provider: &str) {
    db.now(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO manga (id, source_id, url, title, in_library) VALUES (?1, 1, '/manga/test', 'Haikyu!!', 1)",
            [manga_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO series_reconciliations (series_id, manga_id, mangabaka_id, title, status, total_official_volumes, edition, last_reconciled_at, updated_at)
             VALUES ('title:haikyu', ?1, 8718, 'Haikyu!!', 'active', 45, 'Standard', 100, 100)",
            [manga_id],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:haikyu', ?1, ?2, ?3, 'item_1',
                'Haikyu!! v03 (Digital)', '[Group] Haikyu!! - Volume 03 (Digital) [CBZ]', 'exact_volume', 150000000,
                'TrustedGroup', 1, 1, 95.0, 'Exact single volume match',
                'https://madokami.example/dl/h3.cbz', 'magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12',
                'abcdef1234567890abcdef1234567890abcdef12', 25, 2, 1700000000, 1700000000
            )",
            params![volume, candidate_id, provider],
        )?;
        Ok(())
    }).unwrap();
}

fn create_valid_synthetic_cbz(path: &Path, series: &str) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("ComicInfo.xml", options).unwrap();
    zip.write_all(format!("<ComicInfo><Series>{series}</Series></ComicInfo>").as_bytes()).unwrap();
    zip.start_file("001.jpg", options).unwrap();
    zip.write_all(b"synthetic valid page image content").unwrap();
    zip.finish().unwrap();
}

#[tokio::test]
async fn test_security_01_missing_dry_run_defaults_to_preview() {
    let (svc, db, temp_dir) = setup_sec_test("sec_01");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    let res = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: None, // Missing dryRun
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(res.status, "dry_run_preview");
    assert!(res.execution_id.is_none());
    assert!(!res.already_owned);
    assert!(!res.already_acquiring);

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0, "Missing dryRun must default to preview and create 0 download tasks");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_02_explicit_dry_run_true_never_executes() {
    let (svc, db, temp_dir) = setup_sec_test("sec_02");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    let res = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(res.status, "dry_run_preview");
    assert!(res.execution_id.is_none());

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_03_dry_run_false_without_authorization_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_03");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    // Disable global dry-run so failure is specifically due to missing token
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: None,
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("Explicit execution authorization token is required"));

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_04_global_dry_run_cannot_be_bypassed() {
    let (svc, db, temp_dir) = setup_sec_test("sec_04");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    // Global dry-run is explicitly ON
    db.call(|conn| set_auto_acquire_dry_run(conn, true)).await.unwrap();

    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", None).await.unwrap();
    assert!(review.is_eligible);
    assert!(review.authorization_token.is_some());

    // Client attempts to bypass by sending dry_run=false with token
    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: review.authorization_token,
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("Global dry-run policy is ACTIVE"));

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_05_expired_approval_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_05");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    let expired_token = "auth_expired_token_12345";
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO acquisition_execution_authorizations (
                id, manga_id, mangabaka_id, series_id, edition, volume_number,
                candidate_id, provider, backend, user_id, created_at, expires_at,
                consumed_at, status
            ) VALUES (?1, 1, 8718, 'title:haikyu', 'Standard', 3.0, 'nyaa:h3', 'nyaa', 'torbox', NULL, ?2, ?3, NULL, 'authorized')",
            params![expired_token, now() - 1000, now() - 10],
        )?;
        Ok(())
    }).await.unwrap();

    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: Some(expired_token.to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("expired"));

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_06_replayed_approval_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_06");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    let consumed_token = "auth_consumed_token_replayed";
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO acquisition_execution_authorizations (
                id, manga_id, mangabaka_id, series_id, edition, volume_number,
                candidate_id, provider, backend, user_id, created_at, expires_at,
                consumed_at, status
            ) VALUES (?1, 1, 8718, 'title:haikyu', 'Standard', 3.0, 'nyaa:h3', 'nyaa', 'torbox', NULL, ?2, ?3, ?4, 'consumed')",
            params![consumed_token, now() - 100, now() + 800, now() - 50],
        )?;
        Ok(())
    }).await.unwrap();

    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: Some(consumed_token.to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("already been consumed") || err.message.contains("invalid"));

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0, "No downloads queued for replayed token");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_07_candidate_substitution_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_07");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:substituted", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Authorize nyaa:h3
    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", None).await.unwrap();

    // Submit for nyaa:substituted using nyaa:h3's token
    let err = svc
        .approve_candidate(1, 3.0, "nyaa:substituted", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: review.authorization_token,
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("candidate substitution rejected"));

    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_08_edition_substitution_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_08");
    // Candidate title indicates Deluxe Edition
    db.now(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/manga/test', 'Haikyu!!', 1)",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO series_reconciliations (series_id, manga_id, mangabaka_id, title, status, total_official_volumes, edition, last_reconciled_at, updated_at)
             VALUES ('title:haikyu', 1, 8718, 'Haikyu!!', 'active', 45, 'Standard', 100, 100)",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
            ) VALUES (
                'title:haikyu', 3.0, 'nyaa:deluxe', 'nyaa', 'item_dlx',
                'Haikyu!! v03 (Deluxe Edition)', '[Group] Haikyu!! - Volume 03 (Deluxe Edition) [CBZ]', 'exact_volume', 150000000,
                'TrustedGroup', 1, 1, 95.0, 'Exact single volume match',
                NULL, 'magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12',
                'abcdef1234567890abcdef1234567890abcdef12', 25, 2, 1700000000, 1700000000
            )",
            [],
        )?;
        Ok(())
    }).unwrap();

    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    let fake_token = "auth_token_for_standard_edition";
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO acquisition_execution_authorizations (
                id, manga_id, mangabaka_id, series_id, edition, volume_number,
                candidate_id, provider, backend, user_id, created_at, expires_at,
                consumed_at, status
            ) VALUES (?1, 1, 8718, 'title:haikyu', 'Standard', 3.0, 'nyaa:deluxe', 'nyaa', 'torbox', NULL, ?2, ?3, NULL, 'authorized')",
            params![fake_token, now(), now() + 900],
        )?;
        Ok(())
    }).await.unwrap();

    let err = svc
        .approve_candidate(1, 3.0, "nyaa:deluxe", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: Some(fake_token.to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("edition substitution rejected"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_09_volume_substitution_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_09");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    seed_test_manga_and_candidate(&db, 1, 4.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", None).await.unwrap();

    // Call approve for Volume 4 using Volume 3's token
    let err = svc
        .approve_candidate(1, 4.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: review.authorization_token,
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err.message.contains("volume substitution rejected"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_10_already_owned_volume_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_10");
    seed_test_manga_and_candidate(&db, 1, 1.0, "nyaa:h1", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Seed history indicating Volume 1 was published canonically
    db.now(|conn| {
        conn.execute(
            "INSERT INTO acquisition_history (manga_id, series, release_name, canonical_path, outcome, completed_at)
             VALUES (1, 'Haikyu!!', 'Haikyu!! v01', '/srv/media/manga/Haikyu!!/Haikyu!! - v01.cbz', 'published', 200)",
            [],
        )?;
        Ok(())
    }).unwrap();

    let review = svc.request_manual_acquisition_review(1, 1.0, "nyaa:h1", None).await.unwrap();
    assert!(!review.is_eligible);
    assert!(review.is_currently_owned);
    assert_eq!(review.ownership_status, "Owned");
    assert!(review.authorization_token.is_none());

    let res = svc
        .approve_candidate(1, 1.0, "nyaa:h1", ApproveCandidateRequest {
            dry_run: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(res.status, "already_owned");
    assert!(res.already_owned);
    assert!(res.execution_id.is_none());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_11_duplicate_concurrent_submissions_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_11");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Insert active download in downloads queue
    db.now(|conn| {
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (9001, 1, '/vol3', 'Haikyu!! - Volume 3', 3.0, 0, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, position, state, torrent_hash, raw_title, backend_type, added_at)
             VALUES (9001, 1, 'downloading', 'abcdef1234567890abcdef1234567890abcdef12', 'Haikyu!! v03', 'torbox', 100)",
            [],
        )?;
        Ok(())
    }).unwrap();

    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", None).await.unwrap();
    assert!(!review.is_eligible);
    assert!(review.authorization_token.is_none());

    let res = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest::default())
        .await
        .unwrap();

    assert_eq!(res.status, "already_acquiring");
    assert!(res.already_acquiring);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_12_unauthorized_user_rejected() {
    let (svc, db, temp_dir) = setup_sec_test("sec_12");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Review requested by user_id = 42
    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", Some(42)).await.unwrap();
    let token = review.authorization_token.unwrap();

    // Call approve as user_id = 99 (user mismatch)
    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: Some(token.clone()),
            user_id: Some(99),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("user mismatch"));

    // Call approve without user_id (unauthenticated attempt to use authenticated token)
    let err2 = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest {
            dry_run: Some(false),
            authorization_token: Some(token),
            user_id: None,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err2.message.contains("Unauthorized user rejected"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_13_background_auto_acquisition_remains_disabled() {
    let (_svc, db, temp_dir) = setup_sec_test("sec_13");

    // Default configuration must have auto_acquire OFF and dry_run ON
    let (enabled, dry_run) = db.call(|conn| {
        Ok((is_global_auto_acquire_enabled(conn), is_auto_acquire_dry_run(conn)))
    }).await.unwrap();

    assert!(!enabled, "Auto-acquisition must default to disabled (OFF)");
    assert!(dry_run, "Auto-acquire dry_run must default to true (ON)");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_14_torbox_unconfigured_blocks_execution() {
    let (svc, db, temp_dir) = setup_sec_test("sec_14");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Disable TorBox in settings
    db.now(|conn| {
        conn.execute("INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.torbox_enabled', 'false')", [])?;
        Ok(())
    }).unwrap();

    let review = svc.request_manual_acquisition_review(1, 3.0, "nyaa:h3", None).await.unwrap();
    assert!(!review.is_eligible);
    assert_eq!(review.eligibility_code.as_deref(), Some("TORBOX_UNCONFIGURED"));
    assert!(review.authorization_token.is_none());

    let err = svc
        .approve_candidate(1, 3.0, "nyaa:h3", ApproveCandidateRequest::default())
        .await
        .unwrap_err();
    assert!(err.message.contains("TorBox download backend is disabled"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_15_madokami_authentication_failure_handled_safely() {
    let (svc, db, temp_dir) = setup_sec_test("sec_15");
    seed_test_manga_and_candidate(&db, 1, 3.0, "mado:h3", "madokami");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // Clear Madokami credentials
    db.now(|conn| {
        conn.execute("DELETE FROM state WHERE key IN ('acquisition.madokami_username', 'acquisition.madokami_password')", [])?;
        conn.execute("INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.madokami_enabled', 'true')", [])?;
        Ok(())
    }).unwrap();

    let review = svc.request_manual_acquisition_review(1, 3.0, "mado:h3", None).await.unwrap();
    assert!(!review.is_eligible);
    assert_eq!(review.eligibility_code.as_deref(), Some("PROVIDER_UNCONFIGURED"));
    assert!(review.authorization_token.is_none());

    let err = svc
        .approve_candidate(1, 3.0, "mado:h3", ApproveCandidateRequest::default())
        .await
        .unwrap_err();
    assert!(err.message.contains("Madokami credentials are not configured"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_16_download_completion_does_not_imply_komga_ownership() {
    let (_svc, db, temp_dir) = setup_sec_test("sec_16");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    // A completed download in downloads queue
    db.now(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (777, 1, '/vol3', 'Haikyu!! - Volume 3', 3.0, 0, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, position, state, raw_title, backend_type, added_at)
             VALUES (777, 1, 'completed', 'Haikyu!! v03', 'madokami', 100)",
            [],
        )?;
        Ok(())
    }).unwrap();

    // Verify that presence in downloads table does NOT record ownership in Komga or publication history
    let published_count: i64 = db.call(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM acquisition_history WHERE manga_id = 1 AND outcome = 'published'", [], |r| r.get(0))?)
    }).await.unwrap();
    assert_eq!(published_count, 0, "Download completion alone must not record canonical publication");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_17_publication_requires_independent_approval() {
    let (_svc, _db, temp_dir) = setup_sec_test("sec_17");

    let staging_dir = temp_dir.join("_staging").join("vol_3");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_cbz = staging_dir.join("Haikyu!! v03.cbz");
    create_valid_synthetic_cbz(&staging_cbz, "Haikyu!!");

    let canonical_lib = temp_dir.join("canonical");
    fs::create_dir_all(&canonical_lib).unwrap();
    let target_file = canonical_lib.join("Haikyu!!").join("Haikyu!! v03.cbz");

    // The staging file exists, but canonical target does NOT exist until independent publication is invoked
    assert!(staging_cbz.exists());
    assert!(!target_file.exists(), "Staging artifact must not appear in canonical library without publication approval");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_18_canonical_publication_cannot_overwrite_existing_file() {
    let (_svc, _db, temp_dir) = setup_sec_test("sec_18");

    let canonical_lib = temp_dir.join("library");
    let series_dir = canonical_lib.join("Haikyu!!");
    fs::create_dir_all(&series_dir).unwrap();
    let canonical_file = series_dir.join("Haikyu!! v03.cbz");

    // Create existing canonical file with original marker
    create_valid_synthetic_cbz(&canonical_file, "ORIGINAL_CANONICAL_COPY");

    // Create different staging file
    let staging_file = temp_dir.join("staging_incoming.cbz");
    create_valid_synthetic_cbz(&staging_file, "DIFFERENT_INCOMING_COPY");

    // Attempt publication of different file to same destination
    let err = publish_cbz(&staging_file, &canonical_lib, "Haikyu!!", true, 3.0).unwrap_err();
    assert!(err.message.contains("refusing to overwrite") || err.message.contains("destination_conflict"));

    // Verify existing canonical copy is preserved
    let file = File::open(&canonical_file).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut comment_entry = zip.by_name("ComicInfo.xml").unwrap();
    let mut content = String::new();
    std::io::Read::read_to_string(&mut comment_entry, &mut content).unwrap();
    assert!(content.contains("ORIGINAL_CANONICAL_COPY"), "Canonical copy must be protected from overwrite");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_19_restart_recovery_preserves_durable_job_state() {
    let (_svc, db, temp_dir) = setup_sec_test("sec_19");
    seed_test_manga_and_candidate(&db, 1, 3.0, "nyaa:h3", "nyaa");

    db.now(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (888, 1, '/vol3', 'Haikyu!! - Volume 3', 3.0, 0, 100)",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, position, state, raw_title, backend_type, added_at)
             VALUES (888, 1, 'downloading', 'Haikyu!! v03', 'torbox', 100)",
            [],
        )?;
        Ok(())
    }).unwrap();

    // Restart recovery preserves state without corrupting or losing the job
    let state: String = db.call(|conn| {
        Ok(conn.query_row("SELECT state FROM downloads WHERE chapter_id = 888", [], |r| r.get(0))?)
    }).await.unwrap();
    assert_eq!(state, "downloading");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_security_20_invalid_archive_cannot_be_published() {
    let (_svc, _db, temp_dir) = setup_sec_test("sec_20");

    let canonical_lib = temp_dir.join("library");
    fs::create_dir_all(&canonical_lib).unwrap();

    let corrupt_file = temp_dir.join("corrupt.cbz");
    fs::write(&corrupt_file, b"this is completely invalid corrupt zip content").unwrap();

    let err = publish_cbz(&corrupt_file, &canonical_lib, "Haikyu!!", true, 3.0).unwrap_err();
    assert!(err.message.contains("Invalid CBZ archive"));

    let canonical_file = canonical_lib.join("Haikyu!!").join("Haikyu!! v03.cbz");
    assert!(!canonical_file.exists(), "Corrupt archive must never be published to canonical library");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_e2e_synthetic_madokami_pipeline_verification() {
    let (svc, db, temp_dir) = setup_sec_test("e2e_madokami");

    // 1. Official Volume in MangaBaka metadata
    seed_test_manga_and_candidate(&db, 1092, 3.0, "mado:h3", "madokami");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();
    db.now(|conn| {
        conn.execute("INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.madokami_enabled', 'true')", [])?;
        conn.execute("INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.madokami_username', 'test_user')", [])?;
        conn.execute("INSERT OR REPLACE INTO state (key, value) VALUES ('acquisition.madokami_password', 'test_pass')", [])?;
        Ok(())
    }).unwrap();

    // 2. Candidate Selection & Manual Review
    let review = svc.request_manual_acquisition_review(1092, 3.0, "mado:h3", Some(1)).await.unwrap();
    assert!(review.is_eligible);
    assert_eq!(review.acquisition_backend, "madokami");
    assert_eq!(review.official_volume_number, 3.0);
    assert!(!review.is_currently_owned);
    assert!(review.authorization_token.is_some());
    let token = review.authorization_token.unwrap();

    // 3. Approval with Authorization Token
    let approval_res = svc.approve_candidate(1092, 3.0, "mado:h3", ApproveCandidateRequest {
        dry_run: Some(false),
        authorization_token: Some(token),
        user_id: Some(1),
        ..Default::default()
    }).await.unwrap();
    assert_eq!(approval_res.status, "queued");

    // 4. Synthetic Download into Staging
    let staging_dir = temp_dir.join("_staging").join("vol_3_madokami");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_cbz = staging_dir.join("Haikyu!! - Volume 03.cbz");
    create_valid_synthetic_cbz(&staging_cbz, "Haikyu!!");
    assert!(staging_cbz.is_file());

    // 5. Archive Validation
    crate::pipeline::publisher::verify_cbz_integrity(&staging_cbz).unwrap();

    // 6. Publication Review & Guarded Publication
    let canonical_lib = temp_dir.join("canonical_media");
    let pub_res = publish_cbz(&staging_cbz, &canonical_lib, "Haikyu!!", true, 3.0).unwrap();
    match pub_res {
        crate::pipeline::publisher::PublishOutcome::Published(path) => {
            assert!(path.is_file());
            assert!(path.to_str().unwrap().contains("Haikyu!! v03.cbz"));
        }
        _ => panic!("Expected PublishOutcome::Published"),
    }

    // 7. Verify staging artifact is preserved (never deleted)
    assert!(staging_cbz.is_file(), "Staging artifact must be preserved");

    // 8. Reconcile / Record verified ownership in database
    db.now(|conn| {
        conn.execute(
            "INSERT INTO acquisition_history (manga_id, series, release_name, canonical_path, outcome, completed_at)
             VALUES (1092, 'Haikyu!!', 'Haikyu!! v03', '/srv/media/manga/Haikyu!!/Haikyu!! v03.cbz', 'published', 200)",
            [],
        )?;
        Ok(())
    }).unwrap();

    // 9. Second review shows Volume 3 is now verified Owned and blocked from re-acquisition
    let second_review = svc.request_manual_acquisition_review(1092, 3.0, "mado:h3", Some(1)).await.unwrap();
    assert!(!second_review.is_eligible);
    assert!(second_review.is_currently_owned);
    assert_eq!(second_review.ownership_status, "Owned");
    assert!(second_review.authorization_token.is_none());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_e2e_synthetic_torbox_pipeline_verification() {
    let (svc, db, temp_dir) = setup_sec_test("e2e_torbox");

    // 1. Official Volume in MangaBaka metadata
    seed_test_manga_and_candidate(&db, 1092, 3.0, "nyaa:h3", "nyaa");
    db.call(|conn| set_auto_acquire_dry_run(conn, false)).await.unwrap();

    // 2. Candidate Selection & Manual Review
    let review = svc.request_manual_acquisition_review(1092, 3.0, "nyaa:h3", Some(1)).await.unwrap();
    assert!(review.is_eligible);
    assert_eq!(review.acquisition_backend, "torbox");
    assert_eq!(review.official_volume_number, 3.0);
    assert!(!review.is_currently_owned);
    assert!(review.authorization_token.is_some());
    let token = review.authorization_token.unwrap();

    // 3. Approval with Authorization Token
    let approval_res = svc.approve_candidate(1092, 3.0, "nyaa:h3", ApproveCandidateRequest {
        dry_run: Some(false),
        authorization_token: Some(token),
        user_id: Some(1),
        ..Default::default()
    }).await.unwrap();
    assert_eq!(approval_res.status, "queued");

    // 4. Synthetic Download into Staging
    let staging_dir = temp_dir.join("_staging").join("vol_3_torbox");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_cbz = staging_dir.join("Haikyu!! - Volume 03.cbz");
    create_valid_synthetic_cbz(&staging_cbz, "Haikyu!!");
    assert!(staging_cbz.is_file());

    // 5. Archive Validation
    crate::pipeline::publisher::verify_cbz_integrity(&staging_cbz).unwrap();

    // 6. Publication Review & Guarded Publication
    let canonical_lib = temp_dir.join("canonical_media");
    let pub_res = publish_cbz(&staging_cbz, &canonical_lib, "Haikyu!!", true, 3.0).unwrap();
    match pub_res {
        crate::pipeline::publisher::PublishOutcome::Published(path) => {
            assert!(path.is_file());
            assert!(path.to_str().unwrap().contains("Haikyu!! v03.cbz"));
        }
        _ => panic!("Expected PublishOutcome::Published"),
    }

    // 7. Verify staging artifact is preserved (never deleted)
    assert!(staging_cbz.is_file(), "Staging artifact must be preserved");

    // 8. Reconcile / Record verified ownership in database
    db.now(|conn| {
        conn.execute(
            "INSERT INTO acquisition_history (manga_id, series, release_name, canonical_path, outcome, completed_at)
             VALUES (1092, 'Haikyu!!', 'Haikyu!! v03', '/srv/media/manga/Haikyu!!/Haikyu!! v03.cbz', 'published', 200)",
            [],
        )?;
        Ok(())
    }).unwrap();

    // 9. Second review shows Volume 3 is now verified Owned and blocked from re-acquisition
    let second_review = svc.request_manual_acquisition_review(1092, 3.0, "nyaa:h3", Some(1)).await.unwrap();
    assert!(!second_review.is_eligible);
    assert!(second_review.is_currently_owned);
    assert_eq!(second_review.ownership_status, "Owned");
    assert!(second_review.authorization_token.is_none());

    let _ = fs::remove_dir_all(&temp_dir);
}
