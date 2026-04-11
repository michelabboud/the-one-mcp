# Resume prompt for Phase 4 — combined Postgres+pgvector backend

**Date authored:** 2026-04-11
**Baseline commit:** `f010ed6` (v0.16.0 Phase 3 — `PostgresStateStore` impl)
**Baseline tag:** `v0.16.0-phase3`
**Target:** execute **Phase 4** of the multi-backend roadmap
**Status:** fresh session ready — paste this prompt or say "read `docs/plans/2026-04-11-resume-phase4-prompt.md`"

---

## Who I am (Michel)

You're continuing a multi-session refactor that will land full
multi-backend support for both vectors and state in `the-one-mcp`.
Phases 0 through 3 are already on `main`:

- **Phase 0** (`5ff9872`, `v0.16.0-rc1`) — bundled trait extraction
  (`VectorBackend`, `StateStore`), production hardening v0.15.0, Lever 1 audit speedup.
- **Phase 1** (`7666439`, `v0.16.0-phase1`) — broker `state_by_project`
  cache + `with_state_store` sync-closure chokepoint.
- **Phase 2** (`91ff224`, `v0.16.0-phase2`) — pgvector `VectorBackend`
  + four-var env parser (`THE_ONE_{STATE,VECTOR}_{TYPE,URL}`) + startup
  validator.
- **Phase 3** (`f010ed6`, `v0.16.0-phase3`) — `PostgresStateStore`
  implementing every `StateStore` trait method via
  `tokio::task::block_in_place` sync-over-async bridge, FTS5→tsvector
  translation, hand-rolled migration runner.

Phase 4 ships the **first combined single-pool backend**:
`PostgresCombinedBackend`. One `sqlx::PgPool` serving both
`StateStore` AND `VectorBackend` trait roles for transactional
consistency between state writes and vector writes. Activated via
`THE_ONE_STATE_TYPE=postgres-combined` + `THE_ONE_VECTOR_TYPE=postgres-combined`
with byte-identical URLs. The backend-selection parser already
validates this; the current `state_store_factory` branch returns
`NotEnabled` and Phase 4 swaps it for a real construction path.

## Non-negotiable global rules

- Do what I say, full production grade, no shortcuts, no stubs, no
  placeholders, no "good enough". Phase 4 ships complete or doesn't ship.
- NEVER defer, skip, or descope anything without explicit approval.
- NEVER bump `Cargo.toml` version (stays `"0.1.0"`). Real versioning
  is in git tags + commit subjects + `CHANGELOG.md`.
- NEVER commit anything without explicit authorisation.
- If a spec is ambiguous, ASK — don't pick the minimal interpretation.
- **chrono stays OUT.** Phase 2 and Phase 3 deliberately dropped
  `chrono` from sqlx features. Phase 4 inherits that decision — use
  `BIGINT epoch_ms` everywhere. Do not plan chrono work.
- When Phase 4 is fully complete (design + impl + tests + docs +
  committed + pushed + built), suggest `/compact` or `/clear` before
  Phase 5.

## Read FIRST (in this exact order)

1. `CLAUDE.md` — project conventions. Read the Phase 2 and Phase 3
   bullets in particular.
2. `docs/plans/2026-04-11-multi-backend-architecture.md` § 4.3 —
   the "combined adapter" design sketch for the trait composition.
3. `docs/plans/2026-04-11-resume-phase1-onwards.md` § Phase 4 — the
   canonical deliverable list (~300 LOC, one `sqlx::PgPool` serving
   both trait roles, new `postgres_combined.rs` file). Phase 2 and
   Phase 3 DONE blocks in this file document the adjustments that
   landed vs the original plan — cross-reference before executing.
4. `docs/guides/architecture.md` § Multi-Backend Architecture (v0.16.0+)
   — the complete Phase 0-3 architectural context (trait surface,
   broker cache, factory dispatcher).
5. `docs/guides/pgvector-backend.md` — Phase 2's operational surface
   + the `PgVectorBackend::new` construction path you'll be reusing.
6. `docs/guides/postgres-state-backend.md` — Phase 3's construction
   path and the sync-over-async bridge Phase 4 will inherit.
7. `crates/the-one-memory/src/pg_vector.rs` — Phase 2 backend
   implementation. `PgVectorBackend::new` takes a `&PgVectorConfig`,
   `url`, `project_id`, and `&dyn EmbeddingProvider`.
