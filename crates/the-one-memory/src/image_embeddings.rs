//! Image embedding provider trait and fastembed-backed implementation.
//!
//! The fastembed implementation is only compiled when the `image-embeddings`
//! feature is active (which requires `local-embeddings` + `image` crate).

use async_trait::async_trait;
use std::path::PathBuf;

/// Trait for generating vector embeddings from images.
#[async_trait]
pub trait ImageEmbeddingProvider: Send + Sync {
    /// Human-readable name for this provider.
    fn name(&self) -> &str;

    /// Output vector dimensions for this model.
    fn dimensions(&self) -> usize;

    /// Embed a single image from a filesystem path.
    async fn embed_image(&self, path: &std::path::Path) -> Result<Vec<f32>, String> {
        let results = self.embed_batch(&[path.to_path_buf()]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| "empty image embedding result".to_string())
    }

    /// Embed a batch of images from filesystem paths.
    async fn embed_batch(&self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>, String>;
}

// ══════════════════════════════════════════════════════════════════════════
// Local image embeddings (fastembed) — only compiled with "image-embeddings"
// ══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "image-embeddings")]
mod local_image {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Map a fastembed_enum string from the registry to a fastembed::ImageEmbeddingModel.
    pub fn image_enum_from_name(fastembed_enum: &str) -> Option<fastembed::ImageEmbeddingModel> {
        let model = match fastembed_enum {
            "NomicEmbedVisionV15" => fastembed::ImageEmbeddingModel::NomicEmbedVisionV15,
            "ClipVitB32" => fastembed::ImageEmbeddingModel::ClipVitB32,
            "Resnet50" => fastembed::ImageEmbeddingModel::Resnet50,
            "UnicomVitB16" => fastembed::ImageEmbeddingModel::UnicomVitB16,
            "UnicomVitB32" => fastembed::ImageEmbeddingModel::UnicomVitB32,
            _ => return None,
        };
        Some(model)
    }

    /// Resolve a model name or "default" to a `(fastembed::ImageEmbeddingModel, dims)` pair.
    ///
    /// Accepts:
    /// - `"default"` → registry default (NomicEmbedVisionV15)
    /// - Human-readable name from TOML (e.g. `"nomic-embed-vision-v1.5"`)
    /// - fastembed_enum string directly (e.g. `"NomicEmbedVisionV15"`)
    pub fn resolve_image_model(name: &str) -> (fastembed::ImageEmbeddingModel, usize) {
        let name_lower = name.to_ascii_lowercase();
        let name_trimmed = name_lower.trim();

        if name_trimmed == "default" {
            let default = crate::models_registry::default_image_model();
            let model = image_enum_from_name(&default.fastembed_enum).unwrap_or_else(|| {
                tracing::warn!(
                    "Unknown image fastembed_enum '{}' in registry default, falling back to NomicEmbedVisionV15",
                    default.fastembed_enum
                );
                fastembed::ImageEmbeddingModel::NomicEmbedVisionV15
            });
            return (model, default.dims);
        }

        let models = crate::models_registry::list_image_models();

        // Human-readable name match (case-insensitive)
        if let Some(m) = models
            .iter()
            .find(|m| m.name.to_ascii_lowercase() == name_trimmed)
        {
            let model = image_enum_from_name(&m.fastembed_enum).unwrap_or_else(|| {
                tracing::warn!(
                    "Unknown image fastembed_enum '{}' for '{}', falling back to NomicEmbedVisionV15",
                    m.fastembed_enum, m.name
                );
                fastembed::ImageEmbeddingModel::NomicEmbedVisionV15
            });
            return (model, m.dims);
        }

        // Direct fastembed_enum string match
        if let Some(model) = image_enum_from_name(name) {
            let dims = models
                .iter()
                .find(|m| m.fastembed_enum == name)
                .map(|m| m.dims)
                .unwrap_or(768);
            return (model, dims);
        }

        tracing::warn!(
            "Unknown image embedding model '{}', falling back to NomicEmbedVisionV15",
            name
        );
        (fastembed::ImageEmbeddingModel::NomicEmbedVisionV15, 768)
    }

