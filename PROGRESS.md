# Progress Report

## Current Version: v0.16.0-phase1

**In-flight roadmap:** v0.16.0 multi-backend support (Phases 1–7 per
`docs/plans/2026-04-11-resume-phase1-onwards.md`). Phase 0 (trait
extraction) shipped as v0.16.0-rc1 bundled with v0.15.0 production
hardening and v0.15.1 Lever 1. Phase 1 (broker call-site migration
via `StateStore` trait) shipped as tag `v0.16.0-phase1`. Phases 2–7
(pgvector, Postgres state, combined Postgres, Redis state modes,
combined Redis, Redis-Vector parity, v0.16.0 GA release) are still
ahead.

## Overall Status

All planned stages complete. Twenty-four tracked releases shipped
(plus two in-progress milestones for v0.16.0):

- **v0.1.0** — Initial workspace: 8 crates, 14 MCP tools, stub implementations
- **v0.2.0** — Production overhaul: async broker, real embeddings, 3 transports, 24 tools
- **v0.3.0** — Tool catalog: SQLite + Qdrant semantic search, tool lifecycle, 31 tools
- **v0.4.0** — Embedding model registry: TOML-based model registries, quality tier default, interactive installer selection, 33 tools
- **v0.5.0** — Tool consolidation: 33→15 tools (~52% token savings), multiplexed admin, merged work tools
- **v0.6.0** — Multimodal: image embeddings, OCR, reranking (fastembed 5.x), 17 tools, 208 tests
- **v0.7.0** — Hybrid search (dense+sparse), file watcher, admin UI image gallery, screenshot image search, 234 tests
- **v0.8.0** — Watcher auto-reindex (markdown), code-aware chunker (5 languages), extended ChunkMeta, 272 tests
- **v0.8.1** — Documentation refresh: all guides + root docs audited for v0.8.0 accuracy
- **v0.8.2** — Image auto-reindex: watcher now re-ingests image upserts/deletes, standalone helpers
- **v0.9.0** — Tree-sitter AST chunker for 13 languages (5 existing + C/C++/Java/Kotlin/PHP/Ruby/Swift/Zig), retrieval benchmark suite, 283 tests
- **v0.10.0** — MCP Resources API (`resources/list`, `resources/read`, `the-one://` URI scheme), catalog expansion (117 → 184 tools across 10 languages), landing page scaffold, 296 tests
- **v0.12.0** — Phase 3 bundled: Intel Mac `local-embeddings-dynamic` feature flag, observability deep dive (+8 metric counters + per-operation latency + Arc<BrokerMetrics>), backup/restore via `maintain: backup` and `maintain: restore` with tar+gzip, 300 tests
- **v0.12.1** — Docs refresh: 3 new guides (mcp-resources, backup-restore, observability) + README/CLAUDE.md/api-reference/tool-catalog/upgrade-guide/troubleshooting updated for v0.12.0 feature surface
- **v0.13.0** — Major UI overhaul (landing page, /ingest, /graph, v2 dashboard, top nav with project switcher, shared page shell, dark-mode-aware) + Graph RAG end-to-end wiring (extraction pipeline, `maintain: graph.extract`/`graph.stats`, LightRAG-inspired research) + graph-rag.md guide, 302 tests
- **v0.13.1** — Full LightRAG parity: entity name normalization, entity/relation description vector store (3 Qdrant collections), description summarization, query keyword extraction, gleaning/continue-extraction pass, canvas force-directed graph visualization, 308 tests
- **v0.14.0** — Catalog expansion to 365 tools (+248 new from baseline 117). All 10 language files + all 8 category files populated. Closes the deferred Task 5 from the 9-item roadmap.
- **v0.14.1** — Documentation refresh for v0.14.0 catalog expansion and counts.
- **v0.14.2** — Production hardening completion: real Redis vector backend runtime path, `models.check` real check flow, `the-one://catalog/enabled` backed by catalog DB, wake-up `wing/hall/room` filters, test determinism fixes, docs hardening.
- **v0.14.3** — MemPalace production controls: on/off feature toggles, first-class hook capture (`maintain: memory.capture_hook` for `stop`/`precompact`), config/env/runtime wiring, and strict feature gating.
- **v0.15.0** — Production hardening pass addressing every finding from `docs/reviews/2026-04-10-mempalace-comparative-audit.md` (C1–C5, H1–H5, M1–M6). New modules `the_one_core::{naming, pagination, audit}`. Schema v7 adds `outcome`/`error_kind` columns to `audit_events` with indexes. Cursor pagination replaces silent truncation across every list/search endpoint (over-limit requests return `InvalidRequest`). Input sanitization at every broker write entry point via `sanitize_name`/`sanitize_project_id`/`sanitize_action_key`. Error envelope sanitization via `public_error_message` with `corr=<id>` correlation IDs — `CoreError::Sqlite`/`Io`/`Json`/`Embedding` surface only their kind labels; internal details stay in `tracing::error!`. Navigation node digest widened 12→32 hex chars (48→128 bits). 23 new tests (13 `production_hardening` + 9 `stdio_write_path` + 1 lever1 guard — the 1 ignored). New benchmark `production_hardening_bench.rs`. New guide `docs/guides/production-hardening-v0.15.md`.
- **v0.15.1** — Lever 1 audit-write speedup: `ProjectDatabase::open` sets `PRAGMA synchronous=NORMAL` in WAL mode. Measured **67× faster** audit writes (5.56 ms → 83 µs per row). Durability trade-off: safe against process crash (WAL captures every commit), exposed to < 1 s of writes on OS crash — the standard modern-SQLite production setting used by Firefox, Android, rqlite, Litestream, Turso. 2 regression tests (throughput smoke + cross-cutting guard). Lever 2 async batching designed in parallel but explicitly deferred — plans preserved in `docs/plans/2026-04-10-audit-batching-lever2.md`.
- **v0.16.0-rc1** — Phase A multi-backend trait extraction. New `trait VectorBackend` in `the_one_memory::vector_backend` covering chunks/entities/relations/images/hybrid vector operations; `trait StateStore` in `the_one_core::state_store` covering all 22 broker-called methods on `ProjectDatabase`. `MemoryEngine` now holds `Option<Box<dyn VectorBackend>>` (was two concrete Option fields); canonical constructor `MemoryEngine::new_with_backend(embedding_provider, backend, max_chunk_tokens)`. `impl VectorBackend for AsyncQdrantBackend` (full), `impl VectorBackend for RedisVectorStore` (chunks-only, feature-gated), `impl StateStore for ProjectDatabase` (thin forwarding, zero behaviour change). Diary upsert atomicity fix: main INSERT + DELETE FTS + INSERT FTS wrapped in one `unchecked_transaction`. `BackendCapabilities` / `StateStoreCapabilities` for capability reporting. **Bundled with v0.15.0 + v0.15.1 as commit `5ff9872`** because three files carried interleaved changes from all three versions.
- **v0.16.0-phase1** *(in-progress milestone)* — Broker `state_by_project` cache via `StateStore` trait. Mechanical refactor landing the broker-side piece of the multi-backend roadmap. All 16 `ProjectDatabase::open` call sites in `broker.rs` now route through a new `with_state_store(project_root, project_id, |store| ...)` chokepoint. Inner lock is `std::sync::Mutex` (deliberately not tokio) so the guard is `!Send` — the compiler refuses to hold a backend connection across `.await`, preventing Postgres/Redis connection-pool deadlocks in Phase 3+. `get_or_init_state_store` constructs new entries outside the outer write lock (double-checks under it) — load-bearing for Phase 3+ async factories. `pub async fn McpBroker::shutdown()` drains the cache. Two handlers (`memory_ingest_conversation`, `tool_run`) restructured to split async memory/session work from sync DB work. New test `broker_state_store_cache_reuses_connections` verifies `Arc::ptr_eq` identity across repeated lookups, per-project isolation, and clean shutdown drain. Zero user-visible behaviour change. Commit `7666439`, tag `v0.16.0-phase1`. Completes the call-site migration that v0.16.0-rc1 explicitly deferred.
- **MemPalace phase 2** — completed production feature set:
  - AAAK compression + lesson persistence (`memory.aaak.*`)
  - explicit drawers/closets/tunnels primitives (`memory.navigation.*`)
  - diary-specific memory flows (`memory.diary.*`) with refresh-safe identity
  - single-switch profile control (`config: profile.set` + Admin UI preset card)

