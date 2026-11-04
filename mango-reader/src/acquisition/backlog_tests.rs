//! Unit tests for M6.10 Guarded Historical Backlog Acquisition.

use crate::acquisition::backlog::{BacklogScope, ExecuteBacklogRequest, PlanBacklogRequest};
use crate::acquisition::service::AcquisitionService;
use crate::db::{now, Db};

static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TempDbDir {
    path: std::path::PathBuf,
}

impl Drop for TempDbDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn create_test_db() -> (Db, TempDbDir) {
    let count = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "mango_backlog_test_{}_{}_{}",
        now(),
        std::process::id(),
        count
    ));
    let _ = std::fs::create_dir_all(&temp_dir);
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();
    (db, TempDbDir { path: temp_dir })
}

async fn insert_test_manga_with_meta(
    db: &Db,
    manga_id: i64,
    title: &str,
    total_official: Option<i64>,
    future_vols: &[f64],
) {
    let t = title.to_string();
    let t_clone = title.to_string();
    let sid = format!("title:{}", title.to_lowercase().replace(' ', "-"));
    let future_json = serde_json::to_string(future_vols).unwrap();
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO manga (id, source_id, url, title, update_strategy)
             VALUES (?1, 1, ?2, ?3, 'ALWAYS_UPDATE')",
            rusqlite::params![manga_id, format!("url:{}", manga_id), t],
        )?;

        conn.execute(
            "INSERT INTO series_reconciliations (
                series_id, manga_id, title, status, total_official_volumes, future_volumes_json, last_reconciled_at, updated_at
             ) VALUES (?1, ?2, ?3, 'MATCHED', ?4, ?5, ?6, ?6)",
            rusqlite::params![sid, manga_id, t_clone, total_official, future_json, now()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn insert_test_candidate(
    db: &Db,
    title: &str,
    volume: f64,
    provider: &str,
    cand_id: &str,
    score: f64,
) {
    let sid = format!("title:{}", title.to_lowercase().replace(' ', "-"));
    let p = provider.to_string();
    let cid = cand_id.to_string();
    let c_title = format!("{title} Vol. {volume}");
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO discovered_release_candidates (
                series_id, volume_number, candidate_id, provider, provider_item_id,
                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                is_digital, score, score_explanation, download_url, magnet_url,
                info_hash, seeders, leechers, published_at, discovered_at
             ) VALUES (?1, ?2, ?3, ?4, 'item_1', ?5, ?5, 'exact_match', 1000000,
                       'TestUploader', 1, 1, ?6, 'Valid test candidate',
                       'https://example.com/dl', 'magnet:?xt=urn:btih:test', 'test_hash',
                       10, 2, ?7, ?7)",
            rusqlite::params![sid, volume, cid, p, c_title, score, now()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_backlog_plan_bounds_batch_size_and_defaults_to_dry_run() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    insert_test_manga_with_meta(&db, 101, "Frieren", Some(12), &[13.0, 14.0]).await;
    insert_test_candidate(&db, "Frieren", 1.0, "madokami", "frieren-v1-mado", 95.0).await;
    insert_test_candidate(&db, "Frieren", 2.0, "nyaa", "frieren-v2-nyaa", 85.0).await;

    // Test 1: Plan with default batch_size and dry_run
    let plan = svc
        .plan_backlog_acquisition(
            101,
            PlanBacklogRequest {
                scope: BacklogScope::Range,
                range_start: Some(1.0),
                range_end: Some(5.0),
                explicit_volumes: None,
                batch_size: None, // Should default to 5
                is_dry_run: None,  // Should default to true
            },
        )
        .await
        .unwrap();

    assert_eq!(plan.batch_size, 5);
    assert!(plan.is_dry_run);
    assert_eq!(plan.state, "PLANNED");
    assert_eq!(plan.total_requested, 5);

    // Volumes 1 and 2 have candidates -> ELIGIBLE
    // Volume 1 candidate preferred Madokami
    let v1 = plan.items.iter().find(|i| (i.volume_number - 1.0).abs() < 0.001).unwrap();
    assert_eq!(v1.status, "ELIGIBLE");
    assert_eq!(v1.provider.as_deref(), Some("madokami"));

    let v2 = plan.items.iter().find(|i| (i.volume_number - 2.0).abs() < 0.001).unwrap();
    assert_eq!(v2.status, "ELIGIBLE");
    assert_eq!(v2.provider.as_deref(), Some("nyaa"));

    // Volumes 3, 4, 5 have no candidates -> UNAVAILABLE
    let v3 = plan.items.iter().find(|i| (i.volume_number - 3.0).abs() < 0.001).unwrap();
    assert_eq!(v3.status, "UNAVAILABLE");

    // Test 2: Plan clamping batch_size upper bound
    let plan_clamped = svc
        .plan_backlog_acquisition(
            101,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![1.0, 2.0]),
                batch_size: Some(100), // Exceeds MAX_BACKLOG_BATCH_SIZE (20)
                is_dry_run: Some(false),
            },
        )
        .await
        .unwrap();

    assert_eq!(plan_clamped.batch_size, 20); // Clamped to 20
    assert!(!plan_clamped.is_dry_run);
}

