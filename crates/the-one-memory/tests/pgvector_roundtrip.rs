//! Live-database integration tests for `PgVectorBackend` (v0.16.0 Phase 2).
//!
//! These tests run against a real Postgres + pgvector instance. They
//! are gated on the **production env surface** — the same
//! `THE_ONE_VECTOR_TYPE=pgvector` + `THE_ONE_VECTOR_URL=<dsn>` vars
//! the broker uses in production. If those aren't set to the right
//! values, every test returns early via `matching_env()` — no panic,
//! no error, the test is simply skipped. This is the § 7 requirement
//! from the Phase 2 prompt: "no `_TEST`-suffixed shadow vars — the
//! test harness reads the same production env surface per § 1."
//!
//! ## Running these tests locally
//!
//! Start a pgvector-enabled Postgres container:
//!
//! ```bash
//! docker run --rm -d --name pgvector-test \
//!     -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=the_one_test \
//!     -p 55432:5432 ankane/pgvector
//! ```
//!
//! Then run the tests:
//!
//! ```bash
//! THE_ONE_VECTOR_TYPE=pgvector \
//! THE_ONE_VECTOR_URL=postgres://postgres:pw@localhost:55432/the_one_test \
//! cargo test -p the-one-memory --features pg-vectors,local-embeddings \
//!     --test pgvector_roundtrip -- --test-threads=1
//! ```
//!
//! `--test-threads=1` matters: the tests share a schema and would
//! race if run in parallel. Each test `DROP`s + re-creates the
//! schema at the start.
//!
//! ## Why it's feature-gated
//!
//! The entire file is gated on `cfg(feature = "pg-vectors")`. When
//! the feature is off, every `fn` below compiles to an empty module
//! and the file contributes zero test entries. Default workspace
//! builds (no pgvector feature) remain unchanged.

#![cfg(feature = "pg-vectors")]

use async_trait::async_trait;

use the_one_memory::embeddings::EmbeddingProvider;
use the_one_memory::pg_vector::{PgVectorBackend, PgVectorConfig};
use the_one_memory::vector_backend::{
    ChunkPayload, EntityPoint, RelationPoint, VectorBackend, VectorPoint,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the pgvector connection URL iff the production env vars
/// are set correctly for this test run. Tests skip gracefully via
/// `return` when the env vars don't match.
///
/// Deliberately NOT a macro — a function lets each test's `return`
/// happen in its own scope, which is easier to read than a
/// skip-early macro that rewrites control flow.
fn matching_env() -> Option<String> {
    if std::env::var("THE_ONE_VECTOR_TYPE").ok().as_deref() != Some("pgvector") {
        return None;
    }
    let url = std::env::var("THE_ONE_VECTOR_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

/// Simple deterministic embedding provider for tests. Returns
/// `[1024]f32` vectors seeded from an FNV-1a hash of the input —
/// fast, reproducible, and matches the migrated schema dim.
struct DeterministicProvider;

#[async_trait]
impl EmbeddingProvider for DeterministicProvider {
    fn name(&self) -> &str {
        "deterministic-test-provider"
    }

    fn dimensions(&self) -> usize {
        1024
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| fake_vector(t)).collect())
    }
}

/// FNV-1a seeded pseudo-embedding. Produces a stable 1024-dim vector
/// from a string — two calls with the same input always return the
/// same vector, and small string perturbations produce different
/// vectors. Not a real embedding, just enough for cosine-distance
/// tests to assert "query for 'foo' ranks the 'foo' chunk above
/// others."
fn fake_vector(text: &str) -> Vec<f32> {
    // Start with FNV-1a 64-bit offset basis.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Expand the 64-bit hash into a 1024-element f32 vector. Using a
    // simple LCG progression off the hash so each slot has a
    // distinct-ish value. Normalize to unit length so cosine
    // distance reads cleanly.
    let mut v: Vec<f32> = Vec::with_capacity(1024);
    let mut state = hash;
    for _ in 0..1024 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Take the top 32 bits, map to f32 in [-1, 1].
        let raw = ((state >> 32) as u32) as f32 / u32::MAX as f32;
        v.push(raw * 2.0 - 1.0);
    }
    // Normalize.
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Tear down any leftover `the_one` schema from a previous run, so
/// each test starts from a clean slate. Uses a direct sqlx pool —
/// PgVectorBackend doesn't expose a drop-schema method and shouldn't
/// (operators should never DROP schemas through the broker).
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

fn test_config() -> PgVectorConfig {
    PgVectorConfig {
        // Use tiny pool sizing so rapid CREATE/DROP in tests doesn't
        // exhaust the dev Postgres's max_connections.
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_ms: 5_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        ..PgVectorConfig::default()
    }
}

fn chunk_point(id: &str, text: &str, idx: usize) -> VectorPoint {
    VectorPoint {
        id: id.to_string(),
        vector: fake_vector(text),
        payload: ChunkPayload {
            chunk_id: id.to_string(),
            source_path: format!("src/{id}.md"),
            heading: format!("Heading for {id}"),
            chunk_index: idx,
        },
        content: Some(text.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pgvector_migrations_bootstrap_clean_database() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "mig-test", &provider)
        .await
        .expect("backend new");

    // Sanity: applying the migrations twice is idempotent — the
    // second call should verify checksums and return Ok.
    backend.close().await;
    let backend2 = PgVectorBackend::new(&test_config(), &url, "mig-test", &provider)
        .await
        .expect("backend re-new (idempotent)");
    backend2.close().await;
}

#[tokio::test]
async fn pgvector_chunk_upsert_search_roundtrip() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "roundtrip", &provider)
        .await
        .expect("backend new");

    backend
        .ensure_collection(1024)
        .await
        .expect("ensure_collection");

    // Insert three chunks with distinct content.
    let chunks = vec![
        chunk_point(
            "chunk-alpha",
            "alpha beta gamma about rust borrow checker",
            0,
        ),
        chunk_point("chunk-beta", "pgvector HNSW hybrid search indexing", 1),
        chunk_point("chunk-gamma", "unrelated cooking recipe for pasta", 2),
    ];
    backend.upsert_chunks(chunks).await.expect("upsert");

    // Query for "pgvector HNSW" — chunk-beta's own vector should be
    // the top hit because we use the same text for query and stored
    // content (deterministic hash).
    let query = fake_vector("pgvector HNSW hybrid search indexing");
    let hits = backend.search_chunks(query, 3, 0.0).await.expect("search");
    assert!(!hits.is_empty(), "expected at least one hit");
    assert_eq!(
        hits[0].chunk_id, "chunk-beta",
        "top hit should be chunk-beta for pgvector query, got {hits:?}"
    );

    backend.close().await;
}

