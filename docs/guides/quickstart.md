# The-One MCP Quickstart

This is the shortest path to run and verify `the-one-mcp` locally.

For full documentation, see:

- `docs/guides/the-one-mcp-complete-guide.md`

## 1) Verify Toolchain

```bash
cargo --version
rustc --version
```

## 2) Validate Workspace

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 3) Build Release

```bash
# default includes embedded swagger
cargo build --release --workspace

# optional: build MCP without embedded swagger
cargo build --release -p the-one-mcp --no-default-features
```

## 4) Launch Embedded UI

```bash
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui
```

You can also set defaults in JSON config (`${THE_ONE_HOME:-$HOME/.the-one}/config.json` or `<project>/.the-one/config.json`) with keys:

- `project_root`
- `project_id`
- `ui_bind`

Open:

- `http://127.0.0.1:8787/dashboard`
- `http://127.0.0.1:8787/api/health`
- `http://127.0.0.1:8787/swagger` (Swagger UI, 404 if built without `embed-swagger`)
- `http://127.0.0.1:8787/api/swagger` (raw OpenAPI JSON)
- `http://127.0.0.1:8787/audit`
- `http://127.0.0.1:8787/config` (editable config form saved via `/api/config`)

## 5) Run Release Gate

```bash
bash scripts/release-gate.sh
```

## 6) Next Docs

- Operations: `docs/ops/operator-runbook.md`
- Release notes: `docs/releases/v1beta-upgrade-notes.md`
- ADRs: `docs/adr/`
