# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Full validation (run before every commit)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build release binary
cargo build --release -p the-one-mcp --bin the-one-mcp

# Run specific crate tests
cargo test -p the-one-core
cargo test -p the-one-memory
cargo test -p the-one-mcp
cargo test -p the-one-router

# Run a single test by name
cargo test -p the-one-core test_create_and_get

# Release gate (full CI validation)
bash scripts/release-gate.sh

# Run the MCP server (stdio)
cargo run -p the-one-mcp --bin the-one-mcp -- serve

# Run production hardening benchmarks (v0.15.0+)
cargo run --release --example production_hardening_bench -p the-one-core

# Run embedded admin UI
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

## Architecture

This is a Rust MCP (Model Context Protocol) broker — a smart intermediary between AI coding assistants (Claude Code, Gemini CLI, OpenCode, Codex) and projects. It provides semantic document search, managed knowledge storage, policy-gated tool execution, and intelligent request routing. All four CLIs use the same server via stdio JSON-RPC.

### Crate Dependency Flow

```
the-one-ui ──> the-one-mcp ──> the-one-core
                    |               ^
                    +──> the-one-memory (uses the-one-core for error types only)
                    +──> the-one-registry ──> the-one-core
                    +──> the-one-router ──> the-one-core
                    
the-one-claude ──> the-one-mcp
the-one-codex  ──> the-one-mcp
```

### Key Design Patterns

