# Architecture Overview

> High-level architecture of the-one-mcp for contributors and integrators.
> Version: v0.8.0

## Design Goals

the-one-mcp is a smart intermediary between AI coding assistants and projects. Every
architectural decision flows from five core goals:

- **Project-aware** — every operation is scoped to a project root and project ID.
  Two projects never share memory, docs, or tool state.
- **Multi-CLI** — the same server binary serves Claude Code, Gemini CLI, OpenCode,
  and Codex. No CLI-specific forks.
- **Token-efficient** — the tool surface is consolidated (4 multiplexed admin tools,
  not 30+ individual ones). Semantic retrieval brings relevant context in, not entire files.
- **Offline-capable** — local ONNX embeddings (fastembed), local SQLite, local Qdrant.
  No required API calls at runtime.
- **Policy-gated** — risky tool executions require explicit approval, with configurable
  scopes and headless-mode defaults.

## Workspace Layout

The repository is a Cargo workspace with 8 crates:

```
the-one-mcp/
├── crates/
│   ├── the-one-core/       # Storage, config, policy, catalog, profiler
│   ├── the-one-memory/     # RAG: chunker, embeddings, Qdrant, reranker, graph
│   ├── the-one-mcp/        # Broker, API types, JSON-RPC transport, CLI binary
│   ├── the-one-router/     # Request routing, provider pool, health tracking
│   ├── the-one-registry/   # Capability registry
│   ├── the-one-claude/     # Claude Code adapter
│   ├── the-one-codex/      # Codex adapter
│   └── the-one-ui/         # Embedded admin UI
├── schemas/mcp/v1beta/     # 63 JSON Schema files
├── models/                 # Embedding model registries (TOML)
│   ├── local-models.toml   # 17 fastembed models (embedded in binary)
│   └── api-models.toml     # OpenAI, Voyage, Cohere API providers
├── tools/catalog/          # Curated tool catalog (JSON)
└── docs/                   # Guides, ADRs, runbooks, specs
```

### Crate Dependency Flow

```
the-one-ui ──────────> the-one-mcp ──────────> the-one-core
                             |                        ^
                             +──> the-one-memory ─────┤ (error types only)
                             +──> the-one-registry ───+
                             +──> the-one-router ─────+

the-one-claude ──> the-one-mcp
the-one-codex  ──> the-one-mcp
```

`the-one-core` is the shared foundation — nothing depends on `the-one-mcp` except
the adapters and UI. This keeps the domain logic independently testable.

## Crate Responsibilities

### `the-one-core`

The shared foundation. No async runtime, no network I/O.

Key modules:
- `config.rs` — 5-layer config resolution (defaults → global file → project file →
  env vars → runtime overrides). All overlay fields are `Option<T>`; defaults applied last.
- `limits.rs` — 12 configurable limits (chunk size, top-k, max docs, etc.) with
  validation bounds checked at load time.
- `storage/sqlite.rs` — SQLite with WAL mode, per-project databases, schema migrations.
- `policy.rs` — risk-tier classification, approval scope enforcement, session state.
- `profiler.rs` — project detection (language fingerprinting, framework detection).
- `docs_manager.rs` — managed docs CRUD: create, update, soft-delete to `.trash/`, move.
- `tool_catalog.rs` — `ToolCatalog` struct wrapping `rusqlite::Connection`. FTS5 search,
  system inventory, enable/disable state. Uses `std::sync::Mutex<Option<ToolCatalog>>`
  in the broker because `Connection` is `!Sync`.
- `error.rs` — `CoreError` with 8 variants. All library crates use this as the unified
  error type.

### `the-one-memory`

The RAG layer. Async throughout.

Key modules:
- `lib.rs` — `MemoryEngine`: the top-level async struct. Owns an `EmbeddingProvider`
  and a Qdrant client.
- `conversation.rs` — normalizes transcript exports into canonical conversation
  messages before chunking.
- `palace.rs` — palace metadata helper (`wing`, `hall`, `room`) used by
  conversation ingest and broker-side filtering.
- `chunker.rs` — `chunk_file()` dispatcher: routes to a language-specific chunker by file
  extension, falling back to blank-line splitting for unsupported types. Language chunkers
  (`chunker_rust.rs`, `chunker_python.rs`, `chunker_typescript.rs`, `chunker_go.rs`) use
  regex + brace-depth tracking to produce function/struct/class-level chunks with extended
  `ChunkMeta` fields: `language`, `symbol`, `signature`, `line_range` (all added in v0.8.0).
  Conversation chunks reuse these same fields today: palace hierarchy is encoded
  into `heading_hierarchy`, with `hall` mirrored in `signature` and `room`
  mirrored in `symbol`.
