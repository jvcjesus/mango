//! Keiyoushi Extensions Release Provider Adapter (Milestone M10).
//!
//! Exposes installed Keiyoushi/Mihon extension sources to the unified release
//! discovery engine so extensions compete fairly alongside Madokami and Nyaa.
//!
//! Invariants:
//! - Pure discovery: does NOT trigger background downloads or web scraping.
//! - Conservative matching: chapter coverage is clearly distinguished from official true volumes.
//! - Read-only isolation: failures in extension host or sources do not block discovery.

use super::matching::{evaluate_candidate_confidence, match_volume_number};
use super::providers::{
    BoxFuture, ProviderHealth, ProviderStatus, ReleaseCandidate,
    ReleaseProvider, ReleaseQuery,
};
use crate::db::{now, Db};
use crate::error::Result;
use serde::{Deserialize, Serialize};

pub struct KeiyoushiReleaseProvider {
    db: Db,
    enabled: bool,
}

impl KeiyoushiReleaseProvider {
    pub fn new(db: Db, enabled: bool) -> Self {
        Self { db, enabled }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChapterRow {
    id: i64,
    manga_id: i64,
    source_name: String,
    manga_title: String,
    chapter_name: String,
    chapter_number: Option<f64>,
    volume_number: Option<f64>,
    scanlator: Option<String>,
    date: i64,
}

impl ReleaseProvider for KeiyoushiReleaseProvider {
    fn id(&self) -> &'static str {
        "keiyoushi"
    }

    fn name(&self) -> &'static str {
        "Keiyoushi Extensions"
    }

    fn check_health<'a>(&'a self) -> BoxFuture<'a, ProviderHealth> {
        Box::pin(async move {
            if !self.enabled {
                return ProviderHealth {
                    provider_id: "keiyoushi".to_string(),
                    name: "Keiyoushi Extensions".to_string(),
                    status: ProviderStatus::Disabled,
                    message: Some("Disabled in settings".to_string()),
                    last_checked_at: Some(now()),
                };
            }

            // Check if any extension sources are available in the database
            let has_sources = self
                .db
                .call(|conn| {
                    let count: i64 = conn
                        .query_row(
                            "SELECT count(*) FROM sources WHERE id > 0",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    Ok(count > 0)
                })
                .await
                .unwrap_or(false);

            ProviderHealth {
                provider_id: "keiyoushi".to_string(),
                name: "Keiyoushi Extensions".to_string(),
                status: if has_sources {
                    ProviderStatus::Available
                } else {
                    ProviderStatus::Available // Available even with 0 sources installed
                },
                message: if has_sources {
                    None
                } else {
                    Some("No third-party extension sources currently configured".to_string())
                },
                last_checked_at: Some(now()),
            }
        })
    }

    fn search_releases<'a>(
        &'a self,
        query: &'a ReleaseQuery,
    ) -> BoxFuture<'a, Result<Vec<ReleaseCandidate>>> {
        Box::pin(async move {
            if !self.enabled {
                return Ok(Vec::new());
            }

            let series_title_query = query.series_title.clone();
            let target_vol = query.target_volume;

            let chapters: Vec<ChapterRow> = self
                .db
                .call(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT c.id, c.manga_id, s.name, m.title, c.name, c.chapter_number, c.scanlator, c.date_upload
                         FROM chapters c
                         JOIN manga m ON c.manga_id = m.id
                         JOIN sources s ON m.source_id = s.id
                         WHERE lower(m.title) LIKE '%' || lower(?1) || '%'
                         ORDER BY c.chapter_number ASC LIMIT 200",
                    )?;

                    let rows = stmt.query_map([&series_title_query], |r| {
                        let ch_name: String = r.get(4)?;
                        let ch_num: f64 = r.get(5)?;
                        let vol = crate::release::parse_volume(&ch_name)
                            .or_else(|| crate::release::parse_release(&ch_name, ch_num).volume);
                        Ok(ChapterRow {
                            id: r.get(0)?,
                            manga_id: r.get(1)?,
                            source_name: r.get(2)?,
                            manga_title: r.get(3)?,
                            chapter_name: ch_name,
                            chapter_number: Some(ch_num),
                            volume_number: vol,
                            scanlator: r.get(6)?,
                            date: r.get(7)?,
                        })
                    })?;

                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .await?;

            let mut candidates = Vec::new();

            // Group chapters by volume if volume numbers are present
            let mut vol_chapters: std::collections::HashMap<i64, Vec<ChapterRow>> =
                std::collections::HashMap::new();
            for ch in chapters {
                if let Some(vol) = ch.volume_number {
                    vol_chapters.entry(vol.round() as i64).or_default().push(ch);
                }
            }

            // If targeting a specific volume, check if extension has chapters for this volume
            if let Some(target) = target_vol {
                let target_int = target.round() as i64;
                if let Some(chs) = vol_chapters.get(&target_int) {
                    let chapter_count = chs.len();
                    let first_ch = chs.first().map(|c| c.chapter_number.unwrap_or(0.0)).unwrap_or(0.0);
                    let last_ch = chs.last().map(|c| c.chapter_number.unwrap_or(0.0)).unwrap_or(0.0);
                    let source_name = chs.first().map(|c| c.source_name.as_str()).unwrap_or("Extension");
                    let scanlator = chs.first().and_then(|c| c.scanlator.clone());

                    let title = format!(
                        "{} Volume {:02} (Ch. {}-{}) [{}]",
                        query.series_title, target_int, first_ch, last_ch, source_name
                    );

                    let (match_type, _, _, _) = match_volume_number(&title, Some(target));
                    let (_confidence, _confidence_reason) = evaluate_candidate_confidence(
                        &title,
                        &query.series_title,
                        &[],
                        Some(target),
                        match_type,
                        false, // Scans from extension
                        false,
                        "keiyoushi",
                        None,
                        "en",
                    );

                    // Extension chapter coverage receives a baseline score (e.g. 75.0)
                    let score = 75.0;
                    let score_explanation = format!(
                        "Chapter coverage from extension source '{source_name}': {chapter_count} chapters (Ch. {first_ch}-{last_ch})"
                    );

                    candidates.push(ReleaseCandidate {
                        candidate_id: format!("keiyoushi:{}:vol-{}", source_name.to_lowercase(), target_int),
                        provider: "keiyoushi".to_string(),
                        provider_item_id: format!("vol-{}", target_int),
                        title: title.clone(),
                        raw_title: title,
                        match_type,
                        target_volume: Some(target),
                        target_chapter: None,
                        size_bytes: None,
                        uploader: scanlator,
                        is_trusted: false,
                        is_digital: false,
                        score,
                        score_explanation,
                        download_url: None,
                        magnet_url: None,
                        info_hash: None,
                        seeders: None,
                        leechers: None,
                        published_at: chs.last().map(|c| c.date),
                        ..Default::default()
                    });
                }
            }

            Ok(candidates)
        })
    }
}
