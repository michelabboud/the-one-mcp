# Multimodal Image Support + Reranking — Design

**Date:** 2026-04-05
**Target release:** v0.6.0
**Status:** Design — awaiting approval before implementation plan

## Goal

Two features bundled into v0.6.0 because both require the fastembed 4→5 bump:

1. **Image embeddings + search** — store screenshots, architecture diagrams, UI mockups, and whiteboard photos alongside code docs; find them with natural-language queries like "show me the auth flow diagram."

2. **Reranking** — use a cross-encoder model to re-score the top-N results from `memory.search`, dramatically improving retrieval quality (typically 15-30% on benchmarks) for a small latency cost.

## Strategy

Use fastembed-rs 5.x's `ImageEmbedding`, `TextRerank`, and new text model APIs. Store image embeddings in a separate Qdrant collection. Add OCR for text-in-image search. Everything ships behind feature flags and runtime toggles so users who don't need it pay zero cost.

## Prerequisite: fastembed bump 4 → 5.13

v5.x is required for `ImageEmbedding` and `TextRerank`. The bump also unlocks:
- `SparseTextEmbedding` — BM25-style hybrid search (deferred to v0.6.1)
- 6 new text models that were stubbed in the last session (BGE-M3, ModernBertEmbedLarge, JinaEmbeddingsV2BaseEN, SnowflakeArcticEmbedM, AllMpnetBaseV2, EmbeddingGemma300M)

**Breaking changes to check:** v5.0.0 release notes unavailable; must verify by compiling and fixing API drift. Known rename patterns to watch: `InitOptions` constructor signatures, enum variant renames, output type changes.

## Feature 1: Reranking

### Why

Bi-encoder embeddings (our current pipeline) embed query and documents independently — fast but lower accuracy. Cross-encoders rerank a shortlist by processing query+document together, capturing interaction effects. Standard retrieval architecture: bi-encoder recall → cross-encoder precision.

### Pipeline change

```
Before: query → embed → top-5 from Qdrant → return
After:  query → embed → top-20 from Qdrant → rerank → top-5 → return
```

### Models (fastembed 5.x `RerankerModel`)

| Variant | HF Model | Size | Use Case |
|---------|----------|------|----------|
| `JINARerankerV1BaseEn` | jinaai/jina-reranker-v1-base-en | ~140MB | English-only, fast |
| `JINARerankerV1TurboEn` | jinaai/jina-reranker-v1-turbo-en | ~60MB | English, fastest |
| `JINARerankerV2BaseMultilingual` | jinaai/jina-reranker-v2-base-multilingual | ~280MB | **Default — multilingual, already in cache** |
| `BGERerankerBase` | BAAI/bge-reranker-base | ~280MB | Strong English baseline |
| `BGERerankerV2M3` | BAAI/bge-reranker-v2-m3 | ~560MB | Highest quality, multilingual |

**Default: `JINARerankerV2BaseMultilingual`** — already downloaded in `.fastembed_cache/` (the 33GB we noticed earlier included this model). Zero-download first-run experience.

### New trait: `RerankerProvider`

```rust
// crates/the-one-memory/src/reranker.rs

#[async_trait]
pub trait RerankerProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankHit>, String>;
}

pub struct RerankHit {
    pub index: usize,   // position in original input
    pub score: f32,     // rerank score
}

pub struct FastEmbedReranker {
    model: Arc<fastembed::TextRerank>,
    model_name: String,
}
```

### Integration with `memory.search`

```rust
// crates/the-one-mcp/src/broker.rs — memory_search method

// 1. Embed query (existing)
let query_vec = self.memory.embed_single(&query).await?;

// 2. Retrieve more than requested (new)
let fetch_k = if self.config.rerank_enabled { top_k * 4 } else { top_k };
let initial_hits = self.memory.search(query_vec, fetch_k).await?;

// 3. Rerank if enabled (new)
let final_hits = if self.config.rerank_enabled {
    let docs: Vec<String> = initial_hits.iter()
        .map(|h| h.content.clone())
        .collect();
    let reranked = self.reranker.rerank(&query, &docs, top_k).await?;
    reranked.into_iter()
        .map(|r| initial_hits[r.index].clone())
        .collect()
} else {
    initial_hits.into_iter().take(top_k).collect()
};
```