Build/test gates: all green. **450 tests passing** (+1 ignored — the Lever 2 deferred guard). 365 catalog tools. 17 MCP tools + 3 MCP resource types.

## Stats

### Historical (v0.1.0 → v0.12.0)

| Metric | v0.1.0 | v0.2.0 | v0.3.0 | v0.4.0 | v0.5.0 | v0.6.0 | v0.7.0 | v0.8.0 | v0.8.2 | v0.9.0 | v0.10.0 | v0.12.0 |
|--------|--------|--------|--------|--------|--------|--------|--------|--------|--------|--------|---------|---------|
| MCP Tools | 14 | 24 | 31 | 33 | 15 | 17 | 17 | 17 | 17 | 17 | 17 | **17** |
| MCP Resources | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 3 types | **3 types** |
| Tests | 68 | 122 | 135 | 174 | 183 | 208 | 234 | 272 | 272 | 283 | 296 | **300** |
| Supported code languages | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 5 | 13 | 13 | **13** |
| Metrics counters | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | **15** |
| maintain actions | — | — | — | — | 10 | 11 | 12 | 12 | 12 | 12 | 12 | **14** |
| Rust LOC | 6,400 | ~10,000 | ~12,800 | ~14,000 | ~14,200 | ~16,500 | ~19,000 | ~21,000 | ~21,100 | ~22,500 | ~23,200 | **~24,000** |
| JSON Schemas | 33 | 49 | 63 | 63 | 31 | 35 | 35 | 35 | 35 | 35 | 35 | **35** |
| Catalog Tools | — | — | 28 | 28 | 28 | 28 | 28 | 28 | 28 | 28 | 184 | **365** |
| Platforms | 1 | 1 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | **6** |
| AI CLIs | 2 | 2 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | **4** |

