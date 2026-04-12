# The-One MCP API Reference

> Complete reference for all 30 MCP tools + 3 MCP resource types exposed by
> the-one-mcp. Current version: v0.16.0-phase4. Tool and resource shapes are
> unchanged since v0.12.0 — v0.15/v0.16 work is additive and backend-internal.
>
> Tools are invoked via JSON-RPC 2.0 over stdio/SSE/stream. Every tool call uses
> `method: "tools/call"` with `params.name` and `params.arguments`. Results are
> returned as MCP content blocks with `type: "text"` containing pretty-printed JSON.
>
> **Resources** (new in v0.10.0) are invoked via `resources/list` and
> `resources/read` methods directly — not wrapped in `tools/call`. See the
> [Resources section](#mcp-resources) below.
>
> **Backend-agnostic:** every tool below works identically against the
> default SQLite + Qdrant backend and against the v0.16.0 alternatives
> (pgvector, PostgresStateStore). The broker dispatches through the
> `VectorBackend` and `StateStore` traits, so tool behaviour is
> invariant across backend choice. See the
> [multi-backend operations guide](multi-backend-operations.md) for
> operator-level deployment details.

---

## Quick Reference

| Tool | Category | Purpose |
|------|----------|---------|
| `memory.search` | Memory | Semantic search over indexed doc chunks |
| `memory.fetch_chunk` | Memory | Retrieve a specific chunk by ID |
| `memory.search_images` | Memory | Semantic image search |
| `memory.ingest_image` | Memory | Manually index an image file |
| `memory.ingest_conversation` | Memory | Import a transcript export as conversation memory |
| `memory.wake_up` | Memory | Build a compact context pack from conversation memory |
| `docs.list` | Docs | List all indexed documentation paths |
| `docs.get` | Docs | Retrieve a document or a named section |
| `docs.save` | Docs | Create or update a document (upsert) |
| `docs.delete` | Docs | Soft-delete a document to trash |
| `docs.move` | Docs | Rename or move a document |
| `tool.find` | Tools | Unified tool discovery (list/suggest/search) |
| `tool.info` | Tools | Full metadata for a specific tool |
| `tool.install` | Tools | Install a tool and auto-enable it |
| `tool.run` | Tools | Execute a tool action with policy gate |
| `setup` | Admin | Project init, refresh, profile |
| `config` | Admin | Config export/update, `profile.set`, custom tools, models |
| `maintain` | Admin | Reindex, `memory.capture_hook`, tool enable/disable, trash, images |
| `observe` | Admin | Broker metrics and audit events |

### MemPalace Extensions

| Tool | Category | Purpose |
|------|----------|---------|
| `memory.aaak.compress` | Memory | Compress a transcript into the AAAK dialect |
| `memory.aaak.teach` | Memory | Persist reusable AAAK lessons |
| `memory.aaak.list_lessons` | Memory | List stored AAAK lessons for the project |
| `memory.diary.add` | Memory | Create or refresh a diary entry |
| `memory.diary.list` | Memory | List diary entries by date range |
| `memory.diary.search` | Memory | Search diary entries by content / mood / tags |
| `memory.diary.summarize` | Memory | Summarize recent diary entries |
| `memory.navigation.upsert_node` | Memory | Create or update a drawer / closet / room |
| `memory.navigation.link_tunnel` | Memory | Link two navigation nodes with a tunnel |
| `memory.navigation.list` | Memory | List navigation nodes in the current project |
| `memory.navigation.traverse` | Memory | Traverse the navigation graph deterministically |

---

## Transport Notes

All tools are called via the JSON-RPC 2.0 `tools/call` method:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "<tool-name>",
    "arguments": { ... }
  }
}
```

A successful response wraps the result in an MCP content block:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{ ... pretty-printed JSON ... }"
      }
    ]
  }
}
```

Errors return a JSON-RPC error object with one of these codes:

| Code | Meaning |
|------|---------|
| `-32601` | Method not found |
| `-32602` | Invalid params (missing required field) |
| `-32603` | Internal error (broker-level failure) |

---

## Work Tools

### memory.search

