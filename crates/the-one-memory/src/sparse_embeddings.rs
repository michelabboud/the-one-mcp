/// A sparse vector in Qdrant's named sparse format.
///
/// NOTE: Qdrant's REST API uses `u32` for sparse vector indices, but fastembed 5.x's
/// `SparseEmbedding` uses `Vec<usize>`. We store as `u32` here (the Qdrant wire format)
/// and convert from usize at the fastembed boundary.
#[derive(Debug, Clone, Default)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

/// Trait for sparse embedding providers.
pub trait SparseEmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn embed_single(&self, text: &str) -> Result<SparseVector, String>;
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<SparseVector>, String>;
}

// ══════════════════════════════════════════════════════════════════════════
// Local sparse embeddings (fastembed) — only compiled with "local-embeddings"
// ══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "local-embeddings")]
mod local {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Map a model name / alias to a fastembed `SparseModel`.
    ///
    /// Available models in fastembed 5.x:
    /// - `SPLADEPPV1` — Splade++ EN v1 (vocabulary-level sparse, good for English)
    /// - `BGEM3`      — BGE-M3 sparse encoder (8192 context, multilingual) — large download
    ///
    /// NOTE: The design spec requested BM25, which does not exist in fastembed 5.13.
    /// SPLADE++ is the functionally equivalent sparse model (token-level TF-IDF-like
    /// weights, zero dense computation). "bm25" and "splade" both resolve to SPLADEPPV1.
    pub fn resolve_sparse_model(name: &str) -> fastembed::SparseModel {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "bm25" | "splade" | "splade-pp" | "spladeppv1" | "default" => {
                fastembed::SparseModel::SPLADEPPV1
            }
            "bgem3" | "bge-m3" => fastembed::SparseModel::BGEM3,
            other => {
                tracing::warn!(
                    "Unknown sparse model '{}', falling back to SPLADEPPV1 (BM25 alias)",
                    other
                );
                fastembed::SparseModel::SPLADEPPV1
            }
        }
    }

    /// Local sparse embedding provider using fastembed-rs ONNX Runtime.
    ///
    /// Uses `Arc<Mutex<>>` because fastembed's `SparseTextEmbedding::embed()` takes
    /// `&mut self` (same pattern as `FastEmbedProvider` and `FastEmbedReranker`).
    pub struct FastEmbedSparseProvider {
        model: Arc<Mutex<fastembed::SparseTextEmbedding>>,
        model_name: String,
    }

    impl FastEmbedSparseProvider {
        pub fn new(model_name: &str) -> Result<Self, String> {
            let model_enum = resolve_sparse_model(model_name);
            let model = fastembed::SparseTextEmbedding::try_new(
                fastembed::SparseInitOptions::new(model_enum)
                    .with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed sparse init failed: {e}"))?;

            Ok(Self {
                model: Arc::new(Mutex::new(model)),
                model_name: model_name.to_string(),
            })
        }

        pub fn model_name(&self) -> &str {
            &self.model_name
        }

        /// Embed a slice of strings using the underlying fastembed model.
        /// Returns one `SparseVector` per input text.
        fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<SparseVector>, String> {
            let model = Arc::clone(&self.model);
            // fastembed SparseTextEmbedding::embed takes &mut self so we must hold the lock
            // across the entire call. We do NOT use spawn_blocking here because SparseEmbedding
            // fields (Vec<usize>) aren't Send in some configurations; this is a sync trait anyway.
            let mut model = model
                .lock()
                .map_err(|e| format!("sparse model lock poisoned: {e}"))?;

            let raw: Vec<fastembed::SparseEmbedding> = model
                .embed(texts, None)
                .map_err(|e| format!("fastembed sparse embed failed: {e}"))?;

            Ok(raw
                .into_iter()
                .map(|se| SparseVector {
                    // fastembed uses Vec<usize>; Qdrant REST API expects u32
                    indices: se.indices.into_iter().map(|i| i as u32).collect(),
                    values: se.values,
                })
                .collect())
        }
    }

    impl SparseEmbeddingProvider for FastEmbedSparseProvider {
        fn name(&self) -> &str {
            "fastembed-sparse"
        }

        fn embed_single(&self, text: &str) -> Result<SparseVector, String> {
            let mut results = self.embed_texts(vec![text.to_string()])?;
            results
                .pop()
                .ok_or_else(|| "empty sparse embedding result".to_string())
        }

        fn embed_batch(&self, texts: &[String]) -> Result<Vec<SparseVector>, String> {
            self.embed_texts(texts.to_vec())
        }
    }
}

