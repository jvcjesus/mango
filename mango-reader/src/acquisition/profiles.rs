//! Acquisition Profiles domain model and storage.
//!
//! Defines user acquisition preferences, source priorities, quality preferences,
//! budget constraints, and per-series assignments.

use crate::db::now;
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionProfile {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub language: String,
    pub preferred_release_type: String, // "volume", "chapter", "any"
    pub source_selection: String,       // "smart", "manual"
    pub preferred_sources: Vec<String>,
    pub preferred_uploaders: Vec<String>,
    pub min_seeders: u32,
    pub prefer_digital: bool,
    pub trusted_only: bool,
    pub collection_policy: String, // "smart", "prefer_collections", "prefer_individual", "never_collections"
    pub max_unnecessary_duplicate_coverage: u32,
    pub max_acquisition_size_bytes: Option<u64>,
    pub max_plan_size_bytes: Option<u64>,
    pub quality_preference: String, // "best", "balanced", "size"
    pub allow_mixed_collections: bool,
    pub allow_zero_seeders: String, // "no", "ask", "yes"
    pub allow_unsupported_editions: bool,
    pub auto_acquire: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for AcquisitionProfile {
    fn default() -> Self {
        let ts = now();
        Self {
            id: "default".to_string(),
            name: "Default Manga".to_string(),
            is_default: true,
            language: "en".to_string(),
            preferred_release_type: "volume".to_string(),
            source_selection: "smart".to_string(),
            preferred_sources: vec!["nyaa".to_string(), "mangadot".to_string()],
            preferred_uploaders: vec![],
            min_seeders: 1,
            prefer_digital: true,
            trusted_only: false,
            collection_policy: "smart".to_string(),
            max_unnecessary_duplicate_coverage: 3,
            max_acquisition_size_bytes: None,
            max_plan_size_bytes: None,
            quality_preference: "best".to_string(),
            allow_mixed_collections: false,
            allow_zero_seeders: "no".to_string(),
            allow_unsupported_editions: false,
            auto_acquire: false,
            created_at: ts,
            updated_at: ts,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePatch {
    pub name: Option<String>,
    pub is_default: Option<bool>,
    pub language: Option<String>,
    pub preferred_release_type: Option<String>,
    pub source_selection: Option<String>,
    pub preferred_sources: Option<Vec<String>>,
    pub preferred_uploaders: Option<Vec<String>>,
    pub min_seeders: Option<u32>,
    pub prefer_digital: Option<bool>,
    pub trusted_only: Option<bool>,
    pub collection_policy: Option<String>,
    pub max_unnecessary_duplicate_coverage: Option<u32>,
    pub max_acquisition_size_bytes: Option<Option<u64>>,
    pub max_plan_size_bytes: Option<Option<u64>>,
    pub quality_preference: Option<String>,
    pub allow_mixed_collections: Option<bool>,
    pub allow_zero_seeders: Option<String>,
    pub allow_unsupported_editions: Option<bool>,
    pub auto_acquire: Option<bool>,
}

fn row_to_profile(r: &rusqlite::Row) -> rusqlite::Result<AcquisitionProfile> {
    let pref_sources_json: String = r.get(6)?;
    let pref_uploaders_json: String = r.get(7)?;

    let preferred_sources =
        serde_json::from_str(&pref_sources_json).unwrap_or_else(|_| vec!["nyaa".to_string()]);
    let preferred_uploaders =
        serde_json::from_str(&pref_uploaders_json).unwrap_or_default();

    let max_acq_size: Option<i64> = r.get(13)?;
    let max_plan_size: Option<i64> = r.get(14)?;

    Ok(AcquisitionProfile {
        id: r.get(0)?,
        name: r.get(1)?,
        is_default: r.get::<_, i64>(2)? != 0,
        language: r.get(3)?,
        preferred_release_type: r.get(4)?,
        source_selection: r.get(5)?,
        preferred_sources,
        preferred_uploaders,
        min_seeders: r.get::<_, u32>(8)?,
        prefer_digital: r.get::<_, i64>(9)? != 0,
        trusted_only: r.get::<_, i64>(10)? != 0,
        collection_policy: r.get(11)?,
        max_unnecessary_duplicate_coverage: r.get::<_, u32>(12)?,
        max_acquisition_size_bytes: max_acq_size.map(|v| v as u64),
        max_plan_size_bytes: max_plan_size.map(|v| v as u64),
        quality_preference: r.get(15)?,
        allow_mixed_collections: r.get::<_, i64>(16)? != 0,
        allow_zero_seeders: r.get(17)?,
        allow_unsupported_editions: r.get::<_, i64>(18)? != 0,
        auto_acquire: r.get::<_, i64>(19)? != 0,
        created_at: r.get(20)?,
        updated_at: r.get(21)?,
    })
}

const SELECT_PROFILE_FIELDS: &str = "
    id, name, is_default, language, preferred_release_type, source_selection,
    preferred_sources, preferred_uploaders, min_seeders, prefer_digital,
    trusted_only, collection_policy, max_unnecessary_duplicate_coverage,
    max_acquisition_size_bytes, max_plan_size_bytes, quality_preference,
    allow_mixed_collections, allow_zero_seeders, allow_unsupported_editions,
    auto_acquire, created_at, updated_at
";

pub fn list_profiles(conn: &Connection) -> Result<Vec<AcquisitionProfile>> {
    let sql = format!(
        "SELECT {SELECT_PROFILE_FIELDS} FROM acquisition_profiles ORDER BY is_default DESC, name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_profile)?;
    let mut profiles = Vec::new();
    for r in rows {
        profiles.push(r?);
    }
    Ok(profiles)
}

pub fn get_profile_by_id(conn: &Connection, id: &str) -> Result<Option<AcquisitionProfile>> {
    let sql = format!("SELECT {SELECT_PROFILE_FIELDS} FROM acquisition_profiles WHERE id = ?1");
    conn.query_row(&sql, [id], row_to_profile).optional().map_err(Into::into)
}

pub fn get_default_profile(conn: &Connection) -> Result<AcquisitionProfile> {
    let sql = format!(
        "SELECT {SELECT_PROFILE_FIELDS} FROM acquisition_profiles WHERE is_default = 1 LIMIT 1"
    );
    let found = conn.query_row(&sql, [], row_to_profile).optional()?;
    if let Some(p) = found {
        Ok(p)
    } else {
        // Fallback: get first profile or create default in-memory
        let first = list_profiles(conn)?;
        if let Some(p) = first.into_iter().next() {
            Ok(p)
        } else {
            Ok(AcquisitionProfile::default())
        }
    }
}

pub fn get_profile_for_manga(conn: &Connection, manga_id: i64) -> Result<AcquisitionProfile> {
    let assigned_id: Option<String> = conn
        .query_row(
            "SELECT acquisition_profile_id FROM manga WHERE id = ?1",
            [manga_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    if let Some(id) = assigned_id {
        if let Some(prof) = get_profile_by_id(conn, &id)? {
            return Ok(prof);
        }
    }

    get_default_profile(conn)
}

pub fn create_profile(conn: &Connection, mut profile: AcquisitionProfile) -> Result<AcquisitionProfile> {
    if profile.id.trim().is_empty() {
        profile.id = format!("prof_{}", now());
    }
    let ts = now();
    profile.created_at = ts;
    profile.updated_at = ts;
    // Safety: auto-acquire must remain off in this milestone
    profile.auto_acquire = false;

    if profile.is_default {
        conn.execute("UPDATE acquisition_profiles SET is_default = 0", [])?;
    }

    let pref_sources_json = serde_json::to_string(&profile.preferred_sources)
        .map_err(|e| AppError::bad_request(format!("invalid preferred_sources: {e}")))?;
    let pref_uploaders_json = serde_json::to_string(&profile.preferred_uploaders)
        .map_err(|e| AppError::bad_request(format!("invalid preferred_uploaders: {e}")))?;

    conn.execute(
        "INSERT INTO acquisition_profiles (
            id, name, is_default, language, preferred_release_type, source_selection,
            preferred_sources, preferred_uploaders, min_seeders, prefer_digital,
            trusted_only, collection_policy, max_unnecessary_duplicate_coverage,
            max_acquisition_size_bytes, max_plan_size_bytes, quality_preference,
            allow_mixed_collections, allow_zero_seeders, allow_unsupported_editions,
            auto_acquire, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10,
            ?11, ?12, ?13,
            ?14, ?15, ?16,
            ?17, ?18, ?19,
            0, ?20, ?21
        )",
        params![
            profile.id,
            profile.name,
            if profile.is_default { 1 } else { 0 },
            profile.language,
            profile.preferred_release_type,
            profile.source_selection,
            pref_sources_json,
            pref_uploaders_json,
            profile.min_seeders,
            if profile.prefer_digital { 1 } else { 0 },
            if profile.trusted_only { 1 } else { 0 },
            profile.collection_policy,
            profile.max_unnecessary_duplicate_coverage,
            profile.max_acquisition_size_bytes.map(|v| v as i64),
            profile.max_plan_size_bytes.map(|v| v as i64),
            profile.quality_preference,
            if profile.allow_mixed_collections { 1 } else { 0 },
            profile.allow_zero_seeders,
            if profile.allow_unsupported_editions { 1 } else { 0 },
            profile.created_at,
            profile.updated_at,
        ],
    )?;

    Ok(profile)
}

pub fn update_profile(conn: &Connection, id: &str, patch: ProfilePatch) -> Result<AcquisitionProfile> {
    let mut current = get_profile_by_id(conn, id)?
        .ok_or_else(|| AppError::not_found(format!("Profile '{id}' not found")))?;

    if let Some(name) = patch.name {
        current.name = name;
    }
    if let Some(is_default) = patch.is_default {
        if is_default && !current.is_default {
            conn.execute("UPDATE acquisition_profiles SET is_default = 0", [])?;
            current.is_default = true;
        } else if !is_default && current.is_default {
            // Cannot unset default if it is the only one
            return Err(AppError::bad_request("Cannot unset default profile without designating another"));
        }
    }
    if let Some(lang) = patch.language {
        current.language = lang;
    }
    if let Some(prt) = patch.preferred_release_type {
        current.preferred_release_type = prt;
    }
    if let Some(ss) = patch.source_selection {
        current.source_selection = ss;
    }
    if let Some(ps) = patch.preferred_sources {
        current.preferred_sources = ps;
    }
    if let Some(pu) = patch.preferred_uploaders {
        current.preferred_uploaders = pu;
    }
    if let Some(ms) = patch.min_seeders {
        current.min_seeders = ms;
    }
    if let Some(pd) = patch.prefer_digital {
        current.prefer_digital = pd;
    }
    if let Some(to) = patch.trusted_only {
        current.trusted_only = to;
    }
    if let Some(cp) = patch.collection_policy {
        current.collection_policy = cp;
    }
    if let Some(max_dup) = patch.max_unnecessary_duplicate_coverage {
        current.max_unnecessary_duplicate_coverage = max_dup;
    }
    if let Some(max_acq) = patch.max_acquisition_size_bytes {
        current.max_acquisition_size_bytes = max_acq;
    }
    if let Some(max_plan) = patch.max_plan_size_bytes {
        current.max_plan_size_bytes = max_plan;
    }
    if let Some(qp) = patch.quality_preference {
        current.quality_preference = qp;
    }
    if let Some(amc) = patch.allow_mixed_collections {
        current.allow_mixed_collections = amc;
    }
    if let Some(azs) = patch.allow_zero_seeders {
        current.allow_zero_seeders = azs;
    }
    if let Some(aue) = patch.allow_unsupported_editions {
        current.allow_unsupported_editions = aue;
    }
    // Safety: auto-acquire must remain off in this milestone
    current.auto_acquire = false;
    current.updated_at = now();

    let pref_sources_json = serde_json::to_string(&current.preferred_sources)
        .map_err(|e| AppError::bad_request(format!("invalid preferred_sources: {e}")))?;
    let pref_uploaders_json = serde_json::to_string(&current.preferred_uploaders)
        .map_err(|e| AppError::bad_request(format!("invalid preferred_uploaders: {e}")))?;

    conn.execute(
        "UPDATE acquisition_profiles SET
            name = ?2,
            is_default = ?3,
            language = ?4,
            preferred_release_type = ?5,
            source_selection = ?6,
            preferred_sources = ?7,
            preferred_uploaders = ?8,
            min_seeders = ?9,
            prefer_digital = ?10,
            trusted_only = ?11,
            collection_policy = ?12,
            max_unnecessary_duplicate_coverage = ?13,
            max_acquisition_size_bytes = ?14,
            max_plan_size_bytes = ?15,
            quality_preference = ?16,
            allow_mixed_collections = ?17,
            allow_zero_seeders = ?18,
            allow_unsupported_editions = ?19,
            auto_acquire = 0,
            updated_at = ?20
        WHERE id = ?1",
        params![
            current.id,
            current.name,
            if current.is_default { 1 } else { 0 },
            current.language,
            current.preferred_release_type,
            current.source_selection,
            pref_sources_json,
            pref_uploaders_json,
            current.min_seeders,
            if current.prefer_digital { 1 } else { 0 },
            if current.trusted_only { 1 } else { 0 },
            current.collection_policy,
            current.max_unnecessary_duplicate_coverage,
            current.max_acquisition_size_bytes.map(|v| v as i64),
            current.max_plan_size_bytes.map(|v| v as i64),
            current.quality_preference,
            if current.allow_mixed_collections { 1 } else { 0 },
            current.allow_zero_seeders,
            if current.allow_unsupported_editions { 1 } else { 0 },
            current.updated_at,
        ],
    )?;

    Ok(current)
}

pub fn delete_profile(conn: &Connection, id: &str) -> Result<()> {
    let prof = get_profile_by_id(conn, id)?
        .ok_or_else(|| AppError::not_found(format!("Profile '{id}' not found")))?;

    if prof.is_default {
        return Err(AppError::bad_request("Cannot delete the default acquisition profile"));
    }

    // Reset any series using this profile to default (null)
    conn.execute(
        "UPDATE manga SET acquisition_profile_id = NULL WHERE acquisition_profile_id = ?1",
        [id],
    )?;
    conn.execute("DELETE FROM acquisition_plans WHERE profile_id = ?1", [id])?;
    conn.execute("DELETE FROM acquisition_profiles WHERE id = ?1", [id])?;

    Ok(())
}

pub fn duplicate_profile(
    conn: &Connection,
    source_id: &str,
    new_name: Option<String>,
) -> Result<AcquisitionProfile> {
    let source = get_profile_by_id(conn, source_id)?
        .ok_or_else(|| AppError::not_found(format!("Profile '{source_id}' not found")))?;

    let dup_name = new_name.unwrap_or_else(|| format!("{} (Copy)", source.name));
    let new_id = format!("prof_{}", now());

    let mut dup = source;
    dup.id = new_id;
    dup.name = dup_name;
    dup.is_default = false;
    dup.auto_acquire = false;

    create_profile(conn, dup)
}

pub fn set_manga_profile(
    conn: &Connection,
    manga_id: i64,
    profile_id: Option<&str>,
) -> Result<()> {
    if let Some(id) = profile_id {
        if !id.trim().is_empty() {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM acquisition_profiles WHERE id = ?1)",
                    [id],
                    |r| r.get(0),
                )?;
            if !exists {
                return Err(AppError::not_found(format!("Profile '{id}' does not exist")));
            }
            conn.execute(
                "UPDATE manga SET acquisition_profile_id = ?2 WHERE id = ?1",
                params![manga_id, id],
            )?;
            return Ok(());
        }
    }

    conn.execute(
        "UPDATE manga SET acquisition_profile_id = NULL WHERE id = ?1",
        [manga_id],
    )?;
    Ok(())
}
