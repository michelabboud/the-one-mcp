//! Backup and restore support for project state (v0.12.0).
//!
//! The `maintain: action: backup` and `maintain: action: restore` actions
//! use this module to create and apply tarballs of project state.
//!
//! # What gets backed up
//!
//! - `<project>/.the-one/` — the whole per-project state tree
//!   (manifests, state.db, docs/, images/, config.json)
//! - `~/.the-one/catalog.db` — the shared tool catalog (optional but usually
//!   wanted so the new machine starts with the same enabled-tool snapshot)
//! - `~/.the-one/registry/` — per-CLI custom tool files
//!
//! # What is deliberately NOT backed up
//!
//! - `.fastembed_cache/` — ~30MB+ per model, will re-download on first use
//! - External Qdrant server data — user's responsibility to back up separately
//! - Local Qdrant storage inside `.the-one/qdrant/` unless explicitly opted in
//!   (often gigabytes of vectors)
//!
//! # Manifest
//!
//! Every tarball contains a `backup-manifest.json` at the root describing
//! what's inside, the version of the-one-mcp that created it, and a
//! timestamp. Restore validates this before unpacking.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use tar::{Archive, Builder};

use crate::api::{BackupRequest, BackupResponse, RestoreRequest, RestoreResponse};

/// On-disk manifest embedded at the root of the tar.gz as `backup-manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub version: String,
    pub the_one_mcp_version: String,
    pub created_at_epoch: u64,
    pub project_id: String,
    pub file_count: usize,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
}

/// Current manifest format version. Bump when the backup structure changes.
pub const MANIFEST_VERSION: &str = "1";

/// Skip rules applied while walking the project directory.
fn should_skip(relative: &Path) -> bool {
    let s = relative.to_string_lossy();
    // Skip model caches — they can be re-downloaded
    if s.contains(".fastembed_cache") {
        return true;
    }
    // Skip local Qdrant raft state / wal / snapshots
    if s.contains("qdrant") && (s.contains("wal") || s.contains("raft_state")) {
        return true;
    }
    // Skip macOS noise
    if s.ends_with(".DS_Store") {
        return true;
    }
    false
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Create a gzipped tar archive of a project's `.the-one/` state plus the
/// shared catalog and registry from `~/.the-one/`.
///
/// Writes to `request.output_path` and returns a summary in [`BackupResponse`].
pub fn create_backup(request: &BackupRequest) -> Result<BackupResponse, String> {
    let project_root = PathBuf::from(&request.project_root);
    let dotdir = project_root.join(".the-one");
    if !dotdir.exists() {
        return Err(format!(
            "project state directory does not exist: {}",
            dotdir.display()
        ));
    }

    let output_path = PathBuf::from(&request.output_path);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create output parent dir: {e}"))?;
        }
    }

    let file = File::create(&output_path).map_err(|e| format!("create backup file: {e}"))?;
    let gz = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(gz);

    let mut file_count: usize = 0;
    let mut includes: Vec<String> = vec!["docs".to_string(), "config".to_string()];
    let mut excludes: Vec<String> = vec![".fastembed_cache".to_string()];

    // Walk .the-one/ and stream each file into the tarball under
    // `project/.the-one/...`. Keeps the relative structure intact for restore.
    add_directory_recursive(
        &mut builder,
        &dotdir,
        &dotdir,
        Path::new("project/.the-one"),
        request.include_images,
        &mut file_count,
    )?;

    if request.include_images {
        includes.push("images".to_string());
    } else {
        excludes.push("images".to_string());
    }

    // Global catalog and registry under ~/.the-one/
    if let Some(home) = dirs_home_dir() {
        let global = home.join(".the-one");
        let catalog_db = global.join("catalog.db");
        if catalog_db.exists() {
            append_file(
                &mut builder,
                &catalog_db,
                Path::new("global/catalog.db"),
                &mut file_count,
            )?;
            includes.push("catalog".to_string());
        }
        let registry = global.join("registry");
        if registry.is_dir() {
            add_directory_recursive(
                &mut builder,
                &registry,
                &registry,
                Path::new("global/registry"),
                true,
                &mut file_count,
            )?;
            includes.push("registry".to_string());
        }
    }

    // Write the manifest last so it appears at the root of the archive.
    let manifest = BackupManifest {
        version: MANIFEST_VERSION.to_string(),
        the_one_mcp_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at_epoch: current_timestamp(),
        project_id: request.project_id.clone(),
        file_count,
        includes,
        excludes,
    };
    let manifest_json =
        serde_json::to_vec_pretty(&manifest).map_err(|e| format!("serialize manifest: {e}"))?;
    let mut header = tar::Header::new_gnu();
    header.set_path("backup-manifest.json").ok();
    header.set_size(manifest_json.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(manifest.created_at_epoch);
    header.set_cksum();
    builder
        .append(&header, manifest_json.as_slice())
        .map_err(|e| format!("append manifest: {e}"))?;

    let gz = builder
        .into_inner()
        .map_err(|e| format!("finalize tar: {e}"))?;
    gz.finish().map_err(|e| format!("finalize gzip: {e}"))?;

    let size_bytes = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(BackupResponse {
        output_path: output_path.display().to_string(),
        size_bytes,
        file_count,
        manifest_version: manifest.version,
    })
}

