pub mod chunker;
pub mod embeddings;
pub mod graph;
pub mod qdrant;
pub mod reranker;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chunker::{chunk_markdown, ChunkMeta};
use crate::embeddings::EmbeddingProvider;
use crate::graph::KnowledgeGraph;
use crate::qdrant::{AsyncQdrantBackend, QdrantOptions, QdrantPayload, QdrantPoint};
use crate::reranker::Reranker;

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
        })
    }

    /// Create with API embeddings + Qdrant HTTP backend.
    /// Always available — does not require local ONNX runtime.
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
        })
    }

    /// Attach a reranker to this engine. When set, search results from Qdrant
    /// are re-scored using the cross-encoder before being returned.
    pub fn set_reranker(&mut self, reranker: Box<dyn Reranker>) {
        self.reranker = Some(reranker);
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
            qdrant
                .ensure_collection(dims)
                .await
                .map_err(std::io::Error::other)?;

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
    /// - **Local**: Entity-focused graph search → fetch associated chunks.
    /// - **Global**: Relationship-focused graph traversal.
    /// - **Hybrid**: Combines vector search + graph search, deduplicates,
    ///   and optionally reranks using a cross-encoder.
    pub async fn search(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        match request.mode {
            RetrievalMode::Naive => self.search_vector(request).await,
            RetrievalMode::Local | RetrievalMode::Global => self.search_graph(request),
            RetrievalMode::Hybrid => self.search_hybrid(request).await,
        }
    }

    /// Pure vector similarity search via Qdrant (or keyword fallback).
    async fn search_vector(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        // Over-fetch when reranker is available so reranking has more candidates
        let fetch_k = if self.reranker.is_some() {
            (request.top_k * 3).max(10)
        } else {
            request.top_k
        };

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

    /// Hybrid search: combine vector search + graph search, deduplicate,
    /// and optionally rerank.
    async fn search_hybrid(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        // Get vector search results (without reranking — we'll rerank the merged set)
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
}
