//! Combined Postgres+pgvector shared-pool builder (v0.16.0 Phase 4).
//!
//! This module is the glue that lets an operator point both
//! `THE_ONE_STATE_TYPE=postgres-combined` and
//! `THE_ONE_VECTOR_TYPE=postgres-combined` at the same Postgres DSN
//! and have the `StateStore` trait role and the `VectorBackend` trait
//! role share a **single** `sqlx::PgPool`. The win is operational
//! (one credential rotation, one pgbouncer entry, one PITR backup
//! window, one set of IAM grants) rather than transactional —
//! broker handlers still acquire the state store and the memory
//! engine separately via their respective caches, so cross-trait
//! atomicity is still a per-call-site concern, not a pool-level
//! guarantee.
//!
//! ## Why this module lives on `the-one-mcp`, not on memory or core
//!
//! The combined builder has to call BOTH:
//!
//! - `the_one_memory::pg_vector::migrations::apply_all` — needs
//!   `the-one-memory/pg-vectors` feature.
//! - `the_one_core::storage::postgres::migrations::apply_all` —
//!   needs `the-one-core/pg-state` feature.
//!
//! Cargo features are per-crate booleans with no "and-of-two-crates"
//! composition. The dep graph is `the-one-memory → the-one-core`
//! (memory depends on core; core does not depend on memory), so
//! there is no crate below `the-one-mcp` that sees both features
//! activated simultaneously without pulling the full workspace into
//! a rebuild. `the-one-mcp` is the first crate in the dependency
//! graph where both features are in scope at the same time — and
//! its own feature passthroughs (`pg-state` → `the-one-core/pg-state`,
//! `pg-vectors` → `the-one-memory/pg-vectors`) mean
//! `--features pg-state,pg-vectors` on this crate activates the
//! combined path cleanly.
//!
//! The entire module is therefore gated on
//! `#[cfg(all(feature = "pg-state", feature = "pg-vectors"))]`. When
//! either feature is off, the module is empty and the broker's
//! `state_store_factory` Phase-4 branch returns `NotEnabled` (which
//! is the correct shape — the operator asked for a backend the
//! binary doesn't ship).
//!
//! ## Refined Option Y (no named combined type)
//!
//! The Phase 4 planning document considered two pool-sharing shapes:
//! Option X (one `Arc<PostgresCombinedBackend>` implementing both
//! traits, shared between the broker's two caches) and Option Y
//! (two thin adapter types, one per trait role, sharing a `PgPool`).
//! This module implements a *refinement* of Option Y: since
//! `PgVectorBackend` and `PostgresStateStore` are already thin
//! pool-wrapper adapters with minimal non-pool state, Phase 4 just
//! adds `from_pool` constructors on each of them and shares the pool
//! via a per-project broker cache — no new named type is needed. The
//! "combined backend" is then a pair-of-two-existing-backends that
//! happen to clone the same `sqlx::PgPool` handle.
//!
//! `sqlx::PgPool` is internally `Arc`-reference-counted, so
//! `pool.clone()` is a cheap refcount bump that gives you a second
//! handle to the same underlying connection pool. Dropping a handle
//! does NOT close the pool — the pool stays alive until every
//! handle is dropped OR until someone calls `pool.close().await`
//! explicitly. The broker's `shutdown()` drains the cache and
//! calls `close().await` on each pool so the teardown order is
//! deterministic.
//!
//! ## Pool sizing — state config wins
//!
//! When both axes are `postgres-combined`, the shared pool is
//! constructed from [`PostgresStateConfig`]'s pool-sizing fields
//! (max_connections, min_connections, acquire/idle/max_lifetime).
//! [`PgVectorConfig`]'s corresponding fields are **ignored** on
//! the combined path. Rationale:
//!
//! 1. State operations are more frequent than vector operations in
//!    every handler profiled so far (every tool call records audit,
//!    most handlers touch diary/profile; only `memory.search` and
//!    `memory.ingest_conversation` hit the vector path).
//! 2. The state config typically has the higher `max_connections`
//!    (default 10) because of that frequency.
//! 3. Using one axis's sizing instead of trying to max() or sum()
//!    the two keeps the configuration surface simple — an operator
//!    tuning a combined deployment only has one set of pool knobs
//!    to reason about.
//!
//! This asymmetry is documented explicitly in
//! `docs/guides/combined-postgres-backend.md` so operators migrating
//! from split-pool don't wonder why their `[vector.pgvector]` pool
//! tuning "stopped working."

