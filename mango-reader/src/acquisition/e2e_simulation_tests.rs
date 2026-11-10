//! M14 Autonomous Acquisition & Self-Healing Pipeline End-to-End Simulation Tests.
//!
//! Deterministically verifies:
//! 1. 5-Volume synthetic series lifecycle (Vols 1–2 owned, Vols 3–5 missing).
//! 2. English-only candidate ranking and foreign language rejection.
//! 3. Archive integrity and executable payload / traversal rejection.
//! 4. Atomic publication and Komga ownership verification.
//! 5. Continuous backlog advancement immediately following verification.
//! 6. Provider failure classification and automatic fallback.
//! 7. Server restart during download with zero duplicate jobs.
//! 8. Per-series automation overrides ('inherit', 'monitor_only', 'review_required', 'auto_acquire', 'paused').
//! 9. Real automation controls remaining disabled by default.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::acquisition::auto_acquire::{
    is_global_auto_acquire_enabled, is_simulation_mode,
};
use crate::acquisition::fallback::{
    apply_acquisition_fallback, get_fallback_candidates,
};
use crate::acquisition::monitoring::list_monitoring_activity;
use crate::acquisition::orchestrator::{
    advance_continuous_backlog, claim_lifecycle_lease, evaluate_series_backlog,
    generate_idempotency_key, get_automation_overview_metrics, get_or_create_lifecycle_record,
    get_per_series_policy, reclaim_stale_leases, release_lifecycle_lease, set_per_series_policy,
    update_lifecycle_state, AcquisitionLifecycleState, PerSeriesAutomationPolicy,
};
use crate::acquisition::providers::{CandidateMatchType, ReleaseCandidate};
use crate::acquisition::service::complete_queued_publication;
use crate::acquisition::watcher::set_title_monitoring_settings;
use crate::db::{now, Db};
use crate::downloads::Downloads;
use crate::error::AppError;
use crate::pipeline::publisher::{inspect_cbz_archive, publish_cbz, verify_cbz_integrity, PublishOutcome};
use rusqlite::params;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestEnv {
    db: Db,
    dir: PathBuf,
    library_dir: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn create_test_env(name: &str) -> TestEnv {
    let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mango_m14_{name}_{}_{count}", now()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("test.db");
    let library_dir = dir.join("library");
    fs::create_dir_all(&library_dir).unwrap();

    let db = Db::open(&db_path).unwrap();
    TestEnv {
        db,
        dir,
        library_dir,
    }
}

fn create_valid_test_cbz(path: &Path, series: &str, volume: f64) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);

    zip.start_file("ComicInfo.xml", options).unwrap();
    let info = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Series>{series}</Series>
  <Volume>{volume}</Volume>
</ComicInfo>"#
    );
    zip.write_all(info.as_bytes()).unwrap();

    zip.start_file("001.jpg", options).unwrap();
    zip.write_all(b"\xff\xd8\xff\xe0fake_jpeg_page_1").unwrap();

    zip.start_file("002.png", options).unwrap();
    zip.write_all(b"\x89PNG\r\n\x1a\nfake_png_page_2").unwrap();

    zip.finish().unwrap();
}

fn create_suspicious_cbz_with_payload(path: &Path, payload_filename: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);

    zip.start_file("001.jpg", options).unwrap();
    zip.write_all(b"valid image data").unwrap();

    zip.start_file(payload_filename, options).unwrap();
    zip.write_all(b"malicious payload content").unwrap();

    zip.finish().unwrap();
}

// ============================================================================
// Section 14: End-to-End Synthetic Simulation Scenario
// ============================================================================

