# Conversation Memory

Conversation memory lets you import transcript exports as verbatim searchable memory, tag them with palace metadata, and build a compact wake-up pack before resuming work.

## Supported tools

- `memory.ingest_conversation`
- `memory.search`
- `memory.fetch_chunk`
- `memory.wake_up`
- `maintain` action `memory.capture_hook`

## Feature toggles

MemPalace features are controlled by config/env flags:

- `memory_palace_enabled` (default `true`)
- `memory_palace_hooks_enabled` (default `false`)

Equivalent env vars:

- `THE_ONE_MEMORY_PALACE_ENABLED`
- `THE_ONE_MEMORY_PALACE_HOOKS_ENABLED`

Behavior:

- When `memory_palace_enabled=false`, `memory.ingest_conversation` and
  `memory.wake_up` return a `NotEnabled` error.
- `memory.search` still works for docs and ignores palace filters while disabled.
- `memory.capture_hook` additionally requires `memory_palace_hooks_enabled=true`.

## MemPalace profile control

Use `config` action `profile.set` to switch the full MemPalace preset in one
step. The accepted values are `off`, `core`, and `full` (aliases such as
`mempalace_full` are also accepted by the broker).

Exact example:

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

Expected result shape:

```json
{
  "path": "/home/user/myproject/.the-one/config.json"
}
```

Notes:

- `off` disables all MemPalace subfeatures.
- `core` keeps conversation memory enabled but leaves hooks, AAAK, diary, and
  navigation off.
- `full` enables conversation memory, hooks, AAAK, diary, and navigation.
- The admin UI config page shows the active preset plus the resolved flag
  matrix so you can confirm what is actually enabled on disk.

## Ingest a transcript

`memory.ingest_conversation` accepts absolute paths or project-relative paths and stores transcript metadata in the project database.

Exact example:

```json
{
  "name": "memory.ingest_conversation",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "path": "exports/auth-review.json",
    "format": "openai_messages",
    "wing": "ops",
    "hall": "incidents",
    "room": "auth"
  }
}
```

Expected result shape:

```json
{
  "ingested_chunks": 3,
  "source_path": "/home/user/myproject/exports/auth-review.json"
}
```

Notes:

- `format` must be one of `openai_messages`, `claude_transcript`, or `generic_jsonl`.
- If you provide `hall` or `room` without `wing`, the broker uses `project_id` as the stored wing.
- The indexed conversation chunks keep the transcript file path as `source_path`.

## Search with palace filters

Conversation metadata is encoded in chunk fields today, so `memory.search` applies palace filters after retrieval and before the final response is shaped.

Exact example:

```json
{
  "name": "memory.search",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "query": "refresh token staging incident",
    "top_k": 5,
    "wing": "ops",
    "room": "auth"
  }
}
```

Notes:

- Omitting `wing`, `hall`, and `room` preserves existing search behavior.
- Setting any of those fields limits results to conversation chunks whose palace metadata matches.

## Build a wake-up pack

`memory.wake_up` reads persisted conversation source metadata, reloads matching conversation documents, and extracts a compact list of facts.

Exact example:

```json
{
  "name": "memory.wake_up",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "wing": "ops",
    "hall": "incidents",
    "room": "auth",
    "max_items": 4
  }
}
```

Expected result shape:

```json
{
  "summary": "Wake-up pack with 2 fact(s) from 1 conversation source(s).",
  "facts": [
    "We switched auth vendors after refresh-token failures.",
    "The staging outage was fixed by rotating the issuer config."
  ]
}
```

Notes:

- Wake-up filtering supports `wing`, `hall`, and `room`.
- Wake-up filter matching is backed by persisted conversation-source metadata
  in `.the-one/state.db`, then matching transcripts are reloaded and distilled
  into facts.
- If no matching conversation memory exists, the result is:

```json
{
  "summary": "No conversation memory available.",
  "facts": []
}
```

## Capture `stop` / `precompact` hooks

Use `maintain` with action `memory.capture_hook` to ingest hook transcripts as
first-class conversation memory.

Exact example:

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

Expected result shape:

```json
{
  "event": "precompact",
  "ingested_chunks": 2,
  "source_path": "/home/user/myproject/exports/precompact.json",
  "wing": "myproject",
  "hall": "hook:precompact",
  "room": "event:precompact"
}
```

Notes:

- `event` must be `stop` or `precompact`.
- You can override `wing`, `hall`, and `room` explicitly.
- If omitted, defaults are deterministic:
  - `wing = project_id`
  - `hall = hook:<event>`
  - `room = event:<event>`

## Persistence model

- Transcript metadata is persisted in `.the-one/state.db`.
- Indexed chunks are reloaded from persisted transcript records after broker restart.
- Palace metadata currently lives in chunk fields and the conversation source table; there is no separate palace storage layer.
