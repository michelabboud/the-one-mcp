//! Backend-agnostic vector storage trait (Phase A1, v0.16.0).
//!
//! Prior to v0.16.0, [`MemoryEngine`] held two specific `Option<T>` fields
//! — `Option<AsyncQdrantBackend>` and `Option<RedisVectorStore>` — and every
//! write/search path did a hand-written `if let Some(qdrant) … else if let
//! Some(redis) …` chain at 16 different sites in `lib.rs`. Every new backend
//! (pgvector, PG-combined, Redis+RediSearch, Milvus, Weaviate, …) would
//! multiply that branching.
//!
//! This module introduces [`VectorBackend`], a single trait that every
//! backend implementation targets. [`MemoryEngine`] now holds
//! `Option<Box<dyn VectorBackend>>` and dispatches via method calls.
//!
//! ## Design principles
//!
//! 1. **Neutral types.** The trait speaks in [`VectorPoint`], [`VectorHit`],
//!    [`EntityPoint`], etc. — types defined here, not Qdrant-specific.
//!    Each backend converts between its native format and these neutral
//!    types inside its `impl VectorBackend` block. This means pgvector and
//!    Redis impls never depend on `qdrant.rs`.
//!
//! 2. **Capability reporting via [`BackendCapabilities`].** Not every
//!    backend supports every operation (Redis-Vector today has no entity
//!    or hybrid support). Callers can inspect `backend.capabilities()` to
//!    decide whether to call an operation or skip it. Methods that a
//!    backend does not support are handled via **default implementations
//!    that preserve v0.14.x silent-skip semantics** (Ok(()) on writes,
//!    Vec::new() on reads). This keeps behaviour bit-for-bit identical to
//!    the pre-refactor code.
//!
//! 3. **`async_trait` for `Box<dyn>` compatibility.** Standard pattern.
//!    The broker already uses `async_trait` elsewhere.
//!
//! 4. **`String` error type.** The existing Qdrant and Redis backends all
//!    use `Result<_, String>`. Keeping the same error shape means we can
//!    refactor without touching any of the `.map_err(...)` chains at the
//!    call sites.
//!
//! ## What this file does NOT do
//!
//! - Define how backends are constructed (that's a factory concern,
//!   handled in `lib.rs::MemoryEngine::new_with_backend` and in the broker's
//!   `build_memory_engine`).
//! - Deal with state-store operations (audit, diary, navigation, etc.) —
//!   those live behind `the_one_core::state_store::StateStore`.
//! - Introduce any new runtime cost. The trait is dispatched via vtable
//!   once per operation; the overhead is negligible compared to the
//!   network round-trip to Qdrant.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Chunk operations
// ---------------------------------------------------------------------------

/// Payload carried on every chunk vector — the minimal metadata needed to
/// rehydrate a search hit back into a usable result. Mirrors the existing
/// `QdrantPayload` field-for-field so conversion is a single `From` impl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkPayload {
    pub chunk_id: String,
    pub source_path: String,
    pub heading: String,
    pub chunk_index: usize,
}

/// A dense chunk vector + its payload. One point per chunk.
#[derive(Debug, Clone)]
pub struct VectorPoint {
    /// Stable chunk identifier. Backends may hash this into a u64 point ID
    /// (Qdrant) or use it verbatim (Redis).
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: ChunkPayload,
    /// Optional full text content. Required by Redis (which stores it as a
    /// searchable hash field so FT.SEARCH can fall back to BM25 text search
    /// when the dense vector has low similarity). Qdrant ignores it.
    pub content: Option<String>,
}

/// A sparse vector — indices + values pairs, matching Qdrant's named-sparse
/// format and SPLADE-style outputs.
#[derive(Debug, Clone, Default)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

/// A chunk point carrying both dense AND sparse vectors for hybrid indexing.
#[derive(Debug, Clone)]
pub struct HybridVectorPoint {
    pub id: String,
    pub dense: Vec<f32>,
    pub sparse: SparseVector,
    pub payload: ChunkPayload,
    /// See [`VectorPoint::content`].
    pub content: Option<String>,
}