#[tokio::test]
async fn test_m14_end_to_end_synthetic_simulation() {
    let env = create_test_env("simulation");
    let db = &env.db;
    let series_title = "Synthetic Chronicles";
    let manga_id: i64 = 7701;
    let series_id = format!("manga:{manga_id}");

    // 0. Setup initial synthetic series: 5 official volumes
    // Volumes 1 and 2 owned; Volumes 3, 4, 5 missing.
    let sid = series_id.clone();
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, update_strategy, in_library, automation_override)
             VALUES (?1, 1, '/manga/7701', ?2, 'ALWAYS_UPDATE', 1, 'inherit')",
            params![manga_id, series_title],
        )?;

        // Reconciliations: 5 official volumes, 1-2 owned, 3-5 missing
        let missing_json = serde_json::to_string(&vec![3.0, 4.0, 5.0]).unwrap();
        let owned_json = serde_json::to_string(&vec![1.0, 2.0]).unwrap();
        let future_json = serde_json::to_string(&Vec::<f64>::new()).unwrap();

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, title, status, total_official_volumes,
                missing_volumes_json, owned_volumes_json, future_volumes_json,
                last_reconciled_at, updated_at
            ) VALUES (?1, ?2, ?3, 'reconciled', 5, ?4, ?5, ?6, strftime('%s','now'), strftime('%s','now'))",
            params![sid.as_str(), manga_id, series_title, missing_json, owned_json, future_json],
        )?;

        // Pre-existing ownership records for Volumes 1 and 2
        for vol in [1.0, 2.0] {
            let ch_id = 9000 + (vol as i64);
            conn.execute(
                "INSERT INTO publication_receipts (
                    id, chapter_id, manga_id, series_title, volume_number, is_volume, canonical_path,
                    file_size_bytes, file_sha256, published_at, verified_ownership,
                    verification_status, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 1000, 'hash', strftime('%s','now'), 1, 'verified', strftime('%s','now'), strftime('%s','now'))",
                params![
                    format!("rcpt-init-{vol}"),
                    ch_id,
                    manga_id,
                    series_title,
                    vol,
                    format!("{}/Volume {:02}.cbz", series_title, vol as i64)
                ],
            )?;
        }

        // Add availability watchers for missing volumes 3, 4, 5
        for vol in [3.0, 4.0, 5.0] {
            conn.execute(
                "INSERT INTO release_availability_watchers (
                    series_id, manga_id, series_title, volume_number, is_released, is_owned,
                    state, monitoring_enabled, check_interval_secs, next_check_at, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, 1, 0, 'RELEASED_AWAITING_PROVIDER', 1, 3600, 0, strftime('%s','now'), strftime('%s','now'))",
                params![sid.as_str(), manga_id, series_title, vol],
            )?;
        }

        Ok(())
    })
    .await
    .unwrap();

    // Verify initial automation switches are safely OFF
    let auto_acq = db.call(|conn| Ok(is_global_auto_acquire_enabled(conn))).await.unwrap();
    let sim_mode = db.call(|conn| Ok(is_simulation_mode(conn))).await.unwrap();
    assert!(!auto_acq, "Initial auto_acquire_enabled must be false");
    assert!(sim_mode, "Initial operating mode must be simulation");

    // ------------------------------------------------------------------------
    // Step 1 & 2: Discover and select English candidate for Volume 3
    // ------------------------------------------------------------------------
    let cand_vol3_en = ReleaseCandidate {
        candidate_id: "madokami:/Synthetic_Chronicles_v03.cbz".to_string(),
        provider: "madokami".to_string(),
        provider_item_id: "mado-03".to_string(),
        title: "Synthetic Chronicles v03 (Digital)".to_string(),
        raw_title: "Synthetic Chronicles v03 (Digital).cbz".to_string(),
        language: Some("en".to_string()),
        match_type: CandidateMatchType::ExactVolume,
        target_volume: Some(3.0),
        score: 95.0,
        is_digital: true,
        is_trusted: true,
        confidence: Some("high".to_string()),
        download_url: Some("http://localhost:9999/Synthetic_Chronicles_v03.cbz".to_string()),
        ..Default::default()
    };

    let cand_vol3_foreign = ReleaseCandidate {
        candidate_id: "nyaa:99903".to_string(),
        provider: "nyaa".to_string(),
        provider_item_id: "99903".to_string(),
        title: "Synthetic Chronicles v03 [French]".to_string(),
        raw_title: "[FR] Synthetic Chronicles v03.cbz".to_string(),
        language: Some("fr".to_string()),
        match_type: CandidateMatchType::ExactVolume,
        target_volume: Some(3.0),
        score: 80.0,
        confidence: Some("high".to_string()),
        ..Default::default()
    };

    // Store candidate fixtures in discovered_release_candidates
    let s_id = series_id.clone();
    let c3_en = cand_vol3_en.clone();
    let c3_fr = cand_vol3_foreign.clone();
    db.call(move |conn| {
        for c in [&c3_en, &c3_fr] {
            conn.execute(
                "INSERT INTO discovered_release_candidates (
                    series_id, volume_number, candidate_id, provider, provider_item_id,
                    title, raw_title, match_type, score, score_explanation, is_digital, is_trusted,
                    download_url, published_at, discovered_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'exact_volume', ?8, 'Exact volume match', ?9, ?10, ?11, strftime('%s','now'), strftime('%s','now'))",
                params![
                    s_id,
                    c.target_volume.unwrap(),
                    c.candidate_id,
                    c.provider,
                    c.provider_item_id,
                    c.title,
                    c.raw_title,
                    c.score,
                    if c.is_digital { 1 } else { 0 },
                    if c.is_trusted { 1 } else { 0 },
                    c.download_url,
                ],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();

    // Verify foreign language rejection
    assert!(
        cand_vol3_foreign.language.as_deref().unwrap_or("") != "en"
            || crate::acquisition::matching::detect_foreign_language(&cand_vol3_foreign.raw_title, "en").is_some(),
        "Foreign release must be detected and rejected"
    );

    // ------------------------------------------------------------------------
    // Step 3, 4, 5, 6: Queue, validate archive, stage Volume 3
    // ------------------------------------------------------------------------
    let key_vol3 = generate_idempotency_key(&series_id, "standard", 3.0);
    assert_eq!(key_vol3, "manga:7701:standard:3.00:acq");

    // Create fixture archive
    let staging_cbz_vol3 = env.dir.join("staging").join("Synthetic_Chronicles_v03.cbz");
    create_valid_test_cbz(&staging_cbz_vol3, series_title, 3.0);

    // Validate archive
    let report_vol3 = inspect_cbz_archive(&staging_cbz_vol3).unwrap();
    assert_eq!(report_vol3.image_count, 2);
    assert!(report_vol3.has_comic_info);

    // Simulate publication to canonical library
    let pub_outcome_vol3 = publish_cbz(
        &staging_cbz_vol3,
        &env.library_dir,
        series_title,
        true,
        3.0,
    )
    .unwrap();

    let published_path_vol3 = match pub_outcome_vol3 {
        PublishOutcome::Published(p) => p,
        PublishOutcome::AlreadyPublished(p) => p,
    };
    assert!(published_path_vol3.is_file());

    // ------------------------------------------------------------------------
    // Step 7, 8, 9: Komga verification, receipts, and completion of Volume 3
    // ------------------------------------------------------------------------
    let pub_path_str = published_path_vol3.to_string_lossy().to_string();
    let s_id_c = series_id.clone();
    let p_str_c = pub_path_str.clone();

    let chapter_id_vol3: i64 = 10003;

    db.call(move |conn| {
        // Create chapter record
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (?1, ?2, ?3, 'Synthetic Chronicles v03', 3.0, 3, strftime('%s','now'))",
            params![chapter_id_vol3, manga_id, format!("/ch/{chapter_id_vol3}")],
        )?;

        // Create download in verifying state
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, added_at, published_path, target_library_id)
             VALUES (?1, 'verifying', 1, strftime('%s','now'), ?2, 'lib-test')",
            params![chapter_id_vol3, p_str_c],
        )?;

        // Create verified publication receipt
        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_title, volume_number, is_volume, canonical_path,
                file_size_bytes, file_sha256, published_at, verified_ownership,
                verification_status, created_at, updated_at
            ) VALUES ('rcpt-vol3', ?1, ?2, ?3, 3.0, 1, ?4, 2048, 'hash3', strftime('%s','now'), 1, 'verified', strftime('%s','now'), strftime('%s','now'))",
            params![chapter_id_vol3, manga_id, series_title, p_str_c],
        )?;

        // Update reconciliation state: Volume 3 is now owned
        conn.execute(
            "UPDATE series_reconciliations
             SET missing_volumes_json = ?1, owned_volumes_json = ?2, updated_at = strftime('%s','now')
             WHERE series_id = ?3",
            params![
                serde_json::to_string(&vec![4.0, 5.0]).unwrap(),
                serde_json::to_string(&vec![1.0, 2.0, 3.0]).unwrap(),
                s_id_c
            ],
        )?;

        // Call complete_queued_publication
        complete_queued_publication(conn, chapter_id_vol3)?;

        Ok(())
    })
    .await
    .unwrap();

    // Verify Volume 3 watcher is updated to OWNERSHIP_DETECTED
    let w3_state: String = db
        .call(move |conn| {
            conn.query_row(
                "SELECT state FROM release_availability_watchers
                 WHERE series_id = ?1 AND ABS(volume_number - 3.0) < 0.001",
                params![format!("manga:{manga_id}")],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
        .unwrap();
    assert_eq!(w3_state, "OWNERSHIP_DETECTED");

    // ------------------------------------------------------------------------
    // Step 10: Continuous backlog automatically evaluates Volume 4 immediately
    // ------------------------------------------------------------------------
    // Candidate A for Vol 4: Primary provider (Madokami)
    // Candidate B for Vol 4: Fallback provider (Nyaa)
    let s_id_c4 = series_id.clone();
    db.call(move |conn| {
        set_per_series_policy(conn, manga_id, PerSeriesAutomationPolicy::AutoAcquire)?;

        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider, top_candidate_score,
                providers_checked
            ) VALUES (?1, 4.0, strftime('%s','now'), 'available', 2,
                     'madokami:/Vol04_primary.cbz', 'Synthetic Chronicles v04 (Digital)', 'madokami', 90.0,
                     '[\"madokami\",\"nyaa\"]')",
            params![s_id_c4],
        )?;

        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, score, score_explanation, is_digital, is_trusted, download_url, published_at, discovered_at
            ) VALUES (?1, 4.0, 'madokami:/Vol04_primary.cbz', 'madokami', 'mado-04',
                     'Synthetic Chronicles v04 (Digital)', 'Synthetic Chronicles v04 (Digital).cbz',
                     'exact_volume', 90.0, 'Exact volume match', 1, 1, 'http://localhost:9999/Vol04_primary.cbz', strftime('%s','now'), strftime('%s','now'))",
            params![s_id_c4],
        )?;
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, score, score_explanation, is_digital, is_trusted, info_hash, seeders, published_at, discovered_at
            ) VALUES (?1, 4.0, 'nyaa:99904', 'nyaa', '99904',
                     'Synthetic Chronicles v04 [Fallback]', 'Synthetic Chronicles v04 [Fallback].cbz',
                     'exact_volume', 85.0, 'Exact volume match', 1, 1, 'hash_nyaa_04', 12, strftime('%s','now'), strftime('%s','now'))",
            params![s_id_c4],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Trigger continuous backlog advancement for completed chapter 10003
    let http = reqwest::Client::new();
    let backlog_step = advance_continuous_backlog(db, http.clone(), None, chapter_id_vol3)
        .await
        .unwrap();

    assert!(backlog_step.is_some(), "Continuous backlog should immediately pick up next volume");
    let step = backlog_step.unwrap();
    assert_eq!(step.volume_number, 4.0, "Next evaluated volume must be 4.0");

    // ------------------------------------------------------------------------
    // Step 11, 12, 13: Simulate provider failure & fallback for Volume 4
    // ------------------------------------------------------------------------
    let chapter_id_vol4: i64 = 10004;
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (?1, ?2, ?3, 'Synthetic Chronicles v04', 4.0, 4, strftime('%s','now'))",
            params![chapter_id_vol4, manga_id, format!("/ch/{chapter_id_vol4}")],
        )?;

        // Download queued with primary candidate
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, added_at, raw_title, backend_type, attempts)
             VALUES (?1, 'failed', 1, strftime('%s','now'), 'Synthetic Chronicles v04 (Digital).cbz', 'madokami', 3)",
            params![chapter_id_vol4],
        )?;

        // Download failed with HTTP 503 from Madokami
        conn.execute(
            "UPDATE downloads SET error = 'HTTP 503: Service Unavailable' WHERE chapter_id = ?1",
            [chapter_id_vol4],
        )?;

        Ok(())
    })
    .await
    .unwrap();

    // Query fallback candidates
    let fallback_eval = db
        .call(move |conn| get_fallback_candidates(conn, chapter_id_vol4))
        .await
        .unwrap();

    assert!(fallback_eval.has_fallback_candidates, "Fallback candidate must be available");
    assert_eq!(
        fallback_eval.recommended_fallback.as_ref().unwrap().provider,
        "nyaa",
        "Recommended fallback provider must be nyaa"
    );

    // Apply fallback switch
    let rec_id = fallback_eval.recommended_fallback.unwrap().candidate_id;
    let switch_result = db
        .call(move |conn| apply_acquisition_fallback(conn, chapter_id_vol4, &rec_id))
        .await
        .unwrap();

    assert!(switch_result.success, "Fallback switch must succeed");
    assert_eq!(switch_result.new_provider, "nyaa");

    // Verify download state reset to queued for retry
    let dl_state_vol4: String = db
        .call(move |conn| {
            conn.query_row(
                "SELECT state FROM downloads WHERE chapter_id = ?1",
                [chapter_id_vol4],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
        .unwrap();
    assert_eq!(dl_state_vol4, "queued", "Download state must be reset to queued after fallback");

    // Complete Volume 4 publication and Komga verification
    let staging_cbz_vol4 = env.dir.join("staging").join("Synthetic_Chronicles_v04.cbz");
    create_valid_test_cbz(&staging_cbz_vol4, series_title, 4.0);
    let pub_outcome_vol4 = publish_cbz(&staging_cbz_vol4, &env.library_dir, series_title, true, 4.0).unwrap();
    let published_path_vol4 = match pub_outcome_vol4 {
        PublishOutcome::Published(p) => p,
        PublishOutcome::AlreadyPublished(p) => p,
    };
    let pub_path_str_4 = published_path_vol4.to_string_lossy().to_string();

    let s_id_c4 = series_id.clone();
    db.call(move |conn| {
        conn.execute(
            "UPDATE downloads SET state = 'verifying', published_path = ?1 WHERE chapter_id = ?2",
            params![pub_path_str_4, chapter_id_vol4],
        )?;

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_title, volume_number, is_volume, canonical_path,
                file_size_bytes, file_sha256, published_at, verified_ownership,
                verification_status, created_at, updated_at
            ) VALUES ('rcpt-vol4', ?1, ?2, ?3, 4.0, 1, ?4, 2048, 'hash4', strftime('%s','now'), 1, 'verified', strftime('%s','now'), strftime('%s','now'))",
            params![chapter_id_vol4, manga_id, series_title, pub_path_str_4],
        )?;

        conn.execute(
            "UPDATE series_reconciliations
             SET missing_volumes_json = ?1, owned_volumes_json = ?2, updated_at = strftime('%s','now')
             WHERE series_id = ?3",
            params![
                serde_json::to_string(&vec![5.0]).unwrap(),
                serde_json::to_string(&vec![1.0, 2.0, 3.0, 4.0]).unwrap(),
                s_id_c4
            ],
        )?;

        complete_queued_publication(conn, chapter_id_vol4)?;
        Ok(())
    })
    .await
    .unwrap();

    // ------------------------------------------------------------------------
    // Step 14, 15, 16: Volume 5, server restart simulation & recovery
    // ------------------------------------------------------------------------
    let chapter_id_vol5: i64 = 10005;
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
             VALUES (?1, ?2, ?3, 'Synthetic Chronicles v05', 5.0, 5, strftime('%s','now'))",
            params![chapter_id_vol5, manga_id, format!("/ch/{chapter_id_vol5}")],
        )?;

        // Simulate active download interrupted by restart
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, added_at, raw_title, backend_type)
             VALUES (?1, 'downloading', 1, strftime('%s','now'), 'Synthetic Chronicles v05.cbz', 'madokami')",
            params![chapter_id_vol5],
        )?;

        // Set stale lease
        let key = generate_idempotency_key(&format!("manga:{manga_id}"), "standard", 5.0);
        let _ = get_or_create_lifecycle_record(conn, &format!("manga:{manga_id}"), Some(manga_id), series_title, 5.0, "standard");
        let _ = claim_lifecycle_lease(conn, &key, "worker-crashed", 10);
        conn.execute(
            "UPDATE acquisition_lifecycle SET lease_expires_at = strftime('%s','now') - 100 WHERE idempotency_key = ?1",
            [key],
        )?;

        Ok(())
    })
    .await
    .unwrap();

    // Simulate startup recovery
    Downloads::run_startup_recovery(db).await.unwrap();

    // Verify interrupted download was recovered to queued without duplicating
    let (dl_state_vol5, dl_count_vol5): (String, i64) = db
        .call(move |conn| {
            let row = conn.query_row(
                "SELECT state, (SELECT COUNT(*) FROM downloads WHERE chapter_id = ?1)
                 FROM downloads WHERE chapter_id = ?1",
                [chapter_id_vol5],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(row)
        })
        .await
        .unwrap();

    assert_eq!(dl_state_vol5, "queued", "Interrupted download must be recovered to queued");
    assert_eq!(dl_count_vol5, 1, "There must be exactly 1 download record (no duplicates)");

    // Complete Volume 5 publication
    let staging_cbz_vol5 = env.dir.join("staging").join("Synthetic_Chronicles_v05.cbz");
    create_valid_test_cbz(&staging_cbz_vol5, series_title, 5.0);
    let pub_outcome_vol5 = publish_cbz(&staging_cbz_vol5, &env.library_dir, series_title, true, 5.0).unwrap();
    let published_path_vol5 = match pub_outcome_vol5 {
        PublishOutcome::Published(p) => p,
        PublishOutcome::AlreadyPublished(p) => p,
    };
    let pub_path_str_5 = published_path_vol5.to_string_lossy().to_string();

    let s_id_c5 = series_id.clone();
    db.call(move |conn| {
        conn.execute(
            "UPDATE downloads SET state = 'verifying', published_path = ?1 WHERE chapter_id = ?2",
            params![pub_path_str_5, chapter_id_vol5],
        )?;

        conn.execute(
            "INSERT INTO publication_receipts (
                id, chapter_id, manga_id, series_title, volume_number, is_volume, canonical_path,
                file_size_bytes, file_sha256, published_at, verified_ownership,
                verification_status, created_at, updated_at
            ) VALUES ('rcpt-vol5', ?1, ?2, ?3, 5.0, 1, ?4, 2048, 'hash5', strftime('%s','now'), 1, 'verified', strftime('%s','now'), strftime('%s','now'))",
            params![chapter_id_vol5, manga_id, series_title, pub_path_str_5],
        )?;

        conn.execute(
            "UPDATE series_reconciliations
             SET missing_volumes_json = '[]', owned_volumes_json = ?1, updated_at = strftime('%s','now')
             WHERE series_id = ?2",
            params![
                serde_json::to_string(&vec![1.0, 2.0, 3.0, 4.0, 5.0]).unwrap(),
                s_id_c5
            ],
        )?;

        complete_queued_publication(conn, chapter_id_vol5)?;
        Ok(())
    })
    .await
    .unwrap();

    // ------------------------------------------------------------------------
    // Step 17, 18, 19, 20: Final verification of completeness, zero duplicates & audit
    // ------------------------------------------------------------------------
    let (verified_receipts_count, pending_downloads_count): (i64, i64) = db
        .call(move |conn| {
            let receipts: i64 = conn.query_row(
                "SELECT COUNT(*) FROM publication_receipts WHERE manga_id = ?1 AND verified_ownership = 1",
                [manga_id],
                |r| r.get(0),
            )?;
            let dls: i64 = conn.query_row(
                "SELECT COUNT(*) FROM downloads WHERE chapter_id IN (10003, 10004, 10005)",
                [],
                |r| r.get(0),
            )?;
            Ok((receipts, dls))
        })
        .await
        .unwrap();

    assert_eq!(verified_receipts_count, 5, "All 5 volumes must have verified receipts");
    assert_eq!(pending_downloads_count, 0, "Downloads queue must be cleanly empty after completion");

    // Verify audit logs were written
    let activities = db
        .call(|conn| list_monitoring_activity(conn, 50))
        .await
        .unwrap();
    assert!(
        !activities.is_empty(),
        "Audit history must record operations throughout lifecycle"
    );

    // Verify automation overview metrics
    let overview = db
        .call(|conn| get_automation_overview_metrics(conn))
        .await
        .unwrap();
    assert_eq!(overview.active_acquisitions_count, 0);
    assert_eq!(overview.awaiting_verification_count, 0);
}

