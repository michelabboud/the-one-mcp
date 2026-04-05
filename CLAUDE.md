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
- 17 MCP tools (see `crates/the-one-mcp/src/transport/tools.rs`) + 3 MCP resource types (`docs`, `project`, `catalog` via `the-one://` URI scheme, v0.10.0+), 300 tests, 35 schemas, 184 catalog entries across 10 languages
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
