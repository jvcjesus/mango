//! Milestone M10: Intelligent Release Discovery, Ranking & Acquisition Planning Tests.

use super::discovery::discover_for_volume_with_opts;
use super::fallback::{apply_acquisition_fallback, get_fallback_candidates};
use super::keiyoushi::KeiyoushiReleaseProvider;
use super::matching::{
    detect_foreign_language, evaluate_candidate_confidence, match_series_identity,
    strict_fuzzy_volume_match, CandidateConfidence,
};
use super::providers::{CandidateMatchType, ReleaseProvider, ReleaseQuery};
use super::quality::evaluate_candidate_quality_v2;
use super::volume_manager::{plan_volume_manager_batch, VolumeManagerBatchPlanRequest};
use crate::acquisition::profiles::AcquisitionProfile;
use crate::db::Db;
use crate::sync::reconciliation::{save_reconciliation, ReconciliationStatus, ReconciliationSummary};
use rusqlite::Connection;

fn setup_test_db() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    // Run all migrations to test on canonical schema
    crate::db::migrate(&mut conn).unwrap();
    conn
}

#[test]
fn test_m10_spinoff_and_subtitle_rejection() {
    let canonical = "Haikyu!!";
    let aliases = vec!["HQ!!".to_string()];

    // Normal release with quality group
    let (matched, score, reason) =
        match_series_identity("[danke-Empire] Haikyu!! - Vol. 04 [Digital]", canonical, &aliases);
    assert!(matched);
    assert!(score > 0.0);
    assert!(reason.contains("Exact"));

    // Spin-off: Haikyu-bu!!
    let (matched_spinoff, _, reason_spinoff) =
        match_series_identity("[Group] Haikyu-bu!! - Vol. 01", canonical, &aliases);
    assert!(!matched_spinoff);
    assert!(reason_spinoff.contains("Spin-off or subtitle marker"));

    // Attack on Titan vs Junior High
    let aot = "Attack on Titan";
    let (matched_jh, _, _) =
        match_series_identity("Attack on Titan: Junior High Vol. 01", aot, &[]);
    assert!(!matched_jh);
}

#[test]
fn test_m10_volume_4_vs_volume_40_strict_word_boundary() {
    // "v4" should match Volume 4
    assert!(strict_fuzzy_volume_match("[Group] Series v4 [Digital].cbz", 4));
    assert!(strict_fuzzy_volume_match("[Group] Series vol 4.cbz", 4));
    assert!(strict_fuzzy_volume_match("Series Vol. 04.cbz", 4));

    // "v40" should NOT match Volume 4
    assert!(!strict_fuzzy_volume_match("[Group] Series v40 [Digital].cbz", 4));
    assert!(!strict_fuzzy_volume_match("[Group] Series vol 40.cbz", 4));
    assert!(!strict_fuzzy_volume_match("Series Vol. 40.cbz", 4));

    // "v40" should match Volume 40
    assert!(strict_fuzzy_volume_match("[Group] Series v40 [Digital].cbz", 40));
}

#[test]
fn test_m10_foreign_language_detection() {
    assert_eq!(
        detect_foreign_language("[FR] One Piece Tome 01", "en"),
        Some("French".to_string())
    );
    assert_eq!(
        detect_foreign_language("[German] Bleach Band 01", "en"),
        Some("German".to_string())
    );
    assert_eq!(
        detect_foreign_language("[RAW] Naruto Vol 01", "en"),
        Some("Japanese RAW".to_string())
    );
    assert_eq!(
        detect_foreign_language("(esp) Berserk Vol 01", "en"),
        Some("Spanish".to_string())
    );
    // English release should be None
    assert_eq!(
        detect_foreign_language("[danke-Empire] Haikyu!! Vol 04 [Digital]", "en"),
        None
    );
}

