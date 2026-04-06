# Changelog

All notable changes to this project are documented in this file.

## [0.14.1] - 2026-04-06

### Documentation

- All docs refreshed for v0.14.0 catalog expansion (184→365 tools):
  README stats, CLAUDE.md counts, PROGRESS.md version + release entry,
  tool-catalog.md per-file counts, upgrade-guide.md v0.14.0 section,
  landing page tool count, CHANGELOG v0.14.0 entry.

## [0.14.0] - 2026-04-06

### Added

- **Catalog expansion to 365 tools** (+248 new entries from baseline 117).
  Closes the deferred Task 5 from the 9-item roadmap (Phase 2, Task 2.2).
  Every language file and every category file is now populated with curated,
  schema-validated entries. See the v0.14.0 commit message for per-file
  breakdown.

## [0.13.1] - 2026-04-06

Full LightRAG parity — all six features from the v0.13.0 comparison matrix that were marked ❌ are now ✅.

### Added

1. **Entity name normalization** — `normalize_entity_name()` in `graph.rs`: trim, collapse whitespace, strip surrounding punctuation, preserve acronyms (all-uppercase like `API`, `HTTP`), title-case everything else. Applied in `merge_extraction` + new `ExtractionResult::merge` for full dedup across passes. +6 unit tests.
2. **Entity + relation description vector store** — 6 new Qdrant methods (`create/upsert/search` for both entities and relations). `EntityPoint`, `RelationPoint`, `EntitySearchResult`, `RelationSearchResult` types. `MemoryEngine` gains `upsert_entity_vectors` / `upsert_relation_vectors` / `search_entities_semantic` / `search_relations_semantic`. Broker's `graph_extract()` now upserts all entities + relations into Qdrant after extraction.
3. **Description summarization** — `summarize_description()` in `graph_extractor.rs`. After the per-chunk extraction loop, entities whose descriptions exceed `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` (default 2000 chars) get map-reduced via a single LLM summarization call.
4. **Query keyword extraction** — `extract_query_keywords()` in `graph_extractor.rs`. Splits user queries into `high_level` (themes for Global mode) and `low_level` (identifiers for Local mode) via an LLM call. `search_graph()` upgraded from sync to async, now routes through the new Qdrant entity/relation collections when available. Graceful fallback to in-memory keyword search when disabled/offline. Enabled by default when `THE_ONE_GRAPH_ENABLED=true` (opt out via `THE_ONE_GRAPH_QUERY_EXTRACT=false`).
5. **Gleaning / continue-extraction pass** — `extract_with_gleaning()` wraps each chunk's extraction with up to `THE_ONE_GRAPH_GLEANING_ROUNDS` (default 1) follow-up "what did you miss?" prompts. Early-terminates when a round returns empty. `ExtractionResult::merge()` deduplicates entities/relations across passes using normalized names.
6. **Canvas force-directed graph visualization** — `/graph` page now renders a self-contained force-directed layout in ~80 lines of vanilla JS + `<canvas>`. Fetches `/api/graph`, runs 200 force simulation ticks, renders nodes colored by entity type + edges + labels (when < 80 nodes). Click to animate. Zero external deps, works offline.

### Infrastructure changes

- `MemoryEngine` gains `project_id: Option<String>` field + `set_project_id()` setter for scoping Qdrant entity/relation collections.
- `search_graph()` is now `async` (3 call sites updated to `.await`).
- `KnowledgeGraph` gains `all_entities()`, `all_relations()`, and `get_entity_mut()` public accessors.
- Dashboard test assertion updated for v2 heading change.

### New env vars (v0.13.1)

| Var | Default | Purpose |
|-----|---------|---------|
| `THE_ONE_GRAPH_GLEANING_ROUNDS` | `1` | Extra extraction passes per chunk |
| `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` | `2000` | Description char length triggering LLM summarization |
| `THE_ONE_GRAPH_QUERY_EXTRACT` | `true` | Enable query keyword extraction for Local/Global modes |

