# Upgrade Guide

> Breaking changes and migration notes for the-one-mcp version upgrades.

---

## Upgrading to v0.16.1 (from v0.16.0)

v0.16.1 is a **strongly recommended** patch release for anyone running
the-one-mcp under a strict MCP client (Claude Code, Claude Desktop,
Gemini CLI, or any client that validates incoming JSON-RPC frames
against the MCP schema).

### What's fixed

1. **JSON-RPC notifications no longer produce a response frame.**
   v0.16.0's stdio dispatcher replied to every notification — including
   the client's `notifications/initialized` that immediately follows
   the `initialize` handshake — with an out-of-spec frame
   `{"jsonrpc":"2.0","result":null}` (no `id`, no `method`, bare
   `result`). Strict clients rejected it and dropped the session, so
   under Claude Code v0.16.0 sessions connected, handshook, and died
   in ≈ 30 ms. v0.16.1's dispatcher returns `Option<JsonRpcResponse>`,
   and all three transports (stdio/sse/stream) suppress output for
   the `None` case. Stdio writes nothing; HTTP transports return
   `202 Accepted`.

2. **`--version` and `serverInfo.version` now report the real release.**
   Prior to v0.16.1 the workspace `Cargo.toml` still carried the
   scaffold version `0.1.0`, so `the-one-mcp --version` printed
   `the-one-mcp 0.1.0` on every release from v0.1.0 through v0.16.0.
   The MCP `initialize` handshake separately advertised
   `serverInfo.version: "v1beta"` (the schema tag, not the software
   version). v0.16.1 aligns both: `Cargo.toml` → `0.16.1` and
   `serverInfo.version` → `env!("CARGO_PKG_VERSION")`.

### Required action

- **Upgrade the binary.** Run the one-line installer or rebuild from
  source. For local/WSL setups:
  ```bash
  cargo build --release -p the-one-mcp --bin the-one-mcp
  install -m 755 target/release/the-one-mcp ~/.the-one/bin/the-one-mcp
  the-one-mcp --version   # expect: the-one-mcp 0.16.1
  ```
- **Restart MCP client sessions** (Claude Code, Gemini CLI, etc.) so
  they spawn the new binary.

### Spotting the v0.16.0 symptom

If you were running v0.16.0 under Claude Code, `claude mcp list` showed
the server as ✓ Connected (that probe only tests the handshake), but
real sessions logged this sequence under
`~/.cache/claude-cli-nodejs/<project>/mcp-logs-the-one-mcp/*.jsonl`:

```
Successfully connected (transport: stdio)
Connection established with capabilities: { ... "serverVersion": {"name":"the-one-mcp","version":"v1beta"} }
STDIO connection dropped after 0s uptime
Connection error: [ZodError ... "Unrecognized key: \"result\"" ...]
Closing transport (stdio transport error: ZodError)
```

After upgrading to v0.16.1, the same log shows a clean handshake,
`serverVersion: "0.16.1"`, and a clean shutdown.

### No breaking changes

- Tool and resource shapes are identical to v0.16.0.
- Config-file format, CLI adapters, and HTTP endpoints are unchanged.
- `MCP_SCHEMA_VERSION` (`"v1beta"`) is unchanged — the OpenAPI
  swagger path and `report.config.schema_version` field still carry
  the schema tag, which is the correct semantic for those surfaces.

---

## Upgrading to v0.16.0 GA (from v0.16.0-phase4)

### New features (opt-in, non-breaking)

- **Phase 5 — Redis StateStore** with two durability modes: cache
  (`require_aof=false`, volatile) and persistent (`require_aof=true`,
  verifies `aof_enabled:1` at startup). All 26 `StateStore` trait
  methods implemented via HSET, Redis Streams, sorted sets, and
  RediSearch `FT.SEARCH`. New Cargo feature `redis-state`. New
  `CoreError::Redis(String)` variant. New `StateRedisConfig` in
  `config.json`.
- **Phase 6 — Combined Redis+RediSearch** single-client backend.
  Parallel shape to Phase 4's Postgres combined: one `fred::Client`
  shared between `RedisStateStore` and `RedisVectorStore`. Activated
  via `THE_ONE_STATE_TYPE=redis-combined` +
  `THE_ONE_VECTOR_TYPE=redis-combined` with byte-identical URLs.
- **Phase 7 — Redis-Vector entity/relation parity.** `RedisVectorStore`
  now supports entities and relations (was chunks-only). Each type gets
  its own RediSearch index. Images remain unsupported on Redis (tracked
  for v0.16.2).

### Required action

- **None.** Existing deployments (SQLite, Postgres split/combined)
  keep working unchanged. Redis backends are strictly additive.

### To adopt Redis state

1. Stand up Redis 7+ with the RediSearch module loaded.
2. Export env vars:
   ```bash
   export THE_ONE_STATE_TYPE=redis
   export THE_ONE_STATE_URL=redis://localhost:6379
   export THE_ONE_VECTOR_TYPE=qdrant  # or pgvector, etc.
   export THE_ONE_VECTOR_URL=...
   ```
3. Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp --features redis-state`
4. For persistent mode, set `state_redis.require_aof = true` in `config.json`
   and ensure your Redis instance has `appendonly yes`.

### To adopt combined Redis

Same binary as above with both `redis-state` and `redis-vectors` features.
Set both TYPEs to `redis-combined` with byte-identical URLs.

### No breaking changes

- Broker API, MCP tool shapes, config-file format, and CLI adapters
  are unchanged.

---

## Upgrading to v0.16.0-phase4 (from v0.16.0-phase3)

### New features (opt-in, non-breaking)

- **Combined Postgres+pgvector backend** — one `sqlx::PgPool` serving
  both the `StateStore` trait role and the `VectorBackend` trait role
  against a single Postgres database. Activated via
  `THE_ONE_STATE_TYPE=postgres-combined` +
  `THE_ONE_VECTOR_TYPE=postgres-combined` with byte-identical
  `THE_ONE_STATE_URL` and `THE_ONE_VECTOR_URL`. The env-var parser
  already enforced these matching + equality rules in Phase 2; Phase 4
  replaces the previously-`NotEnabled` factory branches with real
  constructors.
- **No new Cargo feature.** The combined dispatcher is active
  whenever both `pg-state` and `pg-vectors` are compiled in. No
  rebuild beyond the standard split-pool Phase 2/3 build is required
  to get combined support — it's the same binary, selected at runtime
  via env vars.
- **New `PgVectorBackend::from_pool` and `PostgresStateStore::from_pool`
  constructors** — sync wrappers that take a pre-built `sqlx::PgPool`
  and config, skip connect + preflight + migrations, and wrap the
  pool in a trait object. Used internally by the combined path; safe
  to use from downstream code if you want to share a pool across
  subsystems.
- **New module** `the_one_mcp::postgres_combined` exposing
  `build_shared_pool`, `mirror_state_postgres_config`, and
  `mirror_pgvector_config`. Gated on `all(pg-state, pg-vectors)`.
- **Phase 3 TODO resolved**: `construct_postgres_state_store` now
  reads `AppConfig::state_postgres` via the mirror helper instead of
  Phase 3's stub `PostgresStateConfig::default()`. Operators who were
  tuning `state_postgres` in `config.json` will now see their values
  take effect on the split-pool path too (previously ignored).

### Required action

- **None.** Existing SQLite, Postgres split-pool, and pgvector split-
  pool deployments keep working unchanged. Combined is strictly
  additive.

### To adopt the combined Postgres+pgvector backend

If you're already running Phase 2 + Phase 3 split-pool against the
**same** database (two sqlx pools pointing at one DSN), the upgrade
is a zero-data-copy env-var swap:

1. Shut down the broker cleanly.
2. Verify `THE_ONE_STATE_URL` and `THE_ONE_VECTOR_URL` are
   byte-identical (same host, port, database, credentials, query
   params, everything).
3. Change both TYPEs to `postgres-combined`:
   ```bash
   export THE_ONE_STATE_TYPE=postgres-combined
   export THE_ONE_VECTOR_TYPE=postgres-combined
   ```
4. Restart. The broker constructs one shared pool instead of two
   split pools; both migration runners are idempotent and detect
   their already-applied state. Data is untouched — both trait roles
   continue to read and write the same schema, just through one pool.

If your split-pool deployment currently points at **different**
databases for state and vectors, combined is not a seamless upgrade
— you have to pick one database as the winner and move the other
side's data in via `pg_dump` / `pg_restore`. See
[`combined-postgres-backend.md § 8`](combined-postgres-backend.md#8-migration-from-split-pool-postgres--different-databases)
for the exact commands.

### statement_timeout inheritance (combined path only)

On combined deployments, the shared pool's `after_connect` hook
wires `SET statement_timeout = '<state_postgres.statement_timeout_ms>ms'`
on every checked-out connection — which means **vector queries
inherit the state-side timeout** even though the split-pool pgvector
path had no equivalent hook at all.

**Practical impact**: if you're migrating from split-pool pgvector
(no statement timeout) to combined and your corpus is large enough
that some vector searches take more than 30 seconds (the default),
bump `state_postgres.statement_timeout_ms` in `config.json` before
flipping the env vars. The default (30 s) is comfortable for corpora
under ~5M vectors on typical hardware.

### Pool sizing rule (combined path only)

When combined is active, `state_postgres`'s pool-sizing fields win:
`max_connections`, `min_connections`, the acquire/idle/max_lifetime
timeouts, and `statement_timeout_ms` all apply to the shared pool.
`vector_pgvector`'s corresponding fields are **ignored** on this
path. HNSW tuning still comes from `vector_pgvector` because
`hnsw_m`, `hnsw_ef_construction` are migration-time settings and
`hnsw_ef_search` is applied per-search — neither is a pool setting.

### No breaking changes

- Broker API, MCP tool shapes, JSON-RPC wire format, config-file
  format, and CLI adapters are unchanged.
- The `StateStore` and `VectorBackend` trait signatures are
  unchanged — no new trait methods ship in Phase 4.
- Split-pool Postgres and split-pool pgvector paths are untouched
  and keep working.

Full operational reference in the new standalone
[combined Postgres backend guide](combined-postgres-backend.md).

---

## Upgrading to v0.16.0-phase3 (from v0.16.0-phase2)

### New features (opt-in, non-breaking)

- **`PostgresStateStore`** — full `StateStore` trait on Postgres (all 26
  methods). Tsvector FTS replaces FTS5, schema v7 parity from day one,
  BIGINT epoch_ms throughout. Activated via
  `THE_ONE_STATE_TYPE=postgres` + `THE_ONE_STATE_URL=<dsn>`, off by default.
- **New Cargo feature `pg-state`** on `the-one-core` + passthrough on
  `the-one-mcp`. Composable with `pg-vectors`:
  `--features pg-state,pg-vectors` ships split-pool Postgres on both axes.
- **New `CoreError::Postgres(String)` variant** for backend runtime
  errors. Short label `"postgres"` surfaced to clients; full text in
  `tracing::error!` logs.
- **`state_store_factory` is now `async`** — Phase 1's doc comment
  pre-announced this. Internal broker change only; handlers unaffected.

### Required action

- **None.** Existing SQLite + Qdrant deployments keep working unchanged.
  New features are opt-in via env vars + Cargo features.

### To adopt PostgresStateStore

1. Stand up Postgres ≥ 13 (no extensions required — uses only native
   features). Managed Postgres (RDS, Cloud SQL, Azure, Supabase) works.
2. Export the env vars:
   ```bash
   export THE_ONE_STATE_TYPE=postgres
   export THE_ONE_STATE_URL=postgres://user:pw@db.internal:5432/the_one
   ```
3. Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-state`
4. First boot applies the migrations automatically. See
   [Postgres state backend guide](postgres-state-backend.md) for the full
   sequence.

