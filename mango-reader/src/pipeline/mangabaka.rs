//! Native asynchronous client for MangaBaka API enrichment.
//!
//! Enriches manga metadata with verified publication dates, volume summaries,
//! official English publishers, writers, pencillers, and genre tags.

use crate::error::{AppError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub const DEFAULT_MANGABAKA_API: &str = "https://api.mangabaka.org/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MangaBakaEnrichment {
    pub series_id: i64,
    pub collection_id: String,
    pub volume: f64,
    pub release_date: Option<String>,
    pub summary: Option<String>,
    pub pages: Option<i64>,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub genres: Vec<String>,
    pub publisher: Option<String>,
}

#[derive(Clone)]
pub struct MangaBakaClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl MangaBakaClient {
    pub fn new(http: Client, base_url: Option<String>, token: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url.unwrap_or_else(|| DEFAULT_MANGABAKA_API.to_string()),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    fn request_builder(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            format!("{}/{}", self.base_url.trim_end_matches('/'), endpoint.trim_start_matches('/'))
        };

        let mut req = self
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mango/1.0");

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        req
    }

    /// Tests whether MangaBaka is reachable and answering.
    pub async fn test_connection(&self) -> Result<String> {
        let resp = self
            .request_builder(reqwest::Method::GET, "/series/search?q=test")
            .send()
            .await
            .map_err(|e| AppError::new(502, "mangabaka_offline", format!("Cannot reach MangaBaka: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::new(
                resp.status().as_u16(),
                "mangabaka_error",
                format!("MangaBaka answered HTTP {}", resp.status()),
            ));
        }

        Ok("MangaBaka API connection successful".to_string())
    }

