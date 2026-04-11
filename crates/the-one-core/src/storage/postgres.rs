//! PostgresStateStore backend (v0.16.0 Phase 3).
//!
//! This module ships the Postgres implementation of
//! [`crate::state_store::StateStore`]. It's the second half of the
//! multi-backend roadmap's Phase B: Phase 2 landed pgvector for the
//! vector axis, Phase 3 lands Postgres for the state axis. Together
//! they let an operator run the-one-mcp on managed Postgres with
//! zero other infrastructure — and Phase 4 will glue them into a
//! single connection pool for transactional consistency.
//!
//! ## Design surface
//!
//! The `StateStore` trait is deliberately **sync** (no async methods)
//! because `rusqlite::Connection` is `Send + !Sync` and the original
//! SQLite impl carried that constraint forward. The broker's Phase 1
//! `with_state_store` chokepoint passes `&dyn StateStore` to a
//! **sync closure** so the compiler refuses to hold a backend guard
//! across an `.await` — that restriction is the anti-deadlock
//! guarantee for pooled backends.
//!
//! sqlx, meanwhile, is async top-to-bottom. So every method in this
//! file bridges sync → async via [`block_on`], which is
//! [`tokio::task::block_in_place`] + [`tokio::runtime::Handle::current`]'s
//! `block_on`. This is the canonical pattern for running async work
//! from a sync callsite that is itself inside a tokio runtime; it
//! requires the **multi-threaded** runtime flavor (so other tasks
//! can migrate off the blocked worker). The binary crate's
//! `#[tokio::main]` default satisfies this.
//!
//! ## Migrations
//!
//! Hand-rolled, same pattern as `the_one_memory::pg_vector::migrations`.
//! We can't use `sqlx::migrate!` because sqlx's `migrate` feature
//! transitively references `sqlx-sqlite?/...` weak-deps that cargo's
//! `links` conflict check pulls into the resolution graph, where they
//! collide with `rusqlite 0.39`'s `libsqlite3-sys ^0.37.0`. The
//! bisection + full rationale lives in
//! `crates/the-one-memory/Cargo.toml`'s `pg-vectors` feature comment.
//!
//! The tracking table is `the_one.state_migrations`, **distinct** from
//! the pgvector module's `the_one.pgvector_migrations` so Phase 4's
//! combined deployment can share one schema without the two runners
//! stepping on each other's versions.
//!
//! ## FTS translation
//!
//! SQLite's FTS5 virtual table becomes a `TSVECTOR` column on
//! `diary_entries` + a GIN index. Search queries use
//! `@@ websearch_to_tsquery('simple', $1)` ordered by `ts_rank`.
//! `'simple'` (not `'english'`) matches FTS5's default tokenization
//! behaviour most closely and works uniformly across languages —
//! `'english'` would stem "running" → "run" and break exact-word
//! searches on English diary content.
//!
//! A LIKE fallback runs on `websearch_to_tsquery` parse errors so
//! a user typing `!@#$` doesn't get a bare `invalid_request` —
//! same shape as the SQLite side's FTS5-error fallback.
//!
//! ## Schema version
//!
//! Postgres ships the v7 shape in one migration (`0001_state_schema_v7.sql`)
//! because a fresh deployment has no v1..v6 history. The
//! `schema_version()` trait method returns `1` (the migration
//! version), NOT `7` (the SQLite-native schema version). These
//! numbers are per-backend — cross-backend parity is reported via
//! [`StateStoreCapabilities::schema_versioned`] + per-backend
//! version spaces.

use std::time::Duration;

use serde_json::Value as JsonValue;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::Row;

use crate::audit::AuditRecord;
use crate::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationNodeKind,
    MemoryNavigationTunnel,
};
use crate::error::CoreError;
use crate::pagination::{Page, PageRequest};
use crate::state_store::{StateStore, StateStoreCapabilities};
use crate::storage::sqlite::{page_limits, AuditEvent, ConversationSourceRecord};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Runtime configuration for [`PostgresStateStore`]. Parallel to
/// `the_one_memory::pg_vector::PgVectorConfig` so the broker can
/// mirror fields from `the_one_core::config::StatePostgresConfig`
/// without either crate depending on the other's config surface.
#[derive(Debug, Clone)]
pub struct PostgresStateConfig {
    /// Schema name the migrations write into. Hardcoded to `the_one`
    /// in the migration SQL itself; overriding this field only
    /// affects NEW installs. Operators wanting schema isolation
    /// should set this BEFORE first boot.
    pub schema: String,

    /// Postgres `statement_timeout` in milliseconds. Applied at
    /// connection time via `SET statement_timeout = '<N>ms'` on every
    /// freshly-checked-out pool connection. `0` disables the timeout
    /// entirely (Postgres default).
    pub statement_timeout_ms: u64,

    // ── sqlx pool sizing — same defaults as PgVectorConfig ──────────
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_lifetime_ms: u64,
}

