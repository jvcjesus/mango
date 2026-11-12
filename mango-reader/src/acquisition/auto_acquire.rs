//! Guarded Auto-Acquisition Policies Subsystem (Milestone M6.8)
//!
//! Provides conservative, opt-in automatic acquisition policies for future physical
//! manga releases while preserving M6.4 as the SINGLE guarded acquisition executor.
//!
//! Invariants:
//! - Global master switch defaults strictly to OFF.
//! - Dry-run mode defaults strictly to ON.
//! - All titles default strictly to OFF.
//! - Per-title consent records activation watermark (`auto_acquire_activated_at`).
//! - Historical backlog releases (release date < activation date) NEVER auto-acquire.
//! - Candidate eligibility hard gates execute BEFORE score ranking.
//! - Multi-volume collections, ranges, series packs, and possible matches are NEVER auto-acquired.
//! - Komga ownership check fails closed (`OWNERSHIP_UNKNOWN`) if unreachable.
//! - Atomic database claim prevents concurrent schedulers / user races.
//! - Reuses `AcquisitionService::approve_candidate` directly; does not implement a parallel download pipeline.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::approval::{ApproveCandidateRequest, PersistedCandidate};
use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::release_discovery::{
    check_volume_correspondence, get_target_by_id, is_volume_owned,
};
use crate::acquisition::service::AcquisitionService;
use crate::db::{now, Db};
use crate::error::{AppError, Result};
use crate::pipeline::ownership::OwnershipStatus;

/// Global settings keys
pub const SETTING_AUTO_ACQUIRE_ENABLED: &str = "acquisition.auto_acquire_enabled";
pub const SETTING_AUTO_ACQUIRE_DRY_RUN: &str = "acquisition.auto_acquire_dry_run";
pub const SETTING_OPERATING_MODE: &str = "acquisition.operating_mode";
pub const SETTING_AUTO_PUBLISH_MANUAL: &str = "acquisition.auto_publish_manual_acquisitions";

/// Default safety bounds
pub const DEFAULT_MIN_SIZE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2.0 GB
pub const DEFAULT_MIN_SCORE: f64 = 50.0;

/// Structured reason codes for automatic acquisition policy decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AutoDecisionCode {
    AutoAllowed,
    WouldAutoAcquire,
    GlobalAutoDisabled,
    TitleAutoDisabled,
    HistoricalBacklogManualOnly,
    NotReleased,
    ReleasePrecisionUnsafe,
    MarketNotAllowed,
    EditionMismatch,
    MappingReviewRequired,
    Owned,
    OwnershipUnknown,
    AlreadyAcquiring,
    AlreadyAcquired,
    NoCandidate,
    NoEligibleCandidate,
    CandidateNotExact,
    RangeRequiresManual,
    PackRequiresManual,
    PossibleMatchRequiresManual,
    PreReleaseRequiresManual,
    AmbiguousTitleTokens,
    LanguageUnknown,
    FormatNotAllowed,
    SeedersBelowPolicy,
    SizeOutOfBounds,
    ScoreBelowThreshold,
    ProviderUnavailable,
    TorboxReconciliationRequired,
}

