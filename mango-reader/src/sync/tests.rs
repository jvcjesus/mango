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
        manual_override: true,
        updated_at: 1700000000,
    };
    save_mapping(&conn, &m).unwrap();

    let fetched = get_mapping(&conn, "komga-1", SyncTarget::AniList).unwrap().unwrap();
    assert_eq!(fetched.tracker_id, "12345");
    assert!(fetched.manual_override);

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
async fn test_anilist_sync_skips_volume_only_series_safely() {
    let dir = std::env::temp_dir().join(format!("mango-anilist-test-{}", std::process::id()));
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
        manual_override: false,
        updated_at: 1700000000,
    };

    // Even in dry-run or live, volume-only series MUST fail closed for chapter-based AniList!
    let record = super::anilist::sync_series_to_anilist(&trackers, &volume_progress, Some(&mapping), true).await;
    assert_eq!(record.category, super::model::SyncResultCategory::SkippedUnsafeProgress);
    assert!(record.message.contains("volume-based (Vol 12)"));
    assert!(record.message.contains("cannot safely derive chapter progress"));

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
    assert_eq!(record.category, super::model::SyncResultCategory::SkippedNoMapping);

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
        manual_override: false,
        updated_at: 1700000000,
    };

    // 1. Without token: AuthError
    let unauth_record = super::mangabaka::sync_series_to_mangabaka(&mb_client, false, &progress, Some(&mapping), true).await;
    assert_eq!(unauth_record.category, super::model::SyncResultCategory::AuthError);

    // 2. With token: DryRunWouldUpdate
    let dry_record = super::mangabaka::sync_series_to_mangabaka(&mb_client, true, &progress, Some(&mapping), true).await;
    assert_eq!(dry_record.category, super::model::SyncResultCategory::DryRunWouldUpdate);
    assert_eq!(dry_record.new_progress, Some(4.0));
    assert!(dry_record.message.contains("Vol 4"));
}