impl Default for PostgresStateConfig {
    fn default() -> Self {
        Self {
            schema: "the_one".to_string(),
            statement_timeout_ms: 30_000,
            max_connections: 10,
            min_connections: 2,
            acquire_timeout_ms: 30_000,
            idle_timeout_ms: 600_000,
            max_lifetime_ms: 1_800_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Sync → async bridge
// ---------------------------------------------------------------------------

/// Run an async block synchronously from inside a tokio runtime
/// worker. See the module docs for the rationale — short version:
/// `tokio::task::block_in_place` tells the runtime "this worker is
/// about to do blocking work," other tasks migrate off, the
/// `Handle::current().block_on` drives the future to completion,
/// and the worker resumes async duty afterward.
///
/// **Required runtime flavor:** multi-threaded. The broker binary's
/// `#[tokio::main]` satisfies this by default; tests must also run
/// on `#[tokio::test(flavor = "multi_thread")]`.
///
/// Panics with a clear message if called outside a tokio runtime;
/// that's a programming error (someone called a StateStore method
/// from a non-async context).
fn block_on<F, R>(fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("PostgresStateStore methods must be called from a tokio runtime")
            .block_on(fut)
    })
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Postgres-backed [`StateStore`] implementation.
///
/// Holds a `sqlx::PgPool` scoped to one project. Internally the pool
/// is `Arc`-reference-counted and cheaply cloneable, so the broker's
/// per-project cache (`state_by_project`) owns the backend exclusively
/// and the handler closures borrow through `with_state_store`.
pub struct PostgresStateStore {
    pool: PgPool,
    project_id: String,
    #[allow(dead_code)] // read by Phase 4 combined-backend adapter
    schema: String,
}

impl std::fmt::Debug for PostgresStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresStateStore")
            .field("project_id", &self.project_id)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl PostgresStateStore {
    /// Open the pool, run migrations, return a ready-to-use backend.
    ///
    /// This is async (unlike SQLite's sync `open`) because sqlx is
    /// async end-to-end. The broker's `state_store_factory` handles
    /// the bridging via [`block_on`] at construction time.
    pub async fn new(
        config: &PostgresStateConfig,
        url: &str,
        project_id: &str,
    ) -> Result<Self, CoreError> {
        let mut pool_options = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(Some(Duration::from_millis(config.idle_timeout_ms)))
            .max_lifetime(Some(Duration::from_millis(config.max_lifetime_ms)));

        // Apply statement_timeout on every freshly-checked-out
        // connection. Non-zero values translate to a `SET` at session
        // start; zero disables the timeout entirely (Postgres default).
        // Using `after_connect` instead of `connect_lazy_with` because
        // we want the pool to verify connectivity at `new()` time so
        // the broker fails loud on boot against a dead database.
        if config.statement_timeout_ms > 0 {
            let timeout_ms = config.statement_timeout_ms;
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
            .map_err(|e| CoreError::Postgres(format!("postgres state pool connect: {e}")))?;

        // Apply schema migrations. The hand-rolled runner is
        // idempotent — re-running against an already-migrated
        // database verifies checksums and exits cleanly.
        migrations::apply_all(&pool).await?;

        Ok(Self {
            pool,
            project_id: project_id.to_string(),
            schema: config.schema.clone(),
        })
    }

    /// Close the pool. Called from `McpBroker::shutdown()` in
    /// Phase 4+ when the state trait gains a shutdown method; until
    /// then the pool drops implicitly when the broker's cache
    /// entries are drained.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

// ---------------------------------------------------------------------------
// Migration runner
// ---------------------------------------------------------------------------

/// Embedded SQL migrations + hand-rolled runner. Mirrors the pattern
/// in `the_one_memory::pg_vector::migrations` (see that module for
/// the full rationale on why we don't use `sqlx::migrate!`).
pub mod migrations {
    use sha2::{Digest, Sha256};
    use sqlx::postgres::PgPool;
    use sqlx::Row;

    use crate::error::CoreError;

    /// One embedded migration. Must be appended to [`MIGRATIONS`] in
    /// version order.
    pub struct Migration {
        pub version: i32,
        pub description: &'static str,
        pub sql: &'static str,
    }

    /// Every Phase 3 migration shipped in this binary. Order is
    /// load-bearing: migration N's SQL may reference objects created
    /// by migration N-1.
    ///
    /// Adding a new migration later: append to this slice with a
    /// monotonically-greater `version`. The runner will detect the
    /// gap on next boot and apply it.
    const MIGRATIONS: &[Migration] = &[
        Migration {
            version: 0,
            description: "state_migrations_table",
            sql: include_str!("../../migrations/postgres-state/0000_state_migrations_table.sql"),
        },
        Migration {
            version: 1,
            description: "state_schema_v7",
            sql: include_str!("../../migrations/postgres-state/0001_state_schema_v7.sql"),
        },
    ];

    fn checksum(sql: &str) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(sql.as_bytes());
        hasher.finalize().to_vec()
    }

    fn now_epoch_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Apply every embedded migration in order. Idempotent.
    pub async fn apply_all(pool: &PgPool) -> Result<(), CoreError> {
        // Migration 0 creates the tracking table itself, so we apply
        // it unconditionally — the body is `CREATE TABLE IF NOT EXISTS`
        // so re-running is safe.
        let bootstrap = &MIGRATIONS[0];
        sqlx::raw_sql(bootstrap.sql)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Postgres(format!("migration 0000: {e}")))?;
        upsert_or_verify(pool, bootstrap).await?;

        for m in &MIGRATIONS[1..] {
            let already_applied: Option<Vec<u8>> = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT checksum FROM the_one.state_migrations WHERE version = $1",
            )
            .bind(m.version)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                CoreError::Postgres(format!(
                    "query state_migrations for version {}: {e}",
                    m.version
                ))
            })?;

            if let Some(stored) = already_applied {
                let live = checksum(m.sql);
                if stored != live {
                    return Err(CoreError::Postgres(format!(
                        "postgres state migration {} ({}) checksum drift: the schema was \
                         applied with a different version of the SQL file than is embedded \
                         in this binary. Refusing to continue — schema is in an unknown \
                         state. Rebuild against the commit that shipped this migration, or \
                         drop the `the_one` schema and re-initialize.",
                        m.version, m.description
                    )));
                }
                continue;
            }

            sqlx::raw_sql(m.sql).execute(pool).await.map_err(|e| {
                CoreError::Postgres(format!(
                    "apply state migration {} ({}): {e}",
                    m.version, m.description
                ))
            })?;

            sqlx::query(
                "INSERT INTO the_one.state_migrations \
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
                CoreError::Postgres(format!(
                    "record state migration {} ({}): {e}",
                    m.version, m.description
                ))
            })?;
        }

        Ok(())
    }

    async fn upsert_or_verify(pool: &PgPool, m: &Migration) -> Result<(), CoreError> {
        let row: Option<Vec<u8>> = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT checksum FROM the_one.state_migrations WHERE version = $1",
        )
        .bind(m.version)
        .fetch_optional(pool)
        .await
        .map_err(|e| CoreError::Postgres(format!("bootstrap row: {e}")))?;

        let live = checksum(m.sql);
        match row {
            Some(stored) if stored == live => Ok(()),
            Some(_) => Err(CoreError::Postgres(format!(
                "postgres state migration {} ({}) bootstrap row checksum drift. Refusing \
                 to continue. Drop `the_one.state_migrations` and restart against a \
                 consistent binary.",
                m.version, m.description
            ))),
            None => {
                sqlx::query(
                    "INSERT INTO the_one.state_migrations \
                         (version, description, checksum, applied_at_ms) \
                         VALUES ($1, $2, $3, $4)",
                )
                .bind(m.version)
                .bind(m.description)
                .bind(live)
                .bind(now_epoch_ms())
                .execute(pool)
                .await
                .map_err(|e| {
                    CoreError::Postgres(format!("record bootstrap state migration: {e}"))
                })?;
                Ok(())
            }
        }
    }

    /// How many migrations are embedded — used by tests and
    /// observability.
    pub fn embedded_count() -> usize {
        MIGRATIONS.len()
    }

    /// Row type for `list_applied`.
    #[derive(Debug)]
    pub struct AppliedMigration {
        pub version: i32,
        pub checksum: Vec<u8>,
    }

    /// List every row in `the_one.state_migrations`. Used by
    /// integration tests to confirm the runner recorded what it
    /// applied.
    pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, CoreError> {
        let rows =
            sqlx::query("SELECT version, checksum FROM the_one.state_migrations ORDER BY version")
                .fetch_all(pool)
                .await
                .map_err(|e| CoreError::Postgres(format!("list_applied: {e}")))?;
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
// Time helper (no chrono — see error.rs / Cargo.toml)
// ---------------------------------------------------------------------------

/// Current time in milliseconds since the Unix epoch. Matches the
/// `BIGINT epoch_ms` convention the whole workspace uses.
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Helpers: approval scope + navigation kind string round-trip
// ---------------------------------------------------------------------------

fn approval_scope_to_str(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Session => "session",
        ApprovalScope::Forever => "forever",
    }
}

