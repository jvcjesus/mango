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
    pub source_units: Vec<super::source_units::SourceUnit>,
    pub edition_ownership_known: bool,
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

fn uncertain_source_candidate(
    series: &str,
    release: super::AcquisitionRelease,
    profile: &AcquisitionProfile,
) -> NormalizedAcquisitionCandidate {
    let uncertain = super::AcquisitionCandidate {
        is_variant: release.is_variant,
        variant_name: release.variant_name.clone(),
        release,
        score: 0.0,
        smart_score: None,
        match_type: "ambiguous".into(),
        ownership_status: "unknown".into(),
        units_included: vec![],
        units_owned: vec![],
        units_missing: vec![],
        coverage_summary: "Catalog identity unverified; explicit source review required".into(),
        useful_efficiency_bytes: None,
    };
    let mut candidate = NormalizedAcquisitionCandidate::from_nyaa_candidate(
        series,
        &uncertain,
        &profile.preferred_uploaders,
        &profile.language,
    );
    candidate.confidence = "review_recommended".into();
    candidate.provider_metadata["requiresIdentityOverride"] = serde_json::json!(true);
    candidate
}

pub fn is_override(candidate: &NormalizedAcquisitionCandidate) -> bool {
    candidate.provider_metadata["identityOverride"] == true
}