- `embeddings.rs` — `EmbeddingProvider` trait with two implementations:
  `FastEmbedProvider` (local ONNX via fastembed, tiered: fast/balanced/quality/multilingual)
  and `ApiEmbeddingProvider` (OpenAI-compatible HTTP).
- `models_registry.rs` — parses `models/local-models.toml` and `models/api-models.toml`
  (embedded via `include_str!`) into a typed model registry. Use `resolve_model("quality")`
  for tier aliases.
- `reranker.rs` — cross-encoder reranker for post-search result refinement.
- `qdrant.rs` — async Qdrant HTTP backend (no gRPC dependency).
- `graph.rs` — LightRAG knowledge graph for relationship-aware retrieval.

Embedding downloads happen on first use and cache in `.fastembed_cache/` (gitignored).
Model sizes range from ~23 MB (fast) to ~220 MB (quality).

### `the-one-mcp`

The broker and transport layer. This is where everything connects.

Key modules:
- `broker.rs` — `McpBroker`: the central async struct. Owns per-project `MemoryEngine`
  and `DocsManager` instances in `RwLock<HashMap<String, _>>`, keyed by
  `{project_root}::{project_id}`. Also owns the `ToolCatalog`, `Router`, `PolicyEngine`,
  and session approvals set. Conversation-memory filtering for `memory.search`
  is applied here, after retrieval and before the final MCP response is shaped.
- `api.rs` — all request and response types. Every MCP tool has a corresponding
  `*Request` / `*Response` struct here.
- `transport/jsonrpc.rs` — JSON-RPC 2.0 dispatcher. Deserializes incoming requests,
  calls the appropriate `McpBroker` method, serializes the response.
- `transport/tools.rs` — the `tool_definitions()` function that returns the 19 MCP tool
  descriptors sent during `initialize`.
- `transport/stdio.rs`, `sse.rs`, `stream.rs` — transport implementations. The broker
  never knows which transport is active.
- `bin/the-one-mcp.rs` — CLI entry point (clap). Handles `serve` subcommand and config
  overrides. Uses `anyhow` for error handling (binary, not library).

### `the-one-router`

Request routing and LLM provider management.

Key modules:
- `lib.rs` — `Router`: classifies incoming requests (rules-first) and optionally
  delegates to a nano LLM for ambiguous cases.
- `providers.rs` — OpenAI-compatible HTTP provider client. Supports Ollama, LM Studio,
  and any OpenAI-compatible endpoint.
- `provider_pool.rs` — `ProviderPool`: manages up to 5 nano LLM providers with three
  routing policies (round-robin, latency-priority, priority).
- `health.rs` — `ProviderHealth`: per-provider health tracking with cooldown escalation
  (5 s → 15 s → 60 s). TCP pre-flight check before every classification attempt.
  Silent fallback to rules-only routing when all providers are unhealthy.

### `the-one-registry`

The `CapabilityRegistry` — tracks which capabilities (tools, features) are available
for a given project and CLI combination.

### `the-one-claude` / `the-one-codex`

Thin CLI adapters. They handle CLI-specific initialization or protocol quirks, then
delegate to `the-one-mcp`'s broker for all domain logic.

### `the-one-ui`

Embedded admin UI. The `embedded-ui` binary serves a local web interface for browsing
project state, memory, docs, and tool catalog. Uses `THE_ONE_PROJECT_ROOT` and
`THE_ONE_PROJECT_ID` environment variables for context.

## Request Flow

```
Client (Claude Code / Gemini CLI / OpenCode / Codex)
  |
  |  JSON-RPC 2.0 over stdio / SSE / Streamable HTTP
  v
Transport Layer (stdio.rs / sse.rs / stream.rs)
  |
  |  deserialize JsonRpcRequest
  v
dispatch_tool() in transport/jsonrpc.rs
  |
  |  match tool name → broker method
  v
McpBroker method (broker.rs)
  |
  +──> the-one-core (SQLite, config, policy, catalog)
  +──> the-one-memory (MemoryEngine → Qdrant)
  +──> the-one-router (Router → ProviderPool)
  |
  v
JsonRpcResponse → serialize → transport → client
```

