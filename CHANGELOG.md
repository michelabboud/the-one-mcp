# Changelog

All notable changes to this project are documented in this file.

## [0.2.0] - 2026-04-03

### Added
- MCP JSON-RPC transport layer with stdio, SSE, and streamable HTTP support
- `the-one-mcp` CLI binary with `serve` command (clap) supporting `--transport stdio|sse|stream`
- Production-grade RAG with fastembed-rs local embeddings (384-dim ONNX, `all-MiniLM-L6-v2`)
- OpenAI-compatible API embedding provider for hosted embeddings
- Smart markdown chunker with heading hierarchy tracking, paragraph-safe splitting, code block preservation
- Async Qdrant HTTP backend with collection management, scored cosine search, and point deletion
- Managed documents system: full CRUD (`docs.create/update/delete/get/list/move`)
- Soft-delete to `.trash/` with `docs.trash.list`, `docs.trash.restore`, `docs.trash.empty`
- `docs.reindex` tool for forcing full re-ingestion into RAG
- `config.update` tool for updating project configuration via MCP
- Nano LLM provider pool with up to 5 OpenAI-compatible providers (Ollama, LM Studio, LiteLLM, etc.)
- Three routing policies: priority, round_robin, latency
- Per-provider health tracking with cooldown strategy (5s/15s/60s) and TCP pre-flight checks
- Configurable limits (12 parameters) with validation bounds, env var support, and admin UI editing
- 24 total MCP tools with JSON Schema definitions (49 schema files)
- Complete implementation guide, quickstart, operator runbook, architecture docs

### Changed
- All broker methods are now async (tokio)
- `MemoryEngine` uses real 384-dim embeddings instead of 16-dim hash-based stubs
- `MemorySearchItem.score` changed from `usize` (0-100) to `f32` (0.0-1.0) for real similarity scores
- Router supports async provider pool alongside existing sync methods
- `PolicyEngine` uses `ConfigurableLimits` (12 fields) instead of hardcoded `PolicyLimits` (4 fields)
- `reqwest` switched from blocking to async throughout
- `std::sync::Mutex` replaced with `tokio::sync::RwLock` for concurrent access
- Expanded config with embedding, nano provider pool, limits, and external docs fields
- Expanded MCP config export to include Qdrant auth/TLS/strict mode visibility

### Fixed
- Config test env var pollution between parallel test runs (isolated with `temp-env`)
- Async future not awaited in embedded-ui binary

### Security
- Enforced fail-closed behavior for remote Qdrant when strict auth enabled and API key missing
- Path traversal protection in managed docs (rejects `../`)
- Document size and count limits enforced on CRUD operations

## [0.1.0] - 2026-04-03

### Added
- Initial workspace with 8 crates
- Project lifecycle: init, refresh, profile detection, fingerprinting
- SQLite storage with WAL mode, migrations, approvals, audit events
- Capability registry with risk-tier filtering and visibility modes
- Rules-first router with nano provider abstraction and hard budget bounds
- Memory ingestion with Qdrant HTTP/local/keyword backends (stub embeddings)
- Claude and Codex adapters with parity tests
- Embedded admin UI: dashboard, config, audit, swagger pages
- Policy engine with approval scopes (once/session/forever)
- 5-layer config precedence (defaults/global/project/env/runtime)
- 33 v1beta JSON schemas with contract validation tests
- CI pipeline with release gate script
- Operator runbook and architecture documentation
