# Architecture Overview

> High-level architecture of the-one-mcp for contributors and integrators.
> Version: v0.6.0

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
- `chunker.rs` — heading-aware markdown chunker that preserves document structure.
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
  and session approvals set.
- `api.rs` — all request and response types. Every MCP tool has a corresponding
  `*Request` / `*Response` struct here.
- `transport/jsonrpc.rs` — JSON-RPC 2.0 dispatcher. Deserializes incoming requests,
  calls the appropriate `McpBroker` method, serializes the response.
- `transport/tools.rs` — the `tool_definitions()` function that returns the 33 MCP tool
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
memory_by_project: RwLock<HashMap<String, MemoryEngine>>
docs_by_project:   RwLock<HashMap<String, DocsManager>>
```

Both maps are keyed by `"{project_root}::{project_id}"`. Projects never share memory
vectors, doc state, or enabled-tool state. The `ToolCatalog` is global (shared), but
`enabled_tools` state inside it is per-CLI per-project-root.

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

### Client-Aware Tool Loading

The MCP `initialize` handshake carries `clientInfo.name`. The broker reads this field
and loads:
1. `~/.the-one/registry/custom.json` (shared, all CLIs)
2. `~/.the-one/registry/custom-<client>.json` (per-CLI, e.g. `custom-claude.json`)
3. `~/.the-one/registry/recommended.json` (curated recommendations)

## Tool Dispatch Architecture

The 33 MCP tools are split into two groups:

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
Markdown document
  |
  v  chunker.rs (heading-aware, preserves structure)
Chunks (title + content + heading path)
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

## Image Pipeline (v0.6.0)

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
- `project.init` / `project.refresh` — automatic detection when fingerprint changes

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

The full test suite: `cargo test --workspace` (174 tests as of v0.6.0).

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
