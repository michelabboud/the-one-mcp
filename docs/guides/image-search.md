# Image Search Guide

Semantic search over images — index your diagrams, screenshots, and design assets so your AI assistant can find them by description.

## What Image Search Is

Image search lets you embed images into the same vector space as your text documents. Once indexed, you can query them in plain language:

- "Find the database schema diagram"
- "Which screenshot shows the login flow?"
- "Show me the architecture diagram from last sprint"

The search returns ranked matches with similarity scores. You can retrieve the full image or its OCR-extracted text for further analysis.

This is distinct from text memory search (`memory.search`) — image search operates over a separate Qdrant collection (`the_one_images`) and uses image embedding models rather than text embedding models.

## Enabling Image Search

Image search is **off by default** in lean builds and **on by default** in full builds. Check or set it in your config:

```json
{
  "image_embedding_enabled": true,
  "image_embedding_model": "nomic-vision"
}
```

Or via environment variable:

```bash
export THE_ONE_IMAGE_EMBEDDING_ENABLED=true
```

### Feature Flags

Two Cargo feature flags control what gets compiled:

| Flag | Default (full) | Default (lean) | What it enables |
|------|----------------|----------------|----------------|
| `image-embeddings` | on | off | Image ingest, search, Qdrant image collection |
| `image-ocr` | off | off | OCR text extraction via tesseract (opt-in everywhere) |

If you built from source without `image-embeddings`, the tools will exist in the schema but return `CoreError::NotEnabled`. Rebuild with the full profile:

```bash
bash scripts/build.sh build          # full (image-embeddings on)
bash scripts/build.sh build --lean   # lean (image-embeddings off)
```

## Image Embedding Models

Five models are available. All are local ONNX models via fastembed — no API calls, no cost, fully offline.

| Model Name | Alias | Dims | Download | Notes |
|------------|-------|------|----------|-------|
| `nomic-embed-vision-v1.5` | `nomic-vision` (default) | 768 | ~275MB | Pairs with nomic-embed-text for cross-modal search |
| `clip-vit-b-32` | `clip` | 512 | ~150MB | Classic CLIP, strong zero-shot |
| `resnet50` | `resnet` | 2048 | ~100MB | CNN features, good for visual similarity |
| `unicom-vit-b16` | `unicom-b16` | 512 | ~330MB | High quality ViT, slower |
| `unicom-vit-b32` | `unicom-b32` | 512 | ~170MB | Faster ViT variant |

**Recommendation:** Use the default `nomic-vision` if you also use `nomic-embed-text-v1.5` for text — they share a common embedding space, enabling cross-modal search (search text to find images and vice versa). Use `clip` for general-purpose image search.

To change the model:

```json
{
  "image_embedding_model": "clip"
}
```

Models download on first use and cache in `.fastembed_cache/` (gitignored). The download happens once per model.

## Supported Formats

- PNG
- JPG / JPEG
- WebP

Other formats (GIF, BMP, TIFF, SVG) are skipped with a warning. PDFs are not supported — export pages as PNG first.

## Adding Images: `memory.ingest_image`

The LLM calls this tool to index a single image:

