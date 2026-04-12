//! Live-database integration tests for the combined
//! Postgres+pgvector backend (v0.16.0 Phase 4).
//!
//! Parallel in shape to Phase 2's `pgvector_roundtrip.rs` and
//! Phase 3's `postgres_state_roundtrip.rs`: the whole suite is
//! gated on `all(feature = "pg-state", feature = "pg-vectors")`,
//! each test reads the production env surface and returns early
//! via `matching_env()` when the vars aren't set, and no
//! `_TEST`-suffixed shadow vars exist.
//!
//! ## Running locally
//!
//! Start a pgvector-enabled Postgres container:
//!
//! ```bash
//! docker run --rm -d --name the-one-pg-combined \
//!     -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=the_one_combined_test \
//!     -p 55434:5432 ankane/pgvector
//! ```
//!
//! Then run the suite:
//!
//! ```bash
//! THE_ONE_STATE_TYPE=postgres-combined \
//! THE_ONE_VECTOR_TYPE=postgres-combined \
//! THE_ONE_STATE_URL=postgres://postgres:pw@localhost:55434/the_one_combined_test \
//! THE_ONE_VECTOR_URL=postgres://postgres:pw@localhost:55434/the_one_combined_test \
//! cargo test -p the-one-mcp --features pg-state,pg-vectors \
//!     --test postgres_combined_roundtrip -- --test-threads=1
//! ```
//!
//! Both URL vars **must be byte-identical** — the Phase 2 env parser
//! rule 7 enforces this at broker startup, and these tests rely on
//! the rule so they can pick either var and use it as "the"
//! combined URL.
//!
//! `--test-threads=1` matters: every test drops and re-creates the
//! `the_one` schema, and parallel tests would race.
//!
//! ## Why the suite is test-crate integration, not broker-level
//!
//! The most load-bearing assertions (shared-pool construction,
//! cross-trait consistency, mirror helpers against live
//! `AppConfig`) don't need the full broker spin-up. They exercise
//! `the_one_mcp::postgres_combined::build_shared_pool` directly,
//! construct `PostgresStateStore::from_pool` and
//! `PgVectorBackend::from_pool` explicitly, and observe the pool
//! via sqlx-level introspection (`pg_stat_activity`, the two
//! migration tracking tables). Broker-level integration would
//! duplicate state-cache and memory-cache plumbing that Phase 1/2/3
//! already covered.

#![cfg(all(feature = "pg-state", feature = "pg-vectors"))]

use async_trait::async_trait;

use the_one_core::state_store::StateStore;
use the_one_core::storage::postgres::{PostgresStateConfig, PostgresStateStore};
use the_one_mcp::postgres_combined::{
    build_shared_pool, mirror_pgvector_config, mirror_state_postgres_config,
};
use the_one_memory::embeddings::EmbeddingProvider;
use the_one_memory::pg_vector::{PgVectorBackend, PgVectorConfig};
use the_one_memory::vector_backend::{ChunkPayload, VectorBackend, VectorPoint};

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Returns the combined URL iff BOTH env vars are set to
/// `postgres-combined` AND both URLs are present AND byte-identical.
/// Skips gracefully via `None` when the env isn't ready for the
/// combined path.
fn matching_env() -> Option<String> {
    if std::env::var("THE_ONE_STATE_TYPE").ok().as_deref() != Some("postgres-combined") {
        return None;
    }
    if std::env::var("THE_ONE_VECTOR_TYPE").ok().as_deref() != Some("postgres-combined") {
        return None;
    }
    let state_url = std::env::var("THE_ONE_STATE_URL").ok()?;
    let vector_url = std::env::var("THE_ONE_VECTOR_URL").ok()?;
    if state_url.trim().is_empty() || vector_url.trim().is_empty() {
        return None;
    }
    if state_url != vector_url {
        // The Phase 2 env parser enforces byte-identical URLs at
        // broker startup — production would have failed loudly
        // before reaching this code. If a user runs the tests
        // with mismatched URLs, we skip cleanly rather than
        // faking the parser's check.
        return None;
    }
    Some(state_url)
}

/// Drop + recreate the `the_one` schema for a clean-slate run.
/// Uses a direct sqlx pool — same pattern as Phases 2 + 3.
async fn reset_schema(url: &str) -> Result<(), String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .map_err(|e| format!("reset_schema connect: {e}"))?;
    sqlx::query("DROP SCHEMA IF EXISTS the_one CASCADE")
        .execute(&pool)
        .await
        .map_err(|e| format!("reset_schema drop: {e}"))?;
    pool.close().await;
    Ok(())
}

