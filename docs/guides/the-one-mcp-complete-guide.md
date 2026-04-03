# The-One MCP Complete Guide

This guide covers setup, installation, features, usage, operations, and deployment options for `the-one-mcp`.

## 1) What This Project Is

`the-one-mcp` is a Rust MCP broker workspace that provides:

- project lifecycle and profiling (`project.init`, `project.refresh`, `project.profile.get`)
- memory/docs ingestion and retrieval (`memory.*`, `docs.*`)
- capability registry and policy-gated tool execution (`tool.*`)
- config export and observability (`config.export`, `metrics.snapshot`, `audit.events`)
- adapters for Claude and Codex
- embedded admin UI runtime

Workspace crates:

- `crates/the-one-core` - config, manifests, policy, storage, project lifecycle
- `crates/the-one-memory` - ingestion, vector/search backends, retrieval
- `crates/the-one-router` - rules+nano routing and route telemetry
- `crates/the-one-registry` - capability catalog/suggestion/search
- `crates/the-one-mcp` - API contracts and broker orchestration
- `crates/the-one-claude` - Claude adapter
- `crates/the-one-codex` - Codex adapter
- `crates/the-one-ui` - admin operations + embedded UI runtime

## 2) Prerequisites

- Rust stable toolchain
- Cargo
- Linux/macOS/WSL recommended

Optional infrastructure:

- Qdrant server for remote vector storage

## 3) Install and Build

### Clone and initial check

```bash
cargo check --workspace
```

### Standard quality gate

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Release builds

```bash
cargo build --release --workspace
```

### Release gate script (recommended before shipping)

```bash
bash scripts/release-gate.sh
```

## 4) Compile-Time Features

`the-one-mcp` features:

- `embed-swagger` (default: enabled)

Meaning:

- when enabled, OpenAPI/Swagger JSON is embedded in the binary
- when disabled, embedded swagger payload is not compiled in

Build examples:

```bash
# default (swagger embedded)
cargo build --release -p the-one-mcp

# explicit on
cargo build --release -p the-one-mcp --features embed-swagger

# explicit off
cargo build --release -p the-one-mcp --no-default-features
```

## 5) Configuration

Config precedence (lowest -> highest):

1. defaults
2. global file: `${THE_ONE_HOME:-$HOME/.the-one}/config.json`
3. project file: `<project>/.the-one/config.json`
4. environment variables
5. runtime overrides

### Core config fields

- `provider` (`local` or `hosted`)
- `log_level`
- `qdrant_url`
- `qdrant_api_key` (optional)
- `qdrant_ca_cert_path` (optional)
- `qdrant_tls_insecure` (bool)
- `qdrant_strict_auth` (bool, default true)
- `nano_provider` (`rules`, `api`, `ollama`, `lmstudio`)
- `nano_model`

### Embedded UI config fields

These can be set in global or project `config.json` and are overridden by env vars:

- `project_root`
- `project_id`
- `ui_bind` (for example `127.0.0.1:8787`)

### Environment variables

- `THE_ONE_HOME`
- `THE_ONE_PROVIDER`
- `THE_ONE_LOG_LEVEL`
- `THE_ONE_QDRANT_URL`
- `THE_ONE_QDRANT_API_KEY`
- `THE_ONE_QDRANT_CA_CERT_PATH`
- `THE_ONE_QDRANT_TLS_INSECURE`
- `THE_ONE_QDRANT_STRICT_AUTH`
- `THE_ONE_NANO_PROVIDER`
- `THE_ONE_NANO_MODEL`

Embedded UI binary environment variables:

- `THE_ONE_PROJECT_ROOT`
- `THE_ONE_PROJECT_ID`
- `THE_ONE_UI_BIND` (default `127.0.0.1:8787`)

## 6) Project State Layout

Global:

- `${THE_ONE_HOME:-$HOME/.the-one}/`

Project local:

- `<project>/.the-one/project.json`
- `<project>/.the-one/overrides.json`
- `<project>/.the-one/fingerprint.json`
- `<project>/.the-one/pointers.json`
- `<project>/.the-one/state.db`
- `<project>/.the-one/qdrant/` (local index fallback path)

## 7) Features and Behavior

### 7.1 Lifecycle + profiling

- Initializes project state and profile snapshot
- Refresh supports cached profile reuse when fingerprint unchanged

### 7.2 Memory/docs

- Ingests markdown trees
- Chunks by headings
- Supports docs list/get/get_section
- Supports memory search/fetch chunk

