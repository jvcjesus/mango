use super::{
    identity::resolve_mangabaka_identity,
    komga_reader::{calculate_contiguous_progress, evaluate_series_progress, parse_chapter_number},
    mapping::{
        delete_mapping, get_mapping, list_mappings, resolve_mapping, save_mapping,
        set_manual_override,
    },
    model::{MappingConfidence, MappingSource, SyncResultCategory, SyncTarget, TrackerMapping},
};
use crate::{
    db::Db,
    pipeline::{
        komga::{KomgaBook, KomgaBookMetadata, KomgaReadProgress, KomgaSeries},
        mangabaka::MangaBakaClient,
    },
};
use axum::{
    Json, Router,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
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

#[derive(Clone)]
struct MockMangaBakaState {
    library: Arc<Mutex<std::collections::HashMap<i64, (Option<f64>, Option<f64>, String)>>>,
    patch_calls: Arc<AtomicUsize>,
    post_calls: Arc<AtomicUsize>,
    last_patch_body: Arc<Mutex<Option<serde_json::Value>>>,
}

async fn start_mock_mangabaka(
    initial_entries: Vec<(i64, Option<f64>, Option<f64>, String)>,
) -> (String, MockMangaBakaState) {
    let mut map = std::collections::HashMap::new();
    for (id, vol, ch, st) in initial_entries {
        map.insert(id, (vol, ch, st));
    }
    let state = MockMangaBakaState {
        library: Arc::new(Mutex::new(map)),
        patch_calls: Arc::new(AtomicUsize::new(0)),
        post_calls: Arc::new(AtomicUsize::new(0)),
        last_patch_body: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route(
            "/source/anilist/{id}",
            get(|Path(id): Path<String>| async move {
                if id == "101" {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "data": {
                                "id": 202,
                                "title": "Berserk"
                            }
                        })),
                    )
                        .into_response()
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }),
        )
        .route(
            "/my/library/{id}",
            get({
                let state = state.clone();
                move |headers: HeaderMap, Path(id): Path<String>| {
                    let state = state.clone();
                    async move {
                        let token = headers.get("x-api-key").and_then(|v| v.to_str().ok());
                        if token == Some("invalid-token") {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        if id == "404" || id == "999" {
                            return StatusCode::NOT_FOUND.into_response();
                        }
                        if id == "502" {
                            return StatusCode::BAD_GATEWAY.into_response();
                        }
                        let num_id = id.parse::<i64>().unwrap_or(0);
                        let lib = state.library.lock().unwrap();
                        if let Some((vol, ch, st)) = lib.get(&num_id) {
                            (
                                StatusCode::OK,
                                Json(serde_json::json!({
                                    "data": {
                                        "id": num_id,
                                        "series_id": num_id,
                                        "progress_volume": vol,
                                        "progress_chapter": ch,
                                        "status": st
                                    }
                                })),
                            )
                                .into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    }
                }
            })
            .patch({
                let state = state.clone();
                move |headers: HeaderMap,
                      Path(id): Path<String>,
                      Json(body): Json<serde_json::Value>| {
                    let state = state.clone();
                    async move {
                        state.patch_calls.fetch_add(1, Ordering::SeqCst);
                        *state.last_patch_body.lock().unwrap() = Some(body.clone());

                        let token = headers.get("x-api-key").and_then(|v| v.to_str().ok());
                        if token == Some("invalid-token") {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        if id == "502" || id == "503" {
                            return StatusCode::BAD_GATEWAY.into_response();
                        }
                        if id == "777" {
                            // Simulation of verification mismatch: return OK without updating state.library
                            return StatusCode::OK.into_response();
                        }
                        if id == "888" {
                            // Simulation of disappeared entry: remove from library
                            state.library.lock().unwrap().remove(&888);
                            return StatusCode::OK.into_response();
                        }

                        let num_id = id.parse::<i64>().unwrap_or(0);
                        let vol = body.get("progress_volume").and_then(|v| v.as_f64());
                        let ch = body.get("progress_chapter").and_then(|v| v.as_f64());

                        let mut lib = state.library.lock().unwrap();
                        if let Some(entry) = lib.get_mut(&num_id) {
                            if vol.is_some() {
                                entry.0 = vol;
                            }
                            if ch.is_some() {
                                entry.1 = ch;
                            }
                        } else {
                            lib.insert(num_id, (vol, ch, "CURRENT".to_string()));
                        }
                        StatusCode::OK.into_response()
                    }
                }
            })
            .post({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move {
                        state.post_calls.fetch_add(1, Ordering::SeqCst);
                        StatusCode::METHOD_NOT_ALLOWED.into_response()
                    }
                }
            }),
        )
        .route(
            "/series/search",
            get(
                |axum::extract::Query(params): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| async move {
                    let q = params.get("q").map(|s| s.as_str()).unwrap_or("");
                    if q == "Berserk" {
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "data": [{ "id": 202, "title": "Berserk" }]
                            })),
                        )
                            .into_response()
                    } else if q == "Ambiguous Title" {
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "data": [
                                    { "id": 1, "title": "Ambiguous Title 1" },
                                    { "id": 2, "title": "Ambiguous Title 2" }
                                ]
                            })),
                        )
                            .into_response()
                    } else {
                        (StatusCode::OK, Json(serde_json::json!({ "data": [] }))).into_response()
                    }
                },
            ),
        )
        .route(
            "/series/{id}/collections",
            get(|Path(_id): Path<String>| async move {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "data": [{
                            "id": "col-std",
                            "edition": "Standard Edition",
                            "language": "en"
                        }]
                    })),
                )
                    .into_response()
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{port}"), state)
}

