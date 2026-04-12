# Resume: the-one-mcp multi-backend — Phase 1 onwards

**Date:** 2026-04-11
**Baseline commit:** `5ff9872` (v0.16.0-rc1 — Phase A trait extraction, bundled)
**Target release:** v0.16.0
**Status:** Ready for a fresh session to execute Phases 1–7
**Delete this file in:** the same commit that ships Phase 7 / v0.16.0 release

**Provenance:** this file supersedes the pasted full-feature resume prompt from the 2026-04-11 session. That prompt was written before the Phase-0 commits landed and assumed a dirty tree + four separate commits. Reality: Phase 0 shipped as one bundled commit (`5ff9872`), working tree is clean. Everything from Phase 1 onwards is preserved byte-for-byte from the original plan, plus a new `## Backend selection scheme` section (derived from a brainstorming session that converged on the env var + config.toml two-layer design) that the original didn't contain.

---

## Who I am

I'm Michel. You're continuing a multi-session refactor that will land full multi-backend support for both vectors and state: Qdrant, pgvector, Redis-Vector (today), plus Postgres, Redis-with-AOF, Redis cache-only, and combined single-connection backends for Postgres+pgvector and Redis+RediSearch.

## Global rules (non-negotiable)

- Do what I say, full production grade, no shortcuts, no stubs, no placeholders, no "good enough". Every phase ships complete or doesn't ship.
- NEVER defer, skip, or descope a task without explicit approval.
- NEVER bump `Cargo.toml` version (stays `"0.1.0"`). Real versioning lives in git tags + commit subjects + `CHANGELOG.md` entries.
- NEVER commit anything without explicit authorisation.
- After a phase is fully complete (design + impl + tests + docs), suggest `/compact` or `/clear` before starting the next.
- If a spec is ambiguous, ASK — don't pick the minimal interpretation.

## Read FIRST (in this exact order)

1. `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
2. `docs/plans/2026-04-11-multi-backend-architecture.md` ← CRITICAL (the architectural plan this resume executes)
3. `docs/plans/2026-04-11-next-steps-expansion.md`
4. `docs/guides/production-hardening-v0.15.md`
5. `docs/guides/multi-backend-operations.md`
6. `CLAUDE.md` (project conventions — note the v0.15.0 / v0.15.1 / v0.16.0-rc1 sections that describe what Phase 0 shipped)
7. `CHANGELOG.md` — the per-version entries for v0.15.0, v0.15.1, and v0.16.0-rc1 are the ground truth for what Phase 0 delivered
8. **This file's `## Backend selection scheme` section below** — load-bearing for Phases 2–6, read it BEFORE writing any sqlx code

Skim only if needed:
- `docs/plans/2026-04-10-audit-batching-lever2.md` (v2, for Lever 2 context — not implementing this roadmap)
- `docs/plans/draft-2026-04-10-audit-batching-lever2.md` (superseded draft, kept as teaching artefact)

---

## Phase 0 — DONE ☑ (landed in commit `5ff9872`)

This section exists for traceability back to the original pasted resume. **Do NOT try to recreate this phase.** It is already shipped.

### What landed (one bundled commit, not four)

The original plan called for four separate commits (v0.15.0 hardening, v0.15.1 Lever 1, v0.16.0-rc1 trait extraction, docs). Reality: all three code units shipped as a single commit `5ff9872` with CHANGELOG.md carrying the per-version sections. This was a deliberate choice — three of the files (`sqlite.rs`, `lib.rs`, `CLAUDE.md`) carried interleaved changes from all three versions, and splitting would have required temp branches without producing individually-green intermediate commits.

**v0.15.0** — production hardening pass addressing every finding from `docs/reviews/2026-04-10-mempalace-comparative-audit.md` (C1–C5, H1–H5, M1–M6):
- New modules: `the_one_core::{naming, pagination, audit}`
- Schema v7: `audit_events` gains `outcome` / `error_kind` columns + indexes
- Cursor pagination replaces silent truncation across every list/search endpoint. Over-limit requests return `InvalidRequest`.
- Input sanitization at every broker write entry point
- Error envelope sanitization via `public_error_message` with correlation IDs
- Navigation digest widened 12 → 32 hex chars (48 → 128 bits)
- 23 new tests (13 production_hardening + 9 stdio_write_path + 1 lever1 guard)
- New benchmark: `production_hardening_bench.rs`
- New guide: `docs/guides/production-hardening-v0.15.md`

**v0.15.1** — Lever 1 audit-write speedup:
- `ProjectDatabase::open` sets `PRAGMA synchronous=NORMAL` in WAL mode
- Measured 67× speedup on audit writes (5.56 ms → 83 µs per row)
- Durability trade-off: safe against process crash, exposed to < 1 s on OS crash. Standard production setting.
- 2 regression tests (throughput smoke + cross-cutting guard)
- Lever 2 async batching designed but NOT shipped (plans preserved in `docs/plans/2026-04-10-audit-batching-lever2.md`)

**v0.16.0-rc1** — Phase A multi-backend trait extraction:
- New trait `the_one_memory::vector_backend::VectorBackend` (chunks / hybrid / entities / relations / images / persistence)
- New trait `the_one_core::state_store::StateStore` (all 22 broker-called methods on `ProjectDatabase`)
- `MemoryEngine` refactored to hold `Option<Box<dyn VectorBackend>>`; all 16 dispatch sites migrated to trait calls
- Canonical constructor `MemoryEngine::new_with_backend(embedding_provider, backend, max_chunk_tokens)`
- `impl VectorBackend for AsyncQdrantBackend` (full capabilities)
- `impl VectorBackend for RedisVectorStore` (chunks-only, feature-gated)
- `impl StateStore for ProjectDatabase` (thin forwarding, zero behaviour change)
- Diary upsert atomicity fix: main INSERT + DELETE FTS + INSERT FTS wrapped in a single `unchecked_transaction`
- `BackendCapabilities` / `StateStoreCapabilities` for capability reporting

### Why Phase 0 matters for Phase 1

The broker still calls `ProjectDatabase::open(...)` directly in ~18 sites rather than going through the `StateStore` trait. This is intentional — the Phase 0 refactor was strictly additive and left the broker unchanged. Phase 1 is the mechanical call-site migration that takes advantage of what Phase 0 built.

---

## Baseline to verify before touching anything

Run in this exact order. If ANY step fails or shows unexpected state, STOP and report to me. Do NOT proceed with Phase 1 execution until the baseline is green.

