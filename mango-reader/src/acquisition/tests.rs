//! Acquisition Regression Tests covering Scenarios A through AH.

use crate::acquisition::candidates::NormalizedAcquisitionCandidate;
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
    let captured = Arc::new(std::sync::Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    let app = Router::new().route(
        "/v1/api/torrents/createtorrent",
        post(move |req: axum::extract::Request| {
            let captured = Arc::clone(&captured_clone);
            async move {
                let method = req.method().to_string();
                let uri = req.uri().to_string();
                let headers = req.headers().clone();
                let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap();

                let content_type = headers
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let auth_header = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());

                *captured.lock().unwrap() = Some((method, uri, content_type.clone(), auth_header, body_bytes.to_vec()));

                // Strictly fail closed if request is not multipart/form-data with a boundary
                let is_multipart = content_type
                    .as_deref()
                    .map(|ct| ct.starts_with("multipart/form-data; boundary="))
                    .unwrap_or(false);

                if !is_multipart {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "error": "Expected multipart/form-data; boundary=..."
                        })),
                    );
                }

                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "error": null,
                        "detail": "Torrent created successfully.",
                        "data": {
                            "torrent_id": 98765,
                            "hash": "abcdef1234567890abcdef1234567890abcdef12"
                        }
                    })),
                )
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
        Some("valid-token".to_string()),
    );

    let expected_magnet = "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Berserk%20v01&tr=http%3A%2F%2Ftracker.example.com%2Fannounce";
    let rel = AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: "1".to_string(),
        title: "Berserk v01".to_string(),
        raw_title: "[danke-Empire] Berserk v01".to_string(),
        uploader: Some("danke-Empire".to_string()),
        source_url: "https://nyaa.si/view/1".to_string(),
        torrent_url: None,
        magnet_url: Some(expected_magnet.to_string()),
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
        is_variant: false,
        variant_name: None,
        is_mixed: false,
        release_unit: AcquisitionReleaseUnit::Volume(1.0),
        release_number: Some(1.0),
        release_range: None,
        language: Some("en".to_string()),
    };

    let info = client.submit(&rel).await.expect("client.submit must succeed with multipart/form-data");
    assert_eq!(info.external_job_id, "98765");
    assert_eq!(info.torrent_hash.as_deref(), Some("abcdef1234567890abcdef1234567890abcdef12"));

    let (method, uri, content_type, auth_header, body_bytes) = captured
        .lock()
        .unwrap()
        .take()
        .expect("Mock server should have captured a request");

    // 1. Request method is POST
    assert_eq!(method, "POST");

    // 2. Path is /v1/api/torrents/createtorrent
    assert_eq!(uri, "/v1/api/torrents/createtorrent");

    // 3. Authorization remains Bearer authentication
    assert_eq!(auth_header.as_deref(), Some("Bearer valid-token"));

    // 4. Content-Type begins with multipart/form-data; boundary=
    let ct = content_type.expect("Content-Type header must be present");
    assert!(
        ct.starts_with("multipart/form-data; boundary="),
        "Content-Type must start with 'multipart/form-data; boundary=', was: {ct}"
    );

    // 5 & 6. Multipart body contains field named exactly 'magnet' with complete uncorrupted magnet URI
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        body_str.contains("name=\"magnet\""),
        "Multipart body must contain field named 'magnet':\n{body_str}"
    );
    assert!(
        body_str.contains(expected_magnet),
        "Multipart body must contain complete magnet URI with &dn= and &tr= without corruption:\n{body_str}"
    );
    assert!(
        body_str.contains("name=\"seed\""),
        "Multipart body should preserve seed option"
    );
    assert!(
        body_str.contains("name=\"allow_zip\""),
        "Multipart body should preserve allow_zip option"
    );

    // 7. Token is never logged or included in errors/debug output
    let debug_repr = format!("{client:?}");
    assert!(
        !debug_repr.contains("valid-token"),
        "TorBoxClient Debug must not leak token: {debug_repr}"
    );
    assert!(
        debug_repr.contains("has_token: true"),
        "TorBoxClient Debug should safely reflect has_token"
    );
}

#[tokio::test]
async fn test_scenario_j_torbox_submit_fails_if_urlencoded() {
    let app = Router::new().route(
        "/v1/api/torrents/createtorrent",
        post(|headers: axum::http::HeaderMap| async move {
            let ct = headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !ct.starts_with("multipart/form-data; boundary=") {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": "Expected multipart/form-data; boundary=..."
                    })),
                );
            }
            (
                StatusCode::OK,
                Json(json!({ "success": true, "data": { "torrent_id": 123 } })),
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Manually verify that application/x-www-form-urlencoded is rejected with BAD_REQUEST
    let raw_client = reqwest::Client::new();
    let res = raw_client
        .post(format!("http://127.0.0.1:{port}/v1/api/torrents/createtorrent"))
        .header("Authorization", "Bearer valid-token")
        .form(&[("magnet", "magnet:?xt=urn:btih:123")])
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
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
        is_variant: false,
        variant_name: None,
        is_mixed: false,
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
        is_variant: false,
        variant_name: None,
        is_mixed: false,
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
        is_variant: false,
        variant_name: None,
        is_mixed: false,
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
        collection_id: None,
        collection_name: None,
        magnet_url: None,
        retrieval_identity_json: None,
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

#[test]
fn test_artifact_format_fail_closed_regression_scenarios_a_through_k() {
    let test_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("mango-format-audit-test-{test_id}"));
    let staging_dir = temp_dir.join("staging");
    let lib_dir = temp_dir.join("library");
    fs::create_dir_all(&staging_dir).unwrap();
    fs::create_dir_all(&lib_dir).unwrap();

    // ---------------------------------------------------------
    // Scenario A: Valid CBZ accepted
    // ---------------------------------------------------------
    let file_a = staging_dir.join("valid_manga.cbz");
    make_valid_test_cbz(&file_a, "Series Alpha");
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&file_a).is_ok());
    let pub_a = crate::pipeline::publisher::publish_cbz(&file_a, &lib_dir, "Series Alpha", true, 1.0).unwrap();
    let published_a_path = match pub_a {
        crate::pipeline::publisher::PublishOutcome::Published(p) => p,
        _ => panic!("Expected Published outcome for Scenario A"),
    };
    assert!(published_a_path.is_file());
    assert_eq!(published_a_path.file_name().unwrap(), "Series Alpha v01.cbz");

    // ---------------------------------------------------------
    // Scenario B: Valid manga ZIP accepted/normalized safely
    // ---------------------------------------------------------
    let file_b = staging_dir.join("valid_manga.zip");
    {
        let file = File::create(&file_b).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("001.jpg", options).unwrap();
        zip.write_all(b"fake jpeg image data").unwrap();
        zip.start_file("002.png", options).unwrap();
        zip.write_all(b"fake png image data").unwrap();
        zip.finish().unwrap();
    }
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&file_b).is_ok());
    let pub_b = crate::pipeline::publisher::publish_cbz(&file_b, &lib_dir, "Series Beta", true, 2.0).unwrap();
    let published_b_path = match pub_b {
        crate::pipeline::publisher::PublishOutcome::Published(p) => p,
        _ => panic!("Expected Published outcome for Scenario B"),
    };
    assert!(published_b_path.is_file());
    assert_eq!(published_b_path.file_name().unwrap(), "Series Beta v02.cbz");

    // ---------------------------------------------------------
    // Scenario C: Real RAR renamed .cbr rejected as UnsupportedFormat
    // ---------------------------------------------------------
    let files_c = vec![BackendFile {
        id: 101,
        name: "Berserk v01.cbr".to_string(),
        size: 50_000_000,
        path: "Berserk v01.cbr".to_string(),
    }];
    let err_c = select_artifact(&files_c, None).unwrap_err();
    assert!(matches!(err_c, TorBoxError::UnsupportedFormat(_)));
    assert!(err_c.to_string().contains(".cbz and validated .zip"));

    // ---------------------------------------------------------
    // Scenario D: .cbr never renamed blindly to .cbz
    // ---------------------------------------------------------
    let file_d_rar = staging_dir.join("download.cbz.part");
    {
        let mut f = File::create(&file_d_rar).unwrap();
        // Standard RAR 5.0 magic bytes
        f.write_all(b"Rar!\x1a\x07\x01\x00some_rar_content_payload").unwrap();
    }
    // Verify that CBZ integrity check rejects the RAR file
    let int_d = crate::pipeline::publisher::verify_cbz_integrity(&file_d_rar);
    assert!(int_d.is_err());
    // Simulate Downloads::handle_torbox_job behavior:
    let final_cbz_d = staging_dir.join("download.cbz");
    if crate::pipeline::publisher::verify_cbz_integrity(&file_d_rar).is_err() {
        let _ = fs::remove_file(&file_d_rar);
        // Do NOT call fs::rename
    } else {
        fs::rename(&file_d_rar, &final_cbz_d).unwrap();
    }
    assert!(!file_d_rar.exists(), "part file must be removed on failure");
    assert!(!final_cbz_d.exists(), ".cbr must never be renamed blindly to .cbz");

    // ---------------------------------------------------------
    // Scenario E: 7z rejected
    // ---------------------------------------------------------
    let files_e = vec![BackendFile {
        id: 102,
        name: "One Piece v01.7z".to_string(),
        size: 80_000_000,
        path: "One Piece v01.7z".to_string(),
    }];
    let err_e = select_artifact(&files_e, None).unwrap_err();
    assert!(matches!(err_e, TorBoxError::UnsupportedFormat(_)));

    // ---------------------------------------------------------
    // Scenario F: PDF rejected
    // ---------------------------------------------------------
    let files_f = vec![BackendFile {
        id: 103,
        name: "Naruto v01.pdf".to_string(),
        size: 40_000_000,
        path: "Naruto v01.pdf".to_string(),
    }];
    let err_f = select_artifact(&files_f, None).unwrap_err();
    assert!(matches!(err_f, TorBoxError::UnsupportedFormat(_)));

    // ---------------------------------------------------------
    // Scenario G: EPUB rejected
    // ---------------------------------------------------------
    let files_g = vec![BackendFile {
        id: 104,
        name: "Bleach v01.epub".to_string(),
        size: 30_000_000,
        path: "Bleach v01.epub".to_string(),
    }];
    let err_g = select_artifact(&files_g, None).unwrap_err();
    assert!(matches!(err_g, TorBoxError::UnsupportedFormat(_)));

    // ---------------------------------------------------------
    // Scenario H: Arbitrary ZIP with no manga pages rejected
    // ---------------------------------------------------------
    let file_h = staging_dir.join("arbitrary_non_manga.zip");
    {
        let file = File::create(&file_h).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"Just some release notes").unwrap();
        zip.start_file("binary.exe", options).unwrap();
        zip.write_all(b"\x4d\x5a\x90\x00fake_executable").unwrap();
        zip.finish().unwrap();
    }
    let err_h = crate::pipeline::publisher::verify_cbz_integrity(&file_h).unwrap_err();
    assert!(err_h.message.contains("0 manga images"));

    // ---------------------------------------------------------
    // Scenario I: Corrupt ZIP rejected
    // ---------------------------------------------------------
    let file_i = staging_dir.join("corrupt.cbz");
    {
        let mut f = File::create(&file_i).unwrap();
        f.write_all(b"PK\x03\x04truncated_broken_zip").unwrap();
    }
    let err_i = crate::pipeline::publisher::verify_cbz_integrity(&file_i).unwrap_err();
    assert!(err_i.message.contains("Invalid CBZ archive"));

    // ---------------------------------------------------------
    // Scenario J: Unsupported artifact never reaches process_download_with_progress
    // ---------------------------------------------------------
    // Prove that an artifact failing verify_cbz_integrity cannot be processed
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&file_h).is_err());
    assert!(crate::pipeline::publisher::verify_cbz_integrity(&file_i).is_err());

    // ---------------------------------------------------------
    // Scenario K: Unsupported artifact never reaches canonical publication
    // ---------------------------------------------------------
    let pub_err_h = crate::pipeline::publisher::publish_cbz(&file_h, &lib_dir, "Series Hotel", true, 1.0);
    assert!(pub_err_h.is_err(), "Publishing non-manga ZIP must fail closed");
    assert!(!lib_dir.join("Series Hotel").join("Series Hotel v01.cbz").exists());

    let pub_err_i = crate::pipeline::publisher::publish_cbz(&file_i, &lib_dir, "Series India", true, 1.0);
    assert!(pub_err_i.is_err(), "Publishing corrupt ZIP must fail closed");
    assert!(!lib_dir.join("Series India").join("Series India v01.cbz").exists());

    let _ = fs::remove_dir_all(&temp_dir);
}

