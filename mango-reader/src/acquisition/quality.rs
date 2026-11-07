//! Candidate Quality Intelligence & Release Selection (M6.11 / M8.4).
//!
//! Provides multi-factor quality signals, explainable scoring, provider preference
//! policy (Madokami direct HTTP preferred over Nyaa torrents), deduplication across
//! sources, side-by-side comparison matrix, and strict no-silent-upgrade protections.

use serde::{Deserialize, Serialize};

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::providers::is_known_quality_group;
use crate::acquisition::torbox::TorBoxCacheStatus;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardEligibilityResult {
    pub is_eligible: bool,
    pub rejection_code: Option<String>,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingBreakdown {
    pub final_score: f64,
    pub quality_score: f64,
    pub reliability_score: f64,
    pub preference_score: f64,
    pub convenience_score: f64,
    pub storage_efficiency_score: f64,
    pub confidence_percent: u32,
    pub is_provisional: bool,
    pub explanation: String,
    pub why_ranks_above: Option<String>,
    pub missing_evidence: Vec<String>,
    pub evidence_coverage_percent: u32,
    #[serde(default)]
    pub verified_evidence: Vec<String>,
    #[serde(default)]
    pub heuristic_estimates: Vec<String>,
}

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
    #[serde(default)]
    pub cache_status: Option<TorBoxCacheStatus>,
    #[serde(default)]
    pub ranking_breakdown: Option<RankingBreakdown>,
    #[serde(default)]
    pub hard_eligibility: Option<HardEligibilityResult>,
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
    #[serde(default)]
    pub cache_status: Option<TorBoxCacheStatus>,
    #[serde(default)]
    pub ranking_breakdown: Option<RankingBreakdown>,
    #[serde(default)]
    pub hard_eligibility: Option<HardEligibilityResult>,
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

/// Computes explainable normalized 0..100 two-stage candidate score:
/// Final Score = 0.35 × Quality Evidence + 0.25 × Acquisition Reliability + 0.20 × Preference Match + 0.15 × Acquisition Convenience + 0.05 × Storage Efficiency
pub fn compute_ranking_breakdown(
    cand: &PersistedCandidate,
    cache_status: Option<TorBoxCacheStatus>,
    is_pre: bool,
    is_revision: bool,
    is_quality_grp: bool,
    resolution: Option<&str>,
    format: &str,
    edition: &str,
) -> RankingBreakdown {
    let mut verified_evidence = Vec::new();
    let mut heuristic_estimates = Vec::new();
    let mut missing_evidence = Vec::new();

    let mut verified_dims = 0u32;
    let total_dims = 5u32;

    // 1. Quality Evidence (0..100, weight 0.35)
    let mut quality_score: f64 = 0.0;
    if cand.is_digital || cand.raw_title.to_ascii_lowercase().contains("digital") {
        quality_score += 40.0;
        heuristic_estimates.push("Digital release indicated by provider metadata (internal archive image fidelity unverified pre-download)".to_string());
    } else {
        missing_evidence.push("Non-digital or unverified rip (archive contents uninspected)".to_string());
    }

    if cand.is_trusted || is_quality_grp {
        quality_score += 30.0;
        heuristic_estimates.push("Release group reputation / trusted flag (heuristic observation, not archive checksum verification)".to_string());
    }

    if let Some(res) = resolution {
        if res.contains("2048") {
            quality_score += 15.0;
        } else if res.contains("1920") || res.contains("1600") {
            quality_score += 10.0;
        } else {
            quality_score += 5.0;
        }
        heuristic_estimates.push(format!("Resolution label '{}' estimated from release title", res));
    }

    if format == "CBZ" {
        quality_score += 10.0;
        verified_evidence.push("Standard CBZ container verified by filename extension".to_string());
        verified_dims += 1;
    } else if format == "ZIP" || format == "CBR" {
        quality_score += 5.0;
        heuristic_estimates.push(format!("Container format '{}' may require conversion to CBZ", format));
    } else {
        missing_evidence.push(format!("Non-standard container format '{}'", format));
    }

    if is_revision {
        quality_score += 5.0;
        heuristic_estimates.push("Revision tag (v2/remake) detected in title".to_string());
    }

    let quality_score = quality_score.clamp(0.0, 100.0);
    missing_evidence.push("Archive image compression, internal dimensions, and CBZ integrity uninspected prior to download".to_string());

    // 2. Acquisition Reliability (0..100, weight 0.25)
    let mut is_provisional = false;
    let reliability_score: f64 = if cand.provider.eq_ignore_ascii_case("madokami") {
        // Madokami telemetry: When live server historical telemetry is unmeasured, label provisional
        is_provisional = true;
        heuristic_estimates.push("Madokami direct HTTP reliability assumed provisional (no historical volume telemetry)".to_string());
        95.0
    } else if cand.provider.eq_ignore_ascii_case("nyaa") {
        match cache_status {
            Some(TorBoxCacheStatus::Cached) => {
                verified_evidence.push("TorBox debrid instant cache verified via API".to_string());
                verified_dims += 1;
                95.0
            }
            Some(TorBoxCacheStatus::Uncached) | Some(TorBoxCacheStatus::Unknown) | None => {
                is_provisional = true;
                match cand.seeders {
                    Some(s) if s >= 20 => {
                        heuristic_estimates.push(format!("Active swarm health estimated from provider seeder count ({s} seeders)"));
                        80.0
                    }
                    Some(s) if s >= 5 => {
                        heuristic_estimates.push(format!("Moderate swarm health estimated from provider seeder count ({s} seeders)"));
                        60.0
                    }
                    Some(s) if s >= 1 => {
                        heuristic_estimates.push(format!("Low swarm health estimated from provider seeder count ({s} seeders)"));
                        35.0
                    }
                    Some(0) => {
                        missing_evidence.push("P2P swarm has 0 reported seeders".to_string());
                        5.0
                    }
                    _ => {
                        heuristic_estimates.push("Swarm seeder count unknown (provisional estimate)".to_string());
                        40.0
                    }
                }
            }
            Some(TorBoxCacheStatus::Unavailable) => {
                missing_evidence.push("TorBox service unavailable or unconfigured".to_string());
                15.0
            }
        }
    } else {
        is_provisional = true;
        50.0
    };

    // 3. Preference Match (0..100, weight 0.20)
    let mut preference_score: f64 = 0.0;
    if detect_unwanted_language(&cand.raw_title).is_none() {
        preference_score += 50.0;
        verified_evidence.push("English language verified by absence of foreign language tags".to_string());
        verified_dims += 1;
    }
    if edition == "Digital" || edition == "Standard" {
        preference_score += 30.0;
    }
    match cand.match_type.as_str() {
        "exact_match" | "exact_volume" => {
            preference_score += 20.0;
            verified_evidence.push(format!("Exact volume {:.0} match verified from release title", cand.volume_number));
            verified_dims += 1;
        }
        "volume_range" | "volume_range_containing_target" => {
            preference_score += 5.0;
            heuristic_estimates.push("Multi-volume range candidate (requires volume extraction)".to_string());
        }
        "series_pack" => {
            preference_score += 5.0;
            heuristic_estimates.push("Series pack candidate (requires volume extraction)".to_string());
        }
        _ => {}
    }
    let preference_score = preference_score.clamp(0.0, 100.0);

    // 4. Acquisition Convenience (0..100, weight 0.15)
    let convenience_score: f64 = if cand.provider.eq_ignore_ascii_case("madokami") {
        90.0
    } else if cache_status == Some(TorBoxCacheStatus::Cached) {
        85.0
    } else if cand.seeders.unwrap_or(0) > 0 {
        50.0
    } else if format == "CBR" || format == "PDF" {
        20.0
    } else {
        10.0
    };

    // 5. Storage Efficiency (0..100, weight 0.05)
    let storage_efficiency_score: f64 = if let Some(bytes) = cand.size_bytes {
        if bytes >= 100_000_000 && bytes <= 350_000_000 {
            verified_evidence.push(format!("Release payload size ({:.1} MB) verified within optimal volume range", (bytes as f64) / 1e6));
            verified_dims += 1;
            100.0
        } else if (bytes >= 50_000_000 && bytes < 100_000_000)
            || (bytes > 350_000_000 && bytes <= 500_000_000)
        {
            verified_evidence.push(format!("Release payload size ({:.1} MB) verified within acceptable volume range", (bytes as f64) / 1e6));
            verified_dims += 1;
            80.0
        } else if bytes > 500_000_000 && bytes <= 800_000_000 {
            heuristic_estimates.push(format!("Release size ({:.1} MB) larger than normal volume range", (bytes as f64) / 1e6));
            60.0
        } else if bytes > 800_000_000 && bytes <= 1_200_000_000 {
            heuristic_estimates.push(format!("Release size ({:.1} MB) significantly exceeds normal volume range", (bytes as f64) / 1e6));
            40.0
        } else if bytes > 1_200_000_000 {
            missing_evidence.push(format!(
                "Archive size ({:.1} GB) exceeds standard volume boundary",
                (bytes as f64) / 1e9
            ));
            15.0
        } else {
            10.0
        }
    } else {
        missing_evidence.push("Release size unknown".to_string());
        50.0
    };

    let mut final_score: f64 = (0.35 * quality_score)
        + (0.25 * reliability_score)
        + (0.20 * preference_score)
        + (0.15 * convenience_score)
        + (0.05 * storage_efficiency_score);

    if is_pre {
        final_score = (final_score - 15.0).max(0.0);
    }
    let final_score = (final_score * 10.0).round() / 10.0;

    let evidence_coverage_percent = ((verified_dims as f64 / total_dims as f64) * 100.0).round() as u32;

    let mut confidence_percent = 70u32;
    if cand.match_type == "exact_match" || cand.match_type == "exact_volume" {
        confidence_percent += 10;
    }
    if cand.size_bytes.is_some() {
        confidence_percent += 5;
    }
    if cand.provider.eq_ignore_ascii_case("madokami") || cache_status.is_some() {
        confidence_percent += 5;
    }
    if cand.is_digital {
        confidence_percent += 5;
    }
    if is_pre {
        confidence_percent = confidence_percent.saturating_sub(25);
    }
    // Honest confidence cap: prior to download and archive validation, confidence is capped at 80%
    let confidence_percent = confidence_percent.min(80);

    let explanation = format!(
        "Normalized Score: {:.1}/100 [Quality: {:.0} (35%), Reliability: {:.0} (25%), Preference: {:.0} (20%), Convenience: {:.0} (15%), Storage: {:.0} (5%)] · Confidence: {}% (Evidence coverage: {}%)",
        final_score, quality_score, reliability_score, preference_score, convenience_score, storage_efficiency_score, confidence_percent, evidence_coverage_percent
    );

    RankingBreakdown {
        final_score,
        quality_score,
        reliability_score,
        preference_score,
        convenience_score,
        storage_efficiency_score,
        confidence_percent,
        is_provisional,
        explanation,
        why_ranks_above: None,
        missing_evidence,
        evidence_coverage_percent,
        verified_evidence,
        heuristic_estimates,
    }
}

/// Evaluates hard candidate eligibility gates before scoring.
pub fn evaluate_hard_eligibility(
    cand: &PersistedCandidate,
    is_owned: bool,
    is_active_acquiring: bool,
) -> HardEligibilityResult {
    if is_owned {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("ALREADY_OWNED".to_string()),
            rejection_reason: Some(format!(
                "Volume {:.0} is already verified owned in Komga library",
                cand.volume_number
            )),
        };
    }
    if is_active_acquiring {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("ALREADY_ACQUIRING".to_string()),
            rejection_reason: Some(format!(
                "Volume {:.0} is already active in download queue",
                cand.volume_number
            )),
        };
    }

    let raw_lower = cand.raw_title.to_ascii_lowercase();
    let is_chapter_only = cand.match_type == "chapter"
        || ((raw_lower.contains("ch.")
            || raw_lower.contains("chapter ")
            || raw_lower.contains(" c0")
            || raw_lower.contains(" c1"))
            && !raw_lower.contains("vol")
            && !raw_lower.contains("v0")
            && !raw_lower.contains("v1"));

    if is_chapter_only {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("CHAPTER_ONLY".to_string()),
            rejection_reason: Some("Chapter-only release is not a full volume".to_string()),
        };
    }

    if cand.match_type == "volume_range"
        || cand.match_type == "volume_range_containing_target"
    {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("RANGE_REQUIRES_MANUAL".to_string()),
            rejection_reason: Some(
                "Multi-volume range candidate requires manual confirmation".to_string(),
            ),
        };
    }

    if cand.match_type == "series_pack" {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("PACK_REQUIRES_MANUAL".to_string()),
            rejection_reason: Some(
                "Series pack candidate requires manual confirmation".to_string(),
            ),
        };
    }

    if let Some(foreign_lang) = detect_unwanted_language(&cand.raw_title) {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("UNWANTED_LANGUAGE".to_string()),
            rejection_reason: Some(format!(
                "Language detected as {foreign_lang} (expected English)"
            )),
        };
    }

    if cand.match_type == "rejected" {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("REJECTED".to_string()),
            rejection_reason: Some("Candidate was marked as rejected".to_string()),
        };
    }

    if cand.provider.eq_ignore_ascii_case("nyaa") && cand.seeders == Some(0) {
        return HardEligibilityResult {
            is_eligible: false,
            rejection_code: Some("DEAD_SWARM".to_string()),
            rejection_reason: Some(
                "Torrent swarm has 0 active seeders and cannot be downloaded".to_string(),
            ),
        };
    }

    HardEligibilityResult {
        is_eligible: true,
        rejection_code: None,
        rejection_reason: None,
    }
}

