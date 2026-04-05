pub mod chunker;
pub mod embeddings;
pub mod graph;
pub mod image_embeddings;
pub mod image_ingest;
pub mod models_registry;
pub mod ocr;
pub mod qdrant;
pub mod reranker;
pub mod sparse_embeddings;
pub mod thumbnail;
pub mod watcher;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chunker::{chunk_markdown, ChunkMeta};
use crate::embeddings::EmbeddingProvider;
use crate::graph::KnowledgeGraph;
use crate::qdrant::{AsyncQdrantBackend, QdrantOptions, QdrantPayload, QdrantPoint};
#[cfg(feature = "local-embeddings")]
use crate::qdrant::{HybridPoint, QdrantSparseVector};
use crate::reranker::Reranker;
#[cfg(feature = "local-embeddings")]
use crate::sparse_embeddings::SparseEmbeddingProvider;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for creating a MemoryEngine with API embeddings + Qdrant.
pub struct ApiEngineConfig<'a> {
    pub embedding_base_url: &'a str,
    pub embedding_api_key: Option<&'a str>,
    pub embedding_model: &'a str,
    pub embedding_dims: usize,
    pub qdrant_url: &'a str,
    pub project_id: &'a str,
    pub qdrant_options: QdrantOptions,
    pub max_chunk_tokens: usize,
}

/// Retrieval mode inspired by LightRAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetrievalMode {
    /// Pure vector similarity search (original behavior).
    Naive,
    /// Entity-focused: search the knowledge graph for entities, then fetch
    /// associated chunks. Best for "what is X?" queries.
    Local,
    /// Relationship-focused: traverse graph relations to find connected chunks.
    /// Best for "how does X relate to Y?" queries.
    Global,
    /// Combines vector search + graph search for comprehensive results.
    #[default]
    Hybrid,
}

#[derive(Debug, Clone)]
pub struct MemorySearchRequest {
    pub query: String,
    pub top_k: usize,
    /// Minimum similarity score for results (0.0 = no filtering).
    pub score_threshold: f32,
    /// Which retrieval strategy to use.
    pub mode: RetrievalMode,
}

impl Default for MemorySearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            top_k: 5,
            score_threshold: 0.0,
            mode: RetrievalMode::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub chunk: ChunkMeta,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// MemoryEngine
// ---------------------------------------------------------------------------

pub struct MemoryEngine {
    chunks: Vec<ChunkMeta>,
    by_id: HashMap<String, usize>,
    embedding_provider: Box<dyn EmbeddingProvider>,
    qdrant: Option<AsyncQdrantBackend>,
    max_chunk_tokens: usize,
    /// Optional cross-encoder reranker for improved result quality.
    reranker: Option<Box<dyn Reranker>>,
    /// Knowledge graph for entity-relation enhanced retrieval.
    graph: KnowledgeGraph,
    /// Optional sparse embedding provider for BM25-style hybrid search.
    #[cfg(feature = "local-embeddings")]
    sparse_provider: Option<Box<dyn SparseEmbeddingProvider>>,
    /// Whether to use hybrid (dense + sparse) vector search.
    #[cfg_attr(not(feature = "local-embeddings"), allow(dead_code))]
    hybrid_search_enabled: bool,
    /// Weight for the dense cosine score in hybrid fusion (0.0-1.0).
    #[cfg_attr(not(feature = "local-embeddings"), allow(dead_code))]
    hybrid_dense_weight: f32,
    /// Weight for the normalized sparse score in hybrid fusion (0.0-1.0).
    #[cfg_attr(not(feature = "local-embeddings"), allow(dead_code))]
    hybrid_sparse_weight: f32,
}