impl AutoDecisionCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AutoAllowed => "AUTO_ALLOWED",
            Self::WouldAutoAcquire => "WOULD_AUTO_ACQUIRE",
            Self::GlobalAutoDisabled => "GLOBAL_AUTO_DISABLED",
            Self::TitleAutoDisabled => "TITLE_AUTO_DISABLED",
            Self::HistoricalBacklogManualOnly => "HISTORICAL_BACKLOG_MANUAL_ONLY",
            Self::NotReleased => "NOT_RELEASED",
            Self::ReleasePrecisionUnsafe => "RELEASE_PRECISION_UNSAFE",
            Self::MarketNotAllowed => "MARKET_NOT_ALLOWED",
            Self::EditionMismatch => "EDITION_MISMATCH",
            Self::MappingReviewRequired => "MAPPING_REVIEW_REQUIRED",
            Self::Owned => "OWNED",
            Self::OwnershipUnknown => "OWNERSHIP_UNKNOWN",
            Self::AlreadyAcquiring => "ALREADY_ACQUIRING",
            Self::AlreadyAcquired => "ALREADY_ACQUIRED",
            Self::NoCandidate => "NO_CANDIDATE",
            Self::NoEligibleCandidate => "NO_ELIGIBLE_CANDIDATE",
            Self::CandidateNotExact => "CANDIDATE_NOT_EXACT",
            Self::RangeRequiresManual => "RANGE_REQUIRES_MANUAL",
            Self::PackRequiresManual => "PACK_REQUIRES_MANUAL",
            Self::PossibleMatchRequiresManual => "POSSIBLE_MATCH_REQUIRES_MANUAL",
            Self::PreReleaseRequiresManual => "PRE_RELEASE_REQUIRES_MANUAL",
            Self::AmbiguousTitleTokens => "AMBIGUOUS_TITLE_TOKENS",
            Self::LanguageUnknown => "LANGUAGE_UNKNOWN",
            Self::FormatNotAllowed => "FORMAT_NOT_ALLOWED",
            Self::SeedersBelowPolicy => "SEEDERS_BELOW_POLICY",
            Self::SizeOutOfBounds => "SIZE_OUT_OF_BOUNDS",
            Self::ScoreBelowThreshold => "SCORE_BELOW_THRESHOLD",
            Self::ProviderUnavailable => "PROVIDER_UNAVAILABLE",
            Self::TorboxReconciliationRequired => "TORBOX_RECONCILIATION_REQUIRED",
        }
    }
}

/// Result of evaluating a release target against auto-acquisition policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoAcquireDecision {
    pub target_id: i64,
    pub series_id: String,
    pub volume_number: f64,
    pub code: AutoDecisionCode,
    pub reason: String,
    pub candidate_id: Option<String>,
    pub candidate_title: Option<String>,
    pub candidate_score: Option<f64>,
    pub dry_run: bool,
    pub evaluated_at: i64,
}

/// Global settings query helper
pub fn is_global_auto_acquire_enabled(conn: &Connection) -> bool {
    crate::settings::get::<bool>(conn, SETTING_AUTO_ACQUIRE_ENABLED).unwrap_or(false)
}

/// Dry-run mode query helper (defaults strictly to true)
pub fn is_auto_acquire_dry_run(conn: &Connection) -> bool {
    crate::settings::get::<bool>(conn, SETTING_AUTO_ACQUIRE_DRY_RUN).unwrap_or(true)
}

/// Set global auto-acquire master switch
pub fn set_global_auto_acquire(conn: &Connection, enabled: bool) -> Result<()> {
    crate::settings::set(conn, SETTING_AUTO_ACQUIRE_ENABLED, &serde_json::json!(enabled))
}

/// Set global auto-acquire dry run mode
pub fn set_auto_acquire_dry_run(conn: &Connection, dry_run: bool) -> Result<()> {
    crate::settings::set(conn, SETTING_AUTO_ACQUIRE_DRY_RUN, &serde_json::json!(dry_run))
}

/// Operating Mode: "simulation" (default) or "normal"
pub fn get_operating_mode(conn: &Connection) -> String {
    crate::settings::get::<String>(conn, SETTING_OPERATING_MODE)
        .unwrap_or_else(|| "simulation".to_string())
}

/// Returns true if the system is in Simulation Mode (default, safe).
/// Fails closed to simulation mode if unknown or unset.
pub fn is_simulation_mode(conn: &Connection) -> bool {
    let mode = get_operating_mode(conn);
    if mode.eq_ignore_ascii_case("normal") {
        return false;
    }
    let auto_dry = crate::settings::get::<bool>(conn, SETTING_AUTO_ACQUIRE_DRY_RUN);
    if auto_dry == Some(false) {
        return false;
    }
    true
}

/// Set operating mode. Fails closed: only explicit "normal" enables Normal Mode.
pub fn set_operating_mode(conn: &Connection, mode: &str) -> Result<()> {
    let normalized = if mode.eq_ignore_ascii_case("normal") {
        "normal"
    } else {
        "simulation"
    };
    crate::settings::set(conn, SETTING_OPERATING_MODE, &serde_json::json!(normalized))?;
    crate::settings::set(conn, SETTING_AUTO_ACQUIRE_DRY_RUN, &serde_json::json!(normalized != "normal"))?;
    Ok(())
}

/// Auto-publish for manual acquisitions query helper (defaults strictly to false)
pub fn is_auto_publish_manual_enabled(conn: &Connection) -> bool {
    crate::settings::get::<bool>(conn, SETTING_AUTO_PUBLISH_MANUAL).unwrap_or(false)
}

