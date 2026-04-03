# The-One MCP Operator Runbook

Last updated: 2026-04-03
Applies to: `v1beta`

## 1. Service Health Checklist

1. Verify workspace health:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

2. Verify per-project state exists after init:
   - `<project>/.the-one/project.json`
   - `<project>/.the-one/overrides.json`
   - `<project>/.the-one/fingerprint.json`
   - `<project>/.the-one/pointers.json`
   - `<project>/.the-one/state.db`

3. Verify global catalog path:
   - `${THE_ONE_HOME:-$HOME/.the-one}/registry/capabilities.json`

## 2. Incident Triage

If users report policy/routing issues:

1. Query recent audit events (`audit_events` endpoint).
2. Capture metrics snapshot (`metrics_snapshot` endpoint).
3. Inspect project config (`config_export` endpoint).
4. Check whether router fallback was triggered:
   - `router_fallback_calls`
   - `router_decision_latency_ms_total`

## 3. Backup and Restore Procedure

### Backup

1. Trigger backup via API/UI (`trigger_manual_backup`).
2. Confirm backup artifacts:
   - `<backup>/state.db.bak`
   - `<backup>/qdrant.bak/`

### Restore

1. Trigger restore via API/UI (`trigger_manual_restore`).
2. Validate:
   - `<project>/.the-one/state.db` exists and is readable.
   - `<project>/.the-one/qdrant/` content is present.
3. Re-run `project.refresh` and a small `memory.search` smoke test.

## 4. Recovery Drills (Recommended)

Run weekly:

1. Create backup from a seeded project.
2. Mutate `.the-one/state.db` and `.the-one/qdrant/`.
3. Restore and validate expected content is recovered.
4. Record drill status in release notes.

## 5. Safe Rollout Rules

1. Do not ship schema changes without contract test updates.
2. Keep `v1beta` response fields additive only.
3. Keep headless high-risk behavior deny-by-default.
4. Keep project-scoped memory and approvals isolated by `project_id` and root.
