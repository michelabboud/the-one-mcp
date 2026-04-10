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

## 7. Recommended Production Checks

- keep `profile` explicit in CI/bootstrap scripts (`core` or `full`)
- run `setup` action `refresh` after major config/profile changes
- monitor memory surfaces with `observe` action `metrics`
- back up project state before large ingest batches