Semantic search over indexed project documentation chunks. The broker embeds the
query and performs a vector similarity search over previously indexed content. The
response includes routing metadata from the intelligent router.

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `query` | string | yes | — | Natural-language search query |
| `top_k` | integer | no | `5` | Maximum number of results to return |
| `wing` | string | no | — | Optional palace wing filter for conversation chunks |
| `hall` | string | no | — | Optional palace hall filter for conversation chunks |
| `room` | string | no | — | Optional palace room filter for conversation chunks |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "memory.search",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "query": "refresh token staging incident",
      "top_k": 3,
      "wing": "ops",
      "room": "auth"
    }
  }
}
```

**Example response**

```json
{
  "hits": [
    {
      "id": "chunk-a1b2c3",
      "source_path": "docs/auth.md",
      "score": 0.92
    },
    {
      "id": "chunk-d4e5f6",
      "source_path": "docs/middleware.md",
      "score": 0.87
    }
  ],
  "route": "semantic",
  "rationale": "query matched auth-related chunks",
  "provider_path": "local/bge-large-en-v1.5",
  "confidence_percent": 88,
  "fallback_used": false,
  "timeout_ms_bound": 500,
  "retries_bound": 0,
  "last_error": null
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `hits[].id` | string | Chunk ID — pass to `memory.fetch_chunk` to retrieve full text |
| `hits[].source_path` | string | Relative path of the source document |
| `hits[].score` | float | Cosine similarity score (0–1) |
| `route` | string | Router decision (`semantic`, `rules`, `fallback`) |
| `rationale` | string | Human-readable routing explanation |
| `provider_path` | string | Embedding model used |
| `confidence_percent` | integer | Router confidence (0–100) |
| `fallback_used` | boolean | Whether rules-only fallback was active |
| `last_error` | string or null | Last provider error if any |

**Notes**

- The project must be initialized with `setup` (action: `project`) before searching.
- Scores above ~0.85 are typically strong matches; below 0.70 may be tangential.
- `top_k` is capped by the number of indexed chunks; requesting more than exist is safe.
- `wing`, `hall`, and `room` are applied after retrieval using the existing conversation chunk metadata. Non-conversation chunks are filtered out only when one of those fields is set.
- **v0.8.0:** Each chunk now carries extended metadata internally — `language`, `symbol`, `signature`, `line_range` — populated for Rust, Python, TypeScript, JavaScript, and Go source files. Use `memory.fetch_chunk` to retrieve the full chunk text; the code-chunking metadata is used to improve chunk boundaries and will be surfaced in API responses in a future release. See the [Code Chunking Guide](code-chunking.md) for details.

---

### memory.ingest_conversation

Import a conversation export and index it as verbatim memory. Palace metadata is optional, but when present it is encoded into the indexed conversation chunks and persisted in the project database for wake-up packs.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Absolute or project-relative path to the transcript export |
| `format` | string | yes | Transcript format: `openai_messages`, `claude_transcript`, or `generic_jsonl` |
| `wing` | string | no | Optional palace wing |
| `hall` | string | no | Optional palace hall |
| `room` | string | no | Optional palace room |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
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
}
```

**Example response**

```json
{
  "ingested_chunks": 3,
  "source_path": "/home/user/myproject/exports/auth-review.json"
}
```

**Notes**

- Conversation chunks keep the transcript file path as `source_path`.
- `wing` defaults to `project_id` only when at least one palace field is provided and `wing` itself is omitted.
- Re-ingesting the same transcript updates the persisted metadata row for that transcript path.

---

### memory.wake_up

Build a compact context pack from recently updated conversation sources. Today the wake-up query filters by `wing` only; it then extracts deduplicated fact lines from the matching conversation documents.

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `wing` | string | no | — | Optional palace wing filter |
| `max_items` | integer | no | `12` | Maximum facts to include |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "memory.wake_up",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "wing": "ops",
      "max_items": 4
    }
  }
}
```

**Example response**

```json
{
  "summary": "Wake-up pack with 2 fact(s) from 1 conversation source(s).",
  "facts": [
    "We switched auth vendors after refresh-token failures.",
    "The staging outage was fixed by rotating the issuer config."
  ]
}
```

**Notes**

- If there are no matching conversation sources, the broker returns `"No conversation memory available."` with an empty `facts` array.
- Wake-up packs reload persisted conversation source metadata after broker restart.

---

### memory.fetch_chunk

Retrieve the full text content of a specific memory chunk by its ID. Use this
after `memory.search` to load the complete text of a matching chunk.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `id` | string | yes | Chunk ID returned from `memory.search` |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "memory.fetch_chunk",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "id": "chunk-a1b2c3"
    }
  }
}
```

**Example response**

```json
{
  "id": "chunk-a1b2c3",
  "source_path": "docs/auth.md",
  "content": "## Authentication Middleware\n\nThe auth middleware validates JWT tokens on every protected route..."
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | The chunk ID echoed back |
| `source_path` | string | Relative path of the originating document |
| `content` | string | Full text content of the chunk |

**Notes**

- Returns an error if the chunk ID does not exist in the project's memory store.
- Chunk boundaries are determined by the indexing pipeline's splitter settings.

---

### memory.search_images

Semantic search over indexed project images. Finds screenshots, diagrams, photos,
and mockups matching a natural-language query or a reference image. Requires
`image_embedding_enabled` to be active in the project configuration.

Exactly one of `query` or `image_base64` must be provided:

- **Text query** — natural language description ("database schema diagram")
- **Image query** — base64-encoded image for image→image similarity (screenshot search)

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `query` | string | no* | — | Natural-language search query |
| `image_base64` | string | no* | — | Base64-encoded image bytes for image→image similarity search |
| `top_k` | integer | no | `5` | Maximum number of results |

*Exactly one of `query` or `image_base64` must be provided. Providing both or neither returns an error.

**Example call (text query)**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "memory.search_images",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "query": "login screen wireframe",
      "top_k": 5
    }
  }
}
```

**Example call (screenshot/image query)**

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "memory.search_images",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "image_base64": "<base64-encoded PNG bytes>",
      "top_k": 3
    }
  }
}
```

**Example response**

```json
{
  "hits": [
    {
      "id": "img-001",
      "source_path": "designs/login-wireframe.png",
      "thumbnail_path": ".the-one/thumbs/img-001.jpg",
      "caption": "Login screen wireframe v2",
      "ocr_text": "Username  Password  Sign In",
      "score": 0.91
    }
  ]
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `hits[].id` | string | Image record ID |
| `hits[].source_path` | string | Path to the original image |
| `hits[].thumbnail_path` | string or null | Path to the generated thumbnail |
| `hits[].caption` | string or null | User-provided or auto-generated caption |
| `hits[].ocr_text` | string or null | Extracted OCR text (if OCR is enabled) |
| `hits[].score` | float | Similarity score (0–1) |

**Notes**

- Requires image indexing to be enabled in config (`image_embedding_enabled: true`).
- Use `maintain` (action: `images.rescan`) to rebuild the image index.
- OCR text extraction is optional and requires Tesseract or compatible backend.
- Screenshot search (`image_base64`) uses the same image embedding model as indexed images — Nomic Vision by default. The base64 bytes are decoded to a temp file, embedded, and used as the query vector.

---

### memory.ingest_image

Manually index a single image file into the project's image memory. Extracts OCR
text (if enabled) and generates a thumbnail. Use this for images not automatically
discovered during a rescan.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Absolute or project-relative path to the image |
| `caption` | string | no | Optional user-provided caption |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "memory.ingest_image",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "/home/user/myproject/screenshots/dashboard.png",
      "caption": "Dashboard overview — Q2 redesign"
    }
  }
}
```

