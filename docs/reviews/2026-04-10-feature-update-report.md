# Feature Update Report — Redis + Memory Palace Production Hardening

Date: 2026-04-10  
Scope: Merge and harden the Redis vector backend path, wake-up palace filtering,
catalog resource integrity, MemPalace feature controls + hook capture flow, and production-readiness cleanup (no stubs/placeholders in runtime behavior).

## Delivered Changes

### 1) Redis vector backend: runtime-complete path

- Broker now constructs a real Redis-backed memory engine when
  `vector_backend = "redis"` with local embeddings.
- Redis-backed ingest/search/delete flows are wired into memory engine methods.
- Redis persistence checks are enforced at runtime when
  `redis_persistence_required = true`.
- Silent local-only fallback behavior for Redis selection was removed.
- API embeddings + Redis remains fail-fast by design (unsupported pairing).

Primary files:
- `/home/michel/projects/the-one-mcp/crates/the-one-memory/src/lib.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-memory/src/redis_vectors.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/Cargo.toml`

### 2) `models.check` moved from placeholder behavior to real check flow

- Replaced static/stub response with script-backed checks:
  - `scripts/update-local-models.sh`
  - `scripts/update-api-models.sh`
- Added structured status and check payloads:
  - `up_to_date`
  - `updates_available`
  - `degraded`
- Includes bounded output excerpts and next-action hints.

Primary file:
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs`

### 3) MCP resources: `the-one://catalog/enabled` now returns real data

- Resource now reads enabled tools from catalog DB for the active project.
- Handles project-root path normalization and dedup behavior.
- Replaced placeholder empty-array semantics with production lookup.

Primary files:
- `/home/michel/projects/the-one-mcp/crates/the-one-core/src/tool_catalog.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/resources.rs`

### 4) Conversation wake-up: full palace filter parity

- `memory.wake_up` request now supports:
  - `wing`
  - `hall`
  - `room`
- SQLite conversation-source query now filters by all palace dimensions.
- JSON-RPC parsing and tool schema updated accordingly.

Primary files:
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/api.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-core/src/storage/sqlite.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/transport/jsonrpc.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/transport/tools.rs`

### 5) Final production-hardening cleanup

- Fixed graph extractor env-race test instability via env lock + cleanup.
- Removed non-functional embedded UI project-switch interaction from runtime UI.
- Removed “stub” wording from OCR feature-disabled implementation/tests.

Primary files:
- `/home/michel/projects/the-one-mcp/crates/the-one-memory/src/graph_extractor.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-ui/src/lib.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-memory/src/ocr.rs`

### 6) MemPalace runtime controls + hook capture (new)

- Added explicit feature toggles:
  - `memory_palace_enabled` (default `true`)
  - `memory_palace_hooks_enabled` (default `false`)
  - env: `THE_ONE_MEMORY_PALACE_ENABLED`, `THE_ONE_MEMORY_PALACE_HOOKS_ENABLED`
- Enforced production gating:
  - `memory.ingest_conversation` and `memory.wake_up` return `NotEnabled`
    when palace is disabled.
  - `memory.search` remains available and ignores palace filters while palace is disabled.
- Added first-class hook ingest flow:
  - `maintain` action: `memory.capture_hook`
  - events: `stop`, `precompact`
  - deterministic defaults:
    - `wing = project_id`
    - `hall = hook:<event>`
    - `room = event:<event>`
- Added config update support:
  - `config.update` now accepts `memory_palace_enabled` and
    `memory_palace_hooks_enabled`.

Primary files:
- `/home/michel/projects/the-one-mcp/crates/the-one-core/src/config.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/api.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/transport/jsonrpc.rs`
- `/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/transport/tools.rs`

## Documentation Updated

- `/home/michel/projects/the-one-mcp/README.md`
- `/home/michel/projects/the-one-mcp/CHANGELOG.md`
- `/home/michel/projects/the-one-mcp/PROGRESS.md`
- `/home/michel/projects/the-one-mcp/docs/guides/redis-vector-backend.md`
- `/home/michel/projects/the-one-mcp/docs/guides/mcp-resources.md`
- `/home/michel/projects/the-one-mcp/docs/guides/conversation-memory.md`
- `/home/michel/projects/the-one-mcp/docs/reviews/2026-04-09-production-hardening-findings.md`
- `/home/michel/projects/the-one-mcp/docs/reviews/2026-04-10-production-hardening-verification.md`

## Verification Summary

- `cargo fmt --check` passed
- `cargo test --workspace` passed
  - 340 tests passed
  - 1 test ignored
- Redis-focused and wake-up/resource targeted tests passed in prior hardening runs.

## Production-Readiness Statement

For this feature scope, runtime placeholder behavior was removed from the
critical paths that were previously flagged:

- Redis backend selection now executes Redis logic at runtime.
- `models.check` performs real checks.
- `catalog/enabled` returns real enabled-tool data.
- Wake-up filtering now matches full palace metadata dimensions.
