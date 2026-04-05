# Hybrid Search Guide

> v0.8.0 — combines dense cosine similarity with sparse lexical matching for stronger exact-match retrieval.

## What Hybrid Search Is

Standard semantic search encodes both your documents and your query as dense vectors and finds the nearest neighbors by cosine similarity. This works very well for paraphrastic queries ("what does this module do?") but can miss exact matches for tokens that are rare in training data — function names, error codes, crate names, variable names, or short identifiers.

**Hybrid search** fuses two signals:

- **Dense** — your existing embedding model (BGE, Nomic, etc.) converted to a cosine-similarity score.
- **Sparse** — a SPLADE++ model that assigns importance weights to individual tokens and returns a dot-product score. Sparse models excel at exact-term recall: they give high weight to uncommon tokens, so `NullPointerException` or `BorrowCheckError` get matched precisely rather than by semantic proximity.

At query time both scores are computed and combined into a final score using configurable weights. Qdrant's hybrid query API handles the fusion using Reciprocal Rank Fusion (RRF) internally, then the-one-mcp applies an additional weighted linear combination on the normalized scores.

### Sparse model in use: SPLADE++

fastembed 5.13 exposes SPLADE++ under the alias `"bm25"` (classical BM25 was removed in that version because it required a different tokenizer pipeline). Despite the name, the underlying model is SPLADE++Ensemble Distil — a learned sparse encoder that produces token-importance weights from a BERT-based model. It does **not** download a separate weight file at first use; it reuses the tokenizer already present in fastembed. This means enabling hybrid search adds essentially no model download overhead.

---

## When to Enable It

Enable hybrid search if your codebase has any of these characteristics:

| Situation | Why hybrid helps |
|-----------|-----------------|
| Code-heavy repos with many identifiers | Function names, struct names, and method names are exact-match tokens that dense models often miss |
| Error string searches | "failed to parse header: unexpected EOF" matches far better with sparse term overlap |
| Short/cryptic queries | Two-word queries like `cargo clippy` or `MCP broker` benefit from sparse boosting |
| Mixed technical + natural language | SPLADE handles jargon; dense handles intent |
| Searching for library/crate names | `serde_json`, `tokio`, `anyhow` — exact matches |

### When NOT to Enable It

- **Pure prose documentation** — if your docs are all natural-language sentences, dense-only usually wins.
- **Semantic-only queries** — "explain how the auth flow works" has no exact tokens to boost.
- **Very low-memory environments** — sparse scoring adds approximately one additional embedding pass per query.
- **Qdrant < 1.7** — hybrid collections require the Qdrant sparse vectors API introduced in 1.7. The installer checks this but older self-hosted instances may not support it.

---

## How to Enable

Add to your config file (`~/.the-one/config.json` or `<project>/.the-one/config.json`):

```json
{
  "hybrid_search_enabled": true
}
```

That's it. All other fields have working defaults. The `memory.search` tool behaves identically from the LLM's perspective — hybrid scoring is applied transparently.

**Requires a full reindex after enabling.** The collection must be recreated with sparse vector support:

```
maintain (action: reindex)
```

Or from the command line:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"maintain","arguments":{"action":"reindex"}}}' \
  | ~/.the-one/bin/the-one-mcp serve
```

---

## Configuration Fields

All hybrid search fields live at the top level of your config file.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hybrid_search_enabled` | bool | `false` | Enable hybrid dense+sparse search. When false, only dense cosine similarity is used. |
| `hybrid_dense_weight` | float | `0.7` | Weight applied to the dense score in the final fusion. Must be in [0.0, 1.0]. |
| `hybrid_sparse_weight` | float | `0.3` | Weight applied to the sparse score in the final fusion. Must be in [0.0, 1.0]. |
| `sparse_model` | string | `"bm25"` | Sparse model alias. Currently only `"bm25"` is supported (maps to SPLADE++Ensemble Distil in fastembed 5.13). |

The weights do not need to sum to 1.0, but values outside [0.0, 1.0] are clamped. Setting `hybrid_sparse_weight: 0.0` effectively disables the sparse signal while still using the hybrid collection format.

---

## How Weights Work

The dense cosine similarity score is already in [-1, 1]. The sparse dot-product score can be much larger and is normalized using a saturation function before fusion:

```
normalized_sparse = score / (score + 1.0)
```

