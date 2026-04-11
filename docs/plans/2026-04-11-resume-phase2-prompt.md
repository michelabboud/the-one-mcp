# Resume prompt for Phase 2 — pgvector VectorBackend + env var parser + startup validator

**Date authored:** 2026-04-11
**Baseline commit:** `7666439` (v0.16.0 Phase 1 — broker `state_by_project` cache via `StateStore` trait)
**Baseline tag:** `v0.16.0-phase1`
**Target:** execute **Phase 2** of the multi-backend roadmap
**Status:** fresh session ready — paste this prompt or say "read `docs/plans/2026-04-11-resume-phase2-prompt.md`"

---

## Who I am (Michel)

You're continuing a multi-session refactor that will land full multi-backend support for both vectors and state in `the-one-mcp`. Phases 0 and 1 are already on `main`:

- **Phase 0** (commit `5ff9872`, tag `v0.16.0-rc1`) — bundled trait extraction: `trait VectorBackend`, `trait StateStore`, `MemoryEngine` now holds `Box<dyn VectorBackend>`, `impl StateStore for ProjectDatabase`, diary upsert atomicity fix, + all v0.15.0 production hardening + v0.15.1 Lever 1 audit speedup.
- **Phase 1** (commit `7666439`, tag `v0.16.0-phase1`) — broker call-site migration: `state_by_project` cache, `state_store_factory`, `with_state_store` chokepoint, `shutdown()`, sync closures guarded by `std::sync::Mutex` for `!Send` hygiene across awaits.

Phase 2 ships the **first real alternative backend** (pgvector) plus the **env var parser** and **startup validator** that all subsequent phases depend on. This is where the four-var selection scheme becomes real code, not just a plan.

## Non-negotiable global rules

- Do what I say, full production grade, no shortcuts, no stubs, no placeholders, no "good enough". Phase 2 ships complete or doesn't ship.
- NEVER defer, skip, or descope anything without explicit approval.
- NEVER bump `Cargo.toml` version (stays `"0.1.0"`). Real versioning is in git tags + commit subjects + `CHANGELOG.md`.
- NEVER commit anything without explicit authorisation.
- If a spec is ambiguous, ASK — don't pick the minimal interpretation.
- When Phase 2 is fully complete (design + impl + tests + docs + committed + pushed + built), suggest `/compact` or `/clear` before Phase 3.

## Read FIRST (in this exact order)

1. `docs/plans/2026-04-11-multi-backend-architecture.md` — the architectural plan (Phase A is shipped; Phase B is what Phase 2 starts delivering)
2. `docs/plans/2026-04-11-resume-phase1-onwards.md` — **§ Backend selection scheme (§§ 1–6) is load-bearing for everything you write in Phase 2.** The § Phase 1 — DONE section at the top of the file documents exactly what shipped in Phase 1 so you can see the cache + factory shape your Phase 2 branch will plug into.
3. `CLAUDE.md` — project conventions block. The `Phase A multi-backend traits (v0.16.0-rc1)` bullet and the `Phase 1 broker state store cache (v0.16.0-phase1)` bullet describe the code surface your Phase 2 work extends.
4. `CHANGELOG.md` — `[Unreleased]` section has the Phase 1 entry. You will prepend Phase 2's entry here.
5. `docs/guides/multi-backend-operations.md` — forward-looking ops guide, already references `state_store.rs`. Phase 2 will add pgvector-specific sections here.
6. `docs/guides/production-hardening-v0.15.md` — § 14 is Lever 1 + rationale. Phase 2 adds a new pgvector section (per the Phase 2 deliverables list).

Then read **this file's § Phase 2 full deliverables** below.

## Baseline to verify before touching anything

```bash
git log --oneline -5
# Expected first line: "7666439 feat(mcp): broker state_by_project cache via StateStore trait"
git status
# Expected: clean (docs commit for Phase 1 closeout may be between rc1 and phase1; verify commit order matches local tags)

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace 2>&1 | tee /tmp/phase2-baseline.log
grep "test result:" /tmp/phase2-baseline.log | awk '{p+=$4;f+=$6} END {print "BASELINE:",p,"passing,",f,"failing"}'
# Expected: 450 passing, 0 failing, 1 ignored (Lever 2 guard)
```

**If ANY of these fail, STOP and report.** A failing baseline means environment drift, not a Phase 2 bug.

Record the baseline count in `/tmp/the-one-baseline.txt`. Phase 2 must end with monotonically-greater-than-450 passing.

## § Phase 2 full deliverables

**Scope (from the execution plan):** ~800 LOC across new files + env var parser introduction.

**Commit message (exact):** `feat(memory): v0.16.0 — pgvector VectorBackend + env var parser + startup validator`

