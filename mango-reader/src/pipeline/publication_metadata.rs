//! Centralized publication metadata resolution and validation engine.
//!
//! Enforces authoritative, library-wide metadata consistency across ALL acquisition sources:
//! - Madokami direct downloads
//! - Nyaa / TorBox torrent acquisitions
//! - Keiyoushi extensions
//! - Physical and manual imports
//!
//! Guarantees:
//! - Single source of truth for canonical series identity, volume/chapter numbering,
//!   reader-facing titles, physical CBZ filenames, ComicInfo.xml fields, and Komga expectations.
//! - Fail-closed pre-publication validation: detects already-owned volumes, duplicate releases,
//!   mismatched numbers, edition/omnibus conflicts, and archive corruption BEFORE publication.
//! - Comprehensive pre-publication preview and post-scan reconciliation verification.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::pipeline::{
    komga::KomgaClient,
    mangabaka::MangaBakaEnrichment,
    naming::{self, CanonicalNamingResult},
    publisher,
    PipelineConfig,
};

/// Pre-publication ownership classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrePublicationOwnership {
    /// Not owned anywhere in the library.
    NotOwned,
    /// Already published and identical archive exists at canonical destination.
    AlreadyPublished,
    /// Destination path exists on disk with differing content (safety block).
    DestinationConflict,
    /// Komga already owns this volume from a different file/release (duplicate release).
    AlreadyOwnedInKomga,
    /// An omnibus or multi-volume range already covers this volume in Komga.
    OmnibusConflict,
    /// Acquiring / download in progress elsewhere.
    Acquiring,
    /// Unknown or unverified ownership state.
    Unknown,
}

impl std::fmt::Display for PrePublicationOwnership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOwned => write!(f, "Not owned"),
            Self::AlreadyPublished => write!(f, "Already published"),
            Self::DestinationConflict => write!(f, "Destination conflict"),
            Self::AlreadyOwnedInKomga => write!(f, "Already owned in Komga"),
            Self::OmnibusConflict => write!(f, "Omnibus conflict"),
            Self::Acquiring => write!(f, "Acquiring"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Consolidated, authoritative publication metadata for a manga acquisition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationMetadata {
    /// Chapter ID in Mango database.
    pub chapter_id: i64,
    /// Manga ID in Mango database.
    pub manga_id: i64,
    /// Canonical series title (e.g. "Haikyu!!").
    pub series_title: String,
    /// Publication series title (edition-aware, e.g. "Haikyu!!" or "Haikyu!! (Omnibus)").
    pub publication_series_title: String,
    /// Whether this release is a true volume (true) or chapter (false).
    pub is_volume: bool,
    /// Structured volume number if applicable.
    pub volume_number: Option<f64>,
    /// Structured chapter number if applicable.
    pub chapter_number: Option<f64>,
    /// Formatted number string (e.g. "03" for vol 3, "001" for ch 1).
    pub formatted_number: String,
    /// Authoritative reader-facing title (e.g. "Volume 03" or "Chapter 001 - Title").
    pub canonical_reader_title: String,
    /// Canonical physical CBZ filename (e.g. "Haikyu!! v03.cbz").
    pub canonical_filename: String,
    /// ComicInfo <Volume> tag value (e.g. "3").
    pub comic_info_volume: Option<String>,
    /// ComicInfo <Number> tag value (for chapters, e.g. "1").
    pub comic_info_number: Option<String>,
    /// MangaBaka catalog series ID.
    pub mangabaka_series_id: Option<i64>,
    /// MangaBaka catalog volume number.
    pub mangabaka_volume: Option<f64>,
    /// MangaBaka release/work ID.
    pub mangabaka_release_id: Option<String>,
    /// AniList catalog tracker ID.
    pub anilist_id: Option<i64>,
    /// Edition name (e.g. "standard", "omnibus", "kanzenban").
    pub edition: Option<String>,
    /// Target language code (e.g. "en", "zh").
    pub language: String,
    /// Acquisition source provider (e.g. "madokami", "nyaa", "extension").
    pub source_type: String,
    /// Underlying backend downloader (e.g. "madokami", "torbox", "internal").
    pub backend_type: String,
    /// Source release group / scanlator (e.g. "danke-Empire").
    pub release_group: Option<String>,
    /// Source uploader if known.
    pub uploader: Option<String>,
    /// Raw release or chapter name from source.
    pub raw_title: String,
    /// Destination library root directory.
    pub target_library_root: PathBuf,
    /// Destination Komga library ID if configured.
    pub target_library_id: Option<String>,
    /// Full canonical destination path for the CBZ file.
    pub canonical_destination: PathBuf,
    /// Pre-publication ownership evaluation.
    pub ownership_status: PrePublicationOwnership,
    /// Existing file path in Komga or library if owned.
    pub existing_path: Option<String>,
    /// Existing book ID in Komga if owned.
    pub existing_book_id: Option<String>,
    /// Metadata match confidence ("High", "Medium", "Ambiguous", "User Override").
    pub confidence: String,
    /// Staging archive status ("Validated", "Corrupted", "Pending", "Missing").
    pub archive_status: String,
    /// Total image page count in the archive if staged.
    pub page_count: usize,
    /// Validation warnings or advisory notices.
    pub warnings: Vec<String>,
    /// Whether all safety gates pass and publication is authorized.
    pub is_safe_to_publish: bool,
}

/// Result of post-scan Komga book reconciliation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KomgaReconciliationOutcome {
    pub is_verified: bool,
    pub needs_review: bool,
    pub book_id: Option<String>,
    pub series_id: Option<String>,
    pub displayed_title: Option<String>,
    pub expected_title: String,
    pub book_number: Option<f64>,
    pub expected_number: f64,
    pub komga_pages_count: Option<i64>,
    pub expected_pages_count: usize,
    pub media_status: Option<String>,
    pub details: String,
    pub warnings: Vec<String>,
}