### Recent (v0.13.0 → v0.16.0-phase1)

| Metric | v0.13.0 | v0.13.1 | v0.14.0 | v0.14.3 | v0.15.0 | v0.15.1 | v0.16.0-rc1 | v0.16.0-phase1 |
|--------|---------|---------|---------|---------|---------|---------|-------------|----------------|
| MCP Tools | 17 | 17 | 17 | 17 | 17 | 17 | 17 | **17** |
| MCP Resources | 3 types | 3 types | 3 types | 3 types | 3 types | 3 types | 3 types | **3 types** |
| Tests (passing) | 302 | 308 | 308 | 387 | 426 | 428 | 449 | **450** |
| Tests (ignored) | 0 | 0 | 0 | 1 | 1 | 1 | 1 | **1** |
| Supported code languages | 13 | 13 | 13 | 13 | 13 | 13 | 13 | **13** |
| Metrics counters | 15 | 15 | 15 | 15 | 15 | 15 | 15 | **15** |
| maintain actions | 16 | 16 | 16 | 17 | 17 | 17 | 17 | **17** |
| Catalog Tools | 184 | 184 | 365 | 365 | 365 | 365 | 365 | **365** |
| SQLite schema version | v5 | v5 | v5 | v6 | **v7** | v7 | v7 | **v7** |
| Backend traits | 0 | 0 | 0 | 0 | 0 | 0 | **2** (`VectorBackend`, `StateStore`) | 2 |
| Broker DB pattern | per-call `open` | per-call `open` | per-call `open` | per-call `open` | per-call `open` | per-call `open` | per-call `open` (trait impl exists, unused by broker) | **cached via `with_state_store` chokepoint** |
| Audit write latency (per row) | ~5.5 ms | ~5.5 ms | ~5.5 ms | ~5.5 ms | ~5.5 ms | **~83 µs** (67× faster, Lever 1) | ~83 µs | ~83 µs |
| Error sanitization | raw | raw | raw | raw | **`public_error_message` envelope + `corr=<id>`** | same | same | same |
| Cursor pagination | silent truncation | silent truncation | silent truncation | silent truncation | **`InvalidRequest` on over-limit** | same | same | same |