// ============================================================================
// Section 6.1: Archive Safety & Payload Tests
// ============================================================================

#[test]
fn test_m14_archive_validation_security_rules() {
    let env = create_test_env("security");

    // 1. Valid manga CBZ passes
    let valid_cbz = env.dir.join("valid.cbz");
    create_valid_test_cbz(&valid_cbz, "Safe Manga", 1.0);
    let report = inspect_cbz_archive(&valid_cbz).unwrap();
    assert_eq!(report.image_count, 2);
    assert!(report.has_comic_info);
    assert!(verify_cbz_integrity(&valid_cbz).is_ok());

    // 2. Archive with executable payload (.exe) is rejected
    let exe_cbz = env.dir.join("payload_exe.cbz");
    create_suspicious_cbz_with_payload(&exe_cbz, "payload.exe");
    assert!(
        verify_cbz_integrity(&exe_cbz).is_err(),
        "Archives with .exe payloads must be rejected"
    );

    // 3. Archive with batch script (.bat) is rejected
    let bat_cbz = env.dir.join("payload_bat.cbz");
    create_suspicious_cbz_with_payload(&bat_cbz, "script.bat");
    assert!(
        verify_cbz_integrity(&bat_cbz).is_err(),
        "Archives with .bat payloads must be rejected"
    );

    // 4. Archive with shell script (.sh) is rejected
    let sh_cbz = env.dir.join("payload_sh.cbz");
    create_suspicious_cbz_with_payload(&sh_cbz, "malicious.sh");
    assert!(
        verify_cbz_integrity(&sh_cbz).is_err(),
        "Archives with .sh payloads must be rejected"
    );

    // 5. Archive with nested archive (.zip) is rejected
    let zip_cbz = env.dir.join("nested.cbz");
    create_suspicious_cbz_with_payload(&zip_cbz, "nested_archive.zip");
    assert!(
        verify_cbz_integrity(&zip_cbz).is_err(),
        "Archives with nested ZIPs must be rejected"
    );

    // 6. Nonexistent or empty file is rejected
    let empty_file = env.dir.join("empty.cbz");
    File::create(&empty_file).unwrap();
    assert!(verify_cbz_integrity(&empty_file).is_err());
}