### 1. Workspace dependencies

Add to the **workspace** `Cargo.toml` (not the individual crate — respect the workspace pattern already in use):

```toml
sqlx = { version = "0.8", default-features = false, features = [
    "runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"
] }
pgvector = { version = "0.4", features = ["sqlx"] }
```

**Decision A — sqlx TLS stack → DECIDED: `tls-rustls`** *(locked in by Michel before the fresh session started; no need to ask again)*

Pure-Rust TLS with `rustls` + `webpki-roots`. No OpenSSL dependency, smallest workspace blast radius, composes cleanly with the rest of the tokio async ecosystem already in use. Alternatives considered and rejected:

- `runtime-tokio` alone (no TLS): local dev only, would fail against any managed Postgres (AWS RDS, Supabase, GCP Cloud SQL, Azure Flexible Server all require TLS).
- `tls-native-tls`: only justified if the workspace already pulls `native-tls` and rustls would double up TLS stacks; we don't, so this would add OpenSSL for no gain.

**One STOP condition still applies:** run `cargo tree | grep rustls` BEFORE adding the sqlx dep. If an existing workspace dep pulls a conflicting rustls major version, STOP and report — the fix might be to align versions across the workspace rather than proceeding with a resolver conflict.

**Decision B — sqlx non-TLS feature set → DECIDED: `["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"]`** *(locked in; no need to ask again)*

Per-feature rationale:
- `runtime-tokio` — tokio runtime integration. Required.
- `tls-rustls` — per Decision A.
- `postgres` — Postgres driver. Required.
- `macros` — enables `sqlx::query!`, `sqlx::query_as!`, compile-time SQL validation. Required for the batched upsert patterns in § 3.
- `migrate` — enables `sqlx::migrate!` macro. **Required for Decision C** (schema migration tracking). Also shared with Phase 3's `PostgresStateStore` which will use a sibling migration directory.
- `chrono` — `TIMESTAMPTZ` ↔ `chrono::DateTime` conversion. Postgres audit timestamps use `timestamptz` and the existing Rust code expects `DateTime`. Required for Phase 3 but harmless to land now with Phase 2.

**`uuid` is deliberately omitted.** The initial read was "probably needed for project identifiers stored as UUID vs TEXT". Decision: project identifiers stay as `TEXT` in the pgvector schema — matches the existing SQLite schema, matches the `StateStore::project_id() -> &str` trait contract, keeps the sqlx dep tree leaner. If a future phase needs UUID columns, add the feature then.

### 2. New Cargo feature `pg-vectors`

- Add `pg-vectors = ["dep:sqlx", "dep:pgvector"]` to `crates/the-one-memory/Cargo.toml`.
- `default` features: **do not** include `pg-vectors` — operators opt in via `cargo build --features pg-vectors` or via the workspace `Cargo.toml` default on the binary crate.
- Confirm the existing feature flags (`local-embeddings`, `redis-vectors`, `image-embeddings`, `tree-sitter-chunker`, etc.) are not disturbed.

### 3. New file `crates/the-one-memory/src/pg_vector.rs`

Module surface:

```rust
#[cfg(feature = "pg-vectors")]
pub struct PgVectorBackend {
    pool: sqlx::PgPool,
    schema: String,
    project_id: String,
    hnsw_m: i32,
    hnsw_ef_construction: i32,
    hnsw_ef_search: i32,
}

#[cfg(feature = "pg-vectors")]
impl PgVectorBackend {
    pub async fn new(config: &PgVectorConfig, url: &str, project_id: &str) -> Result<Self, String> { ... }
    async fn bootstrap_schema(&self) -> Result<(), String> { ... }
}

#[cfg(feature = "pg-vectors")]
#[async_trait::async_trait]
impl VectorBackend for PgVectorBackend {
    // ALL VectorBackend methods — chunks dense + hybrid, entities, relations,
    // images, persistence verification. No `Ok(())` fallbacks; full parity.
    ...
}
```

**Schema bootstrap strategy — use `sqlx::migrate!` with versioned files, NOT ad-hoc `CREATE TABLE IF NOT EXISTS`.**

The naive approach is to run one big `CREATE TABLE IF NOT EXISTS` block at startup. **Do not do this.** Phase 4 (combined Postgres+pgvector) and Phase 7 (Redis-Vector parity) will need to evolve these tables — add columns, add indexes, add constraints. An idempotent one-shot bootstrap silently diverges between fresh installs and upgraded installs once the `CREATE TABLE` text changes, and the divergence is invisible until a query fails.

Use `sqlx::migrate!` instead. Create the migration directory at **`crates/the-one-memory/migrations/pgvector/`** with versioned SQL files:

