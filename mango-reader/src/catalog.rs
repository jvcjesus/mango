//! First-class catalog titles and optional reading sources (Milestone M6.7).
//!
//! Enables manga to exist natively in Mango from MangaBaka work identity without
//! requiring web-chapter scraping, extension dependencies, or fake volume rows.

use crate::{
    api::AppState,
    db::{now, Db},
    error::{AppError, Result},
    pipeline::mangabaka::{
        extract_mb_series_cover, extract_mb_series_title_info, MangaBakaClient,
    },
    pipeline::metadata::{get_cached_series, upsert_cached_series, CachedMangaBakaSeries},
    sync::catalog::add_or_update_tracked,
    sync::reconciliation::ReconciliationTarget,
};
use reqwest::Method;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaBakaSearchResult {
    pub mangabaka_id: i64,
    pub title: String,
    pub english_title: Option<String>,
    pub romaji_title: Option<String>,
    pub native_title: Option<String>,
    pub aliases: Vec<String>,
    pub cover_url: Option<String>,
    pub authors: Vec<String>,
    pub artists: Vec<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub status: Option<String>,
    pub total_volumes: Option<i64>,
    pub description: Option<String>,
    pub already_in_mango: bool,
    pub existing_manga_id: Option<i64>,
    pub existing_is_catalog: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddMangaBakaPayload {
    pub convert_existing: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMangaBakaResponse {
    pub manga_id: i64,
    pub created: bool,
    pub converted: bool,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertToCatalogResponse {
    pub catalog_manga_id: i64,
    pub source_manga_id: i64,
    pub title: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertToSourceResponse {
    pub source_manga_id: i64,
    pub catalog_manga_id: i64,
    pub title: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachSourceResponse {
    pub manga_id: i64,
    pub detached_source_manga_id: i64,
    pub success: bool,
    pub message: String,
}

/// Parses an explicit integer ID from a query string, URL, or plain number.
pub fn parse_mangabaka_id_input(query: &str) -> Option<i64> {
    let trimmed = query.trim();
    if let Ok(id) = trimmed.parse::<i64>() {
        return Some(id);
    }
    if let Some(pos) = trimmed.find("/series/") {
        let after = &trimmed[pos + 8..];
        let id_str = after.split(&['/', '?', '#'][..]).next().unwrap_or("");
        if let Ok(id) = id_str.parse::<i64>() {
            return Some(id);
        }
    }
    None
}

/// Searches MangaBaka catalog for works, caching results and noting existing Mango titles.
pub async fn search_mangabaka_catalog(
    client: &MangaBakaClient,
    db: &Db,
    query: &str,
) -> Result<Vec<MangaBakaSearchResult>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut raw_candidates: Vec<serde_json::Value> = Vec::new();

    // If query is an ID or URL, query GET /series/{id} directly
    if let Some(explicit_id) = parse_mangabaka_id_input(trimmed) {
        let endpoint = format!("/series/{explicit_id}");
        if let Ok(resp) = client.public_request(Method::GET, &endpoint).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let item = body.get("data").unwrap_or(&body).clone();
                    if item.get("id").is_some() {
                        raw_candidates.push(item);
                    }
                }
            }
        }
    }

    // Also run search query if not already found or query has non-numeric characters
    if raw_candidates.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        let resp = match client
            .public_request(Method::GET, "/series/search")
            .query(&[("q", trimmed)])
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                tracing::warn!("MANGABAKA catalog search returned HTTP {}", r.status());
                return Ok(Vec::new());
            }
            Err(e) => {
                tracing::warn!("MANGABAKA catalog search network error: {e}");
                return Ok(Vec::new());
            }
        };

        if let Ok(body) = resp.json::<serde_json::Value>().await {
            if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
                for item in arr {
                    // Avoid duplicating explicit ID if already fetched
                    let item_id = item.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                    if !raw_candidates.iter().any(|c| c.get("id").and_then(|v| v.as_i64()) == Some(item_id)) {
                        raw_candidates.push(item.clone());
                    }
                }
            }
        }
    }

    let mut results = Vec::new();

    for item in &raw_candidates {
        let sid = match item.get("id").and_then(|v| v.as_i64()) {
            Some(id) if id > 0 => id,
            _ => continue,
        };

        let (display_title, english_title, romaji_title, native_title, aliases) =
            extract_mb_series_title_info(item);
        let cover_url = extract_mb_series_cover(item);

        let mut authors: Vec<String> = Vec::new();
        let mut artists: Vec<String> = Vec::new();

        if let Some(arr) = item.get("authors").and_then(|v| v.as_array()) {
            for a in arr {
                if let Some(s) = a.as_str() {
                    let clean = s.trim().to_string();
                    if !clean.is_empty() && !authors.contains(&clean) {
                        authors.push(clean);
                    }
                } else if let Some(obj) = a.as_object() {
                    let name = obj.get("name").or_else(|| obj.get("author")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    let role = obj.get("role").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
                    if !name.is_empty() {
                        if role.contains("art") && !artists.contains(&name) {
                            artists.push(name.clone());
                        }
                        if (role.contains("story") || role.contains("author") || role.contains("writer") || role.is_empty()) && !authors.contains(&name) {
                            authors.push(name);
                        }
                    }
                }
            }
        }

        if let Some(arr) = item.get("artists").and_then(|v| v.as_array()) {
            for a in arr {
                let name = if let Some(s) = a.as_str() {
                    s.trim().to_string()
                } else if let Some(obj) = a.as_object() {
                    obj.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
                } else {
                    String::new()
                };
                if !name.is_empty() && !artists.contains(&name) {
                    artists.push(name);
                }
            }
        }

        let publisher = item.get("publisher").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let status = item.get("status").and_then(|v| v.as_str()).map(|s| s.trim().to_lowercase());
        let total_volumes = item.get("total_volumes").or_else(|| item.get("volumes")).and_then(|v| v.as_i64());
        let description = item.get("description").and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.trim().to_string())
            } else if let Some(d) = v.get("desc").or_else(|| v.get("description")).and_then(|s| s.as_str()) {
                Some(d.trim().to_string())
            } else {
                None
            }
        }).filter(|s| !s.is_empty());

        let mut genres: Vec<String> = Vec::new();
        if let Some(arr) = item.get("genres").and_then(|v| v.as_array()) {
            for g in arr {
                if let Some(s) = g.as_str() {
                    let clean = s.trim().to_string();
                    if !clean.is_empty() && !genres.contains(&clean) {
                        genres.push(clean);
                    }
                }
            }
        }

        let mut tags: Vec<String> = Vec::new();
        if let Some(arr) = item.get("tags").and_then(|v| v.as_array()) {
            for t in arr {
                if let Some(s) = t.as_str() {
                    let clean = s.trim().to_string();
                    if !clean.is_empty() && !tags.contains(&clean) {
                        tags.push(clean);
                    }
                }
            }
        }

        // Cache into SQLite mangabaka_series_cache
        let cached = CachedMangaBakaSeries {
            series_id: sid,
            title: display_title.clone(),
            english_title: english_title.clone(),
            romaji_title: romaji_title.clone(),
            native_title: native_title.clone(),
            aliases: aliases.clone(),
            cover_url: cover_url.clone(),
            authors: authors.clone(),
            artists: artists.clone(),
            publisher: publisher.clone(),
            genres: genres.clone(),
            tags: tags.clone(),
            summary: description.clone(),
            total_volumes,
            status: status.clone(),
            web_url: Some(format!("https://mangabaka.org/series/{sid}")),
            raw_json: Some(item.to_string()),
            updated_at: now(),
        };

        let cached_c = cached.clone();
        let _ = db.call(move |conn| upsert_cached_series(conn, &cached_c)).await;

        // Check if title or MangaBaka ID already exists in Mango manga table
        let query_title = display_title.clone();
        let query_url = format!("catalog:{sid}");
        let existing = db
            .call(move |conn| {
                let row = conn.query_row(
                    "SELECT id, source_id, in_library
                     FROM manga
                     WHERE mangabaka_id = ?1 OR url = ?2 OR (title = ?3 AND in_library = 1)
                     ORDER BY (source_id = 0) DESC, in_library DESC LIMIT 1",
                    params![sid, query_url, query_title],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)? == 1)),
                )
                .optional()?;
                Ok(row)
            })
            .await
            .ok()
            .flatten();

        let (already_in_mango, existing_manga_id, existing_is_catalog) = match existing {
            Some((m_id, s_id, _in_lib)) => (true, Some(m_id), Some(s_id == 0)),
            None => (false, None, None),
        };

        results.push(MangaBakaSearchResult {
            mangabaka_id: sid,
            title: display_title,
            english_title,
            romaji_title,
            native_title,
            aliases,
            cover_url,
            authors,
            artists,
            publisher,
            genres,
            tags,
            status,
            total_volumes,
            description,
            already_in_mango,
            existing_manga_id,
            existing_is_catalog,
        });
    }

    Ok(results)
}

