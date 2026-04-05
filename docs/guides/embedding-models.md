# Embedding Models Guide

> v0.7.0 — authoritative sources: `models/local-models.toml`, `models/api-models.toml`, `models/image-models.toml`, `models/rerank-models.toml`

## Overview

Embeddings are the foundation of semantic search in the-one-mcp. Every document chunk is converted to a dense vector by an embedding model; at query time, your question is embedded the same way and the nearest vectors are returned. The model you pick determines search quality, latency, disk usage, and whether non-English text is supported.

This guide helps you pick the right model for your project.

---

## Quick Decision Tree

```
Do you need non-English or mixed-language support?
├── Yes ──> multilingual tier
│           ├── Any size OK?     → multilingual-e5-large (best quality, 220 MB)
│           ├── Moderate size?   → multilingual-e5-base (90 MB)
│           └── Smallest?        → multilingual-e5-small or paraphrase-ml-minilm-l12-v2 (45 MB)
│
└── No (English only)
    ├── Just getting started / testing?  → fast tier (all-MiniLM-L6-v2, 23 MB)
    ├── Want balance of speed + quality? → balanced tier (BGE-base-en-v1.5, 50 MB)
    ├── Best local quality?              → quality tier (BGE-large-en-v1.5, 130 MB) ← DEFAULT
    ├── Disk is very tight?              → quantized variant (BGE-large-en-v1.5-Q, 40 MB)
    └── API key available?               → API provider (text-embedding-3-small)

Do you also need image search?
└── Yes ──> enable image_embedding_enabled + nomic-embed-vision-v1.5
           (pairs with nomic-embed-text-v1.5 for unified text+image queries)

Do you want better search ranking with slight latency trade-off?
└── Yes ──> enable reranker_enabled + jina-reranker-v2-base-multilingual
```

---

## Text Embedding Models

### Tiered System

the-one-mcp organizes local models into four tiers. You can use either the exact model name or a tier alias in `embedding_model`.

| Tier alias | Typical dims | Size range | Use case |
|---|---|---|---|
| `fast` | 384 | 7–33 MB | Getting started, low-memory environments, fast iteration |
| `balanced` | 768 | 15–60 MB | Quality/speed tradeoff for everyday use |
| `quality` | 1024 | 40–150 MB | Best local search quality (default tier) |
| `multilingual` | 384–1024 | 12–220 MB | Non-English or mixed-language projects |

The default model is **BGE-large-en-v1.5** (quality tier, 1024 dims, 130 MB).

---

### All Supported Local Models

#### Installer-visible models (primary tiers)

| Name | Tier | Dims | Size | Latency | Multilingual | Description | fastembed enum |
|---|---|---|---|---|---|---|---|
| `all-MiniLM-L6-v2` | fast | 384 | 23 MB | fastest | No | Fast, small. Good for getting started. | `AllMiniLML6V2` |
| `BGE-base-en-v1.5` | balanced | 768 | 50 MB | ~2x | No | Good quality/speed tradeoff. | `BGEBaseENV15` |
| **`BGE-large-en-v1.5`** | **quality** | **1024** | **130 MB** | **~4x** | **No** | **Best local quality. Default.** | `BGELargeENV15` |
| `multilingual-e5-large` | multilingual | 1024 | 220 MB | ~5x | Yes | Best for non-English or mixed-language. | `MultilingualE5Large` |
| `multilingual-e5-base` | multilingual | 768 | 90 MB | ~3x | Yes | Multilingual, moderate size. | `MultilingualE5Base` |
| `multilingual-e5-small` | multilingual | 384 | 45 MB | ~1.5x | Yes | Lightweight multilingual option. | `MultilingualE5Small` |
| `paraphrase-ml-minilm-l12-v2` | multilingual | 384 | 45 MB | ~1.5x | Yes | Paraphrase-tuned multilingual model. | `ParaphraseMLMiniLML12V2` |

Latency is relative to `all-MiniLM-L6-v2` on a typical development laptop.

#### Additional local models (available via `models.list`)

