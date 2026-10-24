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

/// Resolves a MangaBaka identity and standard digital edition for a Komga series with fail-closed precision.
pub async fn resolve_mangabaka_identity(
    conn: &Connection,
    mb_client: &MangaBakaClient,
    komga_series_id: &str,
    komga_title: &str,
) -> Result<Option<TrackerMapping>> {
    // 1. Check existing mapping table
    if let Some(existing) = get_mapping(conn, komga_series_id, SyncTarget::MangaBaka)? {
        if existing.manual_override {
            return Ok(Some(existing));
        }
        if existing.confidence == MappingConfidence::Resolved {
            return Ok(Some(existing));
        }
    }

    // 2. Query MangaBaka series matching
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
            return Ok(None);
        }
        MangaBakaSeriesMatch::NotFound => return Ok(None),
        MangaBakaSeriesMatch::AuthError(e) => {
            tracing::warn!("MangaBaka auth error: {e}");
            return Ok(None);
        }
        MangaBakaSeriesMatch::NetworkError(e) => {
            tracing::warn!("MangaBaka network error: {e}");
            return Ok(None);
        }
    };

    let (series_id, series_title) = series_match;

    // 3. Query MangaBaka collection matching for the exact English standard digital edition
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

    let (confidence, edition_id) = match collection_match {
        Some(col_id) => (MappingConfidence::Resolved, Some(col_id)),
        None => (MappingConfidence::NeedsMapping, None),
    };

    let mapping = TrackerMapping {
        komga_series_id: komga_series_id.to_string(),
        tracker: SyncTarget::MangaBaka,
        tracker_id: series_id.to_string(),
        tracker_title: Some(series_title),
        edition_id,
        mapping_source: MappingSource::ExactTitle,
        confidence,
        manual_override: false,
        last_validated: Some(now()),
        updated_at: now(),
    };

    save_mapping(conn, &mapping)?;

    if confidence == MappingConfidence::Resolved {
        Ok(Some(mapping))
    } else {
        Ok(None)
    }
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