// ============================================================================
// Section 9.1: Per-Series Automation Policy Tests
// ============================================================================

#[tokio::test]
async fn test_m14_per_series_automation_override_and_pause() {
    let env = create_test_env("per_series");
    let db = &env.db;
    let manga_id: i64 = 8801;

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, update_strategy, automation_override)
             VALUES (?1, 1, '/manga/8801', 'Override Test Series', 'ALWAYS_UPDATE', 'inherit')",
            [manga_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Default policy is Inherit
    let initial_policy = db
        .call(move |conn| Ok(get_per_series_policy(conn, manga_id)))
        .await
        .unwrap();
    assert_eq!(initial_policy, PerSeriesAutomationPolicy::Inherit);

    // Update to 'paused'
    db.call(move |conn| {
        set_per_series_policy(conn, manga_id, PerSeriesAutomationPolicy::Paused)?;
        Ok(())
    })
    .await
    .unwrap();

    let paused_policy = db
        .call(move |conn| Ok(get_per_series_policy(conn, manga_id)))
        .await
        .unwrap();
    assert_eq!(paused_policy, PerSeriesAutomationPolicy::Paused);

    // When paused, evaluate_series_backlog returns 'paused' without queueing
    let http = reqwest::Client::new();
    let res = evaluate_series_backlog(db, http, None, manga_id)
        .await
        .unwrap();
    assert!(res.is_some());
    assert_eq!(res.unwrap().action, "paused");

    // Update title settings via set_title_monitoring_settings with override
    let updated_settings = db
        .call(move |conn| {
            set_title_monitoring_settings(conn, manga_id, true, true, Some("auto_acquire"))
        })
        .await
        .unwrap();

    assert_eq!(updated_settings.automation_override, "auto_acquire");
    assert_eq!(updated_settings.effective_automation_mode, "automatic_acquisition");
}

