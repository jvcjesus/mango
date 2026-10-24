//! Komga-backed ownership status and conservative matching for manga titles and volumes.

use super::komga::KomgaClient;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

const OWNERSHIP_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnershipStatus {
    Owned,
    Available,
    Acquiring,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeOwnership {
    pub volume: f64,
    pub status: OwnershipStatus,
    pub book_id: Option<String>,
    pub book_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterOwnership {
    pub chapter: f64,
    pub status: OwnershipStatus,
    pub book_id: Option<String>,
    pub book_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipSummary {
    pub owned_volumes_count: usize,
    pub total_source_volumes: Option<usize>,
    pub owned_chapters_count: usize,
    pub total_source_chapters: Option<usize>,
    pub source_release_type: String, // "volume", "chapter", "mixed", "empty"
    #[serde(default)]
    pub authoritative_total_volumes: Option<u32>,
    #[serde(default)]
    pub authoritative_total_chapters: Option<u32>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleOwnership {
    pub komga_connected: bool,
    pub error: Option<String>,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub library_id: Option<String>,
    pub library_name: Option<String>,
    pub volumes: Vec<VolumeOwnership>,
    pub chapters: Vec<ChapterOwnership>,
    pub summary: OwnershipSummary,
    pub last_refreshed: i64,
}

#[derive(Clone)]
pub struct OwnershipService {
    http: Client,
    db: crate::db::Db,
    cache: Arc<RwLock<HashMap<i64, (Instant, TitleOwnership)>>>,
}

impl OwnershipService {
    pub fn new(http: Client, db: crate::db::Db) -> Self {
        Self {
            http,
            db,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Invalidates cached ownership state for a manga title.
    pub fn invalidate(&self, manga_id: i64) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(&manga_id);
        }
    }

    /// Clears all cached ownership states.
    #[allow(dead_code)]
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    /// Retrieves ownership status for a manga title, using cached data if fresh.
    pub async fn get_title_ownership(&self, manga_id: i64, title: &str) -> TitleOwnership {
        // Check cache
        if let Ok(cache) = self.cache.read() {
            if let Some((inserted, ownership)) = cache.get(&manga_id) {
                if inserted.elapsed() < OWNERSHIP_CACHE_TTL {
                    return ownership.clone();
                }
            }
        }

        let ownership = self.query_komga_ownership(manga_id, title).await;

        if let Ok(mut cache) = self.cache.write() {
            cache.insert(manga_id, (Instant::now(), ownership.clone()));
        }

        ownership
    }

    /// Invalidates cached ownership and fetches fresh status directly from Komga.
    pub async fn refresh_title(&self, manga_id: i64, title: &str) -> TitleOwnership {
        self.invalidate(manga_id);
        self.get_title_ownership(manga_id, title).await
    }

    async fn query_komga_ownership(&self, manga_id: i64, title: &str) -> TitleOwnership {
        let now = crate::db::now();

        // Load pipeline config & secrets and source releases from db
        let (komga_enabled, komga_url, komga_key, lib_id, active_downloads, source_releases) =
            match self
                .db
                .call(move |conn| {
                    let enabled = crate::settings::get::<bool>(conn, "pipeline.komga_enabled")
                        .unwrap_or(false);
                    let url = crate::settings::get::<String>(conn, "pipeline.komga_url");
                    let key = crate::db::state(conn, "pipeline.komga_api_key")?;
                    let lib_id = crate::settings::get::<String>(conn, "pipeline.komga_library_id");

                    // Get active downloads for this manga
                    let mut stmt = conn.prepare(
                        "SELECT c.id, c.name, c.chapter_number, d.state
                     FROM downloads d
                     JOIN chapters c ON c.id = d.chapter_id
                     WHERE c.manga_id = ?1",
                    )?;
                    let downloads = stmt
                        .query_map([manga_id], |row| {
                            let id: i64 = row.get(0)?;
                            let name: String = row.get(1)?;
                            let chapter_number: f64 = row.get(2)?;
                            let state: String = row.get(3)?;
                            Ok((id, name, chapter_number, state))
                        })?
                        .filter_map(std::result::Result::ok)
                        .collect::<Vec<_>>();

                    // Query all source releases for this manga
                    let mut ch_stmt = conn
                        .prepare("SELECT name, chapter_number FROM chapters WHERE manga_id = ?1")?;
                    let releases = ch_stmt
                        .query_map([manga_id], |row| {
                            let name: String = row.get(0)?;
                            let num: f64 = row.get(1)?;
                            Ok((name, num))
                        })?
                        .filter_map(std::result::Result::ok)
                        .collect::<Vec<_>>();

                    Ok((enabled, url, key, lib_id, downloads, releases))
                })
                .await
            {
                Ok(res) => res,
                Err(e) => {
                    tracing::warn!("OWNERSHIP: failed to read configuration from DB: {e}");
                    return TitleOwnership {
                        komga_connected: false,
                        error: Some(format!("Database query failed: {e}")),
                        series_id: None,
                        series_name: None,
                        library_id: None,
                        library_name: None,
                        volumes: Vec::new(),
                        chapters: Vec::new(),
                        summary: OwnershipSummary {
                            owned_volumes_count: 0,
                            total_source_volumes: None,
                            owned_chapters_count: 0,
                            total_source_chapters: None,
                            source_release_type: "empty".to_string(),
                            authoritative_total_volumes: None,
                            authoritative_total_chapters: None,
                            explanation: None,
                        },
                        last_refreshed: now,
                    };
                }
            };

        // Classify source releases
        let mut src_volumes = std::collections::HashSet::new();
        let mut src_chapters = std::collections::HashSet::new();
        for (name, num) in &source_releases {
            let rel = crate::release::parse_release(name, *num);
            if rel.release_type == crate::release::ReleaseType::Volume {
                if let Some(v) = rel.volume.or(Some(*num)) {
                    src_volumes.insert(v.to_bits());
                }
            } else {
                if let Some(v) = rel.volume {
                    src_volumes.insert(v.to_bits());
                }
                let ch = rel.chapter_number.unwrap_or(*num);
                src_chapters.insert(ch.to_bits());
            }
        }

        let source_volumes_count = src_volumes.len();
        let source_chapters_count = src_chapters.len();
        let source_release_type = match (source_volumes_count > 0, source_chapters_count > 0) {
            (true, false) => "volume",
            (false, true) => "chapter",
            (true, true) => "mixed",
            (false, false) => "empty",
        }
        .to_string();

        if !komga_enabled {
            return TitleOwnership {
                komga_connected: false,
                error: Some("Komga integration is not enabled in Settings".to_string()),
                series_id: None,
                series_name: None,
                library_id: None,
                library_name: None,
                volumes: Vec::new(),
                chapters: Vec::new(),
                summary: OwnershipSummary {
                    owned_volumes_count: 0,
                    total_source_volumes: (source_release_type == "volume"
                        || source_release_type == "mixed")
                        .then_some(source_volumes_count),
                    owned_chapters_count: 0,
                    total_source_chapters: (source_release_type == "chapter"
                        || source_release_type == "mixed")
                        .then_some(source_chapters_count),
                    source_release_type,
                    authoritative_total_volumes: None,
                    authoritative_total_chapters: None,
                    explanation: None,
                },
                last_refreshed: now,
            };
        }

        let client = KomgaClient::new(self.http.clone(), komga_url, komga_key);

        // Search series in Komga
        let series_candidates = match client.search_series(title, lib_id.as_deref()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("OWNERSHIP: Komga query failed: {e}");
                return TitleOwnership {
                    komga_connected: false,
                    error: Some("Komga server is unreachable".to_string()),
                    series_id: None,
                    series_name: None,
                    library_id: lib_id,
                    library_name: None,
                    volumes: Vec::new(),
                    chapters: Vec::new(),
                    summary: OwnershipSummary {
                        owned_volumes_count: 0,
                        total_source_volumes: (source_release_type == "volume"
                            || source_release_type == "mixed")
                            .then_some(source_volumes_count),
                        owned_chapters_count: 0,
                        total_source_chapters: (source_release_type == "chapter"
                            || source_release_type == "mixed")
                            .then_some(source_chapters_count),
                        source_release_type,
                        authoritative_total_volumes: None,
                        authoritative_total_chapters: None,
                        explanation: None,
                    },
                    last_refreshed: now,
                };
            }
        };

        let wanted_norm = crate::pipeline::mangabaka::normalize_title(title);
        let mut exact_matches = Vec::new();
        for s in series_candidates {
            if crate::pipeline::mangabaka::normalize_title(&s.name) == wanted_norm {
                exact_matches.push(s);
            }
        }

        // Conservative matching: require exactly one match
        if exact_matches.is_empty() {
            return TitleOwnership {
                komga_connected: true,
                error: None,
                series_id: None,
                series_name: None,
                library_id: lib_id,
                library_name: None,
                volumes: Vec::new(),
                chapters: Vec::new(),
                summary: OwnershipSummary {
                    owned_volumes_count: 0,
                    total_source_volumes: (source_release_type == "volume"
                        || source_release_type == "mixed")
                        .then_some(source_volumes_count),
                    owned_chapters_count: 0,
                    total_source_chapters: (source_release_type == "chapter"
                        || source_release_type == "mixed")
                        .then_some(source_chapters_count),
                    source_release_type,
                    authoritative_total_volumes: None,
                    authoritative_total_chapters: None,
                    explanation: None,
                },
                last_refreshed: now,
            };
        }

        if exact_matches.len() > 1 {
            tracing::warn!("OWNERSHIP: found multiple exact series matches for {title:?}; refusing ambiguous match");
            return TitleOwnership {
                komga_connected: true,
                error: Some("Ambiguous match: multiple series found in Komga".to_string()),
                series_id: None,
                series_name: None,
                library_id: lib_id,
                library_name: None,
                volumes: Vec::new(),
                chapters: Vec::new(),
                summary: OwnershipSummary {
                    owned_volumes_count: 0,
                    total_source_volumes: (source_release_type == "volume"
                        || source_release_type == "mixed")
                        .then_some(source_volumes_count),
                    owned_chapters_count: 0,
                    total_source_chapters: (source_release_type == "chapter"
                        || source_release_type == "mixed")
                        .then_some(source_chapters_count),
                    source_release_type,
                    authoritative_total_volumes: None,
                    authoritative_total_chapters: None,
                    explanation: None,
                },
                last_refreshed: now,
            };
        }

        let matched_series = &exact_matches[0];
        let books = match client.get_books_in_series(&matched_series.id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "OWNERSHIP: failed to fetch books for series {}: {e}",
                    matched_series.id
                );
                return TitleOwnership {
                    komga_connected: true,
                    error: Some(format!("Failed to fetch books: {e}")),
                    series_id: Some(matched_series.id.clone()),
                    series_name: Some(matched_series.name.clone()),
                    library_id: Some(matched_series.library_id.clone()),
                    library_name: None,
                    volumes: Vec::new(),
                    chapters: Vec::new(),
                    summary: OwnershipSummary {
                        owned_volumes_count: 0,
                        total_source_volumes: (source_release_type == "volume"
                            || source_release_type == "mixed")
                            .then_some(source_volumes_count),
                        owned_chapters_count: 0,
                        total_source_chapters: (source_release_type == "chapter"
                            || source_release_type == "mixed")
                            .then_some(source_chapters_count),
                        source_release_type,
                        authoritative_total_volumes: None,
                        authoritative_total_chapters: None,
                        explanation: None,
                    },
                    last_refreshed: now,
                };
            }
        };

        // Determine library name
        let libraries = client.list_libraries().await.unwrap_or_default();
        let library_name = libraries
            .iter()
            .find(|l| l.id == matched_series.library_id)
            .map(|l| l.name.clone());

        // Extract owned volumes and chapters from Komga books
        let mut owned_volumes = HashMap::new();
        let mut owned_chapters = HashMap::new();

        for b in &books {
            let vol_from_name = crate::release::parse_volume(&b.name);
            let rel_info = crate::release::parse_release(&b.name, b.number);

            if rel_info.release_type == crate::release::ReleaseType::Volume
                || vol_from_name.is_some()
            {
                let vol_num = vol_from_name
                    .or(rel_info.volume)
                    .or_else(|| {
                        b.metadata.as_ref().and_then(|m| {
                            m.number_sort
                                .or_else(|| m.number.as_deref().and_then(|n| n.parse::<f64>().ok()))
                        })
                    })
                    .unwrap_or(b.number);
                owned_volumes.insert(vol_num.to_bits(), (b.id.clone(), b.name.clone()));
            } else {
                let ch_num = rel_info
                    .chapter_number
                    .or_else(|| {
                        b.metadata.as_ref().and_then(|m| {
                            m.number_sort
                                .or_else(|| m.number.as_deref().and_then(|n| n.parse::<f64>().ok()))
                        })
                    })
                    .unwrap_or(b.number);
                owned_chapters.insert(ch_num.to_bits(), (b.id.clone(), b.name.clone()));
            }
        }

        // Build volume ownership list
        let mut volumes = Vec::new();
        for (vol_bits, (book_id, book_name)) in &owned_volumes {
            let volume = f64::from_bits(*vol_bits);
            volumes.push(VolumeOwnership {
                volume,
                status: OwnershipStatus::Owned,
                book_id: Some(book_id.clone()),
                book_name: Some(book_name.clone()),
            });
        }

        // Add acquiring volumes from active downloads queue if not already owned
        for (_id, name, chapter_number, _state) in &active_downloads {
            let rel = crate::release::parse_release(name, *chapter_number);
            let vol_num = if rel.release_type == crate::release::ReleaseType::Volume {
                rel.volume.or(Some(*chapter_number))
            } else {
                rel.volume
            };
            if let Some(vol) = vol_num {
                if !owned_volumes.contains_key(&vol.to_bits()) {
                    if !volumes.iter().any(|v| (v.volume - vol).abs() < 0.001) {
                        volumes.push(VolumeOwnership {
                            volume: vol,
                            status: OwnershipStatus::Acquiring,
                            book_id: None,
                            book_name: None,
                        });
                    }
                }
            }
        }
        volumes.sort_by(|a, b| {
            a.volume
                .partial_cmp(&b.volume)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build chapter ownership list
        let mut chapters = Vec::new();
        for (ch_bits, (book_id, book_name)) in &owned_chapters {
            let chapter = f64::from_bits(*ch_bits);
            chapters.push(ChapterOwnership {
                chapter,
                status: OwnershipStatus::Owned,
                book_id: Some(book_id.clone()),
                book_name: Some(book_name.clone()),
            });
        }

        // Add acquiring chapters from active downloads queue if not already owned
        for (_id, name, chapter_number, _state) in &active_downloads {
            let rel = crate::release::parse_release(name, *chapter_number);
            if rel.release_type == crate::release::ReleaseType::Chapter {
                let ch_num = rel.chapter_number.unwrap_or(*chapter_number);
                if !owned_chapters.contains_key(&ch_num.to_bits()) {
                    if !chapters.iter().any(|c| (c.chapter - ch_num).abs() < 0.001) {
                        chapters.push(ChapterOwnership {
                            chapter: ch_num,
                            status: OwnershipStatus::Acquiring,
                            book_id: None,
                            book_name: None,
                        });
                    }
                }
            }
        }
        chapters.sort_by(|a, b| {
            a.chapter
                .partial_cmp(&b.chapter)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let owned_volumes_count = owned_volumes.len();
        let owned_chapters_count = owned_chapters.len();

        let total_source_volumes =
            if source_release_type == "volume" || source_release_type == "mixed" {
                Some(source_volumes_count)
            } else {
                None
            };

        let total_source_chapters =
            if source_release_type == "chapter" || source_release_type == "mixed" {
                Some(source_chapters_count)
            } else {
                None
            };

        let explanation = if owned_volumes_count > 0 && source_release_type == "chapter" {
            Some("Source provides chapters".to_string())
        } else if owned_chapters_count > 0 && source_release_type == "volume" {
            Some("Source provides volumes".to_string())
        } else {
            None
        };

        TitleOwnership {
            komga_connected: true,
            error: None,
            series_id: Some(matched_series.id.clone()),
            series_name: Some(matched_series.name.clone()),
            library_id: Some(matched_series.library_id.clone()),
            library_name,
            volumes,
            chapters,
            summary: OwnershipSummary {
                owned_volumes_count,
                total_source_volumes,
                owned_chapters_count,
                total_source_chapters,
                source_release_type,
                authoritative_total_volumes: None,
                authoritative_total_chapters: None,
                explanation,
            },
            last_refreshed: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_status_serialization() {
        let status = OwnershipStatus::Owned;
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"owned\"");
        let status = OwnershipStatus::Available;
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"available\"");
        let status = OwnershipStatus::Acquiring;
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"acquiring\"");
        let status = OwnershipStatus::Unknown;
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"unknown\"");
    }

    #[tokio::test]
    async fn ownership_service_flow_with_mock_komga() {
        use axum::{routing::get, Json, Router};
        use serde_json::json;

        let app = Router::new()
            .route(
                "/api/v1/series",
                get(|axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>| async move {
                    let search = params.get("search").cloned().unwrap_or_default();
                    if search == "Blue Lock" {
                        Json(json!({
                            "content": [
                                {
                                    "id": "s-1",
                                    "name": "Blue Lock",
                                    "libraryId": "lib-en",
                                    "booksCount": 2
                                }
                            ]
                        }))
                    } else if search == "Ambiguous Title" {
                        Json(json!({
                            "content": [
                                { "id": "s-10", "name": "Ambiguous Title", "libraryId": "lib-en", "booksCount": 1 },
                                { "id": "s-11", "name": "Ambiguous Title", "libraryId": "lib-zh", "booksCount": 1 }
                            ]
                        }))
                    } else {
                        Json(json!({ "content": [] }))
                    }
                }),
            )
            .route(
                "/api/v1/series/s-1/books",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "b-1",
                                "seriesId": "s-1",
                                "name": "Blue Lock v01",
                                "number": 1.0,
                                "metadata": { "numberSort": 1.0 }
                            },
                            {
                                "id": "b-2",
                                "seriesId": "s-1",
                                "name": "Blue Lock v02",
                                "number": 2.0,
                                "metadata": { "numberSort": 2.0 }
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

        let db = crate::pipeline::tests::test_db_instance();
        db.call(move |conn| {
            let mut map = serde_json::Map::new();
            map.insert("pipeline.komga_enabled".into(), serde_json::json!(true));
            map.insert("pipeline.komga_url".into(), serde_json::json!(format!("http://127.0.0.1:{port}")));
            map.insert("pipeline.komga_library_id".into(), serde_json::json!("lib-en"));
            crate::settings::put(conn, &map)?;
            crate::db::set_state(conn, "pipeline.komga_api_key", "mock-key")?;

            // Insert test manga
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (10, 1, '/bl', 'Blue Lock', 1)", [])?;
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read, chapter_number) VALUES (101, 10, '/c3', 'Series v03', 0, 0, 0, 3.0)", [])?;
            // Queue volume 3 so it is acquiring
            conn.execute("INSERT INTO downloads (chapter_id, position, state, added_at) VALUES (101, 1, 'downloading', 0)", [])?;

            Ok(())
        }).await.unwrap();

        let ownership_svc = OwnershipService::new(Client::new(), db);

        // 1. Query ownership for Blue Lock
        let ownership = ownership_svc.get_title_ownership(10, "Blue Lock").await;
        assert!(ownership.komga_connected);
        assert_eq!(ownership.series_id.as_deref(), Some("s-1"));
        assert_eq!(ownership.volumes.len(), 3); // Vol 1 (Owned), Vol 2 (Owned), Vol 3 (Acquiring)

        let v1 = ownership.volumes.iter().find(|v| v.volume == 1.0).unwrap();
        assert_eq!(v1.status, OwnershipStatus::Owned);
        assert_eq!(v1.book_id.as_deref(), Some("b-1"));

        let v2 = ownership.volumes.iter().find(|v| v.volume == 2.0).unwrap();
        assert_eq!(v2.status, OwnershipStatus::Owned);

        let v3 = ownership.volumes.iter().find(|v| v.volume == 3.0).unwrap();
        assert_eq!(v3.status, OwnershipStatus::Acquiring);

        // 2. Query ownership for ambiguous series
        let ambig = ownership_svc
            .get_title_ownership(20, "Ambiguous Title")
            .await;
        assert!(ambig.komga_connected);
        assert_eq!(ambig.series_id, None, "Ambiguous matches must fail closed");
        assert!(ambig.volumes.is_empty());

        // 3. Cache invalidation and explicit refresh
        ownership_svc.invalidate(10);
        let fresh = ownership_svc.refresh_title(10, "Blue Lock").await;
        assert_eq!(fresh.volumes.len(), 3);
    }

    #[tokio::test]
    async fn ownership_blue_lock_canonical_regression_and_honest_denominators() {
        use axum::{routing::get, Json, Router};
        use serde_json::json;

        // Mock Komga returns 38 volumes for Blue Lock
        let app = Router::new()
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            {
                                "id": "s-bluelock",
                                "name": "Blue Lock",
                                "libraryId": "lib-en",
                                "booksCount": 38
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/s-bluelock/books",
                get(|| async {
                    let mut books = Vec::new();
                    for i in 1..=38 {
                        books.push(json!({
                            "id": format!("b-{i}"),
                            "seriesId": "s-bluelock",
                            "name": format!("Blue Lock v{:02}", i),
                            "number": i as f64,
                            "metadata": { "numberSort": i as f64 }
                        }));
                    }
                    Json(json!({ "content": books }))
                }),
            )
            .route(
                "/api/v1/libraries",
                get(|| async {
                    Json(json!([
                        { "id": "lib-en", "name": "Manga", "root": "/manga", "unavailable": false }
                    ]))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let db = crate::pipeline::tests::test_db_instance();
        db.call(move |conn| {
            let mut map = serde_json::Map::new();
            map.insert("pipeline.komga_enabled".into(), serde_json::json!(true));
            map.insert("pipeline.komga_url".into(), serde_json::json!(format!("http://127.0.0.1:{port}")));
            map.insert("pipeline.komga_library_id".into(), serde_json::json!("lib-en"));
            crate::settings::put(conn, &map)?;
            crate::db::set_state(conn, "pipeline.komga_api_key", "mock-key")?;

            // 1. Chapter-only source (e.g. MangaDot Blue Lock: 368 chapters, NO volume numbers)
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (50, 1, '/bl', 'Blue Lock', 1)", [])?;
            for i in 1..=368 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read, chapter_number) VALUES (?1, 50, ?2, ?3, ?1, 0, 0, ?4)",
                    rusqlite::params![5000 + i, format!("/c{i}"), format!("Chapter {i}"), i as f64],
                )?;
            }

            Ok(())
        }).await.unwrap();

        let ownership_svc = OwnershipService::new(Client::new(), db);

        // Verify Blue Lock ownership summary:
        // Komga owns 38 volumes. Source provides 368 chapters.
        let ownership = ownership_svc.get_title_ownership(50, "Blue Lock").await;
        assert!(ownership.komga_connected);
        assert_eq!(ownership.series_id.as_deref(), Some("s-bluelock"));
        assert_eq!(ownership.library_name.as_deref(), Some("Manga"));

        // Owned numerator comes strictly from Komga (38)
        assert_eq!(ownership.summary.owned_volumes_count, 38);
        assert_eq!(ownership.volumes.len(), 38);

        // Denominator must be honest:
        // Do NOT invent chapter->volume mappings and do NOT show 38/368!
        assert_eq!(ownership.summary.total_source_volumes, None);
        assert_eq!(ownership.summary.source_release_type, "chapter");
        assert_eq!(
            ownership.summary.explanation.as_deref(),
            Some("Source provides chapters")
        );
        // Chapter ownership must NOT be inferred from volume files
        assert_eq!(ownership.summary.owned_chapters_count, 0);
        assert!(ownership.chapters.is_empty());
    }

    #[tokio::test]
    async fn ownership_volume_source_and_chapter_source_matching() {
        use axum::{routing::get, Json, Router};
        use serde_json::json;

        // Mock Komga returns 12 volumes and 3 chapters for Mixed Title
        let app = Router::new()
            .route(
                "/api/v1/series",
                get(|| async {
                    Json(json!({
                        "content": [
                            { "id": "s-mixed", "name": "Mixed Title", "libraryId": "lib-en", "booksCount": 15 }
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/series/s-mixed/books",
                get(|| async {
                    let mut books = Vec::new();
                    for v in 1..=12 {
                        books.push(json!({
                            "id": format!("bv-{v}"),
                            "seriesId": "s-mixed",
                            "name": format!("Mixed Title v{:02}", v),
                            "number": v as f64,
                            "metadata": { "numberSort": v as f64 }
                        }));
                    }
                    for c in 1..=3 {
                        books.push(json!({
                            "id": format!("bc-{c}"),
                            "seriesId": "s-mixed",
                            "name": format!("Mixed Title - c{:03}", c),
                            "number": c as f64,
                            "metadata": { "numberSort": c as f64 }
                        }));
                    }
                    Json(json!({ "content": books }))
                }),
            )
            .route(
                "/api/v1/libraries",
                get(|| async {
                    Json(json!([
                        { "id": "lib-en", "name": "Manga", "root": "/manga", "unavailable": false }
                    ]))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let db = crate::pipeline::tests::test_db_instance();
        db.call(move |conn| {
            let mut map = serde_json::Map::new();
            map.insert("pipeline.komga_enabled".into(), serde_json::json!(true));
            map.insert("pipeline.komga_url".into(), serde_json::json!(format!("http://127.0.0.1:{port}")));
            map.insert("pipeline.komga_library_id".into(), serde_json::json!("lib-en"));
            crate::settings::put(conn, &map)?;
            crate::db::set_state(conn, "pipeline.komga_api_key", "mock-key")?;

            // Insert manga with mixed releases: 14 volumes and 3 chapters
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (60, 1, '/mixed', 'Mixed Title', 1)", [])?;
            for v in 1..=14 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read, chapter_number) VALUES (?1, 60, ?2, ?3, ?1, 0, 0, ?4)",
                    rusqlite::params![6000 + v, format!("/v{v}"), format!("Volume {v}"), v as f64],
                )?;
            }
            for c in 1..=3 {
                conn.execute(
                    "INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read, chapter_number) VALUES (?1, 60, ?2, ?3, ?1, 0, 0, ?4)",
                    rusqlite::params![7000 + c, format!("/c{c}"), format!("Chapter {c}"), c as f64],
                )?;
            }

            Ok(())
        }).await.unwrap();

        let ownership_svc = OwnershipService::new(Client::new(), db);
        let ownership = ownership_svc.get_title_ownership(60, "Mixed Title").await;

        assert_eq!(ownership.summary.source_release_type, "mixed");
        // Volumes 12 / 14
        assert_eq!(ownership.summary.owned_volumes_count, 12);
        assert_eq!(ownership.summary.total_source_volumes, Some(14));
        // Chapters 3 / 3
        assert_eq!(ownership.summary.owned_chapters_count, 3);
        assert_eq!(ownership.summary.total_source_chapters, Some(3));
        assert_eq!(ownership.summary.explanation, None);
    }
}
