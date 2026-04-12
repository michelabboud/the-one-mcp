# The-One MCP Complete Guide

> **Current version**: v0.16.0 GA. The v0.16.0 line introduced
> multi-backend support — see § 1.1 "Multi-backend architecture
> (v0.16.0+)" below. The default deployment (SQLite + Qdrant) is
> unchanged; everything here still works for that path.

## 1. Overview

`the-one-mcp` is a Rust MCP broker that acts as a smart intermediary between AI coding assistants and your project. Works with **Claude Code**, **Gemini CLI**, **OpenCode**, and **Codex** — same server, same protocol, client-aware tool loading.

It provides:

- **Project lifecycle** — detect languages, frameworks, and risk profile; cache results via fingerprinting
- **Semantic memory** — production-grade RAG with fastembed 5.13 (1024-dim ONNX) or API embeddings. Vector storage is pluggable: Qdrant (default), pgvector split-pool (v0.16.0 Phase 2), pgvector combined-with-state (v0.16.0 Phase 4), or Redis/RediSearch.
- **State store** — SQLite (default), Postgres split-pool (v0.16.0 Phase 3), or Postgres combined-with-pgvector (v0.16.0 Phase 4) for project profiles, approvals, audit events, conversation sources, AAAK lessons, diary entries, and navigation nodes.
- **Image search** — multimodal image indexing and search with optional OCR
- **Reranking** — optional cross-encoder reranking for higher-precision search results
- **Code-aware chunking** — AST chunkers for 13 languages with symbol metadata and fallback behavior
- **Watcher auto-reindex** — background file watcher re-ingests changed markdown and image files
- **MemPalace operations** — profile presets (`off/core/full`), AAAK lessons, diary workflows, navigation primitives, and hook capture
- **Managed documents** — full CRUD for markdown files with soft-delete, trash, and auto-sync
- **Policy-gated tools** — risk-tier approval gates (once/session/forever) with headless deny-by-default
- **Intelligent routing** — rules-first with optional nano LLM provider pool (priority/round-robin/latency)
- **Token efficiency** — configurable limits on search results, doc sizes, tool suggestions

### 1.1 Multi-backend architecture (v0.16.0+)

Both state and vector storage became pluggable in v0.16.0 via two parallel traits — `the_one_core::state_store::StateStore` and `the_one_memory::vector_backend::VectorBackend` — and a four-variable env surface that operators toggle without rebuilding anything except when opting into optional Cargo features.

```bash
# Selection env vars (defaults shown):
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>  # default: sqlite
THE_ONE_STATE_URL=<dsn>                                                       # required if TYPE != sqlite
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>  # default: qdrant
THE_ONE_VECTOR_URL=<dsn>                                                      # required if TYPE != qdrant
```

Shipping today:

| State | Vector | Cargo features | Phase |
|---|---|---|---|
| `sqlite` (default) | `qdrant` (default) | (none) | v0.15.x baseline |
| `sqlite` | `pgvector` | `pg-vectors` | v0.16.0 Phase 2 |
| `postgres` | `qdrant` | `pg-state` | v0.16.0 Phase 3 |
| `postgres` | `pgvector` | `pg-state,pg-vectors` | v0.16.0 Phase 3 (split pools, two independent sqlx pools) |
| `postgres-combined` | `postgres-combined` | `pg-state,pg-vectors` | v0.16.0 Phase 4 (ONE shared sqlx pool, byte-identical URLs) |

Also shipping in v0.16.0 GA:

| `redis` | `qdrant` | `redis-state` | v0.16.0 Phase 5 (cache or persistent mode) |
| `redis-combined` | `redis-combined` | `redis-state,redis-vectors` | v0.16.0 Phase 6 (ONE shared fred client, byte-identical URLs) |