/// Set auto-publish for manual acquisitions
pub fn set_auto_publish_manual_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    crate::settings::set(conn, SETTING_AUTO_PUBLISH_MANUAL, &serde_json::json!(enabled))
}

/// Get per-title auto-acquire opt-in state and activation timestamp
pub fn get_title_auto_acquire(conn: &Connection, manga_id: i64) -> Result<(bool, Option<i64>)> {
    let row: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT auto_acquire, auto_acquire_activated_at FROM manga WHERE id = ?1",
            [manga_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::internal(format!("Failed to get title auto_acquire: {e}")))?;

    if let Some((enabled, activated_at)) = row {
        Ok((enabled != 0, activated_at))
    } else {
        Ok((false, None))
    }
}

/// Set per-title auto-acquire opt-in state with activation watermark
pub fn set_title_auto_acquire(conn: &Connection, manga_id: i64, enabled: bool) -> Result<()> {
    let current_time = now();
    let activated_at = if enabled { Some(current_time) } else { None };

    // 1. Update manga table (authoritative per-title state)
    conn.execute(
        "UPDATE manga
         SET auto_acquire = ?1, auto_acquire_activated_at = ?2
         WHERE id = ?3",
        params![if enabled { 1 } else { 0 }, activated_at, manga_id],
    )?;

    // 2. Synchronize tracked_catalog if a matching row exists
    let _ = conn.execute(
        "UPDATE tracked_catalog
         SET auto_acquire = ?1, updated_at = ?2
         WHERE manga_id = ?3",
        params![if enabled { 1 } else { 0 }, current_time, manga_id],
    );

    Ok(())
}

/// Evaluate hard eligibility gates for a candidate release.
/// Hard gates MUST be satisfied before score ranking is permitted.
pub fn is_candidate_auto_eligible(
    cand: &PersistedCandidate,
    profile: &AcquisitionProfile,
) -> std::result::Result<(), AutoDecisionCode> {
    // Gate 1: Provider must be supported for automation (Nyaa or Madokami)
    if cand.provider != "nyaa" && cand.provider != "madokami" {
        return Err(AutoDecisionCode::ProviderUnavailable);
    }

    // Gate 2: Candidate match type MUST be ExactVolume (or ExactChapter when profile permits)
    match cand.match_type.as_str() {
        "exact_volume" => {}
        "exact_chapter" => {
            if profile.preferred_release_type != "chapter" && profile.preferred_release_type != "any" {
                return Err(AutoDecisionCode::CandidateNotExact);
            }
        }
        "volume_range" | "volume_range_containing_target" => {
            return Err(AutoDecisionCode::RangeRequiresManual);
        }
        "series_pack" => {
            return Err(AutoDecisionCode::PackRequiresManual);
        }
        "possible_match" => {
            return Err(AutoDecisionCode::PossibleMatchRequiresManual);
        }
        _ => {
            return Err(AutoDecisionCode::CandidateNotExact);
        }
    }

    // Gate 3: Parser hardening: check for ambiguous multi-volume, batch, or spinoff tokens
    let raw_lower = cand.raw_title.to_ascii_lowercase();
    let reject_tokens = [
        "omnibus", "2-in-1", "3-in-1", "4-in-1", "batch", "complete", "collection",
    ];
    for tok in reject_tokens {
        if raw_lower.contains(tok) {
            return Err(AutoDecisionCode::AmbiguousTitleTokens);
        }
    }
    for &marker in crate::acquisition::matching::SPINOFF_MARKERS {
        if raw_lower.contains(marker) {
            return Err(AutoDecisionCode::AmbiguousTitleTokens);
        }
    }

    // Gate 4: Language must match target profile (default English "en")
    if crate::acquisition::matching::detect_foreign_language(&cand.raw_title, &profile.language).is_some() {
        return Err(AutoDecisionCode::LanguageUnknown);
    }
    let non_en_tokens = ["french", "français", "spanish", "español", "raw", "japanese", "german", "deutsch", "italian", "italiano"];
    for tok in non_en_tokens {
        if raw_lower.contains(tok) && !raw_lower.contains("english") {
            return Err(AutoDecisionCode::LanguageUnknown);
        }
    }

    // Gate 5: Edition must be standard tankobon
    let edition = crate::acquisition::edition::edition_from_title(&cand.raw_title);
    let is_standard = edition.as_deref().map_or(true, |e| e == "standard");
    if !is_standard && !profile.allow_unsupported_editions {
        return Err(AutoDecisionCode::EditionMismatch);
    }

    // Gate 6: Format allowlist (CBZ or ZIP archive)
    if raw_lower.ends_with(".pdf")
        || raw_lower.ends_with(".epub")
        || raw_lower.ends_with(".rar")
        || raw_lower.ends_with(".cbr")
    {
        return Err(AutoDecisionCode::FormatNotAllowed);
    }

    // Gate 7: Provider-specific availability check
    if cand.provider == "nyaa" {
        let seeders = cand.seeders.unwrap_or(0);
        if seeders == 0 || seeders < profile.min_seeders {
            return Err(AutoDecisionCode::SeedersBelowPolicy);
        }
    } else if cand.provider == "madokami" {
        // Direct download URL must be present
        if cand.download_url.as_deref().unwrap_or("").trim().is_empty() {
            return Err(AutoDecisionCode::ProviderUnavailable);
        }
        // PRE releases are strictly ineligible for auto-acquisition
        if raw_lower.contains("(pre)") || raw_lower.contains("[pre]") || raw_lower.contains(" pre ") {
            return Err(AutoDecisionCode::PreReleaseRequiresManual);
        }
    }

    // Gate 8: Size bounds check
    if let Some(size) = cand.size_bytes {
        if size < DEFAULT_MIN_SIZE_BYTES {
            return Err(AutoDecisionCode::SizeOutOfBounds);
        }
        let max_size = profile
            .max_acquisition_size_bytes
            .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
        if size > max_size {
            return Err(AutoDecisionCode::SizeOutOfBounds);
        }
    }

    // Gate 9: Score threshold check
    if cand.score < DEFAULT_MIN_SCORE {
        return Err(AutoDecisionCode::ScoreBelowThreshold);
    }

    Ok(())
}

