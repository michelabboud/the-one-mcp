# Upgrade Guide

> Breaking changes and migration notes for the-one-mcp version upgrades.

---

## Upgrading to v0.14.0 (from v0.13.x)

### New features (non-breaking)

- **Catalog expansion to 365 tools** — all 10 language files and all 8
  category files are now fully populated. +248 new entries from the baseline
  117. Covers Rust, Python, JS/TS, Go, Java, Kotlin, Ruby, PHP, Swift, C/C++
  plus cross-language categories: security, CI/CD, testing, databases, cloud,
  docs, monitoring, automation.
- Run `maintain (action: tool.refresh)` to import the new entries into your
  local `catalog.db` so `tool.find` can discover them.

### Required action

- **None.** The catalog JSON ships in the binary; `tool.refresh` imports it.

---

## Upgrading to v0.13.1 (from v0.13.0)

### New features (non-breaking)

- **Full LightRAG parity** — 6 features that were missing in v0.13.0 are now
  shipped: entity name normalization, entity/relation description vector store,
  description summarization, query keyword extraction, gleaning pass, and canvas
  force-directed graph visualization. See the updated [Graph RAG guide](graph-rag.md)
  for the full parity matrix.
- **New env vars** for the Graph RAG pipeline: `THE_ONE_GRAPH_GLEANING_ROUNDS`
  (default 1), `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` (default 2000),
  `THE_ONE_GRAPH_QUERY_EXTRACT` (default true).

### Required action

- **None.** All new features are disabled by default (behind `THE_ONE_GRAPH_ENABLED`).
  Existing graph data is forward-compatible.

### Optional actions

- **Re-run extraction** if you populated the graph in v0.13.0 — the new
  normalization + gleaning will produce a cleaner, more complete graph.
- **Check graph viz** at http://localhost:8788/graph after running extraction.
  The canvas renderer replaces the v0.13.0 placeholder.

### No breaking changes

- `MemoryEngine::search_graph` is now async but all callers are in async contexts
- `MemoryEngine` has a new `project_id` field (defaults to `None`, no external effect)

---

## Upgrading to v0.12.0 (from v0.10.x or earlier)

### New features (non-breaking)

