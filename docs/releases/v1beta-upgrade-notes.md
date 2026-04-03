# v1beta Upgrade Notes

Last updated: 2026-04-03

## v0.2.0 — Production Overhaul

### Breaking Changes

- `MemorySearchItem.score` changed from `usize` (0-100) to `f32` (0.0-1.0)
- `PolicyLimits` struct removed; replaced by `ConfigurableLimits`
- `MemoryEngine` API changed: `new()` replaced by `new_local()`, `new_with_qdrant()`, `new_api()`
- `QdrantHttpOptions` renamed to `QdrantOptions`
- All broker methods are now `async`

### New MCP Tools (10 added, 24 total)

- `docs.create` — create managed markdown file
- `docs.update` — update existing managed file
- `docs.delete` — soft-delete to `.trash/`
- `docs.move` — rename/move within managed folder
- `docs.trash.list` — list trash contents
- `docs.trash.restore` — restore from trash
- `docs.trash.empty` — permanently empty trash
- `docs.reindex` — force full re-indexing
- `config.update` — update project configuration

### New Configuration Fields

- `embedding_provider` — `"local"` or `"api"`
- `embedding_model` — model name (default: `all-MiniLM-L6-v2`)
- `embedding_api_base_url` — URL for API embeddings
- `embedding_api_key` — API key for embeddings
- `embedding_dimensions` — vector dimensions (default: 384)
- `nano_providers` — array of OpenAI-compatible provider configs
- `nano_routing_policy` — `"priority"`, `"round_robin"`, or `"latency"`
- `external_docs_root` — external docs directory for read-only ingestion
- `limits` — object with 12 configurable limit fields

### New Environment Variables

- `THE_ONE_EMBEDDING_PROVIDER`, `THE_ONE_EMBEDDING_MODEL`, `THE_ONE_EMBEDDING_API_BASE_URL`, `THE_ONE_EMBEDDING_API_KEY`, `THE_ONE_EMBEDDING_DIMENSIONS`
- `THE_ONE_EXTERNAL_DOCS_ROOT`
- `THE_ONE_LIMIT_*` (one per limit field)

### New Binary

`the-one-mcp serve` — standalone MCP server binary with transport selection:
- `--transport stdio` (default) — for Claude Code / Codex
- `--transport sse` — HTTP + SSE
- `--transport stream` — streamable HTTP

### Operator Actions

1. Update client code for async broker methods (all return futures now)
2. Update `MemorySearchItem.score` handling from integer to float
3. Replace `PolicyLimits` with `ConfigurableLimits` in any custom policy construction
4. Update any `MemoryEngine::new()` calls to use appropriate factory method
5. Review and set `limits` in project config for token efficiency
6. Configure `nano_providers` if using LLM-based routing
7. Set `embedding_provider` and related fields if using API embeddings

### Compatibility

- Schema namespace remains `v1beta`
- New tools are additive
- Config fields have safe defaults (no action required for basic usage)
- Old `nano_provider` / `nano_model` fields still work for backward compatibility
