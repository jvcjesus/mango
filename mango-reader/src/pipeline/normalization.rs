//! Safe, native, dry-run-first Komga manga library normalization engine.
//!
//! Evaluates existing pre-Mango manga CBZs in Komga, matches them to MangaBaka metadata
//! via canonical identity resolution rules, verifies archive integrity, and performs
//! atomic, staged replacement of ComicInfo.xml with lossless rollback capability.

use crate::{
    db::{Db, now},
    error::{AppError, Result},
    pipeline::{
        komga::{KomgaClient, KomgaLibrary, KomgaSeries},
        mangabaka::{
            self, ComicInfoParams, MangaBakaClient, MangaBakaEnrichment, MangaBakaOutcome,
        },
        metadata,
        publisher::{self, verify_cbz_integrity},
    },
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

/// Classification status for books and series in library analysis and normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationStatus {
    Matched,
    AlreadyNormalized,
    NeedsUpdate,
    Ambiguous,
    Unmatched,
    SkippedProviderNotApplicable,
    InvalidArchive,
    Error,
}

impl std::fmt::Display for NormalizationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Matched => write!(f, "MATCHED"),
            Self::AlreadyNormalized => write!(f, "ALREADY_NORMALIZED"),
            Self::NeedsUpdate => write!(f, "NEEDS_UPDATE"),
            Self::Ambiguous => write!(f, "AMBIGUOUS"),
            Self::Unmatched => write!(f, "UNMATCHED"),
            Self::SkippedProviderNotApplicable => write!(f, "SKIPPED_PROVIDER_NOT_APPLICABLE"),
            Self::InvalidArchive => write!(f, "INVALID_ARCHIVE"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

/// A specific metadata field difference between current CBZ ComicInfo and proposed MangaBaka metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMetadataDiff {
    pub field: String,
    pub current: Option<String>,
    pub proposed: Option<String>,
}

/// Normalization item for a single CBZ book.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookNormalizationItem {
    pub komga_book_id: String,
    pub komga_series_id: String,
    pub series_title: String,
    pub book_name: String,
    pub canonical_path: String,
    pub volume: Option<f64>,
    pub status: NormalizationStatus,
    pub reason: String,
    pub mangabaka_id: Option<i64>,
    pub mangabaka_release_id: Option<String>,
    pub anilist_id: Option<i64>,
    pub diffs: Vec<BookMetadataDiff>,
    pub current_sha256: Option<String>,
}

/// Normalization item for a series in a Komga library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesNormalizationItem {
    pub komga_series_id: String,
    pub library_id: String,
    pub series_title: String,
    pub status: NormalizationStatus,
    pub reason: String,
    pub mangabaka_id: Option<i64>,
    pub anilist_id: Option<i64>,
    pub total_books: usize,
    pub needs_update_count: usize,
    pub already_normalized_count: usize,
    pub books: Vec<BookNormalizationItem>,
}

/// Full dry-run analysis summary for a Komga library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAnalysisSummary {
    pub library_id: String,
    pub library_name: String,
    pub library_root: String,
    pub total_series: usize,
    pub total_books: usize,
    pub matched_count: usize,
    pub already_normalized_count: usize,
    pub needs_update_count: usize,
    pub ambiguous_count: usize,
    pub unmatched_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
    pub series: Vec<SeriesNormalizationItem>,
}

/// Persistent record in SQLite normalization_history table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationHistoryRecord {
    pub id: String,
    pub batch_id: String,
    pub canonical_path: String,
    pub series_title: String,
    pub volume: f64,
    pub komga_series_id: Option<String>,
    pub komga_book_id: Option<String>,
    pub mangabaka_id: Option<i64>,
    pub mangabaka_release_id: Option<String>,
    pub anilist_id: Option<i64>,
    pub original_sha256: Option<String>,
    pub normalized_sha256: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub normalized_at: i64,
    pub reverted_at: Option<i64>,
}

/// Result of an approved normalization batch execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationExecutionResult {
    pub batch_id: String,
    pub total_processed: usize,
    pub normalized_count: usize,
    pub already_normalized_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub items: Vec<NormalizationHistoryRecord>,
}

/// Calculates SHA256 checksum for a file on disk.
pub fn calculate_file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Extracts raw `ComicInfo.xml` text from a CBZ archive if present.
pub fn read_comic_info_xml_from_cbz(path: &Path) -> Result<Option<String>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        AppError::bad_request(format!("Cannot open archive {}: {e}", path.display()))
    })?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::bad_request(e.to_string()))?;
        if entry.is_file() && entry.name().eq_ignore_ascii_case("ComicInfo.xml") {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|e| AppError::bad_request(format!("Cannot read ComicInfo.xml: {e}")))?;
            return Ok(Some(text));
        }
    }

    Ok(None)
}

/// Service orchestrating library analysis, dry runs, approved normalization, and rollback.
#[derive(Clone)]
pub struct LibraryNormalizationService {
    db: Db,
    #[allow(dead_code)]
    http: reqwest::Client,
    file_locks: Arc<Mutex<HashSet<PathBuf>>>,
}