```
crates/the-one-memory/migrations/pgvector/
├── 0001_extension_and_schema.sql   -- CREATE EXTENSION + CREATE SCHEMA
├── 0002_chunks_table.sql           -- chunks table + HNSW index + 3 btree indexes
├── 0003_entities_table.sql
├── 0004_relations_table.sql
└── 0005_images_table.sql
```

And at backend construction:

```rust
#[cfg(feature = "pg-vectors")]
impl PgVectorBackend {
    pub async fn new(config: &PgVectorConfig, url: &str, project_id: &str) -> Result<Self, String> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(Some(Duration::from_millis(config.idle_timeout_ms)))
            .max_lifetime(Some(Duration::from_millis(config.max_lifetime_ms)))
            .connect(url)
            .await
            .map_err(|e| format!("pgvector pool connect: {e}"))?;

        // Phase 2: preflight the `vector` extension before running migrations
        // so the operator sees a targeted error message instead of a cryptic
        // sqlx migration failure. See "Extension preconditions" below.
        Self::preflight_vector_extension(&pool).await?;

        // sqlx::migrate! reads the directory at compile time, embeds the
        // SQL files into the binary, and applies them in order using its
        // own `_sqlx_migrations` tracking table. Phase 3's PostgresStateStore
        // will use a sibling `crates/the-one-core/migrations/postgres-state/`
        // directory with the same macro — two independent migration trees
        // that can evolve independently without stepping on each other.
        sqlx::migrate!("./migrations/pgvector")
            .run(&pool)
            .await
            .map_err(|e| format!("pgvector migrations: {e}"))?;

        Ok(Self { pool, schema: config.schema.clone(), project_id: project_id.to_string(),
                  hnsw_m: config.hnsw_m, hnsw_ef_construction: config.hnsw_ef_construction,
                  hnsw_ef_search: config.hnsw_ef_search })
    }
}
```

The first migration (`0001_extension_and_schema.sql`) is:

```sql
CREATE EXTENSION IF NOT EXISTS vector;
CREATE SCHEMA IF NOT EXISTS the_one;
```

Note: `sqlx::migrate!` takes no schema parameter — migration SQL must hardcode the schema name OR the `PgVectorBackend::new` must `SET search_path` on the pool before running migrations. **Default: hardcode `the_one` in the .sql files.** If operators override `[vector.pgvector].schema` in config.toml, the schema override is for NEW installs only — documented clearly in the guide. Cross-schema migration is out of scope for Phase 2.

The second migration (`0002_chunks_table.sql`) creates the actual table:

```sql
CREATE TABLE IF NOT EXISTS the_one.chunks (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    language TEXT,
    signature TEXT,
    symbol TEXT,
    heading_hierarchy JSONB NOT NULL,
    wing TEXT,
    hall TEXT,
    room TEXT,
    content TEXT NOT NULL,
    dense_vector vector($DIMS) NOT NULL,
    sparse_vector_indices INTEGER[],
    sparse_vector_values REAL[],
    created_at_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS chunks_dense_hnsw
    ON the_one.chunks USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

CREATE INDEX IF NOT EXISTS chunks_project_idx  ON the_one.chunks (project_id);
CREATE INDEX IF NOT EXISTS chunks_source_idx   ON the_one.chunks (project_id, source_path);
CREATE INDEX IF NOT EXISTS chunks_palace_idx   ON the_one.chunks (project_id, wing, hall, room);
```

**Problem:** sqlx migrations cannot take runtime parameters — you can't substitute `$DIMS` or `$m` / `$ef_construction` into a migration .sql file. There were two options; one is now locked in.

**Decision C — schema dim strategy → DECIDED: Option A, hardcode `dim=1024` in `0002_chunks_table.sql`** *(locked in by Michel before the fresh session started; no need to ask again)*

Rationale recap:
- `dim=1024` matches the default quality-tier embedding provider (BGE-large-en-v1.5), which is the configured default for new installs.
- Changing the embedding provider tier means re-ingesting every chunk anyway (different vector space, different semantics) — so dim IS a schema-migration-level decision, not a runtime parameter.
- Phase 4 (combined Postgres+pgvector) will want migration-managed schemas for transactional consistency between state writes and vector writes. Starting Phase 2 with migration tracking means Phase 4 doesn't have to retrofit it.
- `sqlx::migrate!` creates its own `_sqlx_migrations` tracking table — no hand-rolled migration infrastructure needed.

Implementation constraint this locks in:

