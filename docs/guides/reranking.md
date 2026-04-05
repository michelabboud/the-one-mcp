# Reranking Guide

Cross-encoder reranking improves the precision of `memory.search` results with a 15–30% quality boost over bi-encoder search alone.

## How Search Works Without Reranking

Standard semantic search uses a **bi-encoder** model:

1. Your query is embedded into a vector
2. The vector is compared against all stored document chunk vectors (cosine similarity)
3. Top-k results are returned, sorted by similarity score

Bi-encoders are fast — they compute embeddings independently and compare them with a dot product. But they have a weakness: the query and document vectors are computed *separately*, so the model can't see how the two relate to each other. Close-in-meaning terms may score similarly to exact matches.

## What Cross-Encoder Reranking Adds

A **cross-encoder** model sees both the query and the candidate document together in a single forward pass. This gives it full attention over both, letting it model:

- Exact keyword overlap
- Semantic entailment
- Contextual relevance

The tradeoff: cross-encoders are slow. You can't run them over your entire corpus. The pattern is:

```
Bi-encoder (fast, recall-oriented)
  → fetch N candidates (e.g., top 20)
Cross-encoder (slower, precision-oriented)
  → rerank those N candidates
  → return top k (e.g., top 5)
```

This gives you recall from the bi-encoder and precision from the cross-encoder.

## Enabling Reranking

Add to your config (`~/.the-one/config.json` or `<project>/.the-one/config.json`):

```json
{
  "reranker_enabled": true
}
```

That's it. The default model (`jina-reranker-v2-base-multilingual`) is downloaded automatically (~180MB) and cached in `.fastembed_cache/`.

To use a different model:

```json
{
  "reranker_enabled": true,
  "reranker_model": "bge-reranker-base"
}
```

## Available Reranker Models

| Model | Size | Latency (top 20) | Languages | Notes |
|-------|------|-----------------|-----------|-------|
| `jina-reranker-v2-base-multilingual` (default) | ~180MB | ~100ms | 100+ | Best multilingual quality |
| `bge-reranker-base` | ~110MB | ~60ms | English | Faster, English-only |
| `bge-reranker-large` | ~350MB | ~200ms | English | Highest English quality |

The default jina-reranker-v2 model works well with any language, matches the multilingual text embeddings, and is likely already in your cache if you ran image search (it downloads on first reranker use).

## How It Integrates with `memory.search`

When `reranker_enabled: true`, `memory.search` automatically applies reranking:

```
memory.search(query, limit=5)
  ↓
1. Bi-encoder: embed query, search Qdrant
   → fetch limit * rerank_fetch_multiplier candidates (e.g., 5 * 4 = 20)
2. Cross-encoder: score each candidate against query
   → sort by cross-encoder score
   → return top limit results (5)
```

The `rerank_fetch_multiplier` controls how many candidates to fetch for reranking. Higher = better recall (more candidates for the reranker to choose from) at slightly higher latency.

```json
{
  "reranker_enabled": true,
  "rerank_fetch_multiplier": 4
}
```

Default multiplier is 4. Setting it to 1 effectively disables the fetch expansion (reranker still runs but over the same candidates). Setting it higher (e.g., 8) improves recall at the cost of more candidates to rerank.

## Performance Characteristics

With the default model and multiplier on a modern CPU:

| Scenario | Typical latency |
|----------|----------------|
| Bi-encoder only (limit=5) | ~15–40ms |
| Bi-encoder + reranker (fetch 20, return 5) | ~80–200ms |
| Bi-encoder + reranker (fetch 40, return 5) | ~120–350ms |

Latency scales with the number of candidates, not with the corpus size. The reranker only processes the fetched candidates, not all your documents.

On Apple Silicon and recent Intel CPUs, ONNX Runtime uses hardware acceleration (ANE/AVX-512), keeping latency toward the lower end.

## When to Use Reranking

**Use it when:**
- You have enough documents that search precision matters (50+ chunks)
- You're getting "pretty close but not quite right" results
- Your documents have a lot of similar-sounding content (e.g., design docs, API references)
- You're doing question-answering style queries where exact relevance matters

**Consider skipping it when:**
- Your project has very few indexed documents (< 20 chunks)
- Search latency is critical (< 20ms requirement)
- You're running on very constrained hardware (< 2GB RAM)

Reranking is transparent — your queries use the same `memory.search` tool either way. You can toggle it on and off in config without any data migration.

## Example: Quality Difference

Here is a concrete example of the improvement. Searching for "how does the auth middleware validate tokens":

**Without reranking (bi-encoder only):**
1. `auth.md#middleware` — score 0.71 — middleware setup overview
2. `auth.md#configuration` — score 0.68 — auth config options
3. `decisions/jwt-choice.md` — score 0.65 — why JWT was chosen

**With reranking:**
1. `auth.md#middleware` → rerank 0.94 — middleware setup (mentions `validate_token`)
2. `decisions/jwt-choice.md` → rerank 0.79 — JWT validation details discussed
3. `auth.md#configuration` → rerank 0.52 — config (less relevant to validation)

The reranker promotes the JWT decision doc because it contains the most direct discussion of token validation, even though its bi-encoder score was third.

## Full Config Reference

```json
{
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual",
  "rerank_fetch_multiplier": 4
}
```

All three fields are optional. Defaults:
- `reranker_enabled`: `false`
- `reranker_model`: `"jina-reranker-v2-base-multilingual"`
- `rerank_fetch_multiplier`: `4`

## Related

- [Image Search Guide](image-search.md) — Semantic search over images
- [Complete Guide: RAG Pipeline](the-one-mcp-complete-guide.md#10-rag-pipeline) — Chunking, embedding, and search
- [Complete Guide: Embeddings](the-one-mcp-complete-guide.md#7-embeddings) — Choosing a text embedding model
