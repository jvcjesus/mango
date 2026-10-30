use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTomeVolumeRelease {
    pub volume_number: f64,
    pub title: Option<String>,
    pub release_date: Option<String>,
    pub release_date_raw: Option<String>,
    pub precision: String, // "day", "month", "year", "unknown"
    pub date_type: String, // "published", "projected", "unknown"
    pub status: String,    // "released", "releases_today", "future", "unknown"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTomeMarketSchedule {
    pub market: String, // "ja" or "en"
    pub line_name: String,
    pub publisher: Option<String>,
    pub tome_id: String,
    pub gcd_series_id: i64,
    pub latest_volume: Option<OpenTomeVolumeRelease>,
    pub next_volume: Option<OpenTomeVolumeRelease>,
    pub total_volumes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTomeSeriesSchedule {
    pub work_id: String,
    pub mapping_confidence: String, // "exact", "high", "ambiguous", "user_confirmed", "unmatched", "rejected"
    pub japanese: Option<OpenTomeMarketSchedule>,
    pub english: Option<OpenTomeMarketSchedule>,
    pub last_evaluated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTomeWorkCandidate {
    pub work_id: String,
    pub primary_name: String,
    pub matched_name: String,
    pub publishers: Vec<String>,
    pub japanese_volume_count: i64,
    pub english_volume_count: i64,
    pub anilist_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct OpenTomeReleaseLine {
    pub gcd_series_id: i64,
    pub name: String,
    pub language: String,
    pub is_main: bool,
    pub is_omnibus: bool,
    pub volume_count: i64,
    pub tome_id: String,
    pub tome_work_id: String,
    pub publisher: Option<String>,
    pub local_name: Option<String>,
    pub medium: Option<String>,
    pub anilist_id: Option<i64>,
}

/// Look up OpenTome work ID by AniList media ID.
pub fn find_work_by_anilist_id(conn: &Connection, anilist_id: i64) -> SqlResult<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT tome_work_id FROM series 
         WHERE anilist_id = ?1 AND tome_work_id IS NOT NULL 
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![anilist_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Find all candidate work IDs matching a title or alias.
/// Returns distinct `(work_id, matched_name)`.
pub fn find_works_by_title_or_alias(
    conn: &Connection,
    title: &str,
) -> SqlResult<Vec<(String, String)>> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut results: Vec<(String, String)> = Vec::new();
    let mut seen_work_ids = std::collections::HashSet::new();

    // 1. Exact match on series name
    let mut stmt1 = conn.prepare(
        "SELECT DISTINCT tome_work_id, name FROM series 
         WHERE lower(name) = lower(?1) AND tome_work_id IS NOT NULL",
    )?;
    let mut rows1 = stmt1.query(params![trimmed])?;
    while let Some(row) = rows1.next()? {
        let work_id: String = row.get(0)?;
        let name: String = row.get(1)?;
        if seen_work_ids.insert(work_id.clone()) {
            results.push((work_id, name));
        }
    }

    // 2. Exact match on aliases
    let mut stmt2 = conn.prepare(
        "SELECT DISTINCT s.tome_work_id, a.alias 
         FROM series_alias a 
         JOIN series s ON s.gcd_series_id = a.gcd_series_id 
         WHERE lower(a.alias) = lower(?1) AND s.tome_work_id IS NOT NULL",
    )?;
    let mut rows2 = stmt2.query(params![trimmed])?;
    while let Some(row) = rows2.next()? {
        let work_id: String = row.get(0)?;
        let alias: String = row.get(1)?;
        if seen_work_ids.insert(work_id.clone()) {
            results.push((work_id, alias));
        }
    }

    Ok(results)
}

/// Fetch all release lines belonging to a work ID.
pub fn get_work_lines(conn: &Connection, work_id: &str) -> SqlResult<Vec<OpenTomeReleaseLine>> {
    let mut stmt = conn.prepare(
        "SELECT gcd_series_id, name, language, is_main, is_omnibus, volume_count,
                tome_id, tome_work_id, publisher, local_name, medium, anilist_id
         FROM series
         WHERE tome_work_id = ?1
         ORDER BY volume_count DESC",
    )?;
    let rows = stmt.query_map(params![work_id], |r| {
        Ok(OpenTomeReleaseLine {
            gcd_series_id: r.get(0)?,
            name: r.get(1)?,
            language: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            is_main: r.get::<_, i64>(3)? != 0,
            is_omnibus: r.get::<_, i64>(4)? != 0,
            volume_count: r.get(5)?,
            tome_id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            tome_work_id: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            publisher: r.get(8)?,
            local_name: r.get(9)?,
            medium: r.get(10)?,
            anilist_id: r.get(11)?,
        })
    })?;

    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// Pick the primary release line for a given market language ('ja' or 'en').
pub fn pick_main_line(
    lines: &[OpenTomeReleaseLine],
    target_lang: &str,
) -> Option<OpenTomeReleaseLine> {
    let lang_lines: Vec<&OpenTomeReleaseLine> = lines
        .iter()
        .filter(|l| l.language.eq_ignore_ascii_case(target_lang))
        .collect();

    if lang_lines.is_empty() {
        return None;
    }

    // Tier 1: is_main, medium == "manga", not omnibus
    let mut c1: Vec<&OpenTomeReleaseLine> = lang_lines
        .iter()
        .filter(|l| {
            l.is_main
                && !l.is_omnibus
                && l.medium.as_deref().unwrap_or("manga").eq_ignore_ascii_case("manga")
        })
        .copied()
        .collect();
    if !c1.is_empty() {
        c1.sort_by(|a, b| b.volume_count.cmp(&a.volume_count));
        return Some(c1[0].clone());
    }

    // Tier 2: is_main, not omnibus
    let mut c2: Vec<&OpenTomeReleaseLine> = lang_lines
        .iter()
        .filter(|l| l.is_main && !l.is_omnibus)
        .copied()
        .collect();
    if !c2.is_empty() {
        c2.sort_by(|a, b| b.volume_count.cmp(&a.volume_count));
        return Some(c2[0].clone());
    }

    // Tier 3: is_main
    let mut c3: Vec<&OpenTomeReleaseLine> = lang_lines
        .iter()
        .filter(|l| l.is_main)
        .copied()
        .collect();
    if !c3.is_empty() {
        c3.sort_by(|a, b| b.volume_count.cmp(&a.volume_count));
        return Some(c3[0].clone());
    }

    // Tier 4: highest volume count non-omnibus
    let mut c4: Vec<&OpenTomeReleaseLine> = lang_lines
        .iter()
        .filter(|l| !l.is_omnibus)
        .copied()
        .collect();
    if !c4.is_empty() {
        c4.sort_by(|a, b| b.volume_count.cmp(&a.volume_count));
        return Some(c4[0].clone());
    }

    // Fallback: highest volume count line
    let mut fallback = lang_lines;
    fallback.sort_by(|a, b| b.volume_count.cmp(&a.volume_count));
    Some(fallback[0].clone())
}

/// Compute the schedule for a specific release line.
pub fn get_line_schedule(
    conn: &Connection,
    line: &OpenTomeReleaseLine,
    today_str: &str,
) -> SqlResult<OpenTomeMarketSchedule> {
    let mut stmt = conn.prepare(
        "SELECT volume_number, title, release_date, release_date_precision, release_date_type, release_date_raw
         FROM volumes
         WHERE gcd_series_id = ?1
         ORDER BY volume_number ASC",
    )?;

    let rows = stmt.query_map(params![line.gcd_series_id], |r| {
        let vnum: f64 = r.get::<_, i64>(0)? as f64;
        let title: Option<String> = r.get(1)?;
        let rdate: Option<String> = r.get(2)?;
        let prec: Option<String> = r.get(3)?;
        let dtype: Option<String> = r.get(4)?;
        let raw: Option<String> = r.get(5)?;
        Ok((vnum, title, rdate, prec, dtype, raw))
    })?;

    let mut latest_released: Option<OpenTomeVolumeRelease> = None;
    let mut next_upcoming: Option<OpenTomeVolumeRelease> = None;
    let mut total_vols: i64 = 0;

    let today_prefix_month = if today_str.len() >= 7 {
        &today_str[..7]
    } else {
        today_str
    };
    let today_prefix_year = if today_str.len() >= 4 {
        &today_str[..4]
    } else {
        today_str
    };

    for row_res in rows {
        let (vnum, title, rdate, prec_opt, dtype_opt, raw_opt) = row_res?;
        total_vols += 1;

        let prec = prec_opt.unwrap_or_else(|| "unknown".to_string());
        let dtype = dtype_opt.unwrap_or_else(|| "unknown".to_string());

        let status = match prec.as_str() {
            "day" => match rdate.as_deref() {
                Some(d) if d < today_str => "released",
                Some(d) if d == today_str => "releases_today",
                Some(d) if d > today_str => "future",
                _ => "unknown",
            },
            "month" => match raw_opt.as_deref() {
                Some(m) if m < today_prefix_month => "released",
                Some(m) if m >= today_prefix_month => "future",
                _ => "unknown",
            },
            "year" => match raw_opt.as_deref() {
                Some(y) if y < today_prefix_year => "released",
                Some(y) if y >= today_prefix_year => "future",
                _ => "unknown",
            },
            _ => match dtype.as_str() {
                "published" => "released",
                "projected" => "future",
                _ => "unknown",
            },
        };

        let release_info = OpenTomeVolumeRelease {
            volume_number: vnum,
            title,
            release_date: rdate.clone(),
            release_date_raw: raw_opt.or(rdate),
            precision: prec,
            date_type: dtype,
            status: status.to_string(),
        };

        if status == "released" || status == "releases_today" {
            if latest_released.as_ref().map_or(true, |l| vnum > l.volume_number) {
                latest_released = Some(release_info.clone());
            }
        }

        if status == "releases_today" || status == "future" || status == "unknown" {
            let is_strictly_after_latest = latest_released
                .as_ref()
                .map_or(true, |l| vnum > l.volume_number || (vnum == l.volume_number && status == "releases_today"));
            if is_strictly_after_latest {
                if next_upcoming.as_ref().map_or(true, |n| vnum < n.volume_number) {
                    next_upcoming = Some(release_info);
                }
            }
        }
    }

    Ok(OpenTomeMarketSchedule {
        market: line.language.clone(),
        line_name: line.name.clone(),
        publisher: line.publisher.clone(),
        tome_id: line.tome_id.clone(),
        gcd_series_id: line.gcd_series_id,
        latest_volume: latest_released,
        next_volume: next_upcoming,
        total_volumes: total_vols,
    })
}

/// Evaluate full schedule for a work.
pub fn evaluate_work_schedule(
    conn: &Connection,
    work_id: &str,
    confidence: &str,
    today_str: &str,
) -> SqlResult<OpenTomeSeriesSchedule> {
    let lines = get_work_lines(conn, work_id)?;

    let ja_line = pick_main_line(&lines, "ja");
    let en_line = pick_main_line(&lines, "en");

    let ja_schedule = if let Some(ja) = ja_line {
        Some(get_line_schedule(conn, &ja, today_str)?)
    } else {
        None
    };

    let en_schedule = if let Some(en) = en_line {
        Some(get_line_schedule(conn, &en, today_str)?)
    } else {
        None
    };

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    Ok(OpenTomeSeriesSchedule {
        work_id: work_id.to_string(),
        mapping_confidence: confidence.to_string(),
        japanese: ja_schedule,
        english: en_schedule,
        last_evaluated_at: now_epoch,
    })
}