#[test]
fn test_m10_confidence_evaluation() {
    // High confidence: exact volume, trusted, digital
    let (conf_high, _) = evaluate_candidate_confidence(
        "[danke-Empire] Haikyu!! Vol. 04 [Digital]",
        "Haikyu!!",
        &[],
        Some(4.0),
        CandidateMatchType::ExactVolume,
        true,
        true,
        "nyaa",
        Some(25),
        "en",
    );
    assert_eq!(conf_high, CandidateConfidence::High);

    // Medium confidence: volume range / omnibus
    let (conf_med, _) = evaluate_candidate_confidence(
        "Haikyu!! Vols 01-05",
        "Haikyu!!",
        &[],
        Some(4.0),
        CandidateMatchType::VolumeRangeContainingTarget,
        false,
        false,
        "nyaa",
        Some(10),
        "en",
    );
    assert_eq!(conf_med, CandidateConfidence::Medium);

    // Ambiguous confidence: foreign language
    let (conf_amb, _) = evaluate_candidate_confidence(
        "[FR] Haikyu!! Tome 04",
        "Haikyu!!",
        &[],
        Some(4.0),
        CandidateMatchType::ExactVolume,
        false,
        false,
        "nyaa",
        Some(15),
        "en",
    );
    assert_eq!(conf_amb, CandidateConfidence::Ambiguous);
}

#[test]
fn test_m10_quality_ranking_breakdown_and_explainability() {
    let cand = crate::acquisition::approval::PersistedCandidate {
        series_id: "manga:1".to_string(),
        volume_number: 4.0,
        candidate_id: "nyaa:12345".to_string(),
        provider: "nyaa".to_string(),
        provider_item_id: "12345".to_string(),
        title: "Haikyu!! Vol. 04".to_string(),
        raw_title: "[danke-Empire] Haikyu!! Vol. 04 (Digital) (1920px).cbz".to_string(),
        match_type: "exact_volume".to_string(),
        size_bytes: Some(120_000_000),
        uploader: Some("danke-Empire".to_string()),
        is_trusted: true,
        is_digital: true,
        score: 95.0,
        score_explanation: "High quality digital release".to_string(),
        download_url: None,
        magnet_url: None,
        info_hash: Some("abcd1234hash".to_string()),
        seeders: Some(40),
        leechers: Some(2),
        published_at: Some(crate::db::now()),
    };

    let eval = evaluate_candidate_quality_v2(&cand, false, None, false, None);
    assert!(eval.ranking_breakdown.is_some());

    let rb = eval.ranking_breakdown.unwrap();
    assert!(rb.final_score > 0.0);
    assert!(rb.quality_score > 0.0);
    assert!(rb.reliability_score > 0.0);
    assert!(!rb.explanation.is_empty());
    assert!(!rb.verified_evidence.is_empty());

    // Verify breakdown JSON serialization
    let json = serde_json::to_string(&rb).unwrap();
    assert!(json.contains("qualityScore"));
    assert!(json.contains("reliabilityScore"));
}

