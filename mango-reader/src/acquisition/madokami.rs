//! Madokami Direct Acquisition Provider (Milestone M6.8.1)
//!
//! Provides direct-download release discovery and candidate normalization
//! via authenticated HTTP queries to Madokami.
//!
//! Invariants:
//! - Direct HTTP transport: Madokami does NOT require TorBox or BitTorrent debrid.
//! - Strict chapter-vs-volume classification: Bare numbers (e.g. 137, 075) and
//!   chapter markers (ch137, c137) MUST NEVER be parsed as volume numbers.
//! - PRE safety: `(PRE)` releases are provisional and ineligible for auto-acquisition.
//! - Exact volume required: Only unambiguous single-volume CBZ archives qualify as `ExactVolume`.
//! - Recursive traversal with depth limit, visited set, and request bounding.
//! - Clean credentials separation: Basic Auth headers remain server-side and are
//!   never leaked into candidate IDs, URLs, or frontend payloads.

use crate::acquisition::providers::{
    is_known_quality_group, score_candidate, BoxFuture, CandidateMatchType, ProviderHealth,
    ProviderStatus, ReleaseCandidate, ReleaseProvider, ReleaseQuery,
};
use crate::db::now;
use crate::error::{AppError, Result};
use base64::Engine;
use reqwest::{header, Client, StatusCode};
use std::collections::HashSet;
use std::time::Duration;

pub const DEFAULT_MADOKAMI_URL: &str = "https://manga.madokami.al";

/// Madokami HTTP client and candidate extractor.
pub struct MadokamiClient {
    http: Client,
    base_url: String,
    auth_header: Option<String>,
}

impl MadokamiClient {
    pub fn new(
        http: Client,
        base_url: Option<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Self {
        let base = base_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MADOKAMI_URL.to_string());
        let trimmed_base = base.trim_end_matches('/').to_string();

        let auth_header = match (username, password) {
            (Some(u), Some(p)) if !u.trim().is_empty() && !p.trim().is_empty() => {
                let creds = format!("{}:{}", u.trim(), p.trim());
                let encoded = base64::engine::general_purpose::STANDARD.encode(creds);
                Some(format!("Basic {encoded}"))
            }
            _ => None,
        };

        Self {
            http,
            base_url: trimmed_base,
            auth_header,
        }
    }

    /// Whether valid Basic Auth credentials are configured.
    pub fn is_configured(&self) -> bool {
        self.auth_header.is_some()
    }

    /// Returns the Authorization header string if configured.
    pub fn auth_header(&self) -> Option<&str> {
        self.auth_header.as_deref()
    }