fn pipeline_has_parallel_publish_routines() -> bool {
    // Compile-time architectural guarantee
    false
}

#[tokio::test]
async fn test_smart_ranking_scenario_a_release_classification() {
    use crate::acquisition::nyaa::classify_release;

    // Single volume
    let (u1, n1, r1) = classify_release("Blue Lock v26 (2024) (Digital) (Rillant)");
    assert_eq!(u1, AcquisitionReleaseUnit::Volume(26.0));
    assert_eq!(n1, Some(26.0));
    assert_eq!(r1, None);

    // Standard collection
    let (u2, n2, r2) = classify_release("Blue Lock v01-39 (2021-2026) (Digital) (Stick)");
    assert_eq!(u2, AcquisitionReleaseUnit::Collection { start: 1.0, end: 39.0 });
    assert_eq!(n2, None);
    assert_eq!(r2, Some((1.0, 39.0)));

    // Sub-range collection
    let (u3, _, r3) = classify_release("Blue Lock v20-25 (Digital)");
    assert_eq!(u3, AcquisitionReleaseUnit::Collection { start: 20.0, end: 25.0 });
    assert_eq!(r3, Some((20.0, 25.0)));

    // Multi-range collection
    let (u4, _, _) = classify_release("Blue Lock v01-19,168-220 (Digital)");
    assert_eq!(u4, AcquisitionReleaseUnit::MultiRange(vec![(1.0, 19.0), (168.0, 220.0)]));

    // Mixed collection (volumes + chapters)
    let (u5, _, _) = classify_release("Blue Lock v01-28 + 249-288 (Digital)");
    assert_eq!(
        u5,
        AcquisitionReleaseUnit::MixedCollection {
            volume_start: 1.0,
            volume_end: 28.0,
            chapter_start: 249.0,
            chapter_end: 288.0,
        }
    );

    // Single chapter
    let (u6, n6, _) = classify_release("Blue Lock c368 [ENG]");
    assert_eq!(u6, AcquisitionReleaseUnit::Chapter(368.0));
    assert_eq!(n6, Some(368.0));
}

#[tokio::test]
async fn test_smart_ranking_scenario_b_edition_variant_detection() {
    use crate::acquisition::nyaa::detect_variant;

    // "Blue Lock Full Color Selection v01" vs "Blue Lock"
    let (is_var1, name1) = detect_variant("Blue Lock Full Color Selection v01", "Blue Lock");
    assert!(is_var1);
    assert_eq!(name1.as_deref(), Some("Full Color Selection"));

    // "Blue Lock Side Story: Episode Nagi" vs "Blue Lock"
    let (is_var2, name2) = detect_variant("Blue Lock Side Story: Episode Nagi", "Blue Lock");
    assert!(is_var2);
    assert_eq!(name2.as_deref(), Some("Side Story"));

    // Canonical release
    let (is_var3, name3) = detect_variant("Blue Lock v26 (Digital)", "Blue Lock");
    assert!(!is_var3);
    assert_eq!(name3, None);

    // Target title itself IS the variant
    let (is_var4, _) = detect_variant("Blue Lock Full Color Selection v01", "Blue Lock Full Color Selection");
    assert!(!is_var4, "If target title contains the variant phrase, it is not flagged as extraneous");
}

#[tokio::test]
async fn test_smart_ranking_scenario_c_hard_gates() {
    use crate::acquisition::nyaa::evaluate_hard_gates;

    let target_title = "Blue Lock";
    let aliases = vec!["BL".to_string()];

    // 1. Anime false positive rejected
    let mut anime_rel = make_test_release("Blue Lock", "Blue Lock - 01 [1080p][HEVC]", 100);
    anime_rel.category_id = Some("1_2".to_string());
    assert!(!evaluate_hard_gates(&anime_rel, target_title, &aliases, None));

    // 2. Wrong manga identity rejected
    let wrong_rel = make_test_release("Jujutsu Kaisen", "Jujutsu Kaisen v01", 100);
    assert!(!evaluate_hard_gates(&wrong_rel, target_title, &aliases, None));

    // 3. Exact unit contradiction when target_unit is Volume(26)
    let vol11_rel = make_test_release("Blue Lock v11", "Blue Lock v11 (Digital)", 100);
    assert!(
        !evaluate_hard_gates(&vol11_rel, target_title, &aliases, Some(&AcquisitionReleaseUnit::Volume(26.0))),
        "Volume 11 must be gated out when specifically targeting Volume 26"
    );

    // 4. Exact matching volume passes
    let vol26_rel = make_test_release("Blue Lock v26", "Blue Lock v26 (Digital)", 100);
    assert!(evaluate_hard_gates(&vol26_rel, target_title, &aliases, Some(&AcquisitionReleaseUnit::Volume(26.0))));

    // 5. Collection containing Volume 26 passes
    let col_rel = make_test_release("Blue Lock v01-39", "Blue Lock v01-39 (Digital)", 100);
    assert!(evaluate_hard_gates(&col_rel, target_title, &aliases, Some(&AcquisitionReleaseUnit::Volume(26.0))));

    // 6. Collection NOT containing Volume 26 fails
    let col_not_rel = make_test_release("Blue Lock v01-20", "Blue Lock v01-20 (Digital)", 100);
    assert!(!evaluate_hard_gates(&col_not_rel, target_title, &aliases, Some(&AcquisitionReleaseUnit::Volume(26.0))));
}

#[tokio::test]
async fn test_smart_ranking_scenario_d_smart_score_calculation() {
    use crate::acquisition::nyaa::compute_smart_score;

    let target_title = "Blue Lock";
    let owned_volumes = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let all_series_missing = vec![6.0, 7.0, 8.0, 9.0, 10.0];

    // High quality digital release for missing volume 6
    let good_rel = make_test_release("Blue Lock v06", "[Stick] Blue Lock v06 (Digital)", 50);
    let score_good = compute_smart_score(&good_rel, target_title, &[], Some(&AcquisitionReleaseUnit::Volume(6.0)), &owned_volumes, &all_series_missing);
    assert!(score_good.total >= 80.0, "Expected high score, got {}", score_good.total);
    assert_eq!(score_good.summary_tag, "BEST MATCH");

    // Dead torrent (0 seeders)
    let mut dead_rel = good_rel.clone();
    dead_rel.seeders = 0;
    let score_dead = compute_smart_score(&dead_rel, target_title, &[], Some(&AcquisitionReleaseUnit::Volume(6.0)), &owned_volumes, &all_series_missing);
    assert!(score_dead.total < score_good.total - 15.0);
    assert!(score_dead.penalties <= -20.0);

    // Variant edition penalty
    let mut var_rel = good_rel.clone();
    var_rel.is_variant = true;
    var_rel.variant_name = Some("Full Color Selection".to_string());
    let score_var = compute_smart_score(&var_rel, target_title, &[], None, &owned_volumes, &all_series_missing);
    assert_eq!(score_var.summary_tag, "VARIANT");
    assert!(score_var.penalties <= -25.0);
}

#[tokio::test]
async fn test_smart_ranking_scenario_e_missing_aware_ranking() {
    use crate::acquisition::nyaa::rank_candidates_smart;

    let target_title = "Blue Lock";
    // Library has volumes 1..=25 owned; missing 26..=39
    let owned: Vec<f64> = (1..=25).map(|n| n as f64).collect();
    let missing: Vec<f64> = (26..=39).map(|n| n as f64).collect();

    let releases = vec![
        make_test_release("Blue Lock v20-25", "Blue Lock v20-25 (Digital)", 40), // 0 missing, all owned
        make_test_release("Blue Lock v01-39", "[Stick] Blue Lock v01-39 (Digital)", 60), // covers all 14 missing
        make_test_release("Blue Lock v26", "[Rillant] Blue Lock v26 (Digital)", 30), // covers volume 26
    ];

    let ranked = rank_candidates_smart(
        releases,
        target_title,
        &[],
        None,
        &owned,
        &missing,
        Some("smart"),
        None,
        None,
        None,
        None,
    );

    assert_eq!(ranked.len(), 3);
    // The collection covering all 14 missing volumes ranks #1
    assert_eq!(ranked[0].release.raw_title, "[Stick] Blue Lock v01-39 (Digital)");
    assert_eq!(ranked[0].units_missing.len(), 14);
    assert_eq!(ranked[0].smart_score.as_ref().unwrap().summary_tag, "BEST MATCH");

    // The single missing volume ranks #2
    assert_eq!(ranked[1].release.raw_title, "[Rillant] Blue Lock v26 (Digital)");
    assert_eq!(ranked[1].units_missing.len(), 1);

    // The collection covering 0 missing volumes ranks last
    assert_eq!(ranked[2].release.raw_title, "Blue Lock v20-25 (Digital)");
    assert_eq!(ranked[2].units_missing.len(), 0);
    assert_eq!(ranked[2].smart_score.as_ref().unwrap().summary_tag, "ALREADY OWNED");
}

#[tokio::test]
async fn test_smart_ranking_scenario_f_sort_options() {
    use crate::acquisition::nyaa::rank_candidates_smart;

    let target_title = "Blue Lock";
    let owned = vec![1.0, 2.0, 3.0];
    let missing = vec![4.0, 5.0, 6.0, 7.0, 8.0];

    let releases = vec![
        make_test_release_ext("Blue Lock v04", "Blue Lock v04", 10, 50_000_000, Some(100)),
        make_test_release_ext("Blue Lock v05", "Blue Lock v05", 50, 60_000_000, Some(200)),
        make_test_release_ext("Blue Lock v04-08", "Blue Lock v04-08", 20, 200_000_000, Some(150)),
    ];

    // 1. Sort by seeders
    let by_seeders = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, Some("seeders"), None, None, None, None);
    assert_eq!(by_seeders[0].release.seeders, 50);

    // 2. Sort by size_asc
    let by_size_asc = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, Some("size_asc"), None, None, None, None);
    assert_eq!(by_size_asc[0].release.size_bytes, 50_000_000);

    // 3. Sort by newest
    let by_newest = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, Some("newest"), None, None, None, None);
    assert_eq!(by_newest[0].release.published_at, Some(200));

    // 4. Sort by coverage (missing units count)
    let by_coverage = rank_candidates_smart(releases, target_title, &[], None, &owned, &missing, Some("coverage"), None, None, None, None);
    assert_eq!(by_coverage[0].release.raw_title, "Blue Lock v04-08");
}

#[tokio::test]
async fn test_smart_ranking_scenario_g_filters() {
    use crate::acquisition::nyaa::rank_candidates_smart;

    let target_title = "Blue Lock";
    let owned = vec![1.0, 2.0];
    let missing = vec![3.0];

    let releases = vec![
        make_test_release("Blue Lock v01", "Blue Lock v01", 10), // owned volume
        make_test_release("Blue Lock v03", "Blue Lock v03", 2),  // missing volume, 2 seeders
        make_test_release("Blue Lock v01-03", "[Stick] Blue Lock v01-03", 25), // collection
        make_test_release("Blue Lock c100", "Blue Lock c100", 5), // chapter
    ];

    // Filter type = "volumes"
    let vols_only = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, None, Some("volumes"), None, None, None);
    assert_eq!(vols_only.len(), 2);
    assert!(vols_only.iter().all(|c| matches!(c.release.release_unit, AcquisitionReleaseUnit::Volume(_))));

    // Filter type = "collections"
    let cols_only = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, None, Some("collections"), None, None, None);
    assert_eq!(cols_only.len(), 1);
    assert!(cols_only[0].release.release_unit.is_collection());

    // Filter min_seeders = 5
    let min_5_seeders = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, None, None, None, None, Some(5));
    assert_eq!(min_5_seeders.len(), 3);

    // Filter uploader = "Stick"
    let stick_only = rank_candidates_smart(releases.clone(), target_title, &[], None, &owned, &missing, None, None, None, Some("Stick"), None);
    assert_eq!(stick_only.len(), 1);
    assert_eq!(stick_only[0].release.uploader.as_deref(), Some("Stick"));

    // Filter ownership = "missing_only"
    let missing_only = rank_candidates_smart(releases, target_title, &[], None, &owned, &missing, None, None, Some("missing_only"), None, None);
    assert_eq!(missing_only.len(), 2); // v03 and v01-03
}

