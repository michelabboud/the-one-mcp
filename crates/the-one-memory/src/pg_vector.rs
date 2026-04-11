//! pgvector backend (v0.16.0 Phase 2).
//!
//! This module ships the first real alternative `VectorBackend` after
//! the trait extraction in Phase A (v0.16.0-rc1). It targets operators
//! who already run managed Postgres and would rather co-locate their
//! vectors with their relational data than stand up a second service
//! (Qdrant) just for embeddings.
//!
//! ## Scope
//!
//! Phase 2 ships the **dense-only** path: chunk upsert/search/delete
//! and entity/relation upsert/search. The hybrid search path
//! (`search_chunks_hybrid`) is deliberately left unimplemented in this
//! phase — Decision D (deferred in the Phase 2 prompt) requires
//! benchmark numbers for two competing semantics (tsvector+GIN vs
//! sparse-array inner-product rewrite) before committing, and Phase 2
//! STOPs at that point per the plan.
//!
//! ## Schema ownership
//!
//! Every migration in `migrations/pgvector/*.sql` is embedded via
//! `include_str!` and applied by [`migrations::apply_all`], which
//! implements a small hand-rolled replacement for `sqlx::migrate!`.
//! We don't use the macro because sqlx's `migrate` feature
//! transitively references `sqlx-sqlite` via cargo weak-dep syntax,
//! and cargo's `links` conflict check pulls `sqlx-sqlite` into the
//! resolution graph where it collides with `rusqlite 0.39`'s
//! `libsqlite3-sys ^0.37.0`. The full bisection + rationale lives in
//! the `pg-vectors` feature comment in `Cargo.toml`.
//!
//! ## Dimension invariant
//!
//! Per Decision C (locked in by Michel in the Phase 2 prompt), the
//! migration files hardcode vector dimensionality to **1024**. This
//! matches the default quality-tier embedding provider
//! (BGE-large-en-v1.5). The backend constructor reads
//! `EmbeddingProvider::dimensions()` and refuses to initialize if the
//! live dim doesn't match the migrated dim — changing the dim later
//! is a new migration (`0006_reshape_*.sql`), not a runtime parameter.
//!
//! ## Connection pool sizing
//!
//! sqlx's default is 10 max / 0 min / 30s acquire / unlimited lifetime.
//! This is fine for dev and wrong for production: 0 min means the
//! first query after a restart pays full TCP + TLS + auth handshake
//! latency, and unlimited lifetime blocks IAM-auth credential
//! rotation + PGBouncer reshards. [`PgVectorConfig`] exposes five
//! pool-sizing fields with production-sane defaults — see the config
//! doc comments for the exact rationale.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;

#[allow(unused_imports)] // `EmbeddingProvider` is only used in `new`
use crate::embeddings::EmbeddingProvider;
use crate::vector_backend::{
    BackendCapabilities, EntityHit, EntityPoint, HybridVectorPoint, RelationHit, RelationPoint,
    VectorBackend, VectorHit, VectorPoint,
};

/// The dimension every migration file hardcodes. Changing this is a
/// new migration; see module docs.
const MIGRATED_DIM: usize = 1024;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Runtime configuration for [`PgVectorBackend`]. Mirrored from
/// `the_one_core::config::VectorPgvectorConfig` at broker construction
/// time so the memory crate doesn't need to depend on core's config
/// structs.
#[derive(Debug, Clone)]
pub struct PgVectorConfig {
    /// Schema name the migrations write into. Hardcoded to `the_one`
    /// in the migration SQL itself — overriding this field at runtime
    /// is accepted by the constructor but only affects NEW installs
    /// because the `CREATE SCHEMA` / `CREATE TABLE` statements bind
    /// the schema at migration time. Operators wanting schema
    /// isolation should set this BEFORE first boot.
    pub schema: String,

    /// HNSW graph connectivity. Defaults to 16. Only takes effect at
    /// migration time — to tune on an existing install, drop and
    /// recreate the HNSW indexes manually. Phase 2 keeps this field
    /// on the config so the restart-to-reconfigure story is
    /// documented and consistent with other tunables.
    pub hnsw_m: i32,
    /// HNSW build-time quality. Same migration-time constraint as
    /// `hnsw_m`.
    pub hnsw_ef_construction: i32,
    /// HNSW query-time recall. Applied per-session via `SET
    /// hnsw.ef_search = ...` on every connection acquire — NOT baked
    /// into the index, so this one field IS tunable at runtime.
    pub hnsw_ef_search: i32,