**Example response**

```json
{
  "path": "screenshots/dashboard.png",
  "dims": 768,
  "ocr_extracted": true,
  "thumbnail_generated": true
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Relative path of the indexed image |
| `dims` | integer | Embedding dimensions used |
| `ocr_extracted` | boolean | Whether OCR text was successfully extracted |
| `thumbnail_generated` | boolean | Whether a thumbnail was created |

**Notes**

- Supported formats depend on the image backend (PNG, JPEG, WebP, GIF typically).
- If the image is already indexed, it will be re-indexed with updated metadata.
- Requires `image_embedding_enabled: true` in project config.

---

### MemPalace tools

#### memory.aaak.compress

Compress a transcript into the AAAK dialect.

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "tools/call",
  "params": {
    "name": "memory.aaak.compress",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "exports/review.json",
      "format": "openai_messages"
    }
  }
}
```

#### memory.aaak.teach

Extract reusable AAAK patterns and persist them as lessons.

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "tools/call",
  "params": {
    "name": "memory.aaak.teach",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "exports/review.json",
      "format": "openai_messages"
    }
  }
}
```

#### memory.aaak.list_lessons

List persisted AAAK lessons for the project.

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "tools/call",
  "params": {
    "name": "memory.aaak.list_lessons",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "limit": 10
    }
  }
}
```

#### memory.diary.add

Create or refresh a diary entry with date, mood, tags, and content.

```json
{
  "jsonrpc": "2.0",
  "id": 8,
  "method": "tools/call",
  "params": {
    "name": "memory.diary.add",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "entry_date": "2026-04-10",
      "mood": "focused",
      "tags": ["planning", "release"],
      "content": "Prepared the MemPalace phase 2 rollout."
    }
  }
}
```

#### memory.diary.list

List diary entries for a date range.

```json
{
  "jsonrpc": "2.0",
  "id": 9,
  "method": "tools/call",
  "params": {
    "name": "memory.diary.list",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "start_date": "2026-04-01",
      "end_date": "2026-04-10",
      "max_results": 20
    }
  }
}
```

#### memory.diary.search

Search diary entries by content, mood, or tags.

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": {
    "name": "memory.diary.search",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "query": "planning release",
      "max_results": 10
    }
  }
}
```

#### memory.diary.summarize

Summarize recent diary entries into a compact memory pack.

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "tools/call",
  "params": {
    "name": "memory.diary.summarize",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "start_date": "2026-04-01",
      "end_date": "2026-04-10",
      "max_summary_items": 8
    }
  }
}
```

#### memory.navigation.upsert_node

Create or update a drawer, closet, or room node.

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "tools/call",
  "params": {
    "name": "memory.navigation.upsert_node",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "node_id": "drawer:release",
      "kind": "drawer",
      "label": "Release notes",
      "hall": "notes"
    }
  }
}
```

#### memory.navigation.link_tunnel

Create or refresh a tunnel between two navigation nodes.

```json
{
  "jsonrpc": "2.0",
  "id": 13,
  "method": "tools/call",
  "params": {
    "name": "memory.navigation.link_tunnel",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "from_node_id": "drawer:release",
      "to_node_id": "room:planning"
    }
  }
}
```

#### memory.navigation.list

List navigation nodes for the current project.

```json
{
  "jsonrpc": "2.0",
  "id": 14,
  "method": "tools/call",
  "params": {
    "name": "memory.navigation.list",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "kind": "drawer",
      "limit": 25
    }
  }
}
```

#### memory.navigation.traverse

Traverse the navigation graph from a starting node.

```json
{
  "jsonrpc": "2.0",
  "id": 15,
  "method": "tools/call",
  "params": {
    "name": "memory.navigation.traverse",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "start_node_id": "drawer:release",
      "max_depth": 8
    }
  }
}
```

**Notes**

- `AAAK` is the compression / lesson dialect used to preserve reusable transcript motifs.
- Diary tools require MemPalace to be enabled and preserve `entry_date` as the stable logical identity.
- Navigation tools use `drawer`, `closet`, and `room` node kinds plus explicit tunnel edges.
- The admin UI exposes the same feature family as a preset-controlled profile so you can turn the whole stack on or off consistently.

### docs.list

List all indexed documentation paths for a project. Returns relative paths only;
use `docs.get` to retrieve content.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "tools/call",
  "params": {
    "name": "docs.list",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject"
    }
  }
}
```

**Example response**

```json
{
  "docs": [
    "docs/architecture.md",
    "docs/auth.md",
    "docs/deployment.md",
    "CLAUDE.md",
    "README.md"
  ]
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `docs` | string[] | List of relative document paths managed by this project |

**Notes**

- Only returns documents managed by the-one-mcp docs manager, not arbitrary project files.
- Soft-deleted (trashed) documents do not appear in this list.
- Run `maintain` (action: `reindex`) to synchronize after external file changes.

---

### docs.get

Retrieve a document's full content, or extract a specific named section. When
`section` is omitted the entire document is returned. When provided, only the
content under that heading (up to `max_bytes`) is returned.

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `path` | string | yes | — | Relative path to the document |
| `section` | string | no | — | Heading text to extract (omit for full document) |
| `max_bytes` | integer | no | `24576` | Maximum bytes for section extraction (24 KB) |

**Example call — full document**

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "tools/call",
  "params": {
    "name": "docs.get",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "docs/auth.md"
    }
  }
}
```

**Example response — full document**