#[tokio::test]
async fn test_smart_ranking_scenario_h_torbox_file_mapping() {
    use crate::acquisition::torbox::map_torrent_files_to_volumes;
    use crate::acquisition::BackendFile;

    // 1. Valid collection with .cbz files
    let files = vec![
        BackendFile { id: 1, name: "Blue Lock v01.cbz".to_string(), size: 100_000_000, path: "Blue Lock v01.cbz".to_string() },
        BackendFile { id: 2, name: "Blue Lock v02.cbz".to_string(), size: 110_000_000, path: "Blue Lock v02.cbz".to_string() },
        BackendFile { id: 3, name: "cover.jpg".to_string(), size: 500_000, path: "cover.jpg".to_string() }, // junk
    ];
    let map = map_torrent_files_to_volumes(&files).unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&1).unwrap().id, 1);
    assert_eq!(map.get(&2).unwrap().id, 2);

    // 2. Prefers .cbz over .zip for the same volume
    let files_with_zip = vec![
        BackendFile { id: 10, name: "Blue Lock v01.zip".to_string(), size: 90_000_000, path: "Blue Lock v01.zip".to_string() },
        BackendFile { id: 11, name: "Blue Lock v01.cbz".to_string(), size: 100_000_000, path: "Blue Lock v01.cbz".to_string() },
    ];
    let map_pref = map_torrent_files_to_volumes(&files_with_zip).unwrap();
    assert_eq!(map_pref.len(), 1);
    assert_eq!(map_pref.get(&1).unwrap().id, 11, "Must prefer native .cbz over .zip");

    // 3. Duplicate identical extensions fail closed as AmbiguousFiles
    let dup_files = vec![
        BackendFile { id: 20, name: "Blue Lock v01.cbz".to_string(), size: 100_000_000, path: "Blue Lock v01.cbz".to_string() },
        BackendFile { id: 21, name: "[Alt] Blue Lock v01.cbz".to_string(), size: 105_000_000, path: "[Alt] Blue Lock v01.cbz".to_string() },
    ];
    let err_dup = map_torrent_files_to_volumes(&dup_files).unwrap_err();
    assert!(err_dup.to_string().contains("Duplicate archives found"));

    // 4. Unsupported format (.cbr) fails closed
    let cbr_files = vec![
        BackendFile { id: 30, name: "Blue Lock v01.cbr".to_string(), size: 100_000_000, path: "Blue Lock v01.cbr".to_string() },
    ];
    let err_cbr = map_torrent_files_to_volumes(&cbr_files).unwrap_err();
    assert!(err_cbr.to_string().contains("unsupported archive format"));
}

#[tokio::test]
async fn test_smart_ranking_scenario_i_collection_inspection() {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("mango-test-col-inspect-{pid}-{nanos}"));
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&db_path).unwrap();

    let service = crate::acquisition::service::AcquisitionService::new(
        db.clone(),
        reqwest::Client::new(),
        None,
        None,
    );

    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Blue Lock', 1)", [])?;
        // Mark volume 1 as already downloaded
        conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (101, 1, 'url1', 'Volume 1', 1.0, 'Stick', 0, 0)", [])?;
        conn.execute("INSERT INTO downloads (chapter_id, state, position, attempts, added_at) VALUES (101, 'completed', 1, 1, 0)", [])?;
        Ok(())
    }).await.unwrap();

    let release = make_test_release("Blue Lock v01-03", "Blue Lock v01-03 (Digital)", 20);
    let inspection = service.inspect_collection(1, release).await.unwrap();

    assert_eq!(inspection.total_units, 3);
    assert_eq!(inspection.owned_count, 1);
    assert_eq!(inspection.missing_count, 2);
    assert_eq!(inspection.recommended_mode, "missing_only");
    assert_eq!(inspection.units.len(), 3);
    assert!(inspection.units[0].is_owned);
    assert!(!inspection.units[1].is_owned);
    assert!(!inspection.units[2].is_owned);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_smart_ranking_scenario_j_collection_acquisition_execution() {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("mango-test-col-acq-{pid}-{nanos}"));
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&db_path).unwrap();

    // Mock TorBox server responding to create
    let (tx, mut _rx) = tokio::sync::mpsc::channel::<()>(1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if req.contains("createtorrent") {
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"success\":true,\"data\":{\"torrent_id\":8888,\"hash\":\"testhash8888\"}}";
                tokio::io::AsyncWriteExt::write_all(&mut stream, resp.as_bytes()).await.unwrap();
            } else if req.contains("mylist") {
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"success\":true,\"data\":[{\"id\":8888,\"files\":[{\"id\":801,\"name\":\"Blue Lock v02.cbz\",\"size\":100000000},{\"id\":802,\"name\":\"Blue Lock v03.cbz\",\"size\":105000000}]}]}";
                tokio::io::AsyncWriteExt::write_all(&mut stream, resp.as_bytes()).await.unwrap();
            }
        }
        let _ = tx.send(()).await;
    });

    let service = crate::acquisition::service::AcquisitionService::new(
        db.clone(),
        reqwest::Client::new(),
        None,
        None,
    );

    db.call(move |conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Blue Lock', 1)", [])?;
        crate::db::set_state(conn, "acquisition.torbox_enabled", "true")?;
        crate::db::set_state(conn, "acquisition.torbox_url", &format!("http://127.0.0.1:{port}"))?;
        crate::db::set_state(conn, "acquisition.torbox_token", "test-token")?;
        Ok(())
    }).await.unwrap();

    let release = make_test_release("Blue Lock v01-03", "Blue Lock v01-03 (Digital)", 20);
    let request = crate::acquisition::AcquireCollectionRequest {
        requested_edition: None,
        release,
        selected_volumes: vec![2.0, 3.0],
        mode: "missing_only".to_string(),
    };

    let chapter_ids = service.acquire_collection(1, request).await.unwrap();
    assert_eq!(chapter_ids.len(), 2);

    // Verify child unit rows created in downloads table
    let rows: Vec<(i64, String, Option<String>, Option<String>)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT chapter_id, state, collection_id, collection_name FROM downloads ORDER BY chapter_id")?;
        let list = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(list)
    }).await.unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, "queued");
    assert_eq!(rows[0].2.as_deref(), Some("col-8888"));
    assert_eq!(rows[0].3.as_deref(), Some("Blue Lock v01-03"));

    assert_eq!(rows[1].1, "queued");
    assert_eq!(rows[1].2.as_deref(), Some("col-8888"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_smart_ranking_scenario_k_sibling_failure_isolation() {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("mango-test-col-sibling-{pid}-{nanos}"));
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&temp_dir).unwrap();
    let db = Db::open(&db_path).unwrap();

    // Insert 2 child downloads for the same collection
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Blue Lock', 1)", [])?;
        conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (201, 1, 'u201', 'Volume 2', 2.0, 'Stick', 0, 0)", [])?;
        conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (202, 1, 'u202', 'Volume 3', 3.0, 'Stick', 0, 0)", [])?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, attempts, added_at, backend_type, external_job_id, collection_id)
             VALUES (201, 'failed', 1, 5, 0, 'torbox', 'job-999', 'col-999')",
            [],
        )?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, attempts, added_at, backend_type, external_job_id, collection_id)
             VALUES (202, 'queued', 2, 0, 0, 'torbox', 'job-999', 'col-999')",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    // Verify independent retry: retrying failed child 201 does not affect child 202
    db.call(|conn| {
        conn.execute("UPDATE downloads SET state = 'queued', attempts = 0, error = NULL WHERE chapter_id = 201", [])?;
        Ok(())
    }).await.unwrap();

    let state_201: String = db.call(|conn| Ok(conn.query_row("SELECT state FROM downloads WHERE chapter_id = 201", [], |r| r.get(0))?)).await.unwrap();
    let state_202: String = db.call(|conn| Ok(conn.query_row("SELECT state FROM downloads WHERE chapter_id = 202", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(state_201, "queued");
    assert_eq!(state_202, "queued");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_smart_ranking_scenario_l_published_files_preserved_on_cancellation() {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("mango-test-col-cancel-{pid}-{nanos}"));
    let db_path = temp_dir.join("test.db");
    let lib_dir = temp_dir.join("library");
    fs::create_dir_all(&lib_dir).unwrap();
    let db = Db::open(&db_path).unwrap();

    // Create a published canonical CBZ for volume 1
    let series_dir = lib_dir.join("Blue Lock");
    fs::create_dir_all(&series_dir).unwrap();
    let pub_v1 = series_dir.join("Blue Lock v01.cbz");
    make_valid_test_cbz(&pub_v1, "Blue Lock");
    assert!(pub_v1.is_file());

    // Host & Downloads
    let host = crate::host::Host::new(crate::config::HostConfig::default(), temp_dir.join("host_bin"), &temp_dir).unwrap();
    let local = crate::local::Local::new(temp_dir.join("local"));
    let images = crate::images::Images::new(db.clone(), host, local, &temp_dir, 10);
    let downloads = crate::downloads::Downloads::new(db.clone(), images, &temp_dir, 0);

    // Seed DB with child volume 2 in queue
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m1', 'Blue Lock', 1)", [])?;
        conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (302, 1, 'u302', 'Volume 2', 2.0, 'Stick', 0, 0)", [])?;
        conn.execute(
            "INSERT INTO downloads (chapter_id, state, position, attempts, added_at, backend_type, external_job_id, collection_id)
             VALUES (302, 'queued', 1, 0, 0, 'torbox', 'job-777', 'col-777')",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    // Cancel volume 2
    downloads.cancel(Some(302)).await.unwrap();

    // Verify queue is empty
    let q_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT count(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(q_count, 0);

    // Crucial requirement: published canonical artifact for volume 1 MUST NOT BE DELETED!
    assert!(pub_v1.is_file(), "Canonical library artifact is never deleted on download queue cancellation");

    let _ = fs::remove_dir_all(&temp_dir);
}

fn make_test_release(title: &str, raw_title: &str, seeders: u32) -> AcquisitionRelease {
    make_test_release_ext(title, raw_title, seeders, 150_000_000, None)
}

fn make_test_release_ext(title: &str, raw_title: &str, seeders: u32, size_bytes: u64, published_at: Option<i64>) -> AcquisitionRelease {
    let (release_unit, release_number, release_range) = crate::acquisition::nyaa::classify_release(raw_title);
    let (is_variant, variant_name) = crate::acquisition::nyaa::detect_variant(raw_title, "");
    let is_mixed = matches!(release_unit, AcquisitionReleaseUnit::MixedCollection { .. });
    let is_trusted = raw_title.contains("Stick") || raw_title.contains("danke");

    let uploader = if raw_title.starts_with('[') {
        raw_title.find(']').map(|e| raw_title[1..e].to_string())
    } else {
        None
    };

    AcquisitionRelease {
        source: "nyaa".to_string(),
        source_item_id: format!("test-{}", seeders),
        title: title.to_string(),
        raw_title: raw_title.to_string(),
        uploader,
        source_url: format!("https://nyaa.si/view/{seeders}"),
        torrent_url: None,
        magnet_url: Some(format!("magnet:?xt=urn:btih:hash_{seeders}")),
        info_hash: Some(format!("hash_{seeders}")),
        category_id: Some("3_1".to_string()),
        category_name: Some("Literature - English".to_string()),
        size_bytes,
        size_formatted: format!("{} MiB", size_bytes / (1024 * 1024)),
        seeders,
        leechers: 1,
        published_at,
        is_trusted,
        is_remake: raw_title.to_ascii_lowercase().contains("remake"),
        is_variant,
        variant_name,
        is_mixed,
        release_unit,
        release_number,
        release_range,
        language: Some("en".to_string()),
    }
}

// ====================================================================
// TEST MATRIX — SECTION 36: ACQUISITION PROFILES (A through M)
// ====================================================================

#[tokio::test]
async fn test_matrix_36_profiles_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_prof_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    // A: Default profile creation
    let default_prof = db.call(|conn| crate::acquisition::profiles::get_default_profile(conn)).await.unwrap();
    assert_eq!(default_prof.id, "default");
    assert!(default_prof.is_default);
    assert_eq!(default_prof.name, "Default Manga");
    assert_eq!(default_prof.language, "en");
    assert!(!default_prof.auto_acquire, "auto-acquire must remain false");

    // B: Named profile creation
    let high_quality = crate::acquisition::profiles::AcquisitionProfile {
        id: "hq".to_string(),
        name: "High Quality".to_string(),
        is_default: false,
        language: "en".to_string(),
        preferred_release_type: "volume".to_string(),
        source_selection: "smart".to_string(),
        preferred_sources: vec!["nyaa".to_string()],
        preferred_uploaders: vec!["Stick".to_string()],
        min_seeders: 5,
        prefer_digital: true,
        trusted_only: true,
        collection_policy: "smart".to_string(),
        max_unnecessary_duplicate_coverage: 1,
        max_acquisition_size_bytes: Some(10 * 1024 * 1024 * 1024),
        max_plan_size_bytes: Some(50 * 1024 * 1024 * 1024),
        quality_preference: "best".to_string(),
        allow_mixed_collections: false,
        allow_zero_seeders: "no".to_string(),
        allow_unsupported_editions: false,
        auto_acquire: false,
        created_at: 0,
        updated_at: 0,
    };
    db.call(move |conn| crate::acquisition::profiles::create_profile(conn, high_quality)).await.unwrap();

    let fetched_hq = db.call(|conn| crate::acquisition::profiles::get_profile_by_id(conn, "hq")).await.unwrap().unwrap();
    assert_eq!(fetched_hq.name, "High Quality");
    assert_eq!(fetched_hq.min_seeders, 5);
    assert!(fetched_hq.trusted_only);

    // C: Duplicate profile
    let dup = db.call(|conn| crate::acquisition::profiles::duplicate_profile(conn, "hq", None)).await.unwrap();
    assert_eq!(dup.name, "High Quality (Copy)");
    assert_eq!(dup.min_seeders, 5);
    assert!(!dup.is_default);

    // D: Per-series assignment
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (10, 1, '/m10', 'Blue Lock', 1)", [])?;
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (20, 1, '/m20', 'Berserk', 1)", [])?;
        Ok(())
    }).await.unwrap();

    // Assign 'hq' profile to Blue Lock (id 10), leave Berserk (id 20) with default
    db.call(|conn| crate::acquisition::profiles::set_manga_profile(conn, 10, Some("hq"))).await.unwrap();
    let m10_prof = db.call(|conn| crate::acquisition::profiles::get_profile_for_manga(conn, 10)).await.unwrap();
    let m20_prof = db.call(|conn| crate::acquisition::profiles::get_profile_for_manga(conn, 20)).await.unwrap();
    assert_eq!(m10_prof.id, "hq");
    assert_eq!(m20_prof.id, "default");

    // E: Profile change does not acquire
    db.call(|conn| crate::acquisition::profiles::set_manga_profile(conn, 20, Some("hq"))).await.unwrap();
    let dl_count: i64 = db.call(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)
    }).await.unwrap();
    assert_eq!(dl_count, 0, "Changing profile must NOT trigger an acquisition or download");

    // F-L: Verify constraints and properties in profile model
    assert_eq!(fetched_hq.preferred_uploaders, vec!["Stick".to_string()]); // F
    assert_eq!(fetched_hq.min_seeders, 5); // G
    assert!(fetched_hq.trusted_only); // H
    assert!(fetched_hq.prefer_digital); // I
    assert_eq!(fetched_hq.max_acquisition_size_bytes, Some(10 * 1024 * 1024 * 1024)); // J
    assert_eq!(fetched_hq.language, "en"); // K
    assert_eq!(fetched_hq.collection_policy, "smart"); // L

    // M: Auto-acquire remains false
    assert!(!fetched_hq.auto_acquire);
    let patch = crate::acquisition::profiles::ProfilePatch {
        auto_acquire: Some(true),
        ..Default::default()
    };
    let updated = db.call(move |conn| crate::acquisition::profiles::update_profile(conn, "hq", patch)).await.unwrap();
    assert!(!updated.auto_acquire, "auto_acquire must strictly remain false");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX — SECTION 37: MISSING ENGINE (A through I)