**Note on the test count jump from v0.13.1 (308) → v0.14.3 (387):** the ~80-test gap covers the v0.14.x series of additions that weren't individually tracked in this file. The v0.14.3 count (387 passing + 1 ignored) is the number PROGRESS.md was stuck at before the v0.16.0 roadmap started. The v0.15.0 count of 426 is back-computed as v0.14.3 + 23 new tests + 16 other contemporaneous additions (audit outcome tests, sanitization tests, pagination tests not listed separately in CHANGELOG). v0.15.1 adds 2. v0.16.0-rc1 adds ~21 (diary atomicity, trait sanity, capability reporting). v0.16.0-phase1 adds 1 (the cache-reuse test). Final baseline measured at commit `5ff9872` was 449 passing / 1 ignored; after Phase 1's commit `7666439` it is **450 passing / 1 ignored**.

## Stage Progress (v0.1.0)

- Stage 0: Program setup — complete
- Stage 1: Core foundations — complete
- Stage 2: Isolation/lifecycle — complete
- Stage 3: Profiler/fingerprint — complete
- Stage 4: Registry/policy/approvals — complete
- Stage 5: Docs/RAG plane — complete
- Stage 6: Router rules+nano — complete
- Stage 7: MCP contracts/versioning — complete
- Stage 8: Claude/Codex parity — complete
- Stage 9: UI/ops/hardening — complete

## Production Overhaul (v0.2.0) — 23 Tasks

All complete: async broker, fastembed embeddings, smart chunker, async Qdrant, provider pool with health tracking, managed documents with soft-delete, configurable limits, stdio/SSE/streamable HTTP transports, clap CLI binary.

## Multi-CLI + Installer (v0.2.1)

All complete: Claude Code + Gemini CLI + OpenCode + Codex auto-detection, tiered embedding models (fast/balanced/quality/multilingual), per-CLI custom tools, install.sh one-command installer, build.sh build manager, cross-platform release workflow.

## Tool Catalog Integration (v0.3.0) — 7 Tasks

- Cat-1: SQLite schema + error variant — complete
- Cat-2: ToolCatalog struct with DB, import, query, scan — complete
- Cat-3: API types for 6 new tools — complete
- Cat-4: Broker methods for catalog tools — complete
- Cat-5: Transport dispatch + catalog bootstrap — complete
- Cat-6: Qdrant semantic search for tools — complete
- Cat-7: Changelog, schemas, docs, validation, tag — complete

## Key Features Delivered

### Tool Catalog (v0.3.0)
- SQLite catalog.db with FTS5 full-text search
- Qdrant semantic search over tool descriptions (with FTS5 fallback)
- System inventory scanning (auto-detects installed tools via `which`)
- Per-CLI per-project tool enable/disable state
- 7 new MCP tools: tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update, tool.list
- Curated catalog seed: 16 Rust tools, 4 security tools, 8 official MCPs
- tool.suggest returns grouped results: enabled / available / recommended
- tool.search: semantic (Qdrant) → FTS5 → registry fallback chain

