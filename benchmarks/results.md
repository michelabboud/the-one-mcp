# Retrieval Benchmark Results

_This file is overwritten by `cargo run --example retrieval_bench -p the-one-memory --features tree-sitter-chunker`._

_Run the benchmark locally against a live Qdrant to populate real numbers — see `benchmarks/README.md` for prerequisites._

## v0.9.0 — baseline (pending first run)

The benchmark harness is shipped in v0.9.0 but has not yet been run against a real
Qdrant instance with the full corpus. When you run it, replace this section with
the generated markdown table.

Expected shape:

| Config | Query Set | N | Recall@1 | Recall@5 | MRR | p50 (ms) | p95 (ms) |
|--------|-----------|---|----------|----------|-----|----------|----------|
| dense_only | exact | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| dense_only | semantic | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| dense_only | mixed | 5 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| dense_rerank | exact | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| dense_rerank | semantic | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| dense_rerank | mixed | 5 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| hybrid | exact | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| hybrid | semantic | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| hybrid | mixed | 5 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| full | exact | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| full | semantic | 10 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |
| full | mixed | 5 | _tbd_ | _tbd_ | _tbd_ | _tbd_ | _tbd_ |

### What to look for after running

The benchmark is meaningful if the relative ordering matches our design hypotheses:

1. **Hybrid should beat dense_only on exact queries.** Dense embeddings are famously bad
   at exact identifier lookups ("give me `parse_config`"); sparse BM25 should recover them.
   If hybrid doesn't improve Recall@1 on the `exact` query set by at least 0.2, the
   sparse weighting is wrong (or SPLADE isn't ingesting).

2. **dense_rerank should beat dense_only on semantic queries.** The cross-encoder sees
   both query and candidate together and is much better at ranking nuance. Expect a
   noticeable lift in MRR on the `semantic` set.

3. **Full pipeline should dominate on mixed queries.** The mixed set is designed to be
   where hybrid + rerank compounds — exact signal from sparse, semantic ranking from
   rerank, recall from dense.

4. **Latency rises with each layer.** dense_only is the fastest. Each layer (sparse,
   rerank) adds roughly 50-100 ms p95. If rerank adds more than ~200 ms p95, the top-k
   fetch is too wide or the reranker model is loading on every query.

If these expectations DON'T hold, the benchmark has done its job: we have a regression
or a design bug to investigate.
