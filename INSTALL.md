# Installing the-one-mcp

## One-Command Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

This will:
1. Detect your OS (Linux, macOS, Windows) and architecture (x86-64, ARM64)
2. Download the latest release binary from GitHub
3. Install to `~/.the-one/bin/`
4. Create default config with sensible defaults
5. Download the curated tool catalog (28+ tools)
6. Auto-detect Claude Code, Gemini CLI, OpenCode, and Codex
7. Register the MCP server with every detected AI assistant
8. Run a smoke test to verify everything works

**After install, start an AI coding session — the MCP connects automatically.**

## Install Options

```bash
# Install specific version
bash install.sh --version v0.8.0

# Install from a local build (after building from source)
bash install.sh --local ./target/release

# Install the lean binary (no Swagger UI, smaller)
bash install.sh --lean

# Non-interactive (for CI/automation)
bash install.sh --yes --skip-register

# Uninstall
bash install.sh --uninstall
```

## Build from Source

```bash
git clone https://github.com/michelabboud/the-one-mcp.git
cd the-one-mcp

# Using the build script (recommended)
bash scripts/build.sh build              # release, with swagger
bash scripts/build.sh build --lean       # release, no swagger (smaller)
bash scripts/build.sh build --with-ui    # also build admin UI binary

# Or raw cargo
cargo build --release -p the-one-mcp --bin the-one-mcp

# Then install from the local build
bash scripts/install.sh --local ./target/release
```

## What Gets Installed

```
~/.the-one/
├── bin/
│   ├── the-one-mcp              MCP server binary
│   └── embedded-ui              Admin UI binary
├── config.json                  Global config (sensible defaults, editable)
├── catalog.db                   Tool catalog (SQLite: 28+ tools, FTS5 search)
├── registry/
│   ├── recommended.json         Pre-built tools (auto-updated from GitHub)
│   ├── custom.json              Your shared custom tools (all CLIs)
│   ├── custom-claude.json       Claude Code only
│   ├── custom-gemini.json       Gemini CLI only
│   ├── custom-opencode.json     OpenCode only
│   └── custom-codex.json        Codex only
├── schemas/                     v1beta JSON schemas (35 files)
└── .fastembed_cache/            ONNX embedding model (auto-downloaded on first use)
```

## Register with Your AI Assistant

The installer auto-registers with detected CLIs. To register manually:

```bash
# Claude Code
claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve

# Gemini CLI
gemini mcp add the-one-mcp ~/.the-one/bin/the-one-mcp serve

# OpenCode
opencode mcp add --name the-one-mcp --command ~/.the-one/bin/the-one-mcp --args serve

# Codex — add to your Codex MCP config
```

## Verify Installation

```bash
# Smoke test
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ~/.the-one/bin/the-one-mcp serve

# List all 17 tools (v0.8.0)
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | ~/.the-one/bin/the-one-mcp serve
```

## Configure (Optional)

Everything works with defaults — no configuration required. To customize:

```bash
$EDITOR ~/.the-one/config.json
```

### Minimal Config

```json
{
  "embedding_model": "quality",
  "limits": {
    "max_search_hits": 10,
    "max_chunk_tokens": 1024
  }
}
```

### Embedding Models

| Tier | Model | Dims | Use Case |
|------|-------|------|----------|
| `fast` | all-MiniLM-L6-v2 | 384 | Getting started |
| `balanced` | BGE-base-en-v1.5 | 768 | Good quality/speed tradeoff |
| `quality` (default) | BGE-large-en-v1.5 | 1024 | **Recommended** |
| `multilingual` | multilingual-e5-large | 1024 | Non-English projects |

The installer prompts you to choose a model interactively. Use `--yes` to accept the default (quality tier). 17 local models plus API options (OpenAI, Voyage, Cohere) are available.

### Add Nano LLM Routing (Optional)

If you have Ollama, LiteLLM, or any OpenAI-compatible endpoint:

```json
{
  "nano_routing_policy": "priority",
  "nano_providers": [
    {
      "name": "ollama",
      "base_url": "http://localhost:11434/v1",
      "model": "qwen2:0.5b",
      "timeout_ms": 500,
      "enabled": true
    }
  ]
}
```

### Full Config Reference

See the [Complete Guide](docs/guides/the-one-mcp-complete-guide.md) for all config fields, environment variables, and limits.

## Transport Modes

| Transport | Use Case | Command |
|-----------|----------|---------|
| `stdio` (default) | Claude Code, Gemini CLI, OpenCode, Codex | `the-one-mcp serve` |
| `sse` | Web clients | `the-one-mcp serve --transport sse --port 3000` |
| `stream` | MCP-spec HTTP clients | `the-one-mcp serve --transport stream --port 3000` |

## Admin UI (Optional)

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" ~/.the-one/bin/embedded-ui
```

Open `http://127.0.0.1:8787/dashboard` for config, metrics, audit events, and Swagger UI.

## Custom Tools

```bash
# Shared across all CLIs
$EDITOR ~/.the-one/registry/custom.json

# Per-CLI custom tools
$EDITOR ~/.the-one/registry/custom-claude.json
$EDITOR ~/.the-one/registry/custom-gemini.json
```

Tools added via `tool.add` MCP command are stored here automatically.

## Update

```bash
# Re-run the installer to update to latest version
bash install.sh

# Or update just the tool catalog
# (from within an AI coding session)
# The LLM calls: tool.update
```

## Uninstall

```bash
bash install.sh --uninstall
```

This removes binaries and recommended tools but preserves your config and custom tools. To remove everything:

```bash
rm -rf ~/.the-one
```

## Supported Platforms

| Platform | Architecture | Status |
|----------|-------------|--------|
| Linux | x86-64 | Fully supported |
| Linux | ARM64 | Fully supported |
| macOS | x86-64 (Intel) | Supported — lean build by default; local embeddings via `local-embeddings-dynamic` + `brew install onnxruntime` (see below) |
| macOS | ARM64 (Apple Silicon) | Fully supported |
| Windows | x86-64 | Supported (binary works, bash scripts need Git Bash/MSYS2) |
| Windows | ARM64 | Supported |

### Intel Mac local embeddings (v0.11.0)

Default Intel Mac binaries ship lean (API embeddings only) because the
upstream `ort-sys 2.0.0-rc.11` crate dropped prebuilt ONNX Runtime libraries
for `x86_64-apple-darwin` in early 2026. v0.11.0 adds a dynamic-loading
alternative that works on Intel Mac:

```bash
# 1. Install libonnxruntime via Homebrew (one-time)
brew install onnxruntime

# 2. Build from source with the dynamic feature
cargo build --release -p the-one-mcp \
    --no-default-features \
    --features "embed-swagger,local-embeddings-dynamic"
```

The resulting binary resolves `libonnxruntime.dylib` at startup from the
standard Homebrew location. Performance should match the bundled ort
backend on other platforms. If `libonnxruntime` cannot be found at
runtime the binary will fall back to API embeddings with a warning.

Users who prefer API embeddings (OpenAI, Voyage, Cohere) can continue
to use the lean binary — nothing about this release changes that path.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `command not found: the-one-mcp` | Add `~/.the-one/bin` to PATH, or re-run installer |
| Slow first search | ONNX embedding model downloading (~130MB for quality tier), cached after |
| `remote qdrant requires api key` | Set `qdrant_api_key` in config, or `qdrant_strict_auth: false` |
| No search results | Run `maintain (action: tool.refresh)` to refresh catalog, or `maintain (action: reindex)` for docs |
| Installer can't download | Use `--local ./target/release` after building from source |
| `tesseract not found` error | Image OCR requires tesseract on host: `apt install tesseract-ocr` / `brew install tesseract` / `pacman -S tesseract`. Or disable with `"image_ocr_enabled": false` in config. |
