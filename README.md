# the-one-mcp

A production-grade Rust MCP (Model Context Protocol) broker that gives AI coding assistants project-aware memory, semantic document search, policy-gated tool execution, and intelligent request routing — while keeping token usage minimal.

Works with **Claude Code**, **Gemini CLI**, **OpenCode**, and **Codex** out of the box.

## Install (One Command)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

The installer auto-detects your OS, downloads the latest release, creates config with sensible defaults, and registers with every AI CLI it finds on your system.

### Or Install Manually

```bash
# Build from source
bash scripts/build.sh build

# Install to ~/.the-one/bin/
bash scripts/install.sh --local ./target/release
```

## Supported AI Assistants

| CLI | Auto-Detected | Registration |
|-----|--------------|--------------|
| **Claude Code** | `claude --version` | `claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve` |
| **Gemini CLI** | `gemini --version` | `gemini mcp add the-one-mcp ~/.the-one/bin/the-one-mcp serve` |
| **OpenCode** | `opencode --version` | `opencode mcp add --name the-one-mcp --command ~/.the-one/bin/the-one-mcp --args serve` |
| **Codex** | `codex --version` | Add to Codex MCP config |

All four use the same MCP server — same 24 tools, same protocol. The server reads `clientInfo` from the MCP handshake to load client-specific custom tools.

## Why

LLMs waste tokens loading irrelevant tools, re-reading docs, and losing context between sessions. The-One MCP acts as a smart intermediary:

- **Progressive tool exposure** — only surfaces relevant capabilities based on project profile
- **Unlimited project memory** — semantic RAG search over your docs without loading everything into context
- **Managed knowledge base** — create, update, and organize markdown docs that persist across sessions
- **Token-efficient retrieval** — configurable limits on search results, doc section sizes, and tool suggestions
- **Policy-gated execution** — approval scopes (once/session/forever) for high-risk tool actions
- **Client-aware tools** — per-CLI custom tools (shared + Claude-specific + Gemini-specific + ...)

## Architecture

```
Claude Code / Gemini CLI / OpenCode / Codex
    |  (JSON-RPC 2.0 via stdio, SSE, or streamable HTTP)
    v
the-one-mcp broker (reads clientInfo to identify which CLI)
    |
    +-- Project Lifecycle    project.init / project.refresh / project.profile.get
    +-- Knowledge (RAG)      memory.search / memory.fetch_chunk
    +-- Documents (CRUD)     docs.create / update / delete / get / list / move
    +-- Trash Management     docs.trash.list / restore / empty
    +-- Tool Management      tool.suggest / search / enable / run
    +-- Configuration        config.export / config.update
    +-- Observability        metrics.snapshot / audit.events / docs.reindex
    |
    +-- Embeddings           Tiered: fast (384d) / balanced (768d) / quality (1024d) / API
    +-- Vector Storage       Qdrant HTTP (remote or local fallback)
    +-- LLM Routing          Provider pool: Ollama/LiteLLM/OpenAI + 3 routing policies
    +-- Policy Engine        Configurable limits + risk-tier approval gates
    +-- SQLite               Project state, approvals, audit trail (WAL mode)
```

## Embedding Models

Local models run offline via ONNX — no API key, no cost:

| Tier | Model | Dims | Speed | Use Case |
|------|-------|------|-------|----------|
| `fast` (default) | all-MiniLM-L6-v2 | 384 | ~30ms | Getting started |
| `balanced` | BGE-base-en-v1.5 | 768 | ~60ms | Production recommended |
| `quality` | BGE-large-en-v1.5 | 1024 | ~120ms | Best local quality |
| `multilingual` | multilingual-e5-large | 1024 | ~150ms | Non-English projects |

Plus 15+ additional models and quantized variants. Or use any OpenAI-compatible API.

Config: `"embedding_model": "balanced"`

## 24 MCP Tools

| Category | Tools |
|----------|-------|
| **Project** | `project.init`, `project.refresh`, `project.profile.get` |
| **Knowledge** | `memory.search`, `memory.fetch_chunk` |
| **Documents** | `docs.create`, `docs.update`, `docs.delete`, `docs.get`, `docs.get_section`, `docs.list`, `docs.move` |
| **Trash** | `docs.trash.list`, `docs.trash.restore`, `docs.trash.empty` |
| **Re-index** | `docs.reindex` |
| **Tools** | `tool.suggest`, `tool.search`, `tool.enable`, `tool.run` |
| **Config** | `config.export`, `config.update` |
| **Observability** | `metrics.snapshot`, `audit.events` |

## Custom Tools (Per-CLI)

```
~/.the-one/tools/
├── recommended.json         # Universal (auto-updated from GitHub)
├── custom.json              # Your shared custom tools (all CLIs)
├── custom-claude.json       # Claude Code only
├── custom-gemini.json       # Gemini CLI only
├── custom-opencode.json     # OpenCode only
└── custom-codex.json        # Codex only
```

The server loads: `recommended.json` + `custom.json` + `custom-<client>.json` based on which CLI connects.

## Install Layout

```
~/.the-one/
├── bin/
│   ├── the-one-mcp          # MCP server binary
│   └── embedded-ui           # Admin UI binary
├── config.json               # Global config (sensible defaults)
├── registry/
│   ├── recommended.json      # Pre-built tools (auto-updated)
│   ├── custom.json           # Your shared tools
│   └── custom-<cli>.json     # Per-CLI custom tools
└── schemas/                  # v1beta JSON schemas
```

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/install.sh` | One-command install: download, configure, register with all CLIs |
| `scripts/build.sh` | Build manager: `build`, `dev`, `test`, `check`, `package`, `install` |
| `scripts/release-gate.sh` | CI validation: fmt + clippy + test + contract checks |

## Documentation

| Document | Description |
|----------|-------------|
| [Quickstart](docs/guides/quickstart.md) | Shortest path to install and run |
| [Complete Guide](docs/guides/the-one-mcp-complete-guide.md) | Full reference for all features |
| [Operator Runbook](docs/ops/operator-runbook.md) | Operations, backup/restore, incident triage |
| [Release Notes](docs/releases/v1beta-upgrade-notes.md) | Upgrade guidance |
| [Architecture](docs/plans/the-one-mcp-architecture-prompt.md) | Design rationale and principles |

## Build from Source

```bash
# Using build.sh (recommended)
bash scripts/build.sh build            # with swagger
bash scripts/build.sh build --lean     # without swagger (smaller)
bash scripts/build.sh check            # full CI pipeline

# Or raw cargo
cargo build --release -p the-one-mcp --bin the-one-mcp
```

## Embedded Admin UI

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

Open `http://127.0.0.1:8787/dashboard` for config, metrics, audit, and swagger.

## License

Apache-2.0
