//! Source-agnostic release discovery layer for logical missing volumes.
//!
//! Evaluates logical missing volumes identified by MangaBaka/Komga reconciliation against
//! registered release discovery providers (Nyaa, Madokami), normalizes and ranks candidates,
//! and persists discovery status and candidate pools without ever triggering downloads or
//! mutating media.

use super::providers::{
    CandidateMatchType, MadokamiReleaseProvider, NyaaReleaseProvider, ReleaseCandidate,
    ReleaseProvider, ReleaseQuery,
};
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::sync::reconciliation::{get_reconciliation, record_event_full, ReconciliationStatus};
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of release discovery for a single logical volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeDiscoveryResult {
    pub series_id: String,
    pub volume_number: f64,
    /// One of: "found", "no_candidates", "owned", "acquiring", "unreleased", "review_required"
    pub status: String,
    pub candidate_count: usize,
    pub top_candidate: Option<ReleaseCandidate>,
    pub candidates: Vec<ReleaseCandidate>,
    pub providers_checked: Vec<String>,
    pub provider_errors: HashMap<String, String>,
    pub last_discovered_at: i64,
}

/// Summary of discovery run across all missing volumes for a series.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesDiscoverySummary {
    pub series_id: String,
    pub missing_volumes_count: usize,
    pub discovered_volumes_count: usize,
    pub results: Vec<VolumeDiscoveryResult>,
}