/// Resolves publication metadata given a database chapter ID.
pub async fn resolve_publication_metadata(
    db: &crate::db::Db,
    komga_client: Option<&KomgaClient>,
    chapter_id: i64,
    staging_file: Option<&Path>,
    config: &PipelineConfig,
) -> Result<PublicationMetadata> {
    struct RowData {
        chapter_id: i64,
        manga_id: i64,
        manga_title: String,
        chapter_name: String,
        chapter_number: Option<f64>,
        scanlator: Option<String>,
        source_type: Option<String>,
        backend_type: Option<String>,
        uploader: Option<String>,
        raw_title: Option<String>,
        retrieval_identity_json: Option<String>,
        published_path: Option<String>,
    }

    let row = db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.manga_id, m.title, c.name, c.chapter_number, c.scanlator,
                        d.source_type, d.backend_type, d.uploader, d.raw_title, d.retrieval_identity_json,
                        d.published_path
                 FROM chapters c
                 JOIN manga m ON m.id = c.manga_id
                 LEFT JOIN downloads d ON d.chapter_id = c.id
                 WHERE c.id = ?1",
            )?;

            stmt.query_row([chapter_id], |r| {
                Ok(RowData {
                    chapter_id: r.get(0)?,
                    manga_id: r.get(1)?,
                    manga_title: r.get(2)?,
                    chapter_name: r.get(3)?,
                    chapter_number: r.get(4)?,
                    scanlator: r.get(5)?,
                    source_type: r.get(6)?,
                    backend_type: r.get(7)?,
                    uploader: r.get(8)?,
                    raw_title: r.get(9)?,
                    retrieval_identity_json: r.get(10)?,
                    published_path: r.get(11)?,
                })
            })
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Chapter {chapter_id} not found")))
        })
        .await?;

    // Parse retrieval identity if present
    let retrieval_identity: Option<crate::acquisition::edition::RetrievalIdentity> = row
        .retrieval_identity_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());

    let (is_volume, raw_number) = if let Some(ri) = &retrieval_identity {
        match ri.requested_unit {
            crate::acquisition::AcquisitionReleaseUnit::Volume(v) => (true, Some(v)),
            crate::acquisition::AcquisitionReleaseUnit::Chapter(c) => (false, Some(c)),
            _ => {
                let is_vol = row.chapter_name.to_lowercase().contains("volume")
                    || row.chapter_name.to_lowercase().contains("vol.")
                    || row.chapter_name.to_lowercase().starts_with("vol ")
                    || (row.chapter_name.to_lowercase().starts_with('v')
                        && row.chapter_name.chars().nth(1).is_some_and(|c| c.is_ascii_digit()));
                let num = row.chapter_number.or_else(|| {
                    naming::parse_number_from_text(&row.chapter_name, is_vol)
                        .or_else(|| row.raw_title.as_deref().and_then(|r| naming::parse_number_from_text(r, is_vol)))
                });
                (is_vol, num)
            }
        }
    } else {
        let is_vol = row.chapter_name.to_lowercase().contains("volume")
            || row.chapter_name.to_lowercase().contains("vol.")
            || row.chapter_name.to_lowercase().starts_with("vol ")
            || (row.chapter_name.to_lowercase().starts_with('v')
                && row.chapter_name.chars().nth(1).is_some_and(|c| c.is_ascii_digit()));
        let num = row.chapter_number.or_else(|| {
            naming::parse_number_from_text(&row.chapter_name, is_vol)
                .or_else(|| row.raw_title.as_deref().and_then(|r| naming::parse_number_from_text(r, is_vol)))
        });
        (is_vol, num)
    };

    let release_group = row.scanlator.clone().or_else(|| row.uploader.clone());
    let source_type = row.source_type.unwrap_or_else(|| "reader".to_string());
    let backend_type = row.backend_type.unwrap_or_else(|| "internal".to_string());
    let raw_title = row.raw_title.unwrap_or(row.chapter_name.clone());

    let requested_edition = retrieval_identity.as_ref().and_then(|ri| ri.requested_edition.clone());

    resolve_metadata_internal(
        row.chapter_id,
        row.manga_id,
        &row.manga_title,
        &raw_title,
        is_volume,
        raw_number,
        release_group.as_deref(),
        row.uploader.as_deref(),
        &source_type,
        &backend_type,
        requested_edition.as_deref(),
        retrieval_identity.as_ref(),
        staging_file,
        row.published_path.as_deref(),
        config,
        komga_client,
        None,
    ).await
}

