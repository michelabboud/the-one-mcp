# Retrieval Benchmarks

This directory contains the retrieval-quality benchmark suite for the-one-mcp.

## What it measures

Four retrieval configurations — run against three query sets (exact, semantic, mixed):

| Config | Description |
|--------|-------------|
| `dense_only` | FastEmbed dense vectors only (the default configuration) |
| `dense_rerank` | Dense + cross-encoder reranker on top-15 |
| `hybrid` | Dense + SPLADE sparse fusion with 70/30 weighting |
| `full` | Dense + sparse + reranker — the "maximum quality" pipeline |

Metrics reported:

- **Recall@1** — fraction of queries where the top result contains an expected substring
- **Recall@5** — fraction where at least one of the top-5 results is correct
- **MRR** — mean reciprocal rank (0.0 = never found, 1.0 = always top-1)
- **p50 / p95 latency** in milliseconds per query (end-to-end, including embedding the query)

## Running the benchmark

The bench is an `examples/` binary that lives in `crates/the-one-memory/examples/retrieval_bench.rs`.
It is NOT part of the CI test suite because it requires a running Qdrant instance and can take
several minutes for first-run embedding downloads.

### Prerequisites

1. **Qdrant** — running locally (default URL `http://localhost:6333`)

   ```bash
   docker run -d --name qdrant-bench -p 6333:6333 qdrant/qdrant
   ```

2. **FastEmbed model cache** — the first run will download the embedding model
   (~90 MB for `all-MiniLM-L6-v2`). The reranker adds ~250 MB more.

### Run

From the repository root:

```bash
QDRANT_URL=http://localhost:6333 \
CORPUS_ROOT="$(pwd)" \
cargo run --release --example retrieval_bench -p the-one-memory \
    --features tree-sitter-chunker
```

The bench writes its output table to `benchmarks/results.md` and also prints it to stdout.

If Qdrant is unreachable, the bench prints a warning and exits cleanly — no CI failure.

### Environment variables

| Variable | Default | Meaning |
|----------|---------|---------|
| `QDRANT_URL` | `http://localhost:6333` | Qdrant HTTP endpoint |
| `CORPUS_ROOT` | Repo root (from `CARGO_MANIFEST_DIR`) | Directory containing `docs/` and `crates/` |
| `THE_ONE_EMBEDDING_MODEL` | `all-MiniLM-L6-v2` | FastEmbed model name |

## Query corpora

The three query sets are hand-curated against the-one-mcp's actual source tree content
and live as compile-time constants in `retrieval_bench.rs`:

- **Exact-match queries (10)** — function names, type names, config fields, feature flags.
  These probe whether sparse/BM25 search actually helps for literal lookups.
- **Semantic queries (10)** — natural-language descriptions of how the code works.
  These probe whether dense embeddings capture intent.
- **Mixed queries (5)** — hybrid exact+semantic queries combining identifiers with behavioural
  descriptions. These are where hybrid search should shine over either pure dense or pure sparse.

Each `QueryCase` lists one or more `expected` substrings. A retrieval result is considered
**correct** if ANY of the top-5 chunks contain ANY of the expected substrings. This is
deliberately forgiving — the benchmark is a *comparison ruler*, not a perfect-ground-truth test.

## Maintaining the queries

If source drift causes retrieval misses across every configuration, update the query lists
rather than tuning retrieval to chase stale labels. Good signals that a query needs updating:

- The expected function/type was renamed
- All 4 configurations return 0.0 Recall@1 for a query
- A new feature added symbols that deserve their own exact-match query

## Limitations (v0.9.0 initial release)

- **Rust source ingestion is a stub** — the benchmark currently only ingests `.md` files via
  `ingest_markdown_tree`. The corpus scanner for `.rs` files counts files but doesn't actually
  feed them through the engine (there's no public `ingest_source_tree` yet). Expected to be
  closed in a follow-up release; for now the semantic queries targeting Rust code will show
  lower recall than the final pipeline will achieve.

- **RetrievalMode is fixed to `Naive`** — the benchmark runs all queries in dense-vector mode.
  `Hybrid` retrieval mode (which adds graph traversal) is not measured because the knowledge
  graph isn't built during ingest.

- **Single embedding model** — only `all-MiniLM-L6-v2` is measured. A future release will
  add a model-tier sweep.
