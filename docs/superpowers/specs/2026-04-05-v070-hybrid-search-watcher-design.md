# v0.7.0 Design: Hybrid Search + File Watcher + Admin UI Gallery + Screenshot Search

**Date:** 2026-04-05
**Target release:** v0.7.0
**Status:** Design — approved, proceeding to implementation

## Goal

Four features bundled into v0.7.0:

1. **Hybrid search (BM25 + dense)** — boost retrieval quality for exact-match queries (function names, error codes, API endpoints) without losing semantic recall
2. **File watcher for incremental indexing** — auto-reindex changed markdown and images in the background (opt-in)
3. **Image gallery in Admin UI** — `/images` route showing indexed thumbnails + captions + OCR
4. **Screenshot-based image search** — `memory.search_images` accepts either text or base64 image for image→image similarity

## Feature 1: Hybrid Search (BM25 + dense)

### Pipeline change

```
Before: query → dense embed → Qdrant dense search → top-k
After:  query → dense embed + BM25 tokenize
            → parallel Qdrant queries (dense + sparse)
            → merge by doc ID
            → final_score = 0.7 * dense_norm + 0.3 * sparse_norm
            → sort, top-k
            → (optional) rerank
```

### Model choice: BM25

- `fastembed::SparseTextEmbedding` variant `BM25` (classical algorithm, no model download)
- Tokenizer is deterministic; embeddings are computed on-the-fly
- Zero first-run latency, ~5MB binary footprint for tokenizer rules

### Score normalization

BM25 scores are unbounded (typical range 0-30+). To combine with cosine dense scores (0-1):

```rust
// Saturation function — bounds BM25 to [0, 1) asymptotically
fn bm25_normalize(score: f32) -> f32 {
    let k = 5.0;
    score / (score + k)
}
```

### Fusion formula

```rust
let final_score = config.hybrid_dense_weight * dense_score
                + config.hybrid_sparse_weight * bm25_normalize(sparse_score);
```

Defaults: `dense_weight = 0.7`, `sparse_weight = 0.3`. Both configurable.

### Storage

Qdrant supports sparse vectors alongside dense in the same collection via named vectors. Collection schema:

```
the_one_<project_id>:
  named_vectors:
    "dense": { size: 1024, distance: Cosine }
    "sparse": { modifier: Idf }  // sparse, uses IDF
```

On upsert, provide both dense and sparse vectors per point. On search, issue two queries or use Qdrant's query_batch API.

### Fallback behavior

- If `hybrid_search_enabled: false` → dense-only (current behavior)
- If Qdrant unavailable → keyword fallback (existing), hybrid is ignored
- If sparse embedding fails for some reason → log warning, dense-only

### New config

```json
{
  "hybrid_search_enabled": false,        // default off, opt-in
  "hybrid_dense_weight": 0.7,
  "hybrid_sparse_weight": 0.3,
  "sparse_model": "bm25"
}
```

### New MCP surface

None — extends `memory.search` transparently. LLMs see the same tool.

## Feature 2: File Watcher for Incremental Indexing

### Crate: `notify` + `notify-debouncer-mini`

Standard Rust file watcher. Cross-platform (inotify on Linux, kqueue on macOS, ReadDirectoryChangesW on Windows).

### Lifecycle

1. When broker creates a `MemoryEngine` with `auto_index_enabled: true`:
   - Spawn a tokio task that watches `<project>/.the-one/docs/` and `<project>/.the-one/images/`
   - Debounce window: 2 seconds (wait for editor to finish saving)
   - On event, queue the file path for re-indexing
2. Re-index worker pulls from queue, runs `docs.save` or `memory.ingest_image` internally
3. When broker drops the engine, the task is cancelled via a cancellation token

### Watched events

- `CREATE` / `WRITE` — re-index file
- `REMOVE` — remove from index (Qdrant + SQLite)
- `RENAME` — move in index (if we can detect both sides)

### Default: opt-in

