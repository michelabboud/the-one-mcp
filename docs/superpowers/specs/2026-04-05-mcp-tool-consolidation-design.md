# MCP Tool Consolidation: 33 → 15

**Date:** 2026-04-05
**Goal:** Reduce MCP tool token cost by ~50% (3,536 → ~1,700 tokens) by consolidating rarely-used admin tools and merging overlapping work tools.

## Motivation

Every MCP session sends all 33 tool schemas to the LLM on init (~3,536 tokens). Over half these tools are admin/config/maintenance operations used infrequently — project setup, config changes, catalog maintenance, observability. They cost tokens on every request regardless.

## Strategy

Two consolidation approaches applied together:

1. **Merge overlapping work tools** — combine tools that share the same domain and differ only in a parameter (e.g., `docs.get` + `docs.get_section`, `docs.create` + `docs.update`, `tool.list` + `tool.suggest` + `tool.search`).

2. **Multiplex admin tools** — collapse 18 admin/config/maintenance tools into 4 multiplexed tools, each with an `action` enum parameter and a flexible `params` object.

## Work Tools (11)

These are the tools used during active coding sessions.

### Unchanged (8)

| Tool | Description |
|------|-------------|
| `memory.search` | Semantic search over indexed project documentation |
| `memory.fetch_chunk` | Fetch full content of a specific memory chunk by ID |
| `docs.list` | List all indexed documentation paths |
| `docs.delete` | Soft-delete a managed document (moves to trash) |
| `docs.move` | Rename or move a managed document |
| `tool.info` | Full metadata for a specific tool |
| `tool.install` | Install a tool by running its install command |
| `tool.run` | Request approval and run a tool action |

### Merged (3 new tools replacing 7 old ones)

#### `docs.get` (replaces `docs.get` + `docs.get_section`)

Add an optional `section` parameter. When omitted, returns the full document. When provided, extracts the named section.

**Parameters:**
- `project_root` (string, required)
- `project_id` (string, required)
- `path` (string, required)
- `section` (string, optional) — heading text to extract; omit for full document

**Dispatch logic:** If `section` is present, delegate to the existing `docs.get_section` broker method. Otherwise, delegate to `docs.get`.

#### `docs.save` (replaces `docs.create` + `docs.update`)

Upsert semantics: if the document at `path` exists, update it. If not, create it.

**Parameters:**
- `project_root` (string, required)
- `project_id` (string, required)
- `path` (string, required)
- `content` (string, required)
- `tags` (array of strings, optional) — on update, replaces existing tags entirely

**Dispatch logic:** Check if the document exists. If yes, call the existing `docs.update` broker method. If no, call `docs.create`.

#### `tool.find` (replaces `tool.list` + `tool.suggest` + `tool.search`)

Unified tool discovery with a `mode` parameter.

**Parameters:**
- `project_root` (string, required)
- `project_id` (string, required)
- `mode` (enum: `"list"` | `"suggest"` | `"search"`, required)
- `filter` (string, optional) — for `list` mode: `"enabled"` | `"available"` | `"recommended"` | `"all"`
- `query` (string, optional) — for `search` mode: the search query
- `cli` (string, optional) — for `suggest`/`list` modes: client name

**Dispatch logic:** Match on `mode` and delegate to the existing `tool_list`, `tool_suggest`, or `tool_search` broker methods.

## Admin Tools (4 multiplexed)

Each multiplexed tool has:
- `action` (string enum, required) — selects the operation
- `params` (object, optional) — operation-specific parameters

The tool description includes a brief summary of all available actions so the LLM knows when to use it.

### `setup` (3 actions)

Project initialization and profile management. Used during first-time setup or when the project changes.

| Action | Was | Params |
|--------|-----|--------|
| `project` | `project.init` | `project_root`, `project_id` |
| `refresh` | `project.refresh` | `project_root`, `project_id` |
| `profile` | `project.profile.get` | `project_root`, `project_id` |

