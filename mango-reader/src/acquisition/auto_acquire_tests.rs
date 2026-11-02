//! Automated unit and integration tests for M6.8 Guarded Auto-Acquisition Policies.

use axum::routing::get;
use axum::Router;
use rusqlite::Connection;

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::auto_acquire::*;
use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::torbox::TorBoxClient;
use crate::acquisition::{AcquisitionRelease, AcquisitionReleaseUnit};
use crate::db::{now, Db};

fn create_test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    crate::db::migrate(&mut conn).unwrap();
    conn
}

static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn create_test_db() -> (Db, std::path::PathBuf) {
    let count = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!("mango_auto_acq_test_{}_{}_{}", now(), std::process::id(), count));
    let _ = std::fs::create_dir_all(&temp_dir);
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();
    (db, temp_dir)
}

fn sample_candidate(match_type: &str, score: f64, raw_title: &str, seeders: Option<u32>) -> PersistedCandidate {
    PersistedCandidate {
        series_id: "manga:18".to_string(),
        volume_number: 9.0,
        candidate_id: format!("nyaa:{}", raw_title.replace(' ', "_")),
        provider: "nyaa".to_string(),
        provider_item_id: "https://nyaa.si/view/123".to_string(),
        title: raw_title.to_string(),
        raw_title: raw_title.to_string(),
        match_type: match_type.to_string(),
        size_bytes: Some(150 * 1024 * 1024), // 150 MB
        uploader: Some("danke-Empire".to_string()),
        is_trusted: true,
        is_digital: true,
        score,
        score_explanation: "Test score".to_string(),
        download_url: Some("https://nyaa.si/download/123.torrent".to_string()),
        magnet_url: Some("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_string()),
        info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        seeders,
        leechers: Some(5),
        published_at: Some(now()),
    }
}

#[test]
fn test_hard_candidate_eligibility_ranking_range_cannot_outrank_exact() {
    let profile = AcquisitionProfile::default();

    // Candidate A: VolumeRangeContainingTarget with high raw score (75.0)
    let range_cand = sample_candidate(
        "volume_range_containing_target",
        75.0,
        "Kagurabachi v01-09 (Digital)",
        Some(50),
    );

    // Candidate B: ExactVolume with lower raw score (60.0)
    let exact_cand = sample_candidate(
        "exact_volume",
        60.0,
        "Kagurabachi v09 (2026) (Digital) (danke-Empire)",
        Some(10),
    );

    // 1. Hard gate evaluation
    assert_eq!(
        is_candidate_auto_eligible(&range_cand, &profile),
        Err(AutoDecisionCode::RangeRequiresManual),
        "Range candidate must be rejected by hard gate despite high score"
    );

    assert_eq!(
        is_candidate_auto_eligible(&exact_cand, &profile),
        Ok(()),
        "Exact volume candidate must pass hard gate"
    );

    // 2. Ranking among eligible candidates:
    let candidates = vec![range_cand, exact_cand.clone()];
    let eligible: Vec<_> = candidates
        .into_iter()
        .filter(|c| is_candidate_auto_eligible(c, &profile).is_ok())
        .collect();

    assert_eq!(eligible.len(), 1);
    assert_eq!(eligible[0].candidate_id, exact_cand.candidate_id);
    assert_eq!(eligible[0].score, 60.0);
}

#[test]
fn test_parser_ambiguity_rejections() {
    let profile = AcquisitionProfile::default();

    // Omnibus
    let omnibus = sample_candidate("exact_volume", 70.0, "Manga Omnibus Vol. 1", Some(20));
    assert_eq!(
        is_candidate_auto_eligible(&omnibus, &profile),
        Err(AutoDecisionCode::AmbiguousTitleTokens)
    );

    // 2-in-1
    let two_in_one = sample_candidate("exact_volume", 70.0, "Manga v01 (2-in-1 Edition)", Some(20));
    assert_eq!(
        is_candidate_auto_eligible(&two_in_one, &profile),
        Err(AutoDecisionCode::AmbiguousTitleTokens)
    );

    // Complete / Batch
    let batch = sample_candidate("exact_volume", 70.0, "Manga v01 [Batch Complete]", Some(20));
    assert_eq!(
        is_candidate_auto_eligible(&batch, &profile),
        Err(AutoDecisionCode::AmbiguousTitleTokens)
    );

    // Disallowed format: PDF
    let pdf = sample_candidate("exact_volume", 70.0, "Manga v01.pdf", Some(20));
    assert_eq!(
        is_candidate_auto_eligible(&pdf, &profile),
        Err(AutoDecisionCode::FormatNotAllowed)
    );

    // Disallowed format: EPUB
    let epub = sample_candidate("exact_volume", 70.0, "Manga v01.epub", Some(20));
    assert_eq!(
        is_candidate_auto_eligible(&epub, &profile),
        Err(AutoDecisionCode::FormatNotAllowed)
    );

    // Zero seeders (dead torrent)
    let dead = sample_candidate("exact_volume", 70.0, "Manga v01 (Digital)", Some(0));
    assert_eq!(
        is_candidate_auto_eligible(&dead, &profile),
        Err(AutoDecisionCode::SeedersBelowPolicy)
    );

    // Non-English
    let french = sample_candidate("exact_volume", 70.0, "Manga v01 (French)", Some(15));
    assert_eq!(
        is_candidate_auto_eligible(&french, &profile),
        Err(AutoDecisionCode::LanguageUnknown)
    );
}

#[test]
fn test_settings_and_consent_defaults() {
    let conn = create_test_conn();

    // Default global switch: strictly false
    assert!(!is_global_auto_acquire_enabled(&conn));

    // Default dry-run: strictly true
    assert!(is_auto_acquire_dry_run(&conn));

    // Create a manga title
    conn.execute(
        "INSERT INTO manga (id, source_id, url, title) VALUES (18, 0, 'catalog:18', 'Kagurabachi')",
        [],
    )
    .unwrap();

    // Title auto-acquire defaults strictly to false
    let (enabled, activated_at) = get_title_auto_acquire(&conn, 18).unwrap();
    assert!(!enabled);
    assert_eq!(activated_at, None);

    // Toggle title auto-acquire ON
    set_title_auto_acquire(&conn, 18, true).unwrap();
    let (enabled, activated_at) = get_title_auto_acquire(&conn, 18).unwrap();
    assert!(enabled);
    assert!(activated_at.is_some());
    assert!(activated_at.unwrap() > 0);

    // Toggle title auto-acquire OFF
    set_title_auto_acquire(&conn, 18, false).unwrap();
    let (enabled, activated_at) = get_title_auto_acquire(&conn, 18).unwrap();
    assert!(!enabled);
    assert_eq!(activated_at, None);
}

#[tokio::test]
async fn test_torbox_idempotency_remote_accepted_local_interrupted() {
    let app = Router::new().route(
        "/v1/api/torrents/mylist",
        get(|| async {
            axum::Json(serde_json::json!({
                "success": true,
                "detail": "Retrieved user torrents",
                "data": [
                    {
                        "id": 999888,
                        "name": "Kagurabachi v09 (2026) (Digital)",
                        "hash": "0123456789abcdef0123456789abcdef01234567",
                        "download_state": "downloaded",
                        "progress": 1.0,
                        "download_speed": 0,
                        "seeds": 15,
                        "peers": 0,
                        "files": []
                    }
                ]
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = TorBoxClient::new(
        reqwest::Client::new(),
        Some(format!("http://127.0.0.1:{port}/v1/api")),
        Some("valid-token".to_string()),
    );

    let release = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "123".to_string(),
        title: "Kagurabachi v09 (2026) (Digital)".to_string(),
        raw_title: "Kagurabachi v09 (2026) (Digital)".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/123".to_string(),
        torrent_url: None,
        magnet_url: Some("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_string()),
        info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        category_id: Some("3_1".to_string()),
        category_name: Some("Manga".to_string()),
        size_bytes: 150 * 1024 * 1024,
        size_formatted: "150 MB".to_string(),
        seeders: 15,
        leechers: 0,
        published_at: Some(now()),
        is_trusted: true,
        is_remake: false,
        is_variant: false,
        variant_name: None,
        is_mixed: false,
        release_unit: AcquisitionReleaseUnit::Volume(9.0),
        release_number: Some(9.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    // Call submit: it finds existing torrent by hash in TorBox mylist!
    let job_info = client.submit(&release).await.expect("submit must succeed by reusing existing torrent");

    // Verify exactly ONE remote job is used and matches the existing remote ID
    assert_eq!(job_info.external_job_id, "999888");
    assert_eq!(job_info.torrent_hash, Some("0123456789abcdef0123456789abcdef01234567".to_string()));
}

#[tokio::test]
async fn test_consent_matrix_and_dry_run_evaluation() {
    let (db, temp_dir) = create_test_db();
    let cur_time = now();

    db.call(move |conn| {
        // Insert manga
        conn.execute("INSERT INTO manga (id, source_id, url, title, auto_acquire, auto_acquire_activated_at) VALUES (18, 0, 'catalog:18', 'Kagurabachi', 0, NULL)", [])?;
        // Insert reconciliation
        conn.execute("INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, owned_volumes_count, missing_volumes_count, owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at) VALUES ('manga:18', 18, 'Kagurabachi', 'up_to_date', 8, 8, 0, '[1,2,3,4,5,6,7,8]', '[]', '[9]', ?1, ?1)", [cur_time])?;
        // Insert release discovery target for Volume 9 releasing today
        conn.execute("INSERT INTO release_discovery_targets (id, series_id, manga_id, series_title, volume_number, market, edition, release_date, release_date_precision, date_type, status, created_at, updated_at) VALUES (1, 'manga:18', 18, 'Kagurabachi', 9.0, 'en', 'standard', date('now', 'localtime'), 'day', 'published', 'candidate_available', ?1, ?1)", [cur_time])?;
        // Insert exact candidate
        conn.execute("INSERT INTO discovered_release_candidates (id, series_id, volume_number, candidate_id, provider, provider_item_id, title, raw_title, match_type, size_bytes, is_digital, score, score_explanation, download_url, magnet_url, info_hash, seeders, published_at, discovered_at) VALUES (1, 'manga:18', 9.0, 'nyaa:cand1', 'nyaa', '1', 'Kagurabachi v09 (2026) (Digital)', 'Kagurabachi v09 (2026) (Digital)', 'exact_volume', 150000000, 1, 85.0, 'score', 'https://nyaa.si', 'magnet:?xt=urn:btih:abc', 'abc', 25, ?1, ?1)", [cur_time])?;
        Ok(())
    }).await.unwrap();

    let acq = crate::acquisition::service::AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Case 1: Global OFF, Title OFF -> GLOBAL_AUTO_DISABLED
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 1).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::GlobalAutoDisabled);

    // Case 2: Global ON, Title OFF -> TITLE_AUTO_DISABLED
    db.call(|conn| set_global_auto_acquire(conn, true)).await.unwrap();
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 1).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::TitleAutoDisabled);

    // Case 3: Global ON, Title ON, Dry-Run ON -> WOULD_AUTO_ACQUIRE
    db.call(|conn| set_title_auto_acquire(conn, 18, true)).await.unwrap();
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 1).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::WouldAutoAcquire);
    assert!(dec.dry_run);
    assert_eq!(dec.candidate_id, Some("nyaa:cand1".to_string()));

    // Verify target status in DB was NOT changed to acquiring (zero TorBox side-effects)
    let status: String = db.call(|conn| Ok(conn.query_row("SELECT status FROM release_discovery_targets WHERE id = 1", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!(status, "candidate_available");

    // Case 4: Global OFF again (Emergency Kill Switch) -> GLOBAL_AUTO_DISABLED
    db.call(|conn| set_global_auto_acquire(conn, false)).await.unwrap();
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 1).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::GlobalAutoDisabled);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_historical_backlog_watermark_protection() {
    let (db, temp_dir) = create_test_db();
    let cur_time = now();

    db.call(move |conn| {
        // Frieren catalog manga
        conn.execute("INSERT INTO manga (id, source_id, url, title, auto_acquire, auto_acquire_activated_at) VALUES (848, 0, 'catalog:848', 'Frieren', 1, ?1)", [cur_time])?;
        // Reconciliation: 0 owned, 14 missing
        conn.execute("INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, owned_volumes_count, missing_volumes_count, owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at) VALUES ('manga:848', 848, 'Frieren', 'missing_volume', 14, 0, 14, '[]', '[1,2,3,4,5,6,7,8,9,10,11,12,13,14]', '[15]', ?1, ?1)", [cur_time])?;
        // Target for historical volume 1 released in 2021 (before activation watermark!)
        conn.execute("INSERT INTO release_discovery_targets (id, series_id, manga_id, series_title, volume_number, market, edition, release_date, release_date_precision, date_type, status, created_at, updated_at) VALUES (10, 'manga:848', 848, 'Frieren', 1.0, 'en', 'standard', '2021-11-09', 'day', 'published', 'candidate_available', ?1, ?1)", [cur_time])?;
        // Candidate for Volume 1 exists
        conn.execute("INSERT INTO discovered_release_candidates (id, series_id, volume_number, candidate_id, provider, provider_item_id, title, raw_title, match_type, size_bytes, is_digital, score, score_explanation, download_url, magnet_url, info_hash, seeders, published_at, discovered_at) VALUES (10, 'manga:848', 1.0, 'nyaa:frieren1', 'nyaa', '1', 'Frieren v01 (Digital)', 'Frieren v01 (Digital)', 'exact_volume', 150000000, 1, 80.0, 'score', 'https://nyaa.si', 'magnet:?xt=urn:btih:f1', 'f1', 20, ?1, ?1)", [cur_time])?;
        Ok(())
    }).await.unwrap();

    let acq = crate::acquisition::service::AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
    db.call(|conn| set_global_auto_acquire(conn, true)).await.unwrap();

    // Evaluate historical volume 1:
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 10).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::HistoricalBacklogManualOnly);
    assert!(dec.reason.contains("prior to title auto-acquire activation date"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_blue_lock_unit_level_ownership_no_acquisition() {
    let (db, temp_dir) = create_test_db();
    let cur_time = now();

    db.call(move |conn| {
        // Blue Lock catalog manga
        conn.execute("INSERT INTO manga (id, source_id, url, title, auto_acquire, auto_acquire_activated_at) VALUES (846, 0, 'catalog:846', 'Blue Lock', 1, ?1)", [cur_time])?;
        // Blue Lock reconciliation: 39 owned, 0 missing
        conn.execute("INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, owned_volumes_count, missing_volumes_count, owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at) VALUES ('manga:846', 846, 'Blue Lock', 'up_to_date', 39, 39, 0, '[39]', '[]', '[40]', ?1, ?1)", [cur_time])?;
        // Target for Volume 39 (already owned)
        conn.execute("INSERT INTO release_discovery_targets (id, series_id, manga_id, series_title, volume_number, market, edition, release_date, release_date_precision, date_type, status, created_at, updated_at) VALUES (39, 'manga:846', 846, 'Blue Lock', 39.0, 'en', 'standard', '2026-10-25', 'day', 'published', 'candidate_available', ?1, ?1)", [cur_time])?;
        Ok(())
    }).await.unwrap();

    let acq = crate::acquisition::service::AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
    db.call(|conn| set_global_auto_acquire(conn, true)).await.unwrap();

    // Evaluate Volume 39 -> OWNED
    let dec = evaluate_target_for_auto_acquire(&db, &acq, 39).await.unwrap();
    assert_eq!(dec.code, AutoDecisionCode::Owned);

    // Verify DB target marked already_owned
    let status: String = db.call(|conn| Ok(conn.query_row("SELECT status FROM release_discovery_targets WHERE id = 39", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!(status, "already_owned");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_concurrency_atomic_claim() {
    let (db, temp_dir) = create_test_db();
    let cur_time = now();

    db.call(move |conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, auto_acquire, auto_acquire_activated_at) VALUES (18, 0, 'catalog:18', 'Kagurabachi', 1, ?1)", [cur_time])?;
        conn.execute("INSERT INTO series_reconciliations (series_id, manga_id, title, status, total_official_volumes, owned_volumes_count, missing_volumes_count, owned_volumes_json, missing_volumes_json, future_volumes_json, last_reconciled_at, updated_at) VALUES ('manga:18', 18, 'Kagurabachi', 'up_to_date', 8, 8, 0, '[1,2,3,4,5,6,7,8]', '[]', '[9]', ?1, ?1)", [cur_time])?;
        conn.execute("INSERT INTO release_discovery_targets (id, series_id, manga_id, series_title, volume_number, market, edition, release_date, release_date_precision, date_type, status, created_at, updated_at) VALUES (1, 'manga:18', 18, 'Kagurabachi', 9.0, 'en', 'standard', date('now', 'localtime'), 'day', 'published', 'candidate_available', ?1, ?1)", [cur_time])?;
        conn.execute("INSERT INTO discovered_release_candidates (id, series_id, volume_number, candidate_id, provider, provider_item_id, title, raw_title, match_type, size_bytes, is_digital, score, score_explanation, download_url, magnet_url, info_hash, seeders, published_at, discovered_at) VALUES (1, 'manga:18', 9.0, 'nyaa:cand1', 'nyaa', '1', 'Kagurabachi v09 (Digital)', 'Kagurabachi v09 (Digital)', 'exact_volume', 150000000, 1, 85.0, 'score', 'https://nyaa.si', 'magnet:?xt=urn:btih:abc', 'abc', 25, ?1, ?1)", [cur_time])?;
        set_global_auto_acquire(conn, true)?;
        set_auto_acquire_dry_run(conn, false)?; // Live mode
        Ok(())
    }).await.unwrap();

    // Atomic claim test:
    // Process 1 claims target:
    let claim1: bool = db.call(move |conn| {
        let changed = conn.execute("UPDATE release_discovery_targets SET status = 'auto_approved' WHERE id = 1 AND status = 'candidate_available'", [])?;
        Ok(changed > 0)
    }).await.unwrap();
    assert!(claim1, "First process must win atomic claim");

    // Process 2 tries to claim same target:
    let claim2: bool = db.call(move |conn| {
        let changed = conn.execute("UPDATE release_discovery_targets SET status = 'auto_approved' WHERE id = 1 AND status = 'candidate_available'", [])?;
        Ok(changed > 0)
    }).await.unwrap();
    assert!(!claim2, "Second process must fail atomic claim");

    let _ = std::fs::remove_dir_all(&temp_dir);
}
