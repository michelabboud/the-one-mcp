# Changelog

All notable changes to this project are documented in this file.

## [0.8.0] - 2026-04-05

### Added
- **Watcher auto-reindex** — the file watcher now actually re-ingests changed markdown files instead of only logging events. Finishes the v0.7.0 watcher promise. Image auto-reindex still logs-only (deferred to v0.8.1).
- **Code-aware chunker** — language-aware chunking for 5 programming languages:
  - Rust (`.rs`): top-level `fn`, `struct`, `enum`, `impl`, `trait`, `mod`, `type`, `const`, `static`, `macro_rules!`
  - Python (`.py`): top-level `def`, `async def`, `class` with decorator handling
  - TypeScript (`.ts`, `.tsx`): `function`, `class`, `interface`, `type`, `enum`, `const`/`let`/`var` with template literal awareness
  - JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`): same engine as TypeScript
  - Go (`.go`): `func` (including method receivers), `type`, `var`, `const`, paren-block handling
- **`chunk_file` dispatcher** — automatically selects the right chunker by file extension; falls back to blank-line text chunking for unknown types
- **Extended `ChunkMeta`** — new optional fields: `language`, `symbol`, `signature`, `line_range`. LLMs can now see function signatures and line ranges in search results.
- **MemoryEngine methods** — `ingest_single_markdown(path)` for incremental updates, `remove_by_path(path)` for deletion
- User guide: `docs/guides/code-chunking.md`

### Changed
- `MemoryEngine` is now held as `Arc<RwLock<HashMap<String, MemoryEngine>>>` in the broker, enabling the watcher's spawned tokio task to hold its own reference for auto-reindex operations
- `split_on_blank_lines` helper promoted to `chunker.rs` as `pub(crate)` for sharing across language chunkers

### Dependencies
- `regex 1` (already a transitive dep, now direct for `the-one-memory`)

## [0.7.1] - 2026-04-05

### Fixed
- **Intel macOS build:** `embedded-ui` binary now respects the `no_local_embeddings` CI flag, fixing the `ort-sys@2.0.0-rc.11: ort does not provide prebuilt binaries for the target x86_64-apple-darwin` failure that blocked 1/6 platforms in the v0.7.0 release.
- `the-one-ui` crate now has proper feature passthrough (`local-embeddings`, `image-embeddings`, `embed-swagger`) so it can be built lean without fastembed.
- `the-one-mcp/src/broker.rs` and `the-one-memory/src/lib.rs` dead-code warnings on the `--no-default-features` build path (reranker import, hybrid_* fields, bm25_normalize function) now properly gated behind `#[cfg(feature = "local-embeddings")]`.

### Changed
- `the-one-ui` depends on `the-one-mcp` with `default-features = false`, then re-enables via its own feature passthrough.
- Release workflow `Build embedded UI` step now branches on `matrix.no_local_embeddings`, mirroring the pattern used for `the-one-mcp` binary builds.

## [0.7.0] - 2026-04-05

### Added
- **Hybrid search (dense + sparse)** — combine cosine similarity with lexical/sparse matching for better exact-match retrieval. Opt-in via `hybrid_search_enabled: true`. Default weights: 70% dense, 30% sparse.
- **File watcher for incremental indexing** — background tokio task watches `.the-one/docs/` and `.the-one/images/` and logs file changes. Opt-in via `auto_index_enabled: true`. Auto re-ingestion deferred to v0.7.1.
- **Screenshot-based image search** — `memory.search_images` now accepts optional `image_base64` field in addition to `query`. Exactly one must be provided. Enables image→image similarity via Nomic Vision dual-encoder.
- **Admin UI image gallery** — new `/images` route with thumbnail grid, `/images/thumbnail/<hash>` serving with security validation, `/api/images` JSON endpoint.
- 2 new user guides: `docs/guides/hybrid-search.md`, `docs/guides/auto-indexing.md`
- `fastembed::SparseTextEmbedding` integration (SPLADE++ as "bm25" alias since fastembed 5.13 lacks classical BM25)
- `notify` + `notify-debouncer-mini` dependencies
- `base64` + `tempfile` (regular deps in the-one-mcp)
- `CoreError::InvalidRequest(String)` variant

