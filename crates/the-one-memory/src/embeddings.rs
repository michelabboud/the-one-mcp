use async_trait::async_trait;

/// Trait for embedding text into vectors.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, String> {
        let results = self.embed_batch(&[text.to_string()]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| "empty embedding result".to_string())
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Local embeddings (fastembed) — only compiled with "local-embeddings" feature
// ══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "local-embeddings")]
mod local {
    use super::*;
    use std::sync::Arc;

    fn enum_from_name(fastembed_enum: &str, dims: usize) -> (fastembed::EmbeddingModel, usize) {
        let model = match fastembed_enum {
            "AllMiniLML6V2" => fastembed::EmbeddingModel::AllMiniLML6V2,
            "AllMiniLML12V2" => fastembed::EmbeddingModel::AllMiniLML12V2,
            "BGESmallENV15" => fastembed::EmbeddingModel::BGESmallENV15,
            "BGEBaseENV15" => fastembed::EmbeddingModel::BGEBaseENV15,
            "BGELargeENV15" => fastembed::EmbeddingModel::BGELargeENV15,
            "MultilingualE5Large" => fastembed::EmbeddingModel::MultilingualE5Large,
            "MultilingualE5Base" => fastembed::EmbeddingModel::MultilingualE5Base,
            "MultilingualE5Small" => fastembed::EmbeddingModel::MultilingualE5Small,
            "ParaphraseMLMiniLML12V2" => fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2,
            "NomicEmbedTextV1" => fastembed::EmbeddingModel::NomicEmbedTextV1,
            "NomicEmbedTextV15" => fastembed::EmbeddingModel::NomicEmbedTextV15,
            "MxbaiEmbedLargeV1" => fastembed::EmbeddingModel::MxbaiEmbedLargeV1,
            "GTEBaseENV15" => fastembed::EmbeddingModel::GTEBaseENV15,
            "GTELargeENV15" => fastembed::EmbeddingModel::GTELargeENV15,
            "AllMiniLML6V2Q" => fastembed::EmbeddingModel::AllMiniLML6V2Q,
            "BGEBaseENV15Q" => fastembed::EmbeddingModel::BGEBaseENV15Q,
            "BGELargeENV15Q" => fastembed::EmbeddingModel::BGELargeENV15Q,
            // New models added in fastembed 4.x
            "ModernBertEmbedLarge" => fastembed::EmbeddingModel::ModernBertEmbedLarge,
            "JinaEmbeddingsV2BaseCode" => fastembed::EmbeddingModel::JinaEmbeddingsV2BaseCode,
            "BGESmallZHV15" => fastembed::EmbeddingModel::BGESmallZHV15,
            "BGELargeZHV15" => fastembed::EmbeddingModel::BGELargeZHV15,
            "ParaphraseMLMpnetBaseV2" => fastembed::EmbeddingModel::ParaphraseMLMpnetBaseV2,
            // Quantized variants
            "BGESmallENV15Q" => fastembed::EmbeddingModel::BGESmallENV15Q,
            "AllMiniLML12V2Q" => fastembed::EmbeddingModel::AllMiniLML12V2Q,
            "NomicEmbedTextV15Q" => fastembed::EmbeddingModel::NomicEmbedTextV15Q,
            "ParaphraseMLMiniLML12V2Q" => fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2Q,
            "MxbaiEmbedLargeV1Q" => fastembed::EmbeddingModel::MxbaiEmbedLargeV1Q,
            "GTEBaseENV15Q" => fastembed::EmbeddingModel::GTEBaseENV15Q,
            "GTELargeENV15Q" => fastembed::EmbeddingModel::GTELargeENV15Q,
            _ => {
                tracing::warn!(
                    "Unknown fastembed enum '{}', falling back to BGELargeENV15",
                    fastembed_enum
                );
                fastembed::EmbeddingModel::BGELargeENV15
            }
        };
        (model, dims)
    }

