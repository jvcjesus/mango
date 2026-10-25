//! Nyaa Release Discovery Provider
//!
//! Provides manga-only release search, RSS XML parsing, anime false-positive rejection,
//! conservative volume/chapter classification, and deterministic candidate ranking.

use crate::acquisition::{AcquisitionCandidate, AcquisitionRelease, AcquisitionReleaseUnit};
use crate::error::{AppError, Result};
use reqwest::Client;

/// Default Nyaa base URL.
pub const DEFAULT_NYAA_URL: &str = "https://nyaa.si";

/// Standard Nyaa BitTorrent trackers for generated magnet links.
pub const NYAA_TRACKERS: &[&str] = &[
    "http://nyaa.tracker.wf:7777/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
];

/// Known anime, video, audio, and non-manga indicators to reject false-positives.
const ANIME_REJECT_TOKENS: &[&str] = &[
    "1080p", "720p", "480p", "2160p", "4k", "hevc", "x264", "x265",
    "h.264", "h264", "h.265", "h265", "avc", "aac", "flac", "opus",
    "subbed", "dual audio", "multi-audio", "bdrip", "web-dl", "mkv",
    "mp4", "avi", "season", "episode", "ost", "soundtrack", "live action",
    "batch [1080p]", "batch [720p]", "dvdrip", "remux", "bluray", "blu-ray",
];

pub struct NyaaClient {
    http: Client,
    base_url: String,
}

impl NyaaClient {
    pub fn new(http: Client, base_url: Option<String>) -> Self {
        let base_url = base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_NYAA_URL.to_string());
        Self { http, base_url }
    }

    /// Searches Nyaa RSS for manga literature releases matching the query.
    pub async fn search(
        &self,
        query: &str,
        category: Option<&str>,
    ) -> Result<Vec<AcquisitionRelease>> {
        let cat = category.unwrap_or("3_1"); // 3_1 is Manga / Literature - English
        let encoded_query = url_encode(query);
        let url = format!("{}/?page=rss&c={}&f=0&q={}", self.base_url, cat, encoded_query);

        let resp = self
            .http
            .get(&url)
            .header("User-Agent", "Mango/1.0 (Manga Acquisition)")
            .send()
            .await
            .map_err(|e| AppError::new(502, "bad_gateway", format!("Failed to query Nyaa RSS: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "remote_error",
                format!("Nyaa returned HTTP status {}", resp.status()),
            ));
        }

        let xml = resp
            .text()
            .await
            .map_err(|e| AppError::new(502, "bad_gateway", format!("Failed to read Nyaa RSS body: {e}")))?;

        Ok(parse_nyaa_rss(&xml))
    }
}

/// Helper function to URL-encode strings.
pub fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

/// Parses a size string (e.g., "123.4 MiB", "1.2 GiB", "500 KiB", "100 B") into bytes.
pub fn parse_size(size_str: &str) -> u64 {
    let s = size_str.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.is_empty() {
        return 0;
    }

    let number: f64 = parts[0].parse().unwrap_or(0.0);
    if parts.len() == 1 {
        return number as u64;
    }

    let unit = parts[1].to_ascii_lowercase();
    let multiplier: f64 = if unit.starts_with("gib") || unit.starts_with("gb") {
        1024.0 * 1024.0 * 1024.0
    } else if unit.starts_with("mib") || unit.starts_with("mb") {
        1024.0 * 1024.0
    } else if unit.starts_with("kib") || unit.starts_with("kb") {
        1024.0
    } else if unit.starts_with("tib") || unit.starts_with("tb") {
        1024.0 * 1024.0 * 1024.0 * 1024.0
    } else {
        1.0
    };

    (number * multiplier) as u64
}

/// Checks whether a title or category indicates an anime, video, audio, or non-manga release.
pub fn is_anime_false_positive(title: &str, category_id: Option<&str>) -> bool {
    // If category is provided and does not start with 3_ (Literature/Manga), reject!
    if let Some(cat) = category_id {
        if !cat.starts_with("3_") && !cat.starts_with("3") {
            return true;
        }
    }

    let lower = title.to_ascii_lowercase();

    for token in ANIME_REJECT_TOKENS {
        if lower.contains(token) {
            return true;
        }
    }

    // Check for patterns like " - 01 [", " - 12 (", " s01", " s02"
    if contains_anime_season_or_episode(&lower) {
        return true;
    }

    false
}