```
User: "Index this architecture diagram"
LLM calls: memory.ingest_image({
  path: "/path/to/architecture-diagram.png",
  description: "System architecture showing service boundaries and data flow",
  tags: ["architecture", "services", "v2"]
})
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | Yes | Absolute path to the image file |
| `description` | No | Human-readable description stored alongside the embedding |
| `tags` | No | Array of strings for filtering |

**What happens:**
1. The image is read and validated (format check, size check)
2. A thumbnail is generated (if `image_thumbnail_enabled`)
3. OCR runs on the image (if `image_ocr_enabled`)
4. The image is embedded via the configured image model
5. The embedding + metadata is stored in Qdrant (`the_one_images` collection)
6. The image file is copied to `.the-one/images/` for retrieval

**Size limit:** `max_image_size_bytes` (default: 10MB). Images over this limit are rejected.

**Count limit:** `max_images_per_project` (default: 1000). At the limit, new ingests are rejected until old images are deleted.

## Searching Images: `memory.search_images`

`memory.search_images` supports two search modes. Exactly one of `query` or `image_base64` must be provided.

### Text query (natural language)

```
User: "Find diagrams related to authentication"
LLM calls: memory.search_images({
  query: "authentication flow login sequence",
  limit: 5
})
```

### Screenshot search (image → image)

Pass a base64-encoded image instead of a text query to find visually similar images:

```
User: "Find images that look like this screenshot"
LLM calls: memory.search_images({
  image_base64: "<base64-encoded PNG/JPEG bytes>",
  limit: 3
})
```

The image is decoded, embedded using the same image model as the indexed images (Nomic Vision by default), and the resulting vector is used as the query. This enables finding indexed images that are visually or structurally similar to the provided screenshot — useful for "find more like this" and deduplication workflows.

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `query` | no* | Natural language description of what to find |
| `image_base64` | no* | Base64-encoded image bytes for image→image similarity search |
| `limit` | No | Max results (default: 5, max: `max_image_search_hits`) |
| `score_threshold` | No | Minimum similarity score (0.0–1.0, default: `image_search_score_threshold`) |

*Exactly one of `query` or `image_base64` must be provided.

**Returns:** Array of matches, each with:
- `image_id` — unique identifier
- `path` — original file path
- `thumbnail_path` — path to thumbnail (if generated)
- `description` — stored description
- `tags` — stored tags
- `ocr_text` — extracted text (if OCR ran)
- `score` — cosine similarity (0.0–1.0)

**How the query works:** The query text is embedded using the *image* embedding model (not the text model), then searched against the image collection. For cross-modal models like Nomic Vision, text queries land in the same embedding space as images, giving good results. For CLIP-only models, the text is processed through CLIP's text encoder.

## Bulk Operations via `maintain`

Three image-specific actions are available through the `maintain` tool:

### `images.rescan`

Re-ingests all images in `.the-one/images/` from scratch. Use this after:
- Changing the image embedding model
- Suspecting corruption in the image index
- Migrating from an old version

```
LLM calls: maintain({ action: "images.rescan" })
```

This is equivalent to `docs`'s `reindex` action. It rebuilds the Qdrant `the_one_images` collection.

### `images.clear`

Removes all indexed images for the current project. Deletes the Qdrant collection entries and the files in `.the-one/images/`. Thumbnails are also cleared.

```
LLM calls: maintain({ action: "images.clear" })
```

Use with caution — there is no undo. Original files in your project are not touched; only the copies in `.the-one/images/` are removed.

### `images.delete`

Delete a single image by ID:

```
LLM calls: maintain({
  action: "images.delete",
  params: { image_id: "abc123" }
})
```

The image is removed from Qdrant, its copy in `.the-one/images/` is deleted, and the thumbnail is removed.

## OCR: Extracting Text from Images

OCR uses [Tesseract](https://tesseract-ocr.github.io/) to extract text from images before embedding. The extracted text is stored in the metadata and returned in search results, which lets the LLM read text in screenshots, diagrams with labels, and handwritten notes.

### Enabling OCR

OCR is **opt-in** — it requires tesseract installed on your host:

**Ubuntu/Debian:**
```bash
sudo apt-get install tesseract-ocr
```

**macOS:**
```bash
brew install tesseract
```

**Arch Linux:**
```bash
sudo pacman -S tesseract tesseract-data-eng
```

**Windows:** Download from [UB Mannheim builds](https://github.com/UB-Mannheim/tesseract/wiki). Add to PATH.

Once tesseract is installed, enable OCR in config:

```json
{
  "image_ocr_enabled": true,
  "image_ocr_language": "eng"
}
```

The `image_ocr_language` field accepts Tesseract language codes: `eng` (English, default), `fra` (French), `deu` (German), `spa` (Spanish), etc. Install additional language packs via your package manager.

### What OCR Adds

The extracted text is:
- Stored in the image metadata (accessible via `memory.search_images` results)
- Prepended to the embedding input (improves search relevance for text-heavy images)
- Returned in search results so the LLM can read it directly

OCR is most useful for:
- Screenshots with UI text
- Architecture diagrams with labels
- Error message screenshots
- Whiteboard photos with written content

OCR adds ~100-500ms per image ingest depending on image size. It does not affect search latency.

## Thumbnail Generation

Thumbnails are small previews generated at ingest time. They are stored in `.the-one/thumbnails/` and their paths are returned in search results.

```json
{
  "image_thumbnail_enabled": true,
  "image_thumbnail_max_px": 256
}
```

`image_thumbnail_max_px` controls the longest edge of the thumbnail. Default is 256 pixels. Thumbnails maintain aspect ratio.

Thumbnails are generated using the `image` crate (pure Rust, no external dependencies). They add ~10-50ms per ingest.

When to disable: if you never display images in your workflow, disable thumbnails to save disk space and ingest time.

## Storage Layout

```
<project>/.the-one/
├── images/                   # Copies of indexed images
│   ├── abc123.png
│   ├── def456.jpg
│   └── ...
├── thumbnails/               # Generated thumbnails
│   ├── abc123_thumb.png
│   ├── def456_thumb.jpg
│   └── ...
└── state.db                  # SQLite: image metadata, paths, tags
```

The Qdrant `the_one_images` collection stores the embeddings. If Qdrant is unavailable, image search falls back to a local file-based index (keyword search over metadata only, no semantic search).

## Size and Count Limits

All limits are configurable in config or via the admin UI:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_image_size_bytes` | 10,485,760 (10MB) | Max size per image |
| `max_images_per_project` | 1,000 | Max indexed images |
| `max_image_search_hits` | 20 | Max results per search query |
| `image_search_score_threshold` | 0.3 | Minimum similarity score |