### Changed
- `ImageSearchRequest.query` is now `Option<String>` (was required) — either `query` or `image_base64` must be set
- `memory.search_images` tool schema updated: query no longer required, image_base64 added
- MCP tool count unchanged at 17 (extensions, not additions)

### Fixed
- **CI release workflow:** fetch-tags in release job checkout, git config identity set before tag creation
- **macOS x86_64 build:** now uses `no_local_embeddings: true` since fastembed 5.13's ort-sys dropped Intel Mac prebuilts

### Dependencies
- notify 6.1
- notify-debouncer-mini 0.4
- base64 0.22
- tempfile 3.x

## [0.6.0] - 2026-04-05

### Added
- Cross-encoder reranking for memory.search — jina-reranker-v2-base-multilingual default
- Image embedding and semantic search via fastembed 5.x ImageEmbedding API
- 5 image models: Nomic Vision (default, 768d, pairs with Nomic text), CLIP ViT-B/32, Resnet50, Unicom ViT-B/16, Unicom ViT-B/32
- OCR text extraction from images via tesseract (feature-gated)
- Thumbnail generation for indexed images
- 2 new MCP tools: `memory.search_images`, `memory.ingest_image`
- 3 new `maintain` actions: `images.rescan`, `images.clear`, `images.delete`
- 6 text model variants previously stubbed now working: BGE-M3, JinaEmbeddingsV2BaseEN, SnowflakeArcticEmbedM, AllMpnetBaseV2, EmbeddingGemma300M, SnowflakeArcticEmbedMQ
- Image model registry: `models/image-models.toml`
- Reranker model registry: `models/rerank-models.toml`
- User guides: `docs/guides/image-search.md`, `docs/guides/reranking.md`
- Config fields: `image_embedding_enabled`, `image_embedding_model`, `image_ocr_enabled`, `image_ocr_language`, `image_thumbnail_enabled`, `image_thumbnail_max_px`
- Limits: `max_image_size_bytes`, `max_images_per_project`, `max_image_search_hits`, `image_search_score_threshold`
- `CoreError::NotEnabled` variant for runtime feature gating
- Feature flags: `image-embeddings`, `image-ocr`

### Changed
- **BREAKING (internal):** fastembed bumped from 4 to 5.13 — API drift fixed (Arc<Mutex<>> wrappers for &mut self on embed/rerank)
- MCP tool count: 15 → 17
- JSON schema count: 31 → 35
- Test count: 183 → 208

### Dependencies
- fastembed 5.13
- image 0.25 (optional, image-embeddings feature)
- tesseract 0.15 (optional, image-ocr feature)

## [0.5.0] - 2026-04-05

### Changed
- **BREAKING:** MCP tool surface consolidated from 33 to 15 tools (~52% token reduction per session)
- `docs.get` now accepts optional `section` parameter (replaces `docs.get_section`)
- `docs.create` + `docs.update` merged into `docs.save` (upsert semantics)
- `tool.list` + `tool.suggest` + `tool.search` merged into `tool.find` with `mode` parameter
- 18 admin tools multiplexed into 4: `setup`, `config`, `maintain`, `observe`
- JSON schema files reduced from 63 to 31

### Added
- `docs.save` tool — upsert: creates if missing, updates if exists
- `tool.find` tool — unified discovery with modes: list, suggest, search
- `setup` tool — multiplexed: project init, refresh, profile
- `config` tool — multiplexed: export, update, tool.add, tool.remove, models.list, models.check
- `maintain` tool — multiplexed: reindex, tool.enable, tool.disable, tool.refresh, trash operations
- `observe` tool — multiplexed: metrics, audit events
- 9 new dispatch and API tests (183 total across workspace)

