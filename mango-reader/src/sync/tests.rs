use super::{
    komga_reader::{evaluate_series_progress, parse_chapter_number},
    mapping::{delete_mapping, get_mapping, list_mappings, resolve_mapping, save_mapping},
    model::{SyncTarget, TrackerMapping},
};
use crate::{
    db::Db,
    pipeline::komga::{KomgaBook, KomgaBookMetadata, KomgaReadProgress, KomgaSeries},
};

#[test]
fn test_parse_chapter_numbers() {
    assert_eq!(parse_chapter_number("Blue Lock c001"), Some(1.0));
    assert_eq!(parse_chapter_number("One Piece ch. 1050"), Some(1050.0));
    assert_eq!(parse_chapter_number("Chapter 45.5 - Special"), Some(45.5));
    assert_eq!(parse_chapter_number("Berserk c128"), Some(128.0));
    assert_eq!(parse_chapter_number("Blue Lock v01"), None);
    assert_eq!(parse_chapter_number("Volume 12"), None);
}

#[test]
fn test_evaluate_volume_series_progress() {
    let series = KomgaSeries {
        id: "series-blue-lock".into(),
        name: "Blue Lock".into(),
        library_id: "lib-en".into(),
        books_count: 3,
    };

    let books = vec![
        KomgaBook {
            id: "b1".into(),
            series_id: "series-blue-lock".into(),
            name: "Blue Lock v01".into(),
            number: 1.0,
            metadata: Some(KomgaBookMetadata {
                title: None,
                number: Some("1".into()),
                number_sort: Some(1.0),
            }),
            read_progress: Some(KomgaReadProgress {
                page: 192,
                completed: true,
                read_date: Some("2024-01-01T00:00:00Z".into()),
            }),
        },
        KomgaBook {
            id: "b2".into(),
            series_id: "series-blue-lock".into(),
            name: "Blue Lock v02".into(),
            number: 2.0,
            metadata: Some(KomgaBookMetadata {
                title: None,
                number: Some("2".into()),
                number_sort: Some(2.0),
            }),
            read_progress: Some(KomgaReadProgress {
                page: 192,
                completed: true,
                read_date: Some("2024-01-02T00:00:00Z".into()),
            }),
        },
        KomgaBook {
            id: "b3".into(),
            series_id: "series-blue-lock".into(),
            name: "Blue Lock v03".into(),
            number: 3.0,
            metadata: Some(KomgaBookMetadata {
                title: None,
                number: Some("3".into()),
                number_sort: Some(3.0),
            }),
            read_progress: Some(KomgaReadProgress {
                page: 50,
                completed: false,
                read_date: None,
            }),
        },
    ];

    let progress = evaluate_series_progress(&series, &books);
    assert_eq!(progress.total_books, 3);
    assert_eq!(progress.completed_books, 2);
    assert_eq!(progress.highest_completed_volume, Some(2.0));
    assert_eq!(progress.highest_completed_chapter, None);
    assert!(!progress.is_series_completed);
    assert_eq!(progress.last_read_date, Some("2024-01-02T00:00:00Z".into()));
}

#[test]
fn test_evaluate_chapter_series_completed() {
    let series = KomgaSeries {
        id: "series-sololeveling".into(),
        name: "Solo Leveling".into(),
        library_id: "lib-en".into(),
        books_count: 2,
    };

    let books = vec![
        KomgaBook {
            id: "b1".into(),
            series_id: "series-sololeveling".into(),
            name: "Solo Leveling c001".into(),
            number: 1.0,
            metadata: None,
            read_progress: Some(KomgaReadProgress {
                page: 25,
                completed: true,
                read_date: Some("2024-02-01T00:00:00Z".into()),
            }),
        },
        KomgaBook {
            id: "b2".into(),
            series_id: "series-sololeveling".into(),
            name: "Solo Leveling c002".into(),
            number: 2.0,
            metadata: None,
            read_progress: Some(KomgaReadProgress {
                page: 30,
                completed: true,
                read_date: Some("2024-02-02T00:00:00Z".into()),
            }),
        },
    ];

    let progress = evaluate_series_progress(&series, &books);
    assert_eq!(progress.total_books, 2);
    assert_eq!(progress.completed_books, 2);
    assert_eq!(progress.highest_completed_chapter, Some(2.0));
    assert!(progress.is_series_completed);
}