/// Retrieves persisted discovery status and candidates for a logical volume.
pub fn get_volume_discovery(
    conn: &Connection,
    series_id: &str,
    volume_number: f64,
) -> Result<Option<VolumeDiscoveryResult>> {
    let status_row: Option<(i64, String, usize, String, Option<String>)> = conn
        .query_row(
            "SELECT last_discovered_at, status, candidate_count, providers_checked, provider_errors
             FROM volume_discovery_status
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
            params![series_id, volume_number],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Failed to query volume discovery status: {e}")))?;

    let Some((last_discovered_at, status, candidate_count, providers_json, errors_json)) =
        status_row
    else {
        return Ok(None);
    };

    let providers_checked: Vec<String> = serde_json::from_str(&providers_json).unwrap_or_default();
    let provider_errors: HashMap<String, String> =
        errors_json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();

    let mut stmt = conn
        .prepare(
            "SELECT candidate_id, provider, provider_item_id, title, raw_title, match_type,
                    size_bytes, uploader, is_trusted, is_digital, score, score_explanation,
                    download_url, magnet_url, info_hash, seeders, leechers, published_at
             FROM discovered_release_candidates
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001
             ORDER BY score DESC",
        )
        .map_err(|e| AppError::internal(format!("Failed to prepare candidates query: {e}")))?;

    let candidates: Vec<ReleaseCandidate> = stmt
        .query_map(params![series_id, volume_number], |r| {
            let match_str: String = r.get(5)?;
            Ok(ReleaseCandidate {
                candidate_id: r.get(0)?,
                provider: r.get(1)?,
                provider_item_id: r.get(2)?,
                title: r.get(3)?,
                raw_title: r.get(4)?,
                match_type: CandidateMatchType::from_str(&match_str),
                target_volume: Some(volume_number),
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
            })
        })
        .map_err(|e| AppError::internal(format!("Failed to query candidates: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    let top_candidate = candidates.first().cloned();

    Ok(Some(VolumeDiscoveryResult {
        series_id: series_id.to_string(),
        volume_number,
        status,
        candidate_count,
        top_candidate,
        candidates,
        providers_checked,
        provider_errors,
        last_discovered_at,
    }))
}

/// Retrieves all persisted volume discovery results for a series.
pub fn list_series_discovery(
    conn: &Connection,
    series_id: &str,
) -> Result<Vec<VolumeDiscoveryResult>> {
    let mut stmt = conn
        .prepare(
            "SELECT volume_number FROM volume_discovery_status
             WHERE series_id = ?1
             ORDER BY volume_number ASC",
        )
        .map_err(|e| AppError::internal(format!("Failed to prepare status list query: {e}")))?;

    let vol_numbers: Vec<f64> = stmt
        .query_map([series_id], |r| r.get(0))
        .map_err(|e| AppError::internal(format!("Failed to query volume numbers: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    let mut results = Vec::new();
    for v in vol_numbers {
        if let Some(res) = get_volume_discovery(conn, series_id, v)? {
            results.push(res);
        }
    }
    Ok(results)
}

/// Checks whether an active download exists for this series and volume.
fn is_volume_actively_acquiring(
    conn: &Connection,
    series_id: &str,
    title: &str,
    volume_number: f64,
) -> Result<bool> {
    let manga_id: Option<i64> = if let Some(stripped) = series_id.strip_prefix("manga:") {
        stripped.parse().ok()
    } else {
        None
    };

    let sql = "SELECT c.name, d.raw_title FROM downloads d
               JOIN chapters c ON d.chapter_id = c.id
               JOIN manga m ON c.manga_id = m.id
               WHERE (m.title = ?1 OR (?2 IS NOT NULL AND m.id = ?2))
                 AND d.state != 'completed'";

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };

    let rows = stmt
        .query_map(params![title, manga_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| AppError::internal(format!("Failed to query downloads: {e}")))?;

    for (c_name, raw_title) in rows.flatten() {
        if let Some(v) = crate::release::parse_volume(&c_name) {
            if (v - volume_number).abs() < 0.001 {
                return Ok(true);
            }
        }
        if let Some(raw) = raw_title {
            if let Some(v) = crate::release::parse_volume(&raw) {
                if (v - volume_number).abs() < 0.001 {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// Discovers release candidates for a specific logical volume without mutating media or enqueuing jobs.
pub async fn discover_for_volume(
    db: &Db,
    http: Client,
    series_id: &str,
    volume_number: f64,
) -> Result<VolumeDiscoveryResult> {
    let s_id = series_id.to_string();

    // 1. Fetch reconciliation summary to enforce pre-flight invariants
    let reconciliation = db
        .call(move |conn| get_reconciliation(conn, &s_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Series reconciliation not found for {series_id}")))?;

    let series_title = reconciliation.title.clone();
    let current_time = now();

    // Pre-flight check A: Review required
    if reconciliation.status == ReconciliationStatus::ReviewRequired {
        return Ok(VolumeDiscoveryResult {
            series_id: series_id.to_string(),
            volume_number,
            status: "review_required".to_string(),
            candidate_count: 0,
            top_candidate: None,
            candidates: Vec::new(),
            providers_checked: Vec::new(),
            provider_errors: HashMap::new(),
            last_discovered_at: current_time,
        });
    }

    // Pre-flight check B: Already owned in Komga
    if reconciliation
        .owned_volumes
        .iter()
        .any(|&ov| (ov - volume_number).abs() < 0.001)
    {
        return Ok(VolumeDiscoveryResult {
            series_id: series_id.to_string(),
            volume_number,
            status: "owned".to_string(),
            candidate_count: 0,
            top_candidate: None,
            candidates: Vec::new(),
            providers_checked: Vec::new(),
            provider_errors: HashMap::new(),
            last_discovered_at: current_time,
        });
    }

    // Pre-flight check C: Future unreleased volume
    if reconciliation
        .future_volumes
        .iter()
        .any(|&fv| (fv - volume_number).abs() < 0.001)
    {
        return Ok(VolumeDiscoveryResult {
            series_id: series_id.to_string(),
            volume_number,
            status: "unreleased".to_string(),
            candidate_count: 0,
            top_candidate: None,
            candidates: Vec::new(),
            providers_checked: Vec::new(),
            provider_errors: HashMap::new(),
            last_discovered_at: current_time,
        });
    }

    // Pre-flight check D: Actively acquiring in downloads queue
    let s_id_c = series_id.to_string();
    let title_c = series_title.clone();
    let is_acquiring = db
        .call(move |conn| is_volume_actively_acquiring(conn, &s_id_c, &title_c, volume_number))
        .await?;
    if is_acquiring {
        return Ok(VolumeDiscoveryResult {
            series_id: series_id.to_string(),
            volume_number,
            status: "acquiring".to_string(),
            candidate_count: 0,
            top_candidate: None,
            candidates: Vec::new(),
            providers_checked: Vec::new(),
            provider_errors: HashMap::new(),
            last_discovered_at: current_time,
        });
    }

    // 2. Query configured providers
    let (nyaa_enabled, nyaa_url, nyaa_cat) = db
        .call(|conn| {
            let enabled = crate::db::state(conn, "acquisition.nyaa_enabled")?
                .map(|s| s.parse::<bool>().unwrap_or(true))
                .unwrap_or(true);
            let url = crate::db::state(conn, "acquisition.nyaa_url")?;
            let cat = crate::db::state(conn, "acquisition.nyaa_category")?;
            Ok((enabled, url, cat))
        })
        .await?;

    let nyaa_provider = NyaaReleaseProvider::new(http.clone(), nyaa_url, nyaa_cat, nyaa_enabled);
    let madokami_provider = MadokamiReleaseProvider::new(http.clone(), None);

    let query = ReleaseQuery {
        series_title: series_title.clone(),
        target_volume: Some(volume_number),
        target_chapter: None,
        preferred_edition: Some(reconciliation.edition.clone()),
    };

    let mut candidates = Vec::new();
    let mut providers_checked = Vec::new();
    let mut provider_errors = HashMap::new();

    // Query Nyaa
    providers_checked.push("nyaa".to_string());
    match nyaa_provider.search_releases(&query).await {
        Ok(res) => {
            candidates.extend(res);
        }
        Err(e) => {
            provider_errors.insert("nyaa".to_string(), e.to_string());
        }
    }

    // Query Madokami seam
    providers_checked.push("madokami".to_string());
    let madokami_health = madokami_provider.check_health().await;
    if let Some(msg) = madokami_health.message {
        provider_errors.insert("madokami".to_string(), msg);
    }
    match madokami_provider.search_releases(&query).await {
        Ok(res) => {
            candidates.extend(res);
        }
        Err(e) => {
            provider_errors.insert("madokami".to_string(), e.to_string());
        }
    }

    // Deduplicate candidates across providers (e.g. by candidate_id or info_hash)
    let mut deduped: Vec<ReleaseCandidate> = Vec::new();
    let mut seen_keys = std::collections::HashSet::new();
    for c in candidates {
        let key = c
            .info_hash
            .clone()
            .unwrap_or_else(|| c.candidate_id.clone());
        if seen_keys.insert(key) {
            deduped.push(c);
        }
    }

    super::providers::score_and_rank_candidates(&mut deduped, Some(volume_number));

    let candidate_count = deduped.len();
    let top_candidate = deduped.first().cloned();
    let status = if candidate_count > 0 {
        "found".to_string()
    } else {
        "no_candidates".to_string()
    };

    let result = VolumeDiscoveryResult {
        series_id: series_id.to_string(),
        volume_number,
        status: status.clone(),
        candidate_count,
        top_candidate: top_candidate.clone(),
        candidates: deduped.clone(),
        providers_checked: providers_checked.clone(),
        provider_errors: provider_errors.clone(),
        last_discovered_at: current_time,
    };

    // 3. Persist discovery results and log event if candidates found
    let res_c = result.clone();
    let s_id_save = series_id.to_string();
    let edition_c = reconciliation.edition.clone();
    let mb_id = reconciliation.mangabaka_id;

    db.call(move |conn| {
        // Delete previous candidates for this volume
        conn.execute(
            "DELETE FROM discovered_release_candidates
             WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001",
            params![s_id_save, volume_number],
        )?;

        // Insert fresh candidates
        for c in &res_c.candidates {
            conn.execute(
                "INSERT INTO discovered_release_candidates (
                    series_id, volume_number, candidate_id, provider, provider_item_id,
                    title, raw_title, match_type, size_bytes, uploader, is_trusted,
                    is_digital, score, score_explanation, download_url, magnet_url,
                    info_hash, seeders, leechers, published_at, discovered_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
                ON CONFLICT(series_id, volume_number, candidate_id) DO UPDATE SET
                    score = excluded.score,
                    score_explanation = excluded.score_explanation,
                    seeders = excluded.seeders,
                    leechers = excluded.leechers,
                    discovered_at = excluded.discovered_at",
                params![
                    s_id_save,
                    volume_number,
                    c.candidate_id,
                    c.provider,
                    c.provider_item_id,
                    c.title,
                    c.raw_title,
                    c.match_type.as_str(),
                    c.size_bytes.map(|s| s as i64),
                    c.uploader,
                    if c.is_trusted { 1 } else { 0 },
                    if c.is_digital { 1 } else { 0 },
                    c.score,
                    c.score_explanation,
                    c.download_url,
                    c.magnet_url,
                    c.info_hash,
                    c.seeders.map(|s| s as i64),
                    c.leechers.map(|s| s as i64),
                    c.published_at,
                    current_time,
                ],
            )?;
        }

        let providers_json = serde_json::to_string(&res_c.providers_checked).unwrap_or_else(|_| "[]".to_string());
        let errors_json = serde_json::to_string(&res_c.provider_errors).unwrap_or_else(|_| "{}".to_string());

        // Upsert volume discovery status
        conn.execute(
            "INSERT INTO volume_discovery_status (
                series_id, volume_number, last_discovered_at, status, candidate_count,
                top_candidate_id, top_candidate_title, top_candidate_provider,
                top_candidate_score, providers_checked, provider_errors
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(series_id, volume_number) DO UPDATE SET
                last_discovered_at = excluded.last_discovered_at,
                status = excluded.status,
                candidate_count = excluded.candidate_count,
                top_candidate_id = excluded.top_candidate_id,
                top_candidate_title = excluded.top_candidate_title,
                top_candidate_provider = excluded.top_candidate_provider,
                top_candidate_score = excluded.top_candidate_score,
                providers_checked = excluded.providers_checked,
                provider_errors = excluded.provider_errors",
            params![
                s_id_save,
                volume_number,
                current_time,
                res_c.status,
                res_c.candidate_count as i64,
                res_c.top_candidate.as_ref().map(|c| c.candidate_id.as_str()),
                res_c.top_candidate.as_ref().map(|c| c.title.as_str()),
                res_c.top_candidate.as_ref().map(|c| c.provider.as_str()),
                res_c.top_candidate.as_ref().map(|c| c.score),
                providers_json,
                errors_json,
            ],
        )?;

        // If candidates were discovered, record a deduplicated event in reconciliation_events
        if let Some(top) = &res_c.top_candidate {
            record_event_full(
                conn,
                &s_id_save,
                &series_title,
                mb_id,
                None,
                &edition_c,
                Some(volume_number),
                "RELEASE_AVAILABLE",
                Some(&top.candidate_id),
                res_c.candidate_count as i64,
                &format!(
                    "Found {} release candidate(s) for {} Vol. {} (Top: {} via {})",
                    res_c.candidate_count, series_title, volume_number, top.title, top.provider
                ),
            )?;
        }

        Ok(())
    })
    .await?;

    Ok(result)
}

/// Runs release discovery for all logical missing volumes in a reconciled series.
pub async fn discover_missing_for_series(
    db: &Db,
    http: Client,
    series_id: &str,
) -> Result<SeriesDiscoverySummary> {
    let s_id = series_id.to_string();
    let reconciliation = db
        .call(move |conn| get_reconciliation(conn, &s_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Series reconciliation not found for {series_id}")))?;

    let missing_volumes = reconciliation.missing_volumes;
    let missing_volumes_count = missing_volumes.len();

    let mut results = Vec::new();
    let mut discovered_volumes_count = 0;

    for &vol in &missing_volumes {
        let res = discover_for_volume(db, http.clone(), series_id, vol).await?;
        if res.candidate_count > 0 {
            discovered_volumes_count += 1;
        }
        results.push(res);
    }

    Ok(SeriesDiscoverySummary {
        series_id: series_id.to_string(),
        missing_volumes_count,
        discovered_volumes_count,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquisition::providers::{
        classify_candidate_release, score_and_rank_candidates, score_candidate, CandidateMatchType,
    };
    use crate::sync::reconciliation::{save_reconciliation, ReconciliationStatus, ReconciliationSummary};

    fn setup_test_db() -> Db {
        Db::open(std::path::Path::new(":memory:")).unwrap()
    }

    #[test]
    fn test_classification_exact_volume() {
        let raw = "[danke-Empire] Chainsaw Man - Vol. 10 [Digital]";
        let (match_type, vol, is_digital) =
            classify_candidate_release("Chainsaw Man - Vol. 10", raw, Some(10.0), None);
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(10.0));
        assert!(is_digital);
    }

    #[test]
    fn test_classification_volume_range() {
        let raw = "[LuScHiOUs] Chainsaw Man v08-v12 [Digital]";
        let (match_type, _, is_digital) =
            classify_candidate_release("Chainsaw Man v08-v12", raw, Some(10.0), None);
        assert_eq!(match_type, CandidateMatchType::VolumeRangeContainingTarget);
        assert!(is_digital);
    }

    #[test]
    fn test_classification_series_pack() {
        let raw = "[Group] Akane-banashi v01-v20 [Complete]";
        let (match_type, _, _) =
            classify_candidate_release("Akane-banashi v01-v20", raw, Some(5.0), None);
        assert_eq!(match_type, CandidateMatchType::SeriesPack);
    }

    #[test]
    fn test_classification_rejects_anime() {
        let raw = "[Erai-raws] Chainsaw Man - 10 [1080p][HEVC][AAC].mkv";
        let (match_type, _, _) =
            classify_candidate_release("Chainsaw Man - 10", raw, Some(10.0), None);
        assert_eq!(match_type, CandidateMatchType::Rejected);
    }

    #[test]
    fn test_classification_rejects_single_chapter() {
        let raw = "[TCB] Chainsaw Man - Chapter 10 [Digital]";
        let (match_type, _, _) =
            classify_candidate_release("Chainsaw Man - Chapter 10", raw, Some(10.0), None);
        assert_eq!(match_type, CandidateMatchType::Rejected);
    }

    #[test]
    fn test_classification_rejects_mismatched_volume() {
        let raw = "[danke-Empire] Chainsaw Man - Vol. 09 [Digital]";
        let (match_type, _, _) =
            classify_candidate_release("Chainsaw Man - Vol. 09", raw, Some(10.0), None);
        assert_eq!(match_type, CandidateMatchType::Rejected);
    }

    #[test]
    fn test_classification_possible_fuzzy_match() {
        let raw = "Chainsaw Man Volume 10";
        let (match_type, vol, _) =
            classify_candidate_release("Chainsaw Man Volume 10", raw, Some(10.0), None);
        // classify_release parses Volume 10 as exact volume or possible match
        assert!(
            match_type == CandidateMatchType::ExactVolume || match_type == CandidateMatchType::PossibleMatch
        );
        assert_eq!(vol, Some(10.0));
    }

    #[test]
    fn test_scoring_ranking_order_and_explanations() {
        let mut candidate_exact = ReleaseCandidate {
            candidate_id: "c1".to_string(),
            provider: "nyaa".to_string(),
            provider_item_id: "1".to_string(),
            title: "Manga Vol. 10".to_string(),
            raw_title: "[danke-Empire] Manga Vol. 10 [Digital]".to_string(),
            match_type: CandidateMatchType::ExactVolume,
            target_volume: Some(10.0),
            target_chapter: None,
            size_bytes: Some(100_000_000),
            uploader: Some("danke-Empire".to_string()),
            is_trusted: true,
            is_digital: true,
            score: 0.0,
            score_explanation: String::new(),
            download_url: None,
            magnet_url: None,
            info_hash: Some("hash1".to_string()),
            seeders: Some(25),
            leechers: Some(2),
            published_at: Some(1000),
        };

        let mut candidate_range = ReleaseCandidate {
            candidate_id: "c2".to_string(),
            provider: "nyaa".to_string(),
            provider_item_id: "2".to_string(),
            title: "Manga v08-v12".to_string(),
            raw_title: "Manga v08-v12".to_string(),
            match_type: CandidateMatchType::VolumeRangeContainingTarget,
            target_volume: Some(10.0),
            target_chapter: None,
            size_bytes: Some(500_000_000),
            uploader: None,
            is_trusted: false,
            is_digital: false,
            score: 0.0,
            score_explanation: String::new(),
            download_url: None,
            magnet_url: None,
            info_hash: Some("hash2".to_string()),
            seeders: Some(3),
            leechers: Some(1),
            published_at: Some(900),
        };

        score_candidate(&mut candidate_exact, Some(10.0));
        score_candidate(&mut candidate_range, Some(10.0));

        // Exact: 60 (base) + 15 (digital) + 10 (trusted) + 5 (quality group) + 10 (>=20 seeds) = 100
        assert_eq!(candidate_exact.score, 100.0);
        assert!(candidate_exact.score_explanation.contains("Exact single volume match (+60)"));
        assert!(candidate_exact.score_explanation.contains("Digital edition (+15)"));
        assert!(candidate_exact.score_explanation.contains("Trusted uploader (+10)"));
        assert!(candidate_exact.score_explanation.contains("Recognized quality group (+5)"));

        // Range: 35 (base) + 2 (1-4 seeds) = 37
        assert_eq!(candidate_range.score, 37.0);

        let mut list = vec![candidate_range.clone(), candidate_exact.clone()];
        score_and_rank_candidates(&mut list, Some(10.0));
        assert_eq!(list[0].candidate_id, "c1");
        assert_eq!(list[1].candidate_id, "c2");
    }

    #[test]
    fn test_scoring_quality_group_and_seeders_penalties() {
        let mut dead_candidate = ReleaseCandidate {
            candidate_id: "dead".to_string(),
            provider: "nyaa".to_string(),
            provider_item_id: "dead".to_string(),
            title: "Manga Vol. 10".to_string(),
            raw_title: "Manga Vol. 10".to_string(),
            match_type: CandidateMatchType::ExactVolume,
            target_volume: Some(10.0),
            target_chapter: None,
            size_bytes: Some(100_000_000),
            uploader: None,
            is_trusted: false,
            is_digital: false,
            score: 0.0,
            score_explanation: String::new(),
            download_url: None,
            magnet_url: None,
            info_hash: Some("deadhash".to_string()),
            seeders: Some(0),
            leechers: Some(0),
            published_at: Some(500),
        };

        score_candidate(&mut dead_candidate, Some(10.0));
        // Base 60 - 15 (dead torrent) = 45
        assert_eq!(dead_candidate.score, 45.0);
        assert!(dead_candidate.score_explanation.contains("Dead torrent: 0 seeders (-15)"));
    }

    #[tokio::test]
    async fn test_preflight_owned_volume_skips_discovery() {
        let db = setup_test_db();
        let current_time = now();

        let rec = ReconciliationSummary {
            series_id: "komga:k10".to_string(),
            manga_id: None,
            komga_series_id: Some("k10".to_string()),
            mangabaka_id: Some(100),
            collection_id: Some("c100".to_string()),
            title: "Blood on the Tracks".to_string(),
            edition: "standard".to_string(),
            status: ReconciliationStatus::MissingVolume,
            is_ongoing: true,
            total_official_volumes: Some(17),
            owned_volumes_count: 16,
            missing_volumes_count: 1,
            owned_volumes: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0],
            missing_volumes: vec![17.0],
            future_volumes: vec![],
            last_reconciled_at: current_time,
            last_error: None,
            review_reason: None,
            display_status: "16 / 17 owned".to_string(),
        };

        db.call(move |conn| save_reconciliation(conn, &rec)).await.unwrap();

        let http = Client::new();
        // Volume 5 is owned: must skip discovery and return "owned"
        let res = discover_for_volume(&db, http, "komga:k10", 5.0).await.unwrap();
        assert_eq!(res.status, "owned");
        assert_eq!(res.candidate_count, 0);
        assert!(res.candidates.is_empty());
        assert!(res.providers_checked.is_empty());
    }

    #[tokio::test]
    async fn test_preflight_future_unreleased_volume_skips_discovery() {
        let db = setup_test_db();
        let current_time = now();

        let rec = ReconciliationSummary {
            series_id: "komga:k20".to_string(),
            manga_id: None,
            komga_series_id: Some("k20".to_string()),
            mangabaka_id: Some(200),
            collection_id: Some("c200".to_string()),
            title: "Future Manga".to_string(),
            edition: "standard".to_string(),
            status: ReconciliationStatus::UpToDate,
            is_ongoing: true,
            total_official_volumes: Some(5),
            owned_volumes_count: 4,
            missing_volumes_count: 0,
            owned_volumes: vec![1.0, 2.0, 3.0, 4.0],
            missing_volumes: vec![],
            future_volumes: vec![5.0],
            last_reconciled_at: current_time,
            last_error: None,
            review_reason: None,
            display_status: "4 owned, 1 unreleased".to_string(),
        };

        db.call(move |conn| save_reconciliation(conn, &rec)).await.unwrap();

        let http = Client::new();
        // Volume 5 is in future_volumes: must skip discovery and return "unreleased"
        let res = discover_for_volume(&db, http, "komga:k20", 5.0).await.unwrap();
        assert_eq!(res.status, "unreleased");
        assert_eq!(res.candidate_count, 0);
        assert!(res.candidates.is_empty());
        assert!(res.providers_checked.is_empty());
    }

    #[tokio::test]
    async fn test_preflight_review_required_skips_discovery() {
        let db = setup_test_db();
        let current_time = now();

        let rec = ReconciliationSummary {
            series_id: "komga:k30".to_string(),
            manga_id: None,
            komga_series_id: Some("k30".to_string()),
            mangabaka_id: Some(300),
            collection_id: None,
            title: "Ambiguous Manga".to_string(),
            edition: "standard".to_string(),
            status: ReconciliationStatus::ReviewRequired,
            is_ongoing: true,
            total_official_volumes: None,
            owned_volumes_count: 0,
            missing_volumes_count: 0,
            owned_volumes: vec![],
            missing_volumes: vec![],
            future_volumes: vec![],
            last_reconciled_at: current_time,
            last_error: None,
            review_reason: Some("Multiple candidate series matched on MangaBaka".to_string()),
            display_status: "Review required".to_string(),
        };

        db.call(move |conn| save_reconciliation(conn, &rec)).await.unwrap();

        let http = Client::new();
        let res = discover_for_volume(&db, http, "komga:k30", 1.0).await.unwrap();
        assert_eq!(res.status, "review_required");
        assert_eq!(res.candidate_count, 0);
        assert!(res.candidates.is_empty());
        assert!(res.providers_checked.is_empty());
    }

    #[tokio::test]
    async fn test_madokami_unavailable_does_not_break_discovery() {
        let db = setup_test_db();
        let current_time = now();

        // Disable Nyaa in settings so only Madokami is checked
        db.call(|conn| {
            crate::db::set_state(conn, "acquisition.nyaa_enabled", "false")
        }).await.unwrap();

        let rec = ReconciliationSummary {
            series_id: "komga:k40".to_string(),
            manga_id: None,
            komga_series_id: Some("k40".to_string()),
            mangabaka_id: Some(400),
            collection_id: Some("c400".to_string()),
            title: "Test Manga".to_string(),
            edition: "standard".to_string(),
            status: ReconciliationStatus::MissingVolume,
            is_ongoing: true,
            total_official_volumes: Some(2),
            owned_volumes_count: 1,
            missing_volumes_count: 1,
            owned_volumes: vec![1.0],
            missing_volumes: vec![2.0],
            future_volumes: vec![],
            last_reconciled_at: current_time,
            last_error: None,
            review_reason: None,
            display_status: "1 owned, 1 missing".to_string(),
        };

        db.call(move |conn| save_reconciliation(conn, &rec)).await.unwrap();

        let http = Client::new();
        let res = discover_for_volume(&db, http, "komga:k40", 2.0).await.unwrap();

        // Global discovery did not fail, Madokami status recorded in provider_errors
        assert_eq!(res.status, "no_candidates");
        assert_eq!(res.candidate_count, 0);
        assert!(res.provider_errors.contains_key("madokami"));
        assert!(res.provider_errors["madokami"].contains("Madokami is currently offline"));
    }
}
