use async_trait::async_trait;

/// Result of reranking a single document.
#[derive(Debug, Clone)]
pub struct RerankedResult {
    /// Original index in the input list.
    pub original_index: usize,
    /// Cross-encoder relevance score (higher = more relevant).
    pub score: f32,
}

/// Trait for reranking search results using a cross-encoder model.
#[async_trait]
pub trait Reranker: Send + Sync {
    fn name(&self) -> &str;

    /// Rerank `documents` against `query`.  Returns results sorted by relevance
    /// (highest score first).
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<RerankedResult>, String>;
}

// ══════════════════════════════════════════════════════════════════════════
// Local reranker (fastembed) — only compiled with "local-embeddings" feature
// ══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "local-embeddings")]
mod local {
    use super::*;
    use std::sync::Arc;

    /// Resolve a reranker model name to a fastembed enum variant.
    pub fn resolve_reranker_model(name: &str) -> Result<fastembed::RerankerModel, String> {
        match name.to_ascii_lowercase().trim() {
            "bge-reranker-base" | "default" => Ok(fastembed::RerankerModel::BGERerankerBase),
            "bge-reranker-v2-m3" | "multilingual" => Ok(fastembed::RerankerModel::BGERerankerV2M3),
            "jina-reranker-v1-turbo-en" | "jina-turbo" => {
                Ok(fastembed::RerankerModel::JINARerankerV1TurboEn)
            }
            "jina-reranker-v2-base-multilingual" | "jina-multilingual" => {
                Ok(fastembed::RerankerModel::JINARerankerV2BaseMultiligual)
            }
            other => Err(format!("unknown reranker model: {other}")),
        }
    }

    /// Local cross-encoder reranker using fastembed-rs (ONNX Runtime).
    pub struct FastEmbedReranker {
        model: Arc<fastembed::TextRerank>,
        model_name: String,
    }

    impl FastEmbedReranker {
        pub fn new(model_name: &str) -> Result<Self, String> {
            let model_enum = resolve_reranker_model(model_name)?;
            let model = fastembed::TextRerank::try_new(
                fastembed::RerankInitOptions::new(model_enum).with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed reranker init failed: {e}"))?;
            Ok(Self {
                model: Arc::new(model),
                model_name: model_name.to_string(),
            })
        }

        pub fn model_name(&self) -> &str {
            &self.model_name
        }
    }

    #[async_trait]
    impl Reranker for FastEmbedReranker {
        fn name(&self) -> &str {
            "fastembed-reranker"
        }

        async fn rerank(
            &self,
            query: &str,
            documents: &[String],
        ) -> Result<Vec<RerankedResult>, String> {
            if documents.is_empty() {
                return Ok(Vec::new());
            }
            let query = query.to_string();
            let docs: Vec<String> = documents.to_vec();
            let model = Arc::clone(&self.model);

            tokio::task::spawn_blocking(move || {
                let doc_refs: Vec<&String> = docs.iter().collect();
                let results = model
                    .rerank(&query, doc_refs, false, None)
                    .map_err(|e| format!("fastembed rerank failed: {e}"))?;
                Ok(results
                    .into_iter()
                    .map(|r| RerankedResult {
                        original_index: r.index,
                        score: r.score,
                    })
                    .collect())
            })
            .await
            .map_err(|e| format!("tokio join error: {e}"))?
        }
    }
}

#[cfg(feature = "local-embeddings")]
pub use local::{resolve_reranker_model, FastEmbedReranker};

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
#[cfg(feature = "local-embeddings")]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_reranker_model_aliases() {
        assert!(resolve_reranker_model("default").is_ok());
        assert!(resolve_reranker_model("bge-reranker-base").is_ok());
        assert!(resolve_reranker_model("multilingual").is_ok());
        assert!(resolve_reranker_model("jina-turbo").is_ok());
        assert!(resolve_reranker_model("jina-multilingual").is_ok());
    }

    #[test]
    fn test_resolve_reranker_model_unknown() {
        assert!(resolve_reranker_model("nonexistent").is_err());
    }
}
