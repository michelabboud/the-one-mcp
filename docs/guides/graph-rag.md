# Graph RAG Guide

> v0.13.1 achieves **full LightRAG parity** for Graph RAG — entity name
> normalization, entity/relation description vector store, query keyword
> extraction, description summarization, gleaning, and canvas-based graph
> visualization all shipped. This guide covers what Graph RAG is, how to
> enable it, and the trade-offs vs. pure vector search.

## What is Graph RAG?

Traditional RAG (Retrieval-Augmented Generation) works by:

1. Chunking documents
2. Embedding each chunk into a dense vector
3. At query time, embedding the query and finding the nearest chunks
4. Stuffing those chunks into the LLM prompt

**Graph RAG** adds a second layer: it uses an LLM to extract **entities**
(people, technologies, organizations, concepts) and **relations** (who/what
connects to what) from each chunk, builds a knowledge graph from those
extractions, and uses graph traversals alongside vector search to find
context that pure vector search would miss.

The technique was popularized by Microsoft's GraphRAG paper and HKU's
[LightRAG](https://github.com/hkuds/lightrag). the-one-mcp's implementation
borrows the best ideas from both.

### Why bother?

Pure vector RAG has well-known weaknesses:

- **Exact identifier lookups** (function names, product codes) get buried
  because semantic embeddings smooth over rare tokens
- **Multi-hop questions** like "which tools does our auth system depend
  on?" need more than similarity — they need actual connection information
- **Thematic queries** like "what do we know about rate limiting?" pull up
  chunks that mention the phrase but miss chunks that describe specific
  algorithms without using the theme word

Graph RAG addresses all three via four retrieval modes (see below).

## Current implementation state (v0.13.1 — full LightRAG parity)

| Component | Status | Location |
|-----------|--------|----------|
| `Entity`, `Relation`, `KnowledgeGraph` types | ✅ shipped | `crates/the-one-memory/src/graph.rs` |
| `merge_extraction`, load/save JSON | ✅ shipped | `crates/the-one-memory/src/graph.rs` |
| `build_extraction_prompt` / `parse_extraction_response` | ✅ shipped | `crates/the-one-memory/src/graph.rs` |
| **LLM extraction pipeline** | ✅ v0.13.0 | `crates/the-one-memory/src/graph_extractor.rs` |
| **`maintain: graph.extract` MCP action** | ✅ v0.13.0 | `crates/the-one-mcp/src/transport/jsonrpc.rs` |
| **`maintain: graph.stats` MCP action** | ✅ v0.13.0 | same |
| **`/graph` admin UI page** | ✅ v0.13.0 | `crates/the-one-ui/src/lib.rs` |
| **`/api/graph` JSON endpoint** | ✅ v0.13.0 | same |
| 4 retrieval modes (naive/local/global/hybrid) | ✅ shipped | `crates/the-one-memory/src/lib.rs` |
| **Entity name normalization** | ✅ **v0.13.1** | `graph.rs` — `normalize_entity_name()` |
| **Entity/relation description vector store** | ✅ **v0.13.1** | `qdrant.rs` — 3 new Qdrant collections per project |
| **Description summarization (map-reduce)** | ✅ **v0.13.1** | `graph_extractor.rs` — `summarize_description()` |
| **Query keyword extraction (LightRAG core)** | ✅ **v0.13.1** | `graph_extractor.rs` — `extract_query_keywords()` |
| **Gleaning / continue-extraction pass** | ✅ **v0.13.1** | `graph_extractor.rs` — `extract_with_gleaning()` |
| **Canvas force-directed graph visualization** | ✅ **v0.13.1** | `/graph` page — zero-dep vanilla JS renderer |

## Enabling extraction

Extraction is **off by default** — it requires an LLM endpoint and costs
real tokens. To enable it, set these environment variables before starting
the broker:

```bash
# Turn it on
export THE_ONE_GRAPH_ENABLED=true

# OpenAI-compatible endpoint. Works with Ollama, LM Studio, LiteLLM, LocalAI,
# vLLM, OpenAI proper, and anything else that speaks /v1/chat/completions.
export THE_ONE_GRAPH_BASE_URL=http://localhost:11434/v1

# Model name. For Ollama: llama3.2, mistral, etc. For OpenAI: gpt-4o-mini.
export THE_ONE_GRAPH_MODEL=llama3.2

# Optional — only needed for hosted APIs that require auth
export THE_ONE_GRAPH_API_KEY=sk-...

# Optional — comma-separated entity types to extract
export THE_ONE_GRAPH_ENTITY_TYPES=person,organization,location,technology,concept,event

# Optional — cap chunks per extraction run (default 50)
export THE_ONE_GRAPH_MAX_CHUNKS=50
```

Then restart the broker so the new env vars are picked up.

> **Why env vars instead of config file?** v0.13.x deliberately keeps graph
> extraction out of `config.json` to avoid touching the four different
> config structs (FileConfig / ProjectOverlay / ProjectConfigUpdate /
> AppConfig). Proper config fields will land in v0.14.0. Env vars work
> today and let you iterate quickly.

## Running extraction

### From an AI CLI (recommended)

Ask the LLM:

> "Extract entities from my indexed docs"

The LLM calls `maintain` with action `graph.extract`:

```json
{
  "name": "maintain",
  "arguments": {
    "action": "graph.extract",
    "params": {
      "project_root": "/abs/path",
      "project_id": "my-project"
    }
  }
}
```

Response:

```json
{
  "chunks_processed": 47,
  "chunks_skipped": 3,
  "entities_added": 82,
  "relations_added": 56,
  "total_entities": 82,
  "total_relations": 56,
  "errors": ["http chunk-42: HTTP 500: ..."],
  "disabled_reason": null
}
```

If extraction is disabled, `disabled_reason` explains what's missing and
the other counters are zero. No error is raised.

### From the admin UI

Open `http://127.0.0.1:8788/graph`. If the graph is empty you'll see an
empty-state card with setup instructions. Once entities are extracted, the
page shows entity count, relation count, top entity types, and a link to
the raw JSON at `/api/graph`.

### From the command line

```bash
# Trigger extraction directly via JSON-RPC over stdio
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"maintain","arguments":{"action":"graph.extract","params":{"project_root":"'$PWD'","project_id":"my-project"}}}}' | the-one-mcp serve
```

### Check stats without running extraction

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"maintain","arguments":{"action":"graph.stats","params":{"project_root":"'$PWD'","project_id":"my-project"}}}}' | the-one-mcp serve
```

Returns:

```json
{
  "entity_count": 82,
  "relation_count": 56,
  "graph_enabled": true,
  "extraction_configured": true,
  "file_exists": true
}
```

## Four retrieval modes

The `MemoryEngine::search` method accepts a `RetrievalMode` that controls
how the query is executed. Defaults to `Hybrid`.

| Mode | What it searches | Best for |
|------|------------------|----------|
| `Naive` | Dense vectors over chunks (+ optional sparse hybrid + rerank) | Free-text semantic queries, "explain X to me" |
| `Local` | Entity graph — matches query keywords against entity names, returns their source chunks | "What is X?" queries where X is a specific entity name |
| `Global` | Relation graph — traverses relations to pull connected chunks | "How does X relate to Y?" thematic / multi-hop queries |
| `Hybrid` (default) | Vector search + graph search, fused and deduplicated | General-purpose; best overall quality when graph is populated |

The mode is controlled per-request. Most MCP clients use `Hybrid` by
default, which gracefully degrades to pure vector search if the graph is
empty.

## Storage model

Entities and relations are persisted to a single JSON file per project:

```
<project-root>/.the-one/knowledge_graph.json
```

Schema:

```json
{
  "entities": [
    {
      "name": "MemoryEngine",
      "entity_type": "technology",
      "description": "Rust struct in the-one-memory that owns chunks, embeddings, graph, and reranker. Offers vector and graph search.",
      "source_chunks": ["chunk-12", "chunk-47"]
    }
  ],
  "relations": [
    {
      "source": "MemoryEngine",
      "target": "KnowledgeGraph",
      "relation_type": "owns",
      "description": "MemoryEngine holds a KnowledgeGraph field used for Local/Global/Hybrid retrieval modes.",
      "weight": 1.0,
      "source_chunks": ["chunk-12"]
    }
  ]
}
```

The file is loaded into memory at project-init time and when `graph.extract`
completes, so the Local/Global/Hybrid retrieval modes can see new entities
immediately without a server restart.

## What the extraction prompt looks like

The prompt is built by `crate::graph::build_extraction_prompt`:

```
Extract entities and relationships from the following text.