- **Intel Mac `local-embeddings-dynamic` feature flag:** Intel Mac users can
  now get local embeddings by installing `libonnxruntime` via Homebrew and
  building with the new feature flag. Default Intel Mac binaries still ship
  lean — no behaviour change unless you opt in. See [INSTALL.md](../../INSTALL.md#intel-mac-local-embeddings-v0110).
- **Observability deep dive:** `observe: action: metrics` now returns 8
  additional counters plus a derived `memory_search_latency_avg_ms`. All
  new fields are `#[serde(default)]` so existing deserializers keep working.
  See the new [Observability Guide](observability.md).
- **Backup / restore via `maintain: backup` and `maintain: restore`:**
  gzipped tar of your project state, catalog, and registry. See the new
  [Backup & Restore Guide](backup-restore.md).

### Required action

- **None.** v0.12.0 adds features; nothing is removed or renamed.

### Optional actions

- **Run `observe: metrics` after a day of normal use** to see the new
  counters populate. Use the [Observability Guide](observability.md) to
  interpret the numbers.
- **Take your first backup:** ask your AI CLI _"Back up this project to
  ~/Desktop/my-project.tar.gz"_ to exercise the new `maintain: backup`
  flow.

### No breaking changes

- Tool count unchanged at 17
- Existing `maintain` actions unchanged; only two new ones added (`backup`, `restore`)
- Existing `MetricsSnapshotResponse` fields unchanged; new fields are additive

---

## Upgrading to v0.10.0 (from v0.9.x)

### New features (non-breaking)

- **MCP Resources API:** the `initialize` handshake now advertises a
  `resources` capability and new `resources/list` / `resources/read`
  JSON-RPC methods are available. See the new [MCP Resources Guide](mcp-resources.md).
- **`the-one://` URI scheme:** three resource types — `docs/<path>`,
  `project/profile`, `catalog/enabled`.
- **Catalog expansion:** 117 → 184 tools across 10 languages (added
  Kotlin, Ruby, PHP, Swift, C++ language files; expanded Python and
  JavaScript).
- **Landing page scaffold** under `docs-site/` — ready for GitHub Pages
  enablement.

### Required action

- **None.** MCP clients that don't know about resources simply ignore the
  capability flag in `initialize`.

### Optional actions

- **Claude Code users:** your client will automatically pick up the new
  resources and surface indexed docs in its `@`-picker.
- **To enable GitHub Pages for the landing page:** go to Settings → Pages,
  set source to `main` branch / `/docs-site` folder. See
  `docs-site/README.md`.

### No breaking changes

- Tool count unchanged at 17
- Resources are a separate JSON-RPC surface and do not affect `tools/*`
- Config schema unchanged

---

## Upgrading to v0.9.0 (from v0.8.x)

### New features (non-breaking)

- **Tree-sitter AST chunker:** the 5 original languages (Rust, Python,
  TypeScript/TSX, JavaScript, Go) now use tree-sitter as their primary
  chunker with transparent regex fallback on parse failure. 8 new
  languages added: C, C++, Java, Kotlin, PHP, Ruby, Swift, Zig. See the
  updated [Code Chunking Guide](code-chunking.md).
- **Retrieval benchmark suite:** new `retrieval_bench.rs` example
  measures 4 retrieval configurations against 3 query sets. Not part of
  CI — run manually against a local Qdrant. See `benchmarks/README.md`.
- **New feature flag `tree-sitter-chunker`** (default on). Lean builds
  can disable for a smaller binary at the cost of only getting the
  v0.8.0 regex chunkers for 5 languages.

### Required action

- **None.** All changes are additive. The dispatcher transparently falls
  back to regex on tree-sitter parse failure for the 5 original languages.

### Optional actions

- **Re-index code-heavy projects** with `setup: action: refresh` to get
  tree-sitter-parsed chunks with richer symbol metadata.

### No breaking changes

- Tool count unchanged at 17
- Config schema unchanged
- Existing ChunkMeta consumers unaffected

---

## Upgrading to v0.8.2 (from v0.8.0 / v0.8.1)

### New features (non-breaking)

- **Image auto-reindex:** the file watcher now re-ingests changed image
  files (PNG/JPG/JPEG/WebP) into the Qdrant image collection in addition
  to markdown files. No config change needed — if you had
  `auto_index_enabled: true` and `image_embedding_enabled: true`, image
  changes are now picked up automatically.

### Required action

- **None.**

### No breaking changes

- API surface unchanged
- Config schema unchanged

---

## Upgrading to v0.8.0 (from v0.7.x)

### New features (non-breaking)

- **Watcher auto-reindex:** if you had `auto_index_enabled: true` in v0.7.x, it will now actually re-ingest changed `.md` files instead of just logging. No action needed.
- **Code-aware chunker:** automatic — code files (`.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.go`) are now chunked by function/class/struct instead of as plain text. Takes effect on the next `setup:refresh`.
- **Extended ChunkMeta:** search results now carry `language`, `symbol`, `signature`, `line_range` fields when chunks came from code files. Older consumers ignore these fields.

### Re-index recommended (not required)

If your project has code files that were previously indexed, running `setup:refresh` will re-chunk them with the new language-aware chunker. This substantially improves retrieval quality for function/class/struct searches.

```bash
# Via MCP (from the LLM):
# Call `setup` tool with action: "refresh"
```

### No breaking changes

- Tool count unchanged at 17
- Config schema unchanged
- Existing ChunkMeta consumers unaffected (new fields are Option with None default for markdown chunks)

---

## Upgrading to v0.6.0 (from v0.5.0)

### New Features (no migration needed)

All new features in v0.6.0 are **opt-in** or **additive**. Existing deployments continue to work without any config changes.

- **Image search** — semantic search over PNG/JPG/WebP files (`memory.search_images`, `memory.ingest_image`). Off by default.
- **Cross-encoder reranking** — 15–30% precision improvement for `memory.search`. Off by default.
- **6 additional text embedding models** — all available immediately via `config:models.list`. No action needed.

---

### Tool count: 15 → 17

Two tools were added in v0.6.0. No tools were removed or renamed.

| New Tool | Description |
|---|---|
| `memory.search_images` | Semantic search over indexed images |
| `memory.ingest_image` | Manually index a single image file |

AI CLI sessions discover tools at startup via `tools/list`. They will see the two new tools automatically on reconnect — no registration step needed.

---

### New config fields (all optional)

All new fields have defaults that preserve v0.5.0 behavior. You do not need to add any of them unless you want to opt in.

| Field | Default | Description |
|---|---|---|
| `image_embedding_enabled` | `false` | Enable image search feature |
| `image_embedding_model` | `"nomic-vision"` | Image embedding model |
| `image_ocr_enabled` | `false` | Enable OCR text extraction from images |
| `image_ocr_language` | `"eng"` | Tesseract language code |
| `image_thumbnail_enabled` | `true` | Generate thumbnails for indexed images |
| `image_thumbnail_max_px` | `512` | Max thumbnail dimension in pixels |
| `reranker_enabled` | `false` | Enable cross-encoder reranking |
| `reranker_model` | `"jina-reranker-v2-base-multilingual"` | Reranker model name |
| `limits.image_search_score_threshold` | `0.25` | Minimum score for image search results |
| `limits.max_image_size_bytes` | `10485760` | Maximum image file size (10MB) |

---

### fastembed 4 → 5.13 (internal upgrade)

The underlying embedding library was upgraded from fastembed 4 to 5.13. This is transparent — the model cache format is compatible and no re-download is required for existing text models.

**Exception:** If you were using the Jina reranker in a pre-release v0.6.0 build, the reranker model may need to be re-downloaded. Delete the cached file and let it re-download on next use:

```bash
rm -rf .fastembed_cache/jina-reranker-v2*
```

---

### Feature flags

Two new Cargo feature flags were introduced:

| Flag | Default (full build) | Default (lean build) | What it enables |
|---|---|---|---|
| `image-embeddings` | on | off | Image ingest, `memory.search_images`, `memory.ingest_image`, Qdrant image collection |
| `image-ocr` | off | off | OCR extraction via tesseract (opt-in everywhere, requires tesseract installed) |

The pre-built release binary always includes `image-embeddings`. Custom lean builds (`bash scripts/build.sh build --lean`) exclude it. If you call `memory.search_images` on a lean binary, you get `CoreError::NotEnabled`.

---

### Verifying the upgrade

```bash
# Confirm binary version:
the-one-mcp --version
# Expected: the-one-mcp 0.6.0

# Confirm tool count:
# In an MCP session, tools/list should return 17 tools.

# Confirm image search (if enabled):
maintain { "action": "images.rescan", "params": { "project_root": "...", "project_id": "..." } }

# Confirm reranker (if enabled):
memory.search { "query": "test query", "project_root": "...", "project_id": "...", "top_k": 5 }
# With RUST_LOG=debug, you should see "reranking N candidates"
```

---

## Upgrading to v0.5.0 (from v0.4.0) — BREAKING

### Tool Consolidation: 33 → 15

v0.5.0 is a **breaking change** to the MCP tool surface. The 33-tool surface of v0.4.0 was consolidated into 15 multiplexed tools (later expanded to 17 in v0.6.0).

**Why:** With 33 tools, the `tools/list` response consumed ~3,500 tokens at session init. The consolidated surface uses ~1,750 tokens — a 50% reduction. This matters because AI CLIs reload the tool list on every new session; smaller lists leave more context budget for actual work.

**Impact:**

- AI CLI sessions discover tools at startup — they will see the new tool names automatically on reconnect. No manual intervention.
- **Custom scripts or automation** that call old tool names by string (e.g., `project.init`, `docs.reindex`) will break and must be updated.
- MCP clients that cached tool schemas locally may need a cache clear.

---

### Tool migration table

| Old Tool (v0.4.0) | New Equivalent (v0.5.0+) |
|---|---|
| `project.init` | `setup` with `action: "project"` |
| `project.refresh` | `setup` with `action: "refresh"` |
| `project.profile.get` | `setup` with `action: "profile"` |
| `docs.get_section` | `docs.get` with `section` parameter |
| `docs.create` | `docs.save` (upsert — creates if missing) |
| `docs.update` | `docs.save` (upsert — updates if exists) |
| `docs.reindex` | `maintain` with `action: "reindex"` |
| `docs.trash.list` | `maintain` with `action: "trash.list"` |
| `docs.trash.restore` | `maintain` with `action: "trash.restore"` |
| `docs.trash.empty` | `maintain` with `action: "trash.empty"` |
| `tool.list` | `tool.find` with `mode: "list"` |
| `tool.suggest` | `tool.find` with `mode: "suggest"` |
| `tool.search` | `tool.find` with `mode: "search"` |
| `tool.add` | `config` with `action: "tool.add"` |
| `tool.remove` | `config` with `action: "tool.remove"` |
| `tool.enable` | `maintain` with `action: "tool.enable"` |
| `tool.disable` | `maintain` with `action: "tool.disable"` |
| `tool.update` | `maintain` with `action: "tool.refresh"` |
| `config.export` | `config` with `action: "export"` |
| `config.update` | `config` with `action: "update"` |
| `metrics.snapshot` | `observe` with `action: "metrics"` |
| `audit.events` | `observe` with `action: "events"` |
| `models.list` | `config` with `action: "models.list"` |
| `models.check_updates` | `config` with `action: "models.check"` |

---

### Before/after call examples

**Project initialization:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "project.init",
  "arguments": { "project_root": "/my/project", "project_id": "myproj" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "setup",
  "arguments": { "action": "project",
    "params": { "project_root": "/my/project", "project_id": "myproj" } } } }