impl LibraryNormalizationService {
    pub fn new(db: Db, http: reqwest::Client) -> Self {
        Self {
            db,
            http,
            file_locks: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Determines whether a Komga library is eligible for MangaBaka normalization.
    ///
    /// Rules:
    /// - English Manga: Eligible
    /// - Chinese Manga: Eligible
    /// - Comics (e.g. ID `0RNH705VV374H` or root containing `/comics`): Explicitly SKIPPED as provider not applicable.
    pub fn is_library_eligible_for_mangabaka(&self, library: &KomgaLibrary) -> bool {
        let name_lower = library.name.to_lowercase();
        let root_lower = library.root.to_lowercase();

        if library.id == "0RNH705VV374H"
            || name_lower.contains("comic")
            || root_lower.contains("comic")
        {
            return false;
        }

        true
    }

    /// Resolves stable identity for a series using strict precedence:
    /// 1. tracker_mappings table for mangabaka
    /// 2. tracker_mappings table for anilist -> MangaBaka xref
    /// 3. Existing ComicInfo.xml <Web> or <Notes> tag in archive
    /// 4. acquisition_history table provenance
    /// 5. Controlled fallback: MangaBaka exact title match (ambiguous fails closed).
    pub async fn resolve_series_identity(
        &self,
        mb_client: &MangaBakaClient,
        komga_series_id: &str,
        series_title: &str,
        sample_cbz_path: Option<&Path>,
    ) -> (Option<i64>, Option<i64>, String) {
        // 1. Check tracker_mappings for MangaBaka
        let k_id = komga_series_id.to_string();
        let mb_mapped: Option<String> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT tracker_id FROM tracker_mappings
                         WHERE komga_series_id = ?1 AND tracker = 'mangabaka' AND confidence != 'ambiguous'",
                        [&k_id],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            .unwrap_or(None);

        if let Some(id_str) = mb_mapped {
            if let Ok(id) = id_str.parse::<i64>() {
                return (
                    Some(id),
                    None,
                    "Resolved from tracker mappings (MangaBaka)".to_string(),
                );
            }
        }

        // 2. Check tracker_mappings for AniList
        let k_id = komga_series_id.to_string();
        let al_mapped: Option<String> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT tracker_id FROM tracker_mappings
                         WHERE komga_series_id = ?1 AND tracker = 'anilist' AND confidence != 'ambiguous'",
                        [&k_id],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            .unwrap_or(None);

        if let Some(id_str) = al_mapped {
            if let Ok(al_id) = id_str.parse::<i64>() {
                if let Ok(Some(xref)) = mb_client.resolve_from_anilist(al_id).await {
                    return (
                        Some(xref.mangabaka_id),
                        Some(al_id),
                        format!("Resolved from AniList ID {al_id} -> MangaBaka xref"),
                    );
                }
            }
        }

        // 3. Check sample CBZ ComicInfo.xml for MangaBaka Web tag or Notes
        if let Some(path) = sample_cbz_path {
            if let Ok(Some(xml)) = read_comic_info_xml_from_cbz(path) {
                for line in xml.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("<Web>") && trimmed.contains("mangabaka.org/series/") {
                        if let Some(num_str) = trimmed
                            .strip_prefix("<Web>")
                            .and_then(|s| s.strip_suffix("</Web>"))
                            .and_then(|s| s.rsplit_once('/'))
                            .map(|(_, id)| id.trim())
                        {
                            if let Ok(id) = num_str.parse::<i64>() {
                                return (
                                    Some(id),
                                    None,
                                    format!("Resolved from CBZ ComicInfo Web tag ({id})"),
                                );
                            }
                        }
                    }
                }
            }
        }

        // 4. Check acquisition_history provenance
        let s_title = series_title.to_string();
        let norm_title = mangabaka::normalize_title(&s_title);
        let hist_mb_id: Option<i64> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT mangabaka_id FROM acquisition_history
                         WHERE (series = ?1 OR LOWER(series) = ?2) AND mangabaka_id IS NOT NULL AND mangabaka_id > 0
                         ORDER BY id DESC LIMIT 1",
                        params![s_title, norm_title],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            .unwrap_or(None);

        if let Some(id) = hist_mb_id {
            return (
                Some(id),
                None,
                format!("Resolved from acquisition history provenance ({id})"),
            );
        }

        // 5. Controlled fallback: title search via MangaBaka API
        match mb_client.match_series(series_title).await {
            Ok(mangabaka::MangaBakaSeriesMatch::Found { id, .. }) => (
                Some(id),
                None,
                "Resolved from MangaBaka title match".to_string(),
            ),
            Ok(mangabaka::MangaBakaSeriesMatch::Ambiguous(candidates)) => (
                None,
                None,
                format!(
                    "Ambiguous match ({} candidates on MangaBaka)",
                    candidates.len()
                ),
            ),
            Ok(mangabaka::MangaBakaSeriesMatch::NotFound) => (
                None,
                None,
                format!("Series '{series_title}' not found on MangaBaka"),
            ),
            Err(e) => (None, None, format!("MangaBaka API error: {e}")),
            _ => (None, None, "Unresolved".to_string()),
        }
    }

    /// Evaluates existing ComicInfo.xml metadata against proposed normalized MangaBaka metadata.
    ///
    /// Determines whether the file already has identical metadata (`AlreadyNormalized`) or
    /// requires updating (`NeedsUpdate`) with a detailed field-level diff.
    pub fn evaluate_metadata_diff(
        existing_xml: Option<&str>,
        enrichment: &MangaBakaEnrichment,
        volume: f64,
        actual_page_count: usize,
    ) -> (NormalizationStatus, String, Vec<BookMetadataDiff>) {
        let mut diffs = Vec::new();

        let Some(xml) = existing_xml else {
            diffs.push(BookMetadataDiff {
                field: "ComicInfo.xml".to_string(),
                current: None,
                proposed: Some("Generate complete MangaBaka ComicInfo.xml".to_string()),
            });
            return (
                NormalizationStatus::NeedsUpdate,
                "Missing ComicInfo.xml".to_string(),
                diffs,
            );
        };

        // Helper to extract XML tag content
        let get_tag = |tag: &str| -> Option<String> {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            for line in xml.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix(&open) {
                    if let Some(val) = rest.strip_suffix(&close) {
                        return Some(val.to_string());
                    }
                }
            }
            None
        };

        // Check Series
        let cur_series = get_tag("Series");
        let prop_series = enrichment.series_title.clone();
        if cur_series.as_deref() != prop_series.as_deref() {
            diffs.push(BookMetadataDiff {
                field: "Series".to_string(),
                current: cur_series,
                proposed: prop_series,
            });
        }

        // Check Volume
        let cur_volume = get_tag("Volume");
        let prop_volume = if volume.fract() == 0.0 {
            (volume as i64).to_string()
        } else {
            volume.to_string()
        };
        if cur_volume.as_deref() != Some(&prop_volume) {
            diffs.push(BookMetadataDiff {
                field: "Volume".to_string(),
                current: cur_volume,
                proposed: Some(prop_volume),
            });
        }

        // Volume Number Semantics: Number tag MUST be absent
        let cur_number = get_tag("Number");
        if cur_number.is_some() {
            diffs.push(BookMetadataDiff {
                field: "Number".to_string(),
                current: cur_number,
                proposed: None, // Must be omitted for volume acquisitions
            });
        }

        // Check Web link
        let cur_web = get_tag("Web");
        let prop_web = format!("https://mangabaka.org/series/{}", enrichment.series_id);
        if cur_web.as_deref() != Some(&prop_web) {
            diffs.push(BookMetadataDiff {
                field: "Web".to_string(),
                current: cur_web,
                proposed: Some(prop_web),
            });
        }

        // Check Publisher
        let cur_pub = get_tag("Publisher");
        let prop_pub = enrichment.publisher.clone();
        if cur_pub.as_deref() != prop_pub.as_deref() && prop_pub.is_some() {
            diffs.push(BookMetadataDiff {
                field: "Publisher".to_string(),
                current: cur_pub,
                proposed: prop_pub,
            });
        }

        // Check Genres
        let cur_genre = get_tag("Genre");
        let norm_genres = metadata::normalize_genres(&enrichment.genres);
        let prop_genre = (!norm_genres.is_empty()).then(|| norm_genres.join(", "));
        if cur_genre.as_deref() != prop_genre.as_deref() && prop_genre.is_some() {
            diffs.push(BookMetadataDiff {
                field: "Genre".to_string(),
                current: cur_genre,
                proposed: prop_genre,
            });
        }

        // Check Writers
        let cur_writer = get_tag("Writer");
        let prop_writer = (!enrichment.writers.is_empty()).then(|| enrichment.writers.join(", "));
        if cur_writer.as_deref() != prop_writer.as_deref() && prop_writer.is_some() {
            diffs.push(BookMetadataDiff {
                field: "Writer".to_string(),
                current: cur_writer,
                proposed: prop_writer,
            });
        }

        // Check Manga Reading Direction
        let cur_manga = get_tag("Manga");
        if cur_manga.as_deref() != Some("YesAndRightToLeft") {
            diffs.push(BookMetadataDiff {
                field: "Manga".to_string(),
                current: cur_manga,
                proposed: Some("YesAndRightToLeft".to_string()),
            });
        }

        // Check PageCount
        let cur_pages = get_tag("PageCount");
        let prop_pages = actual_page_count.to_string();
        if cur_pages.as_deref() != Some(&prop_pages) && actual_page_count > 0 {
            diffs.push(BookMetadataDiff {
                field: "PageCount".to_string(),
                current: cur_pages,
                proposed: Some(prop_pages),
            });
        }

        if diffs.is_empty() {
            (
                NormalizationStatus::AlreadyNormalized,
                "Already contains complete normalized MangaBaka metadata".to_string(),
                diffs,
            )
        } else {
            (
                NormalizationStatus::NeedsUpdate,
                format!(
                    "{} metadata fields differ from canonical MangaBaka representation",
                    diffs.len()
                ),
                diffs,
            )
        }
    }