/// Evaluates multi-factor quality intelligence for a candidate release with full context.
pub fn evaluate_candidate_quality_v2(
    cand: &PersistedCandidate,
    is_owned: bool,
    cache_status: Option<TorBoxCacheStatus>,
    is_active_acquiring: bool,
    mut rejection_reason: Option<String>,
) -> CandidateQualityEvaluation {
    let hard_elig = evaluate_hard_eligibility(cand, is_owned, is_active_acquiring);
    if !hard_elig.is_eligible && rejection_reason.is_none() {
        rejection_reason = hard_elig.rejection_reason.clone();
    }

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

    let mut warning_badges = Vec::new();
    if !hard_elig.is_eligible {
        if let Some(ref code) = hard_elig.rejection_code {
            warning_badges.push(format!("Blocked: {code}"));
        }
    }
    let is_chapter_only = cand.match_type == "chapter"
        || ((raw_lower.contains("ch.")
            || raw_lower.contains("chapter ")
            || raw_lower.contains(" c0")
            || raw_lower.contains(" c1"))
            && !raw_lower.contains("vol")
            && !raw_lower.contains("v0")
            && !raw_lower.contains("v1"));
    if is_chapter_only {
        warning_badges.push("Chapter only".to_string());
    }
    if let Some(lang) = detect_unwanted_language(&cand.raw_title) {
        warning_badges.push(format!("Language: {lang}"));
    }
    if is_pre {
        warning_badges.push("Pre-release".to_string());
    }
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
    if let Some(bytes) = cand.size_bytes {
        if bytes > 1_200_000_000 {
            warning_badges.push("Size anomaly".to_string());
            if rejection_reason.is_none() {
                rejection_reason = Some(format!(
                    "Size {:.1} GB exceeds expected volume size (150-300 MB)",
                    (bytes as f64) / 1e9
                ));
            }
        }
    }

    let is_preferred_provider = cand.provider.eq_ignore_ascii_case("madokami");
    let transport = if is_preferred_provider {
        "Direct HTTP (Range resume)".to_string()
    } else if cache_status == Some(TorBoxCacheStatus::Cached) {
        "TorBox BitTorrent (Cached Instant)".to_string()
    } else {
        "TorBox BitTorrent (Debrid)".to_string()
    };

    let health_rating = if is_preferred_provider {
        "EXCELLENT".to_string()
    } else if cache_status == Some(TorBoxCacheStatus::Cached) {
        "EXCELLENT".to_string()
    } else {
        match cand.seeders {
            Some(s) if s >= 20 => "EXCELLENT".to_string(),
            Some(s) if s >= 5 => "GOOD".to_string(),
            Some(s) if s >= 1 => "DEGRADED".to_string(),
            Some(0) => "DEAD".to_string(),
            _ => "UNKNOWN".to_string(),
        }
    };

    let ranking_breakdown = compute_ranking_breakdown(
        cand,
        cache_status,
        is_pre,
        is_revision,
        is_quality_grp,
        resolution.as_deref(),
        &format,
        &edition,
    );

    let total_score = ranking_breakdown.final_score;

    let mut factors = vec![
        QualityFactorScore {
            name: "Quality Evidence (35%)".to_string(),
            points: (ranking_breakdown.quality_score * 0.35 * 10.0).round() / 10.0,
            is_bonus: ranking_breakdown.quality_score >= 50.0,
            description: format!(
                "Intrinsic release quality score: {:.0}/100 (wt 35%)",
                ranking_breakdown.quality_score
            ),
        },
        QualityFactorScore {
            name: "Acquisition Reliability (25%)".to_string(),
            points: (ranking_breakdown.reliability_score * 0.25 * 10.0).round() / 10.0,
            is_bonus: ranking_breakdown.reliability_score >= 50.0,
            description: format!(
                "Source reliability and health score: {:.0}/100 (wt 25%)",
                ranking_breakdown.reliability_score
            ),
        },
        QualityFactorScore {
            name: "Preference Match (20%)".to_string(),
            points: (ranking_breakdown.preference_score * 0.20 * 10.0).round() / 10.0,
            is_bonus: ranking_breakdown.preference_score >= 50.0,
            description: format!(
                "Edition & language match score: {:.0}/100 (wt 20%)",
                ranking_breakdown.preference_score
            ),
        },
        QualityFactorScore {
            name: "Acquisition Convenience (15%)".to_string(),
            points: (ranking_breakdown.convenience_score * 0.15 * 10.0).round() / 10.0,
            is_bonus: ranking_breakdown.convenience_score >= 50.0,
            description: format!(
                "Transport convenience score: {:.0}/100 (wt 15%)",
                ranking_breakdown.convenience_score
            ),
        },
        QualityFactorScore {
            name: "Storage Efficiency (5%)".to_string(),
            points: (ranking_breakdown.storage_efficiency_score * 0.05 * 10.0).round() / 10.0,
            is_bonus: ranking_breakdown.storage_efficiency_score >= 50.0,
            description: format!(
                "File size efficiency score: {:.0}/100 (wt 5%)",
                ranking_breakdown.storage_efficiency_score
            ),
        },
    ];

    let raw_lower = cand.raw_title.to_ascii_lowercase();
    let is_chapter_only = cand.match_type == "chapter"
        || ((raw_lower.contains("ch.")
            || raw_lower.contains("chapter ")
            || raw_lower.contains(" c0")
            || raw_lower.contains(" c1"))
            && !raw_lower.contains("vol")
            && !raw_lower.contains("v0")
            && !raw_lower.contains("v1"));
    if is_chapter_only {
        factors.push(QualityFactorScore {
            name: "Match Type".to_string(),
            points: -50.0,
            is_bonus: false,
            description: "Chapter-only penalty (-50)".to_string(),
        });
    }

    if let Some(foreign_lang) = detect_unwanted_language(&cand.raw_title) {
        factors.push(QualityFactorScore {
            name: "Unwanted Language".to_string(),
            points: -50.0,
            is_bonus: false,
            description: format!("Unwanted language penalty: {foreign_lang} (-50)"),
        });
    }

    if let Some(bytes) = cand.size_bytes {
        if bytes > 1_200_000_000 {
            let size_gb = (bytes as f64) / (1024.0 * 1024.0 * 1024.0);
            factors.push(QualityFactorScore {
                name: "Extreme Size Anomaly".to_string(),
                points: -30.0,
                is_bonus: false,
                description: format!(
                    "Extreme size anomaly penalty: {:.1} GB exceeds expected volume size (-30)",
                    size_gb
                ),
            });
        }
    }

    if is_pre {
        factors.push(QualityFactorScore {
            name: "Pre-Release Penalty".to_string(),
            points: -20.0,
            is_bonus: false,
            description: "Pre-release or preliminary rip penalty (-20)".to_string(),
        });
    }

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
            silent_upgrade_blocked: true,
        }
    } else {
        UpgradeEvaluation {
            is_currently_owned: false,
            upgrade_eligible: false,
            upgrade_justification: None,
            silent_upgrade_blocked: false,
        }
    };

    let selection_verdict = if is_owned {
        if upgrade_evaluation.upgrade_eligible {
            "MANUAL_UPGRADE_ONLY".to_string()
        } else {
            "ALREADY_OWNED".to_string()
        }
    } else if rejection_reason.is_some() || !hard_elig.is_eligible {
        "BLOCKED".to_string()
    } else if is_preferred_provider {
        "RECOMMENDED".to_string()
    } else {
        "ALTERNATIVE".to_string()
    };

    CandidateQualityEvaluation {
        candidate_id: cand.candidate_id.clone(),
        provider: cand.provider.clone(),
        transport,
        total_score,
        rank: 0,
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
        cache_status,
        ranking_breakdown: Some(ranking_breakdown),
        hard_eligibility: Some(hard_elig),
    }
}