The broker is the only struct that crosses domain boundaries. Transport code only
serializes/deserializes. Domain crates know nothing about MCP or JSON-RPC.

## Key Design Patterns

### Async Broker with Sync SQLite

`McpBroker` methods are all `async fn`. SQLite operations (from `the-one-core`) are
synchronous. Fast file-based operations are called directly; for heavier operations,
use `tokio::task::spawn_blocking`.

### Per-Project Isolation

```rust
memory_by_project: Arc<RwLock<HashMap<String, MemoryEngine>>>
docs_by_project:   RwLock<HashMap<String, DocsManager>>
```

Both maps are keyed by `"{project_root}::{project_id}"`. Projects never share memory
vectors, doc state, or enabled-tool state. The `ToolCatalog` is global (shared), but
`enabled_tools` state inside it is per-CLI per-project-root.

`memory_by_project` was promoted to `Arc<RwLock<...>>` in v0.8.0 so the file watcher's
spawned tokio task can hold its own reference for re-ingestion without borrowing from the
broker.

### Layered Config

Config resolves through 5 layers in order (each overrides the previous):

1. Compiled-in defaults
2. Global config file (`~/.the-one/config.toml`)
3. Project config file (`.the-one/config.toml` in project root)
4. Environment variables (`THE_ONE_*`)
5. Runtime overrides (from `config.update` MCP calls, stored in manifests)

All overlay structs use `Option<T>` fields. `AppConfig::load()` applies defaults at the end.

### Transport-Agnostic Dispatch

`dispatch_tool` in `jsonrpc.rs` receives a tool name and params, calls the appropriate
broker method, and returns a result. It has no knowledge of whether it's running over
stdio, SSE, or HTTP stream. The transport layer calls `dispatch_tool` and handles framing.

### Embedding Provider Abstraction

```rust
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    fn dimension(&self) -> usize;
}
```

`FastEmbedProvider` and `ApiEmbeddingProvider` both implement this trait. The
`MemoryEngine` holds a `Box<dyn EmbeddingProvider>`. No code outside `embeddings.rs`
needs to know which provider is in use.

### Conversation Memory Layer

Conversation memory extends the same `MemoryEngine` rather than introducing a
separate store:

- Transcript exports are normalized into messages.
- Messages are chunked as verbatim conversation turns.
- Palace metadata is encoded in chunk fields that already exist today.
- `memory.search` can optionally filter those chunks by `wing`, `hall`, and `room`.
- `memory.wake_up` uses the persisted conversation-source table to rebuild a
  compact resume pack after restart.

This keeps the current architecture local-first and avoids adding a second
metadata system just for palace state.

### Optional Redis Vector Backend

Vector backend choice is still configured from `the-one-core` config and
resolved in the broker. Qdrant remains the default. Redis/RediSearch is the
optional backend surface for teams that want Redis-managed persistence. With
local embeddings enabled, the broker builds a Redis-backed `MemoryEngine` and
executes vector ingest/search via Redis/RediSearch for that project memory.

### Client-Aware Tool Loading

The MCP `initialize` handshake carries `clientInfo.name`. The broker reads this field
and loads:
1. `~/.the-one/registry/custom.json` (shared, all CLIs)
2. `~/.the-one/registry/custom-<client>.json` (per-CLI, e.g. `custom-claude.json`)
3. `~/.the-one/registry/recommended.json` (curated recommendations)

## Multi-Backend Architecture (v0.16.0+)

v0.16.0 made persistence **pluggable along two orthogonal axes** — state
store and vector backend — without changing any broker handler code.
This section documents the architectural unlock.

### The two traits (Phase A — v0.16.0-rc1)

Phase A ships two parallel traits, one per axis:

```rust
// crates/the-one-memory/src/vector_backend.rs
#[async_trait]
pub trait VectorBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;
    async fn ensure_collection(&self, dims: usize) -> Result<(), String>;
    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String>;
    async fn search_chunks(&self, query: Vec<f32>, top_k: usize, threshold: f32)
        -> Result<Vec<VectorHit>, String>;
    // ...plus entity, relation, hybrid, persistence-verification methods
}

// crates/the-one-core/src/state_store.rs
pub trait StateStore: Send {
    fn project_id(&self) -> &str;
    fn schema_version(&self) -> Result<i64, CoreError>;
    fn capabilities(&self) -> StateStoreCapabilities;
    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError>;
    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError>;
    fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError>;
    // ...26 methods total covering every broker-callable write/read
}
```