### Configuration

```json
{
  "rerank_enabled": true,
  "rerank_model": "jina-reranker-v2-base-multilingual",
  "rerank_fetch_multiplier": 4
}
```

`rerank_fetch_multiplier` controls how many candidates to fetch before reranking. Higher = better recall, slower. Default 4x means `top_k=5` fetches 20, reranks to 5.

### Registry: `models/rerank-models.toml`

```toml
[meta]
fastembed_crate_version = "5"
updated = "2026-04-05"

[models.jina-reranker-v2-base-multilingual]
name = "jina-reranker-v2-base-multilingual"
size_mb = 280
multilingual = true
description = "Multilingual cross-encoder. Recommended default."
fastembed_enum = "JINARerankerV2BaseMultilingual"
default = true

[models.jina-reranker-v1-turbo-en]
name = "jina-reranker-v1-turbo-en"
size_mb = 60
multilingual = false
description = "Fastest English reranker."
fastembed_enum = "JINARerankerV1TurboEn"
default = false

[models.bge-reranker-v2-m3]
name = "bge-reranker-v2-m3"
size_mb = 560
multilingual = true
description = "Highest quality, largest model."
fastembed_enum = "BGERerankerV2M3"
default = false
```

### Feature flag

```toml
[features]
text-reranking = ["local-embeddings"]  # depends on fastembed, no new crates
```

Light feature — no new dependencies, just more fastembed API surface. Default-on because the model is already cached.

## Feature 2: Image Embeddings

## Image Models (fastembed 5.x `ImageEmbeddingModel`)

| Enum Variant | HF Model | Dims | Use Case |
|--------------|----------|------|----------|
| `ClipVitB32` | Qdrant/clip-ViT-B-32-vision | 512 | CLIP — pairs with text CLIP, industry standard |
| `Resnet50` | Qdrant/resnet50-onnx | 2048 | Pure image features, no text pairing |
| `UnicomVitB16` | Qdrant/Unicom-ViT-B-16 | 768 | Fine-grained classification |
| `UnicomVitB32` | Qdrant/Unicom-ViT-B-32 | 512 | Lighter Unicom variant |
| `NomicEmbedVisionV15` | nomic-ai/nomic-embed-vision-v1.5 | 768 | **Default — pairs with `NomicEmbedTextV15`** |
| `Qwen3VLEmbedding2B` | Qwen/Qwen3-VL-Embedding-2B | varies | Candle backend, multimodal |

### Why Nomic as default

The `NomicEmbedVisionV15` + `NomicEmbedTextV15` pair produces embeddings in the **same 768-dim space**. A user search query like "auth flow" can match both:
- A markdown section describing auth (text embedding)
- A diagram image of the auth flow (image embedding)

Both live in the same Qdrant vector space, so `memory.search` returns unified results ranked by cosine similarity regardless of content type.

CLIP ViT-B/32 is the alternative — also dual-encoder, but 512-dim and text-CLIP requires downloading a separate text model we don't currently use.

## Architecture

### New trait: `ImageEmbeddingProvider`

```rust
// crates/the-one-memory/src/image_embeddings.rs

#[async_trait]
pub trait ImageEmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String>;
    async fn embed_batch(&self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>, String>;
}

pub struct FastEmbedImageProvider {
    model: Arc<fastembed::ImageEmbedding>,
    dims: usize,
    model_name: String,
}
```

### New registry: `models/image-models.toml`

Mirrors `local-models.toml` structure with image-specific fields:

```toml
[meta]
fastembed_crate_version = "5"
updated = "2026-04-05"

[models.nomic-embed-vision-v1_5]
name = "nomic-embed-vision-v1.5"
dims = 768
size_mb = 700
description = "Pairs with nomic-embed-text-v1.5 for unified text+image search."
fastembed_enum = "NomicEmbedVisionV15"
default = true
paired_text_model = "NomicEmbedTextV15"

[models.clip-vit-b-32]
name = "clip-ViT-B-32-vision"
dims = 512
size_mb = 350
description = "CLIP — industry standard image encoder."
fastembed_enum = "ClipVitB32"
default = false
```

