# The-One MCP Operator Runbook

Last updated: 2026-04-03
Applies to: v0.2.0 (`v1beta` schema)

## 1. Service Health Checklist

```bash
# Verify workspace builds and tests pass
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Verify binary builds
cargo build --release -p the-one-mcp --bin the-one-mcp

# Run release gate
bash scripts/release-gate.sh
```

### Verify Project State

After `project.init`, check these files exist:
- `<project>/.the-one/project.json`
- `<project>/.the-one/overrides.json`
- `<project>/.the-one/fingerprint.json`
- `<project>/.the-one/pointers.json`
- `<project>/.the-one/state.db`
- `<project>/.the-one/docs/` (managed docs folder)

### Verify Global State

- `${THE_ONE_HOME:-$HOME/.the-one}/config.json` (global config)
- `${THE_ONE_HOME:-$HOME/.the-one}/registry/capabilities.json` (capability catalog)

## 2. Running the Server

```bash
# Stdio (default, for Claude Code / Codex)
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

1. **Query audit events** — `audit.events` tool with recent limit
2. **Check metrics** — `metrics.snapshot` for:
   - `router_fallback_calls` (high = providers failing)
   - `router_provider_error_calls` (provider errors)
   - `router_decision_latency_ms_total` (routing performance)
3. **Inspect config** — `config.export` to verify settings
4. **Check provider health** — `metrics.snapshot` shows pool status
5. **Check nano providers** — verify URLs are reachable, API keys valid

### Common Issues

| Symptom | Diagnosis | Fix |
|---------|-----------|-----|
| All searches return empty | Docs not ingested | Run `docs.reindex` |
| Provider pool falling back constantly | All nano providers unreachable | Check URLs, increase `timeout_ms` |
| `remote qdrant requires api key` | Strict auth enabled, no key | Set `qdrant_api_key` or `qdrant_strict_auth: false` |
| High-risk tool denied | Headless mode, no prior approval | Approve interactively first with `session` or `forever` scope |
| Slow first request | fastembed model downloading | Wait for ~30MB download, cached after |

## 4. Backup and Restore

### Backup

1. Trigger via admin UI or `trigger_manual_backup` API
2. Verify artifacts:
   - `<backup>/state.db.bak` (SQLite database)
   - `<backup>/qdrant.bak/` (vector index)

### Restore

1. Trigger via admin UI or `trigger_manual_restore` API
2. Verify:
   - `<project>/.the-one/state.db` exists and is readable
   - `<project>/.the-one/qdrant/` content present
3. Run `project.refresh` and a `memory.search` smoke test

### Recovery Drill (recommended weekly)

1. Create backup from seeded project
2. Mutate `.the-one/state.db` and `.the-one/qdrant/`
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

- Create: `docs.create` (validates .md extension, size, count limits)
- Update: `docs.update` (must exist)
- Delete: `docs.delete` (moves to `.trash/`)
- Restore: `docs.trash.restore` (moves back from `.trash/`)
- Empty trash: `docs.trash.empty` (permanent delete)
- Reindex: `docs.reindex` (re-ingests all docs into RAG)

### External Docs

Set `external_docs_root` in config to ingest an external directory read-only. Changes to external docs are detected on `project.refresh`.

## 6. Configuration Management

### Update via Admin UI

1. Open `http://127.0.0.1:8787/config`
2. Edit fields (including 12 configurable limits)
3. Submit — writes to `<project>/.the-one/config.json`

### Update via MCP Tool

Use `config.update` with a JSON payload of fields to change.

### Update via File

Edit `<project>/.the-one/config.json` directly. Changes take effect on next broker operation that loads config.

### Precedence

Runtime overrides > env vars > project config > global config > defaults

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

Use `metrics.snapshot` — includes per-provider call counts, errors, and latency.

### Routing Policies

- `"priority"` — try in order (safest default)
- `"round_robin"` — spread load
- `"latency"` — use fastest provider

## 8. Safe Rollout Rules

1. Do not ship schema changes without contract test updates
2. Keep `v1beta` response fields additive only
3. Keep headless high-risk behavior deny-by-default
4. Keep project-scoped memory and approvals isolated by `project_id` and root
5. Test backup/restore in staging before production rollout
6. Monitor `router_fallback_calls` after provider config changes