This maps any positive sparse score into [0, 1) monotonically (higher sparse score → closer to 1). The saturated value is then combined with the dense score:

```
final_score = hybrid_dense_weight * dense_score + hybrid_sparse_weight * normalized_sparse_score
```

A document with both high semantic relevance and strong term overlap will rank highest. A document with only semantic similarity (dense high, sparse low) will still appear, but ranked below the fused results.

---

## Example Configurations

### Balanced (default when enabled)

Best for most code-heavy projects.

```json
{
  "hybrid_search_enabled": true,
  "hybrid_dense_weight": 0.7,
  "hybrid_sparse_weight": 0.3
}
```

### Term-heavy (prioritize exact matches)

Useful for querying by identifier name, error code, or API method signature.

```json
{
  "hybrid_search_enabled": true,
  "hybrid_dense_weight": 0.4,
  "hybrid_sparse_weight": 0.6
}
```

### Semantic-heavy (minimize sparse influence)

Useful when docs are prose-heavy and semantic similarity is more important.

```json
{
  "hybrid_search_enabled": true,
  "hybrid_dense_weight": 0.9,
  "hybrid_sparse_weight": 0.1
}
```

---

## Performance Impact

Hybrid search adds one sparse embedding pass per query. Sparse embedding is significantly faster than dense embedding (it runs a tokenizer, not a full transformer forward pass), so the practical overhead is roughly:

| Setup | Relative query time |
|-------|---------------------|
| Dense only | 1× baseline |
| Dense + sparse (hybrid) | ~1.5–2× baseline |
| Dense + sparse + reranking | ~3–5× baseline |

The additional cost is usually imperceptible for interactive use (single-user CLI). For batch-heavy workloads or very low-latency requirements, benchmark on your hardware.

---

## Combining with Reranking

Hybrid search and cross-encoder reranking are orthogonal and can be combined:

```json
{
  "hybrid_search_enabled": true,
  "hybrid_dense_weight": 0.7,
  "hybrid_sparse_weight": 0.3,
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual"
}
```

In this configuration:
1. Hybrid recall fetches `max_search_hits × rerank_fetch_multiplier` candidates using fused dense+sparse scores.
2. The cross-encoder reranks those candidates using full query-document attention.
3. The top `max_search_hits` results are returned.

This gives the best recall (hybrid) and the best precision (reranker) at the cost of more compute.

---

## Troubleshooting

### "Hybrid search requires Qdrant 1.7+"

Your Qdrant instance is too old to support sparse vectors. Upgrade Qdrant or disable hybrid search:

```json
{ "hybrid_search_enabled": false }
```

### Search results unchanged after enabling

You must reindex after enabling hybrid search. The Qdrant collection must be recreated with sparse vector support. Run `maintain (action: reindex)` from an AI session or via CLI (see above).

### Sparse scores all near 0.0

This is expected for long natural-language queries with high-frequency tokens. SPLADE gives high weight to rare or distinctive tokens. If your query is all common words ("what does this do?"), the sparse signal will be close to zero and dense cosine similarity will dominate — which is correct behavior.

### "sparse_model 'bm25' not found" error

Ensure your binary was compiled with fastembed 5.13 or later. Older builds do not include the SPLADE++ tokenizer. Rebuild from source or download the latest release binary.

### Weights not having expected effect

Check that `hybrid_search_enabled` is `true` in the resolved config. Use `config (action: export)` from an AI session to verify the full resolved configuration. Weights in the project config file override the global config.

---

## Implementation Details

For contributors or advanced users curious about the internals:

- Sparse embeddings are provided by `fastembed::SparseTextEmbedding` initialized with `SparseModel::SPLADEPPV1`.
- The `SparseEmbeddingProvider` trait in `crates/the-one-memory/src/sparse_embeddings.rs` mirrors the dense `EmbeddingProvider` interface.
- Qdrant hybrid collections store both a named dense vector (`"dense"`) and a named sparse vector (`"sparse"`) per point.
- At search time, `MemoryEngine::search_hybrid` runs both a dense nearest-neighbor query and a sparse dot-product query, then fuses the results in Rust before returning.
- The collection is created with sparse support only if `hybrid_search_enabled` is true at init time. Changing the flag requires dropping and recreating the collection, which `maintain reindex` handles.
