# Progress Report

## Current Version: v0.16.1

**Shipped:** v0.16.1 MCP protocol patch on top of v0.16.0 multi-backend
support. Fixes a post-handshake session-drop bug where the stdio
transport replied to the client's `notifications/initialized` with an
out-of-spec `{"jsonrpc":"2.0","result":null}` frame, tripping strict
clients (Claude Code) into a Zod validation error and closing the
pipe. Also corrects `--version` and `serverInfo.version` to report
the real release string instead of the default `0.1.0` and the schema
tag `v1beta`. v0.16.0's full multi-backend surface (SQLite, Postgres,
Redis for state; Qdrant, pgvector, Redis-Vector for vectors; combined
single-pool/single-client modes for Postgres+pgvector and
Redis+RediSearch) is unchanged. Historical execution plan archived
at `docs/plans/historical/2026-04-11-resume-phase1-onwards.md`.

## Overall Status

All planned stages complete. Twenty-four tracked releases shipped
(plus two in-progress milestones for v0.16.0):

- **v0.1.0** — Initial workspace: 8 crates, 14 MCP tools, stub implementations
- **v0.2.0** — Production overhaul: async broker, real embeddings, 3 transports, 24 tools
- **v0.3.0** — Tool catalog: SQLite + Qdrant semantic search, tool lifecycle, 31 tools
- **v0.4.0** — Embedding model registry: TOML-based model registries, quality tier default, interactive installer selection, 33 tools
- **v0.5.0** — Tool consolidation: 33→15 tools (~52% token savings), multiplexed admin, merged work tools
- **v0.6.0** — Multimodal: image embeddings, OCR, reranking (fastembed 5.x), 17 tools, 208 tests
- **v0.7.0** — Hybrid search (dense+sparse), file watcher, admin UI image gallery, screenshot image search, 234 tests
- **v0.8.0** — Watcher auto-reindex (markdown), code-aware chunker (5 languages), extended ChunkMeta, 272 tests
- **v0.8.1** — Documentation refresh: all guides + root docs audited for v0.8.0 accuracy
- **v0.8.2** — Image auto-reindex: watcher now re-ingests image upserts/deletes, standalone helpers
- **v0.9.0** — Tree-sitter AST chunker for 13 languages (5 existing + C/C++/Java/Kotlin/PHP/Ruby/Swift/Zig), retrieval benchmark suite, 283 tests
- **v0.10.0** — MCP Resources API (`resources/list`, `resources/read`, `the-one://` URI scheme), catalog expansion (117 → 184 tools across 10 languages), landing page scaffold, 296 tests
- **v0.12.0** — Phase 3 bundled: Intel Mac `local-embeddings-dynamic` feature flag, observability deep dive (+8 metric counters + per-operation latency + Arc<BrokerMetrics>), backup/restore via `maintain: backup` and `maintain: restore` with tar+gzip, 300 tests
- **v0.12.1** — Docs refresh: 3 new guides (mcp-resources, backup-restore, observability) + README/CLAUDE.md/api-reference/tool-catalog/upgrade-guide/troubleshooting updated for v0.12.0 feature surface
- **v0.13.0** — Major UI overhaul (landing page, /ingest, /graph, v2 dashboard, top nav with project switcher, shared page shell, dark-mode-aware) + Graph RAG end-to-end wiring (extraction pipeline, `maintain: graph.extract`/`graph.stats`, LightRAG-inspired research) + graph-rag.md guide, 302 tests
- **v0.13.1** — Full LightRAG parity: entity name normalization, entity/relation description vector store (3 Qdrant collections), description summarization, query keyword extraction, gleaning/continue-extraction pass, canvas force-directed graph visualization, 308 tests
- **v0.14.0** — Catalog expansion to 365 tools (+248 new from baseline 117). All 10 language files + all 8 category files populated. Closes the deferred Task 5 from the 9-item roadmap.
- **v0.14.1** — Documentation refresh for v0.14.0 catalog expansion and counts.
- **v0.14.2** — Production hardening completion: real Redis vector backend runtime path, `models.check` real check flow, `the-one://catalog/enabled` backed by catalog DB, wake-up `wing/hall/room` filters, test determinism fixes, docs hardening.
- **v0.14.3** — MemPalace production controls: on/off feature toggles, first-class hook capture (`maintain: memory.capture_hook` for `stop`/`precompact`), config/env/runtime wiring, and strict feature gating.
- **v0.15.0** — Production hardening pass addressing every finding from `docs/reviews/2026-04-10-mempalace-comparative-audit.md` (C1–C5, H1–H5, M1–M6). New modules `the_one_core::{naming, pagination, audit}`. Schema v7 adds `outcome`/`error_kind` columns to `audit_events` with indexes. Cursor pagination replaces silent truncation across every list/search endpoint (over-limit requests return `InvalidRequest`). Input sanitization at every broker write entry point via `sanitize_name`/`sanitize_project_id`/`sanitize_action_key`. Error envelope sanitization via `public_error_message` with `corr=<id>` correlation IDs — `CoreError::Sqlite`/`Io`/`Json`/`Embedding` surface only their kind labels; internal details stay in `tracing::error!`. Navigation node digest widened 12→32 hex chars (48→128 bits). 23 new tests (13 `production_hardening` + 9 `stdio_write_path` + 1 lever1 guard — the 1 ignored). New benchmark `production_hardening_bench.rs`. New guide `docs/guides/production-hardening-v0.15.md`.
- **v0.15.1** — Lever 1 audit-write speedup: `ProjectDatabase::open` sets `PRAGMA synchronous=NORMAL` in WAL mode. Measured **67× faster** audit writes (5.56 ms → 83 µs per row). Durability trade-off: safe against process crash (WAL captures every commit), exposed to < 1 s of writes on OS crash — the standard modern-SQLite production setting used by Firefox, Android, rqlite, Litestream, Turso. 2 regression tests (throughput smoke + cross-cutting guard). Lever 2 async batching designed in parallel but explicitly deferred — plans preserved in `docs/plans/2026-04-10-audit-batching-lever2.md`.
- **v0.16.0-rc1** — Phase A multi-backend trait extraction. New `trait VectorBackend` in `the_one_memory::vector_backend` covering chunks/entities/relations/images/hybrid vector operations; `trait StateStore` in `the_one_core::state_store` covering all 22 broker-called methods on `ProjectDatabase`. `MemoryEngine` now holds `Option<Box<dyn VectorBackend>>` (was two concrete Option fields); canonical constructor `MemoryEngine::new_with_backend(embedding_provider, backend, max_chunk_tokens)`. `impl VectorBackend for AsyncQdrantBackend` (full), `impl VectorBackend for RedisVectorStore` (chunks-only, feature-gated), `impl StateStore for ProjectDatabase` (thin forwarding, zero behaviour change). Diary upsert atomicity fix: main INSERT + DELETE FTS + INSERT FTS wrapped in one `unchecked_transaction`. `BackendCapabilities` / `StateStoreCapabilities` for capability reporting. **Bundled with v0.15.0 + v0.15.1 as commit `5ff9872`** because three files carried interleaved changes from all three versions.
- **v0.16.0-phase1** — Broker `state_by_project` cache via `StateStore` trait. Mechanical refactor landing the broker-side piece of the multi-backend roadmap. All 16 `ProjectDatabase::open` call sites in `broker.rs` now route through a new `with_state_store(project_root, project_id, |store| ...)` chokepoint. Inner lock is `std::sync::Mutex` (deliberately not tokio) so the guard is `!Send` — the compiler refuses to hold a backend connection across `.await`, preventing Postgres/Redis connection-pool deadlocks in Phase 3+. `get_or_init_state_store` constructs new entries outside the outer write lock (double-checks under it) — load-bearing for Phase 3+ async factories. `pub async fn McpBroker::shutdown()` drains the cache. Two handlers (`memory_ingest_conversation`, `tool_run`) restructured to split async memory/session work from sync DB work. New test `broker_state_store_cache_reuses_connections` verifies `Arc::ptr_eq` identity across repeated lookups, per-project isolation, and clean shutdown drain. Zero user-visible behaviour change. Commit `7666439`, tag `v0.16.0-phase1`. Completes the call-site migration that v0.16.0-rc1 explicitly deferred. **449 → 450 tests** (+1 cache test).
- **v0.16.0-phase2** — pgvector `VectorBackend` + env var parser + startup validator. First real alternative vector backend after the Phase A trait extraction. New `crates/the-one-memory/src/pg_vector.rs` (~860 LOC) implementing chunks + entities + relations against pgvector (hybrid search deferred to Phase 2.5 as Decision D). Batched upserts via `INSERT ... SELECT * FROM UNNEST(...)`. Defensive `preflight_vector_extension` with targeted per-provider errors (Supabase / RDS / Cloud SQL / Azure / self-hosted). **Hand-rolled migration runner** at `pg_vector::migrations` — not `sqlx::migrate!` because sqlx's `migrate` and `chrono` features transitively reference `sqlx-sqlite?/…` weak-deps that cargo's `links` check pulls into the graph, colliding with rusqlite 0.39's `libsqlite3-sys 0.37`. Five migrations ship with hardcoded `dim=1024` (Decision C — BGE-large-en-v1.5 quality tier). New `the_one_core::config::backend_selection` submodule parsing the four-variable `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` env surface with fail-loud validation (12 tests: 8 negative + 4 positive). New `VectorPgvectorConfig` in the config stack. `McpBroker` gains `backend_selection` field + `try_new_with_policy` fail-loud constructor + `build_pgvector_memory_engine` fast-path. New Cargo feature `pg-vectors` (off by default); sqlx features narrowed to `[runtime-tokio, tls-rustls, postgres, macros]` after bisection. Integration tests in `crates/the-one-memory/tests/pgvector_roundtrip.rs` (8 tests, env-gated, skip via `return`). New bench `pgvector_bench.rs`. Commit `91ff224`, tag `v0.16.0-phase2`. **450 → 464 baseline** (+14 config + parser tests), **450 → 477 with `--features pg-vectors`** (+27 total including 5 pg_vector unit + 8 integration).
- **v0.16.0-phase3** — `PostgresStateStore` impl — second-axis complement to Phase 2's pgvector. Operators can now run the-one-mcp against managed Postgres (RDS, Cloud SQL, Azure, Supabase, self-hosted) with zero SQLite on the state axis. New `crates/the-one-core/src/storage/postgres.rs` (~1,350 LOC) implementing every `StateStore` trait method (26 methods — the plan estimated 22). **Sync-over-async bridge** via `tokio::task::block_in_place` + `Handle::current().block_on` in every trait method (`StateStore` is sync by design; sqlx is async; requires multi-threaded tokio runtime). Hand-rolled migration runner at `postgres::migrations` using the Phase 2 pattern; tracking table `the_one.state_migrations` is distinct from pgvector's `pgvector_migrations` so Phase 4 combined can share one schema. **FTS5 → tsvector translation**: `diary_entries.content_tsv TSVECTOR` column + GIN index + `websearch_to_tsquery('simple', $1)` for matching (not `'english'` — no stemming, works uniformly across languages and code) + `ts_rank` for ordering + LIKE fallback for empty tsquery. `upsert_diary_entry` wraps the INSERT in a sqlx transaction for atomicity. Schema v7 parity from day one (no incremental v1..v6 walk-through — Postgres has no history). **BIGINT epoch_ms throughout** (no chrono, no TIMESTAMPTZ — workspace-wide convention). New `CoreError::Postgres(String)` variant with `"postgres"` label in `error_kind_label` (3 surgical touches: enum, label, exhaustive test). New `StatePostgresConfig` mirroring `VectorPgvectorConfig`. `state_store_factory` is now `async` (Phase 1 pre-announced this) and branches on `BackendSelection.state`. New Cargo feature `pg-state` (off by default, composable with `pg-vectors`). Integration tests in `crates/the-one-core/tests/postgres_state_roundtrip.rs` (11 tests, env-gated, skip via `return`, run with `--test-threads=1`). Cross-backend regression tests deferred — documented coverage gap; 11 PostgresStateStore-specific tests cover the trait surface instead. Commit `f010ed6`, tag `v0.16.0-phase3`. **464 → 466 baseline** (+2 config), **464 → 495 with `--features pg-state,pg-vectors`** (+31 total: 13 Phase-2 gated + 16 Phase-3 gated + 2 base).
- **v0.16.0-phase4** — **combined Postgres+pgvector backend**. The first *combined single-pool* backend on the multi-backend roadmap: one `sqlx::PgPool` serving both the `StateStore` trait role and the `VectorBackend` trait role against a single Postgres database. Activated via `THE_ONE_STATE_TYPE=postgres-combined` + `THE_ONE_VECTOR_TYPE=postgres-combined` with byte-identical URLs — the Phase 2 env-var parser already enforced the matching + equality rules, and Phase 4 flips the previously-`NotEnabled` factory branches to real constructors. **Refined Option Y architecture (no named combined type)**: instead of a hypothetical `PostgresCombinedBackend` struct that would own both sub-backends and forward 34+ trait methods, Phase 4 adds `PgVectorBackend::from_pool` (memory crate) and `PostgresStateStore::from_pool` (core crate) — both sync wrapper constructors that skip connect + preflight + migrations and just take a pre-built pool + config. The pool is shared via `McpBroker::combined_pg_pool_by_project: RwLock<HashMap<String, sqlx::postgres::PgPool>>` using the same read-upgrade-write cold-path pattern as `get_or_init_state_store`. `sqlx::PgPool` is internally `Arc`-reference-counted so `pool.clone()` is a cheap refcount bump giving both trait-role sub-backends a handle to the same underlying pool — no explicit `Arc<PgPool>` needed. **New module** `crates/the-one-mcp/src/postgres_combined.rs` (`#[cfg(all(pg-state, pg-vectors))]`) owns `build_shared_pool` (connect + `preflight_vector_extension` + `pg_vector::migrations::apply_all` + `postgres::migrations::apply_all` + 1024-dim check, all exactly once per project on the cold path) plus two mirror helpers (`mirror_state_postgres_config`, `mirror_pgvector_config`). Lives on `the-one-mcp` because cargo features are per-crate booleans and only the broker crate has both `pg-state` and `pg-vectors` reachable. **Two new factory methods**: `construct_postgres_combined_state_store` (state axis) and `build_postgres_combined_memory_engine` (vector axis, takes priority over the Phase 2 `build_pgvector_memory_engine` branch). **`McpBroker::shutdown()`** now drains the shared-pool cache first and `pool.close().await`s each entry before clearing the state cache, so teardown order is deterministic (without the explicit close, sqlx pools stay alive until the last `clone()` drops, which can race with test cleanup). **Phase 3 TODO resolved**: `construct_postgres_state_store` now reads `AppConfig::state_postgres` via `mirror_state_postgres_config` instead of Phase 3's stub `PostgresStateConfig::default()`. **Pool-sizing rule**: state config wins — `state_postgres.{max,min}_connections`, the timeout fields, AND `statement_timeout_ms` (via the `after_connect` hook) all apply to the shared pool. Consequence: vector queries inherit the state-side `statement_timeout` on combined deployments (the split-pool pgvector path has no equivalent hook). HNSW tuning still comes from `vector_pgvector` because those are migration-time + query-time settings. **New Cargo dep**: `sqlx` as a direct optional dep on `the-one-mcp` with the same narrow `[runtime-tokio, tls-rustls, postgres, macros]` feature set Phase 2/3 bisected, activated by either `pg-state` or `pg-vectors`. **NOT shipped**: no `begin_combined_tx()` trait method (considered and deferred — no call site needed it); no named combined backend type; no automated split → combined migration tool; no trait-surface changes. Integration tests in `crates/the-one-mcp/tests/postgres_combined_roundtrip.rs` (5 tests gated on `all(pg-state, pg-vectors)` + `THE_ONE_{STATE,VECTOR}_TYPE=postgres-combined` + byte-identical URLs, skip gracefully via `return`); mirror-helper unit tests inline in `postgres_combined.rs` (4 tests, no Postgres required). **New standalone guide** `docs/guides/combined-postgres-backend.md`. Updated sections in `multi-backend-operations.md`, `pgvector-backend.md § 12`, `postgres-state-backend.md § 11`, `configuration.md`, `architecture.md`. Commit `8f83f05`, tag `v0.16.0-phase4`. **466 baseline unchanged**, **495 → 504 with `--features pg-state,pg-vectors`** (+4 mirror unit + +5 integration).
- **v0.16.0-phase5** — Redis `StateStore` (cache + persistent modes). All 26 `StateStore` trait methods on Redis (HSET, Redis Streams, sorted sets, RediSearch `FT.SEARCH`). Two modes: cache (`require_aof=false`) and persistent (`require_aof=true`). New `CoreError::Redis(String)` variant. New `StateRedisConfig` + Cargo feature `redis-state`. `RedisStateStore::from_client` for Phase 6. 7 integration tests env-gated. Test count: 466 base, 511 features.
- **v0.16.0-phase6** — Combined Redis+RediSearch backend. One `fred::Client` shared between `RedisStateStore` and `RedisVectorStore`. Broker gains `combined_redis_client_by_project` cache. Factory branches for `RedisCombined` on both axes. `fred` as direct optional dep on `the-one-mcp`.
- **v0.16.0-phase7 / v0.16.0 GA** — Redis-Vector entity/relation parity. `RedisVectorStore` gains entities + relations (was chunks-only); each type gets its own RediSearch index. Images remain unsupported on Redis (tracked for v0.16.2). Decision D (pgvector hybrid) deferred to post-GA. Final test count: **466 base, 521 all features**.
- **v0.16.1** — **MCP protocol fix + version-display fix.** Structural repair of the JSON-RPC notification path: `dispatch` now returns `Option<JsonRpcResponse>` and any id-less message short-circuits to `None`, so notifications (starting with the client's `notifications/initialized` after handshake) no longer produce a `{"jsonrpc":"2.0","result":null}` frame that strict clients (Claude Code's Zod-validated stdio transport) reject. Both HTTP transports (`sse`, `stream`) now return `202 Accepted` with empty body for notifications per MCP's HTTP mapping. Separately, workspace `Cargo.toml` version `0.1.0` → `0.16.1` so `--version` reports the real release, and MCP `serverInfo.version` switches from the schema tag `"v1beta"` to `env!("CARGO_PKG_VERSION")` so clients surface the release string. Two regression tests guard the fix — `test_dispatch_notifications_initialized_emits_no_response` (rewrite of the pre-existing test that had asserted the buggy behaviour) and the new `test_dispatch_any_notification_emits_no_response`. No breaking changes. `the-one-mcp --lib` test count: **113 → 114 passing** (net +1 after the rewrite + add).
- **MemPalace phase 2** — completed production feature set:
  - AAAK compression + lesson persistence (`memory.aaak.*`)
  - explicit drawers/closets/tunnels primitives (`memory.navigation.*`)
  - diary-specific memory flows (`memory.diary.*`) with refresh-safe identity
  - single-switch profile control (`config: profile.set` + Admin UI preset card)