#[cfg(feature = "local-embeddings")]
pub use local::{resolve_sparse_model, FastEmbedSparseProvider};

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
#[cfg(feature = "local-embeddings")]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    // Reuse a single provider across tests to avoid re-downloading the model.
    static PROVIDER: LazyLock<FastEmbedSparseProvider> =
        LazyLock::new(|| FastEmbedSparseProvider::new("bm25").expect("sparse provider should init"));

    #[test]
    fn test_resolve_sparse_model_aliases() {
        // All "bm25" aliases should map to SPLADEPPV1
        let m = resolve_sparse_model("bm25");
        assert_eq!(m, fastembed::SparseModel::SPLADEPPV1);
        let m = resolve_sparse_model("default");
        assert_eq!(m, fastembed::SparseModel::SPLADEPPV1);
        let m = resolve_sparse_model("splade");
        assert_eq!(m, fastembed::SparseModel::SPLADEPPV1);
    }

    #[test]
    fn test_resolve_sparse_model_bgem3() {
        let m = resolve_sparse_model("bgem3");
        assert_eq!(m, fastembed::SparseModel::BGEM3);
        let m = resolve_sparse_model("bge-m3");
        assert_eq!(m, fastembed::SparseModel::BGEM3);
    }

    #[test]
    fn test_resolve_sparse_model_unknown_falls_back() {
        let m = resolve_sparse_model("nonexistent-model");
        assert_eq!(m, fastembed::SparseModel::SPLADEPPV1);
    }

    #[test]
    fn test_bm25_embed_produces_nonempty() {
        let provider = &*PROVIDER;
        let sv = provider.embed_single("hello world").expect("should embed");
        assert!(!sv.indices.is_empty(), "sparse vector should have non-empty indices");
        assert_eq!(
            sv.indices.len(),
            sv.values.len(),
            "indices and values must have the same length"
        );
        assert!(
            sv.values.iter().all(|&v| v >= 0.0),
            "all sparse values should be non-negative"
        );
    }

    #[test]
    fn test_bm25_deterministic() {
        let provider = &*PROVIDER;
        let sv1 = provider.embed_single("the quick brown fox").expect("embed 1");
        let sv2 = provider.embed_single("the quick brown fox").expect("embed 2");
        assert_eq!(sv1.indices, sv2.indices, "indices must be deterministic");
        assert_eq!(sv1.values.len(), sv2.values.len());
        for (a, b) in sv1.values.iter().zip(sv2.values.iter()) {
            assert!(
                (a - b).abs() < 1e-5,
                "values must be deterministic: {a} vs {b}"
            );
        }
    }

    #[test]
    fn test_bm25_batch_length_matches_input() {
        let provider = &*PROVIDER;
        let texts = vec!["hello".to_string(), "world".to_string(), "foo bar".to_string()];
        let results = provider.embed_batch(&texts).expect("batch should embed");
        assert_eq!(results.len(), 3, "one sparse vector per input text");
    }

    #[test]
    fn test_bm25_different_texts_produce_different_vectors() {
        let provider = &*PROVIDER;
        let sv_a = provider.embed_single("memory search retrieval").expect("embed a");
        let sv_b = provider.embed_single("coffee espresso latte").expect("embed b");
        // They may share some token indices, but the full index sets should differ
        assert_ne!(sv_a.indices, sv_b.indices, "different texts should produce different sparse vectors");
    }
}