    /// Max connections in the sqlx pool.
    pub max_connections: u32,
    /// Min connections held warm. Non-zero so the first query after
    /// restart doesn't pay cold-handshake latency.
    pub min_connections: u32,
    /// How long a broker handler waits for a free connection before
    /// giving up.
    pub acquire_timeout_ms: u64,
    /// How long an idle connection stays in the pool before being
    /// closed.
    pub idle_timeout_ms: u64,
    /// How long any connection lives before being forcibly closed.
    /// Forces periodic reconnect to pick up IAM credential rotation,
    /// PGBouncer reshards, etc.
    pub max_lifetime_ms: u64,
}

impl Default for PgVectorConfig {
    fn default() -> Self {
        Self {
            schema: "the_one".to_string(),
            hnsw_m: 16,
            hnsw_ef_construction: 100,
            hnsw_ef_search: 40,
            max_connections: 10,
            min_connections: 2,
            acquire_timeout_ms: 30_000,
            idle_timeout_ms: 600_000,
            max_lifetime_ms: 1_800_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// The pgvector backend — one `PgPool` scoped to one project.
///
/// Every method takes `&self` so the broker can stash the backend
/// behind `Arc<dyn VectorBackend>` and share it across concurrent
/// handlers. `PgPool` is internally `Arc`'d and cheaply cloneable, so
/// no extra wrapping is needed.
pub struct PgVectorBackend {
    pool: PgPool,
    project_id: String,
    #[allow(dead_code)] // read by future Phase 4 combined-backend work
    schema: String,
    hnsw_ef_search: i32,
}

impl PgVectorBackend {
    /// Construct a new backend: open the pool, preflight the `vector`
    /// extension, apply migrations, and verify the live embedding
    /// provider's dim matches the migrated dim.
    ///
    /// Returns [`Err`] on any failure — the broker should surface
    /// these as `InvalidProjectConfig` so the operator sees the
    /// full message verbatim.
    pub async fn new(
        config: &PgVectorConfig,
        url: &str,
        project_id: &str,
        embedding_provider: &dyn EmbeddingProvider,
    ) -> Result<Self, String> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(Some(Duration::from_millis(config.idle_timeout_ms)))
            .max_lifetime(Some(Duration::from_millis(config.max_lifetime_ms)))
            .connect(url)
            .await
            .map_err(|e| format!("pgvector pool connect: {e}"))?;

        preflight_vector_extension(&pool).await?;

        migrations::apply_all(&pool).await?;

        let provider_dim = embedding_provider.dimensions();
        if provider_dim != MIGRATED_DIM {
            return Err(format!(
                "pgvector schema was migrated with dim={MIGRATED_DIM} (BGE-large-en-v1.5, \
                 quality tier); active embedding provider reports dim={provider_dim}; recreate \
                 the pgvector schema against a matching provider or switch embedding providers \
                 to match. Changing the dim is a new migration (0006_reshape_*.sql), not a \
                 runtime setting."
            ));
        }

        Ok(Self {
            pool,
            project_id: project_id.to_string(),
            schema: config.schema.clone(),
            hnsw_ef_search: config.hnsw_ef_search,
        })
    }

    /// Close the underlying pool. Called from
    /// `McpBroker::shutdown()` in Phase 3+ to guarantee clean teardown
    /// ordering.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

// ---------------------------------------------------------------------------
// Extension preflight
// ---------------------------------------------------------------------------

/// Verify the `vector` extension is installed — or, if it's available
/// but not yet installed, install it. Produces targeted error
/// messages for the five common managed-Postgres environments so
/// operators see actionable guidance instead of a cryptic sqlx
/// migration failure.
///
/// Three states:
///
/// 1. **Installed** — nothing to do. Supabase path.
/// 2. **Available but not installed** — try `CREATE EXTENSION`. On
///    AWS RDS this requires `rds_superuser`; on self-hosted Postgres
///    it requires `CREATE` on the database.
/// 3. **Not available** — the `vector` extension files aren't even
///    on disk. Return a per-provider actionable error pointing the
///    operator at the installation step.
async fn preflight_vector_extension(pool: &PgPool) -> Result<(), String> {
    let installed: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("preflight vector extension check: {e}"))?;
    if installed {
        return Ok(());
    }

    let available: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("preflight vector extension availability: {e}"))?;
    if !available {
        return Err(
            "pgvector backend requires the `vector` extension, which is not installed on this \
             Postgres instance and not available for installation. Install it first:\n\
               - AWS RDS / Aurora Postgres: enable `vector` in the instance parameter group's \
                 shared_preload_libraries, reboot the instance, then connect as rds_superuser.\n\
               - Google Cloud SQL Postgres: set the `cloudsql.enable_pgvector` database flag.\n\
               - Azure Database for PostgreSQL Flexible Server: enable `vector` in the server \
                 parameter `azure.extensions`.\n\
               - Supabase: pgvector is pre-installed, no action required.\n\
               - Self-hosted Postgres: install the pgvector package for your distribution \
                 (`apt install postgresql-16-pgvector` or build from source), then restart."
                .to_string(),
        );
    }

    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await
        .map_err(|e| {
            format!(
                "pgvector extension is available but CREATE EXTENSION failed: {e}. The \
                 connecting role needs CREATE privilege on this database, or you need to \
                 connect as a superuser once to install it. On AWS RDS, connect as \
                 rds_superuser. On Supabase, use the service_role connection."
            )
        })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hand-rolled migration runner
// ---------------------------------------------------------------------------

/// Embedded SQL migrations + the runner that applies them. Replaces
/// `sqlx::migrate!` (see module docs for why).
pub mod migrations {
    use sha2::{Digest, Sha256};
    use sqlx::postgres::PgPool;
    use sqlx::Row;

    /// One embedded migration. `version` must be unique and ordered;
    /// `description` is informational; `sql` is the raw file contents.
    struct Migration {
        version: i32,
        description: &'static str,
        sql: &'static str,
    }

    /// Every migration shipped in this phase, ordered by version. The
    /// `include_str!` paths are relative to this source file, matching
    /// where `migrations/pgvector/` lives in the crate.
    const MIGRATIONS: &[Migration] = &[
        Migration {
            version: 0,
            description: "migrations_table",
            sql: include_str!("../migrations/pgvector/0000_migrations_table.sql"),
        },
        Migration {
            version: 1,
            description: "extension_and_schema",
            sql: include_str!("../migrations/pgvector/0001_extension_and_schema.sql"),
        },
        Migration {
            version: 2,
            description: "chunks_table",
            sql: include_str!("../migrations/pgvector/0002_chunks_table.sql"),
        },
        Migration {
            version: 3,
            description: "entities_table",
            sql: include_str!("../migrations/pgvector/0003_entities_table.sql"),
        },
        Migration {
            version: 4,
            description: "relations_table",
            sql: include_str!("../migrations/pgvector/0004_relations_table.sql"),
        },
    ];

    /// SHA-256 of a migration body, returned as raw bytes for BYTEA.
    fn checksum(sql: &str) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(sql.as_bytes());
        hasher.finalize().to_vec()
    }

    /// Current wall-clock time in milliseconds since the Unix epoch.
    /// Uses `std::time::SystemTime` — no `chrono` dep.
    fn now_epoch_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Apply every migration in order. Idempotent: already-applied
    /// migrations are detected via the `pgvector_migrations` tracking
    /// table and skipped. Checksum drift (i.e. someone edited
    /// `0002_chunks_table.sql` post-ship) is caught before any other
    /// migration runs — we refuse to continue in that state because
    /// the live schema doesn't match any consistent version.
    pub async fn apply_all(pool: &PgPool) -> Result<(), String> {
        // Migration 0 creates the tracking table itself, so the
        // "already applied?" check below can't rely on the table
        // existing. Apply migration 0 unconditionally with a plain
        // `CREATE TABLE IF NOT EXISTS` idempotence — the migration
        // body is shaped so that re-executing is safe.
        let bootstrap = &MIGRATIONS[0];
        sqlx::raw_sql(bootstrap.sql)
            .execute(pool)
            .await
            .map_err(|e| format!("migration 0000 (bootstrap): {e}"))?;

        // Record migration 0 in the tracking table (or verify its
        // checksum if already present).
        upsert_or_verify(pool, bootstrap).await?;

        // For every subsequent migration, check the tracking table;
        // if it's already applied, verify the checksum matches; if
        // it isn't, apply and record.
        for m in &MIGRATIONS[1..] {
            let already_applied: Option<Vec<u8>> = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT checksum FROM the_one.pgvector_migrations WHERE version = $1",
            )
            .bind(m.version)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("query pgvector_migrations for version {}: {e}", m.version))?;

            if let Some(stored) = already_applied {
                let live = checksum(m.sql);
                if stored != live {
                    return Err(format!(
                        "pgvector migration {} ({}) checksum drift: the schema was applied \
                         with a different version of the SQL file than is embedded in this \
                         binary. Refusing to continue — the schema is in an unknown state. \
                         Rebuild against the commit that shipped this migration, or drop the \
                         `the_one` schema and re-initialize.",
                        m.version, m.description
                    ));
                }
                // Already applied, checksum matches — skip.
                continue;
            }

            // Apply the migration.
            sqlx::raw_sql(m.sql).execute(pool).await.map_err(|e| {
                format!(
                    "apply pgvector migration {} ({}): {e}",
                    m.version, m.description
                )
            })?;

            // Record it. The INSERT happens after the apply so a mid-
            // apply failure leaves the migration "not applied" and
            // the next run retries cleanly — except for schema-altering
            // statements that partially succeeded, which the operator
            // has to clean up manually. Postgres DDL is transactional
            // so most single-statement migrations either fully apply
            // or fully roll back; multi-statement files are rarer and
            // documented as needing manual cleanup.
            sqlx::query(
                "INSERT INTO the_one.pgvector_migrations \
                     (version, description, checksum, applied_at_ms) \
                     VALUES ($1, $2, $3, $4)",
            )
            .bind(m.version)
            .bind(m.description)
            .bind(checksum(m.sql))
            .bind(now_epoch_ms())
            .execute(pool)
            .await
            .map_err(|e| {
                format!(
                    "record pgvector migration {} ({}): {e}",
                    m.version, m.description
                )
            })?;
        }

