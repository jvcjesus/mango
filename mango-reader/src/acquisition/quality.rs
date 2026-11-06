//! Candidate Quality Intelligence & Release Selection (M6.11 / M8.4).
//!
//! Provides multi-factor quality signals, explainable scoring, provider preference
//! policy (Madokami direct HTTP preferred over Nyaa torrents), deduplication across
//! sources, side-by-side comparison matrix, and strict no-silent-upgrade protections.

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
pub struct AlternateSourceDto {
    pub candidate_id: String,
    pub provider: String,
    pub transport: String,
    pub uploader: Option<String>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub raw_title: String,
    pub score: f64,
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
    pub format: String,
    pub edition: String,
    pub size_per_page_kb: Option<f64>,
    pub warning_badges: Vec<String>,
    pub is_digital: bool,
    pub is_trusted: bool,
    pub is_quality_group: bool,
    pub is_revision: bool,
    pub is_pre_release: bool,
    pub is_preferred_provider: bool,
    pub health_rating: String, // "EXCELLENT", "GOOD", "DEGRADED", "DEAD"
    pub upgrade_evaluation: UpgradeEvaluation,
    pub rejection_reason: Option<String>,
    #[serde(default)]
    pub alternate_sources: Vec<AlternateSourceDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateComparisonItem {
    pub candidate_id: String,
    pub rank: usize,
    pub provider: String,
    pub transport: String,
    pub title: String,
    pub raw_title: String,
    pub format: String,
    pub edition: String,
    pub uploader: Option<String>,
    pub size_bytes: Option<u64>,
    pub size_per_page_kb: Option<f64>,
    pub seeders: Option<u32>,
    pub health_rating: String,
    pub total_score: f64,
    pub selection_verdict: String,
    pub factors: Vec<QualityFactorScore>,
    pub warning_badges: Vec<String>,
    pub rejection_reason: Option<String>,
    pub alternate_sources: Vec<AlternateSourceDto>,
    pub advantages: Vec<String>,
    pub disadvantages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateComparisonMatrix {
    pub series_id: String,
    pub volume_number: f64,
    pub candidates: Vec<CandidateComparisonItem>,
    pub preference_explanation: String,
    pub duplicate_groups_count: usize,
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

/// Detects container archive or document format from title string.
pub fn detect_format(raw_title: &str) -> String {
    let lower = raw_title.to_ascii_lowercase();
    if lower.contains(".cbz") || lower.contains("[cbz]") || lower.contains("(cbz)") {
        "CBZ".to_string()
    } else if lower.contains(".cbr") || lower.contains("[cbr]") || lower.contains("(cbr)") {
        "CBR".to_string()
    } else if lower.contains(".epub") || lower.contains("[epub]") || lower.contains("(epub)") {
        "EPUB".to_string()
    } else if lower.contains(".pdf") || lower.contains("[pdf]") || lower.contains("(pdf)") {
        "PDF".to_string()
    } else if lower.contains("folder") || lower.contains("directory") {
        "Folder".to_string()
    } else {
        "CBZ".to_string()
    }
}

/// Detects edition classification (Digital vs Physical vs Scanlation).
pub fn detect_edition_type(cand: &PersistedCandidate) -> String {
    let lower = cand.raw_title.to_ascii_lowercase();
    if cand.is_digital || lower.contains("digital") {
        "Digital".to_string()
    } else if lower.contains("scan") || lower.contains("raw") || lower.contains("c2c") {
        "Scanlation".to_string()
    } else if lower.contains("paperback") || lower.contains("physical") || lower.contains("tankobon") {
        "Physical".to_string()
    } else {
        "Digital".to_string()
    }
}

/// Detects foreign language tags in release titles.
pub fn detect_unwanted_language(raw_title: &str) -> Option<&'static str> {
    let lower = raw_title.to_ascii_lowercase();
    if lower.contains("[fr]") || lower.contains("[french]") || lower.contains(" french") || lower.contains("français") || lower.contains("francais") {
        Some("French")
    } else if lower.contains("[es]") || lower.contains("[spanish]") || lower.contains(" spanish") || lower.contains("español") || lower.contains("espanol") {
        Some("Spanish")
    } else if lower.contains("[de]") || lower.contains("[german]") || lower.contains(" german") || lower.contains("deutsch") {
        Some("German")
    } else if lower.contains("[ita]") || lower.contains("[italian]") || lower.contains(" italian") || lower.contains("italiano") {
        Some("Italian")
    } else if lower.contains("[pt]") || lower.contains("[pt-br]") || lower.contains("[portuguese]") || lower.contains(" portuguese") {
        Some("Portuguese")
    } else if lower.contains("[raw]") || lower.contains("[jp]") || lower.contains("[japanese]") {
        Some("Japanese Raw")
    } else if lower.contains("[chs]") || lower.contains("[cht]") || lower.contains("[chinese]") {
        Some("Chinese")
    } else {
        None
    }
}

/// Estimates size per page in KB assuming standard ~200 page manga volume.
pub fn estimate_size_per_page_kb(size_bytes: Option<u64>) -> Option<f64> {
    size_bytes.map(|b| {
        let kb = (b as f64) / 1024.0;
        let avg_pages = 200.0;
        (kb / avg_pages).round()
    })
}

/// Computes normalized release fingerprint for deduplicating mirrors across providers.
pub fn extract_release_fingerprint(cand: &PersistedCandidate) -> String {
    if let Some(ref hash) = cand.info_hash {
        if !hash.is_empty() {
            return format!("hash:{}", hash.to_ascii_lowercase());
        }
    }

    let mut raw = cand.raw_title.to_ascii_lowercase();
    for ext in &[".cbz", ".cbr", ".epub", ".pdf", ".zip", ".rar"] {
        if raw.ends_with(ext) {
            raw.truncate(raw.len() - ext.len());
            break;
        }
    }

    let mut clean = String::new();
    let mut in_bracket = false;
    let mut in_paren = false;
    for ch in raw.chars() {
        match ch {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            '(' => in_paren = true,
            ')' => in_paren = false,
            c if !in_bracket && !in_paren => {
                if c.is_alphanumeric() || c.is_whitespace() {
                    clean.push(c);
                } else {
                    clean.push(' ');
                }
            }
            _ => {}
        }
    }
    let words: Vec<&str> = clean
        .split_whitespace()
        .filter(|w| !matches!(*w, "cbz" | "cbr" | "epub" | "pdf" | "zip" | "rar" | "digital" | "v2" | "hd"))
        .collect();
    let normalized = words.join(" ");
    let group = cand.uploader.as_deref().unwrap_or("").trim().to_ascii_lowercase();

    format!("{}:{:.1}:{}", normalized, cand.volume_number, group)
}

/// Evaluates multi-factor quality intelligence for a candidate release.
pub fn evaluate_candidate_quality(
    cand: &PersistedCandidate,
    is_owned: bool,
    mut rejection_reason: Option<String>,
) -> CandidateQualityEvaluation {
    let mut factors = Vec::new();
    let mut warning_badges = Vec::new();
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
    let format = detect_format(&cand.raw_title);
    let edition = detect_edition_type(cand);
    let size_per_page_kb = estimate_size_per_page_kb(cand.size_bytes);

    // 1. Match Type Base Score & Multi-volume / Chapter-only checks
    let is_chapter_only = cand.match_type == "chapter"
        || ((raw_lower.contains("ch.") || raw_lower.contains("chapter ") || raw_lower.contains(" c0") || raw_lower.contains(" c1"))
            && !raw_lower.contains("vol") && !raw_lower.contains("v0") && !raw_lower.contains("v1"));

    let (match_base, match_desc) = if is_chapter_only {
        warning_badges.push("Chapter only".to_string());
        if rejection_reason.is_none() {
            rejection_reason = Some("Chapter-only release (not a full volume)".to_string());
        }
        (-50.0, "Chapter-only penalty (-50)")
    } else {
        match cand.match_type.as_str() {
            "exact_match" | "exact_volume" => (50.0, "Exact volume match bonus (+50)"),
            "volume_range" | "volume_range_containing_target" => {
                warning_badges.push("Multi-volume range".to_string());
                if rejection_reason.is_none() {
                    rejection_reason = Some("Requires multi-volume extraction (currently single-volume mode)".to_string());
                }
                (-25.0, "Multi-volume penalty when looking for single volume (-25)")
            }
            "series_pack" => {
                warning_badges.push("Series pack".to_string());
                if rejection_reason.is_none() {
                    rejection_reason = Some("Requires multi-volume extraction (currently single-volume mode)".to_string());
                }
                (-25.0, "Series pack penalty when looking for single volume (-25)")
            }
            "possible_match" => (10.0, "Possible / fuzzy volume match (+10)"),
            _ => (0.0, "Unclassified match (0)"),
        }
    };

    total_score += match_base;
    factors.push(QualityFactorScore {
        name: "Match Type".to_string(),
        points: match_base,
        is_bonus: match_base > 0.0,
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

    // 3. Digital Archival Edition Bonus (+15)
    if cand.is_digital {
        total_score += 15.0;
        factors.push(QualityFactorScore {
            name: "Official Digital".to_string(),
            points: 15.0,
            is_bonus: true,
            description: "Digital edition bonus (+15)".to_string(),
        });
    }

    // 4. Trusted Uploader / Quality Group Bonus (+20)
    if cand.is_trusted || is_quality_grp {
        total_score += 20.0;
        factors.push(QualityFactorScore {
            name: "Trusted Uploader".to_string(),
            points: 20.0,
            is_bonus: true,
            description: "Trusted uploader bonus (+20)".to_string(),
        });
    }

    // 5. Preferred Language Match (+10) vs Unwanted Language Penalty (-50)
    if let Some(foreign_lang) = detect_unwanted_language(&cand.raw_title) {
        total_score -= 50.0;
        factors.push(QualityFactorScore {
            name: "Unwanted Language".to_string(),
            points: -50.0,
            is_bonus: false,
            description: format!("Unwanted language penalty: {foreign_lang} (-50)"),
        });
        warning_badges.push(format!("Language: {foreign_lang}"));
        if rejection_reason.is_none() {
            rejection_reason = Some(format!("Language detected as {foreign_lang} (expected English)"));
        }
    } else {
        total_score += 10.0;
        factors.push(QualityFactorScore {
            name: "Preferred Language".to_string(),
            points: 10.0,
            is_bonus: true,
            description: "Preferred language match (+10)".to_string(),
        });
    }

    // 6. Expected Size (+10) vs Extreme Size Anomaly Penalty (-30)
    if let Some(bytes) = cand.size_bytes {
        let size_mb = (bytes as f64) / (1024.0 * 1024.0);
        let size_gb = size_mb / 1024.0;
        if bytes > 1_200_000_000 {
            total_score -= 30.0;
            factors.push(QualityFactorScore {
                name: "Extreme Size Anomaly".to_string(),
                points: -30.0,
                is_bonus: false,
                description: format!("Extreme size anomaly penalty: {:.1} GB exceeds expected volume size (-30)", size_gb),
            });
            warning_badges.push("Size anomaly".to_string());
            if rejection_reason.is_none() {
                rejection_reason = Some(format!(
                    "Size {:.1} GB exceeds expected volume size (150-300 MB) by {:.0}x",
                    size_gb,
                    (size_gb / 0.25).max(2.0)
                ));
            }
        } else if bytes < 10_000_000 {
            total_score -= 30.0;
            factors.push(QualityFactorScore {
                name: "Extreme Size Anomaly".to_string(),
                points: -30.0,
                is_bonus: false,
                description: format!("Extreme size anomaly penalty: {:.1} MB is suspiciously small (-30)", size_mb),
            });
            warning_badges.push("Suspiciously small".to_string());
            if rejection_reason.is_none() {
                rejection_reason = Some(format!("Archive size {:.1} MB is suspiciously small for a full volume", size_mb));
            }
        } else if bytes >= 40_000_000 && bytes <= 600_000_000 {
            total_score += 10.0;
            factors.push(QualityFactorScore {
                name: "Expected Size".to_string(),
                points: 10.0,
                is_bonus: true,
                description: format!("Size within expected range: {:.0} MB (+10)", size_mb),
            });
        }
    }

    // 7. Resolution / Archival Dimensions
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

    // 8. Revision / Remake
    if is_revision {
        total_score += 2.0;
        factors.push(QualityFactorScore {
            name: "Revised Edition".to_string(),
            points: 2.0,
            is_bonus: true,
            description: "Revised edition / v2 fix (+2)".to_string(),
        });
    }

    // 9. Health & Swarm Metrics / Low seed penalty (-20)
    let health_rating = if cand.provider.eq_ignore_ascii_case("madokami") {
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
                warning_badges.push("Low seeders".to_string());
                "DEGRADED".to_string()
            }
            Some(0) => {
                total_score -= 20.0;
                factors.push(QualityFactorScore {
                    name: "Dead Swarm".to_string(),
                    points: -20.0,
                    is_bonus: false,
                    description: "Low seed/leech penalty: 0 seeders (-20)".to_string(),
                });
                warning_badges.push("0 Seeders".to_string());
                if rejection_reason.is_none() {
                    rejection_reason = Some("Torrent swarm has 0 active seeders".to_string());
                }
                "DEAD".to_string()
            }
            _ => "UNKNOWN".to_string(),
        }
    };

    // 10. Format Warning
    if format == "CBR" {
        warning_badges.push("CBR (Needs conversion)".to_string());
        if rejection_reason.is_none() {
            rejection_reason = Some("File format CBR requires conversion".to_string());
        }
    } else if format == "PDF" {
        warning_badges.push("PDF (Needs conversion)".to_string());
        if rejection_reason.is_none() {
            rejection_reason = Some("File format PDF requires conversion".to_string());
        }
    }

    // 11. Pre-release / Non-archival penalty
    if is_pre {
        total_score -= 20.0;
        factors.push(QualityFactorScore {
            name: "Pre-Release Penalty".to_string(),
            points: -20.0,
            is_bonus: false,
            description: "Pre-release or preliminary rip (-20)".to_string(),
        });
        warning_badges.push("Pre-release".to_string());
    }

    total_score = total_score.max(0.0);

    // 12. Upgrade Evaluation & No-Silent-Upgrade Integrity
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
        format,
        edition,
        size_per_page_kb,
        warning_badges,
        is_digital: cand.is_digital,
        is_trusted: cand.is_trusted,
        is_quality_group: is_quality_grp,
        is_revision,
        is_pre_release: is_pre,
        is_preferred_provider,
        health_rating,
        upgrade_evaluation,
        rejection_reason,
        alternate_sources: Vec::new(),
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

/// Deduplicates candidates across providers and mirrors, retaining the best candidate
/// as primary and attaching alternate mirrors.
pub fn deduplicate_candidate_evaluations(
    evals: Vec<CandidateQualityEvaluation>,
    candidates: &[PersistedCandidate],
) -> (Vec<CandidateQualityEvaluation>, usize) {
    use std::collections::HashMap;

    let mut cand_map: HashMap<String, &PersistedCandidate> = HashMap::new();
    for cand in candidates {
        cand_map.insert(cand.candidate_id.clone(), cand);
    }

    let mut groups: HashMap<String, Vec<CandidateQualityEvaluation>> = HashMap::new();
    let mut duplicate_count = 0;

    for eval in evals {
        let key = if let Some(cand) = cand_map.get(&eval.candidate_id) {
            extract_release_fingerprint(cand)
        } else {
            eval.candidate_id.clone()
        };
        groups.entry(key).or_default().push(eval);
    }

    let mut result = Vec::new();
    for (_key, mut group) in groups {
        if group.len() > 1 {
            duplicate_count += group.len() - 1;
            group.sort_by(|a, b| {
                let a_ineligible = a.rejection_reason.is_some();
                let b_ineligible = b.rejection_reason.is_some();
                a_ineligible
                    .cmp(&b_ineligible)
                    .then_with(|| b.is_preferred_provider.cmp(&a.is_preferred_provider))
                    .then_with(|| {
                        b.total_score
                            .partial_cmp(&a.total_score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
            let mut primary = group.remove(0);
            for alt in group {
                let alt_cand = cand_map.get(&alt.candidate_id);
                primary.alternate_sources.push(AlternateSourceDto {
                    candidate_id: alt.candidate_id,
                    provider: alt.provider,
                    transport: alt.transport,
                    uploader: alt_cand.and_then(|c| c.uploader.clone()),
                    size_bytes: alt_cand.and_then(|c| c.size_bytes),
                    seeders: alt_cand.and_then(|c| c.seeders),
                    raw_title: alt_cand.map(|c| c.raw_title.clone()).unwrap_or_default(),
                    score: alt.total_score,
                });
            }
            result.push(primary);
        } else if let Some(single) = group.pop() {
            result.push(single);
        }
    }

    rank_quality_evaluations(&mut result);
    (result, duplicate_count)
}

/// Builds a structured side-by-side comparison matrix with explainable advantages and disadvantages.
pub fn build_candidate_comparison_matrix(
    series_id: &str,
    volume_number: f64,
    evals: &[CandidateQualityEvaluation],
    candidates: &[PersistedCandidate],
    duplicate_groups_count: usize,
) -> CandidateComparisonMatrix {
    let mut cand_map = std::collections::HashMap::new();
    for c in candidates {
        cand_map.insert(c.candidate_id.as_str(), c);
    }

    let mut items = Vec::new();
    for eval in evals {
        let cand = cand_map.get(eval.candidate_id.as_str());
        let mut advantages = Vec::new();
        let mut disadvantages = Vec::new();

        if eval.is_preferred_provider {
            advantages.push("Direct HTTP transport from Madokami (range resume, no torrent wait)".to_string());
        } else {
            disadvantages.push("BitTorrent transfer requires debrid caching/swarm availability".to_string());
        }

        if eval.is_digital {
            advantages.push("Official digital archival edition".to_string());
        }

        if eval.is_trusted || eval.is_quality_group {
            advantages.push("Verified trusted uploader / recognized quality group".to_string());
        }

        if eval.format == "CBZ" {
            advantages.push("Standard CBZ archive format (native Komga support)".to_string());
        } else if eval.format == "CBR" {
            disadvantages.push("CBR RAR archive requires format conversion".to_string());
        } else if eval.format == "PDF" {
            disadvantages.push("PDF document format requires conversion for comic reader".to_string());
        }

        if eval.health_rating == "EXCELLENT" {
            advantages.push("High availability swarm / verified direct source".to_string());
        } else if eval.health_rating == "DEAD" {
            disadvantages.push("Dead torrent with zero active seeders".to_string());
        } else if eval.health_rating == "DEGRADED" {
            disadvantages.push("Low seeder count in torrent swarm".to_string());
        }

        for factor in &eval.factors {
            if !factor.is_bonus && factor.points < -10.0 {
                disadvantages.push(factor.description.clone());
            }
        }

        items.push(CandidateComparisonItem {
            candidate_id: eval.candidate_id.clone(),
            rank: eval.rank,
            provider: eval.provider.clone(),
            transport: eval.transport.clone(),
            title: cand.map(|c| c.title.clone()).unwrap_or_else(|| eval.candidate_id.clone()),
            raw_title: cand.map(|c| c.raw_title.clone()).unwrap_or_default(),
            format: eval.format.clone(),
            edition: eval.edition.clone(),
            uploader: cand.and_then(|c| c.uploader.clone()),
            size_bytes: cand.and_then(|c| c.size_bytes),
            size_per_page_kb: eval.size_per_page_kb,
            seeders: cand.and_then(|c| c.seeders),
            health_rating: eval.health_rating.clone(),
            total_score: eval.total_score,
            selection_verdict: eval.selection_verdict.clone(),
            factors: eval.factors.clone(),
            warning_badges: eval.warning_badges.clone(),
            rejection_reason: eval.rejection_reason.clone(),
            alternate_sources: eval.alternate_sources.clone(),
            advantages,
            disadvantages,
        });
    }

    let preference_explanation = if items.len() >= 2 {
        let first = &items[0];
        let second = &items[1];
        let mut reasons = Vec::new();
        if first.provider == "madokami" && second.provider != "madokami" {
            reasons.push("direct HTTP range transport over torrent swarm".to_string());
        }
        if first.edition == "Digital" && second.edition != "Digital" {
            reasons.push("official digital archival edition".to_string());
        }
        if first.total_score > second.total_score {
            reasons.push(format!("higher quality score ({:.0} vs {:.0})", first.total_score, second.total_score));
        }
        if reasons.is_empty() {
            format!("Candidate #1 ({}) is ranked above Candidate #2 ({}) based on overall release signals.", first.provider, second.provider)
        } else {
            format!(
                "Candidate #1 ({}) is preferred over Candidate #2 ({}) due to {}.",
                first.provider,
                second.provider,
                reasons.join(", ")
            )
        }
    } else if items.len() == 1 {
        format!("Single candidate available from {}.", items[0].provider)
    } else {
        "No candidates available for comparison.".to_string()
    };

    CandidateComparisonMatrix {
        series_id: series_id.to_string(),
        volume_number,
        candidates: items,
        preference_explanation,
        duplicate_groups_count,
    }
}
