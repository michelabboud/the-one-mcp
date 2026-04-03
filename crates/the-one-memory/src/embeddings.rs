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

/// Local embedding provider using fastembed-rs (ONNX Runtime).
/// Runs offline, no API calls, no cost. 384 dimensions by default.
pub struct FastEmbedProvider {
    model: Arc<fastembed::TextEmbedding>,
    dims: usize,
}

impl FastEmbedProvider {
    pub fn new(model_name: &str) -> Result<Self, String> {
        let model_enum = match model_name {
            "BGE-small-en-v1.5" => fastembed::EmbeddingModel::BGESmallENV15,
            _ => fastembed::EmbeddingModel::AllMiniLML6V2,
        };

        let model = fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(model_enum).with_show_download_progress(false),
        )
        .map_err(|e| format!("fastembed init failed: {e}"))?;

        let dims = 384; // Both AllMiniLML6V2 and BGESmallENV15 are 384-dim

        Ok(Self {
            model: Arc::new(model),
            dims,
        })
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

    /// Shared provider instance to avoid concurrent model downloads
    /// which can cause file-level race conditions in fastembed.
    static PROVIDER: LazyLock<FastEmbedProvider> =
        LazyLock::new(|| FastEmbedProvider::new("all-MiniLM-L6-v2").expect("should init"));

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
