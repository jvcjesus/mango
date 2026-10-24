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

pub const DEFAULT_TARGET_HEIGHT: u32 = 2400;
pub const MIN_TARGET_HEIGHT: u32 = 500;
pub const MAX_TARGET_HEIGHT: u32 = 10000;

pub const DEFAULT_IMAGE_QUALITY: u32 = 90;
pub const MIN_IMAGE_QUALITY: u32 = 1;
pub const MAX_IMAGE_QUALITY: u32 = 100;

pub const DEFAULT_MIN_SAVINGS_PERCENT: u32 = 10;
pub const MIN_SAVINGS_PERCENT: u32 = 0;
pub const MAX_MIN_SAVINGS_PERCENT: u32 = 99;

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

/// Verifies whether a CBZ archive contains a ComicInfo.xml file.
pub fn cbz_has_comic_info(path: &Path) -> Result<bool> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::bad_request(format!("Cannot open CBZ {}: {e}", path.display())))?;

    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| AppError::bad_request(e.to_string()))?;
        let name = entry.name().to_lowercase();
        if !entry.is_dir() && name.ends_with("comicinfo.xml") {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Self-cleaning isolated workspace for external image optimizer execution.
///
/// Ensures the optimizer is never executed directly in the source downloads directory,
/// permanent library, or any shared directory with unrelated releases.
pub struct IsolatedWorkspace {
    dir: PathBuf,
}

impl IsolatedWorkspace {
    pub fn new() -> Result<Self> {
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mango-opt-{pid}-{now}"));
        fs::create_dir_all(&path)?;
        Ok(Self { dir: path })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for IsolatedWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Optimizes a CBZ file non-destructively in an isolated temporary workspace.
///
/// Guaranteed safety invariants:
/// 1. Source file is NEVER operated on in-place by external tools.
/// 2. External tools are executed inside an isolated single-archive scratch directory.
/// 3. ComicInfo.xml metadata is verified preserved.
/// 4. Archive format and ZIP structure verified before replacing.
/// 5. Exact page count verified identical.
/// 6. Size savings MUST meet or exceed `min_savings_percent`.
/// 7. On ANY failure, threshold miss, or corruption, original source is left 100% untouched.
/// 8. Scratch workspace is guaranteed deleted via RAII guard.
pub async fn optimize_cbz(
    source_path: &Path,
    shrinker_bin: Option<&str>,
    target_height: u32,
    image_quality: u32,
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
    let orig_has_comic_info = cbz_has_comic_info(source_path)?;
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

    // 1. Create dedicated isolated workspace
    let workspace = IsolatedWorkspace::new()?;
    let stem = source_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "candidate".to_string());
    // Safe filename inside isolated workspace ending in .cbz
    let candidate_path = workspace.path().join(format!("{stem}.cbz"));
    fs::copy(source_path, &candidate_path)?;

    // 2. Prepare process command with explicit quality and resolution options
    let bin_str = bin_path.to_string_lossy();
    let mut cmd = tokio::process::Command::new(&bin_path);
    if bin_str.ends_with("RustyLibraryShrinker") || bin_str.contains("RustyLibraryShrinker") {
        cmd.arg("--lang")
            .arg("en")
            .arg("-r")
            .arg("replace")
            .arg("--target-height")
            .arg(format!("{target_height}"))
            .arg("--quality")
            .arg(format!("{image_quality}"))
            .arg("--min-savings")
            .arg(format!("{min_savings_percent}"))
            .arg("--force-shrink")
            .arg(&candidate_path);
    } else {
        cmd.arg("--target-height")
            .arg(format!("{target_height}"))
            .arg("--quality")
            .arg(format!("{image_quality}"))
            .arg("--min-savings")
            .arg(format!("{min_savings_percent}"))
            .arg(&candidate_path);
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

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!(
                "Optimizer failed (code {:?}): {}",
                output.status.code(),
                stderr.trim()
            ),
        });
    }

    // Detect RustyLibraryShrinker reporting no files found despite exit code 0
    if stdout.contains("No files found") || stdout.contains("Aucun fichier trouvé") {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: "Optimizer reported no files found for candidate archive".to_string(),
        });
    }

    // 3. Authoritative verification of candidate archive
    if let Err(e) = verify_cbz_integrity(&candidate_path) {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Optimized CBZ failed integrity verification: {e}"),
        });
    }

    let opt_pages = match count_cbz_pages(&candidate_path) {
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
            reason: format!(
                "Page count mismatch: original had {orig_pages}, optimized had {opt_pages}"
            ),
        });
    }

    let opt_has_comic_info = cbz_has_comic_info(&candidate_path)?;
    if orig_has_comic_info && !opt_has_comic_info {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: orig_size,
            savings_percent: 0.0,
            applied: false,
            reason: "Optimized candidate lost ComicInfo.xml metadata".to_string(),
        });
    }

    let opt_size = fs::metadata(&candidate_path)?.len();
    if opt_size >= orig_size {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: opt_size,
            savings_percent: 0.0,
            applied: false,
            reason: format!(
                "Savings below required threshold (0.0% < {min_savings_percent}%)"
            ),
        });
    }

    let savings_percent = 100.0 * (1.0 - (opt_size as f64 / orig_size as f64));
    if savings_percent < min_savings_percent as f64 {
        return Ok(OptimizationResult {
            original_bytes: orig_size,
            optimized_bytes: opt_size,
            savings_percent,
            applied: false,
            reason: format!(
                "Savings {savings_percent:.1}% below required threshold {min_savings_percent}%"
            ),
        });
    }

    // 4. Atomic replacement into source location
    // Write candidate to a sibling temporary file in source directory, then rename atomically.
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_replace_path = parent.join(format!(".{stem}.opt-replace-{pid}-{now}.tmp"));

    struct TempReplaceGuard<'a>(&'a Path);
    impl<'a> Drop for TempReplaceGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let guard = TempReplaceGuard(&temp_replace_path);

    fs::copy(&candidate_path, &temp_replace_path)?;
    verify_cbz_integrity(&temp_replace_path)?;
    fs::rename(&temp_replace_path, source_path)?;
    std::mem::forget(guard);

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

        let res = optimize_cbz(&cbz, Some("/nonexistent/binary"), DEFAULT_TARGET_HEIGHT, DEFAULT_IMAGE_QUALITY, 10).await.unwrap();
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

    fn make_valid_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for y in 0..h {
            for x in 0..w {
                pixels.push(((x * 3) % 256) as u8);
                pixels.push(((y * 5) % 256) as u8);
                pixels.push((((x + y) * 7) % 256) as u8);
            }
        }
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, 95);
        encoder.encode(&pixels, w, h, jpeg_encoder::ColorType::Rgb).unwrap();
        out
    }

    fn make_realistic_cbz(path: &Path, pages: usize) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(b"<ComicInfo><Series>Real Test Series</Series><Number>3</Number></ComicInfo>").unwrap();
        let jpg_data = make_valid_jpeg(400, 600);
        for i in 1..=pages {
            zip.start_file(format!("{i:03}.jpg"), options).unwrap();
            zip.write_all(&jpg_data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[tokio::test]
    async fn optimizer_detects_no_files_found_gracefully() {
        let dir = TempDir::new("opt-no-files");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);
        let orig_size = fs::metadata(&cbz).unwrap().len();

        let script = dir.path().join("fake_no_files.sh");
        fs::write(&script, b"#!/bin/sh\necho 'RustyLibraryShrinker 1.0.3'\necho '\xe2\x9d\x8c No files found.'\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), DEFAULT_TARGET_HEIGHT, DEFAULT_IMAGE_QUALITY, 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("no files found"));
        // Original file must remain untouched
        assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
    }

    #[tokio::test]
    async fn optimizer_respects_minimum_savings_threshold() {
        let dir = TempDir::new("opt-threshold");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);
        let orig_size = fs::metadata(&cbz).unwrap().len();

        // Script that shrinks by only 1 byte (less than 50%)
        let script = dir.path().join("fake_small_shrink.sh");
        let script_content = r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        *.cbz) target="$arg" ;;
    esac
done
if [ -n "$target" ]; then
    # Overwrite target with slightly smaller valid zip
    head -c 20 "$target" > "$target.tmp"
    mv "$target.tmp" "$target"
fi
exit 0
"#;
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), DEFAULT_TARGET_HEIGHT, DEFAULT_IMAGE_QUALITY, 50).await.unwrap();
        assert!(!res.applied);
        // Source file preserved
        assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
    }

    #[tokio::test]
    async fn optimizer_rejects_lost_comic_info() {
        let dir = TempDir::new("opt-lost-info");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);
        let orig_size = fs::metadata(&cbz).unwrap().len();

        // Script that replaces archive with one lacking ComicInfo.xml
        let replacement_cbz = dir.path().join("no_info.cbz");
        {
            let file = File::create(&replacement_cbz).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("001.webp", options).unwrap();
            zip.write_all(b"small").unwrap();
            zip.start_file("002.webp", options).unwrap();
            zip.write_all(b"small").unwrap();
            zip.finish().unwrap();
        }

        let script = dir.path().join("fake_strip_info.sh");
        let script_content = format!(
            "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in *.cbz) cp \"{}\" \"$arg\" ;; esac; done\nexit 0\n",
            replacement_cbz.display()
        );
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), DEFAULT_TARGET_HEIGHT, DEFAULT_IMAGE_QUALITY, 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("ComicInfo.xml"));
        // Source file preserved
        assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
    }

    #[tokio::test]
    async fn optimizer_command_receives_configured_target_height_and_quality() {
        let dir = TempDir::new("opt-flags");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);

        let args_file = dir.path().join("args.txt");
        let script = dir.path().join("record_args.sh");
        let script_content = format!(
            r#"#!/bin/sh
echo "$@" > "{}"
for arg in "$@"; do
    case "$arg" in
        *.cbz) target="$arg" ;;
    esac