/// Resolves publication metadata for an in-flight Job and optional MangaBaka enrichment.
pub async fn resolve_publication_metadata_for_job(
    job: &crate::downloads::Job,
    staging_file: Option<&Path>,
    config: &PipelineConfig,
    komga_client: Option<&KomgaClient>,
    enrichment: Option<&MangaBakaEnrichment>,
) -> Result<PublicationMetadata> {
    let is_volume = job.release_type == crate::release::ReleaseType::Volume;
    let release_num = if is_volume {
        job.volume.unwrap_or(job.number)
    } else {
        job.chapter_number.unwrap_or(job.number)
    };

    let retrieval_identity: Option<crate::acquisition::edition::RetrievalIdentity> = job
        .retrieval_identity_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());

    let requested_edition = retrieval_identity.as_ref().and_then(|ri| ri.requested_edition.clone());

    let raw_title = job.raw_title.as_deref().unwrap_or(&job.name).to_string();
    let release_group = job.scanlator.as_deref().or(job.uploader.as_deref());

    resolve_metadata_internal(
        job.chapter,
        job.manga_id,
        &job.title,
        &raw_title,
        is_volume,
        Some(release_num),
        release_group,
        job.uploader.as_deref(),
        &job.source_type,
        &job.backend_type,
        requested_edition.as_deref(),
        retrieval_identity.as_ref(),
        staging_file,
        job.published_path.as_deref(),
        config,
        komga_client,
        enrichment,
    ).await
}