    /// Fetches volume enrichment data for a series and volume number.
    ///
    /// Non-fatal: returns Ok(None) if series or volume is not found or ambiguous.
    pub async fn fetch_volume_enrichment(&self, series: &str, volume: f64) -> Result<Option<MangaBakaEnrichment>> {
        let wanted_norm = normalize_title(series);

        // 1. Search series by query
        let resp = match self
            .request_builder(reqwest::Method::GET, "/series/search")
            .query(&[("q", series)])
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                tracing::warn!("MANGABAKA: search returned HTTP {}", r.status());
                return Ok(None);
            }
            Err(e) => {
                tracing::warn!("MANGABAKA: search failed: {e}");
                return Ok(None);
            }
        };

        let body: serde_json::Value = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
        let candidates = body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

        let mut exact_matches = Vec::new();
        for item in &candidates {
            let id = match item.get("id").and_then(|v| v.as_i64()) {
                Some(id) => id,
                None => continue,
            };

            let mut titles = Vec::new();
            for key in ["title", "title_english", "title_romaji", "title_native", "romanized_title", "native_title"] {
                if let Some(t) = item.get(key).and_then(|v| v.as_str()) {
                    titles.push(t);
                }
            }

            if titles.iter().any(|t| normalize_title(t) == wanted_norm) {
                exact_matches.push((id, item.clone()));
            }
        }

        if exact_matches.is_empty() {
            tracing::info!("MANGABAKA: no exact title match for {series:?}");
            return Ok(None);
        }

        // 2. For each exact match, find candidate with English Standard Edition digital collection
        let mut collection_matches = Vec::new();
        for (candidate_id, _) in &exact_matches {
            let collections_url = format!("/series/{candidate_id}/collections");
            let c_resp = match self.request_builder(reqwest::Method::GET, &collections_url).send().await {
                Ok(r) if r.status().is_success() => r,
                _ => continue,
            };

            let c_body: serde_json::Value = match c_resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };

            let collections = c_body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
            for col in collections {
                let language = col
                    .get("language")
                    .and_then(|l| {
                        l.get("iso")
                            .or_else(|| l.get("code"))
                            .or_else(|| l.get("language"))
                            .and_then(|v| v.as_str())
                    })
                    .unwrap_or("")
                    .to_lowercase();

                let edition = col
                    .get("edition")
                    .and_then(|e| e.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_lowercase();

                let medium = col.get("medium").and_then(|m| m.as_str()).unwrap_or("").to_lowercase();

                if (language == "en" || language.starts_with("en")) && edition == "standard edition" && medium == "digital" {
                    if let Some(col_id) = col.get("id").and_then(|v| v.as_str()) {
                        collection_matches.push((*candidate_id, col_id.to_string()));
                    }
                }
            }
        }

        if collection_matches.len() != 1 {
            tracing::info!(
                "MANGABAKA: found {} matching collections for {series:?}; skipping enrichment",
                collection_matches.len()
            );
            return Ok(None);
        }

        let (matched_series_id, matched_collection_id) = collection_matches.remove(0);

        // 3. Fetch normalized series metadata for authors/artists/genres/publishers
        let series_url = format!("/series/{matched_series_id}");
        let s_resp = match self.request_builder(reqwest::Method::GET, &series_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(None),
        };

        let s_body: serde_json::Value = s_resp.json().await.unwrap_or_default();
        let s_data = s_body.get("data").unwrap_or(&s_body);

        let writers: Vec<String> = s_data
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect())
            .unwrap_or_default();

        let pencillers: Vec<String> = s_data
            .get("artists")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect())
            .unwrap_or_default();

        let genres: Vec<String> = s_data
            .get("genres")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| {
                        x.as_str().map(|s| {
                            let mut chars = s.trim().chars();
                            match chars.next() {
                                None => String::new(),
                                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut english_publishers = Vec::new();
        if let Some(pubs) = s_data.get("publishers").and_then(|v| v.as_array()) {
            for p in pubs {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let pub_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
                if !name.is_empty() && pub_type == "english" && !english_publishers.contains(&name.to_string()) {
                    english_publishers.push(name.to_string());
                }
            }
        }
        let publisher = (!english_publishers.is_empty()).then(|| english_publishers.join(", "));

        // 4. Query works in collection to find matching volume number
        let mut works_url = Some(format!("/collections/{matched_collection_id}/works"));
        let mut pages_checked = 0;
        let target_vol_int = volume.round() as i64;

        while let Some(url) = works_url {
            if pages_checked >= 20 {
                break;
            }
            pages_checked += 1;

            let w_resp = match self.request_builder(reqwest::Method::GET, &url).send().await {
                Ok(r) if r.status().is_success() => r,
                _ => break,
            };

            let w_body: serde_json::Value = match w_resp.json().await {
                Ok(v) => v,
                Err(_) => break,
            };

            let works = w_body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
            for work in works {
                let sequence = work.get("sequence_numeric").and_then(|v| v.as_i64());
                if sequence != Some(target_vol_int) {
                    continue;
                }

                let release_date = work.get("release_date").and_then(|v| v.as_str()).map(|s| s.trim().to_string());
                let summary = work.get("description").and_then(|v| {
                    if let Some(s) = v.as_str() {
                        Some(s.trim().to_string())
                    } else if let Some(desc) = v.get("desc").and_then(|d| d.as_str()) {
                        Some(desc.trim().to_string())
                    } else {
                        None
                    }
                });
                let pages = work.get("pages").and_then(|v| v.as_i64()).filter(|p| *p > 0);

                return Ok(Some(MangaBakaEnrichment {
                    series_id: matched_series_id,
                    collection_id: matched_collection_id,
                    volume,
                    release_date,
                    summary,
                    pages,
                    writers,
                    pencillers,
                    genres,
                    publisher,
                }));
            }

            works_url = w_body
                .get("pagination")
                .and_then(|p| p.get("next"))
                .and_then(|n| n.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string());
        }

        tracing::info!("MANGABAKA: volume {volume} not found in collection for {series:?}");
        Ok(None)
    }

    /// Enriches ComicInfo.xml inside an existing CBZ file with MangaBaka metadata.
    pub fn enrich_cbz_archive(cbz_path: &Path, enrichment: &MangaBakaEnrichment) -> Result<bool> {
        let file = File::open(cbz_path)?;
        let mut source = zip::ZipArchive::new(file).map_err(|e| AppError::bad_request(format!("Invalid CBZ {}: {e}", cbz_path.display())))?;

        let mut comicinfo_idx = None;
        for i in 0..source.len() {
            let entry = source.by_index(i).map_err(|e| AppError::bad_request(e.to_string()))?;
            if Path::new(entry.name()).file_name().and_then(|n| n.to_str()).map(|s| s.eq_ignore_ascii_case("ComicInfo.xml")).unwrap_or(false) {
                comicinfo_idx = Some(i);
                break;
            }
        }

        let comicinfo_idx = match comicinfo_idx {
            Some(idx) => idx,
            None => {
                tracing::warn!("MANGABAKA: ComicInfo.xml not found in {}", cbz_path.display());
                return Ok(false);
            }
        };

        let mut existing_xml = String::new();
        source.by_index(comicinfo_idx)?.read_to_string(&mut existing_xml)?;

        let updated_xml = enrich_comic_info_xml(&existing_xml, enrichment)?;

        // Write to temporary file and atomically replace
        let parent = cbz_path.parent().unwrap_or_else(|| Path::new("."));
        let pid = std::process::id();
        let temp_path = parent.join(format!(".{}.mangabaka-{pid}.tmp", cbz_path.file_name().unwrap().to_string_lossy()));

        struct CleanupGuard<'a>(&'a Path);
        impl<'a> Drop for CleanupGuard<'a> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }
        let _guard = CleanupGuard(&temp_path);

        let temp_file = File::create(&temp_path)?;
        let mut dest_zip = zip::ZipWriter::new(temp_file);

        for i in 0..source.len() {
            let mut entry = source.by_index(i)?;
            let options = zip::write::SimpleFileOptions::default().compression_method(entry.compression());
            dest_zip.start_file(entry.name(), options)?;
            if i == comicinfo_idx {
                dest_zip.write_all(updated_xml.as_bytes())?;
            } else {
                std::io::copy(&mut entry, &mut dest_zip)?;
            }
        }
        dest_zip.finish()?;

        // Verify temp zip integrity
        crate::pipeline::publisher::verify_cbz_integrity(&temp_path)?;

        fs::rename(&temp_path, cbz_path)?;
        tracing::info!("MANGABAKA: enriched {}", cbz_path.display());
        Ok(true)
    }
}