To raise limits in config:

```json
{
  "limits": {
    "max_image_size_bytes": 20971520,
    "max_images_per_project": 5000,
    "max_image_search_hits": 50
  }
}
```

## Example Workflow: Indexing a Folder of Diagrams

Here's a complete session for indexing an entire docs/diagrams folder and then querying it:

**Step 1 — Enable image search in config:**

```json
{
  "image_embedding_enabled": true,
  "image_embedding_model": "nomic-vision",
  "image_ocr_enabled": true,
  "image_thumbnail_enabled": true
}
```

**Step 2 — Ask your AI assistant to index the folder:**

```
You: "Index all images in docs/diagrams/"
```

The LLM will call `memory.ingest_image` for each PNG/JPG/WebP file, generating descriptions from the filename and (if OCR is on) the text content.

**Step 3 — Search:**

```
You: "Find the diagram showing how the payment service connects to the database"

LLM calls: memory.search_images({
  query: "payment service database connection architecture",
  limit: 5
})

Returns:
  1. docs/diagrams/payment-arch-v2.png (score: 0.82)
     Description: "Payment service architecture diagram"
     OCR text: "PaymentService → PostgreSQL, PaymentService → Redis cache"
  2. docs/diagrams/service-overview.png (score: 0.61)
     ...
```

**Step 4 — Get full context:**

```
You: "Read the text from the top match and tell me what cache layer is used"

LLM reads the ocr_text field from the search result and answers directly.
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `image search not enabled` error | Set `image_embedding_enabled: true` in config |
| `feature not compiled` error | Rebuild with `bash scripts/build.sh build` (full profile) |
| `tesseract not found` error | Install tesseract (see OCR section), or set `image_ocr_enabled: false` |
| No search results | Lower `image_search_score_threshold` (try 0.1), check that images were ingested |
| Wrong model error on rescan | The Qdrant collection was created with a different model dimension; run `images.clear` then `images.rescan` |
| Images not indexed after config change | Run `maintain({ action: "images.rescan" })` to rebuild index |
| Slow first ingest | Image model downloading (~150-330MB), cached after first use |
| Large memory usage during ingest | Each model loads into RAM; the nomic-vision model uses ~500MB during embedding |

## Admin UI Image Gallery

The embedded admin UI now includes an image gallery at `/images`. When running the admin UI server:

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" ~/.the-one/bin/embedded-ui
```

Navigate to `http://127.0.0.1:8787/images` to see a thumbnail grid of all indexed images for the active project. Clicking a thumbnail shows the full image. The gallery fetches metadata from the `/api/images` JSON endpoint.

Individual thumbnails are served at `/images/thumbnail/<hash>` with security validation on the hash pattern (alphanumeric + hyphens only) to prevent path traversal.

## Related

- [`memory.search`](the-one-mcp-complete-guide.md#10-rag-pipeline) — Text document search
- [`maintain`](the-one-mcp-complete-guide.md#9-managed-documents) — Admin operations
- [Reranking Guide](reranking.md) — Improve search result quality with cross-encoder reranking
- [Hybrid Search Guide](hybrid-search.md) — Dense + sparse search for code-heavy repos
- [Complete Guide](the-one-mcp-complete-guide.md) — Full reference