**Key design choice**: `VectorBackend` is async (`#[async_trait]`),
`StateStore` is sync. The reason is `rusqlite::Connection`: it's
`Send + !Sync`, so holding a connection guard across an `.await` is
impossible by construction. Async state-store methods would need a
tokio mutex, but that reintroduces the "guard held across `.await`"
deadlock pattern the sync trait prevents. sqlx-based backends
(`PostgresStateStore`) bridge async-to-sync internally — see the
"Sync-over-async bridge" section below.

**Capability reporting**: `BackendCapabilities` and
`StateStoreCapabilities` structs are static per-backend reports.
Callers inspect `.hybrid`, `.fts`, `.transactions`, etc. to decide
whether to route an operation through the backend or take a fallback
path. Phase 2's pgvector sets `hybrid = false` (Decision D deferred);
Phase 3's Postgres state sets every capability `true`.

### Broker cache + sync closure chokepoint (Phase 1 — v0.16.0-phase1)

The broker holds a per-project cache of state stores:

```rust
state_by_project: RwLock<HashMap<String, Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>>>
```

Every broker method that needs state-store access routes through a
single chokepoint:

```rust
self.with_state_store(project_root, project_id, |store: &dyn StateStore| {
    store.upsert_project_profile(profile_json)?;
    store.record_audit(&record)?;
    Ok(some_value)
}).await
```

The closure is **synchronous** on purpose. The inner lock is
`std::sync::Mutex` (deliberately NOT `tokio::sync::Mutex`) so its
guard is `!Send`. The compiler refuses any attempt to hold the guard
across an `.await` point — which is exactly the restriction that
prevents Postgres/Redis pool checkouts from being held across
asynchronous work and causing pool exhaustion under load.

`get_or_init_state_store` uses a **double-check-under-write-lock**
pattern so the cold path constructs the new store OUTSIDE the write
lock. Two concurrent cache-miss requests for different projects don't
serialize through the factory — load-bearing for Phase 3+ where
factories are async and do network I/O (TCP + TLS handshake + sqlx
`SET statement_timeout` on every fresh pool connection).

### Env var selection scheme (Phase 2 — v0.16.0-phase2)

Four env vars, two per axis, parsed once at broker construction by
`BackendSelection::from_env`:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string>
```

**Closed enums + fail-loud validation** — unknown TYPE values, missing
URLs, asymmetric specification (one side set, other unset), mismatched
combined TYPEs, and mismatched combined URLs all produce
`CoreError::InvalidProjectConfig` with targeted error messages at
startup. The parser is covered by 12 unit tests (8 negative + 4
positive controls, all `temp_env::with_vars`-isolated).

**Secrets live in env vars**, tuning knobs live in `config.json`
(`vector_pgvector` and `state_postgres` blocks). Clean separation:
credentials in `$THE_ONE_STATE_URL`, HNSW parameters in
`config.json`. See [configuration.md](configuration.md#multi-backend-selection-v0160)
for the validation rule table and the field-by-field tuning surface.

### pgvector backend (Phase 2 — v0.16.0-phase2)

`crates/the-one-memory/src/pg_vector.rs` (~860 LOC) implements
`VectorBackend` against pgvector. Key elements:

- **Defensive extension preflight** — `preflight_vector_extension`
  runs three probe queries (`pg_extension`, `pg_available_extensions`,
  `CREATE EXTENSION`) with targeted error messages for Supabase / AWS
  RDS / GCP Cloud SQL / Azure Flexible Server / self-hosted.
- **Hand-rolled migration runner** — replaces `sqlx::migrate!` because
  sqlx's `migrate` feature transitively references `sqlx-sqlite?/…`
  weak-deps that cargo's `links` conflict check pulls into the graph,
  colliding with rusqlite 0.39's `libsqlite3-sys ^0.37`. Runner uses
  `include_str!` to embed `.sql` files, SHA-256 checksums for drift
  detection, and a `the_one.pgvector_migrations` tracking table.
- **Batched upserts** via `INSERT ... SELECT * FROM UNNEST($1::text[],
  $2::vector[], ...)` — one round trip per batch, no N-query loop.
- **HNSW query-time tuning** — `SET LOCAL hnsw.ef_search = N` inside
  a per-search transaction. Scoped to the transaction so the setting
  doesn't leak to other pool connections.
- **Dim hardcoded to 1024** (Decision C, BGE-large-en-v1.5 quality
  tier) — the backend constructor refuses to boot if the live provider
  reports a different dim.

### PostgresStateStore backend (Phase 3 — v0.16.0-phase3)

`crates/the-one-core/src/storage/postgres.rs` (~1,350 LOC) implements
every `StateStore` trait method on Postgres. Key elements:

- **Sync-over-async bridge** — every trait method wraps sqlx calls in
  `block_on(async { ... })`:

  ```rust
  fn block_on<F, R>(fut: F) -> R
  where
      F: std::future::Future<Output = R>,
  {
      tokio::task::block_in_place(|| {
          tokio::runtime::Handle::try_current()
              .expect("must be called from a tokio runtime")
              .block_on(fut)
      })
  }
  ```

  `block_in_place` tells tokio "this worker is about to do blocking
  work" so other async tasks migrate off; `Handle::current().block_on`
  drives the future to completion; the worker resumes async duty
  afterward. **Requires multi-threaded tokio runtime** — the broker
  binary's `#[tokio::main]` default satisfies this; tests use
  `#[tokio::test(flavor = "multi_thread")]`.