### `config` (6 actions)

Configuration, custom tool management, and embedding model info.

| Action | Was | Params |
|--------|-----|--------|
| `export` | `config.export` | `project_root`, `project_id` |
| `update` | `config.update` | `project_root`, `project_id`, `fields` (object) |
| `tool.add` | `tool.add` | `project_root`, `project_id`, `name`, `description`, `install_command`, `tags`, `cli` (optional) |
| `tool.remove` | `tool.remove` | `project_root`, `project_id`, `name` |
| `models.list` | `models.list` | `tier` (optional), `provider_type` (optional) |
| `models.check` | `models.check_updates` | *(none)* |

### `maintain` (7 actions)

Housekeeping: re-indexing, tool enable/disable, catalog refresh, trash management.

| Action | Was | Params |
|--------|-----|--------|
| `reindex` | `docs.reindex` | `project_root`, `project_id` |
| `tool.enable` | `tool.enable` | `project_root`, `project_id`, `name`, `cli` |
| `tool.disable` | `tool.disable` | `project_root`, `project_id`, `name`, `cli` |
| `tool.refresh` | `tool.update` | `project_root`, `project_id` |
| `trash.list` | `docs.trash.list` | `project_root`, `project_id` |
| `trash.restore` | `docs.trash.restore` | `project_root`, `project_id`, `path` |
| `trash.empty` | `docs.trash.empty` | `project_root`, `project_id` |

### `observe` (2 actions)

Metrics and audit log access.

| Action | Was | Params |
|--------|-----|--------|
| `metrics` | `metrics.snapshot` | `project_root`, `project_id` |
| `events` | `audit.events` | `project_root`, `project_id`, `limit` (optional), `tool_name` (optional) |

## Files to Modify

| File | Changes |
|------|---------|
| `crates/the-one-mcp/src/transport/tools.rs` | Replace 33 tool definitions with 15. Add schemas for merged and multiplexed tools. |
| `crates/the-one-mcp/src/transport/jsonrpc.rs` | Update `dispatch_tool()` match arms: remove old names, add new ones with internal sub-dispatch for multiplexed tools. |
| `crates/the-one-mcp/src/api.rs` | Add request/response types for `docs.save`, `tool.find`, and the 4 admin multiplexed tools (`SetupRequest`, `ConfigRequest`, `MaintainRequest`, `ObserveRequest`). Remove types that become unused. |
| `crates/the-one-mcp/src/broker.rs` | Add `docs_save()` upsert method, `tool_find()` dispatch method. Existing handler methods remain — they're called internally by the new dispatch. |
| `schemas/mcp/v1beta/` | Add new schemas, remove old ones, update test expectations. |
| `crates/the-one-mcp/src/lib.rs` | Update schema validation tests for new tool names. |
| `CLAUDE.md` | Update tool count (33 → 15) and any tool name references. |

## Backward Compatibility

This is a **breaking change** to the MCP tool surface. All 4 supported clients (Claude Code, Gemini CLI, OpenCode, Codex) will see the new tool names immediately since they discover tools via `tools/list` at session init. No migration period needed — these are machine-consumed APIs, not user-facing.

## Token Budget Estimate

| Category | Tools | Estimated Tokens |
|----------|-------|-----------------|
| Work tools (unchanged) | 7 | ~850 |
| Work tools (merged) | 3 | ~450 |
| Admin tools (multiplexed) | 4 | ~400 |
| **Total** | **15** | **~1,700** |

**Savings: ~1,836 tokens per session (~52% reduction)**

## Testing

- All existing broker method tests remain valid (handlers don't change).
- New dispatch tests for merged tools (`docs.get` with/without `section`, `docs.save` upsert, `tool.find` modes).
- New dispatch tests for multiplexed tools (each action routes correctly).
- Schema validation tests updated for new tool names.
- Integration test: `tools/list` returns exactly 15 tools.