```json
{
  "path": "docs/auth.md",
  "content": "# Authentication\n\n## Overview\n\nJWT-based auth is used..."
}
```

**Example call — section extraction**

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "tools/call",
  "params": {
    "name": "docs.get",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "docs/auth.md",
      "section": "Overview",
      "max_bytes": 8192
    }
  }
}
```

**Example response — section extraction**

```json
{
  "path": "docs/auth.md",
  "heading": "Overview",
  "content": "JWT-based auth is used for all API routes..."
}
```

**Notes**

- Section matching is case-insensitive and matches the first heading with that text.
- `max_bytes` applies only to section extraction — full document is returned whole.
- Returns an error if the document path is not managed by this project.

---

### docs.save

Create or update a managed document (upsert semantics). If the path does not
exist it is created; if it already exists the content is updated. The response
indicates which operation occurred.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Relative path for the document |
| `content` | string | yes | Markdown content to write |
| `tags` | string[] | no | Tags for the document (replaces existing on update) |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 8,
  "method": "tools/call",
  "params": {
    "name": "docs.save",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "docs/decisions/adr-001.md",
      "content": "# ADR-001: Use PostgreSQL\n\n## Status\n\nAccepted\n\n## Context\n\n...",
      "tags": ["adr", "database"]
    }
  }
}
```

**Example response**

```json
{
  "path": "docs/decisions/adr-001.md",
  "created": true
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Relative path of the saved document |
| `created` | boolean | `true` if created new, `false` if updated existing |

**Notes**

- `tags` is optional. On create, omitting it stores no tags. On update, providing it
  replaces all existing tags; omitting it leaves them unchanged.
- Parent directories are created automatically if they do not exist.
- The document is re-indexed in the memory engine after saving.

---

### docs.delete

Soft-delete a managed document by moving it to the project trash. The document
is not permanently removed and can be restored with `maintain` (action: `trash.restore`).

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Relative path to the document to delete |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 9,
  "method": "tools/call",
  "params": {
    "name": "docs.delete",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "path": "docs/old-approach.md"
    }
  }
}
```

**Example response**

```json
{
  "deleted": true
}
```

**Notes**

- The document is removed from the memory index immediately.
- Use `maintain` (action: `trash.list`) to view trashed documents.
- Use `maintain` (action: `trash.empty`) to permanently purge all trashed documents.
- Returns an error if the path does not exist or is not managed by this project.

---

### docs.move

Rename or move a managed document to a new relative path. Updates the memory
index automatically.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `from` | string | yes | Current relative path |
| `to` | string | yes | New relative path |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": {
    "name": "docs.move",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "from": "docs/draft-auth.md",
      "to": "docs/auth.md"
    }
  }
}
```

**Example response**

```json
{
  "from": "docs/draft-auth.md",
  "to": "docs/auth.md"
}
```

**Notes**

- Returns an error if `from` does not exist or `to` already exists.
- Parent directories for `to` are created automatically if needed.
- All memory index references are updated to the new path.

---

### tool.find

Unified tool discovery with three modes:

- **`list`** — enumerate tools filtered by state (`enabled`, `available`, `recommended`, `all`)
- **`suggest`** — AI-powered recommendations based on a natural-language task description
- **`search`** — keyword/semantic search against the tool catalog

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `mode` | string | yes | — | `list`, `suggest`, or `search` |
| `filter` | string | no | — | For `list` mode: `enabled`, `available`, `recommended`, or `all` |
| `query` | string | no* | — | For `suggest`/`search`: natural-language query (required in those modes) |
| `cli` | string | no | — | CLI client name for per-CLI filtering |
| `max` | integer | no | `5` | Maximum results returned |

**Example — list enabled tools**

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "tools/call",
  "params": {
    "name": "tool.find",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "mode": "list",
      "filter": "enabled"
    }
  }
}
```

**Example response — list**

```json
{
  "tools": [
    {
      "id": "clippy",
      "name": "Clippy",
      "category": ["linting"],
      "installed": true,
      "enabled": true
    },
    {
      "id": "cargo-audit",
      "name": "cargo-audit",
      "category": ["security"],
      "installed": true,
      "enabled": true
    }
  ]
}
```