/// Tiny pool sizing so dev Postgres instances don't exhaust their
/// `max_connections` when the split-pool + combined test suites
/// run back-to-back.
fn test_state_config() -> PostgresStateConfig {
    PostgresStateConfig {
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_ms: 5_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_timeout_ms: 10_000,
        ..PostgresStateConfig::default()
    }
}

#[allow(dead_code)] // kept for completeness; combined path ignores
                    // pool sizing from PgVectorConfig but this helper
                    // exists so future tests exercising split-pool
                    // can reuse it.
fn test_vector_config() -> PgVectorConfig {
    PgVectorConfig {
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_ms: 5_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        ..PgVectorConfig::default()
    }
}

/// Deterministic 1024-dim stub provider — same shape as the Phase 2
/// tests use. Keeps the test file self-contained and avoids pulling
/// in a real FastEmbed dependency (the default `local-embeddings`
/// feature handles that, but the stub keeps the test surface
/// narrower and faster).
struct DeterministicProvider;

#[async_trait]
impl EmbeddingProvider for DeterministicProvider {
    fn name(&self) -> &str {
        "combined-test-provider"
    }

    fn dimensions(&self) -> usize {
        1024
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| fake_vector(t)).collect())
    }
}

/// FNV-1a seeded 1024-dim pseudo-embedding. Identical implementation
/// to Phase 2's `pgvector_roundtrip.rs` so behavior is byte-stable
/// across phases. Not a real embedding — just enough structure for
/// cosine-distance assertions to rank matching queries above
/// unrelated ones.
fn fake_vector(text: &str) -> Vec<f32> {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut v: Vec<f32> = Vec::with_capacity(1024);
    let mut state = hash;
    for _ in 0..1024 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let raw = ((state >> 32) as u32) as f32 / u32::MAX as f32;
        v.push(raw * 2.0 - 1.0);
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

// ---------------------------------------------------------------------------
// 1. Bootstrap — shared pool builder applies both migration runners
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn combined_build_shared_pool_runs_both_migrations() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset_schema");

    let provider = DeterministicProvider;
    let pool = build_shared_pool(&test_state_config(), &url, &provider)
        .await
        .expect("build_shared_pool should succeed on a fresh DB");

    // Both tracking tables must exist and be populated.
    let pgvector_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM the_one.pgvector_migrations")
            .fetch_one(&pool)
            .await
            .expect("pgvector_migrations count");
    assert!(
        pgvector_count >= 5,
        "expected >=5 pgvector migrations applied (0000-0004), got {pgvector_count}"
    );

    let state_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM the_one.state_migrations")
        .fetch_one(&pool)
        .await
        .expect("state_migrations count");
    assert!(
        state_count >= 2,
        "expected >=2 state migrations applied (0000-0001), got {state_count}"
    );

    // The pgvector extension must also be installed on the target
    // database after build_shared_pool returns — this is the
    // preflight's job.
    let ext_installed: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(&pool)
            .await
            .expect("pg_extension query");
    assert!(ext_installed, "vector extension should be installed");

    pool.close().await;
}

// ---------------------------------------------------------------------------
// 2. Idempotence — running build_shared_pool twice is safe
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn combined_build_shared_pool_is_idempotent() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset_schema");

    let provider = DeterministicProvider;

    // First build — applies migrations fresh.
    let pool1 = build_shared_pool(&test_state_config(), &url, &provider)
        .await
        .expect("first build");
    pool1.close().await;

    // Second build — migration runners should detect already-
    // applied state in both tracking tables and exit cleanly.
    let pool2 = build_shared_pool(&test_state_config(), &url, &provider)
        .await
        .expect("second build must succeed (idempotent)");
    pool2.close().await;
}

// ---------------------------------------------------------------------------
// 3. Dim-mismatch — wrong embedding provider fails before returning
// ---------------------------------------------------------------------------

struct WrongDimProvider;

#[async_trait]
impl EmbeddingProvider for WrongDimProvider {
    fn name(&self) -> &str {
        "wrong-dim-test-provider"
    }

    fn dimensions(&self) -> usize {
        384
    }

    async fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(vec![])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn combined_build_shared_pool_rejects_wrong_dim() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset_schema");

    let provider = WrongDimProvider;
    let err = build_shared_pool(&test_state_config(), &url, &provider)
        .await
        .expect_err("wrong dim must fail");

