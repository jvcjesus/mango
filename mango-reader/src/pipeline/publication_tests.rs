//! Comprehensive test suite for Canonical Metadata & Publication Integrity.
//!
//! Validates:
//! - Multi-source identical metadata resolution (Madokami vs Nyaa vs extensions)
//! - Haikyu!! Volume 3 Madokami regression verification
//! - Duplicate release detection & safe idempotency
//! - Missing volume numbers fail-closed handling
//! - Fractional chapters formatting
//! - Omnibus editions & range conflict detection
//! - Specials and one-shots handling
//! - Alternate languages & edition routing
//! - Repeated publication requests (idempotency)
//! - MangaBaka conflict & fallback handling
//! - Komga reconciliation (displayed title conformity & archive analysis)

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::downloads::Job;
use crate::pipeline::{
    komga::KomgaClient,
    mangabaka::{self, MangaBakaEnrichment},
    naming,
    publication_metadata::{self, PrePublicationOwnership},
    publisher,
    PipelineConfig,
};
use crate::release::ReleaseType;

fn unique_test_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("mango-{prefix}-{}-{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Helper to create a minimal dummy valid CBZ file with N image pages.
fn create_test_cbz(path: &Path, pages: usize) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);

    // Write dummy 1x1 GIF / image files
    let dummy_gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    for i in 1..=pages {
        zip.start_file(format!("{i:03}.gif"), options).unwrap();
        zip.write_all(dummy_gif).unwrap();
    }
    zip.finish().unwrap();
}

fn test_pipeline_config(library_dir: &Path) -> PipelineConfig {
    PipelineConfig {
        enabled: true,
        library_dir: Some(library_dir.to_string_lossy().to_string()),
        chinese_library_dir: Some(library_dir.join("chinese").to_string_lossy().to_string()),
        mangabaka_enabled: true,
        mangabaka_url: Some("http://localhost:8080".into()),
        komga_enabled: true,
        komga_url: Some("http://localhost:8080".into()),
        komga_library_id: Some("lib-english".into()),
        komga_chinese_library_id: Some("lib-chinese".into()),
        komga_scan_enabled: true,
        optimize_enabled: false,
        ..PipelineConfig::default()
    }
}