Build/test gates: all green. **466 tests passing** (base, +1 ignored — the Lever 2 deferred guard), **521 with all features**. 365 catalog tools. 17 MCP tools + 3 MCP resource types.

## Stats

### Historical (v0.1.0 → v0.12.0)

| Metric | v0.1.0 | v0.2.0 | v0.3.0 | v0.4.0 | v0.5.0 | v0.6.0 | v0.7.0 | v0.8.0 | v0.8.2 | v0.9.0 | v0.10.0 | v0.12.0 |
|--------|--------|--------|--------|--------|--------|--------|--------|--------|--------|--------|---------|---------|
| MCP Tools | 14 | 24 | 31 | 33 | 15 | 17 | 17 | 17 | 17 | 17 | 17 | **17** |
| MCP Resources | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 3 types | **3 types** |
| Tests | 68 | 122 | 135 | 174 | 183 | 208 | 234 | 272 | 272 | 283 | 296 | **300** |
| Supported code languages | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 5 | 13 | 13 | **13** |
| Metrics counters | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | **15** |
| maintain actions | — | — | — | — | 10 | 11 | 12 | 12 | 12 | 12 | 12 | **14** |
| Rust LOC | 6,400 | ~10,000 | ~12,800 | ~14,000 | ~14,200 | ~16,500 | ~19,000 | ~21,000 | ~21,100 | ~22,500 | ~23,200 | **~24,000** |
| JSON Schemas | 33 | 49 | 63 | 63 | 31 | 35 | 35 | 35 | 35 | 35 | 35 | **35** |
| Catalog Tools | — | — | 28 | 28 | 28 | 28 | 28 | 28 | 28 | 28 | 184 | **365** |
| Platforms | 1 | 1 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | **6** |
| AI CLIs | 2 | 2 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | **4** |