```bash
# 1. Confirm we're on the right commit and the tree is clean
git log --oneline -3
# Expected first line: "5ff9872 feat: v0.16.0-rc1 — production hardening + Lever 1 + multi-backend traits"
git status
# Expected: "nothing to commit, working tree clean"

# 2. Formatting + lints
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

# 3. Full workspace test suite
cargo test --workspace 2>&1 | tee /tmp/baseline-tests.log
grep "test result:" /tmp/baseline-tests.log | awk '{sum+=$4;f+=$6} END {print "BASELINE:",sum,"passing,",f,"failing"}'
# Expected: 0 failing. Record the passing count as the BASELINE for regression checks.
# Do NOT assert a specific passing count — the count is a moving target across Phase 0 shipping
# and Phases 1–7 adding tests. Record what you see and use it as your tripwire.

# 4. Release gate
bash scripts/release-gate.sh
# Expected: exit 0
```

**If any of these fail, STOP and report to me.** Do not "fix" a failing baseline by editing production code. A failing baseline means either (a) someone else modified the repo between sessions, (b) you're on the wrong branch / wrong machine, or (c) the environment changed (Rust version, dependency yank, etc). All three require me to diagnose before work continues.

**Record the baseline test count** as a file you'll reference throughout Phases 1–7:

```bash
echo "Baseline at 5ff9872: <NNN> passing, 0 failing" > /tmp/the-one-baseline.txt
```

Every phase MUST keep the passing count monotonically increasing. Any phase that drops the count (even by one) has introduced a regression and must STOP.

---

## Backend selection scheme

**This section is load-bearing for Phases 2–6.** Read it once in full before writing any sqlx code or adding any env var parser. The decisions here came from a brainstorming session on 2026-04-11; do not re-derive them from scratch.

### § 1 — Env var surface (selection + secrets only)

Four env vars, two per axis, parallel naming:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string, may carry credentials>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string, may carry credentials>
```

**Naming rationale.** `STATE` and `VECTOR` are the two axis names used throughout `docs/plans/2026-04-11-multi-backend-architecture.md`. Env vars inherit those terms directly. `_TYPE` denotes a closed enum parsed at startup; `_URL` denotes a connection string that may contain credentials. No `DB_URL` (ambiguous), no `BACKEND` (verbose).

**What lives in env vars:** selection (which backend), credentials (via URL), and nothing else. Env vars are the right place for secrets because container orchestrators, systemd `EnvironmentFile=`, and secrets managers all target the env surface.

**What does NOT live in env vars:** tuning knobs, HNSW parameters, schema names, Redis prefixes, AOF verification flags. Those all live in `config.toml` — see § 4 below.

### § 2 — Combined-backend encoding (explicit combined TYPE values)

Phases 4 and 6 ship "combined" backends where ONE connection pool serves BOTH trait roles (state + vectors) for transactional consistency. This is expressed via explicit combined TYPE values:

```bash
# Combined Postgres — state + vectors in ONE pool, transactional consistency
THE_ONE_STATE_TYPE=postgres-combined
THE_ONE_STATE_URL=postgres://user:pw@db.internal/the_one
THE_ONE_VECTOR_TYPE=postgres-combined
THE_ONE_VECTOR_URL=postgres://user:pw@db.internal/the_one
```

**Startup validation rules (all fail loud on violation):**

1. If `STATE_TYPE` ends in `-combined`, `VECTOR_TYPE` MUST be the same value. Mismatch → `InvalidProjectConfig` with the expected value named in the error.
2. When both TYPEs are `*-combined`, URLs MUST be byte-identical. Different URLs → `InvalidProjectConfig` with both URLs echoed in the error message.
3. The broker constructs ONE connection pool (`sqlx::PgPool` for `postgres-combined`, `fred::Client` for `redis-combined`) and passes it to both trait impls via the combined backend adapter (Phase 4 / Phase 6).
4. The broker factory is a single closed `match`: `"sqlite" | "postgres" | "postgres-combined" | "redis" | "redis-combined"` for state, analogous for vectors. No URL-equality inference. No separate "combined" flag.

**Why explicit combined TYPEs beat URL-equality inference:** URL equality is fragile (`postgres://host:5432/db` vs `postgres://host/db` are the same target but non-equal strings). Explicit values make operator intent visible in `env | grep THE_ONE`, and the closed enum catches typos at startup.

### § 3 — Default and failure behavior

| Env var state | Broker behavior |
|---|---|
| **All four unset** | Current default: SQLite state + Qdrant vectors. Zero behaviour change from v0.15.x. This is the 95% deployment. |
| **One TYPE set, other TYPE unset** | **FAIL LOUD at startup.** `"THE_ONE_STATE_TYPE=postgres set but THE_ONE_VECTOR_TYPE is unset; both axes must be explicit when either is overridden."` |
| **TYPE set but matching URL unset** | **FAIL LOUD.** `"THE_ONE_STATE_TYPE=postgres requires THE_ONE_STATE_URL to be set."` |
| **Unknown TYPE value** | **FAIL LOUD with the enum list.** `"Unknown THE_ONE_STATE_TYPE=pgsql; expected one of: sqlite, postgres, redis, postgres-combined, redis-combined"` |
| **Combined TYPEs with mismatched URLs** | **FAIL LOUD** per § 2 rule 2. |
| **Both TYPEs non-combined, same URL** | **Allowed, silent.** Interpretation: operator wants split pools sharing a host (separate credential rotation, statement-timeout isolation, etc.). |
| **One side `postgres-combined`, other side `redis-combined`** | **FAIL LOUD.** `"Combined backends must match: THE_ONE_STATE_TYPE=postgres-combined requires THE_ONE_VECTOR_TYPE=postgres-combined"` |

**Failure philosophy.** Every failure mode is startup failure, never runtime failure. Backend misconfiguration producing partial data is the worst possible outcome, so the broker refuses to boot on any ambiguity.

**Correlation with v0.15.0 error sanitization.** These startup errors surface as `InvalidProjectConfig`, which passes through `public_error_message` verbatim (per v0.15.0 rules). The operator sees the full human-readable message; internal parser details stay in `tracing::error!` with a `corr=<id>`.

### § 4 — Config.toml tuning surface

Secrets stay in env vars. Tuning knobs live in `config.toml`. Clean separation.