**Example — suggest tools for a task**

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "tools/call",
  "params": {
    "name": "tool.find",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "mode": "suggest",
      "query": "security scanning for my Rust project",
      "max": 3
    }
  }
}
```

**Example response — suggest**

```json
{
  "suggestions": [
    {
      "id": "cargo-audit",
      "title": "cargo-audit",
      "reason": "Audits Cargo.lock for known security vulnerabilities"
    },
    {
      "id": "cargo-deny",
      "title": "cargo-deny",
      "reason": "Checks dependencies for license issues, bans, and advisories"
    }
  ]
}
```

**Example — search the catalog**

```json
{
  "jsonrpc": "2.0",
  "id": 13,
  "method": "tools/call",
  "params": {
    "name": "tool.find",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "mode": "search",
      "query": "dead code detection",
      "max": 5
    }
  }
}
```

**Example response — search**

```json
{
  "matches": [
    {
      "id": "cargo-udeps",
      "title": "cargo-udeps",
      "reason": "Detects unused dependencies in Rust projects"
    }
  ]
}
```

**Notes**

- `suggest` uses the nano LLM provider pool for reasoning; falls back to FTS5 if all providers are down.
- `search` uses FTS5 full-text search and Qdrant semantic search against `~/.the-one/catalog.db`.
- For `list` mode, omitting `filter` defaults to `all`.

---

### tool.info

Get full metadata for a specific tool from the catalog, including its install state,
version, run command, risk level, and more.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tool_id` | string | yes | Tool ID to query (e.g. `cargo-audit`) |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 14,
  "method": "tools/call",
  "params": {
    "name": "tool.info",
    "arguments": {
      "tool_id": "cargo-audit"
    }
  }
}
```

**Example response**

```json
{
  "id": "cargo-audit",
  "name": "cargo-audit",
  "tool_type": "cli",
  "category": ["security"],
  "languages": ["rust"],
  "description": "Audit Cargo.lock files for crates with known security vulnerabilities.",
  "install_command": "cargo install cargo-audit",
  "run_command": "cargo audit",
  "risk_level": "low",
  "tags": ["security", "audit", "dependencies"],
  "github": "https://github.com/rustsec/rustsec",
  "installed": true,
  "binary_path": "/home/user/.cargo/bin/cargo-audit",
  "version": "0.20.0",
  "enabled": true
}
```

**Notes**

- Returns an error if the `tool_id` is not found in the catalog.
- `installed` reflects the result of the system inventory scan (updated on `maintain tool.refresh`).
- `enabled` is per-CLI per-project; the same tool may be enabled in one project but not another.

---

### tool.install

Install a tool by running its `install_command` from the catalog, then
automatically enable it for the current project.

**Parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tool_id` | string | yes | Tool ID to install |
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 15,
  "method": "tools/call",
  "params": {
    "name": "tool.install",
    "arguments": {
      "tool_id": "cargo-audit",
      "project_root": "/home/user/myproject",
      "project_id": "myproject"
    }
  }
}
```

**Example response**

```json
{
  "installed": true,
  "binary_path": "/home/user/.cargo/bin/cargo-audit",
  "version": "0.20.0",
  "auto_enabled": true,
  "output": "    Updating crates.io index\n     Installing cargo-audit v0.20.0\n    Finished release..."
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `installed` | boolean | Whether installation succeeded |
| `binary_path` | string or null | Path to the installed binary |
| `version` | string or null | Detected version after install |
| `auto_enabled` | boolean | Whether it was auto-enabled for this project |
| `output` | string | Combined stdout/stderr from the install command |

**Notes**

- Installation runs the tool's `install_command` in a subprocess; network access is required.
- If installation fails, `installed` is `false` and `output` contains the error.
- Use `tool.info` after installing to confirm the binary path and version.

---

### tool.run

Request approval and execute a tool action, respecting the configured policy
gate and approval scope. Non-interactive calls require the action to already be
approved (or policy set to auto-approve). Interactive calls can prompt the user.

**Parameters**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `action_key` | string | yes | — | Action key identifying the tool action to run |
| `interactive` | boolean | no | `false` | Whether the user can be prompted for approval |
| `approval_scope` | string | no | `"once"` | Scope of approval: `once`, `session`, or `forever` |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 16,
  "method": "tools/call",
  "params": {
    "name": "tool.run",
    "arguments": {
      "project_root": "/home/user/myproject",
      "project_id": "myproject",
      "action_key": "cargo-audit:audit",
      "interactive": false,
      "approval_scope": "session"
    }
  }
}
```

**Example response — allowed**

```json
{
  "allowed": true,
  "reason": "action approved for this session"
}
```

**Example response — denied**

```json
{
  "allowed": false,
  "reason": "policy gate denied: risk_level=high requires interactive approval"
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `allowed` | boolean | Whether the action was permitted and executed |
| `reason` | string | Human-readable explanation of the decision |

**Notes**

- `action_key` format is typically `<tool-id>:<action-name>` (e.g. `cargo-audit:audit`).
- `approval_scope: "forever"` persists approval to disk — use with caution.
- High-risk actions always require `interactive: true` unless policy overrides this.

---

## Admin Tools (Multiplexed)

The four admin tools (`setup`, `config`, `maintain`, `observe`) each accept an `action`
field and a `params` object. This multiplexed design keeps the MCP tool list compact
while exposing many operations.

---

### setup

Project initialization and profile management. Must be called with
`action: "project"` before any other tool can be used for that project.

**Top-level parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `action` | string | yes | `project`, `refresh`, or `profile` |
| `params` | object | yes | Action-specific parameters (always requires `project_root` and `project_id`) |

---

#### setup — action: project

Initialize a project. Creates the SQLite database, scans docs, seeds the memory
engine, and sets up the tool catalog for this project.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 20,
  "method": "tools/call",
  "params": {
    "name": "setup",
    "arguments": {
      "action": "project",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "project_id": "myproject",
  "profile_version": "v1",
  "fingerprint": "sha256:a3f2..."
}
```

---

#### setup — action: refresh

Re-scan the project to pick up new or changed files, update the memory index,
and regenerate the project fingerprint.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 21,
  "method": "tools/call",
  "params": {
    "name": "setup",
    "arguments": {
      "action": "refresh",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "project_id": "myproject",
  "mode": "incremental",
  "fingerprint": "sha256:b9e1..."
}
```

---

#### setup — action: profile

Retrieve the current project profile as a JSON string. The profile contains
language detection, framework heuristics, and other metadata derived during init.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 22,
  "method": "tools/call",
  "params": {
    "name": "setup",
    "arguments": {
      "action": "profile",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "project_id": "myproject",
  "profile_json": "{\"languages\":[\"rust\"],\"frameworks\":[],\"has_tests\":true}"
}
```

---

### config

Configuration export, live updates, custom tool registration, and embedding
model management.

**Top-level parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `action` | string | yes | `export`, `update`, `tool.add`, `tool.remove`, `models.list`, `models.check` |
| `params` | object | no | Action-specific parameters |

---

#### config — action: export

Export the current resolved configuration for a project. Shows all active
settings including provider, log level, Qdrant connection, and nano-LLM config.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 30,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "export",
      "params": {
        "project_root": "/home/user/myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "schema_version": "v1beta",
  "provider": "local",
  "log_level": "info",
  "qdrant_url": "http://127.0.0.1:6334",
  "qdrant_auth_configured": false,
  "qdrant_ca_cert_path": null,
  "qdrant_tls_insecure": false,
  "qdrant_strict_auth": true,
  "nano_provider": "rules",
  "nano_model": "none"
}
```

---

#### config — action: profile.set

Apply a MemPalace preset (`off`, `core`, or `full`) in one write. The broker
also accepts aliases such as `mempalace_off`, `mempalace_core`, and
`mempalace_full`.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `profile` | string | yes | Preset name: `off`, `core`, or `full` |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 31,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "profile.set",
      "params": {
        "project_root": "/home/user/myproject",
        "profile": "full"
      }
    }
  }
}
```

**Example response**

```json
{
  "path": "/home/user/myproject/.the-one/config.json"
}
```

**Notes**

- `off` disables all MemPalace subfeatures.
- `core` keeps conversation memory enabled but leaves hooks, AAAK, diary, and
  navigation off.
- `full` enables conversation memory, hooks, AAAK, diary, and navigation.

---

#### config — action: update

Apply a partial configuration update for a project. Only the fields present in
`update` are changed; others are left at their current values.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `update` | object | yes | Key-value pairs to merge into the project config |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 31,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "update",
      "params": {
        "project_root": "/home/user/myproject",
        "update": {
          "log_level": "debug",
          "nano_provider": "ollama"
        }
      }
    }
  }
}
```

