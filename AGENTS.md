# AGENTS.md - The-One MCP Development Guide

Rust MCP broker system (v0.8.0). All agents should follow these guidelines.

## Build Commands

```bash
# Full validation (run before every commit)
bash scripts/build.sh check     # or manually:
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build release binary
bash scripts/build.sh build     # with swagger
bash scripts/build.sh build --lean  # without swagger

# Build entire workspace
cargo build --release --workspace

# Run specific crate tests
cargo test -p the-one-core
cargo test -p the-one-memory
cargo test -p the-one-router
cargo test -p the-one-mcp

# Release gate
bash scripts/release-gate.sh

# Trigger cross-platform release (manual only — does NOT auto-trigger on tags)
bash scripts/build.sh release v0.8.0
bash scripts/build.sh release --status
```

## Workspace Structure

```
the-one-mcp/
├── crates/
│   ├── the-one-core/          # Config, storage, policy, profiler, docs manager, limits
│   │   └── src/
│   │       ├── config.rs      # 5-layer config system
│   │       ├── limits.rs      # 12 configurable limits with validation bounds
│   │       ├── docs_manager.rs # Managed docs CRUD + soft-delete + trash
│   │       ├── policy.rs      # Policy engine with configurable limits
│   │       ├── storage/sqlite.rs # SQLite with WAL + migrations
│   │       ├── profiler.rs    # Project detection + fingerprinting
│   │       ├── project.rs     # Init/refresh lifecycle
│   │       ├── manifests.rs   # .the-one/ manifest management
│   │       ├── backup.rs      # Backup/restore operations
│   │       ├── contracts.rs   # Shared data types + enums
│   │       ├── error.rs       # CoreError with 8 variants
│   │       └── telemetry.rs   # Structured logging setup
│   ├── the-one-mcp/           # Async broker + transport + CLI
│   │   └── src/
│   │       ├── broker.rs      # McpBroker (async, 17 MCP tools)
│   │       ├── api.rs         # Request/response types
│   │       ├── adapter_core.rs # Shared adapter logic
│   │       ├── swagger.rs     # OpenAPI embedding
│   │       ├── transport/
│   │       │   ├── jsonrpc.rs # JSON-RPC 2.0 dispatch
│   │       │   ├── tools.rs   # 17 MCP tool definitions (13 work + 4 admin)
│   │       │   ├── stdio.rs   # Stdio transport
│   │       │   ├── sse.rs     # SSE HTTP transport
│   │       │   └── stream.rs  # Streamable HTTP transport
│   │       └── bin/
│   │           └── the-one-mcp.rs  # CLI binary (clap)
│   ├── the-one-memory/        # RAG: chunker + embeddings + Qdrant + images
│   │   └── src/
│   │       ├── lib.rs             # Async MemoryEngine (Arc<RwLock<HashMap>> for watcher sharing)
│   │       ├── chunker.rs         # Markdown + code chunker dispatcher (chunk_file)
│   │       ├── chunker_rust.rs    # Rust language chunker (fn/struct/enum/impl/trait/mod/…)
│   │       ├── chunker_python.rs  # Python chunker (def/async def/class + decorators)
│   │       ├── chunker_typescript.rs # TypeScript/JavaScript chunker (shared engine)
│   │       ├── chunker_go.rs      # Go chunker (func/type/var/const + paren blocks)
│   │       ├── embeddings.rs      # fastembed 5.x (ONNX) + API provider
│   │       ├── models_registry.rs # TOML model registry parser
│   │       ├── reranker.rs        # Cross-encoder reranker
│   │       ├── image_embeddings.rs # CLIP-based image embedding (feature: image-embeddings)
│   │       ├── image_ingest.rs    # Image indexing pipeline
│   │       ├── thumbnail.rs       # Thumbnail generation
│   │       ├── ocr.rs             # Tesseract OCR extraction (feature: image-ocr)
│   │       ├── graph.rs           # LightRAG knowledge graph
│   │       └── qdrant.rs          # Async Qdrant HTTP backend
│   ├── the-one-router/        # Routing + provider pool
│   │   └── src/
│   │       ├── lib.rs         # Router (sync + async)
│   │       ├── providers.rs   # OpenAI-compatible HTTP provider
│   │       ├── provider_pool.rs # Multi-provider pool (3 policies)
│   │       └── health.rs      # Per-provider health tracking
│   ├── the-one-registry/      # Capability catalog
│   ├── the-one-claude/        # Claude Code adapter
│   ├── the-one-codex/         # Codex adapter
│   └── the-one-ui/            # Embedded admin UI
├── schemas/mcp/v1beta/        # 35 JSON Schema files
├── models/                        # Embedding model registries (TOML)
│   ├── local-models.toml          # 17 fastembed models (embedded in binary)
│   └── api-models.toml            # API providers: OpenAI, Voyage, Cohere
├── docs/                      # Guides, ADRs, runbooks, specs
├── scripts/release-gate.sh    # Release validation
└── .github/workflows/ci.yml   # CI pipeline
```

