# The-One MCP Complete Guide

## 1. Overview

`the-one-mcp` is a Rust MCP broker that acts as a smart intermediary between AI coding assistants and your project. Works with **Claude Code**, **Gemini CLI**, **OpenCode**, and **Codex** — same server, same protocol, client-aware tool loading.

It provides:

- **Project lifecycle** — detect languages, frameworks, and risk profile; cache results via fingerprinting
- **Semantic memory** — production-grade RAG with fastembed (384-dim ONNX) or API embeddings over Qdrant
- **Managed documents** — full CRUD for markdown files with soft-delete, trash, and auto-sync
- **Policy-gated tools** — risk-tier approval gates (once/session/forever) with headless deny-by-default
- **Intelligent routing** — rules-first with optional nano LLM provider pool (priority/round-robin/latency)
- **Token efficiency** — configurable limits on search results, doc sizes, tool suggestions

### Workspace Crates

| Crate | Responsibility |
|-------|---------------|
| `the-one-core` | Config layering, SQLite storage (WAL), policy engine, profiler, manifests, docs manager, configurable limits |
| `the-one-mcp` | Async broker orchestrator, 24 MCP tool API types, JSON-RPC dispatch, transport layer (stdio/SSE/stream), CLI binary |
| `the-one-memory` | Smart markdown chunker, embedding providers (fastembed local + OpenAI-compatible API), async Qdrant HTTP backend |
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

Options: `--version v0.2.0`, `--lean` (no swagger), `--local ./target/release`, `--uninstall`

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
| `embedding_model` | `"all-MiniLM-L6-v2"` | Model name for embeddings |
| `embedding_api_base_url` | `null` | Base URL for API embeddings |
| `embedding_api_key` | `null` | API key for embedding endpoint |
| `embedding_dimensions` | `384` | Vector dimensions (384 for local, configurable for API) |
| `nano_routing_policy` | `"priority"` | Provider pool routing: `"priority"`, `"round_robin"`, or `"latency"` |
| `nano_providers` | `[]` | Array of OpenAI-compatible provider configurations |
| `external_docs_root` | `null` | External docs directory to ingest read-only |

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
| `max_raw_section_bytes` | 24,576 | 1,024 | 1,048,576 | Max bytes for docs.get_section |
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
| `fast` (default) | all-MiniLM-L6-v2 | 384 | ~23MB | ~30ms | Getting started, fast iteration |
| `balanced` | BGE-base-en-v1.5 | 768 | ~50MB | ~60ms | **Production recommended** |
| `quality` | BGE-large-en-v1.5 | 1024 | ~130MB | ~120ms | Best local quality |
| `multilingual` | multilingual-e5-large | 1024 | ~220MB | ~150ms | Non-English / mixed-language |

Config: `"embedding_model": "balanced"` or `"embedding_model": "BGE-base-en-v1.5"`

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
| `docs.create` | Create new `.md` file. Auto-creates subdirectories. Auto-indexes into RAG. |
| `docs.update` | Overwrite file content. Re-indexes changed chunks. |
| `docs.delete` | Soft-delete: moves to `.trash/` preserving path. Removes from RAG. |
| `docs.get` | Return full original markdown as-is. |
| `docs.get_section` | Return heading section, bounded by `max_raw_section_bytes`. |
| `docs.list` | List files with path, size, last modified time. |
| `docs.move` | Rename or move within managed folder. |
| `docs.trash.list` | List trash contents. |
| `docs.trash.restore` | Restore from trash. Re-indexes. |
| `docs.trash.empty` | Permanently delete all trash contents. |
| `docs.reindex` | Force full re-ingestion into RAG index. |

### Validation Rules

- `.md` extension required
- Max file size: `max_doc_size_bytes` (default 100KB)
- Max file count: `max_managed_docs` (default 500)
- No path traversal (`../` rejected)
- Safe characters: alphanumeric, hyphens, underscores, dots, forward slashes

### External Docs

Set `external_docs_root` to ingest an external directory read-only into RAG. No CRUD operations.

## 10. RAG Pipeline

### Chunking

1. Parse heading hierarchy (`#`, `##`, `###`, etc.)
2. Split into sections at heading boundaries
3. Large sections split on paragraph boundaries (never mid-paragraph or mid-code-block)
4. Each chunk: source path, heading hierarchy, byte offset, content hash
5. Target: `max_chunk_tokens` (default 512, ~2KB)

### Search