**Example response**

```json
{
  "path": "/home/user/myproject/.the-one/config.toml"
}
```

---

#### config — action: tool.add

Register a custom tool into the per-user registry. The tool becomes available
to all projects for this user and can be enabled per-project.

**Params fields**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | yes | — | Unique tool identifier |
| `name` | string | yes | — | Display name |
| `tool_type` | string | no | `"cli"` | Tool type (e.g. `cli`, `script`) |
| `category` | string[] | no | `[]` | Category tags |
| `languages` | string[] | no | `[]` | Target languages |
| `description` | string | yes | — | Human-readable description |
| `install_command` | string | yes | — | Command to install the tool |
| `run_command` | string | yes | — | Command to run the tool |
| `risk_level` | string | no | — | `low`, `medium`, or `high` |
| `tags` | string[] | no | `[]` | Searchable tags |
| `github` | string | no | — | GitHub repository URL |
| `cli` | string | no | — | Restrict to a specific CLI client |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 32,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "tool.add",
      "params": {
        "id": "my-linter",
        "name": "My Custom Linter",
        "description": "Internal linting tool",
        "install_command": "npm install -g my-linter",
        "run_command": "my-linter --check .",
        "category": ["linting"],
        "risk_level": "low",
        "tags": ["lint", "internal"]
      }
    }
  }
}
```

**Example response**

```json
{
  "added": true,
  "id": "my-linter"
}
```

---

#### config — action: tool.remove

Remove a custom tool from the registry. Does not affect system catalog tools.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tool_id` | string | yes | ID of the custom tool to remove |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 33,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "tool.remove",
      "params": {
        "tool_id": "my-linter"
      }
    }
  }
}
```

**Example response**

```json
{
  "removed": true
}
```

---

#### config — action: models.list

List available embedding models from the registry, optionally filtered by tier
or provider type.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `filter` | string | no | Filter string (e.g. `local`, `api`, `quality`, `fast`) |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 34,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "models.list",
      "params": {
        "filter": "local"
      }
    }
  }
}
```

**Example response** (structure reflects the model registry format)

```json
[
  {
    "id": "bge-large-en-v1.5",
    "tier": "quality",
    "provider": "local",
    "dims": 1024,
    "description": "BGE Large English v1.5 — high quality, ~1.3 GB"
  },
  {
    "id": "all-minilm-l6-v2",
    "tier": "fast",
    "provider": "local",
    "dims": 384,
    "description": "All-MiniLM-L6-v2 — fast, ~23 MB"
  }
]
```

---

#### config — action: models.check

Check for updates to the embedding model registry. Compares local model
definitions against the latest known versions.

**Params fields**

None required.

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 35,
  "method": "tools/call",
  "params": {
    "name": "config",
    "arguments": {
      "action": "models.check"
    }
  }
}
```

**Example response**

```json
{
  "up_to_date": true,
  "updates_available": [],
  "checked_at": "2026-04-05T12:00:00Z"
}
```

---

### maintain

Housekeeping operations: re-indexing, tool enable/disable, catalog refresh,
trash management, and image index management.

**Top-level parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `action` | string | yes | See actions below |
| `params` | object | no | Action-specific parameters |

---

#### maintain — action: memory.capture_hook

Capture a `stop` or `precompact` hook transcript as first-class conversation
memory. This is the broker-side hook ingestion flow used by MemPalace.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Absolute or project-relative path to the hook transcript |
| `format` | string | yes | Transcript format |
| `event` | string | yes | Hook event name: `stop` or `precompact` |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 40,
  "method": "tools/call",
  "params": {
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
}
```

**Example response**

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

**Notes**

- `event` must be `stop` or `precompact`.
- If `wing`, `hall`, or `room` is omitted, the broker fills deterministic
  defaults based on the project and event name.
- This action also requires `memory_palace_hooks_enabled=true`.

---

#### maintain — action: reindex

Re-index all managed documents for a project. Detects new, updated, removed, and
unchanged files and synchronizes the memory engine accordingly.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 40,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "reindex",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "new": 3,
  "updated": 1,
  "removed": 0,
  "unchanged": 12
}
```

---

#### maintain — action: tool.enable

Enable a tool family for a project. All tools in the specified family become
available for the current CLI in this project.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `family` | string | yes | Tool family name to enable |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 41,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "tool.enable",
      "params": {
        "project_root": "/home/user/myproject",
        "family": "security"
      }
    }
  }
}
```

