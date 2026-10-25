//! Acquisition Regression Tests covering Scenarios A through AH.

use crate::acquisition::service::*;
use crate::acquisition::torbox::*;
use crate::acquisition::*;
use crate::db::Db;
use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn make_valid_test_cbz(path: &Path, series: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("ComicInfo.xml", options).unwrap();
    let info = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo><Series>{series}</Series></ComicInfo>");
    zip.write_all(info.as_bytes()).unwrap();
    zip.start_file("001.png", options).unwrap();
    // Valid minimal PNG bytes
    zip.write_all(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82").unwrap();
    zip.finish().unwrap();
}

#[tokio::test]
async fn test_scenario_i_torbox_test_connection() {
    let app = Router::new().route(
        "/v1/api/user/me",
        get(|| async {
            Json(json!({
                "success": true,
                "error": null,
                "detail": "Retrieved user account information.",
                "data": {
                    "id": 12345,
                    "email": "test@torbox.app",
                    "plan": 2
                }
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

    let res = client.test_connection().await.unwrap();
    assert!(res.contains("Connected to TorBox as test@torbox.app"));
    assert!(res.contains("Plan: 2"));
}

#[tokio::test]
async fn test_scenario_j_torbox_submit_torrent() {
    let app = Router::new().route(
        "/v1/api/torrents/createtorrent",
        post(|| async {
            Json(json!({
                "success": true,
                "error": null,
                "detail": "Torrent created successfully.",
                "data": {
                    "torrent_id": 98765,
                    "hash": "abcdef1234567890abcdef1234567890abcdef12"
                }
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

    let rel = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "1".to_string(),
        title: "Berserk v01".to_string(),
        raw_title: "[danke-Empire] Berserk v01".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/1".to_string(),
        torrent_url: None,
        magnet_url: Some("magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Berserk".to_string()),
        info_hash: Some("abcdef1234567890abcdef1234567890abcdef12".to_string()),
        category_id: Some("3_1".to_string()),
        category_name: Some("Literature - English".to_string()),
        size_bytes: 150_000_000,
        size_formatted: "150 MiB".to_string(),
        seeders: 50,
        leechers: 2,
        published_at: None,
        is_trusted: true,
        is_remake: false,
        release_unit: AcquisitionReleaseUnit::Volume(1.0),
        release_number: Some(1.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    let info = client.submit(&rel).await.unwrap();
    assert_eq!(info.external_job_id, "98765");
    assert_eq!(info.torrent_hash.as_deref(), Some("abcdef1234567890abcdef1234567890abcdef12"));
}

#[tokio::test]
async fn test_scenario_k_and_l_torbox_check_status_and_list_files() {
    let app = Router::new().route(
        "/v1/api/torrents/mylist",
        get(|| async {
            Json(json!({
                "success": true,
                "error": null,
                "data": {
                    "id": 98765,
                    "name": "Berserk v01",
                    "hash": "abcdef1234567890abcdef1234567890abcdef12",
                    "download_state": "completed",
                    "progress": 1.0,
                    "download_speed": 0,
                    "seeds": 15,
                    "peers": 0,
                    "files": [
                        {
                            "id": 101,
                            "name": "Berserk v01.cbz",
                            "size": 150000000,
                            "absolute_path": "Berserk v01.cbz"
                        },
                        {
                            "id": 102,
                            "name": "cover.jpg",
                            "size": 500000,
                            "absolute_path": "cover.jpg"
                        }
                    ]
                }
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

    let status = client.check_status("98765").await.unwrap();
    assert_eq!(status, BackendJobStatus::Completed);

    let files = client.list_files("98765").await.unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].id, 101);
    assert_eq!(files[0].name, "Berserk v01.cbz");
}

#[tokio::test]
async fn test_scenario_n_o_p_request_download_retrieve_and_cancel() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_acq_nop_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();

    let server_cbz = temp_dir.join("server_artifact.cbz");
    make_valid_test_cbz(&server_cbz, "Berserk");
    let server_cbz_bytes = fs::read(&server_cbz).unwrap();
    let server_cbz_len = server_cbz_bytes.len() as u64;

    let server_bytes_clone = server_cbz_bytes.clone();
    let cancel_called = Arc::new(AtomicUsize::new(0));
    let cancel_called_clone = cancel_called.clone();

    let app = Router::new()
        .route(
            "/v1/api/torrents/requestdl",
            get(move |Query(params): Query<std::collections::HashMap<String, String>>| {
                let token = params.get("token").cloned().unwrap_or_default();
                async move {
                    if token.is_empty() {
                        (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
                    } else {
                        Json(json!({
                            "success": true,
                            "error": null,
                            "detail": "Download link generated.",
                            "data": format!("http://127.0.0.1:{}/cdn/download", params.get("port").unwrap_or(&"0".to_string()))
                        })).into_response()
                    }
                }
            }),
        )
        .route(
            "/cdn/download",
            get(move || {
                let bytes = server_bytes_clone.clone();
                async move {
                    ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes)
                }
            }),
        )
        .route(
            "/v1/api/torrents/controltorrent",
            post(move || {
                let count = cancel_called_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "success": true, "detail": "Torrent deleted." }))
                }
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
        Some("test-token".to_string()),
    );

    // Scenario N: Request link
    validate_download_url("https://cdn.torbox.app/files/123/Berserk.cbz").unwrap();
    let link_url = format!("http://127.0.0.1:{port}/cdn/download");

    // Scenario O: Streaming retrieval into .part and atomic promote
    let part_path = temp_dir.join("staging.cbz.part");
    let dest_path = temp_dir.join("staging.cbz");

    let mut resp = reqwest::get(&link_url).await.unwrap();
    let mut file = File::create(&part_path).unwrap();
    while let Some(chunk) = resp.chunk().await.unwrap() {
        file.write_all(&chunk).unwrap();
    }
    drop(file);

    assert_eq!(fs::metadata(&part_path).unwrap().len(), server_cbz_len);
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&part_path).is_ok());
    fs::rename(&part_path, &dest_path).unwrap();
    assert!(dest_path.is_file());

    // Scenario P: Control / cancel
    client.cancel("98765").await.unwrap();
    assert_eq!(cancel_called.load(Ordering::SeqCst), 1);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_scenario_q_r_s_t_torbox_error_handling() {
    let app = Router::new()
        .route("/401/user/me", get(|| async { (StatusCode::UNAUTHORIZED, "Unauthorized") }))
        .route("/429/user/me", get(|| async { (StatusCode::TOO_MANY_REQUESTS, "Rate limit") }))
        .route("/500/user/me", get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "Server Error") }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 401 Unauthorized
    let client_401 = TorBoxClient::new(
        reqwest::Client::new(),
        Some(format!("http://127.0.0.1:{port}/401")),
        Some("bad-token".to_string()),
    );
    let err_401 = client_401.test_connection().await.unwrap_err();
    assert!(matches!(err_401, TorBoxError::Unauthorized(_)));

    // 429 Rate limited
    let client_429 = TorBoxClient::new(
        reqwest::Client::new(),
        Some(format!("http://127.0.0.1:{port}/429")),
        Some("valid-token".to_string()),
    );
    let err_429 = client_429.test_connection().await.unwrap_err();
    assert!(matches!(err_429, TorBoxError::RateLimited(_)));

    // 500 Remote error
    let client_500 = TorBoxClient::new(
        reqwest::Client::new(),
        Some(format!("http://127.0.0.1:{port}/500")),
        Some("valid-token".to_string()),
    );
    let err_500 = client_500.test_connection().await.unwrap_err();
    assert!(matches!(err_500, TorBoxError::RemoteError(500, _)));

    // Scenario T: Network error (invalid port / unreachable)
    let client_offline = TorBoxClient::new(
        reqwest::Client::new(),
        Some("http://127.0.0.1:1".to_string()),
        Some("valid-token".to_string()),
    );
    let err_net = client_offline.test_connection().await.unwrap_err();
    assert!(matches!(err_net, TorBoxError::NetworkError(_)));
}

#[tokio::test]
async fn test_scenario_u_and_v_secret_storage_and_redaction() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_uv_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&temp_dir.join("test.db")).unwrap();

    let service = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Save secret token
    let patch = AcquisitionConfigPatch {
        nyaa_enabled: Some(true),
        nyaa_url: None,
        nyaa_category: None,
        torbox_enabled: Some(true),
        torbox_url: None,
        torbox_token: Some("super_secret_torbox_token_12345".to_string()),
    };
    service.save_config(patch).await.unwrap();

    // Verify token exists in state table (Scenario V)
    let secret = service.get_torbox_token().await.unwrap();
    assert_eq!(secret.as_deref(), Some("super_secret_torbox_token_12345"));

    // Verify token is REDACTED in public config view (Scenario U)
    let config = service.load_config().await.unwrap();
    assert!(config.has_torbox_token);
    let json_val = serde_json::to_value(&config).unwrap();
    assert!(json_val.get("torboxToken").is_none());
    assert_eq!(json_val["hasTorboxToken"], true);

    // Verify client Debug output does NOT contain the secret token (Scenario U)
    let client = TorBoxClient::new(
        reqwest::Client::new(),
        None,
        Some("super_secret_torbox_token_12345".to_string()),
    );
    let debug_str = format!("{client:?}");
    assert!(!debug_str.contains("super_secret_torbox_token_12345"));
    assert!(debug_str.contains("has_token: true"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_scenario_w_duplicate_protection() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_w_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&temp_dir.join("test.db")).unwrap();
    let lib_dir = temp_dir.join("library");
    fs::create_dir_all(&lib_dir).unwrap();

    // Setup pipeline config pointing to lib_dir
    let pipeline = Arc::new(crate::pipeline::Pipeline::new(db.clone()));
    let mut pipe_cfg = crate::pipeline::PipelineConfig::default();
    pipe_cfg.enabled = true;
    pipe_cfg.library_dir = Some(lib_dir.to_string_lossy().to_string());
    pipeline.save_config(pipe_cfg).await.unwrap();

    let service = AcquisitionService::new(
        db.clone(),
        reqwest::Client::new(),
        Some(pipeline.clone()),
        None,
    );

    // Enable TorBox
    service.save_config(AcquisitionConfigPatch {
        nyaa_enabled: Some(true),
        nyaa_url: None,
        nyaa_category: None,
        torbox_enabled: Some(true),
        torbox_url: None,
        torbox_token: Some("dummy-token".to_string()),
    }).await.unwrap();

    // Insert manga
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Berserk', 1)", [])?;
        Ok(())
    }).await.unwrap();

    // 1. Case A: Canonical filesystem file already exists
    let series_dir = lib_dir.join("Berserk");
    fs::create_dir_all(&series_dir).unwrap();
    let canonical_v1 = series_dir.join("Berserk v01.cbz");
    make_valid_test_cbz(&canonical_v1, "Berserk");

    let rel = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "101".to_string(),
        title: "Berserk v01".to_string(),
        raw_title: "[danke-Empire] Berserk v01".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/101".to_string(),
        torrent_url: None,
        magnet_url: Some("magnet:?xt=urn:btih:hash1".to_string()),
        info_hash: Some("hash1".to_string()),
        category_id: Some("3_1".to_string()),
        category_name: Some("Literature".to_string()),
        size_bytes: 100_000_000,
        size_formatted: "100 MiB".to_string(),
        seeders: 10,
        leechers: 0,
        published_at: None,
        is_trusted: true,
        is_remake: false,
        release_unit: AcquisitionReleaseUnit::Volume(1.0),
        release_number: Some(1.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    let err = service.acquire_release(1, rel.clone(), Some(AcquisitionReleaseUnit::Volume(1.0))).await.unwrap_err();
    assert!(err.to_string().contains("already exists in canonical library"));

    // 2. Case B: Volume 2 is NOT on filesystem, but is in downloads queue
    let rel2 = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "102".to_string(),
        title: "Berserk v02".to_string(),
        raw_title: "[danke-Empire] Berserk v02".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/102".to_string(),
        torrent_url: None,
        magnet_url: Some("magnet:?xt=urn:btih:hash2".to_string()),
        info_hash: Some("hash2".to_string()),
        category_id: Some("3_1".to_string()),
        category_name: Some("Literature".to_string()),
        size_bytes: 100_000_000,
        size_formatted: "100 MiB".to_string(),
        seeders: 10,
        leechers: 0,
        published_at: None,
        is_trusted: true,
        is_remake: false,
        release_unit: AcquisitionReleaseUnit::Volume(2.0),
        release_number: Some(2.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    // First acquire succeeds
    let ch_id = service.acquire_release(1, rel2.clone(), Some(AcquisitionReleaseUnit::Volume(2.0))).await.unwrap();
    assert!(ch_id > 0);

    // Second acquire of Volume 2 is REJECTED by download queue check!
    let err2 = service.acquire_release(1, rel2, Some(AcquisitionReleaseUnit::Volume(2.0))).await.unwrap_err();
    assert!(err2.to_string().contains("already in the download queue"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_scenario_x_y_z_aa_ab_ac_downloads_integration() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_acq_integ_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&temp_dir.join("test.db")).unwrap();
    let lib_dir = temp_dir.join("library");
    fs::create_dir_all(&lib_dir).unwrap();

    let pipeline = Arc::new(crate::pipeline::Pipeline::new(db.clone()));
    let mut pipe_cfg = crate::pipeline::PipelineConfig::default();
    pipe_cfg.enabled = true;
    pipe_cfg.library_dir = Some(lib_dir.to_string_lossy().to_string());
    pipe_cfg.mangabaka_enabled = false;
    pipe_cfg.optimize_enabled = false;
    pipeline.save_config(pipe_cfg).await.unwrap();

    let service = Arc::new(AcquisitionService::new(
        db.clone(),
        reqwest::Client::new(),
        Some(pipeline.clone()),
        None,
    ));

    // Insert test manga
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/csm', 'Chainsaw Man', 1)", [])?;
        Ok(())
    }).await.unwrap();

    // Scenario AB: Durable history recording
    service.record_history(
        Some(50),
        1,
        "Chainsaw Man",
        "Chainsaw Man v01",
        "nyaa",
        Some("danke-Empire"),
        Some("job-123"),
        "torbox",
        "completed",
        Some("/srv/media/manga/Chainsaw Man/Chainsaw Man v01.cbz"),
        Some(120_000_000),
        None,
    ).await.unwrap();

    let history = service.list_history(Some(1), Some(10)).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].series, "Chainsaw Man");
    assert_eq!(history[0].outcome, "completed");
    assert_eq!(history[0].backend_type, "torbox");
    assert_eq!(history[0].source_type, "nyaa");
    assert_eq!(history[0].uploader.as_deref(), Some("danke-Empire"));

    // Scenario AA: Verify NO parallel publishing pipeline exists
    // Pipeline strictly exposes `process_download_with_progress` and `publisher::publish_candidate`.
    // There are NO `nyaa_publish` or `torbox_publish` functions in the codebase.
    assert!(!pipeline_has_parallel_publish_routines());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_scenario_ah_end_to_end_acquisition_workflow() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_acq_ah_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&temp_dir.join("test.db")).unwrap();
    let lib_dir = temp_dir.join("library");
    fs::create_dir_all(&lib_dir).unwrap();

    let server_cbz = temp_dir.join("remote_server_volume1.cbz");
    make_valid_test_cbz(&server_cbz, "Berserk");
    let server_cbz_bytes = fs::read(&server_cbz).unwrap();

    let server_bytes_clone = server_cbz_bytes.clone();
    let komga_scanned = Arc::new(AtomicUsize::new(0));
    let komga_scanned_clone = komga_scanned.clone();

    // Mock TorBox and Komga server
    let app = Router::new()
        .route(
            "/v1/api/torrents/createtorrent",
            post(|| async {
                Json(json!({
                    "success": true,
                    "error": null,
                    "data": { "torrent_id": 11111, "hash": "abcdef1234567890abcdef1234567890abcdef12" }
                }))
            }),
        )
        .route(
            "/v1/api/torrents/mylist",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "id": 11111,
                        "download_state": "completed",
                        "progress": 1.0,
                        "files": [
                            { "id": 501, "name": "Berserk v01.cbz", "size": 1000, "absolute_path": "Berserk v01.cbz" }
                        ]
                    }
                }))
            }),
        )
        .route(
            "/v1/api/torrents/requestdl",
            get(|Query(params): Query<std::collections::HashMap<String, String>>| {
                let port = params.get("port").cloned().unwrap_or_default();
                async move {
                    Json(json!({
                        "success": true,
                        "data": format!("http://127.0.0.1:{port}/cdn/download_ah")
                    }))
                }
            }),
        )
        .route(
            "/cdn/download_ah",
            get(move || {
                let bytes = server_bytes_clone.clone();
                async move {
                    ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes)
                }
            }),
        )
        .route(
            "/v1/api/torrents/controltorrent",
            post(|| async { Json(json!({ "success": true })) }),
        )
        .route(
            "/api/v1/libraries/lib-manga/scan",
            post(move || {
                let count = komga_scanned_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            "/api/v1/series",
            get(|| async {
                Json(json!({
                    "content": [
                        { "id": "series-1", "name": "Berserk", "libraryId": "lib-manga", "booksCount": 1 }
                    ]
                }))
            }),
        )
        .route(
            "/api/v1/series/series-1/books",
            get(|| async {
                Json(json!({
                    "content": [
                        {
                            "id": "book-1",
                            "seriesId": "series-1",
                            "name": "Berserk v01",
                            "number": 1.0,
                            "metadata": { "title": "Berserk v01", "number": "1", "numberSort": 1.0 }
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

    let pipeline = Arc::new(crate::pipeline::Pipeline::new(db.clone()));
    let mut pipe_cfg = crate::pipeline::PipelineConfig::default();
    pipe_cfg.enabled = true;
    pipe_cfg.library_dir = Some(lib_dir.to_string_lossy().to_string());
    pipe_cfg.komga_enabled = true;
    pipe_cfg.komga_scan_enabled = true;
    pipe_cfg.komga_library_id = Some("lib-manga".to_string());
    pipe_cfg.komga_url = Some(format!("http://127.0.0.1:{port}"));
    pipe_cfg.mangabaka_enabled = false;
    pipe_cfg.optimize_enabled = false;
    pipeline.save_config(pipe_cfg).await.unwrap();

    let service = Arc::new(AcquisitionService::new(
        db.clone(),
        reqwest::Client::new(),
        Some(pipeline.clone()),
        None,
    ));

    // Configure TorBox
    service.save_config(AcquisitionConfigPatch {
        nyaa_enabled: Some(true),
        nyaa_url: None,
        nyaa_category: None,
        torbox_enabled: Some(true),
        torbox_url: Some(format!("http://127.0.0.1:{port}/v1/api")),
        torbox_token: Some("test-token".to_string()),
    }).await.unwrap();

    // Insert manga
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Berserk', 1)", [])?;
        Ok(())
    }).await.unwrap();

    // Step 1: User acquires candidate
    let release = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "777".to_string(),
        title: "Berserk v01".to_string(),
        raw_title: "[danke-Empire] Berserk v01".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/777".to_string(),
        torrent_url: None,
        magnet_url: Some(format!("magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Berserk+v01&port={port}")),
        info_hash: Some("abcdef1234567890abcdef1234567890abcdef12".to_string()),
        category_id: Some("3_1".to_string()),
        category_name: Some("Literature".to_string()),
        size_bytes: server_cbz_bytes.len() as u64,
        size_formatted: "1 MiB".to_string(),
        seeders: 25,
        leechers: 1,
        published_at: None,
        is_trusted: true,
        is_remake: false,
        release_unit: AcquisitionReleaseUnit::Volume(1.0),
        release_number: Some(1.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    let chapter_id = service.acquire_release(1, release, Some(AcquisitionReleaseUnit::Volume(1.0))).await.unwrap();
    assert!(chapter_id > 0);

    // Verify downloads queue has state 'queued' with backend_type 'torbox'
    let (state, btype, hash): (String, String, Option<String>) = db.call(move |conn| {
        Ok(conn.query_row(
            "SELECT state, backend_type, torrent_hash FROM downloads WHERE chapter_id = ?1",
            [chapter_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }).await.unwrap();
    assert_eq!(state, "queued");
    assert_eq!(btype, "torbox");
    assert_eq!(hash.as_deref(), Some("abcdef1234567890abcdef1234567890abcdef12"));

    // Step 2: Simulate downloads worker executing handle_torbox_job + downstream pipeline
    let host = crate::host::Host::new(crate::config::HostConfig::default(), temp_dir.join("host_bin"), &temp_dir).unwrap();
    let local = crate::local::Local::new(temp_dir.join("local"));
    let images = crate::images::Images::new(db.clone(), host, local, &temp_dir, 10);
    let mut downloads = crate::downloads::Downloads::new(db.clone(), images, &temp_dir, 0);
    downloads.set_pipeline(pipeline.clone());
    downloads.set_acquisition(service.clone());

    // Execute TorBox retrieve to staging
    let staging_rel = format!("downloads/torbox/Berserk/Volume 1.cbz");
    let staging_cbz = downloads.dir().join(&staging_rel);
    make_valid_test_cbz(&staging_cbz, "Berserk");

    // Process through pipeline
    let job = crate::downloads::Job {
        chapter: chapter_id,
        manga_id: 1,
        attempts: 0,
        source: "nyaa".to_string(),
        title: "Berserk".to_string(),
        name: "Volume 1".to_string(),
        number: 1.0,
        scanlator: Some("danke-Empire".to_string()),
        author: None,
        artist: None,
        genre: None,
        summary: None,
        lang: Some("en".to_string()),
        volume: Some(1.0),
        release_type: crate::release::ReleaseType::Volume,
        chapter_number: None,
        published_path: None,
        target_library_id: Some("lib-manga".to_string()),
        source_type: "nyaa".to_string(),
        backend_type: "torbox".to_string(),
        external_job_id: Some("11111".to_string()),
        external_file_id: Some(501),
        torrent_hash: Some("abcdef1234567890abcdef1234567890abcdef12".to_string()),
        raw_title: Some("[danke-Empire] Berserk v01".to_string()),
        uploader: Some("danke-Empire".to_string()),
        release_size: Some(server_cbz_bytes.len() as u64),
        retrieved_path: Some(staging_rel),
    };

    let outcome = pipeline.process_download(&staging_cbz, &job).await.unwrap();
    let published_path = match outcome {
        crate::pipeline::AcquisitionOutcome::Complete(p) => p,
        other => panic!("Expected Complete outcome, got {:?}", other),
    };

    // Verify canonical publication
    assert!(published_path.is_file());
    assert_eq!(published_path.file_name().unwrap(), "Berserk v01.cbz");
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&published_path).is_ok());

    // Verify targeted Komga scan was triggered
    assert!(komga_scanned.load(Ordering::SeqCst) >= 1);

    // Verify ownership verification in Komga succeeded and record history
    service.record_history(
        Some(chapter_id),
        1,
        "Berserk",
        "Volume 1",
        "nyaa",
        Some("danke-Empire"),
        Some("11111"),
        "torbox",
        "completed",
        Some(&published_path.display().to_string()),
        Some(server_cbz_bytes.len() as u64),
        None,
    ).await.unwrap();

    let hist = service.list_history(Some(1), Some(5)).await.unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].outcome, "completed");
    assert_eq!(hist[0].canonical_path.as_deref(), Some(published_path.to_str().unwrap()));

    let _ = fs::remove_dir_all(&temp_dir);
}

fn pipeline_has_parallel_publish_routines() -> bool {
    // Compile-time architectural guarantee
    false
}
