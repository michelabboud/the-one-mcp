# The-One MCP Quickstart

Shortest path to a working MCP server connected to your AI assistant.

## Option A: One-Command Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

This auto-detects your OS, downloads the latest release, creates default config, imports the tool catalog (28+ tools), and registers with every AI CLI it finds.

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
3. On first `project.init`, it:
   - Detects your project (languages, frameworks)
   - Imports the tool catalog into SQLite
   - Scans your system for installed tools
   - Indexes your docs into the RAG engine
4. Now you can:
   - Ask about your code → `memory.search` finds relevant docs
   - Ask for tool recommendations → `tool.suggest` returns what's available
   - Save knowledge → `docs.create` persists it across sessions
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
| "Check my code for security issues" | `tool.suggest({ category: "security" })` → `tool.run(...)` |
| "How does our auth system work?" | `memory.search("auth system")` |
| "Save a note about this decision" | `docs.create("decisions/auth-choice.md", content)` |
| "What tools do I have?" | `tool.list({ state: "enabled" })` |
| "Install cargo-audit" | `tool.install({ tool_id: "cargo-audit" })` |

## Full Docs

- **[INSTALL.md](../../INSTALL.md)** — complete installation guide
- **[Complete Guide](the-one-mcp-complete-guide.md)** — 19 sections, all features
- **[Operator Runbook](../ops/operator-runbook.md)** — backup, incident triage
- **[Tool Ecosystem](../plans/tool-ecosystem-architecture.md)** — catalog vision
