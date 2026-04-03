# The-One MCP: Production Overhaul Design Spec

**Date:** 2026-04-03
**Status:** Approved
**Scope:** Full production-grade overhaul — transport, RAG, docs management, LLM routing, configurable limits

---

## Goals

1. Make the MCP actually usable as an MCP server (transport layer)
2. Production-grade RAG with real embeddings (local + API)
3. Managed document system with CRUD, soft-delete, auto-sync
4. Intelligent nano LLM provider pool with health checks and routing policies
5. Configurable hard limits exposed through layered config
6. Async broker throughout
7. Fix all known issues from code review

---

## Non-Goals

- Multi-user / auth on the MCP server itself
- Distributed deployment
- Custom embedding model training
- Real-time collaborative editing of managed docs

---

## Section 1: MCP Transport Layer

Three transport modes in one binary:

```bash
the-one-mcp serve                              # defaults to stdio
the-one-mcp serve --transport sse --port 3000   # HTTP + SSE
the-one-mcp serve --transport stream --port 3000 # streamable HTTP
```

### Architecture

A `Transport` trait with three implementations:

```rust
#[async_trait]
pub trait Transport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError>;
}

pub struct StdioTransport;
pub struct SseTransport { port: u16 }
pub struct StreamableHttpTransport { port: u16 }
```

All three follow the same flow: deserialize JSON-RPC 2.0 -> dispatch to `McpBroker` -> serialize response. The broker is transport-agnostic.

### Transports

**stdio:** Reads newline-delimited JSON from stdin, writes to stdout. Standard MCP transport for Claude Code and Codex.

**SSE:** axum HTTP server. `POST /message` for client-to-server requests. `GET /sse` for server-to-client events. Session management via session IDs.

**Streamable HTTP:** axum HTTP server with bidirectional streaming per the MCP streamable HTTP specification. `POST /mcp` endpoint with `Accept: text/event-stream` for streaming responses.

### MCP Protocol Handshake

- `initialize` -> returns server capabilities, tool list, protocol version
- `notifications/initialized` -> client confirms ready
- `tools/list` -> returns tool definitions with JSON Schema input schemas
- `tools/call` -> dispatches to broker methods, returns results

### Binary

```
crates/the-one-mcp/src/bin/the-one-mcp.rs
```

Uses `clap` for CLI argument parsing:

```
the-one-mcp serve [--transport stdio|sse|stream] [--port 3000] [--project-root .] [--project-id auto]
```

---

## Section 2: Nano LLM Provider Pool

### Multi-Provider Configuration

Up to 5 OpenAI-compatible providers with intelligent routing:

```json
{
  "nano_providers": [
    {
      "name": "local-ollama",
      "base_url": "http://localhost:11434/v1",
      "model": "qwen2:0.5b",
      "api_key": null,
      "timeout_ms": 500,
      "enabled": true
    },
    {
      "name": "litellm-proxy",
      "base_url": "http://localhost:4000/v1",
      "model": "gpt-4o-mini",
      "api_key": "sk-...",
      "timeout_ms": 1000,
      "enabled": true
    },
    {
      "name": "cloud-fallback",
      "base_url": "https://api.openai.com/v1",
      "model": "gpt-4o-mini",
      "api_key": "sk-...",
      "timeout_ms": 2000,
      "enabled": true
    }
  ],
  "nano_routing_policy": "priority"
}
```

Works with: Ollama, LM Studio, LiteLLM, vLLM, LocalAI, Groq, Together, OpenAI, or any OpenAI-compatible endpoint.

### Routing Policies

| Policy | Behavior |
|--------|----------|
| `priority` (default) | Try providers in config order. First healthy one wins. If it fails, try next. |
| `round_robin` | Rotate across healthy providers. Spreads load evenly. |
| `latency` | Pick the provider with lowest recent p50 latency. Rolling window of last 20 calls. |

### Health Check — Pre-Flight Before Every Classification

```
For each provider (in policy order):
  1. Is it enabled?                   -> skip if no
  2. Is it in cooldown?              -> skip if yes
  3. TCP connect check (<50ms)       -> mark unhealthy if fails, try next
  4. Send classification request     -> use response if valid
  5. Parse failed or timeout?        -> mark unhealthy, try next

All providers exhausted? -> Fall back to rules-only routing (silent, no error)
```

### Health Tracking Per Provider

