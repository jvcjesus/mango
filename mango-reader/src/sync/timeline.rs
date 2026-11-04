//! Unified Series Timeline and Gap Detection Subsystem (Milestone M6.12)
//!
//! Provides a unified, backend-derived chronological and volume-ordered sequence
//! for any manga title, synthesizing:
//! - MangaBaka official logical volumes
//! - OpenTome publisher release schedules & dates
//! - Komga canonical physical library ownership
//! - Mango local download staging
//! - Reading extension chapter cache
//! - Discovered release candidates & quality rankings
//! - Active acquisition / backlog jobs
//! - Durable publication receipts
//! - Volume gap classification (Interior, Leading, Trailing)
//! - Historical reconciliation & notification events

use std::collections::{HashMap, HashSet};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// A single volume's unified timeline state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesTimelineVolumeDto {
    pub volume_number: f64,
    pub display_title: String,
    pub status: String,
    pub is_owned: bool,
    pub is_reader_cached: bool,
    pub is_canonical_file: bool,
    pub is_gap: bool,
    pub gap_type: Option<String>,
    pub release_date: Option<String>,
    pub release_status: Option<String>,
    pub candidate_count: usize,
    pub top_candidate_title: Option<String>,
    pub top_candidate_source: Option<String>,
    pub top_candidate_score: Option<f64>,
    pub top_candidate_auto_eligible: Option<bool>,
    pub receipt_id: Option<String>,
    pub canonical_path: Option<String>,
    pub download_job_id: Option<String>,
}

/// Comprehensive analysis of missing volume gaps across the series.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesGapAnalysisDto {
    pub total_volumes: usize,
    pub owned_count: usize,
    pub missing_count: usize,
    pub interior_gap_count: usize,
    pub leading_gap_count: usize,
    pub trailing_gap_count: usize,
    pub has_interior_gaps: bool,
    pub gap_volume_numbers: Vec<f64>,
    pub summary_text: String,
}

/// Strict separation between canonical Komga physical files, local download staging, and reader caches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingSourceSeparationDto {
    pub canonical_komga_count: usize,
    pub canonical_volumes: Vec<f64>,
    pub local_staging_count: usize,
    pub local_staging_volumes: Vec<f64>,
    pub reader_chapter_count: usize,
    pub reader_chapters_latest: Option<f64>,
    pub has_duplicate_storage: bool,
    pub storage_distinction_notes: Vec<String>,
}

/// Chronological notification or reconciliation event for the series timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesTimelineEventDto {
    pub id: i64,
    pub event_type: String,
    pub message: String,
    pub volume_number: Option<f64>,
    pub severity: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

/// Unified series timeline payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesTimelineDto {
    pub manga_id: i64,
    pub title: String,
    pub source_id: i64,
    pub mangabaka_id: Option<i64>,
    pub collection_id: Option<String>,
    pub collection_market: Option<String>,
    pub collection_edition: Option<String>,
    pub reconciliation_status: String,
    pub auto_acquire_enabled: bool,
    pub volumes: Vec<SeriesTimelineVolumeDto>,
    pub gaps: SeriesGapAnalysisDto,
    pub reading_source_separation: ReadingSourceSeparationDto,
    pub events: Vec<SeriesTimelineEventDto>,
}