### Production RAG (v0.2.0)
- fastembed-rs with tiered models (384-1024 dim ONNX, offline, free)
- OpenAI-compatible API embedding provider
- Smart markdown chunker (heading-aware, paragraph-safe, code-block preserving)
- Async Qdrant HTTP backend with collection management

### Managed Documents (v0.2.0)
- Full CRUD with soft-delete to .trash/
- Auto-sync on project.refresh
- docs.reindex for full re-ingestion

### Multi-CLI Support (v0.2.1)
- Claude Code, Gemini CLI, OpenCode, Codex
- Per-CLI custom tools
- One-command installer with auto-registration

### MCP Transport (v0.2.0)
- stdio (Claude Code, Gemini, OpenCode, Codex)
- SSE (web clients)
- Streamable HTTP (MCP spec compliant)

### Nano LLM Provider Pool (v0.2.0)
- Up to 5 OpenAI-compatible providers
- Priority / round-robin / latency routing
- Per-provider health tracking with cooldown
- TCP pre-flight checks

## Embedding Model Registry (v0.4.0) — 8 Tasks

- Task 1+2: TOML registry files + models_registry module — complete
- Task 3: Rewrite embeddings.rs to use registry — complete
- Task 4: Update config defaults to quality tier — complete
- Task 5: Add models.list and models.check_updates MCP tools — complete
- Task 6: Interactive model selection in installer — complete
- Task 7: Maintenance scripts — complete
- Task 8: Full integration validation — complete

### Key Features Delivered

- TOML model registries (models/local-models.toml, models/api-models.toml) embedded in binary
- Default changed from all-MiniLM-L6-v2 (384d) to BGE-large-en-v1.5 (1024d)
- Interactive model selection during install (7 local + API option)
- API provider support: OpenAI, Voyage AI, Cohere (extensible)
- 2 new MCP tools: models.list, models.check_updates
- Maintenance scripts for tracking upstream model updates

## Infrastructure (v0.3.1)

- SECURITY.md with vulnerability reporting policy and security design documentation
- Hardened .gitignore (secrets, keys, certs, IDE, OS files)
- Weekly cargo-audit + gitleaks CI (security.yml)
- Manual-only release workflow (workflow_dispatch, no auto-trigger on tags)
- `build.sh release` command for triggering cross-platform builds
- Repo made public — curl one-liner install works
- GitHub Release v0.3.1 with 4 platform binaries (Linux x86, macOS x86+ARM, Windows x86)

## Verification Snapshot

**Captured at:** commit `7666439` (tag `v0.16.0-phase1`), 2026-04-11.

- `cargo fmt --check` — passing
- `cargo clippy --workspace --all-targets -- -D warnings` — passing
- `cargo test --workspace` — **450 passing, 1 ignored** (the 1 ignored is the Lever 2 async-batching deferred guard from v0.15.1 — intentional)
- `cargo build --release -p the-one-mcp --bin the-one-mcp` — passing (48M binary)
- `bash scripts/release-gate.sh` — passing (full debug + release profile rebuild)
- `bash scripts/build.sh check` — full CI pipeline passing

## Tool Consolidation (v0.5.0) — 6 Tasks

- Task 1: Add DocsSaveRequest and ToolFindRequest API types — complete
- Task 2: Consolidate 33 tool definitions to 15 — complete
- Task 3: Rewrite dispatch logic with multiplexed admin — complete
- Task 4: Update JSON schemas (63→31) — complete
- Task 5: Update documentation and full validation — complete
- Task 6: Verify token reduction — complete

### Key Changes
- 11 work tools always loaded (memory, docs, tool discovery/lifecycle)
- 4 multiplexed admin tools (setup, config, maintain, observe) with action+params pattern
- `docs.get` + `docs.get_section` → `docs.get` with optional `section` param
- `docs.create` + `docs.update` → `docs.save` (upsert)
- `tool.list` + `tool.suggest` + `tool.search` → `tool.find` with `mode` param
- Estimated token savings: ~1,836 tokens per session (~52% reduction)

