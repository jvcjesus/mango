//! Manual candidate approval bridge for M6.4.
//!
//! Provides explicit, user-initiated approval of a discovered release candidate,
//! performing rigorous pre-checks (ownership, active acquisitions, MangaBaka validation,
//! provider verification, duplicate safety, match confidence) before bridging the candidate
//! into the existing acquisition planner and execution pipeline.

use std::path::Path;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::acquisition::candidates::{
    CandidateHealth, CandidateQualitySignals, NormalizedAcquisitionCandidate,
    UnitMappingProvenance,
};
use crate::acquisition::execution::ApprovePlanRequest;
use crate::acquisition::interactive::PlanningRequest;
use crate::acquisition::service::AcquisitionService;
use crate::acquisition::AcquisitionReleaseUnit;
use crate::db::now;
use crate::error::{AppError, Result};
use crate::pipeline::mangabaka::normalize_title;
use crate::pipeline::ownership::OwnershipStatus;

/// Client request for approving a candidate release for a volume.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveCandidateRequest {
    pub confirm_range: Option<bool>,
    pub confirm_pack: Option<bool>,
    pub confirm_review: Option<bool>,
    pub dry_run: Option<bool>,
    pub authorization_token: Option<String>,
    pub user_id: Option<i64>,
}

/// Structured manual acquisition review DTO for M8.10.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualAcquisitionReviewDto {
    pub catalog_title_id: i64,
    pub series_title: String,
    pub mangabaka_id: Option<i64>,
    pub official_edition: String,
    pub official_volume_number: f64,
    pub candidate_id: String,
    pub candidate_title: String,
    pub provider: String,
    pub acquisition_backend: String,
    pub ownership_status: String,
    pub is_currently_owned: bool,
    pub is_eligible: bool,
    pub eligibility_code: Option<String>,
    pub eligibility_reason: Option<String>,
    pub staging_destination: String,
    pub canonical_destination: String,
    pub dry_run: bool,
    pub publication_requires_separate_approval: bool,
    pub authorization_token: Option<String>,
    pub expires_at: Option<i64>,
}

/// Unified response from approving a candidate release.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveCandidateResponse {
    pub status: String, // "queued", "already_owned", "already_acquiring", "skipped"
    pub message: String,
    pub execution_id: Option<String>,
    pub chapter_ids: Vec<i64>,
    pub already_owned: bool,
    pub already_acquiring: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct PersistedCandidate {
    pub series_id: String,
    pub volume_number: f64,
    pub candidate_id: String,
    pub provider: String,
    pub provider_item_id: String,
    pub title: String,
    pub raw_title: String,
    pub match_type: String,
    pub size_bytes: Option<u64>,
    pub uploader: Option<String>,
    pub is_trusted: bool,
    pub is_digital: bool,
    pub score: f64,
    pub score_explanation: String,
    pub download_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_hash: Option<String>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub published_at: Option<i64>,
}

impl AcquisitionService {
    /// Explicitly approves a discovered candidate release for a logical volume,
    /// revalidating target, ownership, provider capability, and queue state before
    /// routing into the existing planner and execution pipeline.
    pub async fn approve_candidate(
        &self,
        manga_id: i64,
        volume: f64,
        candidate_id: &str,
        req: ApproveCandidateRequest,
    ) -> Result<ApproveCandidateResponse> {
        // 1. Resolve series title and reconciliation identity
        let (title, s_id) = self
            .db
            .call(move |conn| {
                let info: Option<(String, Option<String>)> = conn
                    .query_row(
                        "SELECT m.title, sr.series_id
                         FROM manga m
                         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                         WHERE m.id = ?1",
                        [manga_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let (t, s) = info.ok_or_else(|| AppError::not_found("Manga title not found"))?;
                let sid = s.unwrap_or_else(|| format!("title:{}", normalize_title(&t)));
                Ok((t, sid))
            })
            .await?;

        let norm_series_id = format!("title:{}", normalize_title(&title));
        let c_id_str = candidate_id.to_string();
        let sid_query = s_id.clone();

        // 2. Load persisted candidate from database (authoritative server-side record)
        let candidate_opt: Option<PersistedCandidate> = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT series_id, volume_number, candidate_id, provider, provider_item_id,
                            title, raw_title, match_type, size_bytes, uploader, is_trusted, is_digital,
                            score, score_explanation, download_url, magnet_url, info_hash,
                            seeders, leechers, published_at
                     FROM discovered_release_candidates
                     WHERE (series_id = ?1 OR series_id = ?2)
                       AND ABS(volume_number - ?3) < 0.001
                       AND candidate_id = ?4
                     LIMIT 1",
                )?;

                let cand = stmt
                    .query_row(
                        params![sid_query, norm_series_id, volume, c_id_str],
                        |r| {
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
                        },
                    )
                    .optional()?;

                Ok(cand)
            })
            .await?;

