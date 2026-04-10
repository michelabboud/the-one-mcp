# the-one-mcp

> [!WARNING]
> **This project is under active development and testing.** APIs, tool definitions, and catalog formats may change between releases. Not recommended for production use yet. Feedback and contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

A production-grade Rust MCP (Model Context Protocol) broker that gives AI coding assistants project-aware memory, semantic document search, a curated tool catalog with thousands of developer tools, and intelligent request routing — while keeping token usage minimal.

Works with **Claude Code**, **Gemini CLI**, **OpenCode**, and **Codex** out of the box.

## Install (One Command)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

Auto-detects your OS, downloads the latest release, sets up config with sensible defaults, imports the tool catalog, and registers with every AI CLI it finds. See [INSTALL.md](INSTALL.md) for all options.

## What It Does

```
You: "Check my code for security issues"
                    ↓
Claude/Gemini/OpenCode calls tool.find({ mode: "suggest", query: "security" })
                    ↓
the-one-mcp: "Your project is Rust + Docker. Here's what I found:"
  ENABLED:     cargo-clippy (running)
  AVAILABLE:   cargo-audit (installed, not enabled)
  RECOMMENDED: cargo-deny, semgrep, trivy (not installed)
                    ↓
Claude: "Let me enable cargo-audit and run it."
  → tool.enable("cargo-audit")
  → tool.run("cargo-audit")
  → Analyzes results, reports vulnerabilities
```

The LLM is the brain. The MCP is the data layer — catalog, filtering, execution, memory.

## Key Features

- **Tool Catalog** — 365 curated tools across 10 languages (Rust, Python, JS/TS, Go, Java, Kotlin, Ruby, PHP, Swift, C/C++), searchable via semantic search or full-text. Knows what's installed on your system, what's available, what to recommend.
- **Unlimited Memory** — Semantic RAG search over project docs. Ask about code from last week — it finds the relevant chunks without loading entire files.
- **Conversation Memory** — Import transcript exports with `memory.ingest_conversation`, tag them with palace metadata (`wing`, `hall`, `room`), search them with `memory.search`, build compact resume packs with `memory.wake_up`, and optionally capture `stop`/`precompact` hooks with `maintain: memory.capture_hook`.
- **Hybrid Search** — Combine dense cosine similarity with sparse lexical matching (SPLADE++) for stronger exact-match retrieval. Opt-in. Great for code repos with function names, error strings, and crate identifiers.
- **Redis Vector Backend** — Optional Redis/RediSearch vector storage with durable index naming and persistence enforcement (`redis_persistence_required`) for teams that operate Redis-first infrastructure.
- **Managed Knowledge Base** — Create, update, and organize markdown docs that persist across sessions. The LLM writes notes, decisions, architecture docs.
- **MCP Resources API** — Expose indexed docs, project profile, and tool catalog as native MCP resources addressable via `the-one://` URIs. Claude Code and other MCP clients can `@`-reference them directly. See [MCP Resources Guide](docs/guides/mcp-resources.md).
- **Smart Discovery** — `tool.find` filters by project profile (languages, frameworks), groups by install state (enabled / available / recommended). Token-efficient.
- **Policy-Gated Execution** — Approval scopes (once/session/forever) for high-risk tools. Headless deny-by-default.
- **Auto-Indexing** — Background file watcher on `.the-one/docs/` and `.the-one/images/`. Detects file changes and automatically re-ingests updated markdown AND images into the search index. Opt-in via `auto_index_enabled: true`.
- **Tree-Sitter Code Chunker** — AST-based language-aware chunking for **13 languages** (Rust, Python, TypeScript, JavaScript, Go, C, C++, Java, Kotlin, PHP, Ruby, Swift, Zig). Search results include function/class names, signatures, and line ranges as structured metadata. See [Code Chunking Guide](docs/guides/code-chunking.md).
- **Backup / Restore** — `maintain: backup` creates a gzipped tarball of your entire project state (docs, images, config, catalog, enabled tools) for moving to a new machine or off-site archival. See [Backup & Restore Guide](docs/guides/backup-restore.md).
- **Observability** — Built-in metrics surface with per-operation latency tracking, watcher event counts, Qdrant error counters, and MCP resource usage via the `observe` tool. See [Observability Guide](docs/guides/observability.md).
- **Multi-CLI** — Same server works with Claude Code, Gemini CLI, OpenCode, Codex. Per-CLI custom tools via `clientInfo` detection.