```json
{
  "auto_index_enabled": false,
  "auto_index_debounce_ms": 2000,
  "auto_index_paths": ["docs", "images"]  // relative to .the-one/
}
```

### Concerns to handle

- **Rapid saves** → debounce handles this
- **Large binary files** → filter by extension (md for docs, png/jpg/webp for images)
- **Race with `setup:refresh`** → watcher pauses during explicit refresh, resumes after
- **Shutdown** — drop handle sends cancellation, watcher exits cleanly

### New MCP surface

None — works behind the scenes when enabled.

### New config

Listed above (`auto_index_enabled`, debounce, paths).

## Feature 3: Image Gallery in Admin UI

### Route: `/images`

New HTTP endpoint in `the-one-ui` crate. Server-side HTML (existing pattern).

### Layout

```
┌─ Admin UI ──────────────────────┐
│  Dashboard | Config | Audit | Images | Swagger │
├─────────────────────────────────┤
│  Project: demo                  │
│  42 indexed images              │
│                                 │
│  ┌──┐  ┌──┐  ┌──┐  ┌──┐       │
│  │📷│  │📷│  │📷│  │📷│   <-- 256px thumbnails
│  └──┘  └──┘  └──┘  └──┘       │
│  auth-flow.png                  │
│  "User→Auth→DB" [OCR: 3 words]  │
│                                 │
│  [search box]                   │
└─────────────────────────────────┘
```

### Data source

1. Read `managed_images` table from project SQLite → gets path, hash, caption, ocr_text, thumbnail_path
2. For each row, generate `<img>` tag pointing to thumbnail (served by another new route: `/images/thumbnail/<hash>`)
3. Display metadata: source path, caption, first 100 chars of OCR text

### Search box (stretch)

Uses the existing `memory.search_images` broker method to do live search from the UI. Probably out of scope for v0.7.0, defer to v0.7.1.

### New API endpoints in the-one-ui

- `GET /images` — HTML gallery page
- `GET /images/thumbnail/<hash>` — raw thumbnail bytes (from `.the-one/thumbnails/<hash>.webp`)
- `GET /api/images` — JSON list of indexed images (for potential JS frontend)

## Feature 4: Screenshot-Based Image Search

### API extension

`ImageSearchRequest` gains an optional `image_base64` field. Either `query` OR `image_base64` must be set, not both.

```rust
pub struct ImageSearchRequest {
    pub project_root: String,
    pub project_id: String,
    pub query: Option<String>,        // was required, now optional
    pub image_base64: Option<String>, // NEW
    pub top_k: usize,
}
```

### Dispatch logic

```rust
let query_vec = match (request.query, request.image_base64) {
    (Some(text), None) => {
        // Existing: embed text
        text_provider.embed_single(&text).await?
    }
    (None, Some(base64)) => {
        // NEW: decode, write to temp file, embed via image provider
        let bytes = base64_decode(&base64)?;
        validate_image_bytes(&bytes)?;
        let tmp = write_temp_image(&bytes)?;
        image_provider.embed_image(&tmp).await?
    }
    (Some(_), Some(_)) => return Err("provide exactly one of query or image_base64"),
    (None, None) => return Err("must provide either query or image_base64"),
};

// Then search the image Qdrant collection with query_vec
```

### Validation

- `image_base64` must decode successfully
- Decoded bytes must be a valid PNG/JPEG/WebP (use `image::guess_format`)
- Size must be under `max_image_size_bytes` (default 10MB)
- Max base64 string length ~14MB (10MB * 4/3 base64 overhead)

### Why it works

Both Nomic Vision and CLIP are **dual-encoder** models — text and image embeddings live in the same space. So:
- Text query → search returns similar images (existing)
- Image query → search returns similar images (NEW)
- Image query → search could also return similar text (future work, v0.7.1)

### No new tool

Extends `memory.search_images`. LLMs see an updated schema with both fields optional.

## Files to Create / Modify

