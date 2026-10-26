//! Safe, atomic publication of manga archives into canonical Komga library directories.
//!
//! Enforces critical safety guarantees:
//! - NO SILENT OVERWRITE: If destination already exists and differs, publication fails closed.
//! - ATOMIC PUBLICATION: Writes to temporary hidden files and renames only after ZIP verification.
//! - DUPLICATE PROTECTION: Identical files are recognized as a safe no-op.
//! - CONFIGURABLE ROUTING: Routes English vs Chinese editions based on metadata/settings.

use crate::error::{AppError, Result};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    Published(PathBuf),
    AlreadyPublished(PathBuf),
}

/// Generates the canonical release name according to Komga/library conventions:
/// - Volume: "Series v01" (or "Series v12.5")
/// - Chapter: "Series ch001" (or "Series ch10.5")
pub fn canonical_release_name(series: &str, is_volume: bool, number: f64) -> String {
    let safe_series = series.trim();
    if is_volume {
        if number.fract() == 0.0 && number >= 0.0 {
            format!("{safe_series} v{:02}", number as i64)
        } else {
            format!("{safe_series} v{number}")
        }
    } else {
        if number.fract() == 0.0 && number >= 0.0 {
            format!("{safe_series} ch{:03}", number as i64)
        } else {
            format!("{safe_series} ch{number}")
        }
    }
}

/// Sanitizes a series folder name to prevent directory traversal or invalid characters.
pub fn sanitize_series_name(series: &str) -> Result<String> {
    let trimmed = series.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad_request("Series name cannot be empty"));
    }
    if trimmed == ".." || trimmed == "." {
        return Err(AppError::bad_request(format!("Unsafe series name: {trimmed:?}")));
    }
    // Clean out control, path separators, and filesystem reserved characters
    let safe: String = trimmed
        .chars()
        .map(|c| if c.is_control() || "/\\<>:\"|?*\0".contains(c) { '_' } else { c })
        .collect();
    let safe = safe.trim().trim_end_matches('.').trim();
    if safe.is_empty() {
        return Err(AppError::bad_request("Sanitized series name is empty"));
    }
    Ok(safe.to_string())
}

/// Checks whether two files are byte-for-byte identical.
pub fn files_are_identical(a: &Path, b: &Path) -> Result<bool> {
    let stat_a = fs::metadata(a)?;
    let stat_b = fs::metadata(b)?;
    if stat_a.len() != stat_b.len() {
        return Ok(false);
    }
    if stat_a.len() == 0 {
        return Ok(true);
    }

    let mut reader_a = BufReader::new(File::open(a)?);
    let mut reader_b = BufReader::new(File::open(b)?);

    let mut buf_a = [0u8; 64 * 1024];
    let mut buf_b = [0u8; 64 * 1024];

    loop {
        let n_a = reader_a.read(&mut buf_a)?;
        let n_b = reader_b.read(&mut buf_b)?;
        if n_a != n_b {
            return Ok(false);
        }
        if n_a == 0 {
            break;
        }
        if buf_a[..n_a] != buf_b[..n_b] {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Verifies that a ZIP/CBZ file has valid structure, readable headers, and contains plausible manga image content.
pub fn verify_cbz_integrity(path: &Path) -> Result<()> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| AppError::bad_request(format!("Invalid CBZ archive {}: {e}", path.display())))?;
    if archive.is_empty() {
        return Err(AppError::bad_request(format!("CBZ archive {} has 0 files", path.display())));
    }
    // Quick header scan of all entries and check for plausible manga image content
    let mut image_count = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            AppError::bad_request(format!("Corrupt entry {i} in {}: {e}", path.display()))
        })?;
        if entry.enclosed_name().is_none()
            || entry.name().contains('\\')
            || entry.name().contains(':')
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(AppError::bad_request("Unsafe ZIP entry path or symlink"));
        }
        let name = entry.name();
        let basename = name.rsplit_once('/').map(|(_, b)| b).unwrap_or(name);
        if entry.is_file() && !name.starts_with("__MACOSX") && !basename.starts_with('.') && crate::local::is_image(name) {
            image_count += 1;
        }
        std::io::copy(&mut entry, &mut std::io::sink())
            .map_err(|_| AppError::bad_request("Corrupt CBZ entry data or checksum"))?;
    }
    if image_count == 0 {
        return Err(AppError::bad_request(format!(
            "Archive {} contains 0 manga images (non-manga or unsupported content)",
            path.display()
        )));
    }
    Ok(())
}