8. `crates/the-one-core/src/storage/postgres.rs` — Phase 3 state
   store implementation. `PostgresStateStore::new` takes a
   `&PostgresStateConfig`, `url`, and `project_id`.
9. `crates/the-one-mcp/src/broker.rs` — `state_store_factory`
   (currently branches on `StateTypeChoice::PostgresCombined` and
   returns `NotEnabled` — Phase 4 swaps this branch for real
   construction). `build_memory_engine` has the parallel vector-side
   pgvector fast-path that Phase 4 also needs to handle.
10. `crates/the-one-core/src/config/backend_selection.rs` — the env
    var parser already validates combined TYPE matching and
    byte-identical URL equality (rules 6 + 7 in
    `BackendSelection::from_env`). Phase 4 doesn't need any parser
    changes.

Then read **this file's § Phase 4 full deliverables** below.

## Baseline to verify before touching anything

```bash
git log --oneline -5
# Expected first line: "f010ed6 feat(core): v0.16.0 — PostgresStateStore impl"
git status
# Expected: clean

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features pg-state,pg-vectors -- -D warnings
cargo test --workspace 2>&1 | tee /tmp/phase4-baseline.log
grep "test result:" /tmp/phase4-baseline.log | awk '{p+=$4;f+=$6} END {print "BASELINE:",p,"passing,",f,"failing"}'
# Expected: 466 passing, 0 failing, 1 ignored

cargo test --workspace --features pg-state,pg-vectors 2>&1 | tee /tmp/phase4-baseline-features.log
grep "test result:" /tmp/phase4-baseline-features.log | awk '{p+=$4;f+=$6} END {print "BASELINE:",p,"passing,",f,"failing"}'
# Expected: 495 passing, 0 failing, 1 ignored
```

**If ANY of these fail, STOP and report.** A failing baseline means
environment drift, not a Phase 4 bug.

Record the baseline count into `/tmp/the-one-baseline.txt`. Phase 4
must end with monotonically-greater-than-466 passing (base) and
monotonically-greater-than-495 (features).

## § Phase 4 full deliverables

**Scope (from the execution plan):** ~300 LOC. Mostly dispatcher
wiring — the heavy lifting (pgvector backend, PostgresStateStore,
sync-over-async bridge, hand-rolled migration runner) is already
shipped in Phases 2 and 3.

**Commit message (exact):** `feat: v0.16.0 — combined Postgres+pgvector backend`

### 1. New combined adapter file

**Decision point**: the file location is ambiguous. Two reasonable
choices:

- **Option A**: `crates/the-one-core/src/backend/postgres_combined.rs`
  — a new `backend/` directory on core that will eventually hold
  `redis_combined.rs` (Phase 6) too.
- **Option B**: `crates/the-one-memory/src/postgres_combined.rs`
  next to `pg_vector.rs`.

Option A (new `backend/` module on core) is cleaner because the
combined adapter bridges a core trait (`StateStore`) and a memory
trait (`VectorBackend`). Putting it on core means core can depend on
memory's `VectorBackend` trait — **but that reverses the current
dep graph** (core → memory). Need to check whether the trait is
already re-exported or whether Phase 4 introduces a new core ⇐
memory edge.

**Check first**: `cargo tree -p the-one-core | head -20` — is memory
already in core's dep graph? If yes, Option A. If no, Option B is
forced.

The struct shape is the same either way:

```rust
#[cfg(all(feature = "pg-state", feature = "pg-vectors"))]
pub struct PostgresCombinedBackend {
    /// The shared pool. Both trait impls borrow this.
    pool: sqlx::PgPool,
    project_id: String,
    schema: String,
    // Per-axis config for the pool sizing / tuning that the pool
    // was constructed with. Stored for observability, not used at
    // runtime (the pool is already shaped).
    state_config: PostgresStateConfig,
    vector_config: PgVectorConfig,
}
```

### 2. Single pool, shared across trait roles

The core design: **one `sqlx::PgPool`**, with both trait roles
borrowing the same pool. The alternative (two pools pointing at the
same URL) defeats the whole point — transactional consistency
requires the state writes and vector writes to go through the same
connection, which means one pool.

Construction:

```rust
impl PostgresCombinedBackend {
    pub async fn new(
        state_config: &PostgresStateConfig,
        vector_config: &PgVectorConfig,
        url: &str,
        project_id: &str,
        embedding_provider: &dyn EmbeddingProvider,
    ) -> Result<Self, CoreError> {
        // 1. Build pool using the STATE config's sizing (arbitrary
        //    choice — state config typically has the higher max_connections
        //    because state operations are more frequent). Document this
        //    in the guide: "when combined, the STATE pool settings win."
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(state_config.max_connections)
            .min_connections(state_config.min_connections)
            // ... mirroring PostgresStateStore::new's setup
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query(&format!(
                        "SET statement_timeout = '{}ms'",
                        state_config.statement_timeout_ms
                    ))
                    .execute(conn)
                    .await
                    .map(|_| ())
                })
            })
            .connect(url)
            .await
            .map_err(|e| CoreError::Postgres(format!("combined pool: {e}")))?;

        // 2. Run BOTH migration runners on the same pool.
        //    They use distinct tracking tables
        //    (`state_migrations` + `pgvector_migrations`) so they
        //    coexist cleanly in one schema.
        the_one_memory::pg_vector::migrations::apply_all(&pool).await?;
        the_one_core::storage::postgres::migrations::apply_all(&pool).await?;

        // 3. pgvector's preflight still runs — pgvector extension
        //    must be available on the shared instance.
        preflight_vector_extension(&pool).await?;

        // 4. Dimension check for the vector side.
        let provider_dim = embedding_provider.dimensions();
        if provider_dim != 1024 {
            return Err(CoreError::InvalidProjectConfig(format!(
                "combined pgvector schema is fixed at dim=1024; provider dim={provider_dim}"
            )));
        }

        Ok(Self { pool, project_id: project_id.to_string(),
                  schema: state_config.schema.clone(),
                  state_config: state_config.clone(),
                  vector_config: vector_config.clone() })
    }
}
```

### 3. Trait implementations — delegate to the per-axis types

The combined backend **should NOT duplicate every trait method**.
Instead, it holds an internal `PostgresStateStore` and `PgVectorBackend`
(both constructed from the shared pool via new constructors) and
delegates. Or, more idiomatically: `PostgresCombinedBackend` stores
the `PgPool` directly and implements both traits against it, reusing
the query helpers from the Phase 2 and Phase 3 modules.

**Preferred pattern** (delegation via owned sub-backends):

```rust
pub struct PostgresCombinedBackend {
    state: PostgresStateStore,
    vector: PgVectorBackend,
    // ... or hold just the pool and inline both impls
}

impl StateStore for PostgresCombinedBackend {
    // Forward every method to self.state
    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError> {
        self.state.upsert_project_profile(profile_json)
    }
    // ... 25 more forwards
}

#[async_trait]
impl VectorBackend for PostgresCombinedBackend {
    // Forward every method to self.vector
    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String> {
        self.vector.upsert_chunks(points).await
    }
    // ... more forwards
}
```

**Key constraint**: `PostgresStateStore` and `PgVectorBackend` both
currently hold their own `PgPool` internally. For the combined
backend to share ONE pool, Phase 4 needs new "accepts existing pool"
constructors:

- `PostgresStateStore::from_pool(pool: PgPool, project_id: &str, schema: String) -> Self`
- `PgVectorBackend::from_pool(pool: PgPool, config: &PgVectorConfig, project_id: &str, embedding_provider: &dyn EmbeddingProvider) -> Result<Self, String>`

These constructors skip the `connect()` step (pool is already built)
and skip the preflight + migration runs (combined adapter runs those
once against the shared pool). They set all the per-struct state
directly.

### 4. Optional transaction API

**Decision point**: the plan mentions a new trait method
`StateStore::begin_combined_tx()` that returns a transaction handle
implementing both traits scoped to the transaction. **Do NOT
implement this in Phase 4 unless you find a concrete broker call
site that needs it.**

The simpler Phase 4 shape: the combined backend gives you the
operational benefit of ONE connection pool (credential rotation,
IAM auth, pgbouncer compatibility, PITR backup consistency) without
the API complexity of cross-trait transactions. Broker handlers
keep using `with_state_store` and `with_project_memory` separately;
the savings come from pool consolidation, not from new transactional
primitives.

If you find a specific handler where cross-trait atomicity is
load-bearing (e.g. `memory_ingest_conversation` writing
`conversation_sources` AND chunk vectors), flag it and ask me before
adding the trait method.

### 5. Broker factory wiring

In `broker.rs::state_store_factory`, replace the current
`StateTypeChoice::PostgresCombined => Err(NotEnabled(...))` branch
with a real construction:

```rust
StateTypeChoice::PostgresCombined => {
    self.construct_postgres_combined_state(project_id).await
}
```

And in `build_memory_engine`, add a parallel vector-side branch.
**Key subtlety**: the Phase 4 combined backend is ONE instance that
implements BOTH traits. The broker's state cache
(`state_by_project: RwLock<HashMap<String, Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>>>`)
and the memory cache (`memory_by_project`) currently hold DISTINCT
instances. Phase 4 needs to either:

- **Option X**: Construct ONE `PostgresCombinedBackend` and share an
  `Arc<PostgresCombinedBackend>` between both caches. This requires
  the backend to implement `StateStore + Send` AND
  `VectorBackend + Send + Sync`. The state cache wraps in
  `std::sync::Mutex` so `!Sync` is fine there; the memory cache
  holds a `MemoryEngine` which owns a `Box<dyn VectorBackend>` —
  that's a box, not a borrow, so the memory engine can own ITS
  reference to the combined backend separately.
- **Option Y**: Construct TWO separate thin adapters
  (`CombinedStateAdapter` and `CombinedVectorAdapter`) that each
  hold an `Arc<sqlx::PgPool>` and delegate to per-axis query logic.
  More code, fewer trait gymnastics.

**Option Y is simpler.** The combined adapter holds a shared
`Arc<PgPool>` and exposes two `impl` blocks; the broker constructs
both adapters from the same pool at factory time. Each cache holds
its own adapter instance. Pool construction + migration runs happen
ONCE in a cached path (e.g. a new
`pg_combined_pool_by_project: RwLock<HashMap<String, Arc<PgPool>>>`
field on `McpBroker`).

**Think through this carefully and ask me before committing to X or
Y.** This is the main architectural choice of Phase 4.

### 6. Config

**NO new config section.** Phase 4 explicitly **reuses** the
existing `vector_pgvector` and `state_postgres` blocks. The plan's
§ 4 note on this: "Combined backends reuse existing sections.
`THE_ONE_STATE_TYPE=postgres-combined` reads from `[state.postgres]`
AND `[vector.pgvector]`. There is NO `[combined.postgres]` section.
The combined adapter is a dispatch optimization, not a new config
surface."

When combined, the state config's pool-sizing wins (it typically has
higher `max_connections`). Document this explicitly in the guide.

### 7. Integration tests

`crates/the-one-core/tests/postgres_combined_roundtrip.rs` gated on
both features AND both env vars:

```rust
#![cfg(all(feature = "pg-state", feature = "pg-vectors"))]

fn matching_env() -> Option<String> {
    if std::env::var("THE_ONE_STATE_TYPE").ok().as_deref() != Some("postgres-combined") {
        return None;
    }
    if std::env::var("THE_ONE_VECTOR_TYPE").ok().as_deref() != Some("postgres-combined") {
        return None;
    }
    let state_url = std::env::var("THE_ONE_STATE_URL").ok()?;
    let vector_url = std::env::var("THE_ONE_VECTOR_URL").ok()?;
    if state_url != vector_url {
        return None;  // should have failed at parser time
    }
    Some(state_url)
}
```

Required tests:

- `combined_bootstrap` — both migration runners apply, both
  tracking tables populated, both preflights pass
- `combined_state_and_vector_share_pool` — assert there's ONE pool
  (observable via connection count / pool stats)
- `combined_state_write_then_vector_read` — insert a diary entry,
  then upsert chunks that reference it, then search
- `combined_cross_trait_consistency` — if Phase 4 ships a
  transaction primitive, test that rollback discards both state and
  vector writes atomically. If not, verify this is documented as a
  known gap.

### 8. Negative tests

These should already pass (the parser was built in Phase 2) but
Phase 4 reactivates them by adding a feature-gated module:

- `combined_state_without_vector_fails` — already covered by Phase 2
  parser tests
- `combined_url_mismatch_fails` — already covered
- `cross_combined_tech_fails` (`postgres-combined` + `redis-combined`)
  — already covered

Just verify these still pass with the new branch activated.

### 9. Docs

- **New standalone guide** `docs/guides/combined-postgres-backend.md`
  covering: when to pick combined over split, pool-sizing rationale
  (state config wins), cross-trait transaction limits (or absence),
  migration from split-pool Postgres, per-provider IAM auth notes.
- **Update `docs/guides/multi-backend-operations.md`**: flip the
  "Planned: Postgres + pgvector (combined)" subsection from planned
  to shipped. Add the combined deployment topology to the backend
  matrix.
