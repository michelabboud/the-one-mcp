//! Thumbnail generation using the `image` crate.
//!
//! Gated behind `#[cfg(feature = "image-embeddings")]` since it uses the
//! optional `image` crate dependency.

/// Generate a thumbnail of an image, constraining the longest side to
/// `max_dim` pixels. Uses Lanczos3 downsampling for quality.
///
/// # Errors
///
/// Returns a descriptive string if the input cannot be read or the output
/// cannot be written.
#[cfg(feature = "image-embeddings")]
pub fn generate_thumbnail(
    input: &std::path::Path,
    output: &std::path::Path,
    max_dim: u32,
) -> Result<(), String> {
    let img = image::open(input).map_err(|e| format!("image open: {e}"))?;
    let thumbnail = img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    thumbnail.save(output).map_err(|e| format!("thumbnail save: {e}"))?;
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    #[cfg(feature = "image-embeddings")]
    mod thumbnail_tests {
        use super::super::*;
        use std::path::PathBuf;

        fn tiny_png() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("images")
                .join("tiny.png")
        }

        #[test]
        fn test_generate_thumbnail_creates_output() {
            let input = tiny_png();
            let tmp = tempfile::tempdir().expect("tempdir");
            let output = tmp.path().join("thumb.png");

            generate_thumbnail(&input, &output, 64)
                .expect("thumbnail generation should succeed");

            assert!(output.exists(), "thumbnail file should exist after generation");
        }

        #[test]
        fn test_generate_thumbnail_creates_parent_dirs() {
            let input = tiny_png();
            let tmp = tempfile::tempdir().expect("tempdir");
            let output = tmp.path().join("nested").join("deep").join("thumb.png");

            generate_thumbnail(&input, &output, 64)
                .expect("thumbnail generation should succeed with nested dirs");

            assert!(output.exists(), "thumbnail should exist at nested path");
        }

        #[test]
        fn test_generate_thumbnail_nonexistent_input_returns_error() {
            let input = std::path::Path::new("/nonexistent/image.png");
            let tmp = tempfile::tempdir().expect("tempdir");
            let output = tmp.path().join("thumb.png");

            let result = generate_thumbnail(input, &output, 64);
            assert!(result.is_err(), "should return error for nonexistent input");
        }
    }
}