    pub fn resolve_model(name: &str) -> (fastembed::EmbeddingModel, usize) {
        let name_lower = name.to_ascii_lowercase();
        let name_trimmed = name_lower.trim();

        // "default" → registry default (quality tier)
        if name_trimmed == "default" {
            let default = crate::models_registry::default_local_model();
            return enum_from_name(&default.fastembed_enum, default.dims);
        }

        // Tier alias (fast, balanced, quality, multilingual)
        // When multiple models share a tier, prefer installer_visible ones and pick largest dims
        // (canonical representative — e.g. multilingual → multilingual-e5-large at 1024 dims).
        let models = crate::models_registry::list_local_models();
        let tier_match = models
            .iter()
            .filter(|m| m.tier == name_trimmed && m.installer_visible && !m.deprecated)
            .max_by_key(|m| m.dims);
        if let Some(m) = tier_match {
            return enum_from_name(&m.fastembed_enum, m.dims);
        }

        // Model name match (case-insensitive)
        if let Some(m) = models
            .iter()
            .find(|m| m.name.to_ascii_lowercase() == name_trimmed)
        {
            return enum_from_name(&m.fastembed_enum, m.dims);
        }

        // Direct string match as last resort (for backward compat)
        match name_trimmed {
            "all-minilm-l6-v2" => (fastembed::EmbeddingModel::AllMiniLML6V2, 384),
            "all-minilm-l12-v2" => (fastembed::EmbeddingModel::AllMiniLML12V2, 384),
            "bge-small-en-v1.5" => (fastembed::EmbeddingModel::BGESmallENV15, 384),
            "bge-base-en-v1.5" => (fastembed::EmbeddingModel::BGEBaseENV15, 768),
            "bge-large-en-v1.5" => (fastembed::EmbeddingModel::BGELargeENV15, 1024),
            "multilingual-e5-large" => (fastembed::EmbeddingModel::MultilingualE5Large, 1024),
            "multilingual-e5-base" => (fastembed::EmbeddingModel::MultilingualE5Base, 768),
            "multilingual-e5-small" => (fastembed::EmbeddingModel::MultilingualE5Small, 384),
            "paraphrase-ml-minilm-l12-v2" => {
                (fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2, 384)
            }
            "nomic-embed-text-v1" => (fastembed::EmbeddingModel::NomicEmbedTextV1, 768),
            "nomic-embed-text-v1.5" => (fastembed::EmbeddingModel::NomicEmbedTextV15, 768),
            "mxbai-embed-large-v1" => (fastembed::EmbeddingModel::MxbaiEmbedLargeV1, 1024),
            "gte-base-en-v1.5" => (fastembed::EmbeddingModel::GTEBaseENV15, 768),
            "gte-large-en-v1.5" => (fastembed::EmbeddingModel::GTELargeENV15, 1024),
            "all-minilm-l6-v2-q" | "fast-q" => (fastembed::EmbeddingModel::AllMiniLML6V2Q, 384),
            "bge-base-en-v1.5-q" | "balanced-q" => (fastembed::EmbeddingModel::BGEBaseENV15Q, 768),
            "bge-large-en-v1.5-q" | "quality-q" => {
                (fastembed::EmbeddingModel::BGELargeENV15Q, 1024)
            }
            _ => {
                tracing::warn!(
                    "Unknown embedding model '{}', falling back to BGE-large-en-v1.5",
                    name
                );
                (fastembed::EmbeddingModel::BGELargeENV15, 1024)
            }
        }
    }

