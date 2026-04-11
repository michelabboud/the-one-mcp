# Comparative Audit: `milla-jovovich/mempalace` v3.1.0 → `the-one-mcp`

**Author:** Production Hardening Pass
**Date:** 2026-04-10
**Scope:** Take findings from an external review of the Python project `milla-jovovich/mempalace` v3.1.0 and audit `the-one-mcp` for the same class of issues. Document every confirmed issue, then fix each one with production-grade code, tests, and benchmarks. No corner-cutting, no stubs, no "good enough".

---

## Part 1 — External Review Summary (`milla-jovovich/mempalace` v3.1.0)

### Context

- Python 3.9+ MCP memory system, ChromaDB + SQLite KG backend, 19 MCP tools.
- 39k stars, 346 open issues, single global Chroma collection, single global knowledge graph.
- Hand-rolled stdio JSON-RPC server, 946-line `mcp_server.py`.
- Test coverage gated at 85%, but issue #538 (stdio write path silently fails) shipped anyway.
- Publisher note (April 7, 2026) retracted "96.6% with AAAK", "30× lossless compression" and "+34% palace boost" claims.

### Findings against mempalace

| Id | Severity | Finding |
| --- | --- | --- |
| C1 | Critical | `_wal_log` records `result=None` for every write and is never updated with the actual outcome. It is mis-labelled as a WAL — cannot be used for rollback or drift detection. |
| C2 | Critical | `_get_collection(create=False)` bare-except swallows the real error, returns `None`, callers fall through to `_no_palace()` responses; root cause of issue #538 (stdio writes never land in ChromaDB). |
| C3 | Critical | Deterministic drawer IDs hash only `content[:100]`: two drawers with the same first 100 chars silently collide and become a no-op "already exists" write, losing data. |
| C4 | Critical | KG `_entity_id` is `name.lower().replace(' ', '_').replace("'", "")`. `"Max Jones"` collides with `"max_jones"`, `"O'Brien"` collides with `"OBrien"`. No alias map, no collision detection. |
| C5 | Critical | `tool_status`, `tool_list_wings`, `tool_list_rooms`, `tool_get_taxonomy` all fetch all metadata with `limit=10_000` and aggregate client-side; silent truncation at 10k drawers. Runs on every wake-up. |
| H1 | High | Bare `except Exception: pass` in every list operation, `_get_collection`, and `file_already_mined`. |
| H2 | High | Internal exception strings leaked to the LLM client (`return {"error": str(e)}`). |
| H3 | High | `_SAFE_NAME_RE` permits `.`, spaces, single-quotes — names can later break path-joining code. |
| H4 | High | No end-to-end stdio write test: server is only unit-tested at handler level. |
| H5 | High | `file_already_mined` uses exact float equality on mtime. Breaks on FAT/NFS granularity. |
| M1 | Medium | `KnowledgeGraph.query_entity` default is `outgoing`, `tool_kg_query` default is `both`. |
| M2 | Medium | `sqlite3.connect(..., check_same_thread=False)` with no locking. |
| M3 | Medium | `chromadb>=0.5.0,<0.7` spans a Chroma API-breaking boundary. |
| M4 | Medium | Stop-hook uses `eval $(python3 -c ...)` to set shell vars. |
| M5 | Medium | `PALACE_PROTOCOL` instructions are embedded into the data portion of `tool_status` responses — self-prompt-injection. |
| M6 | Medium | AAAK regresses LongMemEval by 12.4 points yet is still in the default protocol. |

---

## Part 2 — Audit of `the-one-mcp` against the same concerns

For each finding, does the-one-mcp have an analogous issue? The answer drives the fix plan in Part 3.

### C1 — Audit-log records no result

**Status: CONFIRMED — weaker form**

`the-one-mcp` has a proper audit event system (`audit_events` table, `record_audit_event()`, `list_audit_events()`). The table stores an event_type + payload_json pair. **But the broker only records audit events for `tool_run` approvals** (`broker.rs:2892,2903,2912,2937`). Every other state-changing operation runs without an audit trail:

- `memory_ingest_conversation` — no audit event
- `memory_aaak_teach` — no audit event
- `memory_diary_add` — no audit event
- `memory_navigation_upsert_node` / `link_tunnel` — no audit event
- `docs_create` / `docs_update` / `docs_delete` / `docs_move` — no audit event
- `tool_install` / `tool_remove` / `tool_enable` / `tool_disable` — no audit event
- `config_update` / `project_init` / `project_refresh` — no audit event
- `backup` / `restore` — no audit event