Vector backend strategy:

- remote Qdrant HTTP backend (preferred when reachable/configured)
- local persisted qdrant-like index fallback
- keyword vector fallback

### 7.3 Security for Qdrant

- strict remote auth guard: when remote Qdrant is configured and `qdrant_strict_auth=true`, API key is required
- TLS knobs:
  - custom CA cert path
  - insecure TLS toggle (for controlled environments)

### 7.4 Router

- rules-first routing
- optional nano provider classification
- hard budget bounds for timeout/retries
- deterministic fallback path
- route telemetry with provider path, confidence, fallback, attempts, bounds, last error

### 7.5 Policy + approvals

- interactive approval scopes: once, session, forever
- headless high-risk default deny unless prior approval exists
- persisted approvals and audit trail in project DB

### 7.6 Observability

- metrics snapshot counters (project, memory, tool, router fallback/errors, latency totals)
- audit events query endpoint

## 8) Broker API Surface (Rust)

Primary broker methods in `crates/the-one-mcp/src/broker.rs`:

- `project_init`
- `project_refresh`
- `project_profile_get`
- `ingest_docs`
- `memory_search`
- `memory_fetch_chunk`
- `docs_list`
- `docs_get`
- `docs_get_section`
- `tool_suggest`
- `tool_search`
- `tool_enable`
- `tool_run`
- `config_export`
- `metrics_snapshot`
- `audit_events`

Contract structs are defined in `crates/the-one-mcp/src/api.rs` and schemas in `schemas/mcp/v1beta/`.

## 9) Embedded UI Runtime

Run:

```bash
cargo run -p the-one-ui --bin embedded-ui
```

Routes:

- `GET /dashboard`
- `GET /api/health`
- `GET /swagger` (interactive Swagger UI, 404 when swagger embedding is disabled)
- `GET /api/swagger` (raw OpenAPI JSON)
- `GET /audit`
- `GET /config`
- `POST /api/config` (validated config updates; used by the editable config page)

## 10) Swagger/OpenAPI

Embedded source file:

- `schemas/mcp/v1beta/openapi.swagger.json`

Runtime access:

- MCP helper: `the_one_mcp::swagger::embedded_swagger_json()`
- UI endpoints: `GET /swagger` and `GET /api/swagger`

## 11) Adapters

Adapter crates:

- `crates/the-one-claude`
- `crates/the-one-codex`

Shared behavior core:

- `crates/the-one-mcp/src/adapter_core.rs`

Both adapters expose aligned flows for init/refresh/config/audit and parity-tested ingest/tool execution paths.

## 12) Operations

Manual backup/restore APIs are available via core and UI wrappers.

See:

- `docs/ops/operator-runbook.md`
- `docs/releases/v1beta-upgrade-notes.md`

## 13) CI and Release

CI pipeline:

- `.github/workflows/ci.yml`

Jobs:

- `rust` (check/fmt/clippy/test)
- `release-gate` (scripted release checks)

Release validation command:

```bash
bash scripts/release-gate.sh
```

## 14) Quick Start Example

```bash
# 1) Ensure code quality
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# 2) Build release with embedded swagger
cargo build --release --workspace

# 3) Launch embedded UI on current repo
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="demo" cargo run -p the-one-ui --bin embedded-ui

# 4) Open
# http://127.0.0.1:8787/dashboard
# http://127.0.0.1:8787/api/health
# http://127.0.0.1:8787/api/swagger
```

## 15) Troubleshooting

- `remote qdrant requires api key...`
  - set `qdrant_api_key` or disable strict mode for non-production testing
- Swagger endpoint returns 404
  - build with default features or `--features embed-swagger`
- No docs hits
  - ensure docs are ingested for that project (`ingest_docs`) and query is non-empty
- Headless tool run denied
  - expected for high-risk actions without prior approval

## 16) Reference Files

- `crates/the-one-core/src/config.rs`
- `crates/the-one-core/src/project.rs`
- `crates/the-one-core/src/storage/sqlite.rs`
- `crates/the-one-memory/src/lib.rs`
- `crates/the-one-router/src/lib.rs`
- `crates/the-one-registry/src/lib.rs`
- `crates/the-one-mcp/src/api.rs`
- `crates/the-one-mcp/src/broker.rs`
- `crates/the-one-mcp/src/swagger.rs`
- `crates/the-one-ui/src/lib.rs`
- `crates/the-one-ui/src/bin/embedded-ui.rs`
- `schemas/mcp/v1beta/openapi.swagger.json`