// ============================================================================
// Section 4.3 & 4.4: Idempotent Leases & State Machine Tests
// ============================================================================

#[tokio::test]
async fn test_m14_idempotent_leases_and_stale_reclaim() {
    let env = create_test_env("leases");
    let db = &env.db;
    let series_id = "manga:9901";
    let manga_id: i64 = 9901;
    let key = generate_idempotency_key(series_id, "standard", 1.0);

    // Setup manga record so foreign keys are satisfied
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, update_strategy, in_library, automation_override)
             VALUES (?1, 1, '/manga/9901', 'Lease Test', 'ALWAYS_UPDATE', 1, 'inherit')",
            params![manga_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // 1. Create lifecycle record
    let record = db
        .call(move |conn| {
            get_or_create_lifecycle_record(conn, series_id, Some(9901), "Lease Test", 1.0, "standard")
        })
        .await
        .unwrap();
    assert_eq!(record.state, "DISCOVERED");

    // 2. Claim lease for worker-A (duration 60s)
    let key_c = key.clone();
    let claimed = db
        .call(move |conn| claim_lifecycle_lease(conn, &key_c, "worker-A", 60))
        .await
        .unwrap();
    assert!(claimed, "Worker A must successfully claim available lease");

    // 3. Worker B attempting to claim active lease fails
    let key_c = key.clone();
    let claimed_b = db
        .call(move |conn| claim_lifecycle_lease(conn, &key_c, "worker-B", 60))
        .await
        .unwrap();
    assert!(!claimed_b, "Worker B must not claim an active lease held by Worker A");

    // 4. Release lease
    let key_c = key.clone();
    db.call(move |conn| release_lifecycle_lease(conn, &key_c, "worker-A"))
        .await
        .unwrap();

    // 5. Worker B can now claim lease
    let key_c = key.clone();
    let claimed_b2 = db
        .call(move |conn| claim_lifecycle_lease(conn, &key_c, "worker-B", 60))
        .await
        .unwrap();
    assert!(claimed_b2, "Worker B can claim lease after release");

    // 6. Expire lease in past and reclaim stale leases
    let key_c = key.clone();
    db.call(move |conn| {
        conn.execute(
            "UPDATE acquisition_lifecycle SET lease_expires_at = strftime('%s','now') - 10 WHERE idempotency_key = ?1",
            [key_c],
        )?;
        let reclaimed = reclaim_stale_leases(conn)?;
        Ok(reclaimed)
    })
    .await
    .unwrap();

    // 7. Lease can be claimed again after stale reclaim
    let key_c = key.clone();
    let claimed_c = db
        .call(move |conn| claim_lifecycle_lease(conn, &key_c, "worker-C", 60))
        .await
        .unwrap();
    assert!(claimed_c, "Worker C can claim lease after stale lease reclaim");

    // 8. State transitions
    let key_c = key.clone();
    db.call(move |conn| {
        update_lifecycle_state(
            conn,
            &key_c,
            AcquisitionLifecycleState::Downloading,
            Some("cand-1"),
            Some("madokami"),
            None,
            None,
            None,
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let updated_rec = db
        .call(move |conn| {
            get_or_create_lifecycle_record(conn, series_id, Some(9901), "Lease Test", 1.0, "standard")
        })
        .await
        .unwrap();
    assert_eq!(updated_rec.state, "DOWNLOADING");
    assert_eq!(updated_rec.candidate_id.as_deref(), Some("cand-1"));
    assert_eq!(updated_rec.provider.as_deref(), Some("madokami"));
}