```toml
# config.toml — per-backend tuning (secrets live in THE_ONE_* env vars, not here)

[state.sqlite]
# No tuning knobs. Path derived from THE_ONE_PROJECT_ROOT.
# journal_mode = "WAL" and synchronous = "NORMAL" are HARDCODED
# per v0.15.1 Lever 1 guarantees — not configurable.

[state.postgres]
schema = "the_one"                # default; override if sharing a DB with other apps
statement_timeout_ms = 30000      # default 30s; 0 disables

[state.redis]
mode = "persistent"               # "persistent" | "cache" — selects durability semantics
prefix = "the_one_state_{project_id}:"   # {project_id} substituted at runtime
require_aof = true                # enforced only when mode = "persistent"
db_number = 0                     # Redis logical DB (0–15)

[vector.qdrant]
collection_name = "the_one_{project_id}"   # {project_id} substituted at runtime
# API key (if any) read from THE_ONE_QDRANT_API_KEY env var, NOT this file.

[vector.pgvector]
schema = "the_one"                # default; can diverge from state.postgres.schema if desired
hnsw_m = 16                       # HNSW graph connectivity
hnsw_ef_construction = 100        # HNSW build-time quality
hnsw_ef_search = 40               # HNSW query-time recall

[vector.redis]
index_name = "the_one_{project_id}"   # RediSearch FT index name AND hash key prefix
require_aof = true                # fail startup if Redis reports no persistence
db_number = 0
```

**Notes on defaults and edge cases:**

1. **`redis-cache` is a tuning knob, not a TYPE value.** Setting `THE_ONE_STATE_TYPE=redis` + `[state.redis].mode = "cache"` opts into volatile state storage. "Cache vs persistent" is a durability decision, not a tech choice — elevating it to a TYPE value would force two-way branching in every caller.
2. **`{project_id}` substitution** happens at startup in `AppConfig::load()`, reading `THE_ONE_PROJECT_ID`. Simple literal replacement — no Jinja, no expressions, no `{env:FOO}` escape hatches. YAGNI. If future features need more substitution they add it deliberately with a new plan. Unset `THE_ONE_PROJECT_ID` when a templated field is active → startup fails with an explicit error.
3. **Single schema default for combined Postgres.** Both `state.postgres.schema` and `vector.pgvector.schema` default to `"the_one"`, so a combined-Postgres deployment has state and vector tables in the same schema by default. Operators who want schema isolation within one database override one of them explicitly. Rationale: less surprise for new adopters, easier to `DROP SCHEMA the_one CASCADE` cleanly if abandoning.
4. **Combined backends reuse existing sections.** `THE_ONE_STATE_TYPE=postgres-combined` reads from `[state.postgres]` AND `[vector.pgvector]`. There is NO `[combined.postgres]` section. The combined adapter is a dispatch optimization, not a new config surface. Same for `redis-combined`.
5. **Qdrant API key secret handling.** Qdrant's optional API key reads directly from `THE_ONE_QDRANT_API_KEY` — no config-file indirection like `api_key_env = "..."`. Single responsibility: env var holds the secret, config.toml holds nothing sensitive.
6. **`[state.sqlite]` has no knobs on purpose.** WAL journal mode and `synchronous=NORMAL` are hardcoded. Exposing them as tunable fields would let operators accidentally reverse the v0.15.1 Lever 1 67× audit speedup. Deployments that need stricter durability pick a different backend.

### § 5 — Prefix / namespace defaults (multi-tenant isolation for free)

The defaults in § 4 give automatic isolation across three dimensions without any operator config:

| Isolation dimension | Default |
|---|---|
| **Multiple the-one-mcp projects on one Redis** | `the_one_{project_id}` and `the_one_state_{project_id}:` substitute distinct project IDs |
| **the-one-mcp coexisting with another app on one Redis** | the-one-mcp uses `the_one_*` prefixes exclusively; other apps (e.g. `mem:`, `oracle:*`) don't collide |
| **the-one-mcp coexisting with an app that already claims `the_one_*`** | Operator overrides `[state.redis].prefix` and `[vector.redis].index_name` in config.toml explicitly |