/// Adds a title directly from MangaBaka work identity into Mango as a catalog manga (source_id = 0).
pub async fn add_mangabaka_catalog_title(
    app: &AppState,
    mb_id: i64,
    convert_existing: bool,
) -> Result<AddMangaBakaResponse> {
    if mb_id <= 0 {
        return Err(AppError::bad_request("Invalid MangaBaka ID"));
    }

    let catalog_url = format!("catalog:{mb_id}");

    // 1. Idempotency: Check if catalog title already exists for this MangaBaka ID
    let existing_catalog: Option<(i64, String)> = app
        .db
        .call({
            let url_c = catalog_url.clone();
            move |conn| {
                let row = conn.query_row(
                    "SELECT id, title FROM manga WHERE (mangabaka_id = ?1 OR url = ?2) AND source_id = 0 LIMIT 1",
                    params![mb_id, url_c],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?;
                Ok(row)
            }
        })
        .await?;

    if let Some((cat_id, title)) = existing_catalog {
        // Ensure in_library is 1
        let _ = app.db.call(move |conn| {
            conn.execute("UPDATE manga SET in_library = 1 WHERE id = ?1", [cat_id])?;
            Ok(())
        }).await;

        return Ok(AddMangaBakaResponse {
            manga_id: cat_id,
            created: false,
            converted: false,
            title,
            message: "Catalog title already in library".to_string(),
        });
    }

    // 2. Check if an existing source-backed manga exists with this MangaBaka ID or matching title
    let existing_source: Option<(i64, String)> = app
        .db
        .call(move |conn| {
            // Check mangabaka_id or title in mangabaka_series_cache
            let cached_title: Option<String> = conn
                .query_row(
                    "SELECT title FROM mangabaka_series_cache WHERE series_id = ?1",
                    [mb_id],
                    |r| r.get(0),
                )
                .optional()?;

            if let Some(ref t) = cached_title {
                if let Ok(row) = conn.query_row(
                    "SELECT id, title FROM manga WHERE (mangabaka_id = ?1 OR title = ?2) AND source_id > 0 AND in_library = 1 LIMIT 1",
                    params![mb_id, t],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                ) {
                    return Ok(Some(row));
                }
            }

            let row = conn.query_row(
                "SELECT id, title FROM manga WHERE mangabaka_id = ?1 AND source_id > 0 AND in_library = 1 LIMIT 1",
                [mb_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
            Ok(row)
        })
        .await?;

    if let Some((source_id, _source_title)) = existing_source {
        if convert_existing {
            let conv = convert_to_catalog_title(app, source_id).await?;
            return Ok(AddMangaBakaResponse {
                manga_id: conv.catalog_manga_id,
                created: true,
                converted: true,
                title: conv.title,
                message: "Converted existing source title to catalog title".to_string(),
            });
        }
    }

    // 3. Retrieve series metadata from cache or fetch from MangaBaka API
    let cached_series = app
        .db
        .call(move |conn| get_cached_series(conn, mb_id))
        .await?;

    let series_meta = match cached_series {
        Some(s) => s,
        None => {
            let (cfg, mb_token, _) = app.pipeline.load_internal_config().await?;
            let client = MangaBakaClient::new(app.trackers.http().clone(), cfg.mangabaka_url, mb_token);
            let endpoint = format!("/series/{mb_id}");
            let resp = client.public_request(Method::GET, &endpoint).send().await
                .map_err(|e| AppError::internal(format!("Failed to reach MangaBaka: {e}")))?;
            if !resp.status().is_success() {
                return Err(AppError::new(
                    resp.status().as_u16(),
                    "mangabaka_not_found",
                    format!("MangaBaka work #{mb_id} not found"),
                ));
            }
            let raw: serde_json::Value = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
            let item = raw.get("data").unwrap_or(&raw).clone();
            let (display_title, english_title, romaji_title, native_title, aliases) =
                extract_mb_series_title_info(&item);
            let cover_url = extract_mb_series_cover(&item);

            let s = CachedMangaBakaSeries {
                series_id: mb_id,
                title: display_title,
                english_title,
                romaji_title,
                native_title,
                aliases,
                cover_url,
                authors: Vec::new(),
                artists: Vec::new(),
                publisher: item.get("publisher").and_then(|v| v.as_str()).map(|s| s.to_string()),
                genres: Vec::new(),
                tags: Vec::new(),
                summary: item.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
                total_volumes: item.get("total_volumes").and_then(|v| v.as_i64()),
                status: item.get("status").and_then(|v| v.as_str()).map(|s| s.to_string()),
                web_url: Some(format!("https://mangabaka.org/series/{mb_id}")),
                raw_json: Some(item.to_string()),
                updated_at: now(),
            };
            let s_c = s.clone();
            let _ = app.db.call(move |conn| upsert_cached_series(conn, &s_c)).await;
            s
        }
    };

    let title_str = series_meta.title.clone();
    let author_str = if series_meta.authors.is_empty() { None } else { Some(series_meta.authors.join(", ")) };
    let artist_str = if series_meta.artists.is_empty() { None } else { Some(series_meta.artists.join(", ")) };
    let genre_str = if series_meta.genres.is_empty() { None } else { Some(series_meta.genres.join(", ")) };
    let desc_str = series_meta.summary.clone();
    let thumb_str = series_meta.cover_url.clone();
    let status_int = if series_meta.status.as_deref().unwrap_or("").to_lowercase() == "completed" { 2 } else { 1 };
    let now_ts = now();

    // 4. Insert catalog manga row into manga table (source_id = 0)
    let new_manga_id: i64 = app
        .db
        .call({
            let title_c = title_str.clone();
            let url_c = catalog_url.clone();
            move |conn| {
                conn.execute(
                    "INSERT INTO manga (source_id, url, title, author, artist, description, genre, thumbnail_url, status, in_library, mangabaka_id, added_at, initialized)
                     VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, 1)",
                    params![url_c, title_c, author_str, artist_str, desc_str, genre_str, thumb_str, status_int, mb_id, now_ts],
                )?;
                let id = conn.last_insert_rowid();

                // Add to tracked_catalog
                add_or_update_tracked(conn, &title_c, "catalog", None, Some(id))?;

                Ok(id)
            }
        })
        .await?;

    // 5. Look up Komga series if present in local library
    let komga_series_id: Option<String> = {
        let (cfg, _, komga_api_key) = app.pipeline.load_internal_config().await?;
        if cfg.komga_enabled {
            let komga_client = crate::pipeline::komga::KomgaClient::new(
                app.trackers.http().clone(),
                cfg.komga_url,
                komga_api_key,
            );
            match komga_client.search_series(&title_str, None).await {
                Ok(series_list) => {
                    let norm = crate::pipeline::mangabaka::normalize_title(&title_str);
                    series_list.into_iter().find(|s| crate::pipeline::mangabaka::normalize_title(&s.name) == norm).map(|s| s.id)
                }
                Err(_) => None,
            }
        } else {
            None
        }
    };

    // 6. If Komga series found, persist tracker_mappings and update tracked_catalog
    if let Some(ref k_id) = komga_series_id {
        let k_id_c = k_id.clone();
        let title_c = title_str.clone();
        let _ = app
            .db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO tracker_mappings (komga_series_id, tracker, tracker_id, updated_at, mapping_source, confidence)
                     VALUES (?1, 'mangabaka', ?2, ?3, 'manual', 'resolved')
                     ON CONFLICT(komga_series_id, tracker) DO UPDATE SET
                        tracker_id = excluded.tracker_id,
                        confidence = 'resolved',
                        updated_at = excluded.updated_at",
                    params![k_id_c, mb_id.to_string(), now_ts],
                )?;
                add_or_update_tracked(conn, &title_c, "catalog", Some(&k_id_c), Some(new_manga_id))?;
                Ok(())
            })
            .await;
    }

    // 7. Run initial series reconciliation
    let target = ReconciliationTarget {
        manga_id: Some(new_manga_id),
        komga_series_id: komga_series_id.clone(),
        title: title_str.clone(),
        edition: None,
    };
    let _ = app.sync.run_reconciliation(target).await;

    // 8. Trigger OpenTome mapping
    if app.opentome.is_enabled().await {
        let _ = app.opentome.get_series_schedule(new_manga_id).await;
    }

    Ok(AddMangaBakaResponse {
        manga_id: new_manga_id,
        created: true,
        converted: false,
        title: title_str,
        message: "Added catalog manga to library".to_string(),
    })
}