Also, `record_audit_event` takes `event_type` + `payload_json` only — there is no explicit `outcome` / `error_kind` column. The mempalace pattern of "log attempt, never log result" is not quite our problem because we log *nothing* for most operations.

Production-grade fix: audit every state-changing broker method with a structured entry containing `operation`, `params` (sanitised), `outcome ∈ {ok,error}`, and `error_kind` (if error). Add a schema migration for an `outcome` column + an `error_kind` column. Back-compat: older rows get `outcome='unknown'`.

### C2 — Silent error swallowing on load paths

**Status: CONFIRMED — multiple sites**

Worst offenders in `broker.rs`:

| Line | Code | Problem |
| --- | --- | --- |
| 157 | `let _ = self.registry.save_to_path(path);` | registry save failure silently dropped |
| 1474 | `let config = AppConfig::load(...).ok();` | config load failure downgraded to None with no log |
| 1489-1491 | `let _ = self.ensure_project_memory_loaded(...).await;` | memory engine load failure silently dropped, then the search returns empty results |
| 2326 | `let _ = catalog.import_catalog_dir(&dir);` | catalog import failure silently dropped |
| 2328 | `let _ = catalog.scan_system_inventory();` | inventory scan failure silently dropped |
| 2467-2469 | `let _ = cat.scan_system_inventory(); let _ = cat.enable_tool(...)` | post-install auto-enable failure silently dropped yet response claims `auto_enabled: true` |
| 2816 | `self.ensure_catalog().ok();` | catalog ensure failure silently dropped before fallback search |

Production-grade fix: replace every `let _ =` / `.ok()` on non-trivial results with `if let Err(e) = ... { tracing::warn!(error = %e, ...) }`. Fix the `auto_enabled` response lie so it reflects the actual result.

### C3 — Truncated content hash / identifier collisions

**Status: CONFIRMED — `navigation_digest` in `broker.rs:732-741`**

```rust
fn navigation_digest(seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..12]      // ← 48 bits of collision resistance
        .to_string()
}
```

Collision profile:
- 12 hex chars = 48 bits
- Birthday bound: ~2^24 = 16.7M distinct drawer/closet/room IDs before 50% chance of collision
- Used for drawer, closet, room, and tunnel IDs

The `(project_id, node_id)` primary key does prevent cross-project collisions. But **within a single project** at scale (exactly the "store everything verbatim" use case mempalace also targets), 48 bits is not defensible.

`navigation_drawer_node_id`, `navigation_closet_node_id`, `navigation_room_node_id` don't fold `project_id` into the hash input — they rely on the composite primary key. That's correct for uniqueness but means the bit count *must* cover the worst-case cardinality in the biggest single project.

Production-grade fix: widen to 32 hex chars (128 bits, 2^64 birthday bound ≈ 18 quintillion) and fold the project_id into the seed so two projects that happen to hash to the same prefix stay separable even if the primary-key layer is ever relaxed.

### C4 — Entity ID normalisation / alias collisions

**Status: CONFIRMED — `graph.rs:94,154,158,296`**

```rust
// graph.rs:94 — load_from_file
graph.entities.insert(entity.name.to_lowercase(), entity);

// graph.rs:158 — merge_extraction
let canonical = normalize_entity_name(&entity.name);
let key = canonical.to_lowercase();

// graph.rs:296 — get_entity_mut
let key = normalize_entity_name(name).to_lowercase();
```

`normalize_entity_name` is a pure function (strip punctuation → title-case), but the key used for hashing is always `.to_lowercase()`. So:

- `"Max Jones"` → canonical `"Max Jones"` → key `"max jones"`
- Literal `"max jones"` → canonical `"Max Jones"` → key `"max jones"` ← collides
- `"O'Brien"` → stripped → canonical `"O'brien"` → key `"o'brien"`
- `"OBrien"` (typo without apostrophe) → canonical `"Obrien"` → key `"obrien"` — different
- But `"obrien"` and `"Obrien"` and `"OBRIEN"` all collide.

More serious: no alias map, no project-scope. Entity data is cross-document; a `Max Jones` in one chunk and a `max jones` in another always merge silently.