- **FTS5 → tsvector translation** — SQLite's `diary_entries_fts`
  virtual table becomes a `content_tsv TSVECTOR` column on
  `diary_entries` + a GIN index + `websearch_to_tsquery('simple', $1)`
  for matching + `ts_rank` for ordering + LIKE fallback for empty
  tsquery inputs. `'simple'` (not `'english'`) tokenization because
  stemming breaks exact matches on English code identifiers and
  produces wrong results on non-English content.

- **Schema v7 parity in one migration** — Postgres ships
  `audit_events.outcome` + `error_kind` columns from day one (no
  incremental v1..v6 walk-through). `schema_version()` returns the
  highest applied migration version (`1` for Phase 3), not the SQLite
  schema version (`7`). Per-backend number, not cross-backend parity.

- **BIGINT epoch_ms throughout** — no chrono, no TIMESTAMPTZ. Matches
  Phase 2's pgvector convention and side-steps the cargo `links`
  conflict on sqlx's `chrono` feature permanently.

- **New `CoreError::Postgres(String)` variant** — surgical addition
  (3 touches: enum, label in `error_kind_label`, exhaustive test in
  `production_hardening.rs`). The wire-level sanitizer passes the
  short `"postgres"` label to clients and keeps full error text in
  `tracing::error!` logs.

### Factory dispatcher (Phase 2 + Phase 3)

`McpBroker::state_store_factory` is `async` as of Phase 3 (Phase 1's
doc comment pre-announced this). It branches on the parsed
`BackendSelection.state`:

```rust
async fn state_store_factory(
    &self,
    project_root: &Path,
    project_id: &str,
) -> Result<Box<dyn StateStore + Send>, CoreError> {
    match self.backend_selection.state {
        StateTypeChoice::Sqlite => Ok(Box::new(ProjectDatabase::open(...)?)),
        StateTypeChoice::Postgres => self.construct_postgres_state_store(project_id).await,
        StateTypeChoice::Redis => Err(CoreError::NotEnabled("Phase 5".into())),
        StateTypeChoice::PostgresCombined => Err(CoreError::NotEnabled("Phase 4".into())),
        StateTypeChoice::RedisCombined => Err(CoreError::NotEnabled("Phase 6".into())),
    }
}
```

The vector axis has a parallel branch in `build_memory_engine` that
short-circuits to `build_pgvector_memory_engine` when
`BackendSelection.vector == Pgvector`, falling through to the legacy
`config.vector_backend` string-based path otherwise.

### Cross-phase relationship

| Phase | What it ships | What it unlocks |
|---|---|---|
| A (rc1) | Two traits + capability structs + Qdrant/Redis/SQLite impls | Everything below |
| 1 | `state_by_project` cache + sync-closure chokepoint | Phase 3+ async factories without pool deadlocks |
| 2 | pgvector backend + env var parser + startup validator | Alternative vector backend; env-driven selection |
| 3 | PostgresStateStore + `state_store_factory` async | Alternative state backend; cross-axis deployment |
| 4 (pending) | Combined Postgres+pgvector (one pool, two trait roles) | Transactional writes spanning state + vectors |
| 5 (pending) | Redis state in three durability modes | Cache + persistent + AOF deployments |
| 6 (pending) | Combined Redis+RediSearch | One fred client, two trait roles |
| 7 (pending) | Redis-Vector entity/relation parity + v0.16.0 GA | Close the capability gap across backends |