- **Update `docs/guides/pgvector-backend.md` § 12 "Phase 4 combined
  preview"** and `docs/guides/postgres-state-backend.md § 11 "Phase 4
  combined preview"** — mark them as shipped and link to the new
  combined guide.
- **Update `docs/guides/configuration.md`**: add a note in the
  Multi-Backend Selection section that `postgres-combined` reuses
  the `vector_pgvector` and `state_postgres` blocks (no new section).
- **Update `docs/guides/architecture.md` § Cross-phase relationship**:
  mark Phase 4 row as shipped.
- **Prepend Phase 4 entry to CHANGELOG.md `[Unreleased]`** in the
  same format as Phases 2 and 3.
- **Append Phase 4 bullet to CLAUDE.md conventions block** after the
  Phase 3 bullet.
- **Mark Phase 4 DONE in `docs/plans/2026-04-11-resume-phase1-onwards.md`**
  with the same shipped-vs-planned diff format used for Phases 2
  and 3.
- **Update PROGRESS.md**: bump header to v0.16.0-phase4, add the
  per-version entry, rewrite "What's Next" to point at Phase 5.

### 10. Release artifact checks

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features pg-state,pg-vectors -- -D warnings
cargo test --workspace                                     # baseline path
cargo test --workspace --features pg-state,pg-vectors      # with both features
bash scripts/release-gate.sh
cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-state,pg-vectors
```

All seven must pass. Record the passing count; it must be ≥ 466
(base) and ≥ 495 (features).

## Phase 4 STOP conditions

STOP and ask me rather than working around any of these:

- **Pool sharing architectural choice (Option X vs Option Y above).**
  Bring a one-line recommendation with rationale and ask me to
  approve before committing to either.
- **Trait method `begin_combined_tx`** — don't add this unless you
  find a concrete call site that needs it. Flag the call site and
  ask.
- **Core depending on memory** — if Option A for file location is
  blocked by the dep graph, say so and default to Option B.
- **Migration runner collision** — the two tracking tables are
  distinct by design (`pgvector_migrations` vs `state_migrations`).
  If you find yourself wanting to merge them, stop and ask.
- **Any sqlx version incompatibility** surfaced by the new combined
  code path. Run `cargo tree -p the-one-core --features pg-state |
  grep sqlx` BEFORE assuming the dep graph is unchanged.

## After Phase 4 ships

Per the plan's discipline:

1. Commit with `feat: v0.16.0 — combined Postgres+pgvector backend`
2. Ask for authorization BEFORE running `git commit`
3. Create annotated tag `v0.16.0-phase4` mirroring the
   Phases 1–3 bookmark convention
4. Push `origin main` and `origin v0.16.0-phase4`
5. Run `cargo build --release` with `--features pg-state,pg-vectors`
   as the final artifact
6. Report test count delta and build result to me
7. **STOP** and wait for "continue" before starting Phase 5 (Redis
   state store with three modes)
8. Update memory: mark Phase 4 complete in session_todos.md,
   update project_overview.md commit + tag references

Phase 4 is one of the smaller phases — ~300 LOC plus docs + one new
guide + integration tests. Do NOT bundle Phase 5 into the same
commit. Keep the per-phase discipline the plan established.

---

## First action when the fresh session starts

1. Verify baseline per the block above (`git log`, `git status`,
   `cargo fmt/clippy/test`, release gate). Expected HEAD is at or
   after commit `f010ed6` tagged `v0.16.0-phase3`; expected test
   baseline is **466 passing, 1 ignored** (base) / **495 passing,
   1 ignored** (with `--features pg-state,pg-vectors`).
2. Record the baseline counts into `/tmp/the-one-baseline.txt`.
3. **Answer the Option X vs Option Y architectural question**
   (pool sharing pattern) before writing any Phase 4 code. Read the
   existing `broker.rs::build_memory_engine` pgvector fast-path and
   `state_store_factory` Postgres branch to understand the symmetry
   constraint, bring a one-line recommendation, and ask me to
   approve.
4. Read the § Phase 4 full deliverables above in order. § 3
   (trait implementations — delegate vs inline) and § 5 (broker
   factory wiring — Option X vs Y) are the main decision points.
5. Proceed through §§ 1–10 in order, running the release artifact
   checks in § 10 before asking for commit authorization.

Phase 0, 1, 2, and 3 are shipped. Begin at Phase 4.