    /// Local image embedding provider using fastembed-rs (ONNX Runtime).
    /// Runs offline, no API calls, no cost.
    ///
    /// Wraps `fastembed::ImageEmbedding` in `Arc<Mutex>` because `embed()`
    /// takes `&mut self`.
    pub struct FastEmbedImageProvider {
        model: Arc<Mutex<fastembed::ImageEmbedding>>,
        dims: usize,
        model_name: String,
    }

    impl FastEmbedImageProvider {
        /// Initialise an image embedding model by name, tier, or `"default"`.
        ///
        /// Note: The model ONNX file is downloaded on first call (~95–700 MB
        /// depending on model) and cached in `.fastembed_cache/`.
        pub fn new(model_name: &str) -> Result<Self, String> {
            let (model_enum, dims) = resolve_image_model(model_name);

            let model = fastembed::ImageEmbedding::try_new(
                fastembed::ImageInitOptions::new(model_enum).with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed ImageEmbedding init failed: {e}"))?;

            Ok(Self {
                model: Arc::new(Mutex::new(model)),
                dims,
                model_name: model_name.to_string(),
            })
        }

        pub fn model_name(&self) -> &str {
            &self.model_name
        }
    }

    #[async_trait]
    impl ImageEmbeddingProvider for FastEmbedImageProvider {
        fn name(&self) -> &str {
            "fastembed-image-local"
        }

        fn dimensions(&self) -> usize {
            self.dims
        }

        async fn embed_batch(&self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>, String> {
            let paths = paths.to_vec();
            let model = Arc::clone(&self.model);
            tokio::task::spawn_blocking(move || {
                let mut model = model
                    .lock()
                    .map_err(|e| format!("image model lock poisoned: {e}"))?;
                model
                    .embed(paths, None)
                    .map_err(|e| format!("fastembed image embed failed: {e}"))
            })
            .await
            .map_err(|e| format!("tokio join error: {e}"))?
        }
    }
}

#[cfg(feature = "image-embeddings")]
pub use local_image::{image_enum_from_name, resolve_image_model, FastEmbedImageProvider};

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    #[cfg(feature = "image-embeddings")]
    mod image_tests {
        use super::super::*;

        #[test]
        fn test_resolve_image_model_default() {
            // Just verifies no panic — "default" → NomicEmbedVisionV15
            let (_, dims) = resolve_image_model("default");
            assert_eq!(
                dims, 768,
                "default image model should be NomicEmbedVisionV15 (768 dims)"
            );
        }

        #[test]
        fn test_resolve_image_model_by_name() {
            let (_, dims) = resolve_image_model("nomic-embed-vision-v1.5");
            assert_eq!(dims, 768);
        }

        #[test]
        fn test_resolve_image_model_clip() {
            let (_, dims) = resolve_image_model("clip-ViT-B-32-vision");
            assert_eq!(dims, 512);
        }

        #[test]
        fn test_resolve_image_model_resnet50() {
            let (_, dims) = resolve_image_model("resnet50-onnx");
            assert_eq!(dims, 2048);
        }

        #[test]
        fn test_resolve_image_model_unknown_fallback() {
            // Unknown models fall back to NomicEmbedVisionV15 at 768 dims
            let (_, dims) = resolve_image_model("totally-unknown-model");
            assert_eq!(dims, 768);
        }

        #[test]
        fn test_image_enum_from_name_all_variants() {
            assert!(image_enum_from_name("NomicEmbedVisionV15").is_some());
            assert!(image_enum_from_name("ClipVitB32").is_some());
            assert!(image_enum_from_name("Resnet50").is_some());
            assert!(image_enum_from_name("UnicomVitB16").is_some());
            assert!(image_enum_from_name("UnicomVitB32").is_some());
        }

        #[test]
        fn test_image_enum_from_name_unknown_returns_none() {
            assert!(image_enum_from_name("DoesNotExist").is_none());
        }
    }
}
