-- the-one-mcp v0.16.0 Phase 2 — pgvector migration tracking table.
--
-- This file is the hand-rolled replacement for `sqlx::migrate!`'s
-- `_sqlx_migrations` table. The `migrate` feature was dropped from
-- sqlx's feature set because it transitively references `sqlx-sqlite`
-- which conflicts with rusqlite 0.39's `libsqlite3-sys`. See the
-- `pg-vectors` feature comment in `crates/the-one-memory/Cargo.toml`
-- for the bisection that led to this decision.
--
-- The Rust-side runner in `pg_vector::migrations::apply_all` reads
-- every `.sql` file in this directory via `include_str!`, checks
-- which versions have already been applied by querying this table,
-- and applies missing versions inside a transaction per migration.
--
-- Schema name `the_one` is hardcoded. Operators who override
-- `[vector.pgvector].schema` in `config.json` only affect NEW
-- installs — cross-schema migration is out of scope for Phase 2.

CREATE SCHEMA IF NOT EXISTS the_one;

CREATE TABLE IF NOT EXISTS the_one.pgvector_migrations (
    -- Four-digit version from the migration filename (e.g. 0002 for
    -- `0002_chunks_table.sql`). Unique so a double-apply is rejected.
    version       INTEGER PRIMARY KEY,
    -- Human-readable description parsed from the filename suffix
    -- (e.g. "chunks_table" from `0002_chunks_table.sql`). Purely
    -- informational — equality checks use `version` only.
    description   TEXT NOT NULL,
    -- SHA-256 of the migration file's bytes at apply time. The
    -- apply runner rehashes the embedded file on every run and
    -- refuses to continue if the stored checksum doesn't match —
    -- this catches "someone edited 0002_chunks_table.sql post-
    -- ship" drift, which would be invisible to a plain version
    -- check.
    checksum      BYTEA NOT NULL,
    -- Milliseconds since the Unix epoch, matching the rest of the
    -- workspace's time representation. Intentionally NOT
    -- TIMESTAMPTZ — `chrono` was deferred to Phase 3 and this
    -- schema avoids it across the board.
    applied_at_ms BIGINT NOT NULL
);