Deep-dive guides:
- [pgvector-backend.md](pgvector-backend.md) — Phase 2 standalone guide (split-pool vectors)
- [postgres-state-backend.md](postgres-state-backend.md) — Phase 3 standalone guide (split-pool state)
- [combined-postgres-backend.md](combined-postgres-backend.md) — Phase 4 standalone guide (shared-pool, one credential, one backup target)
- [multi-backend-operations.md](multi-backend-operations.md) — deployment matrix
- [configuration.md § Multi-Backend Selection (v0.16.0+)](configuration.md#multi-backend-selection-v0160) — env var rules + field tables
- [architecture.md § Multi-Backend Architecture (v0.16.0+)](architecture.md#multi-backend-architecture-v0160) — trait surface + broker factory

### Workspace Crates

| Crate | Responsibility |
|-------|---------------|
| `the-one-core` | Config layering, SQLite + **Postgres** storage (Phase 3), policy engine, profiler, manifests, docs manager, configurable limits, `StateStore` trait, backend selection parser |
| `the-one-mcp` | Async broker orchestrator, 30 MCP tools (26 work + 4 multiplexed admin), JSON-RPC dispatch, transport layer (stdio/SSE/stream), CLI binary, `state_by_project` cache (Phase 1) |
| `the-one-memory` | Code-aware chunker (13 languages), embedding providers (fastembed local + OpenAI-compatible API), async Qdrant HTTP backend, **pgvector backend (Phase 2)**, `VectorBackend` trait |
| `the-one-router` | Rules-first request classification, OpenAI-compatible nano provider, provider pool with health tracking and 3 routing policies |
| `the-one-registry` | Capability catalog with risk-tier filtering, visibility modes (core/project/dormant) |
| `the-one-claude` | Claude Code adapter (thin async wrapper over broker) |
| `the-one-codex` | Codex adapter (thin async wrapper, parity-tested with Claude adapter) |
| `the-one-ui` | Embedded admin UI: dashboard, config editor with limits, audit explorer, Swagger UI |

### Supported AI Assistants

| CLI | Tested | Registration |
|-----|--------|-------------|
| Claude Code | Yes | `claude mcp add` |
| Gemini CLI | Yes | `gemini mcp add` or `settings.json` |
| OpenCode | Yes | `opencode mcp add` |
| Codex | Yes | Manual MCP config |

All four connect via the same stdio JSON-RPC 2.0 protocol. The server reads `clientInfo` from the MCP handshake to load client-specific custom tools.

## 2. Prerequisites

**Required:**
- Rust stable toolchain (1.75+)
- Cargo

**Optional:**
- Qdrant server for remote vector storage (local keyword fallback works without it)
- An OpenAI-compatible LLM endpoint for nano routing (rules-only fallback works without it)
- An OpenAI-compatible embeddings endpoint (fastembed local works without it)

## 3. Installation

### One-Command Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

The installer:
1. Detects your OS (Linux/macOS/Windows) and architecture (x86-64/ARM64)
2. Downloads the latest release binary
3. Creates `~/.the-one/` with default config
4. Downloads recommended tools catalog
5. Auto-detects Claude Code, Gemini CLI, OpenCode, Codex and registers the MCP
6. Validates with a smoke test

Options: `--version v0.16.0`, `--lean` (no swagger), `--local ./target/release`, `--uninstall`. Add `--features pg-vectors,pg-state,redis-state,redis-vectors` when building from source to enable all v0.16.0 multi-backend paths (see [INSTALL.md § Optional multi-backend features](../../INSTALL.md#optional-multi-backend-features-v0160)).

### Build from Source

```bash
git clone <repo-url>
cd the-one-mcp

# Verify workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build release binary
cargo build --release -p the-one-mcp --bin the-one-mcp
```

The binary is at `./target/release/the-one-mcp`.

## 4. Running the MCP Server

### Stdio (default — for Claude Code / Codex)

```bash
./target/release/the-one-mcp serve
```

### Register with AI Assistants

```bash
# Claude Code
claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve

# Gemini CLI
gemini mcp add the-one-mcp ~/.the-one/bin/the-one-mcp serve

# OpenCode
opencode mcp add --name the-one-mcp --command ~/.the-one/bin/the-one-mcp --args serve

# Codex — add to your MCP config manually
```

Or use the installer: `bash scripts/install.sh` — it registers with all detected CLIs automatically.

### SSE Transport (for web clients)

```bash
./target/release/the-one-mcp serve --transport sse --port 3000
```

Endpoints:
- `POST /message` — send JSON-RPC requests
- `GET /sse` — receive server-sent events

### Streamable HTTP Transport

```bash
./target/release/the-one-mcp serve --transport stream --port 3000
```

Endpoint:
- `POST /mcp` — JSON-RPC with optional `Accept: text/event-stream` for SSE responses

### CLI Options

```
the-one-mcp serve [OPTIONS]

Options:
  --transport <TRANSPORT>    stdio | sse | stream [default: stdio]
  --port <PORT>              Port for HTTP transports [default: 3000]
  --project-root <PATH>      Project root directory
  --project-id <ID>          Project identifier
```

## 5. Configuration

Configuration follows a 5-layer precedence model (lowest to highest):

```
1. Hardcoded defaults
2. Global config file:   ~/.the-one/config.json  (or $THE_ONE_HOME/config.json)
3. Project config file:  <project>/.the-one/config.json
4. Environment variables: THE_ONE_*
5. Runtime overrides
```

### Complete Config Example

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
  "embedding_model": "BGE-large-en-v1.5",
  "embedding_api_base_url": null,
  "embedding_api_key": null,
  "embedding_dimensions": 1024,

  "nano_routing_policy": "priority",
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

### Config Field Reference

| Field | Default | Description |
|-------|---------|-------------|
| `provider` | `"local"` | Provider type: `"local"` or `"hosted"` |
| `log_level` | `"info"` | Tracing log level |
| `qdrant_url` | `"http://127.0.0.1:6334"` | Qdrant server URL |
| `qdrant_api_key` | `null` | Qdrant API key (required for remote with strict auth) |
| `qdrant_ca_cert_path` | `null` | Custom CA certificate path for Qdrant TLS |
| `qdrant_tls_insecure` | `false` | Skip TLS verification (development only) |
| `qdrant_strict_auth` | `true` | Require API key for remote Qdrant connections |
| `embedding_provider` | `"local"` | `"local"` (fastembed ONNX) or `"api"` (OpenAI-compatible) |
| `embedding_model` | `"BGE-large-en-v1.5"` | Model name for embeddings |
| `embedding_api_base_url` | `null` | Base URL for API embeddings |
| `embedding_api_key` | `null` | API key for embedding endpoint |
| `embedding_dimensions` | `1024` | Vector dimensions (1024 for local quality model, configurable for API) |
| `nano_routing_policy` | `"priority"` | Provider pool routing: `"priority"`, `"round_robin"`, or `"latency"` |
| `nano_providers` | `[]` | Array of OpenAI-compatible provider configurations |
| `external_docs_root` | `null` | External docs directory to ingest read-only |
| `image_embedding_enabled` | `false` | Enable image embedding for `memory.search_images` / `memory.ingest_image` |
| `image_embedding_model` | `null` | Image embedding model name (CLIP-compatible) |
| `image_ocr_enabled` | `false` | Enable OCR text extraction from ingested images |
| `reranker_enabled` | `false` | Enable cross-encoder reranking of search results |
| `reranker_model` | `null` | Reranker model name (cross-encoder, local ONNX) |

### Environment Variables

| Variable | Maps to |
|----------|---------|
| `THE_ONE_HOME` | Global state directory (must be absolute) |
| `THE_ONE_PROVIDER` | `provider` |
| `THE_ONE_LOG_LEVEL` | `log_level` |
| `THE_ONE_QDRANT_URL` | `qdrant_url` |
| `THE_ONE_QDRANT_API_KEY` | `qdrant_api_key` |
| `THE_ONE_QDRANT_CA_CERT_PATH` | `qdrant_ca_cert_path` |
| `THE_ONE_QDRANT_TLS_INSECURE` | `qdrant_tls_insecure` |
| `THE_ONE_QDRANT_STRICT_AUTH` | `qdrant_strict_auth` |
| `THE_ONE_EMBEDDING_PROVIDER` | `embedding_provider` |
| `THE_ONE_EMBEDDING_MODEL` | `embedding_model` |
| `THE_ONE_EMBEDDING_API_BASE_URL` | `embedding_api_base_url` |
| `THE_ONE_EMBEDDING_API_KEY` | `embedding_api_key` |
| `THE_ONE_EMBEDDING_DIMENSIONS` | `embedding_dimensions` |
| `THE_ONE_EXTERNAL_DOCS_ROOT` | `external_docs_root` |
| `THE_ONE_LIMIT_*` | Corresponding limit field (e.g., `THE_ONE_LIMIT_MAX_SEARCH_HITS`) |
| `THE_ONE_PROJECT_ROOT` | Project root for embedded UI |
| `THE_ONE_PROJECT_ID` | Project ID for embedded UI |
| `THE_ONE_UI_BIND` | Bind address for embedded UI (default `127.0.0.1:8787`) |

## 6. Configurable Limits

All limits are configurable via config file, environment variables, or admin UI. Out-of-bounds values are clamped with a warning.

| Limit | Default | Min | Max | Description |
|-------|---------|-----|-----|-------------|
| `max_tool_suggestions` | 5 | 1 | 50 | Max tools returned per suggest query |
| `max_search_hits` | 5 | 1 | 100 | Max RAG results per memory.search |
| `max_raw_section_bytes` | 24,576 | 1,024 | 1,048,576 | Max bytes for docs.get (with section param) |
| `max_enabled_families` | 12 | 1 | 100 | Max tool families enabled per project |
| `max_doc_size_bytes` | 102,400 | 1,024 | 10,485,760 | Max single managed doc size |
| `max_managed_docs` | 500 | 10 | 10,000 | Max docs in managed folder |
| `max_embedding_batch_size` | 64 | 1 | 256 | Chunks per embedding batch |
| `max_chunk_tokens` | 512 | 64 | 2,048 | Target chunk size for RAG splitting |
| `max_nano_timeout_ms` | 2,000 | 100 | 10,000 | Max timeout per nano provider call |
| `max_nano_retries` | 3 | 0 | 10 | Max retries across provider pool |
| `max_nano_providers` | 5 | 1 | 10 | Max nano providers in pool |
| `search_score_threshold` | 0.3 | 0.0 | 1.0 | Min cosine similarity for search results |

## 7. Embeddings

### Local Embeddings (default)

Uses [fastembed-rs](https://github.com/Anush008/fastembed-rs) with ONNX Runtime. No API calls, no cost, fully offline. First run downloads the model (cached in `~/.the-one/.fastembed_cache/`).

#### Tiered Model Selection

Use a tier alias or full model name in config:

| Tier | Model | Dims | Download | Speed | Use Case |
|------|-------|------|----------|-------|----------|
| `fast` | all-MiniLM-L6-v2 | 384 | ~23MB | ~30ms | Getting started, fast iteration |
| `balanced` | BGE-base-en-v1.5 | 768 | ~50MB | ~60ms | **Production recommended** |
| `quality` (default) | BGE-large-en-v1.5 | 1024 | ~130MB | ~120ms | Best local quality |
| `multilingual` | multilingual-e5-large | 1024 | ~220MB | ~150ms | Non-English / mixed-language |

Config: `"embedding_model": "quality"` or `"embedding_model": "BGE-large-en-v1.5"`

#### Additional Models

All 15+ fastembed models supported by full name:
`all-MiniLM-L12-v2`, `BGE-small-en-v1.5`, `nomic-embed-text-v1.5`, `mxbai-embed-large-v1`, `gte-base-en-v1.5`, `gte-large-en-v1.5`, `multilingual-e5-small`, `multilingual-e5-base`, `paraphrase-ml-minilm-l12-v2`

Quantized variants (smaller download, slight quality trade-off): append `-q` to tier name or model name — `fast-q`, `balanced-q`, `quality-q`, `bge-base-en-v1.5-q`

### API Embeddings

Any OpenAI-compatible `/v1/embeddings` endpoint. Works with OpenAI, Voyage, Cohere, LiteLLM, etc.

```json
{
  "embedding_provider": "api",
  "embedding_api_base_url": "https://api.openai.com/v1",
  "embedding_api_key": "sk-...",
  "embedding_model": "text-embedding-3-small",
  "embedding_dimensions": 1536
}
```

## 8. Nano LLM Provider Pool

Optional intelligent request routing through lightweight LLMs. The pool manages multiple OpenAI-compatible endpoints with automatic health tracking and failover.

### Provider Configuration

```json
{
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
  ]
}
```

Compatible with: Ollama, LM Studio, LiteLLM, vLLM, LocalAI, Groq, Together, OpenAI.

### Routing Policies

| Policy | Behavior |
|--------|----------|
| `priority` | Try providers in config order. First healthy one wins. |
| `round_robin` | Rotate across healthy providers evenly. |
| `latency` | Use the provider with lowest recent p50 latency. |

### Health Tracking

- **TCP connect check** (50ms timeout) before every classification
- **Cooldown**: 5s (1 failure), 15s (2 failures), 60s (3+ failures)
- **Recovery**: successful call resets to healthy immediately
- **Latency**: rolling window of last 20 calls for p50 calculation
- **Fallback**: if all providers fail, silent fallback to rules-only routing

## 9. Managed Documents

The broker manages a docs folder at `<project>/.the-one/docs/`:

```
<project>/.the-one/docs/
+-- architecture/
|   +-- auth.md
+-- decisions/
|   +-- 2026-04-03-db-choice.md
+-- .trash/                        # soft-deleted files
    +-- old-stuff/
        +-- deprecated.md
```

### Tools

| Tool | Description |
|------|-------------|
| `docs.save` | Create or update a `.md` file (upsert). Auto-creates subdirectories. Auto-indexes into RAG. |
| `docs.get` | Return full document, or a specific section when `section` param is provided (bounded by `max_raw_section_bytes`). |
| `docs.list` | List files with path, size, last modified time. |
| `docs.delete` | Soft-delete: moves to `.trash/` preserving path. Removes from RAG. |
| `docs.move` | Rename or move within managed folder. |
| `maintain` (trash) | Actions: `trash.list`, `trash.restore`, `trash.empty` — via the multiplexed maintain tool. |
| `maintain` (reindex) | Action: `reindex` — force full re-ingestion into RAG index. |

### Validation Rules

- `.md` extension required
- Max file size: `max_doc_size_bytes` (default 100KB)
- Max file count: `max_managed_docs` (default 500)
- No path traversal (`../` rejected)
- Safe characters: alphanumeric, hyphens, underscores, dots, forward slashes

### External Docs

Set `external_docs_root` to ingest an external directory read-only into RAG. No CRUD operations.

## 10. Image Search

The broker supports multimodal search over images — screenshots, diagrams, and UI snapshots — stored in a dedicated Qdrant collection (`the_one_images_*`).

### Enable Image Embedding

```json
{
  "image_embedding_enabled": true,
  "image_embedding_model": "clip-vit-base-patch32",
  "image_ocr_enabled": true
}
```

### Tools

| Tool | Description |
|------|-------------|
| `memory.ingest_image` | Ingest an image file (JPEG, PNG, etc.) into the image index. Optionally runs OCR to extract text alongside the visual embedding. |
| `memory.search_images` | Semantic search over ingested images using a text query. Returns matching images with paths, scores, and any extracted OCR text. |

### maintain Actions for Images

| Action | Description |
|--------|-------------|
| `images.rescan` | Re-scan and re-index all images in the project |
| `images.clear` | Remove all image embeddings from the index |
| `images.delete` | Delete a specific image from the index by path |

### Qdrant Collections

Image embeddings are stored in a separate collection from text: `the_one_images_<project_id>`. This keeps image and text search independent and allows clearing images without affecting text RAG.

## 10b. Reranking

After a vector search, an optional cross-encoder reranker can re-score and re-order results for higher precision.

### Enable Reranking

```json
{
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual"
}
```

Reranking runs locally via fastembed ONNX. It adds latency (~20–80ms per query depending on result count) but meaningfully improves result ordering for ambiguous queries.

## 11. RAG Pipeline (Text)

### Chunking

The chunker dispatches by file extension (v0.8.0):

- **Code files** (`.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.go`) — language-aware chunkers split by function/struct/class/interface boundaries using brace-depth tracking or indentation depth. Each chunk carries extended `ChunkMeta` fields: `language`, `symbol`, `signature`, `line_range`.
- **Markdown / other** — heading-aware splitting, then paragraph splitting for large sections. `language`, `symbol`, `signature`, `line_range` are `null`.

For markdown, the chunking steps are:

1. Parse heading hierarchy (`#`, `##`, `###`, etc.)
2. Split into sections at heading boundaries
3. Large sections split on paragraph boundaries (never mid-paragraph or mid-code-block)
4. Each chunk: source path, heading hierarchy, byte offset, content hash
5. Target: `max_chunk_tokens` (default 512, ~2KB)

See [Code Chunking Guide](code-chunking.md) for details on supported languages and the extended metadata fields.

### Search

```
Query -> Embed (1024-dim) -> Qdrant cosine search -> Top-k results (score >= threshold)
```

Returns chunks with source paths and headings. Follow up with `docs.get` for full context.

### Incremental Re-indexing

Only changed files are re-embedded (via content hash). Full reindex via `maintain` (action: `reindex`).

## 12. Project Lifecycle

The `setup` multiplexed admin tool manages project initialization and refresh:

- **`setup` (action: `init`)** — scans signal files to detect languages (Rust, JavaScript, Python, Go), frameworks (axum, tokio, Docker, GitHub Actions), and risk profile (HighRisk / Caution / Safe). Creates `.the-one/` with manifests, SQLite database, and profile.
- **`setup` (action: `refresh`)** — computes SHA-256 fingerprint of signal files. If unchanged, returns cached profile. If changed, recomputes and syncs document index.

## 13. Tool Catalog

The broker includes a curated catalog of developer tools, LSPs, and MCP servers — stored in SQLite with FTS5 full-text search and Qdrant semantic search.

### Catalog Architecture

```
tools/catalog/                     Source of truth (JSON files on GitHub)
├── languages/rust.json            16 Rust tools (LSP, build, test, QA, security)
├── categories/security.json       4 cross-language security tools
├── mcps/official.json             8 official MCP servers
└── _schema.json                   Schema for tool entries

~/.the-one/catalog.db              SQLite: imported tools + FTS5 + inventory + enabled state
Qdrant: the_one_tools              Semantic search over tool descriptions
```

### How tool.find Works

`tool.find` is the unified discovery interface with three modes:

**`mode: "suggest"`** — project-aware recommendations:
```
User: "I need QA tools"
  → LLM calls: tool.find({ mode: "suggest", query: "qa" })
  → Broker: filter catalog by project languages (Rust) + category (qa)
  → Group by install state:
      ENABLED:     cargo-clippy (active)
      AVAILABLE:   cargo-audit (installed, not enabled)
      RECOMMENDED: cargo-deny, semgrep (not installed, with install commands)
  → LLM decides which to enable/install/run
```

**`mode: "search"`** — semantic/keyword search, three-tier fallback:
1. **Qdrant semantic search** — "check deps for security issues" finds `cargo-audit` even though words don't match
2. **SQLite FTS5** — keyword-based fallback when Qdrant unavailable
3. **Registry fallback** — legacy capability registry

**`mode: "list"`** — enumerate tools by state (enabled / available / all)

### Tool Lifecycle MCP Tools

| Tool | Description |
|------|-------------|
| `tool.find` | Unified discovery — `mode: "suggest"` (project-aware recommendations), `mode: "search"` (semantic/text search), `mode: "list"` (by state) |
| `tool.info` | Full metadata for a tool (description, install, run, risk, state) |
| `tool.install` | Run install command, update inventory, auto-enable |
| `tool.run` | Execute with policy-gated approval |
| `config` (tool.add) | Add a custom tool locally (via config action) |
| `config` (tool.remove) | Remove user-added tool (via config action) |
| `maintain` (tool.enable) | Activate for current CLI/project (via maintain action) |
| `maintain` (tool.disable) | Deactivate for current CLI/project (via maintain action) |
| `maintain` (tool.refresh) | Refresh catalog from GitHub + re-scan inventory (via maintain action) |

### Custom Tools (Per-CLI)

```
~/.the-one/registry/
├── custom.json              # Shared across all CLIs
├── custom-claude.json       # Claude Code only
├── custom-gemini.json       # Gemini CLI only
├── custom-opencode.json     # OpenCode only
└── custom-codex.json        # Codex only
```

Add tools via the `tool.add` MCP command (the LLM calls it), or edit the JSON files directly.

### Adding Tools via MCP

The LLM can add tools on your behalf via the `config` admin tool:
```
User: "Add my custom linter as a tool"
LLM calls: config({
  action: "tool.add",
  params: {
    id: "my-linter",
    name: "My Linter",
    description: "Custom linting for our project",
    install_command: "npm install -g my-linter",
    run_command: "my-linter check ."
  }
})
```

### Contributing Tools to the Catalog

See [CONTRIBUTING.md](../../CONTRIBUTING.md) — submit via GitHub Issue or PR.

## 14. Policy and Approvals

| Risk Level | Approval Required |
|------------|------------------|
| Low / Medium | No |
| High | Yes |

| Approval Scope | Duration |
|----------------|----------|
| `once` | Single execution |
| `session` | Broker session lifetime (in-memory) |
| `forever` | Persisted in SQLite |

**Headless mode**: high-risk tools denied unless prior approval exists.

## 15. Observability

### Metrics (`observe (action: metrics)`)

`project_init_calls`, `project_refresh_calls`, `memory_search_calls`, `tool_run_calls`, `router_fallback_calls`, `router_provider_error_calls`, `router_decision_latency_ms_total`

### Audit Trail (`observe (action: events)`)

All tool executions logged with timestamps and JSON payloads. Max 200 per query.

## 16. Embedded Admin UI

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

| Route | Description |
|-------|-------------|
| `/dashboard` | Config, metrics, audit summary, provider pool status |
| `/config` | Editable form for all config fields and 12 limits |
| `/audit` | Audit event table |
| `/swagger` | Interactive Swagger UI |
| `/api/health` | JSON health check |
| `/api/swagger` | Raw OpenAPI JSON |
| `/api/config` | POST endpoint for config updates |

## 17. Project State Layout

```
~/.the-one/                            # global state ($THE_ONE_HOME)
+-- config.json
+-- registry/capabilities.json

<project>/.the-one/                    # project state
+-- project.json                       # manifest
+-- overrides.json                     # enabled families
+-- fingerprint.json                   # signal file hash
+-- pointers.json                      # DB/RAG paths
+-- config.json                        # project config overrides
+-- state.db                           # SQLite (WAL)
+-- docs/                              # managed documents
|   +-- .trash/                        # soft-deleted
+-- qdrant/                            # local index fallback
```

## 18. CI and Release

### Local Validation

```bash
# Full CI pipeline (what CI runs)
bash scripts/build.sh check

# Or individual steps
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
bash scripts/release-gate.sh
```

### Cross-Platform Release

Releases are **manual only** — tagging does not auto-trigger builds. You decide when to create release artifacts.

```bash
# Trigger a release (builds 6 platform binaries on GitHub Actions)
bash scripts/build.sh release v0.8.0

# Check release workflow status
bash scripts/build.sh release --status

# Preview without triggering
bash scripts/build.sh --dry-run release v0.8.1
```

Or via GitHub UI: Actions → release → Run workflow → enter version tag.

Each release builds: Linux x86-64, Linux ARM64, macOS x86-64, macOS ARM64, Windows x86-64, Windows ARM64. Each archive contains `the-one-mcp`, `the-one-mcp-lean` (no swagger), `embedded-ui`, schemas, and build metadata.

### Security CI

Automated weekly (Monday 06:00 UTC) + on every push/PR:
- `cargo audit` — dependency vulnerability scanning
- `gitleaks` — secret detection in committed code

## 19. Troubleshooting

| Problem | Solution |
|---------|----------|
| `remote qdrant requires api key` | Set `qdrant_api_key` or `qdrant_strict_auth: false` |
| Swagger 404 | Build with `--features embed-swagger` (default) |
| No search results | Run `maintain` (action: `reindex`), lower `search_score_threshold` |
| Headless tool denied | Set approval via interactive mode first |
| Nano provider timeouts | Check URL, increase `timeout_ms`, pool auto-falls back to rules |
| Slow first embedding | Model download (~30MB), cached after |

## 20. Source Reference

| Component | File |
|-----------|------|
| API types | `crates/the-one-mcp/src/api.rs` |
| Broker | `crates/the-one-mcp/src/broker.rs` |
| Tool schemas | `crates/the-one-mcp/src/transport/tools.rs` |
| JSON-RPC | `crates/the-one-mcp/src/transport/jsonrpc.rs` |
| Config | `crates/the-one-core/src/config.rs` |
| Limits | `crates/the-one-core/src/limits.rs` |
| Docs manager | `crates/the-one-core/src/docs_manager.rs` |
| Chunker (dispatcher) | `crates/the-one-memory/src/chunker.rs` |
| Chunker — Rust | `crates/the-one-memory/src/chunker_rust.rs` |
| Chunker — Python | `crates/the-one-memory/src/chunker_python.rs` |
| Chunker — TypeScript/JS | `crates/the-one-memory/src/chunker_typescript.rs` |
| Chunker — Go | `crates/the-one-memory/src/chunker_go.rs` |
| Embeddings | `crates/the-one-memory/src/embeddings.rs` |
| Qdrant | `crates/the-one-memory/src/qdrant.rs` |
| Image embeddings | `crates/the-one-memory/src/image_embeddings.rs` |
| Image ingest | `crates/the-one-memory/src/image_ingest.rs` |
| Thumbnail | `crates/the-one-memory/src/thumbnail.rs` |
| OCR | `crates/the-one-memory/src/ocr.rs` |
| Reranker | `crates/the-one-memory/src/reranker.rs` |
| Provider pool | `crates/the-one-router/src/provider_pool.rs` |
| Health tracking | `crates/the-one-router/src/health.rs` |
| v1beta schemas | `schemas/mcp/v1beta/` |