/// A single chunk search hit.
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub chunk_id: String,
    pub source_path: String,
    pub heading: String,
    pub chunk_index: usize,
    pub score: f32,
}

/// Hybrid search returns two parallel lists — one scored by dense
/// similarity, one by sparse similarity — so the caller can fuse them with
/// whatever weighting it wants.
#[derive(Debug, Clone, Default)]
pub struct HybridHits {
    pub dense: Vec<VectorHit>,
    pub sparse: Vec<VectorHit>,
}

// ---------------------------------------------------------------------------
// Entity operations (LightRAG-style graph RAG)
// ---------------------------------------------------------------------------

/// A single entity vector point.
#[derive(Debug, Clone)]
pub struct EntityPoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
}

/// A single entity search hit.
#[derive(Debug, Clone)]
pub struct EntityHit {
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Relation operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RelationPoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RelationHit {
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Capability reporting
// ---------------------------------------------------------------------------

/// Static capability report for a backend. Callers inspect this to decide
/// whether to route an operation through the backend or skip it (e.g.,
/// Redis-Vector has `entities == false` and `hybrid == false`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BackendCapabilities {
    /// Short name for logs and metrics ("qdrant", "redis-vectors", "pgvector").
    pub name: &'static str,
    /// Chunk dense upsert + search.
    pub chunks: bool,
    /// Hybrid dense + sparse chunk upsert + search.
    pub hybrid: bool,
    /// Entity vector upsert + search (LightRAG-style).
    pub entities: bool,
    /// Relation vector upsert + search.
    pub relations: bool,
    /// Image vector upsert + search (CLIP-style).
    pub images: bool,
    /// Backend requires or supports persistence verification (Redis AOF).
    pub persistence_verifiable: bool,
}

impl BackendCapabilities {
    /// Helper for backends that support every operation.
    pub const fn full(name: &'static str) -> Self {
        Self {
            name,
            chunks: true,
            hybrid: true,
            entities: true,
            relations: true,
            images: true,
            persistence_verifiable: false,
        }
    }

