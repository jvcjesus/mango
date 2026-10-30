use crate::error::{AppError, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/opentomedb/mangarr-metadata/releases/latest/download/version.json";
pub const DEFAULT_DB_FALLBACK_URL: &str =
    "https://github.com/opentomedb/mangarr-metadata/releases/latest/download/manga-metadata.sqlite";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenTomeVersionManifest {
    pub gcd_dump: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub artifact_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTomeStatus {
    pub enabled: bool,
    pub available: bool,
    pub version: Option<String>,
    pub file_size: Option<u64>,
    pub last_updated_at: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub catalogue_path: String,
}

pub struct OpenTomeUpdater {
    dir: PathBuf,
    db_path: PathBuf,
    manifest_path: PathBuf,
    http: reqwest::Client,
    is_updating: Arc<AtomicBool>,
}

impl OpenTomeUpdater {
    pub fn new(data_dir: &Path, http: reqwest::Client) -> Self {
        let dir = data_dir.join("opentome");
        let db_path = dir.join("manga-metadata.sqlite");
        let manifest_path = dir.join("version.json");
        Self {
            dir,
            db_path,
            manifest_path,
            http,
            is_updating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn is_available(&self) -> bool {
        self.db_path.exists()
    }

    pub fn read_manifest(&self) -> Option<OpenTomeVersionManifest> {
        if !self.manifest_path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(&self.manifest_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn status(&self, enabled: bool, last_checked_at: Option<i64>, last_error: Option<String>) -> OpenTomeStatus {
        let manifest = self.read_manifest();
        let available = self.is_available();
        let file_size = if available {
            std::fs::metadata(&self.db_path).map(|m| m.len()).ok()
        } else {
            None
        };
        let last_updated = if available {
            std::fs::metadata(&self.db_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
        } else {
            None
        };

        OpenTomeStatus {
            enabled,
            available,
            version: manifest.map(|m| m.gcd_dump),
            file_size,
            last_updated_at: last_updated,
            last_checked_at,
            last_error,
            catalogue_path: self.db_path.display().to_string(),
        }
    }

    /// Check upstream for a new catalogue and atomically download/replace if needed.
    /// Returns true if a new catalogue was downloaded and installed.
    pub async fn check_and_update(&self, manifest_url: &str, force: bool) -> Result<bool> {
        if self
            .is_updating
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::info!("OpenTome catalogue update already in progress; skipping duplicate call");
            return Ok(false);
        }

        struct UpdateGuard(Arc<AtomicBool>);
        impl Drop for UpdateGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = UpdateGuard(self.is_updating.clone());

        std::fs::create_dir_all(&self.dir).map_err(|e| {
            AppError::internal(format!(
                "Failed to create OpenTome directory {}: {e}",
                self.dir.display()
            ))
        })?;

        tracing::info!("Checking OpenTome catalogue manifest at {manifest_url}");
        let resp = self
            .http
            .get(manifest_url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| {
                AppError::internal(format!("Failed to fetch OpenTome manifest: {e}"))
            })?;

        if !resp.status().is_success() {
            return Err(AppError::internal(format!(
                "OpenTome manifest request returned HTTP {}",
                resp.status()
            )));
        }

        let manifest: OpenTomeVersionManifest = resp.json().await.map_err(|e| {
            AppError::internal(format!("Failed to parse OpenTome manifest JSON: {e}"))
        })?;

        let current_manifest = self.read_manifest();
        if !force
            && self.is_available()
            && current_manifest.as_ref().map(|m| &m.gcd_dump) == Some(&manifest.gcd_dump)
        {
            tracing::info!(
                "OpenTome catalogue is already up to date ({})",
                manifest.gcd_dump
            );
            return Ok(false);
        }

        // Determine download URL
        let download_url = match manifest.artifact_url.as_deref() {
            Some(url) if url.starts_with("http") => url,
            _ => DEFAULT_DB_FALLBACK_URL,
        };

        tracing::info!(
            "Downloading OpenTome catalogue build {} from {download_url} (~{} MB)",
            manifest.gcd_dump,
            manifest.size / (1024 * 1024)
        );

        let tmp_path = self.dir.join("manga-metadata.sqlite.tmp");
        if tmp_path.exists() {
            let _ = std::fs::remove_file(&tmp_path);
        }

        let mut file = std::fs::File::create(&tmp_path).map_err(|e| {
            AppError::internal(format!("Failed to create temporary catalogue file: {e}"))
        })?;

        let mut dl_resp = self
            .http
            .get(download_url)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                AppError::internal(format!("Failed to initiate OpenTome download: {e}"))
            })?;

        if !dl_resp.status().is_success() {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AppError::internal(format!(
                "OpenTome database download returned HTTP {}",
                dl_resp.status()
            )));
        }

        let mut hasher = Sha256::new();
        let mut downloaded_bytes: u64 = 0;

        while let Some(chunk) = dl_resp.chunk().await.map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            AppError::internal(format!("Error streaming OpenTome chunk: {e}"))
        })? {
            hasher.update(&chunk);
            downloaded_bytes += chunk.len() as u64;
            file.write_all(&chunk).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                AppError::internal(format!("Error writing OpenTome chunk to disk: {e}"))
            })?;
        }

        file.flush().map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            AppError::internal(format!("Error flushing OpenTome file: {e}"))
        })?;
        drop(file);

        let computed_hash = hex::encode(hasher.finalize());
        let expected_hash = manifest.sha256.trim().to_lowercase();

        if computed_hash.to_lowercase() != expected_hash {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AppError::internal(format!(
                "OpenTome SHA-256 checksum mismatch: expected {}, computed {}",
                expected_hash, computed_hash
            )));
        }

        // Validate SQLite database integrity and minimum schema
        if let Err(err) = Self::validate_sqlite_file(&tmp_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AppError::internal(format!(
                "OpenTome catalogue validation failed: {err}"
            )));
        }

        // Atomically replace the database file
        std::fs::rename(&tmp_path, &self.db_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            AppError::internal(format!("Failed to rename temporary catalogue file: {e}"))
        })?;

        // Write version.json manifest
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        let _ = std::fs::write(&self.manifest_path, manifest_json);

        tracing::info!(
            "Successfully installed OpenTome catalogue version {} ({} bytes)",
            manifest.gcd_dump,
            downloaded_bytes
        );

        Ok(true)
    }

    /// Validate that a candidate SQLite file is readable and adheres to the minimum schema.
    pub fn validate_sqlite_file(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            AppError::internal(format!("Failed to open candidate SQLite database: {e}"))
        })?;

        // 1. PRAGMA integrity_check
        let integrity: String = conn
            .query_row("PRAGMA integrity_check(1);", [], |r| r.get(0))
            .map_err(|e| AppError::internal(format!("Integrity check query failed: {e}")))?;

        if integrity != "ok" {
            return Err(AppError::internal(format!(
                "SQLite integrity check failed: {integrity}"
            )));
        }

        // 2. Check required tables
        let required_tables = ["series", "volumes", "meta"];
        for tbl in required_tables {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [tbl],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if !exists {
                return Err(AppError::internal(format!(
                    "Missing required OpenTome table: {tbl}"
                )));
            }
        }

        // 3. Check required columns in `series`
        let required_series_cols = ["gcd_series_id", "name", "language", "tome_work_id", "is_main"];
        for col in required_series_cols {
            let col_exists: bool = conn
                .query_row(
                    &format!("SELECT 1 FROM pragma_table_info('series') WHERE name = '{col}'"),
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if !col_exists {
                return Err(AppError::internal(format!(
                    "Missing required column '{col}' in 'series' table"
                )));
            }
        }

        // 4. Check required columns in `volumes`
        let required_vol_cols = ["gcd_series_id", "volume_number", "release_date"];
        for col in required_vol_cols {
            let col_exists: bool = conn
                .query_row(
                    &format!("SELECT 1 FROM pragma_table_info('volumes') WHERE name = '{col}'"),
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if !col_exists {
                return Err(AppError::internal(format!(
                    "Missing required column '{col}' in 'volumes' table"
                )));
            }
        }

        Ok(())
    }
}
