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
    path::Path,
    time::Duration,
};

#[allow(dead_code)]
pub const DEFAULT_MIN_SAVINGS_PERCENT: u32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationResult {
    pub original_bytes: u64,
    pub optimized_bytes: u64,
    pub savings_percent: f64,
    pub applied: bool,
    pub reason: String,
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
    let bin = match shrinker_bin.map(str::trim).filter(|s| !s.is_empty()) {
        Some(b) => b,
        None => {
            return Ok(OptimizationResult {
                original_bytes: fs::metadata(source_path).map(|m| m.len()).unwrap_or(0),
                optimized_bytes: 0,
                savings_percent: 0.0,
                applied: false,
                reason: "Optimizer binary not configured".to_string(),
            });
        }
    };

    if !Path::new(bin).is_file() {
        return Ok(OptimizationResult {
            original_bytes: fs::metadata(source_path).map(|m| m.len()).unwrap_or(0),
            optimized_bytes: 0,
            savings_percent: 0.0,
            applied: false,
            reason: format!("Optimizer binary not found at {bin}"),
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
    let temp_path = parent.join(format!(".{}.shrink-{pid}.tmp", source_path.file_name().unwrap().to_string_lossy()));

    struct TempGuard<'a>(&'a Path);
    impl<'a> Drop for TempGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let _guard = TempGuard(&temp_path);

    fs::copy(source_path, &temp_path)?;

    // Execute optimizer with timeout
    let bin_path = bin.to_string();
    let target_arg = temp_path.to_string_lossy().to_string();
    let command_fut = tokio::process::Command::new(bin_path).arg(target_arg).output();

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
        let dir = std::env::temp_dir().join(format!("mango-opt-pages-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let cbz = dir.join("test.cbz");
        make_test_archive(&cbz, 5);
        assert_eq!(count_cbz_pages(&cbz).unwrap(), 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fallback_when_binary_missing() {
        let dir = std::env::temp_dir().join(format!("mango-opt-missing-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let cbz = dir.join("test.cbz");
        make_test_archive(&cbz, 3);
        let orig_size = fs::metadata(&cbz).unwrap().len();

        let res = optimize_cbz(&cbz, Some("/nonexistent/binary"), 10).await.unwrap();
        assert!(!res.applied);
        assert!(res.reason.contains("not found"));
        // Source file preserved
        assert_eq!(fs::metadata(&cbz).unwrap().len(), orig_size);
        let _ = fs::remove_dir_all(&dir);
    }
}