        let candidate = candidate_opt.ok_or_else(|| {
            AppError::not_found(format!(
                "Candidate '{candidate_id}' not found or stale for Volume {volume}"
            ))
        })?;

        // 3. Verify provider capability
        if candidate.provider != "nyaa" && candidate.provider != "madokami" {
            return Err(AppError::bad_request(format!(
                "Provider '{}' does not support automated acquisition in Mango",
                candidate.provider
            )));
        }

        let (cfg, has_token) = self.load_internal_config().await?;
        if candidate.provider == "nyaa" {
            if !cfg.torbox_enabled {
                return Err(AppError::bad_request(
                    "TorBox download backend is disabled in Settings",
                ));
            }
            if !has_token {
                return Err(AppError::bad_request(
                    "TorBox API token is not configured. Add it in Settings.",
                ));
            }
        } else if candidate.provider == "madokami" {
            if !cfg.madokami_enabled {
                return Err(AppError::bad_request(
                    "Madokami download backend is disabled in Settings",
                ));
            }
            if !cfg.has_madokami_credentials {
                return Err(AppError::bad_request(
                    "Madokami credentials are not configured. Add them in Settings.",
                ));
            }
        }

        // 4. Candidate revalidation & Match Confidence Policy
        match candidate.match_type.as_str() {
            "rejected" => {
                return Err(AppError::bad_request(
                    "Rejected candidates cannot be acquired",
                ));
            }
            "possible_match" => {
                if req.confirm_review != Some(true) {
                    return Err(AppError::bad_request(
                        "Possible match cannot be acquired without explicit confirmation; review required",
                    ));
                }
            }
            "volume_range" | "volume_range_containing_target" => {
                if req.confirm_range != Some(true) {
                    return Err(AppError::bad_request(
                        "Volume range requires explicit confirmation before acquisition because payload may contain multiple volumes",
                    ));
                }
            }
            "series_pack" => {
                if req.confirm_pack != Some(true) {
                    return Err(AppError::bad_request(
                        "Series pack requires explicit confirmation before acquisition because payload may contain multiple volumes",
                    ));
                }
            }
            _ => {}
        }

        if candidate.info_hash.is_none()
            && candidate.magnet_url.is_none()
            && candidate.download_url.is_none()
        {
            return Err(AppError::bad_request(
                "Candidate has no usable torrent locator (hash or magnet URL)",
            ));
        }