### Recent (v0.13.0 → v0.16.0 GA)

| Metric | v0.15.0 | v0.16.0-rc1 | v0.16.0-phase2 | v0.16.0-phase4 | v0.16.0-phase5 | v0.16.0-phase6 | **v0.16.0 GA** |
|--------|---------|-------------|----------------|----------------|----------------|----------------|----------------|
| MCP Tools | 17 | 17 | 17 | 17 | 17 | 17 | **17** |
| Tests (passing, base) | 426 | 449 | 464 | 466 | 466 | 466 | **466** |
| Tests (passing, all features) | — | — | 477 | 504 | 511 | 511 | **521** |
| Tests (ignored) | 1 | 1 | 1 | 1 | 1 | 1 | **1** |
| Backend traits | 0 | **2** | 2 | 2 | 2 | 2 | **2** |
| Vector backends | Qdrant, Redis-Vector, in-memory | same (unified via trait) | **+ pgvector (split)** | + pgvector combined | same | + Redis combined | **+ Redis entities/relations** |
| State backends | SQLite only | same | same | + Postgres split + combined | **+ Redis (cache/persistent)** | + Redis combined | **same** |
| Error variants | `public_error_message` envelope | same | same | + `CoreError::Postgres` | **+ `CoreError::Redis`** | same | same |