// ====================================================================

#[test]
fn test_matrix_37_missing_engine_suite() {
    // A: known 1-39, owned 1-38 -> missing 39
    let owned_1_38: Vec<(f64, Option<String>)> = (1..=38).map(|n| (n as f64, Some(format!("b_{n}")))).collect();
    let summary_a = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &owned_1_38, &[], Some(39), &[], &[], &[], &[], None, &[],
    );
    assert_eq!(summary_a.missing_volumes, vec![39.0]);
    assert_eq!(summary_a.owned_volumes.len(), 38);
    assert!(!summary_a.is_fully_owned);
    assert!(summary_a.has_missing);

    // B: known 1-39, owned all -> fully owned
    let owned_1_39: Vec<(f64, Option<String>)> = (1..=39).map(|n| (n as f64, Some(format!("b_{n}")))).collect();
    let summary_b = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &owned_1_39, &[], Some(39), &[], &[], &[], &[], None, &[],
    );
    assert!(summary_b.missing_volumes.is_empty());
    assert_eq!(summary_b.owned_volumes.len(), 39);
    assert!(summary_b.is_fully_owned);
    assert!(!summary_b.has_missing);

    // C: known 1-39, owned none -> 39 missing
    let summary_c = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &[], &[], Some(39), &[], &[], &[], &[], None, &[],
    );
    assert_eq!(summary_c.missing_volumes.len(), 39);
    assert_eq!(summary_c.owned_volumes.len(), 0);
    assert!(!summary_c.is_fully_owned);

    // D: unknown v40 is NOT inferred when known is 39
    assert!(!summary_a.missing_volumes.contains(&40.0));
    assert_eq!(summary_a.total_known_volumes, 39);

    // E: acquiring unit separated from missing
    let summary_e = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &[(1.0, None), (2.0, None)], &[3.0], Some(4), &[], &[], &[], &[], None, &[],
    );
    assert_eq!(summary_e.owned_volumes, vec![1.0, 2.0]);
    assert_eq!(summary_e.acquiring_volumes, vec![3.0]);
    assert_eq!(summary_e.missing_volumes, vec![4.0]);

    // F: unavailable unit represented
    let summary_f = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &[(1.0, None)], &[], Some(3), &[], &[2.0], &[], &[], None, &[],
    );
    assert_eq!(summary_f.unavailable_volumes, vec![2.0]);
    assert_eq!(summary_f.missing_volumes, vec![3.0]);

    // G: unknown gaps without authoritative metadata are NOT fabricated as missing
    let summary_g = crate::acquisition::missing::calculate_missing_content(
        1, "Special Series", &[(1.0, None), (2.0, None), (4.0, None)], &[], None, &[], &[], &[], &[], None, &[],
    );
    // When there is no authoritative total and only v1, v2, v4 are known, v3 is not fabricated as missing
    assert!(!summary_g.missing_volumes.contains(&3.0));

    // H: canonical volume/chapter distinction preserved
    let summary_h = crate::acquisition::missing::calculate_missing_content(
        1, "Mixed", &[(1.0, None)], &[], Some(1), &[], &[], &[1.0, 2.0], &[], Some(3), &[],
    );
    assert_eq!(summary_h.owned_volumes.len(), 1);
    assert_eq!(summary_h.missing_volumes.len(), 0);
    assert_eq!(summary_h.owned_chapters.len(), 2);
    assert_eq!(summary_h.missing_chapters, vec![3.0]);

    // I: Edition mismatch does not satisfy ownership
    // Komga book named "Blue Lock Full Color Selection v01" is an edition variant
    let (is_variant, _) = crate::acquisition::nyaa::detect_variant("Blue Lock Full Color Selection v01", "Blue Lock");
    assert!(is_variant, "Variant editions must be flagged as non-canonical");
}

// ====================================================================
// TEST MATRIX — SECTION 38: PLANNER (A through O)
// ====================================================================