    /// Helper for chunks-only backends (current Redis-Vector).
    pub const fn chunks_only(name: &'static str) -> Self {
        Self {
            name,
            chunks: true,
            hybrid: false,
            entities: false,
            relations: false,
            images: false,
            persistence_verifiable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Unified interface for any vector storage backend. Implemented today by
/// `AsyncQdrantBackend` and `RedisVectorStore`; designed so pgvector,
/// PG-combined, Weaviate, Milvus, etc. can be added as new files without
/// touching `MemoryEngine`.
///
/// # Default implementations
///
/// Operations that not every backend supports have default implementations
/// that preserve v0.14.x silent-skip semantics:
///
/// - Entity/relation write ops default to `Ok(())` (silent no-op).
/// - Entity/relation read ops default to `Ok(Vec::new())` (empty results).
/// - `search_hybrid` defaults to `Err(...)`, so callers fall back to
///   dense-only search (matches v0.14.x behavior in
///   `search_vector` → `search_vector_dense_only`).
/// - `verify_persistence` defaults to `Ok(())` (non-Redis backends always
///   succeed).
///
/// Backends that **do** support these operations override the default
/// implementations in their `impl VectorBackend for X` block.
#[async_trait]
pub trait VectorBackend: Send + Sync {
    /// Static capability report. Callers can inspect this to decide whether
    /// to route an operation through the backend or skip it.
    fn capabilities(&self) -> BackendCapabilities;

    // ── Chunk operations (REQUIRED) ────────────────────────────────────

    /// Create or verify the dense chunk collection / index. Called once
    /// before the first `upsert_chunks`.
    async fn ensure_collection(&self, dims: usize) -> Result<(), String>;

    /// Create or verify the hybrid (dense + sparse) chunk collection.
    /// Default: forward to `ensure_collection` (backends without sparse
    /// support just create the dense collection).
    async fn ensure_hybrid_collection(&self, dims: usize) -> Result<(), String> {
        self.ensure_collection(dims).await
    }

    /// Upsert a batch of dense chunk vectors.
    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String>;

    /// Upsert a batch of hybrid chunk vectors. Default: returns `Err` so
    /// callers fall back to dense-only upsert.
    async fn upsert_hybrid_chunks(&self, points: Vec<HybridVectorPoint>) -> Result<(), String> {
        let _ = points;
        Err("hybrid chunk upsert not supported by this backend".to_string())
    }

    /// Dense semantic search over chunks.
    async fn search_chunks(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<VectorHit>, String>;

    /// Hybrid dense + sparse search over chunks. Default: returns `Err` so
    /// callers fall back to dense-only search.
    async fn search_chunks_hybrid(
        &self,
        dense: Vec<f32>,
        sparse: SparseVector,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<HybridHits, String> {
        let _ = (dense, sparse, top_k, score_threshold);
        Err("hybrid chunk search not supported by this backend".to_string())
    }

    /// Delete every chunk point whose payload has the given source path.
    /// Used by the file watcher when a markdown file is removed or
    /// re-ingested.
    async fn delete_by_source_path(&self, source_path: &str) -> Result<(), String>;

    // ── Entity operations (OPTIONAL) ───────────────────────────────────

    /// Ensure the entity collection exists for the given project. Default:
    /// best-effort no-op (backends without entity support silently succeed).
    async fn ensure_entity_collection(&self, dims: usize) -> Result<(), String> {
        let _ = dims;
        Ok(())
    }

    /// Upsert a batch of entity vectors. Default: silent skip (Ok) to
    /// preserve v0.14.x semantics in `upsert_entity_vectors` which returned
    /// `Ok(0)` when Qdrant was unavailable.
    async fn upsert_entities(&self, points: Vec<EntityPoint>) -> Result<(), String> {
        let _ = points;
        Ok(())
    }

    /// Semantic search over entity vectors. Default: empty-results fallback.
    async fn search_entities(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<EntityHit>, String> {
        let _ = (query_vector, top_k, score_threshold);
        Ok(Vec::new())
    }

    // ── Relation operations (OPTIONAL) ─────────────────────────────────

    async fn ensure_relation_collection(&self, dims: usize) -> Result<(), String> {
        let _ = dims;
        Ok(())
    }

    async fn upsert_relations(&self, points: Vec<RelationPoint>) -> Result<(), String> {
        let _ = points;
        Ok(())
    }

    async fn search_relations(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<RelationHit>, String> {
        let _ = (query_vector, top_k, score_threshold);
        Ok(Vec::new())
    }

    // ── Persistence verification (Redis AOF only) ──────────────────────

    /// Verify that the backend has durable persistence enabled. Returns
    /// `Ok(())` on backends where durability is always guaranteed (Qdrant,
    /// Postgres) or where the operator has not required it. Returns `Err`
    /// only when the backend is configured to enforce persistence and the
    /// check fails — Redis-Vector uses this to refuse startup if AOF is
    /// disabled when `redis_persistence_required = true`.
    async fn verify_persistence(&self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_capabilities_full_reports_every_operation_supported() {
        let caps = BackendCapabilities::full("qdrant");
        assert_eq!(caps.name, "qdrant");
        assert!(caps.chunks);
        assert!(caps.hybrid);
        assert!(caps.entities);
        assert!(caps.relations);
        assert!(caps.images);
    }

    #[test]
    fn backend_capabilities_chunks_only_reports_only_chunks() {
        let caps = BackendCapabilities::chunks_only("redis-vectors");
        assert_eq!(caps.name, "redis-vectors");
        assert!(caps.chunks);
        assert!(!caps.hybrid);
        assert!(!caps.entities);
        assert!(!caps.relations);
        assert!(!caps.images);
    }
}