### Migration from SQLite

Not automated. Switching from SQLite to Postgres requires re-running
`project.init` against the new backend — existing
audit/profile/diary history in the old SQLite DB is NOT carried over.
See the guide for the manual cutover procedure.

### No breaking changes

Broker API, MCP tool shapes, config-file format, and CLI adapters are
unchanged.

---

## Upgrading to v0.16.0-phase2 (from v0.16.0-phase1)

### New features (opt-in, non-breaking)

- **`PgVectorBackend`** — first real alternative `VectorBackend` after the
  Phase A trait extraction. Implements chunks + entities + relations on
  pgvector (hybrid search deferred to Phase 2.5 as Decision D). Activated
  via `THE_ONE_VECTOR_TYPE=pgvector` + `THE_ONE_VECTOR_URL=<dsn>`, off
  by default.
- **New `the_one_core::config::backend_selection` module** — parses the
  four-variable `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` env surface with
  fail-loud validation (1 `BackendSelection::from_env` call covering all
  § 3 rules from the backend selection scheme). See the "Multi-Backend
  Selection" section of `configuration.md`.
- **New `VectorPgvectorConfig`** in the config stack. Schema, HNSW
  tunables, 5 sqlx pool-sizing fields with production-sane defaults.
- **New Cargo feature `pg-vectors`** on `the-one-memory` + passthrough
  on `the-one-mcp`. Off by default.
- **New `CoreError::Postgres`** variant landed in Phase 3, but Phase 2's
  pgvector code maps its errors to String-internal `VectorBackend` trait
  results and then `CoreError::Embedding` at the broker boundary.

### Required action

- **None.** Existing Qdrant deployments keep working unchanged.

### To adopt pgvector

1. Stand up Postgres ≥ 13 with the `vector` extension available (Supabase
   has it pre-installed; AWS RDS needs it in `shared_preload_libraries`;
   see [pgvector backend guide](pgvector-backend.md) for per-provider
   setup).
2. Export:
   ```bash
   export THE_ONE_VECTOR_TYPE=pgvector
   export THE_ONE_VECTOR_URL=postgres://user:pw@db.internal/the_one
   ```
3. Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-vectors`
4. First boot runs `preflight_vector_extension` (defensive check with
   targeted per-provider errors) and applies the five migration files.

### Vector dimension is locked at 1024

Decision C (locked in during the Phase 2 brainstorm) hardcodes `dim=1024`
in every `vector(...)` literal in the migration SQL. This matches the
default quality-tier embedding provider (BGE-large-en-v1.5). The backend
constructor refuses to start if the live provider reports a different
dim. If you need a different dimension, it's a new migration, not a
runtime setting.

### No breaking changes

---

## Upgrading to v0.16.0-phase1 (from v0.16.0-rc1)

### Internal refactor only — no new features

The broker's state-store access is now routed through a
`state_by_project` cache via the Phase A `StateStore` trait. All 16
`ProjectDatabase::open` call sites in `broker.rs` now go through a new
`with_state_store(project_root, project_id, |store| ...)` sync closure.
The inner lock is `std::sync::Mutex` (deliberately not tokio) so the
guard is `!Send` — the compiler refuses to hold a backend connection
across `.await`, which prevents the Postgres/Redis connection-pool
deadlock pattern that will bite in Phase 3+.

### Required action

- **None.** Zero user-visible behaviour change. Tests passed 449 → 450
  (+1 cache-reuse test). Existing projects work unchanged.

---

## Upgrading to v0.16.0-rc1 (from v0.15.1)

### Internal refactor — Phase A multi-backend trait extraction

- New `trait VectorBackend` in `the_one_memory::vector_backend` (chunks,
  entities, relations, hybrid, persistence verification).
- New `trait StateStore` in `the_one_core::state_store` (all 22
  broker-called methods on `ProjectDatabase`).
- `MemoryEngine` now holds `Option<Box<dyn VectorBackend>>` (was two
  concrete `Option<T>` fields). Canonical constructor
  `MemoryEngine::new_with_backend(embedding_provider, backend,
  max_chunk_tokens)`.
- `impl VectorBackend for AsyncQdrantBackend` (full) + `impl
  VectorBackend for RedisVectorStore` (chunks-only, feature-gated).
- `impl StateStore for ProjectDatabase` (thin forwarding, zero
  behaviour change).
- **Diary upsert atomicity fix**: main INSERT + DELETE FTS + INSERT FTS
  wrapped in one `unchecked_transaction`. If your code inspects partial
  diary FTS states, you may observe fewer transient inconsistencies.

### Required action

- **None.** v0.16.0-rc1 is a pure refactor — every v0.15.x test passes
  bit-for-bit against it. Upgrade by rebuilding.

### No breaking changes

Broker API, MCP tool shapes, config-file format, and CLI adapters are
unchanged.

### Note: v0.15.0 + v0.15.1 + v0.16.0-rc1 bundled

These three versions shipped as one commit (`5ff9872`) because three
files carried interleaved changes across all three. The tag
`v0.16.0-rc1` points at that commit, as do `v0.15.0` and `v0.15.1`.
CHANGELOG.md has per-version sections.

---

## Upgrading to v0.15.1 (from v0.15.0)

### Lever 1 — `synchronous=NORMAL` on WAL mode

- `ProjectDatabase::open` now sets `PRAGMA synchronous=NORMAL` after
  enabling WAL. Measured **67× faster** audit writes
  (5.56 ms → 83 µs per row).
- Durability trade-off: safe against process crash (WAL captures every
  commit), exposed to **< 1 s of writes on OS crash**. This is the
  standard modern-SQLite production setting used by Firefox, Android,
  rqlite, Litestream, and Turso.
- Two regression tests added (throughput smoke + cross-cutting guard).

### Required action

- **None.** If your threat model genuinely requires `synchronous=FULL`
  (e.g. storing financial transactions), file an issue — we'd add a
  `storage_synchronous_mode` config knob. Until then the default is
  `NORMAL` and the `production_hardening_bench.rs` results only make
  sense against that setting.

### No breaking changes

---

## Upgrading to v0.15.0 (from v0.14.3)

### Production hardening pass

v0.15.0 addresses every finding from
`docs/reviews/2026-04-10-mempalace-comparative-audit.md` (C1–C5, H1–H5,
M1–M6). See [production-hardening-v0.15.md](production-hardening-v0.15.md)
for the full 15-section writeup. Highlights:

- **Cursor pagination** replaces silent truncation on every list/search
  endpoint. **Potentially breaking** for clients that pass oversized
  `limit` values: over-limit requests now return `CoreError::InvalidRequest`
  instead of silently clamping. Fix: read `CURSOR_MAX` per endpoint from
  `the_one_core::storage::sqlite::page_limits` and respect the cap, or
  pass `0` to use the endpoint's default.
- **Input sanitization** at every broker write entry point via
  `sanitize_name` / `sanitize_project_id` / `sanitize_action_key`.
  Previously-loose names (e.g. leading `.`, embedded path separators,
  non-ASCII) now fail-loud with `InvalidRequest`.
- **Error envelope sanitization** — internal `rusqlite::Error` /
  `serde_json::Error` / `std::io::Error` text no longer leaks to clients.
  Only the kind label (`"sqlite"`, `"io"`, `"json"`, ...) + a
  correlation ID is surfaced. Full text stays in `tracing::error!` logs.
- **Navigation node digest widened** 12 → 32 hex chars (48 → 128 bits).
  Existing rows with 12-char digests keep working — the widening
  applies to new insertions only.
- **Schema v7** adds `outcome` + `error_kind` columns to `audit_events`
  with indexes. Existing v6 rows get `outcome='unknown'` and
  `error_kind=NULL` automatically.

### Required action

- **Audit your client code for oversized `limit` values.** If you pass
  `limit=10000` to a list endpoint whose `_MAX` is 500, v0.14.x silently
  truncated; v0.15.0 returns `InvalidRequest`. Fix: use the endpoint's
  `_DEFAULT` (pass `limit: 0`) or cap to `_MAX`.
- **Review any client code that parses error messages** — the
  sanitizer only passes `InvalidRequest`, `NotEnabled`, `PolicyDenied`,
  and `InvalidProjectConfig` verbatim. Everything else becomes the
  short label + correlation ID.

### New guide

[production-hardening-v0.15.md](production-hardening-v0.15.md) is the
definitive reference for this release. Read § 1–§ 13 for the original
v0.15.0 hardening, § 14 for v0.15.1 Lever 1, § 15 for v0.16.0-phase2
pgvector, § 16 for v0.16.0-phase3 Postgres state, § 17 for v0.16.0-rc1
Phase A trait extraction.

---

## Upgrading to v0.14.3 (from v0.14.2)

### New features (non-breaking)

- **MemPalace profile presets** via `config` action `profile.set`:
  - `off`, `core`, `full`
  - single switch that maps deterministically to all MemPalace flags
- **AAAK feature family**:
  - `memory.aaak.compress`
  - `memory.aaak.teach`
  - `memory.aaak.list_lessons`
- **Diary feature family**:
  - `memory.diary.add`, `list`, `search`, `summarize`
  - refresh-safe identity (`project_id + entry_date`)
- **Navigation feature family**:
  - `memory.navigation.upsert_node`, `link_tunnel`, `list`, `traverse`
  - project-scoped node/tunnel integrity
- **Hook capture first-class flow**:
  - `maintain` action `memory.capture_hook` for `stop` / `precompact`

### Required action

- **None.** Existing projects continue to work without changes.

### Optional actions

- Set an explicit MemPalace preset for each project with `config: profile.set`
  to avoid ambiguous per-flag drift.
- If adopting AAAK/diary/navigation on existing projects, run `setup` action
  `refresh` once after upgrade.

### No breaking changes

- Existing tools and request shapes remain valid.
- New behavior is additive and gated by MemPalace profile/flags.

---

## Upgrading to v0.14.0 (from v0.13.x)

### New features (non-breaking)

- **Catalog expansion to 365 tools** — all 10 language files and all 8
  category files are now fully populated. +248 new entries from the baseline
  117. Covers Rust, Python, JS/TS, Go, Java, Kotlin, Ruby, PHP, Swift, C/C++
  plus cross-language categories: security, CI/CD, testing, databases, cloud,
  docs, monitoring, automation.
- Run `maintain (action: tool.refresh)` to import the new entries into your
  local `catalog.db` so `tool.find` can discover them.

### Required action

- **None.** The catalog JSON ships in the binary; `tool.refresh` imports it.

---

## Upgrading to v0.13.1 (from v0.13.0)

### New features (non-breaking)

- **Full LightRAG parity** — 6 features that were missing in v0.13.0 are now
  shipped: entity name normalization, entity/relation description vector store,
  description summarization, query keyword extraction, gleaning pass, and canvas
  force-directed graph visualization. See the updated [Graph RAG guide](graph-rag.md)
  for the full parity matrix.
- **New env vars** for the Graph RAG pipeline: `THE_ONE_GRAPH_GLEANING_ROUNDS`
  (default 1), `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` (default 2000),
  `THE_ONE_GRAPH_QUERY_EXTRACT` (default true).

### Required action

- **None.** All new features are disabled by default (behind `THE_ONE_GRAPH_ENABLED`).
  Existing graph data is forward-compatible.

### Optional actions

- **Re-run extraction** if you populated the graph in v0.13.0 — the new
  normalization + gleaning will produce a cleaner, more complete graph.
- **Check graph viz** at http://localhost:8788/graph after running extraction.
  The canvas renderer replaces the v0.13.0 placeholder.

### No breaking changes

- `MemoryEngine::search_graph` is now async but all callers are in async contexts
- `MemoryEngine` has a new `project_id` field (defaults to `None`, no external effect)

---

## Upgrading to v0.12.0 (from v0.10.x or earlier)

### New features (non-breaking)

- **Intel Mac `local-embeddings-dynamic` feature flag:** Intel Mac users can
  now get local embeddings by installing `libonnxruntime` via Homebrew and
  building with the new feature flag. Default Intel Mac binaries still ship
  lean — no behaviour change unless you opt in. See [INSTALL.md](../../INSTALL.md#intel-mac-local-embeddings-v0110).
- **Observability deep dive:** `observe: action: metrics` now returns 8
  additional counters plus a derived `memory_search_latency_avg_ms`. All
  new fields are `#[serde(default)]` so existing deserializers keep working.
  See the new [Observability Guide](observability.md).