```rust
// In PgVectorBackend::new, after sqlx::migrate!().run() returns:
let provider_dim = embedding_provider.dimensions();
const MIGRATED_DIM: i32 = 1024;  // hardcoded to match 0002_chunks_table.sql
if provider_dim != MIGRATED_DIM as usize {
    return Err(format!(
        "pgvector schema was migrated with dim={MIGRATED_DIM}; active embedding provider \
         reports dim={provider_dim}; recreate the pgvector schema against a matching \
         provider (quality tier = BGE-large-en-v1.5 at 1024d) or switch providers to match"
    ));
}
```

The rejected Option B was: skip `sqlx::migrate!` for the chunks table and use runtime-substituted `CREATE TABLE IF NOT EXISTS` with dim from the provider. More flexible on dim, but loses migration tracking on the hottest table and forces Phase 4 to bolt on migration infrastructure retroactively.

Same pattern for `0003_entities_table.sql`, `0004_relations_table.sql`, `0005_images_table.sql` — each hardcoded to `dim=1024`, each mirroring the chunks shape for its own vector type. If a future phase needs to support multiple dim configurations, it'll do so via a new migration (`0006_reshape_chunks_dim.sql`) not via runtime parameterization.

**Hybrid search decision point** (unchanged from original prompt, still requires a brainstorming pass before committing):

```
-- Hybrid search: combine HNSW on dense_vector + GIN on a computed
-- tsvector from content for sparse-like recall, OR implement sparse
-- via the sparse_vector_indices/sparse_vector_values arrays with
-- a manual inner-product rewrite. Ask me which approach before
-- committing to hybrid semantics — there's a real trade-off.
```

**Extension preconditions — `preflight_vector_extension` must exist.**

The naive `CREATE EXTENSION IF NOT EXISTS vector` inside migration `0001` works on Supabase (built-in), fails with an opaque error on AWS RDS unless the `vector` extension is in the parameter group's `shared_preload_libraries` AND the connecting role is `rds_superuser`, fails similarly on GCP Cloud SQL, and fails on self-hosted Postgres unless the connecting role has `CREATE` on the target database. A fresh session that ships Phase 2 without a preflight check will produce support tickets from every managed-Postgres user on day one.

Implement `preflight_vector_extension(&pool)` to run BEFORE `sqlx::migrate!`:

```rust
async fn preflight_vector_extension(pool: &sqlx::PgPool) -> Result<(), String> {
    // 1. Check if the extension is already installed (Supabase path).
    let installed: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("preflight vector extension check: {e}"))?;
    if installed {
        return Ok(());
    }

    // 2. Not installed. Check if it's AVAILABLE to install (i.e. the
    //    extension files are on disk but not yet CREATE EXTENSIONed).
    let available: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector')")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("preflight vector extension availability: {e}"))?;
    if !available {
        return Err(
            "pgvector backend requires the `vector` extension, which is not installed \
             on this Postgres instance and not available for installation. Install it \
             first:\n\
               - AWS RDS / Aurora Postgres: enable `vector` in the instance parameter group's \
                 shared_preload_libraries, reboot the instance, then connect as rds_superuser.\n\
               - Google Cloud SQL Postgres: set the `cloudsql.enable_pgvector` database flag.\n\
               - Azure Database for PostgreSQL Flexible Server: enable `vector` in the \
                 server parameter `azure.extensions`.\n\
               - Supabase: pgvector is pre-installed, no action required.\n\
               - Self-hosted Postgres: install the pgvector package for your distribution \
                 (`apt install postgresql-16-pgvector` or build from source), then restart.".to_string()
        );
    }

    // 3. Available but not yet installed. Try to CREATE EXTENSION.
    //    This requires either superuser or CREATE privilege on the database.
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await
        .map_err(|e| format!(
            "pgvector extension is available but CREATE EXTENSION failed: {e}. \
             The connecting role needs CREATE privilege on this database, or \
             you need to connect as a superuser once to install it. On AWS RDS, \
             connect as rds_superuser. On Supabase, use the service_role connection."
        ))?;
    Ok(())
}
```

This is defensive but not over-engineered — three short queries and one targeted error per path, no silent fallbacks.

**Critical implementation notes:**

- `<DIMS>` must come from `embedding_provider.dimensions()` at construction time OR be hardcoded per Option A above. Either way, store the applied dim somewhere queryable (either the migration version itself or a `the_one.metadata` table) and refuse to load if a later provider reports a different value — `InvalidProjectConfig` with both dims in the message.
- Batched upserts via multi-row `INSERT ... ON CONFLICT (id) DO UPDATE SET ...`. No N-query loops.
- `pgvector::Vector` wraps `Vec<f32>` and implements `sqlx::Type<Postgres>` when the `sqlx` feature is on. Use it for bind parameters.
- HNSW parameters (`m`, `ef_construction`, `ef_search`) come from `[vector.pgvector]` in `config.toml` with the defaults `m=16`, `ef_construction=100`, `ef_search=40` (per § 4 of the backend selection scheme in the Phase 1 resume plan). **Note:** these can only be applied to NEW migrations, not retroactively to existing HNSW indexes, because Option A hardcodes them in the migration SQL. To tune HNSW on an existing install, operators run `DROP INDEX chunks_dense_hnsw; CREATE INDEX chunks_dense_hnsw ... WITH (m = X, ef_construction = Y);` manually — documented in the guide.