Production-grade fix: keep the canonical display name for presentation, but use a collision-resistant identity hash (`blake3(project_id + ":" + canonical)`) as the storage key. Retain a `name_aliases` side map so different surface forms that deliberately normalize to the same canonical still merge, while surface forms that don't share a canonical do not. Add `ascii_folding` (NFKC normalisation) so `"café"` and `"cafe"` follow a documented rule.

### C5 — O(N) list operations with silent caps

**Status: CONFIRMED — multiple sites**

| Location | Problem |
| --- | --- |
| `broker.rs:2134` `memory_navigation_list` | `list_navigation_tunnels(None)` fetches every tunnel in the project, then filters in Rust |
| `broker.rs:2172-2173` `memory_navigation_traverse` | hard-codes `2_000` nodes + unbounded tunnels fetch |
| `sqlite.rs:203` `list_audit_events` | `limit.min(200)` — silent truncation to 200 |
| `sqlite.rs:444` `list_diary_entries` | `limit.clamp(1, 200)` — silent truncation |
| `sqlite.rs:500` `search_diary_entries_in_range` | `limit.clamp(1, 200)` — silent truncation |
| `sqlite.rs:625` `list_aaak_lessons` | `limit.clamp(1, 200)` — silent truncation |
| `sqlite.rs:733` `list_navigation_nodes` | `limit.clamp(1, 2_000)` — silent truncation |

No pagination cursors, no `next_cursor` / `total_count` in responses, no way for a client to tell it was truncated.

Production-grade fix: every list endpoint must

1. Accept an explicit `cursor` parameter (opaque base64-encoded offset + tiebreaker).
2. Return a `next_cursor: Option<String>` and a `total_count: Option<u64>` so clients can paginate deterministically.
3. Cap the per-page limit but reject over-limit requests with `InvalidRequest` instead of silently truncating — **no silent caps, ever**.
4. `memory_navigation_list` must do the tunnel filter in SQL, not Rust. `memory_navigation_traverse` must BFS against paged queries instead of `list_all`.

### H1 — Error swallowing

**Status: CONFIRMED — same sites as C2 plus watcher path**

Already covered in C2. Plus `broker.rs:2651-2741` `ensure_tools_embedded` has an entire async block whose errors are logged at `debug!` level only.

### H2 — Error message leakage into MCP responses

**Status: CONFIRMED — `transport/jsonrpc.rs`**

- Line 131: `JsonRpcResponse::error(id, INTERNAL_ERROR, e.to_string())` — resources/list
- Line 168: same for resources/read
- Every `dispatch_tool` arm does `.await.map_err(|e| e.to_string())?` (250-ish sites)
- Line 215: `JsonRpcResponse::error(id, INTERNAL_ERROR, e)` — tools/call

`CoreError::Sqlite(#[from] rusqlite::Error)` passes the full rusqlite message (which includes schema details, SQL text, parameter types) straight through. `CoreError::Io(#[from] std::io::Error)` passes the OS error message which on Linux includes `os error 2: No such file or directory` — fine — but on macOS sometimes includes file paths.

`broker.rs:1590,1637,1665` also include `transcript_path.display().to_string()` in successful responses, so this also leaks on the happy path.

Production-grade fix:
1. Introduce a `ClientError` type with `code`, `public_message`, `correlation_id`, and an internal `Detail` that stays server-side.
2. `From<CoreError>` maps each variant to a generic public message: `"sqlite failure"`, `"io failure"`, etc. — never the inner message.
3. The full details go through `tracing::error!(correlation_id=..., error=%e)` and the `correlation_id` goes back to the client so they can grep the server log.
4. `broker.rs` response bodies must return `source_path` only if it's a repo-relative path (already-sanitised). If it's an absolute path we either strip to the project root or return just the file stem.

### H3 — Name sanitisation

**Status: CONFIRMED — gap**

`docs_manager.rs:233-254` has `validate_path` (good — blocks `..`, enforces `.md` extension, whitelist charset). `resources.rs:93-121` has `is_safe_doc_identifier` (good — blocks path traversal, null bytes, absolute paths).

But:
- `wing`, `hall`, `room` names flow through `memory_ingest_conversation`, `memory_diary_add`, `memory_navigation_upsert_node`, `config_status`, etc. with no validation.
- `project_id` is similarly unvalidated.
- `action_key` (used in approvals and audit) has no validation.

Production-grade fix: introduce `the_one_core::naming::sanitize_name` and call it at every entry point. Charset: `[A-Za-z0-9 ._\-]`, max 128 chars, no `..`, no null, no leading/trailing whitespace or dot, required non-empty after trimming.