## Multimodal + Reranking (v0.6.0) — 3 Bundles

- Bundle 1: fastembed 4→5.13 migration + reranking infrastructure
  - fastembed API drift fixed (Arc<Mutex<>> wrappers, &mut self methods)
  - 6 previously stubbed text model variants now working
  - Reranker model registry (`models/rerank-models.toml`)
  - `TextRerank` via fastembed, jina-reranker-v2-base-multilingual default
  - `reranker_enabled` + `reranker_model` + `rerank_fetch_multiplier` config fields
  - Reranker integrated into `memory.search` via MemoryEngine

- Bundle 2: Image embedding pipeline
  - Image model registry (`models/image-models.toml`) — 5 models
  - `ImageEmbeddingProvider` trait + `FastEmbedImageProvider` implementation
  - Image ingest module: format validation, size limits, EXIF stripping
  - Qdrant `the_one_images` collection with per-project isolation
  - OCR via tesseract wrapper (`image-ocr` feature flag)
  - Thumbnail generation via `image` crate (`image-embeddings` feature flag)
  - 2 new MCP tools: `memory.search_images`, `memory.ingest_image`
  - 3 new maintain actions: `images.rescan`, `images.clear`, `images.delete`
  - Config fields: `image_embedding_enabled/model`, `image_ocr_enabled/language`, `image_thumbnail_enabled/max_px`
  - Limits: `max_image_size_bytes`, `max_images_per_project`, `max_image_search_hits`, `image_search_score_threshold`
  - 4 new JSON schemas (31 → 35), 25 new tests (183 → 208)

- Bundle 3: Documentation + release
  - New user guides: `docs/guides/image-search.md`, `docs/guides/reranking.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - v0.6.0 tagged and cross-platform release triggered

## Hybrid Search + Watcher + UI Gallery (v0.7.0) — 5 Phases

- Phase A: Sparse embeddings trait + BM25/SPLADE
  - `SparseEmbeddingProvider` trait in `the-one-memory`
  - `FastEmbedSparseProvider` using `fastembed::SparseTextEmbedding` with `SPLADEPPV1`
  - Note: fastembed 5.13 calls this "bm25" alias but the model is SPLADE++Ensemble Distil

- Phase B-D: Qdrant hybrid collection + MemoryEngine integration + config
  - `HybridQdrantCollection` with named dense + sparse vector support
  - `MemoryEngine::search_hybrid` fusing both signals with configurable weights
  - Config fields: `hybrid_search_enabled`, `hybrid_dense_weight`, `hybrid_sparse_weight`, `sparse_model`
  - Score normalization: saturation function for sparse scores

- Phase E-F: File watcher + broker wiring
  - `notify 6.1` + `notify-debouncer-mini 0.4` dependencies
  - `crates/the-one-mcp/src/watcher.rs` — background tokio task per project
  - Config fields: `auto_index_enabled`, `auto_index_debounce_ms` (default 2000ms)
  - Watches `.the-one/docs/` (*.md) and `.the-one/images/` (*.png/jpg/jpeg/webp)
  - Events logged; auto-reingestion deferred to v0.7.1

- Phase G: Screenshot image search
  - `ImageSearchRequest.query` changed to `Option<String>`
  - New optional `image_base64` field — base64-encoded image for image→image similarity
  - Mutual exclusion enforced: exactly one of query or image_base64 must be set
  - Decodes base64 → tempfile → embedding → Qdrant query
  - `CoreError::InvalidRequest(String)` added to error enum

- Phase H: Admin UI image gallery
  - `/images` route: thumbnail grid of all indexed images for active project
  - `/images/thumbnail/<hash>` serving with regex security validation on hash
  - `/api/images` JSON endpoint returning image metadata

- Phase I-J (this release): Documentation + release
  - New guides: `docs/guides/hybrid-search.md`, `docs/guides/auto-indexing.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - v0.7.0 tagged and cross-platform release triggered