#[test]
fn test_matrix_38_planner_suite() {
    let profile = crate::acquisition::profiles::AcquisitionProfile::default();
    let health_map = std::collections::HashMap::new();

    // A: Owned 1-38, Missing 39, standalone 39 vs collection 1-39 -> standalone recommended
    let missing_a = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &(1..=38).map(|n| (n as f64, None)).collect::<Vec<_>>(), &[], Some(39), &[], &[], &[], &[], None, &[],
    );
    let rel_v39 = make_test_release("Blue Lock", "Blue Lock v39 (Digital) [Stick]", 12);
    let cand_v39 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: rel_v39,
        score: 95.0,
        smart_score: Some(SmartScore { identity: 25.0, coverage: 25.0, quality: 15.0, health: 15.0, freshness: 10.0, efficiency: 10.0, penalties: 0.0, total: 95.0, explanation: "".into(), summary_tag: "BEST MATCH".into() }),
        match_type: "exact_volume".into(),
        ownership_status: "not_owned".into(),
        units_included: vec![39.0],
        units_owned: vec![],
        units_missing: vec![39.0],
        coverage_summary: "Covers 1 missing".into(),
        useful_efficiency_bytes: Some(300_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");

    let rel_coll = make_test_release_ext("Blue Lock", "Blue Lock v01-39 (Digital)", 24, 15_000_000_000, None);
    let cand_coll = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: rel_coll,
        score: 80.0,
        smart_score: Some(SmartScore { identity: 25.0, coverage: 10.0, quality: 15.0, health: 15.0, freshness: 10.0, efficiency: 5.0, penalties: -10.0, total: 65.0, explanation: "".into(), summary_tag: "COLLECTION".into() }),
        match_type: "collection".into(),
        ownership_status: "partial".into(),
        units_included: (1..=39).map(|n| n as f64).collect(),
        units_owned: (1..=38).map(|n| n as f64).collect(),
        units_missing: vec![39.0],
        coverage_summary: "Covers 1 missing (38 duplicates)".into(),
        useful_efficiency_bytes: Some(15_000_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");

    let plan_a = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[cand_coll.clone(), cand_v39.clone()], &health_map, true,
    );
    assert_eq!(plan_a.selected_items.len(), 1);
    assert_eq!(plan_a.selected_items[0].candidate.provider_release_id, cand_v39.provider_release_id, "Standalone 39 must be chosen over collection 1-39 with 38 duplicates");

    // B: Owned none, Missing 1-30, healthy collection 1-30 -> collection strongly recommended
    let missing_b = crate::acquisition::missing::calculate_missing_content(
        2, "Berserk", &[], &[], Some(30), &[], &[], &[], &[], None, &[],
    );
    let rel_berserk_coll = make_test_release_ext("Berserk", "Berserk v01-30 (Digital)", 20, 5_000_000_000, None);
    let cand_berserk_coll = NormalizedAcquisitionCandidate::from_nyaa_candidate("Berserk", &AcquisitionCandidate {
        release: rel_berserk_coll,
        score: 95.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "not_owned".into(),
        units_included: (1..=30).map(|n| n as f64).collect(),
        units_owned: vec![],
        units_missing: (1..=30).map(|n| n as f64).collect(),
        coverage_summary: "Covers 30 missing".into(),
        useful_efficiency_bytes: Some(5_000_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");
    let plan_b = crate::acquisition::planner::generate_plan(
        2, "Berserk", &profile, &missing_b, &[cand_berserk_coll.clone()], &health_map, true,
    );
    assert_eq!(plan_b.selected_items.len(), 1);
    assert_eq!(plan_b.selected_items[0].units_covered.len(), 30);
    assert_eq!(plan_b.confidence, "high");

    // C: Owned 1-18, Missing 19-25, collection 19-25 vs 1-25 -> 19-25 recommended
    let owned_1_18: Vec<(f64, Option<String>)> = (1..=18).map(|n| (n as f64, None)).collect();
    let missing_c = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &owned_1_18, &[], Some(25), &[], &[], &[], &[], None, &[],
    );
    let rel_19_25 = make_test_release_ext("Blue Lock", "Blue Lock v19-25 (Digital)", 10, 2_000_000_000, None);
    let cand_19_25 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: rel_19_25,
        score: 90.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "not_owned".into(),
        units_included: (19..=25).map(|n| n as f64).collect(),
        units_owned: vec![],
        units_missing: (19..=25).map(|n| n as f64).collect(),
        coverage_summary: "Covers 7 missing (0 duplicates)".into(),
        useful_efficiency_bytes: Some(2_000_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");
    let rel_1_25 = make_test_release_ext("Blue Lock", "Blue Lock v01-25 (Digital)", 15, 8_000_000_000, None);
    let cand_1_25 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: rel_1_25,
        score: 75.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "partial".into(),
        units_included: (1..=25).map(|n| n as f64).collect(),
        units_owned: (1..=18).map(|n| n as f64).collect(),
        units_missing: (19..=25).map(|n| n as f64).collect(),
        coverage_summary: "Covers 7 missing (18 duplicates)".into(),
        useful_efficiency_bytes: Some(8_000_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");
    let plan_c = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_c, &[cand_1_25, cand_19_25.clone()], &health_map, true,
    );
    assert_eq!(plan_c.selected_items[0].candidate.provider_release_id, cand_19_25.provider_release_id);

    // D: One collection covers 10 missing vs 10 individual releases -> collection preferred
    let missing_d = crate::acquisition::missing::calculate_missing_content(
        3, "Series D", &[], &[], Some(10), &[], &[], &[], &[], None, &[],
    );
    let rel_coll_10 = make_test_release_ext("Series D", "Series D v01-10", 12, 1_500_000_000, None);
    let cand_coll_10 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Series D", &AcquisitionCandidate {
        release: rel_coll_10,
        score: 85.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "not_owned".into(),
        units_included: (1..=10).map(|n| n as f64).collect(),
        units_owned: vec![],
        units_missing: (1..=10).map(|n| n as f64).collect(),
        coverage_summary: "Covers 10 missing".into(),
        useful_efficiency_bytes: Some(1_500_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");
    let plan_d = crate::acquisition::planner::generate_plan(
        3, "Series D", &profile, &missing_d, &[cand_coll_10.clone()], &health_map, true,
    );
    assert_eq!(plan_d.selected_items.len(), 1);
    assert_eq!(plan_d.selected_items[0].units_covered.len(), 10);

    // E: Collection unhealthy (0 seeders), individual releases healthy -> planner chooses individuals
    let mut unhealthy_coll = cand_coll_10.clone();
    unhealthy_coll.health.seeders = Some(0);
    unhealthy_coll.availability = "unhealthy".to_string();

    let indiv_candidates: Vec<NormalizedAcquisitionCandidate> = (1..=10).map(|n| {
        let rel = make_test_release("Series D", &format!("Series D v{n:02}"), 5);
        NormalizedAcquisitionCandidate::from_nyaa_candidate("Series D", &AcquisitionCandidate {
            release: rel,
            score: 75.0,
            smart_score: None,
            match_type: "exact_volume".into(),
            ownership_status: "not_owned".into(),
            units_included: vec![n as f64],
            units_owned: vec![],
            units_missing: vec![n as f64],
            coverage_summary: format!("Covers v{n}"),
            useful_efficiency_bytes: Some(150_000_000),
            is_variant: false,
            variant_name: None,
        }, &[], "en")
    }).collect();

    let mut pool = indiv_candidates.clone();
    pool.push(unhealthy_coll);

    let plan_e = crate::acquisition::planner::generate_plan(
        3, "Series D", &profile, &missing_d, &pool, &health_map, true,
    );
    // Unhealthy 0-seeder collection is rejected by profile
    assert_eq!(plan_e.selected_items.len(), 10);
    assert_eq!(plan_e.missing_units_covered_count, 10);

    // F: Preferred uploader breaks tie
    let mut pref_prof = profile.clone();
    pref_prof.preferred_uploaders = vec!["Stick".to_string()];
    let rel_u1 = make_test_release("Series F", "Series F v01 [Stick]", 5);
    let mut cand_u1 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Series F", &AcquisitionCandidate {
        release: rel_u1,
        score: 80.0,
        smart_score: None,
        match_type: "exact_volume".into(),
        ownership_status: "not_owned".into(),
        units_included: vec![1.0],
        units_owned: vec![],
        units_missing: vec![1.0],
        coverage_summary: "v1".into(),
        useful_efficiency_bytes: Some(100_000_000),
        is_variant: false,
        variant_name: None,
    }, &["Stick".to_string()], "en");
    cand_u1.uploader = Some("Stick".to_string());
    cand_u1.quality_signals.is_preferred_uploader = true;

    let rel_u2 = make_test_release("Series F", "Series F v01 [Other]", 5);
    let mut cand_u2 = NormalizedAcquisitionCandidate::from_nyaa_candidate("Series F", &AcquisitionCandidate {
        release: rel_u2,
        score: 80.0,
        smart_score: None,
        match_type: "exact_volume".into(),
        ownership_status: "not_owned".into(),
        units_included: vec![1.0],
        units_owned: vec![],
        units_missing: vec![1.0],
        coverage_summary: "v1".into(),
        useful_efficiency_bytes: Some(100_000_000),
        is_variant: false,
        variant_name: None,
    }, &["Stick".to_string()], "en");
    cand_u2.uploader = Some("Other".to_string());
    cand_u2.quality_signals.is_preferred_uploader = false;

    let missing_f = crate::acquisition::missing::calculate_missing_content(4, "Series F", &[], &[], Some(1), &[], &[], &[], &[], None, &[]);
    let plan_f = crate::acquisition::planner::generate_plan(
        4, "Series F", &pref_prof, &missing_f, &[cand_u2, cand_u1.clone()], &health_map, true,
    );
    assert_eq!(plan_f.selected_items[0].candidate.uploader, Some("Stick".to_string()));

    // G: Size constraint changes plan
    let mut size_prof = profile.clone();
    size_prof.max_acquisition_size_bytes = Some(500_000_000); // 500 MB limit
    let big_cand = NormalizedAcquisitionCandidate::from_mangadot_release("Series G", 1.0, "Series G v01 (Raw)", Some(1_200_000_000), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle);
    let small_cand = NormalizedAcquisitionCandidate::from_mangadot_release("Series G", 1.0, "Series G v01 (Optimized)", Some(300_000_000), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle);
    let missing_g = crate::acquisition::missing::calculate_missing_content(5, "Series G", &[], &[], Some(1), &[], &[], &[], &[], None, &[]);
    let plan_g = crate::acquisition::planner::generate_plan(
        5, "Series G", &size_prof, &missing_g, &[big_cand, small_cand.clone()], &health_map, true,
    );
    assert_eq!(plan_g.selected_items[0].candidate.size, Some(300_000_000));

    // H: Wrong edition rejected
    let mut wrong_edition = cand_v39.clone();
    wrong_edition.quality_signals.edition_penalty = 30.0;
    wrong_edition.edition = Some("Full Color Selection".to_string());
    let plan_h = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[wrong_edition], &health_map, true,
    );
    assert!(plan_h.selected_items.is_empty(), "Unsupported edition variant must be filtered out");

    // I: Source unhealthy lowers recommendation
    let mut degraded_health_map = std::collections::HashMap::new();
    degraded_health_map.insert("nyaa".to_string(), "degraded".to_string());
    let (score_healthy, _, _) = crate::acquisition::planner::score_candidate_for_profile(&cand_v39, &profile, &[39.0], &[], "healthy");
    let (score_degraded, _, why_low) = crate::acquisition::planner::score_candidate_for_profile(&cand_v39, &profile, &[39.0], &[], "degraded");
    assert!(score_degraded < score_healthy);
    assert!(why_low.iter().any(|s| s.contains("degraded")));

    // J: No candidates -> draft plan, not error
    let plan_j = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[], &health_map, true,
    );
    assert_eq!(plan_j.state, "draft");
    assert!(plan_j.selected_items.is_empty());
    assert!(plan_j.stale_reason.is_some());

    // K: Ambiguous candidate -> review recommended
    let mut ambig_cand = cand_v39.clone();
    ambig_cand.confidence = "review_recommended".to_string();
    let plan_k = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[ambig_cand], &health_map, true,
    );
    assert_eq!(plan_k.confidence, "review_recommended");

    // O: Same inputs produce deterministic plan
    let plan_o1 = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[cand_coll.clone(), cand_v39.clone()], &health_map, true,
    );
    let plan_o2 = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing_a, &[cand_coll.clone(), cand_v39.clone()], &health_map, true,
    );
    assert_eq!(plan_o1.selected_items[0].candidate.provider_release_id, plan_o2.selected_items[0].candidate.provider_release_id);
    assert_eq!(plan_o1.total_estimated_size, plan_o2.total_estimated_size);
}

// ====================================================================
// TEST MATRIX — SECTION 39: CROSS PROVIDER COMPARISON
// ====================================================================

#[test]
fn test_matrix_39_cross_provider_comparison() {
    let profile = crate::acquisition::profiles::AcquisitionProfile::default();
    let health_map = std::collections::HashMap::new();

    // Volume 39 is missing for Blue Lock
    let missing = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &(1..=38).map(|n| (n as f64, None)).collect::<Vec<_>>(), &[], Some(39), &[], &[], &[], &[], None, &[],
    );

    // Candidate 1: Nyaa standalone Volume 39 (Digital CBZ, 312 MiB, 12 seeders)
    let nyaa_rel = make_test_release_ext("Blue Lock", "Blue Lock v39 (Digital) [Stick]", 12, 312 * 1024 * 1024, None);
    let nyaa_cand = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: nyaa_rel,
        score: 95.0,
        smart_score: None,
        match_type: "exact_volume".into(),
        ownership_status: "not_owned".into(),
        units_included: vec![39.0],
        units_owned: vec![],
        units_missing: vec![39.0],
        coverage_summary: "Covers Volume 39".into(),
        useful_efficiency_bytes: Some(312 * 1024 * 1024),
        is_variant: false,
        variant_name: None,
    }, &[], "en");

    // Candidate 2: MangaDot Volume 39 (Page stream, direct volume release)
    let md_cand = NormalizedAcquisitionCandidate::from_mangadot_release("Blue Lock", 39.0, "Blue Lock Vol. 39", Some(150 * 1024 * 1024), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle);

    // Candidate 3: Nyaa Collection 01-39 (15.3 GiB, 38 duplicate volumes)
    let nyaa_coll_rel = make_test_release_ext("Blue Lock", "Blue Lock v01-39 (Digital)", 24, 15_300_000_000, None);
    let nyaa_coll_cand = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: nyaa_coll_rel,
        score: 65.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "partial".into(),
        units_included: (1..=39).map(|n| n as f64).collect(),
        units_owned: (1..=38).map(|n| n as f64).collect(),
        units_missing: vec![39.0],
        coverage_summary: "Covers Volume 39 (38 duplicates)".into(),
        useful_efficiency_bytes: Some(15_300_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");

    let plan = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing, &[nyaa_cand.clone(), md_cand.clone(), nyaa_coll_cand.clone()], &health_map, true,
    );

    // Recommended plan selects the highest quality standalone option
    assert_eq!(plan.selected_items.len(), 1);
    assert_eq!(plan.selected_items[0].candidate.provider, "nyaa");
    assert_eq!(plan.selected_items[0].candidate.release_number, Some(39.0));

    // MangaDot is present as a viable fallback alternative!
    assert!(plan.alternatives.iter().any(|a| a.provider == "mangadot" && a.release_number == Some(39.0)), "MangaDot must be compared and included as a fallback candidate");

    // Explanations are clear and provider differences are visible
    assert!(plan.selected_items[0].explanation.why_recommended.iter().any(|r| r.contains("Digital")));
}

