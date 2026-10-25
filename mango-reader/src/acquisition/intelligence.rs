//! Local source intelligence, provider capability tracking, and deterministic uploader reliability.

use crate::db::now;
use crate::error::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploaderStats {
    pub valid_acquisitions: u32,
    pub attempted_acquisitions: u32,
    pub reliability_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealthReport {
    pub source_id: String,
    pub name: String,
    pub supports_volumes: bool,
    pub supports_chapters: bool,
    pub supports_collections: bool,
    pub requires_external_backend: bool,
    pub status: String, // "healthy", "degraded", "unavailable"
    pub success_count: u32,
    pub failure_count: u32,
    pub last_response_time_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_success_at: Option<i64>,
    pub exact_match_count: u32,
    pub total_candidates_found: u32,
    pub uploader_stats: HashMap<String, UploaderStats>,
    pub updated_at: i64,
}

pub fn get_all_source_health(conn: &Connection) -> Result<Vec<SourceHealthReport>> {
    let mut stmt = conn.prepare(
        "SELECT source_id, name, supports_volumes, supports_chapters, supports_collections,
                requires_external_backend, status, success_count, failure_count,
                last_response_time_ms, last_error, last_success_at,
                exact_match_count, total_candidates_found, uploader_stats_json, updated_at
         FROM source_intelligence
         ORDER BY source_id ASC",
    )?;

    let rows = stmt.query_map([], |r| {
        let stats_json: String = r.get(14)?;
        let uploader_stats: HashMap<String, UploaderStats> =
            serde_json::from_str(&stats_json).unwrap_or_default();

        let resp_time: Option<i64> = r.get(9)?;

        Ok(SourceHealthReport {
            source_id: r.get(0)?,
            name: r.get(1)?,
            supports_volumes: r.get::<_, i64>(2)? != 0,
            supports_chapters: r.get::<_, i64>(3)? != 0,
            supports_collections: r.get::<_, i64>(4)? != 0,
            requires_external_backend: r.get::<_, i64>(5)? != 0,
            status: r.get(6)?,
            success_count: r.get::<_, u32>(7)?,
            failure_count: r.get::<_, u32>(8)?,
            last_response_time_ms: resp_time.map(|v| v as u64),
            last_error: r.get(10)?,
            last_success_at: r.get(11)?,
            exact_match_count: r.get::<_, u32>(12)?,
            total_candidates_found: r.get::<_, u32>(13)?,
            uploader_stats,
            updated_at: r.get(15)?,
        })
    })?;

    let mut reports = Vec::new();
    for r in rows {
        reports.push(r?);
    }
    Ok(reports)
}

pub fn get_source_health(conn: &Connection, source_id: &str) -> Result<Option<SourceHealthReport>> {
    let reports = get_all_source_health(conn)?;
    Ok(reports.into_iter().find(|s| s.source_id == source_id))
}

pub fn record_discovery_success(
    conn: &Connection,
    source_id: &str,
    candidates_count: usize,
    exact_matches: usize,
    duration_ms: u64,
) -> Result<()> {
    let ts = now();
    conn.execute(
        "UPDATE source_intelligence SET
            status = 'healthy',
            success_count = success_count + 1,
            last_response_time_ms = ?2,
            last_error = NULL,
            last_success_at = ?3,
            total_candidates_found = total_candidates_found + ?4,
            exact_match_count = exact_match_count + ?5,
            updated_at = ?3
         WHERE source_id = ?1",
        params![
            source_id,
            duration_ms as i64,
            ts,
            candidates_count as i64,
            exact_matches as i64
        ],
    )?;
    Ok(())
}

pub fn record_discovery_failure(
    conn: &Connection,
    source_id: &str,
    error: &str,
) -> Result<()> {
    let ts = now();
    conn.execute(
        "UPDATE source_intelligence SET
            status = CASE WHEN failure_count >= 3 THEN 'unavailable' ELSE 'degraded' END,
            failure_count = failure_count + 1,
            last_error = ?2,
            updated_at = ?3
         WHERE source_id = ?1",
        params![source_id, error, ts],
    )?;
    Ok(())
}

pub fn record_acquisition_outcome(
    conn: &Connection,
    source_id: &str,
    uploader: Option<&str>,
    is_success: bool,
) -> Result<()> {
    let ts = now();
    let stats_json_res: rusqlite::Result<String> = conn.query_row(
        "SELECT uploader_stats_json FROM source_intelligence WHERE source_id = ?1",
        [source_id],
        |r| r.get(0),
    );

    let mut stats_map: HashMap<String, UploaderStats> = match stats_json_res {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => HashMap::new(),
    };

    if let Some(u) = uploader {
        let entry = stats_map.entry(u.to_string()).or_insert(UploaderStats {
            valid_acquisitions: 0,
            attempted_acquisitions: 0,
            reliability_percent: 100.0,
        });

        entry.attempted_acquisitions += 1;
        if is_success {
            entry.valid_acquisitions += 1;
        }
        if entry.attempted_acquisitions > 0 {
            entry.reliability_percent =
                (entry.valid_acquisitions as f64 / entry.attempted_acquisitions as f64) * 100.0;
        }
    }

    let updated_json = serde_json::to_string(&stats_map).unwrap_or_else(|_| "{}".to_string());

    conn.execute(
        "UPDATE source_intelligence SET
            uploader_stats_json = ?2,
            updated_at = ?3
         WHERE source_id = ?1",
        params![source_id, updated_json, ts],
    )?;

    Ok(())
}

pub fn get_uploader_reliability(
    conn: &Connection,
    source_id: &str,
    uploader: &str,
) -> Result<Option<f64>> {
    let report_opt = get_source_health(conn, source_id)?;
    if let Some(rep) = report_opt {
        if let Some(stats) = rep.uploader_stats.get(uploader) {
            // Only consider reliability if minimum sample threshold (>= 2) is met
            if stats.attempted_acquisitions >= 2 {
                return Ok(Some(stats.reliability_percent));
            }
        }
    }
    Ok(None)
}