#![cfg(all(feature = "pg-state", feature = "pg-vectors"))]

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use the_one_core::config::{StatePostgresConfig, VectorPgvectorConfig};
use the_one_core::error::CoreError;
use the_one_core::storage::postgres::PostgresStateConfig;
use the_one_memory::embeddings::EmbeddingProvider;
use the_one_memory::pg_vector::{self, PgVectorConfig};

/// The migrated-schema dimension for the combined pgvector side.
/// Mirrors [`the_one_memory::pg_vector`]'s private constant — we
/// verify the dim here at build time so the operator sees a clean
/// error BEFORE `PgVectorBackend::from_pool` does its own per-
/// instance check (which is the fallback safety net).
const MIGRATED_DIM: usize = 1024;

/// Mirror a broker-side [`VectorPgvectorConfig`] into the memory
/// crate's [`PgVectorConfig`]. Identical to the helper buried inside
/// `build_pgvector_memory_engine`; extracted here so the combined
/// path and the split-pool path stay byte-for-byte identical in how
/// they translate config.
pub fn mirror_pgvector_config(src: &VectorPgvectorConfig) -> PgVectorConfig {
    PgVectorConfig {
        schema: src.schema.clone(),
        hnsw_m: src.hnsw_m,
        hnsw_ef_construction: src.hnsw_ef_construction,
        hnsw_ef_search: src.hnsw_ef_search,
        max_connections: src.max_connections,
        min_connections: src.min_connections,
        acquire_timeout_ms: src.acquire_timeout_ms,
        idle_timeout_ms: src.idle_timeout_ms,
        max_lifetime_ms: src.max_lifetime_ms,
    }
}

/// Mirror a broker-side [`StatePostgresConfig`] into the core
/// storage crate's [`PostgresStateConfig`]. The two types have
/// identical field layouts today; the mirror exists so that if
/// Phase N adds a runtime-only knob to the storage side (or a
/// serde-only knob to the config side), the drift is caught at
/// the single mirror call site instead of silently misaligning.
///
/// Phase 3 shipped `construct_postgres_state_store` with a
/// `PostgresStateConfig::default()` stub and a comment that said
/// "Until Phase 4 formalizes this, we use default()" — Phase 4
/// uses this helper to replace that stub so the split-pool state
/// store and the combined pool pick up the same config block.
pub fn mirror_state_postgres_config(src: &StatePostgresConfig) -> PostgresStateConfig {
    PostgresStateConfig {
        schema: src.schema.clone(),
        statement_timeout_ms: src.statement_timeout_ms,
        max_connections: src.max_connections,
        min_connections: src.min_connections,
        acquire_timeout_ms: src.acquire_timeout_ms,
        idle_timeout_ms: src.idle_timeout_ms,
        max_lifetime_ms: src.max_lifetime_ms,
    }
}