/// Builds the unified series timeline for a given manga title.
pub fn get_series_timeline(conn: &Connection, manga_id: i64) -> Result<SeriesTimelineDto> {
    // 1. Fetch title record
    let manga_row = conn
        .query_row(
            "SELECT id, title, source_id, source_manga_id, catalog_manga_id, mangabaka_id, auto_acquire
             FROM manga WHERE id = ?1",
            [manga_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                ))
            },
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Failed to query manga: {e}")))?
        .ok_or_else(|| AppError::not_found(format!("Manga {manga_id} not found")))?;

    let (
        m_id,
        title,
        source_id,
        source_manga_id,
        catalog_manga_id,
        manga_mb_id,
        auto_acquire_enabled,
    ) = manga_row;

    // 2. Fetch series reconciliation data
    let recon_row: Option<(String, Option<i64>, Option<String>, Option<String>, Option<String>, Option<u32>, Option<String>)> = conn
        .query_row(
            "SELECT status, mangabaka_id, collection_id, market, edition, total_volumes, review_reason
             FROM series_reconciliations WHERE manga_id = ?1",
            [m_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<u32>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .unwrap_or(None);

    let (
        reconciliation_status,
        mb_id_from_recon,
        collection_id,
        collection_market,
        collection_edition,
        total_official_volumes,
        review_reason,
    ) = match recon_row {
        Some((st, mb, col, mkt, ed, tot, rev)) => (st, mb, col, mkt, ed, tot, rev),
        None => ("unreconciled".to_string(), None, None, None, None, None, None),
    };

    let effective_mb_id = mb_id_from_recon.or(manga_mb_id);

    // 3. Query OpenTome release targets
    // Map volume_number -> (release_date, status)
    let mut opentome_targets: HashMap<u32, (String, String)> = HashMap::new();
    let opentome_work_id: Option<String> = conn
        .query_row(
            "SELECT work_id FROM opentome_mappings WHERE manga_id = ?1",
            [m_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .unwrap_or(None);

    if let Some(ref work_id) = opentome_work_id {
        let mut target_stmt = conn
            .prepare(
                "SELECT volume_number, release_date, status
                 FROM release_targets WHERE work_id = ?1",
            )
            .map_err(|e| AppError::internal(format!("Prepare release targets failed: {e}")))?;

        let rows = target_stmt
            .query_map([work_id], |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| AppError::internal(format!("Query release targets failed: {e}")))?;

        for r in rows.flatten() {
            let (vol, date, stat) = r;
            let key = (vol * 1000.0).round() as u32;
            opentome_targets.insert(key, (date, stat));
        }
    }

    // 4. Query Chapters (canonical storage vs local staging vs reader caches)
    // Inspect both current manga and source/catalog counterpart
    let mut relevant_manga_ids = vec![m_id];
    if let Some(src_id) = source_manga_id {
        relevant_manga_ids.push(src_id);
    }
    if let Some(cat_id) = catalog_manga_id {
        relevant_manga_ids.push(cat_id);
    }

    let mut canonical_volumes_set: HashSet<u32> = HashSet::new();
    let mut canonical_volume_paths: HashMap<u32, String> = HashMap::new();
    let mut local_staging_set: HashSet<u32> = HashSet::new();
    let mut reader_chapters_set: HashSet<u32> = HashSet::new();
    let mut reader_chapter_count: usize = 0;
    let mut max_reader_chapter: Option<f64> = None;

    for rel_id in relevant_manga_ids {
        let mut chap_stmt = conn
            .prepare(
                "SELECT chapter_number, download_path FROM chapters WHERE manga_id = ?1",
            )
            .map_err(|e| AppError::internal(format!("Prepare chapters failed: {e}")))?;

        let rows = chap_stmt
            .query_map([rel_id], |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            })
            .map_err(|e| AppError::internal(format!("Query chapters failed: {e}")))?;

        for r in rows.flatten() {
            let (num, dl_path_opt) = r;
            if num <= 0.0 {
                continue;
            }
            let key = (num * 1000.0).round() as u32;

            match dl_path_opt {
                Some(ref dl_path) => {
                    let is_canonical = dl_path.starts_with("/data/local/")
                        || dl_path.starts_with("/srv/media/")
                        || dl_path.starts_with("/pipeline-library/");

                    if is_canonical {
                        canonical_volumes_set.insert(key);
                        canonical_volume_paths.entry(key).or_insert_with(|| dl_path.clone());
                    } else if dl_path.starts_with("Local/") || !dl_path.starts_with('/') {
                        local_staging_set.insert(key);
                    } else {
                        local_staging_set.insert(key);
                    }
                }
                None => {
                    reader_chapters_set.insert(key);
                    reader_chapter_count += 1;
                    if max_reader_chapter.map_or(true, |cur| num > cur) {
                        max_reader_chapter = Some(num);
                    }
                }
            }
        }
    }

    // 5. Query Publication Receipts (M6.9)
    let mut receipts_map: HashMap<u32, (String, String)> = HashMap::new();
    let mut rec_stmt = conn
        .prepare(
            "SELECT id, volume_number, canonical_path FROM publication_receipts WHERE manga_id = ?1",
        )
        .map_err(|e| AppError::internal(format!("Prepare receipts failed: {e}")))?;

    let rec_rows = rec_stmt
        .query_map([m_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| AppError::internal(format!("Query receipts failed: {e}")))?;

    for r in rec_rows.flatten() {
        let (rec_id, vol_opt, can_path) = r;
        if let Some(vol) = vol_opt {
            let key = (vol * 1000.0).round() as u32;
            receipts_map.insert(key, (rec_id, can_path.clone()));
            // A verified receipt is authoritative proof of canonical publication
            canonical_volumes_set.insert(key);
            canonical_volume_paths.entry(key).or_insert(can_path);
        }
    }

    // 6. Query Candidates and Quality Rankings (M6.11)
    struct CandidateSummary {
        count: usize,
        top_title: Option<String>,
        top_source: Option<String>,
        top_score: Option<f64>,
        top_auto_eligible: Option<bool>,
    }

    let mut candidate_map: HashMap<u32, CandidateSummary> = HashMap::new();
    let mut cand_stmt = conn
        .prepare(
            "SELECT volume_number, provider, title, raw_title, score, auto_acquire_eligible
             FROM discovered_release_candidates WHERE manga_id = ?1 ORDER BY score DESC",
        )
        .map_err(|e| AppError::internal(format!("Prepare candidates failed: {e}")))?;

    let cand_rows = cand_stmt
        .query_map([m_id], |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, i64>(5)? != 0,
            ))
        })
        .map_err(|e| AppError::internal(format!("Query candidates failed: {e}")))?;

    for r in cand_rows.flatten() {
        let (vol, prov, title_opt, raw_title, score, auto_elig) = r;
        let key = (vol * 1000.0).round() as u32;
        let entry = candidate_map.entry(key).or_insert_with(|| CandidateSummary {
            count: 0,
            top_title: None,
            top_source: None,
            top_score: None,
            top_auto_eligible: None,
        });

        entry.count += 1;
        if entry.top_score.is_none() || score > entry.top_score.unwrap() {
            entry.top_title = Some(title_opt.unwrap_or(raw_title));
            entry.top_source = Some(prov);
            entry.top_score = Some(score);
            entry.top_auto_eligible = Some(auto_elig);
        }
    }

    // 7. Active Backlog Jobs & Downloads
    let mut active_jobs_map: HashMap<u32, String> = HashMap::new();
    let mut dl_stmt = conn
        .prepare(
            "SELECT c.chapter_number, d.id
             FROM downloads d
             JOIN chapters c ON d.chapter_id = c.id
             WHERE c.manga_id = ?1 AND d.state IN ('pending', 'running')",
        )
        .map_err(|e| AppError::internal(format!("Prepare active downloads failed: {e}")))?;

    let dl_rows = dl_stmt
        .query_map([m_id], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| AppError::internal(format!("Query active downloads failed: {e}")))?;

    for r in dl_rows.flatten() {
        let (num, dl_id) = r;
        let key = (num * 1000.0).round() as u32;
        active_jobs_map.insert(key, format!("download-{dl_id}"));
    }

    // 8. Determine all Volume Keys for the Timeline
    let mut all_volume_keys: HashSet<u32> = HashSet::new();

    // If MangaBaka established an official collection with total_volumes, include 1..=total_volumes
    if let Some(total) = total_official_volumes {
        if total > 0 && collection_id.is_some() {
            for v in 1..=total {
                all_volume_keys.insert(v * 1000);
            }
        }
    }

    // Add volumes known from OpenTome targets
    for key in opentome_targets.keys() {
        all_volume_keys.insert(*key);
    }

    // Add volumes known from canonical Komga storage
    for key in canonical_volumes_set.iter() {
        all_volume_keys.insert(*key);
    }

    // Add volumes known from local staging
    for key in local_staging_set.iter() {
        all_volume_keys.insert(*key);
    }

    // Add volumes with candidates
    for key in candidate_map.keys() {
        all_volume_keys.insert(*key);
    }

    let mut sorted_keys: Vec<u32> = all_volume_keys.into_iter().collect();
    sorted_keys.sort_unstable();

    // 9. Identify Owned Volumes for Gap Analysis
    let mut owned_keys_sorted: Vec<u32> = sorted_keys
        .iter()
        .copied()
        .filter(|k| canonical_volumes_set.contains(k) || receipts_map.contains_key(k))
        .collect();
    owned_keys_sorted.sort_unstable();

    let min_owned_key = owned_keys_sorted.first().copied();
    let max_owned_key = owned_keys_sorted.last().copied();

    // 10. Build Volume DTOs
    let mut volumes_dto = Vec::new();
    let mut gap_volume_numbers = Vec::new();
    let mut interior_gap_count = 0;
    let mut leading_gap_count = 0;
    let mut trailing_gap_count = 0;

    let now_date_str: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
        .unwrap_or_else(|_| crate::sync::reconciliation::today_ymd());

    for key in &sorted_keys {
        let vol_num = (*key as f64) / 1000.0;
        let is_owned = canonical_volumes_set.contains(key) || receipts_map.contains_key(key);
        let is_reader_cached = local_staging_set.contains(key);
        let is_canonical_file = canonical_volumes_set.contains(key);

        let target_info = opentome_targets.get(key);
        let release_date = target_info.map(|t| t.0.clone());
        let release_status = target_info.map(|t| t.1.clone());

        let cand_info = candidate_map.get(key);
        let candidate_count = cand_info.map_or(0, |c| c.count);
        let top_candidate_title = cand_info.and_then(|c| c.top_title.clone());
        let top_candidate_source = cand_info.and_then(|c| c.top_source.clone());
        let top_candidate_score = cand_info.and_then(|c| c.top_score);
        let top_candidate_auto_eligible = cand_info.and_then(|c| c.top_auto_eligible);

        let receipt_entry = receipts_map.get(key);
        let receipt_id = receipt_entry.map(|r| r.0.clone());
        let canonical_path = canonical_volume_paths.get(key).cloned();
        let download_job_id = active_jobs_map.get(key).cloned();

        // Gap classification
        let mut is_gap = false;
        let mut gap_type: Option<String> = None;

        if !is_owned {
            if let (Some(min_o), Some(max_o)) = (min_owned_key, max_owned_key) {
                if *key > min_o && *key < max_o {
                    is_gap = true;
                    gap_type = Some("INTERIOR".to_string());
                    interior_gap_count += 1;
                    gap_volume_numbers.push(vol_num);
                } else if *key < min_o {
                    is_gap = true;
                    gap_type = Some("LEADING".to_string());
                    leading_gap_count += 1;
                    gap_volume_numbers.push(vol_num);
                } else if *key > max_o {
                    is_gap = true;
                    gap_type = Some("TRAILING".to_string());
                    trailing_gap_count += 1;
                    gap_volume_numbers.push(vol_num);
                }
            } else {
                // If nothing is owned yet, all official unowned volumes are LEADING
                is_gap = true;
                gap_type = Some("LEADING".to_string());
                leading_gap_count += 1;
                gap_volume_numbers.push(vol_num);
            }
        }

        // Status resolution
        let status = if is_owned {
            "OWNED".to_string()
        } else if download_job_id.is_some() {
            "ACQUIRING".to_string()
        } else if candidate_count > 0 && top_candidate_auto_eligible == Some(true) {
            "CANDIDATE_AVAILABLE".to_string()
        } else if candidate_count > 0 {
            "CANDIDATE_UNAVAILABLE".to_string()
        } else if let Some(ref r_date) = release_date {
            if r_date > &now_date_str {
                "UNRELEASED_FUTURE".to_string()
            } else {
                "RELEASED_MISSING".to_string()
            }
        } else if collection_id.is_none() {
            "COLLECTION_UNAVAILABLE".to_string()
        } else if review_reason.is_some() || reconciliation_status == "review_required" {
            "MAPPING_AMBIGUOUS".to_string()
        } else {
            "RELEASED_MISSING".to_string()
        };

        volumes_dto.push(SeriesTimelineVolumeDto {
            volume_number: vol_num,
            display_title: format!("Volume {}", vol_num),
            status,
            is_owned,
            is_reader_cached,
            is_canonical_file,
            is_gap,
            gap_type,
            release_date,
            release_status,
            candidate_count,
            top_candidate_title,
            top_candidate_source,
            top_candidate_score,
            top_candidate_auto_eligible,
            receipt_id,
            canonical_path,
            download_job_id,
        });
    }

    // 11. Reading-Source Separation Summary
    let mut canonical_vols_sorted: Vec<f64> = canonical_volumes_set
        .iter()
        .map(|k| (*k as f64) / 1000.0)
        .collect();
    canonical_vols_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut staging_vols_sorted: Vec<f64> = local_staging_set
        .iter()
        .map(|k| (*k as f64) / 1000.0)
        .collect();
    staging_vols_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let overlap_count = canonical_volumes_set
        .intersection(&local_staging_set)
        .count();
    let has_duplicate_storage = overlap_count > 0;

    let mut storage_notes = Vec::new();
    storage_notes.push(format!(
        "Canonical Komga Physical Ownership: {} volume(s) located in canonical library storage.",
        canonical_vols_sorted.len()
    ));

    if !staging_vols_sorted.is_empty() {
        storage_notes.push(format!(
            "Local Download Staging: {} volume(s) located in local downloads directory.",
            staging_vols_sorted.len()
        ));
    }

    if reader_chapter_count > 0 {
        storage_notes.push(format!(
            "Reading Source Cache: {} chapter(s) cached from external reading extensions (up to chapter {}). Reading chapter caches are kept strictly separate from canonical volume ownership.",
            reader_chapter_count,
            max_reader_chapter.unwrap_or(0.0)
        ));
    }

    if has_duplicate_storage {
        storage_notes.push(format!(
            "Dual-Storage Alignment: {} volume(s) are present in both canonical library storage and historical local download staging. Canonical files are verified in Komga; local download copies represent historical download staging and are preserved safely.",
            overlap_count
        ));
    }

    let reading_source_separation = ReadingSourceSeparationDto {
        canonical_komga_count: canonical_vols_sorted.len(),
        canonical_volumes: canonical_vols_sorted,
        local_staging_count: staging_vols_sorted.len(),
        local_staging_volumes: staging_vols_sorted,
        reader_chapter_count,
        reader_chapters_latest: max_reader_chapter,
        has_duplicate_storage,
        storage_distinction_notes: storage_notes,
    };

    // 12. Gap Analysis DTO
    let total_vols_count = volumes_dto.len();
    let owned_count = owned_keys_sorted.len();
    let missing_count = total_vols_count.saturating_sub(owned_count);
    let has_interior_gaps = interior_gap_count > 0;

    let summary_text = if total_vols_count == 0 {
        "No official volume records found".to_string()
    } else if has_interior_gaps {
        format!(
            "Interior volume gap detected: {} missing volume(s) between owned volumes",
            interior_gap_count
        )
    } else if missing_count == 0 {
        format!("Complete library: All {} volumes canonically owned", owned_count)
    } else {
        format!(
            "Contiguous library: {} of {} volumes owned ({} remaining)",
            owned_count, total_vols_count, missing_count
        )
    };

    let gaps = SeriesGapAnalysisDto {
        total_volumes: total_vols_count,
        owned_count,
        missing_count,
        interior_gap_count,
        leading_gap_count,
        trailing_gap_count,
        has_interior_gaps,
        gap_volume_numbers,
        summary_text,
    };

    // 13. Query Timeline Events
    let mut events = Vec::new();
    let series_id_manga = format!("manga:{m_id}");
    let series_id_num = m_id.to_string();

    let mut event_stmt = conn
        .prepare(
            "SELECT id, event_type, message, volume_number, 'info', created_at, resolved_at
             FROM reconciliation_events
             WHERE series_id = ?1 OR series_id = ?2
             ORDER BY created_at DESC LIMIT 30",
        )
        .map_err(|e| AppError::internal(format!("Prepare events failed: {e}")))?;

    let event_rows = event_stmt
        .query_map([&series_id_manga, &series_id_num], |r| {
            Ok(SeriesTimelineEventDto {
                id: r.get::<_, i64>(0)?,
                event_type: r.get::<_, String>(1)?,
                message: r.get::<_, String>(2)?,
                volume_number: r.get::<_, Option<f64>>(3)?,
                severity: r.get::<_, String>(4)?,
                created_at: r.get::<_, i64>(5)?,
                resolved_at: r.get::<_, Option<i64>>(6)?,
            })
        })
        .map_err(|e| AppError::internal(format!("Query events failed: {e}")))?;

    for ev in event_rows.flatten() {
        events.push(ev);
    }

    Ok(SeriesTimelineDto {
        manga_id: m_id,
        title,
        source_id,
        mangabaka_id: effective_mb_id,
        collection_id,
        collection_market,
        collection_edition,
        reconciliation_status,
        auto_acquire_enabled,
        volumes: volumes_dto,
        gaps,
        reading_source_separation,
        events,
    })
}

/// Convenience function returning only the gap analysis for a title.
pub fn get_series_gap_analysis(conn: &Connection, manga_id: i64) -> Result<SeriesGapAnalysisDto> {
    let timeline = get_series_timeline(conn, manga_id)?;
    Ok(timeline.gaps)
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod timeline_tests;