**Note on the test count jump from v0.13.1 (308) → v0.14.3 (387):** the ~80-test gap covers the v0.14.x series of additions that weren't individually tracked in this file. The v0.14.3 count (387 passing + 1 ignored) is the number PROGRESS.md was stuck at before the v0.16.0 roadmap started. v0.15.0 rebuilt the baseline at 426 (v0.14.3 + ~39 production-hardening tests). v0.15.1 added 2. v0.16.0-rc1 added ~21 (diary atomicity, trait sanity, capability reporting). v0.16.0-phase1 added 1 (cache-reuse test). v0.16.0-phase2 added +14 base (config + env parser) and +13 feature-gated (pgvector unit + integration). v0.16.0-phase3 added +2 base (config) and +18 feature-gated (postgres-state integration + extended gated unit). v0.16.0-phase4 left the base count unchanged at 466 and added +9 feature-gated (4 mirror unit + 5 combined integration), reaching **504 with `--features pg-state,pg-vectors`**.

## Stage Progress (v0.1.0)

- Stage 0: Program setup — complete
- Stage 1: Core foundations — complete
- Stage 2: Isolation/lifecycle — complete
- Stage 3: Profiler/fingerprint — complete
- Stage 4: Registry/policy/approvals — complete
- Stage 5: Docs/RAG plane — complete
- Stage 6: Router rules+nano — complete
- Stage 7: MCP contracts/versioning — complete
- Stage 8: Claude/Codex parity — complete
- Stage 9: UI/ops/hardening — complete

