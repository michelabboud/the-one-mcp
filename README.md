# the-one-mcp

A production-grade Rust MCP (Model Context Protocol) broker that gives AI coding assistants project-aware memory, semantic document search, policy-gated tool execution, and intelligent request routing — while keeping token usage minimal.

## Why

LLMs waste tokens loading irrelevant tools, re-reading docs, and losing context between sessions. The-One MCP acts as a smart intermediary between your AI assistant (Claude Code, Codex) and your project:

- **Progressive tool exposure** — only surfaces relevant capabilities based on project profile
- **Unlimited project memory** — semantic RAG search over your docs without loading everything into context
- **Managed knowledge base** — create, update, and organize markdown docs that persist across sessions
- **Token-efficient retrieval** — configurable limits on search results, doc section sizes, and tool suggestions
- **Policy-gated execution** — approval scopes (once/session/forever) for high-risk tool actions

## Quick Start

```bash
# Build the MCP server
cargo build --release -p the-one-mcp --bin the-one-mcp

# Run as MCP server (stdio transport, default for Claude Code / Codex)
./target/release/the-one-mcp serve

# Add to Claude Code
claude mcp add the-one-mcp -- ./target/release/the-one-mcp serve
```

### Alternative Transports

```bash
# SSE transport (for web clients)
./target/release/the-one-mcp serve --transport sse --port 3000

# Streamable HTTP transport (MCP spec compliant)
./target/release/the-one-mcp serve --transport stream --port 3000
```

## Architecture

```
Claude Code / Codex
    |  (JSON-RPC 2.0 via stdio, SSE, or streamable HTTP)
    v
the-one-mcp broker
    |
    +-- Project Lifecycle    project.init / project.refresh / project.profile.get
    +-- Knowledge (RAG)      memory.search / memory.fetch_chunk
    +-- Documents (CRUD)     docs.create / update / delete / get / list / move
    +-- Trash Management     docs.trash.list / restore / empty
    +-- Tool Management      tool.suggest / search / enable / run
    +-- Configuration        config.export / config.update
    +-- Observability        metrics.snapshot / audit.events / docs.reindex
    |
    +-- Embeddings           fastembed (local ONNX, 384-dim) or OpenAI-compatible API
    +-- Vector Storage       Qdrant HTTP (remote or local)
    +-- LLM Routing          Provider pool with health checks + 3 routing policies
    +-- Policy Engine        Configurable limits + risk-tier approval gates
    +-- SQLite               Project state, approvals, audit trail (WAL mode)
```

## 24 MCP Tools

| Category | Tools |
|----------|-------|
| **Project** | `project.init`, `project.refresh`, `project.profile.get` |
| **Knowledge** | `memory.search`, `memory.fetch_chunk` |
| **Documents** | `docs.create`, `docs.update`, `docs.delete`, `docs.get`, `docs.get_section`, `docs.list`, `docs.move` |
| **Trash** | `docs.trash.list`, `docs.trash.restore`, `docs.trash.empty` |
| **Re-index** | `docs.reindex` |
| **Tools** | `tool.suggest`, `tool.search`, `tool.enable`, `tool.run` |
| **Config** | `config.export`, `config.update` |
| **Observability** | `metrics.snapshot`, `audit.events` |

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `the-one-core` | Config, storage, policy, profiler, manifests, docs manager, limits |
| `the-one-mcp` | Async broker, API types, JSON-RPC transport, CLI binary |
| `the-one-memory` | RAG: chunker, embeddings (fastembed + API), async Qdrant backend |
| `the-one-router` | Rules-first routing, provider pool, health tracking |
| `the-one-registry` | Capability catalog with risk-tier filtering |
| `the-one-claude` | Claude Code adapter |
| `the-one-codex` | Codex adapter |
| `the-one-ui` | Embedded admin UI (dashboard, config, audit, swagger) |

## Configuration

Config is layered (lowest to highest precedence):

1. Hardcoded defaults
2. Global config: `~/.the-one/config.json`
3. Project config: `<project>/.the-one/config.json`
4. Environment variables (`THE_ONE_*`)
5. Runtime overrides

See the [Complete Guide](docs/guides/the-one-mcp-complete-guide.md) for all config fields, environment variables, and limits.

## Documentation

| Document | Description |
|----------|-------------|
| [Quickstart](docs/guides/quickstart.md) | Shortest path to build, run, and verify |
| [Complete Guide](docs/guides/the-one-mcp-complete-guide.md) | Full reference for all features |
| [Operator Runbook](docs/ops/operator-runbook.md) | Operations, backup/restore, incident triage |
| [Release Notes](docs/releases/v1beta-upgrade-notes.md) | Upgrade guidance for v1beta |
| [Architecture](docs/plans/the-one-mcp-architecture-prompt.md) | Design rationale and principles |
| [Production Overhaul Spec](docs/specs/2026-04-03-production-overhaul-design.md) | v0.2.0 design decisions |
| [Code Review](docs/reviews/2026-04-03-deep-dive-code-review.md) | Deep dive codebase analysis |

## Build and Verify

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
```

## Embedded Admin UI

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

| Endpoint | Description |
|----------|-------------|
| `/dashboard` | Health overview with config, metrics, audit summary |
| `/config` | Editable configuration form with limits |
| `/audit` | Audit event explorer |
| `/swagger` | Interactive Swagger UI |
| `/api/health` | JSON health check |
| `/api/swagger` | Raw OpenAPI JSON |
| `/api/config` | POST endpoint for config updates |

## License

Apache-2.0