## MCP Tool Surface (17 tools)

### Work Tools (13)

#### Knowledge (RAG)
- `memory.search` — semantic search across indexed doc chunks
- `memory.fetch_chunk` — fetch specific chunk by ID
- `memory.search_images` — semantic search over indexed images (screenshots, diagrams, mockups)
- `memory.ingest_image` — manually index an image file (OCR + thumbnail)

#### Documents (Managed CRUD)
- `docs.list` — folder tree listing
- `docs.get` — return full document or a named section (optional `section` param)
- `docs.save` — create or update a managed document (upsert)
- `docs.delete` — soft-delete to .trash/
- `docs.move` — rename/move within managed folder

#### Tool Discovery & Execution
- `tool.find` — unified discovery: modes `list`, `suggest`, `search`
- `tool.info` — full metadata for a specific tool
- `tool.install` — run install command, update inventory, auto-enable
- `tool.run` — execute tool with policy-gated approval

### Admin Tools (4 multiplexed)
- `setup` — project init (`project`), re-scan (`refresh`), profile (`profile`)
- `config` — export config, update config, add/remove custom tools, list/check embedding models
- `maintain` — reindex, tool enable/disable/refresh, trash management, image rescan/clear/delete
- `observe` — broker metrics (`metrics`), audit log (`events`)

## Code Style

- Run `cargo fmt` before every commit
- 4 spaces indentation, 100 char max line width
- `thiserror` for library errors, `anyhow` for binary
- `tokio` async runtime, `async/await` throughout
- Tests: `#[tokio::test]` for async, `#[test]` for sync
- Imports: std -> external -> internal

## Architecture Principles

1. **Token efficiency first** — keep always-loaded context tiny
2. **Progressive tool exposure** — expose minimal MCP surface by default
3. **Local-first** — SQLite + fastembed + Qdrant local, API optional
4. **Rules-first routing** — nano LLM is optional enhancement
5. **RAG for discovery, raw markdown for precision**
6. **Single shared backend** with thin CLI adapters
7. **Arc<RwLock<HashMap>> for shared state** — `memory_by_project` is wrapped in `Arc<RwLock<...>>` so the watcher's spawned tokio task can hold its own reference for auto-reindex without going through the broker

## Multi-CLI Support

Works with Claude Code, Gemini CLI, OpenCode, and Codex — same server, same protocol.

- Server reads `clientInfo.name` from MCP `initialize` handshake
- Loads per-CLI custom tools from `~/.the-one/registry/custom-<client>.json`
- Universal tools from `recommended.json` + `custom.json` always loaded
- Install script auto-detects all CLIs and registers with each

## Embedding Models

Tiered selection via config `"embedding_model"`:

| Tier | Dims | Best For |
|------|------|----------|
| `fast` | 384 | Getting started |
| `balanced` | 768 | Good tradeoff |
| `quality` (default) | 1024 | **Recommended** |
| `multilingual` | 1024 | Non-English |

17 local models total. Plus API models (OpenAI, Voyage, Cohere). Model registry in `models/local-models.toml` and `models/api-models.toml`.

## Key Decisions

- All broker methods are async (tokio)
- Embeddings: tiered fastembed 5.x (384-1024 dim ONNX) local, OpenAI-compatible API optional
- Image embeddings: CLIP-based, enabled by default via `image-embeddings` feature
- OCR: Tesseract-based, optional via `image-ocr` feature
- Reranking: cross-encoder reranker in `the-one-memory`
- Provider pool: up to 5 OpenAI-compatible endpoints with health checks
- Docs: managed folder with soft-delete to .trash/; `docs.save` is upsert, `docs.get` supports optional section
- Tool discovery consolidated into `tool.find` (modes: list/suggest/search)
- Limits: 12 configurable parameters with validation bounds
- Transports: stdio (default), SSE, streamable HTTP
- Per-CLI custom tools via `clientInfo` detection
- Install script: one-command setup with auto-registration for all CLIs