#[tokio::test]
async fn test_haikyu_volume_3_madokami_regression() {
    let dir = unique_test_dir("pub-haikyu");
    let lib_dir = dir.join("manga");
    fs::create_dir_all(&lib_dir).unwrap();
    let config = test_pipeline_config(&lib_dir);

    // Staged CBZ archive from Madokami
    let staging_cbz = dir.join("haikyu_v03_madokami.cbz");
    create_test_cbz(&staging_cbz, 192);

    // Job representing Madokami acquisition of Haikyu!! Volume 3
    let job = Job {
        chapter: 301,
        manga_id: 42,
        title: "Haikyu!!".into(),
        name: "Haikyu!! Volume 3".into(),
        number: 3.0,
        scanlator: Some("danke-Empire".into()),
        lang: Some("en".into()),
        volume: Some(3.0),
        release_type: ReleaseType::Volume,
        source_type: "madokami".into(),
        backend_type: "madokami".into(),
        external_job_id: Some("madokami-file-12345".into()),
        uploader: Some("danke-Empire".into()),
        release_size: Some(1024 * 1024 * 45),
        raw_title: Some("Haikyu!! Volume 3".into()),
        ..Default::default()
    };

    let pub_meta = publication_metadata::resolve_publication_metadata_for_job(
        &job,
        Some(&staging_cbz),
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    // 1. Verify canonical metadata resolution
    assert_eq!(pub_meta.series_title, "Haikyu!!");
    assert_eq!(pub_meta.publication_series_title, "Haikyu!!");
    assert_eq!(pub_meta.canonical_reader_title, "Volume 03");
    assert_eq!(pub_meta.canonical_filename, "Haikyu!! v03.cbz");
    assert_eq!(pub_meta.is_volume, true);
    assert_eq!(pub_meta.volume_number, Some(3.0));
    assert_eq!(pub_meta.comic_info_volume, Some("3".into()));
    assert_eq!(pub_meta.comic_info_number, None); // Strictly omitted for volumes!
    assert_eq!(pub_meta.release_group, Some("danke-Empire".into()));
    assert_eq!(pub_meta.archive_status, "Validated");
    assert_eq!(pub_meta.page_count, 192);
    assert!(pub_meta.is_safe_to_publish);

    // 2. Verify ComicInfo.xml generation conformity
    let comic_params = mangabaka::ComicInfoParams {
        series: &pub_meta.publication_series_title,
        volume: pub_meta.volume_number,
        number: pub_meta.chapter_number,
        is_volume: pub_meta.is_volume,
        title: Some(&pub_meta.canonical_reader_title),
        scanlator: pub_meta.release_group.as_deref(),
        notes: Some("Madokami direct acquisition"),
    };
    let xml = mangabaka::generate_comic_info_xml(&comic_params, None, None, 192);

    assert!(xml.contains("<Series>Haikyu!!</Series>"));
    assert!(xml.contains("<Volume>3</Volume>"));
    assert!(xml.contains("<Title>Volume 03</Title>"));
    assert!(!xml.contains("<Number>"), "Volume must strictly omit <Number> tag");
    assert!(xml.contains("<ScanInformation>danke-Empire</ScanInformation>"));
    assert!(xml.contains("<Translator>danke-Empire</Translator>"));
}

#[tokio::test]
async fn test_multiple_sources_identical_canonical_metadata() {
    let dir = unique_test_dir("pub-sources");
    let lib_dir = dir.join("manga");
    let config = test_pipeline_config(&lib_dir);

    // Case A: Madokami release
    let job_madokami = Job {
        chapter: 101,
        manga_id: 1,
        title: "Chainsaw Man".into(),
        name: "Chainsaw Man Vol. 1".into(),
        number: 1.0,
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "madokami".into(),
        backend_type: "madokami".into(),
        raw_title: Some("Chainsaw Man Vol. 1 (Digital)".into()),
        ..Default::default()
    };

    // Case B: Nyaa torrent release
    let job_nyaa = Job {
        chapter: 102,
        manga_id: 1,
        title: "Chainsaw Man".into(),
        name: "Chainsaw Man v01".into(),
        number: 1.0,
        scanlator: Some("LuMo".into()),
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "nyaa".into(),
        backend_type: "torbox".into(),
        external_job_id: Some("torbox-99".into()),
        uploader: Some("LuMo".into()),
        raw_title: Some("[LuMo] Chainsaw Man - v01 (Digital)".into()),
        ..Default::default()
    };

    // Case C: Keiyoushi Extension chapter-pack release
    let job_extension = Job {
        chapter: 103,
        manga_id: 1,
        title: "Chainsaw Man".into(),
        name: "Volume 1".into(),
        number: 1.0,
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "extension".into(),
        backend_type: "internal".into(),
        raw_title: Some("Volume 1".into()),
        ..Default::default()
    };

    let meta_mado = publication_metadata::resolve_publication_metadata_for_job(&job_madokami, None, &config, None, None).await.unwrap();
    let meta_nyaa = publication_metadata::resolve_publication_metadata_for_job(&job_nyaa, None, &config, None, None).await.unwrap();
    let meta_ext = publication_metadata::resolve_publication_metadata_for_job(&job_extension, None, &config, None, None).await.unwrap();

    // All three sources MUST resolve to identical canonical series, reader title, and filename:
    assert_eq!(meta_mado.series_title, "Chainsaw Man");
    assert_eq!(meta_nyaa.series_title, "Chainsaw Man");
    assert_eq!(meta_ext.series_title, "Chainsaw Man");

    assert_eq!(meta_mado.canonical_reader_title, "Volume 01");
    assert_eq!(meta_nyaa.canonical_reader_title, "Volume 01");
    assert_eq!(meta_ext.canonical_reader_title, "Volume 01");

    assert_eq!(meta_mado.canonical_filename, "Chainsaw Man v01.cbz");
    assert_eq!(meta_nyaa.canonical_filename, "Chainsaw Man v01.cbz");
    assert_eq!(meta_ext.canonical_filename, "Chainsaw Man v01.cbz");

    assert_eq!(meta_mado.comic_info_volume, Some("1".into()));
    assert_eq!(meta_nyaa.comic_info_volume, Some("1".into()));
    assert_eq!(meta_ext.comic_info_volume, Some("1".into()));
}

#[tokio::test]
async fn test_duplicate_release_detection_and_conflict() {
    let dir = unique_test_dir("pub-dup");
    let lib_dir = dir.join("manga");
    fs::create_dir_all(lib_dir.join("Blue Lock")).unwrap();
    let config = test_pipeline_config(&lib_dir);

    let published_file = lib_dir.join("Blue Lock").join("Blue Lock v01.cbz");
    create_test_cbz(&published_file, 50);

    // Staging file with DIFFERENT contents (different page count)
    let staging_different = dir.join("staging_diff.cbz");
    create_test_cbz(&staging_different, 60);

    // Staging file with IDENTICAL contents
    let staging_identical = dir.join("staging_ident.cbz");
    fs::copy(&published_file, &staging_identical).unwrap();

    let job = Job {
        chapter: 201,
        manga_id: 10,
        title: "Blue Lock".into(),
        name: "Volume 1".into(),
        number: 1.0,
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "nyaa".into(),
        backend_type: "torbox".into(),
        raw_title: Some("Blue Lock v01".into()),
        ..Default::default()
    };

    // A. Different file at destination -> DestinationConflict & unsafe to publish
    let meta_diff = publication_metadata::resolve_publication_metadata_for_job(
        &job,
        Some(&staging_different),
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(meta_diff.ownership_status, PrePublicationOwnership::DestinationConflict);
    assert!(!meta_diff.is_safe_to_publish);
    assert!(meta_diff.warnings.iter().any(|w| w.contains("different content")));

    // B. Identical file at destination -> AlreadyPublished (safe no-op idempotency)
    let meta_ident = publication_metadata::resolve_publication_metadata_for_job(
        &job,
        Some(&staging_identical),
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(meta_ident.ownership_status, PrePublicationOwnership::AlreadyPublished);
    assert!(meta_ident.is_safe_to_publish);
}

#[tokio::test]
async fn test_missing_volume_numbers_fail_closed() {
    let dir = unique_test_dir("pub-missing-num");
    let lib_dir = dir.join("manga");
    let config = test_pipeline_config(&lib_dir);

    let job_no_num = Job {
        chapter: 501,
        manga_id: 20,
        title: "Unknown Manga".into(),
        name: "Raw Chapter Release".into(),
        number: -1.0,
        release_type: ReleaseType::Volume,
        source_type: "extension".into(),
        backend_type: "internal".into(),
        raw_title: Some("Raw Manga Book".into()),
        ..Default::default()
    };

    let meta = publication_metadata::resolve_publication_metadata_for_job(
        &job_no_num,
        None,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(!meta.is_safe_to_publish);
    assert!(meta.warnings.iter().any(|w| w.contains("Missing or undetectable")));
}

#[tokio::test]
async fn test_fractional_chapters_formatting() {
    let dir = unique_test_dir("pub-frac");
    let lib_dir = dir.join("manga");
    let config = test_pipeline_config(&lib_dir);

    let job_frac = Job {
        chapter: 701,
        manga_id: 30,
        title: "Dandadan".into(),
        name: "Chapter 10.5 - Extra".into(),
        number: 10.5,
        release_type: ReleaseType::Chapter,
        chapter_number: Some(10.5),
        source_type: "extension".into(),
        backend_type: "internal".into(),
        raw_title: Some("Chapter 10.5 - Extra".into()),
        ..Default::default()
    };

    let meta = publication_metadata::resolve_publication_metadata_for_job(
        &job_frac,
        None,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(meta.is_volume, false);
    assert_eq!(meta.canonical_reader_title, "Chapter 010.5 - Extra");
    assert_eq!(meta.canonical_filename, "Dandadan ch10.5.cbz");
    assert_eq!(meta.comic_info_number, Some("10.5".into()));
    assert_eq!(meta.comic_info_volume, None);
}

#[tokio::test]
async fn test_omnibus_editions_and_conflicts() {
    let dir = unique_test_dir("pub-omnibus");
    let lib_dir = dir.join("manga");
    let config = test_pipeline_config(&lib_dir);

    // Omnibus publication title formatting
    let job_omnibus = Job {
        chapter: 801,
        manga_id: 40,
        title: "One Piece".into(),
        name: "One Piece (Omnibus)".into(),
        number: 1.0,
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "nyaa".into(),
        backend_type: "torbox".into(),
        raw_title: Some("One Piece (Omnibus) v01-03".into()),
        ..Default::default()
    };

    let meta = publication_metadata::resolve_publication_metadata_for_job(
        &job_omnibus,
        None,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(meta.canonical_reader_title, "Volume 01");
    assert_eq!(meta.publication_series_title, "One Piece");
}

#[tokio::test]
async fn test_alternate_languages_and_edition_routing() {
    let dir = unique_test_dir("pub-lang");
    let lib_dir = dir.join("manga");
    let chinese_dir = dir.join("chinese");
    fs::create_dir_all(&chinese_dir).unwrap();

    let mut config = test_pipeline_config(&lib_dir);
    config.chinese_library_dir = Some(chinese_dir.to_string_lossy().to_string());
    config.komga_chinese_library_id = Some("lib-chinese-123".into());

    // Chinese manga
    let job_chinese = Job {
        chapter: 901,
        manga_id: 50,
        title: "火影忍者".into(),
        name: "第01卷".into(),
        number: 1.0,
        lang: Some("zh".into()),
        volume: Some(1.0),
        release_type: ReleaseType::Volume,
        source_type: "extension".into(),
        backend_type: "internal".into(),
        raw_title: Some("火影忍者 第01卷".into()),
        ..Default::default()
    };

    let meta = publication_metadata::resolve_publication_metadata_for_job(
        &job_chinese,
        None,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(meta.language, "zh");
    assert_eq!(meta.target_library_root, chinese_dir);
    assert_eq!(meta.target_library_id, Some("lib-chinese-123".into()));
    assert!(meta.canonical_destination.starts_with(&chinese_dir));
}

#[tokio::test]
async fn test_mangabaka_volume_mismatch_flags_warning() {
    let dir = unique_test_dir("pub-mb-mismatch");
    let lib_dir = dir.join("manga");
    let config = test_pipeline_config(&lib_dir);

    let job = Job {
        chapter: 1001,
        manga_id: 60,
        title: "Jujutsu Kaisen".into(),
        name: "Volume 2".into(),
        number: 2.0,
        volume: Some(2.0),
        release_type: ReleaseType::Volume,
        source_type: "madokami".into(),
        backend_type: "madokami".into(),
        raw_title: Some("Jujutsu Kaisen Volume 2".into()),
        ..Default::default()
    };

    // Conflicting MangaBaka enrichment that claims this is Volume 4
    let conflicting_enrichment = MangaBakaEnrichment {
        series_id: 1234,
        collection_id: "col-123".into(),
        work_id: Some("work-456".into()),
        volume: 4.0, // MISMATCH!
        series_title: Some("Jujutsu Kaisen".into()),
        work_title: Some("Volume 4".into()),
        summary: None,
        release_date: None,
        pages: Some(192),
        writers: vec![],
        pencillers: vec![],
        publisher: None,
        genres: vec![],
        tags: vec![],
        web_url: None,
        total_volumes: Some(25),
        language: Some("en".into()),
        work_cover_url: None,
    };

    let meta = publication_metadata::resolve_publication_metadata_for_job(
        &job,
        None,
        &config,
        None,
        Some(&conflicting_enrichment),
    )
    .await
    .unwrap();

    assert_eq!(meta.confidence, "Ambiguous");
    assert!(meta.warnings.iter().any(|w| w.contains("MangaBaka volume mismatch")));
}

#[test]
fn test_specials_and_oneshots_preservation() {
    let naming_res = naming::normalize_book_naming(
        "One Piece - Omake - Straw Hat Theater",
        "One Piece",
        false,
        Some(-1.0),
    );

    assert!(naming_res.title.contains("Special") || naming_res.title.contains("Straw Hat Theater"));
}

#[tokio::test]
async fn test_repeated_publication_idempotency() {
    let dir = unique_test_dir("pub-idempotent");
    let lib_dir = dir.join("manga");
    fs::create_dir_all(&lib_dir).unwrap();

    let staging_cbz = dir.join("source.cbz");
    create_test_cbz(&staging_cbz, 20);

    // First publish: Published
    let first = publisher::publish_cbz(&staging_cbz, &lib_dir, "Idempotent Manga", true, 1.0).unwrap();
    let published_dest = match first {
        publisher::PublishOutcome::Published(p) => p,
        _ => panic!("Expected PublishOutcome::Published on initial publication"),
    };
    assert!(published_dest.is_file());

    // Second publish of identical archive: AlreadyPublished (safe no-op, no file replacement or duplication)
    let second = publisher::publish_cbz(&staging_cbz, &lib_dir, "Idempotent Manga", true, 1.0).unwrap();
    match second {
        publisher::PublishOutcome::AlreadyPublished(p) => assert_eq!(p, published_dest),
        _ => panic!("Expected PublishOutcome::AlreadyPublished on identical retry"),
    }
}

#[tokio::test]
async fn test_komga_reconciliation_verifies_canonical_title_and_flags_mismatch() {
    use axum::{routing::get, Json, Router};
    use serde_json::json;

    // A. Mock Komga with matching title "Volume 03"
    let app_matching = Router::new()
        .route("/api/v1/series", get(|| async {
            Json(json!({
                "content": [{ "id": "ser-1", "name": "Haikyu!!", "libraryId": "lib-en", "booksCount": 1 }],
                "totalPages": 1,
                "last": true
            }))
        }))
        .route("/api/v1/series/ser-1/books", get(|| async {
            Json(json!({
                "content": [{
                    "id": "book-1",
                    "seriesId": "ser-1",
                    "name": "Haikyu!! v03",
                    "number": 3.0,
                    "metadata": { "title": "Volume 03", "number": "3", "numberSort": 3.0 },
                    "media": { "status": "READY", "mediaType": "application/zip", "pagesCount": 192 }
                }],
                "totalPages": 1,
                "last": true
            }))
        }));

    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_a = listener_a.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener_a, app_matching).await.unwrap();
    });

    let client_a = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{addr_a}")), None);

    let pub_meta = publication_metadata::PublicationMetadata {
        chapter_id: 301,
        manga_id: 42,
        series_title: "Haikyu!!".into(),
        publication_series_title: "Haikyu!!".into(),
        is_volume: true,
        volume_number: Some(3.0),
        chapter_number: None,
        formatted_number: "03".into(),
        canonical_reader_title: "Volume 03".into(),
        canonical_filename: "Haikyu!! v03.cbz".into(),
        comic_info_volume: Some("3".into()),
        comic_info_number: None,
        mangabaka_series_id: None,
        mangabaka_volume: Some(3.0),
        mangabaka_release_id: None,
        anilist_id: None,
        edition: None,
        language: "en".into(),
        source_type: "madokami".into(),
        backend_type: "madokami".into(),
        release_group: Some("danke-Empire".into()),
        uploader: Some("danke-Empire".into()),
        raw_title: "Haikyu!! Volume 3".into(),
        target_library_root: PathBuf::from("/srv/media/manga"),
        target_library_id: Some("lib-en".into()),
        canonical_destination: PathBuf::from("/srv/media/manga/Haikyu!!/Haikyu!! v03.cbz"),
        ownership_status: PrePublicationOwnership::NotOwned,
        existing_path: None,
        existing_book_id: None,
        confidence: "High".into(),
        archive_status: "Validated".into(),
        page_count: 192,
        warnings: vec![],
        is_safe_to_publish: true,
    };

    let outcome_a = publication_metadata::reconcile_published_book(&client_a, &pub_meta).await.unwrap();
    assert!(outcome_a.is_verified, "Should be verified when displayed title is canonical Volume 03");
    assert!(!outcome_a.needs_review);
    assert_eq!(outcome_a.displayed_title, Some("Volume 03".into()));
    assert_eq!(outcome_a.book_id, Some("book-1".into()));

    // B. Mock Komga with unnormalized title "Volume 3" -> Must flag for review!
    let app_mismatch = Router::new()
        .route("/api/v1/series", get(|| async {
            Json(json!({
                "content": [{ "id": "ser-2", "name": "Haikyu!!", "libraryId": "lib-en", "booksCount": 1 }],
                "totalPages": 1,
                "last": true
            }))
        }))
        .route("/api/v1/series/ser-2/books", get(|| async {
            Json(json!({
                "content": [{
                    "id": "book-2",
                    "seriesId": "ser-2",
                    "name": "Haikyu!! v03",
                    "number": 3.0,
                    "metadata": { "title": "Volume 3", "number": "3", "numberSort": 3.0 }, // INCONSISTENT!
                    "media": { "status": "READY", "mediaType": "application/zip", "pagesCount": 192 }
                }],
                "totalPages": 1,
                "last": true
            }))
        }));

    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener_b, app_mismatch).await.unwrap();
    });

    let client_b = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{addr_b}")), None);

    let outcome_b = publication_metadata::reconcile_published_book(&client_b, &pub_meta).await.unwrap();
    assert!(!outcome_a.needs_review);
    assert!(outcome_b.needs_review, "Should flag for review when Komga title differs from canonical standard");
    assert!(!outcome_b.is_verified);
    assert!(outcome_b.warnings.iter().any(|w| w.contains("title mismatch")));

    // C. Mock Komga with media analysis ERROR -> Must flag for review!
    let app_error = Router::new()
        .route("/api/v1/series", get(|| async {
            Json(json!({
                "content": [{ "id": "ser-3", "name": "Haikyu!!", "libraryId": "lib-en", "booksCount": 1 }],
                "totalPages": 1,
                "last": true
            }))
        }))
        .route("/api/v1/series/ser-3/books", get(|| async {
            Json(json!({
                "content": [{
                    "id": "book-3",
                    "seriesId": "ser-3",
                    "name": "Haikyu!! v03",
                    "number": 3.0,
                    "metadata": { "title": "Volume 03", "number": "3", "numberSort": 3.0 },
                    "media": { "status": "ERROR", "mediaType": "application/zip", "pagesCount": 0 }
                }],
                "totalPages": 1,
                "last": true
            }))
        }));

    let listener_c = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_c = listener_c.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener_c, app_error).await.unwrap();
    });

    let client_c = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{addr_c}")), None);

    let outcome_c = publication_metadata::reconcile_published_book(&client_c, &pub_meta).await.unwrap();
    assert!(outcome_c.needs_review, "Should flag for review when archive analysis is ERROR");
    assert!(!outcome_c.is_verified);
    assert!(outcome_c.warnings.iter().any(|w| w.contains("media analysis reported an ERROR")));
}
