# the-one-mcp

Rust MCP broker workspace with project lifecycle, docs/memory retrieval, policy-gated tools,
Claude/Codex adapters, and an embedded admin UI runtime.

## Quick Links

- Quickstart: `docs/guides/quickstart.md`
- Complete guide: `docs/guides/the-one-mcp-complete-guide.md`
- Operator runbook: `docs/ops/operator-runbook.md`
- Release notes: `docs/releases/v1beta-upgrade-notes.md`
- Plans: `docs/plans/the-one-mcp-implementation-plan.md`

## Current Capabilities

- `project.*`: init/refresh/profile
- `memory.*` and `docs.*`: markdown ingestion, section retrieval, vector search paths
- `tool.*`: search/suggest/enable/run with approval policy
- `config.export`, `metrics.snapshot`, `audit.events`
- Qdrant HTTP integration with auth/TLS knobs + strict remote auth mode
- Optional compile-time embedded swagger (`embed-swagger`, default enabled)

## Build and Verify

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Release builds:

```bash
cargo build --release --workspace
cargo build --release -p the-one-mcp --no-default-features
```

## Embedded UI

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

Default endpoints:

- `http://127.0.0.1:8787/dashboard`
- `http://127.0.0.1:8787/api/health`
- `http://127.0.0.1:8787/swagger` (interactive Swagger UI)
- `http://127.0.0.1:8787/api/swagger` (raw OpenAPI JSON)
- `http://127.0.0.1:8787/audit`
- `http://127.0.0.1:8787/config` (editable config form)