See [multi-backend-operations.md](multi-backend-operations.md) for
the operator-facing deployment matrix and
[pgvector-backend.md](pgvector-backend.md) /
[postgres-state-backend.md](postgres-state-backend.md) for the
per-backend setup guides.

## Tool Dispatch Architecture

The 19 MCP tools are split into two groups:

### Work Tools (direct methods)

Thin wrappers around broker methods with 1:1 correspondence:

| Tool | Broker Method |
|------|---------------|
| `memory.search` | `broker.memory_search()` |
| `memory.fetch_chunk` | `broker.memory_fetch_chunk()` |
| `memory.search_images` | `broker.memory_search_images()` |
| `memory.ingest_image` | `broker.memory_ingest_image()` |
| `docs.list` | `broker.docs_list()` |
| `docs.get` | `broker.docs_get()` |
| `docs.save` | `broker.docs_save()` |
| `docs.delete` | `broker.docs_delete()` |
| `docs.move` | `broker.docs_move()` |
| `tool.find` | `broker.tool_find()` |
| `tool.info` | `broker.tool_info()` |
| `tool.install` | `broker.tool_install()` |
| `tool.run` | `broker.tool_run()` |

### Multiplexed Admin Tools (sub-dispatch by `action`)

Four tools that consolidate related operations to reduce the MCP surface:

| Tool | Actions |
|------|---------|
| `setup` | `project`, `refresh`, `profile` |
| `config` | `export`, `update`, `tool.add`, `tool.remove`, `models.list`, `models.check` |
| `maintain` | `reindex`, `tool.enable`, `tool.disable`, `tool.refresh`, `trash.list`, `trash.restore`, `trash.empty`, `images.rescan`, `images.clear`, `images.delete` |
| `observe` | `metrics`, `events` |

The `dispatch_tool` function in `jsonrpc.rs` handles the outer tool name. Each multiplexed
tool has a secondary dispatch on the `action` parameter.

## RAG Pipeline

```
Source file (any extension)
  |
  v  chunker.rs — dispatches by file extension (v0.8.0)
  |     .rs  → chunker_rust.rs   (fn/struct/impl/trait/etc.)
  |     .py  → chunker_python.rs (def/class + decorators)
  |     .ts/.tsx → chunker_typescript.rs (function/class/interface/type/enum)
  |     .js/.jsx/.mjs/.cjs → chunker_javascript.rs (same engine as TS)
  |     .go  → chunker_go.rs (func/type/var/const with receiver handling)
  |     other → split_on_blank_lines (text/markdown fallback)
Chunks (title + content + heading path + language + symbol + signature + line_range)
  |
  v  embeddings.rs (FastEmbedProvider or ApiEmbeddingProvider)
Vectors (384–1024 dimensions depending on model tier)
  |
  v  qdrant.rs (async HTTP, per-project collection)
Qdrant vector store
```

On query:
```
Natural-language query
  |
  v  embed query with same provider
Query vector
  |
  v  Qdrant approximate nearest-neighbor search (top-k)
Candidate chunks
  |
  v  (optional) reranker.rs (cross-encoder, re-scores candidates)
Final ranked results → broker → client
```

## Image Pipeline

Images are indexed in a separate Qdrant collection from text documents:

```
Image file (.png / .jpg / .webp / ...)
  |
  +──> FastEmbedImageProvider → image vector
  |       (CLIP or similar vision encoder)
  |
  +──> (optional, if image-ocr feature enabled)
  |       OCR → text chunks → text embeddings → text collection
  |
  +──> (optional) thumbnail generation for UI preview
  |
  v
Separate Qdrant collection: "{project_id}_images"
```

Triggered by:
- `memory.ingest_image` — manual ingestion of a specific file
- `maintain` with `action: images.rescan` — full project image re-scan
- `setup` (action: `project`) / `setup` (action: `refresh`) — automatic detection when fingerprint changes

Feature flags control availability:
- `image-embeddings` — enables `FastEmbedImageProvider`
- `image-ocr` — enables OCR text extraction

The broker detects these at runtime and omits image tools from the MCP surface if
the features are not compiled in.

## Policy Engine

Located in `the-one-core/src/policy.rs`. Every `tool.run` call passes through it.