/// Evaluates a release discovery target against guarded auto-acquisition policies.
///
/// In dry-run mode (default), records WOULD_AUTO_ACQUIRE without contacting TorBox.
/// In live mode (dry-run OFF), claims the target atomically and invokes
/// `AcquisitionService::approve_candidate`.
pub async fn evaluate_target_for_auto_acquire(
    db: &Db,
    acquisition: &AcquisitionService,
    target_id: i64,
) -> Result<AutoAcquireDecision> {
    let cur_time = now();

    // 1. Fetch current target state
    let target = db
        .call(move |conn| get_target_by_id(conn, target_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Target {target_id} not found")))?;

    let series_id = target.series_id.clone();
    let volume_number = target.volume_number;
    let manga_id_opt = target.manga_id;
    let title = target.series_title.clone();

    // Gate 1: Global Master Switch
    let global_enabled = db.call(|conn| Ok(is_global_auto_acquire_enabled(conn))).await?;
    let dry_run = db.call(|conn| Ok(is_auto_acquire_dry_run(conn))).await?;

    if !global_enabled {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::GlobalAutoDisabled,
            reason: "Global automatic acquisition master switch is OFF".to_string(),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 2: Title-Level Opt-In
    let (title_enabled, activated_at_opt) = if let Some(mid) = manga_id_opt {
        db.call(move |conn| get_title_auto_acquire(conn, mid)).await?
    } else {
        (false, None)
    };

    if !title_enabled {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::TitleAutoDisabled,
            reason: "Auto-acquisition is not enabled for this title".to_string(),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 3: Mandatory Ownership Preflight (Komga & Local Canonical)
    // If a volume is already owned, it must never be reacquired regardless of release date.
    let s_id_own = series_id.clone();
    let is_locally_owned = db
        .call(move |conn| {
            if let Some(mid) = manga_id_opt {
                is_volume_owned(conn, mid, &s_id_own, volume_number)
            } else {
                Ok(false)
            }
        })
        .await?;

    if is_locally_owned {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::Owned,
            reason: format!("Volume {} is already owned locally or in Komga reconciliation", volume_number),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        db.call(move |conn| {
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'already_owned', updated_at = ?1
                 WHERE id = ?2",
                params![cur_time, target_id],
            )?;
            Ok(())
        })
        .await?;
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Live Komga Ownership Check (Fail-Closed)
    if let (Some(pipeline), Some(mid)) = (&acquisition.pipeline, manga_id_opt) {
        let title_own = pipeline.ownership.get_title_ownership(mid, &title).await;
        if title_own.volumes.iter().any(|vo| {
            (vo.volume - volume_number).abs() < 0.001 && vo.status == OwnershipStatus::Owned
        }) {
            let dec = AutoAcquireDecision {
                target_id,
                series_id: series_id.clone(),
                volume_number,
                code: AutoDecisionCode::Owned,
                reason: format!("Volume {} verified owned directly in Komga", volume_number),
                candidate_id: None,
                candidate_title: None,
                candidate_score: None,
                dry_run,
                evaluated_at: cur_time,
            };
            db.call(move |conn| {
                conn.execute(
                    "UPDATE release_discovery_targets
                     SET status = 'already_owned', updated_at = ?1
                     WHERE id = ?2",
                    params![cur_time, target_id],
                )?;
                Ok(())
            })
            .await?;
            persist_decision(db, target_id, &dec).await?;
            return Ok(dec);
        }
    }

    // Gate 4: Historical Backlog Protection
    // If target release date is before title activation date, strictly reject automation.
    if let Some(activated_at) = activated_at_opt {
        let activation_date: String = db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT date(?1, 'unixepoch', 'localtime')",
                        [activated_at],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| "9999-99-99".to_string()))
            })
            .await?;

        if target.release_date < activation_date {
            let dec = AutoAcquireDecision {
                target_id,
                series_id: series_id.clone(),
                volume_number,
                code: AutoDecisionCode::HistoricalBacklogManualOnly,
                reason: format!(
                    "Release date {} is prior to title auto-acquire activation date {}; manual review required",
                    target.release_date, activation_date
                ),
                candidate_id: None,
                candidate_title: None,
                candidate_score: None,
                dry_run,
                evaluated_at: cur_time,
            };
            persist_decision(db, target_id, &dec).await?;
            return Ok(dec);
        }
    }

    // Gate 5: Market & Edition
    if target.market != "en" {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::MarketNotAllowed,
            reason: format!("Market '{}' does not qualify for English auto-acquisition", target.market),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    if target.edition != "standard" {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::EditionMismatch,
            reason: format!("Edition '{}' requires manual confirmation", target.edition),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 6: Release Date Precision & Timing
    if target.release_date_precision != "day" {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::ReleasePrecisionUnsafe,
            reason: format!("Imprecise release date precision '{}'; day-precision required", target.release_date_precision),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    let today_str: String = db
        .call(|conn| {
            Ok(conn
                .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
                .unwrap_or_else(|_| "1970-01-01".to_string()))
        })
        .await?;

    if target.release_date > today_str {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::NotReleased,
            reason: format!("Release date {} has not arrived yet (local today: {})", target.release_date, today_str),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 7: Mapping & MangaBaka Volume Correspondence
    let s_id_corr = series_id.clone();
    let is_corresponded = db
        .call(move |conn| check_volume_correspondence(conn, &s_id_corr, volume_number))
        .await?;

    if !is_corresponded {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::MappingReviewRequired,
            reason: "MangaBaka volume correspondence or OpenTome series mapping unverified".to_string(),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 8: Active Acquisition Check
    let s_id_act = series_id.clone();
    let is_active = db
        .call(move |conn| {
            // Check downloads table for active jobs
            let active_dl: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM downloads d
                        JOIN chapters c ON c.id = d.chapter_id
                        WHERE c.manga_id = ?1 AND d.state NOT IN ('completed', 'failed', 'cancelled')
                          AND ABS(c.chapter_number - ?2) < 0.001
                    )",
                    params![manga_id_opt.unwrap_or(0), volume_number],
                    |r| r.get(0),
                )
                .unwrap_or(false);

            if active_dl {
                return Ok(true);
            }

            // Check candidate_approvals table
            let approved: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM candidate_approvals
                        WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001
                          AND status IN ('queued', 'approved', 'acquiring')
                    )",
                    params![s_id_act, volume_number],
                    |r| r.get(0),
                )
                .unwrap_or(false);

            Ok(approved)
        })
        .await?;

    if is_active {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::AlreadyAcquiring,
            reason: format!("Volume {} is already acquiring in active downloads queue", volume_number),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 8: Candidate Hard Filtering & Ranking
    let sid_cands = series_id.clone();
    let candidates = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT series_id, volume_number, candidate_id, provider, provider_item_id,
                        title, raw_title, match_type, size_bytes, uploader, is_trusted, is_digital,
                        score, score_explanation, download_url, magnet_url, info_hash,
                        seeders, leechers, published_at
                 FROM discovered_release_candidates
                 WHERE series_id = ?1 AND ABS(volume_number - ?2) < 0.001
                 ORDER BY score DESC",
            )?;

            let rows = stmt.query_map(params![sid_cands, volume_number], |r| {
                Ok(PersistedCandidate {
                    series_id: r.get(0)?,
                    volume_number: r.get(1)?,
                    candidate_id: r.get(2)?,
                    provider: r.get(3)?,
                    provider_item_id: r.get(4)?,
                    title: r.get(5)?,
                    raw_title: r.get(6)?,
                    match_type: r.get(7)?,
                    size_bytes: r.get(8)?,
                    uploader: r.get(9)?,
                    is_trusted: r.get::<_, i64>(10)? != 0,
                    is_digital: r.get::<_, i64>(11)? != 0,
                    score: r.get(12)?,
                    score_explanation: r.get(13)?,
                    download_url: r.get(14)?,
                    magnet_url: r.get(15)?,
                    info_hash: r.get(16)?,
                    seeders: r.get(17)?,
                    leechers: r.get(18)?,
                    published_at: r.get(19)?,
                })
            })?;

            let mut list = Vec::new();
            for r in rows {
                list.push(r?);
            }
            Ok(list)
        })
        .await?;

    if candidates.is_empty() {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::NoCandidate,
            reason: "No release candidates have been discovered for this volume".to_string(),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    let profile = if let Some(mid) = manga_id_opt {
        acquisition.get_manga_profile(mid).await?
    } else {
        AcquisitionProfile::default()
    };

    // Filter candidates through HARD ELIGIBILITY GATES
    let mut eligible_candidates = Vec::new();
    let mut last_rejection_code = AutoDecisionCode::NoEligibleCandidate;

    for cand in &candidates {
        match is_candidate_auto_eligible(cand, &profile) {
            Ok(()) => {
                eligible_candidates.push(cand.clone());
            }
            Err(code) => {
                last_rejection_code = code;
            }
        }
    }

    if eligible_candidates.is_empty() {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: last_rejection_code,
            reason: format!(
                "Candidate(s) found but failed hard auto-acquisition eligibility gates ({})",
                last_rejection_code.as_str()
            ),
            candidate_id: None,
            candidate_title: None,
            candidate_score: None,
            dry_run,
            evaluated_at: cur_time,
        };

        // Update target status to manual_review_required so user knows a release is available
        db.call(move |conn| {
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'manual_review_required', updated_at = ?1
                 WHERE id = ?2 AND status = 'candidate_available'",
                params![cur_time, target_id],
            )?;
            Ok(())
        })
        .await?;

        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Select top eligible candidate:
    // Provider priority (Madokami preferred direct download over Nyaa torrent),
    // then score, digital marker, and seeders.
    let provider_priority = |p: &str| match p.to_lowercase().as_str() {
        "madokami" => 0,
        "nyaa" => 1,
        _ => 2,
    };

    eligible_candidates.sort_by(|a, b| {
        provider_priority(&a.provider)
            .cmp(&provider_priority(&b.provider))
            .then_with(|| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.is_digital.cmp(&a.is_digital))
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
    });

    let selected = eligible_candidates[0].clone();
    let transport_desc = match selected.provider.as_str() {
        "madokami" => "Direct HTTP",
        "nyaa" => "TorBox",
        other => other,
    };
    let has_other_provider = eligible_candidates.iter().any(|c| c.provider != selected.provider);
    let provider_note = if selected.provider == "madokami" && has_other_provider {
        " (preferred direct provider over torrent)"
    } else if selected.provider == "nyaa" && has_other_provider {
        " (fallback provider)"
    } else {
        ""
    };

    // Gate 9: Dry-Run Mode
    if dry_run {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::WouldAutoAcquire,
            reason: format!(
                "Candidate '{}' [{} / {}]{} (score: {:.1}) passed all guarded auto-acquisition gates (dry-run mode active; zero external side effects)",
                selected.title, selected.provider, transport_desc, provider_note, selected.score
            ),
            candidate_id: Some(selected.candidate_id.clone()),
            candidate_title: Some(selected.title.clone()),
            candidate_score: Some(selected.score),
            dry_run: true,
            evaluated_at: cur_time,
        };
        persist_decision(db, target_id, &dec).await?;
        return Ok(dec);
    }

    // Gate 10: Live Execution (Dry-Run OFF)
    // Atomic Claim on release target
    let claim_success: bool = db
        .call(move |conn| {
            let changed = conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'auto_approved',
                     auto_decision_code = 'AUTO_ALLOWED',
                     auto_decision_reason = 'Candidate passed all guarded gates; submitting to acquisition executor',
                     auto_decision_at = ?1,
                     updated_at = ?1
                 WHERE id = ?2 AND status = 'candidate_available'",
                params![cur_time, target_id],
            )?;
            Ok(changed > 0)
        })
        .await?;

    if !claim_success {
        let dec = AutoAcquireDecision {
            target_id,
            series_id: series_id.clone(),
            volume_number,
            code: AutoDecisionCode::AlreadyAcquiring,
            reason: "Target claim lost to concurrent execution or user action".to_string(),
            candidate_id: Some(selected.candidate_id.clone()),
            candidate_title: Some(selected.title.clone()),
            candidate_score: Some(selected.score),
            dry_run: false,
            evaluated_at: cur_time,
        };
        return Ok(dec);
    }

    // Invoke M6.4 Guarded Acquisition Executor
    let mid = manga_id_opt.ok_or_else(|| AppError::bad_request("Manga ID missing for target"))?;
    let req = ApproveCandidateRequest {
        confirm_range: Some(false),
        confirm_pack: Some(false),
        confirm_review: Some(false),
        dry_run: None,
        authorization_token: None,
        user_id: None,
    };

    let resp = acquisition
        .approve_candidate(mid, volume_number, &selected.candidate_id, req)
        .await;

    match resp {
        Ok(res) => {
            let final_target_status = if res.already_owned {
                "already_owned"
            } else {
                "acquiring"
            };

            db.call(move |conn| {
                conn.execute(
                    "UPDATE release_discovery_targets
                     SET status = ?1, updated_at = ?2
                     WHERE id = ?3",
                    params![final_target_status, now(), target_id],
                )?;
                Ok(())
            })
            .await?;

            let dec = AutoAcquireDecision {
                target_id,
                series_id: series_id.clone(),
                volume_number,
                code: AutoDecisionCode::AutoAllowed,
                reason: format!(
                    "Candidate '{}' [{} / {}]{} successfully queued via M6.4 executor",
                    selected.title, selected.provider, transport_desc, provider_note
                ),
                candidate_id: Some(selected.candidate_id.clone()),
                candidate_title: Some(selected.title.clone()),
                candidate_score: Some(selected.score),
                dry_run: false,
                evaluated_at: cur_time,
            };
            persist_decision(db, target_id, &dec).await?;
            Ok(dec)
        }
        Err(e) => {
            // Revert target status from auto_approved to candidate_available on failure
            let err_msg = e.to_string();
            let err_clone = err_msg.clone();
            db.call(move |conn| {
                conn.execute(
                    "UPDATE release_discovery_targets
                     SET status = 'candidate_available',
                         last_error = ?1,
                         auto_decision_code = 'TORBOX_RECONCILIATION_REQUIRED',
                         auto_decision_reason = ?1,
                         updated_at = ?2
                     WHERE id = ?3",
                    params![err_clone, now(), target_id],
                )?;
                Ok(())
            })
            .await?;

            Err(AppError::internal(format!("Auto-acquisition execution failed: {err_msg}")))
        }
    }
}

/// Helper to persist decision code and reason to target
async fn persist_decision(db: &Db, target_id: i64, dec: &AutoAcquireDecision) -> Result<()> {
    let code_str = dec.code.as_str().to_string();
    let reason_str = dec.reason.clone();
    let at = dec.evaluated_at;

    db.call(move |conn| {
        conn.execute(
            "UPDATE release_discovery_targets
             SET auto_decision_code = ?1,
                 auto_decision_reason = ?2,
                 auto_decision_at = ?3
             WHERE id = ?4",
            params![code_str, reason_str, at, target_id],
        )?;
        Ok(())
    })
    .await
}
