# Next-Steps Expansion — post-v0.15.1

**Date:** 2026-04-11
**Context:** v0.15.0 production hardening + v0.15.1 Lever 1 (audit
speedup) are complete in the working tree but uncommitted. The user
asked what the concrete next actions look like in detail; this
document is the answer, preserved so it can be reviewed and refined
without re-deriving from chat.

---

## Where we are right now

**Shipped but uncommitted:**

- v0.15.0 production hardening pass addressing every finding from the
  mempalace comparative audit (C1-C5, H1-H5, M1-M6).
- v0.15.1 Lever 1 audit-write speedup (`PRAGMA synchronous=NORMAL`,
  67× measured improvement).
- Lever 2 async-batching plan (draft + v2) — design only, no code.
- `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
- `docs/guides/production-hardening-v0.15.md`
- 23 new tests (13 production-hardening regression guards, 9 stdio
  integration tests, 1 lever 1 guard).
- 1 new benchmark (`production_hardening_bench.rs`).

**Working tree state:** 11 modified files + 9 untracked. ~3 000 LOC
of new code + docs. Validation is green (`cargo fmt --check`, clippy
clean, 446 tests passing).

**Not yet done:** git commit, CHANGELOG entry, MemPalace-off
operator docs, multi-backend vector storage (pgvector), MemoryEngine
trait refactor.

---

## Version convention in this repo

Checked before writing the plan. Two notes:

1. `Cargo.toml workspace.package.version = "0.1.0"` and **stays at
   0.1.0 across releases**. The last release was
   `53ee1b4 release: v0.14.3 mempalace phase2 production` and
   Cargo.toml still reads 0.1.0 — so the manifest is a placeholder
   and versioning lives elsewhere.
2. The real source of truth is:
   - git tag / commit message subject (`release: vX.Y.Z ...`)
   - `CHANGELOG.md` (Keep-a-Changelog format, last entry 0.14.3)
   - prose in `docs/guides/*` and `docs/reviews/*`

So we do NOT bump `Cargo.toml`. We add a `CHANGELOG.md` entry and
the commit subject carries the version.

---

## Step 1 — Commit + CHANGELOG + version tagging

### Inventory of changes

| bucket                | files / dirs                                  |
|-----------------------|-----------------------------------------------|
| Review / report       | `docs/reviews/2026-04-10-mempalace-comparative-audit.md` |
| New core modules      | `crates/the-one-core/src/audit.rs`            |
|                       | `crates/the-one-core/src/naming.rs`           |
|                       | `crates/the-one-core/src/pagination.rs`       |
| Core plumbing         | `crates/the-one-core/src/lib.rs` (module exports) |
|                       | `crates/the-one-core/Cargo.toml` (base64 dep) |
|                       | `crates/the-one-core/src/storage/sqlite.rs` (schema v7 + Lever 1) |
| Broker hardening      | `crates/the-one-mcp/src/broker.rs`            |
|                       | `crates/the-one-mcp/src/api.rs`               |
| Transport hardening   | `crates/the-one-mcp/src/transport/jsonrpc.rs` |
|                       | `crates/the-one-mcp/src/transport/stdio.rs`   |
|                       | `crates/the-one-mcp/src/transport/tools.rs`   |
| Integration tests     | `crates/the-one-mcp/tests/production_hardening.rs` |
|                       | `crates/the-one-mcp/tests/stdio_write_path.rs` |
| Benchmarks            | `crates/the-one-core/examples/production_hardening_bench.rs` |
| Docs                  | `docs/guides/production-hardening-v0.15.md`  |
|                       | `docs/guides/mempalace-operations.md` (updated) |
|                       | `CLAUDE.md` (updated)                        |
| Plans                 | `docs/plans/2026-04-10-audit-batching-lever2.md` |
|                       | `docs/plans/draft-2026-04-10-audit-batching-lever2.md` |
| Cargo.lock            | base64 + dashmap refresh                     |

### Commit slicing options

#### Option A — 1 monolithic commit

Subject:

```
feat: v0.15.1 production hardening pass + Lever 1 audit-write speedup
```

All 20 files in one shot. Pros: atomic, trivially revertable, 5
minutes to execute. Cons: `git log` tells a thin story; `git bisect`
narrows to "all of it"; the diff is ~3 000 LOC.

#### Option B — 3 logical commits (recommended)

1. **`feat: v0.15.0 production hardening pass (mempalace audit)`**
   — everything except the single Lever 1 pragma line and the two
   Lever 2 plan files.
2. **`feat: v0.15.1 lever 1 — synchronous=NORMAL for 67x audit throughput`**
   — just the one pragma line + its two regression tests + guide
   § 14 update + CHANGELOG entry.
3. **`docs: lever 2 audit batching plan (draft + v2)`** — both plan
   documents. Kept separate because plans are not shipped work.

Pros: git log mirrors the real development story; each commit passes
`cargo test` independently; Lever 1 is a clean isolable cherry-pick.
Cons: requires `git add -p` on `sqlite.rs` to split the file between
commits 1 and 2. ~15 minutes.

#### Option C — 7 granular commits

Maximum bisectability, ~35 minutes, commits 2-5 may have
intermediate states that don't individually build. Overkill for a
solo project.

### Recommendation

**Option B.** Tells the real story, each commit is independently
valid, Lever 1 isolable.

### Execution sequence for Option B

```
1.1  Sanity pass:
     cargo fmt --check
     cargo clippy --workspace --all-targets -- -D warnings
     cargo test --workspace

1.2  Prepend v0.15.0 and v0.15.1 entries to CHANGELOG.md following
     the existing Keep-a-Changelog format matching v0.14.3.

1.3  Commit 1 (v0.15.0):
     Stage everything EXCEPT:
       - the synchronous=NORMAL line in sqlite.rs
       - docs/plans/2026-04-10-audit-batching-lever2.md
       - docs/plans/draft-2026-04-10-audit-batching-lever2.md
     Requires `git add -p` on sqlite.rs to split the pragma block
     out of the v0.15.0 changes.

1.4  Commit 2 (v0.15.1 Lever 1):
     Stage the remaining sqlite.rs hunk + lever1 regression tests +
     hardening guide § 14 edit + CHANGELOG v0.15.1 entry.

1.5  Commit 3 (Lever 2 plans):
     Stage both plan files.

1.6  Verify:
     git log --oneline -3
     git status   (clean)
     cargo test --workspace   (final sanity)
```

### Push target

- **Direct push to `main`:** matches the repo's history. Recent
  commits (`v0.14.3`, `v0.14.1`, `v0.14.0`) all landed on main
  directly.
- **PR branch:** ~2 minutes extra; gives a GitHub diff view.

The direct-to-main convention is established; PR is optional.

### Estimated time

- Option A: ~8 minutes (including CHANGELOG + validation)
- Option B: ~15 minutes
- Option C: ~35 minutes

---

## Step 2 — D: MemPalace-off operator documentation

### Scope

Add a new section to `docs/guides/mempalace-operations.md` titled
**"Running the-one-mcp without MemPalace"**. Pure documentation; no
code changes.

### Deliverable contents

1. **Up-front answer:** MemPalace is opt-in at the subfeature level,
   doesn't degrade normal MCP operation, can be fully disabled.
2. **Full-disable config.json snippet:**
   ```json
   {
     "memory_palace_enabled": false,
     "memory_palace_hooks_enabled": false,
     "memory_palace_aaak_enabled": false,
     "memory_palace_diary_enabled": false,
     "memory_palace_navigation_enabled": false
   }
   ```
3. **Equivalent one-liner** via the `config` tool with
   `profile.set` → `"off"`.
4. **What still works when disabled:**
   - `memory.search` (minus wing/hall/room filters)
   - `docs.*`, `tool.*`, `config.*`, `maintain.*`
   - Every transport (stdio/SSE/stream)
5. **What returns `NotEnabled`:**
   - `memory.ingest_conversation`, `memory.wake_up`
   - `memory.diary.*`, `memory.aaak.*`, `memory.navigation.*`
   - `memory.capture_hook`
6. **Cost-when-disabled breakdown:**
   - Startup: zero extra
   - Memory: ~8 KB of empty SQLite tables per project
   - CPU: one bool check per gated endpoint
   - Background tasks: none spawned
7. **Cost-when-enabled-but-idle:** identical to disabled — features
   only execute when their RPC is called.
8. **What to check in an emergency degrade:** list the env vars and
   config keys that force-disable individual subfeatures.

### LOC / effort

- ~120 lines of markdown
- 0 lines of code
- ~5 minutes

### Risks

None. Pure documentation addition.

---

## Step 3 — C: Plan + implement Phase A (trait VectorBackend refactor)

This is the substantive step. Two sub-steps: plan document, then
implementation.

### 3a. Planning document

**Deliverable:** `docs/plans/2026-04-11-vector-backend-trait-refactor.md`

**Contents:**

1. **Problem statement.** Current `MemoryEngine` struct at
   `crates/the-one-memory/src/lib.rs:141` holds
   `qdrant: Option<AsyncQdrantBackend>` and
   `redis: Option<RedisVectorStore>`. Dispatch is a hand-written
   `if let Some(qdrant) … else if let Some(redis) …` chain repeated
   at 14+ sites in `lib.rs`. Every new operation added to the engine
   has to be added to every backend branch manually. There is no
   `trait VectorBackend`.
2. **Trait definition.** Every method the engine currently calls on
   either backend, with signatures matching the existing ones. ~8-10
   methods covering chunks, entities, relations, images, and hybrid
   search. Default impls return `Err(Unsupported)` for operations a
   backend doesn't support.
3. **Capability declaration.** `BackendCapabilities` struct
   (`chunks: bool`, `entities: bool`, `relations: bool`,
   `images: bool`, `hybrid: bool`, `name: &'static str`) so callers
   can detect what a backend supports without probing via
   `Err(Unsupported)`.
4. **Feature flag strategy.** `redis-vectors` feature still gates
   Redis impl compilation. When disabled, `Box<dyn VectorBackend>`
   is constructed without the Redis variant. pgvector will mirror
   this pattern later via `pg-vectors`.
5. **Migration plan.** Enumerate the dispatch sites, map each to a
   trait method, show the one-line-per-site replacement pattern.
6. **Test plan.** Goal: zero behavioural drift. Strategy: run the
   full existing 446-test suite before and after, require same
   pass count and same assertions. Add one new unit test
   (`test_backend_capabilities_reported_correctly`) that asserts
   Qdrant reports all-true and Redis reports chunks-only.
7. **Acceptance criteria.** `cargo fmt --check`, clippy clean,
   `cargo test --workspace` ✓, bench runs, stdio integration tests
   ✓, manual smoke test against a running Qdrant.
8. **What this unlocks for Phase B (pgvector):** one-paragraph
   summary of "add a new file, implement the trait, add a broker
   dispatch branch, ship".

### 3b. Implementation

#### New file

| file                                             | LOC  | purpose |
|--------------------------------------------------|-----:|---------|
| `crates/the-one-memory/src/vector_backend.rs`    | ~200 | Trait + `BackendCapabilities` + shared point / hit types |

#### Modified files

| file                                             | LOC Δ | changes |
|--------------------------------------------------|------:|---------|
| `crates/the-one-memory/src/qdrant.rs`            | +80  | `impl VectorBackend for AsyncQdrantBackend` delegating to existing methods |
| `crates/the-one-memory/src/redis_vectors.rs`     | +60  | Same for `RedisVectorStore`; returns `Unsupported` for entity/relation/image/hybrid |
| `crates/the-one-memory/src/lib.rs`               | ~±300 | Replace two `Option<T>` fields with one `Option<Box<dyn VectorBackend>>`; update every dispatch site |
| `crates/the-one-mcp/src/broker.rs`               | ~±60 | Simplify `build_memory_engine` via a factory that returns the boxed backend |
| `crates/the-one-memory/src/lib.rs` (constructors) | ±50 | `new_with_backend(...)` as the canonical path; existing `new_with_qdrant` / `new_with_redis` delegate |

#### Trait draft (approximate, subject to refinement during 3a planning)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait VectorBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    async fn ensure_collection(&self, dims: usize) -> Result<(), String>;

    // Chunks — required for every backend.
    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String>;
    async fn search_chunks(
        &self,
        query: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<VectorHit>, String>;
    async fn delete_by_source_path(&self, path: &str) -> Result<(), String>;

    // Hybrid search — default Unsupported.
    async fn search_hybrid(
        &self,
        dense: Vec<f32>,
        sparse: Option<SparseVector>,
        limit: usize,
    ) -> Result<Vec<VectorHit>, String> {
        Err("hybrid search not supported by this backend".into())
    }

    // Entity / relation / image — default Unsupported.
    async fn upsert_entities(&self, _: Vec<EntityPoint>) -> Result<(), String> {
        Err("entity vectors not supported".into())
    }
    async fn search_entities(&self, _: Vec<f32>, _: usize) -> Result<Vec<EntityHit>, String> {
        Err("entity vectors not supported".into())
    }
    async fn upsert_relations(&self, _: Vec<RelationPoint>) -> Result<(), String> {
        Err("relation vectors not supported".into())
    }
    async fn search_relations(&self, _: Vec<f32>, _: usize) -> Result<Vec<RelationHit>, String> {
        Err("relation vectors not supported".into())
    }
    async fn upsert_images(&self, _: Vec<ImagePoint>) -> Result<(), String> {
        Err("image vectors not supported".into())
    }
    async fn search_images(&self, _: Vec<f32>, _: usize) -> Result<Vec<ImageHit>, String> {
        Err("image vectors not supported".into())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BackendCapabilities {
    pub name: &'static str,
    pub chunks: bool,
    pub entities: bool,
    pub relations: bool,
    pub images: bool,
    pub hybrid: bool,
}
```

#### Risks specific to Phase A

| risk                                                       | mitigation                                    |
|-------------------------------------------------------------|-----------------------------------------------|
| Subtle behaviour drift vs. existing `Option<T>` dispatch    | Run entire 446-test suite before and after; bit-for-bit same pass count |
| Redis branches have project_id scoping that matters         | Carefully read every `redis.is_some()` site before translating |
| Feature flag `redis-vectors` interaction with `Box<dyn>`    | `#[cfg(feature = "redis-vectors")]` on Redis impl only; Box holds only Qdrant when feature is off |
| 14+ dispatch sites are mechanical but error-prone           | Sequential pass with `cargo check` after each site; land as one atomic refactor, not interleaved |
| Existing tests wired to specific backend types              | `tests/stdio_write_path.rs` uses the broker not the backend, so should keep working untouched |
| `async_trait` + `Send + Sync` may hit lifetime issues       | `async_trait` crate handles this; already a workspace dep |

#### Estimated effort breakdown

| sub-task                                         | hours |
|---------------------------------------------------|------:|
| Plan document (step 3a)                           | 0.5   |
| Read + catalog every dispatch site                | 0.5   |
| Trait + shared types (`vector_backend.rs`)        | 0.75  |
| Implement for Qdrant (thin wrappers)              | 0.75  |
| Implement for Redis (+ Unsupported paths)         | 0.5   |
| Refactor `MemoryEngine` field                     | 1.0   |
| Update all dispatch sites in `lib.rs`             | 1.25  |
| Update `build_memory_engine` in broker            | 0.5   |
| Full validation (fmt + clippy + tests + bench)    | 0.5   |
| Fix inevitable clippy warnings                    | 0.5   |
| Commit + CHANGELOG entry                          | 0.25  |
| **Total**                                         | ~7.0  |

#### Acceptance criteria

1. `cargo fmt --check` clean
2. `cargo clippy --workspace --all-targets -- -D warnings` clean
3. `cargo test --workspace` ✓ with ≥446 tests passing (same count or
   +1 for the new capability test)
4. `cargo run --release --example production_hardening_bench
   -p the-one-core` still produces realistic numbers
5. Manual smoke test: broker constructs `AuditMode::Synchronous` +
   Qdrant backend, ingests a conversation, search returns results
6. New test `test_backend_capabilities_reported_correctly` in
   `crates/the-one-memory/src/vector_backend.rs` or tests/
7. CHANGELOG entry for v0.15.2 or v0.16.0-rc1

---

## Step 4 — Later session: B, pgvector

Deferred to a clean session after Phase A ships and is reviewed.
Documented here so the sequence is clear:

1. **Prerequisites (from step 3):** `trait VectorBackend` +
   capability reporting in place.
2. **New file:** `crates/the-one-memory/src/pgvector.rs` (~800 LOC
   — connection pool via `sqlx` or `tokio-postgres` + `pgvector`
   crate, schema creation, HNSW index tuning knobs, `impl
   VectorBackend`).
3. **New feature flag:** `pg-vectors` in
   `the-one-memory/Cargo.toml`.
4. **New config fields:** `postgres_url`, `postgres_schema`,
   `postgres_index_type` (hnsw/ivfflat), `postgres_hnsw_m`,
   `postgres_hnsw_ef_construction`.
5. **New broker dispatch branch** in `build_memory_engine`.
6. **Tests:** integration tests need a real Postgres with `CREATE
   EXTENSION vector;`. Gate behind `#[ignore]` + `POSTGRES_URL` env
   var the way `retrieval_bench.rs` gates on Qdrant.
7. **Bench:** extend `production_hardening_bench.rs` or write a
   dedicated pgvector bench.
8. **Docs:** operations guide section, HNSW vs IVFFlat trade-offs,
   index-tuning recommendations.
9. **Rollout:** feature flag opt-in initially; default build stays
   Qdrant; operators opt in explicitly.

**Estimated: 3–5 engineer-days.** Do NOT start until Phase A is
committed.

### Optional Phase C — Bring Redis to parity

After pgvector ships, you might want Redis to support
entities/relations/images/hybrid too. ~2–3 days of work on
`redis_vectors.rs`. Only if you want Redis as a first-class backend;
otherwise leaving it chunks-only is defensible.

---

## Summary of decisions required

| decision                   | options                                           | default |
|----------------------------|---------------------------------------------------|---------|
| Commit slicing             | A (1), **B (3, recommended)**, C (7)             | B       |
| Push target                | direct to `main`, PR branch                       | main    |
| MemPalace-off doc timing   | bundled with step 1, or own tiny commit           | bundled |
| Phase A scheduling         | plan only (pause for review), or plan + implement | plan + implement |
| Memory refresh             | part of wrap-up, or separate                      | wrap-up |

### Default "just go" sequence if no overrides given

1. Run validation (`fmt + clippy + test`).
2. Prepend v0.15.0 and v0.15.1 entries to `CHANGELOG.md`.
3. Add MemPalace-off section to `mempalace-operations.md` (step 2).
4. Option B commit slicing, direct to `main`.
5. Write Phase A plan doc, pause for review.
6. On approval, implement Phase A as one atomic commit to `main`.
7. Refresh `MEMORY.md` with the session's outcomes.
8. Stop — next session handles Phase B (pgvector).

Total: ~25 minutes for step 1-2, ~7 hours for step 3.

---

## See also

- `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
- `docs/guides/production-hardening-v0.15.md`
- `docs/plans/2026-04-10-audit-batching-lever2.md` (v2)
- `docs/plans/draft-2026-04-10-audit-batching-lever2.md`
- `CHANGELOG.md` (Keep-a-Changelog format, last entry 0.14.3)