- **Backup / restore via `maintain: backup` and `maintain: restore`:**
  gzipped tar of your project state, catalog, and registry. See the new
  [Backup & Restore Guide](backup-restore.md).

### Required action

- **None.** v0.12.0 adds features; nothing is removed or renamed.

### Optional actions

- **Run `observe: metrics` after a day of normal use** to see the new
  counters populate. Use the [Observability Guide](observability.md) to
  interpret the numbers.
- **Take your first backup:** ask your AI CLI _"Back up this project to
  ~/Desktop/my-project.tar.gz"_ to exercise the new `maintain: backup`
  flow.

### No breaking changes

- Tool count unchanged at 17
- Existing `maintain` actions unchanged; only two new ones added (`backup`, `restore`)
- Existing `MetricsSnapshotResponse` fields unchanged; new fields are additive

---

## Upgrading to v0.10.0 (from v0.9.x)

### New features (non-breaking)

- **MCP Resources API:** the `initialize` handshake now advertises a
  `resources` capability and new `resources/list` / `resources/read`
  JSON-RPC methods are available. See the new [MCP Resources Guide](mcp-resources.md).
- **`the-one://` URI scheme:** three resource types — `docs/<path>`,
  `project/profile`, `catalog/enabled`.
- **Catalog expansion:** 117 → 184 tools across 10 languages (added
  Kotlin, Ruby, PHP, Swift, C++ language files; expanded Python and
  JavaScript).
