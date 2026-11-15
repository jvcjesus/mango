//! Regression test suite for Activity recommendations and recommendation scope filtering.
//!
//! Validates:
//! 1. Mango library member with confirmed missing volumes appears in recommendations.
//! 2. Mango library member with all volumes owned does not appear.
//! 3. Mango library member with uncertain Komga mapping or unverified ownership is blocked from automatic recommendations until resolved.
//! 4. Tracked-only title (e.g. from AniList) is excluded under default Mango Library Only scope.
//! 5. Discovered release for an unmonitored title is excluded.
//! 6. Removing a series from the library hides any pending proposals without deleting history.
//! 7. Changing recommendation scope to Tracked Lists reveals eligible AniList proposals.
//! 8. Background watcher respects recommendation scope and does not generate review queue entries for ineligible series.
//! 9. Blood on the Tracks correctly detects owned volumes 1–17 in Komga and disappears from recommendations.
//! 10. Haikyu!! correctly detects volumes 1–7 owned and only recommends missing volumes (vol 8+).

#[cfg(test)]
mod tests {
    use crate::acquisition::monitoring::{
        add_or_update_review_queue_item, get_recommendation_scope, list_review_queue,
        set_recommendation_scope,
    };
    use crate::acquisition::providers::{CandidateMatchType, ReleaseCandidate};
    use crate::db::Db;
    use crate::sync::reconciliation::{
        reconcile_series, ReconciliationStatus, ReconciliationTarget,
    };
    use crate::pipeline::komga::KomgaClient;
    use crate::pipeline::mangabaka::MangaBakaClient;
    use axum::{routing::get, Json, Router};
    use rusqlite::params;
    use serde_json::json;