## Architecture

```
Claude Code / Gemini CLI / OpenCode / Codex
    |  (JSON-RPC 2.0 via stdio, SSE, or streamable HTTP)
    v
the-one-mcp broker
    |
    +-- MCP Primitives       19 tools + 3 resource types (docs/project/catalog)
    +-- Tool Catalog         365 curated tools, SQLite + Qdrant semantic search
    +-- Project Lifecycle    Detect languages/frameworks, fingerprint caching
    +-- Knowledge (RAG)      fastembed (384-1024 dim) + Qdrant or Redis/RediSearch
    +-- Code Chunker         Tree-sitter AST chunking for 13 languages
    +-- Documents (CRUD)     Managed folder with soft-delete, auto-sync
    +-- Auto-Reindex         File watcher for markdown + images
    +-- LLM Routing          Provider pool: Ollama/LiteLLM/OpenAI, 3 policies
    +-- Policy Engine        Configurable limits + risk-tier approval gates
    +-- Backup / Restore     Gzipped tar of project state + catalog + registry
    +-- Observability        Metrics counters + audit events via `observe`
    +-- SQLite               Project state, catalog, approvals, audit trail
```

## 19 MCP Tools

| Category | Tools |
|----------|-------|
| **Knowledge** | `memory.search`, `memory.fetch_chunk`, `memory.ingest_conversation`, `memory.wake_up` |
| **Images** | `memory.search_images`, `memory.ingest_image` |
| **Documents** | `docs.list`, `docs.get`, `docs.save`, `docs.delete`, `docs.move` |
| **Tool Discovery** | `tool.find`, `tool.info` |
| **Tool Lifecycle** | `tool.install`, `tool.run` |
| **Admin** | `setup`, `config`, `maintain`, `observe` |

## Tool Catalog

The curated catalog knows about developer tools, LSPs, and MCP servers — organized by language, category, and type:

```
tools/catalog/
├── languages/     rust.json, python.json, javascript.json, go.json, ...
├── categories/    security.json, testing.json, ci-cd.json, ...
├── mcps/          official.json, community.json
└── _schema.json   Schema for tool entries
```

Each tool has LLM-optimized metadata: `description`, `when_to_use`, `what_it_finds`, `install`, `run`, `risk_level`, `tags`. The LLM matches user intent to tools without us doing any NLP.

Contribute tools via [GitHub PR or Issue](CONTRIBUTING.md).

## Embedding Models

| Tier | Model | Dims | Use Case |
|------|-------|------|----------|
| `fast` | all-MiniLM-L6-v2 | 384 | Getting started |
| `balanced` | BGE-base-en-v1.5 | 768 | Good quality/speed tradeoff |
| `quality` (default) | BGE-large-en-v1.5 | 1024 | **Recommended** |
| `multilingual` | multilingual-e5-large | 1024 | Non-English projects |

17 local text models supported (including quantized variants). Interactive model selection during install. Or use any OpenAI-compatible API (OpenAI, Voyage, Cohere).

**Image embeddings** are also supported — 5 image models (Nomic Vision default, CLIP ViT-B/32, Resnet50, Unicom ViT-B/16+32) for semantic image search. See [Image Search Guide](docs/guides/image-search.md).

## Image Search

Index diagrams, screenshots, and design assets — then find them by description:

```
You: "Find the database schema diagram"
LLM calls: memory.search_images({ query: "database schema tables", limit: 5 })
Returns: ranked matches with similarity scores, OCR text, thumbnail paths
```

Enable with `"image_embedding_enabled": true` in config. OCR text extraction available with tesseract. Screenshot-based image search (image→image similarity) supported via optional `image_base64` field. Browse indexed images in the admin UI at `/images`. See [Image Search Guide](docs/guides/image-search.md).

## Documentation