### 4. Env var parser + startup validator — `the_one_core::config::backend_selection`

**New submodule of `the_one_core::config`.** The plan says "new submodule" — decide where it lives: either `crates/the-one-core/src/config/backend_selection.rs` alongside the existing `config.rs`, or inline in `config.rs`. Prefer the new file for reviewability.

```rust
pub enum StateTypeChoice {
    Sqlite,           // THE_ONE_STATE_TYPE unset or = "sqlite"
    Postgres,         // = "postgres"
    Redis,            // = "redis"  (Phase 5 — parser accepts now, factory refuses until Phase 5)
    PostgresCombined, // = "postgres-combined"
    RedisCombined,    // = "redis-combined"
}

pub enum VectorTypeChoice {
    Qdrant,           // default
    Pgvector,         // = "pgvector"
    RedisVectors,     // = "redis-vectors"  (already exists via redis_vectors.rs feature)
    PostgresCombined,
    RedisCombined,
}

pub struct BackendSelection {
    pub state: StateTypeChoice,
    pub vector: VectorTypeChoice,
    pub state_url: Option<String>,
    pub vector_url: Option<String>,
}

impl BackendSelection {
    /// Parse from process env vars. Enforces ALL rules in § 3 of the
    /// Phase 1 resume plan's § Backend selection scheme.
    pub fn from_env() -> Result<Self, CoreError> { ... }
}
```

**Validation rules that MUST fire at parser time (every one has a negative test):**

| Input state | Expected outcome |
|---|---|
| All four env vars unset | `Sqlite + Qdrant` default |
| One TYPE set, other TYPE unset | `InvalidProjectConfig("THE_ONE_STATE_TYPE=postgres set but THE_ONE_VECTOR_TYPE is unset; both axes must be explicit when either is overridden.")` |
| TYPE set, matching URL unset | `InvalidProjectConfig("THE_ONE_STATE_TYPE=postgres requires THE_ONE_STATE_URL to be set.")` |
| Unknown TYPE value | `InvalidProjectConfig("Unknown THE_ONE_STATE_TYPE=pgsql; expected one of: sqlite, postgres, redis, postgres-combined, redis-combined")` |
| `postgres-combined` on one axis, mismatch on other | `InvalidProjectConfig("Combined backends must match: THE_ONE_STATE_TYPE=postgres-combined requires THE_ONE_VECTOR_TYPE=postgres-combined")` |
| Both `*-combined`, mismatched URLs | `InvalidProjectConfig("Combined THE_ONE_STATE_URL and THE_ONE_VECTOR_URL must be byte-identical; got <url_a> vs <url_b>")` |
| Both non-combined, same URL (split pools sharing a host) | **Allowed, silent** (operator wants split pools on one host) |
| One side `postgres-combined`, other `redis-combined` | **Fail loud** (different combined techs) |

These errors are `InvalidProjectConfig`, which the v0.15.0 error sanitizer passes through verbatim — the operator sees the full message, the `corr=<id>` lands in logs.

**Multi-error reporting order — fail on the first mismatch, in deterministic parse order.** If an operator sets both a bad `TYPE` and a missing `URL`, they see ONE error, not a collected list. The parse order is:

1. `THE_ONE_STATE_TYPE` — validate as known enum value (or "unset")
2. `THE_ONE_STATE_URL` — require iff `STATE_TYPE != unset && STATE_TYPE != sqlite`
3. `THE_ONE_VECTOR_TYPE` — validate as known enum value (or "unset")
4. `THE_ONE_VECTOR_URL` — require iff `VECTOR_TYPE != unset && VECTOR_TYPE != qdrant`
5. Cross-axis asymmetry — one TYPE set, other unset → fail
6. Combined matching — if either TYPE ends in `-combined`, both must be identical value
7. Combined URL equality — if both TYPEs are `*-combined`, URLs must be byte-identical

**Why first-match not collect-all:** collecting multiple validation errors sounds friendlier but (a) obscures the root cause when one error cascades (e.g. unknown TYPE makes the URL check meaningless), (b) adds test surface for every error-combination permutation, and (c) breaks the v0.15.0 "one `corr=<id>` per error" log invariant. First-match keeps the error envelope sane and matches the fail-fast philosophy of the rest of the backend selection scheme. Document the parse order in the function's doc comment so operators understand what they'll see.

