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
    "runtime-tokio", "postgres", "macros", "migrate", "chrono", "uuid"
] }
pgvector = { version = "0.4", features = ["sqlx"] }
```

**Decision to verify with me before adding:** exact `sqlx` feature set. The plan lists `["runtime-tokio", "postgres", "macros"]` minimum; I may want to add `migrate` (for built-in migration runner), `chrono` (for `TIMESTAMPTZ` handling), and `uuid` (for project identifiers if we store them as `UUID` vs `TEXT`). Ask me to confirm the exact feature list before editing `Cargo.toml`. Default to the plan minimum if I'm not available.

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

**Schema bootstrap** (runs on construction, idempotent):

```sql
CREATE EXTENSION IF NOT EXISTS vector;
CREATE SCHEMA IF NOT EXISTS <schema>;

CREATE TABLE IF NOT EXISTS <schema>.chunks (
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
    dense_vector vector(<DIMS>) NOT NULL,
    sparse_vector_indices INTEGER[],
    sparse_vector_values REAL[],
    created_at_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS chunks_dense_hnsw
    ON <schema>.chunks USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = <hnsw_m>, ef_construction = <hnsw_ef_construction>);

CREATE INDEX IF NOT EXISTS chunks_project_idx ON <schema>.chunks (project_id);
CREATE INDEX IF NOT EXISTS chunks_source_idx ON <schema>.chunks (project_id, source_path);
CREATE INDEX IF NOT EXISTS chunks_palace_idx ON <schema>.chunks (project_id, wing, hall, room);

-- Entities, relations, images tables mirror the same pattern.
-- Hybrid search: combine HNSW on dense_vector + GIN on a computed
-- tsvector from content for sparse-like recall, OR implement sparse
-- via the `sparse_vector_indices`/`sparse_vector_values` arrays with
-- a manual inner-product rewrite. Ask me which approach before
-- committing to hybrid semantics — there's a real trade-off.
```

**Critical implementation notes:**

- `<DIMS>` must come from `embedding_provider.dimensions()` at construction time, not hardcoded. Different Nomic/BGE tiers have different dims (384, 512, 768, 1024…). Store it once at bootstrap; refuse to load if a later provider reports a different value (return `InvalidProjectConfig` with both dims in the message).
- Batched upserts via multi-row `INSERT ... ON CONFLICT (id) DO UPDATE SET ...`. No N-query loops.
- `pgvector::Vector` wraps `Vec<f32>` and implements `sqlx::Type<Postgres>` when the `sqlx` feature is on. Use it for bind parameters.
- HNSW parameters (`m`, `ef_construction`, `ef_search`) come from `[vector.pgvector]` in `config.toml` with the defaults `m=16`, `ef_construction=100`, `ef_search=40` (per § 4 of the backend selection scheme in the Phase 1 resume plan).

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

### 5. Config.toml section parser for `[vector.pgvector]`

Add to the existing `config.rs` structure:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct VectorPgvectorConfig {
    #[serde(default = "default_pgvector_schema")]
    pub schema: String,              // "the_one"
    #[serde(default = "default_hnsw_m")]
    pub hnsw_m: i32,                 // 16
    #[serde(default = "default_hnsw_ef_construction")]
    pub hnsw_ef_construction: i32,   // 100
    #[serde(default = "default_hnsw_ef_search")]
    pub hnsw_ef_search: i32,         // 40
}
```

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
- **pgvector crate dimension mismatch** with the active embedding provider. If the provider reports 768 dims but the bootstrap hardcodes 384, STOP and ask me how to plumb the dim through config.
- **HNSW vs IVFFlat decision point**. The plan mentions both. My default is HNSW for recall at small-to-medium scale, IVFFlat for >10M chunks. If you see benchmark numbers that contradict this, STOP and report before committing to one.
- **Hybrid search semantics**. Pgvector supports dense HNSW natively but sparse is a manual implementation. The plan mentions "Batched upserts via multi-row INSERT" but doesn't pin hybrid semantics. Ask before committing.
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

1. Verify baseline per the block above (`git log`, `git status`, `cargo fmt/clippy/test`, release gate)
2. Record the baseline count into `/tmp/the-one-baseline.txt`
3. Read the § Phase 2 full deliverables above in order
4. Stop and ask me: **which sqlx feature set to use** (the minimum `["runtime-tokio", "postgres", "macros"]` or the extended `["runtime-tokio", "postgres", "macros", "migrate", "chrono", "uuid"]`)
5. Wait for my "continue" before adding any dependency to `Cargo.toml`

Phase 0 and Phase 1 are shipped. Begin at Phase 2.
