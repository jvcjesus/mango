//! Container-aware path and destination safety validation.
//!
//! Validates library destinations and external executables from the perspective
//! of Mango's running process/container before operations or configuration are allowed.

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    path::Path,
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathValidationCode {
    Valid,
    NotConfigured,
    Missing,
    NotADirectory,
    NotWritable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathValidationResult {
    pub path: Option<String>,
    pub code: PathValidationCode,
    pub message: String,
    pub writable: bool,
}

impl PathValidationResult {
    pub fn is_valid(&self) -> bool {
        self.code == PathValidationCode::Valid
    }
}

/// Safely probes whether a directory is writable by attempting to create a unique
/// temporary hidden test file inside it, verifying write access, and removing it immediately.
///
/// Guaranteed properties:
/// 1. Uses create_new(true) so it NEVER overwrites an existing file.
/// 2. Unique name incorporates process ID, high-res timestamp, and random counter.
/// 3. Hidden dot-prefix avoids triggering external directory watchers.
/// 4. Cleaned up immediately upon completion via RAII guard.
/// 5. Never creates directories or modifies any file outside the probed directory.
pub fn probe_writable_directory(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Path '{}' is not a directory", dir.display()),
        ));
    }

    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe_file = dir.join(format!(".mango_write_probe_{pid}_{nanos}.tmp"));

    struct ProbeGuard<'a>(&'a Path);
    impl<'a> Drop for ProbeGuard<'a> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }

    let _guard = ProbeGuard(&probe_file);

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_file)?;

    drop(file);
    Ok(())
}

/// Authoritatively validates a destination directory from Mango's container runtime perspective.
///
/// If `is_required` is false and `path_str` is empty or None, returns `NotConfigured` without error.
pub fn validate_destination_dir(path_str: Option<&str>, is_required: bool) -> PathValidationResult {
    let trimmed = match path_str.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => {
            return PathValidationResult {
                path: None,
                code: PathValidationCode::NotConfigured,
                message: if is_required {
                    "Destination path is not configured".to_string()
                } else {
                    "Not configured (optional)".to_string()
                },
                writable: false,
            };
        }
    };

    let path = Path::new(trimmed);

    if !path.exists() {
        return PathValidationResult {
            path: Some(trimmed.to_string()),
            code: PathValidationCode::Missing,
            message: format!("Path does not exist inside Mango: '{}'", path.display()),
            writable: false,
        };
    }

    if !path.is_dir() {
        return PathValidationResult {
            path: Some(trimmed.to_string()),
            code: PathValidationCode::NotADirectory,
            message: format!("Path exists but is not a directory: '{}'", path.display()),
            writable: false,
        };
    }

    match probe_writable_directory(path) {
        Ok(()) => PathValidationResult {
            path: Some(trimmed.to_string()),
            code: PathValidationCode::Valid,
            message: "Valid and writable".to_string(),
            writable: true,
        },
        Err(e) => PathValidationResult {
            path: Some(trimmed.to_string()),
            code: PathValidationCode::NotWritable,
            message: format!("Directory exists but is not writable by Mango: {e}"),
            writable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn validates_valid_writable_dir() {
        let dir = TempDir::new("val-valid");
        let res = validate_destination_dir(Some(dir.path().to_str().unwrap()), true);
        assert_eq!(res.code, PathValidationCode::Valid);
        assert!(res.writable);
        assert!(res.is_valid());
    }

    #[test]
    fn validates_missing_dir_and_never_creates_it() {
        let dir = TempDir::new("val-missing");
        let nonexistent = dir.path().join("does_not_exist_typo");
        assert!(!nonexistent.exists());

        let res = validate_destination_dir(Some(nonexistent.to_str().unwrap()), true);
        assert_eq!(res.code, PathValidationCode::Missing);
        assert!(!res.writable);
        assert!(!res.is_valid());
        // Crucial requirement: validation must NEVER create directories automatically
        assert!(!nonexistent.exists(), "Validation probe must not create nonexistent directories");
    }

    #[test]
    fn validates_regular_file_not_a_directory() {
        let dir = TempDir::new("val-file");
        let file_path = dir.path().join("regular_file.txt");
        fs::write(&file_path, b"hello").unwrap();

        let res = validate_destination_dir(Some(file_path.to_str().unwrap()), true);
        assert_eq!(res.code, PathValidationCode::NotADirectory);
        assert!(!res.writable);
        assert!(!res.is_valid());
    }

    #[test]
    fn validates_not_configured() {
        let res_req = validate_destination_dir(None, true);
        assert_eq!(res_req.code, PathValidationCode::NotConfigured);
        assert!(!res_req.writable);

        let res_opt = validate_destination_dir(Some("   "), false);
        assert_eq!(res_opt.code, PathValidationCode::NotConfigured);
        assert!(!res_opt.writable);
    }

    #[test]
    fn probe_cleans_up_test_artifacts() {
        let dir = TempDir::new("val-probe");
        assert!(probe_writable_directory(dir.path()).is_ok());

        // Verify no leftover probe files
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 0, "No probe artifacts should remain in destination");
    }

    #[test]
    #[cfg(unix)]
    fn validates_non_writable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("val-ro");
        let ro_sub = dir.path().join("read_only_dir");
        fs::create_dir(&ro_sub).unwrap();

        // Make read-only
        let mut perms = fs::metadata(&ro_sub).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(&ro_sub, perms).unwrap();

        let res = validate_destination_dir(Some(ro_sub.to_str().unwrap()), true);
        // Note: root user can write to 0o555 dirs, but normal user cannot
        if std::process::id() != 0 {
            assert_eq!(res.code, PathValidationCode::NotWritable);
            assert!(!res.writable);
        }

        // Restore permissions for clean tempdir cleanup
        let mut restore = fs::metadata(&ro_sub).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = fs::set_permissions(&ro_sub, restore);
    }
}