fn contains_anime_season_or_episode(lower: &str) -> bool {
    // Check for " s01", " s02", " s1 ", " s2 " when preceded by boundary
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    for (i, &w) in words.iter().enumerate() {
        if w.len() == 3 && w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()) {
            return true; // e.g. "s01", "s02"
        }
        if w == "season" && i + 1 < words.len() && words[i + 1].chars().all(|c| c.is_ascii_digit()) {
            return true; // e.g. "season 2"
        }
        if (w == "ep" || w == "episode") && i + 1 < words.len() && words[i + 1].chars().all(|c| c.is_ascii_digit()) {
            return true; // e.g. "episode 12"
        }
        if w.starts_with("ep") && w.len() > 2 && w[2..].chars().all(|c| c.is_ascii_digit()) {
            return true; // e.g. "ep01"
        }
    }
    false
}

/// Classifies a release title into a conservative AcquisitionReleaseUnit:
/// Volume(N), Chapter(N), Collection { start, end }, or Unknown.
pub fn classify_release(title: &str) -> (AcquisitionReleaseUnit, Option<f64>, Option<(f64, f64)>) {
    let lower = title.to_ascii_lowercase();

    // 1. Check for volume ranges / collections: e.g. "v01-v39", "v01 - v39", "v1-10", "vol. 1-12", "volumes 01-20"
    if let Some((start, end)) = extract_volume_range(&lower) {
        return (
            AcquisitionReleaseUnit::Collection { start, end },
            None,
            Some((start, end)),
        );
    }

    // 2. Check for single volume: e.g. "v01", "v1", "vol 1", "vol. 1", "volume 1", "volume 01", "[v01]", "(v01)"
    if let Some(vol) = extract_single_volume(&lower) {
        return (
            AcquisitionReleaseUnit::Volume(vol),
            Some(vol),
            None,
        );
    }

    // 3. Check for single chapter: e.g. "c01", "c001", "ch 1", "ch. 1", "chapter 1", "chapter 01"
    if let Some(ch) = extract_single_chapter(&lower) {
        return (
            AcquisitionReleaseUnit::Chapter(ch),
            Some(ch),
            None,
        );
    }

    (AcquisitionReleaseUnit::Unknown, None, None)
}

fn extract_volume_range(s: &str) -> Option<(f64, f64)> {
    // Look for patterns like "v01-v39", "v1-10", "vol 1-5", "vol. 1-12", "volume 01-20"
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Match prefix "volumes", "volume", "vol.", "vol", "v"
        let prefix_len = if s[i..].starts_with("volumes") {
            7
        } else if s[i..].starts_with("volume") {
            6
        } else if s[i..].starts_with("vol.") {
            4
        } else if s[i..].starts_with("vol") {
            3
        } else if s[i..].starts_with('v') && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            1
        } else {
            i += 1;
            continue;
        };

        let mut curr = i + prefix_len;
        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'.' || bytes[curr] == b'_' || bytes[curr] == b'-') {
            curr += 1;
        }

        // Read first number
        let start_num_begin = curr;
        while curr < len && (bytes[curr].is_ascii_digit() || bytes[curr] == b'.') {
            curr += 1;
        }
        let start_str = &s[start_num_begin..curr];
        if start_str.is_empty() {
            i += 1;
            continue;
        }
        let Ok(start_val) = start_str.parse::<f64>() else {
            i += 1;
            continue;
        };

        // Expect delimiter '-' or '–' or '~' or "to"
        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'_') {
            curr += 1;
        }

        let is_range_delim = if curr < len && (bytes[curr] == b'-' || bytes[curr] == b'~') {
            curr += 1;
            true
        } else if s[curr..].starts_with("to ") {
            curr += 3;
            true
        } else {
            false
        };

        if !is_range_delim {
            i += 1;
            continue;
        }

        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'_') {
            curr += 1;
        }

        // Optional second 'v' or "vol"
        if s[curr..].starts_with("vol.") {
            curr += 4;
        } else if s[curr..].starts_with("vol") {
            curr += 3;
        } else if s[curr..].starts_with('v') {
            curr += 1;
        }

        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'_') {
            curr += 1;
        }

        let end_num_begin = curr;
        while curr < len && (bytes[curr].is_ascii_digit() || bytes[curr] == b'.') {
            curr += 1;
        }
        let end_str = &s[end_num_begin..curr];
        if end_str.is_empty() {
            i += 1;
            continue;
        }
        if let Ok(end_val) = end_str.parse::<f64>() {
            if end_val > start_val {
                return Some((start_val, end_val));
            }
        }

        i += 1;
    }

    None
}

