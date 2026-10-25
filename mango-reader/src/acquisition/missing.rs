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
}

impl std::fmt::Display for UnitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Owned => write!(f, "owned"),
            Self::Acquiring => write!(f, "acquiring"),
            Self::Missing => write!(f, "missing"),
            Self::Unavailable => write!(f, "unavailable"),
            Self::Unknown => write!(f, "unknown"),
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
    pub volume_entries: Vec<UnitStatusEntry>,
    pub total_known_chapters: usize,
    pub owned_chapters: Vec<f64>,
    pub missing_chapters: Vec<f64>,
    pub acquiring_chapters: Vec<f64>,
    pub is_fully_owned: bool,
    pub has_missing: bool,
    pub explanation: String,
}

/// Computes the MissingContentSummary from explicit ownership, catalog, and acquisition inputs.
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

    let owned_map: HashMap<i64, Option<String>> = owned_volumes
        .iter()
        .map(|(v, b)| (v.round() as i64, b.clone()))
        .collect();
    let acquiring_set: BTreeSet<i64> = acquiring_volumes.iter().map(|v| v.round() as i64).collect();
    let unavailable_set: BTreeSet<i64> =
        unavailable_volumes.iter().map(|v| v.round() as i64).collect();

    let mut owned_list = Vec::new();
    let mut missing_list = Vec::new();
    let mut acquiring_list = Vec::new();
    let mut unavailable_list = Vec::new();
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

    let explanation = if is_fully_owned {
        format!("All {} known volumes are owned in Komga", known_vol_set.len())
    } else if has_missing {
        format!(
            "{}/{} volumes owned ({} missing, {} acquiring)",
            owned_list.len(),
            known_vol_set.len(),
            missing_list.len(),
            acquiring_list.len()
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
    }

    // Query catalog entries if totals were not in ownership summary
    if authoritative_total_vols.is_none() {
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

    // Query acquiring downloads from active download queue
    let mut acquiring_vols = Vec::new();
    let acquiring_chaps = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT raw_title, chapter_id FROM downloads WHERE (state = 'downloading' OR state = 'queued' OR state = 'pending') AND (raw_title LIKE ?1 OR collection_name LIKE ?1)",
    ) {
        let pattern = format!("%{title}%");
        if let Ok(rows) = stmt.query_map([&pattern], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?))
        }) {
            for row in rows.flatten() {
                if let Some(raw) = row.0 {
                    if let Some(v) = crate::release::parse_volume(&raw) {
                        acquiring_vols.push(v);
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

    calculate_missing_content(
        manga_id,
        title,
        &owned_vols,
        &acquiring_vols,
        authoritative_total_vols,
        &discovered_vols,
        &[],
        &owned_chaps,
        &acquiring_chaps,
        authoritative_total_chaps,
        &discovered_chaps,
    )
}
