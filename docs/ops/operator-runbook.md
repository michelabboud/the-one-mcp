# The-One MCP Operator Runbook

Last updated: 2026-04-11
Applies to: v0.16.0-phase3 (`v1beta` schema, multi-backend v0.16.0)

## 1. Service Health Checklist

```bash
# Verify workspace builds and tests pass (default: SQLite + Qdrant)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Expected: 466 passing, 0 failing, 1 ignored (v0.16.0-phase3 baseline)

# With Phase 2 + Phase 3 feature-gated backends (pgvector + Postgres state)
cargo clippy --workspace --all-targets --features pg-state,pg-vectors -- -D warnings
cargo test --workspace --features pg-state,pg-vectors
# Expected: 495 passing, 0 failing, 1 ignored

# Verify binary builds — default and feature-gated
cargo build --release -p the-one-mcp --bin the-one-mcp
cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-state,pg-vectors

# Run release gate (fmt + clippy + test + build, SQLite path)
bash scripts/release-gate.sh
```

### Verify Project State (SQLite default)

After `setup` (action: `init`), check these files exist:

- `<project>/.the-one/project.json`
- `<project>/.the-one/overrides.json`
- `<project>/.the-one/fingerprint.json`
- `<project>/.the-one/pointers.json`
- `<project>/.the-one/state.db` (SQLite — schema v7 with `outcome` + `error_kind` columns)
- `<project>/.the-one/docs/` (managed docs folder)

### Verify Project State (Postgres backend — v0.16.0 Phase 3)

When `THE_ONE_STATE_TYPE=postgres` is set, `state.db` does NOT
exist. Instead verify in Postgres:

```sql
\dn the_one                              -- schema exists
\dt the_one.*                            -- 8 tables + state_migrations
\di the_one.*                            -- includes diary_entries_content_tsv GIN
SELECT * FROM the_one.state_migrations ORDER BY version;
-- → 2 rows, versions 0..1, SHA-256 checksums
```

When `THE_ONE_VECTOR_TYPE=pgvector` is set, also check:

```sql
SELECT * FROM the_one.pgvector_migrations ORDER BY version;
-- → 5 rows, versions 0..4 (extension + chunks + entities + relations)
SELECT extname FROM pg_extension WHERE extname = 'vector';
-- → 'vector' row must exist
```

### Verify Global State

- `${THE_ONE_HOME:-$HOME/.the-one}/config.json` (global config)
- `${THE_ONE_HOME:-$HOME/.the-one}/registry/capabilities.json` (capability catalog)

### Verify Backend Selection

When any `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` env var is set, the
broker parses them once at startup via
`the_one_core::config::backend_selection::BackendSelection::from_env`
and fails loud on any inconsistency. Startup failure modes
covered by the parser (all produce
`CoreError::InvalidProjectConfig` at broker construction time):

- One TYPE set, the other unset
- TYPE set but matching URL unset
- Unknown TYPE value (returns the enum list)
- Combined TYPEs with mismatched values
- Combined TYPEs with mismatched URLs

