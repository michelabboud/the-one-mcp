# Changelog

All notable changes to this project are documented in this file.

## [0.2.0] - 2026-04-03

### Added
- MCP JSON-RPC transport layer with stdio, SSE, and streamable HTTP support
- Production-grade RAG with fastembed-rs local embeddings (384-dim) and OpenAI-compatible API embeddings
- Smart markdown chunker with heading hierarchy and paragraph-safe splitting
- Async Qdrant HTTP backend with collection management and scored search
- Managed documents system with CRUD, soft-delete to .trash, and auto-sync
- Nano LLM provider pool with up to 5 OpenAI-compatible providers
- Three routing policies: priority, round_robin, latency
- Per-provider health tracking with cooldown strategy
- TCP connect pre-flight checks before provider classification
- Configurable limits (12 parameters) with validation bounds and env var support
- `the-one-mcp` CLI binary with `serve` command supporting --transport stdio|sse|stream
- 10 new MCP tools: docs.create, docs.update, docs.delete, docs.move, docs.trash.list, docs.trash.restore, docs.trash.empty, docs.reindex, config.update
- 24 total MCP tools with JSON Schema definitions

### Changed
- All broker methods are now async (tokio)
- MemoryEngine uses real embeddings instead of hash-based stubs
- Router supports async provider pool alongside existing sync methods
- PolicyEngine uses ConfigurableLimits instead of hardcoded PolicyLimits
- MemorySearchItem.score changed from usize to f32

### Fixed
- Config test env var pollution (temp-env isolation)

## [Unreleased]

### Added

- Complete implementation guide: `docs/guides/the-one-mcp-complete-guide.md`
- Quickstart guide: `docs/guides/quickstart.md`
- Embedded UI runtime endpoints for dashboard/health/audit/config and config update API
- Interactive Swagger UI page (`/swagger`) in addition to raw OpenAPI JSON (`/api/swagger`)
- Editable config UX on `/config` backed by `POST /api/config`
- Embedded swagger support in MCP (`embed-swagger` feature, default enabled)
- Swagger asset: `schemas/mcp/v1beta/openapi.swagger.json`
- Release gate script + CI release-gate job
- Router hard-bound telemetry fields and provider error tracking
- Remote Qdrant strict-auth enforcement and auth/TLS config knobs
- Qdrant HTTP backend tests and router soak tests

### Changed

- Expanded MCP config export contract to include Qdrant auth/TLS/strict mode visibility
- Expanded memory search response contract with route and telemetry metadata
- Strengthened schema validation/tests to enforce schema inventory and metadata consistency

### Security

- Enforced fail-closed behavior for remote Qdrant when strict auth is enabled and API key is missing
