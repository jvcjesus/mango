//! Non-destructive image optimization for manga archives using external tools.
//!
//! Enforces safety invariants:
//! - Non-destructive: fails safe and leaves original file untouched on any failure.
//! - Page count verification: optimized archive MUST contain the exact same page count.
//! - Minimum savings threshold: rejects optimized output if space saved is below threshold.
//! - Format integrity: verified as valid ZIP archive before replacing the original.

use crate::error::{AppError, Result};
use crate::pipeline::publisher::verify_cbz_integrity;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::Duration,
};

pub const DEFAULT_MIN_SAVINGS_PERCENT: u32 = 10;
pub const BUNDLED_OPTIMIZER_PATH: &str = "/opt/mango/bin/RustyLibraryShrinker";
pub const LEGACY_DEFAULT_OPTIMIZER_BIN: &str = "/srv/apps/manga/manga-itagger/RustyLibraryShrinker";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationResult {
    pub original_bytes: u64,
    pub optimized_bytes: u64,
    pub savings_percent: f64,
    pub applied: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizerValidationCode {
    Valid,
    NotConfigured,
    Missing,
    NotAFile,
    NotExecutable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptimizerValidationResult {
    pub configured_path: Option<String>,
    pub resolved_path: Option<String>,
    pub source: String, // "custom", "bundled", "path", "none"
    pub code: OptimizerValidationCode,
    pub message: String,
    pub version: Option<String>,
}

impl OptimizerValidationResult {
    pub fn is_valid(&self) -> bool {
        self.code == OptimizerValidationCode::Valid
    }
}

/// Checks whether a file exists and has executable permissions.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Searches the system PATH environment variable for an executable binary.
pub fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for split_dir in std::env::split_paths(&path_var) {
        let candidate = split_dir.join(binary_name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Resolves the effective optimizer binary using the clean priority strategy:
/// 1. Explicit deliberate custom configuration (excluding legacy host default)
/// 2. Bundled known executable (/opt/mango/bin/RustyLibraryShrinker)
/// 3. System PATH discovery (e.g. RustyLibraryShrinker anywhere in PATH)
/// 4. Unavailable
pub fn resolve_optimizer_binary(custom_bin: Option<&str>) -> (Option<PathBuf>, &'static str) {
    let trimmed = custom_bin.map(str::trim).filter(|s| !s.is_empty());

    // 1. Explicit deliberate custom configuration
    if let Some(custom) = trimmed {
        if custom != LEGACY_DEFAULT_OPTIMIZER_BIN {
            return (Some(PathBuf::from(custom)), "custom");
        }
    }

    // 2. Bundled known executable inside container image
    let bundled = Path::new(BUNDLED_OPTIMIZER_PATH);
    if bundled.is_file() {
        return (Some(bundled.to_path_buf()), "bundled");
    }

    // 3. System PATH discovery
    if let Some(in_path) = find_in_path("RustyLibraryShrinker") {
        return (Some(in_path), "path");
    }

    (None, "none")
}

/// Tests whether the executable can reasonably be invoked (e.g. `--version`).
pub async fn check_executable_invocable(path: &Path) -> std::result::Result<String, String> {
    if !path.is_file() {
        return Err(format!("'{}' is not a regular file", path.display()));
    }
    if !is_executable(path) {
        return Err(format!("'{}' is not marked executable", path.display()));
    }

    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| "Process invocation timed out after 3 seconds".to_string())?
    .map_err(|e| format!("Failed to execute process: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Process exited with code {:?}: {}",
            output.status.code(),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("OK").trim().to_string();
    Ok(if first_line.is_empty() { "OK".to_string() } else { first_line })
}

/// Authoritatively validates optimizer configuration and binary accessibility.
pub async fn validate_optimizer(custom_bin: Option<&str>) -> OptimizerValidationResult {
    let configured = custom_bin
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != LEGACY_DEFAULT_OPTIMIZER_BIN)
        .map(ToString::to_string);

    let (resolved, source) = resolve_optimizer_binary(custom_bin);

    let resolved_path = match resolved {
        Some(p) => p,
        None => {
            return OptimizerValidationResult {
                configured_path: configured,
                resolved_path: None,
                source: source.to_string(),
                code: OptimizerValidationCode::Missing,
                message: "No optimizer binary found (bundled optimizer not installed and not in PATH)".to_string(),
                version: None,
            };
        }
    };

    let resolved_str = resolved_path.to_string_lossy().to_string();

    if !resolved_path.exists() {
        return OptimizerValidationResult {
            configured_path: configured,
            resolved_path: Some(resolved_str.clone()),
            source: source.to_string(),
            code: OptimizerValidationCode::Missing,
            message: format!("Binary not found at '{resolved_str}'"),
            version: None,
        };
    }

    if !resolved_path.is_file() {
        return OptimizerValidationResult {
            configured_path: configured,
            resolved_path: Some(resolved_str.clone()),
            source: source.to_string(),
            code: OptimizerValidationCode::NotAFile,
            message: format!("Path '{resolved_str}' is not a regular file"),
            version: None,
        };
    }

    if !is_executable(&resolved_path) {
        return OptimizerValidationResult {
            configured_path: configured,
            resolved_path: Some(resolved_str.clone()),
            source: source.to_string(),
            code: OptimizerValidationCode::NotExecutable,
            message: format!("Binary '{resolved_str}' does not have executable permissions"),
            version: None,
        };
    }

    match check_executable_invocable(&resolved_path).await {
        Ok(version) => OptimizerValidationResult {
            configured_path: configured,
            resolved_path: Some(resolved_str),
            source: source.to_string(),
            code: OptimizerValidationCode::Valid,
            message: format!("Valid and executable ({version})"),
            version: Some(version),
        },
        Err(e) => OptimizerValidationResult {
            configured_path: configured,
            resolved_path: Some(resolved_str),
            source: source.to_string(),
            code: OptimizerValidationCode::Failed,
            message: format!("Execution test failed: {e}"),
            version: None,
        },
    }
}

/// Counts image pages in a CBZ archive (excluding ComicInfo.xml and junk files).
pub fn count_cbz_pages(path: &Path) -> Result<usize> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::bad_request(format!("Cannot open CBZ {}: {e}", path.display())))?;

    let mut count = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| AppError::bad_request(e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_lowercase();
        if name.starts_with("__macosx") || name.starts_with('.') || name.ends_with("comicinfo.xml") {
            continue;
        }
        if name.ends_with(".jpg")
            || name.ends_with(".jpeg")
            || name.ends_with(".png")
            || name.ends_with(".webp")
            || name.ends_with(".avif")
            || name.ends_with(".gif")
            || name.ends_with(".jxl")
        {
            count += 1;
        }
    }

    Ok(count)
}

/// Optimizes a CBZ file in-place using an external tool (e.g. RustyLibraryShrinker).
///
/// If optimization fails, loses pages, or saves less than `min_savings_percent`,
/// the source file is left completely untouched.
pub async fn optimize_cbz(
    source_path: &Path,
    shrinker_bin: Option<&str>,
    min_savings_percent: u32,
) -> Result<OptimizationResult> {
    let (resolved_bin, _source) = resolve_optimizer_binary(shrinker_bin);
    let bin_path = match resolved_bin {
        Some(b) => b,
        None => {
            return Ok(OptimizationResult {
                original_bytes: fs::metadata(source_path).map(|m| m.len()).unwrap_or(0),
                optimized_bytes: 0,
                savings_percent: 0.0,
                applied: false,
                reason: "Optimizer binary not configured or found".to_string(),
            });
        }
    };

    if !bin_path.is_file() {
        return Ok(OptimizationResult {
            original_bytes: fs::metadata(source_path).map(|m| m.len()).unwrap_or(0),
            optimized_bytes: 0,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Optimizer binary not found at {}", bin_path.display()),
        });
    }

    verify_cbz_integrity(source_path)?;
    let orig_pages = count_cbz_pages(source_path)?;
    let orig_size = fs::metadata(source_path)?.len();

    if orig_size == 0 || orig_pages == 0 {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: "CBZ archive is empty or contains no pages".to_string(),
        });
    }

    // Work on a temporary file in the same directory to allow atomic replacement
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let stem = source_path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let temp_path = parent.join(format!(".{stem}.shrink-{pid}.cbz"));

    struct TempGuard<'a>(&'a Path);
    impl<'a> Drop for TempGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let _guard = TempGuard(&temp_path);

    fs::copy(source_path, &temp_path)?;

    // Execute optimizer with timeout and safe argument array
    let bin_str = bin_path.to_string_lossy();
    let mut cmd = tokio::process::Command::new(&bin_path);
    if bin_str.ends_with("RustyLibraryShrinker") || bin_str.contains("RustyLibraryShrinker") {
        cmd.arg("--lang")
            .arg("en")
            .arg("-r")
            .arg("replace")
            .arg("--min-savings")
            .arg(format!("{min_savings_percent}"))
            .arg(&temp_path);
    } else {
        cmd.arg(&temp_path);
    }

    let command_fut = cmd.output();

    let output = match tokio::time::timeout(Duration::from_secs(600), command_fut).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Ok(OptimizationResult {
                original_bytes: orig_size,
                optimized_bytes: orig_size,
                savings_percent: 0.0,
                applied: false,
                reason: format!("Optimizer process error: {e}"),
            });
        }
        Err(_) => {
            return Ok(OptimizationResult {
                original_bytes: orig_size,
                optimized_bytes: orig_size,
                savings_percent: 0.0,
                applied: false,
                reason: "Optimizer process timed out after 10 minutes".to_string(),
            });
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Optimizer failed (code {:?}): {}", output.status.code(), stderr.trim()),
        });
    }

    if let Err(e) = verify_cbz_integrity(&temp_path) {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Optimized CBZ failed integrity verification: {e}"),
        });
    }

    let opt_pages = match count_cbz_pages(&temp_path) {
        Ok(p) => p,
        Err(e) => {
            return Ok(OptimizationResult {
                original_bytes: orig_size,
                optimized_bytes: orig_size,
                savings_percent: 0.0,
                applied: false,
                reason: format!("Could not count pages in optimized CBZ: {e}"),
            });
        }
    };

    if opt_pages != orig_pages {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Page count mismatch: original had {orig_pages}, optimized had {opt_pages}"),
        });
    }

    let opt_size = fs::metadata(&temp_path)?.len();
    if opt_size >= orig_size {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: opt_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!("No size savings (original {orig_size} <= optimized {opt_size})"),
        });
    }

    let savings_percent = 100.0 * (1.0 - (opt_size as f64 / orig_size as f64));
    if savings_percent < min_savings_percent as f64 {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: opt_size,
            savings_percent,
            applied: false,
            reason: format!("Savings {savings_percent:.1}% below minimum threshold {min_savings_percent}%"),
        });
    }

    // Atomic replacement
    fs::rename(&temp_path, source_path)?;

    tracing::info!(
        "OPTIMIZER: reduced {} from {} bytes to {} bytes ({savings_percent:.1}% saved)",
        source_path.display(),
        orig_size,
        opt_size
    );

    Ok(OptimizationResult {
        original_bytes: orig_size,
        optimized_bytes: opt_size,
        savings_percent,
        applied: true,
        reason: format!("Optimized successfully ({savings_percent:.1}% saved)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::SystemTime;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(prefix: &str) -> Self {
            let pid = std::process::id();
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_test_archive(path: &Path, pages: usize) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(b"<ComicInfo><Title>Test</Title></ComicInfo>").unwrap();
        for i in 1..=pages {
            zip.start_file(format!("{i:03}.webp"), options).unwrap();
            zip.write_all(b"fake image bytes").unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn counts_pages_correctly() {
        let dir = TempDir::new("mango-opt-pages");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 5);
        assert_eq!(count_cbz_pages(&cbz).unwrap(), 5);
    }

    #[tokio::test]
    async fn fallback_when_binary_missing() {
        let dir = TempDir::new("mango-opt-missing");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 3);
        let orig_size = fs::metadata(&cbz).unwrap().len();

        let res = optimize_cbz(&cbz, Some("/nonexistent/binary"), 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("not found") || res.reason.contains("not configured"));
        // Source file preserved
        assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
    }

    #[test]
    fn resolves_custom_binary() {
        let (path, src) = resolve_optimizer_binary(Some("/custom/bin/shrinker"));
        assert_eq!(path.unwrap(), PathBuf::from("/custom/bin/shrinker"));
        assert_eq!(src, "custom");
    }

    #[test]
    fn migrates_legacy_default_path() {
        // Untouched legacy path must NOT be treated as a deliberate custom path
        let (path, src) = resolve_optimizer_binary(Some(LEGACY_DEFAULT_OPTIMIZER_BIN));
        if Path::new(BUNDLED_OPTIMIZER_PATH).is_file() {
            assert_eq!(path.unwrap(), PathBuf::from(BUNDLED_OPTIMIZER_PATH));
            assert_eq!(src, "bundled");
        } else if let Some(in_path) = find_in_path("RustyLibraryShrinker") {
            assert_eq!(path.unwrap(), in_path);
            assert_eq!(src, "path");
        } else {
            assert!(path.is_none());
            assert_eq!(src, "none");
        }
    }

    #[tokio::test]
    async fn validates_missing_optimizer() {
        let res = validate_optimizer(Some("/nonexistent/path/RustyLibraryShrinker")).await;
        assert_eq!(res.code, OptimizerValidationCode::Missing);
        assert!(!res.is_valid());
        assert_eq!(res.source, "custom");
    }

    #[tokio::test]
    async fn validates_not_a_file_optimizer() {
        let dir = TempDir::new("opt-not-file");
        let res = validate_optimizer(Some(dir.path().to_str().unwrap())).await;
        assert_eq!(res.code, OptimizerValidationCode::NotAFile);
        assert!(!res.is_valid());
    }

    #[tokio::test]
    async fn validates_executable_script() {
        let dir = TempDir::new("opt-script");
        let script = dir.path().join("fake_shrinker.sh");
        fs::write(&script, b"#!/bin/sh\necho 'RustyLibraryShrinker 1.0.3'\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = validate_optimizer(Some(script.to_str().unwrap())).await;
        assert_eq!(res.code, OptimizerValidationCode::Valid);
        assert!(res.is_valid());
        assert_eq!(res.version.as_deref(), Some("RustyLibraryShrinker 1.0.3"));
    }

    #[tokio::test]
    async fn validates_non_executable_binary() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = TempDir::new("opt-no-exec");
            let no_exec = dir.path().join("no_exec_bin");
            fs::write(&no_exec, b"echo test").unwrap();
            let mut perms = fs::metadata(&no_exec).unwrap().permissions();
            perms.set_mode(0o644);
            fs::set_permissions(&no_exec, perms).unwrap();

            let res = validate_optimizer(Some(no_exec.to_str().unwrap())).await;
            assert_eq!(res.code, OptimizerValidationCode::NotExecutable);
            assert!(!res.is_valid());
        }
    }
}
