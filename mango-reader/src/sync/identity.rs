//! Tracker identity resolution layer with typed evidence, provenance, and fail-closed ambiguity handling.

use super::{
    mapping::{get_mapping, resolve_local_mapping, save_mapping},
    model::{
        MappingConfidence, MappingSource, SyncTarget, TrackerMapping, UnresolvedTitleDiagnostic,
    },
};
use crate::{
    db::now,
    error::Result,
    pipeline::mangabaka::{
        MangaBakaClient, MangaBakaCollectionMatch, MangaBakaSeriesMatch, normalize_title,
    },
    trackers::anilist,
};
use reqwest::Client;
use rusqlite::Connection;

/// Resolves an AniList identity for a Komga series with fail-closed precision.
pub async fn resolve_anilist_identity(
    conn: &Connection,
    http: &Client,
    komga_series_id: &str,
    komga_title: &str,
) -> Result<Option<TrackerMapping>> {
    // 1. Check existing mapping table
    if let Some(existing) = get_mapping(conn, komga_series_id, SyncTarget::AniList)? {
        // Manual overrides are permanent and absolute
        if existing.manual_override {
            return Ok(Some(existing));
        }
        if existing.confidence == MappingConfidence::Resolved {
            return Ok(Some(existing));
        }
    }

    // 2. Check local Mango tracks table
    if let Some(local_mapped) =
        resolve_local_mapping(conn, komga_series_id, komga_title, SyncTarget::AniList)?
    {
        return Ok(Some(local_mapped));
    }

    // 3. Search AniList via public GraphQL search
    let search_results = match anilist::search(http, komga_title).await {
        Ok(res) => res,
        Err(e) => {
            tracing::warn!("Failed to query AniList search for {komga_title}: {e}");
            return Ok(None);
        }
    };

    let norm_target = normalize_title(komga_title);
    let mut exact_matches = Vec::new();
    let mut all_candidates = Vec::new();

    for found in search_results {
        let title = found.title;
        all_candidates.push(format!("{} (id: {})", title, found.media_id));
        if normalize_title(&title) == norm_target {
            exact_matches.push((found.media_id, title));
        }
    }

    if exact_matches.len() == 1 {
        let (media_id, title) = exact_matches.remove(0);
        let mapping = TrackerMapping {
            komga_series_id: komga_series_id.to_string(),
            tracker: SyncTarget::AniList,
            tracker_id: media_id.to_string(),
            tracker_title: Some(title),
            edition_id: None,
            mapping_source: MappingSource::ExactTitle,
            confidence: MappingConfidence::Resolved,
            manual_override: false,
            last_validated: Some(now()),
            updated_at: now(),
        };
        save_mapping(conn, &mapping)?;
        return Ok(Some(mapping));
    }

    if exact_matches.len() > 1 {
        tracing::warn!(
            "AniList search for {komga_title} yielded {} exact matches; failing closed",
            exact_matches.len()
        );
        let ambiguous_mapping = TrackerMapping {
            komga_series_id: komga_series_id.to_string(),
            tracker: SyncTarget::AniList,
            tracker_id: String::new(),
            tracker_title: None,
            edition_id: None,
            mapping_source: MappingSource::ExactTitle,
            confidence: MappingConfidence::Ambiguous,
            manual_override: false,
            last_validated: Some(now()),
            updated_at: now(),
        };
        save_mapping(conn, &ambiguous_mapping)?;
        return Ok(None);
    }

    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityResolutionResult {
    Resolved(TrackerMapping),
    Ambiguous(Vec<String>),
    NotFound,
    ApiError(String),
    RateLimited,
    AuthError(String),
    NetworkError(String),
}

/// Resolves a MangaBaka identity and standard digital edition for a Komga series with fail-closed precision.
pub async fn resolve_mangabaka_identity(
    conn: &Connection,
    mb_client: &MangaBakaClient,
    komga_series_id: &str,
    komga_title: &str,
) -> Result<IdentityResolutionResult> {
    // 1. Check existing mapping table (manual override or resolved mapping)
    let existing_mb = get_mapping(conn, komga_series_id, SyncTarget::MangaBaka)?;
    if let Some(ref existing) = existing_mb {
        if existing.manual_override || existing.confidence == MappingConfidence::Resolved {
            return Ok(IdentityResolutionResult::Resolved(existing.clone()));
        }
    }

    // 2. Known AniList media ID -> MangaBaka cross-reference (`GET /v1/source/anilist/{id}`)
    let anilist_id_opt: Option<i64> = get_mapping(conn, komga_series_id, SyncTarget::AniList)
        .ok()
        .flatten()
        .filter(|m| m.confidence == MappingConfidence::Resolved || m.manual_override)
        .and_then(|m| m.tracker_id.parse::<i64>().ok())
        .or_else(|| {
            resolve_local_mapping(conn, komga_series_id, komga_title, SyncTarget::AniList)
                .ok()
                .flatten()
                .and_then(|m| m.tracker_id.parse::<i64>().ok())
        });

    if let Some(anilist_id) = anilist_id_opt {
        match mb_client.resolve_from_anilist(anilist_id).await {
            Ok(Some(xref)) => {
                let mapping = TrackerMapping {
                    komga_series_id: komga_series_id.to_string(),
                    tracker: SyncTarget::MangaBaka,
                    tracker_id: xref.mangabaka_id.to_string(),
                    tracker_title: xref.title.or_else(|| Some(komga_title.to_string())),
                    edition_id: None,
                    mapping_source: MappingSource::CrossReference,
                    confidence: MappingConfidence::Resolved,
                    manual_override: false,
                    last_validated: Some(now()),
                    updated_at: now(),
                };
                save_mapping(conn, &mapping)?;
                return Ok(IdentityResolutionResult::Resolved(mapping));
            }
            Ok(None) => {
                // Cross-reference returned 404 or empty series: fall through to exact title search
            }
            Err(e) if e.status == 429 || e.code == "mangabaka_rate_limited" => {
                return Ok(IdentityResolutionResult::RateLimited);
            }
            Err(e) if e.status == 401 || e.status == 403 || e.code == "mangabaka_unauthorized" => {
                return Ok(IdentityResolutionResult::AuthError(e.message));
            }
            Err(e) if e.status == 400 || e.code == "mangabaka_bad_request" => {
                return Ok(IdentityResolutionResult::ApiError(e.message));
            }
            Err(e) if e.status == 409 || e.code == "mangabaka_ambiguous" => {
                let ambiguous_mapping = TrackerMapping {
                    komga_series_id: komga_series_id.to_string(),
                    tracker: SyncTarget::MangaBaka,
                    tracker_id: String::new(),
                    tracker_title: None,
                    edition_id: None,
                    mapping_source: MappingSource::CrossReference,
                    confidence: MappingConfidence::Ambiguous,
                    manual_override: false,
                    last_validated: Some(now()),
                    updated_at: now(),
                };
                save_mapping(conn, &ambiguous_mapping)?;
                return Ok(IdentityResolutionResult::Ambiguous(vec![]));
            }
            Err(e) => {
                return Ok(IdentityResolutionResult::NetworkError(e.message));
            }
        }
    }

    // If existing mapping was already recorded as ambiguous and no cross-reference resolved it:
    if let Some(existing) = existing_mb {
        if existing.confidence == MappingConfidence::Ambiguous {
            return Ok(IdentityResolutionResult::Ambiguous(vec![]));
        }
    }

    // 3. Query MangaBaka series matching by exact title
    let series_match = match mb_client.match_series(komga_title).await? {
        MangaBakaSeriesMatch::Found { id, title } => (id, title),
        MangaBakaSeriesMatch::Ambiguous(candidates) => {
            tracing::warn!(
                "MangaBaka series search for {komga_title} ambiguous (candidates: {candidates:?})"
            );
            let ambiguous_mapping = TrackerMapping {
                komga_series_id: komga_series_id.to_string(),
                tracker: SyncTarget::MangaBaka,
                tracker_id: String::new(),
                tracker_title: None,
                edition_id: None,
                mapping_source: MappingSource::ExactTitle,
                confidence: MappingConfidence::Ambiguous,
                manual_override: false,
                last_validated: Some(now()),
                updated_at: now(),
            };
            save_mapping(conn, &ambiguous_mapping)?;
            return Ok(IdentityResolutionResult::Ambiguous(candidates));
        }
        MangaBakaSeriesMatch::NotFound => return Ok(IdentityResolutionResult::NotFound),
        MangaBakaSeriesMatch::ApiError(e) => {
            tracing::warn!("MangaBaka API error for {komga_title}: {e}");
            return Ok(IdentityResolutionResult::ApiError(e));
        }
        MangaBakaSeriesMatch::RateLimited => {
            tracing::warn!("MangaBaka rate limited during search for {komga_title}");
            return Ok(IdentityResolutionResult::RateLimited);
        }
        MangaBakaSeriesMatch::AuthError(e) => {
            tracing::warn!("MangaBaka auth error: {e}");
            return Ok(IdentityResolutionResult::AuthError(e));
        }
        MangaBakaSeriesMatch::NetworkError(e) => {
            tracing::warn!("MangaBaka network error: {e}");
            return Ok(IdentityResolutionResult::NetworkError(e));
        }
    };

    let (series_id, series_title) = series_match;

    // 4. Query MangaBaka collection matching for the exact English standard digital edition (best effort)
    let collection_match = match mb_client.match_collection(series_id).await? {
        MangaBakaCollectionMatch::Found {
            id,
            edition: _,
            total_works: _,
        } => Some(id),
        MangaBakaCollectionMatch::Ambiguous(candidates) => {
            tracing::warn!("MangaBaka collection for {komga_title} ambiguous: {candidates:?}");
            None
        }
        MangaBakaCollectionMatch::NotFound => None,
        MangaBakaCollectionMatch::AuthError(_) | MangaBakaCollectionMatch::NetworkError(_) => None,
    };

    let mapping = TrackerMapping {
        komga_series_id: komga_series_id.to_string(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: series_id.to_string(),
        tracker_title: Some(series_title),
        edition_id: collection_match,
        mapping_source: MappingSource::ExactTitle,
        confidence: MappingConfidence::Resolved,
        manual_override: false,
        last_validated: Some(now()),
        updated_at: now(),
    };

    save_mapping(conn, &mapping)?;
    Ok(IdentityResolutionResult::Resolved(mapping))
}

/// Diagnoses unresolved or ambiguous titles.
pub fn get_title_diagnostic(
    mapping: Option<&TrackerMapping>,
    komga_series_id: &str,
    komga_title: &str,
    tracker: SyncTarget,
) -> Option<UnresolvedTitleDiagnostic> {
    match mapping {
        Some(m) if m.confidence == MappingConfidence::Resolved => None,
        Some(m) if m.confidence == MappingConfidence::Ambiguous => Some(UnresolvedTitleDiagnostic {
            komga_series_id: komga_series_id.to_string(),
            komga_title: komga_title.to_string(),
            tracker,
            reason: "Multiple candidates matched; auto-resolution aborted to prevent incorrect synchronization".to_string(),
            candidates: vec![],
            confidence: MappingConfidence::Ambiguous,
            source: m.mapping_source,
        }),
        Some(m) if m.confidence == MappingConfidence::NeedsMapping => Some(UnresolvedTitleDiagnostic {
            komga_series_id: komga_series_id.to_string(),
            komga_title: komga_title.to_string(),
            tracker,
            reason: "Series found but requires specific edition selection".to_string(),
            candidates: vec![],
            confidence: MappingConfidence::NeedsMapping,
            source: m.mapping_source,
        }),
        _ => Some(UnresolvedTitleDiagnostic {
            komga_series_id: komga_series_id.to_string(),
            komga_title: komga_title.to_string(),
            tracker,
            reason: "No matching title or tracker ID discovered".to_string(),
            candidates: vec![],
            confidence: MappingConfidence::NotFound,
            source: MappingSource::Unknown,
        }),
    }
}