| Name | Tier | Dims | Size | Notes |
|---|---|---|---|---|
| `all-MiniLM-L12-v2` | fast | 384 | 33 MB | Slightly better than L6 |
| `BGE-small-en-v1.5` | fast | 384 | 24 MB | Compact BGE |
| `nomic-embed-text-v1` | balanced | 768 | 55 MB | 8192 token context |
| `nomic-embed-text-v1.5` | balanced | 768 | 55 MB | Nomic v1.5, 8192 context. Pairs with Nomic vision model |
| `mxbai-embed-large-v1` | quality | 1024 | 130 MB | Mixedbread AI, top-tier quality |
| `gte-base-en-v1.5` | balanced | 768 | 50 MB | Alibaba GTE, strong English |
| `gte-large-en-v1.5` | quality | 1024 | 130 MB | Alibaba GTE large |
| `BGE-M3` | multilingual | 1024 | 220 MB | 100+ languages |
| `ModernBERT-embed-large` | quality | 1024 | 150 MB | ModernBERT architecture |
| `jina-embeddings-v2-base-code` | balanced | 768 | 55 MB | Code-specific embeddings |
| `jina-embeddings-v2-base-en` | balanced | 768 | 55 MB | 8192 token context |
| `snowflake-arctic-embed-m` | balanced | 768 | 60 MB | Strong mid-range retrieval |
| `all-mpnet-base-v2` | balanced | 768 | 60 MB | Widely used 768d baseline |
| `BGE-small-zh-v1.5` | multilingual | 512 | 24 MB | Chinese-optimized small |
| `BGE-large-zh-v1.5` | multilingual | 1024 | 130 MB | Chinese-optimized large |
| `embeddinggemma-300m` | quality | 1536 | 600 MB | Google Gemma, highest dims |
| `paraphrase-ml-mpnet-base-v2` | multilingual | 768 | 60 MB | Paraphrase-tuned multilingual MPNet |

#### Quantized variants

Quantized models use INT8 weights. They produce slightly lower quality embeddings but are significantly smaller and faster — a good choice when disk space or inference latency is constrained.

| Name | Tier | Dims | Size | Full-precision counterpart |
|---|---|---|---|---|
| `all-MiniLM-L6-v2-Q` | fast | 384 | 7 MB | `all-MiniLM-L6-v2` (23 MB) |
| `BGE-small-en-v1.5-Q` | fast | 384 | 8 MB | `BGE-small-en-v1.5` (24 MB) |
| `all-MiniLM-L12-v2-Q` | fast | 384 | 10 MB | `all-MiniLM-L12-v2` (33 MB) |
| `BGE-base-en-v1.5-Q` | balanced | 768 | 15 MB | `BGE-base-en-v1.5` (50 MB) |
| `nomic-embed-text-v1.5-Q` | balanced | 768 | 18 MB | `nomic-embed-text-v1.5` (55 MB) |
| `gte-base-en-v1.5-Q` | balanced | 768 | 15 MB | `gte-base-en-v1.5` (50 MB) |
| `snowflake-arctic-embed-m-Q` | balanced | 768 | 18 MB | `snowflake-arctic-embed-m` (60 MB) |
| `BGE-large-en-v1.5-Q` | quality | 1024 | 40 MB | `BGE-large-en-v1.5` (130 MB) |
| `mxbai-embed-large-v1-Q` | quality | 1024 | 40 MB | `mxbai-embed-large-v1` (130 MB) |
| `gte-large-en-v1.5-Q` | quality | 1024 | 40 MB | `gte-large-en-v1.5` (130 MB) |
| `paraphrase-ml-minilm-l12-v2-Q` | multilingual | 384 | 12 MB | `paraphrase-ml-minilm-l12-v2` (45 MB) |

---

### Selection Criteria

**Project size**

For small projects (< 500 documents, < 10 MB of text) the fast tier produces perfectly acceptable results. For medium to large projects the quality tier meaningfully improves recall.

**Language(s)**

English-only: use any non-multilingual model. Mixed or non-English: use a multilingual tier model. BGE-M3 supports 100+ languages and is the highest-quality multilingual option. For primarily Chinese text, the dedicated `BGE-small-zh-v1.5` or `BGE-large-zh-v1.5` models often outperform the general multilingual models.

For code-heavy repositories (Rust, Python, Go, etc.) consider `jina-embeddings-v2-base-code`, which was trained specifically on code.

**Latency budget**

On a typical developer laptop:
- fast tier: ~1–5 ms per chunk
- balanced tier: ~2–10 ms per chunk
- quality tier: ~4–20 ms per chunk
- multilingual-e5-large: ~5–25 ms per chunk