#[tokio::test]
async fn pgvector_chunk_upsert_is_idempotent_per_id() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "idempotent", &provider)
        .await
        .expect("backend new");
    backend.ensure_collection(1024).await.expect("ensure");

    // Upsert the same chunk twice with different content — ON
    // CONFLICT DO UPDATE should apply.
    let first = vec![chunk_point("chunk-x", "original content version", 0)];
    backend.upsert_chunks(first).await.expect("upsert 1");

    let second = vec![chunk_point("chunk-x", "updated content version", 0)];
    backend.upsert_chunks(second).await.expect("upsert 2");

    // Searching for the update-version should now return chunk-x.
    let query = fake_vector("updated content version");
    let hits = backend.search_chunks(query, 5, 0.0).await.expect("search");
    let found = hits.iter().find(|h| h.chunk_id == "chunk-x");
    assert!(found.is_some(), "chunk-x should exist after 2 upserts");

    backend.close().await;
}

#[tokio::test]
async fn pgvector_delete_by_source_path_removes_matching_chunks() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "delete-test", &provider)
        .await
        .expect("backend new");
    backend.ensure_collection(1024).await.expect("ensure");

    // Insert two chunks under different source paths.
    let chunks = vec![
        chunk_point("keep", "chunk we should keep", 0),
        VectorPoint {
            id: "drop".to_string(),
            vector: fake_vector("chunk we should drop"),
            payload: ChunkPayload {
                chunk_id: "drop".to_string(),
                source_path: "src/drop.md".to_string(),
                heading: "Drop me".to_string(),
                chunk_index: 0,
            },
            content: Some("chunk we should drop".to_string()),
        },
    ];
    backend.upsert_chunks(chunks).await.expect("upsert");

    // Delete only the drop source.
    backend
        .delete_by_source_path("src/drop.md")
        .await
        .expect("delete");

    // Search should now only find the keep chunk.
    let query = fake_vector("chunk");
    let hits = backend
        .search_chunks(query, 10, -1.0)
        .await
        .expect("search");
    assert!(hits.iter().any(|h| h.chunk_id == "keep"));
    assert!(
        !hits.iter().any(|h| h.chunk_id == "drop"),
        "drop chunk should be gone: {hits:?}"
    );

    backend.close().await;
}