```rust
pub struct ProviderHealth {
    pub status: ProviderStatus,           // healthy | unhealthy | unknown
    pub last_check_epoch_ms: u64,
    pub consecutive_failures: u32,
    pub cooldown_until_epoch_ms: u64,
    pub p50_latency_ms: u64,              // rolling avg, last 20 calls
    pub total_calls: u64,
    pub total_errors: u64,
}
```

### Cooldown Strategy

- 1 failure -> 5s cooldown
- 2 consecutive -> 15s
- 3+ consecutive -> 60s
- Successful call resets to healthy, clears cooldown

### Classification Prompt

~50 tokens sent to the nano model:

```
Classify this request into exactly one category.
Respond with ONLY one word: search_docs, run_tool, configure_system, or unknown.

Request: "{user_query}"
```

Parse single-word response. If response is garbage, treat as provider error and try next.

### Observability

Exposed via `metrics.snapshot`:
- Per-provider: calls, errors, avg latency, current status
- Pool-level: total classifications, total fallbacks-to-rules, active healthy provider count

---

## Section 3: Production RAG System

### Dual Embedding Architecture

```json
{
  "embedding_provider": "local",
  "embedding_model": "all-MiniLM-L6-v2",
  "embedding_api_base_url": null,
  "embedding_api_key": null,
  "embedding_dimensions": 384
}
```

**Local provider (`fastembed-rs`):**
- Model: `all-MiniLM-L6-v2` (default) — 384 dimensions
- Pure Rust via ONNX Runtime
- Offline, no API calls, no cost
- ~30ms per chunk on modern hardware
- CPU-bound: wrapped in `spawn_blocking`

**API provider:**
- Any OpenAI-compatible `/v1/embeddings` endpoint
- Configurable: base_url, api_key, model, dimensions
- Works with: OpenAI, Voyage, Cohere, LiteLLM, etc.

### Chunking Strategy

```
Markdown file
  |
  v
1. Parse heading hierarchy (# / ## / ### etc.)
  |
  v
2. Split into semantic sections (by heading)
  |
  v
3. For each section:
   - If <= max_chunk_tokens (default 512): one chunk
   - If > max_chunk_tokens: split on paragraph boundaries
   - Never split mid-paragraph, mid-code-block, or mid-list
  |
  v
4. Each chunk carries metadata:
   - source_path (relative to docs root)
   - heading_hierarchy (e.g., ["Architecture", "Auth", "OAuth Flow"])
   - chunk_index within file
   - byte_offset and byte_length in original file
   - content_hash (SHA-256 of chunk content, for change detection)
  |
  v
5. Embed each chunk -> store vector + metadata in Qdrant
```

### Vector Storage — Qdrant

- One collection per project: `the_one_{sanitized_project_id}`
- Vector dimensions match embedding provider (384 for local, configurable for API)
- Payload fields indexed: `source_path`, `heading`, `chunk_index`
- Distance metric: Cosine
- HNSW index: `m=16, ef_construct=100`

### Search Pipeline

```
User query
  |
  v
1. Embed query -> vector
  |
  v
2. Qdrant search: top_k candidates (default 5)
   Filter: score >= search_score_threshold (default 0.3)
  |
  v
3. For each hit: return chunk content, source_path, heading, score
  |
  v
4. Response includes source file paths for follow-up docs.get calls
```

### Incremental Re-Indexing

- On `project.refresh`: detect changed/new/deleted files via content hash comparison
- Only re-embed changed chunks (compare stored content_hash vs current)
- Full reindex available via `docs.reindex` tool
- Batch embedding: up to `max_embedding_batch_size` (default 64) chunks per batch

---

## Section 4: Managed Documents System

### Dual-Mode Architecture

```
<project>/.the-one/docs/           <- managed folder (read-write, CRUD, auto-indexed)
+-- decisions/
|   +-- 2026-04-03-db-choice.md
+-- architecture/
|   +-- auth.md
+-- .trash/                        <- soft-deleted files (not indexed)
    +-- decisions/
        +-- old-file.md

<project>/docs/                    <- external docs (read-only ingestion)
```

**Managed folder:** The broker owns `<project>/.the-one/docs/`. Files are real `.md` files in real folders. Human-readable, editable outside the broker. The folder structure is the source of truth.