| Document | Description |
|----------|-------------|
| **[INSTALL.md](INSTALL.md)** | **Complete installation guide** |
| [Quickstart](docs/guides/quickstart.md) | Shortest path to a working setup |
| [Complete Guide](docs/guides/the-one-mcp-complete-guide.md) | Full reference |
| [API Reference](docs/guides/api-reference.md) | All 19 tools + MCP resources schema |
| [Conversation Memory Guide](docs/guides/conversation-memory.md) | Import transcripts, use palace metadata filters, build wake-up packs |
| [Redis Vector Backend](docs/guides/redis-vector-backend.md) | Optional Redis/RediSearch backend settings and persistence expectations |
| [Conversation Memory Benchmarking](docs/benchmarks/conversation-memory-benchmark.md) | Repro checklist for long-memory evaluation runs |
| [MCP Resources Guide](docs/guides/mcp-resources.md) | `resources/list`, `resources/read`, `the-one://` URI scheme |
| [Code Chunking Guide](docs/guides/code-chunking.md) | Tree-sitter AST chunking for 13 languages |
| [Hybrid Search Guide](docs/guides/hybrid-search.md) | Dense + sparse search for exact-match retrieval |
| [Reranking Guide](docs/guides/reranking.md) | Cross-encoder reranking for memory.search |
| [Image Search Guide](docs/guides/image-search.md) | Semantic image search, OCR, thumbnails, screenshot search |
| [Auto-Indexing Guide](docs/guides/auto-indexing.md) | Background file watcher with automatic re-ingestion |
| [Tool Catalog Guide](docs/guides/tool-catalog.md) | 365 curated tools across 10 languages |
| [Backup & Restore Guide](docs/guides/backup-restore.md) | `maintain: backup` / `restore` for moving between machines |
| [Observability Guide](docs/guides/observability.md) | Metrics counters, audit events, debugging with `observe` |
| [Configuration Guide](docs/guides/configuration.md) | All config fields across 5 layers |
| [Troubleshooting](docs/guides/troubleshooting.md) | Symptom-based debugging |
| [Upgrade Guide](docs/guides/upgrade-guide.md) | Version-to-version migration notes |
| [Contributing](CONTRIBUTING.md) | Add tools to the catalog |

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `the-one-core` | Config, storage, policy, profiler, docs manager, limits, **tool catalog** |
| `the-one-mcp` | Async broker, API types, JSON-RPC transport, CLI binary |
| `the-one-memory` | RAG: chunker, embeddings (fastembed + API), async Qdrant, **model registry** |
| `the-one-router` | Rules-first routing, provider pool, health tracking |
| `the-one-registry` | Capability catalog with risk-tier filtering |
| `the-one-claude` | Claude Code adapter |
| `the-one-codex` | Codex adapter |
| `the-one-ui` | Embedded admin UI (dashboard, config, audit, swagger) |

## Build & Release

```bash
# Local builds
bash scripts/build.sh build           # release with swagger
bash scripts/build.sh build --lean    # release without swagger
bash scripts/build.sh check           # full CI pipeline
bash scripts/build.sh info            # show build config

# Cross-platform release (triggers GitHub Actions — manual only)
bash scripts/build.sh release v0.8.0  # build for 6 platforms + create GitHub Release
bash scripts/build.sh release --status # check workflow progress
```

Releases are **manual only** — tagging does not auto-trigger builds. You decide when to build artifacts.

## Stats (v0.14.3)

| Metric | Count |
|--------|-------|
| MCP Tools | 19 |
| MCP Resource Types | 3 (`docs`, `project`, `catalog`) |
| Admin UI Pages | 8 (home, dashboard, ingest, graph, images, config, audit, swagger) |
| Tests | 340 passing (+1 ignored) |
| Rust LOC | ~26,500 |
| JSON Schemas | 35 |
| Catalog Tools | 365 across 10 languages + 8 categories |
| Supported Code Languages (chunker) | 13 |
| `maintain` actions | 15 |
| Metrics counters | 15 |
| Supported Platforms | 6 (Linux/macOS/Windows x86-64 + ARM64) |
| Supported AI CLIs | 4 (Claude Code, Gemini CLI, OpenCode, Codex) |

## License

Apache-2.0