    pub struct ModelInfo {
        pub name: &'static str,
        pub aliases: &'static [&'static str],
        pub dims: usize,
        pub description: &'static str,
        pub size_mb: u32,
        pub latency_vs_fast: &'static str,
        pub multilingual: bool,
    }

    /// List all available local embedding models.
    pub fn available_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                name: "all-MiniLM-L6-v2",
                aliases: &["fast", "default-fast"],
                dims: 384,
                description: "Fast, small. Good for getting started.",
                size_mb: 23,
                latency_vs_fast: "fastest",
                multilingual: false,
            },
            ModelInfo {
                name: "BGE-base-en-v1.5",
                aliases: &["balanced"],
                dims: 768,
                description: "Good quality/speed tradeoff.",
                size_mb: 50,
                latency_vs_fast: "~2x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "BGE-large-en-v1.5",
                aliases: &["quality", "default"],
                dims: 1024,
                description: "Best local quality. Recommended default.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "multilingual-e5-large",
                aliases: &["multilingual"],
                dims: 1024,
                description: "Best for non-English or mixed-language projects.",
                size_mb: 220,
                latency_vs_fast: "~5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "multilingual-e5-base",
                aliases: &[],
                dims: 768,
                description: "Multilingual, moderate size.",
                size_mb: 90,
                latency_vs_fast: "~3x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "multilingual-e5-small",
                aliases: &[],
                dims: 384,
                description: "Lightweight multilingual option.",
                size_mb: 45,
                latency_vs_fast: "~1.5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "paraphrase-ml-minilm-l12-v2",
                aliases: &[],
                dims: 384,
                description: "Paraphrase-tuned multilingual model.",
                size_mb: 45,
                latency_vs_fast: "~1.5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "nomic-embed-text-v1.5",
                aliases: &[],
                dims: 768,
                description: "Nomic AI model. Good quality, 8192 token context.",
                size_mb: 55,
                latency_vs_fast: "~2x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "mxbai-embed-large-v1",
                aliases: &[],
                dims: 1024,
                description: "Mixedbread AI. Top-tier local quality.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "gte-large-en-v1.5",
                aliases: &[],
                dims: 1024,
                description: "Alibaba GTE. Strong English performance.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
        ]
    }

    /// Local embedding provider using fastembed-rs (ONNX Runtime).
    /// Runs offline, no API calls, no cost.
    pub struct FastEmbedProvider {
        model: Arc<fastembed::TextEmbedding>,
        dims: usize,
        model_name: String,
    }

    impl FastEmbedProvider {
        pub fn new(model_name: &str) -> Result<Self, String> {
            let (model_enum, dims) = resolve_model(model_name);

            let model = fastembed::TextEmbedding::try_new(
                fastembed::InitOptions::new(model_enum).with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed init failed: {e}"))?;

            Ok(Self {
                model: Arc::new(model),
                dims,
                model_name: model_name.to_string(),
            })
        }

        pub fn model_name(&self) -> &str {
            &self.model_name
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FastEmbedProvider {
        fn name(&self) -> &str {
            "fastembed-local"
        }
        fn dimensions(&self) -> usize {
            self.dims
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
            let texts = texts.to_vec();
            let model = Arc::clone(&self.model);
            tokio::task::spawn_blocking(move || {
                model
                    .embed(texts, None)
                    .map_err(|e| format!("fastembed embed failed: {e}"))
            })
            .await
            .map_err(|e| format!("tokio join error: {e}"))?
        }
    }
}

// Re-export local embeddings when feature is enabled
#[cfg(feature = "local-embeddings")]
pub use local::{available_models, resolve_model, FastEmbedProvider, ModelInfo};

// ══════════════════════════════════════════════════════════════════════════
// API Embedding Provider (always available — no native dependencies)
// ══════════════════════════════════════════════════════════════════════════

/// API-based embedding provider for OpenAI-compatible endpoints.
/// Works with OpenAI, Voyage, Cohere, LiteLLM, etc.
pub struct ApiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dims: usize,
}

impl ApiEmbeddingProvider {
    pub fn new(base_url: &str, api_key: Option<&str>, model: &str, dims: usize) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = api_key {
            if let Ok(value) = format!("Bearer {key}").parse() {
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("reqwest client build should succeed");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dims,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    fn name(&self) -> &str {
        "api-embedding"
    }
    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("embedding API request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("embedding API error {status}: {body}"));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("embedding API response parse failed: {e}"))?;

        let data = json["data"]
            .as_array()
            .ok_or("missing 'data' array in embedding API response")?;

        let mut results = Vec::with_capacity(data.len());
        for item in data {
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or("missing 'embedding' in response item")?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            results.push(embedding);
        }

        Ok(results)
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "local-embeddings")]
    mod local_tests {
        use super::*;
        use std::sync::LazyLock;

        static PROVIDER: LazyLock<FastEmbedProvider> =
            LazyLock::new(|| FastEmbedProvider::new("fast").expect("should init"));

        #[test]
        fn test_resolve_model_tier_aliases() {
            let (_, dims) = resolve_model("fast");
            assert_eq!(dims, 384);
            let (_, dims) = resolve_model("balanced");
            assert_eq!(dims, 768);
            let (_, dims) = resolve_model("quality");
            assert_eq!(dims, 1024);
            let (_, dims) = resolve_model("multilingual");
            assert_eq!(dims, 1024);
        }

        #[test]
        fn test_resolve_model_full_names() {
            let (_, dims) = resolve_model("all-MiniLM-L6-v2");
            assert_eq!(dims, 384);
            let (_, dims) = resolve_model("BGE-base-en-v1.5");
            assert_eq!(dims, 768);
            let (_, dims) = resolve_model("BGE-large-en-v1.5");
            assert_eq!(dims, 1024);
        }

        #[test]
        fn test_resolve_model_unknown_falls_back() {
            let (_, dims) = resolve_model("nonexistent-model");
            assert_eq!(dims, 1024);
        }

        #[test]
        fn test_available_models_not_empty() {
            let models = available_models();
            assert!(models.len() >= 4);
            assert_eq!(models[0].name, "all-MiniLM-L6-v2");
        }

        #[test]
        fn test_resolve_model_default_is_quality() {
            let (_, dims) = resolve_model("default");
            assert_eq!(
                dims, 1024,
                "default should resolve to quality tier (1024 dims)"
            );
        }

        #[test]
        fn test_available_models_includes_all_registry_entries() {
            let models = available_models();
            assert!(
                models.len() >= 7,
                "expected at least 7 models, got {}",
                models.len()
            );
            let names: Vec<&str> = models.iter().map(|m| m.name).collect();
            assert!(names.contains(&"multilingual-e5-small"));
            assert!(names.contains(&"multilingual-e5-base"));
            assert!(names.contains(&"paraphrase-ml-minilm-l12-v2"));
        }

        #[tokio::test]
        async fn test_fastembed_produces_correct_dimension_vectors() {
            let provider = &*PROVIDER;
            assert_eq!(provider.dimensions(), 384);
            let result = provider
                .embed_single("hello world")
                .await
                .expect("should embed");
            assert_eq!(result.len(), 384);
            assert!(
                result.iter().any(|&v| v != 0.0),
                "vector should not be all zeros"
            );
        }

        #[tokio::test]
        async fn test_fastembed_batch_produces_different_vectors() {
            let provider = &*PROVIDER;
            let texts = vec!["hello".to_string(), "goodbye".to_string()];
            let results = provider.embed_batch(&texts).await.expect("should embed");
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].len(), 384);
            assert_eq!(results[1].len(), 384);
            assert_ne!(
                results[0], results[1],
                "different texts should produce different vectors"
            );
        }
    }
}