### H4 — End-to-end stdio tests

**Status: CONFIRMED — no stdio integration tests**

`crates/the-one-mcp/tests/` does not exist. `transport/stdio.rs` is 55 lines and has no tests. Every broker test calls the handler function directly, skipping the JSON-RPC serialisation + transport layer.

Production-grade fix: add `crates/the-one-mcp/tests/stdio_write_path.rs` with tests that:
1. Spawn the binary as a subprocess (`cargo run -p the-one-mcp --bin the-one-mcp -- serve`), or better, drive the `serve_stdio_from_pipes` function in-process with stdin/stdout pipes.
2. Send `initialize` → `tools/list` → `tools/call` JSON-RPC frames.
3. Verify that write tools (`docs.create`, `memory.ingest_conversation`, `memory.navigation.upsert_node`, `memory.diary.add`) actually land in SQLite after the call returns.
4. Verify error responses do NOT leak internal strings.

### H5 — mtime float equality

**Status: NOT FOUND — already correct**

`docs_manager.rs:321-327` stores modified time as `u64` milliseconds, and `project.rs:94-97` uses SHA-256 content fingerprints for cache invalidation, not mtime. No fix needed here. We will still *add a regression test* to document the invariant.

### M1 — Default argument mismatch

**Status: NOT FOUND**

Rust's type system makes this class of bug hard to introduce — no default-value drift between caller and callee. No fix needed.

### M2 — SQLite thread safety

**Status: CONFIRMED — not unsafe today, but fragile**

`ProjectDatabase` and `ToolCatalog` both hold `rusqlite::Connection` without a `Mutex`. `rusqlite::Connection` is `Send` but not `Sync`, so the Rust borrow checker prevents concurrent `&` access inside a single broker method. But:

- The broker holds the catalog in `std::sync::Mutex<Option<ToolCatalog>>` — the poisoning story is handled.
- `ProjectDatabase` is not held by the broker; it's opened per broker method via `ProjectDatabase::open(...)?`. So every call walks the file system, re-runs migrations, and re-parses PRAGMAs. That's a performance bug in addition to a correctness risk if we ever want to do concurrent writes from a watcher task.

Production-grade fix:
1. Cache `ProjectDatabase` per `(project_root, project_id)` inside the broker in a `RwLock<HashMap<String, Arc<Mutex<ProjectDatabase>>>>`.
2. Add a unit test that deliberately makes two concurrent broker method calls and asserts they both succeed.
3. `rusqlite` WAL mode + busy_timeout stays.

### M3 — Dependency pinning

**Status: NOT FOUND — already correct**

All workspace deps are pinned tightly. No fix needed.

### M4 — Shell-script safety

**Status: N/A — no shell hooks in the-one-mcp**

`scripts/install.sh` exists but does not use `eval $(...)` with untrusted input. No fix needed.

### M5 — Protocol instruction injection

**Status: CONFIRMED — weaker form**

The broker does not bake PALACE_PROTOCOL-style instruction text into data responses. BUT tool **descriptions** in `transport/tools.rs` are operational, which is fine. We should explicitly *test* that no tool description contains imperative AI directives ("always", "never", "you must", "on wake-up") to prevent future drift.

Production-grade fix: add a hygiene test `test_tool_descriptions_are_descriptive_not_imperative` in `transport/tools.rs`.

### M6 — Feature regressions in default path

**Status: N/A**

No AAAK analogue in the-one-mcp has a documented benchmark regression. AAAK here is an opt-in subfeature under `memory_palace_aaak_enabled`. No fix needed but we will document the decision in the guide.

---

## Part 3 — Fix plan

Numbered in execution order. Every item is production-grade — no stubs.