**Example response**

```json
{
  "enabled_families": ["security", "linting"]
}
```

---

#### maintain — action: tool.disable

Disable a specific tool for a project and CLI.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tool_id` | string | yes | Tool ID to disable |
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 42,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "tool.disable",
      "params": {
        "tool_id": "cargo-audit",
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "disabled": true
}
```

---

#### maintain — action: tool.refresh

Refresh the tool catalog from disk and re-run the system inventory scan
(`which` check) to update the installed/not-installed state for all tools.
No params required.

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 43,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "tool.refresh"
    }
  }
}
```

**Example response**

```json
{
  "catalog_version_before": "v1.2.0",
  "catalog_version_after": "v1.3.0",
  "tools_added": 2,
  "tools_updated": 5,
  "system_tools_found": 47
}
```

---

#### maintain — action: trash.list

List all soft-deleted (trashed) documents for a project.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 44,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "trash.list",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "entries": [
    {
      "path": "docs/old-approach.md",
      "deleted_at": "2026-03-15T10:22:00Z"
    }
  ]
}
```

---

#### maintain — action: trash.restore

Restore a soft-deleted document from trash back to its original path.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Relative path of the document to restore |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 45,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "trash.restore",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject",
        "path": "docs/old-approach.md"
      }
    }
  }
}
```

**Example response**

```json
{
  "restored": true
}
```

---

#### maintain — action: trash.empty

Permanently delete all trashed documents for a project. This operation is
irreversible.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 46,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "trash.empty",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Example response**

```json
{
  "emptied": true
}
```

---

#### maintain — action: images.rescan

Re-scan the project directory for new or changed images and rebuild the image
index. Useful after adding bulk screenshots or design assets.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 47,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "images.rescan",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

**Notes**

- Returns a success value on completion. Check logs for per-image details.
- Requires `image_embedding_enabled: true` in config.

---

#### maintain — action: images.clear

Remove all indexed images for a project from the image memory store. Original
image files on disk are not deleted.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 48,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "images.clear",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject"
      }
    }
  }
}
```

---

#### maintain — action: images.delete

Remove a single indexed image from the image memory store by path. The original
file on disk is not deleted.

**Params fields**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `project_root` | string | yes | Absolute path to the project root |
| `project_id` | string | yes | Unique project identifier |
| `path` | string | yes | Relative path of the image to remove from the index |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 49,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "images.delete",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject",
        "path": "screenshots/old-ui.png"
      }
    }
  }
}
```

---

### observe

Broker metrics and audit event access. Use `metrics` for operational counters
and `events` to review the per-project audit log.

**Top-level parameters**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `action` | string | yes | `metrics` or `events` |
| `params` | object | no | Required for `events`; optional for `metrics` |

---

#### observe — action: metrics

Retrieve a snapshot of broker-wide operational counters. Counters are in-memory
and reset on server restart.

**Params fields**

None required.

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 50,
  "method": "tools/call",
  "params": {
    "name": "observe",
    "arguments": {
      "action": "metrics"
    }
  }
}
```

**Example response**

```json
{
  "project_init_calls": 12,
  "project_refresh_calls": 4,
  "memory_search_calls": 287,
  "tool_run_calls": 53,
  "router_fallback_calls": 2,
  "router_decision_latency_ms_total": 14230,
  "router_provider_error_calls": 1
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `project_init_calls` | integer | Total `setup` project-init calls since start |
| `project_refresh_calls` | integer | Total `setup` refresh calls since start |
| `memory_search_calls` | integer | Total `memory.search` calls since start |
| `tool_run_calls` | integer | Total `tool.run` calls since start |
| `router_fallback_calls` | integer | Times the router fell back to rules-only |
| `router_decision_latency_ms_total` | integer | Cumulative routing latency in ms |
| `router_provider_error_calls` | integer | Total provider errors encountered |

---

#### observe — action: events

Retrieve recent audit log entries for a project. Each entry records a significant
broker operation with its payload and timestamp.

**Params fields**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project_root` | string | yes | — | Absolute path to the project root |
| `project_id` | string | yes | — | Unique project identifier |
| `limit` | integer | no | `50` | Maximum number of events to return (most recent first) |

**Example call**

```json
{
  "jsonrpc": "2.0",
  "id": 51,
  "method": "tools/call",
  "params": {
    "name": "observe",
    "arguments": {
      "action": "events",
      "params": {
        "project_root": "/home/user/myproject",
        "project_id": "myproject",
        "limit": 10
      }
    }
  }
}
```

**Example response**

```json
{
  "events": [
    {
      "id": 42,
      "project_id": "myproject",
      "event_type": "memory.search",
      "payload_json": "{\"query\":\"auth middleware\",\"top_k\":5,\"hits\":2}",
      "created_at_epoch_ms": 1743843600000
    },
    {
      "id": 41,
      "project_id": "myproject",
      "event_type": "tool.run",
      "payload_json": "{\"action_key\":\"cargo-audit:audit\",\"allowed\":true}",
      "created_at_epoch_ms": 1743843500000
    }
  ]
}
```

**Response fields**

| Field | Type | Description |
|-------|------|-------------|
| `events[].id` | integer | Monotonically increasing event ID |
| `events[].project_id` | string | Project this event belongs to |
| `events[].event_type` | string | Type of operation recorded |
| `events[].payload_json` | string | JSON-encoded event-specific payload |
| `events[].created_at_epoch_ms` | integer | Unix timestamp in milliseconds |

**Notes**

- Events are stored in the project's SQLite database and persist across restarts.
- Older events are not automatically pruned; use `maintain trash.empty` patterns
  or direct DB management to limit growth.

---

## MCP Resources