## Production Overhaul (v0.2.0) — 23 Tasks

All complete: async broker, fastembed embeddings, smart chunker, async Qdrant, provider pool with health tracking, managed documents with soft-delete, configurable limits, stdio/SSE/streamable HTTP transports, clap CLI binary.

## Multi-CLI + Installer (v0.2.1)

All complete: Claude Code + Gemini CLI + OpenCode + Codex auto-detection, tiered embedding models (fast/balanced/quality/multilingual), per-CLI custom tools, install.sh one-command installer, build.sh build manager, cross-platform release workflow.

## Tool Catalog Integration (v0.3.0) — 7 Tasks

- Cat-1: SQLite schema + error variant — complete
- Cat-2: ToolCatalog struct with DB, import, query, scan — complete
- Cat-3: API types for 6 new tools — complete
- Cat-4: Broker methods for catalog tools — complete
- Cat-5: Transport dispatch + catalog bootstrap — complete
- Cat-6: Qdrant semantic search for tools — complete
- Cat-7: Changelog, schemas, docs, validation, tag — complete

## Key Features Delivered

### Tool Catalog (v0.3.0)
- SQLite catalog.db with FTS5 full-text search
- Qdrant semantic search over tool descriptions (with FTS5 fallback)
- System inventory scanning (auto-detects installed tools via `which`)
- Per-CLI per-project tool enable/disable state
- 7 new MCP tools: tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update, tool.list
- Curated catalog seed: 16 Rust tools, 4 security tools, 8 official MCPs
- tool.suggest returns grouped results: enabled / available / recommended
- tool.search: semantic (Qdrant) → FTS5 → registry fallback chain