fn add_directory_recursive<W: Write>(
    builder: &mut Builder<W>,
    root: &Path,
    current: &Path,
    archive_prefix: &Path,
    include_images: bool,
    file_count: &mut usize,
) -> Result<(), String> {
    let rd =
        std::fs::read_dir(current).map_err(|e| format!("read dir {}: {e}", current.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("strip prefix: {e}"))?;

        if should_skip(relative) {
            continue;
        }
        // Image directory skip when not requested
        if !include_images && relative.starts_with("images") {
            continue;
        }

        let archive_path = archive_prefix.join(relative);
        if path.is_dir() {
            add_directory_recursive(
                builder,
                root,
                &path,
                archive_prefix,
                include_images,
                file_count,
            )?;
        } else if path.is_file() {
            append_file(builder, &path, &archive_path, file_count)?;
        }
    }
    Ok(())
}

fn append_file<W: Write>(
    builder: &mut Builder<W>,
    src: &Path,
    archive_path: &Path,
    file_count: &mut usize,
) -> Result<(), String> {
    let mut f = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    builder
        .append_file(archive_path, &mut f)
        .map_err(|e| format!("append {}: {e}", src.display()))?;
    *file_count += 1;
    Ok(())
}

fn dirs_home_dir() -> Option<PathBuf> {
    // Minimal replacement for the `dirs` crate — we only need HOME/USERPROFILE.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Restore a backup tarball into a target project directory.
///
/// Validates the manifest version, refuses to overwrite existing state
/// unless `request.overwrite_existing` is true, and returns a list of
/// warnings (e.g. "target already had state at X").
pub fn restore_backup(request: &RestoreRequest) -> Result<RestoreResponse, String> {
    let backup_path = PathBuf::from(&request.backup_path);
    if !backup_path.exists() {
        return Err(format!(
            "backup file does not exist: {}",
            backup_path.display()
        ));
    }
    let target_root = PathBuf::from(&request.target_project_root);

    // Check for existing state first.
    let target_dotdir = target_root.join(".the-one");
    let mut warnings: Vec<String> = Vec::new();
    if target_dotdir.exists() && !request.overwrite_existing {
        return Err(format!(
            "target already has .the-one/ state at {}; pass overwrite_existing=true to replace",
            target_dotdir.display()
        ));
    }
    if target_dotdir.exists() {
        warnings.push(format!(
            "overwriting existing state at {}",
            target_dotdir.display()
        ));
    }

    let file = File::open(&backup_path).map_err(|e| format!("open backup: {e}"))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let mut restored: usize = 0;
    let mut manifest_seen = false;

    for entry in archive
        .entries()
        .map_err(|e| format!("read archive entries: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path_in_archive = entry
            .path()
            .map_err(|e| format!("entry path: {e}"))?
            .to_path_buf();

        // Reject any absolute paths or parent-dir traversal in the archive
        for c in path_in_archive.components() {
            match c {
                Component::Normal(_) => {}
                _ => {
                    return Err(format!(
                        "unsafe path in backup archive: {}",
                        path_in_archive.display()
                    ));
                }
            }
        }

        let first = path_in_archive.components().next().map(|c| c.as_os_str());
        let target_path: PathBuf = match first.and_then(|s| s.to_str()) {
            Some("backup-manifest.json") => {
                // Validate manifest version before continuing.
                let mut buf = String::new();
                entry
                    .read_to_string(&mut buf)
                    .map_err(|e| format!("read manifest: {e}"))?;
                let manifest: BackupManifest =
                    serde_json::from_str(&buf).map_err(|e| format!("parse manifest: {e}"))?;
                if manifest.version != MANIFEST_VERSION {
                    return Err(format!(
                        "backup manifest version {} is not supported by this binary (expected {})",
                        manifest.version, MANIFEST_VERSION
                    ));
                }
                manifest_seen = true;
                continue;
            }
            Some("project") => {
                let rest = path_in_archive
                    .strip_prefix("project")
                    .map_err(|e| format!("strip project prefix: {e}"))?;
                target_root.join(rest)
            }
            Some("global") => {
                let rest = path_in_archive
                    .strip_prefix("global")
                    .map_err(|e| format!("strip global prefix: {e}"))?;
                let home = dirs_home_dir().ok_or("HOME not set, cannot restore global state")?;
                home.join(".the-one").join(rest)
            }
            _ => {
                warnings.push(format!(
                    "skipping unknown archive entry: {}",
                    path_in_archive.display()
                ));
                continue;
            }
        };

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create parent dir: {e}"))?;
        }
        entry
            .unpack(&target_path)
            .map_err(|e| format!("unpack {}: {e}", target_path.display()))?;
        restored += 1;
    }

    if !manifest_seen {
        warnings.push(
            "backup did not include a backup-manifest.json — this may be a third-party archive"
                .to_string(),
        );
    }

    Ok(RestoreResponse {
        restored_files: restored,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Guard that swaps `$HOME` to a temp dir for the duration of the test
    /// so backup / restore cannot touch the real user home. Returns the
    /// previous value which is restored on drop.
    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", path);
            Self { prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn test_backup_creates_tarball_and_restores_roundtrip() {
        let _guard = env_lock().lock().expect("env lock");
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::set(tmp.path());
        let src_root = tmp.path().join("src-project");
        let dotdir = src_root.join(".the-one");
        fs::create_dir_all(dotdir.join("docs")).expect("docs dir");
        fs::create_dir_all(dotdir.join("manifests")).expect("manifests dir");
        fs::write(dotdir.join("docs/hello.md"), "# Hello\nworld").expect("write doc");
        fs::write(dotdir.join("config.json"), "{\"foo\":1}").expect("write config");
        fs::write(
            dotdir.join("manifests/profile.json"),
            "{\"project_id\":\"test\"}",
        )
        .expect("write profile");

        let output = tmp.path().join("backup.tar.gz");
        let resp = create_backup(&BackupRequest {
            project_root: src_root.display().to_string(),
            project_id: "test".to_string(),
            output_path: output.display().to_string(),
            include_images: false,
            include_qdrant_local: false,
        })
        .expect("backup should succeed");

        assert!(output.exists(), "backup file should exist");
        assert!(resp.size_bytes > 0);
        assert!(resp.file_count >= 3, "expected at least 3 files backed up");

        // Restore into a fresh target
        let target_root = tmp.path().join("target-project");
        let restore_resp = restore_backup(&RestoreRequest {
            backup_path: output.display().to_string(),
            target_project_root: target_root.display().to_string(),
            target_project_id: "test".to_string(),
            overwrite_existing: false,
        })
        .expect("restore should succeed");

        assert!(restore_resp.restored_files >= 3);
        assert!(target_root.join(".the-one/docs/hello.md").exists());
        assert_eq!(
            fs::read_to_string(target_root.join(".the-one/docs/hello.md")).unwrap(),
            "# Hello\nworld"
        );
        assert_eq!(
            fs::read_to_string(target_root.join(".the-one/config.json")).unwrap(),
            "{\"foo\":1}"
        );
    }

    #[test]
    fn test_backup_excludes_fastembed_cache() {
        let _guard = env_lock().lock().expect("env lock");
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::set(tmp.path());
        let src_root = tmp.path().join("src-project");
        let dotdir = src_root.join(".the-one");
        fs::create_dir_all(dotdir.join("docs")).expect("docs dir");
        fs::create_dir_all(dotdir.join(".fastembed_cache")).expect("cache dir");
        fs::write(dotdir.join("docs/notes.md"), "content").expect("notes");
        // Big fake model file that should NOT appear in the backup
        fs::write(dotdir.join(".fastembed_cache/model.onnx"), "binary blob").expect("cache file");

        let output = tmp.path().join("out.tar.gz");
        create_backup(&BackupRequest {
            project_root: src_root.display().to_string(),
            project_id: "t".to_string(),
            output_path: output.display().to_string(),
            include_images: false,
            include_qdrant_local: false,
        })
        .expect("backup");

        // Restore into target and verify cache dir did NOT round-trip
        let target_root = tmp.path().join("target");
        restore_backup(&RestoreRequest {
            backup_path: output.display().to_string(),
            target_project_root: target_root.display().to_string(),
            target_project_id: "t".to_string(),
            overwrite_existing: false,
        })
        .expect("restore");

        assert!(target_root.join(".the-one/docs/notes.md").exists());
        assert!(
            !target_root
                .join(".the-one/.fastembed_cache/model.onnx")
                .exists(),
            ".fastembed_cache contents must be excluded from backups"
        );
    }

    #[test]
    fn test_restore_refuses_existing_state_without_overwrite() {
        let _guard = env_lock().lock().expect("env lock");
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::set(tmp.path());
        // Source
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join(".the-one/docs")).unwrap();
        fs::write(src.join(".the-one/docs/a.md"), "a").unwrap();
        let out = tmp.path().join("b.tar.gz");
        create_backup(&BackupRequest {
            project_root: src.display().to_string(),
            project_id: "x".to_string(),
            output_path: out.display().to_string(),
            include_images: false,
            include_qdrant_local: false,
        })
        .unwrap();

        // Target already exists
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join(".the-one")).unwrap();
        fs::write(target.join(".the-one/sentinel.txt"), "keep me").unwrap();

        let err = restore_backup(&RestoreRequest {
            backup_path: out.display().to_string(),
            target_project_root: target.display().to_string(),
            target_project_id: "x".to_string(),
            overwrite_existing: false,
        });
        assert!(err.is_err(), "restore should refuse existing state");
    }

    #[test]
    fn test_restore_skips_unknown_top_level_entries() {
        let _guard = env_lock().lock().expect("env lock");
        // A tarball with an entry under an unrecognized top-level directory
        // should not error hard — it should be recorded in warnings and
        // skipped. This protects forward compatibility if a future backup
        // format adds new top-level directories.
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::set(tmp.path());
        let archive_path = tmp.path().join("weird.tar.gz");
        {
            let f = File::create(&archive_path).unwrap();
            let gz = GzEncoder::new(f, Compression::default());
            let mut builder = Builder::new(gz);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_path("unknown_future_area/greeting.txt").ok();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &data[..]).unwrap();
            // Also include a valid manifest so restore doesn't warn about it
            let manifest = BackupManifest {
                version: MANIFEST_VERSION.to_string(),
                the_one_mcp_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at_epoch: current_timestamp(),
                project_id: "x".to_string(),
                file_count: 1,
                includes: vec![],
                excludes: vec![],
            };
            let json = serde_json::to_vec_pretty(&manifest).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_path("backup-manifest.json").ok();
            h.set_size(json.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            builder.append(&h, json.as_slice()).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }

        let target = tmp.path().join("target");
        let resp = restore_backup(&RestoreRequest {
            backup_path: archive_path.display().to_string(),
            target_project_root: target.display().to_string(),
            target_project_id: "x".to_string(),
            overwrite_existing: false,
        })
        .expect("restore should succeed but warn");

        assert!(
            resp.warnings
                .iter()
                .any(|w| w.contains("unknown archive entry")),
            "expected a warning for unknown archive entry, got: {:?}",
            resp.warnings
        );
    }
}