fn navigation_kind_from_str(value: &str) -> Result<MemoryNavigationNodeKind, CoreError> {
    match value {
        "drawer" => Ok(MemoryNavigationNodeKind::Drawer),
        "closet" => Ok(MemoryNavigationNodeKind::Closet),
        "room" => Ok(MemoryNavigationNodeKind::Room),
        other => Err(CoreError::InvalidProjectConfig(format!(
            "unsupported navigation node kind: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Row → struct helpers
// ---------------------------------------------------------------------------

fn audit_event_from_row(row: &PgRow) -> Result<AuditEvent, CoreError> {
    Ok(AuditEvent {
        id: row.try_get::<i64, _>("id").map_err(map_sqlx_err)?,
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        event_type: row
            .try_get::<String, _>("event_type")
            .map_err(map_sqlx_err)?,
        payload_json: row
            .try_get::<String, _>("payload_json")
            .map_err(map_sqlx_err)?,
        outcome: row.try_get::<String, _>("outcome").map_err(map_sqlx_err)?,
        error_kind: row
            .try_get::<Option<String>, _>("error_kind")
            .map_err(map_sqlx_err)?,
        created_at_epoch_ms: row
            .try_get::<i64, _>("created_at_epoch_ms")
            .map_err(map_sqlx_err)?,
    })
}

fn conversation_source_from_row(row: &PgRow) -> Result<ConversationSourceRecord, CoreError> {
    Ok(ConversationSourceRecord {
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        transcript_path: row
            .try_get::<String, _>("transcript_path")
            .map_err(map_sqlx_err)?,
        memory_path: row
            .try_get::<String, _>("memory_path")
            .map_err(map_sqlx_err)?,
        format: row.try_get::<String, _>("format").map_err(map_sqlx_err)?,
        wing: row
            .try_get::<Option<String>, _>("wing")
            .map_err(map_sqlx_err)?,
        hall: row
            .try_get::<Option<String>, _>("hall")
            .map_err(map_sqlx_err)?,
        room: row
            .try_get::<Option<String>, _>("room")
            .map_err(map_sqlx_err)?,
        message_count: row
            .try_get::<i64, _>("message_count")
            .map_err(map_sqlx_err)? as usize,
    })
}

fn aaak_lesson_from_row(row: &PgRow) -> Result<AaakLesson, CoreError> {
    Ok(AaakLesson {
        lesson_id: row
            .try_get::<String, _>("lesson_id")
            .map_err(map_sqlx_err)?,
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        pattern_key: row
            .try_get::<String, _>("pattern_key")
            .map_err(map_sqlx_err)?,
        role: row.try_get::<String, _>("role").map_err(map_sqlx_err)?,
        canonical_text: row
            .try_get::<String, _>("canonical_text")
            .map_err(map_sqlx_err)?,
        occurrence_count: row
            .try_get::<i64, _>("occurrence_count")
            .map_err(map_sqlx_err)? as usize,
        confidence_percent: row
            .try_get::<i64, _>("confidence_percent")
            .map_err(map_sqlx_err)? as u8,
        source_transcript_path: row
            .try_get::<Option<String>, _>("source_transcript_path")
            .map_err(map_sqlx_err)?,
        updated_at_epoch_ms: row
            .try_get::<i64, _>("updated_at_epoch_ms")
            .map_err(map_sqlx_err)?,
    })
}

fn diary_entry_from_row(row: &PgRow) -> Result<DiaryEntry, CoreError> {
    let tags_json: String = row
        .try_get::<String, _>("tags_json")
        .map_err(map_sqlx_err)?;
    Ok(DiaryEntry {
        entry_id: row.try_get::<String, _>("entry_id").map_err(map_sqlx_err)?,
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        entry_date: row
            .try_get::<String, _>("entry_date")
            .map_err(map_sqlx_err)?,
        mood: row
            .try_get::<Option<String>, _>("mood")
            .map_err(map_sqlx_err)?,
        tags: serde_json::from_str(&tags_json)?,
        content: row.try_get::<String, _>("content").map_err(map_sqlx_err)?,
        created_at_epoch_ms: row
            .try_get::<i64, _>("created_at_epoch_ms")
            .map_err(map_sqlx_err)?,
        updated_at_epoch_ms: row
            .try_get::<i64, _>("updated_at_epoch_ms")
            .map_err(map_sqlx_err)?,
    })
}

fn navigation_node_from_row(row: &PgRow) -> Result<MemoryNavigationNode, CoreError> {
    let kind_str: String = row.try_get::<String, _>("kind").map_err(map_sqlx_err)?;
    Ok(MemoryNavigationNode {
        node_id: row.try_get::<String, _>("node_id").map_err(map_sqlx_err)?,
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        kind: navigation_kind_from_str(&kind_str)?,
        label: row.try_get::<String, _>("label").map_err(map_sqlx_err)?,
        parent_node_id: row
            .try_get::<Option<String>, _>("parent_node_id")
            .map_err(map_sqlx_err)?,
        wing: row
            .try_get::<Option<String>, _>("wing")
            .map_err(map_sqlx_err)?,
        hall: row
            .try_get::<Option<String>, _>("hall")
            .map_err(map_sqlx_err)?,
        room: row
            .try_get::<Option<String>, _>("room")
            .map_err(map_sqlx_err)?,
        updated_at_epoch_ms: row
            .try_get::<i64, _>("updated_at_epoch_ms")
            .map_err(map_sqlx_err)?,
    })
}

fn navigation_tunnel_from_row(row: &PgRow) -> Result<MemoryNavigationTunnel, CoreError> {
    Ok(MemoryNavigationTunnel {
        tunnel_id: row
            .try_get::<String, _>("tunnel_id")
            .map_err(map_sqlx_err)?,
        project_id: row
            .try_get::<String, _>("project_id")
            .map_err(map_sqlx_err)?,
        from_node_id: row
            .try_get::<String, _>("from_node_id")
            .map_err(map_sqlx_err)?,
        to_node_id: row
            .try_get::<String, _>("to_node_id")
            .map_err(map_sqlx_err)?,
        updated_at_epoch_ms: row
            .try_get::<i64, _>("updated_at_epoch_ms")
            .map_err(map_sqlx_err)?,
    })
}

/// Uniform sqlx → CoreError mapping. Every sqlx error at query time
/// lands in this function so the string formatting is consistent.
fn map_sqlx_err(e: sqlx::Error) -> CoreError {
    CoreError::Postgres(format!("postgres state: {e}"))
}

// ---------------------------------------------------------------------------
// impl StateStore for PostgresStateStore
// ---------------------------------------------------------------------------

impl StateStore for PostgresStateStore {
    // ── Metadata ───────────────────────────────────────────────────

    fn project_id(&self) -> &str {
        &self.project_id
    }

    fn schema_version(&self) -> Result<i64, CoreError> {
        block_on(async {
            let row: Option<i32> = sqlx::query_scalar::<_, i32>(
                "SELECT COALESCE(MAX(version), 0) FROM the_one.state_migrations",
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(row.unwrap_or(0) as i64)
        })
    }

    fn capabilities(&self) -> StateStoreCapabilities {
        StateStoreCapabilities {
            name: "postgres",
            fts: true,
            transactions: true,
            durable: true,
            schema_versioned: true,
        }
    }

    // ── Project profiles ───────────────────────────────────────────

    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError> {
        let now = now_epoch_ms();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.project_profiles \
                     (project_id, profile_json, updated_at_epoch_ms) \
                     VALUES ($1, $2, $3) \
                     ON CONFLICT (project_id) DO UPDATE SET \
                         profile_json = EXCLUDED.profile_json, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms",
            )
            .bind(&self.project_id)
            .bind(profile_json)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn latest_project_profile(&self) -> Result<Option<String>, CoreError> {
        block_on(async {
            let row: Option<String> = sqlx::query_scalar::<_, String>(
                "SELECT profile_json FROM the_one.project_profiles WHERE project_id = $1",
            )
            .bind(&self.project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(row)
        })
    }

    // ── Approvals ──────────────────────────────────────────────────

    fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError> {
        let scope_str = approval_scope_to_str(scope);
        let approved_int: i32 = if approved { 1 } else { 0 };
        let now = now_epoch_ms();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.approvals \
                     (project_id, action_key, scope, approved, created_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5) \
                     ON CONFLICT (project_id, action_key, scope) DO UPDATE SET \
                         approved = EXCLUDED.approved, \
                         created_at_epoch_ms = EXCLUDED.created_at_epoch_ms",
            )
            .bind(&self.project_id)
            .bind(action_key)
            .bind(scope_str)
            .bind(approved_int)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn is_approved(&self, action_key: &str, scope: ApprovalScope) -> Result<bool, CoreError> {
        let scope_str = approval_scope_to_str(scope);
        block_on(async {
            let row: Option<i32> = sqlx::query_scalar::<_, i32>(
                "SELECT approved FROM the_one.approvals \
                 WHERE project_id = $1 AND action_key = $2 AND scope = $3",
            )
            .bind(&self.project_id)
            .bind(action_key)
            .bind(scope_str)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(matches!(row, Some(1)))
        })
    }

    // ── Audit ──────────────────────────────────────────────────────

    fn record_audit_event(&self, event_type: &str, payload_json: &str) -> Result<(), CoreError> {
        let now = now_epoch_ms();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.audit_events \
                     (project_id, event_type, payload_json, outcome, error_kind, \
                      created_at_epoch_ms) \
                     VALUES ($1, $2, $3, 'unknown', NULL, $4)",
            )
            .bind(&self.project_id)
            .bind(event_type)
            .bind(payload_json)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError> {
        let now = now_epoch_ms();
        let outcome = record.outcome.as_str();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.audit_events \
                     (project_id, event_type, payload_json, outcome, error_kind, \
                      created_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&self.project_id)
            .bind(record.operation)
            .bind(&record.params_json)
            .bind(outcome)
            .bind(record.error_kind)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn audit_event_count_for_project(&self) -> Result<u64, CoreError> {
        block_on(async {
            let count: i64 = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM the_one.audit_events WHERE project_id = $1",
            )
            .bind(&self.project_id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(count.max(0) as u64)
        })
    }

    fn list_audit_events_paged(&self, req: &PageRequest) -> Result<Page<AuditEvent>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let limit = req.limit;
        let current_offset = req.offset;
        block_on(async {
            let rows = sqlx::query(
                "SELECT id, project_id, event_type, payload_json, outcome, error_kind, \
                        created_at_epoch_ms \
                 FROM the_one.audit_events \
                 WHERE project_id = $1 \
                 ORDER BY id DESC \
                 LIMIT $2 OFFSET $3",
            )
            .bind(&self.project_id)
            .bind(fetch)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_err)?;

            let mut events = Vec::with_capacity(rows.len());
            for r in &rows {
                events.push(audit_event_from_row(r)?);
            }

            let total: i64 = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM the_one.audit_events WHERE project_id = $1",
            )
            .bind(&self.project_id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_err)?;

            Ok(Page::from_peek(
                events,
                limit,
                current_offset,
                Some(total.max(0) as u64),
            ))
        })
    }

    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::AUDIT_EVENTS_DEFAULT,
            page_limits::AUDIT_EVENTS_MAX,
        )?;
        Ok(self.list_audit_events_paged(&req)?.items)
    }

    // ── Conversation sources ───────────────────────────────────────

    fn upsert_conversation_source(
        &self,
        record: &ConversationSourceRecord,
    ) -> Result<(), CoreError> {
        let now = now_epoch_ms();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.conversation_sources \
                     (project_id, transcript_path, memory_path, format, wing, hall, room, \
                      message_count, updated_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                     ON CONFLICT (project_id, transcript_path) DO UPDATE SET \
                         memory_path = EXCLUDED.memory_path, \
                         format = EXCLUDED.format, \
                         wing = EXCLUDED.wing, \
                         hall = EXCLUDED.hall, \
                         room = EXCLUDED.room, \
                         message_count = EXCLUDED.message_count, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms",
            )
            .bind(&self.project_id)
            .bind(&record.transcript_path)
            .bind(&record.memory_path)
            .bind(&record.format)
            .bind(record.wing.as_deref())
            .bind(record.hall.as_deref())
            .bind(record.room.as_deref())
            .bind(record.message_count as i64)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn list_conversation_sources(
        &self,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSourceRecord>, CoreError> {
        let safe_limit = limit.max(1).min(i64::MAX as usize) as i64;

        // Build the query dynamically so each optional filter adds
        // another placeholder. We use concatenated SQL with explicit
        // `$N` placeholders instead of sqlx's `QueryBuilder` because
        // the handful of optional args doesn't warrant that machinery.
        let mut sql = String::from(
            "SELECT project_id, transcript_path, memory_path, format, wing, hall, room, \
                    message_count \
             FROM the_one.conversation_sources \
             WHERE project_id = $1",
        );
        let mut next_placeholder = 2;
        if wing.is_some() {
            sql.push_str(&format!(" AND wing = ${next_placeholder}"));
            next_placeholder += 1;
        }
        if hall.is_some() {
            sql.push_str(&format!(" AND hall = ${next_placeholder}"));
            next_placeholder += 1;
        }
        if room.is_some() {
            sql.push_str(&format!(" AND room = ${next_placeholder}"));
            next_placeholder += 1;
        }
        sql.push_str(&format!(
            " ORDER BY updated_at_epoch_ms DESC, transcript_path ASC LIMIT ${next_placeholder}"
        ));

        block_on(async {
            let mut query = sqlx::query(&sql).bind(&self.project_id);
            if let Some(w) = wing {
                query = query.bind(w);
            }
            if let Some(h) = hall {
                query = query.bind(h);
            }
            if let Some(r) = room {
                query = query.bind(r);
            }
            query = query.bind(safe_limit);

            let rows = query.fetch_all(&self.pool).await.map_err(map_sqlx_err)?;
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(conversation_source_from_row(r)?);
            }
            Ok(out)
        })
    }

    // ── AAAK lessons ───────────────────────────────────────────────

    fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError> {
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.aaak_lessons \
                     (lesson_id, project_id, pattern_key, role, canonical_text, \
                      occurrence_count, confidence_percent, source_transcript_path, \
                      updated_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                     ON CONFLICT (lesson_id) DO UPDATE SET \
                         pattern_key = EXCLUDED.pattern_key, \
                         role = EXCLUDED.role, \
                         canonical_text = EXCLUDED.canonical_text, \
                         occurrence_count = EXCLUDED.occurrence_count, \
                         confidence_percent = EXCLUDED.confidence_percent, \
                         source_transcript_path = EXCLUDED.source_transcript_path, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms",
            )
            .bind(&lesson.lesson_id)
            .bind(&lesson.project_id)
            .bind(&lesson.pattern_key)
            .bind(&lesson.role)
            .bind(&lesson.canonical_text)
            .bind(lesson.occurrence_count as i64)
            .bind(lesson.confidence_percent as i64)
            .bind(lesson.source_transcript_path.as_deref())
            .bind(lesson.updated_at_epoch_ms)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn list_aaak_lessons(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::AAAK_LESSONS_DEFAULT,
            page_limits::AAAK_LESSONS_MAX,
        )?;
        let fetch = req.fetch_limit() as i64;
        let target_project = project_id.to_string();
        block_on(async {
            let rows = sqlx::query(
                "SELECT lesson_id, project_id, pattern_key, role, canonical_text, \
                        occurrence_count, confidence_percent, source_transcript_path, \
                        updated_at_epoch_ms \
                 FROM the_one.aaak_lessons \
                 WHERE project_id = $1 \
                 ORDER BY confidence_percent DESC, occurrence_count DESC, lesson_id ASC \
                 LIMIT $2",
            )
            .bind(&target_project)
            .bind(fetch)
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_err)?;

            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(aaak_lesson_from_row(r)?);
                if out.len() >= req.limit {
                    break;
                }
            }
            Ok(out)
        })
    }

    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError> {
        block_on(async {
            let result = sqlx::query("DELETE FROM the_one.aaak_lessons WHERE lesson_id = $1")
                .bind(lesson_id)
                .execute(&self.pool)
                .await
                .map_err(map_sqlx_err)?;
            Ok(result.rows_affected() > 0)
        })
    }

    // ── Diary ──────────────────────────────────────────────────────

    /// Atomic upsert of the main diary row + the tsvector FTS field.
    /// SQLite's `upsert_diary_entry` wraps three statements in a
    /// transaction; the Postgres version only needs one INSERT because
    /// the `content_tsv` column lives on the same row as the main
    /// data. We still run it inside `pool.begin()` so the commit is
    /// atomic against any future additions to the upsert flow.
    fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError> {
        if entry.project_id != self.project_id {
            return Err(CoreError::InvalidRequest(format!(
                "diary entry project_id {} does not match database project {}",
                entry.project_id, self.project_id
            )));
        }

        let tags_json = serde_json::to_string(&entry.tags)?;
        // The FTS tsvector is derived from (mood, tags, content).
        // `to_tsvector('simple', ...)` produces a vector without
        // stemming so exact-word matches work across languages. The
        // Rust layer passes the materialized text and lets Postgres
        // do the tokenization — matches the SQLite side which feeds
        // the same three fields into FTS5.
        let tags_search_text = entry.tags.join(" ");
        let fts_source = format!(
            "{} {} {}",
            entry.mood.as_deref().unwrap_or(""),
            tags_search_text,
            entry.content
        );

        block_on(async {
            let mut tx = self.pool.begin().await.map_err(map_sqlx_err)?;
            sqlx::query(
                "INSERT INTO the_one.diary_entries \
                     (entry_id, project_id, entry_date, mood, tags_json, content, \
                      created_at_epoch_ms, updated_at_epoch_ms, content_tsv) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, to_tsvector('simple', $9)) \
                     ON CONFLICT (project_id, entry_id) DO UPDATE SET \
                         entry_date = EXCLUDED.entry_date, \
                         mood = EXCLUDED.mood, \
                         tags_json = EXCLUDED.tags_json, \
                         content = EXCLUDED.content, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms, \
                         content_tsv = EXCLUDED.content_tsv",
            )
            .bind(&entry.entry_id)
            .bind(&self.project_id)
            .bind(&entry.entry_date)
            .bind(entry.mood.as_deref())
            .bind(&tags_json)
            .bind(&entry.content)
            .bind(entry.created_at_epoch_ms)
            .bind(entry.updated_at_epoch_ms)
            .bind(&fts_source)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_err)?;
            tx.commit().await.map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn list_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::DIARY_ENTRIES_DEFAULT,
            page_limits::DIARY_ENTRIES_MAX,
        )?;
        let fetch = req.fetch_limit() as i64;

        let mut sql = String::from(
            "SELECT entry_id, project_id, entry_date, mood, tags_json, content, \
                    created_at_epoch_ms, updated_at_epoch_ms \
             FROM the_one.diary_entries \
             WHERE project_id = $1",
        );
        let mut next = 2;
        if start_date.is_some() {
            sql.push_str(&format!(" AND entry_date >= ${next}"));
            next += 1;
        }
        if end_date.is_some() {
            sql.push_str(&format!(" AND entry_date <= ${next}"));
            next += 1;
        }
        sql.push_str(&format!(
            " ORDER BY entry_date DESC, updated_at_epoch_ms DESC, entry_id ASC LIMIT ${next}"
        ));

        block_on(async {
            let mut query = sqlx::query(&sql).bind(&self.project_id);
            if let Some(s) = start_date {
                query = query.bind(s);
            }
            if let Some(e) = end_date {
                query = query.bind(e);
            }
            query = query.bind(fetch);
            let rows = query.fetch_all(&self.pool).await.map_err(map_sqlx_err)?;
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(diary_entry_from_row(r)?);
                if out.len() >= req.limit {
                    break;
                }
            }
            Ok(out)
        })
    }

    fn search_diary_entries_in_range(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::DIARY_ENTRIES_DEFAULT,
            page_limits::DIARY_ENTRIES_MAX,
        )?;
        let fetch = req.fetch_limit() as i64;

        // Try the tsvector path first. `websearch_to_tsquery` accepts
        // plain user-typed text (spaces → AND, "quoted phrases",
        // -negation) and never panics — but it CAN produce an empty
        // query for pure-punctuation input, in which case we fall
        // back to LIKE.
        match self.search_diary_fts(query, start_date, end_date, fetch, &req) {
            Ok(entries) if !entries.is_empty() => Ok(entries),
            Ok(_) => self.search_diary_like(query, start_date, end_date, fetch, &req),
            Err(CoreError::Postgres(_)) => {
                self.search_diary_like(query, start_date, end_date, fetch, &req)
            }
            Err(other) => Err(other),
        }
    }

    // ── Navigation ─────────────────────────────────────────────────

    fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError> {
        if node.project_id != self.project_id {
            return Err(CoreError::InvalidRequest(format!(
                "navigation record project_id '{}' does not match database scope '{}'",
                node.project_id, self.project_id
            )));
        }
        let kind_str = node.kind.as_str();
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.navigation_nodes \
                     (node_id, project_id, kind, label, parent_node_id, wing, hall, room, \
                      updated_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                     ON CONFLICT (project_id, node_id) DO UPDATE SET \
                         kind = EXCLUDED.kind, \
                         label = EXCLUDED.label, \
                         parent_node_id = EXCLUDED.parent_node_id, \
                         wing = EXCLUDED.wing, \
                         hall = EXCLUDED.hall, \
                         room = EXCLUDED.room, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms",
            )
            .bind(&node.node_id)
            .bind(&self.project_id)
            .bind(kind_str)
            .bind(&node.label)
            .bind(node.parent_node_id.as_deref())
            .bind(node.wing.as_deref())
            .bind(node.hall.as_deref())
            .bind(node.room.as_deref())
            .bind(node.updated_at_epoch_ms)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn get_navigation_node(
        &self,
        node_id: &str,
    ) -> Result<Option<MemoryNavigationNode>, CoreError> {
        block_on(async {
            let row_opt = sqlx::query(
                "SELECT node_id, project_id, kind, label, parent_node_id, wing, hall, room, \
                        updated_at_epoch_ms \
                 FROM the_one.navigation_nodes \
                 WHERE project_id = $1 AND node_id = $2 \
                 LIMIT 1",
            )
            .bind(&self.project_id)
            .bind(node_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;

            match row_opt {
                Some(row) => Ok(Some(navigation_node_from_row(&row)?)),
                None => Ok(None),
            }
        })
    }

    fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let limit = req.limit;
        let current_offset = req.offset;

        let mut sql = String::from(
            "SELECT node_id, project_id, kind, label, parent_node_id, wing, hall, room, \
                    updated_at_epoch_ms \
             FROM the_one.navigation_nodes \
             WHERE project_id = $1",
        );
        let mut next = 2;
        if parent_node_id.is_some() {
            sql.push_str(&format!(" AND parent_node_id = ${next}"));
            next += 1;
        }
        if kind.is_some() {
            sql.push_str(&format!(" AND kind = ${next}"));
            next += 1;
        }
        sql.push_str(&format!(
            " ORDER BY kind ASC, label ASC, node_id ASC LIMIT ${next} OFFSET ${}",
            next + 1
        ));

        let mut count_sql =
            String::from("SELECT COUNT(*) FROM the_one.navigation_nodes WHERE project_id = $1");
        let mut count_next = 2;
        if parent_node_id.is_some() {
            count_sql.push_str(&format!(" AND parent_node_id = ${count_next}"));
            count_next += 1;
        }
        if kind.is_some() {
            count_sql.push_str(&format!(" AND kind = ${count_next}"));
        }

        block_on(async {
            let mut query = sqlx::query(&sql).bind(&self.project_id);
            if let Some(p) = parent_node_id {
                query = query.bind(p);
            }
            if let Some(k) = kind {
                query = query.bind(k);
            }
            query = query.bind(fetch).bind(offset);
            let rows = query.fetch_all(&self.pool).await.map_err(map_sqlx_err)?;
            let mut nodes = Vec::with_capacity(rows.len());
            for r in &rows {
                nodes.push(navigation_node_from_row(r)?);
            }

            let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql).bind(&self.project_id);
            if let Some(p) = parent_node_id {
                count_query = count_query.bind(p);
            }
            if let Some(k) = kind {
                count_query = count_query.bind(k);
            }
            let total: i64 = count_query
                .fetch_one(&self.pool)
                .await
                .map_err(map_sqlx_err)?;

            Ok(Page::from_peek(
                nodes,
                limit,
                current_offset,
                Some(total.max(0) as u64),
            ))
        })
    }

    fn upsert_navigation_tunnel(&self, tunnel: &MemoryNavigationTunnel) -> Result<(), CoreError> {
        if tunnel.project_id != self.project_id {
            return Err(CoreError::InvalidRequest(format!(
                "navigation tunnel project_id '{}' does not match database scope '{}'",
                tunnel.project_id, self.project_id
            )));
        }
        block_on(async {
            sqlx::query(
                "INSERT INTO the_one.navigation_tunnels \
                     (tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms) \
                     VALUES ($1, $2, $3, $4, $5) \
                     ON CONFLICT (project_id, from_node_id, to_node_id) DO UPDATE SET \
                         tunnel_id = EXCLUDED.tunnel_id, \
                         updated_at_epoch_ms = EXCLUDED.updated_at_epoch_ms",
            )
            .bind(&tunnel.tunnel_id)
            .bind(&self.project_id)
            .bind(&tunnel.from_node_id)
            .bind(&tunnel.to_node_id)
            .bind(tunnel.updated_at_epoch_ms)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
            Ok(())
        })
    }

    fn list_navigation_tunnels_paged(
        &self,
        node_id: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationTunnel>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let limit = req.limit;
        let current_offset = req.offset;

        let (sql, count_sql) = if node_id.is_some() {
            (
                "SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms \
                 FROM the_one.navigation_tunnels \
                 WHERE project_id = $1 AND (from_node_id = $2 OR to_node_id = $2) \
                 ORDER BY from_node_id ASC, to_node_id ASC, tunnel_id ASC \
                 LIMIT $3 OFFSET $4"
                    .to_string(),
                "SELECT COUNT(*) FROM the_one.navigation_tunnels \
                 WHERE project_id = $1 AND (from_node_id = $2 OR to_node_id = $2)"
                    .to_string(),
            )
        } else {
            (
                "SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms \
                 FROM the_one.navigation_tunnels \
                 WHERE project_id = $1 \
                 ORDER BY from_node_id ASC, to_node_id ASC, tunnel_id ASC \
                 LIMIT $2 OFFSET $3"
                    .to_string(),
                "SELECT COUNT(*) FROM the_one.navigation_tunnels WHERE project_id = $1".to_string(),
            )
        };

        block_on(async {
            let rows = if let Some(id) = node_id {
                sqlx::query(&sql)
                    .bind(&self.project_id)
                    .bind(id)
                    .bind(fetch)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
            } else {
                sqlx::query(&sql)
                    .bind(&self.project_id)
                    .bind(fetch)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
            }
            .map_err(map_sqlx_err)?;

            let mut tunnels = Vec::with_capacity(rows.len());
            for r in &rows {
                tunnels.push(navigation_tunnel_from_row(r)?);
            }

            let total: i64 = if let Some(id) = node_id {
                sqlx::query_scalar::<_, i64>(&count_sql)
                    .bind(&self.project_id)
                    .bind(id)
                    .fetch_one(&self.pool)
                    .await
            } else {
                sqlx::query_scalar::<_, i64>(&count_sql)
                    .bind(&self.project_id)
                    .fetch_one(&self.pool)
                    .await
            }
            .map_err(map_sqlx_err)?;

            Ok(Page::from_peek(
                tunnels,
                limit,
                current_offset,
                Some(total.max(0) as u64),
            ))
        })
    }

    fn list_navigation_tunnels_for_nodes(
        &self,
        node_ids: &[String],
        limit: usize,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::NAVIGATION_TUNNELS_DEFAULT,
            page_limits::NAVIGATION_TUNNELS_MAX,
        )?;

        // Dedup the input so ORDER BY is stable.
        let mut unique: Vec<&String> = node_ids.iter().collect();
        unique.sort();
        unique.dedup();

        // Postgres has no SQLITE_MAX_VARIABLE_NUMBER limit worth
        // worrying about (default is 32767 parameters vs SQLite's
        // ~999), but we still chunk at 400 to match the SQLite
        // shape and keep query plans predictable on massive inputs.
        const CHUNK: usize = 400;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<MemoryNavigationTunnel> = Vec::new();

        for chunk in unique.chunks(CHUNK) {
            let placeholders: String = (0..chunk.len())
                .map(|i| format!("${}", i + 2))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms \
                 FROM the_one.navigation_tunnels \
                 WHERE project_id = $1 \
                   AND (from_node_id IN ({ph}) OR to_node_id IN ({ph})) \
                 ORDER BY from_node_id ASC, to_node_id ASC, tunnel_id ASC \
                 LIMIT ${lim}",
                ph = placeholders,
                lim = chunk.len() + 2
            );

            let rows = block_on(async {
                let mut q = sqlx::query(&sql).bind(&self.project_id);
                for id in chunk {
                    q = q.bind(id.as_str());
                }
                q = q.bind(req.limit as i64);
                q.fetch_all(&self.pool).await.map_err(map_sqlx_err)
            })?;

            for r in &rows {
                let tunnel = navigation_tunnel_from_row(r)?;
                if seen.insert(tunnel.tunnel_id.clone()) {
                    out.push(tunnel);
                }
                if out.len() >= req.limit {
                    break;
                }
            }
            if out.len() >= req.limit {
                break;
            }
        }

        out.sort_by(|a, b| {
            a.from_node_id
                .cmp(&b.from_node_id)
                .then(a.to_node_id.cmp(&b.to_node_id))
                .then(a.tunnel_id.cmp(&b.tunnel_id))
        });
        out.truncate(req.limit);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Diary FTS implementation details (non-trait helpers)
