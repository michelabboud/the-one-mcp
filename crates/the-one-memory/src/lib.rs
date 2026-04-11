pub mod chunker;
pub mod chunker_go;
pub mod chunker_python;
pub mod chunker_rust;
pub mod chunker_typescript;
pub mod conversation;
// Tree-sitter chunker infrastructure + language modules (feature-gated).
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_c;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_cpp;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_java;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_kotlin;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_php;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_ruby;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_swift;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_ts_impl;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_zig;
// Tree-sitter replacements for the 5 existing regex chunkers. The
// dispatcher in `chunker::chunk_file` tries these first and falls back to
// the regex implementations on parse failure.
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_go_ts;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_python_ts;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_rust_ts;
#[cfg(feature = "tree-sitter-chunker")]
pub mod chunker_typescript_ts;
pub mod embeddings;
pub mod graph;
pub mod graph_extractor;
pub mod image_embeddings;
pub mod image_ingest;
pub mod models_registry;
pub mod ocr;
pub mod palace;
#[cfg(feature = "pg-vectors")]
pub mod pg_vector;
pub mod qdrant;
#[cfg(feature = "redis-vectors")]
pub mod redis_vectors;
pub mod reranker;
pub mod sparse_embeddings;
pub mod thumbnail;
pub mod vector_backend;
pub mod watcher;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chunker::{chunk_conversation, chunk_markdown, ChunkMeta};
use crate::embeddings::EmbeddingProvider;
use crate::graph::KnowledgeGraph;
use crate::qdrant::{AsyncQdrantBackend, QdrantOptions};
#[cfg(feature = "redis-vectors")]
use crate::redis_vectors::RedisVectorStore;
use crate::reranker::Reranker;
#[cfg(feature = "local-embeddings")]
use crate::sparse_embeddings::SparseEmbeddingProvider;
use crate::vector_backend::{
    ChunkPayload, EntityPoint as VbEntityPoint, HybridVectorPoint,
    RelationPoint as VbRelationPoint, SparseVector as VbSparseVector, VectorBackend, VectorPoint,
};

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