    /// Executes a dry-run analysis on an entire Komga library or a specific series.
    pub async fn analyze_library(
        &self,
        komga_client: &KomgaClient,
        mb_client: &MangaBakaClient,
        library_id: &str,
        series_id_filter: Option<&str>,
        library_dir_override: Option<&Path>,
    ) -> Result<LibraryAnalysisSummary> {
        let libraries = komga_client.list_libraries().await?;
        let lib = libraries
            .into_iter()
            .find(|l| l.id == library_id)
            .ok_or_else(|| AppError::not_found(format!("Komga library {library_id} not found")))?;

        let is_manga = self.is_library_eligible_for_mangabaka(&lib);

        let all_series = komga_client.list_series(Some(library_id)).await?;
        let target_series: Vec<KomgaSeries> = if let Some(s_id) = series_id_filter {
            all_series.into_iter().filter(|s| s.id == s_id).collect()
        } else {
            all_series
        };

        let mut summary = LibraryAnalysisSummary {
            library_id: lib.id.clone(),
            library_name: lib.name.clone(),
            library_root: lib.root.clone(),
            total_series: target_series.len(),
            total_books: 0,
            matched_count: 0,
            already_normalized_count: 0,
            needs_update_count: 0,
            ambiguous_count: 0,
            unmatched_count: 0,
            skipped_count: 0,
            error_count: 0,
            series: Vec::new(),
        };

        if !is_manga {
            for s in target_series {
                let books = komga_client
                    .get_books_in_series(&s.id)
                    .await
                    .unwrap_or_default();
                summary.total_books += books.len();
                summary.skipped_count += books.len();

                summary.series.push(SeriesNormalizationItem {
                    komga_series_id: s.id.clone(),
                    library_id: lib.id.clone(),
                    series_title: s.name.clone(),
                    status: NormalizationStatus::SkippedProviderNotApplicable,
                    reason: "Skipped — provider not applicable (Comics library)".to_string(),
                    mangabaka_id: None,
                    anilist_id: None,
                    total_books: books.len(),
                    needs_update_count: 0,
                    already_normalized_count: 0,
                    books: books
                        .into_iter()
                        .map(|b| BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name.clone(),
                            canonical_path: String::new(),
                            volume: Some(b.number),
                            status: NormalizationStatus::SkippedProviderNotApplicable,
                            reason: "Skipped — provider not applicable (Comics library)"
                                .to_string(),
                            mangabaka_id: None,
                            mangabaka_release_id: None,
                            anilist_id: None,
                            diffs: Vec::new(),
                            current_sha256: None,
                        })
                        .collect(),
                });
            }
            return Ok(summary);
        }