    fn setup_test_db() -> Db {
        Db::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn seed_manga(conn: &rusqlite::Connection, id: i64, title: &str, in_library: bool) {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, in_library) VALUES (?1, 1, ?2, ?3, ?4)",
            params![id, format!("/title/{id}"), title, in_library as i64],
        )
        .unwrap();
    }

    fn seed_tracked_catalog(
        conn: &rusqlite::Connection,
        manga_id: i64,
        title: &str,
        komga_series_id: Option<&str>,
        origins: &str,
    ) {
        conn.execute(
            "INSERT INTO tracked_catalog (manga_id, title, komga_series_id, origins, tracked, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 1700000000, 1700000000)",
            params![manga_id, title, komga_series_id, origins],
        )
        .unwrap();
    }

    fn seed_series_reconciliation(
        conn: &rusqlite::Connection,
        series_id: &str,
        manga_id: i64,
        title: &str,
        komga_series_id: Option<&str>,
        status: &str,
        owned_vols: &[f64],
        missing_vols: &[f64],
    ) {
        let owned_json = serde_json::to_string(owned_vols).unwrap();
        let missing_json = serde_json::to_string(missing_vols).unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, title, komga_series_id, status,
                owned_volumes_count, missing_volumes_count, owned_volumes_json, missing_volumes_json,
                last_reconciled_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1700000000, 1700000000)
             ON CONFLICT(series_id) DO UPDATE SET
                komga_series_id = excluded.komga_series_id,
                status = excluded.status,
                owned_volumes_count = excluded.owned_volumes_count,
                missing_volumes_count = excluded.missing_volumes_count,
                owned_volumes_json = excluded.owned_volumes_json,
                missing_volumes_json = excluded.missing_volumes_json,
                updated_at = excluded.updated_at",
            params![
                series_id,
                manga_id,
                title,
                komga_series_id,
                status,
                owned_vols.len() as i64,
                missing_vols.len() as i64,
                owned_json,
                missing_json
            ],
        )
        .unwrap();
    }

    fn create_test_candidate(id: &str, provider: &str, vol: f64, score: f64) -> ReleaseCandidate {
        ReleaseCandidate {
            candidate_id: id.to_string(),
            provider: provider.to_string(),
            provider_item_id: format!("/item/{id}"),
            title: format!("Test Release {id} Vol {vol}"),
            raw_title: format!("Test Release {id} Vol {vol}.cbz"),
            match_type: CandidateMatchType::ExactVolume,
            target_volume: Some(vol),
            target_chapter: None,
            size_bytes: Some(50 * 1024 * 1024),
            uploader: Some("Official".to_string()),
            is_trusted: true,
            is_digital: true,
            score,
            score_explanation: "Test candidate".to_string(),
            download_url: Some("http://example.com/dl.cbz".to_string()),
            magnet_url: None,
            info_hash: None,
            seeders: None,
            leechers: None,
            published_at: Some(1700000000),
            confidence: Some("high".to_string()),
            confidence_reason: Some("Exact match".to_string()),
            language: Some("en".to_string()),
            edition: Some("standard".to_string()),
            format: Some("cbz".to_string()),
            resolution: Some("1200p".to_string()),
            publisher: Some("Official".to_string()),
            ranking_breakdown: None,
        }
    }

    // 1. Mango library member with confirmed missing volumes appears in recommendations
    #[tokio::test]
    async fn test_manga_library_member_with_missing_volumes_recommended() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 10, "Frieren", true);
            seed_series_reconciliation(conn, "manga:10", 10, "Frieren", Some("komga_frieren"), "missing_volume", &[1.0], &[2.0]);
            let cand = create_test_candidate("c1", "nyaa", 2.0, 90.0);
            add_or_update_review_queue_item(conn, Some(10), "manga:10", "Frieren", 2.0, &cand, &[], None)?;
            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].series_title, "Frieren");
        assert_eq!(items[0].volume_number, 2.0);
        assert!(items[0].in_library);
        assert_eq!(items[0].ownership_status, "confirmed_missing");
        assert_eq!(items[0].eligibility_reason, "Explicit Mango Library Member");
    }

    // 2. Mango library member with all volumes owned does not appear
    #[tokio::test]
    async fn test_manga_library_member_all_owned_not_recommended() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 20, "Fullmetal Alchemist", true);
            // All 27 volumes owned
            let all_owned: Vec<f64> = (1..=27).map(|v| v as f64).collect();
            seed_series_reconciliation(conn, "manga:20", 20, "Fullmetal Alchemist", Some("komga_fma"), "up_to_date", &all_owned, &[]);
            let cand = create_test_candidate("c_fma_1", "nyaa", 1.0, 90.0);
            // Attempting to add volume 1 (which is owned) must be rejected
            let inserted = add_or_update_review_queue_item(conn, Some(20), "manga:20", "Fullmetal Alchemist", 1.0, &cand, &[], None)?;
            assert_eq!(inserted, 0, "Already-owned volume should not be inserted into review queue");
            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 0, "No proposals should exist for fully owned series");
    }

    // 3. Mango library member with uncertain Komga mapping or unverified ownership is blocked from automatic recommendations until resolved
    #[tokio::test]
    async fn test_manga_library_member_uncertain_komga_mapping_blocked() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 30, "Unmapped Series", true);
            // Unmapped to Komga: status is review_required
            seed_series_reconciliation(conn, "manga:30", 30, "Unmapped Series", None, "review_required", &[], &[]);
            
            // Manually simulate an item in queue
            conn.execute(
                "INSERT INTO acquisition_review_queue (
                    manga_id, series_id, series_title, volume_number, recommended_candidate_id,
                    provider, title, raw_title, quality_score, confidence, status, created_at, updated_at
                 ) VALUES (30, 'manga:30', 'Unmapped Series', 1.0, 'c_unmapped', 'nyaa', 'Unmapped Vol 1', 'Unmapped Vol 1.cbz', 80.0, 'high', 'pending', 1000, 1000)",
                [],
            )?;
            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ownership_status, "unmapped_to_komga");
        assert_eq!(items[0].komga_mapping_status, "ambiguous");
        assert!(!items[0].is_safe_to_approve, "Unmapped item must not be safe to approve");
    }

    // 4. Tracked-only title (e.g. from AniList) is excluded under default Mango Library Only scope
    #[tokio::test]
    async fn test_anilist_tracked_only_excluded_under_default_scope() {
        let db = setup_test_db();
        db.call(|conn| {
            // in_library = 0, origins = ["lists"]
            seed_manga(conn, 40, "Slam Dunk", false);
            seed_tracked_catalog(conn, 40, "Slam Dunk", None, "[\"lists\"]");
            seed_series_reconciliation(conn, "manga:40", 40, "Slam Dunk", None, "missing_volume", &[], &[1.0]);

            // Attempt to add review queue item under default scope: should be rejected
            let cand = create_test_candidate("c_sd_1", "madokami", 1.0, 80.0);
            let res = add_or_update_review_queue_item(conn, Some(40), "manga:40", "Slam Dunk", 1.0, &cand, &[], None)?;
            assert_eq!(res, 0, "Candidate for non-library series should be refused under mango_library_only scope");

            // Even if an entry was previously in acquisition_review_queue:
            conn.execute(
                "INSERT INTO acquisition_review_queue (
                    manga_id, series_id, series_title, volume_number, recommended_candidate_id,
                    provider, title, raw_title, quality_score, confidence, status, created_at, updated_at
                 ) VALUES (40, 'manga:40', 'Slam Dunk', 1.0, 'c_sd_1', 'madokami', 'Slam Dunk Vol 1', 'Slam Dunk Vol 1.cbz', 80.0, 'high', 'pending', 1000, 1000)",
                [],
            )?;
            Ok(())
        }).await.unwrap();

        let scope = db.call(|conn| Ok(get_recommendation_scope(conn))).await.unwrap();
        assert_eq!(scope, "mango_library_only");

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 0, "Tracked-only title must be excluded from review queue under mango_library_only");
    }

    // 5. Discovered release for an unmonitored title is excluded
    #[tokio::test]
    async fn test_nyaa_only_discovered_excluded_under_default_scope() {
        let db = setup_test_db();
        db.call(|conn| {
            // Unmonitored, non-library title (in_library = 0, not in tracked_catalog)
            seed_manga(conn, 50, "Random Unmonitored Manga", false);
            let cand = create_test_candidate("c_rand", "nyaa", 1.0, 75.0);
            let res = add_or_update_review_queue_item(conn, Some(50), "manga:50", "Random Unmonitored Manga", 1.0, &cand, &[], None)?;
            assert_eq!(res, 0, "Unmonitored title must not be added to review queue");
            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 0);
    }

    // 6. Removing a series from the library hides any pending proposals without deleting history
    #[tokio::test]
    async fn test_stale_proposals_hidden_when_series_removed_from_library() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 60, "Chainsaw Man", true);
            seed_series_reconciliation(conn, "manga:60", 60, "Chainsaw Man", Some("komga_csm"), "missing_volume", &[], &[1.0]);
            let cand = create_test_candidate("c_csm_1", "nyaa", 1.0, 95.0);
            add_or_update_review_queue_item(conn, Some(60), "manga:60", "Chainsaw Man", 1.0, &cand, &[], None)?;
            Ok(())
        }).await.unwrap();

        let before = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(before.len(), 1, "Should appear while in library");

        // Remove from library
        db.call(|conn| {
            conn.execute("UPDATE manga SET in_library = 0 WHERE id = 60", [])?;
            Ok(())
        }).await.unwrap();

        let after = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(after.len(), 0, "Removing from library should immediately hide pending proposals");

        // History in acquisition_review_queue table is preserved
        let raw_count: i64 = db.call(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM acquisition_review_queue WHERE manga_id = 60", [], |r| r.get(0))?)
        }).await.unwrap();
        assert_eq!(raw_count, 1, "Underlying proposal record should be preserved in database");
    }

    // 7. Changing recommendation scope to Tracked Lists reveals eligible AniList proposals
    #[tokio::test]
    async fn test_changing_scope_to_tracked_lists_reveals_proposals() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 70, "The Promised Neverland", false);
            seed_tracked_catalog(conn, 70, "The Promised Neverland", None, "[\"lists\"]");
            seed_series_reconciliation(conn, "manga:70", 70, "The Promised Neverland", None, "missing_volume", &[], &[1.0]);

            conn.execute(
                "INSERT INTO acquisition_review_queue (
                    manga_id, series_id, series_title, volume_number, recommended_candidate_id,
                    provider, title, raw_title, quality_score, confidence, status, created_at, updated_at
                 ) VALUES (70, 'manga:70', 'The Promised Neverland', 1.0, 'c_tpn_1', 'madokami', 'TPN Vol 1', 'TPN Vol 1.cbz', 85.0, 'high', 'pending', 1000, 1000)",
                [],
            )?;
            Ok(())
        }).await.unwrap();

        // Under default scope: hidden
        let hidden = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(hidden.len(), 0);

        // Switch to tracked_lists
        db.call(|conn| {
            set_recommendation_scope(conn, "tracked_lists")
        }).await.unwrap();

        let revealed = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(revealed.len(), 1);
        assert_eq!(revealed[0].series_title, "The Promised Neverland");
        assert_eq!(revealed[0].eligibility_reason, "Tracked List Entry");
    }

    // 8. Background watcher respects recommendation scope and does not generate review queue entries for ineligible series
    #[tokio::test]
    async fn test_auto_acquire_respects_recommendation_scope() {
        let db = setup_test_db();
        db.call(|conn| {
            // Tracked list entry not in library
            seed_manga(conn, 80, "Non-Library Title", false);
            seed_tracked_catalog(conn, 80, "Non-Library Title", None, "[\"lists\"]");
            Ok(())
        }).await.unwrap();

        let is_eligible_default = db.call(|conn| {
            let scope = get_recommendation_scope(conn);
            Ok(crate::acquisition::monitoring::is_series_eligible_for_scope(conn, Some(80), "Non-Library Title", &scope))
        }).await.unwrap();
        assert!(!is_eligible_default, "Under default mango_library_only, non-library title is ineligible");

        let is_eligible_lists = db.call(|conn| {
            Ok(crate::acquisition::monitoring::is_series_eligible_for_scope(conn, Some(80), "Non-Library Title", "tracked_lists"))
        }).await.unwrap();
        assert!(is_eligible_lists, "Under tracked_lists, tracked catalog entry is eligible");
    }

    // 9. Blood on the Tracks correctly detects owned volumes 1–17 in Komga and disappears from recommendations
    #[tokio::test]
    async fn test_blood_on_the_tracks_owned_volumes_not_recommended() {
        let db = setup_test_db();

        // Mock Komga API with all 17 volumes
        let mut books_json = Vec::new();
        for i in 1..=17 {
            books_json.push(json!({
                "id": format!("book_{i}"),
                "seriesId": "0S291CGSP41WW",
                "name": format!("Blood on the Tracks v{:02}", i),
                "number": i as f64,
                "metadata": { "number": format!("{i}") }
            }));
        }

        let app = Router::new()
            .route("/api/v1/series/0S23QDYV4J6X3/books", get(|| async {
                (axum::http::StatusCode::NOT_FOUND, "Not Found")
            }))
            .route("/api/v1/series", get(|| async {
                Json(json!({
                    "content": [{
                        "id": "0S291CGSP41WW",
                        "libraryId": "lib1",
                        "name": "Blood on the Tracks",
                        "booksCount": 17
                    }]
                }))
            }))
            .route("/api/v1/series/0S291CGSP41WW/books", get({
                let books = books_json.clone();
                move || {
                    let books = books.clone();
                    async move {
                        Json(json!({
                            "content": books,
                            "totalPages": 1,
                            "last": true
                        }))
                    }
                }
            }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Mock MangaBaka API with 17 official volumes
        let mb_app = Router::new()
            .route("/series/25283", get(|| async {
                Json(json!({ "data": { "id": 25283, "title": "Blood on the Tracks", "status": "completed" } }))
            }))
            .route("/series/25283/collections", get(|| async {
                Json(json!({
                    "data": [{
                        "id": "col_bott",
                        "language": "en",
                        "edition": "Standard Edition",
                        "medium": "digital",
                        "total_works": 17
                    }]
                }))
            }))
            .route("/collections/col_bott/works", get(|| async {
                Json(json!({
                    "data": (1..=17).map(|i| json!({
                        "id": format!("w{i}"),
                        "sequence_numeric": i as f64,
                        "release_date": "2021-01-01"
                    })).collect::<Vec<_>>()
                }))
            }));

        let mb_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mb_port = mb_listener.local_addr().unwrap().port();
        let mb_server = tokio::spawn(async move {
            axum::serve(mb_listener, mb_app).await.unwrap();
        });

        let http = reqwest::Client::new();
        let komga = KomgaClient::new(http.clone(), Some(format!("http://127.0.0.1:{port}")), Some("test_key".into()));
        let mb = MangaBakaClient::new(http.clone(), Some(format!("http://127.0.0.1:{mb_port}")), None);

        // Seed with stale Komga ID 0S23QDYV4J6X3
        db.call(|conn| {
            seed_manga(conn, 702, "Blood on the Tracks", true);
            conn.execute("UPDATE manga SET mangabaka_id = 25283 WHERE id = 702", []).unwrap();
            seed_tracked_catalog(conn, 702, "Blood on the Tracks", Some("0S23QDYV4J6X3"), "[\"lists\"]");
            Ok(())
        }).await.unwrap();

        let target = ReconciliationTarget {
            manga_id: Some(702),
            komga_series_id: Some("0S23QDYV4J6X3".to_string()),
            title: "Blood on the Tracks".to_string(),
            edition: None,
        };

        let summary = reconcile_series(&db, Some(&komga), &mb, &target).await.unwrap();

        assert_eq!(summary.status, ReconciliationStatus::UpToDate);
        assert_eq!(summary.owned_volumes_count, 17);
        assert_eq!(summary.missing_volumes_count, 0);
        assert_eq!(summary.komga_series_id.as_deref(), Some("0S291CGSP41WW"), "Should heal stale Komga series ID to 0S291CGSP41WW");

        // Stale queue item (vol 1) should not appear in pending review queue
        db.call(|conn| {
            let cand = create_test_candidate("c_bott_1", "madokami", 1.0, 90.0);
            let inserted = add_or_update_review_queue_item(conn, Some(702), "manga:702", "Blood on the Tracks", 1.0, &cand, &[], None)?;
            assert_eq!(inserted, 0, "Volume 1 must be refused because all 17 volumes are owned in Komga");
            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert!(items.iter().all(|i| i.series_title != "Blood on the Tracks"), "Blood on the Tracks must not appear in recommendations");

        server.abort();
        mb_server.abort();
    }

    // 10. Haikyu!! correctly detects volumes 1–7 owned and only recommends missing volumes (vol 8+)
    #[tokio::test]
    async fn test_haikyu_volumes_1_to_7_recognized_as_owned() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 1092, "Haikyu!!", true);
            // Volumes 1..7 owned, volumes 8..45 missing
            let owned_1_to_7: Vec<f64> = (1..=7).map(|v| v as f64).collect();
            let missing_8_to_45: Vec<f64> = (8..=45).map(|v| v as f64).collect();
            seed_series_reconciliation(
                conn, "manga:1092", 1092, "Haikyu!!", Some("0S4XY0P4ZM30Y"),
                "missing_volume", &owned_1_to_7, &missing_8_to_45
            );

            // Attempt to add proposal for owned Volume 6: should be rejected
            let cand_v6 = create_test_candidate("c_hk_6", "madokami", 6.0, 90.0);
            let res_v6 = add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 6.0, &cand_v6, &[], None)?;
            assert_eq!(res_v6, 0, "Volume 6 is owned in Komga and must be rejected");

            // Attempt to add proposal for missing Volume 8: should succeed
            let cand_v8 = create_test_candidate("c_hk_8", "nyaa", 8.0, 85.0);
            let res_v8 = add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 8.0, &cand_v8, &[], None)?;
            assert!(res_v8 > 0, "Volume 8 is missing and should be added");

            Ok(())
        }).await.unwrap();

        let items = db.call(|conn| list_review_queue(conn, Some("pending"))).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].series_title, "Haikyu!!");
        assert_eq!(items[0].volume_number, 8.0);
        assert_eq!(items[0].confirmed_owned_volumes.len(), 7);
        assert_eq!(items[0].ownership_status, "confirmed_missing");
    }
}