fn extract_single_volume(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let prefix_len = if s[i..].starts_with("volumes") {
            7
        } else if s[i..].starts_with("volume") {
            6
        } else if s[i..].starts_with("vol.") {
            4
        } else if s[i..].starts_with("vol") {
            3
        } else if s[i..].starts_with('v') && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            1
        } else {
            i += 1;
            continue;
        };

        // Ensure prefix was at a boundary
        if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }

        let mut curr = i + prefix_len;
        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'.' || bytes[curr] == b'_' || bytes[curr] == b'#') {
            curr += 1;
        }

        let num_begin = curr;
        while curr < len {
            if bytes[curr].is_ascii_digit() {
                curr += 1;
            } else if bytes[curr] == b'.' && curr + 1 < len && bytes[curr + 1].is_ascii_digit() {
                curr += 2;
            } else {
                break;
            }
        }
        let num_str = &s[num_begin..curr];
        if !num_str.is_empty() {
            if let Ok(val) = num_str.parse::<f64>() {
                // Ensure boundary at end (not followed immediately by range hyphen)
                if curr >= len || (bytes[curr] != b'-' && bytes[curr] != b'~') {
                    return Some(val);
                }
            }
        }

        i += 1;
    }

    None
}

fn extract_single_chapter(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let prefix_len = if s[i..].starts_with("chapters") {
            8
        } else if s[i..].starts_with("chapter") {
            7
        } else if s[i..].starts_with("ch.") {
            3
        } else if s[i..].starts_with("ch") {
            2
        } else if s[i..].starts_with('c') && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            1
        } else {
            i += 1;
            continue;
        };

        if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }

        let mut curr = i + prefix_len;
        while curr < len && (bytes[curr] == b' ' || bytes[curr] == b'.' || bytes[curr] == b'_' || bytes[curr] == b'#') {
            curr += 1;
        }

        let num_begin = curr;
        while curr < len {
            if bytes[curr].is_ascii_digit() {
                curr += 1;
            } else if bytes[curr] == b'.' && curr + 1 < len && bytes[curr + 1].is_ascii_digit() {
                curr += 2;
            } else {
                break;
            }
        }
        let num_str = &s[num_begin..curr];
        if !num_str.is_empty() {
            if let Ok(val) = num_str.parse::<f64>() {
                if curr >= len || (bytes[curr] != b'-' && bytes[curr] != b'~') {
                    return Some(val);
                }
            }
        }

        i += 1;
    }

    None
}

/// Builds a standard BitTorrent magnet link from infoHash and raw title.
pub fn build_magnet(info_hash: &str, title: &str) -> String {
    let clean_hash = info_hash.trim().to_ascii_lowercase();
    let mut magnet = format!("magnet:?xt=urn:btih:{}&dn={}", clean_hash, url_encode(title));
    for tr in NYAA_TRACKERS {
        magnet.push_str("&tr=");
        magnet.push_str(&url_encode(tr));
    }
    magnet
}

/// Robust tag extractor from an XML slice.
fn extract_tag(xml: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{tag_name}>");
    let close_tag = format!("</{tag_name}>");
    let start = xml.find(&open_tag)? + open_tag.len();
    let end = start + xml[start..].find(&close_tag)?;
    let content = &xml[start..end];
    let decoded = content
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    Some(decoded.trim().to_string()).filter(|s| !s.is_empty())
}