pub fn normalize_title(title: &str) -> String {
    title.split_whitespace().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(" ")
}

pub fn enrich_comic_info_xml(xml: &str, enrichment: &MangaBakaEnrichment) -> Result<String> {
    let mut lines = Vec::new();
    let mut inside_comicinfo = false;

    // Tags that MangaBaka can safely enrich
    let managed_tags = ["Year", "Month", "Day", "Summary", "PageCount", "Writer", "Penciller", "Genre", "Publisher", "LanguageISO"];

    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<ComicInfo") {
            inside_comicinfo = true;
            lines.push(line.to_string());
            continue;
        }
        if trimmed.starts_with("</ComicInfo>") {
            inside_comicinfo = false;

            // Append managed tags if not present
            if let Some(date) = &enrichment.release_date {
                if let Some((y, rest)) = date.split_once('-') {
                    if let Some((m, d)) = rest.split_once('-') {
                        lines.push(format!("  <Year>{}</Year>", y.trim()));
                        lines.push(format!("  <Month>{}</Month>", m.trim()));
                        lines.push(format!("  <Day>{}</Day>", d.trim()));
                    }
                }
            }
            if let Some(pages) = enrichment.pages {
                lines.push(format!("  <PageCount>{pages}</PageCount>"));
            }
            if let Some(summary) = &enrichment.summary {
                lines.push(format!("  <Summary>{}</Summary>", crate::downloads::escape(summary)));
            }
            if !enrichment.writers.is_empty() {
                lines.push(format!("  <Writer>{}</Writer>", crate::downloads::escape(&enrichment.writers.join(", "))));
            }
            if !enrichment.pencillers.is_empty() {
                lines.push(format!("  <Penciller>{}</Penciller>", crate::downloads::escape(&enrichment.pencillers.join(", "))));
            }
            if !enrichment.genres.is_empty() {
                lines.push(format!("  <Genre>{}</Genre>", crate::downloads::escape(&enrichment.genres.join(", "))));
            }
            if let Some(publisher) = &enrichment.publisher {
                lines.push(format!("  <Publisher>{}</Publisher>", crate::downloads::escape(publisher)));
            }
            lines.push("  <LanguageISO>en</LanguageISO>".to_string());

            lines.push(line.to_string());
            continue;
        }

        if inside_comicinfo {
            let is_managed = managed_tags.iter().any(|tag| trimmed.starts_with(&format!("<{tag}>")));
            if is_managed {
                continue; // replaced by enriched version
            }
        }
        lines.push(line.to_string());
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_normalization() {
        assert_eq!(normalize_title("  Blue   Lock  "), "blue lock");
        assert_eq!(normalize_title("ONE PIECE"), "one piece");
    }

    #[test]
    fn comic_info_xml_enrichment() {
        let base_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Title>Volume 1</Title>
  <Series>Blue Lock</Series>
  <Volume>1</Volume>
  <Translator>Stick</Translator>
  <Notes>Saved by Mango</Notes>
</ComicInfo>"#;

        let enrichment = MangaBakaEnrichment {
            series_id: 83,
            collection_id: "col-123".into(),
            volume: 1.0,
            release_date: Some("2021-03-16".into()),
            summary: Some("Soccer high school thriller".into()),
            pages: Some(208),
            writers: vec!["Muneyuki Kaneshiro".into()],
            pencillers: vec!["Yuusuke Nomura".into()],
            genres: vec!["Sports".into(), "Shounen".into()],
            publisher: Some("Kodansha Manga".into()),
        };

        let enriched = enrich_comic_info_xml(base_xml, &enrichment).unwrap();
        assert!(enriched.contains("<Series>Blue Lock</Series>"));
        assert!(enriched.contains("<Volume>1</Volume>"));
        assert!(enriched.contains("<Translator>Stick</Translator>"), "Preserves existing Translator");
        assert!(enriched.contains("<Notes>Saved by Mango</Notes>"), "Preserves existing Notes");
        assert!(enriched.contains("<Year>2021</Year>"));
        assert!(enriched.contains("<Month>03</Month>"));
        assert!(enriched.contains("<Day>16</Day>"));
        assert!(enriched.contains("<PageCount>208</PageCount>"));
        assert!(enriched.contains("<Writer>Muneyuki Kaneshiro</Writer>"));
        assert!(enriched.contains("<Penciller>Yuusuke Nomura</Penciller>"));
        assert!(enriched.contains("<Publisher>Kodansha Manga</Publisher>"));
    }
}