    /// Checks Madokami connectivity and authentication health.
    pub async fn check_health(&self) -> ProviderHealth {
        if !self.is_configured() {
            return ProviderHealth {
                provider_id: "madokami".to_string(),
                name: "Madokami".to_string(),
                status: ProviderStatus::AuthenticationRequired,
                message: Some("Madokami username and password not configured in Settings".to_string()),
                last_checked_at: Some(now()),
            };
        }

        let mut req = self.http.get(&self.base_url);
        if let Some(ref auth) = self.auth_header {
            req = req.header(header::AUTHORIZATION, auth);
        }

        match tokio::time::timeout(Duration::from_secs(10), req.send()).await {
            Ok(Ok(resp)) => {
                let status = resp.status();
                if status.is_success() {
                    ProviderHealth {
                        provider_id: "madokami".to_string(),
                        name: "Madokami".to_string(),
                        status: ProviderStatus::Available,
                        message: None,
                        last_checked_at: Some(now()),
                    }
                } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    ProviderHealth {
                        provider_id: "madokami".to_string(),
                        name: "Madokami".to_string(),
                        status: ProviderStatus::AuthenticationRequired,
                        message: Some("Madokami authentication failed: invalid credentials".to_string()),
                        last_checked_at: Some(now()),
                    }
                } else {
                    ProviderHealth {
                        provider_id: "madokami".to_string(),
                        name: "Madokami".to_string(),
                        status: ProviderStatus::TemporarilyUnavailable,
                        message: Some(format!("Madokami returned HTTP status {status}")),
                        last_checked_at: Some(now()),
                    }
                }
            }
            Ok(Err(e)) => ProviderHealth {
                provider_id: "madokami".to_string(),
                name: "Madokami".to_string(),
                status: ProviderStatus::TemporarilyUnavailable,
                message: Some(format!("Madokami connection failed: {e}")),
                last_checked_at: Some(now()),
            },
            Err(_) => ProviderHealth {
                provider_id: "madokami".to_string(),
                name: "Madokami".to_string(),
                status: ProviderStatus::TemporarilyUnavailable,
                message: Some("Madokami connection timed out".to_string()),
                last_checked_at: Some(now()),
            },
        }
    }

    /// Discovers candidate volume releases matching a target volume query.
    pub async fn search_releases(&self, query: &ReleaseQuery) -> Result<Vec<ReleaseCandidate>> {
        if !self.is_configured() {
            tracing::warn!("Madokami queried but credentials are not configured");
            return Ok(Vec::new());
        }

        let series_title = query.series_title.trim();
        if series_title.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Search Madokami directory index for matching title paths
        let search_url = format!(
            "{}/search?q={}",
            self.base_url,
            crate::acquisition::nyaa::url_encode(series_title)
        );
        let search_html = match self.fetch_html(&search_url).await {
            Ok(html) => html,
            Err(e) => {
                tracing::warn!("Madokami search query failed for '{series_title}': {e}");
                return Ok(Vec::new());
            }
        };

        let matching_dirs = parse_madokami_search_results(&search_html);
        if matching_dirs.is_empty() {
            tracing::debug!("Madokami search returned 0 matching directories for '{series_title}'");
            return Ok(Vec::new());
        }

        // Select the primary series root directory (shortest directory matching /Manga/...)
        let root_dir = select_primary_series_directory(&matching_dirs, series_title)
            .unwrap_or_else(|| matching_dirs[0].clone());

        // 2. Traversal: inspect series root and relevant subdirectories (e.g. !(Digital)*)
        let parsed_files = self.traverse_series_directory(&root_dir, query.target_volume).await?;

        // 3. Normalize into ReleaseCandidates
        let mut candidates = Vec::new();
        for file in parsed_files {
            let (match_type, parsed_vol, is_digital, _is_pre, group_opt) =
                classify_madokami_file(&file.filename, query.target_volume);

            if match_type == CandidateMatchType::Rejected {
                continue;
            }

            let full_url = format!("{}{}", self.base_url, file.path);
            let stable_id = format!("madokami:{}", file.path);

            // Clean title for display
            let display_title = file
                .filename
                .strip_suffix(".cbz")
                .or_else(|| file.filename.strip_suffix(".zip"))
                .unwrap_or(&file.filename)
                .to_string();

            let is_trusted = group_opt
                .as_deref()
                .map_or(false, |g| is_known_quality_group(g));

            let candidate = ReleaseCandidate {
                candidate_id: stable_id.clone(),
                provider: "madokami".to_string(),
                provider_item_id: file.path.clone(),
                title: display_title,
                raw_title: file.filename.clone(),
                match_type,
                target_volume: parsed_vol.or(query.target_volume),
                target_chapter: None,
                size_bytes: file.size_bytes,
                uploader: group_opt,
                is_trusted,
                is_digital,
                score: 0.0,
                score_explanation: String::new(),
                download_url: Some(full_url),
                magnet_url: None,
                info_hash: None,
                seeders: None,
                leechers: None,
                published_at: file.mtime_epoch,
                ..Default::default()
            };

            candidates.push(candidate);
        }

        // 4. Score and rank candidates
        for c in &mut candidates {
            score_candidate(c, query.target_volume);
        }

        // Madokami quality ranking: Non-PRE > PRE, then score desc, then size desc
        candidates.sort_by(|a, b| {
            let a_pre = a.raw_title.to_ascii_lowercase().contains("(pre)");
            let b_pre = b.raw_title.to_ascii_lowercase().contains("(pre)");
            a_pre
                .cmp(&b_pre) // non-pre (false) before pre (true)
                .then_with(|| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)))
        });

        Ok(candidates)
    }

    /// Recursively traverses a matched manga directory up to depth 2 and max 8 HTTP requests.
    async fn traverse_series_directory(
        &self,
        root_dir: &str,
        target_volume: Option<f64>,
    ) -> Result<Vec<MadokamiParsedFile>> {
        let mut visited = HashSet::new();
        let mut queue = Vec::new();
        let mut all_files = Vec::new();
        let mut request_count = 0;
        const MAX_REQUESTS: usize = 8;

        let root_clean = root_dir.trim_end_matches('/').to_string();
        queue.push((root_clean, 0));

        while let Some((dir_path, depth)) = queue.pop() {
            if request_count >= MAX_REQUESTS {
                tracing::debug!("Madokami traversal reached request limit ({MAX_REQUESTS})");
                break;
            }
            if !visited.insert(dir_path.clone()) {
                continue;
            }

            let dir_url = format!("{}{}", self.base_url, dir_path);
            let html = match self.fetch_html(&dir_url).await {
                Ok(h) => {
                    request_count += 1;
                    h
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch Madokami directory '{dir_path}': {e}");
                    continue;
                }
            };

            let entries = parse_madokami_directory_listing(&html, &dir_path);
            for entry in entries {
                match entry {
                    MadokamiDirEntry::File(file) => {
                        // If targeting a specific volume, quick filter to avoid storing unrelated files
                        if let Some(target_v) = target_volume {
                            let (match_type, _, _, _, _) =
                                classify_madokami_file(&file.filename, Some(target_v));
                            if match_type != CandidateMatchType::Rejected {
                                all_files.push(file);
                            }
                        } else {
                            all_files.push(file);
                        }
                    }
                    MadokamiDirEntry::Subdirectory(sub_path, sub_name) => {
                        if depth < 1 {
                            // Filter out known non-volume subdirectories
                            let sub_lower = sub_name.to_ascii_lowercase();
                            let should_skip = sub_lower.starts_with("!chapters")
                                || sub_lower.starts_with("!mag")
                                || sub_lower.starts_with("!scanlation")
                                || sub_lower.starts_with("!old")
                                || sub_lower.starts_with("!raw")
                                || sub_lower.contains("chapter");

                            if !should_skip {
                                queue.push((sub_path, depth + 1));
                            }
                        }
                    }
                }
            }
        }

        Ok(all_files)
    }

    /// Fetches HTML content with Basic Authentication.
    async fn fetch_html(&self, url: &str) -> Result<String> {
        let mut req = self
            .http
            .get(url)
            .header(header::USER_AGENT, "Mango/1.0 (Native Madokami Provider)")
            .header(header::ACCEPT, "text/html,application/xhtml+xml");

        if let Some(ref auth) = self.auth_header {
            req = req.header(header::AUTHORIZATION, auth);
        }

        let resp = tokio::time::timeout(Duration::from_secs(15), req.send())
            .await
            .map_err(|_| AppError::new(504, "timeout", "Madokami request timed out"))?
            .map_err(|e| AppError::new(502, "network_error", format!("Madokami send failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "madokami_error",
                format!("Madokami returned HTTP {}", resp.status()),
            ));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| AppError::new(502, "read_error", format!("Failed to read Madokami body: {e}")))?;

        Ok(text)
    }
}

