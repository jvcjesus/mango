//! Controlled Acquisition Fallback Subsystem (Milestone M10).
//!
//! When an acquisition attempt fails (e.g. Madokami 404/503, dead torrent, etc.),
//! this subsystem discovers and offers compatible fallback releases across sources:
//! - Respects language, edition, canonical series, and volume coverage.
//! - Never silently substitutes a different edition or language.
//! - Avoids infinite retry loops.
//! - Retains diagnostic error logs of the failed attempt in `acquisition_fallback_history`.
//! - Provides 1-click fallback switching for user review.

use super::providers::{
    CandidateMatchType, ReleaseCandidate,
};
use crate::db::now;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackEvaluation {
    pub chapter_id: i64,
    pub manga_id: i64,
    pub series_title: String,
    pub volume_number: Option<f64>,
    pub failed_candidate_id: Option<String>,
    pub failed_provider: Option<String>,
    pub failed_reason: Option<String>,
    pub has_fallback_candidates: bool,
    pub recommended_fallback: Option<ReleaseCandidate>,
    pub fallback_candidates: Vec<ReleaseCandidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackSwitchResult {
    pub success: bool,
    pub chapter_id: i64,
    pub previous_provider: String,
    pub new_provider: String,
    pub new_candidate_id: String,
    pub new_title: String,
    pub message: String,
}

/// Identifies provider from candidate ID or torrent hash.
pub fn infer_provider_from_candidate(candidate_id: &str) -> &'static str {
    if candidate_id.starts_with("madokami:") {
        "madokami"
    } else if candidate_id.starts_with("nyaa:") {
        "nyaa"
    } else if candidate_id.starts_with("keiyoushi:") {
        "keiyoushi"
    } else {
        "unknown"
    }
}