### Production RAG (v0.2.0)
- fastembed-rs with tiered models (384-1024 dim ONNX, offline, free)
- OpenAI-compatible API embedding provider
- Smart markdown chunker (heading-aware, paragraph-safe, code-block preserving)
- Async Qdrant HTTP backend with collection management

### Managed Documents (v0.2.0)
- Full CRUD with soft-delete to .trash/
- Auto-sync on project.refresh
- docs.reindex for full re-ingestion

### Multi-CLI Support (v0.2.1)
- Claude Code, Gemini CLI, OpenCode, Codex
- Per-CLI custom tools
- One-command installer with auto-registration

### MCP Transport (v0.2.0)
- stdio (Claude Code, Gemini, OpenCode, Codex)
- SSE (web clients)
- Streamable HTTP (MCP spec compliant)

### Nano LLM Provider Pool (v0.2.0)
- Up to 5 OpenAI-compatible providers
- Priority / round-robin / latency routing
- Per-provider health tracking with cooldown
- TCP pre-flight checks

## Embedding Model Registry (v0.4.0) — 8 Tasks

- Task 1+2: TOML registry files + models_registry module — complete
- Task 3: Rewrite embeddings.rs to use registry — complete
- Task 4: Update config defaults to quality tier — complete
- Task 5: Add models.list and models.check_updates MCP tools — complete
- Task 6: Interactive model selection in installer — complete
- Task 7: Maintenance scripts — complete
- Task 8: Full integration validation — complete

### Key Features Delivered

- TOML model registries (models/local-models.toml, models/api-models.toml) embedded in binary
- Default changed from all-MiniLM-L6-v2 (384d) to BGE-large-en-v1.5 (1024d)
- Interactive model selection during install (7 local + API option)
- API provider support: OpenAI, Voyage AI, Cohere (extensible)
- 2 new MCP tools: models.list, models.check_updates
- Maintenance scripts for tracking upstream model updates

## Infrastructure (v0.3.1)

- SECURITY.md with vulnerability reporting policy and security design documentation
- Hardened .gitignore (secrets, keys, certs, IDE, OS files)
- Weekly cargo-audit + gitleaks CI (security.yml)
- Manual-only release workflow (workflow_dispatch, no auto-trigger on tags)
- `build.sh release` command for triggering cross-platform builds
- Repo made public — curl one-liner install works
- GitHub Release v0.3.1 with 4 platform binaries (Linux x86, macOS x86+ARM, Windows x86)

## Verification Snapshot

**Captured at:** commit `7857647` (tag `v0.16.0`), 2026-04-12.

- `cargo fmt --check` — passing
- `cargo clippy --workspace --all-targets -- -D warnings` — passing
- `cargo test --workspace` — **466 passing, 1 ignored** (the 1 ignored is the Lever 2 async-batching deferred guard from v0.15.1 — intentional)
- `cargo test --workspace` with all features — **521 passing, 1 ignored**
- `cargo build --release -p the-one-mcp --bin the-one-mcp` — passing
- `bash scripts/release-gate.sh` — passing

## Tool Consolidation (v0.5.0) — 6 Tasks

- Task 1: Add DocsSaveRequest and ToolFindRequest API types — complete
- Task 2: Consolidate 33 tool definitions to 15 — complete
- Task 3: Rewrite dispatch logic with multiplexed admin — complete
- Task 4: Update JSON schemas (63→31) — complete
- Task 5: Update documentation and full validation — complete
- Task 6: Verify token reduction — complete

### Key Changes
- 11 work tools always loaded (memory, docs, tool discovery/lifecycle)
- 4 multiplexed admin tools (setup, config, maintain, observe) with action+params pattern
- `docs.get` + `docs.get_section` → `docs.get` with optional `section` param
- `docs.create` + `docs.update` → `docs.save` (upsert)
- `tool.list` + `tool.suggest` + `tool.search` → `tool.find` with `mode` param
- Estimated token savings: ~1,836 tokens per session (~52% reduction)

## Multimodal + Reranking (v0.6.0) — 3 Bundles