These numbers scale with batch size. Reindexing a large project with many chunks is where tier choice has the most visible impact.

**Disk space**

Quantized variants cut disk usage by 65–75% with minimal quality loss. If you need the quality tier but have limited space, `BGE-large-en-v1.5-Q` (40 MB) is a good alternative to `BGE-large-en-v1.5` (130 MB).

**Quality requirements**

For highest retrieval precision: `BGE-large-en-v1.5` or `mxbai-embed-large-v1` (1024 dims, 130 MB each). If you have a spare 600 MB, `embeddinggemma-300m` at 1536 dims may offer better recall for diverse queries.

---

### API-Based Embeddings

Set `embedding_provider` to `"api"` to use an external embedding API instead of a local model. This avoids the local ONNX download and offloads inference, at the cost of network latency and API fees.

**Supported providers:**

| Provider | Model | Dims | Multilingual | Notes |
|---|---|---|---|---|
| OpenAI | `text-embedding-3-small` | 1536 | Yes | Fast, cheap. Good API default. |
| OpenAI | `text-embedding-3-large` | 3072 | Yes | Best quality from OpenAI. Supports Matryoshka truncation. |
| Voyage AI | `voyage-3` | 1024 | Yes | Strong code understanding. |
| Voyage AI | `voyage-3-lite` | 512 | Yes | Lighter, faster Voyage model. |
| Cohere | `embed-v4.0` | 1024 | Yes | Latest Cohere model. |
| Cohere | `embed-multilingual-v3.0` | 1024 | Yes | Optimized for 100+ languages. |

Any OpenAI-compatible endpoint also works (LiteLLM, local proxies, etc.) — set `embedding_api_base_url` to the base URL.

**Config for OpenAI:**
```json
{
  "embedding_provider": "api",
  "embedding_model": "text-embedding-3-small",
  "embedding_api_base_url": "https://api.openai.com/v1",
  "embedding_api_key": "sk-...",
  "embedding_dimensions": 1536
}
```

**Config for LiteLLM proxy:**
```json
{
  "embedding_provider": "api",
  "embedding_model": "text-embedding-3-small",
  "embedding_api_base_url": "http://127.0.0.1:4000/v1",
  "embedding_api_key": "anything"
}
```

---

### Switching Models

1. Update `embedding_model` (and `embedding_provider` if switching between local and API) in your config file or via `config update`.
2. Update `embedding_dimensions` to match the new model's output dimensions.
3. Run a full reindex: use the `maintain` tool with `action: "reindex"` and your `project_root` + `project_id`. This deletes existing Qdrant vectors and re-embeds all indexed documents with the new model.

Skipping the reindex will produce incorrect search results because old vectors (from the previous model) will be compared to new query vectors (from the new model).

---

## Image Embedding Models

Image embedding is disabled by default. Enable it with `image_embedding_enabled: true` in your config (requires the `image-embeddings` feature flag in the binary).

### Supported Models

| Name | Dims | Size | Description | fastembed enum |
|---|---|---|---|---|
| **`nomic-embed-vision-v1.5`** | 768 | 700 MB | Pairs with `nomic-embed-text-v1.5` for unified text+image search. **Default.** | `NomicEmbedVisionV15` |
| `clip-ViT-B-32-vision` | 512 | 350 MB | CLIP — industry-standard image encoder. | `ClipVitB32` |
| `resnet50-onnx` | 2048 | 95 MB | Pure image features, no text pairing. | `Resnet50` |
| `Unicom-ViT-B-16` | 768 | 350 MB | Fine-grained visual classification. | `UnicomVitB16` |
| `Unicom-ViT-B-32` | 512 | 175 MB | Lighter Unicom variant. | `UnicomVitB32` |

---

### Dual-Encoder Pairing

A dual-encoder setup uses the same embedding space for both text and images, allowing you to search images with a text query (e.g. "diagram showing retry logic") and get back relevant screenshots or diagrams.

**Why Nomic is the recommended default**

`nomic-embed-vision-v1.5` was trained jointly with `nomic-embed-text-v1.5`. Both models project into the same 768-dimensional space, so a text query embedding can be directly compared to image embeddings. This is the only pair in the supported model list with a verified shared embedding space.