## Watcher Auto-Reindex + Code Chunker (v0.8.0) — 4 Phases

- Phase A+B: Watcher auto-reindex
  - `ingest_single_markdown(path)` and `remove_by_path(path)` added to `MemoryEngine`
  - `MemoryEngine` HashMap promoted to `Arc<RwLock<...>>` shared between broker and watcher task
  - Watcher tokio task now calls `ingest_single_markdown` on `Create`/`Modify` events and `remove_by_path` on `Remove` events
  - Image events still log-only (auto-reindex deferred to v0.8.1)
  - Integration test: `test_watcher_auto_reindex` with 2s debounce verification

- Phase C: Code chunker core + Rust
  - `ChunkMeta` extended with `language`, `symbol`, `signature`, `line_range` fields
  - `chunk_file(path, content, max_tokens)` dispatcher — selects chunker by extension
  - `split_on_blank_lines` promoted to `pub(crate)` for sharing across chunkers
  - Rust chunker: brace-depth tracking, `impl … for …` detection, all top-level Rust item types
  - `regex 1` added as direct dependency of `the-one-memory`

- Phase D: Python/TypeScript/JavaScript/Go chunkers
  - Python chunker: indentation-based, decorator handling, `async def` support
  - TypeScript/JavaScript chunker: shared engine, template-literal-aware brace tracking
  - Go chunker: method receiver detection (`func (r *T) Method`), paren-block handling for `var`/`const`
  - All 4 chunkers tested: 14 new tests covering edge cases (decorators, receivers, template literals, paren blocks)

- Phase E: Documentation + release
  - New guide: `docs/guides/code-chunking.md`
  - All top-level docs updated (README, CHANGELOG, PROGRESS, CLAUDE.md, INSTALL.md, VERSION)
  - `docs/guides/auto-indexing.md` updated to reflect watcher now does real re-ingestion
  - v0.8.0 tagged and cross-platform release triggered

## Multi-Backend Roadmap (v0.15.0 → v0.16.0)

The v0.16.0 series is a focused infrastructure release that lands full
multi-backend support for both vectors and state. The roadmap was
authored in `docs/plans/2026-04-11-multi-backend-architecture.md`
(design) and `docs/plans/2026-04-11-resume-phase1-onwards.md`
(execution plan). v0.15.0 and v0.15.1 were production-hardening
prerequisites that shipped bundled with the trait extraction.

### Phases and status

| Phase | Name | Status | Commit | Tag |
|-------|------|--------|--------|-----|
| **0** | Trait extraction (`VectorBackend` + `StateStore`) bundled with v0.15.0 hardening + v0.15.1 Lever 1 | ☑ DONE | `5ff9872` | `v0.16.0-rc1` |
| **1** | Broker `state_by_project` cache via `StateStore` trait (call-site migration) | ☑ DONE | `7666439` | `v0.16.0-phase1` |
| **2** | pgvector `VectorBackend` impl + `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` env var parser + startup validator | ☐ Next | — | — |
| **3** | `PostgresStateStore` impl with FTS5 → `tsvector` translation | ☐ | — | — |
| **4** | Combined Postgres+pgvector single-pool backend | ☐ | — | — |
| **5** | Redis `StateStore` with cache/persistent durability modes + `require_aof` enforcement | ☐ | — | — |
| **6** | Combined Redis+RediSearch single-client backend | ☐ | — | — |
| **7** | Redis-Vector entity/relation/image parity + v0.16.0 release | ☐ | — | — |

### Backend selection scheme (activated at Phase 2)