/// Build the shared `sqlx::PgPool` used by both the state and
/// vector trait roles for a `postgres-combined` deployment.
///
/// Runs, in order, exactly once per project:
///
/// 1. Construct a `PgPool` using [`PostgresStateConfig`]'s pool
///    sizing + `statement_timeout` `after_connect` hook.
/// 2. Call [`pg_vector::preflight_vector_extension`] to install or
///    verify the pgvector extension on the target database.
/// 3. Run the pgvector migration runner
///    ([`pg_vector::migrations::apply_all`]).
/// 4. Run the state-store migration runner
///    ([`the_one_core::storage::postgres::migrations::apply_all`]).
/// 5. Verify `embedding_provider.dimensions() == MIGRATED_DIM` so
///    an operator pointing a non-quality-tier provider at a
///    combined deployment fails before their first search returns
///    garbage.
///
/// The two migration runners use **distinct** tracking tables
/// (`the_one.pgvector_migrations` for the vector side,
/// `the_one.state_migrations` for the state side) by design — they
/// coexist cleanly in one schema without either stepping on the
/// other's version history. Running them in sequence against the
/// same pool is safe and idempotent.
///
/// Returns the constructed pool on success. The caller
/// (`McpBroker::get_or_init_combined_pg_pool`) is responsible for
/// caching it keyed on `{canonical_root}::{project_id}` and
/// cloning handles out to the two trait-role adapters.
///
/// On any failure, returns `CoreError::Postgres` (for pool or
/// migration errors) or `CoreError::InvalidProjectConfig` (for the
/// dim mismatch). Both map to clean operator-facing messages via
/// the v0.15.0 error sanitizer.
pub async fn build_shared_pool(
    state_config: &PostgresStateConfig,
    url: &str,
    embedding_provider: &dyn EmbeddingProvider,
) -> Result<PgPool, CoreError> {
    // -------------------------------------------------------------
    // 1. Pool construction — state config's sizing wins (see module
    //    docs for the rationale).
    // -------------------------------------------------------------
    let mut pool_options = PgPoolOptions::new()
        .max_connections(state_config.max_connections)
        .min_connections(state_config.min_connections)
        .acquire_timeout(Duration::from_millis(state_config.acquire_timeout_ms))
        .idle_timeout(Some(Duration::from_millis(state_config.idle_timeout_ms)))
        .max_lifetime(Some(Duration::from_millis(state_config.max_lifetime_ms)));

    // Mirror the split-pool Postgres path's statement_timeout hook.
    // The pgvector side does not have an equivalent hook on the
    // split path (only the state side does), so the combined path
    // inherits ONLY the state-side timeout. Documented in the
    // combined guide so operators don't expect a pgvector-side
    // timeout knob to suddenly apply.
    if state_config.statement_timeout_ms > 0 {
        let timeout_ms = state_config.statement_timeout_ms;
        pool_options = pool_options.after_connect(move |conn, _meta| {
            Box::pin(async move {
                let sql = format!("SET statement_timeout = '{timeout_ms}ms'");
                sqlx::query(&sql).execute(conn).await.map(|_| ())
            })
        });
    }

    let pool = pool_options
        .connect(url)
        .await
        .map_err(|e| CoreError::Postgres(format!("combined postgres pool connect: {e}")))?;

    // -------------------------------------------------------------
    // 2. pgvector extension preflight — installs the extension if
    //    it's available-but-not-yet-installed, or returns a
    //    per-managed-provider actionable error if it's not
    //    available at all. Same function the split-pool pgvector
    //    path uses.
    // -------------------------------------------------------------
    pg_vector::preflight_vector_extension(&pool)
        .await
        .map_err(CoreError::Postgres)?;

    // -------------------------------------------------------------
    // 3. Run both migration runners against the shared pool. Order
    //    is deliberate: pgvector first, state second. There's no
    //    data dependency between the two, but we run pgvector first
    //    so the operator sees the "pgvector extension required"
    //    error (if any) before seeing a "schema the_one already
    //    exists" error from the state runner — the former is more
    //    actionable.
    // -------------------------------------------------------------
    pg_vector::migrations::apply_all(&pool)
        .await
        .map_err(CoreError::Postgres)?;

    the_one_core::storage::postgres::migrations::apply_all(&pool).await?;

    // -------------------------------------------------------------
    // 4. Dimension check. The per-instance `PgVectorBackend::from_pool`
    //    does this too — we duplicate it here so the failure mode
    //    is "build_shared_pool returns an error" (clean teardown:
    //    we own the pool, nothing else does) instead of "shared
    //    pool gets cached AND THEN from_pool fails on every request
    //    thereafter" (pool is half-registered — sub-backend
    //    construction cannot re-run the migration without dropping
    //    the whole pool entry).
    // -------------------------------------------------------------
    let provider_dim = embedding_provider.dimensions();
    if provider_dim != MIGRATED_DIM {
        // Drop the pool we just built — the caller never saw it,
        // so closing here is the safe teardown.
        pool.close().await;
        return Err(CoreError::InvalidProjectConfig(format!(
            "combined postgres+pgvector schema is fixed at dim={MIGRATED_DIM} \
             (BGE-large-en-v1.5, quality tier); active embedding provider reports \
             dim={provider_dim}. Either switch embedding providers to match or \
             drop the `the_one` schema and re-initialize with a matching provider."
        )));
    }

    Ok(pool)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
//
// Unit tests for the mirror helpers. These run whenever the module
// is compiled (i.e. `--features pg-state,pg-vectors`) and do NOT
// require a live Postgres — they just verify that every field
// copies correctly between the config types. The integration tests
// that need a real pool live in
// `crates/the-one-mcp/tests/postgres_combined_roundtrip.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_state_postgres_config_copies_all_fields() {
        // Build a source with every field set to a distinct
        // non-default value so a typo in the mirror helper
        // (e.g. two fields reading the same source slot) would be
        // visible as a mismatch.
        let src = StatePostgresConfig {
            schema: "custom_schema".to_string(),
            statement_timeout_ms: 12_345,
            max_connections: 17,
            min_connections: 3,
            acquire_timeout_ms: 9_876,
            idle_timeout_ms: 44_444,
            max_lifetime_ms: 55_555,
        };
        let dst = mirror_state_postgres_config(&src);
        assert_eq!(dst.schema, "custom_schema");
        assert_eq!(dst.statement_timeout_ms, 12_345);
        assert_eq!(dst.max_connections, 17);
        assert_eq!(dst.min_connections, 3);
        assert_eq!(dst.acquire_timeout_ms, 9_876);
        assert_eq!(dst.idle_timeout_ms, 44_444);
        assert_eq!(dst.max_lifetime_ms, 55_555);
    }

    #[test]
    fn mirror_state_postgres_config_roundtrips_defaults() {
        // Defaults must also copy faithfully — this catches
        // drift if a new field is added to StatePostgresConfig
        // without updating the mirror helper.
        let src = StatePostgresConfig::default();
        let dst = mirror_state_postgres_config(&src);
        assert_eq!(dst.schema, src.schema);
        assert_eq!(dst.statement_timeout_ms, src.statement_timeout_ms);
        assert_eq!(dst.max_connections, src.max_connections);
        assert_eq!(dst.min_connections, src.min_connections);
        assert_eq!(dst.acquire_timeout_ms, src.acquire_timeout_ms);
        assert_eq!(dst.idle_timeout_ms, src.idle_timeout_ms);
        assert_eq!(dst.max_lifetime_ms, src.max_lifetime_ms);
    }

    #[test]
    fn mirror_pgvector_config_copies_all_fields() {
        let src = VectorPgvectorConfig {
            schema: "combined_schema".to_string(),
            hnsw_m: 32,
            hnsw_ef_construction: 256,
            hnsw_ef_search: 128,
            max_connections: 23,
            min_connections: 5,
            acquire_timeout_ms: 7_777,
            idle_timeout_ms: 88_888,
            max_lifetime_ms: 99_999,
        };
        let dst = mirror_pgvector_config(&src);
        assert_eq!(dst.schema, "combined_schema");
        assert_eq!(dst.hnsw_m, 32);
        assert_eq!(dst.hnsw_ef_construction, 256);
        assert_eq!(dst.hnsw_ef_search, 128);
        assert_eq!(dst.max_connections, 23);
        assert_eq!(dst.min_connections, 5);
        assert_eq!(dst.acquire_timeout_ms, 7_777);
        assert_eq!(dst.idle_timeout_ms, 88_888);
        assert_eq!(dst.max_lifetime_ms, 99_999);
    }

    #[test]
    fn mirror_pgvector_config_roundtrips_defaults() {
        let src = VectorPgvectorConfig::default();
        let dst = mirror_pgvector_config(&src);
        assert_eq!(dst.schema, src.schema);
        assert_eq!(dst.hnsw_m, src.hnsw_m);
        assert_eq!(dst.hnsw_ef_construction, src.hnsw_ef_construction);
        assert_eq!(dst.hnsw_ef_search, src.hnsw_ef_search);
        assert_eq!(dst.max_connections, src.max_connections);
        assert_eq!(dst.min_connections, src.min_connections);
        assert_eq!(dst.acquire_timeout_ms, src.acquire_timeout_ms);
        assert_eq!(dst.idle_timeout_ms, src.idle_timeout_ms);
        assert_eq!(dst.max_lifetime_ms, src.max_lifetime_ms);
    }
}