To use dual-encoder search, set both models:
```json
{
  "embedding_provider": "local",
  "embedding_model": "nomic-embed-text-v1.5",
  "embedding_dimensions": 768,
  "image_embedding_enabled": true,
  "image_embedding_model": "nomic-embed-vision-v1.5"
}
```

Note: switching the text model to `nomic-embed-text-v1.5` requires a full text reindex.

**When to use CLIP instead**

`clip-ViT-B-32-vision` is the industry-standard cross-modal model (text + images trained together by OpenAI). However, no corresponding text model is listed in the local registry, so you lose unified text-image search. Use CLIP when you want image similarity search only (image-to-image), or when you are using an API text embedding provider that has its own CLIP-aligned model.

**When to skip dual-encoders (Resnet50)**

`resnet50-onnx` produces pure visual feature vectors with no text alignment. Use it for image deduplication, visual similarity clustering, or when you only need image-to-image search and don't need text queries to return image results.

---

## Reranker Models

Reranking applies a cross-encoder model to re-score the top-N candidates returned by vector search, typically improving precision at the cost of additional latency (the cross-encoder reads both the query and each document together, rather than comparing independent embeddings).

Enable reranking:
```json
{
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual"
}
```

### Supported Reranker Models

| Name | Size | Multilingual | Description | fastembed enum |
|---|---|---|---|---|
| **`jina-reranker-v2-base-multilingual`** | 280 MB | Yes | Multilingual cross-encoder. **Recommended default.** | `JINARerankerV2BaseMultiligual` |
| `jina-reranker-v1-turbo-en` | 60 MB | No | Fastest English reranker. | `JINARerankerV1TurboEn` |
| `bge-reranker-base` | 280 MB | No | BAAI BGE baseline reranker. | `BGERerankerBase` |
| `bge-reranker-v2-m3` | 560 MB | Yes | Highest quality, multilingual. | `BGERerankerV2M3` |

The default reranker (set in `config.rs`) is `bge-reranker-base`. The recommended default for most projects is `jina-reranker-v2-base-multilingual` because it handles both English and non-English content and downloads at half the size of `bge-reranker-v2-m3`.

### When to Enable Reranking

Enable reranking when:
- Search precision matters more than raw latency (e.g. the assistant is answering questions about code)
- You are returning more than 5 results and quality at the top of the list is important
- Your documents are long and chunked with substantial overlap (reranking picks the most relevant chunk more reliably)

Skip reranking when:
- You need sub-100 ms end-to-end search latency
- Your corpus is very small (< 100 documents) — vector recall alone is usually precise enough
- You are running on resource-constrained hardware

### Latency vs Quality Tradeoff

Reranker models run a forward pass for each query-document pair. With `max_search_hits: 5` the extra cost is small. With `max_search_hits: 20` the reranker evaluates 20 pairs per query.

Rough latency additions on a typical laptop (per query, over 10 candidates):

| Model | Added latency |
|---|---|
| `jina-reranker-v1-turbo-en` | ~20–50 ms |
| `jina-reranker-v2-base-multilingual` | ~50–150 ms |
| `bge-reranker-base` | ~50–150 ms |
| `bge-reranker-v2-m3` | ~150–400 ms |

---

## Sparse Models (Hybrid Search)

Sparse models assign importance weights to individual tokens and produce sparse vectors for exact-term matching. They are used by the hybrid search feature (`hybrid_search_enabled: true`) to complement the dense embedding model.

### Why Sparse Models?

Dense models excel at semantic similarity ("meaning matches meaning"). Sparse models excel at lexical overlap — giving high weight to rare or distinctive tokens. In code-heavy projects this helps retrieve:

- Function names (`parse_header`, `spawn_blocking`)
- Error strings (`BorrowCheckError`, `ConnectionRefused`)
- Crate and package names (`serde_json`, `tokio`, `anyhow`)
- Short identifiers that are uncommon in the dense model's training corpus

### Supported Sparse Models

| Alias | Underlying model | Notes |
|-------|-----------------|-------|
| `bm25` | SPLADE++Ensemble Distil | Only option in fastembed 5.13. No additional model download — uses built-in tokenizer. |

> **Note on naming:** fastembed 5.13 registers SPLADE++Ensemble Distil under the alias `"bm25"`. Classical BM25 (probabilistic term-frequency weighting) was removed in this version because it required a separate tokenizer pipeline. The SPLADE++ model is learned and generally outperforms classical BM25, but the alias is retained for compatibility.