### Removed
- Individual tool endpoints replaced by consolidated tools: `project.init`, `project.refresh`, `project.profile.get`, `docs.get_section`, `docs.create`, `docs.update`, `docs.reindex`, `docs.trash.list`, `docs.trash.restore`, `docs.trash.empty`, `tool.list`, `tool.suggest`, `tool.search`, `tool.add`, `tool.remove`, `tool.enable`, `tool.disable`, `tool.update`, `config.export`, `config.update`, `metrics.snapshot`, `audit.events`, `models.list`, `models.check_updates`

## [0.4.0] - 2026-04-04

### Added
- TOML-based embedding model registry (`models/local-models.toml`, `models/api-models.toml`) embedded in binary
- Interactive embedding model selection during install (7 local models + API option)
- 2 new MCP tools: `models.list`, `models.check_updates`
- Model registry maintenance scripts (`update-local-models.sh`, `update-api-models.sh`)
- API embedding provider support: OpenAI, Voyage AI, Cohere (extensible)

### Changed
- Default embedding model changed from all-MiniLM-L6-v2 (384d) to BGE-large-en-v1.5 (1024d quality tier)
- Embedding provider rewritten to use TOML registry with tier resolution
- 33 total MCP tools, 174 tests

## [0.3.1] - 2026-04-04

### Added
- SECURITY.md with vulnerability reporting and security design documentation
- INSTALL.md with complete installation guide
- Weekly security CI: cargo-audit (dependency CVEs) + gitleaks (secret scanning)
- `build.sh release` command for triggering cross-platform GitHub Actions releases
- Manual-only release workflow (workflow_dispatch) — tags no longer auto-trigger builds
- Partial release support: creates GitHub Release even if some platform builds fail

### Changed
- Release workflow: removed sccache (caused failures), switched macOS to macos-latest
- .gitignore hardened: blocks .env, secrets/, keys, certs, IDE, OS files
- Repo made public — curl one-liner install now works
- All docs updated for v0.3.0 features + release workflow + security

### Security
- Added cargo-audit weekly scanning for dependency vulnerabilities
- Added gitleaks scanning for accidentally committed secrets
- Hardened .gitignore to prevent secret exposure in public repo

## [0.3.0] - 2026-04-03

### Added
- Tool catalog system with SQLite storage and FTS5 full-text search
- Qdrant semantic search for tool discovery (with FTS5 fallback)
- System inventory scanning (auto-detects installed tools via `which`)
- Per-CLI tool enable/disable state tracking
- 7 new MCP tools: tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update, tool.list
- 31 total MCP tools with JSON Schema definitions
- Catalog seed: 16 Rust tools, 4 security tools, 8 MCPs
- Catalog changelog and diff-based update mechanism
- Tool entries with LLM-optimized metadata (when_to_use, what_it_finds)
- tool.suggest now returns grouped results: enabled, available, recommended
- tool.search uses semantic search (Qdrant) with FTS5 fallback

### Changed
- tool.suggest filters by project profile (languages, frameworks)
- tool.search tries Qdrant semantic search first, then FTS5, then registry fallback

## [0.2.1] - 2026-04-03

### Added
- Multi-CLI support: Claude Code, Gemini CLI, OpenCode, Codex — auto-detected and registered
- Tiered embedding models: fast (384d), balanced (768d), quality (1024d), multilingual (1024d) + 15 models
- Quantized model variants with `-q` suffix for smaller downloads
- Per-CLI custom tools: `custom-claude.json`, `custom-gemini.json`, `custom-opencode.json`, `custom-codex.json`
- `install.sh` — one-command installer with OS/arch detection, release download, CLI registration, smoke test
- `build.sh` — production build manager (build, dev, test, check, package, install, clean, info)
- `tools/recommended.json` — 15 pre-built tool definitions, auto-downloaded during install
- Cross-platform release workflow: Linux/macOS/Windows x86-64 + ARM64 (6 targets)
- `CLAUDE.md` for Claude Code development guidance
- `available_models()` function listing all supported embedding models
- `resolve_model()` supporting tier aliases and full model names

### Changed
- Embedding provider now uses `resolve_model()` for flexible model selection
- Installer shows version of each detected CLI
- README rewritten with install command, multi-CLI table, embedding tiers, per-CLI tools

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