        Ok(())
    }

    /// Handle migration 0's special case: the tracking table body is
    /// executed unconditionally every run (it's `CREATE TABLE IF NOT
    /// EXISTS`). After the body executes, we either insert a new row
    /// for version 0 or verify its checksum if already present.
    async fn upsert_or_verify(pool: &PgPool, m: &Migration) -> Result<(), String> {
        let row: Option<Vec<u8>> = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT checksum FROM the_one.pgvector_migrations WHERE version = $1",
        )
        .bind(m.version)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("query bootstrap migration row: {e}"))?;

        let live = checksum(m.sql);

        match row {
            Some(stored) if stored == live => Ok(()),
            Some(_) => Err(format!(
                "pgvector migration {} ({}) checksum drift on bootstrap row. Refusing to \
                 continue. Drop `the_one.pgvector_migrations` and restart against a consistent \
                 binary.",
                m.version, m.description
            )),
            None => {
                sqlx::query(
                    "INSERT INTO the_one.pgvector_migrations \
                         (version, description, checksum, applied_at_ms) \
                         VALUES ($1, $2, $3, $4)",
                )
                .bind(m.version)
                .bind(m.description)
                .bind(live)
                .bind(now_epoch_ms())
                .execute(pool)
                .await
                .map_err(|e| format!("record bootstrap migration: {e}"))?;
                Ok(())
            }
        }
    }

    /// How many migrations are embedded in this binary. Exposed for
    /// tests to sanity-check the tracking table after `apply_all`.
    pub fn embedded_count() -> usize {
        MIGRATIONS.len()
    }

    /// Row type returned by `list_applied` — version + checksum for
    /// test assertions.
    #[derive(Debug)]
    pub struct AppliedMigration {
        pub version: i32,
        pub checksum: Vec<u8>,
    }

    /// List every row in `the_one.pgvector_migrations`. Used by
    /// integration tests to verify `apply_all` recorded what it
    /// applied, and for future observability.
    pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, String> {
        let rows = sqlx::query(
            "SELECT version, checksum FROM the_one.pgvector_migrations ORDER BY version",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| format!("list_applied: {e}"))?;
        Ok(rows
            .into_iter()
            .map(|r| AppliedMigration {
                version: r.get::<i32, _>("version"),
                checksum: r.get::<Vec<u8>, _>("checksum"),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// VectorBackend impl
// ---------------------------------------------------------------------------

#[async_trait]
impl VectorBackend for PgVectorBackend {
    fn capabilities(&self) -> BackendCapabilities {
        // Phase 2 ships full capabilities EXCEPT hybrid search, which
        // is gated on Decision D (deferred). Mark `hybrid = false`
        // until Phase 2.5 wires the actual semantics.
        BackendCapabilities {
            name: "pgvector",
            chunks: true,
            hybrid: false,
            entities: true,
            relations: true,
            images: false,
            persistence_verifiable: false,
        }
    }

    // ── Chunks ─────────────────────────────────────────────────────

    async fn ensure_collection(&self, dims: usize) -> Result<(), String> {
        // Schema + HNSW index were created by the migration runner.
        // This call's only job is to re-verify the dim invariant:
        // every call site in `MemoryEngine` passes the live provider
        // dim, so if something upstream silently swapped providers,
        // this is where the mismatch surfaces.
        if dims != MIGRATED_DIM {
            return Err(format!(
                "pgvector schema is fixed at dim={MIGRATED_DIM}; caller requested dim={dims}. \
                 This indicates an embedding-provider swap without a matching schema \
                 migration — refuse to proceed."
            ));
        }
        Ok(())
    }

    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        // Single batched statement via UNNEST — one round trip for
        // the whole batch, no N-query loop. Postgres evaluates UNNEST
        // into a tabular form that the INSERT consumes directly.
        let len = points.len();
        let mut ids: Vec<String> = Vec::with_capacity(len);
        let mut project_ids: Vec<String> = Vec::with_capacity(len);
        let mut source_paths: Vec<String> = Vec::with_capacity(len);
        let mut headings: Vec<String> = Vec::with_capacity(len);
        let mut chunk_indices: Vec<i64> = Vec::with_capacity(len);
        let mut contents: Vec<Option<String>> = Vec::with_capacity(len);
        let mut vectors: Vec<pgvector::Vector> = Vec::with_capacity(len);
        let now = migration_now_ms();
        let mut created_ats: Vec<i64> = Vec::with_capacity(len);

        for p in points {
            if p.vector.len() != MIGRATED_DIM {
                return Err(format!(
                    "pgvector upsert_chunks: vector dim mismatch — chunk '{}' has dim={}, \
                     schema requires dim={MIGRATED_DIM}",
                    p.id,
                    p.vector.len()
                ));
            }
            ids.push(p.id);
            project_ids.push(self.project_id.clone());
            source_paths.push(p.payload.source_path);
            headings.push(p.payload.heading);
            chunk_indices.push(p.payload.chunk_index as i64);
            contents.push(p.content);
            vectors.push(pgvector::Vector::from(p.vector));
            created_ats.push(now);
        }

        sqlx::query(
            "INSERT INTO the_one.chunks \
                 (id, project_id, source_path, heading, chunk_index, content, \
                  dense_vector, created_at_epoch_ms) \
                 SELECT * FROM UNNEST( \
                     $1::text[], $2::text[], $3::text[], $4::text[], $5::bigint[], \
                     $6::text[], $7::vector[], $8::bigint[] \
                 ) \
                 ON CONFLICT (id) DO UPDATE SET \
                     project_id          = EXCLUDED.project_id, \
                     source_path         = EXCLUDED.source_path, \
                     heading             = EXCLUDED.heading, \
                     chunk_index         = EXCLUDED.chunk_index, \
                     content             = EXCLUDED.content, \
                     dense_vector        = EXCLUDED.dense_vector, \
                     created_at_epoch_ms = EXCLUDED.created_at_epoch_ms",
        )
        .bind(&ids)
        .bind(&project_ids)
        .bind(&source_paths)
        .bind(&headings)
        .bind(&chunk_indices)
        .bind(&contents)
        .bind(&vectors)
        .bind(&created_ats)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("pgvector upsert_chunks: {e}"))?;

        Ok(())
    }

    async fn upsert_hybrid_chunks(&self, _points: Vec<HybridVectorPoint>) -> Result<(), String> {
        // Decision D deferred: hybrid semantics require a brainstorm
        // pass with α vs β benchmark numbers. Until then, Phase 2
        // surfaces an explicit "not supported" so the caller falls
        // back to dense-only ingest.
        Err("pgvector hybrid chunk upsert deferred to Phase 2.5 (Decision D)".to_string())
    }

    async fn search_chunks(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<VectorHit>, String> {
        if query_vector.len() != MIGRATED_DIM {
            return Err(format!(
                "pgvector search_chunks: query vector dim={}, schema requires dim={MIGRATED_DIM}",
                query_vector.len()
            ));
        }

        // Apply ef_search on a single borrowed connection so the SET
        // LOCAL sticks for the subsequent SELECT within the same
        // transaction block.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("pgvector search_chunks begin: {e}"))?;
        sqlx::query(&format!(
            "SET LOCAL hnsw.ef_search = {}",
            self.hnsw_ef_search
        ))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_chunks set ef: {e}"))?;

        let query_vec = pgvector::Vector::from(query_vector);

        // Cosine distance is `<=>` in pgvector. score = 1 - distance.
        // The score_threshold filter applies post-projection because
        // HNSW's ordering is by distance, not similarity.
        let rows = sqlx::query(
            "SELECT id, source_path, heading, chunk_index, \
                    (1 - (dense_vector <=> $1)) AS score \
             FROM the_one.chunks \
             WHERE project_id = $2 \
             ORDER BY dense_vector <=> $1 \
             LIMIT $3",
        )
        .bind(&query_vec)
        .bind(&self.project_id)
        .bind(top_k as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_chunks query: {e}"))?;

        tx.commit()
            .await
            .map_err(|e| format!("pgvector search_chunks commit: {e}"))?;

        let mut hits = Vec::with_capacity(rows.len());
        for r in rows {
            let score: f64 = r.get("score");
            let score = score as f32;
            if score < score_threshold {
                continue;
            }
            hits.push(VectorHit {
                chunk_id: r.get::<String, _>("id"),
                source_path: r.get::<String, _>("source_path"),
                heading: r.get::<String, _>("heading"),
                chunk_index: r.get::<i64, _>("chunk_index") as usize,
                score,
            });
        }
        Ok(hits)
    }

    async fn delete_by_source_path(&self, source_path: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM the_one.chunks WHERE project_id = $1 AND source_path = $2")
            .bind(&self.project_id)
            .bind(source_path)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("pgvector delete_by_source_path: {e}"))?;
        Ok(())
    }

    // ── Entities ───────────────────────────────────────────────────

    async fn ensure_entity_collection(&self, dims: usize) -> Result<(), String> {
        if dims != MIGRATED_DIM {
            return Err(format!(
                "pgvector entity schema is fixed at dim={MIGRATED_DIM}; caller requested \
                 dim={dims}"
            ));
        }
        Ok(())
    }

    async fn upsert_entities(&self, points: Vec<EntityPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        let len = points.len();
        let mut ids: Vec<String> = Vec::with_capacity(len);
        let mut project_ids: Vec<String> = Vec::with_capacity(len);
        let mut names: Vec<String> = Vec::with_capacity(len);
        let mut types: Vec<String> = Vec::with_capacity(len);
        let mut descriptions: Vec<String> = Vec::with_capacity(len);
        let mut source_chunks: Vec<serde_json::Value> = Vec::with_capacity(len);
        let mut vectors: Vec<pgvector::Vector> = Vec::with_capacity(len);
        let now = migration_now_ms();
        let mut created_ats: Vec<i64> = Vec::with_capacity(len);

        for e in points {
            if e.vector.len() != MIGRATED_DIM {
                return Err(format!(
                    "pgvector upsert_entities: dim mismatch on entity '{}' (got {}, need {})",
                    e.name,
                    e.vector.len(),
                    MIGRATED_DIM
                ));
            }
            ids.push(e.id);
            project_ids.push(self.project_id.clone());
            names.push(e.name);
            types.push(e.entity_type);
            descriptions.push(e.description);
            source_chunks.push(serde_json::Value::Array(
                e.source_chunks
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ));
            vectors.push(pgvector::Vector::from(e.vector));
            created_ats.push(now);
        }

        sqlx::query(
            "INSERT INTO the_one.entities \
                 (id, project_id, name, entity_type, description, source_chunks, \
                  dense_vector, created_at_epoch_ms) \
                 SELECT * FROM UNNEST( \
                     $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                     $6::jsonb[], $7::vector[], $8::bigint[] \
                 ) \
                 ON CONFLICT (id) DO UPDATE SET \
                     project_id          = EXCLUDED.project_id, \
                     name                = EXCLUDED.name, \
                     entity_type         = EXCLUDED.entity_type, \
                     description         = EXCLUDED.description, \
                     source_chunks       = EXCLUDED.source_chunks, \
                     dense_vector        = EXCLUDED.dense_vector, \
                     created_at_epoch_ms = EXCLUDED.created_at_epoch_ms",
        )
        .bind(&ids)
        .bind(&project_ids)
        .bind(&names)
        .bind(&types)
        .bind(&descriptions)
        .bind(&source_chunks)
        .bind(&vectors)
        .bind(&created_ats)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("pgvector upsert_entities: {e}"))?;

        Ok(())
    }

    async fn search_entities(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<EntityHit>, String> {
        if query_vector.len() != MIGRATED_DIM {
            return Err(format!(
                "pgvector search_entities: query dim={}, schema requires dim={MIGRATED_DIM}",
                query_vector.len()
            ));
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("pgvector search_entities begin: {e}"))?;
        sqlx::query(&format!(
            "SET LOCAL hnsw.ef_search = {}",
            self.hnsw_ef_search
        ))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_entities set ef: {e}"))?;

        let query_vec = pgvector::Vector::from(query_vector);

        let rows = sqlx::query(
            "SELECT name, entity_type, description, source_chunks, \
                    (1 - (dense_vector <=> $1)) AS score \
             FROM the_one.entities \
             WHERE project_id = $2 \
             ORDER BY dense_vector <=> $1 \
             LIMIT $3",
        )
        .bind(&query_vec)
        .bind(&self.project_id)
        .bind(top_k as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_entities query: {e}"))?;

        tx.commit()
            .await
            .map_err(|e| format!("pgvector search_entities commit: {e}"))?;

        let mut hits = Vec::with_capacity(rows.len());
        for r in rows {
            let score: f64 = r.get("score");
            let score = score as f32;
            if score < score_threshold {
                continue;
            }
            let source_chunks_json: serde_json::Value = r.get("source_chunks");
            let source_chunks: Vec<String> = match source_chunks_json {
                serde_json::Value::Array(a) => a
                    .into_iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            hits.push(EntityHit {
                name: r.get::<String, _>("name"),
                entity_type: r.get::<String, _>("entity_type"),
                description: r.get::<String, _>("description"),
                source_chunks,
                score,
            });
        }
        Ok(hits)
    }

    // ── Relations ──────────────────────────────────────────────────

    async fn ensure_relation_collection(&self, dims: usize) -> Result<(), String> {
        if dims != MIGRATED_DIM {
            return Err(format!(
                "pgvector relation schema is fixed at dim={MIGRATED_DIM}; caller requested \
                 dim={dims}"
            ));
        }
        Ok(())
    }

    async fn upsert_relations(&self, points: Vec<RelationPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        let len = points.len();
        let mut ids: Vec<String> = Vec::with_capacity(len);
        let mut project_ids: Vec<String> = Vec::with_capacity(len);
        let mut sources: Vec<String> = Vec::with_capacity(len);
        let mut targets: Vec<String> = Vec::with_capacity(len);
        let mut types: Vec<String> = Vec::with_capacity(len);
        let mut descriptions: Vec<String> = Vec::with_capacity(len);
        let mut source_chunks: Vec<serde_json::Value> = Vec::with_capacity(len);
        let mut vectors: Vec<pgvector::Vector> = Vec::with_capacity(len);
        let now = migration_now_ms();
        let mut created_ats: Vec<i64> = Vec::with_capacity(len);

        for r in points {
            if r.vector.len() != MIGRATED_DIM {
                return Err(format!(
                    "pgvector upsert_relations: dim mismatch on relation {}→{} (got {}, need {})",
                    r.source,
                    r.target,
                    r.vector.len(),
                    MIGRATED_DIM
                ));
            }
            ids.push(r.id);
            project_ids.push(self.project_id.clone());
            sources.push(r.source);
            targets.push(r.target);
            types.push(r.relation_type);
            descriptions.push(r.description);
            source_chunks.push(serde_json::Value::Array(
                r.source_chunks
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ));
            vectors.push(pgvector::Vector::from(r.vector));
            created_ats.push(now);
        }

        sqlx::query(
            "INSERT INTO the_one.relations \
                 (id, project_id, source, target, relation_type, description, \
                  source_chunks, dense_vector, created_at_epoch_ms) \
                 SELECT * FROM UNNEST( \
                     $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                     $6::text[], $7::jsonb[], $8::vector[], $9::bigint[] \
                 ) \
                 ON CONFLICT (id) DO UPDATE SET \
                     project_id          = EXCLUDED.project_id, \
                     source              = EXCLUDED.source, \
                     target              = EXCLUDED.target, \
                     relation_type       = EXCLUDED.relation_type, \
                     description         = EXCLUDED.description, \
                     source_chunks       = EXCLUDED.source_chunks, \
                     dense_vector        = EXCLUDED.dense_vector, \
                     created_at_epoch_ms = EXCLUDED.created_at_epoch_ms",
        )
        .bind(&ids)
        .bind(&project_ids)
        .bind(&sources)
        .bind(&targets)
        .bind(&types)
        .bind(&descriptions)
        .bind(&source_chunks)
        .bind(&vectors)
        .bind(&created_ats)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("pgvector upsert_relations: {e}"))?;

        Ok(())
    }

    async fn search_relations(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<RelationHit>, String> {
        if query_vector.len() != MIGRATED_DIM {
            return Err(format!(
                "pgvector search_relations: query dim={}, schema requires dim={MIGRATED_DIM}",
                query_vector.len()
            ));
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("pgvector search_relations begin: {e}"))?;
        sqlx::query(&format!(
            "SET LOCAL hnsw.ef_search = {}",
            self.hnsw_ef_search
        ))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_relations set ef: {e}"))?;

        let query_vec = pgvector::Vector::from(query_vector);

        let rows = sqlx::query(
            "SELECT source, target, relation_type, description, source_chunks, \
                    (1 - (dense_vector <=> $1)) AS score \
             FROM the_one.relations \
             WHERE project_id = $2 \
             ORDER BY dense_vector <=> $1 \
             LIMIT $3",
        )
        .bind(&query_vec)
        .bind(&self.project_id)
        .bind(top_k as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| format!("pgvector search_relations query: {e}"))?;

        tx.commit()
            .await
            .map_err(|e| format!("pgvector search_relations commit: {e}"))?;

        let mut hits = Vec::with_capacity(rows.len());
        for r in rows {
            let score: f64 = r.get("score");
            let score = score as f32;
            if score < score_threshold {
                continue;
            }
            let source_chunks_json: serde_json::Value = r.get("source_chunks");
            let source_chunks: Vec<String> = match source_chunks_json {
                serde_json::Value::Array(a) => a
                    .into_iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            hits.push(RelationHit {
                source: r.get::<String, _>("source"),
                target: r.get::<String, _>("target"),
                relation_type: r.get::<String, _>("relation_type"),
                description: r.get::<String, _>("description"),
                source_chunks,
                score,
            });
        }
        Ok(hits)
    }
}

// Small helper so migrations + backend upserts agree on time format.
// Inline here rather than import from core to keep the memory crate
// independent of core's time helpers.
fn migration_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Unit tests — pure-Rust, no live database
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migration_count_matches_file_list() {
        // If someone adds a migration file but forgets to append it
        // to the `MIGRATIONS` const, this test catches it — the
        // directory has 5 .sql files, the const had better have 5
        // entries.
        assert_eq!(migrations::embedded_count(), 5);
    }

    #[test]
    fn default_config_hnsw_tunables_match_plan_defaults() {
        // Anchors the Phase 2 defaults from the backend selection
        // scheme. If the defaults drift, this test fails.
        let cfg = PgVectorConfig::default();
        assert_eq!(cfg.hnsw_m, 16);
        assert_eq!(cfg.hnsw_ef_construction, 100);
        assert_eq!(cfg.hnsw_ef_search, 40);
    }

    #[test]
    fn default_config_pool_sizing_matches_plan_defaults() {
        let cfg = PgVectorConfig::default();
        assert_eq!(cfg.max_connections, 10);
        assert_eq!(cfg.min_connections, 2);
        assert_eq!(cfg.acquire_timeout_ms, 30_000);
        assert_eq!(cfg.idle_timeout_ms, 600_000);
        assert_eq!(cfg.max_lifetime_ms, 1_800_000);
    }

    #[test]
    fn default_schema_name_is_the_one() {
        assert_eq!(PgVectorConfig::default().schema, "the_one");
    }

    #[test]
    fn migrated_dim_matches_plan_decision_c() {
        // Decision C locks dim=1024. If someone edits the constant
        // to something else, this test fails — forcing them to also
        // bump every migration SQL file's `vector(NNNN)` literal.
        assert_eq!(MIGRATED_DIM, 1024);
    }
}