/// Evaluates multi-factor quality intelligence for a candidate release.
pub fn evaluate_candidate_quality(
    cand: &PersistedCandidate,
    is_owned: bool,
    rejection_reason: Option<String>,
) -> CandidateQualityEvaluation {
    evaluate_candidate_quality_v2(cand, is_owned, None, false, rejection_reason)
}

/// Ranks a collection of evaluated candidates by quality and preference:
/// 1. Eligible > Ineligible
/// 2. Not Owned > Owned
/// 3. Higher normalized two-stage quality score
/// 4. Digital > Non-digital
/// 5. Non-PRE > PRE
pub fn rank_quality_evaluations(evals: &mut [CandidateQualityEvaluation]) {
    evals.sort_by(|a, b| {
        let a_ineligible = a.rejection_reason.is_some()
            || a.hard_eligibility.as_ref().map_or(false, |h| !h.is_eligible);
        let b_ineligible = b.rejection_reason.is_some()
            || b.hard_eligibility.as_ref().map_or(false, |h| !h.is_eligible);
        a_ineligible
            .cmp(&b_ineligible)
            .then_with(|| a.upgrade_evaluation.is_currently_owned.cmp(&b.upgrade_evaluation.is_currently_owned))
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
        let is_ineligible = eval.rejection_reason.is_some()
            || eval.hard_eligibility.as_ref().map_or(false, |h| !h.is_eligible);

        if is_ineligible {
            eval.selection_verdict = "BLOCKED".to_string();
        } else if eval.upgrade_evaluation.is_currently_owned {
            if eval.upgrade_evaluation.upgrade_eligible {
                eval.selection_verdict = "MANUAL_UPGRADE_ONLY".to_string();
            } else {
                eval.selection_verdict = "ALREADY_OWNED".to_string();
            }
        } else if idx == 0 {
            eval.selection_verdict = "RECOMMENDED".to_string();
        } else {
            eval.selection_verdict = "ALTERNATIVE".to_string();
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
            cache_status: eval.cache_status,
            ranking_breakdown: eval.ranking_breakdown.clone(),
            hard_eligibility: eval.hard_eligibility.clone(),
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
            reasons.push(format!("higher normalized quality score ({:.1} vs {:.1})", first.total_score, second.total_score));
        }
        if reasons.is_empty() {
            format!("Candidate #1 ({}) is recommended and preferred over Candidate #2 ({}) based on overall release signals.", first.provider, second.provider)
        } else {
            format!(
                "Candidate #1 ({}) is recommended and preferred over Candidate #2 ({}) due to {}.",
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