```
tool.run request
  |
  v  look up tool's risk_level in catalog
  |
  +── low / medium ──> auto-approve → execute
  |
  +── high ──> check session_approvals (in-memory HashSet)
                  |
                  +── found → execute
                  |
                  +── not found → check persistent forever-approvals
                                    |
                                    +── found → execute
                                    |
                                    +── not found + interactive → prompt user
                                    |       |
                                    |       +── approved → store by scope → execute
                                    |       +── denied → return error
                                    |
                                    +── not found + headless → deny
```

Approval scopes:
- `once` — consumed after one run (not persisted)
- `session` — stored in `McpBroker.session_approvals` (in-memory, cleared on restart)
- `forever` — written to `.the-one/manifests/overrides.json` (persists across sessions)

## Error Handling

| Layer | Error strategy |
|-------|---------------|
| `the-one-core` | `CoreError` enum via `thiserror` |
| `the-one-memory` | Returns `Result<T, String>` internally; broker maps to `CoreError::Embedding` |
| `the-one-router` | Returns `Result<T, String>` internally; broker maps to `CoreError::Provider` |
| `the-one-mcp` (lib) | `CoreError` propagated, mapped to JSON-RPC error objects |
| `the-one-mcp` (binary) | `anyhow` for startup/config errors |

`CoreError` has 8 variants: `Io`, `Db`, `Config`, `Embedding`, `Provider`, `Policy`,
`Catalog`, `NotFound`. All crates that produce errors use one of these.

## Testing Strategy

```
the-one-core:     unit tests per module (sync, #[test])
the-one-memory:   unit tests for chunker, registry; integration tests require Qdrant
the-one-router:   unit tests for routing logic; provider tests mock HTTP
the-one-mcp:      broker integration tests (#[tokio::test]); schema validation tests
```

Environment isolation: config tests use `temp_env::with_vars` to prevent variable
pollution between parallel test runs.

The full test suite: `cargo test --workspace` (272 tests as of v0.8.0).

Release gate: `bash scripts/release-gate.sh` — runs fmt check, clippy, full test suite,
and binary smoke test.

## Adding a New MCP Tool

Follow these steps in order:

1. **`api.rs`** — add `MyToolRequest` and `MyToolResponse` structs with `serde` derives.

2. **`broker.rs`** — add an `async fn my_tool(&self, req: MyToolRequest) -> Result<MyToolResponse, CoreError>` method. Implement domain logic here.

3. **`transport/jsonrpc.rs`** — add a match arm in `dispatch_tool`:
   ```rust
   "my.tool" => {
       let req: MyToolRequest = serde_json::from_value(params)?;
       let res = broker.my_tool(req).await?;
       serde_json::to_value(res)?
   }
   ```

4. **`transport/tools.rs`** — add a `tool_def("my.tool", "Description", schema)` entry
   in `tool_definitions()`.

5. **`schemas/mcp/v1beta/`** — create `my-tool-request.json` and `my-tool-response.json`
   JSON Schema files. Use `$id` prefix `the-one.mcp.v1beta.` and JSON Schema draft 2020-12.

6. **`lib.rs` expected list** — update the expected tool count and name list in the
   schema validation test.

7. **Tests** — add unit tests for the broker method and an integration test that exercises
   the full JSON-RPC round-trip.

8. **Docs** — add the tool to `AGENTS.md`'s tool surface table and create or update
   the relevant guide in `docs/guides/`.

## Adding a New Embedding Model

1. Add a TOML entry to `models/local-models.toml` (for local) or `models/api-models.toml`
   (for API providers). The file is embedded in the binary via `include_str!`.

2. Add a match arm in `embeddings.rs` `resolve_model()` if adding a new tier alias.

3. No other code changes are needed. The model registry parser picks up new entries
   automatically. The broker's `models.list` and `models.check_updates` tools will
   surface the new entry.

## Adding a New CLI Adapter

1. Create a new crate under `crates/the-one-<cli>/`.
2. Add it to `Cargo.toml` workspace members.
3. Implement any CLI-specific protocol quirks in the adapter crate.
4. Delegate all domain calls to `the-one-mcp`'s `McpBroker`.
5. Register with the install script in `scripts/install.sh`.
6. Add per-CLI custom tool file stub: `~/.the-one/registry/custom-<cli>.json`.
7. Update `clientInfo.name` detection in `broker.rs` to load the per-CLI registry file.