- Bundle 1: fastembed 4→5.13 migration + reranking infrastructure
  - fastembed API drift fixed (Arc<Mutex<>> wrappers, &mut self methods)
  - 6 previously stubbed text model variants now working
  - Reranker model registry (`models/rerank-models.toml`)
  - `TextRerank` via fastembed, jina-reranker-v2-base-multilingual default
  - `reranker_enabled` + `reranker_model` + `rerank_fetch_multiplier` config fields
  - Reranker integrated into `memory.search` via MemoryEngine

- Bundle 2: Image embedding pipeline
  - Image model registry (`models/image-models.toml`) — 5 models
  - `ImageEmbeddingProvider` trait + `FastEmbedImageProvider` implementation
  - Image ingest module: format validation, size limits, EXIF stripping
  - Qdrant `the_one_images` collection with per-project isolation
  - OCR via tesseract wrapper (`image-ocr` feature flag)
  - Thumbnail generation via `image` crate (`image-embeddings` feature flag)
  - 2 new MCP tools: `memory.search_images`, `memory.ingest_image`
  - 3 new maintain actions: `images.rescan`, `images.clear`, `images.delete`
  - Config fields: `image_embedding_enabled/model`, `image_ocr_enabled/language`, `image_thumbnail_enabled/max_px`
  - Limits: `max_image_size_bytes`, `max_images_per_project`, `max_image_search_hits`, `image_search_score_threshold`
  - 4 new JSON schemas (31 → 35), 25 new tests (183 → 208)

- Bundle 3: Documentation + release
  - New user guides: `docs/guides/image-search.md`, `docs/guides/reranking.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - v0.6.0 tagged and cross-platform release triggered

## Hybrid Search + Watcher + UI Gallery (v0.7.0) — 5 Phases

- Phase A: Sparse embeddings trait + BM25/SPLADE
  - `SparseEmbeddingProvider` trait in `the-one-memory`
  - `FastEmbedSparseProvider` using `fastembed::SparseTextEmbedding` with `SPLADEPPV1`
  - Note: fastembed 5.13 calls this "bm25" alias but the model is SPLADE++Ensemble Distil

- Phase B-D: Qdrant hybrid collection + MemoryEngine integration + config
  - `HybridQdrantCollection` with named dense + sparse vector support
  - `MemoryEngine::search_hybrid` fusing both signals with configurable weights
  - Config fields: `hybrid_search_enabled`, `hybrid_dense_weight`, `hybrid_sparse_weight`, `sparse_model`
  - Score normalization: saturation function for sparse scores

- Phase E-F: File watcher + broker wiring
  - `notify 6.1` + `notify-debouncer-mini 0.4` dependencies
  - `crates/the-one-mcp/src/watcher.rs` — background tokio task per project
  - Config fields: `auto_index_enabled`, `auto_index_debounce_ms` (default 2000ms)
  - Watches `.the-one/docs/` (*.md) and `.the-one/images/` (*.png/jpg/jpeg/webp)
  - Events logged; auto-reingestion deferred to v0.7.1

- Phase G: Screenshot image search
  - `ImageSearchRequest.query` changed to `Option<String>`
  - New optional `image_base64` field — base64-encoded image for image→image similarity
  - Mutual exclusion enforced: exactly one of query or image_base64 must be set
  - Decodes base64 → tempfile → embedding → Qdrant query
  - `CoreError::InvalidRequest(String)` added to error enum

- Phase H: Admin UI image gallery
  - `/images` route: thumbnail grid of all indexed images for active project
  - `/images/thumbnail/<hash>` serving with regex security validation on hash
  - `/api/images` JSON endpoint returning image metadata

- Phase I-J (this release): Documentation + release
  - New guides: `docs/guides/hybrid-search.md`, `docs/guides/auto-indexing.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - v0.7.0 tagged and cross-platform release triggered

## Watcher Auto-Reindex + Code Chunker (v0.8.0) — 4 Phases

- Phase A+B: Watcher auto-reindex
  - `ingest_single_markdown(path)` and `remove_by_path(path)` added to `MemoryEngine`
  - `MemoryEngine` HashMap promoted to `Arc<RwLock<...>>` shared between broker and watcher task
  - Watcher tokio task now calls `ingest_single_markdown` on `Create`/`Modify` events and `remove_by_path` on `Remove` events
  - Image events still log-only (auto-reindex deferred to v0.8.1)
  - Integration test: `test_watcher_auto_reindex` with 2s debounce verification

- Phase C: Code chunker core + Rust
  - `ChunkMeta` extended with `language`, `symbol`, `signature`, `line_range` fields
  - `chunk_file(path, content, max_tokens)` dispatcher — selects chunker by extension
  - `split_on_blank_lines` promoted to `pub(crate)` for sharing across chunkers
  - Rust chunker: brace-depth tracking, `impl … for …` detection, all top-level Rust item types
  - `regex 1` added as direct dependency of `the-one-memory`