Entity types to look for: person, organization, location, technology, concept, event

For each entity, provide:
- name: The entity name
- type: One of the entity types above
- description: Brief description

For each relationship, provide:
- source: Source entity name
- target: Target entity name
- type: Relationship type (e.g., "uses", "depends_on", "contains", "implements")
- description: Brief description

Output as JSON:
{
  "entities": [{"name": "...", "type": "...", "description": "..."}],
  "relations": [{"source": "...", "target": "...", "type": "...", "description": "..."}]
}

Text:
<chunk content>
```

This is a simpler format than LightRAG's delimiter-tuple prompt. We chose
JSON because:

- Modern LLMs (Llama 3, Mistral, GPT-4) produce it reliably
- `serde_json::from_str` handles parsing with zero extra code
- JSON tolerates the LLM wrapping its output in ```json ... ``` code fences
  (the parser unwraps them automatically)

The trade-off: LightRAG's delimiter format is more robust to LLM formatting
drift on weaker models. If you're using a small local model and getting
parse errors, consider upgrading to at least a 7B parameter model.

## Running on Ollama locally

```bash
# Install and pull a model
brew install ollama
ollama pull llama3.2

# Point the-one-mcp at it
export THE_ONE_GRAPH_ENABLED=true
export THE_ONE_GRAPH_BASE_URL=http://localhost:11434/v1
export THE_ONE_GRAPH_MODEL=llama3.2