/// Converts an existing source-backed manga into a catalog-native manga (source_id = 0)
/// while preserving the source manga and its scraped chapters for reversible rollback.
pub async fn convert_to_catalog_title(
    app: &AppState,
    source_manga_id: i64,
) -> Result<ConvertToCatalogResponse> {
    // 1. Fetch source manga info
    let source_manga = app.titles.get(source_manga_id).await?;

    if source_manga.is_catalog {
        return Ok(ConvertToCatalogResponse {
            catalog_manga_id: source_manga_id,
            source_manga_id,
            title: source_manga.title,
            success: true,
            message: "Title is already a catalog title".to_string(),
        });
    }

    if let Some(existing_cat_id) = source_manga.catalog_manga_id {
        return Ok(ConvertToCatalogResponse {
            catalog_manga_id: existing_cat_id,
            source_manga_id,
            title: source_manga.title,
            success: true,
            message: "Title is already linked to catalog title".to_string(),
        });
    }

    let title_str = source_manga.title.clone();

    // 2. Resolve MangaBaka ID from multiple authoritative sources
    let mb_id: Option<i64> = app
        .db
        .call({
            let title_c = title_str.clone();
            move |conn| {
                // Check manga.mangabaka_id
                let direct_id: Option<i64> = conn
                    .query_row(
                        "SELECT mangabaka_id FROM manga WHERE id = ?1",
                        [source_manga_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if direct_id.is_some() {
                    return Ok(direct_id);
                }

                // Check series_reconciliations
                let rec_id: Option<i64> = conn
                    .query_row(
                        "SELECT mangabaka_id FROM series_reconciliations WHERE (manga_id = ?1 OR series_id = ?2) AND mangabaka_id IS NOT NULL",
                        params![source_manga_id, format!("manga:{source_manga_id}")],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if rec_id.is_some() {
                    return Ok(rec_id);
                }

                // Check tracked_catalog komga_series_id -> tracker_mappings
                let komga_id: Option<String> = conn
                    .query_row(
                        "SELECT komga_series_id FROM tracked_catalog WHERE manga_id = ?1",
                        [source_manga_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();

                if let Some(ref k_id) = komga_id {
                    let map_id: Option<String> = conn
                        .query_row(
                            "SELECT tracker_id FROM tracker_mappings WHERE komga_series_id = ?1 AND tracker = 'mangabaka'",
                            [k_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(ref s) = map_id {
                        if let Ok(num) = s.parse::<i64>() {
                            return Ok(Some(num));
                        }
                    }
                }

                // Check mangabaka_series_cache
                let cached_id: Option<i64> = conn
                    .query_row(
                        "SELECT series_id FROM mangabaka_series_cache WHERE title = ?1",
                        [&title_c],
                        |r| r.get(0),
                    )
                    .optional()?;

                Ok(cached_id)
            }
        })
        .await?;

    let catalog_url = match mb_id {
        Some(id) => format!("catalog:{id}"),
        None => format!("catalog:{source_manga_id}"),
    };

    // 3. Find or create the catalog manga row
    let catalog_manga_id: i64 = app
        .db
        .call({
            let url_c = catalog_url.clone();
            let title_c = title_str.clone();
            let author_c = source_manga.author.clone();
            let artist_c = source_manga.artist.clone();
            let desc_c = source_manga.description.clone();
            let genre_c = if source_manga.genres.is_empty() { None } else { Some(source_manga.genres.join(", ")) };
            let thumb_c = source_manga.cover.clone();
            let status_c = source_manga.status;
            let now_ts = now();

            move |conn| {
                // Check if catalog manga already exists
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM manga WHERE (source_manga_id = ?1 OR (mangabaka_id IS NOT NULL AND mangabaka_id = ?2) OR url = ?3) AND source_id = 0",
                        params![source_manga_id, mb_id, url_c],
                        |r| r.get(0),
                    )
                    .optional()?;

                let cat_id = if let Some(id) = existing {
                    conn.execute(
                        "UPDATE manga SET in_library = 1, source_manga_id = ?1, mangabaka_id = COALESCE(?2, mangabaka_id) WHERE id = ?3",
                        params![source_manga_id, mb_id, id],
                    )?;
                    id
                } else {
                    conn.execute(
                        "INSERT INTO manga (source_id, url, title, author, artist, description, genre, thumbnail_url, status, in_library, mangabaka_id, source_manga_id, added_at, initialized)
                         VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, 1)",
                        params![url_c, title_c, author_c, artist_c, desc_c, genre_c, thumb_c, status_c, mb_id, source_manga_id, now_ts],
                    )?;
                    conn.last_insert_rowid()
                };

                // Update source manga to point to catalog manga and set in_library = 0
                conn.execute(
                    "UPDATE manga SET in_library = 0, catalog_manga_id = ?1 WHERE id = ?2",
                    params![cat_id, source_manga_id],
                )?;

                // Migrate associations from source_manga_id to cat_id
                let old_series_id = format!("manga:{source_manga_id}");
                let new_series_id = format!("manga:{cat_id}");

                // series_reconciliations
                conn.execute(
                    "DELETE FROM series_reconciliations WHERE series_id = ?1 OR manga_id = ?2",
                    params![new_series_id, cat_id],
                )?;
                conn.execute(
                    "UPDATE series_reconciliations SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
                    params![cat_id, new_series_id, source_manga_id],
                )?;

                // opentome_series_mappings
                conn.execute(
                    "UPDATE opentome_series_mappings SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
                    params![cat_id, new_series_id, source_manga_id],
                )?;

                // tracked_catalog
                conn.execute(
                    "UPDATE tracked_catalog SET manga_id = ?1 WHERE manga_id = ?2",
                    params![cat_id, source_manga_id],
                )?;

                // release_discovery_targets
                conn.execute(
                    "UPDATE release_discovery_targets SET series_id = ?1 WHERE series_id = ?2",
                    params![new_series_id, old_series_id],
                )?;

                // discovered_release_candidates
                conn.execute(
                    "UPDATE discovered_release_candidates SET series_id = ?1 WHERE series_id = ?2",
                    params![new_series_id, old_series_id],
                )?;

                // volume_discovery_status
                conn.execute(
                    "UPDATE volume_discovery_status SET series_id = ?1 WHERE series_id = ?2",
                    params![new_series_id, old_series_id],
                )?;

                // candidate_approvals
                conn.execute(
                    "UPDATE candidate_approvals SET manga_id = ?1 WHERE manga_id = ?2",
                    params![cat_id, source_manga_id],
                )?;

                // acquisition_history
                conn.execute(
                    "UPDATE acquisition_history SET manga_id = ?1 WHERE manga_id = ?2",
                    params![cat_id, source_manga_id],
                )?;

                Ok(cat_id)
            }
        })
        .await?;

    // Invalidate ownership cache for both IDs
    app.pipeline.ownership.invalidate(source_manga_id);
    app.pipeline.ownership.invalidate(catalog_manga_id);

    // Re-reconcile catalog title
    let ownership = app.pipeline.ownership.get_title_ownership(catalog_manga_id, &title_str).await;
    let target = ReconciliationTarget {
        manga_id: Some(catalog_manga_id),
        komga_series_id: ownership.series_id,
        title: title_str.clone(),
        edition: None,
    };
    let _ = app.sync.run_reconciliation(target).await;

    // Refresh OpenTome schedule
    if app.opentome.is_enabled().await {
        let _ = app.opentome.get_series_schedule(catalog_manga_id).await;
    }

    Ok(ConvertToCatalogResponse {
        catalog_manga_id,
        source_manga_id,
        title: title_str,
        success: true,
        message: "Successfully converted to catalog title".to_string(),
    })
}

/// Reverts a conversion, restoring the source-backed manga as the primary library title.
pub async fn revert_to_source_title(
    app: &AppState,
    any_manga_id: i64,
) -> Result<RevertToSourceResponse> {
    let (source_id, catalog_id, title) = app
        .db
        .call(move |conn| {
            // Find pair whether called on source or catalog manga
            let row: (i64, Option<i64>, Option<i64>, String) = conn.query_row(
                "SELECT id, source_manga_id, catalog_manga_id, title FROM manga WHERE id = ?1",
                [any_manga_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;

            let (src, cat) = if let Some(src_id) = row.1 {
                (src_id, row.0)
            } else if let Some(cat_id) = row.2 {
                (row.0, cat_id)
            } else {
                return Err(AppError::bad_request("Manga is not part of a catalog/source conversion pair"));
            };

            // Set source manga in_library = 1, catalog_manga_id = NULL
            conn.execute(
                "UPDATE manga SET in_library = 1, catalog_manga_id = NULL WHERE id = ?1",
                [src],
            )?;

            // Set catalog manga in_library = 0, source_manga_id = NULL
            conn.execute(
                "UPDATE manga SET in_library = 0, source_manga_id = NULL WHERE id = ?1",
                [cat],
            )?;

            // Re-point associations back to src
            let old_series_id = format!("manga:{cat}");
            let new_series_id = format!("manga:{src}");

            // Clear any conflicting destination reconciliation
            conn.execute(
                "DELETE FROM series_reconciliations WHERE series_id = ?1 OR manga_id = ?2",
                params![new_series_id, src],
            )?;
            conn.execute(
                "UPDATE series_reconciliations SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
                params![src, new_series_id, cat],
            )?;
            conn.execute(
                "UPDATE opentome_series_mappings SET manga_id = ?1, series_id = ?2 WHERE manga_id = ?3",
                params![src, new_series_id, cat],
            )?;
            conn.execute(
                "UPDATE tracked_catalog SET manga_id = ?1 WHERE manga_id = ?2",
                params![src, cat],
            )?;
            conn.execute(
                "UPDATE release_discovery_targets SET series_id = ?1 WHERE series_id = ?2",
                params![new_series_id, old_series_id],
            )?;
            conn.execute(
                "UPDATE discovered_release_candidates SET series_id = ?1 WHERE series_id = ?2",
                params![new_series_id, old_series_id],
            )?;
            conn.execute(
                "UPDATE volume_discovery_status SET series_id = ?1 WHERE series_id = ?2",
                params![new_series_id, old_series_id],
            )?;
            conn.execute(
                "UPDATE candidate_approvals SET manga_id = ?1 WHERE manga_id = ?2",
                params![src, cat],
            )?;
            conn.execute(
                "UPDATE acquisition_history SET manga_id = ?1 WHERE manga_id = ?2",
                params![src, cat],
            )?;

            Ok((src, cat, row.3))
        })
        .await?;

    app.pipeline.ownership.invalidate(source_id);
    app.pipeline.ownership.invalidate(catalog_id);

    let ownership = app.pipeline.ownership.get_title_ownership(source_id, &title).await;
    let target = ReconciliationTarget {
        manga_id: Some(source_id),
        komga_series_id: ownership.series_id,
        title: title.clone(),
        edition: None,
    };
    let _ = app.sync.run_reconciliation(target).await;

    Ok(RevertToSourceResponse {
        source_manga_id: source_id,
        catalog_manga_id: catalog_id,
        title,
        success: true,
        message: "Successfully reverted to source title primary".to_string(),
    })
}

/// Detaches an optional reading source from a catalog title.
pub async fn detach_source_from_catalog(
    app: &AppState,
    catalog_manga_id: i64,
) -> Result<DetachSourceResponse> {
    let detached_src: i64 = app
        .db
        .call(move |conn| {
            let src_id: Option<i64> = conn
                .query_row(
                    "SELECT source_manga_id FROM manga WHERE id = ?1 AND source_id = 0",
                    [catalog_manga_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();

            let detached = src_id.ok_or_else(|| AppError::bad_request("Catalog title has no reading source attached"))?;

            conn.execute(
                "UPDATE manga SET source_manga_id = NULL WHERE id = ?1",
                [catalog_manga_id],
            )?;
            conn.execute(
                "UPDATE manga SET catalog_manga_id = NULL WHERE id = ?1",
                [detached],
            )?;

            Ok(detached)
        })
        .await?;

    Ok(DetachSourceResponse {
        manga_id: catalog_manga_id,
        detached_source_manga_id: detached_src,
        success: true,
        message: "Successfully detached reading source".to_string(),
    })
}
