# The-One MCP Quickstart

> Shortest path to a working MCP server connected to your AI assistant.
> Default build uses **SQLite + Qdrant** — the 95% deployment. For
> Postgres-backed state or pgvector-backed vectors (v0.16.0 Phase 2/3),
> see [INSTALL.md § Optional multi-backend features](../../INSTALL.md#optional-multi-backend-features-v0160)
> and the standalone [pgvector-backend.md](pgvector-backend.md) /
> [postgres-state-backend.md](postgres-state-backend.md) guides.

## Option A: One-Command Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

This auto-detects your OS, downloads the latest release, creates default config, imports the tool catalog (365 tools), and registers with every AI CLI it finds.

**Done.** Start an AI coding session — the MCP connects automatically.

## Option B: Build from Source

```bash
git clone https://github.com/michelabboud/the-one-mcp.git
cd the-one-mcp
bash scripts/build.sh build
bash scripts/install.sh --local ./target/release
```

## What Happens After Install

1. You start a Claude Code / Gemini / OpenCode session
2. The MCP server starts automatically (stdio transport)
3. On first use, `setup` initializes your project:
   - Detects your project (languages, frameworks)
   - Imports the tool catalog into SQLite
   - Scans your system for installed tools
   - Indexes your docs into the RAG engine
4. Now you can:
   - Ask about your code → `memory.search` finds relevant docs
   - Search images → `memory.search_images` finds screenshots and diagrams
   - Ask for tool recommendations → `tool.find` returns what's available
   - Save knowledge → `docs.save` persists it across sessions
   - Run tools → `tool.run` with policy-gated approval

## Register Manually (if needed)

```bash
# Claude Code
claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve

# Gemini CLI
gemini mcp add the-one-mcp ~/.the-one/bin/the-one-mcp serve

# OpenCode
opencode mcp add --name the-one-mcp --command ~/.the-one/bin/the-one-mcp --args serve
```

## Configure (Optional)

Everything works with defaults. To customize:

```bash
$EDITOR ~/.the-one/config.json
```

```json
{
  "embedding_model": "balanced",
  "limits": { "max_search_hits": 10 }
}
```

## Key Commands the LLM Uses

| What you say | What the LLM calls |
|-------------|-------------------|
| "Check my code for security issues" | `tool.find({ mode: "suggest", query: "security" })` → `tool.run(...)` |
| "How does our auth system work?" | `memory.search("auth system")` |
| "Save a note about this decision" | `docs.save("decisions/auth-choice.md", content)` |
| "What tools do I have?" | `tool.find({ mode: "list", filter: "enabled" })` |
| "Install cargo-audit" | `tool.install({ tool_id: "cargo-audit" })` |

## Full Docs

- **[INSTALL.md](../../INSTALL.md)** — complete installation guide
- **[Complete Guide](the-one-mcp-complete-guide.md)** — all features including image search and reranking
- **[MemPalace Operations Guide](mempalace-operations.md)** — profile presets, AAAK, diary, navigation, hook capture
- **[Operator Runbook](../ops/operator-runbook.md)** — backup, incident triage, multi-backend operations
- **[Multi-Backend Operations](multi-backend-operations.md)** — deployment matrix across SQLite/Postgres/Qdrant/pgvector
- **[pgvector Backend](pgvector-backend.md)** — v0.16.0 Phase 2 standalone guide
- **[Postgres State Backend](postgres-state-backend.md)** — v0.16.0 Phase 3 standalone guide
- **[Tool Ecosystem](../plans/tool-ecosystem-architecture.md)** — catalog vision