### No Download Required

Unlike dense embedding models that download ONNX weight files (~23–220 MB), the `"bm25"` sparse model uses only a tokenizer that is bundled with the fastembed crate. Enabling hybrid search has **no model download overhead**.

### Configuration

Enable hybrid search to activate the sparse model:

```json
{
  "hybrid_search_enabled": true,
  "sparse_model": "bm25"
}
```

See [Hybrid Search Guide](hybrid-search.md) for weight tuning, score normalization, and troubleshooting.

---

## Model Downloads and Caching

### Where models live

All fastembed ONNX models are cached in `.fastembed_cache/` inside the global state directory (`~/.the-one/.fastembed_cache/` by default). The directory is created on first use. It is gitignored.

Image models and reranker models use the same cache directory.

### First-run download times

Models are downloaded automatically on first use from HuggingFace Hub. Download times depend on your connection speed:

| Size | 100 Mbps | 25 Mbps |
|---|---|---|
| 7–23 MB (fast tier) | < 5 s | < 10 s |
| 50–60 MB (balanced) | ~5 s | ~20 s |
| 130 MB (quality) | ~12 s | ~45 s |
| 220 MB (multilingual-large) | ~20 s | ~75 s |
| 700 MB (nomic-vision) | ~60 s | ~4 min |

### Disk usage per model

Total disk usage equals the ONNX model file plus tokenizer and config files (usually 1–5 MB additional). For the default setup:

- Text model (BGE-large-en-v1.5): ~133 MB
- Default image model (nomic-embed-vision-v1.5): ~705 MB
- Default reranker (bge-reranker-base): ~283 MB

### Pre-downloading for offline use

To cache a model before working offline, trigger a `project.init` or call the `docs.index` tool while connected to the internet. The model download happens at first embedding call and is then cached permanently.

To pre-download multiple models simultaneously, initialize multiple projects with different `embedding_model` values.

---

## Benchmarks / Rough Performance

These are rough relative figures, not rigorous benchmarks. Actual performance depends on hardware, batch size, and document characteristics.

### Search quality vs tier

| Tier | Relative recall@5 |
|---|---|
| fast (384d) | baseline |
| balanced (768d) | +10–15% |
| quality (1024d) | +15–25% |
| quality + reranker | +20–35% |

### Latency per query (local, single thread, no reranker)

| Tier | Embedding latency | Qdrant search | Total |
|---|---|---|---|
| fast | ~1 ms | ~2–5 ms | ~3–6 ms |
| balanced | ~2 ms | ~2–5 ms | ~4–7 ms |
| quality | ~4 ms | ~2–5 ms | ~6–9 ms |
| API (network) | ~50–200 ms | ~2–5 ms | ~52–205 ms |

Query embedding is fast because only a single short text is embedded. Document indexing is the heavy operation; latency there scales with chunk count × embedding time.

---

## Troubleshooting

**Model download fails or hangs**

Check network connectivity to HuggingFace Hub (`huggingface.co`). If operating in an air-gapped environment, manually copy the ONNX files into the cache directory following fastembed's expected layout.

**`embedding_dimensions` mismatch**

If you set `embedding_dimensions` to a value that doesn't match what the model outputs, vectors stored in Qdrant will have the wrong shape and all searches will fail or return garbage results. When in doubt, omit `embedding_dimensions` and let the default for the chosen model apply.

**Search quality degraded after switching models**

You changed `embedding_model` without running a reindex. Run `maintain reindex` to rebuild all vectors with the new model.

**Reranker not loading**

The reranker model downloads at first use like text models. Check for network errors in the server logs. If the model file is corrupt, delete `.fastembed_cache/` and let it re-download.

**Image search returns no results**

1. Confirm `image_embedding_enabled: true` in the resolved config (use `config export`).
2. Confirm the binary was compiled with the `image-embeddings` feature flag.
3. Run `maintain images.rescan` to trigger re-indexing of images in the project.
4. Check `limits.image_search_score_threshold` — the default of `0.25` may need lowering for some image collections.

**OCR produces no text**

1. Verify `image_ocr_enabled: true` and the correct `image_ocr_language` code.
2. Ensure Tesseract is installed: `tesseract --version`.
3. Ensure the required language data pack is installed (e.g. `tesseract-ocr-eng` on Debian/Ubuntu).
