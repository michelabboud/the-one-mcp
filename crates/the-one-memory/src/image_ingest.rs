//! Image discovery and hash-based change detection.
//!
//! Walks a directory tree, finds image files by extension, and hashes each
//! file with SHA-256 for efficient change detection (no re-embedding if
//! unchanged).
//!
//! This module has no feature flags — it is pure file I/O and always compiled.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Default image file extensions to recognise.
pub const DEFAULT_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Metadata for a single discovered image file.
#[derive(Debug, Clone)]
pub struct DiscoveredImage {
    /// Absolute path to the image file.
    pub path: PathBuf,
    /// Hex-encoded SHA-256 hash of the file contents.
    pub hash: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// File modification time as a Unix epoch timestamp (seconds).
    pub mtime_epoch: i64,
}

/// Walk `root` recursively and return all image files whose extension (case-
/// insensitive) is in `extensions`.
///
/// Hidden directories (names starting with `.`) are skipped to avoid
/// traversing `.git`, `.fastembed_cache`, etc.
pub fn discover_images(root: &Path, extensions: &[&str]) -> Vec<DiscoveredImage> {
    let mut results = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue, // skip unreadable directories
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if path.is_dir() {
                // Skip hidden directories
                if !name.starts_with('.') {
                    stack.push(path);
                }
                continue;
            }

            // Check extension
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if !extensions.iter().any(|e| *e == ext.as_str()) {
                continue;
            }

            // Read file and compute hash
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let hash = {
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let result = hasher.finalize();
                result.iter().map(|b| format!("{b:02x}")).collect::<String>()
            };

            let size_bytes = bytes.len() as u64;

            let mtime_epoch = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64)
                })
                .unwrap_or(0);

            results.push(DiscoveredImage {
                path,
                hash,
                size_bytes,
                mtime_epoch,
            });
        }
    }

    // Sort by path for deterministic ordering
    results.sort_by(|a, b| a.path.cmp(&b.path));
    results
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_images_dir() -> PathBuf {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest.join("tests").join("fixtures").join("images")
    }

    #[test]
    fn test_discover_images_finds_fixtures() {
        let dir = fixture_images_dir();
        let found = discover_images(&dir, DEFAULT_IMAGE_EXTENSIONS);
        assert!(
            !found.is_empty(),
            "should find at least tiny.png in fixtures dir"
        );
        let names: Vec<String> = found
            .iter()
            .map(|d| d.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.contains(&"tiny.png".to_string()),
            "should find tiny.png; found: {names:?}"
        );
    }

    #[test]
    fn test_discover_images_filters_extensions() {
        let dir = fixture_images_dir();
        // Request only .jpg — fixture dir only has .png files
        let found = discover_images(&dir, &["jpg"]);
        assert!(
            found.is_empty(),
            "should find no jpg files in fixtures dir, found: {found:?}"
        );
    }

    #[test]
    fn test_hash_deterministic() {
        let dir = fixture_images_dir();
        let first = discover_images(&dir, DEFAULT_IMAGE_EXTENSIONS);
        let second = discover_images(&dir, DEFAULT_IMAGE_EXTENSIONS);
        assert_eq!(
            first.len(),
            second.len(),
            "two runs should find same number of images"
        );
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(
                a.hash, b.hash,
                "hash for {:?} must be deterministic",
                a.path
            );
        }
    }

    #[test]
    fn test_discovered_image_has_positive_size() {
        let dir = fixture_images_dir();
        let found = discover_images(&dir, DEFAULT_IMAGE_EXTENSIONS);
        for img in &found {
            assert!(
                img.size_bytes > 0,
                "image {:?} should have size > 0",
                img.path
            );
        }
    }

    #[test]
    fn test_hidden_dirs_are_skipped() {
        // Build a temp dir with a hidden subdir containing an image
        let tmp = tempfile::tempdir().expect("tempdir");
        let hidden = tmp.path().join(".hidden");
        std::fs::create_dir_all(&hidden).expect("mkdir");
        std::fs::copy(
            fixture_images_dir().join("tiny.png"),
            hidden.join("tiny.png"),
        )
        .expect("copy");
        // Also place one in the root
        std::fs::copy(
            fixture_images_dir().join("tiny.png"),
            tmp.path().join("visible.png"),
        )
        .expect("copy");

        let found = discover_images(tmp.path(), DEFAULT_IMAGE_EXTENSIONS);
        let paths: Vec<_> = found.iter().map(|d| d.path.clone()).collect();
        assert_eq!(found.len(), 1, "should find only visible.png, got: {paths:?}");
        assert!(
            paths[0].ends_with("visible.png"),
            "should find visible.png, got: {paths:?}"
        );
    }
}