### Storage: separate Qdrant collection

```
Qdrant collections:
├── the_one_<project_id>           # existing — text chunks
└── the_one_images_<project_id>    # NEW — image embeddings
```

Each image point:
```rust
struct ImagePoint {
    id: String,           // hash of (path + mtime)
    vector: Vec<f32>,     // 768d (Nomic) or 512d (CLIP)
    payload: {
        source_path: String,
        file_size: u64,
        mtime_epoch: i64,
        caption: Option<String>,       // user-provided
        ocr_text: Option<String>,      // tesseract output
        thumbnail_path: Option<String>, // ~256px WebP
    }
}
```

### SQLite schema extension

```sql
CREATE TABLE IF NOT EXISTS managed_images (
    path TEXT PRIMARY KEY,
    hash TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    mtime_epoch INTEGER NOT NULL,
    caption TEXT,
    ocr_text TEXT,
    thumbnail_path TEXT,
    indexed_at_epoch INTEGER NOT NULL
);
CREATE INDEX idx_managed_images_hash ON managed_images(hash);
```

### OCR integration (tesseract)

Optional pipeline during ingestion:

```
image file → tesseract → text → store in ocr_text field
                                → also index as searchable text chunk
```

**Dependency:** `tesseract` crate (wraps libtesseract). Adds ~50MB runtime (Tesseract OCR engine + eng language data). Behind `image-ocr` feature flag (independent of `image-embeddings`).

**Why index OCR text as a text chunk too?** A diagram with labeled boxes ("User", "Auth Service", "DB") becomes searchable via the existing text memory, and the chunk can link back to the image. Double-coverage: semantic visual match + keyword text match.

## MCP Tool Surface Impact

Adds **2 new tools** to the current 15 → **17 total**.

| Tool | Purpose | Action |
|------|---------|--------|
| `memory.search_images` | Image-only semantic search | New top-level tool |
| `memory.ingest_image` | Manually add an image with caption | New top-level tool |

Alternative: multiplex into existing `memory.search` with a `modality` parameter (`text` / `image` / `both`). Rejected because:
- Separate tools give cleaner schemas
- Response shapes differ (image results include thumbnail paths, OCR text)
- Discoverability — LLMs find "search_images" more readily than inferring a modality param

### Auto-ingestion via `setup:refresh`

Extend the existing `setup` multiplexed tool's `refresh` action to also scan for images:

```
action: "refresh"
  → existing: re-scan markdown docs
  → NEW: walk for .png/.jpg/.jpeg/.webp in docs/ + .the-one/images/
  → NEW: embed new/changed images, update Qdrant + SQLite
```

## Configuration

### New config fields (`ProjectConfig`)

```json
{
  "image_embedding_enabled": false,
  "image_embedding_model": "nomic-embed-vision-v1.5",
  "image_embedding_dims": 768,
  "image_ocr_enabled": false,
  "image_ocr_language": "eng",
  "image_thumbnail_enabled": true,
  "image_thumbnail_max_px": 256,
  "max_image_size_bytes": 10485760,
  "max_images_per_project": 500,
  "image_file_extensions": ["png", "jpg", "jpeg", "webp"]
}
```

### New limits (`ConfigurableLimits`)

| Limit | Default | Min | Max |
|-------|---------|-----|-----|
| `max_image_size_bytes` | 10 MB | 100 KB | 100 MB |
| `max_images_per_project` | 500 | 10 | 10,000 |
| `max_image_search_hits` | 5 | 1 | 50 |
| `image_search_score_threshold` | 0.25 | 0.0 | 1.0 |

### Environment variables

`THE_ONE_IMAGE_EMBEDDING_ENABLED`, `THE_ONE_IMAGE_EMBEDDING_MODEL`, `THE_ONE_IMAGE_OCR_ENABLED`, etc.

## Feature Flags (Cargo)