    // Error should be InvalidProjectConfig with a message mentioning
    // the migrated vs live dim values so operators can act on it.
    let msg = err.to_string();
    assert!(
        msg.contains("1024") && msg.contains("384"),
        "error message should mention both dims: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 4. Shared-pool state + vector roundtrip — both trait roles share
//    the exact same PgPool instance, and writes from one are visible
//    to queries from the other.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn combined_state_and_vector_share_pool_and_persist() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset_schema");

    let provider = DeterministicProvider;
    let pool = build_shared_pool(&test_state_config(), &url, &provider)
        .await
        .expect("build_shared_pool");

    // Mirror helpers produce live configs from typical defaults.
    let state_config =
        mirror_state_postgres_config(&the_one_core::config::StatePostgresConfig::default());
    let vector_config =
        mirror_pgvector_config(&the_one_core::config::VectorPgvectorConfig::default());

    // Build both trait-role adapters from clones of the same pool.
    let project_id = "combined-roundtrip";
    let state = PostgresStateStore::from_pool(pool.clone(), &state_config, project_id);
    let vector = PgVectorBackend::from_pool(pool.clone(), &vector_config, project_id, &provider)
        .expect("PgVectorBackend::from_pool");

    // -- State side: write a project profile and read it back.
    state
        .upsert_project_profile(r#"{"languages":["rust"],"combined":true}"#)
        .expect("upsert_project_profile");
    let fetched = state
        .latest_project_profile()
        .expect("latest_project_profile")
        .expect("profile must be present");
    assert!(fetched.contains("\"combined\":true"));

    // -- Vector side: upsert a chunk, search for it, verify rank.
    let embeddings = provider
        .embed_batch(&["hello combined".to_string()])
        .await
        .expect("embed");
    vector
        .upsert_chunks(vec![VectorPoint {
            id: "combined-chunk-1".to_string(),
            vector: embeddings[0].clone(),
            payload: ChunkPayload {
                chunk_id: "combined-chunk-1".to_string(),
                source_path: "combined.rs".to_string(),
                heading: "combined heading".to_string(),
                chunk_index: 0,
            },
            content: Some("hello combined".to_string()),
        }])
        .await
        .expect("upsert_chunks");

    let query = provider
        .embed_batch(&["hello combined".to_string()])
        .await
        .expect("query embed");
    let hits = vector
        .search_chunks(query[0].clone(), 5, 0.0)
        .await
        .expect("search_chunks");
    assert!(
        hits.iter().any(|h| h.chunk_id == "combined-chunk-1"),
        "search must return the upserted chunk; got {hits:?}"
    );

    // -- Cross-trait: after a state write and a vector write, both
    // values are still observable via the shared pool. The strongest
    // Rust-level pointer-equality check isn't possible (sqlx::PgPool
    // doesn't expose its internal Arc), so we settle for functional
    // equivalence: both adapters read/write the same schema and the
    // same physical rows via distinct trait-role objects that were
    // constructed from clones of the same `pool` binding.
    let state_profile = state.latest_project_profile().unwrap().unwrap();
    assert!(state_profile.contains("\"combined\":true"));
    let vector_hits = vector
        .search_chunks(query[0].clone(), 5, 0.0)
        .await
        .unwrap();
    assert_eq!(vector_hits.len(), 1);

    pool.close().await;
}

// ---------------------------------------------------------------------------
// 5. from_pool constructors skip connect + migration work — the
//    evidence: calling from_pool on a fresh pool that has NOT been
//    prepared by build_shared_pool should silently succeed at the
//    Rust level but fail on the first real query. This test pins
//    that contract so a future refactor can't sneak migration logic
//    back into from_pool.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn combined_from_pool_constructors_do_not_run_migrations() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset_schema");

    // Build a RAW pool without going through build_shared_pool.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("raw pool connect");

    let state_config =
        mirror_state_postgres_config(&the_one_core::config::StatePostgresConfig::default());

    // PostgresStateStore::from_pool is synchronous and does no
    // query work — it must succeed even though the schema is
    // empty.
    let state = PostgresStateStore::from_pool(pool.clone(), &state_config, "no-migrations");
    assert_eq!(state.project_id(), "no-migrations");

    // The first real query MUST fail with a Postgres error (the
    // tables don't exist yet). This is how we prove from_pool
    // didn't secretly run migrations — if it had, this would
    // return `Ok(None)` instead of an error.
    let result = state.latest_project_profile();
    assert!(
        result.is_err(),
        "latest_project_profile on an unmigrated pool must surface an error; got {result:?}"
    );

    pool.close().await;
}
