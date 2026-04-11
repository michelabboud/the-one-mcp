# MemPalace Operations Guide

Production runbook for the MemPalace memory layer: profile presets, transcript ingestion, AAAK lessoning, diary entries, navigation primitives, and hook capture.

## Scope

This guide covers:

- one-switch MemPalace enable/disable control
- AAAK compression and teach/list lesson workflows
- diary add/list/search/summarize workflows
- explicit drawers / closets / tunnels navigation primitives
- stop/precompact hook capture flow

For full request/response schemas, see [API Reference](api-reference.md). For broader transcript usage, see [Conversation Memory Guide](conversation-memory.md).

## 1. Preset Profiles (`off`, `core`, `full`)

Use `config` with action `profile.set`:

```json
{
  "name": "config",
  "arguments": {
    "action": "profile.set",
    "params": {
      "project_root": "/home/user/myproject",
      "profile": "full"
    }
  }
}
```

Preset behavior:

- `off`: disables all MemPalace subfeatures
- `core`: enables core transcript memory only
- `full`: enables transcript memory, hooks, AAAK, diary, navigation

Operational recommendation:

- Use `off` for regulated or short-lived tasks.
- Use `core` as the default team baseline.
- Use `full` for long-running products that benefit from reusable motifs and diary/history continuity.

## 2. Ingest Transcripts (Verbatim Source of Truth)

```json
{
  "name": "memory.ingest_conversation",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "path": "exports/incident-042.json",
    "format": "openai_messages",
    "wing": "ops",
    "hall": "incidents",
    "room": "auth"
  }
}
```

Notes:

- transcript text is kept verbatim for lossless retrieval
- palace metadata is persisted and reloaded after broker restart
- with AAAK enabled in `full`, ingest can auto-teach reusable patterns

## 3. AAAK Compression and Lessoning

Compress:

```json
{
  "name": "memory.aaak.compress",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "path": "exports/incident-042.json",
    "format": "openai_messages"
  }
}
```

Teach:

```json
{
  "name": "memory.aaak.teach",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "path": "exports/incident-042.json",
    "format": "openai_messages"
  }
}
```

List lessons:

```json
{
  "name": "memory.aaak.list_lessons",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "limit": 50
  }
}
```

Operational recommendation:

- teach from high-signal transcripts (postmortems, architecture reviews)
- keep lesson limits bounded in automation to avoid noisy motifs

## 4. Diary Memory Flows

Add or refresh daily entry:

```json
{
  "name": "memory.diary.add",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "entry_date": "2026-04-10",
    "mood": "focused",
    "tags": ["release", "stability"],
    "content": "Validated MemPalace phase 2 and completed release docs."
  }
}
```

List/search/summarize:

- `memory.diary.list`
- `memory.diary.search`
- `memory.diary.summarize`

Behavior guarantee:

- diary identity is stable by `project_id + entry_date`
- refresh updates content without rewriting original `created_at`

## 5. Navigation Primitives (Drawers / Closets / Tunnels)

Upsert a drawer:

```json
{
  "name": "memory.navigation.upsert_node",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "node_id": "ops-drawer",
    "kind": "drawer",
    "label": "Operations"
  }
}
```

Upsert a closet under that drawer:

```json
{
  "name": "memory.navigation.upsert_node",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "node_id": "auth-closet",
    "kind": "closet",
    "label": "Auth",
    "parent_node_id": "ops-drawer"
  }
}
```

Link tunnel:

```json
{
  "name": "memory.navigation.link_tunnel",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "from_node_id": "auth-closet",
    "to_node_id": "incident-room"
  }
}
```

Notes:

- nodes/tunnels are project-scoped
- same `node_id` in different projects remains isolated

## 6. Hook Capture (`stop` / `precompact`)

```json
{
  "name": "maintain",
  "arguments": {
    "action": "memory.capture_hook",
    "params": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "exports/precompact.json",
      "format": "openai_messages",
      "event": "precompact"
    }
  }
}
```

Default metadata when omitted:

- `wing = project_id`
- `hall = hook:<event>`
- `room = event:<event>`

## 7. Pagination and over-limit behavior (v0.15.0+)

As of v0.15.0, every list/search endpoint uses **cursor-based pagination**
and **rejects over-limit requests instead of silently truncating**. See
`docs/guides/production-hardening-v0.15.md` for the full per-endpoint
caps.

### Paginating diary entries

```json
{
  "name": "memory.diary.list",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "max_results": 20
  }
}
```

Response carries `next_cursor` iff there is more data:

```json
{
  "entries": [...],
  "next_cursor": "eyJvIjoyMH0"
}
```

Pass `next_cursor` verbatim in the next request to get the subsequent
page. **Do not parse the cursor bytes** — format is reserved.

### Over-limit behavior

Requesting `max_results: 5000` against `memory.diary.list` returns a
400-equivalent error:

```json
{
  "error": {
    "code": -32603,
    "message": "limit 5000 exceeds maximum of 500 for this endpoint (request fewer items or page with a cursor) (kind=invalid_request, corr=corr-00000042)"
  }
}
```

v0.14.x silently clamped the same request to 200 items and lost the
rest. Always check the response for `next_cursor` to detect truncation.

## 8. Structured audit log (v0.15.0+)

Every state-changing memory palace call now writes a structured audit
row to `audit_events` with fields `event_type`, `payload_json`, `outcome`
(`ok`/`error`/`unknown`), and `error_kind` (populated when
`outcome='error'`).

Inspect via `observe.audit_events` or SQL directly:

```sql
SELECT event_type, outcome, error_kind, COUNT(*) 
FROM audit_events 
WHERE project_id = 'myproject' 
GROUP BY event_type, outcome, error_kind;
```

Operationally useful queries:

- **Error rate by operation**: `SELECT event_type, SUM(CASE WHEN outcome='error' THEN 1 ELSE 0 END) * 100.0 / COUNT(*) AS err_pct FROM audit_events GROUP BY event_type;`
- **Recent failures**: `SELECT * FROM audit_events WHERE outcome='error' ORDER BY id DESC LIMIT 20;`
- **Kind-based alerting**: `SELECT error_kind, COUNT(*) FROM audit_events WHERE outcome='error' AND created_at_epoch_ms > <cutoff> GROUP BY error_kind;`

## 9. Recommended Production Checks

- keep `profile` explicit in CI/bootstrap scripts (`core` or `full`)
- run `setup` action `refresh` after major config/profile changes
- monitor memory surfaces with `observe` action `metrics`
- back up project state before large ingest batches
- sanitize wing/hall/room names at your ingest boundary — the broker
  rejects names containing `/`, `\`, `..`, or punctuation outside
  `[A-Za-z0-9 ._\-:]` as of v0.15.0
- alert on `outcome='error'` rate per operation in the audit table
- when paging through large lists, **always** pass back the
  `next_cursor` instead of bumping `max_results` — the server will
  reject over-limit requests