        // Analyze eligible manga series
        let base_dir = library_dir_override
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&lib.root));

        for s in target_series {
            let books = komga_client
                .get_books_in_series(&s.id)
                .await
                .unwrap_or_default();
            summary.total_books += books.len();

            let sample_path = books
                .first()
                .map(|b| base_dir.join(&s.name).join(format!("{}.cbz", b.name)));

            // Identity resolution
            let (mangabaka_id, anilist_id, id_reason) = self
                .resolve_series_identity(&mb_client, &s.id, &s.name, sample_path.as_deref())
                .await;

            let mut series_item = SeriesNormalizationItem {
                komga_series_id: s.id.clone(),
                library_id: lib.id.clone(),
                series_title: s.name.clone(),
                status: NormalizationStatus::Matched,
                reason: id_reason.clone(),
                mangabaka_id,
                anilist_id,
                total_books: books.len(),
                needs_update_count: 0,
                already_normalized_count: 0,
                books: Vec::new(),
            };

            let Some(mb_id) = mangabaka_id else {
                let status = if id_reason.contains("Ambiguous") {
                    NormalizationStatus::Ambiguous
                } else {
                    NormalizationStatus::Unmatched
                };
                series_item.status = status;
                for b in books {
                    if status == NormalizationStatus::Ambiguous {
                        summary.ambiguous_count += 1;
                    } else {
                        summary.unmatched_count += 1;
                    }
                    series_item.books.push(BookNormalizationItem {
                        komga_book_id: b.id,
                        komga_series_id: s.id.clone(),
                        series_title: s.name.clone(),
                        book_name: b.name.clone(),
                        canonical_path: String::new(),
                        volume: Some(b.number),
                        status,
                        reason: id_reason.clone(),
                        mangabaka_id: None,
                        mangabaka_release_id: None,
                        anilist_id,
                        diffs: Vec::new(),
                        current_sha256: None,
                    });
                }
                summary.series.push(series_item);
                continue;
            };

            // Inspect each book
            for b in books {
                let canonical_path = base_dir.join(&s.name).join(format!("{}.cbz", b.name));
                let path_str = canonical_path.to_string_lossy().to_string();

                if !canonical_path.is_file() {
                    summary.error_count += 1;
                    series_item.books.push(BookNormalizationItem {
                        komga_book_id: b.id,
                        komga_series_id: s.id.clone(),
                        series_title: s.name.clone(),
                        book_name: b.name,
                        canonical_path: path_str,
                        volume: Some(b.number),
                        status: NormalizationStatus::Error,
                        reason: "File not found on local canonical filesystem".to_string(),
                        mangabaka_id: Some(mb_id),
                        mangabaka_release_id: None,
                        anilist_id,
                        diffs: Vec::new(),
                        current_sha256: None,
                    });
                    continue;
                }

                if let Err(e) = verify_cbz_integrity(&canonical_path) {
                    summary.error_count += 1;
                    series_item.books.push(BookNormalizationItem {
                        komga_book_id: b.id,
                        komga_series_id: s.id.clone(),
                        series_title: s.name.clone(),
                        book_name: b.name,
                        canonical_path: path_str,
                        volume: Some(b.number),
                        status: NormalizationStatus::InvalidArchive,
                        reason: format!("Corrupted archive: {e}"),
                        mangabaka_id: Some(mb_id),
                        mangabaka_release_id: None,
                        anilist_id,
                        diffs: Vec::new(),
                        current_sha256: None,
                    });
                    continue;
                }

                let vol = crate::release::parse_volume(&b.name).unwrap_or(b.number);
                let current_sha = calculate_file_sha256(&canonical_path).ok();
                let existing_xml = read_comic_info_xml_from_cbz(&canonical_path).unwrap_or(None);
                let page_count = publisher::verify_cbz_integrity(&canonical_path)
                    .map(|_| {
                        File::open(&canonical_path)
                            .ok()
                            .and_then(|f| zip::ZipArchive::new(f).ok())
                            .map_or(0, |a| a.len().saturating_sub(1))
                    })
                    .unwrap_or(0);

                // Resolve MangaBaka metadata for this specific volume
                match mb_client
                    .resolve_volume_metadata(&s.name, anilist_id, Some(mb_id), vol)
                    .await
                {
                    Ok(MangaBakaOutcome::Success(enrichment)) => {
                        let (b_status, b_reason, diffs) = Self::evaluate_metadata_diff(
                            existing_xml.as_deref(),
                            &enrichment,
                            vol,
                            page_count,
                        );

                        if b_status == NormalizationStatus::AlreadyNormalized {
                            summary.already_normalized_count += 1;
                            series_item.already_normalized_count += 1;
                        } else {
                            summary.needs_update_count += 1;
                            series_item.needs_update_count += 1;
                        }
                        summary.matched_count += 1;

                        series_item.books.push(BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name,
                            canonical_path: path_str,
                            volume: Some(vol),
                            status: b_status,
                            reason: b_reason,
                            mangabaka_id: Some(mb_id),
                            mangabaka_release_id: enrichment.work_id,
                            anilist_id,
                            diffs,
                            current_sha256: current_sha,
                        });
                    }
                    Ok(other) => {
                        summary.unmatched_count += 1;
                        series_item.books.push(BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name,
                            canonical_path: path_str,
                            volume: Some(vol),
                            status: NormalizationStatus::Unmatched,
                            reason: other.description().to_string(),
                            mangabaka_id: Some(mb_id),
                            mangabaka_release_id: None,
                            anilist_id,
                            diffs: Vec::new(),
                            current_sha256: current_sha,
                        });
                    }
                    Err(e) => {
                        summary.error_count += 1;
                        series_item.books.push(BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name,
                            canonical_path: path_str,
                            volume: Some(vol),
                            status: NormalizationStatus::Error,
                            reason: format!("Failed to query MangaBaka: {e}"),
                            mangabaka_id: Some(mb_id),
                            mangabaka_release_id: None,
                            anilist_id,
                            diffs: Vec::new(),
                            current_sha256: current_sha,
                        });
                    }
                }
            }

            summary.series.push(series_item);
        }

        Ok(summary)
    }

    /// Executes approved normalization on a candidate set of books.
    ///
    /// Properties:
    /// - Operates on verified copies staged in temporary files before canonical replacement.
    /// - Idempotent: identical hash produces ALREADY_NORMALIZED with zero file alteration.
    /// - Thread-safe: per-file lock prevents concurrent operations on the same CBZ.
    /// - Lossless: original ComicInfo.xml is persisted in normalization_history for rollback.
    /// - Targeted: triggers Komga metadata refresh for modified books.
    pub async fn normalize_book(
        &self,
        batch_id: &str,
        canonical_path: &Path,
        series_title: &str,
        volume: f64,
        mangabaka_id: i64,
        anilist_id: Option<i64>,
        komga_series_id: Option<&str>,
        komga_book_id: Option<&str>,
        mb_client: &MangaBakaClient,
        komga_client: Option<&KomgaClient>,
    ) -> Result<NormalizationHistoryRecord> {
        let path_buf = canonical_path.to_path_buf();

        // 1. Acquire per-file concurrency lock
        {
            let mut locks = self.file_locks.lock().await;
            if locks.contains(&path_buf) {
                return Err(AppError::new(
                    409,
                    "file_locked",
                    format!(
                        "File is currently being modified by another operation: {}",
                        path_buf.display()
                    ),
                ));
            }
            locks.insert(path_buf.clone());
        }

        struct LockGuard<'a>(&'a Mutex<HashSet<PathBuf>>, PathBuf);
        impl<'a> Drop for LockGuard<'a> {
            fn drop(&mut self) {
                if let Ok(mut locks) = self.0.try_lock() {
                    locks.remove(&self.1);
                }
            }
        }
        let _lock_guard = LockGuard(&self.file_locks, path_buf.clone());

        // 2. Validate original archive
        verify_cbz_integrity(canonical_path)?;
        let original_sha = calculate_file_sha256(canonical_path)?;
        let original_xml = read_comic_info_xml_from_cbz(canonical_path)?;

        // 3. Resolve MangaBaka metadata
        let outcome = mb_client
            .resolve_volume_metadata(series_title, anilist_id, Some(mangabaka_id), volume)
            .await?;

        let enrichment = match outcome {
            MangaBakaOutcome::Success(e) => e,
            other => {
                return Err(AppError::bad_request(format!(
                    "Cannot normalize CBZ: MangaBaka metadata resolution failed: {}",
                    other.description()
                )));
            }
        };

        // 4. Stage isolated temporary candidate
        let parent = canonical_path.parent().unwrap_or_else(|| Path::new("."));
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let temp_filename = format!(
            ".{}.norm-{pid}-{nanos}.tmp",
            canonical_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        let temp_path = parent.join(temp_filename);

        struct CleanupGuard<'a>(&'a Path);
        impl<'a> Drop for CleanupGuard<'a> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }
        let temp_guard = CleanupGuard(&temp_path);

        fs::copy(canonical_path, &temp_path).map_err(|e| {
            AppError::internal(format!("Failed to stage temporary candidate copy: {e}"))
        })?;

        // 5. Write normalized ComicInfo
        let params = ComicInfoParams {
            series: series_title,
            volume: Some(volume),
            number: None,
            is_volume: true,
            title: enrichment.work_title.as_deref(),
            scanlator: None,
            notes: None,
        };

        let _ = mangabaka::write_comic_info_to_cbz(&temp_path, &params, Some(&enrichment))?;
        verify_cbz_integrity(&temp_path)?;

        let new_sha = calculate_file_sha256(&temp_path)?;

        // 6. Idempotency check: if file hash is unchanged, no replacement needed!
        let (status, final_sha) = if new_sha == original_sha {
            ("already_normalized".to_string(), original_sha.clone())
        } else {
            // Atomic replacement
            fs::rename(&temp_path, canonical_path).map_err(|e| {
                AppError::internal(format!("Failed to atomically replace canonical CBZ: {e}"))
            })?;
            std::mem::forget(temp_guard); // Disarm temporary cleanup guard on success
            ("normalized".to_string(), new_sha)
        };

        // 7. Persist provenance and rollback record
        let record_id = format!("norm-{pid}-{nanos}");
        let now_ts = now();

        let rec = NormalizationHistoryRecord {
            id: record_id.clone(),
            batch_id: batch_id.to_string(),
            canonical_path: canonical_path.to_string_lossy().to_string(),
            series_title: series_title.to_string(),
            volume,
            komga_series_id: komga_series_id.map(str::to_string),
            komga_book_id: komga_book_id.map(str::to_string),
            mangabaka_id: Some(mangabaka_id),
            mangabaka_release_id: enrichment.work_id,
            anilist_id,
            original_sha256: Some(original_sha),
            normalized_sha256: Some(final_sha),
            status,
            error_message: None,
            normalized_at: now_ts,
            reverted_at: None,
        };

        let rec_clone = rec.clone();
        let orig_xml_clone = original_xml.clone();
        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO normalization_history (
                        id, batch_id, canonical_path, series_title, volume,
                        komga_series_id, komga_book_id, mangabaka_id, mangabaka_release_id,
                        anilist_id, original_sha256, normalized_sha256, original_comic_info_xml,
                        status, error_message, normalized_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                    params![
                        rec_clone.id,
                        rec_clone.batch_id,
                        rec_clone.canonical_path,
                        rec_clone.series_title,
                        rec_clone.volume,
                        rec_clone.komga_series_id,
                        rec_clone.komga_book_id,
                        rec_clone.mangabaka_id,
                        rec_clone.mangabaka_release_id,
                        rec_clone.anilist_id,
                        rec_clone.original_sha256,
                        rec_clone.normalized_sha256,
                        orig_xml_clone,
                        rec_clone.status,
                        rec_clone.error_message,
                        rec_clone.normalized_at,
                    ],
                )?;
                Ok(())
            })
            .await?;

        // 8. Trigger targeted Komga metadata refresh
        if let Some(kc) = komga_client {
            if let Some(b_id) = komga_book_id {
                let _ = kc.refresh_book_metadata(b_id).await;
            } else if let Some(s_id) = komga_series_id {
                let _ = kc.refresh_series_metadata(s_id).await;
            }
        }

        tracing::info!(
            "NORMALIZATION: successfully processed {} (series {}, volume {}) -> {}",
            canonical_path.display(),
            series_title,
            volume,
            rec.status
        );

        Ok(rec)
    }

    /// Reverts a normalized book to its original ComicInfo.xml using stored rollback data.
    pub async fn rollback_normalization(
        &self,
        history_id: &str,
        komga_client: Option<&KomgaClient>,
    ) -> Result<NormalizationHistoryRecord> {
        let (path_str, orig_xml, _orig_sha, komga_book_id, komga_series_id, _status) = self
            .db
            .call({
                let h_id = history_id.to_string();
                move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT canonical_path, original_comic_info_xml, original_sha256, komga_book_id, komga_series_id, status
                         FROM normalization_history WHERE id = ?1",
                    )?;
                    let res: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, String)> = stmt
                        .query_row([h_id], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                        })
                        .optional()?;
                    Ok(res)
                }
            })
            .await?
            .ok_or_else(|| AppError::not_found(format!("Normalization record {history_id} not found")))?;

        let canonical_path = PathBuf::from(&path_str);
        if !canonical_path.is_file() {
            return Err(AppError::not_found(format!(
                "Cannot revert: canonical file {} not found on disk",
                canonical_path.display()
            )));
        }

        verify_cbz_integrity(&canonical_path)?;

        // Revert ComicInfo in CBZ
        let parent = canonical_path.parent().unwrap_or_else(|| Path::new("."));
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let temp_path = parent.join(format!(
            ".{}.revert-{pid}-{nanos}.tmp",
            canonical_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));

        struct CleanupGuard<'a>(&'a Path);
        impl<'a> Drop for CleanupGuard<'a> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }
        let temp_guard = CleanupGuard(&temp_path);

        // Read source zip and write to temp with restored ComicInfo
        let src_file = File::open(&canonical_path)?;
        let mut src_zip = zip::ZipArchive::new(src_file)
            .map_err(|e| AppError::bad_request(format!("Cannot open source CBZ: {e}")))?;

        let dest_file = File::create(&temp_path)?;
        let mut dest_zip = zip::ZipWriter::new(dest_file);
        let opt = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        for i in 0..src_zip.len() {
            let mut entry = src_zip
                .by_index(i)
                .map_err(|e| AppError::bad_request(e.to_string()))?;
            let name = entry.name().to_string();
            if name.eq_ignore_ascii_case("comicinfo.xml")
                || name.to_lowercase().ends_with("/comicinfo.xml")
            {
                continue;
            }

            dest_zip
                .start_file(&name, opt)
                .map_err(|e| AppError::internal(e.to_string()))?;
            std::io::copy(&mut entry, &mut dest_zip)
                .map_err(|e| AppError::internal(e.to_string()))?;
        }

        // Restore original ComicInfo if it was present
        if let Some(xml_content) = &orig_xml {
            dest_zip
                .start_file("ComicInfo.xml", opt)
                .map_err(|e| AppError::internal(e.to_string()))?;
            dest_zip
                .write_all(xml_content.as_bytes())
                .map_err(|e| AppError::internal(e.to_string()))?;
        }

        dest_zip
            .finish()
            .map_err(|e| AppError::internal(format!("Failed to finalize reverted CBZ: {e}")))?;
        verify_cbz_integrity(&temp_path)?;

        fs::rename(&temp_path, &canonical_path).map_err(|e| AppError::internal(e.to_string()))?;
        std::mem::forget(temp_guard);

        // Update database status
        let now_ts = now();
        let h_id = history_id.to_string();
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE normalization_history SET status = 'reverted', reverted_at = ?2 WHERE id = ?1",
                    params![h_id, now_ts],
                )?;
                Ok(())
            })
            .await?;

        if let Some(kc) = komga_client {
            if let Some(b_id) = komga_book_id.as_deref() {
                let _ = kc.refresh_book_metadata(b_id).await;
            } else if let Some(s_id) = komga_series_id.as_deref() {
                let _ = kc.refresh_series_metadata(s_id).await;
            }
        }

        tracing::info!(
            "NORMALIZATION: successfully reverted {} using history record {history_id}",
            canonical_path.display()
        );

        let res = self.get_history_record(history_id).await?.unwrap();
        Ok(res)
    }

    /// Fetches a normalization history record by ID.
    pub async fn get_history_record(&self, id: &str) -> Result<Option<NormalizationHistoryRecord>> {
        let h_id = id.to_string();
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, batch_id, canonical_path, series_title, volume,
                            komga_series_id, komga_book_id, mangabaka_id, mangabaka_release_id,
                            anilist_id, original_sha256, normalized_sha256, status,
                            error_message, normalized_at, reverted_at
                     FROM normalization_history WHERE id = ?1",
                )?;
                let res = stmt
                    .query_row([h_id], |r| {
                        Ok(NormalizationHistoryRecord {
                            id: r.get(0)?,
                            batch_id: r.get(1)?,
                            canonical_path: r.get(2)?,
                            series_title: r.get(3)?,
                            volume: r.get(4)?,
                            komga_series_id: r.get(5)?,
                            komga_book_id: r.get(6)?,
                            mangabaka_id: r.get(7)?,
                            mangabaka_release_id: r.get(8)?,
                            anilist_id: r.get(9)?,
                            original_sha256: r.get(10)?,
                            normalized_sha256: r.get(11)?,
                            status: r.get(12)?,
                            error_message: r.get(13)?,
                            normalized_at: r.get(14)?,
                            reverted_at: r.get(15)?,
                        })
                    })
                    .optional()?;
                Ok(res)
            })
            .await
    }

    /// Lists recent normalization history records.
    pub async fn list_history(&self, limit: usize) -> Result<Vec<NormalizationHistoryRecord>> {
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, batch_id, canonical_path, series_title, volume,
                            komga_series_id, komga_book_id, mangabaka_id, mangabaka_release_id,
                            anilist_id, original_sha256, normalized_sha256, status,
                            error_message, normalized_at, reverted_at
                     FROM normalization_history
                     ORDER BY normalized_at DESC
                     LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map([limit], |r| {
                        Ok(NormalizationHistoryRecord {
                            id: r.get(0)?,
                            batch_id: r.get(1)?,
                            canonical_path: r.get(2)?,
                            series_title: r.get(3)?,
                            volume: r.get(4)?,
                            komga_series_id: r.get(5)?,
                            komga_book_id: r.get(6)?,
                            mangabaka_id: r.get(7)?,
                            mangabaka_release_id: r.get(8)?,
                            anilist_id: r.get(9)?,
                            original_sha256: r.get(10)?,
                            normalized_sha256: r.get(11)?,
                            status: r.get(12)?,
                            error_message: r.get(13)?,
                            normalized_at: r.get(14)?,
                            reverted_at: r.get(15)?,
                        })
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                Ok(rows)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::io::Write;

    #[test]
    fn test_comics_library_safely_skipped() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let service = LibraryNormalizationService::new(db, reqwest::Client::new());

        // 1. Explicit Comics Library ID 0RNH705VV374H -> false
        let comics_lib1 = KomgaLibrary {
            id: "0RNH705VV374H".into(),
            name: "Comics".into(),
            root: "/comics".into(),
            unavailable: false,
        };
        assert!(!service.is_library_eligible_for_mangabaka(&comics_lib1));

        // 2. Library with name containing comic -> false
        let comics_lib2 = KomgaLibrary {
            id: "other-id".into(),
            name: "Western Comic Books".into(),
            root: "/library/western".into(),
            unavailable: false,
        };
        assert!(!service.is_library_eligible_for_mangabaka(&comics_lib2));

        // 3. Library with root containing /comics -> false
        let comics_lib3 = KomgaLibrary {
            id: "lib-3".into(),
            name: "DC Universe".into(),
            root: "/srv/media/comics/dc".into(),
            unavailable: false,
        };
        assert!(!service.is_library_eligible_for_mangabaka(&comics_lib3));

        // 4. Eligible English Manga library -> true
        let manga_en = KomgaLibrary {
            id: "0RMWEXC3T10TY".into(),
            name: "Manga".into(),
            root: "/srv/media/manga".into(),
            unavailable: false,
        };
        assert!(service.is_library_eligible_for_mangabaka(&manga_en));

        // 5. Eligible Chinese Manga library -> true
        let manga_zh = KomgaLibrary {
            id: "0RZVF3P8MEC03".into(),
            name: "Chinese Manga".into(),
            root: "/srv/media/manga-chinese".into(),
            unavailable: false,
        };
        assert!(service.is_library_eligible_for_mangabaka(&manga_zh));
    }

    #[test]
    fn test_evaluate_metadata_diff_volume_semantics() {
        let legacy_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Series>Blue Lock</Series>
  <Number>39</Number>
  <Volume>39</Volume>
</ComicInfo>"#;

        let enrichment = MangaBakaEnrichment {
            series_id: 83,
            series_title: Some("Blue Lock".into()),
            work_title: Some("Volume 39".into()),
            collection_id: "col-1".into(),
            volume: 39.0,
            release_date: Some("2026-01-20".into()),
            summary: Some("Ego Jinpachi leads the next round.".into()),
            pages: Some(192),
            writers: vec!["Muneyuki Kaneshiro".into()],
            pencillers: vec!["Yusuke Nomura".into()],
            genres: vec!["Sports".into(), "Shounen".into()],
            tags: vec!["Soccer".into()],
            publisher: Some("Kodansha".into()),
            total_volumes: Some(39),
            web_url: Some("https://mangabaka.org/series/83".into()),
            language: Some("en".into()),
            work_id: None,
        };

        let (_status, _reason, diffs) = LibraryNormalizationService::evaluate_metadata_diff(
            Some(legacy_xml),
            &enrichment,
            39.0,
            192,
        );

        // Strict Volume Semantics: Number tag was "39", proposed is None
        let num_diff = diffs.iter().find(|d| d.field == "Number");
        assert!(num_diff.is_some(), "Expected Number tag diff");
        assert_eq!(num_diff.unwrap().current.as_deref(), Some("39"));
        assert_eq!(num_diff.unwrap().proposed, None);

        // Writer was missing, proposed is "Muneyuki Kaneshiro"
        let writer_diff = diffs.iter().find(|d| d.field == "Writer");
        assert!(writer_diff.is_some());
        assert_eq!(writer_diff.unwrap().current, None);
        assert_eq!(
            writer_diff.unwrap().proposed.as_deref(),
            Some("Muneyuki Kaneshiro")
        );

        // Publisher was missing, proposed is "Kodansha"
        let pub_diff = diffs.iter().find(|d| d.field == "Publisher");
        assert!(pub_diff.is_some());
        assert_eq!(pub_diff.unwrap().proposed.as_deref(), Some("Kodansha"));
    }

    #[tokio::test]
    async fn test_atomic_staging_idempotency_and_rollback() {
        use axum::{Json, Router, routing::get};
        use serde_json::json;

        let db = Db::open(Path::new(":memory:")).unwrap();
        let service = LibraryNormalizationService::new(db, reqwest::Client::new());

        // Setup mock MangaBaka server
        let mock_app = Router::new()
            .route(
                "/series/83",
                get(|| async {
                    Json(json!({
                        "id": 83,
                        "title": "Blue Lock",
                        "status": "ongoing",
                        "total_volumes": 39,
                        "authors": ["Muneyuki Kaneshiro"],
                        "artists": ["Yusuke Nomura"],
                        "publisher": "Kodansha",
                        "genres": ["Sports", "Shounen"],
                        "tags": ["Soccer"],
                        "summary": "High school soccer strikers compete."
                    }))
                }),
            )
            .route(
                "/series/83/collections",
                get(|| async {
                    Json(json!({
                        "data": [{
                            "id": "col-83",
                            "name": "Standard Tankobon",
                            "edition": "standard",
                            "language": "en",
                            "medium": "digital",
                            "total_works": 39
                        }]
                    }))
                }),
            )
            .route(
                "/collections/col-83/works",
                get(|| async {
                    Json(json!({
                        "data": [{
                            "id": "work-39",
                            "title": "Volume 39",
                            "sequence_numeric": 39.0,
                            "release_date": "2026-01-20",
                            "pages": 192,
                            "description": "The climax of the tournament."
                        }]
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let mb_client = MangaBakaClient::new(
            reqwest::Client::new(),
            Some(format!("http://{addr}")),
            Some("fake-token".into()),
        );

        // Create temporary CBZ fixture
        let temp_dir = std::env::temp_dir().join(format!("mango-norm-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let cbz_path = temp_dir.join("Blue Lock v39.cbz");

        let legacy_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Series>Blue Lock</Series>
  <Number>39</Number>
</ComicInfo>"#;

        {
            let file = File::create(&cbz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opt = zip::write::SimpleFileOptions::default();
            zip.start_file("001.jpg", opt).unwrap();
            zip.write_all(b"page 1 bytes").unwrap();
            zip.start_file("002.jpg", opt).unwrap();
            zip.write_all(b"page 2 bytes").unwrap();
            zip.start_file("ComicInfo.xml", opt).unwrap();
            zip.write_all(legacy_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let orig_sha = calculate_file_sha256(&cbz_path).unwrap();

        // 1. First Pass: normalize_book
        let record = service
            .normalize_book(
                "batch-1",
                &cbz_path,
                "Blue Lock",
                39.0,
                83,
                None,
                Some("komga-s1"),
                Some("komga-b39"),
                &mb_client,
                None,
            )
            .await
            .unwrap();

        assert_eq!(record.status, "normalized");
        assert_eq!(record.original_sha256.as_deref(), Some(orig_sha.as_str()));
        assert!(record.normalized_sha256.is_some());
        verify_cbz_integrity(&cbz_path).unwrap();

        // Check the written ComicInfo in CBZ
        let new_xml = read_comic_info_xml_from_cbz(&cbz_path).unwrap().unwrap();
        assert!(new_xml.contains("<Volume>39</Volume>"));
        assert!(!new_xml.contains("<Number>39</Number>"));
        assert!(new_xml.contains("<Writer>Muneyuki Kaneshiro</Writer>"));
        assert!(new_xml.contains("<Publisher>Kodansha</Publisher>"));

        // 2. Second Pass: Idempotency (already_normalized)
        let record_second = service
            .normalize_book(
                "batch-2",
                &cbz_path,
                "Blue Lock",
                39.0,
                83,
                None,
                Some("komga-s1"),
                Some("komga-b39"),
                &mb_client,
                None,
            )
            .await
            .unwrap();

        assert_eq!(record_second.status, "already_normalized");

        // 3. Rollback: Lossless revert
        let reverted = service
            .rollback_normalization(&record.id, None)
            .await
            .unwrap();

        assert_eq!(reverted.status, "reverted");
        assert!(reverted.reverted_at.is_some());
        verify_cbz_integrity(&cbz_path).unwrap();

        let rolled_back_xml = read_comic_info_xml_from_cbz(&cbz_path).unwrap().unwrap();
        assert_eq!(rolled_back_xml.trim(), legacy_xml.trim());
        let current_sha = calculate_file_sha256(&cbz_path).unwrap();
        assert_eq!(current_sha, orig_sha);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_concurrent_file_lock_rejection() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let service = LibraryNormalizationService::new(db, reqwest::Client::new());

        let test_path = PathBuf::from("/tmp/concurrent-test.cbz");

        // Manually acquire lock
        {
            let mut locks = service.file_locks.lock().await;
            locks.insert(test_path.clone());
        }

        let mb_client = MangaBakaClient::new(
            reqwest::Client::new(),
            Some("http://127.0.0.1:9".into()),
            None,
        );

        // Attempt normalization while locked -> must fail closed with 409
        let err = service
            .normalize_book(
                "batch-conflict",
                &test_path,
                "Series",
                1.0,
                100,
                None,
                None,
                None,
                &mb_client,
                None,
            )
            .await
            .unwrap_err();

        assert_eq!(err.status, 409);
        assert_eq!(err.code, "file_locked");
    }
}
