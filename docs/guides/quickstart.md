# The-One MCP Quickstart

Shortest path to build, run, and connect the MCP server.

## 1. Build

```bash
cargo build --release -p the-one-mcp --bin the-one-mcp
```

## 2. Run

```bash
# Stdio transport (default — for Claude Code / Codex)
./target/release/the-one-mcp serve

# SSE transport (for web clients)
./target/release/the-one-mcp serve --transport sse --port 3000

# Streamable HTTP transport
./target/release/the-one-mcp serve --transport stream --port 3000
```

## 3. Connect to Claude Code

```bash
claude mcp add the-one-mcp -- /absolute/path/to/the-one-mcp serve
```

## 4. Verify

```bash
# Send an initialize request via stdio
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/the-one-mcp serve

# List available tools
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | ./target/release/the-one-mcp serve
```

## 5. Configure (Optional)

Create `~/.the-one/config.json` for global settings, or `<project>/.the-one/config.json` for project-specific:

```json
{
  "embedding_provider": "local",
  "embedding_model": "all-MiniLM-L6-v2",
  "nano_routing_policy": "priority",
  "nano_providers": [
    {
      "name": "ollama",
      "base_url": "http://localhost:11434/v1",
      "model": "qwen2:0.5b",
      "api_key": null,
      "timeout_ms": 500,
      "enabled": true
    }
  ],
  "limits": {
    "max_search_hits": 10,
    "max_chunk_tokens": 512
  }
}
```

## 6. Admin UI (Optional)

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" \
  cargo run -p the-one-ui --bin embedded-ui
```

Open: `http://127.0.0.1:8787/dashboard`

## 7. Quality Gate

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/release-gate.sh
```

## What's Next

- [Complete Guide](the-one-mcp-complete-guide.md) — full configuration, embeddings, provider pool, limits
- [Operator Runbook](../ops/operator-runbook.md) — backup/restore, incident triage
- [Architecture](../plans/the-one-mcp-architecture-prompt.md) — design rationale