done
exit 0
"#,
            args_file.display()
        );
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        // Test with custom height 3200 and quality 85
        let _ = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 3200, 85, 15).await.unwrap();
        let recorded = fs::read_to_string(&args_file).unwrap();
        assert!(recorded.contains("--target-height 3200"), "Script received configured target height: {recorded}");
        assert!(recorded.contains("--quality 85"), "Script received configured quality: {recorded}");
        assert!(recorded.contains("--min-savings 15"), "Script received configured min savings: {recorded}");

        // Test with default height 2400 and quality 90
        let _ = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 2400, 90, 10).await.unwrap();
        let recorded_def = fs::read_to_string(&args_file).unwrap();
        assert!(recorded_def.contains("--target-height 2400"));
        assert!(recorded_def.contains("--quality 90"));
        assert!(recorded_def.contains("--min-savings 10"));
    }

    #[tokio::test]
    async fn failed_optimization_leaves_original_unchanged() {
        let dir = TempDir::new("opt-failed");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);
        let orig_bytes = fs::read(&cbz).unwrap();

        // Script that exits with error code 1
        let script = dir.path().join("fake_failing.sh");
        fs::write(&script, b"#!/bin/sh\necho 'error processing'\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 2400, 90, 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("failed"));
        let after_bytes = fs::read(&cbz).unwrap();
        assert_eq!(orig_bytes, after_bytes, "Original archive bytes must remain 100% unchanged on failure");
    }

    #[tokio::test]
    async fn failed_validation_leaves_original_unchanged() {
        let dir = TempDir::new("opt-corrupt-val");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 2);
        let orig_bytes = fs::read(&cbz).unwrap();

        // Script that writes garbage bytes instead of valid ZIP
        let script = dir.path().join("fake_corrupt.sh");
        let script_content = r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        *.cbz) target="$arg" ;;
    esac
