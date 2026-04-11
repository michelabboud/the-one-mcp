-- the-one-mcp v0.16.0 Phase 2 — entities table.
--
-- LightRAG-style graph-RAG entity vectors. Each entity has:
--   - a name (e.g. "McpBroker::with_state_store")
--   - a type (e.g. "function", "struct", "concept")
--   - a description carrying the entity's semantic payload
--   - the list of chunks it was extracted from
--   - an embedding of the description for retrieval
--
-- Dim hardcoded to 1024 per Decision C. Same HNSW parameters as
-- the chunks table (m = 16, ef_construction = 100).

CREATE TABLE IF NOT EXISTS the_one.entities (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL,
    name                TEXT NOT NULL,
    entity_type         TEXT NOT NULL,
    description         TEXT NOT NULL,
    -- JSONB array of chunk IDs. Stored as JSONB (not TEXT[])
    -- because the broker-facing trait carries `Vec<String>` and
    -- JSONB gives us free NULL, length, and containment queries
    -- for later observability without schema churn.
    source_chunks       JSONB NOT NULL,
    dense_vector        vector(1024) NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS entities_dense_hnsw
    ON the_one.entities
    USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

CREATE INDEX IF NOT EXISTS entities_project_idx
    ON the_one.entities (project_id);

-- Common query: "is this entity already in the graph?"
-- Scoped per-project so two projects can share an entity name.
CREATE INDEX IF NOT EXISTS entities_name_idx
    ON the_one.entities (project_id, name);