# Restart the broker and run extraction
the-one-mcp serve &
# Ask Claude: "Extract entities from my project docs"
```

Extraction time on a typical project (~50 chunks):

- `llama3.2:3b` on M1 MacBook Pro: ~3 minutes
- `llama3.2:8b` on M2 Max: ~2 minutes
- `gpt-4o-mini` via OpenAI: ~30 seconds (but costs real money)

## Limitations and trade-offs

### Known limitations (after v0.13.1 parity pass)

1. ~~**No entity name normalization.**~~ **Fixed in v0.13.1.** `normalize_entity_name()` collapses `Rust`/`rust`/`RUST` into one entity.
2. ~~**No description summarization.**~~ **Fixed in v0.13.1.** Descriptions exceeding `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` (default 2000 chars) are map-reduced via an LLM call.
3. **Relation dedup is fragile.** Relations `A→B` and `B→A` are stored as
   distinct edges even when they're semantically the same. Normalize by
   sorting endpoints if this matters for your use case.
4. ~~**No gleaning pass.**~~ **Fixed in v0.13.1.** `THE_ONE_GRAPH_GLEANING_ROUNDS=1` (default) runs a second "what did you miss?" pass per chunk.
5. ~~**Local mode uses keyword search.**~~ **Fixed in v0.13.1.** Entity/relation descriptions are now embedded into dedicated Qdrant vector collections. `search_graph` routes through them when available.
6. **No incremental extraction.** Every `graph.extract` call processes
   chunks from scratch. Adding new chunks re-extracts everything (bounded
   by `THE_ONE_GRAPH_MAX_CHUNKS`). Planned for v0.14.0.

### When NOT to use Graph RAG

- **Small projects (<20 chunks)** — vector search works fine at this size,
  graph extraction cost isn't justified
- **Pure prose knowledge bases** (articles, blog posts) — entity extraction
  on narrative text tends to pull out proper nouns that aren't actually
  useful anchors for retrieval
- **Offline CI environments** — extraction requires a reachable LLM

Graph RAG shines on **technical documentation** where the entities
(functions, types, services, APIs) ARE the things users want to find.

## Cost considerations

For a typical medium project (say, 100 chunks of 500 tokens each):

| Setup | Tokens in | Tokens out | $ cost |
|-------|-----------|------------|--------|
| Ollama llama3.2:3b | ~50k | ~10k | Free (local CPU/GPU) |
| gpt-4o-mini | ~50k | ~10k | ~$0.01 |
| gpt-4o | ~50k | ~10k | ~$0.40 |

Extraction is a one-time cost per corpus. Incremental re-extraction (once
implemented) will make ongoing cost marginal.

## Relation to LightRAG

LightRAG is a research implementation in Python. Our implementation is a
pragmatic production port in Rust with a deliberately narrower feature set:

| Feature | LightRAG | the-one-mcp v0.13.1 |
|---------|----------|-----------|
| Entity / relation extraction | ✅ (delimiter tuples) | ✅ (JSON format) |
| Four retrieval modes | ✅ | ✅ |
| Entity description vector store | ✅ | ✅ (3 Qdrant collections per project) |
| Keyword extraction for queries | ✅ | ✅ (LLM-driven high/low split) |
| Graph visualization | ✅ (Sigma.js) | ✅ (Canvas force-directed, zero deps) |
| Gleaning (continue-extraction) | ✅ | ✅ (configurable rounds, early termination) |
| Description summarization | ✅ | ✅ (LLM map-reduce above threshold) |
| Entity name normalization | ✅ | ✅ (title-case + acronym preservation + dedup) |
| Languages | Python | **Rust** |
| Storage | NetworkX + multiple vector stores | **Single JSON + 3 Qdrant collections** |

As of v0.13.1, the-one-mcp has **full feature parity** with LightRAG's
core Graph RAG pipeline, reimplemented in Rust with a production-grade
admin UI.

## Roadmap (v0.14.0+)

- **v0.14.0** — graph extraction config fields in `config.json` (instead of
  env vars), incremental extraction (only new chunks), automatic extraction
  on ingest
- **v0.15.0** — multi-hop relation traversal, entity merge UI, graph
  pruning for concept drift, Sigma.js upgrade for larger graphs

## See also

- [API Reference — maintain actions](api-reference.md#maintain-actions)
- [Auto-Indexing Guide](auto-indexing.md)
- [LightRAG upstream](https://github.com/hkuds/lightrag) — the original
  research implementation we mirror
