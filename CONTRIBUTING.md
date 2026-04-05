# Contributing to the-one-mcp

## Adding Tools to the Catalog

The tool catalog lives in `tools/catalog/`. We welcome contributions of developer tools, LSPs, and MCP servers.

### Option A: Submit via GitHub Issue (Easiest)

Open a [New Tool Suggestion](../../issues/new?template=tool-suggestion.md) issue with:
- Tool name and GitHub URL
- What it does (one sentence)
- Install command
- Run command
- Which languages/frameworks it supports

We'll add it to the catalog.

### Option B: Pull Request (Developer-Friendly)

1. Fork this repo
2. Add your tool to the appropriate file:
   - Language-specific: `tools/catalog/languages/<lang>.json`
   - Cross-language: `tools/catalog/categories/<category>.json`
   - MCP server: `tools/catalog/mcps/community.json`
3. Follow the schema in `tools/catalog/_schema.json`
4. Open a PR

### Tool Entry Format

```json
{
  "id": "your-tool-id",
  "name": "Your Tool",
  "type": "cli",
  "category": ["testing", "qa"],
  "languages": ["rust"],
  "description": "One-line description for LLM consumption",
  "when_to_use": "When this tool is most useful",
  "what_it_finds": "What issues it detects (optional)",
  "install": {
    "command": "cargo install your-tool",
    "binary_name": "your-tool"
  },
  "run": {
    "command": "your-tool run",
    "common_flags": ["--flag1", "--flag2"]
  },
  "risk_level": "low",
  "requires": ["cargo"],
  "cli_support": ["claude", "gemini", "opencode", "codex"],
  "tags": ["relevant", "tags"],
  "github": "https://github.com/you/your-tool"
}
```

### Required Fields

- `id` — unique, lowercase, hyphenated
- `name` — display name
- `type` — `cli`, `lsp`, `mcp`, `framework`, `library`, `extension`
- `category` — at least one from: build, test, lint, qa, security, ci-cd, database, cloud, docs, automation, monitoring, formatting, debugging, profiling, coverage, mutation-testing, e2e, api-testing, load-testing
- `description` — one clear sentence
- `install.command` — how to install
- `run.command` — how to run

### LLM-Friendly Descriptions

The `description`, `when_to_use`, and `what_it_finds` fields are read by LLMs to match tools to user intent. Write them as if explaining to a smart colleague:

- **Good:** "Audit Cargo.lock for crates with known security vulnerabilities from the RustSec Advisory Database"
- **Bad:** "Security tool for Rust"

### Trust Levels

New submissions start as `community`. Promotion to `verified` happens after review:
- `verified` — well-known, maintained, widely used
- `community` — submitted by users, validated schema
- `unverified` — new submission, awaiting review

## Contributing Code

### Setup

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Guidelines

- Follow existing patterns in the codebase
- `thiserror` for library code, `anyhow` for binaries
- All broker methods are `async`
- Tests use `#[tokio::test]` for async, `#[test]` for sync
- Config tests must use `temp_env::with_vars`

### PR Process

1. Fork and create a feature branch
2. Make changes with tests
3. Run `bash scripts/build.sh check`
4. Open PR with description of what and why

### PR Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes (272+ tests)
- [ ] If touching `the-one-memory`: confirm `image-embeddings` feature still compiles (`cargo check -p the-one-memory --features image-embeddings`)
- [ ] If touching language chunkers (`chunker_rust.rs`, `chunker_python.rs`, `chunker_typescript.rs`, `chunker_go.rs`): add/update chunker unit tests covering edge cases for that language
- [ ] If touching image ingest or OCR: add tests covering path traversal rejection and file size limits
- [ ] If touching reranker: add unit tests for score ordering and empty-input edge case
- [ ] Schema changes: update corresponding file in `schemas/mcp/v1beta/` (35 schemas total)
