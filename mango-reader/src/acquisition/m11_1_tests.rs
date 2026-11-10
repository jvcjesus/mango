//! Comprehensive test suite for M11.1:
//! - Canonical Manga Identity Resolution
//! - Availability Watcher Deduplication
//! - Sequential (Lowest Missing First) Acquisition Ordering & Review Queue Priority
//! - TorBox Health Telemetry Shared Source of Truth
//! - Library Integrity Audit Enhancements

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use crate::acquisition::dashboard::compute_provider_health;
    use crate::acquisition::monitoring::{
        add_or_update_review_queue_item, calculate_acquisition_priority, get_review_queue_item,
        list_review_queue, reject_review_queue_item,
    };
    use crate::acquisition::providers::{CandidateMatchType, ReleaseCandidate};
    use crate::db::Db;
    use crate::sync::integrity::audit_library_integrity;
    use crate::sync::reconciliation::resolve_canonical_manga_id;

    fn setup_test_db() -> Db {
        Db::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn seed_manga(conn: &rusqlite::Connection, id: i64, title: &str) {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title) VALUES (?1, 1, ?2, ?3)",
            params![id, format!("/title/{id}"), title],
        )
        .unwrap();
    }

    fn create_test_candidate(id: &str, title: &str, vol: f64, score: f64) -> ReleaseCandidate {
        ReleaseCandidate {
            candidate_id: id.to_string(),
            provider: "torbox".to_string(),
            provider_item_id: format!("/item/{id}"),
            title: title.to_string(),
            raw_title: format!("{title}.cbz"),
            match_type: CandidateMatchType::ExactVolume,
            target_volume: Some(vol),
            target_chapter: None,
            size_bytes: Some(100 * 1024 * 1024),
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
            confidence: Some("HIGH".to_string()),
            confidence_reason: Some("Exact match".to_string()),
            language: Some("en".to_string()),
            edition: Some("standard".to_string()),
            format: Some("cbz".to_string()),
            resolution: Some("1200p".to_string()),
            publisher: Some("VIZ".to_string()),
            ranking_breakdown: None,
        }
    }

    #[tokio::test]
    async fn test_canonical_manga_identity_resolution() {
        let db = setup_test_db();
        db.call(|conn| {
            // Seed mirrors: 1092 is library manga, 1066 is Madokami mirror, 1172 is MangaDex mirror
            seed_manga(conn, 1066, "Haikyu!!");
            seed_manga(conn, 1092, "Haikyu!!");
            seed_manga(conn, 1172, "Haikyu!!");

            // Configure linking
            conn.execute("UPDATE manga SET in_library = 1, source_manga_id = 1066 WHERE id = 1092", []).unwrap();
            conn.execute("UPDATE manga SET catalog_manga_id = 1092 WHERE id = 1066", []).unwrap();
            conn.execute("UPDATE manga SET catalog_manga_id = 1092 WHERE id = 1172", []).unwrap();

            // All three should resolve to canonical ID 1092
            assert_eq!(resolve_canonical_manga_id(conn, 1092), 1092);
            assert_eq!(resolve_canonical_manga_id(conn, 1066), 1092);
            assert_eq!(resolve_canonical_manga_id(conn, 1172), 1092);

            // Independent unlinked title resolves to its own ID
            seed_manga(conn, 2000, "Blood on the Tracks");
            assert_eq!(resolve_canonical_manga_id(conn, 2000), 2000);

            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_lowest_missing_volume_first_priority_calculation() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 1092, "Haikyu!!");
            conn.execute("UPDATE manga SET in_library = 1 WHERE id = 1092", []).unwrap();

            // Insert owned volumes 1, 2, 3, 4
            for v in 1..=4 {
                conn.execute(
                    "INSERT INTO chapters (manga_id, name, url, chapter_number, download_path, source_order, fetched_at)
                     VALUES (1092, ?1, ?2, ?3, ?4, ?5, ?6)",
                    params![format!("Volume {v}"), format!("/vol/{v}"), v as f64, format!("/srv/media/manga/Haikyu!!/Haikyu!! - v{v:02}.cbz"), v, 1700000000],
                ).unwrap();
            }

            // Seed watchers for missing volumes 5..=9
            for v in 5..=9 {
                conn.execute(
                    "INSERT INTO release_availability_watchers (series_id, canonical_series_id, series_title, volume_number, market, edition, is_released, is_owned, monitoring_enabled, notify_on_available, state, top_candidate_id, next_check_at, check_interval_secs, consecutive_failures, candidate_count, created_at, updated_at)
                     VALUES ('manga:1092', 'manga:1092', 'Haikyu!!', ?1, 'en', 'standard', 1, 0, 1, 0, 'ELIGIBLE_CANDIDATE_FOUND', 'cand_active', 100, 3600, 0, 1, 100, 100)",
                    params![v as f64],
                ).unwrap();
            }

            // Test Volume 5: Lowest missing volume -> Priority #1, safe to approve
            let (p5, gaps5, exp5, safe5) = calculate_acquisition_priority(conn, "Haikyu!!", 5.0);
            assert_eq!(p5, 1);
            assert!(gaps5.is_none());
            assert!(safe5);
            assert!(exp5.unwrap().contains("Lowest missing volume"));

            // Test Volume 6: Priority #2, out of order because Volume 5 is missing
            let (p6, gaps6, exp6, safe6) = calculate_acquisition_priority(conn, "Haikyu!!", 6.0);
            assert_eq!(p6, 2);
            assert!(gaps6.unwrap().contains("5"));
            assert!(!safe6);
            assert!(exp6.unwrap().contains("Vol. 5"));

            // Test Volume 9: Priority #5, earlier missing gaps are 5, 6, 7, 8
            let (p9, gaps9, exp9, safe9) = calculate_acquisition_priority(conn, "Haikyu!!", 9.0);
            assert_eq!(p9, 5);
            assert!(gaps9.unwrap().contains("5"));
            assert!(!safe9);
            assert!(exp9.unwrap().contains("Out-of-order proposal"));

            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_review_queue_ranking_and_single_active_proposal() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 1092, "Haikyu!!");
            conn.execute("UPDATE manga SET in_library = 1 WHERE id = 1092", []).unwrap();

            // Seed owned volumes 1..4
            for v in 1..=4 {
                conn.execute(
                    "INSERT INTO chapters (manga_id, name, url, chapter_number, download_path, source_order, fetched_at)
                     VALUES (1092, ?1, ?2, ?3, ?4, ?5, ?6)",
                    params![format!("Volume {v}"), format!("/vol/{v}"), v as f64, format!("/srv/media/manga/Haikyu!!/Haikyu!! - v{v:02}.cbz"), v, 1700000000],
                ).unwrap();
            }

            // Seed watchers for missing volumes 5..=9
            for v in 5..=9 {
                conn.execute(
                    "INSERT INTO release_availability_watchers (series_id, canonical_series_id, series_title, volume_number, market, edition, is_released, is_owned, monitoring_enabled, notify_on_available, state, top_candidate_id, next_check_at, check_interval_secs, consecutive_failures, candidate_count, created_at, updated_at)
                     VALUES ('manga:1092', 'manga:1092', 'Haikyu!!', ?1, 'en', 'standard', 1, 0, 1, 0, 'ELIGIBLE_CANDIDATE_FOUND', 'cand_active', 100, 3600, 0, 1, 100, 100)",
                    params![v as f64],
                ).unwrap();
            }

            // Propose Volume 9 first (simulating discovery arriving out-of-order)
            let cand9 = create_test_candidate("c9", "Haikyu!! Vol 9", 9.0, 95.0);
            let id9 = add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 9.0, &cand9, &[], None).unwrap();
            let item9 = get_review_queue_item(conn, id9).unwrap().unwrap();
            assert_eq!(item9.acquisition_priority, 5);
            assert!(!item9.is_safe_to_approve);

            // Propose Volume 5 next
            let cand5 = create_test_candidate("c5", "Haikyu!! Vol 5", 5.0, 90.0);
            let id5 = add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 5.0, &cand5, &[], None).unwrap();
            let item5 = get_review_queue_item(conn, id5).unwrap().unwrap();
            assert_eq!(item5.acquisition_priority, 1);
            assert!(item5.is_safe_to_approve);

            // List queue: Volume 5 MUST appear before Volume 9 despite Volume 9 having higher quality score (95 vs 90)
            let queue = list_review_queue(conn, Some("pending")).unwrap();
            assert_eq!(queue.len(), 2);
            assert_eq!(queue[0].volume_number, 5.0);
            assert_eq!(queue[0].acquisition_priority, 1);
            assert_eq!(queue[1].volume_number, 9.0);
            assert_eq!(queue[1].acquisition_priority, 5);

            // Adding a second candidate for Volume 5 must update existing item, NOT create duplicate
            let cand5_better = create_test_candidate("c5b", "Haikyu!! Vol 5 Digital", 5.0, 98.0);
            add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 5.0, &cand5_better, &[], None).unwrap();
            let queue_after = list_review_queue(conn, Some("pending")).unwrap();
            assert_eq!(queue_after.len(), 2);
            assert_eq!(queue_after[0].recommended_candidate_id, "c5b");

            // Operator rejecting Volume 5 candidate
            reject_review_queue_item(conn, queue_after[0].id, Some("Poor release quality")).unwrap();
            let pending_after_reject = list_review_queue(conn, Some("pending")).unwrap();
            assert_eq!(pending_after_reject.len(), 1);
            assert_eq!(pending_after_reject[0].volume_number, 9.0);

            // Adding candidate for Volume 5 again must respect user rejection
            add_or_update_review_queue_item(conn, Some(1092), "manga:1092", "Haikyu!!", 5.0, &cand5, &[], None).unwrap();
            let pending_check = list_review_queue(conn, Some("pending")).unwrap();
            assert_eq!(pending_check.len(), 1); // Volume 5 was not resurrected into pending

            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_torbox_telemetry_shared_source_of_truth() {
        let db = setup_test_db();
        db.call(|conn| {
            // Case 1: Disabled
            crate::db::set_state(conn, "acquisition.torbox_enabled", "false").unwrap();
            let health_disabled = compute_provider_health(conn);
            let tb_disabled = health_disabled.iter().find(|p| p.name == "TorBox").unwrap();
            assert_eq!(tb_disabled.status, "DISABLED");

            // Case 2: Enabled without token
            crate::db::set_state(conn, "acquisition.torbox_enabled", "true").unwrap();
            crate::db::set_state(conn, "acquisition.torbox_token", "").unwrap();
            let health_unconfigured = compute_provider_health(conn);
            let tb_unconfigured = health_unconfigured.iter().find(|p| p.name == "TorBox").unwrap();
            assert_eq!(tb_unconfigured.status, "UNCONFIGURED");

            // Case 3: Enabled with validated token -> HEALTHY
            crate::db::set_state(conn, "acquisition.torbox_token", "tb_valid_token").unwrap();
            crate::db::set_state(conn, "acquisition.torbox_validated", "valid").unwrap();
            let health_healthy = compute_provider_health(conn);
            let tb_healthy = health_healthy.iter().find(|p| p.name == "TorBox").unwrap();
            assert_eq!(tb_healthy.status, "HEALTHY");
            assert!(tb_healthy.details.contains("active") || tb_healthy.details.contains("validated"));

            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_library_integrity_audit_duplicate_watchers_and_proposals() {
        let db = setup_test_db();
        db.call(|conn| {
            seed_manga(conn, 1066, "Haikyu!!");
            seed_manga(conn, 1092, "Haikyu!!");
            conn.execute("UPDATE manga SET catalog_manga_id = 1092 WHERE id = 1066", []).unwrap();
            conn.execute("UPDATE manga SET mangabaka_id = 8718 WHERE id IN (1066, 1092)", []).unwrap();

            // Run integrity audit: 1066 and 1092 should NOT trigger MangaBaka identity conflict because they are linked mirrors
            let rep1 = audit_library_integrity(conn, None).unwrap();
            let mb_conflicts: Vec<_> = rep1.issues.iter().filter(|i| i.code == "MANGABAKA_IDENTITY_CONFLICT").collect();
            assert!(mb_conflicts.is_empty(), "Linked mirror series should not be flagged as MangaBaka identity conflicts");

            // Insert duplicate active pending review proposals with different mirror series IDs
            conn.execute(
                "INSERT INTO acquisition_review_queue (series_id, series_title, volume_number, recommended_candidate_id, provider, title, raw_title, quality_score, confidence, status, created_at, updated_at)
                 VALUES ('manga:1066', 'Haikyu!!', 5.0, 'cand1', 'torbox', 'Vol 5', 'Vol 5', 90.0, 'HIGH', 'pending', 100, 100)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO acquisition_review_queue (series_id, series_title, volume_number, recommended_candidate_id, provider, title, raw_title, quality_score, confidence, status, created_at, updated_at)
                 VALUES ('manga:1092', 'Haikyu!!', 5.0, 'cand2', 'madokami', 'Vol 5', 'Vol 5', 85.0, 'HIGH', 'pending', 101, 101)",
                [],
            ).unwrap();

            // Also insert duplicate availability watchers (with canonical_series_id NULL as historical pre-migration rows)
            conn.execute(
                "INSERT INTO release_availability_watchers (series_id, canonical_series_id, series_title, volume_number, market, edition, is_released, is_owned, monitoring_enabled, notify_on_available, state, next_check_at, check_interval_secs, consecutive_failures, candidate_count, created_at, updated_at)
                 VALUES ('manga:1066', NULL, 'Haikyu!!', 5.0, 'en', 'standard', 1, 0, 1, 0, 'ELIGIBLE_CANDIDATE_FOUND', 100, 3600, 0, 1, 100, 100)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO release_availability_watchers (series_id, canonical_series_id, series_title, volume_number, market, edition, is_released, is_owned, monitoring_enabled, notify_on_available, state, next_check_at, check_interval_secs, consecutive_failures, candidate_count, created_at, updated_at)
                 VALUES ('manga:1172', NULL, 'Haikyu!!', 5.0, 'en', 'standard', 1, 0, 1, 0, 'ELIGIBLE_CANDIDATE_FOUND', 101, 3600, 0, 1, 101, 101)",
                [],
            ).unwrap();

            let rep2 = audit_library_integrity(conn, None).unwrap();
            let dup_proposals: Vec<_> = rep2.issues.iter().filter(|i| i.code == "DUPLICATE_REVIEW_PROPOSALS").collect();
            assert_eq!(dup_proposals.len(), 1);
            assert!(dup_proposals[0].message.contains("2 active pending review proposals"));

            let dup_watchers: Vec<_> = rep2.issues.iter().filter(|i| i.code == "DUPLICATE_AVAILABILITY_WATCHERS").collect();
            assert_eq!(dup_watchers.len(), 1);
            assert!(dup_watchers[0].message.contains("duplicate availability watchers"));

            Ok(())
        }).await.unwrap();
    }
}