```toml
[features]
default = ["local-embeddings"]
local-embeddings = ["dep:fastembed"]
image-embeddings = ["local-embeddings", "dep:image"]  # Nomic/CLIP vision via fastembed
image-ocr = ["image-embeddings", "dep:tesseract"]     # Requires Tesseract installed on host
```

**Why both feature flag AND runtime toggle:**
- **Feature flag** (`image-embeddings`): compile-time opt-out for users who want zero binary size/dependency overhead. Default release builds include it; the `--lean` variant excludes it.
- **Runtime toggle** (`image_embedding_enabled`): for users who have the feature compiled in but don't need it on a particular project. No model download, no Qdrant collection creation.

This matches the existing pattern: `local-embeddings` is a feature flag, and `embedding_provider: "local" | "api"` is a runtime toggle.

## New API Types

```rust
// crates/the-one-mcp/src/api.rs

pub struct ImageSearchRequest {
    pub project_root: String,
    pub project_id: String,
    pub query: String,
    pub top_k: usize,
}

pub struct ImageSearchHit {
    pub id: String,
    pub source_path: String,
    pub thumbnail_path: Option<String>,
    pub caption: Option<String>,
    pub ocr_text: Option<String>,
    pub score: f32,
}

pub struct ImageSearchResponse {
    pub hits: Vec<ImageSearchHit>,
}

pub struct ImageIngestRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub caption: Option<String>,
}

pub struct ImageIngestResponse {
    pub path: String,
    pub dims: usize,
    pub ocr_extracted: bool,
    pub thumbnail_generated: bool,
}
```

## Files to Create / Modify

### New files
- `crates/the-one-memory/src/reranker.rs` — RerankerProvider trait + FastEmbedReranker
- `crates/the-one-memory/src/image_embeddings.rs` — trait + FastEmbedImageProvider
- `crates/the-one-memory/src/image_ingest.rs` — file walker, hash-based change detection
- `crates/the-one-memory/src/ocr.rs` — tesseract wrapper (behind `image-ocr`)
- `crates/the-one-memory/src/thumbnail.rs` — image resize via `image` crate
- `models/rerank-models.toml` — rerank model registry
- `models/image-models.toml` — image model registry
- `schemas/mcp/v1beta/memory.search_images.request.schema.json` + response
- `schemas/mcp/v1beta/memory.ingest_image.request.schema.json` + response
- `docs/guides/image-search.md` — user guide
- `docs/guides/reranking.md` — rerank guide

### Modified files
- `Cargo.toml` (workspace) — bump `fastembed = "5"`, add `image`, `tesseract` optional deps
- `crates/the-one-memory/Cargo.toml` — new features
- `crates/the-one-memory/src/lib.rs` — module exports
- `crates/the-one-memory/src/qdrant.rs` — `create_image_collection`, `upsert_image_points`, `search_images`
- `crates/the-one-memory/src/models_registry.rs` — parse image-models.toml
- `crates/the-one-memory/src/embeddings.rs` — fix any fastembed 5.x API drift
- `crates/the-one-core/src/config.rs` — new image_* fields
- `crates/the-one-core/src/limits.rs` — new limits
- `crates/the-one-mcp/src/api.rs` — new types
- `crates/the-one-mcp/src/broker.rs` — `image_search()`, `image_ingest()` methods
- `crates/the-one-mcp/src/transport/tools.rs` — 2 new tool definitions
- `crates/the-one-mcp/src/transport/jsonrpc.rs` — 2 new dispatch arms
- `CHANGELOG.md`, `PROGRESS.md`, `README.md`, `CLAUDE.md` — v0.6.0 entries

## Token Cost Impact

Current: 15 tools @ ~1,700 tokens per session
After: 17 tools @ ~1,900 tokens per session (+~200 tokens, ~12% increase)