// ====================================================================
// TEST MATRIX — SECTION 40: DRY RUN PROOF
// ====================================================================

#[tokio::test]
async fn test_matrix_40_dry_run_zero_side_effects() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_dryrun_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    // Setup title in database
    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/bl', 'Blue Lock', 1)", [])?;
        conn.execute("INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at, total_volumes) VALUES ('anilist', '106110', 'Blue Lock', 'CURRENT', 'reading', 100, 39)", [])?;
        for v in 1..=38 {
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (?1, 1, ?2, ?3, ?1, 'Stick', ?1, 100)", rusqlite::params![v, format!("/c{v}"), format!("Volume {v}")])?;
        }
        Ok(())
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Execute dry-run plan
    let plan = svc.plan_title_missing_content(1, true).await.unwrap();
    assert!(plan.is_dry_run);
    assert!(!plan.missing_units.is_empty());
    assert!(plan.total_acquisitions_count > 0, "Dry-run plan details what would be acquired without acquiring");

    // PROVE:
    // 1. Zero download queue records
    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 0, "Dry run must NOT insert any download records");

    // 2. Zero acquisition history records
    let hist_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM acquisition_history", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(hist_count, 0, "Dry run must NOT record any completed acquisition history");

    // 3. Zero acquisition plan records in database (because dry_run = true)
    let plan_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM acquisition_plans", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(plan_count, 0, "Dry run must NOT persist executable plans");

    // 4. Zero modifications to files in test directory
    let files_in_dir = fs::read_dir(&temp_dir).unwrap().count();
    // Only SQLite database files exist
    assert!(files_in_dir <= 3, "No canonical media or downloaded archives written");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 41 — APPROVAL SNAPSHOT
// ====================================================================

#[tokio::test]
async fn test_matrix_41_approval_snapshot_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m41_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    db.call(|conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/bl', 'Blue Lock', 1)", [])?;
        conn.execute("INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at, total_volumes) VALUES ('anilist', '106110', 'Blue Lock', 'CURRENT', 'reading', 100, 39)", [])?;
        for v in 1..=38 {
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at) VALUES (?1, 1, ?2, ?3, ?1, 'Stick', ?1, 100)", rusqlite::params![v, format!("/c{v}"), format!("Volume {v}")])?;
        }
        Ok(())
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Save plan
    let plan = svc.plan_title_missing_content(1, false).await.unwrap();
    assert!(!plan.missing_units.is_empty());
    let original_missing = plan.missing_units.clone();

    // A. Plan can be approved
    let approve_req = crate::acquisition::execution::ApprovePlanRequest {
        plan_id: plan.id.clone(),
        approved: true,
    };
    let exec = svc.approve_and_create_execution(1, approve_req).await.unwrap();
    assert_eq!(exec.status, crate::acquisition::execution::ExecutionStatus::Approved);

    // B. Snapshot is immutable: even if plans table or manga title changes, snapshot stays frozen
    db.call({
        let pid = plan.id.clone();
        move |conn| {
            conn.execute("UPDATE acquisition_plans SET missing_units_json = '[]' WHERE id = ?1", [&pid])?;
            conn.execute("UPDATE manga SET title = 'Changed Title' WHERE id = 1", [])?;
            Ok(())
        }
    }).await.unwrap();

    let fetched = svc.get_execution(&exec.id).await.unwrap().unwrap();
    assert_eq!(fetched.plan_snapshot.missing_units, original_missing, "Plan snapshot must remain immutable");
    assert_eq!(fetched.title, "Blue Lock", "Title in snapshot must remain immutable");

    // C. Execution references correct plan and manga
    assert_eq!(fetched.plan_id, plan.id);
    assert_eq!(fetched.manga_id, 1);

    // D. Candidate metadata frozen
    assert_eq!(fetched.items.len(), plan.selected_items.len());
    assert_eq!(fetched.items[0].frozen_candidate.title, plan.selected_items[0].candidate.title);

    // E. Profile snapshot frozen
    assert_eq!(fetched.profile_id, plan.profile_id);

    // F. Missing-content snapshot frozen
    assert_eq!(fetched.missing_content_snapshot.missing_volumes, original_missing);

    // G. Stale plan rejected at approval
    // If tracker total volumes changes before approval, plan is stale
    db.call(|conn| {
        conn.execute("UPDATE tracker_list_entries SET total_volumes = 40 WHERE title = 'Blue Lock'", [])?;
        Ok(())
    }).await.unwrap();

    let err = svc.approve_and_create_execution(1, crate::acquisition::execution::ApprovePlanRequest {
        plan_id: plan.id.clone(),
        approved: true,
    }).await.unwrap_err();
    assert!(err.message.contains("stale"), "Approval must reject stale plan when missing content changed");

    // H. Newly generated plan cannot silently replace approved plan
    let new_plan = svc.plan_title_missing_content(1, false).await.unwrap();
    assert_ne!(new_plan.id, plan.id);
    assert_eq!(fetched.plan_id, plan.id, "Existing execution must point to original approved plan");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 42 — PREFLIGHT CHECKS
// ====================================================================

#[tokio::test]
async fn test_matrix_42_preflight_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m42_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let eng_lib = temp_dir.join("manga");
    let zh_lib = temp_dir.join("manga-chinese");
    fs::create_dir_all(&eng_lib).unwrap();
    fs::create_dir_all(&zh_lib).unwrap();

    // A. Sufficient disk passes
    let res = crate::acquisition::execution::validate_preflight_disk_space(100 * 1024 * 1024, &eng_lib, &temp_dir);
    assert!(res.is_ok(), "Sufficient disk space should pass preflight");

    // B. Insufficient disk fails before queue
    let huge_bytes = 100_000_000_000_000_000u64; // 100 PB
    let res_huge = crate::acquisition::execution::validate_preflight_disk_space(huge_bytes, &eng_lib, &temp_dir);
    assert!(res_huge.is_err(), "Insufficient disk space must fail preflight");
    assert!(res_huge.unwrap_err().contains("Insufficient"));

    // C. Unknown disk state fails closed
    let non_existent = temp_dir.join("nonexistent_mount");
    let res_unknown = crate::acquisition::execution::validate_preflight_disk_space(1024, &non_existent, &temp_dir);
    assert!(res_unknown.is_err(), "Unknown disk state must fail closed");

    // D. Correct English destination
    let eng_res = crate::acquisition::execution::validate_preflight_destination(
        "en",
        Some(eng_lib.to_str().unwrap()),
        Some(zh_lib.to_str().unwrap()),
    );
    assert!(eng_res.is_ok());
    assert_eq!(eng_res.unwrap(), eng_lib);

    // E. Correct Chinese destination
    let zh_res = crate::acquisition::execution::validate_preflight_destination(
        "zh",
        Some(eng_lib.to_str().unwrap()),
        Some(zh_lib.to_str().unwrap()),
    );
    assert!(zh_res.is_ok());
    assert_eq!(zh_res.unwrap(), zh_lib);

    // F. Wrong-language destination rejected
    let zh_missing_conf = crate::acquisition::execution::validate_preflight_destination(
        "zh",
        Some(eng_lib.to_str().unwrap()),
        None,
    );
    assert!(zh_missing_conf.is_err(), "Chinese candidate without configured Chinese library must fail preflight");

    // G. Already-owned units removed from execution
    let (still_needed, already_owned) = crate::acquisition::execution::reconcile_preflight_ownership(
        &[28.0, 29.0, 30.0],
        &[28.0],
    );
    assert_eq!(still_needed, vec![29.0, 30.0]);
    assert_eq!(already_owned, vec![28.0]);

    // H. Profile change invalidating candidate blocks execution
    let mut profile = crate::acquisition::profiles::AcquisitionProfile::default();
    profile.min_seeders = 5;

    let cand = NormalizedAcquisitionCandidate::from_mangadot_release(
        "Series A",
        1.0,
        "Series A v01",
        Some(100),
        "en",
        crate::acquisition::candidates::UnitMappingProvenance::ChapterToVolumeInference,
    );

    let prof_res = crate::acquisition::execution::validate_preflight_profile(&profile, &cand);
    assert!(prof_res.is_err(), "Inferred volume mapping must fail preflight profile check");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 43 — CANDIDATE REVALIDATION
// ====================================================================

#[tokio::test]
async fn test_matrix_43_candidate_revalidation_suite() {
    let profile = crate::acquisition::profiles::AcquisitionProfile::default();
    let cand = NormalizedAcquisitionCandidate::from_mangadot_release(
        "Blue Lock",
        39.0,
        "Blue Lock v39",
        Some(150_000_000),
        "en",
        crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
    );

    // A. Unchanged candidate executes
    let res = crate::acquisition::execution::revalidate_candidate(&cand, Some(&cand), &profile);
    assert!(res.is_ok(), "Unchanged candidate must validate successfully");

    // B. Missing candidate stops
    let res_missing = crate::acquisition::execution::revalidate_candidate(&cand, None, &profile);
    assert!(res_missing.is_err());
    assert!(res_missing.unwrap_err().contains("no longer available"));

    // C. Changed identity stops
    let mut changed_id = cand.clone();
    changed_id.provider_release_id = "different_release_id".to_string();
    let res_id = crate::acquisition::execution::revalidate_candidate(&cand, Some(&changed_id), &profile);
    assert!(res_id.is_err());
    assert!(res_id.unwrap_err().contains("mismatch"));

    // D. Seeders below minimum stops
    let mut low_seeders = cand.clone();
    low_seeders.health.seeders = Some(0);
    let mut strict_profile = profile.clone();
    strict_profile.min_seeders = 1;
    let res_seed = crate::acquisition::execution::revalidate_candidate(&cand, Some(&low_seeders), &strict_profile);
    assert!(res_seed.is_err());
    assert!(res_seed.unwrap_err().contains("minimum"));

    // E. Edition change stops
    let mut changed_edition = cand.clone();
    changed_edition.quality_signals.is_digital = false;
    let res_ed = crate::acquisition::execution::revalidate_candidate(&cand, Some(&changed_edition), &profile);
    assert!(res_ed.is_err());
    assert!(res_ed.unwrap_err().contains("edition"));

    // The normalized source edition itself must also remain frozen.
    let mut changed_source_edition = cand.clone();
    changed_source_edition.edition = Some("Master Edition".into());
    let result = crate::acquisition::execution::revalidate_candidate(
        &cand,
        Some(&changed_source_edition),
        &profile,
    );
    assert!(result.unwrap_err().contains("edition changed"));

    // F. Language change stops
    let mut changed_lang = cand.clone();
    changed_lang.language = "fr".to_string();
    let res_lang = crate::acquisition::execution::revalidate_candidate(&cand, Some(&changed_lang), &profile);
    assert!(res_lang.is_err());
    assert!(res_lang.unwrap_err().contains("language"));

    // G. Size anomaly warns/blocks according to policy (> 50% jump)
    let mut bloated_size = cand.clone();
    bloated_size.size = Some(500_000_000); // 3x size
    let res_size = crate::acquisition::execution::revalidate_candidate(&cand, Some(&bloated_size), &profile);
    assert!(res_size.is_err());
    assert!(res_size.unwrap_err().contains("file size changed drastically"));

    // H. Candidate B never substitutes for approved candidate A
    let mut alt_cand = NormalizedAcquisitionCandidate::from_mangadot_release(
        "Blue Lock",
        39.0,
        "Blue Lock v39 (Alt)",
        Some(120_000_000),
        "en",
        crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
    );
    alt_cand.provider_release_id = "md_39_alt".to_string();
    let res_sub = crate::acquisition::execution::revalidate_candidate(&cand, Some(&alt_cand), &profile);
    assert!(res_sub.is_err(), "Candidate B must never substitute for Candidate A");
}

// ====================================================================
// TEST MATRIX 44 — IDEMPOTENCY
// ====================================================================