fn confident_source_identity(candidate: &NormalizedAcquisitionCandidate) -> bool {
    candidate.provider_metadata["requiresIdentityOverride"] != true
        && (candidate.confidence == "high"
            || (candidate.provider_metadata["smartScore"]["identity"]
                .as_f64()
                .is_some_and(|score| score >= 18.0)
                && !candidate.quality_signals.edition_recognized))
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
    // Override means source-owned content, including when its edition is unknown.
    // Neither unknown nor known nonstandard editions inherit tracker numbering.
    let source_local_units =
        req.use_release_anyway || edition.as_deref().is_some_and(|ed| ed != "standard");
    let source_units: Vec<super::source_units::SourceUnit> = selected
        .as_ref()
        .and_then(|c| serde_json::from_value(c.provider_metadata["sourceUnits"].clone()).ok())
        .unwrap_or_default();
    let inspected = selected
        .as_ref()
        .is_some_and(|c| c.provider_metadata["sourceFilesInspected"] == true);
    let edition_owned: Vec<f64> = selected
        .as_ref()
        .and_then(|c| serde_json::from_value(c.provider_metadata["editionOwnedUnits"].clone()).ok())
        .unwrap_or_default();
    let edition_ownership_known = selected
        .as_ref()
        .is_some_and(|c| c.provider_metadata["editionOwnershipKnown"] == true);
    let source_volumes = source_units
        .iter()
        .filter(|u| u.eligible)
        .filter_map(|u| match u.unit {
            super::AcquisitionReleaseUnit::Volume(n) => Some(n),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut units = if req.content_mode.as_deref() == Some("specific") {
        explicit_units
    } else if source_local_units
        && req.content_mode.as_deref().unwrap_or("missing") == "missing"
        && !edition_ownership_known
    {
        vec![]
    } else if source_local_units && inspected {
        if req.content_mode.as_deref().unwrap_or("missing") == "missing" {
            if edition_ownership_known {
                source_volumes
                    .iter()
                    .copied()
                    .filter(|n| !edition_owned.contains(n))
                    .collect()
            } else {
                vec![]
            }
        } else {
            source_volumes.clone()
        }
    } else if source_local_units {
        selected
            .as_ref()
            .map(|c| c.release_unit.included_volumes())
            .unwrap_or_default()
    } else {
        missing.missing_volumes.clone()
    };
    if units.is_empty() && !source_local_units && req.content_mode.as_deref() != Some("specific") {
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
    if source_local_units
        && req.content_mode.as_deref().unwrap_or("missing") == "missing"
        && edition_ownership_known
    {
        units.retain(|n| !edition_owned.contains(n));
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
    if source_local_units {
        context.owned_volumes = edition_owned;
        context.acquiring_volumes.clear();
    }
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
            if c.provider_metadata["requiresIdentityOverride"] == true {
                return false;
            }
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
        let selected_chapter = req
            .selected_archive
            .as_ref()
            .and_then(|archive| {
                source_units
                    .iter()
                    .find(|u| u.eligible && &u.archive == archive)
            })
            .is_some_and(|u| matches!(u.unit, super::AcquisitionReleaseUnit::Chapter(_)));
        let mut selection_error = None;
        let mut resolved_items = vec![];
        for unit in &units {
            let mut per_unit = candidate.clone();
            if inspected {
                let matches = source_units
                    .iter()
                    .filter(|u| {
                        u.eligible
                            && match u.unit {
                                super::AcquisitionReleaseUnit::Volume(n) => {
                                    !selected_chapter && n == *unit
                                }
                                super::AcquisitionReleaseUnit::Chapter(n) => {
                                    selected_chapter && n == *unit
                                }
                                _ => false,
                            }
                    })
                    .filter(|u| {
                        req.selected_archive
                            .as_ref()
                            .is_none_or(|a| a == &u.archive)
                    })
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    selection_error = Some(format!("Unit {unit}: {} matching source archives. Choose one numbered archive, or another release; file order cannot establish a unit.", matches.len()));
                    break;
                }
                per_unit.provider_metadata["selectedArchive"] =
                    serde_json::json!(matches[0].archive);
                per_unit.release_unit = matches[0].unit.clone();
                per_unit.provider_metadata["requestedUnit"] = serde_json::json!(matches[0].unit);
            }
            if let Some(metadata) = per_unit.provider_metadata.as_object_mut() {
                metadata.remove("sourceUnits");
            }
            resolved_items.push(AcquisitionPlanItem {
                candidate: per_unit, units_covered: vec![*unit], duplicate_units: vec![],
                explanation: CandidateExplanation { candidate_id: id.clone(), candidate_title: candidate.title.clone(), provider: candidate.provider.clone(),
                    why_recommended: vec!["Explicit release and edition-local unit; archive must corroborate selection".into()], why_lower: vec![] },
            });
        }
        if selection_error.is_none()
            && !units.is_empty()
            && (confident_source_identity(&candidate) || req.use_release_anyway)
        {
            plan.selected_items = resolved_items;
        }
        plan.state = if plan.selected_items.is_empty() {
            "draft"
        } else {
            "ready"
        }
        .into();
        plan.stale_reason = if selection_error.is_some() {
            selection_error
        } else if units.is_empty()
            && source_local_units
            && req.content_mode.as_deref().unwrap_or("missing") == "missing"
            && !edition_ownership_known
        {
            Some("Missing-volume calculation is unavailable for this edition. Choose Complete Series or Specific Volumes using the source-local controls.".into())
        } else if units.is_empty() {
            Some("Choose source-local volumes or a numbered archive below; catalog totals cannot establish this release's numbering.".into())
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
        source_units,
        edition_ownership_known,
    })
}

pub fn edition_owned_units(
    conn: &rusqlite::Connection,
    manga_id: i64,
    edition: &str,
) -> Result<Vec<f64>> {
    let mut stmt = conn.prepare("SELECT retrieval_identity_json, canonical_path FROM acquisition_history WHERE manga_id = ?1 AND canonical_path IS NOT NULL AND outcome IN ('published','completed','verification_pending','published_scan_failed')")?;
    let rows = stmt.query_map([manga_id], |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut owned = vec![];
    for row in rows {
        let (json, path) = row?;
        let Some(identity) =
            json.and_then(|s| serde_json::from_str::<super::edition::RetrievalIdentity>(&s).ok())
        else {
            continue;
        };
        if identity
            .requested_edition
            .as_deref()
            .map(normalize_edition)
            .as_deref()
            == Some(edition)
            && std::path::Path::new(&path).is_file()
        {
            if let super::AcquisitionReleaseUnit::Volume(n) = identity.requested_unit {
                owned.push(n);
            }
        }
    }
    owned.sort_by(f64::total_cmp);
    owned.dedup();
    Ok(owned)
}

impl AcquisitionService {
    pub(super) async fn source_edition_ownership(
        &self,
        manga_id: i64,
        title: &str,
        edition: &str,
    ) -> Result<(bool, Vec<f64>)> {
        let ed = normalize_edition(edition);
        let query_ed = ed.clone();
        let mut owned = self
            .db
            .call(move |conn| edition_owned_units(conn, manga_id, &query_ed))
            .await?;
        let mut known = !owned.is_empty();
        if let Some(pipeline) = &self.pipeline {
            // Reuse exactly the canonical/ComicInfo namespace used by publication.
            let identity = super::edition::RetrievalIdentity::new(
                title,
                "",
                super::AcquisitionReleaseUnit::Unknown,
                Some(&ed),
            );
            let job = crate::downloads::Job {
                title: title.into(),
                retrieval_identity_json: Some(serde_json::to_string(&identity)?),
                ..Default::default()
            };
            let qualified = job.publication_title();
            if let Ok(ownership) = tokio::time::timeout(
                PROVIDER_DEADLINE,
                pipeline.ownership.get_title_ownership(manga_id, &qualified),
            )
            .await
            {
                if ownership.komga_connected
                    && ownership.error.is_none()
                    && ownership
                        .series_name
                        .as_deref()
                        .and_then(super::edition::edition_from_title)
                        .as_deref()
                        == Some(ed.as_str())
                {
                    known = true;
                    owned.extend(
                        ownership
                            .volumes
                            .iter()
                            .filter(|v| {
                                v.status == crate::pipeline::ownership::OwnershipStatus::Owned
                            })
                            .map(|v| v.volume),
                    );
                }
            }
        }
        owned.sort_by(f64::total_cmp);
        owned.dedup();
        Ok((known, owned))
    }
    async fn discover_source_files(
        &self,
        candidate: &NormalizedAcquisitionCandidate,
        nyaa_url: Option<&str>,
    ) -> Result<Vec<super::BackendFile>> {
        // Reuse a matching known job read-only; never submit during a preview.
        if let Some(hash) = candidate.provider_metadata["torrentHash"]
            .as_str()
            .filter(|h| !h.is_empty())
        {
            let hash = hash.to_owned();
            let query_hash = hash.clone();
            let job = self.db.call(move |conn| Ok(conn.query_row(
                "SELECT external_job_id FROM downloads WHERE torrent_hash = ?1 AND backend_type='torbox' AND external_job_id IS NOT NULL ORDER BY added_at DESC LIMIT 1", [query_hash], |row| row.get::<_,String>(0)).optional()?)).await?;
            if let Some(job) = job {
                let (config, _) = self.load_internal_config().await?;
                if config.torbox_enabled {
                    let client = super::torbox::TorBoxClient::new(
                        self.http.clone(),
                        config.torbox_url,
                        self.get_torbox_token().await?,
                    );
                    if let Ok(data) = client.get_torrent_data(&job).await {
                        if data
                            .hash
                            .as_deref()
                            .is_some_and(|actual| actual.eq_ignore_ascii_case(&hash))
                        {
                            if let Some(files) = data
                                .files
                                .filter(|files| !files.is_empty() && files.len() <= 500)
                            {
                                return Ok(files
                                    .into_iter()
                                    .map(|file| super::BackendFile {
                                        id: file.id,
                                        name: file.name.clone(),
                                        path: file.absolute_path.unwrap_or(file.name),
                                        size: file.size,
                                    })
                                    .collect());
                            }
                        }
                    }
                }
            }
        }
        let url = candidate.provider_metadata["torrentUrl"]
            .as_str()
            .ok_or_else(|| AppError::bad_request("No read-only torrent manifest is available"))?;
        let url = reqwest::Url::parse(url)
            .map_err(|_| AppError::bad_request("Invalid source manifest URL"))?;
        let origin = reqwest::Url::parse(nyaa_url.unwrap_or(super::nyaa::DEFAULT_NYAA_URL))
            .map_err(|_| AppError::bad_request("Invalid Nyaa URL"))?;
        if url.origin() != origin.origin() {
            return Err(AppError::bad_request(
                "Manifest must belong to the configured source",
            ));
        }
        let mut response = self
            .http
            .get(url)
            .timeout(PROVIDER_DEADLINE)
            .send()
            .await
            .map_err(|_| {
                AppError::new(
                    502,
                    "source_files_failed",
                    "Could not read source file manifest",
                )
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                502,
                "source_files_failed",
                "Source file manifest is unavailable",
            ));
        }
        let mut bytes = vec![];
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AppError::bad_request("Source manifest read failed"))?
        {
            if bytes.len() + chunk.len() > super::source_units::MAX_METAINFO_BYTES {
                return Err(AppError::bad_request(
                    "Source manifest exceeds planning limits",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        super::source_units::metainfo_files(&bytes)
    }
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
        let (_, aliases) = self.get_title_and_aliases(manga_id).await?;
        let missing = bounded(provider_deadline, self.get_title_missing_content(manga_id)).await?;
        let (config, _) = self.load_internal_config().await?;
        let mut candidates = vec![];
        let mut diagnostics = vec![];
        if matches!(source, "auto" | "nyaa") {
            if config.nyaa_enabled {
                // One bounded title query. Interactive planning does not wait on a
                // sequential alias/source search chain or repeat ownership lookups.
                let client =
                    super::nyaa::NyaaClient::new(self.http.clone(), config.nyaa_url.clone());
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
                        let unranked = releases.clone();
                        let ranked = super::nyaa::rank_candidates_smart(
                            releases,
                            &title,
                            &aliases,
                            None,
                            &missing.owned_volumes,
                            &missing.missing_volumes,
                            None,
                            None,
                            None,
                            None,
                            None,
                        );
                        for release in unranked {
                            if !ranked.iter().any(|candidate| {
                                candidate.release.source_item_id == release.source_item_id
                            }) {
                                candidates
                                    .push(uncertain_source_candidate(&title, release, &profile));
                            }
                        }
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
                            let parsed = crate::release::parse_release(name, -1.0);
                            parsed
                                .volume
                                .filter(|_| {
                                    parsed.release_type == crate::release::ReleaseType::Volume
                                })
                                .map(|v| {
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
        if let Some(candidate) = candidates
            .iter_mut()
            .find(|c| req.selected_release_id.as_deref() == Some(&c.provider_release_id))
        {
            if candidate.provider == "nyaa" {
                match bounded(provider_deadline, self.discover_source_files(candidate, config.nyaa_url.as_deref())).await {
                    Ok(files) => {
                        let units = super::source_units::discover(&files, candidate.provider_metadata["rawTitle"].as_str().unwrap_or(&candidate.title), candidate.edition.as_deref());
                        candidate.provider_metadata["sourceUnits"] = serde_json::json!(units);
                        candidate.provider_metadata["sourceFilesInspected"] = serde_json::json!(true);
                    }
                    Err(error) => diagnostics.push(format!("Source files: {}. Use Specific Volumes to express intent; retrieval must still corroborate the archive.", error.message)),
                }
            }
            let ed = candidate
                .edition
                .clone()
                .or_else(|| req.requested_edition.clone());
            if let Some(ed) = ed.filter(|ed| ed != "standard") {
                let (known, owned) = self.source_edition_ownership(manga_id, &title, &ed).await?;
                candidate.provider_metadata["editionOwnershipKnown"] = serde_json::json!(known);
                candidate.provider_metadata["editionOwnedUnits"] = serde_json::json!(owned);
            }
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
    fn unverified_provider_identity_is_review_only_until_override() {
        let release = super::super::nyaa::parse_nyaa_rss(r#"<item><title>Unmapped Source Name v01-12 (Digital)</title><guid>source-other</guid><nyaa:categoryId>3_1</nyaa:categoryId><nyaa:seeders>10</nyaa:seeders></item>"#).remove(0);
        let candidate =
            uncertain_source_candidate("Series X", release, &AcquisitionProfile::default());
        let automatic = PlanningRequest {
            title: "Series X".into(),
            ..Default::default()
        };
        assert!(prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![candidate.clone()],
            &automatic,
            vec![]
        )
        .unwrap()
        .plan
        .selected_items
        .is_empty());
        let mut explicit = request();
        explicit.selected_release_id = Some(candidate.provider_release_id.clone());
        assert!(prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![candidate.clone()],
            &explicit,
            vec![]
        )
        .unwrap()
        .plan
        .selected_items
        .is_empty());
        explicit.use_release_anyway = true;
        assert!(is_override(
            &prepare_plan(
                1,
                "Series X",
                &AcquisitionProfile::default(),
                &missing(),
                vec![candidate],
                &explicit,
                vec![]
            )
            .unwrap()
            .plan
            .selected_items[0]
                .candidate
        ));
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
        let mut cached_source = result.plan.selected_items[0].candidate.clone();
        cached_source.confidence = "review_recommended".into();
        cached_source.health.seeders = Some(0);
        let cache_profile = AcquisitionProfile {
            min_seeders: 0,
            ..Default::default()
        };
        let cached = prepare_plan(
            1,
            "Series X",
            &cache_profile,
            &missing(),
            vec![cached_source],
            &req,
            vec![],
        )
        .unwrap();
        assert!(
            !cached.plan.selected_items.is_empty(),
            "Health/availability review must not require an identity override for a known source"
        );
        assert!(!is_override(&cached.plan.selected_items[0].candidate));
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
        req.use_release_anyway = true;
        let unknown = prepare_plan(
            1,
            "Series X",
            &AcquisitionProfile::default(),
            &missing(),
            vec![candidate(None)],
            &req,
            vec![],
        )
        .unwrap();
        assert!(
            unknown.plan.missing_units.is_empty(),
            "Unknown overridden editions must not inherit standard catalog totals"
        );
        assert!(unknown.plan.selected_items.is_empty());
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
        let limited_profile = AcquisitionProfile {
            max_acquisition_size_bytes: Some(50),
            ..Default::default()
        };
        assert!(super::super::execution::validate_preflight_profile(
            &limited_profile,
            &p.selected_items[0].candidate
        )
        .unwrap_err()
        .contains("size limit"));
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
            conn.execute("INSERT INTO chapters (manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (90,'/v4','Volume 4',4,0,0)",[])?;
            conn.execute("INSERT INTO chapters (manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (90,'/c5','Vol. 4 Ch. 5',5,1,0)",[])?;Ok(())
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
        assert!(
            result
                .candidates
                .iter()
                .all(|candidate| candidate.title != "Vol. 4 Ch. 5"),
            "Chapter membership is not a first-class volume"
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

#[cfg(test)]
mod source_local_tests {
    use super::*;
    use crate::acquisition::{source_units, AcquisitionReleaseUnit as Unit};
    fn candidate(files: &[super::super::BackendFile]) -> NormalizedAcquisitionCandidate {
        let mut c = NormalizedAcquisitionCandidate::from_mangadot_release(
            "Series",
            4.0,
            "Series Master Edition (2016-2017)",
            Some(2100),
            "en",
            super::super::candidates::UnitMappingProvenance::ParsedVolumeFromTitle,
        );
        c.provider = "nyaa".into();
        c.provider_release_id = "source-master".into();
        c.edition = Some("master".into());
        c.release_unit = Unit::Unknown;
        c.provider_metadata = serde_json::json!({"rawTitle": c.title, "sourceFilesInspected":true, "sourceUnits":source_units::discover(files, &c.title, Some("master"))});
        c
    }
    fn request(mode: &str) -> PlanningRequest {
        PlanningRequest {
            title: "Series".into(),
            content_mode: Some(mode.into()),
            selected_release_id: Some("source-master".into()),
            ..Default::default()
        }
    }
    fn missing() -> MissingContentSummary {
        super::super::missing::calculate_missing_content(
            1,
            "Series",
            &[(4.0, None)],
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
    fn plan(c: NormalizedAcquisitionCandidate, req: &PlanningRequest) -> InteractivePlan {
        prepare_plan(
            1,
            "Series",
            &AcquisitionProfile::default(),
            &missing(),
            vec![c],
            req,
            vec![],
        )
        .unwrap()
    }
    #[test]
    fn complete_selects_six_and_specific_four_ignores_standard_totals_ownership() {
        let files = source_units::tests::files("Master Edition");
        let complete = plan(candidate(&files), &request("complete"));
        assert_eq!(complete.plan.selected_items.len(), 6);
        assert_eq!(complete.plan.missing_units, vec![1., 2., 3., 4., 5., 6.]);
        let mut req = request("specific");
        req.requested_volumes = Some(vec![4.]);
        let specific = plan(candidate(&files), &req);
        assert_eq!(specific.plan.selected_items.len(), 1);
        let item = &specific.plan.selected_items[0];
        assert_eq!(item.units_covered, vec![4.]);
        assert_eq!(item.candidate.release_unit, Unit::Volume(4.));
        assert_eq!(
            item.candidate.provider_metadata["requestedEdition"],
            "master"
        );
        assert_eq!(
            item.candidate.provider_metadata["selectedArchive"]["path"],
            files[3].path
        );
    }
    #[test]
    fn missing_requires_edition_qualified_physical_ownership() {
        let files = source_units::tests::files("Master Edition");
        let unknown = plan(candidate(&files), &request("missing"));
        assert!(unknown.plan.selected_items.is_empty());
        assert!(unknown
            .plan
            .stale_reason
            .unwrap()
            .contains("Missing-volume calculation is unavailable"));
        let mut known = candidate(&files);
        known.provider_metadata["editionOwnershipKnown"] = serde_json::json!(true);
        known.provider_metadata["editionOwnedUnits"] = serde_json::json!([4.]);
        let result = plan(known, &request("missing"));
        assert_eq!(result.plan.missing_units, vec![1., 2., 3., 5., 6.]);
        assert_eq!(result.plan.selected_items.len(), 5);
    }
    #[test]
    fn ambiguous_number_requires_exact_archive_even_under_override() {
        let mut files = source_units::tests::files("Master Edition");
        let mut alternate = files[3].clone();
        alternate.path = "Series Master Edition/Alternative v04.cbz".into();
        alternate.name = "Alternative v04.cbz".into();
        files.push(alternate.clone());
        let mut req = request("specific");
        req.requested_volumes = Some(vec![4.]);
        assert!(plan(candidate(&files), &req).plan.selected_items.is_empty());
        req.use_release_anyway = true;
        assert!(plan(candidate(&files), &req).plan.selected_items.is_empty());
        req.selected_archive = Some(super::super::edition::SelectedArchive {
            path: alternate.path,
            size: alternate.size,
        });
        assert_eq!(plan(candidate(&files), &req).plan.selected_items.len(), 1);
    }
    #[test]
    fn override_regenerates_real_units_but_cannot_invent_unlabelled_files() {
        let files = source_units::tests::files("Master Edition");
        let mut uncertain = candidate(&files);
        uncertain.confidence = "review_recommended".into();
        uncertain.provider_metadata["requiresIdentityOverride"] = serde_json::json!(true);
        assert!(plan(uncertain.clone(), &request("complete"))
            .plan
            .selected_items
            .is_empty());
        let mut req = request("complete");
        req.use_release_anyway = true;
        let accepted = plan(uncertain, &req);
        assert_eq!(accepted.plan.selected_items.len(), 6);
        assert!(accepted
            .plan
            .selected_items
            .iter()
            .all(|i| is_override(&i.candidate)));
        let files = vec![super::super::BackendFile {
            id: 1,
            name: "archive.cbz".into(),
            path: "Series Master Edition/archive.cbz".into(),
            size: 100,
        }];
        req.content_mode = Some("specific".into());
        req.requested_volumes = Some(vec![4.]);
        req.selected_archive = Some(super::super::edition::SelectedArchive {
            path: files[0].path.clone(),
            size: 100,
        });
        assert!(plan(candidate(&files), &req).plan.selected_items.is_empty());
    }
    #[test]
    fn frozen_source_selection_survives_execution_restart_and_recreated_file_ids() {
        let mut files = source_units::tests::files("Master Edition");
        let planned = plan(candidate(&files), &request("complete"));
        let dir = std::env::temp_dir().join(format!(
            "source-unit-freeze-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("test.db")).unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO manga (id,source_id,url,title) VALUES (1,0,'/source','Series')",
            [],
        )
        .unwrap();
        super::super::planner::save_plan(&conn, &planned.plan).unwrap();
        let exec =
            super::super::execution::create_execution(&conn, &planned.plan, &missing()).unwrap();
        drop(conn);
        let conn = rusqlite::Connection::open(dir.join("test.db")).unwrap();
        let restored = super::super::execution::get_execution(&conn, &exec.id)
            .unwrap()
            .unwrap();
        assert_eq!(restored.items.len(), 6);
        let item = restored
            .items
            .iter()
            .find(|i| i.expected_units == vec![4.])
            .unwrap();
        let frozen = &item.frozen_candidate;
        let mut identity = super::super::edition::RetrievalIdentity::new(
            "Series",
            &frozen.title,
            Unit::Volume(4.),
            frozen.provider_metadata["requestedEdition"].as_str(),
        );
        identity.selected_archive = Some(
            serde_json::from_value(frozen.provider_metadata["selectedArchive"].clone()).unwrap(),
        );
        files.reverse();
        for file in &mut files {
            file.id += 100;
        }
        let selected = super::super::torbox::resolve_file_with_identity(
            &files,
            None,
            None,
            None,
            Some(4),
            Some(&identity),
        )
        .unwrap();
        assert_eq!(selected.size, 400);
        for file in &mut files {
            if file.size == 400 {
                file.size += 1;
            }
        }
        assert!(super::super::torbox::resolve_file_with_identity(
            &files,
            None,
            None,
            None,
            None,
            Some(&identity)
        )
        .is_err());
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn physical_history_ownership_requires_matching_edition_and_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "source-owned-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO manga (id,source_id,url,title) VALUES (1,0,'/owned','Series')",
            [],
        )
        .unwrap();
        let path = dir.join("Master v04.cbz");
        std::fs::write(&path, b"fixture").unwrap();
        for edition in ["standard", "master"] {
            let identity = super::super::edition::RetrievalIdentity::new(
                "Series",
                "Series",
                Unit::Volume(4.),
                Some(edition),
            );
            conn.execute("INSERT INTO acquisition_history (manga_id,series,release_name,outcome,canonical_path,completed_at,retrieval_identity_json) VALUES (1,'Series','Series','completed',?1,0,?2)", rusqlite::params![path.to_str().unwrap(),serde_json::to_string(&identity).unwrap()]).unwrap();
            assert_eq!(
                edition_owned_units(&conn, 1, "master").unwrap(),
                if edition == "standard" {
                    vec![]
                } else {
                    vec![4.]
                }
            );
        }
        std::fs::remove_file(path).unwrap();
        assert!(edition_owned_units(&conn, 1, "master").unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn selected_rss_release_reads_manifest_without_submitting_downloads() {
        use axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let rss = format!(
            r#"<rss><channel><item><title>Series Master Edition (2016-2017) (Digital)</title><guid>release-master</guid><nyaa:categoryId>3_1</nyaa:categoryId><nyaa:seeders>10</nyaa:seeders><enclosure url="http://{addr}/download/1.torrent" /></item></channel></rss>"#
        );
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/",
                        get(move || {
                            let rss = rss.clone();
                            async move { rss }
                        }),
                    )
                    .route(
                        "/download/1.torrent",
                        get(|| async { source_units::tests::manifest("Master Edition") }),
                    ),
            )
            .await
            .unwrap();
        });
        let dir = std::env::temp_dir().join(format!(
            "source-manifest-http-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("test.db")).unwrap();
        let service = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
        service
            .save_config(super::super::service::AcquisitionConfigPatch {
                nyaa_enabled: Some(true),
                nyaa_url: Some(format!("http://{addr}")),
                nyaa_category: None,
                torbox_enabled: None,
                torbox_url: None,
                torbox_token: None,
            })
            .await
            .unwrap();
        let mut req = request("complete");
        req.selected_release_id = Some("release-master".into());
        let result = service.plan_interactive(req.clone()).await.unwrap();
        assert_eq!(result.source_units.len(), 6);
        assert_eq!(
            result.plan.selected_items.len(),
            6,
            "{:?}",
            result.plan.stale_reason
        );
        req.content_mode = Some("specific".into());
        req.requested_volumes = Some(vec![4.]);
        let result = service.plan_interactive(req).await.unwrap();
        assert_eq!(result.plan.selected_items.len(), 1);
        assert!(
            result.plan.selected_items[0].candidate.provider_metadata["selectedArchive"]["path"]
                .as_str()
                .unwrap()
                .ends_with("v04.cbz")
        );
        assert_eq!(
            db.call(|conn| Ok(
                conn.query_row("SELECT count(*) FROM downloads", [], |r| r.get::<_, i64>(0))?
            ))
            .await
            .unwrap(),
            0
        );
        server.abort();
        drop(service);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn explicitly_selected_chapter_keeps_its_kind() {
        let files = vec![super::super::BackendFile {
            id: 9,
            name: "Series Master Edition ch. 4.cbz".into(),
            path: "Series Master Edition/ch. 4.cbz".into(),
            size: 400,
        }];
        let mut req = request("specific");
        req.requested_volumes = Some(vec![4.]);
        req.selected_archive = Some(super::super::edition::SelectedArchive {
            path: files[0].path.clone(),
            size: 400,
        });
        let result = plan(candidate(&files), &req);
        assert_eq!(result.plan.selected_items.len(), 1);
        assert_eq!(
            result.plan.selected_items[0].candidate.provider_metadata["requestedUnit"],
            serde_json::json!({"type":"chapter","value":4.0})
        );
    }
    #[tokio::test]
    async fn existing_matching_torbox_job_can_supply_read_only_file_evidence() {
        use axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/torrents/mylist",get(|| async { axum::Json(serde_json::json!({"success":true,"data":{"id":99,"hash":"fixture-hash","files":[{"id":400,"name":"Series Master Edition v04.cbz","absolute_path":"Series Master Edition/Series Master Edition v04.cbz","size":400}]}})) }))).await.unwrap();
        });
        let dir = std::env::temp_dir().join(format!(
            "source-torbox-read-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("test.db")).unwrap();
        db.call(|conn| {
            conn.execute("INSERT INTO manga (id,source_id,url,title) VALUES (1,0,'/known','Series')",[])?;
            conn.execute("INSERT INTO chapters (id,manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (1,1,'/v4','Volume 4',4,0,0)",[])?;
            conn.execute("INSERT INTO downloads (chapter_id,state,position,attempts,added_at,torrent_hash,backend_type,external_job_id) VALUES (1,'failed',0,0,0,'fixture-hash','torbox','99')",[])?; Ok(())
        }).await.unwrap();
        let service = AcquisitionService::new(db.clone(), reqwest::Client::new(), None, None);
        service
            .save_config(super::super::service::AcquisitionConfigPatch {
                nyaa_enabled: None,
                nyaa_url: None,
                nyaa_category: None,
                torbox_enabled: Some(true),
                torbox_url: Some(format!("http://{addr}")),
                torbox_token: Some("fixture-token".into()),
            })
            .await
            .unwrap();
        let mut c = candidate(&source_units::tests::files("Master Edition"));
        c.provider_metadata["torrentHash"] = serde_json::json!("fixture-hash");
        let files = service.discover_source_files(&c, None).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, 400);
        assert!(source_units::discover(&files, &c.title, Some("master"))[0].eligible);
        server.abort();
        drop(service);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn komga_edition_ownership_is_separate_from_cached_standard_ownership() {
        use axum::{routing::get, Json, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/api/v1/series",get(|axum::extract::Query(params): axum::extract::Query<HashMap<String,String>>| async move {
            let master = params.get("search").is_some_and(|s| s.contains("master"));
            Json(serde_json::json!({"content":[{"id":if master {"master"} else {"standard"},"name":if master {"Series (master Edition)"} else {"Series"},"libraryId":"test","booksCount":1}]}))
        })).route("/api/v1/series/{id}/books",get(|axum::extract::Path(id):axum::extract::Path<String>| async move {
            Json(serde_json::json!({"content":[{"id":"book","seriesId":id,"name":if id == "master" {"Series Master Edition v04"} else {"Series v01"},"number":if id == "master" {4.0} else {1.0},"metadata":{"numberSort":if id == "master" {4.0} else {1.0}}}]}))
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = std::env::temp_dir().join(format!(
            "source-owned-komga-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("test.db")).unwrap();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO manga (id,source_id,url,title) VALUES (1,0,'/komga','Series')",
                [],
            )?;
            let mut settings = serde_json::Map::new();
            settings.insert("pipeline.komga_enabled".into(), serde_json::json!(true));
            settings.insert(
                "pipeline.komga_url".into(),
                serde_json::json!(format!("http://{addr}")),
            );
            settings.insert(
                "pipeline.komga_library_id".into(),
                serde_json::json!("test"),
            );
            crate::settings::put(conn, &settings)?;
            crate::db::set_state(conn, "pipeline.komga_api_key", "fixture")?;
            Ok(())
        })
        .await
        .unwrap();
        let pipeline = std::sync::Arc::new(crate::pipeline::Pipeline::new(db.clone()));
        let standard = pipeline.ownership.get_title_ownership(1, "Series").await;
        assert!(standard
            .volumes
            .iter()
            .any(|v| v.volume == 1.0
                && v.status == crate::pipeline::ownership::OwnershipStatus::Owned));
        let service = AcquisitionService::new(
            db.clone(),
            reqwest::Client::new(),
            Some(pipeline.clone()),
            None,
        );
        assert_eq!(
            service
                .source_edition_ownership(1, "Series", "master")
                .await
                .unwrap(),
            (true, vec![4.])
        );
        let standard = pipeline.ownership.get_title_ownership(1, "Series").await;
        assert_eq!(standard.series_name.as_deref(), Some("Series"));
        pipeline.ownership.invalidate(1);
        server.abort();
        drop(service);
        drop(pipeline);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
