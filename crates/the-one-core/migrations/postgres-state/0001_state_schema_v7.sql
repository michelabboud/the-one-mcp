-- the-one-mcp v0.16.0 Phase 3 — PostgresStateStore schema v7.
--
-- This file is the Postgres equivalent of the SQLite schema built up
-- across migrations 1..7 in `crates/the-one-core/src/storage/sqlite.rs::run_migrations`.
-- Because a fresh Postgres deployment has no v1..v6 history, we ship
-- the full v7 shape in one migration instead of recreating the
-- SQLite 7-step history.
--
-- ## Translations from SQLite
--
-- - `INTEGER PRIMARY KEY AUTOINCREMENT` → `BIGSERIAL PRIMARY KEY`
-- - `strftime('%s','now') * 1000` → Rust-side `EXTRACT(EPOCH FROM NOW()) * 1000`
--   applied in the query layer (Postgres has no equivalent column default)
-- - `FTS5 virtual table` → `TSVECTOR` column on the main table + GIN index
--   + `websearch_to_tsquery('simple', ...)` for matching + `ts_rank`
--   for ordering. The FTS column lives on `diary_entries.content_tsv`
--   and is populated by the Rust layer in `upsert_diary_entry` inside
--   a transaction so the main table and the FTS data are committed
--   atomically — same v0.16.0 atomicity guarantee the SQLite side got.
-- - `TEXT` columns stay `TEXT`, `INTEGER` → `BIGINT`, `REAL` → `DOUBLE PRECISION`
--
-- ## Schema version reporting
--
-- `PostgresStateStore::schema_version()` reads the MAX version from
-- `the_one.state_migrations`. A fresh install with this migration
-- applied returns `1` (the migration version), NOT `7` (the SQLite
-- schema version). Broker code that compares schema versions across
-- backends must use `StateStoreCapabilities::schema_versioned` +
-- the backend's own version space — there is no cross-backend
-- version number, by design. The `CURRENT_SCHEMA_VERSION = 7` constant
-- in `sqlite.rs` is a SQLite-internal concept.

-- ── Project profiles ─────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS the_one.project_profiles (
    project_id          TEXT PRIMARY KEY,
    profile_json        TEXT NOT NULL,
    updated_at_epoch_ms BIGINT NOT NULL
);

-- ── Approvals ────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS the_one.approvals (
    id                  BIGSERIAL PRIMARY KEY,
    project_id          TEXT NOT NULL,
    action_key          TEXT NOT NULL,
    scope               TEXT NOT NULL,
    -- Postgres doesn't have a native 0/1 BOOLEAN tradition like
    -- SQLite; we still store an integer to match the SQLite
    -- on-the-wire representation so any cross-backend data export
    -- is byte-compatible.
    approved            INTEGER NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL,
    UNIQUE (project_id, action_key, scope)
);

-- ── Audit events (schema v7) ─────────────────────────────────────────
--
-- Postgres ships the v7 shape from day one: `outcome` + `error_kind`
-- columns are not incremental additions here. Everything below
-- corresponds to the merged state after SQLite migrations 1 + 7.

