-- the-one-mcp v0.16.0 Phase 2 — chunks table.
--
-- Decision C (locked in by Michel in the Phase 2 prompt) hardcodes
-- the vector dimension to 1024. This matches the default
-- quality-tier embedding provider (BGE-large-en-v1.5). The
-- `PgVectorBackend::new` constructor rejects the backend if the
-- active embedding provider reports a different dim. Changing the
-- dim later is a NEW migration (e.g. `0006_reshape_chunks_dim.sql`),
-- not a runtime parameter.
--
-- Columns mirror the trait's ChunkPayload surface and add the
-- fields needed for hybrid search (once Decision D lands):
--   - dense_vector          — HNSW-indexed vector(1024)
--   - sparse_vector_indices — SPLADE-style sparse indices
--   - sparse_vector_values  — matching float values
--
-- Hybrid search is NOT wired up in Phase 2 — the dense-only path
-- is all that ships, per STOP condition on Decision D. Phase 2
-- lays the schema for it so Phase 2.5 / Phase 4 doesn't require
-- a schema migration.
--
-- HNSW parameters are baked into the index definition per
-- Decision C rationale: tuning HNSW on an existing install means
-- DROP INDEX + CREATE INDEX manually, which is documented in
-- docs/guides/production-hardening-v0.15.md § 15.

CREATE TABLE IF NOT EXISTS the_one.chunks (
    -- Trait-level stable chunk identifier. Stored as TEXT (not
    -- UUID or u64) to preserve the exact round-trip Qdrant and
    -- Redis-Vector use. Matches `VectorPoint::id` verbatim.
    id                    TEXT PRIMARY KEY,
    -- Project scoping. Every broker call carries this; partial
    -- indexes below accelerate per-project lookups.
    project_id            TEXT NOT NULL,
    -- Trait payload fields.
    source_path           TEXT NOT NULL,
    heading               TEXT NOT NULL,
    chunk_index           BIGINT NOT NULL,
    -- Full chunk text. Required for Redis FT.SEARCH BM25
    -- fallback; carried through to pgvector for consistency so
    -- backend swap is lossless.
    content               TEXT,
    -- Dense chunk vector. Dimension hardcoded to 1024 per
    -- Decision C. The `pgvector::Vector` Rust type wraps
    -- `Vec<f32>` and implements `sqlx::Type<Postgres>` via the
    -- crate's `sqlx` feature.
    dense_vector          vector(1024) NOT NULL,
    -- Sparse vector components (empty for backends not using
    -- hybrid search). Stored as parallel arrays so a single
    -- COPY / INSERT writes both at once; Phase 2 leaves these
    -- NULL on every insert because hybrid search (Decision D)
    -- is deferred.
    sparse_vector_indices INTEGER[],
    sparse_vector_values  REAL[],
    -- Milliseconds since Unix epoch. Matches the rest of the
    -- workspace's time representation. `chrono` was deferred
    -- to Phase 3 — this column stays BIGINT regardless.
    created_at_epoch_ms   BIGINT NOT NULL
);

-- HNSW index on the dense vector for cosine-similarity search.
-- `m = 16` + `ef_construction = 100` are the defaults baked into
-- both Qdrant and the Phase 1 backend selection scheme. Operators
-- wanting different tuning drop + recreate this index manually;
-- see the operations guide.
CREATE INDEX IF NOT EXISTS chunks_dense_hnsw
    ON the_one.chunks
    USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

-- Per-project fast path for list + count queries.
CREATE INDEX IF NOT EXISTS chunks_project_idx
    ON the_one.chunks (project_id);

-- Delete-by-source-path (watcher re-ingest path) uses this.
CREATE INDEX IF NOT EXISTS chunks_source_idx
    ON the_one.chunks (project_id, source_path);