/// Atomically publishes a CBZ file to the permanent library.
///
/// Guaranteed properties:
/// 1. Destination already exists and is identical -> Safe no-op (AlreadyPublished).
/// 2. Destination already exists and differs -> Error (refuses silent overwrite).
/// 3. Copied file is verified as a valid ZIP before being exposed at final destination.
/// 4. Written via a verified hidden staging file and atomically linked without replacement.
pub fn publish_cbz(
    source: &Path,
    library_root: &Path,
    series: &str,
    is_volume: bool,
    number: f64,
) -> Result<PublishOutcome> {
    if !source.is_file() {
        return Err(AppError::not_found("Source acquisition artifact does not exist"));
    }

    verify_cbz_integrity(source)?;

    let safe_series = sanitize_series_name(series)?;
    let release_name = canonical_release_name(&safe_series, is_volume, number);
    let destination_dir = library_root.join(&safe_series);
    let destination = destination_dir.join(format!("{release_name}.cbz"));

    fs::create_dir_all(&destination_dir)?;

    if destination.exists() {
        if files_are_identical(source, &destination)? {
            tracing::info!("LIBRARY: already published at {}", destination.display());
            return Ok(PublishOutcome::AlreadyPublished(destination));
        }

        return Err(AppError::new(
            409,
            "destination_conflict",
            format!(
                "Destination already exists and differs from source; refusing to overwrite: {}",
                destination.display()
            ),
        ));
    }

    // Stage a unique file on the destination filesystem for atomic publication
    let pid = std::process::id();
    static STAGING_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = STAGING_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_filename = format!(".{release_name}.mango-{pid}-{sequence}.tmp");
    let temp_path = destination_dir.join(&temp_filename);

    struct CleanupGuard<'a>(&'a Path);
    impl<'a> Drop for CleanupGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }

    let mut staged = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;
    let _guard = CleanupGuard(&temp_path);
    std::io::copy(&mut File::open(source)?, &mut staged)?;
    staged.sync_all()?;
    drop(staged);

    // Verify the copied archive before committing it to permanent library
    verify_cbz_integrity(&temp_path)?;

    // Atomic no-replace publication. rename() may overwrite a competing writer
    // that won after the initial exists() check. Hard-link only the verified,
    // same-filesystem staging inode; the cleanup guard then removes its temp name.
    if let Err(error) = fs::hard_link(&temp_path, &destination) {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            if files_are_identical(source, &destination)? {
                return Ok(PublishOutcome::AlreadyPublished(destination));
            }
            return Err(AppError::new(409, "destination_conflict", "Another artifact occupies this canonical destination; replacement requires separate permission"));
        }
        return Err(error.into());
    }

    tracing::info!("LIBRARY: published to {}", destination.display());
    Ok(PublishOutcome::Published(destination))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_test_cbz(path: &Path, marker: &str) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(format!("<ComicInfo><Series>{marker}</Series></ComicInfo>").as_bytes()).unwrap();
        zip.start_file("001.jpg", options).unwrap();
        zip.write_all(b"fake image data").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn canonical_naming_rules() {
        assert_eq!(canonical_release_name("Blue Lock", true, 1.0), "Blue Lock v01");
        assert_eq!(canonical_release_name("Blue Lock", true, 12.0), "Blue Lock v12");
        assert_eq!(canonical_release_name("Blue Lock", true, 12.5), "Blue Lock v12.5");
        assert_eq!(canonical_release_name("Blue Lock", false, 1.0), "Blue Lock ch001");
        assert_eq!(canonical_release_name("Blue Lock", false, 105.0), "Blue Lock ch105");
        assert_eq!(canonical_release_name("Blue Lock", false, 12.5), "Blue Lock ch12.5");
    }

    #[test]
    fn series_name_sanitization() {
        assert_eq!(sanitize_series_name("Blue Lock").unwrap(), "Blue Lock");
        assert_eq!(sanitize_series_name("One Piece / 航海王").unwrap(), "One Piece _ 航海王");
        assert!(sanitize_series_name("").is_err());
        assert!(sanitize_series_name("..").is_err());
    }

    #[test]
    fn atomic_publish_and_duplicate_safety() {
        let dir = std::env::temp_dir().join(format!("mango-pub-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let source_dir = dir.join("staging");
        let library_dir = dir.join("library");
        fs::create_dir_all(&source_dir).unwrap();

        let source_file = source_dir.join("test_vol_1.cbz");
        create_test_cbz(&source_file, "Alpha");

        // 1. Initial publication succeeds
        let res = publish_cbz(&source_file, &library_dir, "Alpha Series", true, 1.0).unwrap();
        let published_path = match res {
            PublishOutcome::Published(p) => p,
            _ => panic!("expected Published outcome"),
        };
        assert!(published_path.exists());
        assert_eq!(published_path.file_name().unwrap(), "Alpha Series v01.cbz");

        // 2. Re-publishing identical file is a safe no-op (AlreadyPublished)
        let res2 = publish_cbz(&source_file, &library_dir, "Alpha Series", true, 1.0).unwrap();
        assert_eq!(res2, PublishOutcome::AlreadyPublished(published_path.clone()));

        // 3. Publishing a DIFFERENT file to the same destination REFUSES TO OVERWRITE
        let source_diff = source_dir.join("test_vol_1_diff.cbz");
        create_test_cbz(&source_diff, "DifferentContentBeta");
        let err = publish_cbz(&source_diff, &library_dir, "Alpha Series", true, 1.0).unwrap_err();
        assert_eq!(err.code, "destination_conflict");
        assert!(err.message.contains("refusing to overwrite"));

        // Original file in library was NOT overwritten
        let mut reader = File::open(&published_path).unwrap();
        let mut archive = zip::ZipArchive::new(&mut reader).unwrap();
        let mut xml = String::new();
        archive.by_name("ComicInfo.xml").unwrap().read_to_string(&mut xml).unwrap();
        assert!(xml.contains("Alpha"), "Original content must remain preserved");
    }
}

#[cfg(test)]
mod override_integrity_tests {
    use super::*;
    use std::io::Write;
    fn temp(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mango-override-{name}-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn archive(path: &Path, name: &str, bytes: &[u8]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn integrity_and_paths_remain_non_overridable() {
        let dir = temp("integrity");
        let file = dir.join("source.cbz");
        fs::write(&file, b"not a zip").unwrap();
        assert!(verify_cbz_integrity(&file).is_err());
        archive(&file, "../cover.jpg", b"unsafe cover");
        assert!(verify_cbz_integrity(&file).is_err());
        archive(&file, "cover.jpg", b"unique source image bytes");
        let mut bytes = fs::read(&file).unwrap();
        let pos = bytes
            .windows(25)
            .position(|w| w == b"unique source image bytes")
            .unwrap();
        bytes[pos] ^= 1;
        fs::write(&file, bytes).unwrap();
        assert!(
            verify_cbz_integrity(&file).is_err(),
            "Header-valid corrupt data must fail checksum validation"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn differing_existing_file_requires_separate_replacement_permission() {
        let dir = temp("collision");
        let source = dir.join("source.cbz");
        archive(&source, "cover.jpg", b"first cover");
        let destination = match publish_cbz(
            &source,
            &dir.join("library"),
            "Series X (master Edition)",
            true,
            4.0,
        )
        .unwrap()
        {
            PublishOutcome::Published(path) => path,
            _ => panic!(),
        };
        let original = fs::read(&destination).unwrap();
        archive(&source, "cover.jpg", b"second cover");
        assert_eq!(
            publish_cbz(
                &source,
                &dir.join("library"),
                "Series X (master Edition)",
                true,
                4.0
            )
            .unwrap_err()
            .code,
            "destination_conflict"
        );
        assert_eq!(fs::read(&destination).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod concurrent_publication_test {
    use super::*;
    use std::io::Write;
    #[test]
    fn concurrent_different_artifacts_never_replace_the_winner() {
        let dir = std::env::temp_dir().join(format!(
            "mango-publication-race-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut paths = vec![];
        for (name, marker) in [("a", 1u8), ("b", 2u8)] {
            let path = dir.join(format!("{name}.cbz"));
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            zip.start_file("cover.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&vec![marker; 100_000]).unwrap();
            zip.finish().unwrap();
            paths.push(path);
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = paths
            .into_iter()
            .map(|path| {
                let barrier = barrier.clone();
                let root = dir.join("library");
                std::thread::spawn(move || {
                    barrier.wait();
                    publish_cbz(&path, &root, "Series X", true, 4.0)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| r.as_ref().is_err_and(|e| e.code == "destination_conflict"))
                .count(),
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