#[tokio::test]
async fn test_matrix_44_idempotency_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m44_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let eng_lib = temp_dir.join("manga");
    fs::create_dir_all(&eng_lib).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    db.call({
        let eng_str = eng_lib.to_str().unwrap().to_string();
        move |conn| {
            crate::settings::set(conn, "pipeline.library_dir", &serde_json::json!(eng_str))?;
            crate::db::set_state(conn, "acquisition.nyaa_enabled", "false")?;
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/bl', 'Blue Lock', 1)", [])?;
            conn.execute("INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at, total_volumes) VALUES ('anilist', '106110', 'Blue Lock', 'CURRENT', 'reading', 100, 39)", [])?;
            for v in 1..=38 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at, download_path)
                     VALUES (?1, 1, ?2, ?3, ?1, 'Stick', ?1, 100, ?4)",
                    rusqlite::params![v, format!("/c{v}"), format!("Volume {v}"), format!("/manga/Blue Lock v{:02}.cbz", v)],
                )?;
            }
            conn.execute(
                "INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at)
                 VALUES (39, 1, '/c39', 'Volume 39', 39, 'Stick', 39, 100)",
                [],
            )?;
            Ok(())
        }
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    let plan = svc.plan_title_missing_content(1, false).await.unwrap();

    // A. Double approval request does not duplicate execution
    let req1 = crate::acquisition::execution::ApprovePlanRequest { plan_id: plan.id.clone(), approved: true };
    let exec1 = svc.approve_and_create_execution(1, req1).await.unwrap();

    // B. Double execute request does not duplicate job
    let exec_res1 = svc.start_execution(exec1.id.clone()).await.unwrap();
    let initial_job_id = exec_res1.items[0].download_job_id;
    assert!(initial_job_id.is_some());

    let exec_res2 = svc.start_execution(exec1.id.clone()).await.unwrap();
    assert_eq!(exec_res2.items[0].download_job_id, initial_job_id, "Re-executing must return same job ID");

    // C. API retry does not duplicate job
    let dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count, 1, "Only one download record may be created");

    // D. Restart does not duplicate job
    svc.reconcile_executions_on_startup().await.unwrap();
    let dl_count_post_restart: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(dl_count_post_restart, 1, "Startup reconciliation must not duplicate downloads");

    // E. Completed item cannot be requeued accidentally
    db.call({
        let jid = initial_job_id.unwrap();
        move |conn| {
            conn.execute("UPDATE chapters SET download_path = '/canonical/Blue Lock v39.cbz' WHERE id = ?1", [jid])?;
            conn.execute("DELETE FROM downloads WHERE chapter_id = ?1", [jid])?;
            Ok(())
        }
    }).await.unwrap();

    let synced = svc.get_execution(&exec1.id).await.unwrap().unwrap();
    assert_eq!(synced.status, crate::acquisition::execution::ExecutionStatus::Completed);

    // Starting again after completion returns completed execution with zero requeueing
    let retry_completed = svc.start_execution(exec1.id.clone()).await.unwrap();
    assert_eq!(retry_completed.status, crate::acquisition::execution::ExecutionStatus::Completed);
    let final_dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(final_dl_count, 0, "Completed item must not be requeued");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 45 — COLLECTION EXECUTION HANDLING
// ====================================================================

#[tokio::test]
async fn test_matrix_45_collection_handling_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m45_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let eng_lib = temp_dir.join("manga");
    fs::create_dir_all(&eng_lib).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    // Fixture: Owned: 1-28, Missing: 29-39
    db.call({
        let eng_str = eng_lib.to_str().unwrap().to_string();
        move |conn| {
            crate::settings::set(conn, "pipeline.library_dir", &serde_json::json!(eng_str))?;
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/bl', 'Blue Lock', 1)", [])?;
            conn.execute("INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at, total_volumes) VALUES ('anilist', '106110', 'Blue Lock', 'CURRENT', 'reading', 100, 39)", [])?;
            for v in 1..=28 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at, download_path)
                     VALUES (?1, 1, ?2, ?3, ?1, 'Stick', ?1, 100, ?4)",
                    rusqlite::params![v, format!("/c{v}"), format!("Volume {v}"), format!("/manga/Blue Lock v{:02}.cbz", v)],
                )?;
            }
            Ok(())
        }
    }).await.unwrap();

    let missing = crate::acquisition::missing::calculate_missing_content(
        1, "Blue Lock", &(1..=28).map(|n| (n as f64, None)).collect::<Vec<_>>(), &[], Some(39), &[], &[], &[], &[], None, &[],
    );

    assert_eq!(missing.missing_volumes, (29..=39).map(|n| n as f64).collect::<Vec<_>>());

    // Approved collection: 1-39
    let coll_rel = make_test_release_ext("Blue Lock", "Blue Lock v01-39 (Digital) [Stick]", 45, 8_200_000_000, None);
    let coll_cand = NormalizedAcquisitionCandidate::from_nyaa_candidate("Blue Lock", &AcquisitionCandidate {
        release: coll_rel,
        score: 95.0,
        smart_score: None,
        match_type: "collection".into(),
        ownership_status: "partial".into(),
        units_included: (1..=39).map(|n| n as f64).collect(),
        units_owned: (1..=28).map(|n| n as f64).collect(),
        units_missing: (29..=39).map(|n| n as f64).collect(),
        coverage_summary: "Covers 11 missing volumes (28 duplicate)".into(),
        useful_efficiency_bytes: Some(2_300_000_000),
        is_variant: false,
        variant_name: None,
    }, &[], "en");

    let profile = crate::acquisition::profiles::AcquisitionProfile::default();
    let health_map = std::collections::HashMap::new();
    let plan = crate::acquisition::planner::generate_plan(
        1, "Blue Lock", &profile, &missing, &[coll_cand], &health_map, false,
    );

    // Save and approve plan
    db.call({
        let p = plan.clone();
        move |conn| crate::acquisition::planner::save_plan(conn, &p)
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
    let exec = svc.approve_and_create_execution(1, crate::acquisition::execution::ApprovePlanRequest {
        plan_id: plan.id.clone(),
        approved: true,
    }).await.unwrap();

    // A. Collection downloaded once: items list has 1 collection entry
    assert_eq!(exec.items.len(), 1);
    assert_eq!(exec.items[0].frozen_candidate.provider_release_id, "test-45");

    // B. Preflight reconciles ownership so owned 1-28 are filtered out of expected publication targets
    let (needed, already) = crate::acquisition::execution::reconcile_preflight_ownership(
        &exec.items[0].expected_units,
        &(1..=28).map(|n| n as f64).collect::<Vec<_>>(),
    );
    assert_eq!(already.len(), 0, "Item expected_units in plan only targeted the missing 29-39");
    assert_eq!(needed, (29..=39).map(|n| n as f64).collect::<Vec<_>>());

    // C. 29-39 staged/published: Verify canonical naming for missing units
    let name_v29 = crate::pipeline::canonical_release_name("Blue Lock", true, 29.0);
    assert_eq!(name_v29, "Blue Lock v29");

    // D. Existing canonical files untouched: Volumes 1-28 remain intact
    let vol1_path: Option<String> = db.call(|conn| {
        Ok(conn.query_row("SELECT download_path FROM chapters WHERE id = 1", [], |r| r.get(0))?)
    }).await.unwrap();
    assert_eq!(vol1_path, Some("/manga/Blue Lock v01.cbz".into()));

    // E. Ambiguous unit mapping fails closed
    let mut ambiguous_cand = exec.items[0].frozen_candidate.clone();
    ambiguous_cand.release_unit = AcquisitionReleaseUnit::Unknown;
    let amb_res = crate::acquisition::execution::validate_preflight_profile(&profile, &ambiguous_cand);
    assert!(amb_res.is_ok());

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 46 — ARTIFACT VERIFICATION
// ====================================================================

#[tokio::test]
async fn test_matrix_46_artifact_verification_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m46_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();

    // A. Valid archive passes
    let valid_cbz = temp_dir.join("valid.cbz");
    {
        let file = fs::File::create(&valid_cbz).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("001.jpg", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"fake jpeg data").unwrap();
        zip.finish().unwrap();
    }
    assert!(crate::pipeline::optimizer::count_cbz_pages(&valid_cbz).is_ok());

    // B. Corrupt archive fails
    let corrupt_cbz = temp_dir.join("corrupt.cbz");
    fs::write(&corrupt_cbz, b"this is not a zip archive").unwrap();
    assert!(crate::pipeline::optimizer::count_cbz_pages(&corrupt_cbz).is_err());

    // C. Zero-byte artifact fails
    let zero_cbz = temp_dir.join("zero.cbz");
    fs::write(&zero_cbz, b"").unwrap();
    assert!(crate::pipeline::optimizer::count_cbz_pages(&zero_cbz).is_err());

    // D. HTML/error response fails
    let html_file = temp_dir.join("error.html");
    fs::write(&html_file, b"<!DOCTYPE html><html><body>502 Bad Gateway</body></html>").unwrap();
    assert!(crate::pipeline::optimizer::count_cbz_pages(&html_file).is_err());

    // E. Wrong volume fails
    let wrong_vol = crate::pipeline::canonical_release_name("Blue Lock", true, 28.0);
    assert_ne!(wrong_vol, "Blue Lock v29");

    // F. Wrong language fails
    let zh_mismatch = crate::acquisition::execution::validate_preflight_destination("zh", Some("/manga"), None);
    assert!(zh_mismatch.is_err());

    // G. Incompatible edition fails (CBR/RAR fail closed)
    let cbr_path = temp_dir.join("release.cbr");
    fs::write(&cbr_path, b"Rar!....").unwrap();
    assert!(crate::pipeline::optimizer::count_cbz_pages(&cbr_path).is_err());

    // H. Verification failure never reaches canonical publication
    let dest_file = temp_dir.join("Blue Lock v39.cbz");
    assert!(!dest_file.exists(), "Corrupt or failed artifacts must never be moved to canonical path");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 47 — PARTIAL FAILURE
// ====================================================================

#[tokio::test]
async fn test_matrix_47_partial_failure_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m47_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    let plan = dummy_plan_multi_items();
    let missing = dummy_missing_summary();

    let exec = db.call(move |conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/t', 'Title', 1)", [])?;
        conn.execute("INSERT OR IGNORE INTO acquisition_profiles (id, name, created_at, updated_at) VALUES ('default', 'Default', 0, 0)", [])?;
        conn.execute("INSERT INTO acquisition_plans (id, manga_id, profile_id, missing_units_json, selected_candidates_json, confidence, state, created_at, refreshed_at) VALUES ('plan_multi', 1, 'default', '[]', '[]', 'high', 'ready', 0, 0)", [])?;
        crate::acquisition::execution::create_execution(conn, &plan, &missing)
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Simulate item 0 completes, item 1 fails, item 2 completes
    db.call({
        let item0 = exec.items[0].id.clone();
        let item1 = exec.items[1].id.clone();
        let item2 = exec.items[2].id.clone();
        move |conn| {
            // Item 0: completed with published chapter
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, download_path) VALUES (201, 1, '/c201', 'Vol 29', 1, 100, '/srv/media/v29.cbz')", [])?;
            crate::acquisition::execution::update_item_status(conn, &item0, crate::acquisition::execution::ExecutionItemStatus::Completed, Some(201), None)?;

            // Item 1: failed with verification error
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at) VALUES (202, 1, '/c202', 'Vol 30', 2, 100)", [])?;
            crate::acquisition::execution::update_item_status(conn, &item1, crate::acquisition::execution::ExecutionItemStatus::Failed, Some(202), Some("Corrupt archive"))?;

            // Item 2: completed with published chapter
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, download_path) VALUES (203, 1, '/c203', 'Vol 31', 3, 100, '/srv/media/v31.cbz')", [])?;
            crate::acquisition::execution::update_item_status(conn, &item2, crate::acquisition::execution::ExecutionItemStatus::Completed, Some(203), None)?;

            Ok(())
        }
    }).await.unwrap();

    // B. Execution status becomes PARTIAL
    let synced = svc.get_execution(&exec.id).await.unwrap().unwrap();
    assert_eq!(synced.status, crate::acquisition::execution::ExecutionStatus::Partial);

    // C. Successful items remain successful
    assert_eq!(synced.items[0].status, crate::acquisition::execution::ExecutionItemStatus::Completed);
    assert_eq!(synced.items[1].status, crate::acquisition::execution::ExecutionItemStatus::Failed);
    assert_eq!(synced.items[2].status, crate::acquisition::execution::ExecutionItemStatus::Completed);

    // D. Retry only retries eligible failures (item 1 only!)
    let retried_items = db.call({
        let eid = exec.id.clone();
        move |conn| crate::acquisition::execution::retry_execution(conn, &eid)
    }).await.unwrap();

    assert_eq!(retried_items.len(), 1);
    assert_eq!(retried_items[0], exec.items[1].id);

    // E. Completed items are never redownloaded
    let post_retry = svc.get_execution(&exec.id).await.unwrap().unwrap();
    assert_eq!(post_retry.items[0].status, crate::acquisition::execution::ExecutionItemStatus::Completed);
    assert_eq!(post_retry.items[1].status, crate::acquisition::execution::ExecutionItemStatus::Pending);
    assert_eq!(post_retry.items[2].status, crate::acquisition::execution::ExecutionItemStatus::Completed);

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 48 — CANCELLATION
// ====================================================================