// ---------------------------------------------------------------------------

impl PostgresStateStore {
    /// Run the tsvector / websearch_to_tsquery path. Returns the
    /// matching entries on success; an empty vec if the parsed
    /// query produced no tokens (common with pure-punctuation input).
    fn search_diary_fts(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        fetch: i64,
        req: &PageRequest,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let mut sql = String::from(
            "SELECT entry_id, project_id, entry_date, mood, tags_json, content, \
                    created_at_epoch_ms, updated_at_epoch_ms \
             FROM the_one.diary_entries \
             WHERE project_id = $1 \
               AND content_tsv @@ websearch_to_tsquery('simple', $2)",
        );
        let mut next = 3;
        if start_date.is_some() {
            sql.push_str(&format!(" AND entry_date >= ${next}"));
            next += 1;
        }
        if end_date.is_some() {
            sql.push_str(&format!(" AND entry_date <= ${next}"));
            next += 1;
        }
        sql.push_str(&format!(
            " ORDER BY ts_rank(content_tsv, websearch_to_tsquery('simple', $2)) DESC, \
                       entry_date DESC, updated_at_epoch_ms DESC, entry_id ASC \
              LIMIT ${next}"
        ));

        let limit = req.limit;
        let query = query.to_string();
        let start = start_date.map(str::to_string);
        let end = end_date.map(str::to_string);

        block_on(async {
            let mut q = sqlx::query(&sql).bind(&self.project_id).bind(&query);
            if let Some(s) = &start {
                q = q.bind(s);
            }
            if let Some(e) = &end {
                q = q.bind(e);
            }
            q = q.bind(fetch);
            let rows = q.fetch_all(&self.pool).await.map_err(map_sqlx_err)?;
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(diary_entry_from_row(r)?);
                if out.len() >= limit {
                    break;
                }
            }
            Ok(out)
        })
    }

    /// LIKE fallback for queries that produce an empty tsquery or
    /// hit a Postgres-level parse error. Scans `content` + `mood`
    /// with case-insensitive substring matching.
    fn search_diary_like(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        fetch: i64,
        req: &PageRequest,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut sql = String::from(
            "SELECT entry_id, project_id, entry_date, mood, tags_json, content, \
                    created_at_epoch_ms, updated_at_epoch_ms \
             FROM the_one.diary_entries \
             WHERE project_id = $1 \
               AND (content ILIKE $2 OR COALESCE(mood, '') ILIKE $2 OR tags_json ILIKE $2)",
        );
        let mut next = 3;
        if start_date.is_some() {
            sql.push_str(&format!(" AND entry_date >= ${next}"));
            next += 1;
        }
        if end_date.is_some() {
            sql.push_str(&format!(" AND entry_date <= ${next}"));
            next += 1;
        }
        sql.push_str(&format!(
            " ORDER BY entry_date DESC, updated_at_epoch_ms DESC, entry_id ASC \
              LIMIT ${next}"
        ));

        let limit = req.limit;
        let start = start_date.map(str::to_string);
        let end = end_date.map(str::to_string);

        block_on(async {
            let mut q = sqlx::query(&sql).bind(&self.project_id).bind(&pattern);
            if let Some(s) = &start {
                q = q.bind(s);
            }
            if let Some(e) = &end {
                q = q.bind(e);
            }
            q = q.bind(fetch);
            let rows = q.fetch_all(&self.pool).await.map_err(map_sqlx_err)?;
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(diary_entry_from_row(r)?);
                if out.len() >= limit {
                    break;
                }
            }
            Ok(out)
        })
    }
}