done
if [ -n "$target" ]; then
    echo "THIS IS CORRUPT GARBAGE NOT A ZIP" > "$target"
fi
exit 0
"#;
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 2400, 90, 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("failed integrity verification"));
        let after_bytes = fs::read(&cbz).unwrap();
        assert_eq!(orig_bytes, after_bytes, "Original archive bytes must remain 100% untouched on invalid ZIP output");
    }

    #[tokio::test]
    async fn page_count_mismatch_leaves_original_unchanged() {
        let dir = TempDir::new("opt-page-mismatch");
        let cbz = dir.path().join("test.cbz");
        make_test_archive(&cbz, 3); // 3 pages
        let orig_bytes = fs::read(&cbz).unwrap();

        // Replacement with only 1 page
        let replacement_cbz = dir.path().join("mismatch.cbz");
        make_test_archive(&replacement_cbz, 1);

        let script = dir.path().join("fake_page_drop.sh");
        let script_content = format!(
            "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in *.cbz) cp \"{}\" \"$arg\" ;; esac; done\nexit 0\n",
            replacement_cbz.display()
        );
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 2400, 90, 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("Page count mismatch"));
        let after_bytes = fs::read(&cbz).unwrap();
        assert_eq!(orig_bytes, after_bytes, "Original archive bytes must remain 100% untouched on page count mismatch");
    }

    #[tokio::test]
    async fn successful_validated_optimization_replaces_download_safely() {
        let dir = TempDir::new("opt-replace-success");
        let cbz = dir.path().join("Chapter 1 [STICK].cbz");

        // Create substantial source archive
        {
            let file = File::create(&cbz).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("ComicInfo.xml", options).unwrap();
            zip.write_all(b"<ComicInfo><Series>Blue Lock</Series><Number>6</Number></ComicInfo>").unwrap();
            let large_data = vec![42u8; 50000];
            zip.start_file("001.webp", options).unwrap();
            zip.write_all(&large_data).unwrap();
            zip.start_file("002.webp", options).unwrap();
            zip.write_all(&large_data).unwrap();
            zip.finish().unwrap();
        }
        let orig_size = fs::metadata(&cbz).unwrap().len();
        assert!(orig_size > 100000);

        // Script that shrinks the candidate archive by 50% while preserving ComicInfo and 2 pages
        let script = dir.path().join("fake_successful_shrink.sh");
        let script_content = r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        *.cbz) target="$arg" ;;
    esac