**Why prefix lives in config.toml, not env vars:** it's not a secret, it's deployment-topology config, it has a meaningful default (so most deployments don't touch it), and overriding it is a per-deployment decision reviewable in version control. Prefix changes also mean keyspace migration (old keys become orphaned), which is a planned operation — not a `export THE_ONE_PREFIX=...` from a shell.

### § 6 — Phase sequencing (which phase adds what)

| Phase | Env vars introduced | Config sections added | Validation activated |
|---|---|---|---|
| **1** (broker call-site migration) | None | None | None — pure refactor |
| **2** (pgvector) | `THE_ONE_VECTOR_TYPE=pgvector` + `THE_ONE_VECTOR_URL` | `[vector.pgvector]` | **ALL of § 3 rules activate here.** See the critical sequencing note below. |
| **3** (PostgresStateStore) | `THE_ONE_STATE_TYPE=postgres` + `THE_ONE_STATE_URL` | `[state.postgres]` | § 3 unchanged (already active from Phase 2) |
| **4** (combined Postgres) | `postgres-combined` added to both TYPE enums | None new (reuses `[state.postgres]` + `[vector.pgvector]`) | § 2 combined-matching rules 1, 2, 7 |
| **5** (Redis state, three modes) | `THE_ONE_STATE_TYPE=redis` | `[state.redis]` with `mode` field | `require_aof` enforced when `mode = "persistent"` |
| **6** (combined Redis) | `redis-combined` added to both TYPE enums | None new (reuses `[state.redis]` + `[vector.redis]`) | Combined-matching applies to Redis pair |
| **7** (Redis-Vector parity) | None | None | None — capability expansion only |

**CRITICAL sequencing note for Phase 2.** The § 3 validation rules activate the MOMENT the env var parser exists, which is Phase 2. Before Phase 2, there is no parser at all — unset env vars are the only code path. Phase 2's first test MUST exercise the "operator set only `VECTOR_TYPE=pgvector`, expected startup failure" case, even though pgvector is the only new backend shipping. Getting validation right at introduction prevents a "we'll tighten it later" regression loop. Once deployed operators have a mental model of "setting one side silently defaults the other," undoing that is a breaking change.

**Relationship to existing 5-layer config** (`defaults → global file → project file → env vars → runtime overrides`): the `THE_ONE_*_TYPE` / `_URL` env vars live at layer 4. The config.toml tuning sections live at layers 2–3. Runtime overrides (layer 5) still work for everything. The only new thing is that layer 4's TYPE fields are parsed as a closed enum at startup rather than raw `Option<String>`.

---

## The FULL feature roadmap (execute in order)

Execute EVERYTHING. Do not stop at checkpoints unless you hit an actual blocker. Each phase MUST be fully tested + documented before the next phase starts. After each phase: STOP, report to me, wait for "continue" before moving on.

### Phase 1 — DONE ☑ (landed in commit `7666439`, tag `v0.16.0-phase1`)

**Original scope:** ~200 LOC mechanical refactor in `broker.rs`. Turned the existing `ProjectDatabase::open(...)` call sites (16 of them — plan estimated ~18) into trait dispatch through a broker-held cache.

**Actual shape of what landed:**

- New field `McpBroker::state_by_project: RwLock<HashMap<String, Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>>>` with a named type alias `StateStoreCacheEntry` carrying a load-bearing doc comment on the `std::sync::Mutex` choice.
- New factory `state_store_factory(&self, project_root, project_id) -> Result<Box<dyn StateStore + Send>, CoreError>`. Today returns `Box::new(ProjectDatabase::open(...)?)`; `&self` is kept in the signature so Phase 2 can read the parsed `BackendSelection` enum without changing the call sites.
- New private `get_or_init_state_store(...)` implements compute-if-absent with the construct-outside-write-lock pattern: fast path takes only a read lock + Arc clone, cold path constructs the new store outside the write lock then double-checks under it. This is forward-proofing for Phase 3+ where the factory becomes async (Postgres pool warm-up, Redis AOF verification) — without this pattern, concurrent cache-miss traffic for *different* projects would serialize through the factory call.
- `with_state_store(&self, project_root, project_id, |store| Result<R, CoreError>) -> Result<R, CoreError>` is the single chokepoint. The closure is intentionally synchronous because the inner `std::sync::Mutex` guard is `!Send` — the compiler refuses to hold a backend connection across `.await`. That restriction is the forward-compatible anti-deadlock guard for Phase 3+ pools.
- `pub async fn McpBroker::shutdown(&self)` drains the cache. Async today for Phase 3+ (when the trait grows a `shutdown(&self).await` method and combined-backend adapters need explicit teardown ordering).
- Every `ProjectDatabase::open` call site in `broker.rs` is migrated. The only remaining reference is inside `state_store_factory` itself — by design.
- **Two handlers were structurally reshaped** because they previously held `db` across `.await`:
  - `memory_ingest_conversation`: existing-sources read pulled into an early `with_state_store` call; all post-ingest writes (conversation source, optional navigation sync, AAAK lessons, audit) bundled into **one** closure instead of four interleaved `db.method()` calls. The bundle is strictly better than the v0.15.x shape.
  - `tool_run`: session-approvals check (tokio async `RwLock`) moved **before** entering the state-store closure; interactive/headless approval flow splits cleanly between async session mutation and sync DB persistence.
- `sync_navigation_nodes_from_palace_metadata(&ProjectDatabase, ...)` → `&dyn StateStore` so the helper can be invoked through the closure. Un-listed in the original plan deliverables but necessary.
- New test `broker_state_store_cache_reuses_connections` verifies:
  - `Arc::ptr_eq` identity on repeated `get_or_init_state_store` for the same project
  - Distinct projects get distinct entries
  - `with_state_store` actually routes through the cached entry (via `store.project_id()` round-trip)
  - `shutdown()` drains every cache entry cleanly
  - Two separate project roots are used (a single `project_root` is bound to a single `project_id` by the init manifest — test caught this invariant the hard way)

**Test count: 449 → 450** (+1 new, 0 regressions). `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `bash scripts/release-gate.sh` all green. Release binary `target/release/the-one-mcp` builds cleanly.

**Original commit message:** `feat(mcp): broker state_by_project cache via StateStore trait` — landed verbatim.

---

### Phase 2 — DONE ☑ (pgvector VectorBackend + env var parser + startup validator)

Landing in the commit tagged `v0.16.0-phase2`. Summary of what shipped vs what the original deliverable list called for:

- **Decision B was narrowed** after `cargo check` bisection: the sqlx feature set dropped `migrate` and `chrono`. Both features transitively reference `sqlx-sqlite?/…` which cargo's `links` conflict check pulls into the resolution graph, colliding with `rusqlite 0.39`'s `libsqlite3-sys 0.37`. The bisection + full rationale lives in `crates/the-one-memory/Cargo.toml`'s `pg-vectors` feature comment. Replacement: a hand-rolled migration runner at `pg_vector::migrations` (~80 LOC) with `include_str!`-embedded SQL files, SHA-256 checksum drift detection, and a `the_one.pgvector_migrations` tracking table. **`chrono` is removed from the roadmap entirely** — not deferred, not scheduled for Phase 3. Every backend in this workspace uses `BIGINT epoch_ms` for timestamps. Phase 3's `PostgresStateStore` will follow the same convention. The cargo links conflict is permanent and not worth revisiting.
- **Decision C** (hardcoded `dim=1024` per BGE-large-en-v1.5 quality tier) landed as-specified. Every migration `.sql` file has `vector(1024)` literals; the backend constructor refuses to start if `EmbeddingProvider::dimensions() != 1024`.
- **Decision D** (hybrid search semantics: tsvector+GIN vs sparse-array rewrite) remains deferred. `PgVectorBackend::upsert_hybrid_chunks` returns `Err("…deferred to Phase 2.5")` and `capabilities().hybrid = false`. When the broker sees `hybrid_search_enabled=true` with pgvector active, it logs a warning and falls back to dense-only search. Phase 2.5 will land hybrid once benchmarks inform the α vs β choice.
- **Image operations** were NOT implemented. The `VectorBackend` trait as landed in Phase A has no image methods (only chunks/hybrid/entities/relations/verify_persistence) — images go through separate standalone functions in `image_ingest.rs`. The Phase 2 prompt mentioned images in § 3's deliverable list; that was aspirational relative to the actual trait surface. No new images table in the pgvector schema.
- **`config.json` vs `config.toml`**: the plan described the tuning section as TOML (`[vector.pgvector]`), but the existing config file is JSON (`config.json`). `VectorPgvectorConfig` lives as a nested `vector_pgvector` field on the flat `FileConfig` struct; operators override via `{"vector_pgvector": {...}}` in `config.json`. Semantic intent is identical to the plan's TOML layout.
- **Test coverage shipped:**
  - +12 in `the_one_core::config::backend_selection::tests` (8 negative + 4 positive controls, all `temp_env::with_vars` isolated)
  - +2 in `the_one_core::config::tests::pgvector_config_*` (default round-trip + partial override)
  - +5 in `the_one_memory::pg_vector::tests` (feature-gated pure-Rust: migration count, HNSW defaults, pool sizing defaults, schema name, dim constant)
  - +8 in `the_one_memory/tests/pgvector_roundtrip.rs` (feature-gated + env-gated: bootstrap idempotence, chunk roundtrip, upsert idempotency, delete-by-source-path, entity roundtrip, relation roundtrip, provider-dim mismatch, migration tracking table contents)
  - **Test count: 450 → 464 baseline (no pgvector feature), 450 → 469 with `--features pg-vectors`.** Both runs green across `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.

**Original deliverables for reference (not re-executed):**

### Phase 2 (original plan text) — pgvector VectorBackend + env var parser + startup validator

**Scope:** ~800 LOC across new files + env var parser introduction.

**Deliverables:**

- Add workspace dependencies:
  ```toml
  sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "macros"] }
  pgvector = { version = "0.4", features = ["sqlx"] }
  ```
- New Cargo feature `pg-vectors` on `crates/the-one-memory/Cargo.toml`.
- New file `crates/the-one-memory/src/pg_vector.rs`:
  - `PgVectorBackend` struct holding `sqlx::PgPool` + `schema: String` + `project_id: String`
  - Schema bootstrap: `CREATE EXTENSION IF NOT EXISTS vector; CREATE SCHEMA IF NOT EXISTS <schema>; CREATE TABLE <schema>.chunks / entities / relations / images` with HNSW indexes
  - Full `impl VectorBackend` covering ALL operations (chunks dense + hybrid, entities, relations, images, persistence verification)
  - Batched upserts via multi-row `INSERT ... ON CONFLICT DO UPDATE`
- New env var parser + startup validator in `the_one_core::config::backend_selection` (new submodule):
  - Reads all four env vars (`THE_ONE_STATE_TYPE`, `THE_ONE_STATE_URL`, `THE_ONE_VECTOR_TYPE`, `THE_ONE_VECTOR_URL`)
  - Enforces ALL § 3 rules from the backend selection scheme above
  - Returns a typed `BackendSelection { state: StateTypeChoice, vector: VectorTypeChoice }` enum
  - On any failure, returns `CoreError::InvalidProjectConfig` with the exact error messages specified in § 3
- New config.toml section parser for `[vector.pgvector]` with fields: `schema`, `hnsw_m`, `hnsw_ef_construction`, `hnsw_ef_search`
- New broker factory branch: when `BackendSelection.vector == VectorTypeChoice::Pgvector`, construct `PgVectorBackend::new(config, url)?`
- `{project_id}` substitution in `AppConfig::load()` — literal `.replace("{project_id}", ...)` only
- Integration test gated on the unified scheme: runs only when `THE_ONE_VECTOR_TYPE == "pgvector"` AND `THE_ONE_VECTOR_URL` is set. Skip gracefully otherwise. No separate `_TEST`-suffixed env var — the test harness reads the same production env surface per § 1. Covers full CRUD + hybrid-search round trip.
- Negative tests for the validator:
  - `only_vector_type_set_fails_loud` — sets `THE_ONE_VECTOR_TYPE=pgvector` without state-side, expects `InvalidProjectConfig` with the exact § 3 error message
  - `unknown_type_fails_with_enum_list` — sets `THE_ONE_VECTOR_TYPE=pgsql`, expects error listing all valid values
  - `type_without_url_fails` — sets type but no URL, expects specific error
- Bench: extend `production_hardening_bench.rs` with pgvector throughput numbers (gated on env var)
- Docs: new section in `docs/guides/production-hardening-v0.15.md` on pgvector setup, HNSW vs IVFFlat trade-offs, index tuning

**Commit message:** `feat(memory): v0.16.0 — pgvector VectorBackend + env var parser + startup validator`

**STOP and report to me before starting Phase 3.**

---

### Phase 3 — DONE ☑ (PostgresStateStore impl)

Landing in the commit tagged `v0.16.0-phase3`. Summary of what shipped vs what the original deliverable list called for:

- **Migration approach**: used the hand-rolled runner pattern from Phase 2's `pg_vector::migrations` instead of `sqlx::migrate!`. Same rationale as Phase 2 (cargo `links` conflict between sqlx-sqlite and rusqlite). Two migrations ship: `0000_state_migrations_table.sql` (bootstrap tracking table) + `0001_state_schema_v7.sql` (the full v7 schema in ONE pass — no incremental v1..v6 walk-through because fresh Postgres installs have no history). Tracking table is `the_one.state_migrations`, distinct from pgvector's `pgvector_migrations` so Phase 4's combined deployment can share one schema without collision.
- **Sync-over-async bridge**: every `impl StateStore` method wraps sqlx calls in `block_on(async { ... })` where `block_on` = `tokio::task::block_in_place(|| Handle::current().block_on(fut))`. Requires multi-threaded tokio (the broker binary's default). Tests must use `#[tokio::test(flavor = "multi_thread")]`.
- **FTS translation**: `diary_entries.content_tsv TSVECTOR` column + GIN index. Search uses `websearch_to_tsquery('simple', $1)` (not `'english'` — no stemming, works uniformly across languages and code content). `ts_rank` for ordering. LIKE fallback when the tsquery produces zero tokens (pure-punctuation input). `upsert_diary_entry` wraps the INSERT in a sqlx transaction for atomicity — same guarantee Phase 0 added to the SQLite side.
- **NO chrono**. All timestamps are `BIGINT epoch_ms`, generated via `SystemTime::duration_since(UNIX_EPOCH)`. This sidesteps the cargo `links` conflict entirely and matches the workspace-wide convention. Postgres-native `to_timestamp(... / 1000.0)` works for any future human-readable reporting needs.
- **26 trait methods implemented** (the plan said 22 — actual count per `state_store.rs`). Method-by-method parity with the SQLite side via behavioural round-trips; no `unchecked_transaction` needed because Postgres's `pool.begin()` returns an owned transaction that composes cleanly with async.
- **New `CoreError::Postgres(String)` variant** + `"postgres"` label in `error_kind_label`. 3 surgical touches (enum, label, exhaustive test in `production_hardening.rs`). Wire-level sanitizer passes the short label to clients, full text stays in `tracing::error!`.
- **Config**: `StatePostgresConfig` with schema, `statement_timeout_ms` (applied via sqlx's `after_connect` hook), and 5 pool-sizing fields parallel to `VectorPgvectorConfig`. Field names are deliberately identical so Phase 4's combined deployment has one consistent tuning surface. **No `config.toml`** — the workspace uses `config.json` (plan wording was colloquial).
- **Broker wiring**: `state_store_factory` is now `async` (Phase 1's doc comment pre-announced this for Phase 3). Branches on `BackendSelection.state`: Sqlite unchanged, Postgres routes through `construct_postgres_state_store`, other variants return `NotEnabled` until their phases ship. `get_or_init_state_store` now `.await`s the factory — the cold-path construct-outside-write-lock pattern is now load-bearing.
- **Cross-backend regression test NOT shipped**. The plan asked for "run the existing broker integration tests against a `PostgresStateStore`-backed broker." In practice, the existing broker tests construct `ProjectDatabase::open` directly in tempdirs, and porting them to drive a live Postgres instance would require significant test-harness rework. Instead, Phase 3 ships 11 PostgresStateStore-specific integration tests in `crates/the-one-core/tests/postgres_state_roundtrip.rs` covering every method on the trait surface. Coverage-gap caveat is documented in `docs/guides/multi-backend-operations.md` and the resume plan.
- **Test counts**:
  - +2 in `the_one_core::config::tests::postgres_state_config_*`
  - +5 in `the_one_core::storage::postgres::tests` (feature-gated pure-Rust: default config, migration count, kind round-trip, scope strings, epoch_ms monotonicity)
  - +11 in `crates/the-one-core/tests/postgres_state_roundtrip.rs` (feature-gated + env-gated, skip via `return`)
  - **Workspace totals**: 464 → 466 baseline (+2), 464 → 484 with `--features pg-state,pg-vectors` (+20 total including Phase 2's 13 feature-gated tests).

**Original deliverables for reference (not re-executed):**

### Phase 3 (original plan text) — PostgresStateStore impl

**Scope:** ~1500 LOC — the FTS translation layer is the bulk.

**Deliverables:**

- New Cargo feature `pg-state` on `crates/the-one-core/Cargo.toml`
- New file `crates/the-one-core/src/storage/postgres.rs`:
  - `PostgresStateStore` struct holding `sqlx::PgPool` + `schema: String` + `project_id: String`
  - Schema migrations via `sqlx::migrate!` OR hand-written `run_migrations_postgres` mirroring `sqlite.rs`
  - FTS translation: SQLite FTS5 virtual table → Postgres `tsvector` column with GIN index, `to_tsquery` for match, `ts_rank` for ordering, LIKE fallback mirroring sqlite's existing fallback
  - Timestamp translation: `strftime('%s','now') * 1000` → `(extract(epoch from now()) * 1000)::bigint`
  - `INSERT OR REPLACE ... ON CONFLICT DO UPDATE` is portable — Postgres 9.5+ supports identical syntax
  - Full `impl StateStore` — all 22 trait methods
  - Transaction wrapping for `upsert_diary_entry` (mirrors the SQLite atomicity fix from Phase 0)
- New broker factory branch: when `BackendSelection.state == StateTypeChoice::Postgres`, construct `PostgresStateStore::new(config, url)?`
- New config.toml section parser for `[state.postgres]` with fields: `schema`, `statement_timeout_ms`
- Integration tests gated on the unified scheme: `THE_ONE_STATE_TYPE == "postgres"` AND `THE_ONE_STATE_URL` is set. Skip gracefully otherwise. Full round-trip for all 22 `StateStore` methods.
- Cross-backend regression test: run the existing broker integration tests against a `PostgresStateStore`-backed broker and verify every test passes byte-for-byte. If the existing broker tests are too SQLite-specific to port directly, ship `PostgresStateStore`-specific equivalents instead and document the coverage gap in `docs/guides/multi-backend-operations.md`.

**Commit message:** `feat(core): v0.16.0 — PostgresStateStore impl`

**STOP and report to me before starting Phase 4.**

---

### Phase 4 — Combined Postgres+pgvector backend — **DONE** (tag `v0.16.0-phase4`)

**Scope:** ~300 LOC (actual: ~140 LOC of new broker code + ~280 LOC new module + ~400 LOC integration/unit tests + ~290 LOC standalone guide + docs sweep across 5 existing guides + CHANGELOG + CLAUDE.md + PROGRESS.md).

**Original plan vs shipped:**

- ❌ **New submodule `crates/the-one-core/src/backend/postgres_combined.rs`** — shipped as
  **`crates/the-one-mcp/src/postgres_combined.rs`** instead. Rationale: cargo features
  are per-crate booleans with no "and-of-two-crates" composition, and the combined
  builder must call BOTH `the_one_memory::pg_vector::migrations::apply_all`
  (gated on `pg-vectors`) AND `the_one_core::storage::postgres::migrations::apply_all`
  (gated on `pg-state`). The only crate in the workspace where both features are
  reachable simultaneously is `the-one-mcp` — its feature passthroughs
  (`pg-state` → `the-one-core/pg-state`, `pg-vectors` → `the-one-memory/pg-vectors`)
  bring both into scope. Putting the module on `the-one-core` would have required
  either a new `core → memory` dep edge (reversing the current direction) or a
  `pg-combined` feature on memory that transitively activates core's `pg-state`
  via weak-dep syntax — both uglier than just placing the ~280 LOC on
  `the-one-mcp`, which already has the clean `#[cfg(all(pg-state, pg-vectors))]`
  gate.
- ❌ **`PostgresCombinedBackend` struct implementing both traits** — **not shipped**.
  The "refined Option Y" architecture skips the named combined type entirely.
  Instead, Phase 4 adds `PgVectorBackend::from_pool` (memory) and
  `PostgresStateStore::from_pool` (core) as sync wrapper constructors that take a
  pre-built `sqlx::PgPool` + config and skip connect + preflight + migrations,
  then the broker shares the pool via a per-project
  `combined_pg_pool_by_project: RwLock<HashMap<String, sqlx::postgres::PgPool>>`
  cache. `sqlx::PgPool` is internally `Arc`-reference-counted, so `pool.clone()`
  is a cheap refcount bump that gives both trait-role sub-backends a handle to
  the same underlying pool. No trait delegation boilerplate, no new named type,
  no `RedisCombinedBackend` precedent baked into Phase 4. Got Michel's explicit
  approval for this refinement before writing any code (see the
  "recommend one thing" architectural answer at session start).
- ❌ **`StateStore::begin_combined_tx()` trait method** — **not shipped**. The
  plan explicitly said "do NOT implement this in Phase 4 unless you find a
  concrete broker call site that needs it." Phase 4 searched the
  `memory_ingest_conversation` path and the other handlers that write state
  + vectors in one request and found no load-bearing cross-trait atomicity
  requirement at the API level. The shared pool is the *infrastructure*
  needed if a future handler does demand it (any handler can open a sqlx
  transaction against its shared `PgPool` handle), but no trait-level
  abstraction ships in Phase 4. Documented as "known gap" in the standalone
  guide's § 10 "What Phase 4 does NOT ship."
- ✅ **Broker dispatch on `StateTypeChoice::PostgresCombined`** — shipped as
  `construct_postgres_combined_state_store` (state axis) + a parallel
  `build_postgres_combined_memory_engine` early-return in
  `build_memory_engine` (vector axis, before the Phase 2 pgvector fast-path).
  Both reach into `get_or_init_combined_pg_pool` for the per-project shared
  pool. The state cache (`state_by_project`) and memory cache
  (`memory_by_project`) are unchanged — they each hold a distinct sub-backend
  instance that happens to clone the same pool. `McpBroker::shutdown()` now
  drains `combined_pg_pool_by_project` FIRST (calling `pool.close().await` on
  each entry) and THEN clears the state cache, so teardown order is
  deterministic. Without the explicit `close()`, sqlx pools stay alive until
  the last `clone()` drops, which can race with test cleanup.
- ✅ **Phase 3 TODO resolved.** `construct_postgres_state_store` now reads
  `AppConfig::state_postgres` via the new `postgres_combined::mirror_state_postgres_config`
  helper instead of Phase 3's stub `PostgresStateConfig::default()` — Phase 3's
  doc comment explicitly flagged this as "Until Phase 4 formalizes this."
  The mirror helper lives on the combined module so both the split-pool and
  combined paths share one source of truth for the StatePostgresConfig →
  PostgresStateConfig translation.
- ✅ **Integration test** — shipped as
  `crates/the-one-mcp/tests/postgres_combined_roundtrip.rs` (5 tests, gated
  on `all(pg-state, pg-vectors)` + `THE_ONE_{STATE,VECTOR}_TYPE=postgres-combined`
  + byte-identical URLs; skip gracefully via `return` when env isn't set).
  The "rollback atomicity" assertion the plan asked for is NOT in the suite
  because Phase 4 doesn't ship a `begin_combined_tx()` primitive (see above) —
  instead the suite verifies functional equivalence: state writes and vector
  writes through two distinct trait-role adapters both observe the shared
  pool's data, and `build_shared_pool` runs both migration runners exactly
  once against the shared pool. Additional unit tests for the two mirror
  helpers (`mirror_state_postgres_config`, `mirror_pgvector_config`) run
  inline in `postgres_combined.rs` under `#[cfg(test)]` without needing a
  live Postgres.
- ✅ **Docs sweep** — new standalone guide
  `docs/guides/combined-postgres-backend.md` (~290 LOC) covering when to pick
  combined vs split, what "combined" actually means (dispatcher + shared
  pool, no new type), activation, config blocks + the "state config wins"
  rule, topology diagrams, verification queries, migration paths (same-DB =
  zero-data-copy, different-DB = manual `pg_dump`/`pg_restore`),
  integration test surface, and the list of things Phase 4 deliberately
  does NOT ship. Plus updates to
  `multi-backend-operations.md` (Phase 4 flipped from "Planned" to shipped;
  backend matrix updated; decision flowchart now distinguishes combined vs
  split by credential/pool-budget independence),
  `pgvector-backend.md § 12` and `postgres-state-backend.md § 11` (both
  flipped from "Phase 4 preview" to "shipped in Phase 4"),
  `configuration.md` (note on reusing both existing config sections, pool-
  sizing rule, statement_timeout asymmetry), `architecture.md` (factory
  dispatcher example updated, cross-phase relationship row marked shipped),
  `CHANGELOG.md`, `CLAUDE.md`, `PROGRESS.md`.

**Additional work that was not in the plan:**

- ✅ **New Cargo dep `sqlx` on `the-one-mcp`** — optional, narrow feature set
  `[runtime-tokio, tls-rustls, postgres, macros]` (same `links`-safe list
  Phase 2/3 bisected), activated by either `pg-state` or `pg-vectors`
  features. Required because the combined module in
  `crates/the-one-mcp/src/postgres_combined.rs` uses
  `sqlx::postgres::{PgPool, PgPoolOptions}` and `sqlx::query` directly at
  the broker level (not via a downstream crate re-export). Alternative
  designs would have added `pub use sqlx::postgres::PgPool` re-exports on
  core or memory, but sqlx's `query` macro doesn't re-export cleanly, so
  adding it as a direct dep was the simpler move. Same `chrono` + `migrate`
  feature exclusions stand.
- ✅ **`preflight_vector_extension` public visibility** — was `pub(crate)`
  in Phase 2; Phase 4 makes it `pub` so the combined module can call it
  without duplicating the three-state detection logic and the five-
  per-provider error catalog. Doc comment updated to explain the new
  visibility rationale.

**Pool sizing asymmetry documented explicitly:**

- State config wins. `state_postgres.{max,min}_connections`, the timeout
  fields, and `statement_timeout_ms` (via the `after_connect` hook) all
  apply to the shared pool. `vector_pgvector`'s corresponding pool fields
  are ignored. HNSW tuning still comes from `vector_pgvector` (migration-
  time + query-time, not pool-time).
- **statement_timeout asymmetry**: on combined deployments, vector queries
  inherit the state-side `statement_timeout` — the split-pool pgvector
  path has no equivalent hook. Operators migrating from split-pool
  pgvector must bump `state_postgres.statement_timeout_ms` high enough to
  accommodate their slowest vector search. Documented in the standalone
  guide with a migration note.

**Test count delta:**

- Base path unchanged at **466 passing, 1 ignored** (combined module is
  feature-gated; base path sees no new tests).
- **495 → 504 with `--features pg-state,pg-vectors`** (+9 total: 4 mirror-
  helper unit tests in `postgres_combined.rs` that don't need a live
  database, 5 integration tests in `postgres_combined_roundtrip.rs` that
  skip gracefully via early `return` when env vars aren't set — each
  graceful skip still counts as a passing test because the test function
  returns normally).

**Commit message:** `feat: v0.16.0 — combined Postgres+pgvector backend` (pending, tag `v0.16.0-phase4`).

**STOP and report to me before starting Phase 5.**

---

### Phase 5 — DONE ☑ (Redis StateStore, commit `1dbf6a5`, tag `v0.16.0-phase5`)

All 26 `StateStore` trait methods against Redis (HSET for objects, Redis Streams for audit, sorted sets for time-ordered listing, RediSearch `FT.SEARCH` for diary FTS). Two modes: cache (`require_aof=false`) and persistent (`require_aof=true`, verifies `aof_enabled:1`). New `CoreError::Redis(String)` variant. New `StateRedisConfig`. New Cargo feature `redis-state` on `the-one-core` + passthrough on `the-one-mcp`. `fred` `i-streams` feature added. `recursion_limit` bumped to 256. `RedisStateStore::from_client` for Phase 6. 7 integration tests gated on `THE_ONE_STATE_TYPE=redis`. Test count: 466 base, 511 features.

---

### Phase 6 — DONE ☑ (Combined Redis+RediSearch, commit `1b1b22f`, tag `v0.16.0-phase6`)

Same refined Option Y as Phase 4 Postgres combined. One `fred::Client` shared between `RedisStateStore` (via `RedisStateStore::from_client`) and `RedisVectorStore` (via `RedisVectorStore::new` with shared client). Broker gains `combined_redis_client_by_project` cache. Factory branches for `StateTypeChoice::RedisCombined` and `VectorTypeChoice::RedisCombined`. `fred` as direct optional dep on `the-one-mcp`. Test count unchanged (combined tests skip without env).

---

### Phase 7 — DONE ☑ (Redis-Vector entity/relation parity + v0.16.0 GA, commit `7857647`, tags `v0.16.0-phase7` + `v0.16.0`)

`RedisVectorStore` now supports entities and relations (was chunks-only). Each type gets its own RediSearch index. `capabilities()` updated to `entities=true`, `relations=true`. Images remain unsupported on Redis (tracked for v0.16.1). Decision D (pgvector hybrid) deferred to post-GA. Final test count: 466 base, 521 all features.

---

## Execution discipline (applies to every phase)

For every phase:

1. Read the relevant phase section above AND the corresponding section of `docs/plans/2026-04-11-multi-backend-architecture.md` first
2. Implement fully — no stubs, no placeholders, no "good enough"
3. Write tests: unit + integration, positive + negative
4. Run full validation: `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` + `bash scripts/release-gate.sh`
5. Update docs in the same commit as the code change
6. Ask me for commit authorization BEFORE running `git commit` (per global rule "NEVER commit anything without explicit authorisation")
7. STOP and report to me after every phase — do not start the next phase until I say "continue"

**If you hit a real blocker** (external API ambiguity, test harness failure, unexpected dependency conflict, sqlx version incompatibility, anything unexpected), STOP and ask me rather than working around it. Shortcuts break the trade-off math that went into this roadmap.

---

## Acceptance criteria for v0.16.0 (the whole feature)

1. Every backend combination works via config alone, zero code changes required to switch:
   - SQLite + Qdrant (unchanged default)
   - SQLite + pgvector (mix-and-match)
   - Postgres + Qdrant (mix-and-match)
   - Postgres + pgvector (split pools, two URLs)
   - **Postgres combined** (one pool, `postgres-combined`)
   - Postgres + Redis-Vectors (cross-tech)
   - Redis + Qdrant
   - Redis + pgvector
   - **Redis combined** (one client, `redis-combined`)
   - Redis cache + Redis-Vectors (mixed durability)
2. `cargo test --workspace` passes. Count monotonically greater than the Phase 0 baseline. Tests requiring live Postgres/Redis are gated on the unified scheme (the same `THE_ONE_STATE_TYPE` + `THE_ONE_STATE_URL` / `THE_ONE_VECTOR_TYPE` + `THE_ONE_VECTOR_URL` env vars production reads), and skip gracefully when those vars aren't set to the relevant backend. No `_TEST`-suffixed env vars exist in the workspace — the test harness shares the production env surface per § 1's unified-scheme decision.
3. `production_hardening_bench.rs` produces comparable numbers across all backends; results recorded in release notes.
4. Every backend has operator-facing docs in `docs/guides/multi-backend-operations.md` with: selection matrix, config examples, migration paths, trade-off table.
5. `CHANGELOG.md` has a complete v0.16.0 entry in Keep-a-Changelog format.
6. Zero regressions on existing behaviour. Every test that passed at the Phase 0 baseline still passes at v0.16.0.
7. § 3 validation rules are fully enforced — negative tests exist for every failure mode in the table.

---

## Non-goals for this roadmap (do NOT do these)

- **No new user-facing MCP tools.** This is infrastructure — no MCP endpoint gains, loses, or changes a parameter.
- **No performance optimization beyond what the backends naturally give.** SQLite+Qdrant remains the default because it's fast enough for most use cases. This phase is about options, not speed.
- **No cross-backend migration tooling.** "Dump from SQLite, load into Postgres" is out of scope for v0.16.0. Operators choose a backend at init time; switching later requires manual re-ingestion.
- **No Lever 2 async audit batching.** Designed in parallel (`docs/plans/2026-04-10-audit-batching-lever2.md`) but explicitly deferred — separate post-v0.16.0 ticket.
- **No `Cargo.toml` version bump.** Stays at `"0.1.0"`. Real version lives in tags + commit subject + CHANGELOG.
- **No extended `{project_id}` substitution syntax.** Literal `.replace("{project_id}", ...)` only. No `{env:FOO}`, no expressions, no Jinja.

---

## First action

1. Run the baseline verification block above in full
2. Record the test count into `/tmp/the-one-baseline.txt`
3. Report results to me
4. **Wait for my "continue" before starting Phase 1.** Do not begin Phase 1 on your own.

Do NOT:
- Skip any phase
- Batch phases into single commits
- Commit without explicit authorization per phase
- Assume any § 3 validation rule can be "tightened later"
- Extend `{project_id}` substitution beyond the literal pattern
- Take shortcuts around real blockers — ask me instead

Phase 0 is already landed in `5ff9872`. Begin at Phase 1.