#[tokio::test]
async fn test_mangabaka_cross_reference_success_and_not_found() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    // 1. Success cross-reference
    let xref = client.resolve_from_anilist(101).await.unwrap();
    assert!(xref.is_some());
    let x = xref.unwrap();
    assert_eq!(x.mangabaka_id, 202);
    assert_eq!(x.title.as_deref(), Some("Berserk"));

    // 2. Not found cross-reference
    let not_found = client.resolve_from_anilist(99999).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_mangabaka_identity_precedence_and_cross_ref() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-id-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Tier 1: Manual override takes precedence over everything
    set_manual_override(
        &conn,
        "series-manual",
        SyncTarget::MangaBaka,
        "777",
        None,
        Some("Manual Title"),
    )
    .unwrap();
    let res1 = resolve_mangabaka_identity(&conn, &client, "series-manual", "Berserk")
        .await
        .unwrap();
    assert!(res1.is_some());
    let m1 = res1.unwrap();
    assert_eq!(m1.tracker_id, "777");
    assert!(m1.manual_override);

    // Tier 2: Persisted resolved cross-reference mapping is preserved
    let persisted_xref = TrackerMapping {
        komga_series_id: "series-persisted".into(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: "202".into(),
        tracker_title: Some("Berserk".into()),
        edition_id: None,
        mapping_source: MappingSource::CrossReference,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &persisted_xref).unwrap();
    let res2 = resolve_mangabaka_identity(&conn, &client, "series-persisted", "Berserk")
        .await
        .unwrap();
    assert_eq!(res2.unwrap().mapping_source, MappingSource::CrossReference);

    // Tier 3: Automatic cross-reference lookup via known AniList mapping
    let anilist_map = TrackerMapping {
        komga_series_id: "series-auto-xref".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "101".into(),
        tracker_title: Some("Berserk".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();
    let res3 = resolve_mangabaka_identity(
        &conn,
        &client,
        "series-auto-xref",
        "Different Title In Komga",
    )
    .await
    .unwrap();
    assert!(res3.is_some());
    let m3 = res3.unwrap();
    assert_eq!(m3.tracker_id, "202");
    assert_eq!(m3.mapping_source, MappingSource::CrossReference);

    // Tier 4: Exact title matcher fallback when no AniList mapping exists
    let res4 = resolve_mangabaka_identity(&conn, &client, "series-title-only", "Berserk")
        .await
        .unwrap();
    assert!(res4.is_some());
    let m4 = res4.unwrap();
    assert_eq!(m4.tracker_id, "202");
    assert_eq!(m4.mapping_source, MappingSource::ExactTitle);

    // Tier 5: Ambiguous series fails closed
    let res5 = resolve_mangabaka_identity(&conn, &client, "series-ambig", "Ambiguous Title")
        .await
        .unwrap();
    assert!(res5.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_get_volume_and_chapter_progress() {
    let (url, _) =
        start_mock_mangabaka(vec![(202, Some(4.0), Some(35.0), "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let entry = client.get_my_library_entry(202).await.unwrap();
    assert!(entry.is_some());
    let e = entry.unwrap();
    assert_eq!(e.progress_volume, Some(4.0));
    assert_eq!(e.progress_chapter, Some(35.0));
    assert_eq!(e.status.as_deref(), Some("CURRENT"));
}

#[tokio::test]
async fn test_mangabaka_absent_from_library_skips_without_auto_add() {
    let (url, state) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "404".into(), // Not in library
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::NotInTrackerLibrary);
    assert!(
        record
            .message
            .contains("skipping without automatically adding")
    );
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.post_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_mangabaka_volume_equal_no_change() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, Some(4.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "202".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::NoChange);
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_mangabaka_tracker_ahead_preserves_progress() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, Some(10.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "202".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::TrackerAhead);
    assert!(record.message.contains("keeping current progress"));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_mangabaka_dry_run_volume_zero_patches() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, Some(2.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "202".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), true)
            .await;
    assert_eq!(record.category, SyncResultCategory::WouldUpdate);
    assert_eq!(record.new_progress, Some(4.0));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_mangabaka_dry_run_chapter_zero_patches() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, None, Some(10.0), "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let progress = super::model::KomgaSeriesProgress {
        series_id: "series-ch".into(),
        series_title: "Solo Leveling".into(),
        library_id: "lib-en".into(),
        total_books: 50,
        completed_books: 25,
        highest_completed_book: Some(25.0),
        highest_completed_volume: None,
        highest_completed_chapter: Some(25.0),
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let mapping = TrackerMapping {
        komga_series_id: "series-ch".into(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: "202".into(),
        tracker_title: Some("Solo Leveling".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), true)
            .await;
    assert_eq!(record.category, SyncResultCategory::WouldUpdate);
    assert_eq!(record.new_progress, Some(25.0));
    assert_eq!(record.dimension, Some("chapter".to_string()));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_mangabaka_live_volume_patch_and_verification() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, Some(2.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "202".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::Updated);
    assert_eq!(record.new_progress, Some(4.0));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 1);

    // Verify dimension separation: only progress_volume was sent
    let last_body = state.last_patch_body.lock().unwrap().clone().unwrap();
    assert_eq!(
        last_body.get("progress_volume"),
        Some(&serde_json::json!(4.0))
    );
    assert!(last_body.get("progress_chapter").is_none());

    // Verify stored state in mock server
    let lib = state.library.lock().unwrap();
    assert_eq!(lib.get(&202).unwrap().0, Some(4.0));
}

#[tokio::test]
async fn test_mangabaka_live_chapter_patch_and_verification() {
    let (url, state) =
        start_mock_mangabaka(vec![(202, None, Some(10.0), "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let progress = super::model::KomgaSeriesProgress {
        series_id: "series-ch".into(),
        series_title: "Solo Leveling".into(),
        library_id: "lib-en".into(),
        total_books: 50,
        completed_books: 25,
        highest_completed_book: Some(25.0),
        highest_completed_volume: None,
        highest_completed_chapter: Some(25.0),
        is_series_completed: false,
        last_read_date: None,
        books: vec![],
    };

    let mapping = TrackerMapping {
        komga_series_id: "series-ch".into(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: "202".into(),
        tracker_title: Some("Solo Leveling".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::Updated);
    assert_eq!(record.new_progress, Some(25.0));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 1);

    // Verify dimension separation: only progress_chapter was sent
    let last_body = state.last_patch_body.lock().unwrap().clone().unwrap();
    assert_eq!(
        last_body.get("progress_chapter"),
        Some(&serde_json::json!(25.0))
    );
    assert!(last_body.get("progress_volume").is_none());

    // Verify stored state in mock server
    let lib = state.library.lock().unwrap();
    assert_eq!(lib.get(&202).unwrap().1, Some(25.0));
}

#[tokio::test]
async fn test_mangabaka_post_update_verification_mismatch() {
    let (url, state) =
        start_mock_mangabaka(vec![(777, Some(2.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "777".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::VerificationFailed);
    assert!(record.message.contains("Post-update verification mismatch"));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_mangabaka_post_update_verification_entry_disappeared() {
    let (url, state) =
        start_mock_mangabaka(vec![(888, Some(2.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "888".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::VerificationFailed);
    assert!(record.message.contains("disappeared after update"));
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_mangabaka_patch_network_error() {
    let (url, state) =
        start_mock_mangabaka(vec![(503, Some(2.0), None, "CURRENT".to_string())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
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
        tracker_id: "503".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };

    let record =
        super::mangabaka::sync_series_to_mangabaka(&client, true, &progress, Some(&mapping), false)
            .await;
    assert_eq!(record.category, SyncResultCategory::NetworkError);
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_mangabaka_auth_failure_reported() {
    let (url, _) = start_mock_mangabaka(vec![(202, Some(2.0), None, "CURRENT".to_string())]).await;

    // 1. Client has no token configured
    let client_no_token = MangaBakaClient::new(reqwest::Client::new(), Some(url.clone()), None);
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
        tracker_id: "202".into(),
        tracker_title: Some("Sample Manga".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    let rec1 = super::mangabaka::sync_series_to_mangabaka(
        &client_no_token,
        false,
        &progress,
        Some(&mapping),
        true,
    )
    .await;
    assert_eq!(rec1.category, SyncResultCategory::AuthError);
    assert_eq!(rec1.message, "MangaBaka API token is not configured");

    // 2. Client token is rejected (returns 401)
    let client_bad_token = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("invalid-token".into()),
    );
    let rec2 = super::mangabaka::sync_series_to_mangabaka(
        &client_bad_token,
        true,
        &progress,
        Some(&mapping),
        false,
    )
    .await;
    assert_eq!(rec2.category, SyncResultCategory::AuthError);
    assert!(
        rec2.message.contains("rejected")
            || rec2.message.contains("invalid")
            || rec2.message.contains("failed")
    );
}

#[test]
fn test_contiguous_volume_progress_gap_and_order() {
    // 1. Uninterrupted: 1, 2, 3 complete, 4 incomplete, 5 complete -> safe progress is 3.0
    let releases1 = vec![
        (1.0, true),
        (2.0, true),
        (3.0, true),
        (4.0, false),
        (5.0, true),
    ];
    assert_eq!(calculate_contiguous_progress(&releases1), Some(3.0));

    // 2. Missing release (gap): 1, 2, 3 complete, 5 complete (4 missing) -> safe progress is 3.0
    let releases2 = vec![(1.0, true), (2.0, true), (3.0, true), (5.0, true)];
    assert_eq!(calculate_contiguous_progress(&releases2), Some(3.0));

    // 3. Must start at 1.0: only 2 and 3 complete -> None
    let releases3 = vec![(2.0, true), (3.0, true)];
    assert_eq!(calculate_contiguous_progress(&releases3), None);

    // 4. Multiple entries for same release: all must be complete
    let releases4 = vec![(1.0, true), (1.0, false), (2.0, true)];
    assert_eq!(calculate_contiguous_progress(&releases4), None);

    // 5. Empty
    assert_eq!(calculate_contiguous_progress(&[]), None);
}

#[test]
fn test_contiguous_chapter_prologue_behavior() {
    // 1. Incomplete Chapter 0 does NOT block Chapter 1 streak
    let ch1 = vec![(0.0, false), (1.0, true), (2.0, true), (3.0, true)];
    assert_eq!(calculate_contiguous_progress(&ch1), Some(3.0));

    // 2. Completed Chapter 0 does NOT shift Chapter 1-based progress
    let ch2 = vec![(0.0, true), (1.0, true), (2.0, true)];
    assert_eq!(calculate_contiguous_progress(&ch2), Some(2.0));

    // 3. Completed Chapter 0 with incomplete Chapter 1 reports Some(0.0)
    let ch3 = vec![(0.0, true), (1.0, false)];
    assert_eq!(calculate_contiguous_progress(&ch3), Some(0.0));

    // 4. Incomplete Chapter 0 with incomplete Chapter 1 reports None
    let ch4 = vec![(0.0, false), (1.0, false)];
    assert_eq!(calculate_contiguous_progress(&ch4), None);

    // 5. Fractional sub-chapters: 1, 1.5, 2, 3 complete -> 3.0
    let ch5 = vec![(1.0, true), (1.5, true), (2.0, true), (3.0, true)];
    assert_eq!(calculate_contiguous_progress(&ch5), Some(3.0));
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
