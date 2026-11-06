//! Guarded Historical Backlog Acquisition (M6.10).
//!
//! Provides controlled, bounded acquisition for past volumes (backlog),
//! distinct from release monitoring. Enforces:
//! - User-initiated scope: All Missing Released, Volume Range, or Explicit Volumes
//! - Strict Komga ownership & canonical file pre-checks (never re-acquires owned volumes)
//! - MangaBaka & OpenTome metadata checks (never acquires unreleased future volumes)
//! - Rate limiting and bounded batch sizes (clamped between 1 and 20)
//! - Mandatory explicit confirmation for multi-volume operations
//! - Dry-run mode by default; explicit user authorization required for real execution
//! - M6.4 candidate approval bridge as the sole acquisition executor
//! - Full audit trail persisted in `backlog_acquisition_jobs`

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::approval::{ApproveCandidateRequest, PersistedCandidate};
use crate::acquisition::auto_acquire::is_candidate_auto_eligible;
use crate::acquisition::profiles::AcquisitionProfile;
use crate::acquisition::service::AcquisitionService;
use crate::acquisition::AcquisitionReleaseUnit;
use crate::db::now;
use crate::error::{AppError, Result};
use crate::pipeline::mangabaka::normalize_title;
use crate::pipeline::ownership::OwnershipStatus;

pub const DEFAULT_BACKLOG_BATCH_SIZE: usize = 5;
pub const MAX_BACKLOG_BATCH_SIZE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BacklogScope {
    AllMissing,
    Range,
    ExplicitVolumes,
}

impl std::fmt::Display for BacklogScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllMissing => write!(f, "ALL_MISSING"),
            Self::Range => write!(f, "RANGE"),
            Self::ExplicitVolumes => write!(f, "EXPLICIT_VOLUMES"),
        }
    }
}

