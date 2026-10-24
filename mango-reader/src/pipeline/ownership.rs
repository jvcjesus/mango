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
pub struct TitleOwnership {
    pub komga_connected: bool,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub library_id: Option<String>,
    pub volumes: Vec<VolumeOwnership>,
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

    async fn query_komga_ownership(&self, manga_id: i64, title: &str) -> TitleOwnership {
        // Load pipeline config & secrets from db
        let (komga_enabled, komga_url, komga_key, lib_id, active_downloads) = match self
            .db
            .call(move |conn| {
                let enabled = crate::settings::get::<bool>(conn, "pipeline.komga_enabled").unwrap_or(false);
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

                Ok((enabled, url, key, lib_id, downloads))
            })
            .await
        {
            Ok(res) => res,
            Err(e) => {
                tracing::warn!("OWNERSHIP: failed to read configuration from DB: {e}");
                return TitleOwnership {
                    komga_connected: false,
                    series_id: None,
                    series_name: None,
                    library_id: None,
                    volumes: Vec::new(),
                };
            }
        };

        if !komga_enabled {
            return TitleOwnership {
                komga_connected: false,
                series_id: None,
                series_name: None,
                library_id: None,
                volumes: Vec::new(),
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
                    series_id: None,
                    series_name: None,
                    library_id: lib_id,
                    volumes: Vec::new(),
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
        if exact_matches.len() != 1 {
            if exact_matches.len() > 1 {
                tracing::warn!("OWNERSHIP: found multiple exact series matches for {title:?}; refusing ambiguous match");
            }
            return TitleOwnership {
                komga_connected: true,
                series_id: None,
                series_name: None,
                library_id: lib_id,
                volumes: Vec::new(),
            };
        }

        let matched_series = &exact_matches[0];
        let books = match client.get_books_in_series(&matched_series.id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("OWNERSHIP: failed to fetch books for series {}: {e}", matched_series.id);
                return TitleOwnership {
                    komga_connected: true,
                    series_id: Some(matched_series.id.clone()),
                    series_name: Some(matched_series.name.clone()),
                    library_id: Some(matched_series.library_id.clone()),
                    volumes: Vec::new(),
                };
            }
        };

        // Extract owned volumes from books
        let mut owned_volumes = HashMap::new();
        for b in books {
            let vol_num = crate::release::parse_volume(&b.name).or_else(|| {
                b.metadata
                    .as_ref()
                    .and_then(|m| m.number_sort.or_else(|| m.number.as_deref().and_then(|n| n.parse::<f64>().ok())))
            });

            if let Some(vol) = vol_num {
                owned_volumes.insert(vol.to_bits(), (b.id, b.name));
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
        for (_id, name, chapter_number, _state) in active_downloads {
            let rel = crate::release::parse_release(&name, chapter_number);
            let vol_num = if rel.release_type == crate::release::ReleaseType::Volume {
                rel.volume.or(Some(chapter_number))
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

        volumes.sort_by(|a, b| a.volume.partial_cmp(&b.volume).unwrap_or(std::cmp::Ordering::Equal));

        TitleOwnership {
            komga_connected: true,
            series_id: Some(matched_series.id.clone()),
            series_name: Some(matched_series.name.clone()),
            library_id: Some(matched_series.library_id.clone()),
            volumes,
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
        let ambig = ownership_svc.get_title_ownership(20, "Ambiguous Title").await;
        assert!(ambig.komga_connected);
        assert_eq!(ambig.series_id, None, "Ambiguous matches must fail closed");
        assert!(ambig.volumes.is_empty());

        // 3. Cache invalidation
        ownership_svc.invalidate(10);
        let fresh = ownership_svc.get_title_ownership(10, "Blue Lock").await;
        assert_eq!(fresh.volumes.len(), 3);
    }
}