**Test isolation for validator tests — use `temp_env::with_vars`.** Every negative test must wrap its env var mutation in `temp_env::with_vars([...], || { ... })` so parallel `cargo test` runs don't poison each other. There are already working examples in `crates/the-one-core/src/config.rs` tests — grep for `temp_env` there and follow the same pattern. Do NOT use `std::env::set_var` directly in tests, ever — it breaks other tests running in the same process.

### 5. Config.toml section parser for `[vector.pgvector]`

Add to the existing `config.rs` structure:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct VectorPgvectorConfig {
    // ── Schema / HNSW tuning ──────────────────────────────────────────
    #[serde(default = "default_pgvector_schema")]
    pub schema: String,              // "the_one"

    #[serde(default = "default_hnsw_m")]
    pub hnsw_m: i32,                 // 16 — HNSW graph connectivity

    #[serde(default = "default_hnsw_ef_construction")]
    pub hnsw_ef_construction: i32,   // 100 — HNSW build-time quality

    #[serde(default = "default_hnsw_ef_search")]
    pub hnsw_ef_search: i32,         // 40 — HNSW query-time recall

    // ── sqlx pool sizing ──────────────────────────────────────────────
    //
    // These are first-class config fields, not docs-only guidance. sqlx
    // defaults are 10 max connections + 30s acquire timeout + no idle
    // or max lifetime bounds — defensible for dev, wrong for production.
    // Phase 2's first integration test under CI load will discover the
    // defaults are wrong, so name them here.
    #[serde(default = "default_pgvector_max_connections")]
    pub max_connections: u32,        // 10

    #[serde(default = "default_pgvector_min_connections")]
    pub min_connections: u32,        // 2 — prevents cold-start latency spikes on first query

    #[serde(default = "default_pgvector_acquire_timeout_ms")]
    pub acquire_timeout_ms: u64,     // 30_000

    #[serde(default = "default_pgvector_idle_timeout_ms")]
    pub idle_timeout_ms: u64,        // 600_000 (10 min)

    #[serde(default = "default_pgvector_max_lifetime_ms")]
    pub max_lifetime_ms: u64,        // 1_800_000 (30 min, forces periodic reconnect)
}
```

All pool fields are applied via `sqlx::postgres::PgPoolOptions` in `PgVectorBackend::new` (see the example in § 3). The defaults are production-sane but operators can override in `config.toml` without a code change.

**Why `min_connections = 2` not 0:** sqlx's default 0 means the pool is empty until the first query, which forces a cold TCP + TLS + auth handshake on the critical path of whatever broker handler runs first after a restart. With `min_connections = 2`, two connections are established at pool construction and stay warm. This matters most on managed Postgres where TLS handshake can add 100–300 ms.

**Why `max_lifetime_ms = 30 min` not unlimited:** forces periodic reconnection to pick up credential rotation (IAM auth on AWS RDS, dynamic secrets from Vault, etc.) and to recover from network-level connection state that sqlx doesn't see (PGBouncer pool bounces, load-balancer reshards). 30 min is a production-default compromise between reconnect overhead and recovery latency.

`{project_id}` substitution happens in `AppConfig::load()` via literal `.replace("{project_id}", project_id)` — no Jinja, no expressions, no escape hatches. If `THE_ONE_PROJECT_ID` is unset when a templated field is active, startup fails with an explicit error.

### 6. Broker factory branch

In `crates/the-one-mcp/src/broker.rs::McpBroker::state_store_factory` (or a new companion `vector_backend_factory` — decide which based on whether the same method should handle both axes), add the Phase 2 branches:

```rust
// Pseudocode — adapt to the real method shape.
match backend_selection.vector {
    VectorTypeChoice::Qdrant => {
        // existing path — unchanged
    }
    VectorTypeChoice::Pgvector => {
        #[cfg(not(feature = "pg-vectors"))]
        return Err(CoreError::InvalidProjectConfig(
            "THE_ONE_VECTOR_TYPE=pgvector requires the `pg-vectors` Cargo feature".into()
        ));
        #[cfg(feature = "pg-vectors")]
        {
            let url = backend_selection.vector_url.as_ref().ok_or_else(|| ...)?;
            Box::new(PgVectorBackend::new(&config.vector.pgvector, url, project_id).await?)
        }
    }
    ...
}
```

**Parse `BackendSelection` once at broker construction** (not per call) and stash it on `McpBroker`. The plan's "forward-proofing `&self` on factory methods" from Phase 1 was exactly for this — you're now using it.

### 7. Integration tests

**Gated on the unified env surface** (NO `_TEST`-suffixed shadow vars — see § 1 of the backend selection scheme):

```rust
#[tokio::test]
async fn pgvector_roundtrip() {
    let Some(url) = matching_env("pgvector") else { return };  // skip gracefully
    // ... full CRUD + hybrid search round trip ...
}