- **Landing page scaffold** under `docs-site/` — ready for GitHub Pages
  enablement.

### Required action

- **None.** MCP clients that don't know about resources simply ignore the
  capability flag in `initialize`.

### Optional actions

- **Claude Code users:** your client will automatically pick up the new
  resources and surface indexed docs in its `@`-picker.
- **To enable GitHub Pages for the landing page:** go to Settings → Pages,
  set source to `main` branch / `/docs-site` folder. See
  `docs-site/README.md`.

### No breaking changes

- Tool count unchanged at 17
- Resources are a separate JSON-RPC surface and do not affect `tools/*`
- Config schema unchanged

---

## Upgrading to v0.9.0 (from v0.8.x)

### New features (non-breaking)

- **Tree-sitter AST chunker:** the 5 original languages (Rust, Python,
  TypeScript/TSX, JavaScript, Go) now use tree-sitter as their primary
  chunker with transparent regex fallback on parse failure. 8 new
  languages added: C, C++, Java, Kotlin, PHP, Ruby, Swift, Zig. See the
  updated [Code Chunking Guide](code-chunking.md).
- **Retrieval benchmark suite:** new `retrieval_bench.rs` example
  measures 4 retrieval configurations against 3 query sets. Not part of
  CI — run manually against a local Qdrant. See `benchmarks/README.md`.
- **New feature flag `tree-sitter-chunker`** (default on). Lean builds
  can disable for a smaller binary at the cost of only getting the
  v0.8.0 regex chunkers for 5 languages.

### Required action

- **None.** All changes are additive. The dispatcher transparently falls
  back to regex on tree-sitter parse failure for the 5 original languages.

### Optional actions

- **Re-index code-heavy projects** with `setup: action: refresh` to get
  tree-sitter-parsed chunks with richer symbol metadata.

### No breaking changes

- Tool count unchanged at 17
- Config schema unchanged
- Existing ChunkMeta consumers unaffected

---

## Upgrading to v0.8.2 (from v0.8.0 / v0.8.1)

### New features (non-breaking)

- **Image auto-reindex:** the file watcher now re-ingests changed image
  files (PNG/JPG/JPEG/WebP) into the Qdrant image collection in addition
  to markdown files. No config change needed — if you had
  `auto_index_enabled: true` and `image_embedding_enabled: true`, image
  changes are now picked up automatically.

### Required action

- **None.**

### No breaking changes

- API surface unchanged
- Config schema unchanged

---

## Upgrading to v0.8.0 (from v0.7.x)

### New features (non-breaking)

- **Watcher auto-reindex:** if you had `auto_index_enabled: true` in v0.7.x, it will now actually re-ingest changed `.md` files instead of just logging. No action needed.
- **Code-aware chunker:** automatic — code files (`.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.go`) are now chunked by function/class/struct instead of as plain text. Takes effect on the next `setup:refresh`.
- **Extended ChunkMeta:** search results now carry `language`, `symbol`, `signature`, `line_range` fields when chunks came from code files. Older consumers ignore these fields.

### Re-index recommended (not required)

If your project has code files that were previously indexed, running `setup:refresh` will re-chunk them with the new language-aware chunker. This substantially improves retrieval quality for function/class/struct searches.

```bash
# Via MCP (from the LLM):
# Call `setup` tool with action: "refresh"
```

### No breaking changes

- Tool count unchanged at 17
- Config schema unchanged
- Existing ChunkMeta consumers unaffected (new fields are Option with None default for markdown chunks)

---

## Upgrading to v0.6.0 (from v0.5.0)

### New Features (no migration needed)

All new features in v0.6.0 are **opt-in** or **additive**. Existing deployments continue to work without any config changes.

- **Image search** — semantic search over PNG/JPG/WebP files (`memory.search_images`, `memory.ingest_image`). Off by default.
- **Cross-encoder reranking** — 15–30% precision improvement for `memory.search`. Off by default.
- **6 additional text embedding models** — all available immediately via `config:models.list`. No action needed.

---

### Tool count: 15 → 17

Two tools were added in v0.6.0. No tools were removed or renamed.

| New Tool | Description |
|---|---|
| `memory.search_images` | Semantic search over indexed images |
| `memory.ingest_image` | Manually index a single image file |

AI CLI sessions discover tools at startup via `tools/list`. They will see the two new tools automatically on reconnect — no registration step needed.

---

### New config fields (all optional)

All new fields have defaults that preserve v0.5.0 behavior. You do not need to add any of them unless you want to opt in.