/// Configuration for the Redis-backed vector backend.
#[cfg(feature = "redis-vectors")]
#[derive(Clone, Debug)]
pub struct RedisEngineConfig {
    pub redis_url: String,
    pub index_name: String,
    pub persistence_required: bool,
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
    /// v0.16.0 — single backend slot. Replaces the pre-v0.16 pair of
    /// `qdrant: Option<AsyncQdrantBackend>` + `redis: Option<RedisVectorStore>`
    /// fields. Only one backend is ever active at a time (the broker's
    /// `vector_backend` config selects it). `None` means no external vector
    /// store is configured and the engine falls back to in-memory keyword
    /// search.
    backend: Option<Box<dyn VectorBackend>>,
    /// v0.16.0 — Redis-specific flag, retained here because it's a
    /// constructor-time decision rather than a backend-trait concern. When
    /// true and the active backend is Redis, the engine refuses any write
    /// path if `backend.verify_persistence()` reports AOF disabled.
    /// Non-Redis backends ignore this flag entirely (their default
    /// `verify_persistence` always succeeds).
    #[cfg(feature = "redis-vectors")]
    redis_persistence_required: bool,
    max_chunk_tokens: usize,
    /// Optional cross-encoder reranker for improved result quality.
    reranker: Option<Box<dyn Reranker>>,
    /// Knowledge graph for entity-relation enhanced retrieval.
    graph: KnowledgeGraph,
    /// v0.13.1 — project identifier used to scope Qdrant entity/relation
    /// vector collections. Set via [`MemoryEngine::set_project_id`] after
    /// construction. When `None`, the semantic graph-search fallback uses
    /// the in-memory keyword search only.
    project_id: Option<String>,
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
    /// v0.16.0 — canonical constructor. Takes an already-constructed
    /// embedding provider and optional vector backend. All other
    /// constructors delegate to this. Use this to plug in custom backends
    /// (pgvector, Postgres-combined, etc.) from downstream crates.
    pub fn new_with_backend(
        embedding_provider: Box<dyn EmbeddingProvider>,
        backend: Option<Box<dyn VectorBackend>>,
        max_chunk_tokens: usize,
    ) -> Self {
        Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider,
            backend,
            #[cfg(feature = "redis-vectors")]
            redis_persistence_required: false,
            max_chunk_tokens,
            reranker: None,
            graph: KnowledgeGraph::new(),
            #[cfg(feature = "local-embeddings")]
            sparse_provider: None,
            hybrid_search_enabled: false,
            hybrid_dense_weight: 0.7,
            hybrid_sparse_weight: 0.3,
            project_id: None,
        }
    }

    /// Create with fastembed local embeddings, no external vector backend
    /// (in-memory keyword fallback). Only available with the
    /// `local-embeddings` feature (default).
    #[cfg(feature = "local-embeddings")]
    pub fn new_local(model_name: &str, max_chunk_tokens: usize) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        Ok(Self::new_with_backend(
            Box::new(provider),
            None,
            max_chunk_tokens,
        ))
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
        Ok(Self::new_with_backend(
            Box::new(provider),
            Some(Box::new(qdrant)),
            max_chunk_tokens,
        ))
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
        Ok(Self::new_with_backend(
            Box::new(provider),
            Some(Box::new(qdrant)),
            config.max_chunk_tokens,
        ))
    }

    /// v0.16.0 Phase 2 — fastembed local embeddings + pgvector backend.
    ///
    /// Gated on both `local-embeddings` and `pg-vectors`. The pgvector
    /// constructor takes a `&dyn EmbeddingProvider` to verify the
    /// live dim matches the migration-baked dim (1024 per Decision C).
    /// We construct the provider first, let the backend borrow it for
    /// the preflight check, then transfer ownership into the engine
    /// via `new_with_backend`.
    #[cfg(all(feature = "local-embeddings", feature = "pg-vectors"))]
    pub async fn new_with_pgvector(
        model_name: &str,
        max_chunk_tokens: usize,
        config: &crate::pg_vector::PgVectorConfig,
        url: &str,
        project_id: &str,
    ) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        let backend =
            crate::pg_vector::PgVectorBackend::new(config, url, project_id, &provider).await?;
        Ok(Self::new_with_backend(
            Box::new(provider),
            Some(Box::new(backend)),
            max_chunk_tokens,
        ))
    }

    /// v0.16.0 Phase 2 — API embeddings + pgvector backend.
    ///
    /// Operators using an OpenAI-compatible embedding API get the same
    /// pgvector path — no local-embeddings dependency required.
    #[cfg(feature = "pg-vectors")]
    pub async fn new_api_with_pgvector(
        api_config: ApiEngineConfig<'_>,
        pg_config: &crate::pg_vector::PgVectorConfig,
        pg_url: &str,
    ) -> Result<Self, String> {
        let provider = crate::embeddings::ApiEmbeddingProvider::new(
            api_config.embedding_base_url,
            api_config.embedding_api_key,
            api_config.embedding_model,
            api_config.embedding_dims,
        );
        let backend = crate::pg_vector::PgVectorBackend::new(
            pg_config,
            pg_url,
            api_config.project_id,
            &provider,
        )
        .await?;
        Ok(Self::new_with_backend(
            Box::new(provider),
            Some(Box::new(backend)),
            api_config.max_chunk_tokens,
        ))
    }

    /// Create with fastembed local embeddings + Redis vector backend.
    #[cfg(all(feature = "local-embeddings", feature = "redis-vectors"))]
    pub async fn new_with_redis(
        model_name: &str,
        max_chunk_tokens: usize,
        redis: RedisEngineConfig,
    ) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        let persistence_required = redis.persistence_required;
        let redis_store =
            RedisVectorStore::from_url(&redis.redis_url, redis.index_name, provider.dimensions())?;

        let mut engine = Self::new_with_backend(
            Box::new(provider),
            Some(Box::new(redis_store)),
            max_chunk_tokens,
        );
        engine.redis_persistence_required = persistence_required;
        Ok(engine)
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

    /// v0.13.1 — set the project identifier used for Qdrant entity/relation
    /// vector collections. Called by the broker right after construction.
    pub fn set_project_id(&mut self, id: String) {
        self.project_id = Some(id);
    }

    /// v0.13.1 — read the configured project id, if any.
    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    /// Return the active vector backend name. Derived from the backend's
    /// own capability report — backends self-identify.
    pub fn vector_backend_name(&self) -> &'static str {
        self.backend
            .as_ref()
            .map(|b| b.capabilities().name)
            .unwrap_or("local")
    }

    fn use_hybrid_indexing(&self) -> bool {
        #[cfg(feature = "local-embeddings")]
        {
            self.hybrid_search_enabled && self.sparse_provider.is_some()
        }

        #[cfg(not(feature = "local-embeddings"))]
        {
            false
        }
    }

    /// Get a reference to the knowledge graph.
    pub fn graph(&self) -> &KnowledgeGraph {
        &self.graph
    }

    /// Expose the indexed chunks for read-only access by the graph extraction
    /// pipeline (v0.13.0). Returns a slice; callers should not mutate.
    pub fn chunks(&self) -> &[ChunkMeta] {
        &self.chunks
    }

    // -----------------------------------------------------------------------
    // Graph RAG entity/relation vector stores (v0.13.1)
    //
    // LightRAG-parity feature: after extraction, we embed each entity's
    // "name + description" and each relation's "source + target + type +
    // description" into separate Qdrant collections. This enables:
    //   - Local mode: semantic entity lookup (not substring match)
    //   - Global mode: semantic relation traversal from themes
    //   - Hybrid mode: fuse vector + entity/relation hits
    // -----------------------------------------------------------------------

    /// Embed and upsert entities into the project's entity vector collection.
    /// Does nothing if Qdrant is not configured.
    pub async fn upsert_entity_vectors(
        &self,
        _project_id: &str,
        entities: &[crate::graph::Entity],
    ) -> Result<usize, String> {
        let Some(backend) = self.backend.as_deref() else {
            return Ok(0);
        };
        if entities.is_empty() {
            return Ok(0);
        }
        let dims = self.embedding_provider.dimensions();
        backend.ensure_entity_collection(dims).await?;

        let texts: Vec<String> = entities
            .iter()
            .map(|e| format!("{}\n{}", e.name, e.description))
            .collect();
        let vectors = self.embedding_provider.embed_batch(&texts).await?;

        let points: Vec<VbEntityPoint> = entities
            .iter()
            .zip(vectors)
            .map(|(e, vec)| VbEntityPoint {
                id: e.name.to_lowercase(),
                vector: vec,
                name: e.name.clone(),
                entity_type: e.entity_type.clone(),
                description: e.description.clone(),
                source_chunks: e.source_chunks.clone(),
            })
            .collect();
        let count = points.len();
        backend.upsert_entities(points).await?;
        Ok(count)
    }

    /// Embed and upsert relations into the project's relation vector collection.
    pub async fn upsert_relation_vectors(
        &self,
        _project_id: &str,
        relations: &[crate::graph::Relation],
    ) -> Result<usize, String> {
        let Some(backend) = self.backend.as_deref() else {
            return Ok(0);
        };
        if relations.is_empty() {
            return Ok(0);
        }
        let dims = self.embedding_provider.dimensions();
        backend.ensure_relation_collection(dims).await?;

        let texts: Vec<String> = relations
            .iter()
            .map(|r| {
                format!(
                    "{} {} {}\n{}",
                    r.source, r.relation_type, r.target, r.description
                )
            })
            .collect();
        let vectors = self.embedding_provider.embed_batch(&texts).await?;

        let points: Vec<VbRelationPoint> = relations
            .iter()
            .zip(vectors)
            .map(|(r, vec)| {
                // Hash-like key so re-extracting the same relation overwrites.
                let id = format!(
                    "{}|{}|{}",
                    r.source.to_lowercase(),
                    r.target.to_lowercase(),
                    r.relation_type.to_lowercase()
                );
                VbRelationPoint {
                    id,
                    vector: vec,
                    source: r.source.clone(),
                    target: r.target.clone(),
                    relation_type: r.relation_type.clone(),
                    description: r.description.clone(),
                    source_chunks: r.source_chunks.clone(),
                }
            })
            .collect();
        let count = points.len();
        backend.upsert_relations(points).await?;
        Ok(count)
    }

    /// Semantic search over the project's entity vector collection.
    /// Returns `(source_chunk_ids, entity_names)` for the top matches.
    pub async fn search_entities_semantic(
        &self,
        _project_id: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<crate::vector_backend::EntityHit>, String> {
        let Some(backend) = self.backend.as_deref() else {
            return Ok(Vec::new());
        };
        let vector = self.embedding_provider.embed_single(query).await?;
        backend.search_entities(vector, top_k, 0.0).await
    }

    /// Semantic search over the project's relation vector collection.
    pub async fn search_relations_semantic(
        &self,
        _project_id: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<crate::vector_backend::RelationHit>, String> {
        let Some(backend) = self.backend.as_deref() else {
            return Ok(Vec::new());
        };
        let vector = self.embedding_provider.embed_single(query).await?;
        backend.search_relations(vector, top_k, 0.0).await
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

        // v0.16.0: route through trait. Backends that don't support hybrid
        // fall back automatically via the trait's default `Err(...)` on
        // `upsert_hybrid_chunks`, which we catch below.
        if let Some(backend) = self.backend.as_deref() {
            let dims = self.embedding_provider.dimensions();

            #[cfg(feature = "local-embeddings")]
            let use_hybrid = self.hybrid_search_enabled
                && self.sparse_provider.is_some()
                && backend.capabilities().hybrid;
            #[cfg(not(feature = "local-embeddings"))]
            let use_hybrid = false;

            if use_hybrid {
                backend
                    .ensure_hybrid_collection(dims)
                    .await
                    .map_err(std::io::Error::other)?;
            } else {
                backend
                    .ensure_collection(dims)
                    .await
                    .map_err(std::io::Error::other)?;
            }

            // Embed in batches of 64.
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

                // Hybrid upsert when sparse provider is active AND backend
                // supports it.
                #[cfg(feature = "local-embeddings")]
                if use_hybrid {
                    if let Some(sparse_prov) = &self.sparse_provider {
                        let sparse_vecs = sparse_prov
                            .embed_batch(&texts)
                            .map_err(std::io::Error::other)?;

                        let hybrid_points: Vec<HybridVectorPoint> = self.chunks
                            [batch_start..batch_end]
                            .iter()
                            .zip(vectors.into_iter())
                            .zip(sparse_vecs.into_iter())
                            .map(|((chunk, dense_vec), sv)| HybridVectorPoint {
                                id: chunk.id.clone(),
                                dense: dense_vec,
                                sparse: VbSparseVector {
                                    indices: sv.indices,
                                    values: sv.values,
                                },
                                payload: ChunkPayload {
                                    chunk_id: chunk.id.clone(),
                                    source_path: chunk.source_path.clone(),
                                    heading: chunk
                                        .heading_hierarchy
                                        .last()
                                        .cloned()
                                        .unwrap_or_default(),
                                    chunk_index: chunk.chunk_index,
                                },
                                content: Some(chunk.content.clone()),
                            })
                            .collect();

                        backend
                            .upsert_hybrid_chunks(hybrid_points)
                            .await
                            .map_err(std::io::Error::other)?;
                        continue;
                    }
                }

                // Dense-only upsert (default path).
                let points: Vec<VectorPoint> = self.chunks[batch_start..batch_end]
                    .iter()
                    .zip(vectors.into_iter())
                    .map(|(chunk, vector)| VectorPoint {
                        id: chunk.id.clone(),
                        vector,
                        payload: ChunkPayload {
                            chunk_id: chunk.id.clone(),
                            source_path: chunk.source_path.clone(),
                            heading: chunk.heading_hierarchy.last().cloned().unwrap_or_default(),
                            chunk_index: chunk.chunk_index,
                        },
                        content: Some(chunk.content.clone()),
                    })
                    .collect();

                backend
                    .upsert_chunks(points)
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
            RetrievalMode::Local | RetrievalMode::Global => self.search_graph(request).await,
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

        // Hybrid path (dense + sparse, linear fusion). Only runs when the
        // backend advertises hybrid capability; otherwise fall back to
        // dense-only search.
        #[cfg(feature = "local-embeddings")]
        if self.hybrid_search_enabled {
            let backend_supports_hybrid = self
                .backend
                .as_ref()
                .map(|b| b.capabilities().hybrid)
                .unwrap_or(false);
            if let (true, Some(backend), Some(sparse_prov)) = (
                backend_supports_hybrid,
                self.backend.as_deref(),
                &self.sparse_provider,
            ) {
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

                let trait_sparse = VbSparseVector {
                    indices: sparse_vec.indices,
                    values: sparse_vec.values,
                };

                let hybrid_hits = match backend
                    .search_chunks_hybrid(dense_vec, trait_sparse, fetch_k, request.score_threshold)
                    .await
                {
                    Ok(hits) => hits,
                    Err(e) => {
                        tracing::warn!("hybrid search failed, falling back to dense-only: {e}");
                        return self.search_vector_dense_only(request, fetch_k).await;
                    }
                };
                let dense_results = hybrid_hits.dense;
                let sparse_results = hybrid_hits.sparse;

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
        // v0.16.0: unified dispatch. The broker selects exactly one
        // backend at construction time — the old Redis-first / Qdrant-
        // fallback branching collapses into a single trait call.
        #[cfg(feature = "redis-vectors")]
        if self.redis_persistence_required {
            if let Some(backend) = self.backend.as_deref() {
                if backend.verify_persistence().await.is_err() {
                    return Vec::new();
                }
            }
        }

        let results = if let Some(backend) = self.backend.as_deref() {
            let query_vector = match self.embedding_provider.embed_single(&request.query).await {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            };

            let hits = match backend
                .search_chunks(query_vector, fetch_k, request.score_threshold)
                .await
            {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };

            hits.into_iter()
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

        self.finish_search_results(request, results).await
    }

    async fn finish_search_results(
        &self,
        request: &MemorySearchRequest,
        mut results: Vec<MemorySearchResult>,
    ) -> Vec<MemorySearchResult> {
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
    ///
    /// v0.13.1 upgrade: when a `project_id` is set and Qdrant is configured,
    /// this routes through [`crate::graph_extractor::extract_query_keywords`]
    /// + semantic entity/relation vector search instead of substring matching.
    ///   Falls back to the in-memory `KnowledgeGraph::search` when the LLM
    ///   endpoint is unavailable or returns empty keywords.
    async fn search_graph(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        let mut chunk_scores: HashMap<String, f32> = HashMap::new();

        // --- v0.13.1 semantic path -----------------------------------------
        // Try the LightRAG-style keyword extraction + entity/relation vector
        // search first. Only available when project_id + a backend that
        // supports entity/relation vectors are set AND THE_ONE_GRAPH_ENABLED
        // is true (the keyword extractor handles the flag).
        let backend_has_entities = self
            .backend
            .as_ref()
            .map(|b| b.capabilities().entities)
            .unwrap_or(false);
        if let (Some(pid), true) = (self.project_id.as_deref(), backend_has_entities) {
            let keywords = crate::graph_extractor::extract_query_keywords(&request.query).await;
            if !keywords.is_empty() {
                // Local mode: search entity collection for low-level identifiers
                for keyword in &keywords.low_level {
                    if let Ok(hits) = self.search_entities_semantic(pid, keyword, 5).await {
                        for (rank, hit) in hits.iter().enumerate() {
                            let score = hit.score * (1.0 - rank as f32 * 0.1).max(0.1);
                            for cid in &hit.source_chunks {
                                let entry = chunk_scores.entry(cid.clone()).or_insert(0.0);
                                *entry = entry.max(score);
                            }
                        }
                    }
                }
                // Global mode: search relation collection for high-level themes
                for theme in &keywords.high_level {
                    if let Ok(hits) = self.search_relations_semantic(pid, theme, 5).await {
                        for (rank, hit) in hits.iter().enumerate() {
                            let score = hit.score * (1.0 - rank as f32 * 0.1).max(0.1) * 0.9;
                            for cid in &hit.source_chunks {
                                let entry = chunk_scores.entry(cid.clone()).or_insert(0.0);
                                *entry = entry.max(score);
                            }
                        }
                    }
                }
            }
        }

        // --- Fallback keyword path ------------------------------------------
        // Always run — supplements the semantic path and is the only path when
        // the LLM endpoint isn't configured. Uses the in-memory graph search
        // over entity names.
        let terms: Vec<String> = request
            .query
            .split_whitespace()
            .map(|w| w.to_string())
            .collect();

        let graph_results = self.graph.search(&terms, request.top_k);
        if chunk_scores.is_empty() && graph_results.is_empty() {
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
        // v0.16.0: unified backend dispatch. The old Redis-first / Qdrant-
        // fallback / cfg-gated branching collapses to one trait call because
        // only one backend is ever active.
        let vector_results: Vec<MemorySearchResult> = {
            let fetch_k = (request.top_k * 2).max(10);
            #[cfg(feature = "redis-vectors")]
            if self.redis_persistence_required {
                if let Some(backend) = self.backend.as_deref() {
                    if backend.verify_persistence().await.is_err() {
                        return self.search_graph(request).await;
                    }
                }
            }
            if let Some(backend) = self.backend.as_deref() {
                let query_vector = match self.embedding_provider.embed_single(&request.query).await
                {
                    Ok(v) => v,
                    Err(_) => return self.search_graph(request).await,
                };

                match backend
                    .search_chunks(query_vector, fetch_k, request.score_threshold)
                    .await
                {
                    Ok(hits) => hits
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
        let graph_results = self.search_graph(request).await;

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

    async fn upsert_chunks_to_backend(&self, chunks: &[ChunkMeta]) -> Result<(), String> {
        if chunks.is_empty() {
            return Ok(());
        }

        let Some(backend) = self.backend.as_deref() else {
            return Ok(());
        };

        #[cfg(feature = "redis-vectors")]
        if self.redis_persistence_required {
            backend.verify_persistence().await?;
        }

        let dims = self.embedding_provider.dimensions();
        let use_hybrid_backend = backend.capabilities().hybrid && self.use_hybrid_indexing();
        if use_hybrid_backend {
            backend.ensure_hybrid_collection(dims).await?;
        } else {
            backend.ensure_collection(dims).await?;
        }

        let batch_size = 64;
        for batch_start in (0..chunks.len()).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(chunks.len());
            let batch = &chunks[batch_start..batch_end];
            let texts: Vec<String> = batch.iter().map(|chunk| chunk.content.clone()).collect();
            let vectors = self.embedding_provider.embed_batch(&texts).await?;

            #[cfg(feature = "local-embeddings")]
            if use_hybrid_backend {
                if let Some(sparse_prov) = &self.sparse_provider {
                    let sparse_vecs = sparse_prov.embed_batch(&texts)?;
                    let hybrid_points: Vec<HybridVectorPoint> = batch
                        .iter()
                        .zip(vectors.into_iter())
                        .zip(sparse_vecs.into_iter())
                        .map(|((chunk, dense), sparse)| HybridVectorPoint {
                            id: chunk.id.clone(),
                            dense,
                            sparse: VbSparseVector {
                                indices: sparse.indices,
                                values: sparse.values,
                            },
                            payload: ChunkPayload {
                                chunk_id: chunk.id.clone(),
                                source_path: chunk.source_path.clone(),
                                heading: chunk
                                    .heading_hierarchy
                                    .last()
                                    .cloned()
                                    .unwrap_or_default(),
                                chunk_index: chunk.chunk_index,
                            },
                            content: Some(chunk.content.clone()),
                        })
                        .collect();

                    backend.upsert_hybrid_chunks(hybrid_points).await?;
                    continue;
                }
            }

            let points: Vec<VectorPoint> = batch
                .iter()
                .zip(vectors.into_iter())
                .map(|(chunk, vector)| VectorPoint {
                    id: chunk.id.clone(),
                    vector,
                    payload: ChunkPayload {
                        chunk_id: chunk.id.clone(),
                        source_path: chunk.source_path.clone(),
                        heading: chunk.heading_hierarchy.last().cloned().unwrap_or_default(),
                        chunk_index: chunk.chunk_index,
                    },
                    content: Some(chunk.content.clone()),
                })
                .collect();

            backend.upsert_chunks(points).await?;
        }

        Ok(())
    }

    #[cfg(feature = "redis-vectors")]
    #[allow(dead_code)]
    async fn ensure_redis_persistence_if_required(
        &self,
        _redis: &RedisVectorStore,
    ) -> Result<(), String> {
        if let Some(backend) = self.backend.as_deref() {
            if self.redis_persistence_required {
                backend.verify_persistence().await?;
            }
        }
        Ok(())
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

        let content = std::fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;
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

        self.upsert_chunks_to_backend(&new_chunks).await?;

        Ok(count)
    }

    /// Ingest a verbatim conversation transcript directly into memory.
    ///
    /// The first version keeps the existing memory model intact by mapping each
    /// transcript message onto a regular `ChunkMeta`, with palace metadata
    /// stored in stable existing fields.
    pub async fn ingest_conversation(
        &mut self,
        source_path: &str,
        transcript: &crate::conversation::ConversationTranscript,
        palace: Option<crate::palace::PalaceMetadata>,
    ) -> Result<usize, String> {
        let source = Path::new(source_path);

        self.remove_by_path(source).await?;

        let new_chunks = chunk_conversation(source_path, transcript, palace.as_ref());
        let count = new_chunks.len();

        for chunk in &new_chunks {
            self.by_id.insert(chunk.id.clone(), self.chunks.len());
            self.chunks.push(chunk.clone());
        }

        self.upsert_chunks_to_backend(&new_chunks).await?;

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

        // v0.16.0: unified delete dispatch through trait.
        if let Some(backend) = self.backend.as_deref() {
            backend
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

    struct TestSparseProvider;

    impl crate::sparse_embeddings::SparseEmbeddingProvider for TestSparseProvider {
        fn name(&self) -> &str {
            "test-sparse"
        }

        fn embed_single(
            &self,
            _text: &str,
        ) -> Result<crate::sparse_embeddings::SparseVector, String> {
            Ok(crate::sparse_embeddings::SparseVector {
                indices: vec![1, 2],
                values: vec![0.5, 1.0],
            })
        }

        fn embed_batch(
            &self,
            texts: &[String],
        ) -> Result<Vec<crate::sparse_embeddings::SparseVector>, String> {
            Ok(texts
                .iter()
                .map(|_| crate::sparse_embeddings::SparseVector {
                    indices: vec![1, 2],
                    values: vec![0.5, 1.0],
                })
                .collect())
        }
    }

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

    #[cfg(feature = "redis-vectors")]
    #[tokio::test]
    async fn redis_memory_engine_reports_redis_backend() {
        let engine = MemoryEngine::new_with_redis(
            "all-MiniLM-L6-v2",
            500,
            RedisEngineConfig {
                redis_url: "redis://127.0.0.1:6379".to_string(),
                index_name: "the_one_memories_test".to_string(),
                persistence_required: false,
            },
        )
        .await
        .expect("redis engine should construct");

        // v0.16.0: the trait reports the backend's self-identified name.
        assert_eq!(engine.vector_backend_name(), "redis-vectors");
    }

    #[cfg(feature = "redis-vectors")]
    #[tokio::test]
    async fn redis_memory_engine_rejects_invalid_index_names() {
        let result = MemoryEngine::new_with_redis(
            "all-MiniLM-L6-v2",
            500,
            RedisEngineConfig {
                redis_url: "redis://127.0.0.1:6379".to_string(),
                index_name: "bad index".to_string(),
                persistence_required: false,
            },
        )
        .await;

        assert!(result.is_err());
        assert!(result.err().unwrap().contains("index_name"));
    }

    #[cfg(feature = "redis-vectors")]
    #[tokio::test]
    async fn redis_memory_engine_rejects_empty_redis_url() {
        let result = MemoryEngine::new_with_redis(
            "all-MiniLM-L6-v2",
            500,
            RedisEngineConfig {
                redis_url: String::new(),
                index_name: "the_one_memories_test".to_string(),
                persistence_required: false,
            },
        )
        .await;

        assert!(result.is_err());
        assert!(result.err().unwrap().contains("redis_url"));
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
    async fn ingested_conversation_is_searchable_by_exact_reasoning() {
        let _ = &*PROVIDER;

        let transcript = crate::conversation::ConversationTranscript {
            source_id: "claude-auth-session".to_string(),
            messages: vec![
                crate::conversation::ConversationMessage {
                    role: crate::conversation::ConversationRole::User,
                    content: "Why did we switch auth vendors?".to_string(),
                    turn_index: 0,
                },
                crate::conversation::ConversationMessage {
                    role: crate::conversation::ConversationRole::Assistant,
                    content: "We switched because refresh token rotation failed in staging and support was slow.".to_string(),
                    turn_index: 1,
                },
            ],
        };

        let mut engine = make_local_engine();
        engine
            .ingest_conversation(
                "/tmp/auth-session.json",
                &transcript,
                Some(crate::palace::PalaceMetadata::new(
                    "proj-auth",
                    Some("hall_facts".to_string()),
                    Some("auth-migration".to_string()),
                )),
            )
            .await
            .expect("conversation should ingest");

        let results = engine
            .search(&MemorySearchRequest {
                query: "refresh token rotation failed in staging".to_string(),
                top_k: 5,
                score_threshold: 0.0,
                mode: RetrievalMode::Naive,
            })
            .await;

        assert!(
            !results.is_empty(),
            "search should return ingested conversation"
        );
        assert!(results[0].chunk.source_path.contains("auth-session"));
        assert!(
            results[0]
                .chunk
                .content
                .contains("refresh token rotation failed in staging"),
            "verbatim assistant content should be searchable"
        );
    }

    #[tokio::test]
    async fn ingested_conversation_carries_palace_metadata_in_chunk_fields() {
        let _ = &*PROVIDER;

        let transcript = crate::conversation::ConversationTranscript {
            source_id: "session".to_string(),
            messages: vec![crate::conversation::ConversationMessage {
                role: crate::conversation::ConversationRole::Assistant,
                content: "Clerk won over Auth0 after the outage review.".to_string(),
                turn_index: 0,
            }],
        };

        let mut engine = make_local_engine();
        engine
            .ingest_conversation(
                "/tmp/session.json",
                &transcript,
                Some(crate::palace::PalaceMetadata::new(
                    "proj-auth",
                    Some("hall_facts".to_string()),
                    Some("auth-migration".to_string()),
                )),
            )
            .await
            .expect("conversation should ingest");

        let chunk = engine
            .fetch_chunk("/tmp/session.json:turn:0")
            .expect("chunk should exist");

        assert_eq!(
            chunk.heading_hierarchy,
            vec![
                "conversation".to_string(),
                "proj-auth".to_string(),
                "hall_facts".to_string(),
                "auth-migration".to_string(),
            ]
        );
        assert_eq!(chunk.signature.as_deref(), Some("hall_facts"));
        assert_eq!(chunk.symbol.as_deref(), Some("auth-migration"));
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

    #[test]
    fn test_use_hybrid_indexing_requires_sparse_provider() {
        let mut engine = make_local_engine();
        assert!(!engine.use_hybrid_indexing());

        engine.set_sparse_provider(Box::new(TestSparseProvider), 0.7, 0.3);
        assert!(engine.use_hybrid_indexing());
    }
}