#[tokio::test]
async fn test_backlog_plan_respects_future_unreleased_volumes() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Official released = 8, Future = 9, 10
    insert_test_manga_with_meta(&db, 102, "Kagurabachi", Some(8), &[9.0, 10.0]).await;
    insert_test_candidate(&db, "Kagurabachi", 9.0, "nyaa", "kb-v9-fake", 50.0).await;

    let plan = svc
        .plan_backlog_acquisition(
            102,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![9.0]),
                batch_size: Some(5),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    let v9 = plan.items.iter().find(|i| (i.volume_number - 9.0).abs() < 0.001).unwrap();
    assert_eq!(v9.status, "UNRELEASED");
    assert_eq!(plan.total_eligible, 0);
}

#[tokio::test]
async fn test_backlog_execution_requires_user_confirmation() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    insert_test_manga_with_meta(&db, 103, "Dungeon Meshi", Some(14), &[]).await;
    insert_test_candidate(&db, "Dungeon Meshi", 1.0, "madokami", "dm-v1", 90.0).await;

    let plan = svc
        .plan_backlog_acquisition(
            103,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![1.0]),
                batch_size: Some(5),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    // Calling execute without confirm: true must fail
    let err = svc
        .execute_backlog_job(
            &plan.id,
            ExecuteBacklogRequest {
                job_id: Some(plan.id.clone()),
                confirm: false,
                confirm_multiple: None,
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap_err();

    assert!(err.message.contains("confirmation required"));
}

#[tokio::test]
async fn test_backlog_execution_requires_multi_volume_confirmation() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    insert_test_manga_with_meta(&db, 104, "Chainsaw Man", Some(15), &[]).await;
    insert_test_candidate(&db, "Chainsaw Man", 1.0, "madokami", "csm-v1", 90.0).await;
    insert_test_candidate(&db, "Chainsaw Man", 2.0, "madokami", "csm-v2", 90.0).await;

    let plan = svc
        .plan_backlog_acquisition(
            104,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![1.0, 2.0]),
                batch_size: Some(5),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    assert_eq!(plan.total_eligible, 2);

    // Calling execute on 2 volumes without confirm_multiple: true must fail
    let err = svc
        .execute_backlog_job(
            &plan.id,
            ExecuteBacklogRequest {
                job_id: Some(plan.id.clone()),
                confirm: true,
                confirm_multiple: Some(false),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap_err();

    assert!(err.message.contains("Multi-volume confirmation required"));
}

#[tokio::test]
async fn test_backlog_dry_run_execution_marks_complete_without_downloads() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    insert_test_manga_with_meta(&db, 105, "Berserk", Some(41), &[]).await;
    insert_test_candidate(&db, "Berserk", 1.0, "madokami", "berserk-v1", 90.0).await;
    insert_test_candidate(&db, "Berserk", 2.0, "madokami", "berserk-v2", 90.0).await;

    let plan = svc
        .plan_backlog_acquisition(
            105,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![1.0, 2.0]),
                batch_size: Some(5),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    let exec_res = svc
        .execute_backlog_job(
            &plan.id,
            ExecuteBacklogRequest {
                job_id: Some(plan.id.clone()),
                confirm: true,
                confirm_multiple: Some(true),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    assert_eq!(exec_res.state, "COMPLETED");
    assert!(exec_res.is_dry_run);
    assert_eq!(exec_res.queued_count, 0); // Zero real acquisitions initiated

    // Verify audit record was updated
    let jobs = svc.get_manga_backlog_jobs(105, 10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, "COMPLETED");
    assert!(jobs[0].confirmed_by_user);
    assert!(jobs[0].is_dry_run);
}

#[tokio::test]
async fn test_backlog_job_cancellation() {
    let (db, _dir) = create_test_db();
    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    insert_test_manga_with_meta(&db, 106, "Vinland Saga", Some(27), &[]).await;
    insert_test_candidate(&db, "Vinland Saga", 1.0, "madokami", "vs-v1", 90.0).await;

    let plan = svc
        .plan_backlog_acquisition(
            106,
            PlanBacklogRequest {
                scope: BacklogScope::ExplicitVolumes,
                range_start: None,
                range_end: None,
                explicit_volumes: Some(vec![1.0]),
                batch_size: Some(5),
                is_dry_run: Some(true),
            },
        )
        .await
        .unwrap();

    assert_eq!(plan.state, "PLANNED");

    let cancelled = svc.cancel_backlog_job(&plan.id).await.unwrap();
    assert!(cancelled);

    let jobs = svc.get_manga_backlog_jobs(106, 10).await.unwrap();
    assert_eq!(jobs[0].state, "CANCELLED");

    // Second cancel returns false
    let cancelled_again = svc.cancel_backlog_job(&plan.id).await.unwrap();
    assert!(!cancelled_again);
}