| Field | Default | Description |
|---|---|---|
| `image_embedding_enabled` | `false` | Enable image search feature |
| `image_embedding_model` | `"nomic-vision"` | Image embedding model |
| `image_ocr_enabled` | `false` | Enable OCR text extraction from images |
| `image_ocr_language` | `"eng"` | Tesseract language code |
| `image_thumbnail_enabled` | `true` | Generate thumbnails for indexed images |
| `image_thumbnail_max_px` | `512` | Max thumbnail dimension in pixels |
| `reranker_enabled` | `false` | Enable cross-encoder reranking |
| `reranker_model` | `"jina-reranker-v2-base-multilingual"` | Reranker model name |
| `limits.image_search_score_threshold` | `0.25` | Minimum score for image search results |
| `limits.max_image_size_bytes` | `10485760` | Maximum image file size (10MB) |

---

### fastembed 4 → 5.13 (internal upgrade)

The underlying embedding library was upgraded from fastembed 4 to 5.13. This is transparent — the model cache format is compatible and no re-download is required for existing text models.

**Exception:** If you were using the Jina reranker in a pre-release v0.6.0 build, the reranker model may need to be re-downloaded. Delete the cached file and let it re-download on next use:

```bash
rm -rf .fastembed_cache/jina-reranker-v2*
```

---

### Feature flags

Two new Cargo feature flags were introduced:

| Flag | Default (full build) | Default (lean build) | What it enables |
|---|---|---|---|
| `image-embeddings` | on | off | Image ingest, `memory.search_images`, `memory.ingest_image`, Qdrant image collection |
| `image-ocr` | off | off | OCR extraction via tesseract (opt-in everywhere, requires tesseract installed) |

The pre-built release binary always includes `image-embeddings`. Custom lean builds (`bash scripts/build.sh build --lean`) exclude it. If you call `memory.search_images` on a lean binary, you get `CoreError::NotEnabled`.

---

### Verifying the upgrade

```bash
# Confirm binary version:
the-one-mcp --version
# Expected: the-one-mcp 0.6.0

# Confirm tool count:
# In an MCP session, tools/list should return 17 tools.

# Confirm image search (if enabled):
maintain { "action": "images.rescan", "params": { "project_root": "...", "project_id": "..." } }

# Confirm reranker (if enabled):
memory.search { "query": "test query", "project_root": "...", "project_id": "...", "top_k": 5 }
# With RUST_LOG=debug, you should see "reranking N candidates"
```

---

## Upgrading to v0.5.0 (from v0.4.0) — BREAKING

### Tool Consolidation: 33 → 15

v0.5.0 is a **breaking change** to the MCP tool surface. The 33-tool surface of v0.4.0 was consolidated into 15 multiplexed tools (later expanded to 17 in v0.6.0).

**Why:** With 33 tools, the `tools/list` response consumed ~3,500 tokens at session init. The consolidated surface uses ~1,750 tokens — a 50% reduction. This matters because AI CLIs reload the tool list on every new session; smaller lists leave more context budget for actual work.

**Impact:**

- AI CLI sessions discover tools at startup — they will see the new tool names automatically on reconnect. No manual intervention.
- **Custom scripts or automation** that call old tool names by string (e.g., `project.init`, `docs.reindex`) will break and must be updated.
- MCP clients that cached tool schemas locally may need a cache clear.

---

### Tool migration table

| Old Tool (v0.4.0) | New Equivalent (v0.5.0+) |
|---|---|
| `project.init` | `setup` with `action: "project"` |
| `project.refresh` | `setup` with `action: "refresh"` |
| `project.profile.get` | `setup` with `action: "profile"` |
| `docs.get_section` | `docs.get` with `section` parameter |
| `docs.create` | `docs.save` (upsert — creates if missing) |
| `docs.update` | `docs.save` (upsert — updates if exists) |
| `docs.reindex` | `maintain` with `action: "reindex"` |
| `docs.trash.list` | `maintain` with `action: "trash.list"` |
| `docs.trash.restore` | `maintain` with `action: "trash.restore"` |
| `docs.trash.empty` | `maintain` with `action: "trash.empty"` |
| `tool.list` | `tool.find` with `mode: "list"` |
| `tool.suggest` | `tool.find` with `mode: "suggest"` |
| `tool.search` | `tool.find` with `mode: "search"` |
| `tool.add` | `config` with `action: "tool.add"` |
| `tool.remove` | `config` with `action: "tool.remove"` |
| `tool.enable` | `maintain` with `action: "tool.enable"` |
| `tool.disable` | `maintain` with `action: "tool.disable"` |
| `tool.update` | `maintain` with `action: "tool.refresh"` |
| `config.export` | `config` with `action: "export"` |
| `config.update` | `config` with `action: "update"` |
| `metrics.snapshot` | `observe` with `action: "metrics"` |
| `audit.events` | `observe` with `action: "events"` |
| `models.list` | `config` with `action: "models.list"` |
| `models.check_updates` | `config` with `action: "models.check"` |

---

### Before/after call examples

**Project initialization:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "project.init",
  "arguments": { "project_root": "/my/project", "project_id": "myproj" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "setup",
  "arguments": { "action": "project",
    "params": { "project_root": "/my/project", "project_id": "myproj" } } } }