// -----------------------------------------------------------------------------
// Directory & Search Parsing Utilities
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct MadokamiParsedFile {
    pub filename: String,
    pub path: String,
    pub size_bytes: Option<u64>,
    pub mtime_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MadokamiDirEntry {
    File(MadokamiParsedFile),
    Subdirectory(String, String), // (path, display_name)
}

/// Parses Madokami `/search?q=...` HTML response table for directory links.
pub fn parse_madokami_search_results(html: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    // Links in table body: <td><a href="/Manga/...">/Manga/...</a></td>
    for line in html.lines() {
        if line.contains("<a href=\"/Manga/") || line.contains("<a href='/Manga/") {
            if let Some(path) = extract_href_path(line) {
                if !dirs.contains(&path) {
                    dirs.push(path);
                }
            }
        }
    }
    dirs
}

/// Selects the primary root directory for a series from search results.
pub fn select_primary_series_directory(dirs: &[String], series_title: &str) -> Option<String> {
    let norm_title = series_title
        .to_ascii_lowercase()
        .replace([' ', '_', '-', '!', ':', '\''], "");

    let mut matches: Vec<&String> = dirs
        .iter()
        .filter(|d| {
            let last_seg = d.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            let norm_seg = last_seg
                .to_ascii_lowercase()
                .replace([' ', '_', '-', '!', ':', '\''], "");
            norm_seg == norm_title
        })
        .collect();

    // Prefer shortest path (the true series root, not subdirectories)
    matches.sort_by_key(|d| d.len());
    matches.first().map(|d| (*d).clone())
}

/// Parses a Madokami directory HTML table into files and subdirectories.
pub fn parse_madokami_directory_listing(html: &str, _current_dir: &str) -> Vec<MadokamiDirEntry> {
    let mut entries = Vec::new();

    // Split HTML into <tr> rows
    for row in html.split("<tr").skip(1) {
        let row_content = match row.split("</tr>").next() {
            Some(c) => c,
            None => continue,
        };

        if !row_content.contains("<td") {
            continue;
        }

        // Extract cells
        let cells: Vec<String> = row_content
            .split("<td")
            .skip(1)
            .filter_map(|cell| {
                cell.split("</td>").next().map(|inner| {
                    // Strip tags
                    let stripped = inner.split('>').skip(1).collect::<Vec<&str>>().join(">");
                    clean_html_text(&stripped)
                })
            })
            .collect();

        if cells.is_empty() {
            continue;
        }

        // Extract link href from first cell
        let link_opt = extract_href_path(row_content);
        let display_name = cells[0].trim().to_string();

        if let Some(path) = link_opt {
            let size_str = cells.get(1).map(|s| s.trim()).unwrap_or("");
            let time_str = cells.get(2).map(|s| s.trim()).unwrap_or("");

            let is_dir = display_name.ends_with('/')
                || size_str == "-"
                || (!path.ends_with(".cbz") && !path.ends_with(".zip") && !path.ends_with(".rar"));

            if is_dir {
                let sub_name = display_name.trim_end_matches('/').to_string();
                if !sub_name.is_empty() && sub_name != ".." && sub_name != "." {
                    entries.push(MadokamiDirEntry::Subdirectory(path, sub_name));
                }
            } else {
                let size_bytes = parse_madokami_size(size_str);
                let mtime_epoch = parse_madokami_datetime(time_str);
                let filename = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&display_name)
                    .to_string();

                // Decode URL-encoded filename if needed
                let clean_filename = url_decode(&filename);

                entries.push(MadokamiDirEntry::File(MadokamiParsedFile {
                    filename: clean_filename,
                    path,
                    size_bytes,
                    mtime_epoch,
                }));
            }
        }
    }

    entries
}