#[tokio::test]
async fn pgvector_entity_upsert_search_roundtrip() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "entity-test", &provider)
        .await
        .expect("backend new");
    backend
        .ensure_entity_collection(1024)
        .await
        .expect("ensure");

    let entities = vec![
        EntityPoint {
            id: "ent-1".to_string(),
            vector: fake_vector("McpBroker state_by_project cache"),
            name: "McpBroker".to_string(),
            entity_type: "struct".to_string(),
            description: "the broker's state cache".to_string(),
            source_chunks: vec!["chunk-a".to_string(), "chunk-b".to_string()],
        },
        EntityPoint {
            id: "ent-2".to_string(),
            vector: fake_vector("unrelated cooking pasta"),
            name: "Pasta".to_string(),
            entity_type: "concept".to_string(),
            description: "not related".to_string(),
            source_chunks: vec![],
        },
    ];
    backend.upsert_entities(entities).await.expect("upsert");

    let query = fake_vector("McpBroker state_by_project cache");
    let hits = backend
        .search_entities(query, 5, 0.0)
        .await
        .expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].name, "McpBroker");
    assert_eq!(hits[0].source_chunks, vec!["chunk-a", "chunk-b"]);

    backend.close().await;
}

#[tokio::test]
async fn pgvector_relation_upsert_search_roundtrip() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let backend = PgVectorBackend::new(&test_config(), &url, "rel-test", &provider)
        .await
        .expect("backend new");
    backend
        .ensure_relation_collection(1024)
        .await
        .expect("ensure");

    let relations = vec![RelationPoint {
        id: "rel-1".to_string(),
        vector: fake_vector("McpBroker owns StateStore cache"),
        source: "McpBroker".to_string(),
        target: "StateStore".to_string(),
        relation_type: "owns".to_string(),
        description: "broker owns per-project state stores".to_string(),
        source_chunks: vec!["chunk-a".to_string()],
    }];
    backend.upsert_relations(relations).await.expect("upsert");

    let query = fake_vector("McpBroker owns StateStore cache");
    let hits = backend
        .search_relations(query, 5, 0.0)
        .await
        .expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].source, "McpBroker");
    assert_eq!(hits[0].target, "StateStore");
    assert_eq!(hits[0].relation_type, "owns");
    assert_eq!(hits[0].source_chunks, vec!["chunk-a"]);

    backend.close().await;
}

#[tokio::test]
async fn pgvector_provider_dim_mismatch_fails_construction() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");

    // A provider that reports dim=768 — migration expects 1024.
    struct WrongDimProvider;
    #[async_trait]
    impl EmbeddingProvider for WrongDimProvider {
        fn name(&self) -> &str {
            "wrong-dim-test-provider"
        }
        fn dimensions(&self) -> usize {
            768
        }
        async fn embed_batch(&self, _: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(vec![])
        }
    }

    // `PgVectorBackend` doesn't implement Debug, so we can't use
    // `expect_err` — match explicitly on the Result shape.
    let result = PgVectorBackend::new(&test_config(), &url, "wrong-dim", &WrongDimProvider).await;
    let err = match result {
        Ok(_) => panic!("should refuse wrong-dim provider"),
        Err(e) => e,
    };
    assert!(err.contains("dim=1024"), "err missing migrated dim: {err}");
    assert!(err.contains("dim=768"), "err missing provider dim: {err}");
}

#[tokio::test]
async fn pgvector_migration_tracking_table_records_every_migration() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    let provider = DeterministicProvider;
    let _backend = PgVectorBackend::new(&test_config(), &url, "track", &provider)
        .await
        .expect("backend new");

    // Open a direct pool to inspect the tracking table.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("inspect pool");

    let applied = the_one_memory::pg_vector::migrations::list_applied(&pool)
        .await
        .expect("list_applied");

    assert_eq!(
        applied.len(),
        the_one_memory::pg_vector::migrations::embedded_count(),
        "every embedded migration should be recorded"
    );
    // Versions should be 0..N contiguous.
    for (i, m) in applied.iter().enumerate() {
        assert_eq!(m.version as usize, i, "migrations must be in order");
        assert!(
            !m.checksum.is_empty(),
            "every migration must have a checksum"
        );
    }
    pool.close().await;
}