**External docs:** User can configure an external docs directory (e.g., project's `docs/`). These are ingested read-only into the RAG index. No CRUD operations.

### MCP Tool Surface

| Tool | Description |
|------|-------------|
| `docs.create` | Create new markdown file in managed folder. Auto-creates subdirectories. Auto-indexes into RAG. |
| `docs.update` | Overwrite file content. Re-indexes changed chunks only. |
| `docs.delete` | Soft-delete: moves to `.trash/` preserving folder structure. Removes from RAG index. |
| `docs.get` | Return full original markdown file as-is. Works for managed and external docs. |
| `docs.get_section` | Return specific heading section, bounded by `max_raw_section_bytes`. |
| `docs.list` | List full folder tree (managed + external). Shows path, size, last modified. |
| `docs.move` | Move/rename a doc within managed folder. Updates RAG index. |
| `docs.trash.list` | List contents of `.trash/`. |
| `docs.trash.restore` | Move file from `.trash/` back to original location. Re-indexes. |
| `docs.trash.empty` | Permanently delete all files in `.trash/`. |

### Auto-Sync on Refresh

```
project.refresh called
  |
  v
1. Scan managed folder for file changes (mtime + content hash)
  |
  v
2. New files: chunk -> embed -> index
3. Modified files: re-chunk -> re-embed changed chunks -> update index
4. Missing files (deleted outside broker): remove from index
  |
  v
5. Scan external docs root (if configured) same way
  |
  v
6. Report: { new: N, updated: N, removed: N, unchanged: N }
```

### Soft Delete

- `docs.delete "decisions/old-file.md"` moves to `.trash/decisions/old-file.md`
- Preserves full path structure under `.trash/`
- Removes all chunks from RAG index
- Trash contents are NOT indexed (invisible to search)
- `docs.trash.restore` moves back to original location and re-indexes
- `docs.trash.empty` permanently deletes all files in `.trash/`

### Validation Rules

- File must be `.md` extension
- File size <= `max_doc_size_bytes` (default 100KB)
- Total managed docs <= `max_managed_docs` (default 500)
- Path must be within managed folder (no path traversal: `../` rejected)
- File names: alphanumeric, hyphens, underscores, dots, forward slashes only

---

## Section 5: Configurable Limits

### Full Limits Block in Config

```json
{
  "limits": {
    "max_tool_suggestions": 5,
    "max_search_hits": 5,
    "max_raw_section_bytes": 24576,
    "max_enabled_families": 12,
    "max_doc_size_bytes": 102400,
    "max_managed_docs": 500,
    "max_embedding_batch_size": 64,
    "max_chunk_tokens": 512,
    "max_nano_timeout_ms": 2000,
    "max_nano_retries": 3,
    "max_nano_providers": 5,
    "search_score_threshold": 0.3
  }
}
```

### Precedence

Same layered precedence as all config:

```
Hardcoded defaults -> global config -> project config -> env vars -> runtime overrides
```

### Environment Variable Mapping

Every limit has a `THE_ONE_LIMIT_*` env var:

```bash
THE_ONE_LIMIT_MAX_SEARCH_HITS=10
THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES=49152
THE_ONE_LIMIT_MAX_CHUNK_TOKENS=1024
# etc.
```

### Validation Bounds

| Limit | Floor | Ceiling | Default |
|-------|-------|---------|---------|
| `max_tool_suggestions` | 1 | 50 | 5 |
| `max_search_hits` | 1 | 100 | 5 |
| `max_raw_section_bytes` | 1024 | 1,048,576 | 24,576 |
| `max_enabled_families` | 1 | 100 | 12 |
| `max_doc_size_bytes` | 1024 | 10,485,760 | 102,400 |
| `max_managed_docs` | 10 | 10,000 | 500 |
| `max_embedding_batch_size` | 1 | 256 | 64 |
| `max_chunk_tokens` | 64 | 2,048 | 512 |
| `max_nano_timeout_ms` | 100 | 10,000 | 2,000 |
| `max_nano_retries` | 0 | 10 | 3 |
| `max_nano_providers` | 1 | 10 | 5 |
| `search_score_threshold` | 0.0 | 1.0 | 0.3 |

Out-of-bounds values are clamped to the nearest bound with a tracing warning.

### Admin UI

The `/config` page gains a "Limits" section with all fields editable. `POST /api/config` accepts limits updates with the same validation.

---

## Section 6: Async Broker Overhaul

### Current State -> Target State

| Component | Current | Target |
|-----------|---------|--------|
| McpBroker methods | `fn` | `async fn` |
| memory_by_project | `std::sync::Mutex` | `tokio::sync::RwLock` |
| session_approvals | `std::sync::Mutex` | `tokio::sync::RwLock` |
| reqwest client | `reqwest::blocking::Client` | `reqwest::Client` (async) |
| SQLite calls | direct | `spawn_blocking` wrapper |
| Embedding (fastembed) | N/A | `spawn_blocking` (CPU-bound) |
| Nano provider calls | keyword stubs | async HTTP via `reqwest::Client` |

### Runtime

```rust
#[tokio::main]
async fn main() {
    // Parse CLI args
    // Load config
    // Initialize broker
    // Start transport
}
```

Multi-thread tokio runtime. Both MCP server and admin UI use the same runtime.

---

## Section 7: Fixes & Cleanup

| Issue | Fix |
|-------|-----|
| Failing config test (env var pollution) | Isolate test with scoped env guard using `temp_env` crate |
| No tool execution engine | Add `ToolExecutor` trait + subprocess executor for CLI tools |
| No partial-init rollback | Add cleanup-on-failure in `project_init` (remove `.the-one/` on error) |
| Hash-based 16-dim embeddings | Replaced by `fastembed-rs` (384-dim) + API provider |
| Stub nano providers | Replaced by OpenAI-compatible HTTP provider pool with health checks |
| No MCP transport | Three transports: stdio, SSE, streamable HTTP |
| Blocking reqwest | Async reqwest throughout |
| `std::sync::Mutex` | `tokio::sync::RwLock` where needed |
| Hardcoded policy limits | Configurable via layered config with validation bounds |

---

## Section 8: Updated MCP Tool Surface

### Project Lifecycle (3 tools)
- `project.init` — detect project, create state, index docs
- `project.refresh` — re-fingerprint, sync docs, refresh profile
- `project.profile.get` — return cached project profile

### Knowledge / RAG (2 tools)
- `memory.search` — semantic search across all indexed docs
- `memory.fetch_chunk` — fetch specific chunk by ID

### Documents (10 tools)
- `docs.create` — create markdown file in managed folder
- `docs.update` — update existing markdown file
- `docs.delete` — soft-delete to `.trash/`
- `docs.get` — return full original file as-is
- `docs.get_section` — return bounded heading section
- `docs.list` — folder tree listing with sizes and timestamps
- `docs.move` — rename/move within managed folder
- `docs.trash.list` — list trash contents
- `docs.trash.restore` — restore from trash, re-index
- `docs.trash.empty` — permanently empty trash

### Tools (4 tools)
- `tool.suggest` — suggest capabilities by query
- `tool.search` — search capabilities
- `tool.enable` — enable tool family
- `tool.run` — execute tool with approval gate

### Config & Observability (5 tools)
- `config.export` — full config including limits and provider pool status
- `config.update` — update project config and limits
- `metrics.snapshot` — broker + provider pool metrics
- `audit.events` — query audit trail
- `docs.reindex` — force full re-indexing of all docs

**Total: 24 tools**

---

## New Dependencies

| Dependency | Purpose | Notes |
|-----------|---------|-------|
| `fastembed` | Local ONNX embeddings | ~50-100MB binary size increase with bundled model |
| `clap` | CLI argument parsing | Standard for Rust CLIs |
| `async-trait` | Async trait support | Until Rust stabilizes async traits fully |
| `tokio-util` | Codec for stdio framing | JSON line codec for stdin/stdout |
| `temp-env` | Scoped env var mutation in tests | Dev dependency only |

---

## Config Example (Complete)

```json
{
  "provider": "local",
  "log_level": "info",

  "qdrant_url": "http://127.0.0.1:6334",
  "qdrant_api_key": null,
  "qdrant_ca_cert_path": null,
  "qdrant_tls_insecure": false,
  "qdrant_strict_auth": true,

  "embedding_provider": "local",
  "embedding_model": "all-MiniLM-L6-v2",
  "embedding_api_base_url": null,
  "embedding_api_key": null,
  "embedding_dimensions": 384,

  "nano_routing_policy": "priority",
  "nano_providers": [
    {
      "name": "local-ollama",
      "base_url": "http://localhost:11434/v1",
      "model": "qwen2:0.5b",
      "api_key": null,
      "timeout_ms": 500,
      "enabled": true
    }
  ],

  "external_docs_root": null,

  "limits": {
    "max_tool_suggestions": 5,
    "max_search_hits": 5,
    "max_raw_section_bytes": 24576,
    "max_enabled_families": 12,
    "max_doc_size_bytes": 102400,
    "max_managed_docs": 500,
    "max_embedding_batch_size": 64,
    "max_chunk_tokens": 512,
    "max_nano_timeout_ms": 2000,
    "max_nano_retries": 3,
    "max_nano_providers": 5,
    "search_score_threshold": 0.3
  }
}
```
