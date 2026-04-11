-- the-one-mcp v0.16.0 Phase 2 — relations table.
--
-- LightRAG-style graph-RAG relations between entities. Each
-- relation has:
--   - a source entity name
--   - a target entity name
--   - a relation type (e.g. "calls", "uses", "extends")
--   - a description of the semantic link
--   - the chunks it was extracted from
--   - an embedding of the description for retrieval
--
-- Dim hardcoded to 1024 per Decision C. Same HNSW parameters as
-- the chunks and entities tables.

CREATE TABLE IF NOT EXISTS the_one.relations (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL,
    source              TEXT NOT NULL,
    target              TEXT NOT NULL,
    relation_type       TEXT NOT NULL,
    description         TEXT NOT NULL,
    -- JSONB array of chunk IDs. See `entities.source_chunks`
    -- comment for rationale.
    source_chunks       JSONB NOT NULL,
    dense_vector        vector(1024) NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS relations_dense_hnsw
    ON the_one.relations
    USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

CREATE INDEX IF NOT EXISTS relations_project_idx
    ON the_one.relations (project_id);

-- Directed-edge lookup (source → target) is the common access
-- pattern for graph traversal.
CREATE INDEX IF NOT EXISTS relations_edge_idx
    ON the_one.relations (project_id, source, target);