### New files
- `crates/the-one-memory/src/sparse_embeddings.rs` — SparseEmbeddingProvider trait + FastEmbedSparseProvider (BM25)
- `crates/the-one-memory/src/watcher.rs` — notify integration, debounced tokio task
- `crates/the-one-ui/src/routes/images.rs` — /images gallery route + thumbnail server
- `docs/guides/hybrid-search.md` — user guide
- `docs/guides/auto-indexing.md` — file watcher user guide

### Modified files
- `Cargo.toml` — add `notify`, `notify-debouncer-mini`, `base64` (may already exist)
- `crates/the-one-memory/Cargo.toml` — features wiring
- `crates/the-one-memory/src/lib.rs` — MemoryEngine gains sparse_provider + watcher
- `crates/the-one-memory/src/qdrant.rs` — sparse vector upsert/search, named vectors schema
- `crates/the-one-memory/src/models_registry.rs` — optional SPLADE registry (future, not v0.7.0)
- `crates/the-one-core/src/config.rs` — hybrid_search_*, auto_index_*
- `crates/the-one-core/src/limits.rs` — sparse-related limits
- `crates/the-one-mcp/src/api.rs` — ImageSearchRequest.query becomes Option, add image_base64
- `crates/the-one-mcp/src/broker.rs` — image_search dispatches on query vs image_base64
- `crates/the-one-mcp/src/transport/tools.rs` — update memory.search_images schema
- `crates/the-one-mcp/src/transport/jsonrpc.rs` — dispatch update
- `crates/the-one-ui/src/lib.rs` — register /images route
- `schemas/mcp/v1beta/memory.search_images.request.schema.json` — update for optional query
- `VERSION`, `CHANGELOG.md`, `README.md`, `PROGRESS.md`, `CLAUDE.md`, `INSTALL.md`

### Unchanged (important)
- `.fastembed_cache/` directories in all crates — **DO NOT TOUCH, user explicitly requested**
- Existing embedding models, reranker logic, image embedding logic
- Tool count — stays at 17 (all extensions are to existing tools)

## Token Cost Impact

**None** — no new tools added. `memory.search_images` schema gains one optional field. Net token delta per session: ~0.

## Tests

- `sparse_embeddings.rs` — BM25 produces non-empty sparse vector, same text produces same vector
- `qdrant.rs` — named vector upsert + search roundtrip
- `lib.rs` — hybrid search path combines scores correctly
- `watcher.rs` — notify integration detects file changes within debounce window
- `broker.rs` — image_search with base64 input
- `the-one-ui` — /images route renders correct HTML

Target: 20+ new tests, 208 → ~230 total.

## Success Criteria

- `hybrid_search_enabled: true` + code-heavy project finds function names via BM25 that dense search misses
- Auto-indexing detects a changed .md file within 3 seconds of save
- Admin UI /images gallery renders all indexed images with thumbnails
- `memory.search_images` accepts a base64 screenshot and returns similar images
- Cache directories (`.fastembed_cache/`) untouched
- CI passes: cargo fmt --check, clippy --all-targets -D warnings, full test suite
- v0.7.0 binary builds cleanly on 6 platforms
- Previously-failing release workflow (fmt issue) stays fixed — this is now baseline

## Rollout Phases (implementation order)

1. **Phase A** — fmt fix verification + v0.6.0 successful release (in progress)
2. **Phase B** — Sparse embedding infrastructure (trait, provider, registry)
3. **Phase C** — Qdrant sparse vector support (named vectors, upsert, search)
4. **Phase D** — Hybrid search integration in MemoryEngine + config + tests
5. **Phase E** — File watcher module
6. **Phase F** — Auto-indexing wiring in broker + config
7. **Phase G** — Screenshot-based image search (API extension + dispatch)
8. **Phase H** — Admin UI image gallery
9. **Phase I** — Docs (hybrid-search.md, auto-indexing.md, update existing guides)
10. **Phase J** — CHANGELOG, PROGRESS, README, tag v0.7.0, release