- Phase D: Python/TypeScript/JavaScript/Go chunkers
  - Python chunker: indentation-based, decorator handling, `async def` support
  - TypeScript/JavaScript chunker: shared engine, template-literal-aware brace tracking
  - Go chunker: method receiver detection (`func (r *T) Method`), paren-block handling for `var`/`const`
  - All 4 chunkers tested: 14 new tests covering edge cases (decorators, receivers, template literals, paren blocks)

- Phase E: Documentation + release
  - New guide: `docs/guides/code-chunking.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - `docs/guides/auto-indexing.md` updated to reflect watcher now does real re-ingestion
  - v0.8.0 tagged and cross-platform release triggered

## Multi-Backend Roadmap (v0.15.0 → v0.16.0)

The v0.16.0 series is a focused infrastructure release that lands full
multi-backend support for both vectors and state. The roadmap was
authored in `docs/plans/2026-04-11-multi-backend-architecture.md`
(design) and `docs/plans/2026-04-11-resume-phase1-onwards.md`
(execution plan). v0.15.0 and v0.15.1 were production-hardening
prerequisites that shipped bundled with the trait extraction.

### Phases and status

| Phase | Name | Status | Commit | Tag |
|-------|------|--------|--------|-----|
| **0** | Trait extraction (`VectorBackend` + `StateStore`) bundled with v0.15.0 hardening + v0.15.1 Lever 1 | ☑ DONE | `5ff9872` | `v0.16.0-rc1` |
| **1** | Broker `state_by_project` cache via `StateStore` trait (call-site migration) | ☑ DONE | `7666439` | `v0.16.0-phase1` |
| **2** | pgvector `VectorBackend` impl + env var parser + startup validator | ☑ DONE | `91ff224` | `v0.16.0-phase2` |
| **3** | `PostgresStateStore` impl with FTS5 → `tsvector` translation | ☑ DONE | `f010ed6` | `v0.16.0-phase3` |
| **4** | Combined Postgres+pgvector single-pool backend | ☑ DONE | — | `v0.16.0-phase4` |
| **5** | Redis `StateStore` with cache/persistent durability modes + `require_aof` enforcement | ☑ DONE | `1dbf6a5` | `v0.16.0-phase5` |
| **6** | Combined Redis+RediSearch single-client backend | ☑ DONE | `1b1b22f` | `v0.16.0-phase6` |
| **7** | Redis-Vector entity/relation parity + v0.16.0 GA release | ☑ DONE | `7857647` | `v0.16.0-phase7` / `v0.16.0` |

### Backend selection scheme (activated at Phase 2)

Four env vars, two per axis, parallel naming:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string, may carry credentials>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string, may carry credentials>
```

- All four unset → SQLite + Qdrant default (the 95% deployment).
- Any asymmetric specification (one TYPE without the other, type without URL, unknown type) → fail loud at startup with `InvalidProjectConfig` and the exact offending value named in the message.
- `postgres-combined` / `redis-combined` are explicit TYPE values, not URL-equality inference; both axes must match and URLs must be byte-identical when either is combined.
- Tuning knobs (HNSW parameters, schema names, Redis prefixes, AOF verification) live in `config.toml`, NOT in env vars. Secrets stay in env vars.
- `{project_id}` substitution in config.toml is literal `.replace` only — no Jinja, no escape hatches.

Full rationale and the per-rule test matrix in `docs/plans/2026-04-11-resume-phase1-onwards.md § Backend selection scheme`.

### Historical plan files

The execution plan (`2026-04-11-resume-phase1-onwards.md`) and
Phase 4 resume prompt (`2026-04-11-resume-phase4-prompt.md`) are
archived in `docs/plans/historical/` as reference for the decision
trail behind the v0.16.0 multi-backend architecture.

## What's Next (post-v0.16.0)

### Near-Term

- **v0.16.1** — Redis-Vector image support (tracked gap from Phase 7). Close the last capability gap so `RedisVectorStore` reaches full parity with Qdrant.
- **Decision D — pgvector hybrid search** — deferred throughout v0.16.0. Benchmark tsvector+GIN vs sparse-array rewrites before shipping. Tracked for a post-GA release.

### Deferred

- **Lever 2 async audit batching** — designed in parallel (`docs/plans/2026-04-10-audit-batching-lever2.md`), explicitly deferred. Triggered only if audit writes become a real bottleneck above the Lever 1 baseline (currently ~83 µs/row, 67x faster than v0.14.3).
- **Cross-backend migration tooling** — "dump from SQLite, load into Postgres" is out of scope. Operators choose a backend at init time; switching later is manual re-ingestion.
- **Multi-broker HA** — the current broker design assumes exclusive access to its state store. HA across multiple brokers is a future feature (would need advisory locks, lease-based ownership, or a Postgres `SELECT ... FOR UPDATE SKIP LOCKED` queue pattern).

### Future

- Web marketplace for browsing, rating, and reviewing catalog tools
- Community-curated "markets" (collections by use case)
- Automated tool discovery from package registries
- Install analytics and usage tracking
