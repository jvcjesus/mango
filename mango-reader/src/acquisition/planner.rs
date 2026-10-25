//! Missing-Content Acquisition Planner.
//!
//! Solves the constrained coverage problem deterministically:
//! Evaluates missing units against normalized candidates from all sources,
//! applies Acquisition Profile preferences, enforces edition safety and budget bounds,
//! computes explainable confidence and recommendations, and supports comprehensive dry-run planning.

use super::candidates::NormalizedAcquisitionCandidate;
use super::missing::MissingContentSummary;
use super::profiles::AcquisitionProfile;
use super::AcquisitionReleaseUnit;
use crate::db::now;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateExplanation {
    pub candidate_id: String,
    pub candidate_title: String,
    pub provider: String,
    pub why_recommended: Vec<String>,
    pub why_lower: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionPlanItem {
    pub candidate: NormalizedAcquisitionCandidate,
    pub units_covered: Vec<f64>,
    pub duplicate_units: Vec<f64>,
    pub explanation: CandidateExplanation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionPlan {
    pub id: String,
    pub manga_id: i64,
    pub series_title: String,
    pub profile_id: String,
    pub profile_name: String,
    pub missing_units: Vec<f64>,
    pub selected_items: Vec<AcquisitionPlanItem>,
    pub alternatives: Vec<NormalizedAcquisitionCandidate>,
    pub confidence: String, // "high", "review_recommended"
    pub state: String,      // "draft", "ready", "partially_acquired", "completed", "stale"
    pub stale_reason: Option<String>,
    pub total_estimated_size: u64,
    pub total_acquisitions_count: usize,
    pub missing_units_covered_count: usize,
    pub is_dry_run: bool,
    pub created_at: i64,
    pub refreshed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPlanSummary {
    pub total_series_analyzed: usize,
    pub series_planned: Vec<AcquisitionPlan>,
    pub total_volumes_to_acquire: usize,
    pub total_collections_count: usize,
    pub total_individual_count: usize,
    pub estimated_total_bytes: u64,
    pub unavailable_volumes_count: usize,
    pub needs_review_count: usize,
    pub already_satisfied_count: usize,
}

/// Evaluates a candidate against an AcquisitionProfile and series ownership.
/// Returns adjusted score and an explanation of bonuses/penalties.
pub fn score_candidate_for_profile(
    cand: &NormalizedAcquisitionCandidate,
    profile: &AcquisitionProfile,
    missing_units: &[f64],
    owned_units: &[f64],
    source_status: &str,
) -> (f64, Vec<String>, Vec<String>) {
    let mut score = cand.score;
    let mut why_rec = Vec::new();
    let mut why_low = Vec::new();

    // 1. Coverage analysis
    let missing_set: HashSet<i64> = missing_units.iter().map(|v| v.round() as i64).collect();
    let owned_set: HashSet<i64> = owned_units.iter().map(|v| v.round() as i64).collect();

    let inc_vols = cand.release_unit.included_volumes();
    let mut covers_missing = Vec::new();
    let mut duplicates = Vec::new();

    for v in &inc_vols {
        let v_i = v.round() as i64;
        if missing_set.contains(&v_i) {
            covers_missing.push(*v);
        } else if owned_set.contains(&v_i) {
            duplicates.push(*v);
        }
    }

    if !covers_missing.is_empty() {
        if covers_missing.len() == missing_units.len() && missing_units.len() > 1 {
            why_rec.push(format!("✓ Covers all {} missing volumes", covers_missing.len()));
            score += 15.0;
        } else if covers_missing.len() == 1 && missing_units.len() == 1 {
            why_rec.push(format!("✓ Exact missing volume {}", covers_missing[0]));
            score += 15.0;
        } else {
            why_rec.push(format!("✓ Covers {} missing volume(s)", covers_missing.len()));
        }
    } else {
        why_low.push("− Covers 0 missing volumes".to_string());
        score -= 50.0;
    }

    // 2. Duplicate penalty & collection policy
    if !duplicates.is_empty() {
        why_low.push(format!("− Duplicates {} already-owned volumes", duplicates.len()));
        score -= (duplicates.len() as f64) * 4.0;
        if duplicates.len() as u32 > profile.max_unnecessary_duplicate_coverage {
            score -= 20.0;
        }
    } else {
        why_rec.push("✓ No duplicate volumes".to_string());
    }

    match profile.collection_policy.as_str() {
        "prefer_collections" => {
            if cand.release_unit.is_collection() {
                score += 15.0;
                why_rec.push("✓ Collection matches profile preference".to_string());
            }
        }
        "prefer_individual" => {
            if !cand.release_unit.is_collection() {
                score += 15.0;
                why_rec.push("✓ Standalone volume matches profile preference".to_string());
            } else {
                score -= 15.0;
                why_low.push("− Profile prefers individual volumes over collections".to_string());
            }
        }
        _ => {
            // "smart"
            if missing_units.len() == 1 && !cand.release_unit.is_collection() {
                score += 10.0;
            } else if missing_units.len() >= 3 && cand.release_unit.is_collection() && duplicates.is_empty() {
                score += 12.0;
                why_rec.push("✓ Collection cleanly covers multiple missing units in 1 acquisition".to_string());
            }
        }
    }

    // 3. Digital vs Physical
    if profile.prefer_digital {
        if cand.quality_signals.is_digital {
            score += 8.0;
            why_rec.push("✓ Digital release".to_string());
        } else {
            score -= 5.0;
            why_low.push("− Physical scan release".to_string());
        }
    }

    // 4. Preferred Uploaders
    if cand.quality_signals.is_preferred_uploader {
        score += 15.0;
        if let Some(u) = &cand.uploader {
            why_rec.push(format!("✓ Preferred uploader '{u}'"));
        }
    }

    // 5. Seeders & Health
    if let Some(s) = cand.health.seeders {
        if s >= profile.min_seeders {
            if s >= 10 {
                why_rec.push(format!("✓ {s} healthy seeders"));
            }
        } else if s == 0 {
            if profile.allow_zero_seeders == "no" {
                score -= 50.0;
                why_low.push("− Zero seeders (profile disallows dead releases)".to_string());
            } else {
                score -= 25.0;
                why_low.push("− Zero seeders".to_string());
            }
        } else {
            score -= 10.0;
            why_low.push(format!("− Low seeder count ({s})"));
        }
    }

    // 6. Source Health
    if source_status == "degraded" {
        score -= 15.0;
        why_low.push("− Provider is currently degraded".to_string());
    } else if source_status == "unavailable" {
        score -= 50.0;
        why_low.push("− Provider is currently unavailable".to_string());
    } else {
        why_rec.push(format!("✓ Provider {} healthy", cand.provider));
    }

    // 7. Edition safety check
    if cand.quality_signals.edition_penalty > 0.0 {
        if !profile.allow_unsupported_editions {
            score -= 40.0;
            why_low.push("− Variant / non-canonical edition".to_string());
        } else {
            score -= 15.0;
            why_low.push("− Variant edition (allowed by profile)".to_string());
        }
    }

    // 8. Size budget check
    if let Some(max_acq) = profile.max_acquisition_size_bytes {
        if let Some(sz) = cand.size {
            if sz > max_acq {
                score -= 40.0;
                why_low.push(format!("− Exceeds max acquisition size ({} bytes > {} limit)", sz, max_acq));
            }
        }
    }

    (score, why_rec, why_low)
}

/// Optimizes acquisition across missing units deterministically using constrained coverage.
pub fn generate_plan(
    manga_id: i64,
    series_title: &str,
    profile: &AcquisitionProfile,
    missing_summary: &MissingContentSummary,
    available_candidates: &[NormalizedAcquisitionCandidate],
    source_health_map: &std::collections::HashMap<String, String>, // source_id -> status
    is_dry_run: bool,
) -> AcquisitionPlan {
    let missing_vols = &missing_summary.missing_volumes;

    if missing_vols.is_empty() {
        return AcquisitionPlan {
            id: format!("plan_{manga_id}_{}", now()),
            manga_id,
            series_title: series_title.to_string(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            missing_units: vec![],
            selected_items: vec![],
            alternatives: vec![],
            confidence: "high".to_string(),
            state: "completed".to_string(),
            stale_reason: Some("No missing content; library is fully satisfied".to_string()),
            total_estimated_size: 0,
            total_acquisitions_count: 0,
            missing_units_covered_count: 0,
            is_dry_run,
            created_at: now(),
            refreshed_at: now(),
        };
    }

    // Score and filter all candidates
    struct ScoredCandidate {
        candidate: NormalizedAcquisitionCandidate,
        score: f64,
        why_rec: Vec<String>,
        why_low: Vec<String>,
        useful_volumes: Vec<f64>,
        duplicate_volumes: Vec<f64>,
    }

    let mut scored_candidates: Vec<ScoredCandidate> = Vec::new();
    let missing_set: HashSet<i64> = missing_vols.iter().map(|v| v.round() as i64).collect();
    let owned_set: HashSet<i64> = missing_summary.owned_volumes.iter().map(|v| v.round() as i64).collect();

    for cand in available_candidates {
        // Filter out completely disallowed editions
        if cand.quality_signals.edition_penalty >= 20.0 && !profile.allow_unsupported_editions {
            continue;
        }

        // Filter out mixed collections if not allowed
        if matches!(cand.release_unit, AcquisitionReleaseUnit::MixedCollection { .. })
            && !profile.allow_mixed_collections
        {
            continue;
        }

        // Filter out zero seeders if strictly forbidden
        if cand.health.seeders == Some(0) && profile.allow_zero_seeders == "no" {
            continue;
        }

        // Filter out size constraint violations
        if let Some(max_acq) = profile.max_acquisition_size_bytes {
            if cand.size.unwrap_or(0) > max_acq {
                continue;
            }
        }

        let source_status = source_health_map
            .get(&cand.provider)
            .map(|s| s.as_str())
            .unwrap_or("healthy");

        let (adj_score, why_rec, why_low) = score_candidate_for_profile(
            cand,
            profile,
            missing_vols,
            &missing_summary.owned_volumes,
            source_status,
        );

        let inc_vols = cand.release_unit.included_volumes();
        let mut useful = Vec::new();
        let mut dup = Vec::new();

        for v in inc_vols {
            let v_i = v.round() as i64;
            if missing_set.contains(&v_i) {
                useful.push(v);
            } else if owned_set.contains(&v_i) {
                dup.push(v);
            }
        }

        // Candidate must cover at least one missing volume
        if !useful.is_empty() {
            scored_candidates.push(ScoredCandidate {
                candidate: cand.clone(),
                score: adj_score,
                why_rec,
                why_low,
                useful_volumes: useful,
                duplicate_volumes: dup,
            });
        }
    }

    // Sort scored candidates deterministically:
    // 1. Score desc
    // 2. Useful volumes count desc
    // 3. Duplicate volumes count asc
    // 4. Seeders desc
    // 5. Release ID asc
    scored_candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.useful_volumes.len().cmp(&a.useful_volumes.len()))
            .then_with(|| a.duplicate_volumes.len().cmp(&b.duplicate_volumes.len()))
            .then_with(|| b.candidate.health.seeders.cmp(&a.candidate.health.seeders))
            .then_with(|| a.candidate.provider_release_id.cmp(&b.candidate.provider_release_id))
    });

    // Check if a single high-scoring collection covers all missing units
    let single_collection_cover = scored_candidates.iter().find(|sc| {
        sc.candidate.release_unit.is_collection()
            && sc.useful_volumes.len() == missing_vols.len()
            && sc.duplicate_volumes.len() as u32 <= profile.max_unnecessary_duplicate_coverage
            && sc.score >= 50.0
    });

    // Constrained coverage selection
    let mut selected_items = Vec::new();
    let mut covered_units: BTreeSet<i64> = BTreeSet::new();
    let mut total_size: u64 = 0;
    let mut alternatives = Vec::new();

    if let Some(coll) = single_collection_cover {
        // If collection policy is not "prefer_individual", and we have a clean collection covering all missing units
        let prefer_individual = profile.collection_policy == "prefer_individual";
        let only_one_missing = missing_vols.len() == 1;

        if !prefer_individual && !only_one_missing {
            // Choose the collection as the primary recommended acquisition
            for v in &coll.useful_volumes {
                covered_units.insert(v.round() as i64);
            }
            total_size += coll.candidate.size.unwrap_or(0);
            selected_items.push(AcquisitionPlanItem {
                candidate: coll.candidate.clone(),
                units_covered: coll.useful_volumes.clone(),
                duplicate_units: coll.duplicate_volumes.clone(),
                explanation: CandidateExplanation {
                    candidate_id: coll.candidate.provider_release_id.clone(),
                    candidate_title: coll.candidate.title.clone(),
                    provider: coll.candidate.provider.clone(),
                    why_recommended: coll.why_rec.clone(),
                    why_lower: coll.why_low.clone(),
                },
            });

            // Populate alternatives with other viable candidates
            for sc in &scored_candidates {
                if sc.candidate.provider_release_id != coll.candidate.provider_release_id {
                    alternatives.push(sc.candidate.clone());
                }
            }
        }
    }

    // If no collection chosen above, run greedy set coverage
    if selected_items.is_empty() {
        let mut remaining_missing: BTreeSet<i64> = missing_vols.iter().map(|v| v.round() as i64).collect();

        for sc in &scored_candidates {
            if remaining_missing.is_empty() {
                alternatives.push(sc.candidate.clone());
                continue;
            }

            // Check if this candidate covers any remaining missing units
            let newly_covered: Vec<f64> = sc
                .useful_volumes
                .iter()
                .filter(|v| remaining_missing.contains(&(v.round() as i64)))
                .copied()
                .collect();

            if !newly_covered.is_empty() {
                // Check plan budget limit if configured
                let cand_size = sc.candidate.size.unwrap_or(0);
                if let Some(max_plan) = profile.max_plan_size_bytes {
                    if total_size + cand_size > max_plan && !selected_items.is_empty() {
                        // Budget reached
                        continue;
                    }
                }

                for v in &newly_covered {
                    remaining_missing.remove(&(v.round() as i64));
                    covered_units.insert(v.round() as i64);
                }
                total_size += cand_size;

                selected_items.push(AcquisitionPlanItem {
                    candidate: sc.candidate.clone(),
                    units_covered: newly_covered,
                    duplicate_units: sc.duplicate_volumes.clone(),
                    explanation: CandidateExplanation {
                        candidate_id: sc.candidate.provider_release_id.clone(),
                        candidate_title: sc.candidate.title.clone(),
                        provider: sc.candidate.provider.clone(),
                        why_recommended: sc.why_rec.clone(),
                        why_lower: sc.why_low.clone(),
                    },
                });
            } else {
                alternatives.push(sc.candidate.clone());
            }
        }
    }

    // Evaluate confidence
    let all_covered = covered_units.len() == missing_vols.len();
    let any_ambiguous = selected_items.iter().any(|item| {
        item.candidate.confidence != "high"
            || item.candidate.quality_signals.edition_penalty > 0.0
            || item.candidate.health.seeders.unwrap_or(0) < profile.min_seeders
    });
    let any_heavy_duplicates = selected_items.iter().any(|item| {
        item.duplicate_units.len() as u32 > profile.max_unnecessary_duplicate_coverage
    });

    let confidence = if all_covered && !any_ambiguous && !any_heavy_duplicates && !selected_items.is_empty() {
        "high".to_string()
    } else {
        "review_recommended".to_string()
    };

    let state = if selected_items.is_empty() {
        "draft".to_string()
    } else {
        "ready".to_string()
    };

    let stale_reason = if selected_items.is_empty() {
        Some("No viable candidates currently available from configured providers".to_string())
    } else if !all_covered {
        Some(format!(
            "Partial plan: covers {}/{} missing volumes",
            covered_units.len(),
            missing_vols.len()
        ))
    } else {
        None
    };

    AcquisitionPlan {
        id: format!("plan_{manga_id}_{}", now()),
        manga_id,
        series_title: series_title.to_string(),
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        missing_units: missing_vols.clone(),
        selected_items,
        alternatives,
        confidence,
        state,
        stale_reason,
        total_estimated_size: total_size,
        total_acquisitions_count: covered_units.len(),
        missing_units_covered_count: covered_units.len(),
        is_dry_run,
        created_at: now(),
        refreshed_at: now(),
    }
}

/// Persists an acquisition plan to the database.
pub fn save_plan(conn: &Connection, plan: &AcquisitionPlan) -> Result<()> {
    let missing_json = serde_json::to_string(&plan.missing_units)
        .map_err(|e| AppError::bad_request(format!("invalid missing_units: {e}")))?;
    let selected_json = serde_json::to_string(&plan.selected_items)
        .map_err(|e| AppError::bad_request(format!("invalid selected_items: {e}")))?;
    let alts_json = serde_json::to_string(&plan.alternatives)
        .map_err(|e| AppError::bad_request(format!("invalid alternatives: {e}")))?;

    // Collect explanations
    let explanations: Vec<&CandidateExplanation> =
        plan.selected_items.iter().map(|item| &item.explanation).collect();
    let explanations_json = serde_json::to_string(&explanations)
        .map_err(|e| AppError::bad_request(format!("invalid explanations: {e}")))?;

    // Upsert plan for manga_id
    conn.execute(
        "INSERT INTO acquisition_plans (
            id, manga_id, profile_id, missing_units_json, selected_candidates_json,
            alternative_candidates_json, explanations_json, confidence, state,
            stale_reason, created_at, refreshed_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(id) DO UPDATE SET
            profile_id = excluded.profile_id,
            missing_units_json = excluded.missing_units_json,
            selected_candidates_json = excluded.selected_candidates_json,
            alternative_candidates_json = excluded.alternative_candidates_json,
            explanations_json = excluded.explanations_json,
            confidence = excluded.confidence,
            state = excluded.state,
            stale_reason = excluded.stale_reason,
            refreshed_at = excluded.refreshed_at",
        params![
            plan.id,
            plan.manga_id,
            plan.profile_id,
            missing_json,
            selected_json,
            alts_json,
            explanations_json,
            plan.confidence,
            plan.state,
            plan.stale_reason,
            plan.created_at,
            plan.refreshed_at,
        ],
    )?;

    Ok(())
}

/// Retrieves a persisted plan for a title and performs stale-plan detection.
pub fn get_plan_for_manga(
    conn: &Connection,
    manga_id: i64,
    current_owned_volumes: &[f64],
) -> Result<Option<AcquisitionPlan>> {
    let sql = "SELECT id, manga_id, profile_id, missing_units_json, selected_candidates_json,
                      alternative_candidates_json, confidence, state, stale_reason, created_at, refreshed_at
               FROM acquisition_plans
               WHERE manga_id = ?1
               ORDER BY refreshed_at DESC LIMIT 1";

    let row = conn.query_row(sql, [manga_id], |r| {
        let missing_json: String = r.get(3)?;
        let selected_json: String = r.get(4)?;
        let alts_json: String = r.get(5)?;

        let missing_units: Vec<f64> = serde_json::from_str(&missing_json).unwrap_or_default();
        let selected_items: Vec<AcquisitionPlanItem> =
            serde_json::from_str(&selected_json).unwrap_or_default();
        let alternatives: Vec<NormalizedAcquisitionCandidate> =
            serde_json::from_str(&alts_json).unwrap_or_default();

        Ok(AcquisitionPlan {
            id: r.get(0)?,
            manga_id: r.get(1)?,
            series_title: String::new(),
            profile_id: r.get(2)?,
            profile_name: String::new(),
            missing_units,
            selected_items,
            alternatives,
            confidence: r.get(6)?,
            state: r.get(7)?,
            stale_reason: r.get(8)?,
            total_estimated_size: 0,
            total_acquisitions_count: 0,
            missing_units_covered_count: 0,
            is_dry_run: false,
            created_at: r.get(9)?,
            refreshed_at: r.get(10)?,
        })
    }).optional()?;

    let mut plan = match row {
        Some(p) => p,
        None => return Ok(None),
    };

    // Populate profile name
    if let Ok(Some(prof)) = crate::acquisition::profiles::get_profile_by_id(conn, &plan.profile_id) {
        plan.profile_name = prof.name;
    }

    // Populate series title
    if let Ok(title) = conn.query_row("SELECT title FROM manga WHERE id = ?1", [manga_id], |r| r.get(0)) {
        plan.series_title = title;
    }

    plan.total_acquisitions_count = plan.selected_items.len();
    plan.total_estimated_size = plan.selected_items.iter().filter_map(|i| i.candidate.size).sum();
    plan.missing_units_covered_count = plan
        .selected_items
        .iter()
        .map(|i| i.units_covered.len())
        .sum();

    // Check stale conditions against current Komga ownership
    let owned_set: HashSet<i64> = current_owned_volumes.iter().map(|v| v.round() as i64).collect();
    let all_satisfied = !plan.missing_units.is_empty()
        && plan
            .missing_units
            .iter()
            .all(|v| owned_set.contains(&(v.round() as i64)));

    if all_satisfied {
        plan.state = "stale".to_string();
        plan.stale_reason =
            Some("Plan satisfied: all previously missing units are now owned in Komga library".to_string());
    } else {
        let any_satisfied = plan
            .missing_units
            .iter()
            .any(|v| owned_set.contains(&(v.round() as i64)));
        if any_satisfied && plan.state != "partially_acquired" {
            plan.state = "stale".to_string();
            plan.stale_reason =
                Some("Ownership changed: some planned units are now owned in Komga library. Refresh recommended."
                    .to_string());
        }
    }

    Ok(Some(plan))
}