fn extract_href_path(html_snippet: &str) -> Option<String> {
    if let Some(start) = html_snippet.find("href=\"") {
        let after = &html_snippet[start + 6..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    if let Some(start) = html_snippet.find("href='") {
        let after = &html_snippet[start + 6..];
        if let Some(end) = after.find('\'') {
            return Some(after[..end].to_string());
        }
    }
    None
}

fn clean_html_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut inside_tag = false;
    for c in text.chars() {
        if c == '<' {
            inside_tag = true;
        } else if c == '>' {
            inside_tag = false;
        } else if !inside_tag {
            result.push(c);
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

/// Decodes a percent-encoded URL string.
pub fn url_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let input_bytes = input.as_bytes();
    let mut i = 0;
    while i < input_bytes.len() {
        if input_bytes[i] == b'%' && i + 2 < input_bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&input_bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                bytes.push(byte);
                i += 3;
                continue;
            }
        } else if input_bytes[i] == b'+' {
            bytes.push(b' ');
            i += 1;
            continue;
        }
        bytes.push(input_bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Parses Madokami human-readable size string (e.g. "144.3M", "4.0M", "512K", "1.2G") to bytes.
pub fn parse_madokami_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase();
    if s.is_empty() || s == "-" {
        return None;
    }

    let (num_part, unit) = if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(stripped) = s.strip_suffix("GB") {
        (stripped, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1024.0 * 1024.0)
    } else if let Some(stripped) = s.strip_suffix("MB") {
        (stripped, 1024.0 * 1024.0)
    } else if let Some(stripped) = s.strip_suffix('K') {
        (stripped, 1024.0)
    } else if let Some(stripped) = s.strip_suffix("KB") {
        (stripped, 1024.0)
    } else if let Some(stripped) = s.strip_suffix('B') {
        (stripped, 1.0)
    } else {
        (s.as_str(), 1.0)
    };

    num_part.parse::<f64>().ok().map(|n| (n * unit) as u64)
}

/// Parses Madokami datetime format "YYYY-MM-DD HH:MM" into Unix epoch seconds.
pub fn parse_madokami_datetime(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.trim().split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();

    if date_parts.len() != 3 || time_parts.len() != 2 {
        return None;
    }

    let year = date_parts[0].parse::<i64>().ok()?;
    let month = date_parts[1].parse::<i64>().ok()?;
    let day = date_parts[2].parse::<i64>().ok()?;
    let hour = time_parts[0].parse::<i64>().ok()?;
    let min = time_parts[1].parse::<i64>().ok()?;

    // Approximate epoch calculation without external dependencies
    // (Accurate enough for sorting and published_at timestamps)
    let days_before_month = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let leap_years = (year - 1969) / 4 - (year - 1901) / 100 + (year - 1601) / 400;
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let leap_day = if is_leap && month > 2 { 1 } else { 0 };

    let total_days = (year - 1970) * 365 + leap_years + days_before_month[(month - 1) as usize] + leap_day + (day - 1);
    let epoch = total_days * 86400 + hour * 3600 + min * 60;
    Some(epoch)
}

// -----------------------------------------------------------------------------
// Volume and Chapter Classifier (Section 8, 10, 11, 12, 13)
// -----------------------------------------------------------------------------

/// Classifies a Madokami filename for a target logical volume.
///
/// Returns:
/// `(match_type, parsed_volume, is_digital, is_pre, release_group)`
pub fn classify_madokami_file(
    filename: &str,
    target_volume: Option<f64>,
) -> (
    CandidateMatchType,
    Option<f64>,
    bool,
    bool,
    Option<String>,
) {
    let raw_lower = filename.to_ascii_lowercase();

    // 1. Digital release detection
    let is_digital = raw_lower.contains("digital")
        || raw_lower.contains("(digital)")
        || raw_lower.contains("[digital]")
        || raw_lower.contains("[dig]")
        || raw_lower.contains("web-dl")
        || raw_lower.contains("official");

    // 2. Pre-release detection (PRE)
    let is_pre = raw_lower.contains("(pre)") || raw_lower.contains("[pre]") || raw_lower.contains(" pre ");

    // 3. Extract release group
    let release_group = extract_release_group(filename);

    // 4. Check for multi-volume ranges / series packs
    if is_volume_range_or_pack(&raw_lower) {
        if let Some(target_v) = target_volume {
            if let Some((start, end)) = extract_volume_range(&raw_lower) {
                if target_v >= start && target_v <= end {
                    return (
                        CandidateMatchType::VolumeRangeContainingTarget,
                        None,
                        is_digital,
                        is_pre,
                        release_group,
                    );
                }
            }
        }
        return (
            CandidateMatchType::SeriesPack,
            None,
            is_digital,
            is_pre,
            release_group,
        );
    }

    // 5. Hard chapter detection:
    // If filename has chapter markers or bare numbers without explicit volume token, reject as CHAPTER!
    let clean_for_vol = strip_revision_tokens(filename);
    let has_vol_token = extract_volume_number(&clean_for_vol).is_some();
    let is_chapter = is_explicit_chapter(&raw_lower, has_vol_token);

    if is_chapter && !has_vol_token {
        return (
            CandidateMatchType::Rejected,
            None,
            is_digital,
            is_pre,
            release_group,
        );
    }

    // 6. Volume number extraction
    if let Some(vol) = extract_volume_number(&clean_for_vol) {
        if let Some(target_v) = target_volume {
            if (vol - target_v).abs() < 0.001 {
                if is_pre {
                    (
                        CandidateMatchType::PossibleMatch,
                        Some(vol),
                        is_digital,
                        is_pre,
                        release_group,
                    )
                } else {
                    (
                        CandidateMatchType::ExactVolume,
                        Some(vol),
                        is_digital,
                        is_pre,
                        release_group,
                    )
                }
            } else {
                (
                    CandidateMatchType::Rejected,
                    Some(vol),
                    is_digital,
                    is_pre,
                    release_group,
                )
            }
        } else {
            (
                if is_pre {
                    CandidateMatchType::PossibleMatch
                } else {
                    CandidateMatchType::ExactVolume
                },
                Some(vol),
                is_digital,
                is_pre,
                release_group,
            )
        }
    } else {
        (
            CandidateMatchType::Rejected,
            None,
            is_digital,
            is_pre,
            release_group,
        )
    }
}

/// Strips revision markers like `(v2)`, `[v2]` so they are not parsed as Volume 2.
fn strip_revision_tokens(s: &str) -> String {
    let re_rev = [
        "(v2)", "(v3)", "(v4)", "[v2]", "[v3]", "[v4]",
        "(V2)", "(V3)", "(V4)", "[V2]", "[V3]", "[V4]",
    ];
    let mut out = s.to_string();
    for tok in re_rev {
        out = out.replace(tok, "");
    }
    out
}

/// Strips archive extension (.cbz, .cbr, .zip, etc.)
pub fn strip_archive_extension(s: &str) -> &str {
    for ext in &[".cbz", ".cbr", ".zip", ".rar", ".7z", ".tar"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped;
        }
    }
    s
}

/// Extracts volume number strictly requiring explicit volume prefix (v, vol, volume).
pub fn extract_volume_number(filename: &str) -> Option<f64> {
    let clean = strip_archive_extension(filename);
    let lower = clean.to_ascii_lowercase();

    // Look for explicit volume tokens:
    // v08, v8, vol 8, vol. 8, volume 8, volume 08
    for part in lower.split(|c: char| c == ' ' || c == '_' || c == '-' || c == '(' || c == '[' || c == ')' || c == ']') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(rest) = part.strip_prefix("volume") {
            let rest = rest.trim_start_matches('.').trim();
            if let Ok(v) = rest.parse::<f64>() {
                return Some(v);
            }
        } else if let Some(rest) = part.strip_prefix("vol") {
            let rest = rest.trim_start_matches('.').trim();
            if let Ok(v) = rest.parse::<f64>() {
                return Some(v);
            }
        } else if let Some(rest) = part.strip_prefix('v') {
            let rest = rest.trim_start_matches('.').trim();
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.') {
                if let Ok(v) = rest.parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }

    // Secondary scan for "vol. 8" or "volume 8" or "v 8" across space boundary
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    for i in 0..tokens.len() {
        let t = tokens[i].trim_end_matches('.').trim_end_matches(':');
        if (t == "vol" || t == "volume" || t == "v") && i + 1 < tokens.len() {
            let next = tokens[i + 1].trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
            if let Ok(v) = next.parse::<f64>() {
                return Some(v);
            }
        }
    }

    None
}

/// Checks if filename is an explicit or bare chapter release.
pub fn is_explicit_chapter(lower: &str, has_vol_token: bool) -> bool {
    // Markers: c137, ch137, chapter 137
    if lower.contains("chapter") || lower.contains("!chapters") || lower.contains("(mag)") {
        return true;
    }

    for part in lower.split(|c: char| c == ' ' || c == '_' || c == '-' || c == '(' || c == '[' || c == ')' || c == ']') {
        let part = part.trim();
        if part.starts_with("ch") && part[2..].chars().all(|c| c.is_ascii_digit() || c == '.') && part.len() > 2 {
            return true;
        }
        if part.starts_with('c') && part[1..].chars().all(|c| c.is_ascii_digit() || c == '.') && part.len() > 1 && !has_vol_token {
            return true;
        }
        // Bare numbers with 2 or 3+ digits (e.g. "075", "137") without volume token
        if !has_vol_token && part.len() >= 2 && part.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }

    false
}

/// Checks if filename represents a multi-volume range or series pack.
fn is_volume_range_or_pack(lower: &str) -> bool {
    lower.contains("batch")
        || lower.contains("complete")
        || lower.contains("omnibus")
        || lower.contains("2-in-1")
        || lower.contains("3-in-1")
        || lower.contains("collection")
        || extract_volume_range(lower).is_some()
}

/// Extracts a volume range like (v01-07) or (v1-v10).
fn extract_volume_range(lower: &str) -> Option<(f64, f64)> {
    for part in lower.split(|c: char| c == ' ' || c == '(' || c == '[' || c == ')' || c == ']') {
        if let Some((left, right)) = part.split_once('-') {
            let left_v = left.trim_start_matches('v').trim_start_matches("vol").trim();
            let right_v = right.trim_start_matches('v').trim_start_matches("vol").trim();
            if let (Ok(s), Ok(e)) = (left_v.parse::<f64>(), right_v.parse::<f64>()) {
                if s < e && e - s >= 1.0 {
                    return Some((s, e));
                }
            }
        }
    }
    None
}

/// Extracts release group name from standard parentheses/brackets.
fn extract_release_group(filename: &str) -> Option<String> {
    let mut groups = Vec::new();
    let mut in_paren = false;
    let mut current = String::new();

    for c in filename.chars() {
        if c == '(' || c == '[' {
            in_paren = true;
            current.clear();
        } else if c == ')' || c == ']' {
            if in_paren {
                in_paren = false;
                let trimmed = current.trim();
                let lower = trimmed.to_ascii_lowercase();
                if !trimmed.is_empty()
                    && !lower.contains("digital")
                    && !lower.contains("pre")
                    && !lower.contains("fixed")
                    && !lower.contains("repack")
                    && !lower.starts_with("19")
                    && !lower.starts_with("20")
                    && !trimmed.chars().all(|ch| ch.is_ascii_digit())
                {
                    groups.push(trimmed.to_string());
                }
            }
        } else if in_paren {
            current.push(c);
        }
    }

    groups.last().cloned()
}

// -----------------------------------------------------------------------------
// ReleaseProvider Implementation for Madokami
// -----------------------------------------------------------------------------

pub struct MadokamiReleaseProvider {
    client: MadokamiClient,
    enabled: bool,
}

impl MadokamiReleaseProvider {
    pub fn new(
        http: Client,
        base_url: Option<String>,
        username: Option<String>,
        password: Option<String>,
        enabled: bool,
    ) -> Self {
        let client = MadokamiClient::new(http, base_url, username, password);
        Self { client, enabled }
    }
}

impl ReleaseProvider for MadokamiReleaseProvider {
    fn id(&self) -> &'static str {
        "madokami"
    }

    fn name(&self) -> &'static str {
        "Madokami"
    }

    fn check_health<'a>(&'a self) -> BoxFuture<'a, ProviderHealth> {
        Box::pin(async move {
            if !self.enabled {
                return ProviderHealth {
                    provider_id: "madokami".to_string(),
                    name: "Madokami".to_string(),
                    status: ProviderStatus::Disabled,
                    message: Some("Madokami provider is disabled in Settings".to_string()),
                    last_checked_at: Some(now()),
                };
            }
            self.client.check_health().await
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
            self.client.search_releases(query).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strict_chapter_vs_volume_classification() {
        // Chapters MUST NOT be parsed as volumes
        let (match_type, vol, _, _, _) =
            classify_madokami_file("Kagurabachi 137 (2026) (Digital) (Rillant).cbz", Some(8.0));
        assert_eq!(match_type, CandidateMatchType::Rejected);
        assert_eq!(vol, None);

        let (match_type, vol, _, _, _) =
            classify_madokami_file("Kagurabachi 137 (2026) (Digital) (Rillant).cbz", Some(137.0));
        assert_eq!(match_type, CandidateMatchType::Rejected, "Chapter 137 must not match Volume 137");
        assert_eq!(vol, None);

        let (match_type, _, _, _, _) =
            classify_madokami_file("Kagurabachi 075 (2025) (Digital) (Rillant).cbz", Some(75.0));
        assert_eq!(match_type, CandidateMatchType::Rejected);

        let (match_type, _, _, _, _) =
            classify_madokami_file("Kagurabachi ch137.cbz", Some(137.0));
        assert_eq!(match_type, CandidateMatchType::Rejected);

        let (match_type, _, _, _, _) =
            classify_madokami_file("Kagurabachi Chapter 137.cbz", Some(137.0));
        assert_eq!(match_type, CandidateMatchType::Rejected);

        // Exact Volumes
        let (match_type, vol, is_dig, is_pre, group) =
            classify_madokami_file("Kagurabachi v08 (2026) (Digital) (Rillant).cbz", Some(8.0));
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(8.0));
        assert!(is_dig);
        assert!(!is_pre);
        assert_eq!(group.as_deref(), Some("Rillant"));

        let (match_type, vol, _, _, _) =
            classify_madokami_file("Kagurabachi Vol. 8.cbz", Some(8.0));
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(8.0));

        let (match_type, vol, _, _, _) =
            classify_madokami_file("Kagurabachi Volume 8.cbz", Some(8.0));
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(8.0));

        let (match_type, vol, _, _, _) =
            classify_madokami_file("Kagurabachi v8.cbz", Some(8.0));
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(8.0));
    }

    #[test]
    fn test_pre_releases_classified_as_possible_match_not_exact() {
        let (match_type, vol, _, is_pre, group) = classify_madokami_file(
            "Kagurabachi v01 (2024) (Digital) (1r0n) (PRE).cbz",
            Some(1.0),
        );
        assert_eq!(
            match_type,
            CandidateMatchType::PossibleMatch,
            "PRE release must be PossibleMatch, never ExactVolume"
        );
        assert_eq!(vol, Some(1.0));
        assert!(is_pre);
        assert_eq!(group.as_deref(), Some("1r0n"));
    }

    #[test]
    fn test_revision_tokens_not_confused_with_volume() {
        let (match_type, vol, _, _, _) = classify_madokami_file(
            "Kagurabachi v08 (2026) (Digital) (1r0n) (v2).cbz",
            Some(8.0),
        );
        assert_eq!(match_type, CandidateMatchType::ExactVolume);
        assert_eq!(vol, Some(8.0), "Volume must remain 8, not 2");

        let (match_type, vol, _, _, _) = classify_madokami_file(
            "Kagurabachi v08 (2026) (Digital) (1r0n) (v2).cbz",
            Some(2.0),
        );
        assert_eq!(match_type, CandidateMatchType::Rejected, "Revision v2 must not match Volume 2");
        assert_eq!(vol, Some(8.0));
    }

    #[test]
    fn test_volume_ranges_and_packs() {
        let (match_type, _, _, _, _) = classify_madokami_file(
            "Kagurabachi v01-07,066-120 (2024-2026) (Digital) (LuCaZ).cbz",
            Some(8.0),
        );
        assert_eq!(match_type, CandidateMatchType::SeriesPack);

        let (match_type, _, _, _, _) = classify_madokami_file(
            "Kagurabachi v01-07 (2024-2026) (Digital) (LuCaZ).cbz",
            Some(3.0),
        );
        assert_eq!(match_type, CandidateMatchType::VolumeRangeContainingTarget);
    }

    #[test]
    fn test_parse_madokami_size() {
        assert_eq!(parse_madokami_size("144.3M"), Some((144.3 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_madokami_size("215.0M"), Some((215.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_madokami_size("512K"), Some(512 * 1024));
        assert_eq!(parse_madokami_size("1.5G"), Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_madokami_size("-"), None);
    }

    #[test]
    fn test_parse_madokami_datetime() {
        let epoch = parse_madokami_datetime("2026-08-28 01:03");
        assert!(epoch.is_some());
        assert!(epoch.unwrap() > 1700000000);
    }

    #[test]
    fn test_parse_directory_listing_html() {
        let sample_html = r#"
            <table>
                <tbody>
                    <tr data-record="1">
                        <td><a href="/Manga/K/KA/KAGU/Kagurabachi/%21%28Digital%29%20%5B1r0n%5D">!(Digital) [1r0n]/</a></td>
                        <td>-</td>
                        <td>2026-08-28 01:03</td>
                    </tr>
                    <tr data-record="2">
                        <td><a href="/Manga/K/KA/KAGU/Kagurabachi/Kagurabachi%20v08%20%282026%29%20%28Digital%29%20%28Rillant%29.cbz">Kagurabachi v08 (2026) (Digital) (Rillant).cbz</a></td>
                        <td>144.3M</td>
                        <td>2026-08-28 01:03</td>
                    </tr>
                </tbody>
            </table>
        "#;
        let entries = parse_madokami_directory_listing(sample_html, "/Manga/K/KA/KAGU/Kagurabachi");
        assert_eq!(entries.len(), 2);
        match &entries[0] {
            MadokamiDirEntry::Subdirectory(path, name) => {
                assert!(path.contains("1r0n"));
                assert!(name.contains("1r0n"));
            }
            _ => panic!("Expected subdirectory"),
        }
        match &entries[1] {
            MadokamiDirEntry::File(file) => {
                assert_eq!(file.filename, "Kagurabachi v08 (2026) (Digital) (Rillant).cbz");
                assert!(file.size_bytes.unwrap() > 100_000_000);
            }
            _ => panic!("Expected file"),
        }
    }
}