fn matching_env(expected: &str) -> Option<String> {
    if std::env::var("THE_ONE_VECTOR_TYPE").ok().as_deref() == Some(expected) {
        std::env::var("THE_ONE_VECTOR_URL").ok()
    } else {
        None
    }
}
```

Test harness reads the **same** `THE_ONE_VECTOR_TYPE` / `THE_ONE_VECTOR_URL` that production would. Run with:

```bash
THE_ONE_VECTOR_TYPE=pgvector \
THE_ONE_VECTOR_URL=postgres://the_one:pw@localhost:5432/the_one_test \
THE_ONE_STATE_TYPE=sqlite \
cargo test -p the-one-memory --features pg-vectors pgvector_roundtrip
```

**Negative tests** for the env var parser (NO live DB required — these run in CI):

- `only_vector_type_set_fails_loud` — sets `THE_ONE_VECTOR_TYPE=pgvector` without state side, asserts exact error message
- `unknown_type_fails_with_enum_list` — sets `THE_ONE_VECTOR_TYPE=pgsql`, asserts error contains every valid enum value
- `type_without_url_fails` — sets type but no URL, asserts specific error
- `combined_mismatch_fails` — sets `state=postgres-combined`, `vector=qdrant`, asserts error
- `combined_url_mismatch_fails` — both `*-combined`, different URLs, asserts both URLs echoed in error
- `both_unset_defaults_silently` — all four unset, asserts `BackendSelection { Sqlite, Qdrant, None, None }`
- `both_non_combined_same_url_allowed` — split `postgres` + `pgvector` with same URL, asserts no error (split-pool on one host)
- `cross_combined_tech_fails` — `postgres-combined` + `redis-combined`, asserts fail-loud

These negative tests are the **critical sequencing point** from § 6 of the backend selection scheme: "Phase 2's first test MUST exercise the operator-set-only-one-side case, even though pgvector is the only new backend shipping. Getting validation right at introduction prevents a 'we'll tighten it later' regression loop."

Use `temp_env::with_vars` to isolate the env var mutations — parallel test runs will poison each other otherwise. See existing `config.rs` tests for the pattern.

### 8. Bench extension

Extend `crates/the-one-core/examples/production_hardening_bench.rs` to report pgvector throughput numbers when `THE_ONE_VECTOR_TYPE=pgvector` is set at bench time. Gate the pgvector path behind `#[cfg(feature = "pg-vectors")]` so the default bench still runs without pg deps.

Targets to record in the Phase 2 commit message body:

- Chunk upsert throughput (ops/sec, batch size)
- Hybrid search latency (p50, p95, p99) at 10k / 100k / 1M chunks
- HNSW build time vs IVFFlat (if both options are exposed)
- Compare to Qdrant baseline under identical query load

### 9. Docs

- **New section in `docs/guides/production-hardening-v0.15.md`** titled "§ 15 — pgvector setup, HNSW vs IVFFlat, index tuning". Cover:
  - How to install pgvector extension (`CREATE EXTENSION vector`)
  - HNSW parameter tuning guidance (m / ef_construction / ef_search trade-offs)
  - When to prefer IVFFlat over HNSW (dataset size, recall requirements)
  - Monitoring queries: `pg_stat_user_indexes`, `EXPLAIN ANALYZE` for vector search
  - Connection pool sizing (sqlx default vs production values)
- **Update `docs/guides/multi-backend-operations.md`**: add pgvector-specific subsection with config examples, the three HNSW tunables, and operator migration notes. The guide already has the four-var selection scheme — fold pgvector into the matrix.
- **Prepend Phase 2 entry to CHANGELOG.md `[Unreleased]`** in the same format as the Phase 1 entry (commit hash + tag + what shipped + test count delta).
- **Append Phase 2 bullet to CLAUDE.md conventions block** after the `Phase 1 broker state store cache (v0.16.0-phase1)` bullet.
- **Mark Phase 2 DONE in `docs/plans/2026-04-11-resume-phase1-onwards.md`** (mirror the Phase 0 and Phase 1 DONE headers).

