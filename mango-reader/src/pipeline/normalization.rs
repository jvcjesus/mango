//! Safe, native, dry-run-first Komga manga library normalization engine.
//!
//! Evaluates existing pre-Mango manga CBZs in Komga, matches them to MangaBaka metadata
//! via canonical identity resolution rules, verifies archive integrity, and performs
//! atomic, staged replacement of ComicInfo.xml with lossless rollback capability.

use crate::{
    db::{now, Db},
    error::{AppError, Result},
    pipeline::{
        komga::{KomgaClient, KomgaLibrary, KomgaSeries},
        mangabaka::{
            self, ComicInfoParams, MangaBakaClient, MangaBakaEnrichment, MangaBakaOutcome,
        },
        publisher::verify_cbz_integrity,
    },
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

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

pub(super) struct CanonicalTarget<'a> {
    pub batch_id: &'a str,
    pub canonical_path: &'a Path,
    pub series_title: &'a str,
    pub volume: f64,
    pub anilist_id: Option<i64>,
    pub komga_series_id: Option<&'a str>,
    pub komga_book_id: Option<&'a str>,
    pub expected_sha: Option<&'a str>,
}

// Existing item_json remains backward compatible; legacy previews omit mangoCover.
#[derive(Serialize, Deserialize)]
struct FrozenItem {
    #[serde(flatten)]
    item: BookNormalizationItem,
    #[serde(default)]
    mango_cover: Option<super::cover::PreparedCover>,
}
fn cover_diff(cover: &super::cover::PreparedCover) -> BookMetadataDiff {
    BookMetadataDiff {
        field: "Exact work cover".into(),
        current: Some("Missing or changed upstream artwork".into()),
        proposed: Some(format!("Validated artwork SHA256 {}", cover.sha256)),
    }
}
pub(super) fn maintenance_edition(title: &str, xml: Option<&str>) -> Result<Option<String>> {
    let named = crate::acquisition::edition::edition_from_title(title);
    let recorded = xml.map(comic_fields).transpose()?.and_then(|f| {
        f.get("Series")
            .and_then(|s| crate::acquisition::edition::edition_from_title(s))
    });
    if named
        .as_ref()
        .zip(recorded.as_ref())
        .is_some_and(|(a, b)| a != b)
    {
        return Err(AppError::bad_request(
            "Conflicting canonical edition; manual review required",
        ));
    }
    Ok(named.or(recorded))
}

fn retain_edition_namespace(
    enrichment: &mut MangaBakaEnrichment,
    title: &str,
    xml: Option<&str>,
) -> Result<()> {
    if maintenance_edition(title, xml)?.is_some_and(|ed| ed != "standard") {
        let recorded = xml.map(comic_fields).transpose()?.and_then(|f| {
            f.get("Series")
                .filter(|s| crate::acquisition::edition::edition_from_title(s).is_some())
                .cloned()
        });
        enrichment.series_title = Some(recorded.unwrap_or_else(|| title.to_string()));
    }
    Ok(())
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
    pub preview_id: String,
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
    pub refresh_pending: bool,
    pub rollback_available: bool,
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

    let mut found: Option<String> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_file()
            && entry
                .name()
                .rsplit('/')
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case("ComicInfo.xml")
        {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            if let Some(previous) = &found {
                if comic_fields(previous)? != comic_fields(&text)? {
                    return Err(AppError::bad_request(
                        "Conflicting ComicInfo entries; manual review required",
                    ));
                }
            } else {
                found = Some(text);
            }
        }
    }
    Ok(found)
}

// Parse XML before preserving existing fields. Never treat malformed XML as missing.
fn comic_fields(xml: &str) -> Result<BTreeMap<String, String>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| AppError::bad_request("Malformed ComicInfo XML; review manually"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "ComicInfo" || root.tag_name().namespace().is_some() {
        return Err(AppError::bad_request("Expected ComicInfo root"));
    }
    let mut fields = BTreeMap::new();
    for child in root.children().filter(|n| n.is_element()) {
        // Nested fields such as Pages are preserved verbatim. Attributed top-level
        // fields need manual review because the canonical generator does not emit them.
        if child.attributes().len() != 0 || child.tag_name().namespace().is_some() {
            return Err(AppError::bad_request(
                "Attributed ComicInfo field requires manual review",
            ));
        }
        let raw = &xml[child.range()];
        let inner = if raw.ends_with("/>") {
            ""
        } else {
            let start = raw.find('>').unwrap() + 1;
            &raw[start..raw.rfind("</").unwrap()]
        };
        if fields
            .insert(child.tag_name().name().into(), inner.into())
            .is_some()
        {
            return Err(AppError::bad_request(
                "Duplicate ComicInfo field; review manually",
            ));
        }
    }
    Ok(fields)
}

fn proposed_metadata(
    existing: Option<&str>,
    enrichment: &MangaBakaEnrichment,
    volume: f64,
    pages: usize,
) -> Result<(String, Vec<BookMetadataDiff>)> {
    let old = existing.map(comic_fields).transpose()?.unwrap_or_default();
    if let Some(web) = old
        .get("Web")
        .and_then(|w| w.strip_prefix("https://mangabaka.org/series/"))
    {
        if web.trim_end_matches('/').parse::<i64>().ok() != Some(enrichment.series_id) {
            return Err(AppError::bad_request(
                "Conflicting book MangaBaka identity; review manually",
            ));
        }
    }
    let params = ComicInfoParams {
        series: enrichment.series_title.as_deref().unwrap_or(""),
        volume: Some(volume),
        number: None,
        is_volume: true,
        title: enrichment.work_title.as_deref(),
        scanlator: None,
        notes: None,
    };
    let generated =
        mangabaka::generate_comic_info_xml(&params, Some(enrichment), None, pages as i64);
    let mut proposed = comic_fields(&generated)?;
    // Retain useful metadata when the authoritative provider has no justified replacement,
    // along with all unknown flat fields. Keep the established volume Number omission.
    for (key, value) in &old {
        if key != "Number" {
            proposed.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if enrichment
        .work_title
        .as_deref()
        .is_none_or(|v| v.trim().is_empty())
    {
        if let Some(title) = old.get("Title") {
            proposed.insert("Title".into(), title.clone());
        }
    }
    if enrichment
        .language
        .as_deref()
        .is_none_or(|v| v.trim().is_empty())
    {
        if let Some(language) = old.get("LanguageISO") {
            proposed.insert("LanguageISO".into(), language.clone());
        }
    }
    for key in [
        "Notes",
        "Translator",
        "ScanInformation",
        "Colorist",
        "Letterer",
        "Editor",
        "Imprint",
    ] {
        if let Some(value) = old.get(key) {
            proposed.insert(key.into(), value.clone());
        }
    }
    let keys: std::collections::BTreeSet<_> = old.keys().chain(proposed.keys()).collect();
    let diffs = keys
        .into_iter()
        .filter_map(|key| {
            let (current, next) = (old.get(key), proposed.get(key));
            (current != next).then(|| BookMetadataDiff {
                field: key.clone(),
                current: current.cloned(),
                proposed: next.cloned(),
            })
        })
        .collect();
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo>\n");
    for (key, value) in proposed {
        xml.push_str(&format!("  <{key}>{value}</{key}>\n"));
    }
    xml.push_str("</ComicInfo>\n");
    comic_fields(&xml)?;
    Ok((xml, diffs))
}

// Do not reinterpret a Komga display ordinal as a volume number. Maintenance
// only handles unambiguous volumes; existing chapter releases remain untouched.
fn maintenance_volume(name: &str, existing: Option<&str>) -> Option<f64> {
    if crate::release::has_explicit_chapter_signal(name) {
        return None;
    }
    let named = crate::release::parse_volume(name);
    let recorded = existing
        .and_then(|xml| comic_fields(xml).ok())
        .and_then(|fields| fields.get("Volume").and_then(|v| v.parse::<f64>().ok()));
    if named.zip(recorded).is_some_and(|(a, b)| a != b) {
        return None;
    }
    named.or(recorded).filter(|v| v.is_finite() && *v >= 0.0)
}

fn unique_id(prefix: &str) -> String {
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random).expect("OS random source");
    format!("{prefix}-{}", hex::encode(random))
}

// Komga and Mango may mount the same library under different absolute roots.
// Translate only a selected-library-relative path; never search or guess a basename.
fn book_path(komga_root: &Path, local_root: &Path, url: Option<&str>) -> Result<PathBuf> {
    use std::path::Component;
    let raw = url.ok_or_else(|| AppError::bad_request("Komga did not provide a filesystem URL"))?;
    // URL parsing normalizes dot segments. Reject them before parsing, including
    // encoded dots; encoded separators are checked again after to_file_path().
    if raw.contains('\\')
        || raw
            .split('/')
            .any(|part| part.to_ascii_lowercase().replace("%2e", ".") == "..")
    {
        return Err(AppError::bad_request("Traversal in Komga book path"));
    }
    let reported = if raw.starts_with("file:") {
        reqwest::Url::parse(raw)
            .map_err(|_| AppError::bad_request("Invalid Komga file URL"))?
            .to_file_path()
            .map_err(|_| AppError::bad_request("Komga URL must identify a local file"))?
    } else {
        PathBuf::from(raw)
    };
    if [reported.as_path(), komga_root, local_root]
        .iter()
        .any(|path| {
            !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir))
        })
    {
        return Err(AppError::bad_request(
            "Book and library paths must be absolute without traversal",
        ));
    }
    let root = fs::canonicalize(local_root)?;
    let candidate = if let Ok(relative) = reported.strip_prefix(komga_root) {
        root.join(relative)
    } else if reported.starts_with(local_root) || reported.starts_with(&root) {
        // Komga may already report the Mango-visible namespace.
        reported
    } else {
        return Err(AppError::bad_request(
            "Book is outside the selected library roots",
        ));
    };
    let path = fs::canonicalize(candidate)?;
    if !path.starts_with(&root)
        || path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("cbz"))
    {
        return Err(AppError::bad_request(
            "Book must be a CBZ inside the configured canonical library root",
        ));
    }
    Ok(path)
}