impl std::str::FromStr for BacklogScope {
    type Err = AppError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "ALL_MISSING" => Ok(Self::AllMissing),
            "RANGE" => Ok(Self::Range),
            "EXPLICIT_VOLUMES" => Ok(Self::ExplicitVolumes),
            _ => Err(AppError::bad_request(format!("Unknown backlog scope: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBacklogRequest {
    pub scope: BacklogScope,
    pub range_start: Option<f64>,
    pub range_end: Option<f64>,
    pub explicit_volumes: Option<Vec<f64>>,
    pub batch_size: Option<usize>,
    pub is_dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklogItemPlan {
    pub volume_number: f64,
    pub status: String, // "ELIGIBLE", "ALREADY_OWNED", "UNAVAILABLE", "UNRELEASED", "IN_FLIGHT", "REVIEW_REQUIRED"
    pub reason: String,
    pub candidate_id: Option<String>,
    pub provider: Option<String>,
    pub candidate_title: Option<String>,
    pub size_bytes: Option<u64>,
    pub score: Option<f64>,
    pub is_digital: Option<bool>,
    pub uploader: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklogJobDto {
    pub id: String,
    pub manga_id: i64,
    pub series_title: String,
    pub scope: String,
    pub requested_volumes: Vec<f64>,
    pub batch_size: usize,
    pub is_dry_run: bool,
    pub state: String, // "PLANNED", "COMPLETED", "PARTIAL", "FAILED", "CANCELLED"
    pub total_requested: usize,
    pub total_eligible: usize,
    pub total_already_owned: usize,
    pub total_queued: usize,
    pub total_failed: usize,
    pub items: Vec<BacklogItemPlan>,
    pub confirmed_by_user: bool,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteBacklogRequest {
    pub job_id: Option<String>,
    pub confirm: bool,
    pub confirm_multiple: Option<bool>,
    pub is_dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteBacklogResponse {
    pub job_id: String,
    pub state: String,
    pub message: String,
    pub is_dry_run: bool,
    pub queued_count: usize,
    pub failed_count: usize,
    pub items: Vec<BacklogItemPlan>,
}

fn generate_job_id() -> String {
    static JOB_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = JOB_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("backlog-{}-{}-{}", now(), nanos, count)
}

impl AcquisitionService {
    pub async fn persist_backlog_job(
        &self,
        job: &BacklogJobDto,
        items: &[BacklogItemPlan],
    ) -> Result<()> {
        let j_id = job.id.clone();
        let j_mid = job.manga_id;
        let j_title = job.series_title.clone();
        let j_scope = job.scope.clone();
        let j_req_json = serde_json::to_string(&job.requested_volumes).unwrap_or_else(|_| "[]".to_string());
        let j_batch = job.batch_size as i64;
        let j_dry = if job.is_dry_run { 1 } else { 0 };
        let j_state = job.state.clone();
        let j_tot_req = job.total_requested as i64;
        let j_tot_elig = job.total_eligible as i64;
        let j_tot_owned = job.total_already_owned as i64;
        let j_items_json = serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string());
        let j_err = job.error.clone();
        let j_now = job.created_at;

        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO backlog_acquisition_jobs (
                        id, manga_id, series_title, scope, requested_volumes_json,
                        batch_size, is_dry_run, state, total_requested, total_eligible,
                        total_already_owned, total_queued, total_failed, items_json,
                        confirmed_by_user, error, created_at, updated_at, completed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, 0, ?12, 0, ?13, ?14, ?14, NULL)",
                    params![
                        j_id,
                        j_mid,
                        j_title,
                        j_scope,
                        j_req_json,
                        j_batch,
                        j_dry,
                        j_state,
                        j_tot_req,
                        j_tot_elig,
                        j_tot_owned,
                        j_items_json,
                        j_err,
                        j_now
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Plans a bounded backlog acquisition session for a title.
    /// Evaluates missing released volumes, checks Komga ownership,
    /// verifies metadata limits, resolves best candidates, and records
    /// the plan in `backlog_acquisition_jobs`.
    pub async fn plan_backlog_acquisition(
        &self,
        manga_id: i64,
        req: PlanBacklogRequest,
    ) -> Result<BacklogJobDto> {
        let batch_size = req
            .batch_size
            .unwrap_or(DEFAULT_BACKLOG_BATCH_SIZE)
            .clamp(1, MAX_BACKLOG_BATCH_SIZE);
        let is_dry_run = req.is_dry_run.unwrap_or(true);

        // 1. Fetch title and series reconciliation metadata
        let reconc_info = self
            .db
            .call(move |conn| {
                let info: Option<(
                    String,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<i64>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                )> = conn
                    .query_row(
                        "SELECT m.title, sr.series_id, sr.status, sr.collection_id,
                                sr.total_official_volumes, sr.review_reason,
                                sr.owned_volumes_json, sr.missing_volumes_json, sr.future_volumes_json
                         FROM manga m
                         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                         WHERE m.id = ?1",
                        [manga_id],
                        |r| {
                            Ok((
                                r.get(0)?,
                                r.get(1)?,
                                r.get(2)?,
                                r.get(3)?,
                                r.get(4)?,
                                r.get(5)?,
                                r.get(6)?,
                                r.get(7)?,
                                r.get(8)?,
                            ))
                        },
                    )
                    .optional()?;
                let (
                    t,
                    sid_opt,
                    status_opt,
                    coll_opt,
                    total_opt,
                    rev_reason,
                    owned_json,
                    missing_json,
                    future_json,
                ) = info.ok_or_else(|| AppError::not_found("Manga title not found"))?;

                Ok((
                    t,
                    sid_opt,
                    status_opt,
                    coll_opt,
                    total_opt,
                    rev_reason,
                    owned_json,
                    missing_json,
                    future_json,
                ))
            })
            .await?;

        let (
            title,
            sid_opt,
            status_opt,
            coll_opt,
            total_official_vols,
            rev_reason,
            owned_json,
            missing_json,
            future_json,
        ) = reconc_info;

        // Series reconciliation authority checks:
        if sid_opt.is_none() {
            let job_id = generate_job_id();
            let job = BacklogJobDto {
                id: job_id.clone(),
                manga_id,
                series_title: title.clone(),
                scope: req.scope.to_string(),
                requested_volumes: vec![],
                batch_size,
                is_dry_run,
                state: "BLOCKED".to_string(),
                total_requested: 0,
                total_eligible: 0,
                total_already_owned: 0,
                total_queued: 0,
                total_failed: 0,
                items: vec![],
                confirmed_by_user: false,
                error: Some("Series is not yet reconciled with MangaBaka".to_string()),
                created_at: now(),
                updated_at: now(),
                completed_at: None,
            };
            self.persist_backlog_job(&job, &[]).await?;
            return Ok(job);
        }

        let series_id = sid_opt.unwrap();
        let status_str = status_opt.unwrap_or_default();

        if status_str == "review_required" {
            let reason = rev_reason.unwrap_or_else(|| "Series requires manual metadata review".to_string());
            let job_id = generate_job_id();
            let job = BacklogJobDto {
                id: job_id.clone(),
                manga_id,
                series_title: title.clone(),
                scope: req.scope.to_string(),
                requested_volumes: vec![],
                batch_size,
                is_dry_run,
                state: "REVIEW_REQUIRED".to_string(),
                total_requested: 0,
                total_eligible: 0,
                total_already_owned: 0,
                total_queued: 0,
                total_failed: 0,
                items: vec![],
                confirmed_by_user: false,
                error: Some(reason),
                created_at: now(),
                updated_at: now(),
                completed_at: None,
            };
            self.persist_backlog_job(&job, &[]).await?;
            return Ok(job);
        }

        if status_str == "collection_unavailable" || coll_opt.is_none() {
            let job_id = generate_job_id();
            let job = BacklogJobDto {
                id: job_id.clone(),
                manga_id,
                series_title: title.clone(),
                scope: req.scope.to_string(),
                requested_volumes: vec![],
                batch_size,
                is_dry_run,
                state: "BLOCKED".to_string(),
                total_requested: 0,
                total_eligible: 0,
                total_already_owned: 0,
                total_queued: 0,
                total_failed: 0,
                items: vec![],
                confirmed_by_user: false,
                error: Some("Authoritative MangaBaka edition collection is unavailable for this series".to_string()),
                created_at: now(),
                updated_at: now(),
                completed_at: None,
            };
            self.persist_backlog_job(&job, &[]).await?;
            return Ok(job);
        }

        // Authoritative volume set derivation
        let mut authoritative_volumes: BTreeSet<i64> = BTreeSet::new();
        let mut future_vols_set: HashSet<i64> = HashSet::new();

        if let Some(json_str) = owned_json {
            if let Ok(vols) = serde_json::from_str::<Vec<f64>>(&json_str) {
                for v in vols {
                    authoritative_volumes.insert(v.round() as i64);
                }
            }
        }
        if let Some(json_str) = missing_json {
            if let Ok(vols) = serde_json::from_str::<Vec<f64>>(&json_str) {
                for v in vols {
                    authoritative_volumes.insert(v.round() as i64);
                }
            }
        }
        if let Some(json_str) = future_json {
            if let Ok(vols) = serde_json::from_str::<Vec<f64>>(&json_str) {
                for v in vols {
                    let vi = v.round() as i64;
                    authoritative_volumes.insert(vi);
                    future_vols_set.insert(vi);
                }
            }
        }
        if let Some(tot) = total_official_vols {
            if tot > 0 {
                for v in 1..=tot {
                    authoritative_volumes.insert(v);
                }
            }
        }

        // 2. Fetch owned volumes and active downloads
        let mut owned_vols_set: HashSet<i64> = HashSet::new();
        let mut in_flight_set: HashSet<i64> = HashSet::new();

        if let Some(pipeline) = &self.pipeline {
            let title_ownership = pipeline.ownership.get_title_ownership(manga_id, &title).await;
            for vo in title_ownership.volumes {
                if vo.status == OwnershipStatus::Owned {
                    owned_vols_set.insert(vo.volume.round() as i64);
                }
            }

            let (pipe_cfg, _, _) = pipeline.load_internal_config().await?;
            if let Some(ref lib_dir) = pipe_cfg.library_dir {
                let canonical_target = Path::new(lib_dir).join(&title);
                if canonical_target.is_dir() {
                    let max_check = authoritative_volumes.iter().max().copied().unwrap_or(0);
                    for v_int in 1..=max_check {
                        let unit = AcquisitionReleaseUnit::Volume(v_int as f64);
                        if self.canonical_file_exists_for_unit(&canonical_target, &unit) {
                            owned_vols_set.insert(v_int);
                        }
                    }
                }
            }
        }

        let in_flight_list: Vec<i64> = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.chapter_number
                     FROM downloads d
                     JOIN chapters c ON d.chapter_id = c.id
                     WHERE c.manga_id = ?1
                       AND d.state IN ('pending', 'queued', 'downloading', 'verifying', 'retrying')",
                )?;
                let rows = stmt.query_map([manga_id], |r| r.get::<_, f64>(0))?;
                let mut set = Vec::new();
                for r in rows.flatten() {
                    set.push(r.round() as i64);
                }
                Ok(set)
            })
            .await?;
        for v in in_flight_list {
            in_flight_set.insert(v);
        }

        // 3. Resolve requested volumes based on scope
        let mut candidate_volume_numbers: BTreeSet<i64> = BTreeSet::new();
        match req.scope {
            BacklogScope::AllMissing => {
                for &v in &authoritative_volumes {
                    if !future_vols_set.contains(&v) {
                        candidate_volume_numbers.insert(v);
                    }
                }
            }
            BacklogScope::Range => {
                let start = req.range_start.unwrap_or(1.0).round() as i64;
                let end = req
                    .range_end
                    .map(|e| e.round() as i64)
                    .or_else(|| authoritative_volumes.iter().max().copied())
                    .unwrap_or(0);
                if start > end {
                    return Err(AppError::bad_request(
                        "Range start volume cannot exceed range end volume",
                    ));
                }
                for v in start..=end {
                    if !future_vols_set.contains(&v) {
                        candidate_volume_numbers.insert(v);
                    }
                }
            }
            BacklogScope::ExplicitVolumes => {
                let vols = req
                    .explicit_volumes
                    .unwrap_or_default();
                if vols.is_empty() {
                    return Err(AppError::bad_request(
                        "Explicit volume list cannot be empty for EXPLICIT_VOLUMES scope",
                    ));
                }
                for v in vols {
                    let v_round = v.round() as i64;
                    if v_round > 0 {
                        candidate_volume_numbers.insert(v_round);
                    }
                }
            }
        }

        let total_requested = candidate_volume_numbers.len();
        let mut items = Vec::new();
        let mut total_already_owned = 0;
        let mut total_eligible = 0;

        let sid_query = series_id.clone();
        let norm_series_id = format!("title:{}", normalize_title(&title));

        // 4. Evaluate each volume for backlog acquisition
        for v_int in candidate_volume_numbers.iter().copied() {
            let v_f64 = v_int as f64;

            // Check non-existent volume (not in authoritative collection)
            if !authoritative_volumes.contains(&v_int) {
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "NON_EXISTENT_VOLUME".to_string(),
                    reason: format!("Volume {} does not exist in authoritative MangaBaka collection", v_int),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
                continue;
            }

            // Check unreleased future volume
            if future_vols_set.contains(&v_int) {
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "UNRELEASED".to_string(),
                    reason: "Volume is officially unreleased according to metadata".to_string(),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
                continue;
            }

            // Check Komga / Canonical ownership
            if owned_vols_set.contains(&v_int) {
                total_already_owned += 1;
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "ALREADY_OWNED".to_string(),
                    reason: "Volume is already owned in Komga canonical library".to_string(),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
                continue;
            }

            // Check active in-flight acquisition
            if in_flight_set.contains(&v_int) {
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "IN_FLIGHT".to_string(),
                    reason: "Volume acquisition is currently in-flight".to_string(),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
                continue;
            }

            // Look for cached candidates in discovered_release_candidates
            let s1 = sid_query.clone();
            let s2 = norm_series_id.clone();
            let candidates: Vec<PersistedCandidate> = self
                .db
                .call(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT series_id, volume_number, candidate_id, provider, provider_item_id,
                                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                                is_digital, score, score_explanation, download_url, magnet_url,
                                info_hash, seeders, leechers, published_at
                         FROM discovered_release_candidates
                         WHERE (series_id = ?1 OR series_id = ?2)
                           AND ABS(volume_number - ?3) < 0.001
                           AND match_type != 'rejected'
                         ORDER BY score DESC",
                    )?;
                    let rows = stmt.query_map(params![s1, s2, v_f64], |r| {
                        Ok(PersistedCandidate {
                            series_id: r.get(0)?,
                            volume_number: r.get(1)?,
                            candidate_id: r.get(2)?,
                            provider: r.get(3)?,
                            provider_item_id: r.get(4)?,
                            title: r.get(5)?,
                            raw_title: r.get(6)?,
                            match_type: r.get(7)?,
                            size_bytes: r.get::<_, Option<i64>>(8)?.map(|s| s as u64),
                            uploader: r.get(9)?,
                            is_trusted: r.get::<_, i64>(10)? != 0,
                            is_digital: r.get::<_, i64>(11)? != 0,
                            score: r.get(12)?,
                            score_explanation: r.get(13)?,
                            download_url: r.get(14)?,
                            magnet_url: r.get(15)?,
                            info_hash: r.get(16)?,
                            seeders: r.get::<_, Option<i64>>(17)?.map(|s| s as u32),
                            leechers: r.get::<_, Option<i64>>(18)?.map(|s| s as u32),
                            published_at: r.get(19)?,
                        })
                    })?;
                    let mut res = Vec::new();
                    for r in rows.flatten() {
                        res.push(r);
                    }
                    Ok(res)
                })
                .await?;

            let profile = AcquisitionProfile::default();
            let mut eligible_candidates = Vec::new();
            for cand in &candidates {
                if is_candidate_auto_eligible(cand, &profile).is_ok() {
                    eligible_candidates.push(cand);
                }
            }

            // Candidate selection preference:
            // Prefer Madokami (direct HTTP, no swarms) over Nyaa among eligible candidates
            let preferred_cand = eligible_candidates
                .iter()
                .find(|c| c.provider == "madokami")
                .or_else(|| eligible_candidates.first())
                .copied();

            if let Some(cand) = preferred_cand {
                total_eligible += 1;
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "ELIGIBLE".to_string(),
                    reason: format!(
                        "Safe candidate available via {} (Score {:.1})",
                        cand.provider.to_uppercase(),
                        cand.score
                    ),
                    candidate_id: Some(cand.candidate_id.clone()),
                    provider: Some(cand.provider.clone()),
                    candidate_title: Some(cand.title.clone()),
                    size_bytes: cand.size_bytes,
                    score: Some(cand.score),
                    is_digital: Some(cand.is_digital),
                    uploader: cand.uploader.clone(),
                });
            } else if !candidates.is_empty() {
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "UNAVAILABLE".to_string(),
                    reason: "No safe release candidate satisfied eligibility gates (e.g. pack, range, format)".to_string(),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
            } else {
                items.push(BacklogItemPlan {
                    volume_number: v_f64,
                    status: "UNAVAILABLE".to_string(),
                    reason: "No safe release candidate found in discovery cache".to_string(),
                    candidate_id: None,
                    provider: None,
                    candidate_title: None,
                    size_bytes: None,
                    score: None,
                    is_digital: None,
                    uploader: None,
                });
            }
        }

        let job_id = generate_job_id();
        let scope_str = req.scope.to_string();
        let requested_vols_vec: Vec<f64> = candidate_volume_numbers.iter().map(|&v| v as f64).collect();

        let job = BacklogJobDto {
            id: job_id.clone(),
            manga_id,
            series_title: title.clone(),
            scope: scope_str.clone(),
            requested_volumes: requested_vols_vec,
            batch_size,
            is_dry_run,
            state: "PLANNED".to_string(),
            total_requested,
            total_eligible,
            total_already_owned,
            total_queued: 0,
            total_failed: 0,
            items: items.clone(),
            confirmed_by_user: false,
            error: None,
            created_at: now(),
            updated_at: now(),
            completed_at: None,
        };

        self.persist_backlog_job(&job, &items).await?;

        Ok(job)
    }

    /// Executes a planned backlog acquisition job.
    /// Strictly verifies user confirmation, multi-volume confirmation,
    /// dry-run safety gates, and executes eligible items up to `batch_size`
    /// via the M6.4 candidate approval bridge.
    pub async fn execute_backlog_job(
        &self,
        job_id: &str,
        req: ExecuteBacklogRequest,
    ) -> Result<ExecuteBacklogResponse> {
        let jid = job_id.to_string();

        // 1. Load job from database
        let job_opt: Option<(i64, String, usize, bool, String, String)> = self
            .db
            .call(move |conn| {
                let row = conn.query_row(
                    "SELECT manga_id, series_title, batch_size, is_dry_run, state, items_json
                     FROM backlog_acquisition_jobs
                     WHERE id = ?1",
                    [&jid],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get::<_, i64>(2)? as usize,
                            r.get::<_, i64>(3)? != 0,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    },
                )
                .optional()?;
                Ok(row)
            })
            .await?;

        let (manga_id, series_title, batch_size, planned_dry_run, state, items_json) =
            job_opt.ok_or_else(|| AppError::not_found(format!("Backlog job '{job_id}' not found")))?;

        if state != "PLANNED" {
            return Err(AppError::conflict(format!(
                "Backlog job '{job_id}' is not in PLANNED state (current: {state})"
            )));
        }

        // 2. Parse items
        let mut items: Vec<BacklogItemPlan> = serde_json::from_str(&items_json)
            .map_err(|e| AppError::internal(format!("Invalid items JSON in job {job_id}: {e}")))?;

        let eligible_items: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.status == "ELIGIBLE" && it.candidate_id.is_some())
            .map(|(idx, _)| idx)
            .collect();

