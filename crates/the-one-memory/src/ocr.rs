//! OCR text extraction from images via tesseract.
//!
//! The `image-ocr` feature is off by default because it requires the
//! Tesseract OCR system library (`libtesseract-dev`) to be installed.
//! Enable it with `--features image-ocr` (which also implies `image-embeddings`).

/// Extract text from an image using Tesseract OCR.
///
/// # Arguments
///
/// * `image_path` — path to the image file.
/// * `language` — Tesseract language code, e.g. `"eng"` for English.
///
/// # Errors
///
/// Returns an error string if the path is not valid UTF-8, if Tesseract
/// fails to initialise, or if text extraction fails.
#[cfg(feature = "image-ocr")]
pub fn extract_text(image_path: &std::path::Path, language: &str) -> Result<String, String> {
    let path_str = image_path
        .to_str()
        .ok_or_else(|| "image path is not valid UTF-8".to_string())?;
    let tess = tesseract::Tesseract::new(None, Some(language))
        .map_err(|e| format!("tesseract init: {e}"))?;
    let tess = tess
        .set_image(path_str)
        .map_err(|e| format!("set_image: {e}"))?;
    let text = tess.get_text().map_err(|e| format!("get_text: {e}"))?;
    Ok(text.trim().to_string())
}

/// Feature-disabled implementation for builds without `image-ocr`.
///
/// Returns `Err` immediately so callers can handle the case at runtime
/// without needing compile-time feature detection.
#[cfg(not(feature = "image-ocr"))]
pub fn extract_text(_: &std::path::Path, _: &str) -> Result<String, String> {
    Err("OCR not enabled at compile time (feature image-ocr)".to_string())
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_returns_error_when_feature_disabled() {
        // When image-ocr is disabled (the default), extract_text must return Err
        // with a message explaining the feature requirement.
        #[cfg(not(feature = "image-ocr"))]
        {
            let result = extract_text(std::path::Path::new("any.png"), "eng");
            assert!(result.is_err(), "feature-disabled path must return Err");
            let msg = result.unwrap_err();
            assert!(
                msg.contains("image-ocr"),
                "error should mention image-ocr feature, got: {msg}"
            );
        }
        // When image-ocr IS enabled this test body is empty — the enabled-path
        // tests require tesseract system libs and are not run in CI.
        #[cfg(feature = "image-ocr")]
        let _ = (); // nothing to test here without tesseract system libs
    }
}
