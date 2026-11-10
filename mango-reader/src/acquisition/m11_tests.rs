//! Comprehensive test suite for M10.1 + M11 (Followed-Series Monitoring & Autonomous Acquisition).

#[cfg(test)]
mod tests {
    use crate::acquisition::monitoring::{
        add_or_update_review_queue_item, get_automation_mode, get_monitored_tracking_statuses,
        get_monitoring_interval_hours, get_monitoring_settings, get_review_queue_item,
        list_monitoring_activity, list_review_queue, log_monitoring_activity_internal,
        reject_review_queue_item, set_automation_mode, skip_review_queue_item,
        sync_followed_series_to_watchers, update_monitoring_settings, AutomationMode,
        MonitoringSettings, UpdateMonitoringSettingsRequest,
    };
    use crate::acquisition::providers::{CandidateMatchType, ReleaseCandidate};
    use crate::db::Db;
    use rusqlite::params;

    fn setup_test_db() -> Db {
        Db::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn seed_manga(conn: &rusqlite::Connection, id: i64, title: &str) {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title) VALUES (?1, 1, ?2, ?3)",
            params![id, format!("/title/{}", id), title],
        )
        .unwrap();
    }

    fn create_test_candidate(id: &str, provider: &str, score: f64, conf: &str, lang: &str) -> ReleaseCandidate {
        ReleaseCandidate {
            candidate_id: id.to_string(),
            provider: provider.to_string(),
            provider_item_id: format!("/item/{id}"),
            title: format!("Test Title {id}"),
            raw_title: format!("Test Title {id}.cbz"),
            match_type: CandidateMatchType::ExactVolume,
            target_volume: Some(1.0),
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
            confidence: Some(conf.to_string()),
            confidence_reason: Some("High confidence test".to_string()),
            language: Some(lang.to_string()),
            edition: Some("standard".to_string()),
            format: Some("cbz".to_string()),
            resolution: Some("1200p".to_string()),
            publisher: Some("VIZ".to_string()),
            ranking_breakdown: None,
        }
    }

    #[tokio::test]
    async fn test_m11_automation_mode_defaults_and_transitions() {
        let db = setup_test_db();

        // Verify default is MonitorOnly (Mode A)
        let initial_mode = db.call(|conn| Ok(get_automation_mode(conn))).await.unwrap();
        assert_eq!(initial_mode, AutomationMode::MonitorOnly);

        // Transition to ReviewBeforeAcquisition (Mode B)
        db.call(|conn| set_automation_mode(conn, AutomationMode::ReviewBeforeAcquisition))
            .await
            .unwrap();
        let mode_b = db.call(|conn| Ok(get_automation_mode(conn))).await.unwrap();
        assert_eq!(mode_b, AutomationMode::ReviewBeforeAcquisition);

        // Transition to AutomaticAcquisition (Mode C)
        db.call(|conn| set_automation_mode(conn, AutomationMode::AutomaticAcquisition))
            .await
            .unwrap();
        let mode_c = db.call(|conn| Ok(get_automation_mode(conn))).await.unwrap();
        assert_eq!(mode_c, AutomationMode::AutomaticAcquisition);

        // Transition back to MonitorOnly
        db.call(|conn| set_automation_mode(conn, AutomationMode::MonitorOnly))
            .await
            .unwrap();
        let mode_a = db.call(|conn| Ok(get_automation_mode(conn))).await.unwrap();
        assert_eq!(mode_a, AutomationMode::MonitorOnly);
    }

    #[tokio::test]
    async fn test_m11_monitored_tracking_statuses_and_interval() {
        let db = setup_test_db();

        // Defaults: 12 hours, ["reading", "planning"]
        let default_interval = db
            .call(|conn| Ok(get_monitoring_interval_hours(conn)))
            .await
            .unwrap();
        assert_eq!(default_interval, 12);

        let default_statuses = db
            .call(|conn| Ok(get_monitored_tracking_statuses(conn)))
            .await
            .unwrap();
        assert_eq!(default_statuses, vec!["reading", "planning"]);

        // Custom settings update
        let new_settings = MonitoringSettings {
            automation_mode: AutomationMode::ReviewBeforeAcquisition,
            interval_hours: 6,
            tracked_statuses: vec!["reading".to_string(), "planning".to_string(), "paused".to_string()],
            last_cycle_at: None,
            auto_acquire_enabled: false,
            operating_mode: "simulation".to_string(),
            auto_publish_manual: false,
        };

        db.call(move |conn| update_monitoring_settings(conn, &new_settings.into()))
            .await
            .unwrap();

        let updated = db.call(|conn| get_monitoring_settings(conn)).await.unwrap();
        assert_eq!(updated.automation_mode, AutomationMode::ReviewBeforeAcquisition);
        assert_eq!(updated.interval_hours, 6);
        assert_eq!(updated.tracked_statuses.len(), 3);
        assert!(updated.tracked_statuses.contains(&"paused".to_string()));
    }

    #[tokio::test]
    async fn test_m11_review_queue_crud_and_status_transitions() {
        let db = setup_test_db();

        db.call(|conn| {
            seed_manga(conn, 10, "Haikyu!!");
            Ok(())
        })
        .await
        .unwrap();

        let cand = create_test_candidate("mado-vol-05", "madokami", 95.0, "high", "en");

        // Add to review queue
        let item_id = db
            .call(move |conn| {
                add_or_update_review_queue_item(
                    conn,
                    Some(10),
                    "manga:10",
                    "Haikyu!!",
                    5.0,
                    &cand,
                    &[],
                    Some("Haikyu!!/Volume 05.cbz"),
                )
            })
            .await
            .unwrap();

        assert!(item_id > 0);

        // Fetch and verify pending status
        let item = db
            .call(move |conn| get_review_queue_item(conn, item_id))
            .await
            .unwrap()
            .expect("Item should exist");
        assert_eq!(item.status, "pending");
        assert_eq!(item.series_title, "Haikyu!!");
        assert_eq!(item.volume_number, 5.0);
        assert_eq!(item.recommended_candidate_id, "mado-vol-05");
        assert_eq!(item.confidence, "high");

        // Reject item
        db.call(move |conn| reject_review_queue_item(conn, item_id, Some("Poor scan quality")))
            .await
            .unwrap();
        let rejected = db
            .call(move |conn| get_review_queue_item(conn, item_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rejected.status, "rejected");
        assert_eq!(rejected.status_reason.as_deref(), Some("Poor scan quality"));

        // Skip item
        db.call(move |conn| skip_review_queue_item(conn, item_id, Some("Will acquire omnibus instead")))
            .await
            .unwrap();
        let skipped = db
            .call(move |conn| get_review_queue_item(conn, item_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(skipped.status, "skipped");
        assert_eq!(skipped.status_reason.as_deref(), Some("Will acquire omnibus instead"));
    }

    #[tokio::test]
    async fn test_m11_monitoring_activity_history_logging() {
        let db = setup_test_db();

        db.call(|conn| {
            seed_manga(conn, 1, "Chainsaw Man");
            Ok(())
        })
        .await
        .unwrap();

        // Log discovered
        db.call(|conn| {
            log_monitoring_activity_internal(
                conn,
                Some(1),
                "Chainsaw Man",
                12.0,
                "discovered",
                "nyaa",
                "high",
                Some(92.0),
                Some("EXACT_VOLUME"),
                "Eligible release discovered on Nyaa",
                1700000000,
            )?;
            // Log proposed
            log_monitoring_activity_internal(
                conn,
                Some(1),
                "Chainsaw Man",
                12.0,
                "plan_proposed",
                "nyaa",
                "high",
                Some(92.0),
                Some("AWAITING_OPERATOR_REVIEW"),
                "Added to Review Queue",
                1700000010,
            )?;
            // Log rejected
            log_monitoring_activity_internal(
                conn,
                Some(1),
                "Chainsaw Man",
                12.0,
                "rejected",
                "nyaa",
                "high",
                Some(92.0),
                Some("OPERATOR_REJECTED"),
                "Operator rejected release",
                1700000020,
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let entries = db
            .call(|conn| list_monitoring_activity(conn, 10))
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].action, "rejected");
        assert_eq!(entries[1].action, "plan_proposed");
        assert_eq!(entries[2].action, "discovered");
        assert_eq!(entries[0].series_title, "Chainsaw Man");
        assert_eq!(entries[0].volume_number, 12.0);
    }

    #[tokio::test]
    async fn test_m11_followed_series_to_watchers_sync() {
        let db = setup_test_db();

        // Seed manga and tracked_catalog
        db.call(|conn| {
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title) VALUES (101, 1, 'http://example.com/jjk', 'Jujutsu Kaisen')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracked_catalog (manga_id, title, tracked, created_at, updated_at)
                 VALUES (101, 'Jujutsu Kaisen', 1, 1000, 1000)",
                [],
            )?;
            // Insert tracker entry as 'reading'
            conn.execute(
                "INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at)
                 VALUES ('anilist', 'jjk-101', 'Jujutsu Kaisen', 'CURRENT', 'reading', 1000)",
                [],
            )?;
            // Insert reconciliation with missing volume 3 and 4
            conn.execute(
                "INSERT INTO series_reconciliations (
                    series_id, title, mangabaka_id, collection_id, status,
                    missing_volumes_json, future_volumes_json, owned_volumes_json, last_reconciled_at, updated_at
                ) VALUES ('manga:101', 'Jujutsu Kaisen', 555, 'col-jjk', 'reconciled', '[3.0, 4.0]', '[]', '[1.0, 2.0]', 1000, 1000)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let http = reqwest::Client::new();
        let synced = sync_followed_series_to_watchers(&db, http).await.unwrap();

        // Should have created watchers for missing volumes 3 and 4
        assert_eq!(synced, 2);

        // Verify watchers exist in database
        let watchers = db
            .call(|conn| crate::acquisition::watcher::list_watchers_for_series(conn, "manga:101"))
            .await
            .unwrap();
        assert_eq!(watchers.len(), 2);
        assert_eq!(watchers[0].volume_number, 3.0);
        assert_eq!(watchers[1].volume_number, 4.0);
        assert_eq!(watchers[0].state.as_str(), "RELEASED_AWAITING_PROVIDER");
    }

    #[tokio::test]
    async fn test_m11_dropped_series_exclusion_from_monitoring() {
        let db = setup_test_db();

        // Seed manga as 'dropped' in tracker list
        db.call(|conn| {
            conn.execute(
                "INSERT INTO manga (id, source_id, url, title) VALUES (202, 1, 'http://example.com/bleach', 'Bleach')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracked_catalog (manga_id, title, tracked, created_at, updated_at)
                 VALUES (202, 'Bleach', 1, 1000, 1000)",
                [],
            )?;
            // Tracker list status: 'dropped'
            conn.execute(
                "INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at)
                 VALUES ('anilist', 'bleach-202', 'Bleach', 'DROPPED', 'dropped', 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO series_reconciliations (
                    series_id, title, mangabaka_id, collection_id, status,
                    missing_volumes_json, future_volumes_json, owned_volumes_json, last_reconciled_at, updated_at
                ) VALUES ('manga:202', 'Bleach', 777, 'col-bleach', 'reconciled', '[5.0]', '[]', '[]', 1000, 1000)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let http = reqwest::Client::new();
        let synced = sync_followed_series_to_watchers(&db, http).await.unwrap();

        // Dropped series should be excluded by default tracking status filter ["reading", "planning"]
        assert_eq!(synced, 0);

        let watchers = db
            .call(|conn| crate::acquisition::watcher::list_watchers_for_series(conn, "manga:202"))
            .await
            .unwrap();
        assert_eq!(watchers.len(), 0);
    }

    #[tokio::test]
    async fn test_m11_mode_b_review_queue_lifecycle() {
        let db = setup_test_db();

        db.call(|conn| {
            seed_manga(conn, 50, "Spy x Family");
            Ok(())
        })
        .await
        .unwrap();

        let cand = create_test_candidate("nyaa-sxf-01", "nyaa", 88.0, "high", "en");

        // 1. Propose candidate into Review Queue
        let item_id = db
            .call(move |conn| {
                add_or_update_review_queue_item(
                    conn,
                    Some(50),
                    "manga:50",
                    "Spy x Family",
                    1.0,
                    &cand,
                    &[],
                    Some("Spy x Family/Volume 01.cbz"),
                )
            })
            .await
            .unwrap();

        // Check it appears in pending review queue
        let pending = db
            .call(|conn| list_review_queue(conn, Some("pending")))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, item_id);
        assert_eq!(pending[0].status, "pending");

        // 2. Mark item approved directly (as would happen during operator approval)
        db.call(move |conn| {
            conn.execute(
                "UPDATE acquisition_review_queue SET status = 'approved', updated_at = 2000 WHERE id = ?1",
                [item_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let approved = db
            .call(move |conn| get_review_queue_item(conn, item_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(approved.status, "approved");

        // Pending list should now be empty
        let pending_after = db
            .call(|conn| list_review_queue(conn, Some("pending")))
            .await
            .unwrap();
        assert_eq!(pending_after.len(), 0);
    }

    #[tokio::test]
    async fn test_m11_mode_c_safety_gates_and_simulation() {
        let db = setup_test_db();

        db.call(|conn| {
            seed_manga(conn, 60, "Berserk");
            Ok(())
        })
        .await
        .unwrap();

        // Check global auto-acquire default is false
        let is_auto = db
            .call(|conn| Ok(crate::acquisition::auto_acquire::is_global_auto_acquire_enabled(conn)))
            .await
            .unwrap();
        assert!(!is_auto, "Global auto acquire must be OFF by default");

        // Check simulation mode default is true
        let is_sim = db
            .call(|conn| Ok(crate::acquisition::auto_acquire::is_simulation_mode(conn)))
            .await
            .unwrap();
        assert!(is_sim, "Simulation mode must be default active for safety");

        // Simulation activity logging test
        db.call(|conn| {
            log_monitoring_activity_internal(
                conn,
                Some(60),
                "Berserk",
                1.0,
                "simulated",
                "madokami",
                "high",
                Some(95.0),
                Some("WOULD_AUTO_ACQUIRE"),
                "Simulation Mode: Would auto-acquire Berserk Vol. 1",
                1700000000,
            )
        })
        .await
        .unwrap();

        let activity = db
            .call(|conn| list_monitoring_activity(conn, 10))
            .await
            .unwrap();
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].decision_code.as_deref(), Some("WOULD_AUTO_ACQUIRE"));
        assert_eq!(activity[0].action, "simulated");
    }

    #[tokio::test]
    async fn test_m11_partial_update_only_automation_mode() {
        let db = setup_test_db();

        let initial = db.call(|conn| get_monitoring_settings(conn)).await.unwrap();
        assert_eq!(initial.automation_mode, AutomationMode::MonitorOnly);
        assert_eq!(initial.interval_hours, 12);
        assert_eq!(initial.tracked_statuses, vec!["reading", "planning"]);
        assert!(!initial.auto_acquire_enabled);
        assert_eq!(initial.operating_mode, "simulation");
        assert!(!initial.auto_publish_manual);

        // Update ONLY automation_mode
        let req = UpdateMonitoringSettingsRequest {
            automation_mode: Some("review_before_acquisition".to_string()),
            ..Default::default()
        };

        let updated = db
            .call(move |conn| update_monitoring_settings(conn, &req))
            .await
            .unwrap();

        assert_eq!(updated.automation_mode, AutomationMode::ReviewBeforeAcquisition);
        // Assert all other 5 fields preserved
        assert_eq!(updated.interval_hours, 12);
        assert_eq!(updated.tracked_statuses, vec!["reading", "planning"]);
        assert!(!updated.auto_acquire_enabled);
        assert_eq!(updated.operating_mode, "simulation");
        assert!(!updated.auto_publish_manual);

        // Verify persisted state matches
        let persisted = db.call(|conn| get_monitoring_settings(conn)).await.unwrap();
        assert_eq!(persisted.automation_mode, AutomationMode::ReviewBeforeAcquisition);
        assert_eq!(persisted.interval_hours, 12);
    }

    #[tokio::test]
    async fn test_m11_partial_update_only_tracked_statuses() {
        let db = setup_test_db();

        // Update ONLY tracked_statuses
        let req = UpdateMonitoringSettingsRequest {
            tracked_statuses: Some(vec!["reading".to_string(), "completed".to_string()]),
            ..Default::default()
        };

        let updated = db
            .call(move |conn| update_monitoring_settings(conn, &req))
            .await
            .unwrap();

        assert_eq!(updated.tracked_statuses, vec!["reading", "completed"]);
        assert_eq!(updated.automation_mode, AutomationMode::MonitorOnly);
        assert_eq!(updated.interval_hours, 12);
        assert!(!updated.auto_acquire_enabled);
        assert_eq!(updated.operating_mode, "simulation");
        assert!(!updated.auto_publish_manual);
    }

    #[tokio::test]
    async fn test_m11_partial_update_only_interval_hours() {
        let db = setup_test_db();

        // Update ONLY interval_hours
        let req = UpdateMonitoringSettingsRequest {
            interval_hours: Some(6),
            ..Default::default()
        };

        let updated = db
            .call(move |conn| update_monitoring_settings(conn, &req))
            .await
            .unwrap();

        assert_eq!(updated.interval_hours, 6);
        assert_eq!(updated.automation_mode, AutomationMode::MonitorOnly);
        assert_eq!(updated.tracked_statuses, vec!["reading", "planning"]);
        assert!(!updated.auto_acquire_enabled);
        assert_eq!(updated.operating_mode, "simulation");
        assert!(!updated.auto_publish_manual);
    }

    #[tokio::test]
    async fn test_m11_partial_update_preserves_custom_safety_settings() {
        let db = setup_test_db();

        // Ensure safety defaults are strictly preserved across sequential partial updates
        let req1 = UpdateMonitoringSettingsRequest {
            automation_mode: Some("monitor_only".to_string()),
            ..Default::default()
        };
        db.call(move |conn| update_monitoring_settings(conn, &req1)).await.unwrap();

        let req2 = UpdateMonitoringSettingsRequest {
            interval_hours: Some(24),
            ..Default::default()
        };
        db.call(move |conn| update_monitoring_settings(conn, &req2)).await.unwrap();

        let req3 = UpdateMonitoringSettingsRequest {
            tracked_statuses: Some(vec!["reading".to_string(), "paused".to_string()]),
            ..Default::default()
        };
        let final_settings = db.call(move |conn| update_monitoring_settings(conn, &req3)).await.unwrap();

        assert_eq!(final_settings.interval_hours, 24);
        assert_eq!(final_settings.tracked_statuses, vec!["reading", "paused"]);
        // Critical safety invariants: never activate auto-acquire or normal mode accidentally
        assert!(!final_settings.auto_acquire_enabled, "auto_acquire must remain disabled");
        assert_eq!(final_settings.operating_mode, "simulation", "operating_mode must remain simulation");
        assert!(!final_settings.auto_publish_manual, "auto_publish_manual must remain disabled");
    }

    #[tokio::test]
    async fn test_m11_partial_update_validation_rejects_malformed_input() {
        let db = setup_test_db();

        // 1. Invalid automation mode
        let bad_mode = UpdateMonitoringSettingsRequest {
            automation_mode: Some("invalid_mode_xyz".to_string()),
            ..Default::default()
        };
        let err = db.call(move |conn| update_monitoring_settings(conn, &bad_mode)).await;
        assert!(err.is_err(), "Invalid automation mode must fail validation");

        // 2. Invalid interval (< 1 or > 168)
        let zero_interval = UpdateMonitoringSettingsRequest {
            interval_hours: Some(0),
            ..Default::default()
        };
        let err2 = db.call(move |conn| update_monitoring_settings(conn, &zero_interval)).await;
        assert!(err2.is_err(), "Interval 0 must fail validation");

        let huge_interval = UpdateMonitoringSettingsRequest {
            interval_hours: Some(999),
            ..Default::default()
        };
        let err3 = db.call(move |conn| update_monitoring_settings(conn, &huge_interval)).await;
        assert!(err3.is_err(), "Interval 999 must fail validation");

        // 3. Invalid tracking status
        let bad_status = UpdateMonitoringSettingsRequest {
            tracked_statuses: Some(vec!["reading".to_string(), "bogus_status".to_string()]),
            ..Default::default()
        };
        let err4 = db.call(move |conn| update_monitoring_settings(conn, &bad_status)).await;
        assert!(err4.is_err(), "Invalid tracking status must fail validation");

        // 4. Invalid operating mode
        let bad_op_mode = UpdateMonitoringSettingsRequest {
            operating_mode: Some("unrestricted".to_string()),
            ..Default::default()
        };
        let err5 = db.call(move |conn| update_monitoring_settings(conn, &bad_op_mode)).await;
        assert!(err5.is_err(), "Invalid operating mode must fail validation");

        // Verify atomicity: database is untouched and remains at defaults
        let settings = db.call(|conn| get_monitoring_settings(conn)).await.unwrap();
        assert_eq!(settings.automation_mode, AutomationMode::MonitorOnly);
        assert_eq!(settings.interval_hours, 12);
        assert_eq!(settings.tracked_statuses, vec!["reading", "planning"]);
    }

    #[tokio::test]
    async fn test_m11_http_endpoint_partial_updates_and_no_422() {
        use axum::routing::post;
        use axum::Router;
        use tokio::net::TcpListener;

        let db = setup_test_db();
        let db_handler = db.clone();

        // Construct Axum router reproducing the /api/monitoring/settings endpoint
        let app = Router::new().route(
            "/api/monitoring/settings",
            post(move |axum::Json(payload): axum::Json<UpdateMonitoringSettingsRequest>| {
                let db = db_handler.clone();
                async move {
                    let updated = db
                        .call(move |conn| update_monitoring_settings(conn, &payload))
                        .await
                        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?;
                    Ok::<_, (axum::http::StatusCode, String)>(axum::Json(updated))
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let base_url = format!("http://{addr}/api/monitoring/settings");

        // 1. Partial update: ONLY automationMode (reproducing exact UI payload that previously produced 422)
        let resp1 = client
            .post(&base_url)
            .header("Content-Type", "application/json")
            .body(r#"{"automationMode":"review_before_acquisition"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp1.status(), reqwest::StatusCode::OK, "Partial automationMode must return 200 OK, not 422");
        let body1: MonitoringSettings = resp1.json().await.unwrap();
        assert_eq!(body1.automation_mode, AutomationMode::ReviewBeforeAcquisition);
        assert_eq!(body1.interval_hours, 12);
        assert_eq!(body1.tracked_statuses, vec!["reading", "planning"]);

        // 2. Partial update: ONLY trackedStatuses
        let resp2 = client
            .post(&base_url)
            .header("Content-Type", "application/json")
            .body(r#"{"trackedStatuses":["reading","completed"]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp2.status(), reqwest::StatusCode::OK, "Partial trackedStatuses must return 200 OK, not 422");
        let body2: MonitoringSettings = resp2.json().await.unwrap();
        assert_eq!(body2.tracked_statuses, vec!["reading", "completed"]);
        assert_eq!(body2.automation_mode, AutomationMode::ReviewBeforeAcquisition);

        // 3. Partial update: ONLY intervalHours
        let resp3 = client
            .post(&base_url)
            .header("Content-Type", "application/json")
            .body(r#"{"intervalHours":6}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp3.status(), reqwest::StatusCode::OK, "Partial intervalHours must return 200 OK, not 422");
        let body3: MonitoringSettings = resp3.json().await.unwrap();
        assert_eq!(body3.interval_hours, 6);
        assert_eq!(body3.automation_mode, AutomationMode::ReviewBeforeAcquisition);

        // 4. Snake_case alias support
        let resp4 = client
            .post(&base_url)
            .header("Content-Type", "application/json")
            .body(r#"{"automation_mode":"monitor_only"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp4.status(), reqwest::StatusCode::OK);
        let body4: MonitoringSettings = resp4.json().await.unwrap();
        assert_eq!(body4.automation_mode, AutomationMode::MonitorOnly);

        // 5. Malformed payload returns 400 Bad Request, NOT 422 or 500
        let resp5 = client
            .post(&base_url)
            .header("Content-Type", "application/json")
            .body(r#"{"intervalHours":0}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp5.status(), reqwest::StatusCode::BAD_REQUEST);
    }
}