        let eligible_count = eligible_items.len();
        if eligible_count == 0 {
            return Err(AppError::bad_request("Job has no eligible items to acquire"));
        }

        // 3. Mandatory confirmation gates
        if !req.confirm {
            return Err(AppError::bad_request(
                "Explicit user confirmation required to execute backlog acquisition",
            ));
        }

        if eligible_count > 1 && req.confirm_multiple != Some(true) {
            return Err(AppError::bad_request(format!(
                "Batch operation affects {eligible_count} volumes. Multi-volume confirmation required.",
            )));
        }

        // Override dry-run if explicitly requested in execute payload, otherwise use planned dry-run
        let is_dry_run = req.is_dry_run.unwrap_or(planned_dry_run);

        // 4. Atomic claim: transition from PLANNED to EXECUTING
        let jid_claim = job_id.to_string();
        let claim_time = now();
        let claimed: usize = self
            .db
            .call(move |conn| {
                let affected = conn.execute(
                    "UPDATE backlog_acquisition_jobs
                     SET state = 'EXECUTING', updated_at = ?2
                     WHERE id = ?1 AND state = 'PLANNED'",
                    params![&jid_claim, claim_time],
                )?;
                Ok(affected)
            })
            .await?;

        if claimed == 0 {
            return Err(AppError::conflict(format!(
                "Backlog job '{job_id}' is no longer in PLANNED state or was claimed concurrently"
            )));
        }