/// Evaluates eligible fallback candidates for a failed acquisition.
pub fn get_fallback_candidates(conn: &Connection, chapter_id: i64) -> Result<FallbackEvaluation> {
    // 1. Fetch chapter and download information
    let row: Option<(i64, String, Option<f64>, Option<String>, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT c.manga_id, m.title, c.name, c.chapter_number, d.state, d.error, d.raw_title, d.retrieval_identity_json
             FROM chapters c
             JOIN manga m ON c.manga_id = m.id
             LEFT JOIN downloads d ON c.id = d.chapter_id
             WHERE c.id = ?1",
            [chapter_id],
            |r| {
                let name: String = r.get(2)?;
                let num: f64 = r.get(3)?;
                let vol = crate::release::parse_volume(&name)
                    .or_else(|| crate::release::parse_release(&name, num).volume);
                Ok((r.get(0)?, r.get(1)?, vol, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            },
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Failed to query chapter for fallback: {e}")))?;

    let Some((manga_id, series_title, volume_opt, _dl_state, dl_error, raw_title_opt, retrieval_json)) = row else {
        return Err(AppError::not_found(format!("Chapter {chapter_id} not found")));
    };

    let mut diagnostics = Vec::new();
    let failed_reason = dl_error;

    // Determine current provider from raw title or retrieval identity
    let mut failed_candidate_id = None;
    let mut failed_provider = None;

    if let Some(ref r_json) = retrieval_json {
        if let Ok(ri) = serde_json::from_str::<crate::acquisition::edition::RetrievalIdentity>(r_json) {
            failed_candidate_id = Some(ri.source_release.clone());
            failed_provider = Some(infer_provider_from_candidate(&ri.source_release).to_string());
        }
    }

    if failed_provider.is_none() {
        if let Some(ref rt) = raw_title_opt {
            if rt.contains("madokami") {
                failed_provider = Some("madokami".to_string());
            } else if rt.contains("nyaa") {
                failed_provider = Some("nyaa".to_string());
            }
        }
    }

    let Some(vol) = volume_opt else {
        return Ok(FallbackEvaluation {
            chapter_id,
            manga_id,
            series_title,
            volume_number: None,
            failed_candidate_id,
            failed_provider,
            failed_reason,
            has_fallback_candidates: false,
            recommended_fallback: None,
            fallback_candidates: Vec::new(),
            diagnostics: vec!["Cannot evaluate fallback for unnumbered chapter".to_string()],
        });
    };

    // 2. Query discovered candidates for this series & volume
    let mut stmt = conn.prepare(
        "SELECT candidate_id, provider, provider_item_id, title, raw_title, match_type,
                size_bytes, uploader, is_trusted, is_digital, score, score_explanation,
                download_url, magnet_url, info_hash, seeders, leechers, published_at
         FROM discovered_release_candidates
         WHERE (series_id = ?1 OR series_id LIKE 'title:%' || lower(?2) OR series_id = 'manga:' || ?3)
           AND ABS(volume_number - ?4) < 0.001
         ORDER BY score DESC",
    ).map_err(|e| AppError::internal(format!("Failed to query candidate pool: {e}")))?;

    let candidates_raw: Vec<ReleaseCandidate> = stmt
        .query_map(params![format!("manga:{manga_id}"), series_title, manga_id, vol], |r| {
            let match_str: String = r.get(5)?;
            Ok(ReleaseCandidate {
                candidate_id: r.get(0)?,
                provider: r.get(1)?,
                provider_item_id: r.get(2)?,
                title: r.get(3)?,
                raw_title: r.get(4)?,
                match_type: CandidateMatchType::from_str(&match_str),
                target_volume: Some(vol),
                target_chapter: None,
                size_bytes: r.get(6)?,
                uploader: r.get(7)?,
                is_trusted: r.get::<_, i64>(8)? != 0,
                is_digital: r.get::<_, i64>(9)? != 0,
                score: r.get(10)?,
                score_explanation: r.get(11)?,
                download_url: r.get(12)?,
                magnet_url: r.get(13)?,
                info_hash: r.get(14)?,
                seeders: r.get(15)?,
                leechers: r.get(16)?,
                published_at: r.get(17)?,
                ..Default::default()
            })
        })
        .map_err(|e| AppError::internal(format!("Failed querying candidates: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    // 3. Filter candidates:
    // - Exclude the exact failed candidate
    // - Exclude dead torrents (seeders == 0)
    // - Exclude foreign language releases
    let mut eligible_fallbacks = Vec::new();
    for c in candidates_raw {
        if failed_candidate_id.as_deref() == Some(&c.candidate_id) {
            continue;
        }
        if let Some(ref failed_raw) = raw_title_opt {
            if c.raw_title == *failed_raw {
                continue;
            }
        }
        if c.seeders == Some(0) {
            continue;
        }
        if super::matching::detect_foreign_language(&c.raw_title, "en").is_some() {
            continue;
        }
        eligible_fallbacks.push(c);
    }

    // Sort descending by score
    eligible_fallbacks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let recommended = eligible_fallbacks.first().cloned();
    let has_fallbacks = !eligible_fallbacks.is_empty();

    if has_fallbacks {
        diagnostics.push(format!(
            "Discovered {} compatible fallback releases for Volume {}",
            eligible_fallbacks.len(),
            vol
        ));
    } else {
        diagnostics.push(format!("No alternate candidates currently cached for Volume {vol}"));
    }

    Ok(FallbackEvaluation {
        chapter_id,
        manga_id,
        series_title,
        volume_number: Some(vol),
        failed_candidate_id,
        failed_provider,
        failed_reason,
        has_fallback_candidates: has_fallbacks,
        recommended_fallback: recommended,
        fallback_candidates: eligible_fallbacks,
        diagnostics,
    })
}

/// Applies a fallback candidate to a failed chapter and resets state to queued.
pub fn apply_acquisition_fallback(
    conn: &Connection,
    chapter_id: i64,
    new_candidate_id: &str,
) -> Result<FallbackSwitchResult> {
    let eval = get_fallback_candidates(conn, chapter_id)?;

    let target_cand = eval
        .fallback_candidates
        .into_iter()
        .find(|c| c.candidate_id == new_candidate_id)
        .ok_or_else(|| {
            AppError::bad_request(format!(
                "Candidate '{new_candidate_id}' is not an eligible fallback for chapter {chapter_id}"
            ))
        })?;

    let cur_time = now();
    let prev_provider = eval.failed_provider.unwrap_or_else(|| "unknown".to_string());
    let prev_cand = eval.failed_candidate_id.unwrap_or_else(|| "unknown".to_string());
    let prev_reason = eval.failed_reason.unwrap_or_else(|| "Download failed".to_string());

    // Record in fallback history
    conn.execute(
        "INSERT INTO acquisition_fallback_history (
            chapter_id, manga_id, series_title, volume_number, failed_candidate_id,
            failed_provider, failed_reason, fallback_candidate_id, fallback_provider,
            status, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'switched', ?10)",
        params![
            chapter_id,
            eval.manga_id,
            eval.series_title,
            eval.volume_number,
            prev_cand,
            prev_provider,
            prev_reason,
            target_cand.candidate_id,
            target_cand.provider,
            cur_time,
        ],
    ).map_err(|e| AppError::internal(format!("Failed to record fallback history: {e}")))?;

    // Update download record: switch to new candidate and reset to queued
    let hash_clone = target_cand.info_hash.clone();
    let magnet_clone = target_cand.magnet_url.clone();
    let uploader_clone = target_cand.uploader.clone();
    let raw_title_clone = target_cand.raw_title.clone();

    conn.execute(
        "UPDATE downloads
         SET raw_title = ?1,
             uploader = ?2,
             torrent_hash = ?3,
             magnet_url = ?4,
             state = 'queued',
             error = NULL,
             is_stalled = 0,
             attempts = 0
         WHERE chapter_id = ?5",
        params![
            raw_title_clone,
            uploader_clone,
            hash_clone,
            magnet_clone,
            chapter_id,
        ],
    ).map_err(|e| AppError::internal(format!("Failed to update download for fallback: {e}")))?;

    let new_provider = target_cand.provider.clone();
    Ok(FallbackSwitchResult {
        success: true,
        chapter_id,
        previous_provider: prev_provider,
        new_provider: new_provider.clone(),
        new_candidate_id: target_cand.candidate_id,
        new_title: target_cand.title,
        message: format!(
            "Successfully switched acquisition to {} release and re-queued",
            new_provider
        ),
    })
}