// Keep this import live so compilers notice unused-import issues if the
// json path ever gets refactored out.
#[allow(dead_code)]
fn _force_json_path_used() -> JsonValue {
    JsonValue::Null
}

// ---------------------------------------------------------------------------
// Pure-Rust unit tests (no live database required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_plan_defaults() {
        let cfg = PostgresStateConfig::default();
        assert_eq!(cfg.schema, "the_one");
        assert_eq!(cfg.statement_timeout_ms, 30_000);
        assert_eq!(cfg.max_connections, 10);
        assert_eq!(cfg.min_connections, 2);
        assert_eq!(cfg.acquire_timeout_ms, 30_000);
        assert_eq!(cfg.idle_timeout_ms, 600_000);
        assert_eq!(cfg.max_lifetime_ms, 1_800_000);
    }

    #[test]
    fn embedded_migration_count_matches_file_list() {
        // Catches "added a .sql file but forgot to append to
        // MIGRATIONS" — should ALWAYS be 2 until a new migration
        // ships in a later phase.
        assert_eq!(migrations::embedded_count(), 2);
    }

    #[test]
    fn navigation_kind_round_trip() {
        assert_eq!(
            navigation_kind_from_str("drawer").unwrap(),
            MemoryNavigationNodeKind::Drawer
        );
        assert_eq!(
            navigation_kind_from_str("closet").unwrap(),
            MemoryNavigationNodeKind::Closet
        );
        assert_eq!(
            navigation_kind_from_str("room").unwrap(),
            MemoryNavigationNodeKind::Room
        );
        assert!(navigation_kind_from_str("unknown").is_err());
    }

    #[test]
    fn approval_scope_strings_stable() {
        assert_eq!(approval_scope_to_str(ApprovalScope::Once), "once");
        assert_eq!(approval_scope_to_str(ApprovalScope::Session), "session");
        assert_eq!(approval_scope_to_str(ApprovalScope::Forever), "forever");
    }

    #[test]
    fn now_epoch_ms_is_positive_and_monotonic() {
        let a = now_epoch_ms();
        let b = now_epoch_ms();
        assert!(a > 0);
        assert!(b >= a);
    }
}