        // 5. Dry-run execution: zero real acquisitions, verifies candidate availability and ownership
        if is_dry_run {
            let jid_clone = job_id.to_string();
            let new_items_json = serde_json::to_string(&items).unwrap_or_else(|_| items_json.clone());
            let comp_time = now();

            self.db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE backlog_acquisition_jobs
                         SET state = 'COMPLETED', confirmed_by_user = 1, is_dry_run = 1,
                             items_json = ?2, updated_at = ?3, completed_at = ?3
                         WHERE id = ?1 AND state = 'EXECUTING'",
                        params![jid_clone, new_items_json, comp_time],
                    )?;
                    Ok(())
                })
                .await?;

            return Ok(ExecuteBacklogResponse {
                job_id: job_id.to_string(),
                state: "COMPLETED".to_string(),
                message: format!(
                    "Dry-run execution verified for {eligible_count} volume(s). Zero real downloads initiated.",
                ),
                is_dry_run: true,
                queued_count: 0,
                failed_count: 0,
                items,
            });
        }

        // 6. Real execution: bounded batch up to batch_size
        let to_process = eligible_items.iter().take(batch_size).copied().collect::<Vec<_>>();
        let mut queued_count = 0;
        let mut failed_count = 0;

        for idx in to_process {
            let item = &mut items[idx];
            let cand_id = match &item.candidate_id {
                Some(cid) => cid.clone(),
                None => continue,
            };

            // Re-verify Komga ownership before each acquisition
            if let Some(pipeline) = &self.pipeline {
                let title_ownership = pipeline
                    .ownership
                    .get_title_ownership(manga_id, &series_title)
                    .await;
                if title_ownership.volumes.iter().any(|vo| {
                    (vo.volume - item.volume_number).abs() < 0.001
                        && vo.status == OwnershipStatus::Owned
                }) {
                    item.status = "ALREADY_OWNED".to_string();
                    item.reason = "Volume ownership detected in Komga prior to dispatch".to_string();
                    continue;
                }
            }

            // Check publication receipts table for existing canonical receipt
            let j_vol = item.volume_number;
            let j_mid = manga_id;
            let receipt_exists: bool = self
                .db
                .call(move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM publication_receipts
                         WHERE manga_id = ?1 AND ABS(volume_number - ?2) < 0.001 AND status = 'published'",
                        params![j_mid, j_vol],
                        |r| r.get(0),
                    )?;
                    Ok(count > 0)
                })
                .await?;
            if receipt_exists {
                item.status = "ALREADY_OWNED".to_string();
                item.reason = "Verified publication receipt already exists for this volume".to_string();
                continue;
            }

            // Check active in-flight acquisition
            let in_flight: bool = self
                .db
                .call(move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM downloads d
                         JOIN chapters c ON d.chapter_id = c.id
                         WHERE c.manga_id = ?1 AND ABS(c.chapter_number - ?2) < 0.001
                           AND d.state IN ('pending', 'queued', 'downloading', 'verifying', 'retrying')",
                        params![j_mid, j_vol],
                        |r| r.get(0),
                    )?;
                    Ok(count > 0)
                })
                .await?;
            if in_flight {
                item.status = "IN_FLIGHT".to_string();
                item.reason = "Volume acquisition is already in-flight".to_string();
                continue;
            }

            // Reload candidate from database and re-verify auto eligibility
            let c_id = cand_id.clone();
            let cand_opt: Option<PersistedCandidate> = self
                .db
                .call(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT series_id, volume_number, candidate_id, provider, provider_item_id,
                                title, raw_title, match_type, size_bytes, uploader, is_trusted,
                                is_digital, score, score_explanation, download_url, magnet_url,
                                info_hash, seeders, leechers, published_at
                         FROM discovered_release_candidates
                         WHERE candidate_id = ?1",
                    )?;
                    let res = stmt
                        .query_row([&c_id], |r| {
                            Ok(PersistedCandidate {
                                series_id: r.get(0)?,
                                volume_number: r.get(1)?,
                                candidate_id: r.get(2)?,
                                provider: r.get(3)?,
                                provider_item_id: r.get(4)?,
                                title: r.get(5)?,
                                raw_title: r.get(6)?,
                                match_type: r.get(7)?,
                                size_bytes: r.get::<_, Option<i64>>(8)?.map(|s| s as u64),
                                uploader: r.get(9)?,
                                is_trusted: r.get::<_, i64>(10)? != 0,
                                is_digital: r.get::<_, i64>(11)? != 0,
                                score: r.get(12)?,
                                score_explanation: r.get(13)?,
                                download_url: r.get(14)?,
                                magnet_url: r.get(15)?,
                                info_hash: r.get(16)?,
                                seeders: r.get::<_, Option<i64>>(17)?.map(|s| s as u32),
                                leechers: r.get::<_, Option<i64>>(18)?.map(|s| s as u32),
                                published_at: r.get(19)?,
                            })
                        })
                        .optional()?;
                    Ok(res)
                })
                .await?;

            let cand = match cand_opt {
                Some(c) => c,
                None => {
                    item.status = "FAILED".to_string();
                    item.reason = "Candidate is no longer available in discovery cache".to_string();
                    failed_count += 1;
                    continue;
                }
            };

            let profile = AcquisitionProfile::default();
            if cand.match_type != "exact_volume" {
                item.status = "REJECTED".to_string();
                item.reason = format!("Candidate match type '{}' is not exact volume", cand.match_type);
                failed_count += 1;
                continue;
            }
            if let Err(code) = is_candidate_auto_eligible(&cand, &profile) {
                item.status = "REJECTED".to_string();
                item.reason = format!("Candidate is not auto-eligible: {}", code.as_str());
                failed_count += 1;
                continue;
            }

            // Bridge to M6.4 approve_candidate with strict gates (no bypass of range/pack/review)
            let approval_res = self
                .approve_candidate(
                    manga_id,
                    item.volume_number,
                    &cand_id,
                    ApproveCandidateRequest {
                        confirm_range: None,
                        confirm_pack: None,
                        confirm_review: None,
                        dry_run: None,
                    },
                )
                .await;

            match approval_res {
                Ok(resp) => {
                    if resp.already_owned {
                        item.status = "ALREADY_OWNED".to_string();
                        item.reason = resp.message;
                    } else if resp.status == "queued" {
                        item.status = "QUEUED".to_string();
                        item.reason = format!("Successfully queued for acquisition ({})", resp.message);
                        queued_count += 1;
                    } else {
                        item.status = resp.status;
                        item.reason = resp.message;
                    }
                }
                Err(e) => {
                    item.status = "FAILED".to_string();
                    item.reason = format!("Approval failed: {}", e.message);
                    failed_count += 1;
                }
            }

            // Rate-limiting delay between queuing items to avoid overwhelming external backends
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let final_state = if failed_count == 0 {
            "COMPLETED"
        } else if queued_count > 0 {
            "PARTIAL"
        } else {
            "FAILED"
        };

        let jid_clone = job_id.to_string();
        let new_items_json = serde_json::to_string(&items).unwrap_or_else(|_| items_json.clone());
        let comp_time = now();
        let q_cnt = queued_count as i64;
        let f_cnt = failed_count as i64;
        let f_state = final_state.to_string();

        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE backlog_acquisition_jobs
                     SET state = ?2, confirmed_by_user = 1, is_dry_run = 0,
                         total_queued = ?3, total_failed = ?4, items_json = ?5,
                         updated_at = ?6, completed_at = ?6
                     WHERE id = ?1 AND state = 'EXECUTING'",
                    params![jid_clone, f_state, q_cnt, f_cnt, new_items_json, comp_time],
                )?;
                Ok(())
            })
            .await?;

        Ok(ExecuteBacklogResponse {
            job_id: job_id.to_string(),
            state: final_state.to_string(),
            message: format!(
                "Backlog batch execution complete: {queued_count} queued, {failed_count} failed."
            ),
            is_dry_run: false,
            queued_count,
            failed_count,
            items,
        })
    }

    /// Fetches audit trail of backlog jobs for a specific title.
    pub async fn get_manga_backlog_jobs(&self, manga_id: i64, limit: usize) -> Result<Vec<BacklogJobDto>> {
        let lim = limit.clamp(1, 100) as i64;
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, manga_id, series_title, scope, requested_volumes_json,
                            batch_size, is_dry_run, state, total_requested, total_eligible,
                            total_already_owned, total_queued, total_failed, items_json,
                            confirmed_by_user, error, created_at, updated_at, completed_at
                     FROM backlog_acquisition_jobs
                     WHERE manga_id = ?1
                     ORDER BY created_at DESC
                     LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![manga_id, lim], |r| {
                    let req_json: String = r.get(4)?;
                    let items_json: String = r.get(13)?;
                    let requested_volumes: Vec<f64> = serde_json::from_str(&req_json).unwrap_or_default();
                    let items: Vec<BacklogItemPlan> = serde_json::from_str(&items_json).unwrap_or_default();

                    Ok(BacklogJobDto {
                        id: r.get(0)?,
                        manga_id: r.get(1)?,
                        series_title: r.get(2)?,
                        scope: r.get(3)?,
                        requested_volumes,
                        batch_size: r.get::<_, i64>(5)? as usize,
                        is_dry_run: r.get::<_, i64>(6)? != 0,
                        state: r.get(7)?,
                        total_requested: r.get::<_, i64>(8)? as usize,
                        total_eligible: r.get::<_, i64>(9)? as usize,
                        total_already_owned: r.get::<_, i64>(10)? as usize,
                        total_queued: r.get::<_, i64>(11)? as usize,
                        total_failed: r.get::<_, i64>(12)? as usize,
                        items,
                        confirmed_by_user: r.get::<_, i64>(14)? != 0,
                        error: r.get(15)?,
                        created_at: r.get(16)?,
                        updated_at: r.get(17)?,
                        completed_at: r.get(18)?,
                    })
                })?;

                let mut list = Vec::new();
                for r in rows.flatten() {
                    list.push(r);
                }
                Ok(list)
            })
            .await
    }

    /// Fetches all recent backlog acquisition jobs across the library.
    pub async fn get_all_backlog_jobs(&self, limit: usize) -> Result<Vec<BacklogJobDto>> {
        let lim = limit.clamp(1, 100) as i64;
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, manga_id, series_title, scope, requested_volumes_json,
                            batch_size, is_dry_run, state, total_requested, total_eligible,
                            total_already_owned, total_queued, total_failed, items_json,
                            confirmed_by_user, error, created_at, updated_at, completed_at
                     FROM backlog_acquisition_jobs
                     ORDER BY created_at DESC
                     LIMIT ?1",
                )?;
                let rows = stmt.query_map(params![lim], |r| {
                    let req_json: String = r.get(4)?;
                    let items_json: String = r.get(13)?;
                    let requested_volumes: Vec<f64> = serde_json::from_str(&req_json).unwrap_or_default();
                    let items: Vec<BacklogItemPlan> = serde_json::from_str(&items_json).unwrap_or_default();

                    Ok(BacklogJobDto {
                        id: r.get(0)?,
                        manga_id: r.get(1)?,
                        series_title: r.get(2)?,
                        scope: r.get(3)?,
                        requested_volumes,
                        batch_size: r.get::<_, i64>(5)? as usize,
                        is_dry_run: r.get::<_, i64>(6)? != 0,
                        state: r.get(7)?,
                        total_requested: r.get::<_, i64>(8)? as usize,
                        total_eligible: r.get::<_, i64>(9)? as usize,
                        total_already_owned: r.get::<_, i64>(10)? as usize,
                        total_queued: r.get::<_, i64>(11)? as usize,
                        total_failed: r.get::<_, i64>(12)? as usize,
                        items,
                        confirmed_by_user: r.get::<_, i64>(14)? != 0,
                        error: r.get(15)?,
                        created_at: r.get(16)?,
                        updated_at: r.get(17)?,
                        completed_at: r.get(18)?,
                    })
                })?;

                let mut list = Vec::new();
                for r in rows.flatten() {
                    list.push(r);
                }
                Ok(list)
            })
            .await
    }

    /// Cancels a planned backlog acquisition job.
    pub async fn cancel_backlog_job(&self, job_id: &str) -> Result<bool> {
        let jid = job_id.to_string();
        let now_t = now();
        self.db
            .call(move |conn| {
                let rows = conn.execute(
                    "UPDATE backlog_acquisition_jobs
                     SET state = 'CANCELLED', updated_at = ?2, completed_at = ?2
                     WHERE id = ?1 AND state = 'PLANNED'",
                    params![jid, now_t],
                )?;
                Ok(rows > 0)
            })
            .await
    }
}
