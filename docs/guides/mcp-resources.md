# MCP Resources Guide

> v0.10.0 introduced first-class support for the MCP `resources/list` and
> `resources/read` primitives alongside the existing `tools/*`. This guide
> covers the URI scheme, the default resource set, security, and client
> integration patterns.

## Why resources?

The Model Context Protocol exposes three primitives to clients: **tools**,
**resources**, and **prompts**. Before v0.10.0, the-one-mcp exposed only
tools — every piece of project data had to be fetched via a tool call like
`memory.search` or `docs.get`.

Resources are the MCP way to expose *browsable reference content*. When a
server advertises resources, MCP clients (Claude Code, etc.) can:

- List them in a file-tree-like picker
- Let users `@`-reference them in conversations
- Attach them as context without running a tool
- Cache them by URI

For the-one-mcp, resources give you a stable, URI-addressable view of your
indexed project content. You don't have to guess which `memory.search` query
would find a particular doc — you can just list and read.

---

## URI scheme

Every resource uses the `the-one://` scheme with the form:

```
the-one://<resource_type>/<identifier>
```

The three supported resource types in v0.10.0+:

| Resource type | Identifier | Example URI | MIME type |
|---------------|------------|-------------|-----------|
| `docs` | relative path under `.the-one/docs/` | `the-one://docs/architecture.md` | `text/markdown` |
| `project` | always `profile` | `the-one://project/profile` | `application/json` |
| `catalog` | always `enabled` | `the-one://catalog/enabled` | `application/json` |

### Managed docs (`docs`)

Every file under `<project>/.the-one/docs/` is exposed as a `docs` resource.
Subdirectories are walked recursively. Paths are relative to the docs
directory and use forward slashes regardless of host OS.

Examples:

```
the-one://docs/architecture.md
the-one://docs/notes/2026-04-05-session.md
the-one://docs/runbooks/incident-response.md
```

Trash files under `.the-one/docs/.trash/` are **not** exposed — they are
intentionally hidden from the resource list so deleted content doesn't leak
back to clients.

### Project profile (`project/profile`)

Returns the current project profile JSON — languages, frameworks, test
frameworks, typical build commands. Always available even on fresh projects
(returns `{}` if the profile hasn't been generated yet).

### Enabled tools (`catalog/enabled`)

Returns the list of enabled tools for this project from the catalog database.
The content is a JSON array of tool IDs.

Implementation notes (v0.14.2+):

- Data is read from `.the-one/catalog/catalog.db` enabled-tool state for the
  active project root.
- Results are deduplicated and normalized before returning to the client.

---

## JSON-RPC methods

### `resources/list`

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "resources/list",
  "params": {
    "project_root": "/path/to/project",
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
      {
        "uri": "the-one://docs/architecture.md",
        "name": "architecture.md",
        "description": "Managed doc: architecture.md",
        "mimeType": "text/markdown"
      },
      {
        "uri": "the-one://project/profile",
        "name": "Project profile",
        "description": "Profile metadata for this project (languages, frameworks, tests, commands)",
        "mimeType": "application/json"
      },
      {
        "uri": "the-one://catalog/enabled",
        "name": "Enabled tools",
        "description": "Tools from the global catalog that are enabled for this project",
        "mimeType": "application/json"
      }
    ]
  }
}
```

The response always includes at least `project/profile` and `catalog/enabled`
even on empty projects. Managed doc entries appear in the order they were
discovered by `fs::read_dir` — do not rely on sort order.

### `resources/read`

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "resources/read",
  "params": {
    "project_root": "/path/to/project",
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
      {
        "uri": "the-one://docs/architecture.md",
        "mimeType": "text/markdown",
        "text": "# Architecture\n..."
      }
    ]
  }
}
```

Multiple content blocks are allowed by the MCP spec but v0.10.0 always
returns exactly one.

---

## Initialize handshake

Servers must advertise resource capability to clients during `initialize`.
Starting in v0.10.0, the-one-mcp's initialize response includes:

```json
{
  "protocolVersion": "2024-11-05",
  "capabilities": {
    "tools": {},
    "resources": {
      "subscribe": false,
      "listChanged": false
    }
  },
  "serverInfo": {
    "name": "the-one-mcp",
    "version": "v1beta"
  }
}
```

- **`subscribe: false`** — v0.10.0 does not support client subscriptions for
  change notifications. A future version may add this.
- **`listChanged: false`** — the server will not proactively emit
  `notifications/resources/list_changed`. Clients should re-list on demand.

---

## Security

### Path traversal

The `docs` resource type is the only one that touches the filesystem with a
user-supplied identifier. v0.10.0 rejects any of the following before
opening a file:

- Empty identifier
- Any `..` component (e.g. `the-one://docs/../../etc/passwd`)
- Absolute paths (e.g. `the-one://docs//etc/passwd`)
- NUL bytes
- Tilde expansion attempts (e.g. `the-one://docs/~/.ssh/id_rsa`)
- Drive prefixes on Windows
- Any path component that isn't `Normal` in the `std::path::Component` sense

Rejection surfaces as a JSON-RPC `-32603` (Internal Error) with a message
like `"unsafe or invalid docs identifier: ../../etc/passwd"`. The file is
never opened. The validation logic lives in
`crate::resources::is_safe_doc_identifier` and is tested in
`crates/the-one-mcp/src/resources.rs` and
`crates/the-one-mcp/src/transport/jsonrpc.rs`.

### Cross-project isolation

`project_root` is required on every `resources/list` and `resources/read`
call. There is no way to ask for "all docs across all projects" — clients
must scope per project. This mirrors the rest of the-one-mcp API.

---

## Client integration patterns

### Claude Code

Claude Code picks up resources automatically once the initialize handshake
advertises the capability. Users see indexed docs as attachable references
in the `@`-picker.

### Custom MCP clients

To integrate with a custom client:

1. Send `initialize` and confirm the server returned `capabilities.resources`
2. Send `resources/list` to populate a picker
3. Send `resources/read` when the user selects one
4. Embed the returned `contents[].text` into the model prompt or UI

For reliable resource caching, clients should key by the full URI — it
uniquely identifies the content across sessions.

### Raw JSON-RPC testing

```bash
# List resources via the stdio transport
the-one-mcp serve <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{"project_root":"/abs/path/to/project","project_id":"test"}}
EOF
```

---

## Future extensions (not in v0.10.0)

These are documented here so expectations are clear:

- **`images/<hash>`** — one resource per indexed image. Needs MIME-aware
  binary content blocks.
- **`chunks/<chunk_id>`** — direct access to individual memory chunks by ID.
  Useful for LLMs that want to follow a `memory.search` hit to its source.
- **`resources/subscribe`** — clients get notified when indexed docs change.
  Would combine with the v0.8.0 auto-reindex watcher.
- **`notifications/resources/list_changed`** — server-initiated notification
  sent whenever the watcher detects a new or removed managed doc.

---

## See also

- [Auto-Indexing Guide](auto-indexing.md) — how managed docs get into the index
- [API Reference](api-reference.md) — full JSON-RPC schema for tools and resources
- [Architecture](architecture.md) — where resources sit in the broker architecture
