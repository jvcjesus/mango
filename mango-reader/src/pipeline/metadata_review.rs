//! Metadata Review and Catalog Disambiguation Queue (Milestone v1.5).
//!
//! Provides proactive scanning, classification, and one-click resolution for:
//! - Ambiguous MangaBaka work identities
//! - Conflicting multi-edition series designations (Standard vs Omnibus/Deluxe)
//! - Tracker vs official catalog volume count discrepancies
//! - Unlinked catalog-native manga records

use crate::db::now;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataReviewItem {
    pub id: String,
    pub manga_id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub issue_type: String, // "ambiguous_identity", "edition_conflict", "tracker_mismatch", "missing_mangabaka"
    pub status: String,     // "pending", "resolved", "dismissed"
    pub reason: String,
    pub mangabaka_id: Option<i64>,
    pub current_edition: Option<String>,
    pub suggested_mangabaka_id: Option<i64>,
    pub suggested_edition: Option<String>,
    pub details: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveReviewRequest {
    pub action: String, // "link_mangabaka", "set_edition", "dismiss"
    pub mangabaka_id: Option<i64>,
    pub edition: Option<String>,
}

/// Scans the operational database for all titles requiring metadata review or disambiguation.
pub fn get_metadata_review_items(conn: &Connection) -> Result<Vec<MetadataReviewItem>> {
    let mut items = Vec::new();
    let ts = now();

    // 1. Scan series_reconciliations for review_required or explicit review_reason
    let mut stmt = conn.prepare(
        "SELECT r.manga_id, r.title, r.mangabaka_id, r.edition, r.status, r.review_reason,
                m.cover_path, m.source_id, m.in_library
         FROM series_reconciliations r
         LEFT JOIN manga m ON m.id = r.manga_id
         WHERE (r.status = 'review_required' OR r.review_reason IS NOT NULL)
           AND r.manga_id IS NOT NULL",
    )?;

    let recon_rows = stmt.query_map([], |row| {
        let manga_id: i64 = row.get(0)?;
        let title: String = row.get(1)?;
        let mangabaka_id: Option<i64> = row.get(2)?;
        let edition: String = row.get(3)?;
        let status: String = row.get(4)?;
        let review_reason: Option<String> = row.get(5)?;
        let cover_path: Option<String> = row.get(6)?;

        let reason_text = review_reason.unwrap_or_else(|| {
            if status == "review_required" {
                "Series requires identity or metadata review".to_string()
            } else {
                "Unknown metadata review condition".to_string()
            }
        });

        let issue_type = if reason_text.to_lowercase().contains("edition")
            || reason_text.to_lowercase().contains("omnibus")
        {
            "edition_conflict".to_string()
        } else if reason_text.to_lowercase().contains("tracker")
            || reason_text.to_lowercase().contains("mismatch")
        {
            "tracker_mismatch".to_string()
        } else {
            "ambiguous_identity".to_string()
        };

        Ok(MetadataReviewItem {
            id: format!("rev_{manga_id}"),
            manga_id,
            title,
            cover_url: cover_path,
            issue_type,
            status: "pending".to_string(),
            reason: reason_text,
            mangabaka_id,
            current_edition: Some(edition),
            suggested_mangabaka_id: None,
            suggested_edition: None,
            details: serde_json::json!({
                "source": "series_reconciliations",
                "reconciliationStatus": status,
            }),
            created_at: ts,
        })
    })?;

    for r in recon_rows {
        items.push(r?);
    }

    // 2. Scan catalog manga (source_id = 0) with in_library = 1 that lack a MangaBaka ID
    let mut stmt_missing = conn.prepare(
        "SELECT m.id, m.title, m.cover_path
         FROM manga m
         WHERE m.source_id = 0
           AND m.in_library = 1
           AND m.mangabaka_id IS NULL",
    )?;

    let missing_rows = stmt_missing.query_map([], |row| {
        let manga_id: i64 = row.get(0)?;
        let title: String = row.get(1)?;
        let cover_path: Option<String> = row.get(2)?;

        Ok(MetadataReviewItem {
            id: format!("rev_missing_{manga_id}"),
            manga_id,
            title: title.clone(),
            cover_url: cover_path,
            issue_type: "missing_mangabaka".to_string(),
            status: "pending".to_string(),
            reason: format!("Catalog title '{title}' has no associated MangaBaka ID"),
            mangabaka_id: None,
            current_edition: Some("standard".to_string()),
            suggested_mangabaka_id: None,
            suggested_edition: None,
            details: serde_json::json!({
                "source": "catalog_manga_unlinked",
            }),
            created_at: ts,
        })
    })?;

    for r in missing_rows {
        let item = r?;
        if !items.iter().any(|existing| existing.manga_id == item.manga_id) {
            items.push(item);
        }
    }

    items.sort_by(|a, b| a.manga_id.cmp(&b.manga_id));
    Ok(items)
}

/// Resolves a metadata review item by updating manga identity, edition, or dismissing warnings.
pub fn resolve_metadata_review_item(
    conn: &Connection,
    manga_id: i64,
    req: &ResolveReviewRequest,
) -> Result<MetadataReviewItem> {
    // Verify manga exists
    let title: String = conn
        .query_row(
            "SELECT title FROM manga WHERE id = ?1",
            [manga_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("Manga '{manga_id}' not found")))?;

    match req.action.as_str() {
        "link_mangabaka" => {
            let mb_id = req
                .mangabaka_id
                .ok_or_else(|| AppError::bad_request("mangabaka_id is required for link_mangabaka action"))?;

            conn.execute(
                "UPDATE manga SET mangabaka_id = ?1 WHERE id = ?2",
                params![mb_id, manga_id],
            )?;

            conn.execute(
                "UPDATE series_reconciliations
                 SET mangabaka_id = ?1,
                     status = 'up_to_date',
                     review_reason = NULL,
                     updated_at = ?3
                 WHERE manga_id = ?2",
                params![mb_id, manga_id, now()],
            )?;
        }
        "set_edition" => {
            let ed = req
                .edition
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("standard");

            conn.execute(
                "UPDATE series_reconciliations
                 SET edition = ?1,
                     review_reason = NULL,
                     updated_at = ?3
                 WHERE manga_id = ?2",
                params![ed, manga_id, now()],
            )?;
        }
        "dismiss" => {
            conn.execute(
                "UPDATE series_reconciliations
                 SET status = 'up_to_date',
                     review_reason = NULL,
                     updated_at = ?2
                 WHERE manga_id = ?1",
                params![manga_id, now()],
            )?;
        }
        other => {
            return Err(AppError::bad_request(format!("Unknown review resolution action '{other}'")));
        }
    }

    Ok(MetadataReviewItem {
        id: format!("rev_{manga_id}"),
        manga_id,
        title,
        cover_url: None,
        issue_type: "resolved".to_string(),
        status: "resolved".to_string(),
        reason: format!("Resolved via '{}'", req.action),
        mangabaka_id: req.mangabaka_id,
        current_edition: req.edition.clone(),
        suggested_mangabaka_id: None,
        suggested_edition: None,
        details: serde_json::json!({ "action": req.action }),
        created_at: now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v1_5_metadata_review_scan_and_resolve() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE manga (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                cover_path TEXT,
                source_id INTEGER NOT NULL DEFAULT 0,
                in_library INTEGER NOT NULL DEFAULT 1,
                mangabaka_id INTEGER
            );
            CREATE TABLE series_reconciliations (
                series_id TEXT PRIMARY KEY,
                manga_id INTEGER,
                title TEXT NOT NULL,
                mangabaka_id INTEGER,
                edition TEXT NOT NULL DEFAULT 'standard',
                status TEXT NOT NULL,
                review_reason TEXT,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        // 1. Insert an ambiguous series
        conn.execute(
            "INSERT INTO manga (id, title, in_library, mangabaka_id) VALUES (1, 'Berserk', 1, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO series_reconciliations (series_id, manga_id, title, mangabaka_id, edition, status, review_reason, updated_at)
             VALUES ('ser_1', 1, 'Berserk', NULL, 'standard', 'review_required', 'Ambiguous MangaBaka identity: 3 candidates', 1000)",
            [],
        )
        .unwrap();

        // 2. Insert a catalog manga missing mangabaka_id
        conn.execute(
            "INSERT INTO manga (id, title, source_id, in_library, mangabaka_id) VALUES (2, 'Vinland Saga', 0, 1, NULL)",
            [],
        )
        .unwrap();

        // Scan items
        let items = get_metadata_review_items(&conn).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].manga_id, 1);
        assert_eq!(items[0].issue_type, "ambiguous_identity");
        assert_eq!(items[1].manga_id, 2);
        assert_eq!(items[1].issue_type, "missing_mangabaka");

        // Resolve item 1 by linking MangaBaka ID 83
        let resolved = resolve_metadata_review_item(
            &conn,
            1,
            &ResolveReviewRequest {
                action: "link_mangabaka".to_string(),
                mangabaka_id: Some(83),
                edition: None,
            },
        )
        .unwrap();
        assert_eq!(resolved.status, "resolved");

        // Verify DB update
        let (mb_id, status, reason): (Option<i64>, String, Option<String>) = conn
            .query_row(
                "SELECT mangabaka_id, status, review_reason FROM series_reconciliations WHERE manga_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(mb_id, Some(83));
        assert_eq!(status, "up_to_date");
        assert!(reason.is_none());

        // Resolve item 2 by linking MangaBaka ID 105
        resolve_metadata_review_item(
            &conn,
            2,
            &ResolveReviewRequest {
                action: "link_mangabaka".to_string(),
                mangabaka_id: Some(105),
                edition: None,
            },
        )
        .unwrap();

        // Scan again: items should be empty
        let items_after = get_metadata_review_items(&conn).unwrap();
        assert_eq!(items_after.len(), 0);
    }
}
