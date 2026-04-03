use async_trait::async_trait;
use std::sync::Arc;

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

// ── Model catalog ─────────────────────────────────────────────────────────

/// Supported local embedding models with tier aliases.
///
/// | Tier        | Model                  | Dims | Download | Speed   |
/// |-------------|------------------------|------|----------|---------|
/// | fast        | all-MiniLM-L6-v2       | 384  | ~23MB    | ~30ms   |
/// | balanced    | BGE-base-en-v1.5       | 768  | ~50MB    | ~60ms   |
/// | quality     | BGE-large-en-v1.5      | 1024 | ~130MB   | ~120ms  |
/// | multilingual| multilingual-e5-large  | 1024 | ~220MB   | ~150ms  |
pub fn resolve_model(name: &str) -> (fastembed::EmbeddingModel, usize) {
    match name.to_ascii_lowercase().trim() {
        // ── Tier aliases ──
        "fast" | "default" | "all-minilm-l6-v2" => {
            (fastembed::EmbeddingModel::AllMiniLML6V2, 384)
        }
        "balanced" | "bge-base-en-v1.5" => (fastembed::EmbeddingModel::BGEBaseENV15, 768),
        "quality" | "bge-large-en-v1.5" => (fastembed::EmbeddingModel::BGELargeENV15, 1024),
        "multilingual" | "multilingual-e5-large" => {
            (fastembed::EmbeddingModel::MultilingualE5Large, 1024)
        }

        // ── Additional models by name ──
        "all-minilm-l12-v2" => (fastembed::EmbeddingModel::AllMiniLML12V2, 384),
        "bge-small-en-v1.5" => (fastembed::EmbeddingModel::BGESmallENV15, 384),
        "nomic-embed-text-v1" => (fastembed::EmbeddingModel::NomicEmbedTextV1, 768),
        "nomic-embed-text-v1.5" => (fastembed::EmbeddingModel::NomicEmbedTextV15, 768),
        "mxbai-embed-large-v1" => (fastembed::EmbeddingModel::MxbaiEmbedLargeV1, 1024),
        "gte-base-en-v1.5" => (fastembed::EmbeddingModel::GTEBaseENV15, 768),
        "gte-large-en-v1.5" => (fastembed::EmbeddingModel::GTELargeENV15, 1024),
        "multilingual-e5-small" => (fastembed::EmbeddingModel::MultilingualE5Small, 384),
        "multilingual-e5-base" => (fastembed::EmbeddingModel::MultilingualE5Base, 768),
        "paraphrase-ml-minilm-l12-v2" => {
            (fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2, 384)
        }

        // ── Quantized variants (smaller download, slight quality loss) ──
        "fast-q" | "all-minilm-l6-v2-q" => (fastembed::EmbeddingModel::AllMiniLML6V2Q, 384),
        "balanced-q" | "bge-base-en-v1.5-q" => (fastembed::EmbeddingModel::BGEBaseENV15Q, 768),
        "quality-q" | "bge-large-en-v1.5-q" => {
            (fastembed::EmbeddingModel::BGELargeENV15Q, 1024)
        }

        // ── Default fallback ──
        _ => {
            tracing::warn!(
                "Unknown embedding model '{}', falling back to all-MiniLM-L6-v2",
                name
            );
            (fastembed::EmbeddingModel::AllMiniLML6V2, 384)
        }
    }
}

/// List all available local embedding models.
pub fn available_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            name: "fast",
            aliases: &["all-MiniLM-L6-v2", "default"],
            dims: 384,
            description: "Fast, small. Best for getting started.",
        },
        ModelInfo {
            name: "balanced",
            aliases: &["BGE-base-en-v1.5"],
            dims: 768,
            description: "Good quality/speed tradeoff. Recommended for production.",
        },
        ModelInfo {
            name: "quality",
            aliases: &["BGE-large-en-v1.5"],
            dims: 1024,
            description: "Best local quality. Larger download and slower.",
        },
        ModelInfo {
            name: "multilingual",
            aliases: &["multilingual-e5-large"],
            dims: 1024,
            description: "Best for non-English or mixed-language projects.",
        },
        ModelInfo {
            name: "nomic-embed-text-v1.5",
            aliases: &[],
            dims: 768,
            description: "Nomic AI model. Good quality, 8192 token context.",
        },
        ModelInfo {
            name: "mxbai-embed-large-v1",
            aliases: &[],
            dims: 1024,
            description: "Mixedbread AI. Top-tier local quality.",
        },
        ModelInfo {
            name: "gte-large-en-v1.5",
            aliases: &[],
            dims: 1024,
            description: "Alibaba GTE. Strong English performance.",
        },
    ]
}

pub struct ModelInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub dims: usize,
    pub description: &'static str,
}

// ── FastEmbed Provider ────────────────────────────────────────────────────

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

// ── API Embedding Provider ────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
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
        assert_eq!(dims, 384); // falls back to fast
    }

    #[test]
    fn test_available_models_not_empty() {
        let models = available_models();
        assert!(models.len() >= 4);
        assert_eq!(models[0].name, "fast");
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