```

**Re-indexing:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "docs.reindex",
  "arguments": { "project_root": "/my/project", "project_id": "myproj" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "maintain",
  "arguments": { "action": "reindex",
    "params": { "project_root": "/my/project", "project_id": "myproj" } } } }
```

**Exporting config:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "config.export",
  "arguments": { "project_root": "/my/project" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "config",
  "arguments": { "action": "export",
    "params": { "project_root": "/my/project" } } } }
```

**Listing metrics:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "metrics.snapshot", "arguments": {} } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "observe",
  "arguments": { "action": "metrics", "params": {} } } }
```

---

### Upgrade steps for v0.5.0

1. Back up `~/.the-one/`
2. Install the new binary:
   ```bash
   bash scripts/install.sh
   ```
3. Restart all AI CLI sessions (they re-discover tools on init)
4. Update any custom scripts using old tool names (see table above)
5. Verify tool count:
   ```
   tools/list → 15 tools (v0.5.0) or 17 tools (v0.6.0)
   ```

---

## Upgrading to v0.4.0 (from v0.3.0)

### New features (non-breaking)

- **TOML embedding model registry** — model definitions moved from hardcoded Rust to `models/local-models.toml` and `models/api-models.toml`. No config changes needed.
- **Interactive installer model selection** — `install.sh` now prompts for embedding tier. Existing installs keep their current model.
- **Two new tools:** `models.list` and `models.check_updates` (later consolidated in v0.5.0)
- **Default embedding model changed:** `all-MiniLM-L6-v2` (384 dims) → `BGE-large-en-v1.5` (1024 dims)

---

### Re-indexing required after default model change

If you are upgrading from v0.3.0 and keeping the default embedding model, your stored vectors are 384-dimensional but the new model produces 1024-dimensional vectors. This mismatch causes Qdrant errors on search.

**Fix:**

1. Drop the old Qdrant collection (via the Qdrant dashboard or API):
   ```
   DELETE /collections/the_one_docs
   ```

2. Re-index your project:
   ```bash
   # In an MCP session:
   maintain { "action": "reindex", "params": { "project_root": "...", "project_id": "..." } }
   ```

**Alternative:** Keep the old model explicitly to avoid re-indexing:

```json
// ~/.the-one/config.json
{
  "embedding_model": "all-MiniLM-L6-v2"
}
```

This preserves your existing index without any re-indexing.

---

## Upgrading to v0.3.0 (from v0.2.x)

### Tool catalog system added

v0.3.0 introduced the tool catalog: a SQLite database (`~/.the-one/catalog.db`) with FTS5 full-text search, populated from `tools/catalog/*.json`. Seven new tools were added:

- `tool.list`, `tool.suggest`, `tool.search`
- `tool.add`, `tool.remove`
- `tool.enable`, `tool.disable`

**Note:** All of these were later consolidated into `tool.find`, `config`, and `maintain` in v0.5.0.

### First-run behavior

On first `project.init` after upgrading to v0.3.0, the broker:

1. Imports the tool catalog from `tools/catalog/*.json`
2. Scans the system with `which` for installed tools
3. Populates `enabled_tools` in the catalog

This adds 5–10 seconds to the first init. Subsequent inits are fast.

### No breaking changes from v0.2.x

All existing tools from v0.2.x (`memory.search`, `docs.save`, etc.) continue to work unchanged in v0.3.0.

---

## General Upgrade Checklist

Follow these steps for any version upgrade:

1. **Back up your data directory:**
   ```bash
   cp -r ~/.the-one ~/.the-one.bak-$(date +%Y%m%d)
   ```

2. **Read the CHANGELOG** for breaking changes specific to your version pair.

3. **Install the new binary:**
   ```bash
   bash scripts/install.sh
   ```
   The installer updates the binary, refreshes the tool catalog, and re-registers with discovered AI CLIs.

4. **Restart all MCP clients** — Claude Code, Gemini CLI, OpenCode, Codex. MCP servers start at session init; a live session won't pick up the new binary.

5. **Run a smoke test:**
   ```bash
   # Confirm version:
   the-one-mcp --version

   # In a new AI session, call setup:
   setup { "action": "profile", "params": { "project_root": "...", "project_id": "..." } }

   # Confirm search works:
   memory.search { "query": "test", "project_root": "...", "project_id": "...", "top_k": 3 }
   ```

6. **Check tool count matches the expected version:**

   | Version | Tool count |
   |---|---|
   | v0.8.0 | 17 |
   | v0.7.0 | 17 |
   | v0.6.0 | 17 |
   | v0.5.0 | 15 |
   | v0.4.0 | 33 |
   | v0.3.0 | 30 |

7. **Re-index if you changed the embedding model** (see the v0.4.0 section above for details).

---

## Getting Help

If you encounter an issue not covered here:

- Check the [Troubleshooting guide](./troubleshooting.md) for common error patterns
- Run `RUST_LOG=debug the-one-mcp serve` and capture the output
- Open an issue at [github.com/michelabboud/the-one-mcp](https://github.com/michelabboud/the-one-mcp/issues) with your version, OS, and the debug log