#[tokio::test]
async fn test_m10_keiyoushi_extension_release_aggregation() {
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    db.call(move |db_conn| {
        db_conn.execute("INSERT INTO extensions (pkg, name, version_name, version_code, lib, content_warning, installed_at, updated_at) VALUES ('pkg.test', 'Ext', '1.0', 1, '1.0', 0, 1000, 1000)", [])?;
        db_conn.execute("INSERT INTO sources (id, pkg, name, lang, supports_latest) VALUES (10, 'pkg.test', 'MangaDex', 'en', 1)", [])?;
        db_conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (1, 10, '/manga/1', 'Haikyu!!')", [])?;
        db_conn.execute(
            "INSERT INTO chapters (id, manga_id, url, name, chapter_number, date_upload, scanlator, source_order, fetched_at)
             VALUES (101, 1, '/c18', 'Volume 03 Chapter 18', 18.0, 1600000000, 'NekoScans', 1, 1600000000),
                    (102, 1, '/c19', 'Volume 03 Chapter 19', 19.0, 1600000010, 'NekoScans', 2, 1600000010),
                    (103, 1, '/c20', 'Volume 03 Chapter 20', 20.0, 1600000020, 'NekoScans', 3, 1600000020)",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    let provider = KeiyoushiReleaseProvider::new(db, true);
    let query = ReleaseQuery {
        series_title: "Haikyu!!".to_string(),
        target_volume: Some(3.0),
        target_chapter: None,
        preferred_edition: None,
    };

    let candidates = provider.search_releases(&query).await.unwrap();
    assert_eq!(candidates.len(), 1);
    let cand = &candidates[0];
    assert_eq!(cand.provider, "keiyoushi");
    assert!(cand.title.contains("Volume 03 (Ch. 18-20)"));
    assert_eq!(cand.target_volume, Some(3.0));
}

#[test]
fn test_m10_acquisition_fallback_switching_and_audit() {
    let conn = setup_test_db();

    // 1. Setup test manga, chapter and failed download
    conn.execute("INSERT INTO extensions (pkg, name, version_name, version_code, lib, content_warning, installed_at, updated_at) VALUES ('pkg.test', 'Ext', '1.0', 1, '1.0', 0, 1000, 1000)", []).unwrap();
    conn.execute("INSERT INTO sources (id, pkg, name, lang, supports_latest) VALUES (1, 'pkg.test', 'Test', 'en', 1)", []).unwrap();
    conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (1, 1, '/h', 'Haikyu!!')", []).unwrap();
    conn.execute(
        "INSERT INTO chapters (id, manga_id, url, name, chapter_number, source_order, fetched_at)
         VALUES (40, 1, '/h/v4', 'Volume 04', 4.0, 1, 1000)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO downloads (chapter_id, position, state, raw_title, error, attempts, added_at)
         VALUES (40, 0, 'failed', '[Madokami] Haikyu!! - v04.cbz', 'HTTP 404 Not Found from Madokami server', 3, 1000)",
        [],
    ).unwrap();

    // 2. Insert candidate pool:
    // - Failed Madokami candidate
    // - Compatible Nyaa candidate (seeders: 12)
    // - Dead torrent (seeders: 0) -> should be excluded
    // - Foreign torrent ([FR]) -> should be excluded
    conn.execute(
        "INSERT INTO discovered_release_candidates (
            series_id, volume_number, candidate_id, provider, provider_item_id,
            title, raw_title, match_type, size_bytes, score, score_explanation,
            seeders, published_at, discovered_at, confidence
        ) VALUES
        ('manga:1', 4.0, 'madokami:h4', 'madokami', 'h4', 'Haikyu!! Vol 4', '[Madokami] Haikyu!! - v04.cbz', 'exact_volume', 80000000, 95.0, 'Madokami', NULL, 1000, 1000, 'high'),
        ('manga:1', 4.0, 'nyaa:999', 'nyaa', '999', 'Haikyu!! Vol 4 Digital', '[danke-Empire] Haikyu!! v04 (Digital).cbz', 'exact_volume', 90000000, 90.0, 'Nyaa Digital', 12, 1000, 1000, 'high'),
        ('manga:1', 4.0, 'nyaa:dead', 'nyaa', 'dead', 'Haikyu!! Vol 4 Dead', 'Haikyu!! v04.cbz', 'exact_volume', 90000000, 50.0, 'Dead', 0, 1000, 1000, 'low'),
        ('manga:1', 4.0, 'nyaa:fr', 'nyaa', 'fr', 'Haikyu!! Tome 4', '[FR] Haikyu!! Tome 04.cbz', 'exact_volume', 90000000, 85.0, 'French', 20, 1000, 1000, 'ambiguous')",
        [],
    ).unwrap();

    // 3. Evaluate fallback candidates
    let eval = get_fallback_candidates(&conn, 40).unwrap();
    assert!(eval.has_fallback_candidates);
    assert_eq!(eval.fallback_candidates.len(), 1);
    assert_eq!(eval.fallback_candidates[0].candidate_id, "nyaa:999");
    assert_eq!(eval.recommended_fallback.as_ref().unwrap().candidate_id, "nyaa:999");

    // 4. Switch to fallback candidate
    let switch_res = apply_acquisition_fallback(&conn, 40, "nyaa:999").unwrap();
    assert!(switch_res.success);
    assert_eq!(switch_res.new_provider, "nyaa");
    assert_eq!(switch_res.new_candidate_id, "nyaa:999");

    // 5. Verify download row was reset to queued and error cleared
    let (dl_state, dl_error, raw_t): (String, Option<String>, String) = conn.query_row(
        "SELECT state, error, raw_title FROM downloads WHERE chapter_id = 40",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(dl_state, "queued");
    assert!(dl_error.is_none());
    assert_eq!(raw_t, "[danke-Empire] Haikyu!! v04 (Digital).cbz");

    // 6. Verify audit entry in acquisition_fallback_history
    let (_hist_failed_cand, hist_fb_cand, hist_status): (String, String, String) = conn.query_row(
        "SELECT failed_candidate_id, fallback_candidate_id, status FROM acquisition_fallback_history WHERE chapter_id = 40",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(hist_fb_cand, "nyaa:999");
    assert_eq!(hist_status, "switched");
}

#[tokio::test]
async fn test_m10_discovery_ttl_caching_and_force_refresh() {
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    let current_time = crate::db::now();

    // Setup cached discovery result (age: 100 seconds, well within 3600s TTL)
    db.call(move |conn| {
        conn.execute("INSERT INTO extensions (pkg, name, version_name, version_code, lib, content_warning, installed_at, updated_at) VALUES ('pkg.test', 'Ext', '1.0', 1, '1.0', 0, 1000, 1000)", [])?;
        conn.execute("INSERT INTO sources (id, pkg, name, lang, supports_latest) VALUES (1, 'pkg.test', 'Test', 'en', 1)", [])?;
        conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (1, 1, '/h', 'Haikyu!!')", [])?;

        let rec = ReconciliationSummary {
            series_id: "manga:1".to_string(),
            manga_id: Some(1),
            komga_series_id: None,
            title: "Haikyu!!".to_string(),
            mangabaka_id: Some(100),
            collection_id: Some("col-100".to_string()),
            edition: "Official Digital".to_string(),
            status: ReconciliationStatus::MissingVolume,
            is_ongoing: false,
            total_official_volumes: Some(45),
            owned_volumes_count: 2,
            missing_volumes_count: 2,
            owned_volumes: vec![1.0, 2.0],
            missing_volumes: vec![3.0, 4.0],
            future_volumes: vec![],
            last_reconciled_at: current_time,
            last_error: None,
            review_reason: None,
            display_status: "Missing volumes".to_string(),
        };
        save_reconciliation(conn, &rec)?;

        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider,
                top_candidate_score, providers_checked, provider_errors
            ) VALUES ('manga:1', 4.0, ?1, 'found', 1, 'cached:c1', 'Cached Release Vol 4', 'madokami', 95.0, '[\"madokami\"]', '{}')",
            [current_time - 100],
        )?;

        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, score, score_explanation, published_at, discovered_at, confidence
            ) VALUES ('manga:1', 4.0, 'cached:c1', 'madokami', 'item1', 'Cached Release Vol 4', 'Cached Release Vol 4.cbz', 'exact_volume', 95.0, 'Cached', 1000, ?1, 'high')",
            [current_time - 100],
        )?;

        Ok(())
    }).await.unwrap();

    let client = reqwest::Client::new();

    // 1. Without forceRefresh, returns cached result
    let cached_res = discover_for_volume_with_opts(&db, client.clone(), "manga:1", 4.0, false)
        .await
        .unwrap();
    assert_eq!(cached_res.status, "found");
    assert_eq!(cached_res.candidate_count, 1);
    assert_eq!(cached_res.candidates[0].candidate_id, "cached:c1");
    assert_eq!(cached_res.candidates[0].confidence.as_deref(), Some("high"));
}

