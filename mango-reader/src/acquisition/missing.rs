//! Missing Content Engine.
//!
//! Evaluates known catalog units vs Komga owned units vs in-flight acquisitions.
//! Enforces conservative missingness: does not infer unreleased future volumes or fabricate
//! gaps without evidence.

use crate::pipeline::ownership::OwnershipStatus;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitState {
    Owned,
    Acquiring,
    Missing,
    Unavailable,
    Unknown,
    Unreleased,
}

impl std::fmt::Display for UnitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Owned => write!(f, "owned"),
            Self::Acquiring => write!(f, "acquiring"),
            Self::Missing => write!(f, "missing"),
            Self::Unavailable => write!(f, "unavailable"),
            Self::Unknown => write!(f, "unknown"),
            Self::Unreleased => write!(f, "unreleased"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitStatusEntry {
    pub unit: f64,
    pub state: UnitState,
    pub evidence: String,
    pub book_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingContentSummary {
    pub manga_id: i64,
    pub title: String,
    pub total_known_volumes: usize,
    pub owned_volumes: Vec<f64>,
    pub missing_volumes: Vec<f64>,
    pub acquiring_volumes: Vec<f64>,
    pub unavailable_volumes: Vec<f64>,
    pub unknown_volumes: Vec<f64>,
    #[serde(default)]
    pub future_volumes: Vec<f64>,
    #[serde(default)]
    pub interior_gaps: Vec<f64>,
    #[serde(default)]
    pub leading_gaps: Vec<f64>,
    #[serde(default)]
    pub trailing_gaps: Vec<f64>,
    #[serde(default)]
    pub has_interior_gaps: bool,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub collection_id: Option<String>,
    pub volume_entries: Vec<UnitStatusEntry>,
    pub total_known_chapters: usize,
    pub owned_chapters: Vec<f64>,
    pub missing_chapters: Vec<f64>,
    pub acquiring_chapters: Vec<f64>,
    pub is_fully_owned: bool,
    pub has_missing: bool,
    pub explanation: String,
}

/// Computes the MissingContentSummary with gap classification, future volume awareness, and edition metadata.
pub fn calculate_missing_content_extended(
    manga_id: i64,
    title: &str,
    owned_volumes: &[(f64, Option<String>)], // (volume_number, book_id)
    acquiring_volumes: &[f64],
    authoritative_total_volumes: Option<u32>,
    discovered_volumes: &[f64],
    unavailable_volumes: &[f64],
    future_volumes: &[f64],
    edition: Option<String>,
    collection_id: Option<String>,
    owned_chapters: &[f64],
    acquiring_chapters: &[f64],
    authoritative_total_chapters: Option<u32>,
    discovered_chapters: &[f64],
) -> MissingContentSummary {
    let mut known_vol_set: BTreeSet<i64> = BTreeSet::new();

    // 1. Authoritative total from AniList/MangaBaka (e.g. 39 volumes) -> 1..=39 are known to exist
    if let Some(total) = authoritative_total_volumes {
        for v in 1..=(total as i64) {
            known_vol_set.insert(v);
        }
    }

    // 2. Volumes already owned in Komga are known
    for (v, _) in owned_volumes {
        let v_round = v.round() as i64;
        known_vol_set.insert(v_round);
    }

    // 3. Specific volumes with positive evidence from discovery
    for v in discovered_volumes {
        let v_round = v.round() as i64;
        known_vol_set.insert(v_round);
    }

    // 4. In-flight acquiring volumes
    for v in acquiring_volumes {
        let v_round = v.round() as i64;
        known_vol_set.insert(v_round);
    }

    // 5. Future scheduled volumes
    for v in future_volumes {
        let v_round = v.round() as i64;
        known_vol_set.insert(v_round);
    }

    let owned_map: HashMap<i64, Option<String>> = owned_volumes
        .iter()
        .map(|(v, b)| (v.round() as i64, b.clone()))
        .collect();
    let acquiring_set: BTreeSet<i64> = acquiring_volumes.iter().map(|v| v.round() as i64).collect();
    let unavailable_set: BTreeSet<i64> =
        unavailable_volumes.iter().map(|v| v.round() as i64).collect();
    let future_set: BTreeSet<i64> = future_volumes.iter().map(|v| v.round() as i64).collect();

    let mut owned_list = Vec::new();
    let mut missing_list = Vec::new();
    let mut acquiring_list = Vec::new();
    let mut unavailable_list = Vec::new();
    let mut future_list = Vec::new();
    let mut volume_entries = Vec::new();

    for &v_int in &known_vol_set {
        let v_f64 = v_int as f64;
        if let Some(book_id) = owned_map.get(&v_int) {
            owned_list.push(v_f64);
            volume_entries.push(UnitStatusEntry {
                unit: v_f64,
                state: UnitState::Owned,
                evidence: "Owned in Komga library".to_string(),
                book_id: book_id.clone(),
            });
        } else if acquiring_set.contains(&v_int) {
            acquiring_list.push(v_f64);
            volume_entries.push(UnitStatusEntry {
                unit: v_f64,
                state: UnitState::Acquiring,
                evidence: "Currently in active download queue".to_string(),
                book_id: None,
            });
        } else if future_set.contains(&v_int) {
            future_list.push(v_f64);
            volume_entries.push(UnitStatusEntry {
                unit: v_f64,
                state: UnitState::Unreleased,
                evidence: "Scheduled future release in OpenTome / MangaBaka".to_string(),
                book_id: None,
            });
        } else if unavailable_set.contains(&v_int) {
            unavailable_list.push(v_f64);
            volume_entries.push(UnitStatusEntry {
                unit: v_f64,
                state: UnitState::Unavailable,
                evidence: "Known release but unavailable from current providers".to_string(),
                book_id: None,
            });
        } else {
            missing_list.push(v_f64);
            let evidence = if let Some(tot) = authoritative_total_volumes {
                format!("Known from AniList/MangaBaka catalog (1..={tot})")
            } else {
                "Known from discovery/library evidence".to_string()
            };
            volume_entries.push(UnitStatusEntry {
                unit: v_f64,
                state: UnitState::Missing,
                evidence,
                book_id: None,
            });
        }
    }

    // Gap classification across released missing volumes
    let mut interior_gaps = Vec::new();
    let mut leading_gaps = Vec::new();
    let mut trailing_gaps = Vec::new();

    let owned_set_ints: BTreeSet<i64> = owned_list.iter().map(|v| v.round() as i64).collect();
    if owned_set_ints.is_empty() {
        leading_gaps = missing_list.clone();
    } else {
        let min_owned = *owned_set_ints.iter().next().unwrap();
        let max_owned = *owned_set_ints.iter().next_back().unwrap();
        for &m in &missing_list {
            let m_i = m.round() as i64;
            if m_i > min_owned && m_i < max_owned {
                interior_gaps.push(m);
            } else if m_i < min_owned {
                leading_gaps.push(m);
            } else if m_i > max_owned {
                trailing_gaps.push(m);
            }
        }
    }
    let has_interior_gaps = !interior_gaps.is_empty();

    // Chapters calculation
    let mut known_chap_set: BTreeSet<i64> = BTreeSet::new();
    if let Some(total) = authoritative_total_chapters {
        for c in 1..=(total as i64) {
            known_chap_set.insert(c);
        }
    }
    for c in owned_chapters {
        known_chap_set.insert(c.round() as i64);
    }
    for c in discovered_chapters {
        known_chap_set.insert(c.round() as i64);
    }
    let owned_chap_set: BTreeSet<i64> = owned_chapters.iter().map(|c| c.round() as i64).collect();
    let acquiring_chap_set: BTreeSet<i64> =
        acquiring_chapters.iter().map(|c| c.round() as i64).collect();

    let mut owned_chap_list = Vec::new();
    let mut missing_chap_list = Vec::new();
    let mut acquiring_chap_list = Vec::new();

    for &c_int in &known_chap_set {
        let c_f64 = c_int as f64;
        if owned_chap_set.contains(&c_int) {
            owned_chap_list.push(c_f64);
        } else if acquiring_chap_set.contains(&c_int) {
            acquiring_chap_list.push(c_f64);
        } else {
            missing_chap_list.push(c_f64);
        }
    }

    let is_fully_owned = !known_vol_set.is_empty() && missing_list.is_empty() && acquiring_list.is_empty();
    let has_missing = !missing_list.is_empty();

    let explanation = if known_vol_set.is_empty() {
        "No volume releases known or owned".to_string()
    } else if is_fully_owned && future_list.is_empty() {
        format!("All {} known volumes are owned in Komga", owned_list.len())
    } else if is_fully_owned && !future_list.is_empty() {
        format!("All {} released volumes are owned in Komga ({} upcoming scheduled)", owned_list.len(), future_list.len())
    } else if has_interior_gaps {
        format!(
            "{}/{} released volumes owned · Interior gap: volume(s) {:?} missing between owned volumes",
            owned_list.len(),
            owned_list.len() + missing_list.len(),
            interior_gaps
        )
    } else if has_missing {
        let upcoming_suffix = if future_list.is_empty() {
            String::new()
        } else {
            format!(", {} upcoming", future_list.len())
        };
        format!(
            "{}/{} released volumes owned ({} missing, {} acquiring{})",
            owned_list.len(),
            owned_list.len() + missing_list.len(),
            missing_list.len(),
            acquiring_list.len(),
            upcoming_suffix
        )
    } else if !acquiring_list.is_empty() {
        format!(
            "{}/{} volumes owned ({} currently acquiring)",
            owned_list.len(),
            known_vol_set.len(),
            acquiring_list.len()
        )
    } else {
        "No volume releases known or owned".to_string()
    };

    MissingContentSummary {
        manga_id,
        title: title.to_string(),
        total_known_volumes: known_vol_set.len(),
        owned_volumes: owned_list,
        missing_volumes: missing_list,
        acquiring_volumes: acquiring_list,
        unavailable_volumes: unavailable_list,
        unknown_volumes: vec![],
        future_volumes: future_list,
        interior_gaps,
        leading_gaps,
        trailing_gaps,
        has_interior_gaps,
        edition,
        collection_id,
        volume_entries,
        total_known_chapters: known_chap_set.len(),
        owned_chapters: owned_chap_list,
        missing_chapters: missing_chap_list,
        acquiring_chapters: acquiring_chap_list,
        is_fully_owned,
        has_missing,
        explanation,
    }
}

/// Backwards-compatible wrapper calling `calculate_missing_content_extended`.
pub fn calculate_missing_content(
    manga_id: i64,
    title: &str,
    owned_volumes: &[(f64, Option<String>)], // (volume_number, book_id)
    acquiring_volumes: &[f64],
    authoritative_total_volumes: Option<u32>,
    discovered_volumes: &[f64],
    unavailable_volumes: &[f64],
    owned_chapters: &[f64],
    acquiring_chapters: &[f64],
    authoritative_total_chapters: Option<u32>,
    discovered_chapters: &[f64],
) -> MissingContentSummary {
    calculate_missing_content_extended(
        manga_id,
        title,
        owned_volumes,
        acquiring_volumes,
        authoritative_total_volumes,
        discovered_volumes,
        unavailable_volumes,
        &[],
        None,
        None,
        owned_chapters,
        acquiring_chapters,
        authoritative_total_chapters,
        discovered_chapters,
    )
}

/// Loads missing content for a title from the database and Komga ownership.
pub fn resolve_missing_content_for_title(
    conn: &Connection,
    ownership_opt: Option<&crate::pipeline::ownership::TitleOwnership>,
    manga_id: i64,
    title: &str,
) -> MissingContentSummary {
    let mut owned_vols = Vec::new();
    let mut owned_chaps = Vec::new();
    let mut authoritative_total_vols: Option<u32> = None;
    let mut authoritative_total_chaps: Option<u32> = None;

    if let Some(own) = ownership_opt {
        for v in &own.volumes {
            if v.status == OwnershipStatus::Owned {
                owned_vols.push((v.volume, v.book_id.clone()));
            }
        }
        for c in &own.chapters {
            if c.status == OwnershipStatus::Owned {
                owned_chaps.push(c.chapter);
            }
        }
        authoritative_total_vols = own.summary.authoritative_total_volumes;
        authoritative_total_chaps = own.summary.authoritative_total_chapters;
    } else {
        // Fallback when Komga ownership is not available / offline: check local chapters with download_path
        if let Ok(mut stmt) = conn.prepare(
            "SELECT name, download_path, chapter_number FROM chapters WHERE manga_id = ?1 AND download_path IS NOT NULL"
        ) {
            if let Ok(rows) = stmt.query_map([manga_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, f64>(2)?))
            }) {
                for row in rows.flatten() {
                    let vol_opt = crate::release::parse_volume(&row.0)
                        .or_else(|| row.1.as_deref().and_then(crate::release::parse_volume));
                    if let Some(v) = vol_opt {
                        owned_vols.push((v, None));
                    } else if row.2 >= 0.0 {
                        owned_chaps.push(row.2);
                    }
                }
            }
        }
    }

    // 2. Query series_reconciliations for collection, edition, totals, and future volumes
    let mut recon_edition: Option<String> = None;
    let mut recon_collection_id: Option<String> = None;
    let mut recon_status: Option<String> = None;
    let mut future_vols_set: BTreeSet<i64> = BTreeSet::new();

    let series_id_manga = format!("manga:{manga_id}");
    if let Ok(Some((st, col, ed, tot, ow_json, _mi_json, fu_json))) = conn
        .query_row(
            "SELECT status, collection_id, edition, total_official_volumes,
                    owned_volumes_json, missing_volumes_json, future_volumes_json
             FROM series_reconciliations
             WHERE manga_id = ?1 OR series_id = ?2",
            rusqlite::params![manga_id, series_id_manga],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<u32>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
    {
        recon_status = Some(st.clone());
        recon_collection_id = col;
        recon_edition = ed;

        if let Some(tot_val) = tot {
            if tot_val > 0 {
                authoritative_total_vols = Some(tot_val);
            }
        }

        // If owned_vols is empty, populate from owned_volumes_json
        if owned_vols.is_empty() {
            if let Some(ref json_str) = ow_json {
                if let Ok(vols) = serde_json::from_str::<Vec<f64>>(json_str) {
                    for v in vols {
                        owned_vols.push((v, None));
                    }
                }
            }
        }

        // Parse future_volumes_json
        if let Some(ref json_str) = fu_json {
            if let Ok(vols) = serde_json::from_str::<Vec<f64>>(json_str) {
                for v in vols {
                    future_vols_set.insert(v.round() as i64);
                }
            }
        }

        // Fail-closed on collection unavailable: do not fabricate unowned volumes!
        if st == "collection_unavailable" || (recon_collection_id.is_none() && st == "review_required") {
            authoritative_total_vols = None;
        }
    }

    // 3. Query OpenTome release targets for future / pre-release volumes
    let today = crate::sync::reconciliation::today_ymd();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT volume_number, release_date, status
         FROM release_discovery_targets
         WHERE manga_id = ?1 OR series_id = ?2",
    ) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![manga_id, series_id_manga], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        }) {
            for (vol, date, stat) in rows.flatten() {
                if stat == "pre" || date > today {
                    future_vols_set.insert(vol.round() as i64);
                }
            }
        }
    }

    // 4. Query catalog entries if totals were not in ownership or reconciliation
    // Only query if edition is standard or unset, preventing cross-edition omnibus skew
    if authoritative_total_vols.is_none() && recon_status.as_deref() != Some("collection_unavailable") {
        let is_non_standard = recon_edition
            .as_deref()
            .map(|e| e.to_lowercase())
            .map(|e| e.contains("omnibus") || e.contains("deluxe") || e.contains("2-in-1") || e.contains("3-in-1") || e.contains("collector"))
            .unwrap_or(false);

        if !is_non_standard {
            let sql = "SELECT total_volumes, total_chapters FROM tracker_list_entries WHERE title = ?1 OR english_title = ?1 OR romaji_title = ?1 LIMIT 1";
            if let Ok(Some((v_tot, c_tot))) = conn
                .query_row(sql, [title], |r| {
                    Ok((r.get::<_, Option<u32>>(0)?, r.get::<_, Option<u32>>(1)?))
                })
                .optional()
            {
                if authoritative_total_vols.is_none() {
                    authoritative_total_vols = v_tot;
                }
                if authoritative_total_chaps.is_none() {
                    authoritative_total_chaps = c_tot;
                }
            }
        }
    }

    let future_vols_vec: Vec<f64> = future_vols_set.into_iter().map(|v| v as f64).collect();

    // Query acquiring downloads from active download queue for this manga
    let mut acquiring_vols = Vec::new();
    let mut acquiring_chaps = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT d.raw_title, c.name, c.chapter_number, d.retrieval_identity_json, d.published_path
         FROM downloads d
         JOIN chapters c ON c.id = d.chapter_id
         WHERE c.manga_id = ?1
           AND d.state IN ('downloading', 'queued', 'pending', 'verifying', 'metadata_wait', 'metadata_review', 'metadata_failed', 'importing', 'needs_review')",
    ) {
        if let Ok(rows) = stmt.query_map([manga_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        }) {
            for (raw, name, num, identity_json, pub_path) in rows.flatten() {
                // If already published, it is owned/published, not just acquiring
                if pub_path.is_some() {
                    let vol_opt = crate::release::parse_volume(&name)
                        .or_else(|| pub_path.as_deref().and_then(crate::release::parse_volume))
                        .or_else(|| raw.as_deref().and_then(crate::release::parse_volume));
                    if let Some(v) = vol_opt {
                        if !owned_vols.iter().any(|(ov, _)| (*ov - v).abs() < 0.001) {
                            owned_vols.push((v, None));
                        }
                    } else if num >= 0.0 && !owned_chaps.iter().any(|oc| (*oc - num).abs() < 0.001) {
                        owned_chaps.push(num);
                    }
                    continue;
                }
                let parsed_unit = identity_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<super::edition::RetrievalIdentity>(s).ok())
                    .map(|id| id.requested_unit);
                if let Some(super::AcquisitionReleaseUnit::Volume(v)) = parsed_unit {
                    acquiring_vols.push(v);
                } else if let Some(super::AcquisitionReleaseUnit::Chapter(c)) = parsed_unit {
                    acquiring_chaps.push(c);
                } else {
                    let vol_opt = raw.as_deref().and_then(crate::release::parse_volume)
                        .or_else(|| crate::release::parse_volume(&name));
                    if let Some(v) = vol_opt {
                        acquiring_vols.push(v);
                    } else if num >= 0.0 {
                        acquiring_chaps.push(num);
                    }
                }
            }
        }
    }

    // Also check acquisition_history for completed/published items
    if let Ok(mut stmt) = conn.prepare(
        "SELECT release_name, canonical_path FROM acquisition_history
         WHERE manga_id = ?1 AND outcome IN ('completed', 'published') AND canonical_path IS NOT NULL",
    ) {
        if let Ok(rows) = stmt.query_map([manga_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        }) {
            for (rel_name, can_path) in rows.flatten() {
                let path_ref = can_path.as_deref().unwrap_or(&rel_name);
                if let Some(v) = crate::release::parse_volume(path_ref).or_else(|| crate::release::parse_volume(&rel_name)) {
                    if !owned_vols.iter().any(|(ov, _)| (*ov - v).abs() < 0.001) {
                        owned_vols.push((v, None));
                    }
                }
            }
        }
    }

    // Discovered volumes from known chapters/releases in DB
    let mut discovered_vols = Vec::new();
    let mut discovered_chaps = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT name, chapter_number FROM chapters WHERE manga_id = ?1") {
        if let Ok(rows) = stmt.query_map([manga_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        }) {
            for row in rows.flatten() {
                if let Some(v) = crate::release::parse_volume(&row.0) {
                    discovered_vols.push(v);
                }
                if row.1 >= 0.0 {
                    discovered_chaps.push(row.1);
                }
            }
        }
    }

    calculate_missing_content_extended(
        manga_id,
        title,
        &owned_vols,
        &acquiring_vols,
        authoritative_total_vols,
        &discovered_vols,
        &[],
        &future_vols_vec,
        recon_edition,
        recon_collection_id,
        &owned_chaps,
        &acquiring_chaps,
        authoritative_total_chaps,
        &discovered_chaps,
    )
}
