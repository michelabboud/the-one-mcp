-- the-one-mcp v0.16.0 Phase 3 — PostgresStateStore migration tracking.
--
-- This is the hand-rolled replacement for `sqlx::migrate!`'s
-- `_sqlx_migrations` table, mirroring Phase 2's `pg_vector::migrations`
-- runner pattern. The `migrate` feature was dropped from sqlx's
-- feature set in Phase 2 because it transitively references
-- `sqlx-sqlite`, which collides with `rusqlite 0.39`'s
-- `libsqlite3-sys ^0.37.0`. See
-- `crates/the-one-memory/Cargo.toml`'s `pg-vectors` feature comment
-- for the full bisection and rationale.
--
-- The tracking table name here is **distinct** from Phase 2's
-- `pgvector_migrations` so a future Phase 4 combined deployment
-- (postgres-combined, one pool serving both state and vectors)
-- can share the same `the_one` schema without the two migration
-- runners stepping on each other's versions. Phase 4 is a new
-- adapter that orchestrates both hand-rolled runners against one
-- pool.
--
-- The Rust-side runner in `postgres::migrations::apply_all` reads
-- every `.sql` file in this directory via `include_str!`, checks
-- applied versions against this table, rehashes and compares
-- checksums, and applies missing versions. Drift detection is the
-- same SHA-256 rehash the pgvector runner uses.
--
-- Schema `the_one` may already exist — the pgvector runner may have
-- created it first, or the operator may have created it manually.
-- `CREATE SCHEMA IF NOT EXISTS` handles both cases.

CREATE SCHEMA IF NOT EXISTS the_one;

CREATE TABLE IF NOT EXISTS the_one.state_migrations (
    -- Four-digit version from the filename (e.g. 1 for
    -- `0001_state_schema_v7.sql`). Unique so re-apply is rejected.
    version       INTEGER PRIMARY KEY,
    -- Descriptor parsed from the filename suffix (e.g.
    -- "state_schema_v7"). Purely informational.
    description   TEXT NOT NULL,
    -- SHA-256 of the migration file's bytes at apply time. The
    -- runner rehashes the embedded file on every boot and refuses
    -- to continue if the stored checksum diverges from the live
    -- file — catches "someone edited a migration post-ship."
    checksum      BYTEA NOT NULL,
    -- Milliseconds since the Unix epoch. Every backend in this
    -- workspace stores timestamps as BIGINT epoch_ms; Phase 3 does
    -- not use `chrono` / `TIMESTAMPTZ`. The cargo `links` conflict
    -- that drove Decision B's narrowing in Phase 2 applies here
    -- too — see the feature comment in
    -- `crates/the-one-core/Cargo.toml`.
    applied_at_ms BIGINT NOT NULL
);