v0.10.0 added first-class support for the MCP `resources/list` and
`resources/read` primitives alongside the existing `tools/*`. Resources are
invoked directly on the JSON-RPC envelope — they do NOT go through `tools/call`.

See the dedicated [MCP Resources Guide](mcp-resources.md) for the full URI
scheme, security model, and client integration patterns.

### Resource types (v0.12.0)

| URI | MIME type | Content |
|-----|-----------|---------|
| `the-one://docs/<relative-path>` | `text/markdown` | A managed doc under `.the-one/docs/` |
| `the-one://project/profile` | `application/json` | Project profile (languages, frameworks, commands) |
| `the-one://catalog/enabled` | `application/json` | Enabled tool IDs from `.the-one/catalog/catalog.db` for the project |

### resources/list

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "resources/list",
  "params": {
    "project_root": "/abs/path",
    "project_id": "my-project"
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "resources": [
      { "uri": "the-one://docs/README.md", "name": "README.md", "description": "Managed doc: README.md", "mimeType": "text/markdown" },
      { "uri": "the-one://project/profile", "name": "Project profile", "description": "Profile metadata for this project", "mimeType": "application/json" },
      { "uri": "the-one://catalog/enabled", "name": "Enabled tools", "description": "Tools from the global catalog that are enabled for this project", "mimeType": "application/json" }
    ]
  }
}
```

### resources/read

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "resources/read",
  "params": {
    "project_root": "/abs/path",
    "project_id": "my-project",
    "uri": "the-one://docs/architecture.md"
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "contents": [
      { "uri": "the-one://docs/architecture.md", "mimeType": "text/markdown", "text": "# Architecture\n..." }
    ]
  }
}
```

### Initialize handshake capability

v0.10.0+ advertises the resources capability during `initialize`:

```json
{
  "capabilities": {
    "tools": {},
    "resources": { "subscribe": false, "listChanged": false }
  }
}
```

**Security:** `docs` identifiers containing `..`, absolute paths, NUL bytes,
tilde prefixes, or non-Normal path components are rejected before the
filesystem is touched.

---

## maintain: backup and restore (v0.12.0)

The `maintain` multiplexed tool gained two new actions in v0.12.0 for moving
project state between machines. Full guide: [Backup & Restore](backup-restore.md).

### maintain (action: backup)

Request:

```json
{
  "name": "maintain",
  "arguments": {
    "action": "backup",
    "params": {
      "project_root": "/abs/path",
      "project_id": "my-project",
      "output_path": "/abs/path/to/backup.tar.gz",
      "include_images": true,
      "include_qdrant_local": false
    }
  }
}
```

Response:

```json
{
  "output_path": "/abs/path/to/backup.tar.gz",
  "size_bytes": 2345678,
  "file_count": 142,
  "manifest_version": "1"
}
```

| Param | Required | Default | Meaning |
|-------|----------|---------|---------|
| `project_root` | yes | — | Absolute project path |
| `project_id` | yes | — | Project identifier |
| `output_path` | yes | — | Where to write the tarball (parent dirs auto-created) |
| `include_images` | no | `true` | Include indexed images + thumbnails |
| `include_qdrant_local` | no | `false` | Include local Qdrant storage (can be large) |

The archive contains `<project>/.the-one/` tree, `~/.the-one/catalog.db`,
`~/.the-one/registry/`, and a `backup-manifest.json` at the root. Excludes
`.fastembed_cache/`, Qdrant wal/raft state, `.DS_Store`.

### maintain (action: restore)

Request:

```json
{
  "name": "maintain",
  "arguments": {
    "action": "restore",
    "params": {
      "backup_path": "/abs/path/to/backup.tar.gz",
      "target_project_root": "/abs/path/to/new-location",
      "target_project_id": "my-project",
      "overwrite_existing": false
    }
  }
}
```

Response:

```json
{
  "restored_files": 142,
  "warnings": []
}
```

Refuses to overwrite existing `.the-one/` state unless `overwrite_existing: true`.
Rejects unsafe archive paths (absolute, `..`, non-Normal components). Validates
the manifest version before unpacking.

---

## observe: metrics (v0.12.0 fields)

`observe: action: metrics` returns the in-memory counter snapshot. v0.12.0
added 8 new fields plus a derived average latency. All new fields are
`#[serde(default)]` so older clients don't break.

Full response schema:

```json
{
  "project_init_calls": 0,
  "project_refresh_calls": 0,
  "memory_search_calls": 0,
  "tool_run_calls": 0,
  "router_fallback_calls": 0,
  "router_decision_latency_ms_total": 0,
  "router_provider_error_calls": 0,
  "memory_search_latency_ms_total": 0,
  "memory_search_latency_avg_ms": 0,
  "image_search_calls": 0,
  "image_ingest_calls": 0,
  "resources_list_calls": 0,
  "resources_read_calls": 0,
  "watcher_events_processed": 0,
  "watcher_events_failed": 0,
  "qdrant_errors": 0
}
```

See the [Observability Guide](observability.md) for how to use these for debugging.

---

## Error Reference

All errors follow the JSON-RPC error format:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "missing project_root"
  }
}
```

Common error scenarios:

| Scenario | Code | Example message |
|----------|------|----------------|
| Unknown method | `-32601` | `method not found: tools/unknown` |
| Missing required param | `-32602` | `missing project_root` |
| Unknown tool name | `-32603` | `unknown tool: foo.bar` |
| Unknown action | `-32603` | `unknown setup action: deploy` |
| Project not initialized | `-32603` | `project not found: myproject` |
| Tool not in catalog | `-32603` | `tool not found: my-tool` |
| Install failed | `-32603` | `installation failed: exit code 1` |
| Image indexing disabled | `-32603` | `image embedding not enabled` |