See [docs/guides/configuration.md § Multi-Backend Selection](../guides/configuration.md#multi-backend-selection-v0160)
for the full matrix and error messages.

## 2. Running the Server

```bash
# Stdio (default — for Claude Code, Gemini CLI, OpenCode, Codex)
./target/release/the-one-mcp serve

# SSE transport
./target/release/the-one-mcp serve --transport sse --port 3000

# Streamable HTTP transport
./target/release/the-one-mcp serve --transport stream --port 3000

# With specific project
./target/release/the-one-mcp serve --project-root /path/to/project --project-id myproject
```

## 3. Incident Triage

If users report issues:

1. **Query audit events** — `observe` (action: `events`) with recent limit
2. **Check metrics** — `observe` (action: `metrics`) for:
   - `router_fallback_calls` (high = providers failing)
   - `router_provider_error_calls` (provider errors)
   - `router_decision_latency_ms_total` (routing performance)
3. **Inspect config** — `config` (action: `export`) to verify settings
4. **Check provider health** — `observe` (action: `metrics`) shows pool status
5. **Check nano providers** — verify URLs are reachable, API keys valid

### Common Issues

| Symptom | Diagnosis | Fix |
|---------|-----------|-----|
| All searches return empty | Docs not ingested | Run `maintain` (action: `reindex`) |
| Provider pool falling back constantly | All nano providers unreachable | Check URLs, increase `timeout_ms` |
| `remote qdrant requires api key` | Strict auth enabled, no key | Set `qdrant_api_key` or `qdrant_strict_auth: false` |
| High-risk tool denied | Headless mode, no prior approval | Approve interactively first with `session` or `forever` scope |
| Slow first request | fastembed model downloading | Wait for ~30MB download, cached after |
| `memory.search_images` returns empty | Image embedding not enabled or no images ingested | Set `image_embedding_enabled: true`, run `maintain` (action: `images.rescan`) |
| OCR text missing from image results | OCR not enabled | Set `image_ocr_enabled: true`, re-ingest images |
| Image index inconsistent after restore | Image Qdrant collection not restored | Ensure `qdrant-images.bak/` was included in backup; run `maintain` (action: `images.rescan`) |

## 4. Backup and Restore

### Backup

1. Trigger via admin UI or `trigger_manual_backup` API
2. Verify artifacts:
   - `<backup>/state.db.bak` (SQLite database)
   - `<backup>/qdrant.bak/` (text vector index — `the_one_*` collections)
   - `<backup>/qdrant-images.bak/` (image vector index — `the_one_images_*` collections, if image embedding is enabled)

### Restore

1. Trigger via admin UI or `trigger_manual_restore` API
2. Verify:
   - `<project>/.the-one/state.db` exists and is readable
   - `<project>/.the-one/qdrant/` content present
   - `<project>/.the-one/qdrant-images/` present (if image embedding was enabled)
3. Run `setup` (action: `refresh`) and a `memory.search` smoke test

### Recovery Drill (recommended weekly)

1. Create backup from seeded project
2. Mutate `.the-one/state.db` and `.the-one/qdrant/` (and `.the-one/qdrant-images/` if applicable)
3. Restore and validate content recovered
4. Document results

## 5. Managed Documents

### Folder Structure

```
<project>/.the-one/docs/           # managed (read-write)
+-- folder/subfolder/file.md       # auto-created directories
+-- .trash/                        # soft-deleted files
    +-- folder/subfolder/file.md   # preserves original path
```

### Operations

- Create or update: `docs.save` (upsert — validates .md extension, size, count limits)
- Delete: `docs.delete` (moves to `.trash/`)
- Restore: `maintain` (action: `trash.restore`)
- Empty trash: `maintain` (action: `trash.empty`)
- Reindex: `maintain` (action: `reindex`) — re-ingests all docs into RAG

### External Docs

Set `external_docs_root` in config to ingest an external directory read-only. Changes to external docs are detected on `project.refresh`.

## 6. Configuration Management

### Update via Admin UI

1. Open `http://127.0.0.1:8787/config`
2. Edit fields (including 12 configurable limits)
3. Submit — writes to `<project>/.the-one/config.json`

### Update via MCP Tool

Use `config` (action: `update`) with a JSON payload of fields to change.

### Update via File

Edit `<project>/.the-one/config.json` directly. Changes take effect on next broker operation that loads config.

### Precedence

Runtime overrides > env vars > project config > global config > defaults

### Multi-Backend Selection (v0.16.0+)

State store and vector backend are pluggable via four env vars
parsed at broker startup:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string>
```

Shipping today (v0.16.0-phase3):

| State | Vector | Features | Notes |
|---|---|---|---|
| `sqlite` (default) | `qdrant` (default) | (none) | v0.15.x baseline, no rebuild needed |
| `sqlite` | `pgvector` | `pg-vectors` | Phase 2 — single-axis Postgres |
| `postgres` | `qdrant` | `pg-state` | Phase 3 — single-axis Postgres |
| `postgres` | `pgvector` | `pg-state,pg-vectors` | Phase 3 — split-pool on both axes |

Pending: `postgres-combined` (Phase 4), `redis` + `redis-combined`
(Phases 5–6), `redis-vectors` at full parity (Phase 7).

**Tuning knobs** live in `config.json` under `vector_pgvector` (HNSW
parameters + pool sizing) and `state_postgres` (statement timeout +
pool sizing). Secrets stay in env vars. See
[docs/guides/configuration.md § Multi-Backend Selection](../guides/configuration.md#multi-backend-selection-v0160)
for the full field tables.

Per-backend operational playbooks:
- [pgvector-backend.md](../guides/pgvector-backend.md) — per-provider
  extension install, HNSW retune recipes, monitoring queries.
- [postgres-state-backend.md](../guides/postgres-state-backend.md) —
  sync-over-async bridge, FTS5 → tsvector translation, pool sizing.
- [multi-backend-operations.md](../guides/multi-backend-operations.md)
  — deployment matrix across state + vector axes.

## 7. Nano Provider Pool Management

### Add a Provider

Add to `nano_providers` array in config:
```json
{
  "name": "my-provider",
  "base_url": "http://localhost:11434/v1",
  "model": "qwen2:0.5b",
  "api_key": null,
  "timeout_ms": 500,
  "enabled": true
}
```

### Disable a Provider

Set `"enabled": false` in the provider entry.

### Monitor Health

Use `observe` (action: `metrics`) — includes per-provider call counts, errors, and latency.

### Routing Policies

- `"priority"` — try in order (safest default)
- `"round_robin"` — spread load
- `"latency"` — use fastest provider

## 8. Per-CLI Custom Tools

The tool catalog supports per-client customization:

```
~/.the-one/registry/
├── recommended.json         # Universal (auto-updated from GitHub)
├── custom.json              # Shared across all CLIs
├── custom-claude.json       # Claude Code only
├── custom-gemini.json       # Gemini CLI only
├── custom-opencode.json     # OpenCode only
└── custom-codex.json        # Codex only
```

Loading order: `recommended.json` + `custom.json` + `custom-<client>.json`

The server identifies the client via `clientInfo.name` in the MCP `initialize` handshake. If a per-CLI file doesn't exist, only universal tools are loaded.

To add a custom tool for Claude Code only:
```bash
$EDITOR ~/.the-one/registry/custom-claude.json
```

## 9. Safe Rollout Rules

1. Do not ship schema changes without contract test updates
2. Keep `v1beta` response fields additive only
3. Keep headless high-risk behavior deny-by-default
4. Keep project-scoped memory and approvals isolated by `project_id` and root
5. Test backup/restore in staging before production rollout
6. Monitor `router_fallback_calls` after provider config changes
