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
        if candidate.provider != "nyaa" {
            if candidate.provider == "madokami" {
                return Err(AppError::bad_request(
                    "Madokami provider is currently unavailable for acquisition",
                ));
            }
            return Err(AppError::bad_request(format!(
                "Provider '{}' does not support automated acquisition in Mango",
                candidate.provider
            )));
        }

        let (cfg, has_token) = self.load_internal_config().await?;
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
                let pat2 = format!("%Vol% {vol_int}.cbz");
                let pat3 = format!("%Volume% {vol_int}.cbz");
                let req_unit_vol = format!("v{vol_int}");
                let req_unit_exact = format!("Volume {vol_int}");

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
                                OR requested_unit = ?5
                                OR requested_unit = ?6
                              )
                        )",
                        params![manga_id, pat1, pat2, pat3, req_unit_vol, req_unit_exact],
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
        let (classified_unit, _, _) = crate::acquisition::nyaa::classify_release(&candidate.raw_title);
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

        self.db
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
            .await?;

        Ok(ApproveCandidateResponse {
            status: "queued".to_string(),
            message: format!("Acquisition queued for Volume {}", volume),
            execution_id: Some(started.id),
            chapter_ids,
            already_owned: false,
            already_acquiring: false,
        })
    }
}
