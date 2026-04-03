# AGENTS.md - The-One MCP Development Guide

This is a Rust-based MCP broker system. All agents should follow these guidelines.

## Build Commands

```bash
# Build entire workspace
cargo build

# Build specific crate
cargo build -p the-one-core

# Run in debug mode
cargo run

# Release build
cargo build --release

# Run a single test
cargo test --package <package-name> <test-name>

# Example: run only the registry tests
cargo test -p the-one-registry

# Run all tests
cargo test

# Run doc tests
cargo test --doc

# Check (fast, no build)
cargo check

# Format check
cargo fmt --check

# Lint with clippy
cargo clippy -- -D warnings

# Full check (fmt + clippy + test)
cargo fmt && cargo clippy && cargo test
```

## Workspace Structure

```
the-one/
├── core/               # Shared business logic
│   ├── registry/       # Capability registry
│   ├── profiler/       # Project profiler
│   ├── router/         # Request router
│   ├── memory/         # Memory/RAG ingestion
│   ├── docs/           # Document management
│   ├── policy/         # Policy engine
│   ├── cache/          # In-memory cache
│   └── schemas/        # Data schemas
├── mcp-server/         # MCP protocol server
├── ui/                 # Embedded web UI
└── adapters/           # CLI-specific adapters
```

## Code Style Guidelines

### Formatting
- Run `cargo fmt` before every commit
- Use 4 spaces for indentation
- Maximum line length: 100 characters
- Use trailing commas in multi-line structs/arrays

### Imports
- Use absolute paths for crate modules: `crate::core::registry`
- Group imports: std → external → internal
- Use `use` for items used more than once; use path prefixes otherwise

### Types
- Prefer explicit type annotations in public APIs
- Use `Result<T, Error>` with custom error types, not `Box<dyn Error>`
- Define error types with `thiserror` for library code
- Use `anyhow` for application/bin code

### Naming
- `snake_case` for variables, functions, methods
- `SCREAMING_SNAKE_CASE` for consts
- `PascalCase` for types, traits, enums
- `kebab-case` for file names

### Error Handling
- Use `?` operator for propagation
- Never silently ignore errors
- Return context-rich errors using `thiserror` or `anyhow`
- Use `Result` for fallible operations, `Option` for optionality

### Async
- Use `tokio` for async runtime
- Prefer async/await over manual futures
- Use `#[tokio::main]` for binary entry points

### Testing
- Unit tests go in `mod tests` at module end
- Integration tests in `tests/` directory
- Use `#[cfg(test)]` guards
- Follow Arrange-Act-Assert pattern
- Name tests descriptively: `fn test_<what>_<expected_behavior>`

### Documentation
- Document public APIs with doc comments
- Include examples for complex functions
- Use `///` for documentation, `//` for implementation comments

## Architecture Principles (from architecture-prompt.md)

1. **Token efficiency first**: Keep always-loaded instructions tiny
2. **Progressive tool exposure**: Expose minimal MCP surface by default
3. **Local-first**: SQLite + Qdrant in `~/.the-one/`, project data in `<repo>/.the-one/`
4. **Rules-first routing**: Nano model is optional enhancement, not primary brain
5. **RAG for discovery, raw markdown for precision**
6. **Single shared backend** with thin CLI satellites

## Storage Strategy

- **Global**: `~/.the-one/` - SQLite DB, Qdrant data, embeddings, logs, registry
- **Project**: `<repo>/.the-one/` - `project.json`, `overrides.json`, `fingerprint.json`

## MCP Tool Surface (expected)

- `project.init`, `project.refresh`, `project.profile.get`
- `memory.search`, `memory.fetch_chunk`
- `docs.list`, `docs.get`, `docs.get_section`
- `tool.search`, `tool.suggest`, `tool.enable`, `tool.run`
- `config.export`

## Important Notes

- Do NOT preload hundreds of tools into the client
- Do NOT store heavy docs in AGENTS.md/CLAUDE.md (keep them tiny)
- Do NOT dump giant raw docs or search payloads by default
- Use fingerprinting for project profile caching
- Separate detected state from user choices in storage