        // 5. MangaBaka Logical-Target Validation
        let sid_for_mb = s_id.clone();
        let target_check = self
            .db
            .call(move |conn| {
                let rec_opt: Option<(String, Option<i64>, String)> = conn
                    .query_row(
                        "SELECT status, total_official_volumes, future_volumes_json
                         FROM series_reconciliations WHERE series_id = ?1",
                        [&sid_for_mb],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;

                if let Some((s_status, total_opt, future_json)) = rec_opt {
                    if s_status == "review_required" {
                        return Err(AppError::bad_request(
                            "Cannot acquire: Series reconciliation requires manual review",
                        ));
                    }

                    // Check if volume is in future/unreleased list
                    if let Ok(future_vols) = serde_json::from_str::<Vec<f64>>(&future_json) {
                        if future_vols.iter().any(|&v| (v - volume).abs() < 0.001) {
                            return Err(AppError::bad_request(
                                "Cannot acquire: Volume is unreleased according to official MangaBaka metadata",
                            ));
                        }
                    }

                    if let Some(total) = total_opt {
                        if total > 0 && volume > total as f64 + 0.001 {
                            return Err(AppError::bad_request(
                                "Cannot acquire: Volume exceeds maximum released volume according to official metadata",
                            ));
                        }
                    }
                }

                Ok(())
            })
            .await;

        if let Err(e) = target_check {
            return Err(e);
        }

        // 6. Mandatory Ownership Recheck (Komga + Canonical FS + Acquisition History)
        // A. Komga ownership
        if let Some(pipeline) = &self.pipeline {
            let title_ownership = pipeline.ownership.get_title_ownership(manga_id, &title).await;
            if title_ownership.volumes.iter().any(|vo| {
                (vo.volume - volume).abs() < 0.001 && vo.status == OwnershipStatus::Owned
            }) {
                return Ok(ApproveCandidateResponse {
                    status: "already_owned".to_string(),
                    message: format!("Volume {} is already owned in Komga library", volume),
                    execution_id: None,
                    chapter_ids: vec![],
                    already_owned: true,
                    already_acquiring: false,
                });
            }

            // B. Canonical filesystem check
            let (pipe_cfg, _, _) = pipeline.load_internal_config().await?;
            if let Some(ref lib_dir) = pipe_cfg.library_dir {
                let canonical_target = Path::new(lib_dir).join(&title);
                if canonical_target.is_dir()
                    && self.canonical_file_exists_for_unit(
                        &canonical_target,
                        &AcquisitionReleaseUnit::Volume(volume),
                    )
                {
                    return Ok(ApproveCandidateResponse {
                        status: "already_owned".to_string(),
                        message: format!("Volume {} already exists in canonical library", volume),
                        execution_id: None,
                        chapter_ids: vec![],
                        already_owned: true,
                        already_acquiring: false,
                    });
                }
            }
        }

        // C. Durable publication history check
        let is_in_history: bool = self
            .db
            .call(move |conn| {
                let vol_int = volume.round() as i64;
                let pat1 = format!("%v{vol_int}.cbz");
                let pat2 = format!("%v{:02}.cbz", vol_int);
                let pat3 = format!("%Vol%{vol_int}.cbz");
                let pat4 = format!("%Volume%{vol_int}.cbz");
                let pat5 = format!("%v{vol_int}%");
                let pat6 = format!("%v{:02}%", vol_int);

                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM acquisition_history
                            WHERE manga_id = ?1
                              AND outcome IN ('published', 'completed')
                              AND (
                                canonical_path LIKE ?2
                                OR canonical_path LIKE ?3
                                OR canonical_path LIKE ?4
                                OR canonical_path LIKE ?5
                                OR canonical_path LIKE ?6
                                OR canonical_path LIKE ?7
                                OR release_name LIKE ?2
                                OR release_name LIKE ?3
                                OR release_name LIKE ?4
                                OR release_name LIKE ?5
                                OR release_name LIKE ?6
                                OR release_name LIKE ?7
                              )
                        )",
                        params![manga_id, pat1, pat2, pat3, pat4, pat5, pat6],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);

                Ok(exists)
            })
            .await?;

        if is_in_history {
            return Ok(ApproveCandidateResponse {
                status: "already_owned".to_string(),
                message: format!("Volume {} was already published canonically", volume),
                execution_id: None,
                chapter_ids: vec![],
                already_owned: true,
                already_acquiring: false,
            });
        }

        // 7. Active Acquisition Check (Downloads Queue / Duplicate Safety)
        let hash_check = candidate.info_hash.clone();
        let is_active_acquiring: bool = self
            .db
            .call(move |conn| {
                // Check by chapter volume number in downloads
                let mut stmt = conn.prepare(
                    "SELECT c.name, c.chapter_number, d.torrent_hash
                     FROM downloads d
                     JOIN chapters c ON c.id = d.chapter_id
                     WHERE c.manga_id = ?1 AND d.state != 'failed'",
                )?;

                let rows = stmt.query_map([manga_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, Option<String>>(2)?))
                })?;

                for row in rows.flatten() {
                    let (name, num, thash) = row;
                    if let Some(ref h1) = hash_check {
                        if let Some(ref h2) = thash {
                            if h1.eq_ignore_ascii_case(h2) {
                                return Ok(true);
                            }
                        }
                    }

                    let parsed = crate::release::parse_release(&name, num);
                    if parsed.release_type == crate::release::ReleaseType::Volume {
                        if (parsed.volume.unwrap_or(num) - volume).abs() < 0.001 {
                            return Ok(true);
                        }
                    } else if (num - volume).abs() < 0.001 {
                        return Ok(true);
                    }
                }

                Ok(false)
            })
            .await?;

        if is_active_acquiring {
            return Ok(ApproveCandidateResponse {
                status: "already_acquiring".to_string(),
                message: format!("Volume {} is already in the download queue", volume),
                execution_id: None,
                chapter_ids: vec![],
                already_owned: false,
                already_acquiring: true,
            });
        }

        // 8. Bridge candidate into existing acquisition planner & execution pipeline
        let (classified_unit, _, _) = if candidate.provider == "madokami" {
            let (_, v, _, _, _) = crate::acquisition::madokami::classify_madokami_file(
                &candidate.raw_title,
                Some(volume),
            );
            match v {
                Some(vol) => (AcquisitionReleaseUnit::Volume(vol), None, None),
                None => (AcquisitionReleaseUnit::Volume(volume), None, None),
            }
        } else {
            crate::acquisition::nyaa::classify_release(&candidate.raw_title)
        };
        let unit = match classified_unit {
            AcquisitionReleaseUnit::Unknown => AcquisitionReleaseUnit::Volume(volume),
            other => other,
        };

        let norm_cand = NormalizedAcquisitionCandidate {
            series_identity: title.clone(),
            provider: candidate.provider.clone(),
            provider_release_id: candidate.provider_item_id.clone(),
            title: candidate.title.clone(),
            release_unit: unit,
            release_number: Some(volume),
            release_range: None,
            language: "en".to_string(),
            edition: crate::acquisition::edition::edition_from_title(&candidate.raw_title),
            format: if candidate.is_digital {
                Some("Digital CBZ".to_string())
            } else {
                Some("CBZ Archive".to_string())
            },
            uploader: candidate.uploader.clone(),
            size: candidate.size_bytes,
            published_at: candidate.published_at,
            availability: if candidate.seeders == Some(0) {
                "unhealthy".to_string()
            } else {
                "available".to_string()
            },
            health: CandidateHealth {
                seeders: candidate.seeders,
                leechers: candidate.leechers,
                source_healthy: true,
                uploader_reliability_percent: None,
            },
            quality_signals: CandidateQualitySignals {
                is_digital: candidate.is_digital,
                is_trusted: candidate.is_trusted,
                is_preferred_uploader: false,
                edition_recognized: false,
                edition_penalty: 0.0,
            },
            owned_units: vec![],
            missing_units: vec![volume],
            confidence: if candidate.score >= 50.0 && candidate.match_type != "possible_match" {
                "high".to_string()
            } else {
                "review_recommended".to_string()
            },
            score: candidate.score,
            mapping_provenance: UnitMappingProvenance::ParsedVolumeFromTitle,
            provider_metadata: serde_json::json!({
                "rawTitle": candidate.raw_title,
                "infoHash": candidate.info_hash,
                "magnetUrl": candidate.magnet_url,
                "downloadUrl": candidate.download_url,
                "scoreExplanation": candidate.score_explanation,
            }),
        };

        let profile = self.get_manga_profile(manga_id).await?;
        let missing = self.get_title_missing_content(manga_id).await?;

        let plan_req = PlanningRequest {
            title: title.clone(),
            selected_release_id: Some(candidate.provider_item_id.clone()),
            requested_volumes: Some(vec![volume]),
            content_mode: Some("specific".to_string()),
            use_release_anyway: req.confirm_review.unwrap_or(false),
            ..Default::default()
        };

        let planned = crate::acquisition::interactive::prepare_plan(
            manga_id,
            &title,
            &profile,
            &missing,
            vec![norm_cand],
            &plan_req,
            vec![],
        )?;

        let saved_plan = planned.plan.clone();
        self.db
            .call(move |conn| crate::acquisition::planner::save_plan(conn, &saved_plan))
            .await?;

        let is_dry_run = req.dry_run.unwrap_or(true);
        if is_dry_run {
            return Ok(ApproveCandidateResponse {
                status: "dry_run_preview".to_string(),
                message: format!(
                    "Dry-run plan preview generated for Volume {volume} (execution inhibited)"
                ),
                execution_id: None,
                chapter_ids: vec![],
                already_owned: false,
                already_acquiring: false,
            });
        }

        // Live execution authorization checks
        // 1. Authoritative global dry-run / operating mode policy
        let auto_dry = self
            .db
            .call(|conn| Ok(crate::acquisition::auto_acquire::is_auto_acquire_dry_run(conn) || crate::acquisition::auto_acquire::is_simulation_mode(conn)))
            .await?;
        if auto_dry || cfg.auto_acquire_dry_run {
            return Err(AppError::bad_request(
                "Global dry-run policy is ACTIVE. Live execution is inhibited by server policy.",
            ));
        }

        // 2. Authorization token: validate supplied token or seamlessly generate one-click token for authenticated user
        if req.authorization_token.is_none() && req.user_id.is_none() {
            return Err(AppError::bad_request(
                "Explicit execution authorization token is required for live acquisition",
            ));
        }

        let expected_edition =
            crate::acquisition::edition::edition_from_title(&candidate.raw_title);
        let c_id_validate = candidate.candidate_id.clone();
        let prov_validate = candidate.provider.clone();
        let user_id_check = req.user_id;

        if let Some(ref auth_token_str) = req.authorization_token {
            let auth_token_str = auth_token_str.clone();
            let exp_ed_clone = expected_edition.clone();
            self.db
                .call(move |conn| {
                    let row: Option<(
                        i64,
                        f64,
                        String,
                        String,
                        String,
                        Option<i64>,
                        i64,
                        Option<i64>,
                        String,
                    )> = conn
                        .query_row(
                            "SELECT manga_id, volume_number, candidate_id, provider, edition, user_id, expires_at, consumed_at, status
                             FROM acquisition_execution_authorizations
                             WHERE id = ?1",
                            [&auth_token_str],
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
                        t_manga_id,
                        t_vol,
                        t_cand,
                        t_prov,
                        t_ed,
                        t_user_id,
                        t_exp,
                        t_consumed,
                        t_status,
                    ) = row.ok_or_else(|| {
                        AppError::bad_request("Invalid or unrecognized authorization token")
                    })?;

                    if t_status != "authorized" || t_consumed.is_some() {
                        return Err(AppError::bad_request(
                            "Authorization token has already been consumed or is invalid",
                        ));
                    }

                    if t_exp < now() {
                        return Err(AppError::bad_request("Authorization token has expired"));
                    }

                    if t_manga_id != manga_id {
                        return Err(AppError::bad_request("Authorization token title mismatch"));
                    }

                    if (t_vol - volume).abs() > 0.001 {
                        return Err(AppError::bad_request(
                            "Authorization token volume substitution rejected",
                        ));
                    }

                    if t_cand != c_id_validate {
                        return Err(AppError::bad_request(
                            "Authorization token candidate substitution rejected",
                        ));
                    }

                    if t_prov != prov_validate {
                        return Err(AppError::bad_request(
                            "Authorization token provider substitution rejected",
                        ));
                    }

                    if let Some(ref exp_ed) = exp_ed_clone {
                        if !exp_ed.is_empty()
                            && !t_ed.is_empty()
                            && !t_ed.eq_ignore_ascii_case(exp_ed)
                        {
                            return Err(AppError::bad_request(
                                "Authorization token edition substitution rejected",
                            ));
                        }
                    }

                    if let Some(token_uid) = t_user_id {
                        if let Some(calling_uid) = user_id_check {
                            if token_uid != calling_uid {
                                return Err(AppError::bad_request("Authorization token user mismatch"));
                            }
                        } else {
                            return Err(AppError::bad_request("Unauthorized user rejected"));
                        }
                    }

                    let updated = conn.execute(
                        "UPDATE acquisition_execution_authorizations
                         SET consumed_at = ?1, status = 'consumed'
                         WHERE id = ?2 AND status = 'authorized' AND consumed_at IS NULL",
                        params![now(), auth_token_str],
                    )?;

                    if updated == 0 {
                        return Err(AppError::bad_request(
                            "Authorization token has already been consumed or is invalid",
                        ));
                    }

                    Ok(())
                })
                .await?;
        } else {
            // Seamless one-click authorization: generate and record consumed token immediately
            let auto_token_id = generate_secure_auth_token();
            let p_cand_id = candidate.candidate_id.clone();
            let p_prov = candidate.provider.clone();
            let p_bk = if candidate.provider == "nyaa" { "torbox" } else { "madokami" };
            let p_ed = expected_edition.as_deref().unwrap_or("Standard").to_string();
            let p_sid = s_id.clone();

            self.db
                .call(move |conn| {
                    let now_ts = now();
                    conn.execute(
                        "INSERT INTO acquisition_execution_authorizations (
                            id, manga_id, mangabaka_id, series_id, edition, volume_number,
                            candidate_id, provider, backend, user_id, created_at, expires_at,
                            consumed_at, status
                        ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'consumed')",
                        params![
                            auto_token_id,
                            manga_id,
                            p_sid,
                            p_ed,
                            volume,
                            p_cand_id,
                            p_prov,
                            p_bk,
                            user_id_check,
                            now_ts,
                            now_ts + 900,
                            now_ts,
                        ],
                    )?;
                    Ok(())
                })
                .await?;
        }

        let approve_req = ApprovePlanRequest {
            plan_id: planned.plan.id.clone(),
            approved: true,
            override_confirmed: req.confirm_review.unwrap_or(false),
        };
        let execution = self.approve_and_create_execution(manga_id, approve_req).await?;
        let started = self.start_execution(execution.id.clone()).await?;

        let chapter_ids: Vec<i64> = started
            .items
            .iter()
            .filter_map(|i| i.download_job_id)
            .collect();

        // 9. Persist candidate approval provenance record
        let p_sid = s_id.clone();
        let p_cid = candidate.candidate_id.clone();
        let p_prov = candidate.provider.clone();
        let p_rel_id = candidate.provider_item_id.clone();
        let p_match = candidate.match_type.clone();
        let p_eid = started.id.clone();
        let p_ch_id = chapter_ids.first().copied();

        let insert_res = self
            .db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO candidate_approvals (
                        manga_id, series_id, volume_number, candidate_id, provider,
                        provider_release_id, match_type, approved_at, execution_id,
                        chapter_id, status
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued')",
                    params![
                        manga_id,
                        p_sid,
                        volume,
                        p_cid,
                        p_prov,
                        p_rel_id,
                        p_match,
                        now(),
                        p_eid,
                        p_ch_id,
                    ],
                )?;
                Ok(())
            })
            .await;

        if let Err(ref e) = insert_res {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint failed") || msg.contains("constraint failed") {
                return Ok(ApproveCandidateResponse {
                    status: "already_acquiring".to_string(),
                    message: format!(
                        "Volume {volume} is already being acquired concurrently under an active candidate approval"
                    ),
                    execution_id: None,
                    chapter_ids: vec![],
                    already_owned: false,
                    already_acquiring: true,
                });
            }
            return Err(insert_res.unwrap_err());
        }

        Ok(ApproveCandidateResponse {
            status: "queued".to_string(),
            message: format!("Acquisition queued for Volume {}", volume),
            execution_id: Some(started.id),
            chapter_ids,
            already_owned: false,
            already_acquiring: false,
        })
    }

    /// Generates a validated manual acquisition review and issues a single-use execution authorization.
    pub async fn request_manual_acquisition_review(
        &self,
        manga_id: i64,
        volume: f64,
        candidate_id: &str,
        user_id: Option<i64>,
    ) -> Result<ManualAcquisitionReviewDto> {
        // 1. Resolve series title, mangabaka_id, edition, and series_id
        let (title, s_id, mb_id, target_edition) = self
            .db
            .call(move |conn| {
                let info: Option<(String, Option<String>, Option<i64>, Option<String>)> = conn
                    .query_row(
                        "SELECT m.title, sr.series_id, sr.mangabaka_id, sr.edition
                         FROM manga m
                         LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                         WHERE m.id = ?1",
                        [manga_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let (t, s, mb, ed) = info.ok_or_else(|| AppError::not_found("Manga title not found"))?;
                let sid = s.unwrap_or_else(|| format!("title:{}", normalize_title(&t)));
                let edition = ed.unwrap_or_else(|| "Standard".to_string());
                Ok((t, sid, mb, edition))
            })
            .await?;

        let norm_series_id = format!("title:{}", normalize_title(&title));
        let c_id_str = candidate_id.to_string();
        let sid_query = s_id.clone();

        // 2. Load persisted candidate
        let candidate_opt: Option<PersistedCandidate> = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT series_id, volume_number, candidate_id, provider, provider_item_id,
                            title, raw_title, match_type, size_bytes, uploader, is_trusted, is_digital,
                            score, score_explanation, download_url, magnet_url, info_hash,
                            seeders, leechers, published_at
                     FROM discovered_release_candidates
                     WHERE (series_id = ?1 OR series_id = ?2)
                       AND ABS(volume_number - ?3) < 0.001
                       AND candidate_id = ?4
                     LIMIT 1",
                )?;

                let cand = stmt
                    .query_row(
                        params![sid_query, norm_series_id, volume, c_id_str],
                        |r| {
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
                        },
                    )
                    .optional()?;

                Ok(cand)
            })
            .await?;

        let candidate = candidate_opt.ok_or_else(|| {
            AppError::not_found(format!(
                "Candidate '{candidate_id}' not found or stale for Volume {volume}"
            ))
        })?;

        // 3. Provider & Backend verification
        let (cfg, has_token) = self.load_internal_config().await?;
        let (backend, provider_ok, unconf_reason) = if candidate.provider == "nyaa" {
            let ok = cfg.torbox_enabled && has_token;
            let reason = if !cfg.torbox_enabled {
                Some("TorBox download backend is disabled in Settings".to_string())
            } else if !has_token {
                Some("TorBox API token is not configured. Add it in Settings.".to_string())
            } else {
                None
            };
            ("torbox".to_string(), ok, reason)
        } else if candidate.provider == "madokami" {
            let ok = cfg.madokami_enabled && cfg.has_madokami_credentials;
            let reason = if !cfg.madokami_enabled {
                Some("Madokami download backend is disabled in Settings".to_string())
            } else if !cfg.has_madokami_credentials {
                Some("Madokami credentials are not configured. Add them in Settings.".to_string())
            } else {
                None
            };
            ("madokami".to_string(), ok, reason)
        } else {
            ("unsupported".to_string(), false, Some(format!("Provider '{}' does not support automated acquisition", candidate.provider)))
        };

        // 4. Recheck ownership (Komga + Canonical FS + Durable publication history)
        let mut is_currently_owned = false;
        let mut lib_dir_str = "/srv/media/manga".to_string();
        if let Some(pipeline) = &self.pipeline {
            let title_ownership = pipeline.ownership.get_title_ownership(manga_id, &title).await;
            if title_ownership.volumes.iter().any(|vo| {
                (vo.volume - volume).abs() < 0.001 && vo.status == OwnershipStatus::Owned
            }) {
                is_currently_owned = true;
            }

            let (pipe_cfg, _, _) = pipeline.load_internal_config().await?;
            if let Some(ref l_dir) = pipe_cfg.library_dir {
                lib_dir_str = l_dir.clone();
                let canonical_target = Path::new(l_dir).join(&title);
                if canonical_target.is_dir()
                    && self.canonical_file_exists_for_unit(
                        &canonical_target,
                        &AcquisitionReleaseUnit::Volume(volume),
                    )
                {
                    is_currently_owned = true;
                }
            }
        }

        let is_in_history: bool = self
            .db
            .call(move |conn| {
                let vol_int = volume.round() as i64;
                let pat1 = format!("%v{vol_int}.cbz");
                let pat2 = format!("%v{:02}.cbz", vol_int);
                let pat3 = format!("%Vol%{vol_int}.cbz");
                let pat4 = format!("%Volume%{vol_int}.cbz");
                let pat5 = format!("%v{vol_int}%");
                let pat6 = format!("%v{:02}%", vol_int);

                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM acquisition_history
                            WHERE manga_id = ?1
                              AND outcome IN ('published', 'completed')
                              AND (
                                canonical_path LIKE ?2
                                OR canonical_path LIKE ?3
                                OR canonical_path LIKE ?4
                                OR canonical_path LIKE ?5
                                OR canonical_path LIKE ?6
                                OR canonical_path LIKE ?7
                                OR release_name LIKE ?2
                                OR release_name LIKE ?3
                                OR release_name LIKE ?4
                                OR release_name LIKE ?5
                                OR release_name LIKE ?6
                                OR release_name LIKE ?7
                              )
                        )",
                        params![manga_id, pat1, pat2, pat3, pat4, pat5, pat6],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);

                Ok(exists)
            })
            .await?;

        if is_in_history {
            is_currently_owned = true;
        }

        // 5. Active acquisition check
        let hash_check = candidate.info_hash.clone();
        let is_active_acquiring: bool = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.name, c.chapter_number, d.torrent_hash
                     FROM downloads d
                     JOIN chapters c ON c.id = d.chapter_id
                     WHERE c.manga_id = ?1 AND d.state != 'failed'",
                )?;

                let rows = stmt.query_map([manga_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, Option<String>>(2)?))
                })?;

                for row in rows.flatten() {
                    let (name, num, thash) = row;
                    if let Some(ref h1) = hash_check {
                        if let Some(ref h2) = thash {
                            if h1.eq_ignore_ascii_case(h2) {
                                return Ok(true);
                            }
                        }
                    }

                    let parsed = crate::release::parse_release(&name, num);
                    if parsed.release_type == crate::release::ReleaseType::Volume {
                        if (parsed.volume.unwrap_or(num) - volume).abs() < 0.001 {
                            return Ok(true);
                        }
                    } else if (num - volume).abs() < 0.001 {
                        return Ok(true);
                    }
                }

                Ok(false)
            })
            .await?;

        // 6. Hard eligibility check
        let mut hard_elig = crate::acquisition::quality::evaluate_hard_eligibility(
            &candidate,
            is_currently_owned,
            is_active_acquiring,
        );

        if !provider_ok {
            hard_elig.is_eligible = false;
            hard_elig.rejection_code = Some(if candidate.provider == "nyaa" {
                "TORBOX_UNCONFIGURED".to_string()
            } else {
                "PROVIDER_UNCONFIGURED".to_string()
            });
            hard_elig.rejection_reason = unconf_reason;
        }

        // 7. Staging and canonical destinations
        let safe_cid = candidate.candidate_id.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
        let staging_destination = format!("{lib_dir_str}/_staging/vol_{:.0}_{safe_cid}/", volume);
        let canonical_destination = format!("{lib_dir_str}/{title}/{title} - v{:02}.cbz", volume.round() as i64);

        // 8. Global dry-run status
        let auto_dry = self.db.call(|conn| Ok(crate::acquisition::auto_acquire::is_auto_acquire_dry_run(conn))).await?;
        let is_dry_run_active = auto_dry || cfg.auto_acquire_dry_run;

        // 9. Generate authorization token if eligible and not owned/acquiring
        let manual_eligible = (hard_elig.is_eligible
            || hard_elig.rejection_code.as_deref() == Some("PACK_REQUIRES_MANUAL")
            || hard_elig.rejection_code.as_deref() == Some("RANGE_REQUIRES_MANUAL"))
            && !is_currently_owned
            && !is_active_acquiring
            && provider_ok;

        let (auth_token, expires_at) = if manual_eligible {
            let token = generate_secure_auth_token();
            let exp = now() + 900; // 15 minutes
            let t_insert = token.clone();
            let p_cand_id = candidate.candidate_id.clone();
            let p_prov = candidate.provider.clone();
            let p_bk = backend.clone();
            let p_ed = target_edition.clone();
            let p_sid = s_id.clone();

            self.db.call(move |conn| {
                conn.execute(
                    "INSERT INTO acquisition_execution_authorizations (
                        id, manga_id, mangabaka_id, series_id, edition, volume_number,
                        candidate_id, provider, backend, user_id, created_at, expires_at,
                        consumed_at, status
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, 'authorized')",
                    params![
                        t_insert,
                        manga_id,
                        mb_id,
                        p_sid,
                        p_ed,
                        volume,
                        p_cand_id,
                        p_prov,
                        p_bk,
                        user_id,
                        now(),
                        exp,
                    ],
                )?;
                Ok(())
            }).await?;

            (Some(token), Some(exp))
        } else {
            (None, None)
        };

        let is_auto_publish = self
            .db
            .call(|conn| Ok(crate::acquisition::auto_acquire::is_auto_publish_manual_enabled(conn)))
            .await?;

        Ok(ManualAcquisitionReviewDto {
            catalog_title_id: manga_id,
            series_title: title,
            mangabaka_id: mb_id,
            official_edition: target_edition,
            official_volume_number: volume,
            candidate_id: candidate.candidate_id,
            candidate_title: candidate.title,
            provider: candidate.provider,
            acquisition_backend: backend,
            ownership_status: if is_currently_owned { "Owned".to_string() } else { "Missing".to_string() },
            is_currently_owned,
            is_eligible: hard_elig.is_eligible,
            eligibility_code: hard_elig.rejection_code,
            eligibility_reason: hard_elig.rejection_reason,
            staging_destination,
            canonical_destination,
            dry_run: is_dry_run_active,
            publication_requires_separate_approval: !is_auto_publish,
            authorization_token: auth_token,
            expires_at,
        })
    }
}

fn generate_secure_auth_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_ok() {
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        format!("auth_{hex}")
    } else {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("auth_{}_{}", now(), nanos)
    }
}