### 10. Release artifact checks (before commit authorization)

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features pg-vectors -- -D warnings   # feature-gated compile check
cargo test --workspace                                                         # baseline path
cargo test --workspace --features pg-vectors                                   # with feature on
bash scripts/release-gate.sh
cargo build --release -p the-one-mcp --bin the-one-mcp
cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-vectors
```

All seven must pass. Record the passing count; it must be ≥ 450 + the Phase 2 new tests.

## Phase 2 STOP conditions

STOP and ask me rather than working around any of these:

- **sqlx version incompatibility** with our existing tokio version. Check first: `cargo tree -p the-one-memory --features pg-vectors | grep -E 'tokio|sqlx'`. If sqlx pulls a different tokio major, STOP.
- **rustls version conflict in the workspace.** Run `cargo tree | grep rustls` BEFORE adding the sqlx dep. If an existing dep pulls a conflicting `rustls` major, STOP and report — Decision A (`tls-rustls`) assumes no pre-existing conflict; if one exists, the fix might be a workspace-wide alignment rather than proceeding.
- **Provider dim ≠ migrated dim.** Decision C locks `dim=1024` (BGE-large-en-v1.5 quality tier) into the migration SQL. If the active embedding provider reports anything other than 1024, this is EXPECTED FAIL behavior per § 3 — surface the `InvalidProjectConfig` error exactly as shown, do NOT work around it by making the dim runtime-configurable. If Michel later wants multi-dim support, that's a new migration (`0006_reshape_*`) in a follow-up phase, not a Phase 2 scope change.
- **HNSW vs IVFFlat decision point**. The plan mentions both. My default is HNSW for recall at small-to-medium scale, IVFFlat for >10M chunks. If you see benchmark numbers that contradict this, STOP and report before committing to one.
- **Hybrid search semantics (Decision D — DEFERRED).** Write the dense-only path through §§ 1–6 first. When you reach the hybrid implementation inside § 3's `impl VectorBackend` body, STOP and bring Michel benchmark numbers for both options (α: tsvector+GIN, β: sparse arrays + manual inner-product rewrite) before committing to one.
- **Test harness flakiness** if the Postgres instance isn't available. Tests MUST skip gracefully via `return` when `matching_env` yields `None` — never a `.expect()` or `.unwrap()` on env presence.
- **Any unexpected dependency conflict** in `Cargo.lock`. Don't delete `Cargo.lock`. Report and ask.

## After Phase 2 ships

Per the plan's discipline:

1. Commit with `feat(memory): v0.16.0 — pgvector VectorBackend + env var parser + startup validator`
2. Ask for authorization BEFORE running `git commit`
3. Create annotated tag `v0.16.0-phase2` mirroring the Phase 1 bookmark convention
4. Push `origin main` and `origin v0.16.0-phase2`
5. Run `cargo build --release` as the final artifact
6. Report test count delta, benchmark numbers, and build result to me
7. **STOP** and wait for "continue" before starting Phase 3 (PostgresStateStore)

Phase 2 is one of the larger phases — ~800 LOC plus the docs + benches + negative tests. Do NOT bundle Phase 3 into the same commit. Keep the per-phase discipline the plan established.

---

## First action when the fresh session starts

1. Verify baseline per the block above (`git log`, `git status`, `cargo fmt/clippy/test`, release gate). Expected HEAD is at or after the commit tagged `v0.16.0-phase2-ready`; expected test baseline is **450 passing, 1 ignored**.
2. Record the baseline count into `/tmp/the-one-baseline.txt`.
3. **Run `cargo tree | grep rustls`** — this is the one pre-code check that might still force a STOP. If an existing workspace dep pulls a conflicting `rustls` major version, STOP and report. Otherwise proceed.
4. Read the § Phase 2 full deliverables above in order — **in particular, do not skim § 3**, which has the migration strategy, the extension preflight, and the dim hardcoded to 1024 that everything downstream depends on.
5. **Decisions A, B, C are already locked in** (see DECIDED markers in § 1 and § 3). Do NOT ask about them — proceed directly with:
   - **A:** sqlx `tls-rustls`
   - **B:** sqlx features `["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"]`
   - **C:** `sqlx::migrate!` with hardcoded `dim=1024` in `0002_chunks_table.sql` + matching provider-dim check in `PgVectorBackend::new`
6. **One decision remains deferred — D (hybrid search semantics).** You do NOT need to resolve D before starting work on §§ 1–6. Write the dense-only path first (§§ 1–6 cover workspace deps, feature flag, module scaffold, validator, config, factory branch — none of which depend on hybrid semantics). When you reach the hybrid implementation inside § 3's `impl VectorBackend` body, STOP and bring me benchmark numbers for both options (α: tsvector+GIN, β: sparse arrays + manual inner-product rewrite) before committing to one.
7. Proceed through §§ 1–10 in order, running the release artifact checks in § 10 before asking for commit authorization.

Phase 0 and Phase 1 are shipped. Decisions A/B/C are locked in. Begin at Phase 2.