#[test]
fn test_m10_batch_plan_enriched_metadata_and_alternatives() {
    let conn = setup_test_db();
    let current_time = crate::db::now();

    // 1. Setup series and reconciliation
    conn.execute("INSERT INTO extensions (pkg, name, version_name, version_code, lib, content_warning, installed_at, updated_at) VALUES ('pkg.test', 'Ext', '1.0', 1, '1.0', 0, 1000, 1000)", []).unwrap();
    conn.execute("INSERT INTO sources (id, pkg, name, lang, supports_latest) VALUES (1, 'pkg.test', 'Test', 'en', 1)", []).unwrap();
    conn.execute("INSERT INTO manga (id, source_id, url, title) VALUES (1, 1, '/h', 'Haikyu!!')", []).unwrap();

    let rec = ReconciliationSummary {
        series_id: "manga:1".to_string(),
        manga_id: Some(1),
        komga_series_id: None,
        title: "Haikyu!!".to_string(),
        mangabaka_id: Some(100),
        collection_id: Some("col-100".to_string()),
        edition: "Official Digital".to_string(),
        status: ReconciliationStatus::MissingVolume,
        is_ongoing: false,
        total_official_volumes: Some(45),
        owned_volumes_count: 2,
        missing_volumes_count: 2,
        owned_volumes: vec![1.0, 2.0],
        missing_volumes: vec![3.0, 4.0],
        future_volumes: vec![],
        last_reconciled_at: current_time,
        last_error: None,
        review_reason: None,
        display_status: "Missing volumes".to_string(),
    };
    save_reconciliation(&conn, &rec).unwrap();

    // 2. Insert discovered candidates: 1 top candidate and 1 alternative candidate
    conn.execute(
        "INSERT INTO discovered_release_candidates (
            series_id, volume_number, candidate_id, provider, provider_item_id,
            title, raw_title, match_type, score, score_explanation, published_at, discovered_at, confidence
        ) VALUES
        ('manga:1', 4.0, 'nyaa:top', 'nyaa', '101', 'Haikyu!! Vol. 04 (Digital)', '[danke-Empire] Haikyu!! Vol. 04.cbz', 'exact_volume', 98.0, 'Top candidate', 1000, 1000, 'high'),
        ('manga:1', 4.0, 'nyaa:alt', 'nyaa', '102', 'Haikyu!! Vol. 04 (Alt Rip)', 'Haikyu!! Vol. 04 Alt.cbz', 'exact_volume', 82.0, 'Alt candidate', 1000, 1000, 'medium')",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO volume_discovery_status (
            series_id, volume_number, last_discovered_at, status, candidate_count,
            top_candidate_id, top_candidate_title, top_candidate_provider,
            top_candidate_score, providers_checked, provider_errors
        ) VALUES ('manga:1', 4.0, ?1, 'found', 2, 'nyaa:top', 'Haikyu!! Vol. 04 (Digital)', 'nyaa', 98.0, '[\"nyaa\"]', '{}')",
        [current_time],
    ).unwrap();

    let req = VolumeManagerBatchPlanRequest {
        selected_volumes: vec![4.0],
        profile_id: None,
        is_dry_run: Some(true),
    };

    let profile = AcquisitionProfile::default();
    let res = plan_volume_manager_batch(&conn, 1, &req, &profile, None).unwrap();

    assert_eq!(res.items.len(), 1);
    let item = &res.items[0];
    assert_eq!(item.volume_number, 4.0);
    assert!(item.is_eligible);
    assert_eq!(item.recommended_release.as_deref(), Some("Haikyu!! Vol. 04 (Digital)"));
    assert_eq!(item.confidence.as_deref(), Some("high"));
    assert_eq!(item.publication_destination.as_deref(), Some("Haikyu!!/Volume 04.cbz"));
    assert_eq!(item.alternative_releases.len(), 1);
    assert!(item.alternative_releases[0].contains("Haikyu!! Vol. 04 (Alt Rip)"));
}