Four env vars, two per axis, parallel naming:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string, may carry credentials>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string, may carry credentials>
```

- All four unset → SQLite + Qdrant default (the 95% deployment).
- Any asymmetric specification (one TYPE without the other, type without URL, unknown type) → fail loud at startup with `InvalidProjectConfig` and the exact offending value named in the message.
- `postgres-combined` / `redis-combined` are explicit TYPE values, not URL-equality inference; both axes must match and URLs must be byte-identical when either is combined.
- Tuning knobs (HNSW parameters, schema names, Redis prefixes, AOF verification) live in `config.toml`, NOT in env vars. Secrets stay in env vars.
- `{project_id}` substitution in config.toml is literal `.replace` only — no Jinja, no escape hatches.

Full rationale and the per-rule test matrix in `docs/plans/2026-04-11-resume-phase1-onwards.md § Backend selection scheme`.

### Phase 2 resume prompt

A self-contained resume prompt for a fresh session to pick up Phase 2
lives at `docs/plans/2026-04-11-resume-phase2-prompt.md` — 376 lines,
enumerates all 10 deliverables (workspace deps, `pg-vectors` feature,
`pg_vector.rs` module, `BackendSelection` parser, config.toml section,
broker factory branch, integration + 8 negative validator tests, bench
extension, docs work, release gate) plus 6 explicit STOP conditions.

## What's Next

### Immediate (next session)

- **Phase 2 of v0.16.0** — pgvector `VectorBackend` + env var parser + startup validator. Resume prompt in `docs/plans/2026-04-11-resume-phase2-prompt.md`. Estimated ~800 LOC across new files + workspace sqlx/pgvector deps + config.toml `[vector.pgvector]` section + broker factory branch + 8 negative validator tests + 1 bench extension + 1 new guide section.

### Near-Term (Phases 3–6)

- Phase 3: `PostgresStateStore` impl (~1500 LOC — the FTS5 → `tsvector` translation is the bulk) with full 22-method `StateStore` trait coverage and a cross-backend regression test.
- Phase 4: Combined Postgres+pgvector (~300 LOC) — ONE `sqlx::PgPool` serving both trait roles for transactional consistency between state writes and vector writes.
- Phase 5: Redis `StateStore` with three modes (cache-only, persistent+AOF-required, combined-with-RediSearch). `require_aof` enforcement refuses to boot on Redis without AOF when `mode = "persistent"`.
- Phase 6: Combined Redis+RediSearch (~300 LOC) — ONE `fred::Client` serving both roles, with MULTI/EXEC transaction semantics (limits documented in the combined adapter).

### Medium-Term (Phase 7 / v0.16.0 GA)

- Phase 7: Redis-Vector entity/relation/image parity (~400 LOC) — close the v0.14.x silent-skip fallback for Redis entities. Benchmark every backend permutation. Write the v0.16.0 section of `production-hardening-v0.15.md` and the final multi-backend operations guide. Full release docs pass + v0.16.0 GA tag + deletion of the resume plan files.

### Deferred / Not on the v0.16.0 roadmap

- **Lever 2 async audit batching** — designed in parallel (`docs/plans/2026-04-10-audit-batching-lever2.md`), explicitly deferred to a post-v0.16.0 ticket. Triggered only if audit writes become a real bottleneck above the Lever 1 baseline (currently ~83 µs/row, 67× faster than v0.14.3).
- **Cross-backend migration tooling** — "dump from SQLite, load into Postgres" is out of scope for v0.16.0. Operators choose a backend at init time; switching later is manual re-ingestion.
- **Multi-broker HA** — the current broker design assumes exclusive access to its state store. HA across multiple brokers is a future feature (would need advisory locks, lease-based ownership, or a Postgres `SELECT ... FOR UPDATE SKIP LOCKED` queue pattern).

### Future (post-v0.16.0)

- Web marketplace for browsing, rating, and reviewing catalog tools
- Community-curated "markets" (collections by use case)
- Automated tool discovery from package registries
- Install analytics and usage tracking