/// Internal resolution and validation logic.
async fn resolve_metadata_internal(
    chapter_id: i64,
    manga_id: i64,
    series_title: &str,
    raw_title: &str,
    is_volume: bool,
    number_opt: Option<f64>,
    release_group: Option<&str>,
    uploader: Option<&str>,
    source_type: &str,
    backend_type: &str,
    requested_edition: Option<&str>,
    retrieval_identity: Option<&crate::acquisition::edition::RetrievalIdentity>,
    staging_file: Option<&Path>,
    published_path: Option<&str>,
    config: &PipelineConfig,
    komga_client: Option<&KomgaClient>,
    enrichment: Option<&MangaBakaEnrichment>,
) -> Result<PublicationMetadata> {
    let mut warnings: Vec<String> = Vec::new();
    let is_chinese = series_title.chars().any(|c| c >= '\u{4e00}' && c <= '\u{9fff}')
        || raw_title.to_lowercase().contains("chinese")
        || raw_title.to_lowercase().contains("mandarin");

    // 1. Determine destination permanent library root
    let dest_root = if is_chinese
        && config
            .chinese_library_dir
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
    {
        PathBuf::from(config.chinese_library_dir.as_ref().unwrap())
    } else if let Some(dir) = &config.library_dir {
        PathBuf::from(dir)
    } else {
        PathBuf::from("/srv/media/manga")
    };

    let target_library_id = if is_chinese {
        config
            .komga_chinese_library_id
            .as_deref()
            .or(config.komga_library_id.as_deref())
            .map(|s| s.to_string())
    } else {
        config.komga_library_id.as_deref().map(|s| s.to_string())
    };

    // 2. Publication series title: preserve edition namespace if requested
    let publication_series_title = match requested_edition {
        Some(edition) if edition != "standard" => {
            let edition_cap = {
                let mut c = edition.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            };
            format!("{} ({})", series_title.trim(), edition_cap)
        }
        _ => series_title.trim().to_string(),
    };

    // 3. Resolve Numbering
    let mut release_number = number_opt;
    if release_number.is_none() {
        release_number = naming::parse_number_from_text(raw_title, is_volume);
    }

    let num_val = match release_number {
        Some(n) if n >= 0.0 => n,
        _ => {
            warnings.push("Missing or undetectable volume/chapter number in release title".to_string());
            0.0
        }
    };

    let formatted_number = if is_volume {
        if num_val.fract() == 0.0 {
            format!("{:02}", num_val as i64)
        } else {
            format!("{:02}.{}", num_val.trunc() as i64, num_val.fract().to_string().trim_start_matches("0."))
        }
    } else if num_val.fract() == 0.0 {
        format!("{:03}", num_val as i64)
    } else {
        format!("{:03}.{}", num_val.trunc() as i64, num_val.fract().to_string().trim_start_matches("0."))
    };

    // 4. Resolve Canonical Reader Title
    let naming_result: CanonicalNamingResult = naming::normalize_book_naming(
        raw_title,
        &publication_series_title,
        is_volume,
        Some(num_val),
    );
    let canonical_reader_title = naming_result.title;

    // 5. Canonical Filename and Destination Path
    let safe_series = match publisher::sanitize_series_name(&publication_series_title) {
        Ok(s) => s,
        Err(e) => {
            warnings.push(format!("Unsafe series name: {e}"));
            "unknown_series".to_string()
        }
    };

    let canonical_release_stem = publisher::canonical_release_name(&safe_series, is_volume, num_val);
    let canonical_filename = format!("{canonical_release_stem}.cbz");
    let canonical_destination = dest_root.join(&safe_series).join(&canonical_filename);

    // 6. ComicInfo tag values
    let comic_info_volume = if is_volume {
        Some(if num_val.fract() == 0.0 {
            (num_val as i64).to_string()
        } else {
            num_val.to_string()
        })
    } else {
        None
    };

    let comic_info_number = if !is_volume {
        Some(if num_val.fract() == 0.0 {
            (num_val as i64).to_string()
        } else {
            num_val.to_string()
        })
    } else {
        None
    };

    // 7. MangaBaka Metadata Matching & Confidence
    let mut mangabaka_series_id = None;
    let mut mangabaka_volume = None;
    let mut mangabaka_release_id = None;
    let anilist_id = None;
    let mut confidence = "Medium".to_string();

    if let Some(e) = enrichment {
        if e.series_id > 0 {
            mangabaka_series_id = Some(e.series_id);
        }
        mangabaka_volume = Some(e.volume);
        mangabaka_release_id = e.work_id.clone().or_else(|| {
            if e.collection_id.is_empty() { None } else { Some(e.collection_id.clone()) }
        });
        if (e.volume - num_val).abs() > 0.001 {
            warnings.push(format!(
                "MangaBaka volume mismatch: catalog indicates volume {}, but release indicates volume {}",
                e.volume, num_val
            ));
            confidence = "Ambiguous".to_string();
        } else {
            confidence = "High".to_string();
        }
    } else if let Some(ri) = retrieval_identity {
        if ri.identity_override {
            confidence = "User Override".to_string();
        }
    }

    // 8. Staging Archive Validation
    let mut archive_status = "Pending".to_string();
    let mut page_count = 0usize;

    if let Some(archive_path) = staging_file.filter(|p| p.is_file()) {
        match publisher::verify_cbz_integrity(archive_path) {
            Ok(()) => {
                archive_status = "Validated".to_string();
                if let Ok(file) = fs::File::open(archive_path) {
                    if let Ok(mut zip) = zip::ZipArchive::new(file) {
                        for i in 0..zip.len() {
                            if let Ok(entry) = zip.by_index(i) {
                                let name = entry.name();
                                if entry.is_file() && !name.starts_with("__MACOSX") && crate::local::is_image(name) {
                                    page_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                archive_status = "Corrupted".to_string();
                warnings.push(format!("Staged archive failed integrity validation: {e}"));
            }
        }
    } else if let Some(pub_path) = published_path.map(Path::new).filter(|p| p.is_file()) {
        if publisher::verify_cbz_integrity(pub_path).is_ok() {
            archive_status = "Validated".to_string();
        }
    }

    // 9. Pre-Publication Ownership & Conflict Validation
    let mut ownership_status = PrePublicationOwnership::NotOwned;
    let mut existing_path = None;
    let mut existing_book_id = None;

    // A. Check filesystem destination
    if canonical_destination.is_file() {
        if let Some(staged) = staging_file.filter(|p| p.is_file()) {
            match publisher::files_are_identical(staged, &canonical_destination) {
                Ok(true) => {
                    ownership_status = PrePublicationOwnership::AlreadyPublished;
                    existing_path = Some(canonical_destination.to_string_lossy().to_string());
                }
                Ok(false) => {
                    ownership_status = PrePublicationOwnership::DestinationConflict;
                    existing_path = Some(canonical_destination.to_string_lossy().to_string());
                    warnings.push(format!(
                        "Destination file '{}' already exists on disk with different content (refusing silent overwrite)",
                        canonical_destination.display()
                    ));
                }
                Err(e) => {
                    warnings.push(format!("Failed comparing destination file: {e}"));
                }
            }
        } else {
            ownership_status = PrePublicationOwnership::AlreadyPublished;
            existing_path = Some(canonical_destination.to_string_lossy().to_string());
        }
    }

    // B. Check Komga ownership and conflicting releases (if Komga connected)
    if let Some(komga) = komga_client {
        if let Ok(series_list) = komga.search_series(&publication_series_title, target_library_id.as_deref()).await {
            let matched_series = series_list.into_iter().find(|s| {
                s.name.eq_ignore_ascii_case(&publication_series_title)
                    || s.name.eq_ignore_ascii_case(series_title)
            });

            if let Some(series) = matched_series {
                if let Ok(books) = komga.get_books_in_series(&series.id).await {
                    let series_context = crate::release::determine_series_context(books.iter().map(|b| {
                        (b.name.as_str(), b.metadata.as_ref().and_then(|m| m.title.as_deref()))
                    }));

                    for b in &books {
                        let meta_title = b.metadata.as_ref().and_then(|m| m.title.as_deref());
                        let meta_num = b.metadata.as_ref().and_then(|m| m.number.as_deref());
                        let (unit, num_opt) = crate::release::classify_komga_book(
                            &b.name,
                            meta_title,
                            meta_num,
                            b.number,
                            series_context,
                        );

                        // Check omnibus / range conflict:
                        // If an existing Komga book covers a multi-volume range (e.g. Volumes 1-3)
                        let b_lower = b.name.to_lowercase();
                        if is_volume && (b_lower.contains("v0") || b_lower.contains("vol") || b_lower.contains("volume")) {
                            if let Some((start, end)) = parse_volume_range_from_title(&b.name) {
                                if num_val >= start && num_val <= end {
                                    ownership_status = PrePublicationOwnership::OmnibusConflict;
                                    existing_book_id = Some(b.id.clone());
                                    existing_path = b.url.clone();
                                    warnings.push(format!(
                                        "Omnibus conflict: existing book '{}' in Komga series '{}' already covers Volume {}",
                                        b.name, series.name, num_val
                                    ));
                                }
                            }
                        }

                        // Check exact volume ownership:
                        let matched_unit = if is_volume {
                            unit == crate::release::ReleaseUnit::Volume
                        } else {
                            unit == crate::release::ReleaseUnit::Chapter
                        };

                        if matched_unit {
                            let book_val = num_opt
                                .or_else(|| {
                                    b.metadata.as_ref().and_then(|m| {
                                        m.number_sort.or_else(|| {
                                            m.number.as_deref().and_then(|n| n.parse::<f64>().ok())
                                        })
                                    })
                                })
                                .unwrap_or(b.number);

                            if (book_val - num_val).abs() < 0.001 {
                                if ownership_status == PrePublicationOwnership::NotOwned {
                                    ownership_status = PrePublicationOwnership::AlreadyOwnedInKomga;
                                    existing_book_id = Some(b.id.clone());
                                    existing_path = b.url.clone();
                                    warnings.push(format!(
                                        "Volume {} is already owned in Komga series '{}' as '{}'",
                                        num_val, series.name, b.name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 10. Safety Verdict
    let has_valid_number = release_number.is_some_and(|n| n >= 0.0);
    let is_safe_to_publish = has_valid_number
        && ownership_status != PrePublicationOwnership::DestinationConflict
        && ownership_status != PrePublicationOwnership::OmnibusConflict
        && archive_status != "Corrupted"
        && !canonical_destination.as_os_str().is_empty();

    Ok(PublicationMetadata {
        chapter_id,
        manga_id,
        series_title: series_title.trim().to_string(),
        publication_series_title,
        is_volume,
        volume_number: if is_volume { Some(num_val) } else { None },
        chapter_number: if !is_volume { Some(num_val) } else { None },
        formatted_number,
        canonical_reader_title,
        canonical_filename,
        comic_info_volume,
        comic_info_number,
        mangabaka_series_id,
        mangabaka_volume,
        mangabaka_release_id,
        anilist_id,
        edition: requested_edition.map(str::to_string),
        language: if is_chinese { "zh".into() } else { "en".into() },
        source_type: source_type.to_string(),
        backend_type: backend_type.to_string(),
        release_group: release_group.map(str::to_string),
        uploader: uploader.map(str::to_string),
        raw_title: raw_title.to_string(),
        target_library_root: dest_root,
        target_library_id,
        canonical_destination,
        ownership_status,
        existing_path,
        existing_book_id,
        confidence,
        archive_status,
        page_count,
        warnings,
        is_safe_to_publish,
    })
}

/// Helper to parse a multi-volume range like "v01-03" or "Volumes 1-3" from a book title.
fn parse_volume_range_from_title(title: &str) -> Option<(f64, f64)> {
    let lower = title.to_lowercase();
    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.') {
        if let Some((start_str, end_str)) = token.split_once('-') {
            let s_clean = start_str.trim_start_matches(|c: char| !c.is_ascii_digit());
            let e_clean = end_str.trim_start_matches(|c: char| !c.is_ascii_digit());
            if let (Ok(s), Ok(e)) = (s_clean.parse::<f64>(), e_clean.parse::<f64>()) {
                if s > 0.0 && e >= s && (e - s) < 50.0 {
                    return Some((s, e));
                }
            }
        }
    }
    None
}

/// Authoritative post-publication Komga reconciliation verification.
pub async fn reconcile_published_book(
    komga_client: &KomgaClient,
    pub_meta: &PublicationMetadata,
) -> Result<KomgaReconciliationOutcome> {
    let mut warnings: Vec<String> = Vec::new();

    // 1. Find series in Komga
    let series_candidates = match komga_client.search_series(&pub_meta.publication_series_title, pub_meta.target_library_id.as_deref()).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(KomgaReconciliationOutcome {
                is_verified: false,
                needs_review: true,
                book_id: None,
                series_id: None,
                displayed_title: None,
                expected_title: pub_meta.canonical_reader_title.clone(),
                book_number: None,
                expected_number: pub_meta.volume_number.or(pub_meta.chapter_number).unwrap_or(0.0),
                komga_pages_count: None,
                expected_pages_count: pub_meta.page_count,
                media_status: None,
                details: format!("Failed searching Komga for series '{}': {e}", pub_meta.publication_series_title),
                warnings: vec![format!("Series search failed: {e}")],
            });
        }
    };

    let matched_series = match series_candidates.into_iter().find(|s| {
        s.name.eq_ignore_ascii_case(&pub_meta.publication_series_title)
            || s.name.eq_ignore_ascii_case(&pub_meta.series_title)
    }) {
        Some(s) => s,
        None => {
            return Ok(KomgaReconciliationOutcome {
                is_verified: false,
                needs_review: false, // May still be indexing
                book_id: None,
                series_id: None,
                displayed_title: None,
                expected_title: pub_meta.canonical_reader_title.clone(),
                book_number: None,
                expected_number: pub_meta.volume_number.or(pub_meta.chapter_number).unwrap_or(0.0),
                komga_pages_count: None,
                expected_pages_count: pub_meta.page_count,
                media_status: None,
                details: format!("Series '{}' not yet visible in Komga library", pub_meta.publication_series_title),
                warnings: vec!["Series not found in Komga".to_string()],
            });
        }
    };

    // 2. Fetch books in series
    let books = match komga_client.get_books_in_series(&matched_series.id).await {
        Ok(b) => b,
        Err(e) => {
            return Ok(KomgaReconciliationOutcome {
                is_verified: false,
                needs_review: true,
                book_id: None,
                series_id: Some(matched_series.id),
                displayed_title: None,
                expected_title: pub_meta.canonical_reader_title.clone(),
                book_number: None,
                expected_number: pub_meta.volume_number.or(pub_meta.chapter_number).unwrap_or(0.0),
                komga_pages_count: None,
                expected_pages_count: pub_meta.page_count,
                media_status: None,
                details: format!("Failed fetching books in series: {e}"),
                warnings: vec![format!("Book list failed: {e}")],
            });
        }
    };

    let expected_num = pub_meta.volume_number.or(pub_meta.chapter_number).unwrap_or(0.0);
    let expected_stem = pub_meta.canonical_filename.trim_end_matches(".cbz");

    let series_context = crate::release::determine_series_context(books.iter().map(|b| {
        (b.name.as_str(), b.metadata.as_ref().and_then(|m| m.title.as_deref()))
    }));

    // Find the book matching our release filename or volume number
    let mut matching_books = Vec::new();
    for b in &books {
        let meta_title = b.metadata.as_ref().and_then(|m| m.title.as_deref());
        let meta_num = b.metadata.as_ref().and_then(|m| m.number.as_deref());

        let (unit, num_opt) = crate::release::classify_komga_book(
            &b.name,
            meta_title,
            meta_num,
            b.number,
            series_context,
        );

        let unit_matches = if pub_meta.is_volume {
            unit == crate::release::ReleaseUnit::Volume
        } else {
            unit == crate::release::ReleaseUnit::Chapter
        };

        let num_matches = num_opt
            .or_else(|| {
                b.metadata.as_ref().and_then(|m| {
                    m.number_sort.or_else(|| m.number.as_deref().and_then(|n| n.parse::<f64>().ok()))
                })
            })
            .is_some_and(|n| (n - expected_num).abs() < 0.001);

        let name_matches = b.name.eq_ignore_ascii_case(expected_stem);

        if name_matches || (unit_matches && num_matches) {
            matching_books.push(b);
        }
    }

    if matching_books.is_empty() {
        return Ok(KomgaReconciliationOutcome {
            is_verified: false,
            needs_review: false, // May still be scanning
            book_id: None,
            series_id: Some(matched_series.id),
            displayed_title: None,
            expected_title: pub_meta.canonical_reader_title.clone(),
            book_number: None,
            expected_number: expected_num,
            komga_pages_count: None,
            expected_pages_count: pub_meta.page_count,
            media_status: None,
            details: format!("Book '{}' not yet indexed in Komga", pub_meta.canonical_filename),
            warnings: vec!["Book not yet visible in Komga".to_string()],
        });
    }

    // Check for duplicate books with the same number in Komga
    if matching_books.len() > 1 {
        warnings.push(format!(
            "Multiple books ({}) in Komga share the same volume/chapter number {}",
            matching_books.len(), expected_num
        ));
    }

    let target_book = matching_books.first().unwrap();
    let displayed_title = target_book.metadata.as_ref().and_then(|m| m.title.clone());
    let media_status = target_book.media.as_ref().and_then(|m| m.status.clone());
    let pages_count = target_book.media.as_ref().and_then(|m| m.pages_count);

    // If Komga is still performing media analysis or hasn't read metadata, wait for completion
    let is_in_progress = media_status
        .as_deref()
        .is_none_or(|s| s.eq_ignore_ascii_case("IN_PROGRESS") || s.eq_ignore_ascii_case("UNKNOWN"));
    if is_in_progress && displayed_title.is_none() {
        return Ok(KomgaReconciliationOutcome {
            is_verified: false,
            needs_review: false, // Still processing
            book_id: Some(target_book.id.clone()),
            series_id: Some(matched_series.id),
            displayed_title,
            expected_title: pub_meta.canonical_reader_title.clone(),
            book_number: Some(target_book.number),
            expected_number: expected_num,
            komga_pages_count: pages_count,
            expected_pages_count: pub_meta.page_count,
            media_status,
            details: format!(
                "Book '{}' registered in Komga; media analysis in progress...",
                pub_meta.canonical_filename
            ),
            warnings: vec!["Media analysis in progress in Komga".to_string()],
        });
    }

    let mut needs_review = false;

    // Check 1: Displayed Title Conformity
    if let Some(ref disp) = displayed_title {
        let is_canonical_match = disp == &pub_meta.canonical_reader_title
            || disp == expected_stem
            || disp.ends_with(&pub_meta.canonical_reader_title);
        if !is_canonical_match {
            needs_review = true;
            warnings.push(format!(
                "Komga displayed title mismatch: got '{}', expected canonical '{}'",
                disp, pub_meta.canonical_reader_title
            ));
        }
    } else {
        needs_review = true;
        warnings.push("Komga book title metadata is empty".to_string());
    }

    // Check 2: Archive media status
    if let Some(ref status) = media_status {
        if status.to_uppercase() == "ERROR" {
            needs_review = true;
            warnings.push("Komga media analysis reported an ERROR for this archive".to_string());
        }
    }

    // Check 3: Page count plausibility
    if let Some(pages) = pages_count {
        if pub_meta.page_count > 0 && pages as usize != pub_meta.page_count {
            warnings.push(format!(
                "Page count notice: Komga indexed {} pages, archive contains {} image pages",
                pages, pub_meta.page_count
            ));
        }
    }

    let is_verified = !needs_review && media_status.as_deref().unwrap_or("READY").to_uppercase() != "ERROR";
    let details = if is_verified {
        format!(
            "Verified in Komga: series '{}', book '{}', title '{}', media READY",
            matched_series.name, target_book.name, displayed_title.as_deref().unwrap_or("unknown")
        )
    } else {
        format!(
            "Reconciliation review required: {}",
            warnings.join("; ")
        )
    };

    Ok(KomgaReconciliationOutcome {
        is_verified,
        needs_review,
        book_id: Some(target_book.id.clone()),
        series_id: Some(matched_series.id),
        displayed_title,
        expected_title: pub_meta.canonical_reader_title.clone(),
        book_number: Some(target_book.number),
        expected_number: expected_num,
        komga_pages_count: pages_count,
        expected_pages_count: pub_meta.page_count,
        media_status,
        details,
        warnings,
    })
}