// Read every entry to EOF (CRC verification), and compare all non-metadata content.
fn archive_manifest(path: &Path) -> Result<Vec<(String, String)>> {
    verify_cbz_integrity(path)?;
    let mut zip = zip::ZipArchive::new(File::open(path)?)?;
    let mut result = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        if !name
            .rsplit('/')
            .next()
            .unwrap_or("")
            .eq_ignore_ascii_case("ComicInfo.xml")
        {
            result.push((name, hex::encode(hash.finalize())));
        }
    }
    Ok(result)
}
fn image_count(path: &Path) -> Result<usize> {
    let zip = zip::ZipArchive::new(File::open(path)?)?;
    Ok(zip
        .file_names()
        .filter(|n| {
            crate::local::is_image(n)
                && !n.starts_with("__MACOSX")
                && !n.rsplit('/').next().unwrap_or("").starts_with('.')
        })
        .count())
}
fn comic_info_count(path: &Path) -> Result<usize> {
    let zip = zip::ZipArchive::new(File::open(path)?)?;
    Ok(zip
        .file_names()
        .filter(|n| {
            n.rsplit('/')
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case("ComicInfo.xml")
        })
        .count())
}
fn canonical_comic_info_layout(path: &Path) -> Result<bool> {
    let zip = zip::ZipArchive::new(File::open(path)?)?;
    let names: Vec<_> = zip
        .file_names()
        .filter(|n| {
            n.rsplit('/')
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case("ComicInfo.xml")
        })
        .collect();
    Ok(names == ["ComicInfo.xml"])
}
fn copy_new_synced(source: &Path, dest: &Path) -> Result<()> {
    let mut input = File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_permissions(input.metadata()?.permissions())?;
    output.sync_all()?;
    sync_parent(dest)
}
fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn publish_normalized(
    path: &Path,
    stage: &Path,
    expected_sha: &str,
    expected_stage_sha: &str,
    manifest: &[(String, String)],
) -> Result<()> {
    if calculate_file_sha256(stage)? != expected_stage_sha
        || archive_manifest(stage)? != manifest
        || !canonical_comic_info_layout(stage)?
    {
        return Err(AppError::bad_request(
            "Invalid staged archive; canonical book preserved",
        ));
    }
    let xml = read_comic_info_xml_from_cbz(stage)?
        .ok_or_else(|| AppError::bad_request("Staged metadata missing"))?;
    comic_fields(&xml)?;
    if calculate_file_sha256(path)? != expected_sha {
        return Err(AppError::new(
            409,
            "stale_preview",
            "Book changed before replacement",
        ));
    }
    File::open(stage)?.sync_all()?;
    fs::rename(stage, path)?;
    sync_parent(path)
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct FileLock<'a>(&'a Mutex<HashSet<PathBuf>>, PathBuf);
impl<'a> FileLock<'a> {
    fn acquire(locks: &'a Mutex<HashSet<PathBuf>>, path: &Path) -> Result<Self> {
        if !locks.lock().unwrap().insert(path.into()) {
            return Err(AppError::new(
                409,
                "file_locked",
                "File is already being modified",
            ));
        }
        Ok(Self(locks, path.into()))
    }
}
impl Drop for FileLock<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}
// SQLite normally uses NORMAL in this app. The pre-replacement journal and
// rollback intent must reach stable storage before any canonical rename.
fn durable_execute(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<()> {
    let previous: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", 2)?;
    let result = conn.execute(sql, params);
    conn.pragma_update(None, "synchronous", previous)?;
    result?;
    Ok(())
}

fn history_item(
    id: String,
    batch: &str,
    item: &BookNormalizationItem,
    old: &str,
    new: &str,
    status: &str,
) -> NormalizationHistoryRecord {
    NormalizationHistoryRecord {
        id,
        batch_id: batch.into(),
        canonical_path: item.canonical_path.clone(),
        series_title: item.series_title.clone(),
        volume: item.volume.unwrap_or(0.0),
        komga_series_id: (!item.komga_series_id.is_empty()).then(|| item.komga_series_id.clone()),
        komga_book_id: (!item.komga_book_id.is_empty()).then(|| item.komga_book_id.clone()),
        mangabaka_id: item.mangabaka_id,
        mangabaka_release_id: item.mangabaka_release_id.clone(),
        anilist_id: item.anilist_id,
        original_sha256: Some(old.into()),
        normalized_sha256: Some(new.into()),
        status: status.into(),
        error_message: None,
        normalized_at: now(),
        reverted_at: None,
        refresh_pending: false,
        rollback_available: false,
    }
}

// Spaces create a pipeline per account. Locks must coordinate every maintenance
// service in this process, including separate administrator accounts.
static MAINTENANCE_FILE_LOCKS: OnceLock<Arc<Mutex<HashSet<PathBuf>>>> = OnceLock::new();

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
            file_locks: MAINTENANCE_FILE_LOCKS
                .get_or_init(|| Arc::new(Mutex::new(HashSet::new())))
                .clone(),
        }
    }

    /// Determines whether a Komga library is eligible for MangaBaka normalization.
    ///
    /// Rules:
    /// - English Manga: Eligible
    /// - Chinese Manga: excluded until a separate policy is validated
    /// - Comics/unknown providers: skipped as provider not applicable.
    pub fn is_library_eligible_for_mangabaka(&self, library: &KomgaLibrary) -> bool {
        let name_lower = library.name.to_lowercase();
        let root_lower = library.root.to_lowercase();

        !library.unavailable
            && !name_lower.contains("comic")
            && !root_lower.contains("comic")
            && !name_lower.contains("chinese")
            && !root_lower.contains("chinese")
            && !name_lower.contains("中文")
            && (name_lower.contains("manga") || root_lower.contains("manga"))
    }

    pub fn applicability_reason(&self, library: &KomgaLibrary) -> String {
        if library.name.to_lowercase().contains("chinese")
            || library.root.to_lowercase().contains("chinese")
            || library.name.contains("中文")
        {
            "Skipped — Chinese normalization policy has not been validated".into()
        } else {
            "Skipped — provider not applicable or library unavailable; English Manga only".into()
        }
    }

    // Reuse the English publication destination. The override/Komga fallback
    // preserves callers/installations without a configured pipeline destination.
    async fn canonical_library_root(
        &self,
        library: &KomgaLibrary,
        fallback: Option<&Path>,
    ) -> Result<PathBuf> {
        let configured = self
            .db
            .call(|conn| Ok(crate::settings::get::<String>(conn, "pipeline.library_dir")))
            .await?;
        Ok(configured
            .filter(|root| !root.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                fallback
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(&library.root))
            }))
    }

    async fn library_policy_reason(&self, library: &KomgaLibrary) -> Result<Option<String>> {
        let (english, chinese, chinese_root): (Option<String>, Option<String>, Option<String>) =
            self.db
                .call(|conn| {
                    Ok((
                        crate::settings::get(conn, "pipeline.komga_library_id"),
                        crate::settings::get(conn, "pipeline.komga_chinese_library_id"),
                        crate::settings::get(conn, "pipeline.chinese_library_dir"),
                    ))
                })
                .await?;
        if chinese.as_deref() == Some(&library.id) || chinese_root.as_deref() == Some(&library.root)
        {
            return Ok(Some(
                "Skipped — configured Chinese library; normalization policy has not been validated"
                    .into(),
            ));
        }
        if !self.is_library_eligible_for_mangabaka(library) {
            return Ok(Some(self.applicability_reason(library)));
        }
        if english
            .as_ref()
            .is_some_and(|id| !id.is_empty() && id != &library.id)
        {
            return Ok(Some(
                "Skipped — library is not the configured English Manga target".into(),
            ));
        }
        Ok(None)
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
        let series_id = komga_series_id.to_string();
        let ambiguous = self.db.call(move |conn| Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracker_mappings WHERE komga_series_id=?1 AND confidence='ambiguous'", [series_id], |r|r.get::<_,i64>(0))?)).await;
        match ambiguous {
            Ok(n) if n > 0 => {
                return (
                    None,
                    None,
                    "Ambiguous persisted tracker identity; manual review required".into(),
                )
            }
            Err(_) => return (None, None, "Identity database error".into()),
            _ => {}
        }
        // 1. Check tracker_mappings for MangaBaka
        let k_id = komga_series_id.to_string();
        let mb_mapped: Result<Option<String>> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT tracker_id FROM tracker_mappings
                         WHERE komga_series_id = ?1 AND tracker = 'mangabaka' AND confidence = 'resolved'",
                        [&k_id],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            ;
        let mb_mapped = match mb_mapped {
            Ok(value) => value,
            Err(_) => return (None, None, "Identity database error".into()),
        };

        if let Some(id_str) = mb_mapped {
            if let Ok(id) = id_str.parse::<i64>() {
                if id <= 0 {
                    return (None, None, "Invalid persisted MangaBaka identity".into());
                }
                return (
                    Some(id),
                    None,
                    "Resolved from tracker mappings (MangaBaka)".to_string(),
                );
            }
        }

        // 2. Check tracker_mappings for AniList
        let k_id = komga_series_id.to_string();
        let al_mapped: Result<Option<String>> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT tracker_id FROM tracker_mappings
                         WHERE komga_series_id = ?1 AND tracker = 'anilist' AND confidence = 'resolved'",
                        [&k_id],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            ;
        let al_mapped = match al_mapped {
            Ok(value) => value,
            Err(_) => return (None, None, "Identity database error".into()),
        };

        if let Some(id_str) = al_mapped {
            if let Ok(al_id) = id_str.parse::<i64>() {
                match mb_client.resolve_from_anilist(al_id).await {
                    Ok(Some(xref)) => {
                        return (
                            Some(xref.mangabaka_id),
                            Some(al_id),
                            format!("Resolved from AniList ID {al_id} -> MangaBaka xref"),
                        )
                    }
                    Ok(None) => {
                        return (
                            None,
                            Some(al_id),
                            "AniList cross-reference not found; manual mapping required".into(),
                        )
                    }
                    Err(e) if e.status == 409 => {
                        return (
                            None,
                            Some(al_id),
                            "Ambiguous AniList cross-reference".into(),
                        )
                    }
                    Err(_) => return (None, Some(al_id), "MangaBaka cross-reference error".into()),
                }
            }
        }

        // Existing authoritative Web identity; malformed or unsupported XML fails closed.
        if let Some(path) = sample_cbz_path {
            match read_comic_info_xml_from_cbz(path)
                .and_then(|xml| xml.map(|x| comic_fields(&x)).transpose())
            {
                Ok(Some(fields)) => {
                    if let Some(web) = fields.get("Web") {
                        if let Some(id) = web
                            .strip_prefix("https://mangabaka.org/series/")
                            .and_then(|id| id.trim_end_matches('/').parse::<i64>().ok())
                            .filter(|id| *id > 0)
                        {
                            return (Some(id), None, "Resolved from ComicInfo Web".into());
                        }
                    }
                }
                Err(_) => {
                    return (
                        None,
                        None,
                        "Metadata XML error: review ComicInfo before normalization".into(),
                    )
                }
                _ => {}
            }
        }

        let title = series_title.to_string();
        let conflicts=self.db.call(move |conn|Ok(conn.query_row(
            "SELECT COUNT(DISTINCT mangabaka_id) FROM acquisition_history WHERE series=?1 AND mangabaka_id>0", [title], |r|r.get::<_,i64>(0))?)).await;
        match conflicts {
            Ok(n) if n > 1 => return (None, None, "Ambiguous acquisition history identity".into()),
            Err(_) => return (None, None, "Identity database error".into()),
            _ => {}
        }
        // 4. Check acquisition_history provenance
        let s_title = series_title.to_string();
        let norm_title = mangabaka::normalize_title(&s_title);
        let hist_mb_id: Result<Option<i64>> = self
            .db
            .call(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT CASE WHEN COUNT(DISTINCT mangabaka_id) = 1 THEN MAX(mangabaka_id) ELSE NULL END FROM acquisition_history
                         WHERE (series = ?1 OR LOWER(series) = ?2) AND mangabaka_id IS NOT NULL AND mangabaka_id > 0
                         ",
                        params![s_title, norm_title],
                        |r| r.get(0),
                    )
                    .optional()?)
            })
            .await
            ;
        let hist_mb_id = match hist_mb_id {
            Ok(value) => value,
            Err(_) => return (None, None, "Identity database error".into()),
        };

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
            _ => (
                None,
                None,
                "MangaBaka provider error; retry or check configuration".to_string(),
            ),
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
        match proposed_metadata(existing_xml, enrichment, volume, actual_page_count) {
            Ok((_, diffs)) => {
                let status = if diffs.is_empty() {
                    NormalizationStatus::AlreadyNormalized
                } else {
                    NormalizationStatus::NeedsUpdate
                };
                (
                    status,
                    format!("{} proposed metadata field changes", diffs.len()),
                    diffs,
                )
            }
            Err(_) => (
                NormalizationStatus::Error,
                "Unsupported or invalid ComicInfo XML; manual review required".into(),
                Vec::new(),
            ),
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

        self.db
            .call(|conn| {
                conn.execute(
                    "DELETE FROM normalization_previews WHERE created_at<?1",
                    [now() - 86400],
                )?;
                Ok(())
            })
            .await?;
        let policy_reason = self.library_policy_reason(&lib).await?;
        let is_manga = policy_reason.is_none();

        let all_series = komga_client.list_series(Some(library_id)).await?;
        let target_series: Vec<KomgaSeries> = if let Some(s_id) = series_id_filter {
            all_series.into_iter().filter(|s| s.id == s_id).collect()
        } else {
            all_series
        };

        let mut summary = LibraryAnalysisSummary {
            preview_id: unique_id("preview"),
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
                let books = komga_client.get_books_in_series(&s.id).await?;
                summary.total_books += books.len();
                summary.skipped_count += books.len();

                summary.series.push(SeriesNormalizationItem {
                    komga_series_id: s.id.clone(),
                    library_id: lib.id.clone(),
                    series_title: s.name.clone(),
                    status: NormalizationStatus::SkippedProviderNotApplicable,
                    reason: policy_reason.clone().unwrap_or_default(),
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
                            reason: policy_reason.clone().unwrap_or_default(),
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
        let base_dir = self
            .canonical_library_root(&lib, library_dir_override)
            .await?;

        for s in target_series {
            let books = komga_client.get_books_in_series(&s.id).await?;
            summary.total_books += books.len();

            let sample_path = books
                .first()
                .and_then(|b| book_path(Path::new(&lib.root), &base_dir, b.url.as_deref()).ok());

            // Identity resolution
            let (mangabaka_id, anilist_id, id_reason) = self
                .resolve_series_identity(mb_client, &s.id, &s.name, sample_path.as_deref())
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
                } else if id_reason.to_lowercase().contains("error") {
                    NormalizationStatus::Error
                } else {
                    NormalizationStatus::Unmatched
                };
                series_item.status = status;
                for b in books {
                    if status == NormalizationStatus::Ambiguous {
                        summary.ambiguous_count += 1;
                    } else if status == NormalizationStatus::Error {
                        summary.error_count += 1;
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
                let resolved_path = book_path(Path::new(&lib.root), &base_dir, b.url.as_deref());
                let path_error = resolved_path.as_ref().err().map(|error| error.to_string());
                let canonical_path = resolved_path.unwrap_or_default();
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
                        reason: path_error.unwrap_or_else(|| {
                            "File not found on local canonical filesystem".to_string()
                        }),
                        mangabaka_id: Some(mb_id),
                        mangabaka_release_id: None,
                        anilist_id,
                        diffs: Vec::new(),
                        current_sha256: None,
                    });
                    continue;
                }

                if let Err(e) = archive_manifest(&canonical_path) {
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

                let current_sha = calculate_file_sha256(&canonical_path).ok();
                let existing_xml = match read_comic_info_xml_from_cbz(&canonical_path) {
                    Ok(xml) => xml,
                    Err(_) => {
                        summary.error_count += 1;
                        series_item.books.push(BookNormalizationItem {
                            komga_book_id:b.id,komga_series_id:s.id.clone(),series_title:s.name.clone(),book_name:b.name,
                            canonical_path:path_str,volume:None,status:NormalizationStatus::Error,
                            reason:"Unreadable or conflicting ComicInfo entries; manual review required".into(),
                            mangabaka_id:Some(mb_id),mangabaka_release_id:None,anilist_id,diffs:Vec::new(),current_sha256:current_sha,
                        });
                        continue;
                    }
                };
                let volume_name = b
                    .metadata
                    .as_ref()
                    .and_then(|m| m.title.as_deref())
                    .filter(|t| crate::release::has_explicit_chapter_signal(t))
                    .unwrap_or(&b.name);
                let Some(vol) = maintenance_volume(volume_name, existing_xml.as_deref()) else {
                    summary.unmatched_count += 1;
                    series_item.books.push(BookNormalizationItem {
                        komga_book_id:b.id,komga_series_id:s.id.clone(),series_title:s.name.clone(),book_name:b.name,
                        canonical_path:path_str,volume:None,status:NormalizationStatus::Unmatched,
                        reason:"Uncertain/conflicting volume or chapter release; left unchanged for manual review".into(),
                        mangabaka_id:Some(mb_id),mangabaka_release_id:None,anilist_id,diffs:Vec::new(),current_sha256:current_sha,
                    });
                    continue;
                };
                let page_count = image_count(&canonical_path)?;

                // Resolve MangaBaka metadata for this specific volume
                match mb_client
                    .resolve_edition_volume_metadata(
                        &s.name,
                        anilist_id,
                        Some(mb_id),
                        vol,
                        maintenance_edition(&s.name, existing_xml.as_deref())?.as_deref(),
                    )
                    .await
                {
                    Ok(MangaBakaOutcome::Success(mut enrichment)) => {
                        retain_edition_namespace(
                            &mut enrichment,
                            &s.name,
                            existing_xml.as_deref(),
                        )?;
                        let (mut b_status, mut b_reason, mut diffs) = Self::evaluate_metadata_diff(
                            existing_xml.as_deref(),
                            &enrichment,
                            vol,
                            page_count,
                        );

                        let mut frozen_cover = None;
                        if b_status != NormalizationStatus::Error {
                            match super::cover::preview(
                                &self.http,
                                &canonical_path,
                                enrichment.work_cover_url.as_deref(),
                            )
                            .await
                            {
                                Ok(Some(cover)) => {
                                    diffs.push(cover_diff(&cover));
                                    frozen_cover = Some(cover);
                                    b_status = NormalizationStatus::NeedsUpdate;
                                    b_reason =
                                        "Reviewed metadata and exact-work cover enrichment".into();
                                }
                                Ok(None) => {}
                                Err(error) if error.code == "cover_conflict" => {
                                    b_status = NormalizationStatus::Error;
                                    b_reason = format!(
                                        "Cover conflict; canonical book unchanged: {error}"
                                    );
                                }
                                Err(error) => {
                                    b_reason =
                                        format!("{b_reason}; optional cover skipped: {error}");
                                    tracing::warn!(reason=%error, "Optional maintenance cover unavailable");
                                }
                            }
                        }
                        if b_status == NormalizationStatus::AlreadyNormalized
                            && !canonical_comic_info_layout(&canonical_path)?
                        {
                            b_status = NormalizationStatus::NeedsUpdate;
                            b_reason =
                                "Canonicalize metadata placement: exactly one root ComicInfo.xml"
                                    .into();
                        }
                        if b_status == NormalizationStatus::AlreadyNormalized {
                            summary.already_normalized_count += 1;
                            series_item.already_normalized_count += 1;
                        } else if b_status == NormalizationStatus::NeedsUpdate {
                            summary.needs_update_count += 1;
                            series_item.needs_update_count += 1;
                        }
                        if b_status == NormalizationStatus::Error {
                            summary.error_count += 1;
                        }
                        summary.matched_count += 1;

                        let item = BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name,
                            canonical_path: path_str,
                            volume: Some(vol),
                            status: b_status,
                            reason: b_reason,
                            mangabaka_id: Some(mb_id),
                            mangabaka_release_id: enrichment.work_id.clone(),
                            anilist_id,
                            diffs,
                            current_sha256: current_sha,
                        };
                        if item.status == NormalizationStatus::NeedsUpdate {
                            let (xml, _) = proposed_metadata(
                                existing_xml.as_deref(),
                                &enrichment,
                                vol,
                                page_count,
                            )?;
                            if Some(calculate_file_sha256(&canonical_path)?) != item.current_sha256
                            {
                                return Err(AppError::bad_request(
                                    "Book changed during preview; repeat analysis",
                                ));
                            }
                            let json = serde_json::to_string(&FrozenItem {
                                item: item.clone(),
                                mango_cover: frozen_cover,
                            })?;
                            let preview = summary.preview_id.clone();
                            let book = item.komga_book_id.clone();
                            let library = lib.clone();
                            self.db.call(move |conn| {
                                conn.execute("INSERT INTO normalization_previews (preview_id,book_id,library_id,library_root,item_json,proposed_xml,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![preview,book,library.id,library.root,json,xml,now()])?;Ok(())
                            }).await?;
                        }
                        series_item.books.push(item);
                    }
                    Ok(other) => {
                        let status = match &other {
                            MangaBakaOutcome::AmbiguousSeries(_)
                            | MangaBakaOutcome::AmbiguousCollection(_) => {
                                summary.ambiguous_count += 1;
                                NormalizationStatus::Ambiguous
                            }
                            MangaBakaOutcome::AuthError(_)
                            | MangaBakaOutcome::NetworkError(_)
                            | MangaBakaOutcome::NotConfigured => {
                                summary.error_count += 1;
                                NormalizationStatus::Error
                            }
                            _ => {
                                summary.unmatched_count += 1;
                                NormalizationStatus::Unmatched
                            }
                        };
                        series_item.books.push(BookNormalizationItem {
                            komga_book_id: b.id,
                            komga_series_id: s.id.clone(),
                            series_title: s.name.clone(),
                            book_name: b.name,
                            canonical_path: path_str,
                            volume: Some(vol),
                            status,
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

        // Proposals are frozen per book to bound artwork memory. Execution rechecks
        // each canonical hash and uses the exact approved bytes without another fetch.
        Ok(summary)
    }

    /// Executes approved normalization on a candidate set of books.
    ///
    /// Properties:
    /// - Operates on verified copies staged in temporary files before canonical replacement.
    /// - Idempotent: identical hash produces ALREADY_NORMALIZED with zero file alteration.
    /// - Thread-safe: per-file lock prevents concurrent operations on the same CBZ.
    /// - Rollback uses a verified complete original archive and a pre-replacement journal.
    /// - Targeted: triggers Komga metadata refresh for modified books.
    #[cfg(test)]
    async fn normalize_book(
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
        let path = fs::canonicalize(canonical_path)?;
        let expected_sha = calculate_file_sha256(&path)?;
        let existing = read_comic_info_xml_from_cbz(&path)?;
        let outcome = mb_client
            .resolve_edition_volume_metadata(
                series_title,
                anilist_id,
                Some(mangabaka_id),
                volume,
                maintenance_edition(series_title, existing.as_deref())?.as_deref(),
            )
            .await?;
        let MangaBakaOutcome::Success(enrichment) = outcome else {
            return Err(AppError::bad_request("Authoritative metadata unavailable"));
        };
        self.normalize_canonical(
            CanonicalTarget {
                batch_id,
                canonical_path,
                series_title,
                volume,
                anilist_id,
                komga_series_id,
                komga_book_id,
                expected_sha: Some(&expected_sha),
            },
            enrichment,
            komga_client,
        )
        .await
    }

    pub(super) async fn normalize_canonical(
        &self,
        target: CanonicalTarget<'_>,
        mut enrichment: MangaBakaEnrichment,
        komga_client: Option<&KomgaClient>,
    ) -> Result<NormalizationHistoryRecord> {
        let CanonicalTarget {
            batch_id,
            canonical_path,
            series_title,
            volume,
            anilist_id,
            komga_series_id,
            komga_book_id,
            expected_sha,
        } = target;
        let path = fs::canonicalize(canonical_path)?;
        let current_sha = calculate_file_sha256(&path)?;
        if expected_sha.is_some_and(|sha| sha != current_sha) {
            return Err(AppError::new(
                409,
                "stale_preview",
                "Canonical book changed during repair resolution",
            ));
        }
        let existing = read_comic_info_xml_from_cbz(&path)?;
        retain_edition_namespace(&mut enrichment, series_title, existing.as_deref())?;
        let (xml, _) = proposed_metadata(
            existing.as_deref(),
            &enrichment,
            volume,
            image_count(&path)?,
        )?;
        let item = BookNormalizationItem {
            komga_book_id: komga_book_id.unwrap_or_default().into(),
            komga_series_id: komga_series_id.unwrap_or_default().into(),
            series_title: series_title.into(),
            book_name: String::new(),
            canonical_path: path.to_string_lossy().into(),
            volume: Some(volume),
            status: NormalizationStatus::NeedsUpdate,
            reason: String::new(),
            mangabaka_id: Some(enrichment.series_id),
            mangabaka_release_id: enrichment.work_id,
            anilist_id,
            diffs: Vec::new(),
            current_sha256: Some(current_sha),
        };
        let cover =
            match super::cover::preview(&self.http, &path, enrichment.work_cover_url.as_deref())
                .await
            {
                Ok(cover) => cover,
                Err(error) if error.code == "cover_conflict" => return Err(error),
                Err(error) => {
                    tracing::warn!(reason=%error, "Optional repair cover skipped");
                    None
                }
            };
        self.apply_preview_with_cover(batch_id, &item, &xml, komga_client, cover.as_ref())
            .await
    }

    #[cfg(test)]
    async fn apply_preview(
        &self,
        batch: &str,
        item: &BookNormalizationItem,
        xml: &str,
        komga: Option<&KomgaClient>,
    ) -> Result<NormalizationHistoryRecord> {
        self.apply_preview_with_cover(batch, item, xml, komga, None)
            .await
    }

    async fn apply_preview_with_cover(
        &self,
        batch: &str,
        item: &BookNormalizationItem,
        xml: &str,
        komga: Option<&KomgaClient>,
        cover: Option<&super::cover::PreparedCover>,
    ) -> Result<NormalizationHistoryRecord> {
        comic_fields(xml)?;
        let path = fs::canonicalize(&item.canonical_path)?;
        let _lock = FileLock::acquire(&self.file_locks, &path)?;
        let sha = calculate_file_sha256(&path)?;
        if item.current_sha256.as_deref() != Some(&sha) {
            return Err(AppError::new(
                409,
                "stale_preview",
                "Canonical book changed; run a new dry run",
            ));
        }
        let manifest = archive_manifest(&path)?;
        let original_xml = read_comic_info_xml_from_cbz(&path)?;
        if cover.is_none()
            && original_xml
                .as_deref()
                .is_some_and(|old| comic_fields(old).ok() == comic_fields(xml).ok())
            && canonical_comic_info_layout(&path)?
        {
            // No rewrite or new rollback record for an unchanged book.
            return Ok(history_item(
                unique_id("noop"),
                batch,
                item,
                &sha,
                &sha,
                "already_normalized",
            ));
        }
        let id = unique_id("norm");
        let backup = path.with_file_name(format!(
            ".{}.{}.original.backup",
            path.file_name().unwrap().to_string_lossy(),
            id
        ));
        copy_new_synced(&path, &backup)?;
        if calculate_file_sha256(&backup)? != sha {
            return Err(AppError::internal("Backup verification failed"));
        }
        let stage = path.with_file_name(format!(
            ".{}.{}.stage.tmp",
            path.file_name().unwrap().to_string_lossy(),
            id
        ));
        let _cleanup = Cleanup(stage.clone());
        copy_new_synced(&path, &stage)?;
        mangabaka::write_prepared_comic_info_to_cbz(&stage, xml)?;
        let mut final_manifest = archive_manifest(&stage)?;
        let mut artwork_applied = false;
        if let Some(cover) = cover {
            // The shared cover writer is atomic: failure leaves metadata-only staging intact.
            match super::cover::apply_prepared(&stage, cover) {
                Ok((old, new)) => {
                    let artwork_manifest = archive_manifest(&stage)?;
                    let retained = |entries: &[(String, String)], removed: &[String]| {
                        entries
                            .iter()
                            .filter(|(n, _)| !removed.contains(n))
                            .cloned()
                            .collect::<Vec<_>>()
                    };
                    if retained(&manifest, &old) != retained(&artwork_manifest, &new) {
                        return Err(AppError::internal("Cover rewrite changed source entries"));
                    }
                    final_manifest = artwork_manifest;
                    artwork_applied = true;
                }
                Err(error) => {
                    tracing::warn!(reason=%error, "Optional frozen cover skipped; metadata-only staging retained")
                }
            }
        }
        if (!artwork_applied && final_manifest != manifest) || comic_info_count(&stage)? != 1 {
            return Err(AppError::internal(
                "Staged archive changed non-metadata contents",
            ));
        }
        if !artwork_applied
            && original_xml
                .as_deref()
                .is_some_and(|old| comic_fields(old).ok() == comic_fields(xml).ok())
            && canonical_comic_info_layout(&path)?
        {
            fs::remove_file(&backup)?;
            return Ok(history_item(
                unique_id("noop"),
                batch,
                item,
                &sha,
                &sha,
                "already_normalized",
            ));
        }
        fs::set_permissions(&stage, fs::metadata(&path)?.permissions())?;
        File::open(&stage)?.sync_all()?;
        let new_sha = calculate_file_sha256(&stage)?;
        let mut record = history_item(id, batch, item, &sha, &new_sha, "prepared");
        let r = record.clone();
        let backup_str = backup.to_string_lossy().to_string();
        let stage_str = stage.to_string_lossy().to_string();
        self.db.call(move |conn| {
            durable_execute(conn,"INSERT INTO normalization_history (id,batch_id,canonical_path,series_title,volume,komga_series_id,komga_book_id,mangabaka_id,mangabaka_release_id,anilist_id,original_sha256,normalized_sha256,original_comic_info_xml,status,normalized_at,backup_path,staging_path,refresh_pending) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'prepared',?14,?15,?16,1)",
                params![r.id,r.batch_id,r.canonical_path,r.series_title,r.volume,r.komga_series_id,r.komga_book_id,r.mangabaka_id,r.mangabaka_release_id,r.anilist_id,r.original_sha256,r.normalized_sha256,original_xml,r.normalized_at,backup_str,stage_str])?;
            Ok(())
        }).await?;
        // Revalidate staged contents after journal persistence, immediately before replacement.
        publish_normalized(&path, &stage, &sha, &new_sha, &final_manifest)?;
        self.set_status(&record.id, "normalized").await?;
        record.status = "normalized".into();
        record.refresh_pending = true;
        if let Some(kc) = komga {
            self.refresh_record(&record, kc).await?;
        }
        Ok(self.get_history_record(&record.id).await?.unwrap())
    }

    pub(super) async fn acknowledge_library_scan(&self, id: String) -> Result<()> {
        self.db.call(move |c| { c.execute("UPDATE normalization_history SET refresh_pending=0,error_message=NULL WHERE id=?1",[id])?; Ok(()) }).await
    }

    async fn set_status(&self, id: &str, status: &str) -> Result<()> {
        let (id, status) = (id.to_string(), status.to_string());
        self.db.call(move |conn| {
            durable_execute(conn,"UPDATE normalization_history SET status=?2, reverted_at=CASE WHEN ?2='reverted' THEN ?3 ELSE reverted_at END WHERE id=?1", params![id,status,now()])?;
            Ok(())
        }).await
    }

    /// Recover the journal's replacement/status crash window by comparing durable hashes.
    async fn recover_record(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        let locks = self.file_locks.clone();
        self.db.call(move |conn| {
            let state: Option<(String,String,Option<String>,Option<String>)> = conn.query_row(
                "SELECT status,canonical_path,original_sha256,normalized_sha256 FROM normalization_history WHERE id=?1", [&id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((status,path,original,normalized)) = state {
                if status == "prepared" || status == "reverting" {
                    let Ok(_lock) = FileLock::acquire(&locks, Path::new(&path)) else {return Ok(());};
                    let sha = calculate_file_sha256(Path::new(&path)).ok();
                    let next = if status == "reverting" && sha.is_some() && sha==original { "reverted" }
                        else if sha.is_some() && sha==normalized { "normalized" }
                        else if sha.is_some() && sha==original { "failed" }
                        else { "conflict" };
                    conn.execute("UPDATE normalization_history SET status=?2, reverted_at=CASE WHEN ?2='reverted' THEN ?3 ELSE reverted_at END,
                        refresh_pending=CASE WHEN ?2 IN ('failed','conflict') THEN 0 WHEN ?2='reverted' THEN 1 ELSE refresh_pending END,
                        error_message=CASE WHEN ?2='failed' THEN 'Interrupted before canonical replacement; original preserved'
                            WHEN ?2='conflict' THEN 'Canonical file differs from journal or is unreadable; manual review required' ELSE error_message END WHERE id=?1",
                        params![id,next,now()])?;
                }
            }
            Ok(())
        }).await
    }

    pub async fn rollback_normalization(
        &self,
        history_id: &str,
        komga: Option<&KomgaClient>,
    ) -> Result<NormalizationHistoryRecord> {
        let record = self
            .get_history_record(history_id)
            .await?
            .ok_or_else(|| AppError::not_found("History record not found"))?;
        let path = fs::canonicalize(&record.canonical_path)?;
        let _lock = FileLock::acquire(&self.file_locks, &path)?;
        if record.status == "reverted" {
            return Ok(record);
        }
        if record.status != "normalized" {
            return Err(AppError::bad_request(
                "Only a normalized record can be reverted",
            ));
        }
        if Some(calculate_file_sha256(&path)?) != record.normalized_sha256 {
            return Err(AppError::new(
                409,
                "rollback_conflict",
                "Book changed after normalization; refusing to overwrite",
            ));
        }
        let id = history_id.to_string();
        let backup: Option<String> = self
            .db
            .call(move |conn| {
                Ok(conn.query_row(
                    "SELECT backup_path FROM normalization_history WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )?)
            })
            .await?;
        let backup = backup.ok_or_else(|| {
            AppError::bad_request(
                "Legacy record has no full-file backup; automatic rollback unavailable",
            )
        })?;
        if Some(calculate_file_sha256(Path::new(&backup))?) != record.original_sha256 {
            return Err(AppError::bad_request("Rollback backup checksum mismatch"));
        }
        archive_manifest(Path::new(&backup))?;
        let stage = path.with_file_name(format!(
            ".{}.revert-{}.tmp",
            path.file_name().unwrap().to_string_lossy(),
            unique_id("stage")
        ));
        let _cleanup = Cleanup(stage.clone());
        copy_new_synced(Path::new(&backup), &stage)?;
        if Some(calculate_file_sha256(&stage)?) != record.original_sha256 {
            return Err(AppError::internal("Rollback staging mismatch"));
        }
        self.set_status(history_id, "reverting").await?;
        if Some(calculate_file_sha256(&path)?) != record.normalized_sha256 {
            return Err(AppError::new(
                409,
                "rollback_conflict",
                "Book changed before rollback publication",
            ));
        }
        fs::rename(&stage, &path)?;
        sync_parent(&path)?;
        self.set_status(history_id, "reverted").await?;
        let id = history_id.to_string();
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE normalization_history SET refresh_pending=1 WHERE id=?1",
                    [id],
                )?;
                Ok(())
            })
            .await?;
        let result = self.get_history_record(history_id).await?.unwrap();
        if let Some(kc) = komga {
            self.refresh_record(&result, kc).await?;
        }
        Ok(self.get_history_record(history_id).await?.unwrap())
    }

    pub async fn refresh_record(
        &self,
        record: &NormalizationHistoryRecord,
        komga: &KomgaClient,
    ) -> Result<()> {
        if !record.refresh_pending || !matches!(record.status.as_str(), "normalized" | "reverted") {
            return Ok(());
        }
        let mut succeeded = false;
        for _ in 0..3 {
            let result = if let Some(book) = &record.komga_book_id {
                komga.refresh_book_metadata(book).await
            } else if let Some(series) = &record.komga_series_id {
                komga.refresh_series_metadata(series).await
            } else {
                break;
            };
            if result.is_ok() {
                succeeded = true;
                break;
            }
        }
        let id = record.id.clone();
        self.db.call(move |conn| {
            conn.execute("UPDATE normalization_history SET refresh_pending=?2, error_message=?3 WHERE id=?1",
                params![id,!succeeded, if succeeded {None} else {Some("Komga refresh pending; retry from Maintenance history")}])?;
            Ok(())
        }).await
    }

    pub async fn execute_preview(
        &self,
        preview: &str,
        books: &[String],
        approved: bool,
        dry_run: bool,
        komga: &KomgaClient,
    ) -> Result<NormalizationExecutionResult> {
        if books.is_empty()
            || books.len() > 25
            || books.iter().collect::<HashSet<_>>().len() != books.len()
        {
            return Err(AppError::bad_request("Select 1–25 distinct preview books"));
        }
        if !dry_run && !approved {
            return Err(AppError::bad_request("Explicit mutation approval required"));
        }
        let libraries = komga.list_libraries().await?;
        let batch = unique_id("batch");
        let mut result = NormalizationExecutionResult {
            batch_id: batch.clone(),
            total_processed: 0,
            normalized_count: 0,
            already_normalized_count: 0,
            failed_count: 0,
            skipped_count: 0,
            items: Vec::new(),
        };
        for book in books {
            let (preview, book) = (preview.to_string(), book.clone());
            let saved: Option<(String,String,String,String)> = self.db.call(move |conn| Ok(conn.query_row(
                "SELECT library_id,library_root,item_json,proposed_xml FROM normalization_previews WHERE preview_id=?1 AND book_id=?2 AND created_at>=?3",
                params![preview,book,now()-86400], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?)).await?;
            let Some((lib_id, root, json, xml)) = saved else {
                return Err(AppError::bad_request("Missing or expired dry-run preview"));
            };
            let frozen: FrozenItem =
                serde_json::from_str(&json).map_err(|e| AppError::internal(e.to_string()))?;
            let item = frozen.item;
            let library = libraries.iter().find(|l| l.id == lib_id && l.root == root);
            let policy = match library {
                Some(l) => self.library_policy_reason(l).await?,
                None => Some("Library configuration changed; repeat dry run".into()),
            };
            if policy.is_some() {
                result.skipped_count += 1;
                let mut rec = history_item(unique_id("skip"), &batch, &item, "", "", "skipped");
                rec.error_message = policy;
                result.items.push(rec);
                continue;
            }
            let outcome = async {
                if !komga
                    .list_series(Some(&lib_id))
                    .await?
                    .iter()
                    .any(|s| s.id == item.komga_series_id && s.library_id == lib_id)
                {
                    return Err(AppError::bad_request(
                        "Series moved out of preview library; repeat dry run",
                    ));
                }
                let live_books = komga.get_books_in_series(&item.komga_series_id).await?;
                let live = live_books
                    .iter()
                    .find(|b| b.id == item.komga_book_id)
                    .ok_or_else(|| AppError::bad_request("Book no longer in series"))?;
                let local_root = self
                    .canonical_library_root(
                        library.ok_or_else(|| {
                            AppError::bad_request("Preview library is no longer available")
                        })?,
                        None,
                    )
                    .await?;
                let path = book_path(Path::new(&root), &local_root, live.url.as_deref())?;
                if path.to_string_lossy() != item.canonical_path {
                    return Err(AppError::bad_request(
                        "Canonical path changed; repeat dry run",
                    ));
                }
                if dry_run {
                    if Some(calculate_file_sha256(&path)?) != item.current_sha256 {
                        return Err(AppError::bad_request("Stale preview"));
                    }
                    return Ok(history_item(
                        unique_id("dry"),
                        &batch,
                        &item,
                        "",
                        "",
                        "would_change",
                    ));
                }
                self.apply_preview_with_cover(
                    &batch,
                    &item,
                    &xml,
                    Some(komga),
                    frozen.mango_cover.as_ref(),
                )
                .await
            }
            .await;
            match outcome {
                Ok(rec) => {
                    match rec.status.as_str() {
                        "normalized" => result.normalized_count += 1,
                        "already_normalized" => result.already_normalized_count += 1,
                        _ => {}
                    }
                    result.items.push(rec);
                }
                Err(e) => {
                    result.failed_count += 1;
                    let mut rec =
                        history_item(unique_id("failed"), &batch, &item, "", "", "failed");
                    rec.error_message = Some(e.to_string());
                    let r = rec.clone();
                    self.db.call(move |conn|{conn.execute("INSERT INTO normalization_history (id,batch_id,canonical_path,series_title,volume,status,error_message,normalized_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![r.id,r.batch_id,r.canonical_path,r.series_title,r.volume,r.status,r.error_message,r.normalized_at])?;Ok(())}).await?;
                    result.items.push(rec);
                }
            }
        }
        result.total_processed = result.items.len();
        Ok(result)
    }

    /// Fetches a normalization history record by ID.
    pub async fn get_history_record(&self, id: &str) -> Result<Option<NormalizationHistoryRecord>> {
        self.recover_record(id).await?;
        let h_id = id.to_string();
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, batch_id, canonical_path, series_title, volume,
                            komga_series_id, komga_book_id, mangabaka_id, mangabaka_release_id,
                            anilist_id, original_sha256, normalized_sha256, status,
                            error_message, normalized_at, reverted_at, refresh_pending, backup_path IS NOT NULL
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
                            refresh_pending: r.get(16)?,
                            rollback_available: r.get(17)?,
                        })
                    })
                    .optional()?;
                Ok(res)
            })
            .await
    }

    /// Lists recent normalization history records.
    pub async fn list_history(&self, limit: usize) -> Result<Vec<NormalizationHistoryRecord>> {
        let pending: Vec<String> = self.db.call(move |conn| {
            let mut stmt=conn.prepare("SELECT id FROM normalization_history WHERE status IN ('prepared','reverting') ORDER BY normalized_at LIMIT ?1")?;
            let rows=stmt.query_map([limit], |r|r.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(rows)
        }).await?;
        for id in pending {
            self.recover_record(&id).await?;
        }
        self.db
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, batch_id, canonical_path, series_title, volume,
                            komga_series_id, komga_book_id, mangabaka_id, mangabaka_release_id,
                            anilist_id, original_sha256, normalized_sha256, status,
                            error_message, normalized_at, reverted_at, refresh_pending, backup_path IS NOT NULL
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
                            refresh_pending: r.get(16)?,
                            rollback_available: r.get(17)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
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

        // 5. Chinese Manga is excluded until policy validation.
        let manga_zh = KomgaLibrary {
            id: "0RZVF3P8MEC03".into(),
            name: "Chinese Manga".into(),
            root: "/srv/media/manga-chinese".into(),
            unavailable: false,
        };
        assert!(!service.is_library_eligible_for_mangabaka(&manga_zh));
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
            work_cover_url: None,
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
        use axum::{routing::get, Json, Router};
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
  <Web>https://mangabaka.org/series/83</Web>
  <Translator>Legacy Translator</Translator>
  <CustomField>Preserved</CustomField>
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

        // Mock Komga uses the actual filesystem URL, unrelated to display names.
        let second = temp_dir.join("another-book.cbz");
        fs::copy(&cbz_path, &second).unwrap();
        let root = temp_dir.to_string_lossy().to_string();
        let first_url = cbz_path.to_string_lossy().to_string();
        let second_url = second.to_string_lossy().to_string();
        let refresh_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts = refresh_attempts.clone();
        let komga_app=Router::new()
            .route("/api/v1/libraries",get(move || {let root=root.clone();async move {Json(json!([
                {"id":"en","name":"English Manga","root":root},
                {"id":"comics","name":"Comics","root":"/nonexistent/comics"},
                {"id":"zh","name":"Chinese Manga","root":"/nonexistent/manga-chinese"}
            ]))}}))
            .route("/api/v1/series",get(||async {Json(json!({"content":[{"id":"komga-s1","name":"Blue Lock","libraryId":"en","booksCount":2}]}))}))
            .route("/api/v1/series/komga-s1/books",get(move || {let (first_url,second_url)=(first_url.clone(),second_url.clone());async move {Json(json!({"content":[
                {"id":"komga-b39","seriesId":"komga-s1","name":"Misleading display name v39","number":39,"url":first_url},
                {"id":"broken","seriesId":"komga-s1","name":"Blue Lock v39","number":39,"url":second_url}
            ]}))}}))
            .route("/api/v1/books/komga-b39/analyze",axum::routing::post(move || {let attempts=attempts.clone();async move {
                if attempts.fetch_add(1,std::sync::atomic::Ordering::SeqCst)<3 {axum::http::StatusCode::SERVICE_UNAVAILABLE} else {axum::http::StatusCode::NO_CONTENT}
            }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, komga_app).await.unwrap();
        });
        let kc = KomgaClient::new(reqwest::Client::new(), Some(format!("http://{addr}")), None);
        for library in ["comics", "zh"] {
            let skipped = service
                .analyze_library(&kc, &mb_client, library, None, None)
                .await
                .unwrap();
            assert_eq!(skipped.skipped_count, 2);
            assert_eq!(skipped.error_count, 0);
            assert!(skipped.series[0]
                .books
                .iter()
                .all(|b| b.status == NormalizationStatus::SkippedProviderNotApplicable));
            if library == "zh" {
                assert!(skipped.series[0].reason.contains("policy"));
            }
        }
        let analysis = service
            .analyze_library(&kc, &mb_client, "en", None, None)
            .await
            .unwrap();
        assert_eq!(analysis.total_books, 2);
        assert_eq!(analysis.needs_update_count, 2);
        assert_eq!(calculate_file_sha256(&cbz_path).unwrap(), orig_sha);
        let ids = vec!["broken".into(), "komga-b39".into()];
        assert!(service
            .execute_preview(&analysis.preview_id, &ids, false, false, &kc)
            .await
            .is_err());
        let dry = service
            .execute_preview(&analysis.preview_id, &ids, false, true, &kc)
            .await
            .unwrap();
        assert_eq!(dry.items[0].status, "would_change");
        assert!(service.list_history(50).await.unwrap().is_empty());
        assert_eq!(calculate_file_sha256(&cbz_path).unwrap(), orig_sha);
        // One stale/invalid book must not block independent successful books.
        fs::write(&second, b"corrupted after preview").unwrap();
        let batch = service
            .execute_preview(&analysis.preview_id, &ids, true, false, &kc)
            .await
            .unwrap();
        assert_eq!(batch.failed_count, 1);
        assert_eq!(batch.normalized_count, 1);
        assert_eq!(batch.total_processed, 2);
        let record = batch
            .items
            .iter()
            .find(|r| r.status == "normalized")
            .unwrap()
            .clone();
        assert!(service
            .list_history(50)
            .await
            .unwrap()
            .iter()
            .any(|r| r.status == "failed"));
        assert!(record.refresh_pending);
        assert_eq!(
            refresh_attempts.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        service.refresh_record(&record, &kc).await.unwrap();
        assert!(
            !service
                .get_history_record(&record.id)
                .await
                .unwrap()
                .unwrap()
                .refresh_pending
        );

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

        assert!(new_xml.contains("<Translator>Legacy Translator</Translator>"));
        assert!(new_xml.contains("<CustomField>Preserved</CustomField>"));
        assert_eq!(comic_info_count(&cbz_path).unwrap(), 1);
        let backup: String = service
            .db
            .call({
                let id = record.id.clone();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT backup_path FROM normalization_history WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(
            archive_manifest(&cbz_path).unwrap(),
            archive_manifest(Path::new(&backup)).unwrap()
        );

        let normalized_bytes = fs::read(&cbz_path).unwrap();
        let id = record.id.clone();
        service
            .db
            .call(move |conn| {
                conn.execute(
                    "UPDATE normalization_history SET status='prepared' WHERE id=?1",
                    [id],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get_history_record(&record.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "normalized"
        );
        fs::write(&cbz_path, b"externally changed").unwrap();
        assert_eq!(
            service
                .rollback_normalization(&record.id, None)
                .await
                .err()
                .unwrap()
                .code,
            "rollback_conflict"
        );
        fs::write(&cbz_path, &normalized_bytes).unwrap();

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

    #[test]
    fn test_concurrent_file_lock_rejection() {
        let locks = Mutex::new(HashSet::new());
        let path = Path::new("/tmp/concurrent-test.cbz");
        let guard = FileLock::acquire(&locks, path).unwrap();
        assert_eq!(
            FileLock::acquire(&locks, path).err().unwrap().code,
            "file_locked"
        );
        drop(guard);
        assert!(FileLock::acquire(&locks, path).is_ok());
    }
    fn fixture(path: &Path, xmls: &[(&str, &str)]) {
        let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("pages/001.jpg", options).unwrap();
        archive.write_all(b"original image bytes").unwrap();
        archive.start_file("extra.txt", options).unwrap();
        archive.write_all(b"preserved non-image entry").unwrap();
        for (name, xml) in xmls {
            archive.start_file(*name, options).unwrap();
            archive.write_all(xml.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }
    fn enrichment() -> MangaBakaEnrichment {
        MangaBakaEnrichment {
            series_id: 83,
            series_title: Some("A & B".into()),
            work_title: None,
            collection_id: "edition".into(),
            volume: 1.0,
            release_date: None,
            summary: None,
            pages: None,
            writers: vec![],
            pencillers: vec![],
            genres: vec![
                "Award_winning".into(),
                "Slice_of_life".into(),
                "Sci_fi".into(),
                "Sci-Fi".into(),
                "boys love".into(),
            ],
            tags: vec!["Football".into(), "Soccer".into()],
            publisher: None,
            total_volumes: None,
            web_url: None,
            language: Some("en".into()),
            work_id: None,
            work_cover_url: None,
        }
    }
    #[test]
    fn proposed_diff_matches_written_fields_and_preserves_missing_provider_values() {
        let old="<ComicInfo><Series>Old</Series><Title>Useful legacy title</Title><Summary>Useful summary &amp; text</Summary><Publisher>Legacy</Publisher><Pages><Page Image=\"0\" Type=\"FrontCover\" /></Pages></ComicInfo>";
        let (xml, diffs) = proposed_metadata(Some(old), &enrichment(), 1.0, 1).unwrap();
        let fields = comic_fields(&xml).unwrap();
        assert_eq!(fields["Publisher"], "Legacy");
        assert_eq!(fields["Title"], "Useful legacy title");
        assert_eq!(fields["Summary"], "Useful summary &amp; text");
        assert!(fields["Pages"].contains("FrontCover"));
        assert_eq!(
            fields["Genre"],
            "Award Winning, Slice of Life, Sci-Fi, Boys Love"
        );
        assert_eq!(fields["Tags"], "Soccer");
        assert_eq!(fields["PageCount"], "1");
        assert_eq!(fields["Series"], "A &amp; B");
        assert!(diffs.iter().any(|d| d.field == "Tags"));
        assert!(!diffs
            .iter()
            .any(|d| d.field == "Summary" || d.field == "Publisher"));
        assert!(proposed_metadata(Some(&xml), &enrichment(), 1.0, 1)
            .unwrap()
            .1
            .is_empty());
        assert!(proposed_metadata(
            Some("<ComicInfo><Series>broken</ComicInfo>"),
            &enrichment(),
            1.0,
            1
        )
        .is_err());
        assert!(proposed_metadata(
            Some("<ComicInfo><Web>https://mangabaka.org/series/99</Web></ComicInfo>"),
            &enrichment(),
            1.0,
            1
        )
        .is_err());
    }

    #[tokio::test]
    async fn duplicate_metadata_staging_validation_and_stale_rejection() {
        let dir = std::env::temp_dir().join(unique_id("mango-normalization-safety"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.cbz");
        let original = "<ComicInfo><Series>Old</Series></ComicInfo>";
        fixture(
            &path,
            &[
                ("comicinfo.xml", original),
                ("nested/ComicInfo.xml", original),
            ],
        );
        let manifest = archive_manifest(&path).unwrap();
        let sha = calculate_file_sha256(&path).unwrap();
        let xml = proposed_metadata(Some(original), &enrichment(), 1.0, 1)
            .unwrap()
            .0;
        let item = BookNormalizationItem {
            komga_book_id: "b".into(),
            komga_series_id: "s".into(),
            series_title: "A & B".into(),
            book_name: "book".into(),
            canonical_path: path.to_string_lossy().into(),
            volume: Some(1.0),
            status: NormalizationStatus::NeedsUpdate,
            reason: String::new(),
            mangabaka_id: Some(83),
            mangabaka_release_id: None,
            anilist_id: None,
            diffs: vec![],
            current_sha256: Some(sha.clone()),
        };
        let db_path = dir.join("test.sqlite");
        let db = Db::open(&db_path).unwrap();
        let service = LibraryNormalizationService::new(db, reqwest::Client::new());
        assert!(service
            .apply_preview("batch", &item, "not XML", None)
            .await
            .is_err());
        assert_eq!(calculate_file_sha256(&path).unwrap(), sha);
        assert!(service.list_history(10).await.unwrap().is_empty());
        let lock = FileLock::acquire(&service.file_locks, &path).unwrap();
        assert_eq!(
            service
                .apply_preview("batch", &item, &xml, None)
                .await
                .err()
                .unwrap()
                .code,
            "file_locked"
        );
        drop(lock);
        service.db.call(|conn|{conn.execute_batch("CREATE TRIGGER reject_normalization BEFORE INSERT ON normalization_history BEGIN SELECT RAISE(ABORT,'simulated journal failure'); END;")?;Ok(())}).await.unwrap();
        assert!(service
            .apply_preview("batch", &item, &xml, None)
            .await
            .is_err());
        assert_eq!(calculate_file_sha256(&path).unwrap(), sha);
        service
            .db
            .call(|conn| {
                conn.execute_batch("DROP TRIGGER reject_normalization;")?;
                Ok(())
            })
            .await
            .unwrap();
        let record = service
            .apply_preview("batch", &item, &xml, None)
            .await
            .unwrap();
        assert_eq!(comic_info_count(&path).unwrap(), 1);
        assert!(canonical_comic_info_layout(&path).unwrap());
        assert_eq!(archive_manifest(&path).unwrap(), manifest);
        assert_eq!(
            service
                .apply_preview("batch", &item, &xml, None)
                .await
                .err()
                .unwrap()
                .code,
            "stale_preview"
        );
        // Simulate a crash after replacement but before the final journal update.
        let id = record.id.clone();
        service
            .db
            .call(move |conn| {
                conn.execute(
                    "UPDATE normalization_history SET status='prepared' WHERE id=?1",
                    [id],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        drop(service);
        let restarted =
            LibraryNormalizationService::new(Db::open(&db_path).unwrap(), reqwest::Client::new());
        assert_eq!(
            restarted.list_history(10).await.unwrap()[0].status,
            "normalized"
        );
        let backup: String = restarted
            .db
            .call({
                let id = record.id.clone();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT backup_path FROM normalization_history WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        let backup_bytes = fs::read(&backup).unwrap();
        fs::write(&backup, b"invalid backup").unwrap();
        let current = calculate_file_sha256(&path).unwrap();
        assert!(restarted
            .rollback_normalization(&record.id, None)
            .await
            .is_err());
        assert_eq!(calculate_file_sha256(&path).unwrap(), current);
        fs::write(&backup, backup_bytes).unwrap();
        restarted
            .rollback_normalization(&record.id, None)
            .await
            .unwrap();
        assert_eq!(calculate_file_sha256(&path).unwrap(), sha);
        // Repeated rollback is harmless; recovery of the rollback status window is durable.
        let id = record.id.clone();
        restarted
            .db
            .call(move |conn| {
                conn.execute(
                    "UPDATE normalization_history SET status='reverting' WHERE id=?1",
                    [id],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            restarted
                .rollback_normalization(&record.id, None)
                .await
                .unwrap()
                .status,
            "reverted"
        );
        // An invalid staged archive cannot pass publication validation.
        let invalid = dir.join("invalid.cbz");
        fs::write(&invalid, b"not a zip").unwrap();
        assert!(publish_normalized(
            &path,
            &invalid,
            &sha,
            &calculate_file_sha256(&invalid).unwrap(),
            &manifest
        )
        .is_err());
        assert_eq!(calculate_file_sha256(&path).unwrap(), sha);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn persisted_ambiguous_identity_fails_closed_without_external_lookup() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.call(|conn|{conn.execute("INSERT INTO tracker_mappings (komga_series_id,tracker,tracker_id,updated_at,confidence) VALUES ('s','mangabaka','83',0,'ambiguous')",[])?;Ok(())}).await.unwrap();
        let service = LibraryNormalizationService::new(db.clone(), reqwest::Client::new());
        let mb = MangaBakaClient::new(
            reqwest::Client::new(),
            Some("http://127.0.0.1:1".into()),
            None,
        );
        let (id, _, reason) = service
            .resolve_series_identity(&mb, "s", "Title", None)
            .await;
        assert!(id.is_none());
        assert!(reason.contains("Ambiguous"));
        db.call(|conn| {
            conn.execute("UPDATE tracker_mappings SET confidence='resolved'", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            service
                .resolve_series_identity(&mb, "s", "Title", None)
                .await
                .0,
            Some(83)
        );
    }
    #[tokio::test]
    async fn configured_chinese_library_is_excluded_even_with_generic_name_and_root() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.call(|conn| {
            crate::settings::set(
                conn,
                "pipeline.komga_chinese_library_id",
                &serde_json::json!("zh"),
            )
        })
        .await
        .unwrap();
        let service = LibraryNormalizationService::new(db, reqwest::Client::new());
        let library = KomgaLibrary {
            id: "zh".into(),
            name: "Manga".into(),
            root: "/library/manga".into(),
            unavailable: false,
        };
        assert!(service
            .library_policy_reason(&library)
            .await
            .unwrap()
            .unwrap()
            .contains("Chinese"));
    }
    #[test]
    fn maintenance_does_not_convert_chapters_or_komga_ordinals_to_volumes() {
        assert_eq!(
            maintenance_volume(
                "Chapter 5",
                Some("<ComicInfo><Volume>1</Volume></ComicInfo>")
            ),
            None
        );
        assert_eq!(maintenance_volume("Book", None), None);
        assert_eq!(
            maintenance_volume("Book", Some("<ComicInfo><Volume>5</Volume></ComicInfo>")),
            Some(5.0)
        );
        assert_eq!(
            maintenance_volume(
                "Title v01",
                Some("<ComicInfo><Volume>5</Volume></ComicInfo>")
            ),
            None
        );
        assert_eq!(maintenance_volume("Title v12.5", None), Some(12.5));
    }
    #[test]
    fn komga_paths_support_file_urls_but_never_escape_library_root() {
        let dir = std::env::temp_dir().join(unique_id("mango-path-test"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Book with spaces.cbz");
        fixture(&path, &[]);
        let url = reqwest::Url::from_file_path(&path).unwrap();
        assert_eq!(
            book_path(&dir, &dir, Some(url.as_str())).unwrap(),
            fs::canonicalize(&path).unwrap()
        );
        assert!(book_path(&dir.join("other"), &dir.join("other"), Some(url.as_str())).is_err());
        assert!(book_path(&dir, &dir, Some("https://example.invalid/book.cbz")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn separate_account_services_share_file_locks() {
        let first = LibraryNormalizationService::new(
            Db::open(Path::new(":memory:")).unwrap(),
            reqwest::Client::new(),
        );
        let second = LibraryNormalizationService::new(
            Db::open(Path::new(":memory:")).unwrap(),
            reqwest::Client::new(),
        );
        assert!(Arc::ptr_eq(&first.file_locks, &second.file_locks));
        let path = PathBuf::from(format!("/tmp/{}", unique_id("cross-account")));
        let guard = FileLock::acquire(&first.file_locks, &path).unwrap();
        assert!(FileLock::acquire(&second.file_locks, &path).is_err());
        drop(guard);
        assert!(FileLock::acquire(&second.file_locks, &path).is_ok());
    }
}

#[cfg(test)]
mod canonical_cover_tests {
    use super::*;
    use axum::{
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    use std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Fixture {
        dir: PathBuf,
        path: PathBuf,
        base: String,
        service: LibraryNormalizationService,
        mb: MangaBakaClient,
        komga: KomgaClient,
        art: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
    fn jpeg(value: u8) -> Vec<u8> {
        let mut bytes = vec![];
        jpeg_encoder::Encoder::new(&mut bytes, 80)
            .encode(&[value; 12], 2, 2, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        bytes
    }
    fn entry(path: &Path, name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut b = vec![];
        zip.by_name(name).unwrap().read_to_end(&mut b).unwrap();
        b
    }
    fn make_book(path: &Path, collision: Option<&str>) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (n, b) in [("001.jpg", jpeg(10)), ("002.jpg", jpeg(20))] {
            zip.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&b).unwrap();
        }
        if let Some(name) = collision {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&jpeg(30)).unwrap();
        }
        zip.finish().unwrap();
    }
    impl Fixture {
        async fn new(collision: Option<&str>, edition: &str) -> Self {
            Self::with_mapping(collision, edition, false).await
        }
        async fn with_mapping(collision: Option<&str>, edition: &str, mapped: bool) -> Self {
            let dir = std::env::temp_dir().join(unique_id("manga-canonical-cover"));
            fs::create_dir_all(&dir).unwrap();
            let relative = if mapped {
                Path::new("Publisher (EN)/Stories & Series/Fixture v03 [Official!].cbz")
            } else {
                Path::new("Fixture v03.cbz")
            };
            let path = dir.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            make_book(&path, collision);
            let db = Db::open(&dir.join("state.db")).unwrap();
            db.call(|c|{c.execute("INSERT INTO tracker_mappings(komga_series_id,tracker,tracker_id,updated_at,confidence)VALUES('s','mangabaka','83',0,'resolved')",[])?;Ok(())}).await.unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let art = Arc::new(AtomicUsize::new(100));
            let artwork = art.clone();
            let work_art = art.clone();
            let remote = if mapped {
                PathBuf::from(format!("/komga-visible-only/{}", unique_id("manga")))
            } else {
                dir.clone()
            };
            if mapped {
                let local = dir.to_string_lossy().to_string();
                db.call(move |c| {
                    crate::settings::set(c, "pipeline.library_dir", &json!(local))?;
                    Ok(())
                })
                .await
                .unwrap();
            }
            let root = remote.to_string_lossy().to_string();
            let book = remote.join(relative).to_string_lossy().to_string();
            let title = if edition == "master" {
                "Fixture Master Edition"
            } else {
                "Fixture"
            }
            .to_string();
            let app=Router::new()
                .route("/api/v1/libraries",get(move||{let root=root.clone();async move{Json(json!([{"id":"en","name":"Manga","root":root,"unavailable":false}]))}}))
                .route("/api/v1/series",get(move||{let title=title.clone();async move{Json(json!({"content":[{"id":"s","name":title,"libraryId":"en","booksCount":1}],"last":true}))}}))
                .route("/api/v1/series/s/books",get(move||{let book=book.clone();async move{Json(json!({"content":[{"id":"b","seriesId":"s","name":"Fixture v03","number":3,"url":book}],"last":true}))}}))
                .route("/api/v1/books/b/analyze",post(||async{axum::http::StatusCode::NO_CONTENT}))
                .route("/series/83",get(||async{Json(json!({"data":{"id":83,"title":"Fixture"}}))}))
                .route("/series/83/collections",get(||async{Json(json!({"data":[
                    {"id":"standard","language":"en","edition":{"name":"Standard Edition"},"medium":"digital"},
                    {"id":"master","language":"en","edition":{"name":"Master Edition"},"medium":"digital"}
                ]}))}))
                .route("/collections/{edition}/works",get(move |axum::extract::Path(edition): axum::extract::Path<String>| {
                    let art = work_art.clone();
                    async move {
                        let works = (2..=4).map(|n| {
                            let images = if art.load(Ordering::SeqCst) == 2 { json!([]) } else {
                                json!([{"image":{"raw":{"url":format!("http://{addr}/art/{edition}/{n}")}}}])
                            };
                            json!({"id":format!("{edition}-{n}"),"sequence_numeric":n,"images":images})
                        }).collect::<Vec<_>>();
                        Json(json!({"data":works}))
                    }
                }))
                .route("/art/{edition}/{volume}",get(move|axum::extract::Path((edition,volume)):axum::extract::Path<(String,u8)>|{let art=artwork.clone();async move{
                    match art.load(Ordering::SeqCst) {0=>axum::http::StatusCode::NOT_FOUND.into_response(),1=>([(axum::http::header::CONTENT_TYPE,"image/jpeg")],"invalid").into_response(),v=>jpeg((v as u8).wrapping_add(volume).wrapping_add(if edition=="master"{20}else{0})).into_response()}
                }}));
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let http = reqwest::Client::new();
            let mb = MangaBakaClient::new(http.clone(), Some(format!("http://{addr}")), None);
            let komga = KomgaClient::new(http.clone(), Some(format!("http://{addr}")), None);
            Self {
                dir,
                path,
                base: format!("http://{addr}"),
                service: LibraryNormalizationService::new(db, http),
                mb,
                komga,
                art,
                server,
            }
        }
        async fn analyze(&self) -> LibraryAnalysisSummary {
            self.service
                .analyze_library(&self.komga, &self.mb, "en", None, None)
                .await
                .unwrap()
        }
        async fn execute(&self, preview: &LibraryAnalysisSummary) -> NormalizationExecutionResult {
            self.service
                .execute_preview(&preview.preview_id, &["b".into()], true, false, &self.komga)
                .await
                .unwrap()
        }
        async fn metadata_only(&self, edition: &str) {
            let MangaBakaOutcome::Success(mut e) = self
                .mb
                .resolve_edition_volume_metadata("Fixture", None, Some(83), 3., Some(edition))
                .await
                .unwrap()
            else {
                panic!()
            };
            if edition == "master" {
                e.series_title = Some("Fixture Master Edition".into());
            }
            let xml = proposed_metadata(None, &e, 3., image_count(&self.path).unwrap())
                .unwrap()
                .0;
            mangabaka::write_prepared_comic_info_to_cbz(&self.path, &xml).unwrap();
        }
    }
    #[tokio::test]
    async fn canonical_cover_preview_execution_idempotency_and_exact_rollback() {
        let f = Fixture::new(None, "standard").await;
        f.metadata_only("standard").await;
        let sha = calculate_file_sha256(&f.path).unwrap();
        let images = [entry(&f.path, "001.jpg"), entry(&f.path, "002.jpg")];
        let preview = f.analyze().await;
        assert_eq!(preview.needs_update_count, 1);
        assert_eq!(preview.series[0].books[0].diffs.len(), 1);
        assert_eq!(
            preview.series[0].books[0].diffs[0].field,
            "Exact work cover"
        );
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
        let result = f.execute(&preview).await;
        assert_eq!(result.normalized_count, 1);
        assert_eq!(result.failed_count, 0);
        assert!(!result.items[0].refresh_pending);
        assert_eq!(entry(&f.path, "000-cover.jpg"), jpeg(103));
        for (i, name) in ["001.jpg", "002.jpg"].iter().enumerate() {
            assert_eq!(entry(&f.path, name), images[i]);
        }
        let xml = read_comic_info_xml_from_cbz(&f.path).unwrap().unwrap();
        let fields = comic_fields(&xml).unwrap();
        assert_eq!(fields["PageCount"], "3");
        assert!(fields["Pages"].contains("Image=\"0\" Type=\"FrontCover\""));
        assert_eq!(comic_info_count(&f.path).unwrap(), 1);
        verify_cbz_integrity(&f.path).unwrap();
        assert_eq!(f.analyze().await.already_normalized_count, 1);
        f.service
            .rollback_normalization(&result.items[0].id, None)
            .await
            .unwrap();
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
    }
    #[tokio::test]
    async fn frozen_artwork_survives_restart_outage_and_stale_canonical_is_refused() {
        let f = Fixture::new(None, "standard").await;
        f.metadata_only("standard").await;
        let preview = f.analyze().await;
        f.art.store(0, Ordering::SeqCst);
        let restarted = LibraryNormalizationService::new(
            Db::open(&f.dir.join("state.db")).unwrap(),
            reqwest::Client::new(),
        );
        let result = restarted
            .execute_preview(&preview.preview_id, &["b".into()], true, false, &f.komga)
            .await
            .unwrap();
        assert_eq!(result.normalized_count, 1);
        assert_eq!(entry(&f.path, "000-cover.jpg"), jpeg(103));
        f.service
            .rollback_normalization(&result.items[0].id, None)
            .await
            .unwrap();
        f.art.store(150, Ordering::SeqCst);
        let preview = f.analyze().await;
        fs::write(&f.path, b"changed after approval").unwrap();
        let sha = calculate_file_sha256(&f.path).unwrap();
        let result = f.execute(&preview).await;
        assert_eq!(result.failed_count, 1);
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
    }
    #[tokio::test]
    async fn changed_exact_art_updates_owned_cover_and_edition_is_preserved() {
        let f = Fixture::new(None, "master").await;
        f.metadata_only("master").await;
        let first = f.execute(&f.analyze().await).await;
        assert_eq!(first.normalized_count, 1);
        assert_eq!(entry(&f.path, "000-cover.jpg"), jpeg(123));
        let original = entry(&f.path, "001.jpg");
        f.art.store(150, Ordering::SeqCst);
        let preview = f.analyze().await;
        assert_eq!(
            preview.series[0].books[0].mangabaka_release_id.as_deref(),
            Some("master-3")
        );
        assert_eq!(f.execute(&preview).await.normalized_count, 1);
        assert_eq!(entry(&f.path, "000-cover.jpg"), jpeg(173));
        assert_eq!(entry(&f.path, "001.jpg"), original);
        assert!(read_comic_info_xml_from_cbz(&f.path)
            .unwrap()
            .unwrap()
            .contains("Fixture Master Edition"));
        assert_eq!(f.analyze().await.already_normalized_count, 1);
    }
    #[tokio::test]
    async fn optional_download_validation_and_collision_leave_canonical_safe() {
        for collision in [None, Some("000-COVER.JPG")] {
            let f = Fixture::new(collision, "standard").await;
            f.metadata_only("standard").await;
            let sha = calculate_file_sha256(&f.path).unwrap();
            if collision.is_some() {
                let p = f.analyze().await;
                assert_eq!(p.error_count, 1);
                assert_eq!(p.needs_update_count, 0);
            } else {
                for value in [0, 1, 2] {
                    f.art.store(value, Ordering::SeqCst);
                    let p = f.analyze().await;
                    assert_eq!(p.already_normalized_count, 1);
                    assert_eq!(p.needs_update_count, 0);
                }
            }
            assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
        }
    }
    #[tokio::test]
    async fn legacy_owned_cover_is_retained_and_damaged_ownership_is_refused() {
        let f = Fixture::new(None, "standard").await;
        f.metadata_only("standard").await;
        let preview = f.analyze().await;
        let legacy = serde_json::to_string(&preview.series[0].books[0]).unwrap();
        assert!(serde_json::from_str::<FrozenItem>(&legacy)
            .unwrap()
            .mango_cover
            .is_none());
        assert_eq!(f.execute(&preview).await.normalized_count, 1);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&entry(&f.path, "mango-volume-cover.json")).unwrap();
        marker.as_object_mut().unwrap().remove("source_sha256");
        let xml = read_comic_info_xml_from_cbz(&f.path).unwrap().unwrap();
        mangabaka::rewrite_comic_info_with_entries(
            &f.path,
            &xml,
            &["mango-volume-cover.json"],
            &[(
                "mango-volume-cover.json",
                &serde_json::to_vec(&marker).unwrap(),
            )],
        )
        .unwrap();
        let sha = calculate_file_sha256(&f.path).unwrap();
        f.art.store(150, Ordering::SeqCst);
        assert_eq!(f.analyze().await.already_normalized_count, 1);
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
        mangabaka::rewrite_comic_info_with_entries(
            &f.path,
            &xml,
            &["000-cover.jpg"],
            &[("000-cover.jpg", &jpeg(200))],
        )
        .unwrap();
        let sha = calculate_file_sha256(&f.path).unwrap();
        assert_eq!(f.analyze().await.error_count, 1);
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
    }

    #[tokio::test]
    async fn canonical_repair_routes_through_backup_history_and_cover_normalization() {
        let f = Fixture::new(None, "standard").await;
        let original = calculate_file_sha256(&f.path).unwrap();
        let source = entry(&f.path, "001.jpg");
        let base = f.base.clone();
        f.service
            .db
            .call(move |c| {
                crate::settings::set(c, "pipeline.mangabaka_url", &json!(base))?;
                crate::settings::set(c, "pipeline.komga_enabled", &json!(false))?;
                Ok(())
            })
            .await
            .unwrap();
        let pipeline = crate::pipeline::Pipeline::new(f.service.db.clone());
        let result = pipeline
            .repair_canonical_cbz(&f.path, Some("Fixture"), Some(3.), None, Some(83))
            .await
            .unwrap();
        assert_eq!(result.status, "repaired");
        assert_eq!(result.page_count, 3);
        assert_eq!(entry(&f.path, "000-cover.jpg"), jpeg(103));
        assert_eq!(entry(&f.path, "001.jpg"), source);
        let history = f.service.list_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].rollback_available);
        let sha = calculate_file_sha256(&f.path).unwrap();
        assert_eq!(
            pipeline
                .repair_canonical_cbz(&f.path, Some("Fixture"), Some(3.), None, Some(83))
                .await
                .unwrap()
                .status,
            "already_normalized"
        );
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), sha);
        assert_eq!(f.service.list_history(10).await.unwrap().len(), 1);
        f.service
            .rollback_normalization(&history[0].id, None)
            .await
            .unwrap();
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), original);
    }

    #[test]
    fn legacy_preview_and_edition_conflict_remain_safe() {
        assert!(maintenance_edition(
            "Fixture Master Edition",
            Some("<ComicInfo><Series>Fixture Deluxe Edition</Series></ComicInfo>")
        )
        .is_err());
        assert_eq!(
            maintenance_edition(
                "Fixture",
                Some("<ComicInfo><Series>Fixture Master Edition</Series></ComicInfo>")
            )
            .unwrap()
            .as_deref(),
            Some("master")
        );
    }
    #[tokio::test]
    async fn mapped_roots_reach_cover_analysis_without_dry_run_mutation_and_execute_safely() {
        let f = Fixture::with_mapping(None, "standard", true).await;
        f.metadata_only("standard").await;
        let original = calculate_file_sha256(&f.path).unwrap();
        let source = entry(&f.path, "001.jpg");
        let preview = f.analyze().await;
        assert_eq!(preview.total_books, 1);
        assert_eq!(preview.error_count, 0);
        assert_eq!(preview.needs_update_count, 1);
        let book = &preview.series[0].books[0];
        assert_eq!(
            book.canonical_path,
            fs::canonicalize(&f.path).unwrap().to_string_lossy()
        );
        assert_eq!(book.mangabaka_release_id.as_deref(), Some("standard-3"));
        assert!(book.diffs.iter().any(|d| d.field == "Exact work cover"));
        assert!(!Path::new(&preview.library_root).exists());
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), original);
        let dry = f
            .service
            .execute_preview(&preview.preview_id, &["b".into()], false, true, &f.komga)
            .await
            .unwrap();
        assert_eq!(dry.failed_count, 0);
        assert_eq!(dry.items[0].status, "would_change");
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), original);
        assert!(f.service.list_history(10).await.unwrap().is_empty());
        let result = f.execute(&preview).await;
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.normalized_count, 1);
        assert_eq!(entry(&f.path, "001.jpg"), source);
        f.service
            .rollback_normalization(&result.items[0].id, None)
            .await
            .unwrap();
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), original);
    }
    #[tokio::test]
    async fn changed_configured_mapping_refuses_even_identical_book_at_another_root() {
        let f = Fixture::with_mapping(None, "standard", true).await;
        f.metadata_only("standard").await;
        let original = calculate_file_sha256(&f.path).unwrap();
        let preview = f.analyze().await;
        let other_root = f.dir.join("different-local-root");
        let other_book = other_root.join(f.path.strip_prefix(&f.dir).unwrap());
        fs::create_dir_all(other_book.parent().unwrap()).unwrap();
        fs::copy(&f.path, &other_book).unwrap();
        let root = other_root.to_string_lossy().to_string();
        f.service
            .db
            .call(move |c| {
                crate::settings::set(c, "pipeline.library_dir", &json!(root))?;
                Ok(())
            })
            .await
            .unwrap();
        let result = f.execute(&preview).await;
        assert_eq!(result.failed_count, 1);
        assert!(result.items[0]
            .error_message
            .as_deref()
            .unwrap()
            .contains("Canonical path changed"));
        assert_eq!(calculate_file_sha256(&f.path).unwrap(), original);
        assert_eq!(calculate_file_sha256(&other_book).unwrap(), original);
    }
    #[test]
    fn mapped_book_paths_support_nested_names_file_urls_and_reject_traversal() {
        let dir = std::env::temp_dir().join(unique_id("manga-path-mapping"));
        fs::create_dir_all(&dir).unwrap();
        let remote = dir.join("unmounted-provider-root");
        let local = dir.join("local-canonical");
        let relative = Path::new("Publisher (EN)/Series & Stories/Book v03 [Official!].cbz");
        let path = local.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        make_book(&path, None);
        let reported = remote.join(relative);
        let expected = fs::canonicalize(&path).unwrap();
        assert!(!remote.exists());
        assert_eq!(
            book_path(&remote, &local, reported.to_str()).unwrap(),
            expected
        );
        let url = reqwest::Url::from_file_path(&reported).unwrap();
        assert_eq!(
            book_path(&remote, &local, Some(url.as_str())).unwrap(),
            expected
        );
        assert_eq!(book_path(&remote, &local, path.to_str()).unwrap(), expected);
        for invalid in [
            format!(
                "{}/../local-canonical/{}",
                remote.display(),
                relative.display()
            ),
            format!(
                "file://{}/nested/%2e%2e/{}",
                remote.display(),
                relative.display()
            ),
            format!("{}/nested/../{}", remote.display(), relative.display()),
            format!("{}-other/{}", remote.display(), relative.display()),
            "relative/Book.cbz".into(),
            "https://example.invalid/Book.cbz".into(),
        ] {
            assert!(
                book_path(&remote, &local, Some(&invalid)).is_err(),
                "{invalid}"
            );
        }
        assert!(book_path(&remote, &local, remote.join("missing.cbz").to_str()).is_err());
        let non_book = local.join("not-a-book.txt");
        fs::write(&non_book, "text").unwrap();
        assert!(book_path(&remote, &local, remote.join("not-a-book.txt").to_str()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn mapped_book_path_rejects_symlink_escape_from_selected_local_root() {
        let dir = std::env::temp_dir().join(unique_id("manga-symlink-mapping"));
        let local = dir.join("canonical");
        let outside = dir.join("outside");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&outside).unwrap();
        make_book(&outside.join("Book.cbz"), None);
        std::os::unix::fs::symlink(&outside, local.join("escaped")).unwrap();
        let remote = dir.join("unmounted-provider-root");
        assert!(book_path(&remote, &local, remote.join("escaped/Book.cbz").to_str()).is_err());
        assert!(book_path(&remote, &local, local.join("escaped/Book.cbz").to_str()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