**Async broker with sync SQLite**: `McpBroker` methods are all `async fn`. SQLite operations (from `the-one-core`) are synchronous and should be wrapped in `tokio::task::spawn_blocking` when called from async contexts. The broker currently calls them directly (they're fast file-based operations).

**Per-project isolation**: Every project gets its own SQLite DB, manifests, memory engine, and docs manager, keyed by `{project_root}::{project_id}`. `memory_by_project` is `Arc<RwLock<HashMap<String, MemoryEngine>>>` (Arc-wrapped so the watcher task can hold its own reference for auto-reindex). `docs_by_project` uses a plain `tokio::sync::RwLock`-wrapped HashMap.

**Layered config**: Config resolves through 5 layers (defaults → global file → project file → env vars → runtime overrides). All fields are `Option` in the file/overlay structs, with defaults applied at the end in `AppConfig::load()`.

**Transport-agnostic dispatch**: JSON-RPC requests arrive via any transport (stdio/SSE/stream), get deserialized in `transport/jsonrpc.rs`, dispatched to `McpBroker` methods, and serialized back. The broker never knows which transport is in use.

**Embedding provider abstraction**: `EmbeddingProvider` trait with two implementations — `FastEmbedProvider` (local ONNX, tiered: fast/balanced/quality/multilingual) and `ApiEmbeddingProvider` (OpenAI-compatible HTTP). Use `resolve_model("quality")` for tier aliases (quality is the default). The `models_registry` module in `the-one-memory` parses TOML model definitions from `models/local-models.toml` and `models/api-models.toml` (embedded via `include_str!`). The `MemoryEngine` holds a `Box<dyn EmbeddingProvider>`.

**Client-aware tool loading**: The MCP `initialize` handshake carries `clientInfo.name`. The broker reads this to load per-CLI custom tools from `~/.the-one/registry/custom-<client>.json` alongside universal `custom.json` and `recommended.json`.

**Provider pool with health tracking**: Up to 5 nano LLM providers managed by `ProviderPool`. Each has independent `ProviderHealth` with cooldown escalation (5s → 15s → 60s). TCP pre-flight check before every classification. Silent fallback to rules-only routing when all providers fail.

**Tool catalog (SQLite + Qdrant)**: `ToolCatalog` in `the-one-core/src/tool_catalog.rs` manages `catalog.db` — a SQLite database with FTS5 full-text search and Qdrant semantic search. Tools are imported from `tools/catalog/` JSON files on first `project.init`. System inventory (`which` scan) tracks what's installed. Per-CLI enable/disable in `enabled_tools` table. The catalog is global (not per-project), while enabled state is per-CLI per-project. The `std::sync::Mutex<Option<ToolCatalog>>` pattern is used because `rusqlite::Connection` is `!Sync`.

### Error Handling

All crates use `CoreError` from `the-one-core::error`. Library code uses `thiserror`, the binary (`the-one-mcp.rs`) uses `anyhow`. The `MemoryEngine` and `ProviderPool` return `Result<T, String>` internally — the broker maps these to `CoreError::Embedding` or `CoreError::Provider`.

**v0.15.0 wire-level sanitization**: `transport/jsonrpc.rs` converts every `CoreError` into a client-safe envelope via `public_error_message`. `CoreError::Sqlite`/`Io`/`Json`/`Embedding` etc. surface only their `error_kind_label` (`"sqlite"`, `"io"`, …) to clients — never the inner rusqlite/serde/fs text. `InvalidRequest`/`NotEnabled`/`PolicyDenied`/`InvalidProjectConfig` carry deliberately-crafted human-readable messages and pass through verbatim. Every error response carries a `corr=<id>` that matches a `tracing::error!` line in the server log with full internal details. Use `the_one_core::audit::error_kind_label(&err)` for the same labels in audit rows.

### Input Sanitization (v0.15.0+)

All user-supplied names go through `the_one_core::naming`:
- `sanitize_name(value, field)` — wing/hall/room/label/tag. Charset `[A-Za-z0-9 ._\-:]`, max 128. The `:` is allowed for namespaced hook names (`hook:precompact`). Blocks `..`, path separators, leading/trailing dot, control whitespace, non-ASCII.
- `sanitize_project_id(value)` — `[A-Za-z0-9_\-]`, max 64, no leading/trailing dash.
- `sanitize_action_key(value)` — `[A-Za-z0-9_.:\-]`, max 128, no whitespace. Used for action keys AND navigation node IDs.

Every broker write entry point (`memory_ingest_conversation`, `memory_diary_add`, `memory_navigation_upsert_node`, `memory_navigation_link_tunnel`) calls the sanitizer first and returns `InvalidRequest` on failure.

### Pagination & Audit (v0.15.0+)

List/search endpoints use `the_one_core::pagination::PageRequest::decode(limit, cursor, default, max)`. Over-limit requests **return `InvalidRequest`** instead of silently truncating. Per-endpoint caps are declared in `the_one_core::storage::sqlite::page_limits`. Responses carry `next_cursor: Option<String>` — opaque, passed verbatim by clients.

Audit log gets structured outcomes via `the_one_core::audit::{AuditRecord, AuditOutcome}` + `ProjectDatabase::record_audit(&record)`. Schema v7 adds `outcome` and `error_kind` columns. See `docs/guides/production-hardening-v0.15.md` for the full rationale.

## Code Conventions

- `rustfmt` with `max_width = 100`
- `cargo clippy -- -D warnings` must pass (use `is_some_and` not `map_or(false, ...)`, avoid derivable impls, no redundant closures)
- Async tests use `#[tokio::test]`, sync tests use `#[test]`
- Config tests must isolate env vars with `temp_env::with_vars` to prevent pollution between parallel test runs
- JSON schema files in `schemas/mcp/v1beta/` use `$id` prefix `the-one.mcp.v1beta.` and JSON Schema draft 2020-12
- The `fastembed` model downloads on first use (~23-220MB depending on tier) and caches in `.fastembed_cache/` (gitignored)
- `scripts/install.sh` handles full installation: download, config, CLI registration (Claude/Gemini/OpenCode/Codex)
- `scripts/update-local-models.sh` and `scripts/update-api-models.sh` check for new embedding model versions
- `scripts/build.sh` is the build + release manager: `build`, `build --lean`, `dev`, `test`, `check`, `package`, `install`, `release`
- Releases are manual-only via `build.sh release v0.8.0` (triggers GitHub Actions workflow_dispatch, does NOT auto-trigger on tags)
- Tool catalog: `tools/catalog/` (curated JSON), `~/.the-one/catalog.db` (SQLite with FTS5), Qdrant `the_one_tools` collection (semantic)
- Custom tools: `~/.the-one/registry/custom.json` (shared), `custom-<cli>.json` (per-CLI)
- 17 MCP tools (see `crates/the-one-mcp/src/transport/tools.rs`) + 3 MCP resource types (`docs`, `project`, `catalog` via `the-one://` URI scheme, v0.10.0+), 308 tests, 35 schemas, 365 catalog entries across 10 languages + 8 categories
- Hybrid search: `hybrid_search_enabled`, `hybrid_dense_weight`, `hybrid_sparse_weight`, `sparse_model` config fields; requires reindex after enabling
- File watcher: `auto_index_enabled`, `auto_index_debounce_ms` config fields; background tokio task with auto-reingestion for markdown (v0.8.0) AND images (v0.8.2) via `image_ingest_standalone`/`image_remove_standalone` free functions
- Admin UI image gallery: `/images` route, `/images/thumbnail/<hash>`, `/api/images` JSON endpoint
- Screenshot search: `memory.search_images` accepts optional `image_base64` OR `query` (exactly one required)
- Tree-sitter chunker (v0.9.0): `chunk_file(path, content, max_tokens)` dispatcher in `the-one-memory/src/chunker.rs`; 13 languages via tree-sitter (Rust/Python/TS/JS/Go with regex fallback + C/C++/Java/Kotlin/PHP/Ruby/Swift/Zig tree-sitter-only) behind `tree-sitter-chunker` feature (default on); shared walker in `chunker_ts_impl::chunk_with_tree_sitter`; `tree-sitter-kotlin-ng` (not `tree-sitter-kotlin`, which is pinned to tree-sitter 0.20)
- MCP Resources API (v0.10.0): `resources_list`/`resources_read` broker methods in `crates/the-one-mcp/src/resources.rs`; JSON-RPC `resources/list` + `resources/read` handlers in `transport/jsonrpc.rs`; `is_safe_doc_identifier` guards path traversal
- Backup/restore (v0.12.0): `maintain: backup` and `maintain: restore` actions backed by `crates/the-one-mcp/src/backup.rs`; uses `tar 0.4` + `flate2 1`; excludes `.fastembed_cache/` and Qdrant wal/raft; manifest version "1"
- Observability (v0.12.0): `BrokerMetrics` wrapped in `Arc<BrokerMetrics>` so watcher task can clone; 15 metric counters total (7 original + 8 new); `MetricsSnapshotResponse` fields added with `#[serde(default)]`
- Retrieval benchmark (v0.9.0): `crates/the-one-memory/examples/retrieval_bench.rs`; requires running Qdrant; NOT in CI; run manually with `cargo run --release --example retrieval_bench -p the-one-memory --features tree-sitter-chunker`
- Intel Mac (v0.11.0 flag in v0.12.0 release): `local-embeddings-dynamic` feature resolves libonnxruntime at runtime via `brew install onnxruntime`; mutually exclusive with `local-embeddings` (ort-download-binaries)
- Production hardening (v0.15.0): schema v7 adds `outcome`/`error_kind` columns to `audit_events`; navigation node IDs widened from 12 to 32 hex chars; cursor pagination replaces silent truncation in every `list_*`/`search_*` endpoint; `the_one_core::naming` + `the_one_core::pagination` + `the_one_core::audit` modules; stdio integration tests in `crates/the-one-mcp/tests/stdio_write_path.rs`; cross-finding regression tests in `crates/the-one-mcp/tests/production_hardening.rs`; bench in `crates/the-one-core/examples/production_hardening_bench.rs`. See `docs/reviews/2026-04-10-mempalace-comparative-audit.md` and `docs/guides/production-hardening-v0.15.md`.
- Lever 1 audit throughput (v0.15.1): SQLite opens with `PRAGMA synchronous=NORMAL` in WAL mode — 67× faster audit writes (~83µs/row vs ~5.56ms/row). Durability trade-off: safe against process crash (WAL captures every commit), exposed to < 1s of writes on OS crash. Standard choice for modern SQLite apps. See production-hardening-v0.15.md § 14.
- Phase A multi-backend traits (v0.16.0-rc1): `VectorBackend` trait in `the_one_memory::vector_backend` and `StateStore` trait in `the_one_core::state_store`. `MemoryEngine` now holds `Option<Box<dyn VectorBackend>>` (was two concrete Option fields); canonical constructor `MemoryEngine::new_with_backend(embedding_provider, backend, max_chunk_tokens)`. `ProjectDatabase` implements `StateStore`. Diary upsert is now transactionally atomic (FTS5 index + main table in one `unchecked_transaction`). Adding a new backend (pgvector, Postgres state, Redis-AOF) requires only implementing the trait in a new file — no broker changes. See `docs/plans/2026-04-11-multi-backend-architecture.md`, `docs/plans/2026-04-11-resume-phase1-onwards.md` (execution plan for Phases 1–7 with four-var `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` backend selection scheme, fail-loud startup validation, and explicit `postgres-combined`/`redis-combined` TYPE values for single-pool backends), and `docs/guides/multi-backend-operations.md`.
- Phase 1 broker state store cache (v0.16.0-phase1, commit `7666439`): `McpBroker` gains `state_by_project: RwLock<HashMap<String, Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>>>` keyed by `{canonical_root}::{project_id}`. Every broker method that previously called `ProjectDatabase::open(...)` inline now goes through `with_state_store(project_root, project_id, |store| ...)` — a sync-closure helper that is the single chokepoint for all state-store access. The inner `std::sync::Mutex` (not tokio) is deliberate: its guard is `!Send`, so the compiler refuses to hold a backend connection across `.await`, preventing Postgres/Redis connection-pool deadlocks in Phase 3+. Factory construction happens OUTSIDE the outer write lock (double-check under write) so cold-path traffic for one project doesn't serialize cache misses for other projects — load-bearing for Phase 3+ async factories. `pub async fn McpBroker::shutdown()` drains the cache for clean teardown. Two handlers (`memory_ingest_conversation`, `tool_run`) were restructured to split async memory/session work from sync DB work. `sync_navigation_nodes_from_palace_metadata` now takes `&dyn StateStore` instead of `&ProjectDatabase`. The only `ProjectDatabase::open` call site remaining in `broker.rs` is inside `state_store_factory` — Phase 2+ adds branches there without touching any handler. Test coverage: `broker_state_store_cache_reuses_connections` asserts `Arc::ptr_eq` identity across repeated lookups, distinct-project isolation, `with_state_store` routing, and clean `shutdown()` drain.
- Phase 3 PostgresStateStore (v0.16.0-phase3): new `crates/the-one-core/src/storage/postgres.rs` with `PostgresStateStore` implementing every `StateStore` trait method (26 methods, sync trait bridged to async sqlx via `tokio::task::block_in_place` + `Handle::current().block_on` — requires multi-threaded tokio, which the broker binary has by default). Hand-rolled migration runner at `postgres::migrations` mirrors Phase 2's `pg_vector::migrations` pattern; tracking table is `the_one.state_migrations` (distinct from `pgvector_migrations` so Phase 4 combined can share one schema). Two migrations ship: `0000_state_migrations_table.sql` + `0001_state_schema_v7.sql` — the full v7 schema in one pass (no incremental v1..v6 since Postgres has no history). FTS5 → tsvector translation: `diary_entries.content_tsv TSVECTOR` column + GIN index + `websearch_to_tsquery('simple', $1)` for matching with `ts_rank` for ordering, LIKE fallback for empty-tsquery inputs, `upsert_diary_entry` wrapped in a sqlx transaction for atomicity. BIGINT epoch_ms throughout — NO chrono (deferred decision still holds; see Cargo.toml `pg-vectors` comment). SQL translations: `strftime('%s','now')*1000` → Rust-side `SystemTime::duration_since(UNIX_EPOCH)`, `INTEGER PRIMARY KEY AUTOINCREMENT` → `BIGSERIAL PRIMARY KEY`, `ON CONFLICT DO UPDATE` is portable, `FTS5 MATCH` → `content_tsv @@ websearch_to_tsquery`. New `CoreError::Postgres(String)` variant + `"postgres"` label in `error_kind_label`. New `StatePostgresConfig` in `the_one_core::config` mirroring `VectorPgvectorConfig` (schema, statement_timeout_ms, 5 pool-sizing fields). `McpBroker::state_store_factory` is now `async` and branches on `BackendSelection.state` — `Sqlite` unchanged, `Postgres` routes through `construct_postgres_state_store`, `Redis`/`*-combined` return `NotEnabled` until their phases ship. `get_or_init_state_store` now `.await`s the factory; the cold-path construct-outside-write-lock pattern from Phase 1 is now load-bearing. New Cargo feature `pg-state` on `the-one-core` (with sqlx 0.8.6 + tokio as optional deps) + passthrough on `the-one-mcp`. Composable with `pg-vectors`: `--features pg-state,pg-vectors` for split-pool Postgres state + pgvector. Integration tests in `crates/the-one-core/tests/postgres_state_roundtrip.rs` (11 tests, gated on `THE_ONE_STATE_TYPE=postgres` + `THE_ONE_STATE_URL`, skip gracefully via `return`; run with `--test-threads=1` because every test drops and recreates the `the_one` schema). Test count: 464 → 466 baseline (+2 config), 464 → 484 with `--features pg-state,pg-vectors` (+20 total). See `docs/guides/production-hardening-v0.15.md` § 16 and `docs/guides/multi-backend-operations.md`.
- Phase 2 pgvector backend + env var parser (v0.16.0-phase2): new `crates/the-one-memory/src/pg_vector.rs` with `PgVectorBackend` implementing the full `VectorBackend` trait (dense-only; hybrid = Decision D deferred to Phase 2.5). Batched upserts via `INSERT ... SELECT * FROM UNNEST(...)`. Per-search `SET LOCAL hnsw.ef_search` inside a transaction. Defensive `preflight_vector_extension` with targeted errors for Supabase / AWS RDS / Cloud SQL / Azure / self-hosted. **Hand-rolled migration runner** at `pg_vector::migrations` (not `sqlx::migrate!`) because sqlx's `migrate` and `chrono` features transitively reference `sqlx-sqlite?/…` weak-deps that cargo's `links` conflict check pulls into the graph, colliding with `rusqlite 0.39`'s `libsqlite3-sys 0.37`. Five migrations ship with hardcoded `dim=1024` (Decision C — BGE-large-en-v1.5 quality tier). New `the_one_core::config::backend_selection` submodule parses the four-variable `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` env surface with 8 negative + 4 positive tests. `McpBroker` gains `backend_selection` field + `try_new_with_policy` fail-loud constructor; new `build_pgvector_memory_engine` fast-path short-circuits the legacy `config.vector_backend` branch when `BackendSelection.vector == Pgvector`. Legacy Qdrant/Redis paths untouched. New Cargo feature `pg-vectors` (off by default) on `the-one-memory` + passthrough on `the-one-mcp`; sqlx features narrowed to `[runtime-tokio, tls-rustls, postgres, macros]` after bisection. Integration tests in `crates/the-one-memory/tests/pgvector_roundtrip.rs` (8 tests, gated on `THE_ONE_VECTOR_TYPE=pgvector` + `THE_ONE_VECTOR_URL`, skip gracefully via `return`). New bench `crates/the-one-memory/examples/pgvector_bench.rs` for throughput and latency. Test count: 450 → 464 baseline, 450 → 469 with `--features pg-vectors`. See `docs/plans/2026-04-11-resume-phase2-prompt.md`, `docs/guides/production-hardening-v0.15.md` § 15, and `docs/guides/multi-backend-operations.md`.