done
if [ -n "$target" ]; then
    tmp="$target.small.zip"
    python3 -c "
import zipfile
with zipfile.ZipFile('$tmp', 'w') as z:
    z.writestr('ComicInfo.xml', '<ComicInfo><Series>Blue Lock</Series><Number>6</Number></ComicInfo>')
    z.writestr('001.webp', b'small compressed data 1')
    z.writestr('002.webp', b'small compressed data 2')
" 2>/dev/null
    if [ -f "$tmp" ]; then
        mv "$tmp" "$target"
    fi
fi
exit 0
"#;
        fs::write(&script, script_content.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let res = optimize_cbz(&cbz, Some(script.to_str().unwrap()), 2400, 90, 10).await.unwrap();
        assert!(res.applied, "Optimization should be applied: {}", res.reason);
        assert!(res.savings_percent > 50.0);

        // Invariant: original download file was replaced atomically with optimized candidate
        let replaced_size = fs::metadata(&cbz).unwrap().len();
        assert_eq!(replaced_size, res.optimized_bytes);
        assert!(replaced_size < orig_size);
        assert_eq!(count_cbz_pages(&cbz).unwrap(), 2);
        assert!(cbz_has_comic_info(&cbz).unwrap());
        verify_cbz_integrity(&cbz).unwrap();

        // Check that no temporary replacement artifacts linger in the download directory
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("opt-replace") || n.contains(".tmp"))
            .collect();
        assert!(entries.is_empty(), "No temporary replacement files should linger: {entries:?}");
    }

    #[tokio::test]
    async fn real_synthetic_cbz_optimization_with_bundled_shrinker() {
        let bundled = Path::new(BUNDLED_OPTIMIZER_PATH);
        let optimizer_bin: Option<String> = if bundled.is_file() {
            Some(BUNDLED_OPTIMIZER_PATH.to_string())
        } else if let Some(in_path) = find_in_path("RustyLibraryShrinker") {
            Some(in_path.to_string_lossy().to_string())
        } else {
            None
        };

        let bin_str = match &optimizer_bin {
            Some(s) => s.as_str(),
            None => {
                // When running outside Docker where binary is not installed, verify fallback behavior
                let dir = TempDir::new("real-opt-skip");
                let cbz = dir.path().join("test.cbz");
                make_realistic_cbz(&cbz, 2);
                let orig_size = fs::metadata(&cbz).unwrap().len();
                let res = optimize_cbz(&cbz, None, DEFAULT_TARGET_HEIGHT, DEFAULT_IMAGE_QUALITY, 10).await.unwrap();
                assert!(!res.applied);
                assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
                return;
            }
        };

        let dir = TempDir::new("real-opt-test");
        let cbz = dir.path().join("Chapter 1 [Test].cbz");
        make_realistic_cbz(&cbz, 2);
        let orig_size = fs::metadata(&cbz).unwrap().len();
        assert!(orig_size > 50000, "Synthetic CBZ has substantial image content");

        // 1. Run optimization with 2400 target height, 90 quality, 10% min savings
        let res = optimize_cbz(&cbz, Some(bin_str), 2400, 90, 10).await.unwrap();
        if res.applied {
            assert!(res.savings_percent >= 10.0);
            let opt_size = fs::metadata(&cbz).unwrap().len();
            assert!(opt_size < orig_size);
            assert_eq!(opt_size, res.optimized_bytes);
            assert!(cbz_has_comic_info(&cbz).unwrap(), "ComicInfo.xml preserved in optimized CBZ");
            assert_eq!(count_cbz_pages(&cbz).unwrap(), 2, "Exact page count preserved");
            verify_cbz_integrity(&cbz).unwrap();
        } else {
            // If already compact, original must remain byte-identical
            assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
        }

        // 2. Run with impossible 99% savings threshold -> must NOT apply
        let res_high = optimize_cbz(&cbz, Some(bin_str), 2400, 90, 99).await.unwrap();
        assert!(!res_high.applied);
        assert!(res_high.reason.contains("below required threshold"));
    }
}