```

**Re-indexing:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "docs.reindex",
  "arguments": { "project_root": "/my/project", "project_id": "myproj" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "maintain",
  "arguments": { "action": "reindex",
    "params": { "project_root": "/my/project", "project_id": "myproj" } } } }
```

**Exporting config:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "config.export",
  "arguments": { "project_root": "/my/project" } } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "config",
  "arguments": { "action": "export",
    "params": { "project_root": "/my/project" } } } }
```

**Listing metrics:**

```jsonc
// v0.4.0
{ "method": "tools/call", "params": { "name": "metrics.snapshot", "arguments": {} } }

// v0.5.0+
{ "method": "tools/call", "params": { "name": "observe",
  "arguments": { "action": "metrics", "params": {} } } }
```

---

### Upgrade steps for v0.5.0

1. Back up `~/.the-one/`
2. Install the new binary:
   ```bash
   bash scripts/install.sh
   ```
3. Restart all AI CLI sessions (they re-discover tools on init)
4. Update any custom scripts using old tool names (see table above)
5. Verify tool count:
   ```
   tools/list → 15 tools (v0.5.0) or 17 tools (v0.6.0)
   ```

---

## Upgrading to v0.4.0 (from v0.3.0)

### New features (non-breaking)

- **TOML embedding model registry** — model definitions moved from hardcoded Rust to `models/local-models.toml` and `models/api-models.toml`. No config changes needed.
- **Interactive installer model selection** — `install.sh` now prompts for embedding tier. Existing installs keep their current model.
- **Two new tools:** `models.list` and `models.check_updates` (later consolidated in v0.5.0)
- **Default embedding model changed:** `all-MiniLM-L6-v2` (384 dims) → `BGE-large-en-v1.5` (1024 dims)

---

### Re-indexing required after default model change

If you are upgrading from v0.3.0 and keeping the default embedding model, your stored vectors are 384-dimensional but the new model produces 1024-dimensional vectors. This mismatch causes Qdrant errors on search.

**Fix:**

1. Drop the old Qdrant collection (via the Qdrant dashboard or API):
   ```
   DELETE /collections/the_one_docs
   ```

2. Re-index your project:
   ```bash
   # In an MCP session:
   maintain { "action": "reindex", "params": { "project_root": "...", "project_id": "..." } }
   ```

**Alternative:** Keep the old model explicitly to avoid re-indexing:

```json
// ~/.the-one/config.json
{
  "embedding_model": "all-MiniLM-L6-v2"
}
```

This preserves your existing index without any re-indexing.

---

## Upgrading to v0.3.0 (from v0.2.x)

### Tool catalog system added

v0.3.0 introduced the tool catalog: a SQLite database (`~/.the-one/catalog.db`) with FTS5 full-text search, populated from `tools/catalog/*.json`. Seven new tools were added:

- `tool.list`, `tool.suggest`, `tool.search`
- `tool.add`, `tool.remove`
- `tool.enable`, `tool.disable`

**Note:** All of these were later consolidated into `tool.find`, `config`, and `maintain` in v0.5.0.

### First-run behavior

On first `project.init` after upgrading to v0.3.0, the broker:

1. Imports the tool catalog from `tools/catalog/*.json`
2. Scans the system with `which` for installed tools
3. Populates `enabled_tools` in the catalog

This adds 5–10 seconds to the first init. Subsequent inits are fast.

### No breaking changes from v0.2.x

All existing tools from v0.2.x (`memory.search`, `docs.save`, etc.) continue to work unchanged in v0.3.0.

---

## General Upgrade Checklist

Follow these steps for any version upgrade:

1. **Back up your data directory:**
   ```bash
   cp -r ~/.the-one ~/.the-one.bak-$(date +%Y%m%d)
   ```

2. **Read the CHANGELOG** for breaking changes specific to your version pair.

3. **Install the new binary:**
   ```bash
   bash scripts/install.sh
   ```
   The installer updates the binary, refreshes the tool catalog, and re-registers with discovered AI CLIs.

4. **Restart all MCP clients** — Claude Code, Gemini CLI, OpenCode, Codex. MCP servers start at session init; a live session won't pick up the new binary.

5. **Run a smoke test:**
   ```bash
   # Confirm version:
   the-one-mcp --version

   # In a new AI session, call setup:
   setup { "action": "profile", "params": { "project_root": "...", "project_id": "..." } }

   # Confirm search works:
   memory.search { "query": "test", "project_root": "...", "project_id": "...", "top_k": 3 }
   ```

6. **Check tool count matches the expected version:**

   | Version | Tool count |
   |---|---|
   | v0.8.0 | 17 |
   | v0.7.0 | 17 |
   | v0.6.0 | 17 |
   | v0.5.0 | 15 |
   | v0.4.0 | 33 |
   | v0.3.0 | 30 |

7. **Re-index if you changed the embedding model** (see the v0.4.0 section above for details).

---

## Getting Help

If you encounter an issue not covered here:

- Check the [Troubleshooting guide](./troubleshooting.md) for common error patterns
- Run `RUST_LOG=debug the-one-mcp serve` and capture the output
- Open an issue at [github.com/michelabboud/the-one-mcp](https://github.com/michelabboud/the-one-mcp/issues) with your version, OS, and the debug log