impl MemoryEngine {
    /// Create with fastembed local embeddings, no Qdrant (in-memory keyword fallback).
    /// Only available with the `local-embeddings` feature (default).
    #[cfg(feature = "local-embeddings")]
    pub fn new_local(model_name: &str, max_chunk_tokens: usize) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: None,
            max_chunk_tokens,
            reranker: None,
            graph: KnowledgeGraph::new(),
            #[cfg(feature = "local-embeddings")]
            sparse_provider: None,
            hybrid_search_enabled: false,
            hybrid_dense_weight: 0.7,
            hybrid_sparse_weight: 0.3,
        })
    }

    /// Create with fastembed local embeddings + Qdrant HTTP backend.
    /// Only available with the `local-embeddings` feature (default).
    #[cfg(feature = "local-embeddings")]
    pub fn new_with_qdrant(
        model_name: &str,
        qdrant_url: &str,
        project_id: &str,
        qdrant_options: QdrantOptions,
        max_chunk_tokens: usize,
    ) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        let qdrant = AsyncQdrantBackend::new(qdrant_url, project_id, qdrant_options)?;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: Some(qdrant),
            max_chunk_tokens,
            reranker: None,
            graph: KnowledgeGraph::new(),
            #[cfg(feature = "local-embeddings")]
            sparse_provider: None,
            hybrid_search_enabled: false,
            hybrid_dense_weight: 0.7,
            hybrid_sparse_weight: 0.3,
        })
    }

    /// Create with API embeddings + Qdrant HTTP backend.
    /// Always available -- does not require local ONNX runtime.
    pub fn new_api(config: ApiEngineConfig<'_>) -> Result<Self, String> {
        let provider = crate::embeddings::ApiEmbeddingProvider::new(
            config.embedding_base_url,
            config.embedding_api_key,
            config.embedding_model,
            config.embedding_dims,
        );
        let qdrant =
            AsyncQdrantBackend::new(config.qdrant_url, config.project_id, config.qdrant_options)?;
        let max_chunk_tokens = config.max_chunk_tokens;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: Some(qdrant),
            max_chunk_tokens,
            reranker: None,
            graph: KnowledgeGraph::new(),
            #[cfg(feature = "local-embeddings")]
            sparse_provider: None,
            hybrid_search_enabled: false,
            hybrid_dense_weight: 0.7,
            hybrid_sparse_weight: 0.3,
        })
    }

    /// Attach a reranker to this engine. When set, search results from Qdrant
    /// are re-scored using the cross-encoder before being returned.
    pub fn set_reranker(&mut self, reranker: Box<dyn Reranker>) {
        self.reranker = Some(reranker);
    }

    /// Attach a sparse embedding provider and enable hybrid (dense + sparse) search.
    ///
    /// When set, `search_vector` will issue parallel dense and sparse Qdrant queries
    /// and fuse results using linear combination:
    /// `final = dense_weight * dense_score + sparse_weight * bm25_normalize(sparse_score)`
    ///
    /// Ingest also switches to `upsert_hybrid_points` so both vector types are stored.
    #[cfg(feature = "local-embeddings")]
    pub fn set_sparse_provider(
        &mut self,
        provider: Box<dyn SparseEmbeddingProvider>,
        dense_weight: f32,
        sparse_weight: f32,
    ) {
        self.sparse_provider = Some(provider);
        self.hybrid_search_enabled = true;
        self.hybrid_dense_weight = dense_weight;
        self.hybrid_sparse_weight = sparse_weight;
    }

    /// Get a mutable reference to the knowledge graph.
    pub fn graph_mut(&mut self) -> &mut KnowledgeGraph {
        &mut self.graph
    }

    /// Get a reference to the knowledge graph.
    pub fn graph(&self) -> &KnowledgeGraph {
        &self.graph
    }

    /// Ingest all `.md` files from a directory tree.
    /// Returns the number of chunks indexed.
    ///
    /// When a sparse provider is set, switches to hybrid collection setup and
    /// upserts both dense and sparse vectors for each chunk.
    pub async fn ingest_markdown_tree(
        &mut self,
        docs_root: &Path,
    ) -> Result<usize, std::io::Error> {
        if !docs_root.exists() {
            return Ok(0);
        }

        let docs_root_prefix = docs_root.display().to_string();

        // Walk directory tree for .md files
        let mut new_chunks: Vec<ChunkMeta> = Vec::new();
        let mut stack = vec![docs_root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }

                let content = fs::read_to_string(&path)?;
                let relative_path = path.display().to_string();
                let chunks = chunk_markdown(&relative_path, &content, self.max_chunk_tokens);
                new_chunks.extend(chunks);
            }
        }

        // Remove old chunks from this docs_root, then add new ones
        self.chunks
            .retain(|c| !c.source_path.starts_with(&docs_root_prefix));
        let count = new_chunks.len();
        self.chunks.extend(new_chunks);

        // Rebuild the by_id index
        self.by_id.clear();
        for (idx, chunk) in self.chunks.iter().enumerate() {
            self.by_id.insert(chunk.id.clone(), idx);
        }

        // If qdrant is available, ensure collection + embed + upsert
        if let Some(qdrant) = &self.qdrant {
            let dims = self.embedding_provider.dimensions();

            // Choose dense-only or hybrid collection setup
            #[cfg(feature = "local-embeddings")]
            let use_hybrid = self.hybrid_search_enabled && self.sparse_provider.is_some();
            #[cfg(not(feature = "local-embeddings"))]
            let use_hybrid = false;

            if use_hybrid {
                qdrant
                    .ensure_hybrid_collection(dims)
                    .await
                    .map_err(std::io::Error::other)?;
            } else {
                qdrant
                    .ensure_collection(dims)
                    .await
                    .map_err(std::io::Error::other)?;
            }

            // Embed in batches of 64
            let batch_size = 64;
            for batch_start in (0..self.chunks.len()).step_by(batch_size) {
                let batch_end = (batch_start + batch_size).min(self.chunks.len());
                let texts: Vec<String> = self.chunks[batch_start..batch_end]
                    .iter()
                    .map(|c| c.content.clone())
                    .collect();

                let vectors = self
                    .embedding_provider
                    .embed_batch(&texts)
                    .await
                    .map_err(std::io::Error::other)?;

                // Hybrid upsert when sparse provider is active
                #[cfg(feature = "local-embeddings")]
                if use_hybrid {
                    if let Some(sparse_prov) = &self.sparse_provider {
                        let sparse_vecs = sparse_prov
                            .embed_batch(&texts)
                            .map_err(std::io::Error::other)?;

                        let hybrid_points: Vec<HybridPoint> = self.chunks[batch_start..batch_end]
                            .iter()
                            .zip(vectors.into_iter())
                            .zip(sparse_vecs.into_iter())
                            .map(|((chunk, dense_vec), sv)| HybridPoint {
                                id: chunk.id.clone(),
                                dense: dense_vec,
                                sparse: QdrantSparseVector {
                                    indices: sv.indices,
                                    values: sv.values,
                                },
                                payload: QdrantPayload {
                                    chunk_id: chunk.id.clone(),
                                    source_path: chunk.source_path.clone(),
                                    heading: chunk
                                        .heading_hierarchy
                                        .last()
                                        .cloned()
                                        .unwrap_or_default(),
                                    chunk_index: chunk.chunk_index,
                                },
                            })
                            .collect();

                        qdrant
                            .upsert_hybrid_points(hybrid_points)
                            .await
                            .map_err(std::io::Error::other)?;
                        continue;
                    }
                }

                // Dense-only upsert (default path)
                let points: Vec<QdrantPoint> = self.chunks[batch_start..batch_end]
                    .iter()
                    .zip(vectors.into_iter())
                    .map(|(chunk, vector)| QdrantPoint {
                        id: chunk.id.clone(),
                        vector,
                        payload: QdrantPayload {
                            chunk_id: chunk.id.clone(),
                            source_path: chunk.source_path.clone(),
                            heading: chunk.heading_hierarchy.last().cloned().unwrap_or_default(),
                            chunk_index: chunk.chunk_index,
                        },
                    })
                    .collect();

                qdrant
                    .upsert_points(points)
                    .await
                    .map_err(std::io::Error::other)?;
            }
        }

        Ok(count)
    }

    /// Search for relevant chunks using the configured retrieval mode.
    ///
    /// Retrieval modes (inspired by LightRAG):
    /// - **Naive**: Pure vector similarity search (original behavior).
    /// - **Local**: Entity-focused graph search -> fetch associated chunks.
    /// - **Global**: Relationship-focused graph traversal.
    /// - **Hybrid**: Combines vector search + graph search, deduplicates,
    ///   and optionally reranks using a cross-encoder.
    pub async fn search(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        match request.mode {
            RetrievalMode::Naive => self.search_vector(request).await,
            RetrievalMode::Local | RetrievalMode::Global => self.search_graph(request),
            RetrievalMode::Hybrid => self.search_hybrid_mode(request).await,
        }
    }

    /// Pure vector similarity search via Qdrant (or keyword fallback).
    ///
    /// When `hybrid_search_enabled` is true and a sparse provider is set, this method
    /// issues two parallel Qdrant queries (dense + sparse) and fuses results:
    /// `final = dense_weight * dense_score + sparse_weight * bm25_normalize(sparse_score)`
    async fn search_vector(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        // Over-fetch when reranker is available so reranking has more candidates
        let fetch_k = if self.reranker.is_some() {
            (request.top_k * 3).max(10)
        } else {
            request.top_k
        };

        // Hybrid path (dense + sparse, linear fusion)
        #[cfg(feature = "local-embeddings")]
        if self.hybrid_search_enabled {
            if let (Some(qdrant), Some(sparse_prov)) = (&self.qdrant, &self.sparse_provider) {
                let dense_vec = match self.embedding_provider.embed_single(&request.query).await {
                    Ok(v) => v,
                    Err(_) => return Vec::new(),
                };

                let sparse_vec = match sparse_prov.embed_single(&request.query) {
                    Ok(sv) => sv,
                    Err(e) => {
                        tracing::warn!("sparse embed failed, falling back to dense-only: {e}");
                        return self.search_vector_dense_only(request, fetch_k).await;
                    }
                };

                let qdrant_sparse = QdrantSparseVector {
                    indices: sparse_vec.indices,
                    values: sparse_vec.values,
                };

                let (dense_results, sparse_results) = match qdrant
                    .search_hybrid(dense_vec, qdrant_sparse, fetch_k, request.score_threshold)
                    .await
                {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("hybrid search failed, falling back to dense-only: {e}");
                        return self.search_vector_dense_only(request, fetch_k).await;
                    }
                };

                // Fuse: linear combination with BM25 normalization on sparse score
                let mut fused: HashMap<String, f32> = HashMap::new();
                for r in &dense_results {
                    let entry = fused.entry(r.chunk_id.clone()).or_insert(0.0);
                    *entry += self.hybrid_dense_weight * r.score;
                }
                for r in &sparse_results {
                    let entry = fused.entry(r.chunk_id.clone()).or_insert(0.0);
                    *entry += self.hybrid_sparse_weight * bm25_normalize(r.score);
                }

                let mut results: Vec<MemorySearchResult> = fused
                    .into_iter()
                    .filter_map(|(chunk_id, score)| {
                        self.by_id
                            .get(&chunk_id)
                            .and_then(|&idx| self.chunks.get(idx))
                            .map(|chunk| MemorySearchResult {
                                chunk: chunk.clone(),
                                score,
                            })
                    })
                    .collect();

                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                if let Some(reranker) = &self.reranker {
                    results = self
                        .apply_reranking(reranker.as_ref(), &request.query, results)
                        .await;
                }

                results.truncate(request.top_k);
                return results;
            }
        }

        // Dense-only path
        self.search_vector_dense_only(request, fetch_k).await
    }

    /// Dense-only vector search (original behavior, used as fallback from hybrid path).
    async fn search_vector_dense_only(
        &self,
        request: &MemorySearchRequest,
        fetch_k: usize,
    ) -> Vec<MemorySearchResult> {
        let mut results = if let Some(qdrant) = &self.qdrant {
            let query_vector = match self.embedding_provider.embed_single(&request.query).await {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            };

            let qdrant_results = match qdrant
                .search(query_vector, fetch_k, request.score_threshold)
                .await
            {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };

            qdrant_results
                .into_iter()
                .filter_map(|r| {
                    self.by_id
                        .get(&r.chunk_id)
                        .and_then(|&idx| self.chunks.get(idx))
                        .map(|chunk| MemorySearchResult {
                            chunk: chunk.clone(),
                            score: r.score,
                        })
                })
                .collect()
        } else {
            self.search_keyword(request)
        };

        // Apply cross-encoder reranking if available
        if let Some(reranker) = &self.reranker {
            results = self
                .apply_reranking(reranker.as_ref(), &request.query, results)
                .await;
        }

        results.truncate(request.top_k);
        results
    }

    /// Keyword-based fallback search (no Qdrant).
    fn search_keyword(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        let query_words: Vec<String> = request
            .query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        if query_words.is_empty() {
            return Vec::new();
        }

        let total_words = query_words.len() as f32;

        let mut scored: Vec<MemorySearchResult> = self
            .chunks
            .iter()
            .filter_map(|chunk| {
                let content_lower = chunk.content.to_lowercase();
                let matches = query_words
                    .iter()
                    .filter(|w| content_lower.contains(w.as_str()))
                    .count() as f32;
                let score = matches / total_words;
                if score > request.score_threshold {
                    Some(MemorySearchResult {
                        chunk: chunk.clone(),
                        score,
                    })
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(request.top_k);
        scored
    }

    /// Graph-based search: find entities matching query terms, then return
    /// chunks associated with those entities and their neighbors.
    fn search_graph(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        let terms: Vec<String> = request
            .query
            .split_whitespace()
            .map(|w| w.to_string())
            .collect();

        let graph_results = self.graph.search(&terms, request.top_k);
        if graph_results.is_empty() {
            return Vec::new();
        }

        // Collect unique chunk IDs from graph results, weighted by graph relevance
        let mut chunk_scores: HashMap<String, f32> = HashMap::new();
        for (rank, result) in graph_results.iter().enumerate() {
            let base_score = 1.0 - (rank as f32 * 0.1).min(0.9);
            // Direct entity chunks get full score
            for cid in &result.entity.source_chunks {
                let entry = chunk_scores.entry(cid.clone()).or_insert(0.0);
                *entry = entry.max(base_score);
            }
            // Neighbor chunks get slightly lower score
            for (_, neighbor) in &result.neighbors {
                for cid in &neighbor.source_chunks {
                    let entry = chunk_scores.entry(cid.clone()).or_insert(0.0);
                    *entry = entry.max(base_score * 0.8);
                }
            }
        }

        let mut results: Vec<MemorySearchResult> = chunk_scores
            .into_iter()
            .filter_map(|(cid, score)| {
                self.by_id
                    .get(&cid)
                    .and_then(|&idx| self.chunks.get(idx))
                    .map(|chunk| MemorySearchResult {
                        chunk: chunk.clone(),
                        score,
                    })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(request.top_k);
        results
    }

    /// Hybrid mode search: combine vector search + graph search, deduplicate,
    /// and optionally rerank.
    ///
    /// Note: this is the LightRAG-style hybrid (vector + graph). The
    /// BM25-sparse hybrid (dense cosine + sparse SPLADE) runs inside
    /// `search_vector` when `hybrid_search_enabled` is true.
    async fn search_hybrid_mode(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        // Get vector search results (without reranking -- we'll rerank the merged set)
        let vector_results = {
            let fetch_k = (request.top_k * 2).max(10);
            if let Some(qdrant) = &self.qdrant {
                let query_vector = match self.embedding_provider.embed_single(&request.query).await
                {
                    Ok(v) => v,
                    Err(_) => return self.search_graph(request),
                };

                match qdrant
                    .search(query_vector, fetch_k, request.score_threshold)
                    .await
                {
                    Ok(r) => r
                        .into_iter()
                        .filter_map(|r| {
                            self.by_id
                                .get(&r.chunk_id)
                                .and_then(|&idx| self.chunks.get(idx))
                                .map(|chunk| MemorySearchResult {
                                    chunk: chunk.clone(),
                                    score: r.score,
                                })
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                }
            } else {
                self.search_keyword(request)
            }
        };

        // Get graph search results
        let graph_results = self.search_graph(request);

        // Merge and deduplicate: graph results boost vector results
        let mut merged: HashMap<String, MemorySearchResult> = HashMap::new();
        for result in vector_results {
            merged
                .entry(result.chunk.id.clone())
                .and_modify(|existing| {
                    existing.score = existing.score.max(result.score);
                })
                .or_insert(result);
        }
        for result in graph_results {
            merged
                .entry(result.chunk.id.clone())
                .and_modify(|existing| {
                    // Boost: if chunk appears in both vector and graph results,
                    // increase its score (reciprocal rank fusion inspired)
                    existing.score += result.score * 0.3;
                })
                .or_insert(result);
        }

        let mut results: Vec<MemorySearchResult> = merged.into_values().collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply cross-encoder reranking if available
        if let Some(reranker) = &self.reranker {
            results = self
                .apply_reranking(reranker.as_ref(), &request.query, results)
                .await;
        }

        results.truncate(request.top_k);
        results
    }

    /// Apply cross-encoder reranking to a set of results.
    async fn apply_reranking(
        &self,
        reranker: &dyn Reranker,
        query: &str,
        results: Vec<MemorySearchResult>,
    ) -> Vec<MemorySearchResult> {
        if results.is_empty() {
            return results;
        }

        let documents: Vec<String> = results.iter().map(|r| r.chunk.content.clone()).collect();

        match reranker.rerank(query, &documents).await {
            Ok(reranked) => {
                let mut reranked_results: Vec<MemorySearchResult> = reranked
                    .into_iter()
                    .filter_map(|r| {
                        results
                            .get(r.original_index)
                            .map(|orig| MemorySearchResult {
                                chunk: orig.chunk.clone(),
                                score: r.score,
                            })
                    })
                    .collect();
                reranked_results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                reranked_results
            }
            Err(e) => {
                tracing::warn!("reranking failed, using original scores: {e}");
                results
            }
        }
    }

    /// Fetch a specific chunk by ID.
    pub fn fetch_chunk(&self, id: &str) -> Option<ChunkMeta> {
        self.by_id.get(id).map(|&idx| self.chunks[idx].clone())
    }

    /// List all indexed document paths.
    pub fn docs_list(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .chunks
            .iter()
            .map(|c| c.source_path.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        paths.sort();
        paths
    }

    /// Get full content of a document by reconstructing from chunks.
    pub fn docs_get(&self, path: &str) -> Option<String> {
        let mut file_chunks: Vec<&ChunkMeta> = self
            .chunks
            .iter()
            .filter(|c| c.source_path == path)
            .collect();
        if file_chunks.is_empty() {
            return None;
        }
        file_chunks.sort_by_key(|c| c.chunk_index);
        Some(
            file_chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Ingest or re-ingest a single markdown file.
    ///
    /// Removes existing chunks for that path first (by `source_path`), re-chunks,
    /// re-embeds, and upserts. Returns the number of chunks indexed for this file.
    pub async fn ingest_single_markdown(&mut self, path: &Path) -> Result<usize, String> {
        if !path.exists() {
            return Err(format!("file does not exist: {}", path.display()));
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("md") && ext != Some("markdown") {
            return Err(format!("not a markdown file: {}", path.display()));
        }

        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;
        let path_str = path.display().to_string();

        // Remove existing chunks for this path (in-memory index + Qdrant)
        self.remove_by_path(path).await?;

        // Chunk and add
        let new_chunks = crate::chunker::chunk_markdown(&path_str, &content, self.max_chunk_tokens);
        let count = new_chunks.len();

        for chunk in &new_chunks {
            self.by_id.insert(chunk.id.clone(), self.chunks.len());
            self.chunks.push(chunk.clone());
        }

        // Upsert to Qdrant if available
        if let Some(qdrant) = &self.qdrant {
            let dims = self.embedding_provider.dimensions();
            qdrant.ensure_collection(dims).await.map_err(|e| e.to_string())?;

            let texts: Vec<String> = new_chunks.iter().map(|c| c.content.clone()).collect();
            let vectors = self.embedding_provider.embed_batch(&texts).await?;

            let points: Vec<QdrantPoint> = new_chunks
                .iter()
                .zip(vectors.into_iter())
                .map(|(chunk, vector)| QdrantPoint {
                    id: chunk.id.clone(),
                    vector,
                    payload: QdrantPayload {
                        chunk_id: chunk.id.clone(),
                        source_path: chunk.source_path.clone(),
                        heading: chunk.heading_hierarchy.last().cloned().unwrap_or_default(),
                        chunk_index: chunk.chunk_index,
                    },
                })
                .collect();

            qdrant.upsert_points(points).await.map_err(|e| e.to_string())?;
        }

        Ok(count)
    }

    /// Remove all chunks for a given file path from the in-memory index and Qdrant.
    ///
    /// Used by the watcher when a file is deleted, or internally by
    /// [`ingest_single_markdown`] before re-ingesting. Returns the number of
    /// chunks removed.
    pub async fn remove_by_path(&mut self, path: &Path) -> Result<usize, String> {
        let path_str = path.display().to_string();

        let count = self
            .chunks
            .iter()
            .filter(|c| c.source_path == path_str)
            .count();

        if count == 0 {
            return Ok(0);
        }

        // Remove from in-memory vectors
        self.chunks.retain(|c| c.source_path != path_str);
        self.by_id.clear();
        for (idx, chunk) in self.chunks.iter().enumerate() {
            self.by_id.insert(chunk.id.clone(), idx);
        }

        // Remove from Qdrant if available
        if let Some(qdrant) = &self.qdrant {
            qdrant
                .delete_by_source_path(&path_str)
                .await
                .map_err(|e| e.to_string())?;
        }

        Ok(count)
    }

    /// Get a section of a document by heading.
    pub fn docs_get_section(&self, path: &str, heading: &str, max_bytes: usize) -> Option<String> {
        let matching: Vec<&ChunkMeta> = self
            .chunks
            .iter()
            .filter(|c| c.source_path == path && c.heading_hierarchy.iter().any(|h| h == heading))
            .collect();
        if matching.is_empty() {
            return None;
        }
        let content: String = matching
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if content.len() > max_bytes {
            Some(content[..max_bytes].to_string())
        } else {
            Some(content)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Saturating BM25-style normalization: maps any positive score to [0, 1).
///
/// Formula: `score / (score + k)` where k=5.0.
/// - At score=0: returns 0.0
/// - At score=5: returns 0.5
/// - As score goes to infinity: approaches 1.0
#[cfg(feature = "local-embeddings")]
fn bm25_normalize(score: f32) -> f32 {
    const K: f32 = 5.0;
    score / (score + K)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "local-embeddings")]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    /// Shared FastEmbedProvider to avoid concurrent model downloads.
    static PROVIDER: LazyLock<crate::embeddings::FastEmbedProvider> = LazyLock::new(|| {
        crate::embeddings::FastEmbedProvider::new("all-MiniLM-L6-v2").expect("should init")
    });

    /// Helper: build a local-only engine reusing the shared provider is tricky
    /// because MemoryEngine owns the provider. For tests we just create engines
    /// with `new_local` which will init fastembed (the model is already cached).
    fn make_local_engine() -> MemoryEngine {
        MemoryEngine::new_local("all-MiniLM-L6-v2", 500).expect("engine should init")
    }

    #[test]
    fn test_bm25_normalize_properties() {
        // At zero should be zero
        assert!((bm25_normalize(0.0) - 0.0).abs() < 1e-6);
        // At k=5.0 should be exactly 0.5
        assert!((bm25_normalize(5.0) - 0.5).abs() < 1e-6);
        // Should always be in [0, 1)
        for score in [0.1_f32, 1.0, 10.0, 100.0, 1000.0] {
            let n = bm25_normalize(score);
            assert!((0.0..1.0).contains(&n), "normalized score {n} out of [0,1)");
        }
        // Monotonically increasing
        assert!(bm25_normalize(1.0) < bm25_normalize(2.0));
        assert!(bm25_normalize(2.0) < bm25_normalize(10.0));
    }

    #[tokio::test]
    async fn test_ingest_and_search_keyword_fallback() {
        // Force the shared provider to initialize (caches the model)
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(
            docs.join("readme.md"),
            "# Intro\nThis broker manages routing and memory search.\n# Details\nSome other content about configuration.",
        )
        .expect("write");
        fs::write(
            docs.join("guide.md"),
            "# Setup\nHow to set up the system for production use.",
        )
        .expect("write");

        let mut engine = make_local_engine();
        let count = engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");
        assert!(count >= 3, "expected at least 3 chunks, got {count}");

        // Keyword fallback search (no qdrant)
        let results = engine
            .search(&MemorySearchRequest {
                query: "routing memory".to_string(),
                top_k: 5,
                ..Default::default()
            })
            .await;
        assert!(
            !results.is_empty(),
            "should find at least one result for 'routing memory'"
        );
        assert!(results[0].score > 0.0);
    }

    #[tokio::test]
    async fn test_docs_list_and_get() {
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(
            docs.join("readme.md"),
            "# Intro\nHello world\n# Usage\nUse it well",
        )
        .expect("write");

        let mut engine = make_local_engine();
        engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");

        let listed = engine.docs_list();
        assert_eq!(listed.len(), 1);

        let content = engine.docs_get(&listed[0]).expect("should have doc");
        assert!(content.contains("Hello world"));
        assert!(content.contains("Use it well"));
    }

    #[tokio::test]
    async fn test_fetch_chunk_by_id() {
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(docs.join("readme.md"), "# Intro\nSome content here").expect("write");

        let mut engine = make_local_engine();
        engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");

        // The chunk ID format from the chunker is "{source_path}:{chunk_index}"
        let listed = engine.docs_list();
        let chunk_id = format!("{}:0", listed[0]);
        let chunk = engine.fetch_chunk(&chunk_id).expect("chunk should exist");
        assert!(chunk.content.contains("Intro"));
        assert!(chunk.content.contains("Some content"));
    }

    #[tokio::test]
    async fn test_ingest_single_markdown_adds_chunks() {
        let _ = &*PROVIDER;
        let temp = tempfile::tempdir().expect("tempdir");
        let md_path = temp.path().join("doc.md");
        std::fs::write(
            &md_path,
            "# Title\nContent here.\n## Section\nMore content.",
        )
        .expect("write");

        let mut engine = make_local_engine();
        let count = engine
            .ingest_single_markdown(&md_path)
            .await
            .expect("ingest");
        assert!(count >= 1);
        assert!(!engine.docs_list().is_empty());
    }

    #[tokio::test]
    async fn test_ingest_single_markdown_replaces_existing() {
        let _ = &*PROVIDER;
        let temp = tempfile::tempdir().expect("tempdir");
        let md_path = temp.path().join("doc.md");

        std::fs::write(&md_path, "# First version\nOld content").expect("write");
        let mut engine = make_local_engine();
        engine
            .ingest_single_markdown(&md_path)
            .await
            .expect("first ingest");
        let initial_count = engine.chunks.len();

        // Modify the file
        std::fs::write(&md_path, "# Second version\nCompletely new content").expect("write");
        engine
            .ingest_single_markdown(&md_path)
            .await
            .expect("second ingest");

        // Old content should be gone, new content present
        let all_content: String = engine.chunks.iter().map(|c| c.content.as_str()).collect();
        assert!(
            !all_content.contains("Old content"),
            "old content should be replaced"
        );
        assert!(
            all_content.contains("Completely new content")
                || all_content.contains("Second version")
        );
        let _ = initial_count;
    }

    #[tokio::test]
    async fn test_remove_by_path_removes_all_chunks() {
        let _ = &*PROVIDER;
        let temp = tempfile::tempdir().expect("tempdir");
        let md_path = temp.path().join("doc.md");
        std::fs::write(&md_path, "# Title\nSome content").expect("write");

        let mut engine = make_local_engine();
        engine
            .ingest_single_markdown(&md_path)
            .await
            .expect("ingest");
        assert!(!engine.chunks.is_empty());

        let removed = engine.remove_by_path(&md_path).await.expect("remove");
        assert!(removed >= 1);
        let path_str = md_path.display().to_string();
        assert!(
            engine.chunks.iter().all(|c| c.source_path != path_str),
            "all chunks for path should be removed"
        );
    }

    #[tokio::test]
    async fn test_hybrid_search_flag_fallback_to_keyword() {
        // Without Qdrant or sparse provider, hybrid flag should silently fall
        // through to keyword search and still return results.
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(
            docs.join("api.md"),
            "# API Reference\nThe embed function produces sparse vectors for BM25 retrieval.",
        )
        .expect("write");

        let mut engine = make_local_engine();
        // Manually enable hybrid without attaching sparse provider (graceful no-op path)
        engine.hybrid_search_enabled = true;

        engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");

        let results = engine
            .search(&MemorySearchRequest {
                query: "sparse vectors".to_string(),
                top_k: 3,
                mode: RetrievalMode::Naive,
                ..Default::default()
            })
            .await;

        // Keyword fallback should still find results
        assert!(
            !results.is_empty(),
            "should find results even in hybrid mode without qdrant/sparse provider"
        );
    }
}