CREATE TABLE IF NOT EXISTS the_one.audit_events (
    id                  BIGSERIAL PRIMARY KEY,
    project_id          TEXT NOT NULL,
    event_type          TEXT NOT NULL,
    payload_json        TEXT NOT NULL,
    outcome             TEXT NOT NULL DEFAULT 'unknown',
    error_kind          TEXT,
    created_at_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_events_project_outcome
    ON the_one.audit_events (project_id, outcome, created_at_epoch_ms DESC);

CREATE INDEX IF NOT EXISTS idx_audit_events_project_event
    ON the_one.audit_events (project_id, event_type, created_at_epoch_ms DESC);

-- ── Conversation sources ─────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS the_one.conversation_sources (
    id                  BIGSERIAL PRIMARY KEY,
    project_id          TEXT NOT NULL,
    transcript_path     TEXT NOT NULL,
    memory_path         TEXT NOT NULL,
    format              TEXT NOT NULL,
    wing                TEXT,
    hall                TEXT,
    room                TEXT,
    message_count       BIGINT NOT NULL,
    updated_at_epoch_ms BIGINT NOT NULL,
    UNIQUE (project_id, transcript_path)
);

CREATE INDEX IF NOT EXISTS idx_conversation_sources_project_wing_updated
    ON the_one.conversation_sources (project_id, wing, updated_at_epoch_ms DESC);

-- ── AAAK lessons ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS the_one.aaak_lessons (
    lesson_id               TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL,
    pattern_key             TEXT NOT NULL,
    role                    TEXT NOT NULL,
    canonical_text          TEXT NOT NULL,
    occurrence_count        BIGINT NOT NULL,
    confidence_percent      BIGINT NOT NULL,
    source_transcript_path  TEXT,
    updated_at_epoch_ms     BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_aaak_lessons_project_confidence
    ON the_one.aaak_lessons (project_id, confidence_percent DESC, occurrence_count DESC);

-- ── Diary entries (with native tsvector FTS replacement) ─────────────

CREATE TABLE IF NOT EXISTS the_one.diary_entries (
    entry_id            TEXT NOT NULL,
    project_id          TEXT NOT NULL,
    entry_date          TEXT NOT NULL,
    mood                TEXT,
    tags_json           TEXT NOT NULL,
    content             TEXT NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL,
    updated_at_epoch_ms BIGINT NOT NULL,
    -- v0.16.0 Phase 3 — FTS5 virtual table replaced by a tsvector
    -- column. The Rust-side `upsert_diary_entry` populates it inside
    -- the same transaction as the main row via:
    --
    --     content_tsv = to_tsvector(
    --         'simple',
    --         COALESCE(mood, '') || ' ' ||
    --         <tags joined with spaces> || ' ' ||
    --         content
    --     )
    --
    -- `'simple'` is the dictionary config: no stemming, no
    -- stop-words, just whitespace/punctuation tokenization. This
    -- matches FTS5's default behavior more closely than `'english'`
    -- (which would stem "running" → "run" and break exact-match
    -- searches) and works uniformly across languages.
    --
    -- The column is NOT NULL with a default empty tsvector so
    -- existing queries that don't update this field still produce
    -- a valid row (belt + suspenders — the Rust layer always sets
    -- it, but defensive SQL guards against bugs).
    content_tsv         TSVECTOR NOT NULL DEFAULT to_tsvector('simple', ''),
    PRIMARY KEY (project_id, entry_id)
);

-- Range + ordering index matching SQLite's.
CREATE INDEX IF NOT EXISTS idx_diary_entries_project_date
    ON the_one.diary_entries (project_id, entry_date DESC, updated_at_epoch_ms DESC, entry_id);

-- GIN index over the tsvector for `@@ websearch_to_tsquery(...)` queries.
CREATE INDEX IF NOT EXISTS idx_diary_entries_content_tsv
    ON the_one.diary_entries USING GIN (content_tsv);

-- ── Navigation nodes ─────────────────────────────────────────────────
--
-- Uses the merged shape from SQLite migration 5 (the "v2" tables
-- after the CASCADE refactor). The self-referential foreign key
-- uses DEFERRABLE INITIALLY DEFERRED so multi-row inserts in a
-- single transaction can insert children before parents and still
-- resolve at COMMIT. SQLite doesn't have this feature; Postgres
-- operators get it for free.

CREATE TABLE IF NOT EXISTS the_one.navigation_nodes (
    node_id             TEXT NOT NULL,
    project_id          TEXT NOT NULL,
    kind                TEXT NOT NULL,
    label               TEXT NOT NULL,
    parent_node_id      TEXT,
    wing                TEXT,
    hall                TEXT,
    room                TEXT,
    updated_at_epoch_ms BIGINT NOT NULL,
    PRIMARY KEY (project_id, node_id),
    FOREIGN KEY (project_id, parent_node_id)
        REFERENCES the_one.navigation_nodes (project_id, node_id)
        ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX IF NOT EXISTS idx_navigation_nodes_project_parent_kind_label
    ON the_one.navigation_nodes (project_id, parent_node_id, kind, label, node_id);

-- ── Navigation tunnels ───────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS the_one.navigation_tunnels (
    tunnel_id           TEXT NOT NULL,
    project_id          TEXT NOT NULL,
    from_node_id        TEXT NOT NULL,
    to_node_id          TEXT NOT NULL,
    updated_at_epoch_ms BIGINT NOT NULL,
    CHECK (from_node_id <> to_node_id),
    PRIMARY KEY (project_id, tunnel_id),
    FOREIGN KEY (project_id, from_node_id)
        REFERENCES the_one.navigation_nodes (project_id, node_id)
        ON DELETE CASCADE,
    FOREIGN KEY (project_id, to_node_id)
        REFERENCES the_one.navigation_nodes (project_id, node_id)
        ON DELETE CASCADE,
    UNIQUE (project_id, from_node_id, to_node_id)
);

CREATE INDEX IF NOT EXISTS idx_navigation_tunnels_project_endpoints
    ON the_one.navigation_tunnels (project_id, from_node_id, to_node_id, tunnel_id);
