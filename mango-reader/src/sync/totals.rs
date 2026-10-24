//! Authoritative release totals resolution hierarchy.
//!
//! Totals hierarchy:
//! 1. Total Volumes:
//!    - First choice: MangaBaka edition collection total works
//!    - Second choice: AniList media `volumes` total (if > 0)
//!    - Fallback: Unknown (`None`) - NEVER use source chapter counts as volume denominators!
//! 2. Total Chapters:
//!    - First choice: AniList media `chapters` total (if > 0)
//!    - Fallback: Unknown (`None`)

use super::{mapping::get_mapping, model::SyncTarget};
use crate::{
    error::Result,
    pipeline::mangabaka::MangaBakaClient,
    trackers::{Account, anilist},
};
use reqwest::Client;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthoritativeTotals {
    pub total_volumes: Option<u32>,
    pub total_chapters: Option<u32>,
    pub volume_source: Option<String>,
    pub chapter_source: Option<String>,
}

/// Resolves authoritative total volumes and chapters following the strict totals hierarchy.
pub async fn resolve_authoritative_totals(
    conn: &Connection,
    mb_client: &MangaBakaClient,
    http: &Client,
    anilist_account: Option<&Account>,
    komga_series_id: &str,
) -> Result<AuthoritativeTotals> {
    let mut totals = AuthoritativeTotals::default();

    // 1. Check MangaBaka mapping for collection total works (Top priority for volume totals)
    if let Ok(Some(mb_map)) = get_mapping(conn, komga_series_id, SyncTarget::MangaBaka) {
        if let Some(col_id) = &mb_map.edition_id {
            if let Ok(Some(count)) = mb_client.fetch_collection_total_works(col_id).await {
                if count > 0 {
                    totals.total_volumes = Some(count);
                    totals.volume_source = Some("mangabaka_collection".to_string());
                }
            }
        }
    }

    // 2. Check AniList mapping for volumes and chapters
    if let Ok(Some(al_map)) = get_mapping(conn, komga_series_id, SyncTarget::AniList) {
        if let Ok(media_id) = al_map.tracker_id.parse::<i64>() {
            if let Some(acc) = anilist_account {
                if let Ok(Some(entry)) = anilist::find(http, acc, media_id).await {
                    if totals.total_volumes.is_none() && entry.total_volumes > 0 {
                        totals.total_volumes = Some(entry.total_volumes as u32);
                        totals.volume_source = Some("anilist_volumes".to_string());
                    }
                    if entry.total_chapters > 0 {
                        totals.total_chapters = Some(entry.total_chapters as u32);
                        totals.chapter_source = Some("anilist_chapters".to_string());
                    }
                }
            }
        }
    }

    Ok(totals)
}
