# AGENTS.md - The-One MCP Development Guide

Rust MCP broker system (v0.2.0). All agents should follow these guidelines.

## Build Commands

```bash
# Full validation (run before every commit)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build release binary
cargo build --release -p the-one-mcp --bin the-one-mcp

# Build entire workspace
cargo build --release --workspace

# Run specific crate tests
cargo test -p the-one-core
cargo test -p the-one-memory
cargo test -p the-one-router
cargo test -p the-one-mcp

# Release gate
bash scripts/release-gate.sh
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
│   │       ├── broker.rs      # McpBroker (async, 24 tool methods)
│   │       ├── api.rs         # Request/response types
│   │       ├── adapter_core.rs # Shared adapter logic
│   │       ├── swagger.rs     # OpenAPI embedding
│   │       ├── transport/
│   │       │   ├── jsonrpc.rs # JSON-RPC 2.0 dispatch
│   │       │   ├── tools.rs   # 24 MCP tool definitions
│   │       │   ├── stdio.rs   # Stdio transport
│   │       │   ├── sse.rs     # SSE HTTP transport
│   │       │   └── stream.rs  # Streamable HTTP transport
│   │       └── bin/
│   │           └── the-one-mcp.rs  # CLI binary (clap)
│   ├── the-one-memory/        # RAG: chunker + embeddings + Qdrant
│   │   └── src/
│   │       ├── lib.rs         # Async MemoryEngine
│   │       ├── chunker.rs     # Heading-aware markdown chunker
│   │       ├── embeddings.rs  # fastembed (ONNX) + API provider
│   │       └── qdrant.rs      # Async Qdrant HTTP backend
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
├── schemas/mcp/v1beta/        # 49 JSON Schema files
├── docs/                      # Guides, ADRs, runbooks, specs
├── scripts/release-gate.sh    # Release validation
└── .github/workflows/ci.yml   # CI pipeline
```

## MCP Tool Surface (24 tools)

### Project Lifecycle
- `project.init` — detect project, create state, index docs
- `project.refresh` — re-fingerprint, sync docs, refresh profile
- `project.profile.get` — return cached project profile

### Knowledge (RAG)
- `memory.search` — semantic search across indexed docs
- `memory.fetch_chunk` — fetch specific chunk by ID

### Documents (Managed CRUD)
- `docs.create` — create markdown in managed folder
- `docs.update` — update existing markdown
- `docs.delete` — soft-delete to .trash/
- `docs.get` — return full original file
- `docs.get_section` — return bounded heading section
- `docs.list` — folder tree listing
- `docs.move` — rename/move within managed folder

### Trash
- `docs.trash.list` — list trash contents
- `docs.trash.restore` — restore from trash
- `docs.trash.empty` — permanently empty trash

### Re-index
- `docs.reindex` — force full re-indexing

### Tools
- `tool.suggest` — suggest capabilities by query
- `tool.search` — search capabilities
- `tool.enable` — enable tool family
- `tool.run` — execute tool with approval gate

### Config & Observability
- `config.export` — full config with limits
- `config.update` — update project config
- `metrics.snapshot` — broker + provider metrics
- `audit.events` — query audit trail

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

## Key Decisions

- All broker methods are async (tokio)
- Embeddings: fastembed (384-dim ONNX) local, OpenAI-compatible API optional
- Provider pool: up to 5 OpenAI-compatible endpoints with health checks
- Docs: managed folder with soft-delete to .trash/
- Limits: 12 configurable parameters with validation bounds
- Transports: stdio (default), SSE, streamable HTTP