#[test]
fn test_tracker_mappings_persistence_and_resolution() {
    let dir = std::env::temp_dir().join(format!("mango-synctest-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Initially no mapping
    let initial = get_mapping(&conn, "komga-1", SyncTarget::AniList).unwrap();
    assert!(initial.is_none());

    // Save mapping
    let m = TrackerMapping {
        komga_series_id: "komga-1".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "12345".into(),
        tracker_title: Some("Chainsaw Man".into()),
        edition_id: None,
        mapping_source: super::model::MappingSource::Manual,
        confidence: super::model::MappingConfidence::Resolved,
        manual_override: true,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &m).unwrap();

    let fetched = get_mapping(&conn, "komga-1", SyncTarget::AniList)
        .unwrap()
        .unwrap();
    assert_eq!(fetched.tracker_id, "12345");
    assert!(fetched.manual_override);
    assert_eq!(fetched.mapping_source, super::model::MappingSource::Manual);

    let list = list_mappings(&conn, Some(SyncTarget::AniList)).unwrap();
    assert_eq!(list.len(), 1);

    // Resolution check
    let resolved = resolve_mapping(&conn, "komga-1", "Chainsaw Man", SyncTarget::AniList).unwrap();
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().tracker_id, "12345");

    // Deletion
    let deleted = delete_mapping(&conn, "komga-1", SyncTarget::AniList).unwrap();
    assert!(deleted);
    let after_delete = get_mapping(&conn, "komga-1", SyncTarget::AniList).unwrap();
    assert!(after_delete.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_anilist_sync_volume_series_syncs_volumes_dimension() {
    let dir = std::env::temp_dir().join(format!("mango-anilist-voltest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let trackers = crate::trackers::Trackers::new(db).unwrap();

    let volume_progress = super::model::KomgaSeriesProgress {
        series_id: "series-bluelock-vol".into(),
        series_title: "Blue Lock".into(),
        library_id: "lib-en".into(),
        total_books: 38,
        completed_books: 12,
        highest_completed_book: Some(12.0),
        highest_completed_volume: Some(12.0),
        highest_completed_chapter: None, // strictly volume based
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let mapping = TrackerMapping {
        komga_series_id: "series-bluelock-vol".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "106130".into(), // Blue Lock AniList ID
        tracker_title: Some("Blue Lock".into()),
        edition_id: None,
        mapping_source: super::model::MappingSource::ExactTitle,
        confidence: super::model::MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    // AniList is dimension-aware: volume series syncs volume dimension (not SkippedUnsafeProgress)
    let record =
        super::anilist::sync_series_to_anilist(&trackers, &volume_progress, Some(&mapping), true)
            .await;
    // Without credentials in test, it hits AuthError cleanly after validating dimension
    assert_eq!(record.dimension, Some("volume".to_string()));
    assert_eq!(record.category, super::model::SyncResultCategory::AuthError);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_anilist_sync_skips_when_no_mapping() {
    let dir = std::env::temp_dir().join(format!("mango-nomap-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let trackers = crate::trackers::Trackers::new(db).unwrap();

    let progress = super::model::KomgaSeriesProgress {
        series_id: "series-unknown".into(),
        series_title: "Unknown Series".into(),
        library_id: "lib-en".into(),
        total_books: 10,
        completed_books: 5,
        highest_completed_book: Some(5.0),
        highest_completed_volume: None,
        highest_completed_chapter: Some(5.0),
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let record = super::anilist::sync_series_to_anilist(&trackers, &progress, None, true).await;
    assert_eq!(
        record.category,
        super::model::SyncResultCategory::SkippedNoMapping
    );
    assert_eq!(record.dimension, Some("chapter".to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_sync_skips_untouched_series() {
    let dir = std::env::temp_dir().join(format!("mango-untouched-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let trackers = crate::trackers::Trackers::new(db).unwrap();

    let untouched = super::model::KomgaSeriesProgress {
        series_id: "series-untouched".into(),
        series_title: "Untouched Series".into(),
        library_id: "lib-en".into(),
        total_books: 10,
        completed_books: 0,
        highest_completed_book: None,
        highest_completed_volume: None,
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let mapping = TrackerMapping {
        komga_series_id: "series-untouched".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "99999".into(),
        tracker_title: Some("Untouched Series".into()),
        edition_id: None,
        mapping_source: super::model::MappingSource::Manual,
        confidence: super::model::MappingConfidence::Resolved,
        manual_override: false,
        last_validated: None,
        updated_at: 1700000000,
    };

    let record =
        super::anilist::sync_series_to_anilist(&trackers, &untouched, Some(&mapping), true).await;
    assert_eq!(
        record.category,
        super::model::SyncResultCategory::SkippedUntouched
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_sync_dry_run_and_auth() {
    let mb_client = crate::pipeline::mangabaka::MangaBakaClient::new(
        reqwest::Client::new(),
        None,
        Some("dummy-token".into()),
    );

    let progress = super::model::KomgaSeriesProgress {
        series_id: "series-vol".into(),
        series_title: "Sample Manga".into(),
        library_id: "lib-en".into(),
        total_books: 10,
        completed_books: 4,
        highest_completed_book: Some(4.0),
        highest_completed_volume: Some(4.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let mapping = TrackerMapping {
        komga_series_id: "series-vol".into(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: "mb-123".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: Some("col-standard".into()),
        mapping_source: super::model::MappingSource::ExactTitle,
        confidence: super::model::MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    // 1. Without token: AuthError
    let unauth_record = super::mangabaka::sync_series_to_mangabaka(
        &mb_client,
        false,
        &progress,
        Some(&mapping),
        true,
    )
    .await;
    assert_eq!(
        unauth_record.category,
        super::model::SyncResultCategory::AuthError
    );

    // 2. With token in Dry Run: WouldUpdate
    let dry_record = super::mangabaka::sync_series_to_mangabaka(
        &mb_client,
        true,
        &progress,
        Some(&mapping),
        true,
    )
    .await;
    assert_eq!(
        dry_record.category,
        super::model::SyncResultCategory::WouldUpdate
    );
    assert_eq!(dry_record.new_progress, Some(4.0));
    assert_eq!(dry_record.dimension, Some("volume".to_string()));
    assert!(dry_record.message.contains("Vol 4"));

    // 3. Live update: fails closed as Unsupported (MangaBaka REST API v1 has no user mutation endpoint)
    let live_record = super::mangabaka::sync_series_to_mangabaka(
        &mb_client,
        true,
        &progress,
        Some(&mapping),
        false,
    )
    .await;
    assert_eq!(
        live_record.category,
        super::model::SyncResultCategory::Unsupported
    );
}

#[test]
fn test_sync_scheduler_defaults_and_round_trip() {
    let dir = std::env::temp_dir().join(format!("mango-sched-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Defaults MUST be disabled
    let initial = super::scheduler::get_schedule_config(&conn);
    assert!(
        !initial.enabled,
        "Scheduled sync must strictly default to OFF"
    );
    assert_eq!(initial.interval_hours, 24);

    // Persist new config
    let updated = super::scheduler::SyncScheduleConfig {
        enabled: true,
        interval_hours: 12,
        last_run: None,
    };
    super::scheduler::set_schedule_config(&conn, &updated).unwrap();

    let fetched = super::scheduler::get_schedule_config(&conn);
    assert!(fetched.enabled);
    assert_eq!(fetched.interval_hours, 12);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_tracked_catalog_separation_and_auto_acquire_safety() {
    let dir = std::env::temp_dir().join(format!("mango-cat-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // 1. Initial tracking must NEVER auto-acquire (auto-acquire strictly defaults to false)
    let entry = super::catalog::add_or_update_tracked(
        &conn,
        "Dandadan",
        "mango",
        Some("komga-dandadan"),
        None,
    )
    .unwrap();

    assert!(entry.tracked);
    assert!(
        !entry.auto_acquire,
        "Tracking must NEVER imply auto-acquisition permission"
    );
    assert_eq!(entry.origins, vec!["mango"]);

    // 2. Add second origin (e.g. MangaBaka or AniList discovery)
    let updated = super::catalog::add_or_update_tracked(
        &conn,
        "Dandadan",
        "anilist",
        Some("komga-dandadan"),
        None,
    )
    .unwrap();

    assert_eq!(updated.id, entry.id);
    assert_eq!(updated.origins, vec!["mango", "anilist"]);
    assert!(
        !updated.auto_acquire,
        "Adding origins must preserve auto_acquire = false"
    );

    // 3. Explicitly toggle auto-acquire
    super::catalog::set_auto_acquire(&conn, entry.id, true).unwrap();
    let after_toggle = super::catalog::get_tracked_by_id(&conn, entry.id)
        .unwrap()
        .unwrap();
    assert!(after_toggle.auto_acquire);

    // 4. Verify unified status separation of concerns
    let status = super::catalog::UnifiedTitleStatus {
        title: "Dandadan".into(),
        komga_series_id: Some("komga-dandadan".into()),
        is_tracked: after_toggle.tracked,
        is_owned: true,
        owned_volumes_count: 8,
        is_acquiring: false,
        auto_acquire_enabled: after_toggle.auto_acquire,
        reading_progress: Some(8.0),
        origins: after_toggle.origins,
    };

    assert!(status.is_tracked);
    assert!(status.is_owned);
    assert!(!status.is_acquiring);
    assert!(status.auto_acquire_enabled);
    assert_eq!(status.reading_progress, Some(8.0));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sync_eligibility_partial_vs_completed_book() {
    use super::eligibility::{SyncEligibility, evaluate_eligibility};

    // 1. Completely untouched series (0 books, no progress) -> Untouched
    let untouched = super::model::KomgaSeriesProgress {
        series_id: "s1".into(),
        series_title: "Untouched".into(),
        library_id: "lib".into(),
        total_books: 10,
        completed_books: 0,
        highest_completed_book: None,
        highest_completed_volume: None,
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };
    assert_eq!(evaluate_eligibility(&untouched), SyncEligibility::Untouched);

    // 2. Partial book read (page 50 of 200, completed = false, highest_completed = None) -> Untouched
    let partial = super::model::KomgaSeriesProgress {
        series_id: "s2".into(),
        series_title: "Partial".into(),
        library_id: "lib".into(),
        total_books: 10,
        completed_books: 0,
        highest_completed_book: None,
        highest_completed_volume: None,
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };
    assert_eq!(evaluate_eligibility(&partial), SyncEligibility::Untouched);

    // 3. One completed book (e.g. Vol 1 finished) -> Eligible
    let eligible = super::model::KomgaSeriesProgress {
        series_id: "s3".into(),
        series_title: "Eligible".into(),
        library_id: "lib".into(),
        total_books: 10,
        completed_books: 1,
        highest_completed_book: Some(1.0),
        highest_completed_volume: Some(1.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: Some("2024-03-01T00:00:00Z".into()),
        books: vec![],
    };
    assert_eq!(evaluate_eligibility(&eligible), SyncEligibility::Eligible);
}

#[tokio::test]
async fn test_manual_mapping_override_and_diagnostics() {
    let dir = std::env::temp_dir().join(format!("mango-diag-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // 1. Save manual mapping override
    let manual = super::mapping::set_manual_override(
        &conn,
        "komga-blue-lock",
        SyncTarget::AniList,
        "106130",
        None,
        Some("Blue Lock"),
    )
    .unwrap();
    assert!(manual.manual_override);
    assert_eq!(manual.mapping_source, super::model::MappingSource::Manual);
    assert_eq!(manual.confidence, super::model::MappingConfidence::Resolved);

    // 2. Manual override is fetched and takes precedence
    let fetched = super::mapping::resolve_mapping(
        &conn,
        "komga-blue-lock",
        "Blue Lock (Other)",
        SyncTarget::AniList,
    )
    .unwrap()
    .unwrap();
    assert_eq!(fetched.tracker_id, "106130");
    assert!(fetched.manual_override);

    // 3. Mark an ambiguous mapping
    let ambig = TrackerMapping {
        komga_series_id: "komga-ambig".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "".into(),
        tracker_title: Some("Ambiguous Manga".into()),
        edition_id: None,
        mapping_source: super::model::MappingSource::ExactTitle,
        confidence: super::model::MappingConfidence::Ambiguous,
        manual_override: false,
        last_validated: None,
        updated_at: 1700000000,
    };
    super::mapping::save_mapping(&conn, &ambig).unwrap();

    let unres = super::mapping::list_unresolved_mappings(&conn).unwrap();
    assert_eq!(unres.len(), 1);
    assert_eq!(unres[0].komga_series_id, "komga-ambig");

    // Diagnostic generation
    let diag = super::identity::get_title_diagnostic(
        Some(&unres[0]),
        "komga-ambig",
        "Ambiguous Manga",
        SyncTarget::AniList,
    )
    .unwrap();
    assert_eq!(diag.confidence, super::model::MappingConfidence::Ambiguous);

    // 4. Reset / clear manual override
    super::mapping::clear_manual_override(&conn, "komga-blue-lock", SyncTarget::AniList).unwrap();
    let after_clear =
        super::mapping::get_mapping(&conn, "komga-blue-lock", SyncTarget::AniList).unwrap();
    assert!(after_clear.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