```
Query -> Embed (384-dim) -> Qdrant cosine search -> Top-k results (score >= threshold)
```

Returns chunks with source paths and headings. Follow up with `docs.get` for full context.

### Incremental Re-indexing

On `project.refresh`, only changed files are re-embedded (via content hash). Full reindex via `docs.reindex`.

## 11. Project Lifecycle

### project.init

Scans signal files to detect:
- **Languages**: Rust, JavaScript, Python, Go
- **Frameworks**: axum, tokio, Docker, GitHub Actions
- **Risk profile**: HighRisk / Caution / Safe

Creates `.the-one/` with manifests, SQLite database, and profile.

### project.refresh

Computes SHA-256 fingerprint of signal files. If unchanged, returns cached profile. If changed, recomputes and syncs document index.

## 12. Custom Tools (Per-CLI)

The tool catalog supports three layers, loaded in order:

```
~/.the-one/registry/
├── recommended.json         # Universal baseline (auto-updated from GitHub)
├── custom.json              # Your shared tools (loaded for ALL clients)
├── custom-claude.json       # Loaded only when Claude Code connects
├── custom-gemini.json       # Loaded only when Gemini CLI connects
├── custom-opencode.json     # Loaded only when OpenCode connects
└── custom-codex.json        # Loaded only when Codex connects
```

### How Client Detection Works

The MCP protocol's `initialize` handshake includes a `clientInfo.name` field. The broker reads this to determine which CLI is connecting and loads the appropriate custom tools file.

### Adding Custom Tools

Edit the appropriate file (JSON array of tool objects):

```json
[
  {
    "id": "my-custom-tool",
    "title": "My Custom Tool",
    "capability_type": "McpTool",
    "family": "custom",
    "visibility_mode": "Core",
    "risk_level": "Low",
    "description": "What this tool does"
  }
]
```

- Tools you want in **all** CLIs: add to `custom.json`
- Tools only for **Claude Code**: add to `custom-claude.json`
- Tools only for **Gemini CLI**: add to `custom-gemini.json`
- `recommended.json` is auto-updated from GitHub — don't edit it

### Recommended Tools

The `recommended.json` file ships with 15 pre-built tool definitions covering project management, RAG search, document CRUD, and observability. It's downloaded during installation and can be refreshed by re-running the installer.

## 13. Policy and Approvals

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

## 14. Observability

### Metrics (`metrics.snapshot`)

`project_init_calls`, `project_refresh_calls`, `memory_search_calls`, `tool_run_calls`, `router_fallback_calls`, `router_provider_error_calls`, `router_decision_latency_ms_total`

### Audit Trail (`audit.events`)

All tool executions logged with timestamps and JSON payloads. Max 200 per query.

## 15. Embedded Admin UI

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

## 16. Project State Layout

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

## 17. CI and Release

```bash
# Full validation
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
bash scripts/release-gate.sh
```

## 18. Troubleshooting

| Problem | Solution |
|---------|----------|
| `remote qdrant requires api key` | Set `qdrant_api_key` or `qdrant_strict_auth: false` |
| Swagger 404 | Build with `--features embed-swagger` (default) |
| No search results | Run `docs.reindex`, lower `search_score_threshold` |
| Headless tool denied | Set approval via interactive mode first |
| Nano provider timeouts | Check URL, increase `timeout_ms`, pool auto-falls back to rules |
| Slow first embedding | Model download (~30MB), cached after |

## 19. Source Reference

| Component | File |
|-----------|------|
| API types | `crates/the-one-mcp/src/api.rs` |
| Broker | `crates/the-one-mcp/src/broker.rs` |
| Tool schemas | `crates/the-one-mcp/src/transport/tools.rs` |
| JSON-RPC | `crates/the-one-mcp/src/transport/jsonrpc.rs` |
| Config | `crates/the-one-core/src/config.rs` |
| Limits | `crates/the-one-core/src/limits.rs` |
| Docs manager | `crates/the-one-core/src/docs_manager.rs` |
| Chunker | `crates/the-one-memory/src/chunker.rs` |
| Embeddings | `crates/the-one-memory/src/embeddings.rs` |
| Qdrant | `crates/the-one-memory/src/qdrant.rs` |
| Provider pool | `crates/the-one-router/src/provider_pool.rs` |
| Health tracking | `crates/the-one-router/src/health.rs` |
| v1beta schemas | `schemas/mcp/v1beta/` |