### Tests

- +6 entity name normalization tests (title-case, acronyms, multi-word, punctuation, empty, dedup roundtrip)
- Workspace total: **308 tests**, 0 failures on default + lean matrices

## [0.13.0] - 2026-04-06

Major UI overhaul + Graph RAG end-to-end wiring, based on research into
[HKU's LightRAG](https://github.com/hkuds/lightrag) for the retrieval-quality
pieces we were missing.

### Added

#### Admin UI — multi-project home + new pages + v2 dashboard

- **Landing page at `/`** — hero banner, feature summary, admin section links, GitHub / docs / issues links, install one-liner, responsive layout.
- **`/ingest` page** — 4-card form for markdown upload, image path ingest, code file chunking, and full reindex. Validates paths against `..` traversal, talks to new `/api/ingest/{markdown,image,code,reindex}` endpoints.
- **`/graph` page** — entity/relation explorer. Empty-state with setup CTA when graph is not yet populated. Stat grid + top-entity-types bar chart + query-modes reference table + placeholder for Sigma.js force-directed viz (v0.13.1).
- **Dashboard v2** — replaces the v0.12.x 4-card format. Includes a 6-stat grid (searches / tool runs / graph entities / watcher health / Qdrant errors / audit events), a LightRAG-inspired bar chart of tool-call distribution across 8 counters, runtime config table, embedding model card with async fetch, and a Graph RAG status table.
- **Top nav with project switcher** — `NAV_ITEMS` const drives a shared `render_nav(active, project_id, registry)` helper used by every page. Project switcher reads from `~/.the-one/projects.json` (new `ProjectRegistry` with `load/save/touch`). Live cross-project switching is documented as a v0.13.1 follow-up (the embedded UI is still scoped to one project per server instance).
- **Shared `render_page_shell(title, active, project, registry, body)`** — every new page uses it for a consistent header/nav/footer. Dark-mode-aware CSS variables in `shell_styles()` respect `prefers-color-scheme`. Mobile breakpoint at 720px. Sticky top nav, badge system (ok/warn/err/idle), bar-chart component, stat-grid, empty-state card.
- **New JSON APIs**:
  - `GET /api/projects` — list tracked projects with last-seen timestamps
  - `GET /api/models` — list local FastEmbed models + current active model
  - `GET /api/graph` — nodes + edges JSON for viz consumers
  - `POST /api/ingest/markdown|image|code|reindex` — ingest handlers
  - `POST /api/graph/extract` — triggers extraction

#### Graph RAG — end-to-end wiring (Tasks 12 + 9 from roadmap)

- **`crates/the-one-memory/src/graph_extractor.rs`** — new module implementing the LLM extraction pipeline. Takes indexed chunks, builds the extraction prompt via existing `graph::build_extraction_prompt`, calls an OpenAI-compatible `/v1/chat/completions` endpoint via reqwest, parses responses with `graph::parse_extraction_response`, merges into `KnowledgeGraph`, persists to `knowledge_graph.json`. Includes `GraphExtractResult` with chunks processed/skipped/errors for UI display.
- **Environment-driven config** — `THE_ONE_GRAPH_ENABLED`, `THE_ONE_GRAPH_BASE_URL`, `THE_ONE_GRAPH_MODEL`, `THE_ONE_GRAPH_API_KEY`, `THE_ONE_GRAPH_ENTITY_TYPES`, `THE_ONE_GRAPH_MAX_CHUNKS`. Works with Ollama, LM Studio, LiteLLM, LocalAI, vLLM, OpenAI proper. Disabled by default — returns `disabled_reason` in the response if not enabled rather than erroring.
- **`McpBroker::graph_extract(project_root, project_id)`** — public method that drains the project's chunks, calls the extractor, reloads the updated graph into the memory engine so `Local`/`Global`/`Hybrid` retrieval modes can see new entities immediately.
- **`McpBroker::graph_stats(project_root, project_id)`** — returns entity/relation counts + whether extraction is configured.
- **Two new `maintain` actions** — `graph.extract` and `graph.stats` exposed via JSON-RPC dispatch. See [Graph RAG guide](docs/guides/graph-rag.md) for full usage.
- **`MemoryEngine::chunks()` accessor** — read-only slice exposed so the extractor can iterate without borrowing the whole engine.

#### Documentation

- **New `docs/guides/graph-rag.md`** (~400 lines) — what Graph RAG is, current implementation state (shipped vs v0.13.1 vs v0.14.0), enablement walkthrough with Ollama / gpt-4o-mini examples, 4 retrieval modes explanation, storage model, prompt format, cost table, limitations, comparison matrix with LightRAG upstream, roadmap.

### Tests

- +2 `graph_extractor` tests (disabled-by-default behaviour, missing-base-url error)
- Workspace total: **302 tests**, 0 failures on default and lean matrices

### Dependencies

- `the-one-ui` now depends on `the-one-memory` (previously only on `the-one-core`) for the models_registry passthrough on the embedding model card

### Known follow-ups (v0.13.1 roadmap)

- Live cross-project switching via cookie/header (currently requires server restart with new `THE_ONE_PROJECT_ID`)
- Sigma.js force-directed graph visualization on `/graph` (placeholder renders today)
- Graph extraction config fields in `config.json` instead of env vars, with matching UI selector on `/config` page
- Entity name normalization + description summarization (LightRAG parity)
- Entity-description vector store for proper `Local` mode (currently uses keyword match)
- Config page embedding model dropdown (endpoint exists, page edit deferred)

## [0.12.1] - 2026-04-06

### Documentation

- **Three new guides** for Phase 2 / Phase 3 features:
  - `docs/guides/mcp-resources.md` — full coverage of the `the-one://` URI scheme, `resources/list` / `resources/read` JSON-RPC, security model, client integration patterns, and future extensions.
  - `docs/guides/backup-restore.md` — when to back up, what's included/excluded, the `maintain: backup` + `maintain: restore` workflow, move-to-new-machine flow, safety properties, troubleshooting.
  - `docs/guides/observability.md` — the 15 metrics counters (7 existing + 8 v0.12.0 additions), debugging playbooks for slow search / watcher health / Qdrant errors, audit events vs counters, Prometheus export notes.

- **Root docs refreshed for v0.12.0**:
  - `README.md` — Key Features list updated (184 catalog tools, 13 chunker languages, MCP resources, backup/restore, observability); architecture diagram refreshed; documentation index expanded; Stats table bumped (17 tools, 3 resource types, 300 tests, ~24,000 LOC, 184 catalog tools).
  - `CLAUDE.md` — landmark bullets updated to mention tree-sitter chunker feature flag, MCP resources module, backup module, Arc<BrokerMetrics>, retrieval benchmark example, Intel Mac `local-embeddings-dynamic`.
  - `PROGRESS.md` — stats table and current version bumped in v0.12.0 commit (no changes in v0.12.1).

- **Guide updates for v0.10.0/v0.12.0**:
  - `docs/guides/api-reference.md` — new "MCP Resources" section with URI scheme, `resources/list` / `resources/read` schema, initialize handshake capability. New `maintain: backup` and `maintain: restore` documentation with parameter tables and response shapes. New `observe: metrics` v0.12.0 field documentation.
  - `docs/guides/tool-catalog.md` — v0.10.0 expansion note (28 → 184 tools), per-file counts in the layout diagram, new language files called out.
  - `docs/guides/upgrade-guide.md` — new sections for v0.8.2, v0.9.0, v0.10.0, v0.12.0 migration notes. Each section covers new features, required actions (always "none"), optional actions, and no-breaking-changes confirmation.
  - `docs/guides/troubleshooting.md` — new "Backup & Restore Issues" section (7 symptoms) and new "Observability & Metrics Debugging" section (6 symptoms) with cross-links to the dedicated guides.

### Dependencies

- No changes (docs-only release).

### No code changes

This is a patch release for docs only. All 300 tests still pass, no behaviour changes.

## [0.12.0] - 2026-04-06

Phase 3 of the v0.8.2 → v0.12.0 roadmap: Intel Mac prep, observability deep dive, and backup / restore. All three tasks bundled into one release because the code paths are orthogonal but small individually.

### Task 3.1 — Intel Mac `local-embeddings-dynamic` feature flag

- **New feature flag `local-embeddings-dynamic`** — enables FastEmbed-based local embeddings on platforms where the prebuilt ONNX Runtime binaries are unavailable, most notably **Intel Mac** (`x86_64-apple-darwin`). When enabled, the binary links against a runtime-loaded `libonnxruntime.dylib` / `.so` / `.dll` instead of bundling C++ libraries at build time.

  Intel Mac users can now get local embeddings with:

  ```bash
  brew install onnxruntime
  cargo build --release -p the-one-mcp \
      --no-default-features \
      --features "embed-swagger,local-embeddings-dynamic"
  ```

- **Workspace + per-crate feature wiring** — `the-one-memory`, `the-one-mcp`, and `the-one-ui` all expose `local-embeddings-dynamic` as a passthrough feature.
- **INSTALL.md** — new "Intel Mac local embeddings (v0.11.0)" section (retained header; applies as of this v0.12.0 release).
- `fastembed` workspace dep now declares `default-features = false` so feature selection propagates cleanly through both `local-embeddings` and `local-embeddings-dynamic` bundles.

_Not shipping:_ CI matrix Intel Mac job still ships lean by default. Pure-Rust tract backend is not included because fastembed 5.13 does not expose one — upstream support would unblock that cleanly.

### Task 3.2 — Observability deep dive

- **`BrokerMetrics` extended with 8 new counters** for the v0.12.0 snapshot:
  `memory_search_latency_ms_total`, `image_search_calls`, `image_ingest_calls`, `resources_list_calls`, `resources_read_calls`, `watcher_events_processed`, `watcher_events_failed`, `qdrant_errors`.
- **`MetricsSnapshotResponse` extended** with the eight new fields plus a derived `memory_search_latency_avg_ms`. All new fields are `#[serde(default)]` for forward/backward compatibility.
- **`BrokerMetrics` now held as `Arc<BrokerMetrics>`** so the watcher task can clone it and increment watcher event counters from outside the broker's own methods.
- **Wired increments** in `memory_search` (with latency timing), `image_search`, `image_ingest`, `resources_list`, `resources_read`, and the watcher task.

### Task 3.3 — Backup / restore via `maintain: backup`

- **New `crates/the-one-mcp/src/backup.rs` module** implementing gzipped tar backup + restore of project state.
- **Two new `maintain` actions:** `backup` (takes `project_root`, `project_id`, `output_path`, optional `include_images`) and `restore` (takes `backup_path`, `target_project_root`, `target_project_id`, optional `overwrite_existing`).
- **What gets backed up:** the full `<project>/.the-one/` tree, `~/.the-one/catalog.db`, and `~/.the-one/registry/`.
- **What is excluded:** `.fastembed_cache/` (models re-download on first use), Qdrant wal/raft state (too large), `.DS_Store`.
- **Security:** unsafe archive paths (absolute, `..`, NUL, etc.) are rejected at restore time before any write. Restore refuses to overwrite existing project state unless `overwrite_existing: true`.
- **Manifest:** every backup embeds `backup-manifest.json` at the archive root with version, the-one-mcp version, timestamp, file count, and include/exclude lists. Restore validates the manifest version before unpacking.
- New API types: `BackupRequest`, `BackupResponse`, `RestoreRequest`, `RestoreResponse`.

### Tests

- +4 backup tests: roundtrip (backup → restore → verify content), fastembed_cache exclusion, refuse-without-overwrite, unknown-entry warning handling. Isolated via a `HomeGuard` helper that swaps `$HOME` during the test to avoid clobbering real user state.
- Workspace total: 296 → **300 tests**, all green on default and lean matrices.

### Dependencies

- New: `tar 0.4`, `flate2 1` — pulled into `the-one-mcp` for the backup module. Pure-Rust, widely used, no C deps.

## [0.10.0] - 2026-04-06

### Added

- **MCP Resources API** — first-class implementation of the MCP `resources/list` and `resources/read` primitives alongside the existing `tools/*`. The `initialize` handshake now advertises the `resources` capability (subscribe=false, listChanged=false), so compliant MCP clients like Claude Code can browse and reference indexed project content as native resources.
- **`the-one://` URI scheme** for resource addressing. Current resource types: `docs/<relative-path>` (managed markdown under `.the-one/docs/`), `project/profile` (project metadata JSON), and `catalog/enabled` (enabled tools per client). Path traversal is explicitly rejected — `the-one://docs/../../etc/passwd` returns an InvalidRequest error.
- **`crates/the-one-mcp/src/resources.rs`** — new module with `parse_uri`, `is_safe_doc_identifier`, `list_resources`, and `read_resource` helpers. Thirteen unit + dispatch tests cover URI parsing, directory walking, path traversal rejection, and empty-project defaults.
- **Catalog expansion (117 → 184 tools, +67)**. New per-language files: `kotlin.json` (7 tools), `ruby.json` (8), `php.json` (7), `swift.json` (5). Existing files grown: `python.json` (23 → 40), `javascript.json` (24 → 38), `cpp.json` (0 → 9). All entries schema-valid against `tools/catalog/_schema.json`.
- **Landing page** at `docs-site/` — single-page static HTML + CSS (zero frameworks, zero build step) ready to ship via GitHub Pages. See `docs-site/README.md` for one-time Pages enablement instructions.

### Changed

- **`initialize` response** now includes `"resources": { "subscribe": false, "listChanged": false }` in the capabilities object alongside `"tools": {}`.
- **`McpBroker`** gains two new methods: `resources_list(project_root, project_id)` and `resources_read(project_root, project_id, uri)` — both delegate to the new `crate::resources` module.

### Tests

- +13 tests: 10 for `resources` module (URI parsing, path traversal, dispatcher defaults, doc reading, catalog/profile reads), 3 for JSON-RPC dispatch (`resources/list`, missing params, path traversal rejection through the transport layer).
- Workspace total: 283 → 296 tests. Default and lean matrices both green.

### Not in this release (deferred follow-ups for Phase 3 or later)

- `resources/subscribe` and `notifications/resources/list_changed` — subscribe capability is advertised as `false` in v0.10.0.
- `catalog/enabled` currently returns an empty array; wiring it to the SQLite `enabled_tools` table is planned for a follow-up patch.
- Full catalog target was ~200 new tools; this release ships 67 curated, schema-valid entries. The remaining Go/Java/Kotlin/Ruby/PHP/Swift depth will land in follow-up patches as the ecosystem research continues.
- Landing page demo GIF and catalog browser widget are documented as future enhancements in `docs-site/README.md`.

### Dependencies

- No new crate dependencies in this release.

## [0.9.0] - 2026-04-05

### Added
- **Tree-sitter AST chunker** — language-aware code chunking upgraded from regex to tree-sitter for the original 5 languages (Rust, Python, TypeScript/TSX, JavaScript, Go) and extended to 8 new languages: **C, C++, Java, Kotlin, PHP, Ruby, Swift, Zig**. Each language gets its own tree-sitter grammar crate and a shared walker (`chunker_ts_impl::chunk_with_tree_sitter`) that emits one `ChunkMeta` per top-level AST node.
- **Regex fallback on parse failure** — the dispatcher in `chunker::chunk_file` tries tree-sitter first for the original 5 languages and transparently falls back to the v0.8.0 regex chunkers if tree-sitter cannot parse the input. Lean builds (`--no-default-features`) get the regex chunkers directly with no tree-sitter dependency.
- **New feature flag `tree-sitter-chunker`** — default on. Users who want the leanest possible binary can disable it to strip ~3-5 MB of grammar code (each grammar ships as a compiled C library via its tree-sitter-language binding).
- **Retrieval benchmark suite** — new `crates/the-one-memory/examples/retrieval_bench.rs` runs 4 retrieval configurations (dense only, dense + rerank, hybrid, full pipeline) against 3 query sets (exact match, semantic, mixed) and reports Recall@1, Recall@5, MRR, and p50/p95 latency. Query corpora are hand-curated against the-one-mcp's own source tree. Benchmarks are NOT in CI (they require a running Qdrant) — run manually with `cargo run --release --example retrieval_bench -p the-one-memory --features tree-sitter-chunker`. See `benchmarks/README.md` for prerequisites and `benchmarks/results.md` for published numbers.

### Changed
- `chunker::chunk_file` dispatcher now routes to tree-sitter backed chunkers when `tree-sitter-chunker` feature is enabled, with language-specific cfg gates so lean builds compile cleanly.
- `the-one-memory` now depends on the `tree-sitter` crate (0.26) plus 14 grammar crates (Rust 0.24, Python 0.25, JS 0.25, TS 0.23, Go 0.25, Swift 0.7, Ruby 0.23, C 0.24, C++ 0.23, Java 0.23, Kotlin-ng 1.1, Zig 1.1, PHP 0.24). All pinned via workspace dependencies.

### Tests
- +11 chunker tests covering the 8 new languages (C, C++, Java, Kotlin, PHP, Ruby, Swift, Zig) plus tree-sitter/regex parity checks for Rust and line_range metadata validation. Total workspace tests: **283** (272 → 283), 0 failures.

### Dependencies
- Added: `tree-sitter`, `tree-sitter-{rust,python,javascript,typescript,go,swift,ruby,c,cpp,java,kotlin-ng,zig,php}`

## [0.8.2] - 2026-04-05

### Added
- **Image auto-reindex** — the file watcher now re-ingests changed image files (PNG/JPG/JPEG/WebP) into the Qdrant image collection, completing the watcher auto-reindex feature that landed for markdown in v0.8.0. Upserted images go through the full pipeline (embed → optional OCR → optional thumbnail → Qdrant upsert); removed images are deleted from the image collection by source path.
- **Broker standalone helpers** — `image_ingest_standalone` and `image_remove_standalone` free functions in `broker.rs`. These extract the image ingest/remove pipeline from `McpBroker` methods so they can be called from the watcher's spawned tokio task without needing `&self`. The existing `McpBroker::image_ingest` / `McpBroker::image_delete` methods now delegate to these helpers.

### Fixed
- **Watcher routing** — markdown and image events are no longer processed under the same `memory_by_project` write lock. Image events reload config per-event so the watcher picks up live config edits (e.g., toggling `image_embedding_enabled`).

### Tests
- +2 unit tests for `image_ingest_standalone` (NotEnabled guard, missing-path guard)
- +1 `#[ignore]` integration test for the watcher image upsert path

### Dependencies
- No changes

## [0.8.1] - 2026-04-05

### Changed
- Documentation refresh: audited all guides and root docs for v0.8.0 accuracy. Added v0.7→v0.8 migration section to upgrade-guide. Updated stale test counts, version references, and feature mentions across guides. Added code-aware chunker mentions to complete-guide and architecture docs.

### Dependencies
- No changes (docs-only release)

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
