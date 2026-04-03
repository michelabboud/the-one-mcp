# The-One MCP Quickstart

Shortest path to a working MCP server connected to your AI assistant.

## Option A: One-Command Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

This auto-detects your OS, downloads the latest release, creates default config, and registers with every AI CLI it finds (Claude Code, Gemini CLI, OpenCode, Codex).

**Done.** Start an AI coding session — the MCP connects automatically.

## Option B: Build from Source

```bash
git clone https://github.com/michelabboud/the-one-mcp.git
cd the-one-mcp

# Build
bash scripts/build.sh build

# Install to ~/.the-one/bin/ and register with CLIs
bash scripts/install.sh --local ./target/release
```

## Register with Your AI Assistant

If the installer didn't auto-register (or you want to do it manually):

```bash
# Claude Code
claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve

# Gemini CLI
gemini mcp add the-one-mcp ~/.the-one/bin/the-one-mcp serve

# OpenCode
opencode mcp add --name the-one-mcp --command ~/.the-one/bin/the-one-mcp --args serve
```

## Verify

```bash
# Quick smoke test
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ~/.the-one/bin/the-one-mcp serve
```

## Configure (Optional)

Everything works with defaults. To customize:

```bash
$EDITOR ~/.the-one/config.json
```

Common tweaks:
```json
{
  "embedding_model": "balanced",
  "limits": {
    "max_search_hits": 10,
    "max_chunk_tokens": 1024
  }
}
```

### Embedding Model Tiers

| Tier | Dims | Use Case |
|------|------|----------|
| `fast` (default) | 384 | Getting started |
| `balanced` | 768 | Production recommended |
| `quality` | 1024 | Best local quality |
| `multilingual` | 1024 | Non-English projects |

### Add Nano LLM Routing (Optional)

If you have Ollama running:
```json
{
  "nano_providers": [{
    "name": "ollama",
    "base_url": "http://localhost:11434/v1",
    "model": "qwen2:0.5b",
    "timeout_ms": 500,
    "enabled": true
  }]
}
```

## Custom Tools

```bash
# Shared across all CLIs
$EDITOR ~/.the-one/registry/custom.json

# For a specific CLI only
$EDITOR ~/.the-one/registry/custom-claude.json
$EDITOR ~/.the-one/registry/custom-gemini.json
$EDITOR ~/.the-one/registry/custom-opencode.json
```

## Admin UI (Optional)

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" ~/.the-one/bin/embedded-ui
```

Open `http://127.0.0.1:8787/dashboard`

## Transport Modes

| Transport | Use Case | Command |
|-----------|----------|---------|
| `stdio` (default) | Claude Code, Gemini, OpenCode, Codex | `the-one-mcp serve` |
| `sse` | Web clients | `the-one-mcp serve --transport sse --port 3000` |
| `stream` | MCP-spec HTTP clients | `the-one-mcp serve --transport stream --port 3000` |

## What's Next

- [Complete Guide](the-one-mcp-complete-guide.md) — all 19 sections: config, embeddings, provider pool, limits, per-CLI tools
- [Operator Runbook](../ops/operator-runbook.md) — backup/restore, incident triage