1. **`the_one_core::naming`** — new module with `sanitize_name`, `sanitize_project_id`, `sanitize_action_key`. Wire into every broker entry point.
2. **`the_one_core::audit` record + migration** — add `outcome` and `error_kind` columns via schema v7 migration. Keep `record_audit_event` back-compat. Add `record_audit_event_with_outcome`.
3. **Broker audit wrapper** — `fn audit<T>(db, op, params, result) -> Result<T>` used by every state-changing method.
4. **`navigation_digest` widening** — 32 hex chars, fold project_id into seed. Preserve the legacy 12-char format behind a compatibility constant so previously-stored rows continue to round-trip; new writes use the new format.
5. **`graph::KnowledgeGraph` identity key** — swap `to_lowercase()` keys for `blake3(canonical)` identity hashes with a side-mapped canonical name. Preserve public API.
6. **`CoreError` → `ClientError` mapping** — introduce `ClientError` in `the-one-mcp::api`, add `From<CoreError>`, update every `jsonrpc.rs` arm to map via that type + emit `tracing::error!` with a correlation ID.
7. **Pagination** — add `Cursor` utility to `the_one_core::storage`, update every `list_*` + `search_*` function to take `Cursor` and return `(items, Option<Cursor>, total_count)`. Update `broker.rs` + `api.rs` contracts. **Reject over-limit requests** instead of silently truncating.
8. **`memory_navigation_list` SQL-side filter** — push the tunnel filter into SQL. Add a new `list_navigation_tunnels_for_nodes(&[node_id])` helper.
9. **`memory_navigation_traverse` paginated BFS** — stream neighbors via paged queries; never fetch all 2 000 nodes.
10. **Cached `ProjectDatabase` per project** — new `db_by_project: RwLock<HashMap<String, Arc<Mutex<ProjectDatabase>>>>` in broker.
11. **Error swallow fixes** — every `let _ = x` becomes `if let Err(e) = x { tracing::warn!(...) }` with structured context. Fix the `auto_enabled` lie.
12. **`tests/stdio_write_path.rs` integration test** — spawn in-process server, drive JSON-RPC through pipes, assert writes land in SQLite.
13. **Hygiene tests** — tool description imperative-word scan, `navigation_digest` collision test, entity key collision test, mtime monotonicity test, sanitizer round-trip test, audit outcome round-trip test.
14. **Benchmarks** — `crates/the-one-core/benches/list_pagination.rs`, `crates/the-one-mcp/benches/audit_throughput.rs`, `crates/the-one-memory/benches/entity_identity.rs`.
15. **Documentation** — update `docs/guides/mempalace-operations.md`, add `docs/guides/production-hardening-v0.15.md`, write this report (already done), update CHANGELOG.

---

## Part 4 — Acceptance criteria

1. `cargo fmt --check` passes.
2. `cargo clippy --workspace --all-targets -- -D warnings` passes.
3. `cargo test --workspace` passes (all new tests green).
4. New benchmarks compile and produce sensible numbers.
5. `bash scripts/release-gate.sh` passes.
6. No `let _ = ` on non-trivial results in `broker.rs` (grep gate).
7. No `e.to_string()` passed into JSON-RPC error messages for `CoreError::Sqlite` / `Io` (grep gate).
8. `navigation_digest` emits ≥ 32 hex chars (unit test).
9. `list_*` endpoints reject `limit > max_limit` with `InvalidRequest` (unit test).
10. stdio write path integration test proves writes persist (new test).

---

## Appendix A — Inventory of touched files

Planned edits:

- `crates/the-one-core/src/lib.rs` — export new `naming`, `audit`, `pagination` modules.
- `crates/the-one-core/src/naming.rs` — new.
- `crates/the-one-core/src/audit.rs` — new helper wrapper around audit events.
- `crates/the-one-core/src/pagination.rs` — new cursor type.
- `crates/the-one-core/src/storage/sqlite.rs` — schema v7 migration, new `list_*_paginated`, `audit_events` gets outcome+error_kind columns, `list_navigation_tunnels_for_nodes`, no silent caps.
- `crates/the-one-core/src/error.rs` — keep as-is but add `CoreError::kind_label()`.
- `crates/the-one-memory/src/graph.rs` — new identity key scheme.
- `crates/the-one-mcp/src/api.rs` — new `Cursor`, `PageInfo`, `ClientError`.
- `crates/the-one-mcp/src/broker.rs` — all fixes.
- `crates/the-one-mcp/src/transport/jsonrpc.rs` — map errors via `ClientError`.
- `crates/the-one-mcp/src/transport/tools.rs` — hygiene test.
- `crates/the-one-mcp/tests/stdio_write_path.rs` — new.
- `crates/the-one-mcp/tests/production_hardening.rs` — new, cross-finding tests.
- `crates/the-one-core/benches/list_pagination.rs` — new.
- `crates/the-one-mcp/benches/audit_throughput.rs` — new.
- `docs/guides/production-hardening-v0.15.md` — new.
- `docs/guides/mempalace-operations.md` — update pagination + audit sections.
- `CHANGELOG.md` (or `docs/releases/`) — entry for the hardening pass.
