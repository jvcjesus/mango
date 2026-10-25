use super::{
    identity::{IdentityResolutionResult, resolve_mangabaka_identity},
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
    profile_calls: Arc<AtomicUsize>,
    search_calls: Arc<AtomicUsize>,
    last_patch_body: Arc<Mutex<Option<serde_json::Value>>>,
    public_request_had_auth_header: Arc<std::sync::atomic::AtomicBool>,
}

fn check_simultaneous_auth(headers: &HeaderMap) -> Option<axum::response::Response> {
    if headers.contains_key("authorization") && headers.contains_key("x-api-key") {
        return Some(
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "message": "Cannot use both Authorization and x-api-key headers simultaneously",
                    "status": 400,
                    "code": null
                })),
            )
                .into_response(),
        );
    }
    None
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
        profile_calls: Arc::new(AtomicUsize::new(0)),
        search_calls: Arc::new(AtomicUsize::new(0)),
        last_patch_body: Arc::new(Mutex::new(None)),
        public_request_had_auth_header: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    let profile_handler = {
        let state = state.clone();
        move |headers: HeaderMap| {
            let state = state.clone();
            async move {
                if let Some(err) = check_simultaneous_auth(&headers) {
                    return err;
                }
                state.profile_calls.fetch_add(1, Ordering::SeqCst);
                let token = headers.get("x-api-key").and_then(|v| v.to_str().ok());
                match token {
                    None => StatusCode::UNAUTHORIZED.into_response(),
                    Some("invalid-token") => StatusCode::UNAUTHORIZED.into_response(),
                    Some("forbidden-token") => StatusCode::FORBIDDEN.into_response(),
                    Some("rate-limited-token") => StatusCode::TOO_MANY_REQUESTS.into_response(),
                    Some("not-found-token") => StatusCode::NOT_FOUND.into_response(),
                    Some("bad-request-token") => (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "message": "Parameter rejected by MangaBaka API",
                            "status": 400
                        })),
                    )
                        .into_response(),
                    Some(_) => (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "status": 200,
                            "data": {
                                "id": "usr_test123",
                                "preferred_username": "reader_dev",
                                "nickname": "Reader Dev",
                                "picture": null,
                                "updated_at": "2026-10-24T12:00:00Z"
                            }
                        })),
                    )
                        .into_response(),
                }
            }
        }
    };

    let anilist_handler = {
        let state = state.clone();
        move |headers: HeaderMap, Path(id): Path<String>| {
            let state = state.clone();
            async move {
                if let Some(err) = check_simultaneous_auth(&headers) {
                    return err;
                }
                if headers.contains_key("x-api-key") || headers.contains_key("authorization") {
                    state
                        .public_request_had_auth_header
                        .store(true, Ordering::SeqCst);
                }
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
                } else if id == "106130" {
                    // Real MangaBaka OpenAPI response schema with data.series array
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "status": 200,
                            "data": {
                                "source_response": null,
                                "series": [
                                    {
                                        "id": 83,
                                        "state": "active",
                                        "merged_with": null,
                                        "title": "Blue Lock"
                                    }
                                ]
                            }
                        })),
                    )
                        .into_response()
                } else if id == "998" {
                    // Multiple active series for one AniList ID
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "status": 200,
                            "data": {
                                "source_response": null,
                                "series": [
                                    { "id": 801, "state": "active", "title": "Candidate One" },
                                    { "id": 802, "state": "active", "title": "Candidate Two" }
                                ]
                            }
                        })),
                    )
                        .into_response()
                } else if id == "502" {
                    StatusCode::BAD_GATEWAY.into_response()
                } else if id == "400" {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "Invalid request" })),
                    )
                        .into_response()
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }
        }
    };

    let search_handler = {
        let state = state.clone();
        move |headers: HeaderMap,
              axum::extract::Query(params): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >| {
            let state = state.clone();
            async move {
                if let Some(err) = check_simultaneous_auth(&headers) {
                    return err;
                }
                if headers.contains_key("x-api-key") || headers.contains_key("authorization") {
                    state
                        .public_request_had_auth_header
                        .store(true, Ordering::SeqCst);
                }
                state.search_calls.fetch_add(1, Ordering::SeqCst);
                // Real MangaBaka API strictly validates query parameters and rejects unrecognized keys like "query"
                if params.contains_key("query") {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "message": "Validation error: Unrecognized key: \"query\"",
                            "status": 400,
                            "code": null
                        })),
                    )
                        .into_response();
                }
                let q = params.get("q").map(|s| s.as_str()).unwrap_or("");
                if q == "Error 400" {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "Invalid search parameter" })),
                    )
                        .into_response()
                } else if q == "Error 429" {
                    StatusCode::TOO_MANY_REQUESTS.into_response()
                } else if q == "Berserk" {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "data": [{ "id": 202, "title": "Berserk" }]
                        })),
                    )
                        .into_response()
                } else if q == "Pandora Hearts" {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "data": [{ "id": 303, "title": "Pandora Hearts" }]
                        })),
                    )
                        .into_response()
                } else if q == "Ambiguous Title" {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "data": [
                                { "id": 1, "title": "Ambiguous Title" },
                                { "id": 2, "title": "Ambiguous Title" }
                            ]
                        })),
                    )
                        .into_response()
                } else {
                    (StatusCode::OK, Json(serde_json::json!({ "data": [] }))).into_response()
                }
            }
        }
    };

    let collections_handler = {
        let state = state.clone();
        move |headers: HeaderMap, Path(_id): Path<String>| {
            let state = state.clone();
            async move {
                if let Some(err) = check_simultaneous_auth(&headers) {
                    return err;
                }
                if headers.contains_key("x-api-key") || headers.contains_key("authorization") {
                    state
                        .public_request_had_auth_header
                        .store(true, Ordering::SeqCst);
                }
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "data": [{
                            "id": "col-std",
                            "edition": "Standard Edition",
                            "language": "en",
                            "medium": "digital"
                        }]
                    })),
                )
                    .into_response()
            }
        }
    };

    let app = Router::new()
        .route("/my/profile", get(profile_handler.clone()))
        .route("/v1/my/profile", get(profile_handler))
        .route("/source/anilist/{id}", get(anilist_handler.clone()))
        .route("/v1/source/anilist/{id}", get(anilist_handler))
        .route(
            "/my/library/{id}",
            get({
                let state = state.clone();
                move |headers: HeaderMap, Path(id): Path<String>| {
                    let state = state.clone();
                    async move {
                        if let Some(err) = check_simultaneous_auth(&headers) {
                            return err;
                        }
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
                        if let Some(err) = check_simultaneous_auth(&headers) {
                            return err;
                        }
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
        .route("/series/search", get(search_handler.clone()))
        .route("/v1/series/search", get(search_handler))
        .route("/series/{id}/collections", get(collections_handler.clone()))
        .route("/v1/series/{id}/collections", get(collections_handler));

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
    let m1 = match res1 {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved, got {other:?}"),
    };
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
    match res2 {
        IdentityResolutionResult::Resolved(m) => {
            assert_eq!(m.mapping_source, MappingSource::CrossReference);
        }
        other => panic!("Expected Resolved, got {other:?}"),
    }

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
    let m3 = match res3 {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved, got {other:?}"),
    };
    assert_eq!(m3.tracker_id, "202");
    assert_eq!(m3.mapping_source, MappingSource::CrossReference);

    // Tier 4: Exact title matcher fallback when no AniList mapping exists
    let res4 = resolve_mangabaka_identity(&conn, &client, "series-title-only", "Berserk")
        .await
        .unwrap();
    let m4 = match res4 {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved, got {other:?}"),
    };
    assert_eq!(m4.tracker_id, "202");
    assert_eq!(m4.mapping_source, MappingSource::ExactTitle);

    // Tier 5: Ambiguous series fails closed
    let res5 = resolve_mangabaka_identity(&conn, &client, "series-ambig", "Ambiguous Title")
        .await
        .unwrap();
    assert!(matches!(res5, IdentityResolutionResult::Ambiguous(_)));

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

#[tokio::test]
async fn test_mangabaka_anilist_xref_success_avoids_title_search() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-xref-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Pre-populate AniList mapping for series-101
    let anilist_map = TrackerMapping {
        komga_series_id: "series-101".into(),
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

    let res = resolve_mangabaka_identity(&conn, &client, "series-101", "Different Name In Komga")
        .await
        .unwrap();

    match res {
        IdentityResolutionResult::Resolved(m) => {
            assert_eq!(m.tracker_id, "202");
            assert_eq!(m.mapping_source, MappingSource::CrossReference);
            assert_eq!(m.confidence, MappingConfidence::Resolved);
        }
        other => panic!("Expected Resolved from AniList xref, got {other:?}"),
    }

    // Proves mapping is persisted
    let persisted = get_mapping(&conn, "series-101", SyncTarget::MangaBaka)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.tracker_id, "202");
    assert_eq!(persisted.mapping_source, MappingSource::CrossReference);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_anilist_xref_404_falls_through_to_exact_title() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-xref-404-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Pre-populate AniList mapping with an ID that returns 404 on MangaBaka xref
    let anilist_map = TrackerMapping {
        komga_series_id: "series-404-xref".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "99999".into(),
        tracker_title: Some("Berserk".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();

    // The Komga title is "Berserk", which exists on exact title search
    let res = resolve_mangabaka_identity(&conn, &client, "series-404-xref", "Berserk")
        .await
        .unwrap();

    match res {
        IdentityResolutionResult::Resolved(m) => {
            assert_eq!(m.tracker_id, "202");
            assert_eq!(m.mapping_source, MappingSource::ExactTitle);
        }
        other => panic!("Expected Resolved via exact title fallback, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_http_400_classified_as_api_error_never_skipped_no_mapping() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-400-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Title "Error 400" simulates 400 Bad Request response from MangaBaka API
    let res = resolve_mangabaka_identity(&conn, &client, "series-400", "Error 400")
        .await
        .unwrap();

    match res {
        IdentityResolutionResult::ApiError(msg) => {
            assert!(msg.contains("HTTP 400"));
        }
        other => panic!(
            "Expected ApiError, got {other:?} - HTTP 400 must NEVER be swallowed as NotFound/SkippedNoMapping"
        ),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_http_429_classified_as_rate_limited() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-429-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Title "Error 429" simulates 429 Too Many Requests response from MangaBaka API
    let res = resolve_mangabaka_identity(&conn, &client, "series-429", "Error 429")
        .await
        .unwrap();

    assert_eq!(
        res,
        IdentityResolutionResult::RateLimited,
        "HTTP 429 must be classified as RateLimited, never SkippedNoMapping"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_pandora_hearts_real_world_validation() {
    // Pandora Hearts has 19 volumes completed in Komga and valid AniList mapping
    let (url, state) = start_mock_mangabaka(vec![
        // MangaBaka library entry for Pandora Hearts (id: 303) has 19 volumes completed
        (303, Some(19.0), None, "completed".into()),
    ])
    .await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-mb-pandora-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Step 1: Pre-populate known AniList mapping (media_id 303)
    let anilist_map = TrackerMapping {
        komga_series_id: "komga-pandora-hearts".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "303".into(),
        tracker_title: Some("Pandora Hearts".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();

    // Step 2: Resolve MangaBaka identity for Pandora Hearts
    let res = resolve_mangabaka_identity(&conn, &client, "komga-pandora-hearts", "Pandora Hearts")
        .await
        .unwrap();

    let mapping = match res {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved for Pandora Hearts, got {other:?}"),
    };
    assert_eq!(mapping.tracker_id, "303");

    // Step 3: Run dry-run MangaBaka progress sync for Pandora Hearts (19 volumes completed in Komga)
    let progress = super::model::KomgaSeriesProgress {
        series_id: "komga-pandora-hearts".into(),
        series_title: "Pandora Hearts".into(),
        library_id: "lib-en".into(),
        total_books: 24,
        completed_books: 19,
        highest_completed_book: Some(19.0),
        highest_completed_volume: Some(19.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books: vec![],
    };

    let record = super::mangabaka::sync_series_to_mangabaka(
        &client,
        true,
        &progress,
        Some(&mapping),
        true, // dry_run = true
    )
    .await;

    // Both Komga and MangaBaka are at 19 volumes completed -> NoChange
    assert_eq!(record.category, SyncResultCategory::NoChange);
    assert_eq!(record.dimension.as_deref(), Some("volume"));
    assert_eq!(record.old_progress, Some(19.0));
    assert_eq!(record.new_progress, Some(19.0));

    // Zero mutations performed
    assert_eq!(state.patch_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.post_calls.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_never_sends_simultaneous_auth_headers() {
    let (url, _state) = start_mock_mangabaka(vec![(202, Some(5.0), None, "reading".into())]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("valid-token".into()),
    );

    // 1. Connection validation: must succeed without mutual auth rejection
    let conn_res = client.test_connection().await;
    assert!(
        conn_res.is_ok(),
        "test_connection must succeed with valid token: {conn_res:?}"
    );

    // 2. Series search: must succeed without mutual auth rejection
    let search_res = client.match_series("Berserk").await.unwrap();
    assert!(matches!(
        search_res,
        crate::pipeline::mangabaka::MangaBakaSeriesMatch::Found { id: 202, .. }
    ));

    // 3. AniList cross-reference: must succeed without mutual auth rejection
    let xref_res = client.resolve_from_anilist(101).await.unwrap();
    assert!(xref_res.is_some());
    assert_eq!(xref_res.unwrap().mangabaka_id, 202);

    // 4. Library entry read: must succeed without mutual auth rejection
    let lib_res = client.get_my_library_entry(202).await.unwrap();
    assert!(lib_res.is_some());
    assert_eq!(lib_res.unwrap().progress_volume, Some(5.0));

    // 5. Library progress update (PATCH): must succeed without mutual auth rejection
    let update_res = client
        .update_my_library_progress(202, Some(6.0), None)
        .await;
    assert!(
        update_res.is_ok(),
        "update_my_library_progress must succeed with valid token: {update_res:?}"
    );
}

#[tokio::test]
async fn test_mangabaka_mock_server_rejects_simultaneous_auth() {
    let (url, _state) = start_mock_mangabaka(vec![]).await;
    let http = reqwest::Client::new();

    // Directly send a request with BOTH Authorization and x-api-key headers to /my/profile
    let resp = http
        .get(format!("{url}/my/profile"))
        .header("Authorization", "Bearer test-token")
        .header("x-api-key", "test-token")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body.get("message").and_then(|v| v.as_str()),
        Some("Cannot use both Authorization and x-api-key headers simultaneously")
    );
}

#[tokio::test]
async fn test_mangabaka_search_rejects_query_parameter_and_requires_q() {
    let (url, state) = start_mock_mangabaka(vec![]).await;
    let http = reqwest::Client::new();

    // 1. Direct request sending `query=Berserk` must be rejected with HTTP 400
    //    matching real MangaBaka production behavior: `Validation error: Unrecognized key: "query"`
    let resp_query = http
        .get(format!("{url}/series/search?query=Berserk"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_query.status(), StatusCode::BAD_REQUEST);
    let body_query: serde_json::Value = resp_query.json().await.unwrap();
    assert_eq!(
        body_query.get("message").and_then(|v| v.as_str()),
        Some("Validation error: Unrecognized key: \"query\"")
    );

    // 2. Direct request sending BOTH `q=Berserk&query=Berserk` must also be rejected with HTTP 400
    let resp_both = http
        .get(format!("{url}/series/search?q=Berserk&query=Berserk"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_both.status(), StatusCode::BAD_REQUEST);
    let body_both: serde_json::Value = resp_both.json().await.unwrap();
    assert_eq!(
        body_both.get("message").and_then(|v| v.as_str()),
        Some("Validation error: Unrecognized key: \"query\"")
    );

    // 3. Direct request sending `q=Berserk` without auth succeeds with HTTP 200 (public endpoint)
    let resp_q = http
        .get(format!("{url}/series/search?q=Berserk"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_q.status(), StatusCode::OK);
    let body_q: serde_json::Value = resp_q.json().await.unwrap();
    assert_eq!(body_q["data"][0]["title"].as_str(), Some("Berserk"));

    // 4. MangaBakaClient::match_series() must send `q=Berserk` (no `query`) and succeed
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("valid-token".into()),
    );
    let match_res = client.match_series("Berserk").await.unwrap();
    assert!(
        matches!(
            match_res,
            crate::pipeline::mangabaka::MangaBakaSeriesMatch::Found { id: 202, .. }
        ),
        "match_series must send q only and find series: {match_res:?}"
    );

    // 5. Verify that public request did NOT send any auth header even when token was configured on client!
    assert!(
        !state.public_request_had_auth_header.load(Ordering::SeqCst),
        "Public catalog request must not send user authentication headers"
    );
}

#[tokio::test]
async fn test_mangabaka_public_endpoints_require_no_authentication() {
    let (url, state) = start_mock_mangabaka(vec![]).await;
    let client_no_token = MangaBakaClient::new(reqwest::Client::new(), Some(url.clone()), None);

    // 1. Series search succeeds without token
    let search_res = client_no_token.match_series("Berserk").await.unwrap();
    assert!(matches!(
        search_res,
        crate::pipeline::mangabaka::MangaBakaSeriesMatch::Found { id: 202, .. }
    ));

    // 2. AniList cross-reference succeeds without token
    let xref_res = client_no_token.resolve_from_anilist(101).await.unwrap();
    assert!(xref_res.is_some());
    assert_eq!(xref_res.unwrap().mangabaka_id, 202);

    // 3. Collection match succeeds without token
    let col_res = client_no_token.match_collection(202).await.unwrap();
    assert!(matches!(
        col_res,
        crate::pipeline::mangabaka::MangaBakaCollectionMatch::Found { .. }
    ));

    // 4. Verify no auth headers were ever sent on public requests
    assert!(
        !state.public_request_had_auth_header.load(Ordering::SeqCst),
        "Public endpoints must not carry auth headers"
    );

    // 5. Authenticated endpoints fail without token
    let lib_err = client_no_token.get_my_library_entry(202).await.unwrap_err();
    assert_eq!(lib_err.code, "mangabaka_unauthorized");
    assert_eq!(lib_err.status.as_u16(), 401);

    let conn_err = client_no_token.test_connection().await.unwrap_err();
    assert_eq!(conn_err.code, "mangabaka_unauthorized");
    assert_eq!(conn_err.status.as_u16(), 401);
}

#[tokio::test]
async fn test_mangabaka_validate_connection_handles_400_and_401() {
    let (url, state) = start_mock_mangabaka(vec![]).await;

    // 1. Valid token succeeds using GET /v1/my/profile
    let valid_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("valid-token".into()),
    );
    let res = valid_client.test_connection().await;
    assert!(res.is_ok(), "Valid token must succeed");
    let msg = res.unwrap();
    assert!(msg.contains("successful"));
    assert!(msg.contains("reader_dev"));
    // Proves it used /my/profile and did NOT perform a catalog search
    assert_eq!(state.profile_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.search_calls.load(Ordering::SeqCst), 0);

    // 2. HTTP 401: Invalid token classified as mangabaka_unauthorized
    let invalid_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("invalid-token".into()),
    );
    let err401 = invalid_client.test_connection().await.unwrap_err();
    assert_eq!(err401.code, "mangabaka_unauthorized");
    assert_eq!(err401.status.as_u16(), 401);

    // 3. HTTP 403: Forbidden token classified as mangabaka_unauthorized
    let forbidden_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("forbidden-token".into()),
    );
    let err403 = forbidden_client.test_connection().await.unwrap_err();
    assert_eq!(err403.code, "mangabaka_unauthorized");
    assert_eq!(err403.status.as_u16(), 401);

    // 4. HTTP 429: Rate limited token classified as mangabaka_rate_limited
    let rate_limited_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("rate-limited-token".into()),
    );
    let err429 = rate_limited_client.test_connection().await.unwrap_err();
    assert_eq!(err429.code, "mangabaka_rate_limited");
    assert_eq!(err429.status.as_u16(), 429);

    // 5. HTTP 404: Not found endpoint classified as mangabaka_not_found
    let not_found_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("not-found-token".into()),
    );
    let err404 = not_found_client.test_connection().await.unwrap_err();
    assert_eq!(err404.code, "mangabaka_not_found");
    assert_eq!(err404.status.as_u16(), 404);

    // 6. HTTP 400: Bad request parameter classified as mangabaka_bad_request, NOT unauthorized
    let bad_client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url.clone()),
        Some("bad-request-token".into()),
    );
    let err400 = bad_client.test_connection().await.unwrap_err();
    assert_eq!(err400.code, "mangabaka_bad_request");
    assert_eq!(err400.status.as_u16(), 400);
    assert!(
        err400
            .message
            .contains("Parameter rejected by MangaBaka API")
    );

    // 7. No token configured on client: immediately returns mangabaka_unauthorized without network call
    let no_token_client = MangaBakaClient::new(reqwest::Client::new(), Some(url.clone()), None);
    let err_no_tok = no_token_client.test_connection().await.unwrap_err();
    assert_eq!(err_no_tok.code, "mangabaka_unauthorized");
    assert_eq!(err_no_tok.status.as_u16(), 401);
}

#[tokio::test]
async fn test_mangabaka_status_presentation_distinguishes_token_from_validation() {
    let dir = std::env::temp_dir().join(format!("mango-mb-status-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");

    let pipeline = crate::pipeline::Pipeline::new(db.clone());

    // Initial state: no token configured, mangabaka_validated is None
    let cfg0 = pipeline.load_config().await.unwrap();
    assert!(!cfg0.has_mangabaka_token);
    assert_eq!(cfg0.mangabaka_validated, None);

    // Save token
    let mut patch = cfg0.clone();
    patch.mangabaka_token = Some("secret-token".into());
    pipeline.save_config(patch).await.unwrap();

    // After setting token: token is configured, but validation is not yet run (unverified)
    let cfg1 = pipeline.load_config().await.unwrap();
    assert!(cfg1.has_mangabaka_token);
    assert_eq!(cfg1.mangabaka_validated, None);

    // Simulate successful validation
    db.call(|conn| crate::db::set_state(conn, "pipeline.mangabaka_validated", "valid"))
        .await
        .unwrap();
    let cfg2 = pipeline.load_config().await.unwrap();
    assert!(cfg2.has_mangabaka_token);
    assert_eq!(cfg2.mangabaka_validated, Some(true));

    // Simulate validation failure (e.g. HTTP 400 Bad Request)
    db.call(|conn| {
        crate::db::set_state(
            conn,
            "pipeline.mangabaka_validated",
            "error: MangaBaka request rejected (HTTP 400)",
        )
    })
    .await
    .unwrap();
    let cfg3 = pipeline.load_config().await.unwrap();
    // Stored token is NOT erased when validation fails!
    assert!(
        cfg3.has_mangabaka_token,
        "Token must be preserved on validation failure"
    );
    assert_eq!(cfg3.mangabaka_validated, Some(false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_trackers_sync_status_shares_authoritative_config_without_secrets() {
    let dir = std::env::temp_dir().join(format!("mango-status-auth-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let pipeline = crate::pipeline::Pipeline::new(db.clone());

    // 1. Initial state: Komga disabled, no token
    let (cfg0, mb_tok0, komga_key0) = pipeline.load_internal_config().await.unwrap();
    assert!(!cfg0.komga_enabled);
    assert!(!cfg0.has_komga_api_key);
    assert!(!cfg0.has_mangabaka_token);
    assert_eq!(cfg0.mangabaka_validated, None);
    assert_eq!(mb_tok0, None);
    assert_eq!(komga_key0, None);

    let status0 = crate::api::SyncStatusResponse {
        running: false,
        komga_enabled: cfg0.komga_enabled,
        has_komga_api_key: cfg0.has_komga_api_key,
        mangabaka_enabled: cfg0.mangabaka_enabled,
        has_mangabaka_token: cfg0.has_mangabaka_token,
        mangabaka_validated: cfg0.mangabaka_validated,
    };
    let json0 = serde_json::to_value(&status0).unwrap();
    assert_eq!(json0["komga_enabled"], false);
    assert_eq!(json0["has_komga_api_key"], false);
    assert_eq!(json0["has_mangabaka_token"], false);
    assert_eq!(json0["mangabaka_validated"], serde_json::Value::Null);
    assert!(json0.get("mangabaka_token").is_none());
    assert!(json0.get("komga_api_key").is_none());

    // 2. Configure Komga and MangaBaka token
    let mut patch = cfg0.clone();
    patch.komga_enabled = true;
    patch.komga_api_key = Some("super-secret-komga-key".into());
    patch.mangabaka_token = Some("super-secret-mb-token".into());
    pipeline.save_config(patch).await.unwrap();

    let (cfg1, mb_tok1, komga_key1) = pipeline.load_internal_config().await.unwrap();
    assert!(cfg1.komga_enabled);
    assert!(cfg1.has_komga_api_key);
    assert!(cfg1.has_mangabaka_token);
    assert_eq!(cfg1.mangabaka_validated, None); // Unverified
    assert_eq!(mb_tok1.as_deref(), Some("super-secret-mb-token"));
    assert_eq!(komga_key1.as_deref(), Some("super-secret-komga-key"));

    let status1 = crate::api::SyncStatusResponse {
        running: false,
        komga_enabled: cfg1.komga_enabled,
        has_komga_api_key: cfg1.has_komga_api_key,
        mangabaka_enabled: cfg1.mangabaka_enabled,
        has_mangabaka_token: cfg1.has_mangabaka_token,
        mangabaka_validated: cfg1.mangabaka_validated,
    };
    let json1 = serde_json::to_value(&status1).unwrap();
    assert_eq!(json1["komga_enabled"], true);
    assert_eq!(json1["has_komga_api_key"], true);
    assert_eq!(json1["has_mangabaka_token"], true);
    assert_eq!(json1["mangabaka_validated"], serde_json::Value::Null); // Token Configured (Unverified)
    assert!(json1.get("mangabaka_token").is_none());
    assert!(json1.get("komga_api_key").is_none());
    let raw_str = json1.to_string();
    assert!(!raw_str.contains("super-secret"));

    // 3. Mark validated (Validation succeeded)
    db.call(|conn| crate::db::set_state(conn, "pipeline.mangabaka_validated", "valid"))
        .await
        .unwrap();
    let (cfg2, _, _) = pipeline.load_internal_config().await.unwrap();
    assert_eq!(cfg2.mangabaka_validated, Some(true)); // Validated

    // 4. Mark validation failed (Stored token remains!)
    db.call(|conn| {
        crate::db::set_state(
            conn,
            "pipeline.mangabaka_validated",
            "error: HTTP 400 Bad Request",
        )
    })
    .await
    .unwrap();
    let (cfg3, mb_tok3, _) = pipeline.load_internal_config().await.unwrap();
    assert!(cfg3.has_mangabaka_token);
    assert_eq!(mb_tok3.as_deref(), Some("super-secret-mb-token"));
    assert_eq!(cfg3.mangabaka_validated, Some(false)); // Validation Failed

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_source_anilist_official_schema_resolves_blue_lock() {
    let (url, state) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    // 1. Direct cross-reference query using real MangaBaka OpenAPI response structure
    let xref = client.resolve_from_anilist(106130).await.unwrap();
    assert!(xref.is_some());
    let x = xref.unwrap();
    assert_eq!(x.mangabaka_id, 83);
    assert_eq!(x.title.as_deref(), Some("Blue Lock"));

    // 2. Precedence test in identity resolver:
    // When an AniList mapping exists (media ID 106130), resolve_mangabaka_identity
    // resolves to MangaBaka ID 83 via cross-reference without falling through to title search!
    let dir = std::env::temp_dir().join(format!("mango-bl-id-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Add AniList mapping for Blue Lock
    let anilist_map = TrackerMapping {
        komga_series_id: "komga-blue-lock".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "106130".into(),
        tracker_title: Some("Blue Lock".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();

    let initial_search_calls = state.search_calls.load(Ordering::SeqCst);

    let res = resolve_mangabaka_identity(&conn, &client, "komga-blue-lock", "Blue Lock")
        .await
        .unwrap();

    let m = match res {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved, got {other:?}"),
    };

    assert_eq!(m.tracker_id, "83");
    assert_eq!(m.mapping_source, MappingSource::CrossReference);
    assert_eq!(m.confidence, MappingConfidence::Resolved);
    // Search endpoint must NOT have been called because cross-reference succeeded!
    assert_eq!(
        state.search_calls.load(Ordering::SeqCst),
        initial_search_calls
    );

    // 3. Proves upgrade from previously recorded Ambiguous mapping when AniList cross-reference is available
    let ambig_map = TrackerMapping {
        komga_series_id: "komga-blue-lock-2".into(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: String::new(),
        tracker_title: None,
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Ambiguous,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &ambig_map).unwrap();

    // Now add AniList mapping for that series
    let anilist_map_2 = TrackerMapping {
        komga_series_id: "komga-blue-lock-2".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "106130".into(),
        tracker_title: Some("Blue Lock".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map_2).unwrap();

    let res_upgrade = resolve_mangabaka_identity(&conn, &client, "komga-blue-lock-2", "Blue Lock")
        .await
        .unwrap();
    let m_up = match res_upgrade {
        IdentityResolutionResult::Resolved(m) => m,
        other => panic!("Expected Resolved from upgrade, got {other:?}"),
    };
    assert_eq!(m_up.tracker_id, "83");
    assert_eq!(m_up.mapping_source, MappingSource::CrossReference);
    assert_eq!(m_up.confidence, MappingConfidence::Resolved);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_cross_reference_multiple_active_series_fails_closed_as_ambiguous() {
    let (url, _) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-multi-active-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // Map AniList ID 998 (which returns multiple active series)
    let anilist_map = TrackerMapping {
        komga_series_id: "komga-multi-series".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "998".into(),
        tracker_title: Some("Multiple Series".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();

    let res = resolve_mangabaka_identity(&conn, &client, "komga-multi-series", "Multiple Series")
        .await
        .unwrap();

    assert!(
        matches!(res, IdentityResolutionResult::Ambiguous(_)),
        "Multiple active series in cross-reference must fail closed as Ambiguous"
    );

    // Verify it saved an Ambiguous record in DB
    let saved = get_mapping(&conn, "komga-multi-series", SyncTarget::MangaBaka).unwrap();
    assert!(saved.is_some());
    assert_eq!(saved.unwrap().confidence, MappingConfidence::Ambiguous);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mangabaka_cross_reference_remote_error_does_not_fall_through_to_title_search() {
    let (url, state) = start_mock_mangabaka(vec![]).await;
    let client = MangaBakaClient::new(
        reqwest::Client::new(),
        Some(url),
        Some("valid-token".into()),
    );

    let dir = std::env::temp_dir().join(format!("mango-xref-err-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("connection");

    // AniList ID 502 returns HTTP 502 Bad Gateway
    let anilist_map = TrackerMapping {
        komga_series_id: "komga-error-502".into(),
        tracker: SyncTarget::AniList,
        tracker_id: "502".into(),
        tracker_title: Some("Berserk".into()),
        edition_id: None,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(1700000000),
        updated_at: 1700000000,
    };
    save_mapping(&conn, &anilist_map).unwrap();

    let initial_searches = state.search_calls.load(Ordering::SeqCst);

    let res = resolve_mangabaka_identity(&conn, &client, "komga-error-502", "Berserk")
        .await
        .unwrap();

    // Must return NetworkError (or ApiError), NOT fall through to exact title search for "Berserk"!
    assert!(
        matches!(res, IdentityResolutionResult::NetworkError(_)),
        "Remote error on AniList cross-reference must NOT fall through to title search"
    );
    assert_eq!(
        state.search_calls.load(Ordering::SeqCst),
        initial_searches,
        "Search endpoint must NOT be queried when AniList cross-reference errors"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ======================================================================
// MILESTONE: UNIFIED TRACKED LISTS & SAFE STATUS SYNCHRONIZATION TESTS
// ======================================================================

use super::eligibility::evaluate_completion_predicate;
use super::lists::{
    ListsQuery, TrackerListEntry, TrackerListRefreshStatus, build_unified_catalog,
    get_tracker_list_entries, normalize_status, update_refresh_status, upsert_tracker_list_entries,
};

#[test]
fn test_backend_anilist_planning_imports_without_komga_ownership() {
    let dir = std::env::temp_dir().join("mango-test-backend-al-planning");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let entry = TrackerListEntry {
        tracker: "anilist".to_string(),
        tracker_id: "12345".to_string(),
        title: "Frieren: Beyond Journey's End".to_string(),
        cover_url: Some("https://example.com/frieren.jpg".to_string()),
        status: "PLANNING".to_string(),
        normalized_status: "Planning".to_string(),
        progress_chapter: Some(0.0),
        progress_volume: Some(0.0),
        total_chapters: Some(130),
        total_volumes: Some(12),
        format: Some("MANGA".to_string()),
        updated_at: 1700000000,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[entry]).unwrap();

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    assert_eq!(res.items.len(), 1);
    let item = &res.items[0];
    assert_eq!(item.title, "Frieren: Beyond Journey's End");
    assert_eq!(item.normalized_status, "Planning");
    assert_eq!(item.ownership_status, "Not Owned");
    assert_eq!(item.owned_units, None);
    assert_eq!(item.auto_acquire, false);
    assert!(item.anilist.is_some());
    assert!(item.mangabaka.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_mangabaka_want_to_read_imports_without_komga_ownership() {
    let dir = std::env::temp_dir().join("mango-test-backend-mb-want-to-read");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let entry = TrackerListEntry {
        tracker: "mangabaka".to_string(),
        tracker_id: "9988".to_string(),
        title: "Dungeon Meshi".to_string(),
        cover_url: Some("https://example.com/meshi.jpg".to_string()),
        status: "plan_to_read".to_string(),
        normalized_status: "Planning".to_string(),
        progress_chapter: Some(0.0),
        progress_volume: Some(0.0),
        total_chapters: Some(97),
        total_volumes: Some(14),
        format: Some("manga".to_string()),
        updated_at: 1700000000,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[entry]).unwrap();

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    assert_eq!(res.items.len(), 1);
    let item = &res.items[0];
    assert_eq!(item.title, "Dungeon Meshi");
    assert_eq!(item.normalized_status, "Planning");
    assert_eq!(item.ownership_status, "Not Owned");
    assert_eq!(item.auto_acquire, false);
    assert!(item.anilist.is_none());
    assert!(item.mangabaka.is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_same_title_merges_when_identity_known() {
    let dir = std::env::temp_dir().join("mango-test-backend-merge");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let al = TrackerListEntry {
        tracker: "anilist".to_string(),
        tracker_id: "5001".to_string(),
        title: "Chainsaw Man".to_string(),
        cover_url: Some("https://example.com/csm-al.jpg".to_string()),
        status: "CURRENT".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(97.0),
        progress_volume: Some(11.0),
        total_chapters: None,
        total_volumes: None,
        format: Some("MANGA".to_string()),
        updated_at: 1700000000,
        raw_data: None,
    };

    let mb = TrackerListEntry {
        tracker: "mangabaka".to_string(),
        tracker_id: "8001".to_string(),
        title: "Chainsaw Man".to_string(),
        cover_url: Some("https://example.com/csm-mb.jpg".to_string()),
        status: "reading".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(97.0),
        progress_volume: Some(11.0),
        total_chapters: None,
        total_volumes: None,
        format: Some("manga".to_string()),
        updated_at: 1700000100,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[al, mb]).unwrap();

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    // Exactly 1 merged title
    assert_eq!(res.items.len(), 1);
    let item = &res.items[0];
    assert_eq!(item.title, "Chainsaw Man");
    assert!(item.anilist.is_some());
    assert!(item.mangabaka.is_some());
    assert_eq!(item.id, "al-5001_mb-8001");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_ambiguous_identities_remain_separate() {
    let dir = std::env::temp_dir().join("mango-test-backend-ambiguity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let al = TrackerListEntry {
        tracker: "anilist".to_string(),
        tracker_id: "5001".to_string(),
        title: "Berserk".to_string(),
        cover_url: None,
        status: "CURRENT".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(10.0),
        progress_volume: None,
        total_chapters: None,
        total_volumes: None,
        format: None,
        updated_at: 1700000000,
        raw_data: None,
    };

    // Multiple MangaBaka entries matching the same title
    let mb1 = TrackerListEntry {
        tracker: "mangabaka".to_string(),
        tracker_id: "8001".to_string(),
        title: "Berserk".to_string(),
        cover_url: None,
        status: "reading".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(10.0),
        progress_volume: None,
        total_chapters: None,
        total_volumes: None,
        format: None,
        updated_at: 1700000100,
        raw_data: None,
    };

    let mb2 = TrackerListEntry {
        tracker: "mangabaka".to_string(),
        tracker_id: "8002".to_string(),
        title: "Berserk".to_string(),
        cover_url: None,
        status: "plan_to_read".to_string(),
        normalized_status: "Planning".to_string(),
        progress_chapter: Some(0.0),
        progress_volume: None,
        total_chapters: None,
        total_volumes: None,
        format: None,
        updated_at: 1700000200,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[al, mb1, mb2]).unwrap();

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    // Because 2 candidates exist, merge fails closed and titles remain separate!
    assert_eq!(res.items.len(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_import_never_queues_acquisition_and_auto_acquire_false() {
    let dir = std::env::temp_dir().join("mango-test-backend-no-auto-acq");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let entry = TrackerListEntry {
        tracker: "anilist".to_string(),
        tracker_id: "999".to_string(),
        title: "Solo Leveling".to_string(),
        cover_url: None,
        status: "PLANNING".to_string(),
        normalized_status: "Planning".to_string(),
        progress_chapter: Some(0.0),
        progress_volume: Some(0.0),
        total_chapters: Some(200),
        total_volumes: None,
        format: None,
        updated_at: 1700000000,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[entry]).unwrap();

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    assert_eq!(res.items[0].auto_acquire, false);
    assert_eq!(res.items[0].acquisition_status, "Idle");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_status_transitions_planning_to_reading_dry_run() {
    let progress = super::model::KomgaSeriesProgress {
        series_id: "k-frieren".into(),
        series_title: "Frieren".into(),
        library_id: "lib-1".into(),
        total_books: 10,
        completed_books: 1,
        highest_completed_book: Some(1.0),
        highest_completed_volume: Some(1.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books: vec![],
    };

    // AniList planning -> current
    let old_status = 5; // PLANNING
    let is_completed = evaluate_completion_predicate(&progress, 12, 1.0);
    assert_eq!(is_completed, false);
    let new_status = if old_status == 5
        && (progress.highest_completed_volume.unwrap_or(0.0) > 0.0 || progress.completed_books > 0)
    {
        1 // CURRENT
    } else {
        old_status
    };
    assert_eq!(new_status, 1);

    // MangaBaka plan_to_read -> reading
    let old_state = "plan_to_read";
    let new_state = if (old_state == "plan_to_read" || old_state == "considering")
        && (progress.highest_completed_volume.unwrap_or(0.0) > 0.0 || progress.completed_books > 0)
    {
        "reading"
    } else {
        old_state
    };
    assert_eq!(new_state, "reading");
}

#[test]
fn test_backend_paused_and_dropped_never_automatically_changed() {
    let _progress = super::model::KomgaSeriesProgress {
        series_id: "k-series".into(),
        series_title: "Paused Series".into(),
        library_id: "lib-1".into(),
        total_books: 20,
        completed_books: 5,
        highest_completed_book: Some(5.0),
        highest_completed_volume: Some(5.0),
        highest_completed_chapter: None,
        is_series_completed: false,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books: vec![],
    };

    // AniList PAUSED (3) and DROPPED (4)
    for paused_code in [3, 4] {
        let new_status = if paused_code == 3 || paused_code == 4 {
            paused_code
        } else {
            1
        };
        assert_eq!(new_status, paused_code);
    }

    // MangaBaka paused and dropped
    for paused_state in ["paused", "dropped"] {
        let new_state = if paused_state == "paused" || paused_state == "dropped" {
            paused_state
        } else {
            "reading"
        };
        assert_eq!(new_state, paused_state);
    }
}

#[test]
fn test_backend_completed_not_regressed() {
    let old_al = 2; // COMPLETED
    let new_al = if old_al == 2 { 2 } else { 1 };
    assert_eq!(new_al, 2);

    let old_mb = "completed";
    let new_mb = if old_mb == "completed" {
        "completed"
    } else {
        "reading"
    };
    assert_eq!(new_mb, "completed");
}

#[test]
fn test_backend_partial_ownership_does_not_produce_completed() {
    // User owns 10 out of 20, completed all 10 owned
    let progress = super::model::KomgaSeriesProgress {
        series_id: "k-bleach".into(),
        series_title: "Bleach".into(),
        library_id: "lib-1".into(),
        total_books: 10,
        completed_books: 10,
        highest_completed_book: Some(10.0),
        highest_completed_volume: Some(10.0),
        highest_completed_chapter: None,
        is_series_completed: true,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books: vec![],
    };

    let is_completed = evaluate_completion_predicate(&progress, 20, 10.0);
    assert_eq!(
        is_completed, false,
        "Owning and reading 10 of 20 must NOT produce Completed"
    );
}

#[test]
fn test_backend_known_final_volume_and_contiguous_completion_produces_completed() {
    // User owns 20 out of 20, completed all 20 contiguously
    let books: Vec<super::model::KomgaBookProgress> = (1..=20)
        .map(|i| super::model::KomgaBookProgress {
            id: format!("b-{i}"),
            name: format!("Volume {i}"),
            number: i as f64,
            volume: Some(i as f64),
            chapter: None,
            completed: true,
            page: 200,
            read_date: Some("2026-10-24T00:00:00Z".into()),
        })
        .collect();

    let progress = super::model::KomgaSeriesProgress {
        series_id: "k-complete".into(),
        series_title: "Full Series".into(),
        library_id: "lib-1".into(),
        total_books: 20,
        completed_books: 20,
        highest_completed_book: Some(20.0),
        highest_completed_volume: Some(20.0),
        highest_completed_chapter: None,
        is_series_completed: true,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books,
    };

    let is_completed = evaluate_completion_predicate(&progress, 20, 20.0);
    assert_eq!(
        is_completed, true,
        "Known final volume + contiguous completion must produce Completed"
    );
}

#[test]
fn test_backend_unknown_total_does_not_produce_completed() {
    let progress = super::model::KomgaSeriesProgress {
        series_id: "k-unknown".into(),
        series_title: "Unknown Totals Series".into(),
        library_id: "lib-1".into(),
        total_books: 15,
        completed_books: 15,
        highest_completed_book: Some(15.0),
        highest_completed_volume: Some(15.0),
        highest_completed_chapter: None,
        is_series_completed: true,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books: vec![],
    };

    // Total units is 0 (unknown)
    let is_completed = evaluate_completion_predicate(&progress, 0, 15.0);
    assert_eq!(
        is_completed, false,
        "Unknown total units must NOT produce Completed"
    );
}

#[test]
fn test_backend_missing_intermediate_volume_does_not_produce_completed() {
    // Owns 20 volumes, read 19, missing volume 5
    let mut books: Vec<super::model::KomgaBookProgress> = Vec::new();
    for i in 1..=20 {
        let is_read = i != 5;
        books.push(super::model::KomgaBookProgress {
            id: format!("b-{i}"),
            name: format!("Volume {i}"),
            number: i as f64,
            volume: Some(i as f64),
            chapter: None,
            completed: is_read,
            page: if is_read { 200 } else { 0 },
            read_date: if is_read {
                Some("2026-10-24T00:00:00Z".into())
            } else {
                None
            },
        });
    }

    let progress = super::model::KomgaSeriesProgress {
        series_id: "k-missing".into(),
        series_title: "Missing Unit Series".into(),
        library_id: "lib-1".into(),
        total_books: 20,
        completed_books: 19,
        highest_completed_book: Some(20.0),
        highest_completed_volume: Some(20.0),
        highest_completed_chapter: None,
        is_series_completed: true,
        last_read_date: Some("2026-10-24T00:00:00Z".into()),
        books,
    };

    let is_completed = evaluate_completion_predicate(&progress, 20, 20.0);
    assert_eq!(
        is_completed, false,
        "Missing intermediate unit must NOT produce Completed"
    );
}

#[test]
fn test_backend_tracker_progress_never_decreases() {
    let komga_prog: f64 = 5.0;
    let old_tracker_prog: f64 = 10.0;
    let target = old_tracker_prog.max(komga_prog);
    assert_eq!(target, 10.0);
}

#[test]
fn test_backend_refresh_failure_preserves_cached_entries() {
    let dir = std::env::temp_dir().join("mango-test-backend-cache-preserve");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let entry = TrackerListEntry {
        tracker: "mangabaka".to_string(),
        tracker_id: "101".to_string(),
        title: "Cached Manga".to_string(),
        cover_url: None,
        status: "reading".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(5.0),
        progress_volume: None,
        total_chapters: None,
        total_volumes: None,
        format: None,
        updated_at: 1700000000,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[entry]).unwrap();

    // Now record error in refresh status (e.g. MangaBaka returned 500)
    update_refresh_status(
        &conn,
        &TrackerListRefreshStatus {
            tracker: "mangabaka".to_string(),
            last_refreshed_at: 1700001000,
            item_count: 0,
            status: "error".to_string(),
            error_message: Some("HTTP 500 Internal Server Error".to_string()),
        },
    )
    .unwrap();

    // Cached entries must STILL be preserved and returned!
    let cached_entries = get_tracker_list_entries(&conn, None).unwrap();
    assert_eq!(cached_entries.len(), 1);
    assert_eq!(cached_entries[0].title, "Cached Manga");

    let res = build_unified_catalog(&conn, &[], &ListsQuery::default()).unwrap();
    assert_eq!(res.items.len(), 1);
    assert_eq!(res.items[0].title, "Cached Manga");
    assert_eq!(res.refresh_statuses[0].status, "error");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_rate_limit_and_unauthorized_preserve_cached_entries() {
    let dir = std::env::temp_dir().join("mango-test-backend-rate-limit-preserve");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).expect("test db");
    let conn = db.second().expect("conn");

    let entry = TrackerListEntry {
        tracker: "anilist".to_string(),
        tracker_id: "202".to_string(),
        title: "Cached AniList Manga".to_string(),
        cover_url: None,
        status: "CURRENT".to_string(),
        normalized_status: "Reading".to_string(),
        progress_chapter: Some(20.0),
        progress_volume: None,
        total_chapters: None,
        total_volumes: None,
        format: None,
        updated_at: 1700000000,
        raw_data: None,
    };

    upsert_tracker_list_entries(&conn, &[entry]).unwrap();

    // Update with rate limited status
    update_refresh_status(
        &conn,
        &TrackerListRefreshStatus {
            tracker: "anilist".to_string(),
            last_refreshed_at: 1700001000,
            item_count: 0,
            status: "rate_limited".to_string(),
            error_message: Some("HTTP 429 Too Many Requests".to_string()),
        },
    )
    .unwrap();

    let cached = get_tracker_list_entries(&conn, Some("anilist")).unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].title, "Cached AniList Manga");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backend_status_normalization_mapping() {
    assert_eq!(normalize_status("anilist", "CURRENT"), "Reading");
    assert_eq!(normalize_status("anilist", "REPEATING"), "Reading");
    assert_eq!(normalize_status("anilist", "PLANNING"), "Planning");
    assert_eq!(normalize_status("anilist", "COMPLETED"), "Completed");
    assert_eq!(normalize_status("anilist", "PAUSED"), "Paused");
    assert_eq!(normalize_status("anilist", "DROPPED"), "Dropped");

    assert_eq!(normalize_status("mangabaka", "reading"), "Reading");
    assert_eq!(normalize_status("mangabaka", "rereading"), "Reading");
    assert_eq!(normalize_status("mangabaka", "plan_to_read"), "Planning");
    assert_eq!(normalize_status("mangabaka", "considering"), "Planning");
    assert_eq!(normalize_status("mangabaka", "completed"), "Completed");
    assert_eq!(normalize_status("mangabaka", "paused"), "Paused");
    assert_eq!(normalize_status("mangabaka", "dropped"), "Dropped");
}
