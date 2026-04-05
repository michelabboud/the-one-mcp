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
    use std::sync::{Arc, Mutex};

    /// Map a `fastembed_enum` string from the registry to the actual fastembed enum variant.
    fn reranker_enum_from_name(fastembed_enum: &str) -> Option<fastembed::RerankerModel> {
        let model = match fastembed_enum {
            "BGERerankerBase" => fastembed::RerankerModel::BGERerankerBase,
            "BGERerankerV2M3" => fastembed::RerankerModel::BGERerankerV2M3,
            "JINARerankerV1TurboEn" => fastembed::RerankerModel::JINARerankerV1TurboEn,
            // NOTE: fastembed upstream spells this with a typo (missing "n" in "Multilingual")
            "JINARerankerV2BaseMultiligual" => {
                fastembed::RerankerModel::JINARerankerV2BaseMultiligual
            }
            _ => return None,
        };
        Some(model)
    }

    /// Resolve a reranker model name (or alias) to a fastembed enum variant.
    ///
    /// Resolution order:
    /// 1. `"default"` → registry default model.
    /// 2. Convenience aliases (backward-compat shortcuts).
    /// 3. Registry name lookup (case-insensitive).
    /// 4. Falls back to registry default with a warning on unknown names.
    pub fn resolve_reranker_model(name: &str) -> Result<fastembed::RerankerModel, String> {
        let name_lower = name.to_ascii_lowercase();
        let name_trimmed = name_lower.trim();

        // 1. "default" → registry default
        if name_trimmed == "default" {
            let default = crate::models_registry::default_rerank_model();
            return reranker_enum_from_name(&default.fastembed_enum).ok_or_else(|| {
                format!(
                    "default reranker enum '{}' not found in fastembed",
                    default.fastembed_enum
                )
            });
        }

        // 2. Convenience aliases for backward compatibility
        let alias_resolved = match name_trimmed {
            "multilingual" | "jina-multilingual" => Some("jina-reranker-v2-base-multilingual"),
            "jina-turbo" => Some("jina-reranker-v1-turbo-en"),
            _ => None,
        };
        if let Some(canonical) = alias_resolved {
            // Resolve the canonical name through the registry
            let models = crate::models_registry::list_rerank_models();
            if let Some(m) = models.iter().find(|m| m.name == canonical) {
                return reranker_enum_from_name(&m.fastembed_enum).ok_or_else(|| {
                    format!(
                        "reranker enum '{}' not found in fastembed",
                        m.fastembed_enum
                    )
                });
            }
        }

        // 3. Registry name lookup (case-insensitive)
        let models = crate::models_registry::list_rerank_models();
        if let Some(m) = models
            .iter()
            .find(|m| m.name.to_ascii_lowercase() == name_trimmed)
        {
            return reranker_enum_from_name(&m.fastembed_enum).ok_or_else(|| {
                format!(
                    "reranker enum '{}' not found in fastembed",
                    m.fastembed_enum
                )
            });
        }

        // 4. Unknown — fall back to registry default with a warning
        tracing::warn!(
            "Unknown reranker model '{}', falling back to registry default",
            name
        );
        let default = crate::models_registry::default_rerank_model();
        reranker_enum_from_name(&default.fastembed_enum).ok_or_else(|| {
            format!(
                "default reranker enum '{}' not found in fastembed",
                default.fastembed_enum
            )
        })
    }

    /// Local cross-encoder reranker using fastembed-rs (ONNX Runtime).
    pub struct FastEmbedReranker {
        model: Arc<Mutex<fastembed::TextRerank>>,
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
                model: Arc::new(Mutex::new(model)),
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
                let mut model = model
                    .lock()
                    .map_err(|e| format!("model lock poisoned: {e}"))?;
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
    fn test_resolve_reranker_model_unknown_falls_back_to_default() {
        // Unknown names fall back to the registry default rather than erroring
        assert!(resolve_reranker_model("nonexistent").is_ok());
    }
}