/// Robust attribute extractor from a self-closing tag or element (e.g. `<enclosure url="..." />`).
fn extract_attr(xml: &str, tag_name: &str, attr_name: &str) -> Option<String> {
    let start_tag = format!("<{tag_name} ");
    let tag_start = xml.find(&start_tag)? + start_tag.len();
    let tag_end = tag_start + xml[tag_start..].find('>')?;
    let tag_body = &xml[tag_start..tag_end];

    let attr_pattern = format!("{}=\"", attr_name);
    let attr_start = tag_body.find(&attr_pattern)? + attr_pattern.len();
    let attr_end = attr_start + tag_body[attr_start..].find('"')?;
    let val = &tag_body[attr_start..attr_end];
    Some(val.trim().to_string()).filter(|s| !s.is_empty())
}

/// Parses Nyaa RSS XML into normalized AcquisitionRelease items.
pub fn parse_nyaa_rss(xml: &str) -> Vec<AcquisitionRelease> {
    let mut releases = Vec::new();
    let mut cursor = 0;

    while let Some(item_start_rel) = xml[cursor..].find("<item>") {
        let item_start = cursor + item_start_rel + "<item>".len();
        let Some(item_end_rel) = xml[item_start..].find("</item>") else {
            break;
        };
        let item_end = item_start + item_end_rel;
        cursor = item_end + "</item>".len();

        let item_xml = &xml[item_start..item_end];

        let title = extract_tag(item_xml, "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let category_id = extract_tag(item_xml, "nyaa:categoryId");
        let category_name = extract_tag(item_xml, "nyaa:category");

        // Reject anime / video / audio false positives
        if is_anime_false_positive(&title, category_id.as_deref()) {
            continue;
        }

        let link = extract_tag(item_xml, "link").unwrap_or_default();
        let guid = extract_tag(item_xml, "guid").unwrap_or_else(|| link.clone());

        let seeders = extract_tag(item_xml, "nyaa:seeders")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let leechers = extract_tag(item_xml, "nyaa:leechers")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        let info_hash = extract_tag(item_xml, "nyaa:infoHash").map(|h| h.trim().to_ascii_lowercase());
        let size_str = extract_tag(item_xml, "nyaa:size").unwrap_or_default();
        let size_bytes = parse_size(&size_str);

        let is_trusted = extract_tag(item_xml, "nyaa:trusted")
            .map(|s| s.eq_ignore_ascii_case("Yes"))
            .unwrap_or(false);
        let is_remake = extract_tag(item_xml, "nyaa:remake")
            .map(|s| s.eq_ignore_ascii_case("Yes"))
            .unwrap_or(false);

        // Parse optional torrent download URL from enclosure
        let torrent_url = extract_attr(item_xml, "enclosure", "url")
            .or_else(|| if link.ends_with(".torrent") { Some(link.clone()) } else { None });

        // Deterministic magnet link from infoHash
        let magnet_url = info_hash.as_ref().map(|hash| build_magnet(hash, &title));

        // Extract uploader from brackets if present (e.g. "[danke-Empire] ...")
        let uploader = extract_uploader(&title);

        let (release_unit, release_number, release_range) = classify_release(&title);

        let clean_title = clean_release_title(&title);

        let release = AcquisitionRelease {
            source: "nyaa".to_string(),
            source_item_id: guid,
            title: clean_title,
            raw_title: title,
            uploader,
            source_url: link,
            torrent_url,
            magnet_url,
            info_hash,
            category_id,
            category_name,
            size_bytes,
            size_formatted: size_str,
            seeders,
            leechers,
            published_at: None,
            is_trusted,
            is_remake,
            release_unit,
            release_number,
            release_range,
            language: Some("en".to_string()),
        };

        releases.push(release);
    }

    releases
}

fn extract_uploader(title: &str) -> Option<String> {
    let t = title.trim();
    if t.starts_with('[') {
        if let Some(end) = t.find(']') {
            let candidate = &t[1..end];
            if !candidate.is_empty() && !candidate.chars().all(|c| c.is_ascii_digit()) {
                return Some(candidate.trim().to_string());
            }
        }
    }
    None
}

fn clean_release_title(title: &str) -> String {
    let mut s = title.trim();
    // Strip leading group bracket if any: "[group] Title" -> "Title"
    if s.starts_with('[') {
        if let Some(end) = s.find(']') {
            s = s[end + 1..].trim();
        }
    }
    s.to_string()
}

/// Evaluates and deterministically ranks release candidates for a target Mango title.
pub fn rank_candidates(
    releases: Vec<AcquisitionRelease>,
    target_title: &str,
    aliases: &[String],
    target_unit: Option<&AcquisitionReleaseUnit>,
) -> Vec<AcquisitionCandidate> {
    let target_norm = target_title.to_ascii_lowercase();
    let aliases_norm: Vec<String> = aliases.iter().map(|a| a.to_ascii_lowercase()).collect();

    let mut scored: Vec<AcquisitionCandidate> = releases
        .into_iter()
        .map(|rel| {
            let mut score: f64 = 0.0;
            let rel_title_lower = rel.raw_title.to_ascii_lowercase();

            // 1. Title match score
            if rel_title_lower.contains(&target_norm) {
                score += 500.0;
            } else if aliases_norm.iter().any(|a| rel_title_lower.contains(a)) {
                score += 300.0;
            }

            // 2. Unit match score & match_type classification
            let match_type = match (target_unit, &rel.release_unit) {
                (Some(AcquisitionReleaseUnit::Volume(target_v)), AcquisitionReleaseUnit::Volume(v)) => {
                    if (target_v - v).abs() < 0.001 {
                        score += 1000.0;
                        "exact_volume"
                    } else {
                        "related"
                    }
                }
                (Some(AcquisitionReleaseUnit::Volume(target_v)), AcquisitionReleaseUnit::Collection { start, end }) => {
                    if *target_v >= *start && *target_v <= *end {
                        score += 750.0;
                        "collection"
                    } else {
                        "related"
                    }
                }
                (Some(AcquisitionReleaseUnit::Chapter(target_c)), AcquisitionReleaseUnit::Chapter(c)) => {
                    if (target_c - c).abs() < 0.001 {
                        score += 1000.0;
                        "exact_chapter"
                    } else {
                        "related"
                    }
                }
                (None, AcquisitionReleaseUnit::Volume(_)) => {
                    score += 200.0;
                    "volume"
                }
                (None, AcquisitionReleaseUnit::Collection { .. }) => {
                    score += 150.0;
                    "collection"
                }
                (None, AcquisitionReleaseUnit::Chapter(_)) => {
                    score += 100.0;
                    "chapter"
                }
                (_, AcquisitionReleaseUnit::Unknown) => {
                    score -= 50.0;
                    "ambiguous"
                }
                _ => "related",
            };

            // 3. Trusted uploader bonus
            if rel.is_trusted {
                score += 150.0;
            }

            // 4. Seeders bonus (capped at +50)
            let seeder_bonus = (rel.seeders as f64).min(100.0) * 0.5;
            score += seeder_bonus;
            if rel.seeders == 0 {
                score -= 100.0; // Penalty for dead torrent
            }

            // 5. Reasonable file size bonus (30MB to 600MB per volume)
            if matches!(rel.release_unit, AcquisitionReleaseUnit::Volume(_)) {
                let mb = rel.size_bytes as f64 / (1024.0 * 1024.0);
                if (20.0..=800.0).contains(&mb) {
                    score += 50.0;
                }
            }

            // 6. Digital edition tag bonus
            if rel_title_lower.contains("digital") {
                score += 30.0;
            }

            // 7. Remake penalty
            if rel.is_remake {
                score -= 50.0;
            }

            AcquisitionCandidate {
                release: rel,
                score,
                match_type: match_type.to_string(),
                ownership_status: "not_owned".to_string(),
            }
        })
        .collect();

    // Deterministic sort: descending score, then descending seeders, then title
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.release.seeders.cmp(&a.release.seeders))
            .then_with(|| a.release.raw_title.cmp(&b.release.raw_title))
    });

    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NYAA_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <title>Nyaa - Manga Search</title>
    <item>
      <title>[danke-Empire] Berserk v01 (Digital) [Empire]</title>
      <link>https://nyaa.si/view/100001</link>
      <guid isPermaLink="true">https://nyaa.si/view/100001</guid>
      <pubDate>Mon, 15 Jan 2024 12:00:00 +0000</pubDate>
      <nyaa:seeders>45</nyaa:seeders>
      <nyaa:leechers>2</nyaa:leechers>
      <nyaa:infoHash>abcdef0123456789abcdef0123456789abcdef01</nyaa:infoHash>
      <nyaa:size>185.4 MiB</nyaa:size>
      <nyaa:trusted>Yes</nyaa:trusted>
      <nyaa:remake>No</nyaa:remake>
      <nyaa:categoryId>3_1</nyaa:categoryId>
      <nyaa:category>Literature - English</nyaa:category>
    </item>
    <item>
      <title>Berserk v01-v39 (Digital) (danke-Empire)</title>
      <link>https://nyaa.si/view/100002</link>
      <guid isPermaLink="true">https://nyaa.si/view/100002</guid>
      <pubDate>Tue, 16 Jan 2024 14:00:00 +0000</pubDate>
      <nyaa:seeders>120</nyaa:seeders>
      <nyaa:leechers>15</nyaa:leechers>
      <nyaa:infoHash>1122334455667788990011223344556677889900</nyaa:infoHash>
      <nyaa:size>5.8 GiB</nyaa:size>
      <nyaa:trusted>Yes</nyaa:trusted>
      <nyaa:remake>No</nyaa:remake>
      <nyaa:categoryId>3_1</nyaa:categoryId>
      <nyaa:category>Literature - English</nyaa:category>
    </item>
    <item>
      <title>[Erai-raws] Jujutsu Kaisen 2nd Season - 01 [1080p][HEVC]</title>
      <link>https://nyaa.si/view/100003</link>
      <guid isPermaLink="true">https://nyaa.si/view/100003</guid>
      <pubDate>Wed, 17 Jan 2024 16:00:00 +0000</pubDate>
      <nyaa:seeders>500</nyaa:seeders>
      <nyaa:leechers>50</nyaa:leechers>
      <nyaa:infoHash>deadbeefdeadbeefdeadbeefdeadbeefdeadbeef</nyaa:infoHash>
      <nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:trusted>No</nyaa:trusted>
      <nyaa:remake>No</nyaa:remake>
      <nyaa:categoryId>1_2</nyaa:categoryId>
      <nyaa:category>Anime - English</nyaa:category>
    </item>
    <item>
      <title>[1-Shot] Chainsaw Man c001 [ENG]</title>
      <link>https://nyaa.si/view/100004</link>
      <guid isPermaLink="true">https://nyaa.si/view/100004</guid>
      <pubDate>Thu, 18 Jan 2024 18:00:00 +0000</pubDate>
      <nyaa:seeders>12</nyaa:seeders>
      <nyaa:leechers>0</nyaa:leechers>
      <nyaa:infoHash>9988776655443322110099887766554433221100</nyaa:infoHash>
      <nyaa:size>45.2 MiB</nyaa:size>
      <nyaa:trusted>No</nyaa:trusted>
      <nyaa:remake>No</nyaa:remake>
      <nyaa:categoryId>3_1</nyaa:categoryId>
      <nyaa:category>Literature - English</nyaa:category>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn test_scenario_a_nyaa_rss_xml_parsing() {
        let releases = parse_nyaa_rss(SAMPLE_NYAA_RSS);
        // The anime item ([Erai-raws]) must be rejected! 3 items remain.
        assert_eq!(releases.len(), 3);

        let item1 = &releases[0];
        assert_eq!(item1.raw_title, "[danke-Empire] Berserk v01 (Digital) [Empire]");
        assert_eq!(item1.uploader.as_deref(), Some("danke-Empire"));
        assert_eq!(item1.seeders, 45);
        assert_eq!(item1.leechers, 2);
        assert_eq!(item1.info_hash.as_deref(), Some("abcdef0123456789abcdef0123456789abcdef01"));
        assert!(item1.is_trusted);
        assert!(!item1.is_remake);
        assert_eq!(item1.category_id.as_deref(), Some("3_1"));
    }

    #[test]
    fn test_scenario_b_volume_parsing() {
        let cases = vec![
            ("Berserk v01", 1.0),
            ("One Piece Vol 105", 105.0),
            ("Naruto Vol. 22", 22.0),
            ("Bleach Volume 74", 74.0),
            ("[danke-Empire] Chainsaw Man [v05]", 5.0),
            ("Claymore (v12)", 12.0),
            ("Vinland Saga v09.5", 9.5),
        ];

        for (title, expected) in cases {
            let (unit, num, _) = classify_release(title);
            assert_eq!(unit, AcquisitionReleaseUnit::Volume(expected), "Failed on {title}");
            assert_eq!(num, Some(expected), "Failed on number for {title}");
        }
    }

    #[test]
    fn test_scenario_c_chapter_parsing() {
        let cases = vec![
            ("One Piece c1000", 1000.0),
            ("Chainsaw Man ch 98", 98.0),
            ("Jujutsu Kaisen ch. 236", 236.0),
            ("Spy x Family Chapter 85", 85.0),
            ("[Group] Dandadan c001 [ENG]", 1.0),
        ];

        for (title, expected) in cases {
            let (unit, num, _) = classify_release(title);
            assert_eq!(unit, AcquisitionReleaseUnit::Chapter(expected), "Failed on {title}");
            assert_eq!(num, Some(expected), "Failed on number for {title}");
        }
    }

    #[test]
    fn test_scenario_d_magnet_uri_generation() {
        let hash = "abcdef0123456789abcdef0123456789abcdef01";
        let title = "Berserk v01 (Digital)";
        let magnet = build_magnet(hash, title);

        assert!(magnet.starts_with("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01"));
        assert!(magnet.contains("&dn=Berserk+v01+%28Digital%29") || magnet.contains("&dn=Berserk+v01+(Digital)") || magnet.contains("&dn=Berserk%20v01"));
        assert!(magnet.contains("nyaa.tracker.wf"));
        assert!(magnet.contains("open.stealth.si"));
    }

    #[test]
    fn test_scenario_e_size_string_parsing() {
        assert_eq!(parse_size("185.4 MiB"), (185.4 * 1024.0 * 1024.0) as u64);
        assert_eq!(parse_size("5.8 GiB"), (5.8 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(parse_size("500 KiB"), 500 * 1024);
        assert_eq!(parse_size("1024 B"), 1024);
        assert_eq!(parse_size(""), 0);
    }

    #[test]
    fn test_scenario_f_candidate_ranking() {
        let releases = parse_nyaa_rss(SAMPLE_NYAA_RSS);
        let ranked = rank_candidates(
            releases,
            "Berserk",
            &["Berserk".to_string()],
            Some(&AcquisitionReleaseUnit::Volume(1.0)),
        );

        assert!(!ranked.is_empty());
        // Volume 1 must be ranked first because of exact_volume match!
        assert_eq!(ranked[0].match_type, "exact_volume");
        assert_eq!(ranked[0].release.release_unit, AcquisitionReleaseUnit::Volume(1.0));
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn test_scenario_g_reject_anime_false_positives() {
        let anime_titles = vec![
            "[Erai-raws] Jujutsu Kaisen 2nd Season - 01 [1080p][HEVC]",
            "Attack on Titan Season 4 Part 2 [720p]",
            "Bleach Original Soundtrack OST [FLAC]",
            "Death Note Live Action Movie [BDRip]",
            "Demon Slayer S02 - 05 [MKV]",
            "One Piece Episode 1080 [1080p]",
            "Chainsaw Man Dual Audio Multi-Audio [WEBRip]",
            "Naruto Shippuden Batch [1080p]",
        ];

        for t in anime_titles {
            assert!(is_anime_false_positive(t, Some("3_1")), "Failed to reject anime: {t}");
            assert!(is_anime_false_positive(t, Some("1_2")), "Failed to reject non-manga category: {t}");
        }
    }

    #[test]
    fn test_scenario_h_collections_complete_batches() {
        let (unit, num, range) = classify_release("Berserk v01-v39 (Digital) (danke-Empire)");
        assert_eq!(unit, AcquisitionReleaseUnit::Collection { start: 1.0, end: 39.0 });
        assert_eq!(num, None);
        assert_eq!(range, Some((1.0, 39.0)));

        let (unit2, _, range2) = classify_release("Claymore Volumes 01-27 Complete");
        assert_eq!(unit2, AcquisitionReleaseUnit::Collection { start: 1.0, end: 27.0 });
        assert_eq!(range2, Some((1.0, 27.0)));
    }
}
