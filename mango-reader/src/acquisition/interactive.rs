//! Bounded interactive planning on the existing frozen plan/execution model.
use super::{
    candidates::NormalizedAcquisitionCandidate,
    edition::normalize_edition,
    missing::MissingContentSummary,
    planner::{AcquisitionPlan, AcquisitionPlanItem, CandidateExplanation},
    profiles::AcquisitionProfile,
    service::AcquisitionService,
};
use crate::error::{AppError, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, future::Future, time::Duration};

pub const PROVIDER_DEADLINE: Duration = Duration::from_secs(12);
pub const PLANNING_DEADLINE: Duration = Duration::from_secs(30);

pub async fn bounded<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(duration, operation)
        .await
        .map_err(|_| {
            AppError::new(
                504,
                "planning_timeout",
                "Release discovery timed out. Try Again or choose another source.",
            )
        })?
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlanningRequest {
    pub title: String,
    pub komga_series_id: Option<String>,
    pub source_id: Option<String>,
    pub content_mode: Option<String>,
    pub requested_volumes: Option<Vec<f64>>,
    pub requested_edition: Option<String>,
    pub selected_release_id: Option<String>,
    pub selected_archive: Option<super::edition::SelectedArchive>,
    #[serde(default)]
    pub use_release_anyway: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractivePlan {
    #[serde(flatten)]
    pub plan: AcquisitionPlan,
    pub editions: Vec<String>,
    pub candidates: Vec<NormalizedAcquisitionCandidate>,
    pub diagnostics: Vec<String>,
}

// Bound range expansion before ranking, which is synchronous and cannot be
// interrupted by an async request deadline once a provider response arrives.
fn bounded_coverage(unit: &super::AcquisitionReleaseUnit) -> bool {
    use super::AcquisitionReleaseUnit::*;
    let range = |start: f64, end: f64| {
        start.is_finite() && end.is_finite() && start >= 0.0 && end >= start && end - start < 500.0
    };
    match unit {
        Collection { start, end } => range(*start, *end),
        MultiRange(ranges) => {
            ranges.len() <= 500
                && ranges.iter().all(|(start, end)| range(*start, *end))
                && ranges
                    .iter()
                    .map(|(start, end)| end - start + 1.0)
                    .sum::<f64>()
                    <= 500.0
        }
        MixedCollection {
            volume_start,
            volume_end,
            chapter_start,
            chapter_end,
        } => range(*volume_start, *volume_end) && range(*chapter_start, *chapter_end),
        Volume(unit) | Chapter(unit) => unit.is_finite() && *unit >= 0.0,
        Unknown => true,
    }
}

pub fn is_override(candidate: &NormalizedAcquisitionCandidate) -> bool {
    candidate.provider_metadata["identityOverride"] == true
}

pub fn prepare_plan(
    manga_id: i64,
    title: &str,
    profile: &AcquisitionProfile,
    missing: &MissingContentSummary,
    mut candidates: Vec<NormalizedAcquisitionCandidate>,
    req: &PlanningRequest,
    diagnostics: Vec<String>,
) -> Result<InteractivePlan> {
    let mut editions = candidates
        .iter()
        .filter_map(|c| c.edition.clone())
        .collect::<Vec<_>>();
    editions.sort();
    editions.dedup();
    let selected = req
        .selected_release_id
        .as_ref()
        .map(|id| {
            candidates
                .iter()
                .find(|c| &c.provider_release_id == id)
                .cloned()
                .ok_or_else(|| {
                    AppError::bad_request("Selected release is no longer available; discover again")
                })
        })
        .transpose()?;
    let edition = req
        .requested_edition
        .as_deref()
        .map(normalize_edition)
        .or_else(|| selected.as_ref().and_then(|c| c.edition.clone()));
    if let (Some(ed), Some(c)) = (&edition, &selected) {
        if c.edition.as_ref().is_some_and(|source| source != ed) {
            return Err(AppError::new(422, "edition_mismatch", "Selected release targets a different edition. Choose matching edition-local units."));
        }
    }
    if req.use_release_anyway && selected.is_none() {
        return Err(AppError::bad_request(
            "Use Release Anyway requires one explicitly selected release",
        ));
    }
    let explicit_units = req.requested_volumes.clone().unwrap_or_default();
    if explicit_units.len() > 500 || explicit_units.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(AppError::bad_request(
            "Choose at most 500 valid edition-local volumes",
        ));
    }
    // Nonstandard editions never inherit standard tracker numbering/totals.
    let mut units = if req.content_mode.as_deref() == Some("specific") {
        explicit_units
    } else if edition.as_deref().is_some_and(|ed| ed != "standard") {
        selected
            .as_ref()
            .map(|c| c.release_unit.included_volumes())
            .unwrap_or_default()
    } else {
        missing.missing_volumes.clone()
    };
    if units.is_empty()
        && !edition.as_deref().is_some_and(|ed| ed != "standard")
        && req.content_mode.as_deref() != Some("specific")
    {
        units = selected
            .as_ref()
            .map(|c| c.release_unit.included_volumes())
            .unwrap_or_else(|| {
                candidates
                    .iter()
                    .flat_map(|c| c.release_unit.included_volumes())
                    .collect()
            });
        units.retain(|unit| {
            !missing.owned_volumes.contains(unit) && !missing.acquiring_volumes.contains(unit)
        });
    }
    units.sort_by(f64::total_cmp);
    units.dedup();
    if let Some(archive) = &req.selected_archive {
        if selected.is_none()
            || units.len() != 1
            || archive.path.is_empty()
            || archive.path.len() > 4096
            || archive.size == 0
        {
            return Err(AppError::bad_request("Explicit archive selection requires one release, one edition-local unit, and an exact path/nonzero size"));
        }
    }
    let mut context = missing.clone();
    context.missing_volumes = units.clone();
    for candidate in &mut candidates {
        let detected = candidate.edition.clone();
        let compatible = edition.as_ref().is_none_or(|ed| {
            detected
                .as_ref()
                .map_or(ed == "standard", |source| source == ed)
        });
        if compatible
            && (req.requested_edition.is_some()
                || req.selected_release_id.as_deref() == Some(&candidate.provider_release_id))
        {
            let intent = edition.clone().or_else(|| detected.clone());
            candidate.provider_metadata["requestedEdition"] = serde_json::json!(intent);
        }
        candidate.provider_metadata["interactive"] = serde_json::json!(true);
        candidate.provider_metadata["requestedUnits"] = serde_json::json!(units);
        if req.selected_release_id.as_deref() == Some(&candidate.provider_release_id) {
            candidate.provider_metadata["selectedArchive"] =
                serde_json::json!(req.selected_archive);
            candidate.provider_metadata["identityOverride"] =
                serde_json::json!(req.use_release_anyway);
            if req.use_release_anyway && candidate.provider_metadata["requestedEdition"].is_null() {
                candidate.provider_metadata["requestedEdition"] =
                    serde_json::json!(format!("source {}", candidate.provider_release_id));
            }
        }
    }
    let eligible = candidates
        .iter()
        .filter(|c| {
            let ed = c.edition.as_deref().unwrap_or("standard");
            edition.as_deref().is_none_or(|selected| {
                selected == ed || (req.use_release_anyway && c.edition.is_none())
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut plan = super::planner::generate_plan(
        manga_id,
        title,
        profile,
        &context,
        &eligible,
        &HashMap::new(),
        true,
    );
    if let Some(id) = &req.selected_release_id {
        let candidate = candidates
            .iter()
            .find(|c| &c.provider_release_id == id)
            .unwrap()
            .clone();
        plan.selected_items.clear();
        if !units.is_empty() && (candidate.confidence == "high" || req.use_release_anyway) {
            // Explicit units express user intent, not inferred source coverage. Retrieval
            // still requires labelled unit evidence (or a future explicit file mapping).
            plan.selected_items = units.iter().map(|unit| AcquisitionPlanItem {
                candidate: candidate.clone(), units_covered: vec![*unit], duplicate_units: vec![],
                explanation: CandidateExplanation { candidate_id: id.clone(), candidate_title: candidate.title.clone(), provider: candidate.provider.clone(),
                    why_recommended: vec!["Explicit release and edition-local unit; archive must corroborate selection".into()], why_lower: vec![] },
            }).collect();
        }
        plan.state = if plan.selected_items.is_empty() {
            "draft"
        } else {
            "ready"
        }
        .into();
        plan.stale_reason = if units.is_empty() {
            Some("Specify edition-local volumes; catalog totals cannot establish this release's numbering.".into())
        } else if plan.selected_items.is_empty() {
            Some("Exact catalog identity is uncertain. Review Use Release Anyway.".into())
        } else {
            None
        };
        plan.missing_units_covered_count = if plan.selected_items.is_empty() {
            0
        } else {
            units.len()
        };
        plan.total_acquisitions_count = plan.selected_items.len();
        plan.total_estimated_size = if plan.selected_items.is_empty() {
            0
        } else {
            candidate.size.unwrap_or(0)
        };
    }
    plan.alternatives = candidates.clone();
    Ok(InteractivePlan {
        plan,
        editions,
        candidates,
        diagnostics,
    })
}

impl AcquisitionService {
    pub async fn plan_interactive(&self, req: PlanningRequest) -> Result<InteractivePlan> {
        bounded(
            PLANNING_DEADLINE,
            self.plan_interactive_inner(req, PROVIDER_DEADLINE),
        )
        .await
    }
    async fn plan_interactive_inner(
        &self,
        req: PlanningRequest,
        provider_deadline: Duration,
    ) -> Result<InteractivePlan> {
        let title = req.title.trim().to_string();
        if title.is_empty() {
            return Err(AppError::bad_request("Title cannot be empty"));
        }
        if !matches!(
            req.content_mode.as_deref().unwrap_or("missing"),
            "missing" | "complete" | "specific"
        ) {
            return Err(AppError::bad_request("Unknown content selection mode"));
        }
        let source = req.source_id.as_deref().unwrap_or("auto");
        if !matches!(source, "auto" | "nyaa" | "mangadot") {
            return Err(AppError::bad_request(
                "This source does not support interactive volume planning",
            ));
        }
        let title_db = title.clone();
        let komga = req.komga_series_id.clone();
        let manga_id = self
            .db
            .call(move |conn| {
                // Catalog IDs and manga IDs belong to different tables. Never conflate them.
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM manga WHERE title = ?1 ORDER BY id LIMIT 1",
                        [&title_db],
                        |r| r.get(0),
                    )
                    .optional()?;
                let id = if let Some(id) = existing {
                    id
                } else {
                    conn.execute(
                        "INSERT INTO manga (source_id, url, title) VALUES (0, ?1, ?2)",
                        rusqlite::params![format!("catalog:{title_db}"), title_db],
                    )?;
                    conn.last_insert_rowid()
                };
                crate::sync::catalog::add_or_update_tracked(
                    conn,
                    &title_db,
                    "lists",
                    komga.as_deref(),
                    Some(id),
                )?;
                Ok(id)
            })
            .await?;
        let profile = self.get_manga_profile(manga_id).await?;
        let missing = bounded(provider_deadline, self.get_title_missing_content(manga_id)).await?;
        let (config, _) = self.load_internal_config().await?;
        let mut candidates = vec![];
        let mut diagnostics = vec![];
        if matches!(source, "auto" | "nyaa") {
            if config.nyaa_enabled {
                // One bounded title query. Interactive planning does not wait on a
                // sequential alias/source search chain or repeat ownership lookups.
                let client = super::nyaa::NyaaClient::new(self.http.clone(), config.nyaa_url);
                match bounded(
                    provider_deadline,
                    client.search(&title, config.nyaa_category.as_deref()),
                )
                .await
                {
                    Ok(mut releases) => {
                        let count = releases.len();
                        releases.retain(|release| bounded_coverage(&release.release_unit));
                        if releases.len() != count {
                            diagnostics.push("Nyaa: an oversized or invalid coverage range was rejected; select a supported release.".into());
                        }
                        let ranked = super::nyaa::rank_candidates_smart(
                            releases,
                            &title,
                            &[],
                            None,
                            &missing.owned_volumes,
                            &missing.missing_volumes,
                            None,
                            None,
                            None,
                            None,
                            None,
                        );
                        candidates.extend(ranked.iter().map(|c| {
                            NormalizedAcquisitionCandidate::from_nyaa_candidate(
                                &title,
                                c,
                                &profile.preferred_uploaders,
                                &profile.language,
                            )
                        }));
                    }
                    Err(error) => diagnostics.push(format!(
                        "Nyaa: {}",
                        super::torbox::diagnostic_text(&error.message)
                    )),
                }
            } else {
                diagnostics.push("Nyaa is disabled in Settings".into());
            }
        }
        if matches!(source, "auto" | "mangadot") {
            let series = title.clone();
            let language = profile.language.clone();
            let local = self
                .db
                .call(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT name FROM chapters WHERE manga_id = ?1 AND url NOT LIKE 'nyaa:%'",
                    )?;
                    let names = stmt
                        .query_map([manga_id], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(names
                        .iter()
                        .filter_map(|name| {
                            crate::release::parse_volume(name).map(|v| {
                                NormalizedAcquisitionCandidate::from_mangadot_release(
                                    &series,
                                    v,
                                    name,
                                    None,
                                    &language,
                                    super::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
                                )
                            })
                        })
                        .collect::<Vec<_>>())
                })
                .await?;
            candidates.extend(local);
        }
        if candidates.is_empty() && !diagnostics.is_empty() {
            return Err(AppError::new(
                502,
                "discovery_failed",
                diagnostics.join("; "),
            ));
        }
        let result = prepare_plan(
            manga_id,
            &title,
            &profile,
            &missing,
            candidates,
            &req,
            diagnostics,
        )?;
        let saved = result.plan.clone();
        self.db
            .call(move |conn| super::planner::save_plan(conn, &saved))
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquisition::{candidates::UnitMappingProvenance, AcquisitionReleaseUnit};
    fn missing() -> MissingContentSummary {
        super::super::missing::calculate_missing_content(
            1,
            "Series X",
            &[],
            &[],
            Some(12),
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
        )
    }
    fn candidate(edition: Option<&str>) -> NormalizedAcquisitionCandidate {
        let mut c = NormalizedAcquisitionCandidate::from_mangadot_release(
            "Series X",
            4.0,
            "Series X",
            Some(100),
            "en",
            UnitMappingProvenance::ParsedVolumeFromTitle,
        );
        c.provider = "nyaa".into();
        c.provider_release_id = "release4".into();
        c.confidence = "high".into();
        c.edition = edition.map(normalize_edition);
        c.title = edition
            .map(|ed| format!("Series X {ed} (2016-2017) (Digital)"))
            .unwrap_or_else(|| "Series X".into());
        c.release_unit = AcquisitionReleaseUnit::Unknown;
        c.provider_metadata = serde_json::json!({"rawTitle":c.title});
        c
    }
    fn request() -> PlanningRequest {
        PlanningRequest {
            title: "Series X".into(),
            selected_release_id: Some("release4".into()),
            requested_volumes: Some(vec![4.0]),
            content_mode: Some("specific".into()),
            ..Default::default()
        }
    }
    #[test]
    fn provider_ranges_are_bounded_before_ranking() {
        assert!(!bounded_coverage(&AcquisitionReleaseUnit::Collection {
            start: 1.0,
            end: 1e12
        }));
        assert!(bounded_coverage(&AcquisitionReleaseUnit::Collection {
            start: 1.0,
            end: 39.0
        }));
    }
    #[test]
    fn explicit_master_release_propagates_without_override() {
        let releases = super::super::nyaa::parse_nyaa_rss(
            r#"<rss><channel><item>
            <title>Series X Master Edition (2016-2017) (Digital) (Group)</title>
            <link>https://nyaa.si/view/44</link><guid>https://nyaa.si/view/44</guid>
            <nyaa:categoryId>3_1</nyaa:categoryId><nyaa:seeders>10</nyaa:seeders>
            <nyaa:size>1 GiB</nyaa:size><nyaa:infoHash>0123456789012345678901234567890123456789</nyaa:infoHash>
            </item></channel></rss>"#,
        );
        let ranked = super::super::nyaa::rank_candidates_smart(
            releases,
            "Series X",
            &[],
            None,
            &[],
            &[4.0],
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(ranked.len(), 1);
        let actual =
            NormalizedAcquisitionCandidate::from_nyaa_candidate("Series X", &ranked[0], &[], "en");
        assert_eq!(actual.confidence, "high");
        let mut req = request();
        req.selected_release_id = Some(actual.provider_release_id.clone());
        let result = prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![actual],
            &req,
            vec![],
        )
        .unwrap();
        assert_eq!(result.plan.selected_items.len(), 1);
        assert_eq!(
            result.plan.selected_items[0].candidate.provider_metadata["requestedEdition"],
            "master"
        );
        assert!(!is_override(&result.plan.selected_items[0].candidate));
        assert_eq!(result.plan.selected_items[0].units_covered, vec![4.0]);
    }
    #[test]
    fn edition_change_never_reuses_standard_totals() {
        let mut req = request();
        req.content_mode = Some("complete".into());
        let result = prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![candidate(Some("Master Edition"))],
            &req,
            vec![],
        )
        .unwrap();
        assert!(result.plan.selected_items.is_empty());
        assert!(result.plan.missing_units.is_empty());
    }
    #[test]
    fn genuine_mismatch_rejects_even_override() {
        let mut req = request();
        req.requested_edition = Some("Standard Edition".into());
        req.use_release_anyway = true;
        assert_eq!(
            prepare_plan(
                1,
                "Series X",
                &AcquisitionProfile::default(),
                &missing(),
                vec![candidate(Some("Master Edition"))],
                &req,
                vec![]
            )
            .unwrap_err()
            .code,
            "edition_mismatch"
        );
    }
    #[test]
    fn uncertain_identity_requires_explicit_release_override() {
        let mut c = candidate(None);
        c.confidence = "review_recommended".into();
        let mut req = request();
        assert!(prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![c.clone()],
            &req,
            vec![]
        )
        .unwrap()
        .plan
        .selected_items
        .is_empty());
        req.use_release_anyway = true;
        let p = prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![c],
            &req,
            vec![],
        )
        .unwrap()
        .plan;
        assert!(is_override(&p.selected_items[0].candidate));
        assert_eq!(
            p.selected_items[0].candidate.provider_metadata["requestedEdition"],
            "source release4"
        );
        req.selected_release_id = None;
        assert!(prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![],
            &req,
            vec![]
        )
        .is_err());
    }
    #[tokio::test]
    async fn pending_provider_is_bounded_and_error_not_swallowed() {
        let result: Result<()> = bounded(Duration::from_millis(5), std::future::pending()).await;
        assert_eq!(result.unwrap_err().code, "planning_timeout");
        let result: Result<()> = bounded(Duration::from_secs(1), async {
            Err(AppError::new(502, "provider_error", "Unavailable"))
        })
        .await;
        assert_eq!(result.unwrap_err().code, "provider_error");
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::{
        acquisition::{
            edition::RetrievalIdentity,
            execution::{create_execution, ApprovePlanRequest},
            AcquisitionReleaseUnit,
        },
        db::Db,
    };
    #[tokio::test]
    async fn approval_freezes_edition_override_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "mango-interactive-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("test.db")).unwrap();
        db.call(|conn| {
            conn.execute(
                "INSERT INTO manga (id,source_id,url,title) VALUES (1,0,'/s','Series X')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let service = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
        let profile = service.get_manga_profile(1).await.unwrap();
        let missing = super::super::missing::calculate_missing_content(
            1,
            "Series X",
            &[],
            &[],
            Some(4),
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
        );
        let mut candidate = NormalizedAcquisitionCandidate::from_mangadot_release(
            "Series X",
            4.0,
            "Series X Master Edition",
            Some(100),
            "en",
            super::super::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
        );
        candidate.edition = Some("master".into());
        candidate.provider_metadata = serde_json::json!({"rawTitle":"Series X Master Edition"});
        let req = PlanningRequest {
            title: "Series X".into(),
            selected_release_id: Some(candidate.provider_release_id.clone()),
            content_mode: Some("specific".into()),
            requested_volumes: Some(vec![4.0]),
            use_release_anyway: true,
            selected_archive: Some(super::super::edition::SelectedArchive {
                path: "Series X Master Edition v04.cbz".into(),
                size: 100,
            }),
            ..Default::default()
        };
        let plan = prepare_plan(
            1,
            "Series X",
            &profile,
            &missing,
            vec![candidate],
            &req,
            vec![],
        )
        .unwrap()
        .plan;
        let saved = plan.clone();
        db.call(move |conn| super::super::planner::save_plan(conn, &saved))
            .await
            .unwrap();
        assert!(service
            .approve_and_create_execution(
                1,
                ApprovePlanRequest {
                    plan_id: plan.id.clone(),
                    approved: true,
                    override_confirmed: false
                }
            )
            .await
            .unwrap_err()
            .message
            .contains("explicit confirmation"));
        let exec = service
            .approve_and_create_execution(
                1,
                ApprovePlanRequest {
                    plan_id: plan.id.clone(),
                    approved: true,
                    override_confirmed: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            exec.items[0].frozen_candidate.provider_metadata["requestedEdition"],
            "master"
        );
        assert!(is_override(&exec.items[0].frozen_candidate));
        let duplicate = db
            .call(move |conn| create_execution(conn, &plan, &missing))
            .await
            .unwrap();
        assert_eq!(exec.id, duplicate.id);
        let reloaded = service.get_execution(&exec.id).await.unwrap().unwrap();
        assert_eq!(
            reloaded.items[0].frozen_candidate,
            exec.items[0].frozen_candidate
        );
        let library = dir.to_string_lossy().to_string();
        db.call(move |conn| {
            crate::settings::set(conn, "pipeline.library_dir", &serde_json::json!(library))?;
            Ok(())
        })
        .await
        .unwrap();
        let queued = service.start_execution(exec.id.clone()).await.unwrap();
        let chapter = queued.items[0].download_job_id.unwrap();
        let json: String = db
            .call(move |conn| {
                Ok(conn.query_row(
                    "SELECT retrieval_identity_json FROM downloads WHERE chapter_id=?1",
                    [chapter],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        let queued_identity: RetrievalIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(queued_identity.requested_edition.as_deref(), Some("master"));
        assert_eq!(
            queued_identity.selected_archive.as_ref().unwrap().path,
            "Series X Master Edition v04.cbz"
        );
        assert!(queued_identity.identity_override);
        assert_eq!(queued_identity.override_approved_at, Some(exec.approved_at));
        let restarted_db = Db::open(&dir.join("test.db")).unwrap();
        let restored: String = restarted_db
            .call(move |conn| {
                Ok(conn.query_row(
                    "SELECT retrieval_identity_json FROM downloads WHERE chapter_id=?1",
                    [chapter],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(restored, json);

        let mut identity = RetrievalIdentity::new(
            "Series X",
            "Series X Master Edition",
            AcquisitionReleaseUnit::Volume(4.0),
            Some("master"),
        );
        identity.identity_override = true;
        identity.override_approved_at = Some(exec.approved_at);
        assert_eq!(
            serde_json::from_str::<RetrievalIdentity>(&serde_json::to_string(&identity).unwrap())
                .unwrap(),
            identity
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::{
        acquisition::{service::AcquisitionConfigPatch, AcquisitionReleaseUnit},
        db::Db,
    };
    #[tokio::test]
    async fn auto_keeps_local_results_and_reports_provider_failure() {
        use axum::{http::StatusCode, routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stalled = stalled.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/",
                    get(move || {
                        let stalled = server_stalled.clone();
                        async move {
                            if stalled.load(std::sync::atomic::Ordering::Relaxed) {
                                std::future::pending::<()>().await;
                            }
                            StatusCode::BAD_GATEWAY
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let path = std::env::temp_dir().join(format!(
            "mango-auto-{}-{}.db",
            std::process::id(),
            crate::db::now()
        ));
        let db = Db::open(&path).unwrap();
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id,source_id,url,title) VALUES (90,1,'/s','Series X')",[])?;
            conn.execute("INSERT INTO chapters (manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (90,'/v4','Volume 4',4,0,0)",[])?;Ok(())
        }).await.unwrap();
        let service = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
        service
            .save_config(AcquisitionConfigPatch {
                nyaa_enabled: Some(true),
                nyaa_url: Some(format!("http://{addr}")),
                nyaa_category: None,
                torbox_enabled: None,
                torbox_url: None,
                torbox_token: None,
            })
            .await
            .unwrap();
        let result = service
            .plan_interactive(PlanningRequest {
                title: "Series X".into(),
                source_id: Some("auto".into()),
                content_mode: Some("specific".into()),
                requested_volumes: Some(vec![4.0]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            result.plan.manga_id, 90,
            "Catalog IDs must never replace manga IDs"
        );
        assert!(!result.diagnostics.is_empty());
        assert!(!result.plan.selected_items.is_empty());
        assert_eq!(
            result.plan.selected_items[0].candidate.release_unit,
            AcquisitionReleaseUnit::Volume(4.0)
        );
        let fail = service
            .plan_interactive(PlanningRequest {
                title: "Series X".into(),
                source_id: Some("nyaa".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(fail.code, "discovery_failed");
        stalled.store(true, std::sync::atomic::Ordering::Relaxed);
        let result = service
            .plan_interactive_inner(
                PlanningRequest {
                    title: "Series X".into(),
                    source_id: Some("auto".into()),
                    content_mode: Some("specific".into()),
                    requested_volumes: Some(vec![4.0]),
                    ..Default::default()
                },
                Duration::from_millis(10),
            )
            .await
            .unwrap();
        assert!(!result.plan.selected_items.is_empty());
        assert!(result
            .diagnostics
            .iter()
            .any(|reason| reason.contains("timed out")));
        server.abort();
        std::fs::remove_file(path).unwrap();
    }
}
