# Progress Report

## Current Version: v0.4.0

## Overall Status

All planned stages complete. Four major releases shipped:
- **v0.1.0** — Initial workspace: 8 crates, 14 MCP tools, stub implementations
- **v0.2.0** — Production overhaul: async broker, real embeddings, 3 transports, 24 tools
- **v0.3.0** — Tool catalog: SQLite + Qdrant semantic search, tool lifecycle, 31 tools
- **v0.4.0** — Embedding model registry: TOML-based model registries, quality tier default, interactive installer selection, 33 tools

Build/test gates: all green. 174 tests, 0 failures.

## Stats

| Metric | v0.1.0 | v0.2.0 | v0.3.0 | v0.4.0 |
|--------|--------|--------|--------|--------|
| MCP Tools | 14 | 24 | 31 | **33** |
| Tests | 68 | 122 | 135 | **174** |
| Rust LOC | 6,400 | ~10,000 | ~12,800 | **~14,000** |
| JSON Schemas | 33 | 49 | 63 | **63** |
| Catalog Tools | — | — | 28 | **28** |
| Platforms | 1 | 1 | 6 | **6** |
| AI CLIs | 2 | 2 | 4 | **4** |

## Stage Progress (v0.1.0)

- Stage 0: Program setup — complete
- Stage 1: Core foundations — complete
- Stage 2: Isolation/lifecycle — complete
- Stage 3: Profiler/fingerprint — complete
- Stage 4: Registry/policy/approvals — complete
- Stage 5: Docs/RAG plane — complete
- Stage 6: Router rules+nano — complete
- Stage 7: MCP contracts/versioning — complete
- Stage 8: Claude/Codex parity — complete
- Stage 9: UI/ops/hardening — complete

## Production Overhaul (v0.2.0) — 23 Tasks

All complete: async broker, fastembed embeddings, smart chunker, async Qdrant, provider pool with health tracking, managed documents with soft-delete, configurable limits, stdio/SSE/streamable HTTP transports, clap CLI binary.

## Multi-CLI + Installer (v0.2.1)

All complete: Claude Code + Gemini CLI + OpenCode + Codex auto-detection, tiered embedding models (fast/balanced/quality/multilingual), per-CLI custom tools, install.sh one-command installer, build.sh build manager, cross-platform release workflow.

## Tool Catalog Integration (v0.3.0) — 7 Tasks

- Cat-1: SQLite schema + error variant — complete
- Cat-2: ToolCatalog struct with DB, import, query, scan — complete
- Cat-3: API types for 6 new tools — complete
- Cat-4: Broker methods for catalog tools — complete
- Cat-5: Transport dispatch + catalog bootstrap — complete
- Cat-6: Qdrant semantic search for tools — complete
- Cat-7: Changelog, schemas, docs, validation, tag — complete

## Key Features Delivered

### Tool Catalog (v0.3.0)
- SQLite catalog.db with FTS5 full-text search
- Qdrant semantic search over tool descriptions (with FTS5 fallback)
- System inventory scanning (auto-detects installed tools via `which`)
- Per-CLI per-project tool enable/disable state
- 7 new MCP tools: tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update, tool.list
- Curated catalog seed: 16 Rust tools, 4 security tools, 8 official MCPs
- tool.suggest returns grouped results: enabled / available / recommended
- tool.search: semantic (Qdrant) → FTS5 → registry fallback chain

### Production RAG (v0.2.0)
- fastembed-rs with tiered models (384-1024 dim ONNX, offline, free)
- OpenAI-compatible API embedding provider
- Smart markdown chunker (heading-aware, paragraph-safe, code-block preserving)
- Async Qdrant HTTP backend with collection management

### Managed Documents (v0.2.0)
- Full CRUD with soft-delete to .trash/
- Auto-sync on project.refresh
- docs.reindex for full re-ingestion

### Multi-CLI Support (v0.2.1)
- Claude Code, Gemini CLI, OpenCode, Codex
- Per-CLI custom tools
- One-command installer with auto-registration

### MCP Transport (v0.2.0)
- stdio (Claude Code, Gemini, OpenCode, Codex)
- SSE (web clients)
- Streamable HTTP (MCP spec compliant)

### Nano LLM Provider Pool (v0.2.0)
- Up to 5 OpenAI-compatible providers
- Priority / round-robin / latency routing
- Per-provider health tracking with cooldown
- TCP pre-flight checks

## Embedding Model Registry (v0.4.0) — 8 Tasks

- Task 1+2: TOML registry files + models_registry module — complete
- Task 3: Rewrite embeddings.rs to use registry — complete
- Task 4: Update config defaults to quality tier — complete
- Task 5: Add models.list and models.check_updates MCP tools — complete
- Task 6: Interactive model selection in installer — complete
- Task 7: Maintenance scripts — complete
- Task 8: Full integration validation — complete

### Key Features Delivered

- TOML model registries (models/local-models.toml, models/api-models.toml) embedded in binary
- Default changed from all-MiniLM-L6-v2 (384d) to BGE-large-en-v1.5 (1024d)
- Interactive model selection during install (7 local + API option)
- API provider support: OpenAI, Voyage AI, Cohere (extensible)
- 2 new MCP tools: models.list, models.check_updates
- Maintenance scripts for tracking upstream model updates

## Infrastructure (v0.3.1)

- SECURITY.md with vulnerability reporting policy and security design documentation
- Hardened .gitignore (secrets, keys, certs, IDE, OS files)
- Weekly cargo-audit + gitleaks CI (security.yml)
- Manual-only release workflow (workflow_dispatch, no auto-trigger on tags)
- `build.sh release` command for triggering cross-platform builds
- Repo made public — curl one-liner install works
- GitHub Release v0.3.1 with 4 platform binaries (Linux x86, macOS x86+ARM, Windows x86)

## Verification Snapshot

- `cargo fmt --check` — passing
- `cargo clippy --workspace --all-targets -- -D warnings` — passing
- `cargo test --workspace` — **174 tests passing**
- `cargo build --release -p the-one-mcp --bin the-one-mcp` — passing
- `bash scripts/release-gate.sh` — passing
- `bash scripts/build.sh check` — full CI pipeline passing

## What's Next

### Near-Term
- Fill catalog: Python, JavaScript, Go, Java language files (~200 tools)
- GitHub Pages tool submission form (low-friction community contributions)
- GitHub Actions catalog validation + security checks
- Pre-built catalog snapshots (SQLite + Qdrant) in GitHub Releases

### Medium-Term
- Community contribution pipeline (PR template, auto-validation)
- Periodic security re-check cron (GitHub Advisory Database)
- Tool ratings and trust level promotion
- clientInfo-based tool loading (per-CLI catalog filtering)

### Future
- Web marketplace for browsing, rating, and reviewing tools
- Community-curated "markets" (collections by use case)
- Automated tool discovery from package registries
- Install analytics and usage tracking