Justification: image search is a distinct capability that saves orders of magnitude more context elsewhere (user doesn't need to paste image descriptions, LLM can query for diagrams directly).

## Testing Strategy

### Unit tests (new)
- `image_embeddings.rs` — FastEmbedImageProvider produces correct-dimension vectors for PNG/JPEG
- `image_ingest.rs` — hash-based change detection, walker respects extension filter
- `ocr.rs` — extracts known text from a test fixture image (only with `image-ocr` feature)
- `thumbnail.rs` — generates ≤256px WebP from larger inputs
- `models_registry.rs` — parses image-models.toml, default model resolves correctly

### Integration tests (new)
- Full ingest → search round-trip with a fixture diagram
- Auto-ingestion via `setup:refresh` picks up new images
- Config disabled → ingest returns "not enabled" error
- Feature flag off → code compiles and search tool is absent from `tools/list`

### Fixtures needed
- `crates/the-one-memory/tests/fixtures/images/auth-flow.png` — architecture diagram with visible labels (for OCR)
- `crates/the-one-memory/tests/fixtures/images/schema.jpg` — DB schema sketch
- `crates/the-one-memory/tests/fixtures/images/tiny.png` — 1×1 minimal valid PNG for quick tests

## Breaking Changes

None for existing users. Image support is opt-in at both compile time and runtime. The fastembed 4→5 bump may surface minor API drift in `embeddings.rs` which will be fixed as part of this work.

## Rollout Phases (for implementation plan)

1. **Phase 1 — fastembed 5.x migration** (1-2 commits): bump version, fix API drift, wire up the 6 previously-stubbed text model variants, tests pass
2. **Phase 2 — reranking** (2 commits): RerankerProvider trait, FastEmbedReranker, rerank-models.toml, integrate into `memory_search`, config field, tests
3. **Phase 3 — image infrastructure** (4-5 commits): trait, provider, registry, Qdrant collection, thumbnail
4. **Phase 4 — OCR integration** (2 commits): tesseract wrapper, OCR pipeline, feature flag
5. **Phase 5 — MCP surface** (3 commits): API types, broker methods, tool definitions + dispatch (image tools + maintain actions for image management)
6. **Phase 6 — config + limits** (1 commit): runtime toggles, limits, env vars
7. **Phase 7 — docs + release** (2 commits): guides, CHANGELOG, tag v0.6.0

## Open Questions

1. **Should `memory.search` (text) also return image hits when `image_embedding_enabled: true`?** My recommendation: no — keep them as separate tools. Unified search is possible but makes response shapes heterogeneous, confusing for LLMs.

2. **Should OCR text be searchable via `memory.search`?** My recommendation: yes — OCR text becomes a regular text chunk with metadata linking back to the source image. This way keyword search finds labeled diagrams naturally.

3. **Image thumbnails — store where?** Options: (a) `.the-one/thumbnails/` in project, (b) `~/.the-one/cache/thumbnails/` global. Recommendation: project-local so it stays with the project and gets cleaned up on `setup:reset`.

4. **What about GIF/SVG/HEIC?** Defer. Start with PNG/JPEG/WebP (covered by `image` crate out of the box). Add later if users ask.

5. **Do we need a `memory.delete_image` tool?** Or does `docs.delete` cover it? Images aren't markdown docs, so `docs.delete` doesn't apply. Add `image.delete` as a third tool, or use multiplexed `maintain: images.delete`? **Recommendation:** put image management under `maintain` — `maintain action: "images.rescan"`, `maintain action: "images.delete"`.

## Success Criteria

**Reranking:**
- `memory.search` with `rerank_enabled: true` returns higher-quality results than the bi-encoder alone
- Reranker model loads from existing cache (no download needed for default)
- Config toggle disables reranking cleanly, falls back to direct top-k
- Rerank latency budget: <200ms for 20 candidates

**Image support:**
- Users can add images to `.the-one/images/` and find them via natural-language search
- `setup:refresh` auto-ingests new/changed images
- OCR-extracted text from diagrams is searchable alongside markdown
- Feature compiles out cleanly with `--no-default-features --features local-embeddings` (no image support)

**General:**
- All existing 183 tests still pass after fastembed 5.x migration
- New tests: 20+ unit + 8+ integration (reranker + images combined)
- Binary size increase with full features: <30 MB (mostly image crate + optional tesseract)
- Token cost per session: ~2,000 tokens (up from 1,700) after adding image tools