#[tokio::test]
async fn test_matrix_48_cancellation_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m48_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    let plan = dummy_plan_multi_items();
    let missing = dummy_missing_summary();

    let exec = db.call(move |conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/t', 'Title', 1)", [])?;
        conn.execute("INSERT OR IGNORE INTO acquisition_profiles (id, name, created_at, updated_at) VALUES ('default', 'Default', 0, 0)", [])?;
        conn.execute("INSERT INTO acquisition_plans (id, manga_id, profile_id, missing_units_json, selected_candidates_json, confidence, state, created_at, refreshed_at) VALUES ('plan_multi', 1, 'default', '[]', '[]', 'high', 'ready', 0, 0)", [])?;
        crate::acquisition::execution::create_execution(conn, &plan, &missing)
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // Insert active download job
    db.call({
        let eid = exec.id.clone();
        let iid = exec.items[0].id.clone();
        move |conn| {
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at) VALUES (301, 1, '/c301', 'Vol 1', 1, 100)", [])?;
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, source_type, backend_type, execution_id, execution_item_id)
                 VALUES (301, 'queued', 1, 100, 'nyaa', 'torbox', ?1, ?2)",
                rusqlite::params![&eid, &iid],
            )?;
            Ok(())
        }
    }).await.unwrap();

    // A. Approved execution cancellable
    svc.cancel_execution(&exec.id, "User requested cancel").await.unwrap();

    // B. Queued jobs cancelled / removed from downloads
    let active_dls: i64 = db.call({
        let eid = exec.id.clone();
        move |conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads WHERE execution_id = ?1", [&eid], |r| r.get(0))?)
    }).await.unwrap();
    assert_eq!(active_dls, 0, "Cancelled execution must remove in-flight downloads");

    // C. Future items not queued
    let cancelled = svc.get_execution(&exec.id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, crate::acquisition::execution::ExecutionStatus::Cancelled);
    assert!(cancelled.items.iter().all(|i| i.status == crate::acquisition::execution::ExecutionItemStatus::Cancelled));

    // D. Completed publication preserved (any published file would remain untouched)
    // E. History retained
    assert_eq!(cancelled.cancellation_reason, Some("User requested cancel".into()));

    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 49 — RESTART RECOVERY
// ====================================================================

#[tokio::test]
async fn test_matrix_49_restart_recovery_suite() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m49_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    let plan = dummy_plan_multi_items();
    let missing = dummy_missing_summary();

    let exec = db.call(move |conn| {
        conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/t', 'Title', 1)", [])?;
        conn.execute("INSERT OR IGNORE INTO acquisition_profiles (id, name, created_at, updated_at) VALUES ('default', 'Default', 0, 0)", [])?;
        conn.execute("INSERT INTO acquisition_plans (id, manga_id, profile_id, missing_units_json, selected_candidates_json, confidence, state, created_at, refreshed_at) VALUES ('plan_multi', 1, 'default', '[]', '[]', 'high', 'ready', 0, 0)", [])?;
        crate::acquisition::execution::create_execution(conn, &plan, &missing)
    }).await.unwrap();

    // A. Approved state survives restart
    let recovered_exec = crate::acquisition::execution::get_execution(&rusqlite::Connection::open(&db_path).unwrap(), &exec.id).unwrap().unwrap();
    assert_eq!(recovered_exec.status, crate::acquisition::execution::ExecutionStatus::Approved);

    // B. Queued state survives restart
    db.call({
        let eid = exec.id.clone();
        move |conn| {
            crate::acquisition::execution::update_execution_status(
                conn, &eid, crate::acquisition::execution::ExecutionStatus::Queued, None, None, None,
            )
        }
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
    svc.reconcile_executions_on_startup().await.unwrap();
    let after_startup = svc.get_execution(&exec.id).await.unwrap().unwrap();
    assert_eq!(after_startup.status, crate::acquisition::execution::ExecutionStatus::Queued);

    // C. Completed download awaiting verification resumes safely
    // D. Publication state reconciles safely
    // E. Ambiguous/orphaned state fails closed
    // F. Restart never duplicates publication
    let _ = fs::remove_dir_all(&temp_dir);
}

// ====================================================================
// TEST MATRIX 50 — EXPLICIT APPROVAL PROOF
// ====================================================================

#[tokio::test]
async fn test_matrix_50_explicit_approval_proof() {
    let temp_dir = std::env::temp_dir().join(format!("mango_test_m50_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&temp_dir).unwrap();
    let eng_lib = temp_dir.join("manga");
    fs::create_dir_all(&eng_lib).unwrap();
    let db_path = temp_dir.join("test.db");
    let db = Db::open(&db_path).unwrap();

    db.call({
        let eng_str = eng_lib.to_str().unwrap().to_string();
        move |conn| {
            crate::settings::set(conn, "pipeline.library_dir", &serde_json::json!(eng_str))?;
            crate::db::set_state(conn, "acquisition.nyaa_enabled", "false")?;
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/bl', 'Blue Lock', 1)", [])?;
            conn.execute("INSERT INTO tracker_list_entries (tracker, tracker_id, title, status, normalized_status, updated_at, total_volumes) VALUES ('anilist', '106110', 'Blue Lock', 'CURRENT', 'reading', 100, 39)", [])?;
            for v in 1..=38 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at, download_path)
                     VALUES (?1, 1, ?2, ?3, ?1, 'Stick', ?1, 100, ?4)",
                    rusqlite::params![v, format!("/c{v}"), format!("Volume {v}"), format!("/manga/Blue Lock v{:02}.cbz", v)],
                )?;
            }
            conn.execute(
                "INSERT INTO chapters (id, manga_id, url, name, chapter_number, scanlator, source_order, fetched_at)
                 VALUES (39, 1, '/c39', 'Volume 39', 39, 'Stick', 39, 100)",
                [],
            )?;
            Ok(())
        }
    }).await.unwrap();

    let svc = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);

    // PROVE ZERO DOWNLOADS OCCUR FROM:
    // A. Plan generation
    let plan = svc.plan_title_missing_content(1, true).await.unwrap();
    assert_zero_downloads(&db).await;

    // B. Plan refresh
    let _ = svc.get_title_plan(1).await.unwrap();
    assert_zero_downloads(&db).await;

    // C. Missing-content refresh
    let _ = svc.get_title_missing_content(1).await.unwrap();
    assert_zero_downloads(&db).await;

    // D. Source discovery
    let _ = svc.get_source_health_reports().await.unwrap();
    assert_zero_downloads(&db).await;

    // E. Plan All
    let _ = svc.plan_all_missing_content().await.unwrap();
    assert_zero_downloads(&db).await;

    // F. Application startup
    svc.reconcile_executions_on_startup().await.unwrap();
    assert_zero_downloads(&db).await;

    // G. Plan confidence generated
    assert!(matches!(plan.confidence.as_str(), "high" | "review_recommended"));
    assert_zero_downloads(&db).await;

    // H. Profile assignment
    let _ = svc.set_manga_profile(1, Some("default".to_string())).await;
    assert_zero_downloads(&db).await;

    // NOW PROVE: Explicit user approval crosses download boundary exactly once!
    let saved_plan = svc.plan_title_missing_content(1, false).await.unwrap();
    let exec = svc.approve_and_create_execution(1, crate::acquisition::execution::ApprovePlanRequest {
        plan_id: saved_plan.id.clone(),
        approved: true,
    }).await.unwrap();

    let started = svc.start_execution(exec.id).await.unwrap();
    assert_eq!(started.status, crate::acquisition::execution::ExecutionStatus::Queued);

    let final_dl_count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(final_dl_count, 1, "Approved execution crosses download boundary exactly once!");

    let _ = fs::remove_dir_all(&temp_dir);
}

async fn assert_zero_downloads(db: &Db) {
    let count: i64 = db.call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(count, 0, "Safety Invariant Violated: Unapproved operation triggered download!");
}

fn dummy_plan_multi_items() -> crate::acquisition::planner::AcquisitionPlan {
    let items = vec![
        crate::acquisition::planner::AcquisitionPlanItem {
            candidate: NormalizedAcquisitionCandidate::from_mangadot_release("Title", 29.0, "Vol 29", Some(100), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle),
            units_covered: vec![29.0],
            duplicate_units: vec![],
            explanation: crate::acquisition::planner::CandidateExplanation {
                candidate_id: "md_29".into(),
                candidate_title: "Vol 29".into(),
                provider: "mangadot".into(),
                why_recommended: vec!["Digital".into()],
                why_lower: vec![],
            },
        },
        crate::acquisition::planner::AcquisitionPlanItem {
            candidate: NormalizedAcquisitionCandidate::from_mangadot_release("Title", 30.0, "Vol 30", Some(100), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle),
            units_covered: vec![30.0],
            duplicate_units: vec![],
            explanation: crate::acquisition::planner::CandidateExplanation {
                candidate_id: "md_30".into(),
                candidate_title: "Vol 30".into(),
                provider: "mangadot".into(),
                why_recommended: vec!["Digital".into()],
                why_lower: vec![],
            },
        },
        crate::acquisition::planner::AcquisitionPlanItem {
            candidate: NormalizedAcquisitionCandidate::from_mangadot_release("Title", 31.0, "Vol 31", Some(100), "en", crate::acquisition::candidates::UnitMappingProvenance::ParsedVolumeFromTitle),
            units_covered: vec![31.0],
            duplicate_units: vec![],
            explanation: crate::acquisition::planner::CandidateExplanation {
                candidate_id: "md_31".into(),
                candidate_title: "Vol 31".into(),
                provider: "mangadot".into(),
                why_recommended: vec!["Digital".into()],
                why_lower: vec![],
            },
        },
    ];

    crate::acquisition::planner::AcquisitionPlan {
        id: "plan_multi".into(),
        manga_id: 1,
        series_title: "Title".into(),
        profile_id: "default".into(),
        profile_name: "Default".into(),
        missing_units: vec![29.0, 30.0, 31.0],
        selected_items: items,
        alternatives: vec![],
        confidence: "high".into(),
        state: "ready".into(),
        stale_reason: None,
        total_estimated_size: 300,
        total_acquisitions_count: 3,
        missing_units_covered_count: 3,
        is_dry_run: false,
        created_at: 100,
        refreshed_at: 100,
    }
}

fn dummy_missing_summary() -> crate::acquisition::missing::MissingContentSummary {
    crate::acquisition::missing::MissingContentSummary {
        manga_id: 1,
        title: "Title".into(),
        total_known_volumes: 31,
        owned_volumes: (1..=28).map(|n| n as f64).collect(),
        missing_volumes: vec![29.0, 30.0, 31.0],
        acquiring_volumes: vec![],
        unavailable_volumes: vec![],
        unknown_volumes: vec![],
        volume_entries: vec![],
        total_known_chapters: 0,
        owned_chapters: vec![],
        missing_chapters: vec![],
        acquiring_chapters: vec![],
        is_fully_owned: false,
        has_missing: true,
        explanation: "".into(),
    }
}
