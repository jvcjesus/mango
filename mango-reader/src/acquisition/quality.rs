//! Candidate Quality Intelligence & Release Selection (M6.11).
//!
//! Provides multi-factor quality signals, explainable scoring, provider preference
//! policy (Madokami direct HTTP preferred over Nyaa torrents), and strict no-silent-upgrade
//! integrity protections.

use serde::{Deserialize, Serialize};

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::providers::is_known_quality_group;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityFactorScore {
    pub name: String,
    pub points: f64,
    pub is_bonus: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeEvaluation {
    pub is_currently_owned: bool,
    pub upgrade_eligible: bool,
    pub upgrade_justification: Option<String>,
    pub silent_upgrade_blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateQualityEvaluation {
    pub candidate_id: String,
    pub provider: String,
    pub transport: String,
    pub total_score: f64,
    pub rank: usize,
    pub selection_verdict: String, // "PREFERRED", "ALTERNATIVE", "INELIGIBLE", "MANUAL_UPGRADE_ONLY"
    pub factors: Vec<QualityFactorScore>,
    pub resolution_label: Option<String>,
    pub is_digital: bool,
    pub is_trusted: bool,
    pub is_quality_group: bool,
    pub is_revision: bool,
    pub is_pre_release: bool,
    pub is_preferred_provider: bool,
    pub health_rating: String, // "EXCELLENT", "GOOD", "DEGRADED", "DEAD"
    pub upgrade_evaluation: UpgradeEvaluation,
    pub rejection_reason: Option<String>,
}

/// Detects resolution or dimension markers from title string.
pub fn detect_resolution_label(raw_title: &str) -> Option<String> {
    let lower = raw_title.to_ascii_lowercase();
    if lower.contains("2048px") || lower.contains("2048x") || lower.contains("2048 p") {
        Some("2048px (Ultra-HD)".to_string())
    } else if lower.contains("1920px") || lower.contains("1920x") || lower.contains("1080p") {
        Some("1920px (Full HD)".to_string())
    } else if lower.contains("1600px") || lower.contains("1600x") {
        Some("1600px (High-Res)".to_string())
    } else if lower.contains("1200px") || lower.contains("1200x") || lower.contains("720p") {
        Some("1200px (Standard HD)".to_string())
    } else if lower.contains("digital") {
        Some("Digital Archival".to_string())
    } else {
        None
    }
}

/// Evaluates multi-factor quality intelligence for a candidate release.
pub fn evaluate_candidate_quality(
    cand: &PersistedCandidate,
    is_owned: bool,
    rejection_reason: Option<String>,
) -> CandidateQualityEvaluation {
    let mut factors = Vec::new();
    let mut total_score = 0.0;

    let raw_lower = cand.raw_title.to_ascii_lowercase();
    let is_pre = raw_lower.contains("(pre)") || raw_lower.contains("[pre]");
    let is_revision = raw_lower.contains(" v2")
        || raw_lower.contains("[v2]")
        || raw_lower.contains("(v2)")
        || raw_lower.contains("remake");
    let is_quality_grp = is_known_quality_group(&cand.raw_title)
        || cand.uploader.as_deref().map_or(false, is_known_quality_group);
    let resolution = detect_resolution_label(&cand.raw_title);

    // 1. Match Type Base Score
    let (match_base, match_desc) = match cand.match_type.as_str() {
        "exact_match" | "exact_volume" => (60.0, "Exact single volume match (+60)"),
        "volume_range" | "volume_range_containing_target" => {
            (30.0, "Volume range containing target (+30)")
        }
        "series_pack" => (20.0, "Series pack / complete batch (+20)"),
        "possible_match" => (10.0, "Possible / fuzzy volume match (+10)"),
        _ => (0.0, "Unclassified match (0)"),
    };
    total_score += match_base;
    factors.push(QualityFactorScore {
        name: "Match Type".to_string(),
        points: match_base,
        is_bonus: true,
        description: match_desc.to_string(),
    });

    // 2. Provider Preference: Madokami Direct HTTP vs Nyaa BitTorrent
    let is_preferred_provider = cand.provider.eq_ignore_ascii_case("madokami");
    let (transport, provider_pts, provider_desc) = if is_preferred_provider {
        (
            "Direct HTTP (Range resume)".to_string(),
            10.0,
            "Preferred transport: Direct HTTP (no torrent swarms or debrid queue) (+10)",
        )
    } else {
        (
            "TorBox BitTorrent (Debrid)".to_string(),
            0.0,
            "Standard transport: BitTorrent via TorBox (0)",
        )
    };
    if provider_pts > 0.0 {
        total_score += provider_pts;
        factors.push(QualityFactorScore {
            name: "Provider Preference".to_string(),
            points: provider_pts,
            is_bonus: true,
            description: provider_desc.to_string(),
        });
    }

    // 3. Digital Archival Edition
    if cand.is_digital {
        total_score += 15.0;
        factors.push(QualityFactorScore {
            name: "Official Digital".to_string(),
            points: 15.0,
            is_bonus: true,
            description: "Official digital release for pristine archival (+15)".to_string(),
        });
    }

    // 4. Trusted Uploader
    if cand.is_trusted {
        total_score += 10.0;
        factors.push(QualityFactorScore {
            name: "Trusted Uploader".to_string(),
            points: 10.0,
            is_bonus: true,
            description: "Verified trusted community uploader (+10)".to_string(),
        });
    }

    // 5. Recognized Quality Group
    if is_quality_grp {
        total_score += 5.0;
        factors.push(QualityFactorScore {
            name: "Quality Group".to_string(),
            points: 5.0,
            is_bonus: true,
            description: "Recognized high-quality release group (+5)".to_string(),
        });
    }

    // 6. Resolution / Archival Dimensions
    if let Some(ref res_lbl) = resolution {
        let pts = if res_lbl.contains("2048") { 5.0 } else { 3.0 };
        total_score += pts;
        factors.push(QualityFactorScore {
            name: "Resolution".to_string(),
            points: pts,
            is_bonus: true,
            description: format!("{res_lbl} (+{pts:.0})"),
        });
    }

    // 7. Revision / Remake
    if is_revision {
        total_score += 2.0;
        factors.push(QualityFactorScore {
            name: "Revised Edition".to_string(),
            points: 2.0,
            is_bonus: true,
            description: "Revised edition / v2 fix (+2)".to_string(),
        });
    }

    // 8. Health Metrics
    let health_rating = if cand.provider == "madokami" {
        "EXCELLENT".to_string()
    } else {
        match cand.seeders {
            Some(s) if s >= 20 => {
                total_score += 10.0;
                factors.push(QualityFactorScore {
                    name: "Swarm Health".to_string(),
                    points: 10.0,
                    is_bonus: true,
                    description: format!("Well-seeded: {s} seeds (+10)"),
                });
                "EXCELLENT".to_string()
            }
            Some(s) if s >= 5 => {
                total_score += 5.0;
                factors.push(QualityFactorScore {
                    name: "Swarm Health".to_string(),
                    points: 5.0,
                    is_bonus: true,
                    description: format!("Active seeds: {s} seeds (+5)"),
                });
                "GOOD".to_string()
            }
            Some(s) if s >= 1 => {
                total_score += 2.0;
                factors.push(QualityFactorScore {
                    name: "Swarm Health".to_string(),
                    points: 2.0,
                    is_bonus: true,
                    description: format!("Low seeds: {s} seeds (+2)"),
                });
                "DEGRADED".to_string()
            }
            Some(0) => {
                total_score -= 15.0;
                factors.push(QualityFactorScore {
                    name: "Dead Swarm".to_string(),
                    points: -15.0,
                    is_bonus: false,
                    description: "Dead torrent: 0 seeders (-15)".to_string(),
                });
                "DEAD".to_string()
            }
            _ => "UNKNOWN".to_string(),
        }
    };

    // 9. Pre-release / Non-archival penalty
    if is_pre {
        total_score -= 20.0;
        factors.push(QualityFactorScore {
            name: "Pre-Release Penalty".to_string(),
            points: -20.0,
            is_bonus: false,
            description: "Pre-release or preliminary rip (-20)".to_string(),
        });
    }

    total_score = total_score.max(0.0);

    // 10. Upgrade Evaluation & No-Silent-Upgrade Integrity
    let upgrade_evaluation = if is_owned {
        let (eligible, justification) = if cand.is_digital && is_quality_grp {
            (
                true,
                Some("High-quality official digital archive available for manual upgrade".to_string()),
            )
        } else {
            (
                false,
                Some("Candidate does not meet upgrade threshold over canonical copy".to_string()),
            )
        };

        UpgradeEvaluation {
            is_currently_owned: true,
            upgrade_eligible: eligible,
            upgrade_justification: justification,
            silent_upgrade_blocked: true, // Non-negotiable invariant
        }
    } else {
        UpgradeEvaluation {
            is_currently_owned: false,
            upgrade_eligible: false,
            upgrade_justification: None,
            silent_upgrade_blocked: false,
        }
    };

    let selection_verdict = if rejection_reason.is_some() {
        "INELIGIBLE".to_string()
    } else if is_owned {
        if upgrade_evaluation.upgrade_eligible {
            "MANUAL_UPGRADE_ONLY".to_string()
        } else {
            "ALREADY_OWNED".to_string()
        }
    } else if is_preferred_provider {
        "PREFERRED".to_string()
    } else {
        "ALTERNATIVE".to_string()
    };

    CandidateQualityEvaluation {
        candidate_id: cand.candidate_id.clone(),
        provider: cand.provider.clone(),
        transport,
        total_score,
        rank: 0, // Assigned after sorting
        selection_verdict,
        factors,
        resolution_label: resolution,
        is_digital: cand.is_digital,
        is_trusted: cand.is_trusted,
        is_quality_group: is_quality_grp,
        is_revision,
        is_pre_release: is_pre,
        is_preferred_provider,
        health_rating,
        upgrade_evaluation,
        rejection_reason,
    }
}

/// Ranks a collection of evaluated candidates by quality and preference:
/// 1. Eligible > Ineligible
/// 2. Not Owned > Owned
/// 3. Preferred Provider (Madokami) > Alternative (Nyaa)
/// 4. Higher total quality score
/// 5. Digital > Non-digital
/// 6. Non-PRE > PRE
pub fn rank_quality_evaluations(evals: &mut [CandidateQualityEvaluation]) {
    evals.sort_by(|a, b| {
        let a_ineligible = a.rejection_reason.is_some();
        let b_ineligible = b.rejection_reason.is_some();
        a_ineligible
            .cmp(&b_ineligible)
            .then_with(|| a.upgrade_evaluation.is_currently_owned.cmp(&b.upgrade_evaluation.is_currently_owned))
            .then_with(|| b.is_preferred_provider.cmp(&a.is_preferred_provider))
            .then_with(|| {
                b.total_score
                    .partial_cmp(&a.total_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.is_digital.cmp(&a.is_digital))
            .then_with(|| a.is_pre_release.cmp(&b.is_pre_release))
    });

    for (idx, eval) in evals.iter_mut().enumerate() {
        eval.rank = idx + 1;
        if eval.rejection_reason.is_none() && !eval.upgrade_evaluation.is_currently_owned {
            if idx == 0 {
                eval.selection_verdict = "PREFERRED".to_string();
            } else {
                eval.selection_verdict = "ALTERNATIVE".to_string();
            }
        }
    }
}
