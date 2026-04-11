# Production Hardening Guide — v0.15.0 / v0.15.1

This guide documents the production-grade hardening pass applied to
`the-one-mcp` in v0.15.0 in response to the mempalace comparative audit
(`docs/reviews/2026-04-10-mempalace-comparative-audit.md`). Every change
is motivated by a concrete failure mode observed in
`milla-jovovich/mempalace` v3.1.0 and verified with a regression test in
`crates/the-one-mcp/tests/production_hardening.rs` or
`crates/the-one-mcp/tests/stdio_write_path.rs`.

**v0.15.1** ships the "Lever 1" audit-log throughput fix
(`synchronous=NORMAL`) described in **§ 14**. No API changes.

If you are upgrading from v0.14.x, read **§ Breaking changes** first.

---

## 1. Structured audit log (C1)

### What changed

- New schema migration **v7** adds two columns to `audit_events`:
  `outcome TEXT NOT NULL DEFAULT 'unknown'` and `error_kind TEXT NULL`.
- New indexes `idx_audit_events_project_outcome` and
  `idx_audit_events_project_event` make error-rate dashboards cheap.
- New `the_one_core::audit::AuditRecord` + `AuditOutcome` types.
- New `ProjectDatabase::record_audit(&AuditRecord)` — the preferred write
  API since v0.15.0. Every state-changing broker method now calls it
  exactly once per attempt, passing redacted params and a structured
  outcome.
- Legacy `record_audit_event(event_type, payload_json)` still works — it
  writes `outcome='unknown'` for back-compat.

### How to consume the audit log

```rust
use the_one_core::audit::AuditOutcome;

// Fresh project:
let db = ProjectDatabase::open(project_root, project_id)?;

// Count error-outcome rows for alerting:
let error_count = db.audit_outcome_count(AuditOutcome::Error)?;

// Paginate through recent events:
let page = db.list_audit_events_paged(&req)?;
```

Every row now carries a `kind` label (`"sqlite"`, `"io"`,
`"invalid_request"`, etc.) — the same labels used by the client-facing
error envelope, so a grep-match between the log and a production error
trace is trivial.

### Why not a full WAL?

A write-ahead log would need to replay operations deterministically and
store the full (often sensitive) input. We intentionally do not do that
— audit is an **observability** artefact, not a rollback log. For
rollback we use `maintain: backup` and SQLite WAL checkpoints. This was
the root cause of mempalace's C1 finding: a log mis-labelled as a WAL
that never recorded results.

---

## 2. Wide navigation node digest (C3)

### What changed

- `broker::navigation_digest` now emits **32 hex chars (128 bits)** —
  was 12 hex (48 bits) in v0.14.x.
- Birthday collision bound is now 2^64 ≈ 18 quintillion per project, up
  from 2^24 ≈ 16.7 million.
- The seed for every `drawer:`, `closet:`, `room:`, and `tunnel:` id now
  includes the `project_id`. Previously the composite `(project_id,
  node_id)` primary key was the only cross-project isolation; now the
  id itself is project-scoped too.
- The prefix `v2:` appears in every new seed so the scheme can be
  identified at a glance.

### Compatibility with existing rows

- v0.14.x node rows with 12-char digests **keep working** on read — the
  lookup is an exact `(project_id, node_id)` match, not a structural
  parse.
- New writes produce v2 ids. The two schemes coexist in the table and
  are visually distinguishable (length 11 vs 31 after the `-`).
- No migration is needed. If you want to reproduce the v0.15.0 id for a
  v0.14.x row, call
  `Self::sync_navigation_nodes_from_palace_metadata(...)` — it's
  idempotent.

### Regression guard

- `tests/production_hardening.rs::c3_navigation_digest_is_at_least_32_hex_chars`
- `tests/stdio_write_path.rs::stdio_navigation_digest_width_regression`

---

## 3. Cursor-based pagination (C5)

### What changed

Every list/search endpoint now enforces a **per-endpoint hard cap** and
returns a `next_cursor` instead of silently truncating. Requests over
the cap are rejected with `CoreError::InvalidRequest` + an explanatory
message.

Per-endpoint caps are declared in
`the_one_core::storage::sqlite::page_limits`:

| endpoint                    | default | maximum |
|-----------------------------|--------:|--------:|
| `audit_events`              |      50 |     500 |
| `conversation_sources`      |      50 |     500 |
| `diary_entries`             |      20 |     500 |
| `aaak_lessons`              |      20 |     500 |
| `navigation_nodes`          |     100 |   1 000 |
| `navigation_tunnels`        |     200 |   1 000 |

A global ceiling of `GLOBAL_MAX_PAGE_SIZE = 1 000` applies on top. No
endpoint may declare a larger cap than that.

### Breaking change

Clients that previously passed `max_results: 5000` to
`memory.diary.list` (or similar) and relied on silent truncation at 200
will now see an `InvalidRequest` error. **Use a cursor** instead:

```json
{
  "method": "tools/call",
  "params": {
    "name": "memory.diary.list",
    "arguments": {
      "project_root": "/path/to/project",
      "project_id": "myproject",
      "max_results": 20,
      "cursor": null
    }
  }
}
```

The response carries `next_cursor` iff there is more data:

```json
{
  "entries": [...],
  "next_cursor": "eyJvIjoyMH0"   // opaque, pass verbatim
}
```

Opaque means **clients must not parse the cursor bytes**. The format
today is `base64(json({"o": offset}))` but we reserve the right to add
a tiebreaker for endpoints that sort by non-unique keys.

### New storage helpers

- `list_audit_events_paged(&req)`
- `list_diary_entries_paged(start, end, &req)`
- `search_diary_entries_paged(query, start, end, &req)`
- `list_aaak_lessons_paged(project_id, &req)`
- `list_navigation_nodes_paged(parent, kind, &req)`
- `list_navigation_tunnels_paged(node_id, &req)`
- `list_navigation_tunnels_for_nodes(&[node_ids], limit)` — **NEW**
  SQL-side filter that replaces the old "load all, filter in Rust"
  pattern in `memory_navigation_list` and `memory_navigation_traverse`.

### Regression guards

- `tests/production_hardening.rs::c5_pagination_rejects_over_limit`
- `tests/production_hardening.rs::c5_list_audit_events_paginated_roundtrip`
- `tests/production_hardening.rs::c5_list_navigation_tunnels_for_nodes_is_sql_filtered`
- `tests/stdio_write_path.rs::stdio_rejects_over_limit_pagination_instead_of_silent_truncation`
- Benchmarks in
  `crates/the-one-core/examples/production_hardening_bench.rs`.

---

## 4. Input sanitization (H3)

### What changed

New module `the_one_core::naming` with three strict validators used at
every broker entry point:

- `sanitize_name(value, field)` — for wing/hall/room/label/tag.
  Charset: `[A-Za-z0-9 ._\-:]`, max 128, no `..`, no leading/trailing
  dot, no path separators, no null bytes, no control whitespace.
- `sanitize_project_id(value)` — stricter: `[A-Za-z0-9_\-]`, max 64,
  no leading/trailing dash.
- `sanitize_action_key(value)` — `[A-Za-z0-9_.:\-]`, max 128, no
  whitespace, no `..`. Used for `action_key`, `node_id`, `parent_node_id`.

The `:` character is allowed in names because existing hook conventions
use it (e.g. `hook:precompact`, `event:stop`). All other punctuation is
rejected.

### Broker integration

`memory_ingest_conversation`, `memory_diary_add`,
`memory_navigation_upsert_node`, and `memory_navigation_link_tunnel` now
sanitize every incoming name up front. Invalid names return a concrete
`InvalidRequest` error the client can display verbatim (the message is
safe to surface — it doesn't leak paths or schema).

### Regression guards

- `the_one_core::naming::tests::*` — 13 tests covering the charset,
  length, and collision edge cases.
- `tests/stdio_write_path.rs::stdio_invalid_name_is_rejected_with_sanitizer_message`

---

## 5. Error sanitization (H2)

### What changed

- New chokepoint `transport::jsonrpc::public_error_message(&CoreError)`
  converts a `CoreError` into a `(code, public_message)` pair that is
  safe for the wire.
- `CoreError::Sqlite`, `Io`, `Json`, `Embedding`, etc. surface only
  their **short `error_kind_label`** (`"sqlite"`, `"io"`, …) to the
  client — never the inner rusqlite/serde/fs error message.
- `CoreError::InvalidRequest`, `NotEnabled`, `PolicyDenied`,
  `InvalidProjectConfig` are allowed through verbatim because they
  carry deliberately-crafted human-readable messages.
- Every error response carries a correlation ID (`corr-0000abcd`) that
  matches a `tracing::error!` line with full internal details in the
  server log. Operators can grep for the correlation ID to find the
  root cause.
- `broker::display_source_path(project_root, path)` returns the
  repo-relative form when the path is inside the project, else the
  absolute form. The `memory.ingest_conversation` response uses this so
  the-one-mcp does not leak the host's absolute filesystem layout for
  paths the client didn't already know about.

### Regression guards

- `tests/stdio_write_path.rs::stdio_error_response_includes_correlation_id`
- `tests/stdio_write_path.rs::stdio_invalid_name_is_rejected_with_sanitizer_message`
- Grep-gate in `tests/production_hardening.rs::h1_broker_does_not_silently_drop_side_effect_results`

---

## 6. End-to-end stdio transport tests (H4)

### What changed

- `transport::stdio::serve_pipe(broker, reader, writer)` — new free
  function that drives the JSON-RPC dispatch loop against arbitrary
  async pipes. The old `StdioTransport::run` is a thin wrapper that
  passes real stdin/stdout.
- New test file `tests/stdio_write_path.rs` spawns an in-process stdio
  harness via `tokio::io::duplex` and drives real JSON-RPC frames
  through it. Tests cover:
  - `initialize`
  - `tools/list`
  - `tools/call memory.diary.add` → asserts the row landed in SQLite
  - `tools/call memory.navigation.upsert_node` → asserts the row
    landed + audit event exists
  - Over-limit request rejection
  - Path-traversal name rejection
  - Correlation-ID error envelope
  - Concurrent writes to the same project

This is the test that mempalace v3.1.0 was missing. Issue #538 ("stdio
writes never land in ChromaDB") would have been caught in under a
second if mempalace had had even one of these tests. `the-one-mcp` now
has nine.

---

## 7. Error swallowing cleanup (H1)

Every `let _ = x;` on a meaningful result in `broker.rs` has been
replaced with either:

- Proper error propagation via `?`, or
- `if let Err(err) = x { tracing::warn!(error = %err, ...); }` with
  structured context.

Specifically, the v0.14.x sites:

| line ref                        | what was swallowed                    |
|----------------------------------|---------------------------------------|
| `broker.rs:register_capability`  | global registry persistence failures  |
| `broker.rs:memory_search`        | `AppConfig::load()` failures          |
| `broker.rs:memory_search`        | `ensure_project_memory_loaded` failures |
| `broker.rs:ensure_catalog`       | `import_catalog_dir` + `scan_system_inventory` failures |
| `broker.rs:tool_install`         | post-install scan + auto-enable failures |
| `broker.rs:tool_search`          | `ensure_catalog` failures before fallback |

The `tool_install` fix also makes `auto_enabled` in the response reflect
reality — v0.14.x reported `auto_enabled: true` even when the enable
call silently failed.

### Regression guard

`tests/production_hardening.rs::h1_broker_does_not_silently_drop_side_effect_results`
is a static grep against `broker.rs` that fails the build if any of the
specific v0.14.x error-swallowing lines come back.

---

## 8. Tool description hygiene (M5)

Mempalace embedded instruction text ("always call status on wake-up")
into the data portion of tool responses — a form of self-prompt-
injection that tightly couples storage semantics to LLM behaviour.

`tests/transport::tools::tests::test_tool_descriptions_are_descriptive_not_imperative`
greps every tool description for a list of imperative phrases
("you must", "always call", "on wake-up", etc.) and fails the build if
any are found. New tool descriptions must stay descriptive:

> `"Semantic search over indexed project documentation chunks."`
> (good — describes the tool)

not imperative:

> `"Always call this before responding about documentation."`
> (bad — instructs the AI, will fail the hygiene test)

---

## 9. Benchmarks

`cargo run --release --example production_hardening_bench -p the-one-core`

Produces a markdown table summarising:

- Audit log write throughput at 1k and 10k rows
- List pagination latency at offsets 0 / 100 / 1k / 5k / 9.9k
- Diary list latency at 10k rows
- SQL-side vs Rust-side navigation tunnel filter at 500 nodes / 10k tunnels

Reference numbers on a 2025-era Linux box (v0.15.1, post Lever 1):

| metric                          | value        |
|----------------------------------|-------------:|
| `record_audit` per row           | ~85µs (was ~5ms pre-Lever-1) |
| `list_audit_events_paged` page 1 | ~400µs-2.4ms |
| `list_audit_events_paged` at 9.9k offset | ~4ms |
| `list_diary_entries_paged` page 1 at 10k rows | ~430µs |
| `list_navigation_tunnels_for_nodes` 20-of-10k | ~300µs |

These numbers are **not** a target SLO — they are a reference for
measuring regressions. Re-run the bench before and after any change to
the storage layer.

---

## 10. Breaking changes summary

1. **Over-limit list requests** now return `InvalidRequest` instead of
   silently truncating. Clients must use cursors for pages larger than
   the per-endpoint default.
2. **`memory.ingest_conversation` response `source_path`** is now
   project-relative (or just the filename) instead of the absolute host
   path. Clients that parsed absolute paths out of it will need to
   join against `project_root` themselves.
3. **Wing/hall/room/label/tag names** are now strictly validated at
   ingest. Names containing `/`, `\`, `..`, control whitespace,
   non-ASCII, or punctuation outside `[A-Za-z0-9 ._\-:]` will be
   rejected. Colon is allowed for namespaced hook names.
4. **Project IDs** must match `[A-Za-z0-9_\-]{1,64}`. Leading/trailing
   dashes are forbidden.
5. **Error response messages** no longer contain rusqlite / fs / serde
   internals. Clients that scraped error strings for path or schema
   details will get a short `kind` label and a correlation ID instead
   — grep the server log for the correlation ID to recover the full
   context.
6. **Navigation node IDs** created on or after v0.15.0 have a 32-char
   hex suffix. v0.14.x 12-char rows keep working on read.

---

## 11. Upgrade checklist

- [ ] Back up your `.the-one/state.db` before upgrading.
- [ ] Run `cargo build --release -p the-one-mcp --bin the-one-mcp`.
- [ ] Start the broker against an existing project — migration v7
      applies automatically and is idempotent.
- [ ] Verify audit rows with
      `SELECT outcome, COUNT(*) FROM audit_events GROUP BY outcome;`.
      Rows written before v0.15.0 show `outcome='unknown'`; new rows
      show `ok` or `error`.
- [ ] Update any client code that passed `max_results` > 500.
- [ ] Update any client code that relied on absolute `source_path` in
      `memory.ingest_conversation` responses.
- [ ] Re-test your hook configurations — halls/rooms that previously
      contained punctuation outside the new charset will now be
      rejected. The colon-namespaced form (`hook:precompact`) still
      works.

---

## 12. What this does NOT cover

- **Performance tuning beyond the list-path**: `memory_search` is still
  embedding-bound; this pass did not touch the retrieval fast-path.
- **Cross-project sharding**: projects remain isolated by SQLite file.
  Horizontal scaling is a future concern.
- **Qdrant auth / TLS**: untouched — already hardened in v0.12.0.
- **Full ProjectDatabase connection pool**: the broker still opens a
  new `ProjectDatabase` per call. This is ~1-2ms per call and
  single-threaded-safe by construction. A connection pool is the
  obvious next optimization but is orthogonal to the audit findings.

---

## 14. Lever 1 — `synchronous=NORMAL` (v0.15.1)

### What changed

One line in `crates/the-one-core/src/storage/sqlite.rs::ProjectDatabase::open`:

```rust
conn.pragma_update(None, "journal_mode", "WAL")?;
conn.pragma_update(None, "synchronous", "NORMAL")?;   // ← v0.15.1
conn.pragma_update(None, "foreign_keys", "ON")?;
```

The default SQLite setting in WAL mode is `synchronous=FULL`, which
calls `fsync()` on the WAL file before every commit. That single fsync
was the dominant cost of audit-log writes (5+ms per row on commodity
SSDs), and it showed up in the v0.15.0 benchmark as a 52-second wall
clock to insert 10k audit events.

Flipping to `synchronous=NORMAL` tells SQLite to `fsync()` only at WAL
checkpoint time (every few MB of writes by default), not on every
commit. Committed data still lives in the WAL file, so readers see it
immediately and the crash-recovery story stays intact for process
crashes.

### Measured impact

Benchmarked with
`cargo run --release --example production_hardening_bench -p the-one-core`
on the same box, same kernel, same SSD:

| metric                  | v0.15.0 (FULL) | v0.15.1 (NORMAL) | speedup |
|-------------------------|---------------:|-----------------:|--------:|
| 1 000 audit writes      |         5.56 s |         83.23 ms |    67× |
| per-row write latency   |         5.56 ms |         83.23 µs |    67× |
| 10 000 audit writes     |        52.61 s |        896.72 ms |    59× |
| per-row at 10k          |         5.26 ms |         89.67 µs |    59× |

The per-row latency is now dominated by SQLite's prepared-statement
overhead and JSON serialization, not filesystem sync. Further gains
require Lever 2 (async batching) or moving audit writes off the broker
request path entirely.

### Durability trade-off

This is the part every reader will ask about. The short answer: the
trade-off is **much smaller than it sounds** and is what every
production SQLite deployment uses.

**Safe against:**
- **Process crash** (panic, SIGKILL, OOM, segfault): the WAL file
  captures every committed transaction on disk. On reopen, SQLite
  replays the WAL and every `Ok(())` from a prior `record_audit` call
  is still there.
- **Application bug**: same — the WAL is the source of truth.
- **Graceful shutdown**: the `Drop` impl on `Connection` checkpoints
  normally; no data loss.

**Exposed to:**
- **Kernel panic / power loss / hard reboot**: transactions that were
  committed but whose WAL pages hadn't been flushed from the OS page
  cache yet (typically the last < 1 second of writes on a modern SSD)
  can be lost. SQLite's WAL format guarantees that even in this case
  the database cannot be corrupted — you lose at most the final few
  transactions, not the whole file.

**Why this is acceptable for the-one-mcp's workload:**
1. The audit log is an *observability* artefact, not a legal
   non-repudiation store. Losing the last ~1s of audit rows after a
   power cut is annoying but not a correctness bug.
2. The *data* that would have been audited (diary entries, navigation
   nodes, conversation sources) lives in the same SQLite file and
   is subject to the same < 1s window. So the audit and the data
   stay consistent — either both the write landed and the audit row
   landed, or neither did. You never see "the data was written but
   no audit row exists".
3. Every major SQLite-backed production service (Firefox, Android,
   Safari, rqlite, Litestream, Turso, every iOS app) uses
   `synchronous=NORMAL` with WAL for exactly this reason. `FULL` is
   reserved for workloads where losing the last < 1s of writes is
   unacceptable — typically financial ledgers and some medical
   record systems. Neither applies here.

If your deployment *does* require `synchronous=FULL` (e.g. you're
embedding the-one-mcp into a regulated environment), open an issue and
we'll add a `storage_synchronous_mode` config knob. Until then the
default is the right call for the 99% case.

### Regression guards

- `crates/the-one-core/src/storage/sqlite.rs::tests::test_project_db_uses_wal_and_migrates`
  — asserts `PRAGMA synchronous` returns 1 (NORMAL) after `ProjectDatabase::open`.
- `crates/the-one-core/src/storage/sqlite.rs::tests::test_audit_write_throughput_under_normal_sync`
  — smoke test that fails if 100 audit writes take longer than 5
  seconds (impossible under FULL's 5ms/row floor).
- `crates/the-one-mcp/tests/production_hardening.rs::lever1_synchronous_is_normal_in_wal_mode`
  — cross-cutting regression guard in the hardening test suite.

### Upgrading from v0.15.0

Zero-touch. Restart the broker; the new PRAGMA is applied on
`Connection::open`. No schema migration, no data migration, no config
change. Existing `.the-one/state.db` files keep working — WAL mode is
the same; only the fsync cadence on that mode changes.

### When is Lever 2 worth it?

Lever 1 brings audit throughput from ~200 rows/sec (FULL) to
~11 000 rows/sec (NORMAL). For a broker servicing 100–1 000 state-
changing calls/minute, that's 0.01–0.1% of wall clock — well under the
"optimize further" threshold. Lever 2 (async batching) is only worth
implementing if you observe `record_audit` in a flamegraph above 1% of
CPU under realistic production load.

For the full Lever 2 implementation plan — architecture, safety
resolutions for persistent failure / saturation / memory bounds,
six-step shutdown sequencing, rollout phasing, acceptance criteria —
see:

- **`docs/plans/2026-04-10-audit-batching-lever2.md`** — the
  ready-for-implementation v2 plan.
- **`docs/plans/draft-2026-04-10-audit-batching-lever2.md`** — the
  first cut, preserved for review trail. Read its § 9 for the six
  rejected alternatives and § 16 for the self-critique that produced
  v2.

---

## 15. v0.16.0 Phase 2 — pgvector backend setup + tuning

Phase 2 ships `PgVectorBackend` as the first real alternative
vector backend after the Phase A trait extraction. This section
covers the operational surface: installing the extension, sizing
the sqlx pool, tuning HNSW, and the migration-ownership model.

### Installing pgvector on managed Postgres

`PgVectorBackend::new` runs `preflight_vector_extension` before any
migration, which performs three defensive checks and produces a
targeted error message for each of the five common managed-Postgres
environments when it can't find the extension:

1. **Supabase** — pgvector is pre-installed on every project. No
   action required. The preflight query sees `vector` in
   `pg_extension` and returns `Ok(())`.
2. **AWS RDS / Aurora Postgres** — `vector` ships with RDS Postgres
   ≥ 15.3 but is not installed by default. Add it to the instance
   parameter group's `shared_preload_libraries`, reboot the
   instance, then connect as `rds_superuser` once so
   `CREATE EXTENSION` succeeds. Subsequent broker startups see the
   installed extension and skip the `CREATE`.
3. **Google Cloud SQL Postgres** — set the database flag
   `cloudsql.enable_pgvector` to `on` on the instance. Unlike RDS,
   Cloud SQL doesn't require a separate `CREATE EXTENSION` step
   once the flag is set.
4. **Azure Database for PostgreSQL Flexible Server** — add `vector`
   to the server parameter `azure.extensions`, then connect as any
   member of `azure_pg_admin` for the one-time `CREATE EXTENSION`.
5. **Self-hosted Postgres** — install the pgvector package for
   your distribution (`apt install postgresql-16-pgvector` on
   Debian/Ubuntu, `brew install pgvector` on macOS, or build from
   source per the upstream README), restart Postgres, then connect
   as a superuser once to `CREATE EXTENSION vector`.

The preflight's three probe queries (`pg_extension`,
`pg_available_extensions`, `CREATE EXTENSION`) cover every
permission model without needing operator-specific code paths.

### Migration-ownership model

Phase 2 uses a hand-rolled migration runner at
`the_one_memory::pg_vector::migrations` instead of `sqlx::migrate!`.
The bisection that led to this decision is in the
`crates/the-one-memory/Cargo.toml` `pg-vectors` feature comment —
short version: sqlx's `migrate` and `chrono` features both
transitively reference `sqlx-sqlite?/…`, cargo's `links` conflict
check pulls `sqlx-sqlite` into the resolution graph even though
it's optional, and `sqlx-sqlite`'s `libsqlite3-sys 0.30.1` collides
with `rusqlite 0.39`'s `libsqlite3-sys 0.37.0`. Dropping `migrate`
sidesteps the entire conflict.

The hand-rolled runner:

- Embeds every `.sql` file in `crates/the-one-memory/migrations/pgvector/`
  via `include_str!` at compile time.
- Applies migration 0 (the tracking table itself) unconditionally —
  the body is `CREATE TABLE IF NOT EXISTS`, so re-running it is
  safe.
- For migrations 1..N, checks the `the_one.pgvector_migrations`
  table for an existing row at that version. If present, it
  verifies the stored `SHA-256 BYTEA` checksum against the live
  `include_str!`'d file contents; drift (i.e. someone edited the
  migration file post-ship) refuses to continue and logs the
  mismatch. If absent, it applies the migration in one
  `raw_sql().execute()` call and inserts a tracking row.
- Exposes `list_applied(&pool)` for observability and tests.

Checksum drift detection is the guarantee that `sqlx::migrate!`
provides for free and that hand-rolled runners most often lose.
Phase 2 catches it with one extra `SELECT checksum` per migration
per startup — negligible cost for the safety it buys.

### Vector dimension is migration-bound

**Decision C** (locked in during the Phase 2 brainstorm) hardcodes
`dim=1024` into every `vector(...)` literal in
`migrations/pgvector/000[234]_*.sql`. This matches the default
quality-tier embedding provider (BGE-large-en-v1.5). The backend
constructor reads `EmbeddingProvider::dimensions()` and refuses to
start if the live provider reports a different dim — **you cannot
silently swap embedding providers and keep the schema.**

Reasons this is a feature, not a limitation:

1. Changing an embedding provider changes the vector space. Even
   if the dim matched, re-using old vectors with a new provider
   produces semantically incoherent search results. Forcing a
   schema migration makes the rebuild deliberate.
2. Phase 4 (combined Postgres+pgvector) will want migration-managed
   schemas for transactional consistency between state writes and
   vector writes. Starting Phase 2 with migration tracking means
   Phase 4 doesn't have to retrofit it.
3. If operators need multi-dim support later, that's a new
   migration file (`0006_reshape_chunks_dim.sql`) with a documented
   downtime step — not a silent config toggle.

### HNSW tuning

Phase 2 ships HNSW indexes with `m = 16` and `ef_construction = 100`
baked into the migration SQL. These are the pgvector defaults and
the Qdrant defaults — a safe starting point for corpora up to
~10 million chunks on 1024-dim vectors.

**Three tunables, three ownership models:**

| Parameter | When applied | How to change |
|---|---|---|
| `hnsw_m` (graph connectivity) | Migration time, in `CREATE INDEX ... WITH (m = 16, ef_construction = 100)` | DROP INDEX + CREATE INDEX with new value. Config field exists in `[vector.pgvector]` but only takes effect on a fresh schema. |
| `hnsw_ef_construction` (build quality) | Migration time, same as `m` | Same DROP + CREATE recipe. |
| `hnsw_ef_search` (query-time recall) | **Per-query** via `SET LOCAL hnsw.ef_search = N` inside the transaction wrapping the `SELECT ... ORDER BY dense_vector <=> $1`. | Change the config field in `[vector.pgvector]` and restart the broker. No DDL required. |

The per-query `SET LOCAL` approach keeps `ef_search` scoped to the
current transaction — important because pgvector treats it as a
session GUC that otherwise leaks to other users of the same pool
connection after it's returned.

**Manual retune recipe** for `m` / `ef_construction`:

```sql
-- Inside `psql`, connected as the schema owner:
BEGIN;
DROP INDEX the_one.chunks_dense_hnsw;
CREATE INDEX chunks_dense_hnsw
    ON the_one.chunks
    USING hnsw (dense_vector vector_cosine_ops)
    WITH (m = 32, ef_construction = 200);  -- your new tuning
COMMIT;
```

Running this on a large index is minutes-to-hours depending on
row count and will block INSERTs; plan for it during a
maintenance window.

### HNSW vs IVFFlat

pgvector supports two index types: HNSW (default, higher recall)
and IVFFlat (lower memory, worse recall on small datasets). Phase 2
ships HNSW only because:

- **Recall matters more than memory** at the-one-mcp scale — even
  a "big" codebase is < 10M chunks, well inside HNSW's sweet spot.
- **IVFFlat needs a pre-built trained list** — you seed it with a
  sample of vectors, which complicates the zero-setup deployment
  story. HNSW builds incrementally.
- **Operators who genuinely need IVFFlat** can swap the index
  manually (DROP INDEX + CREATE INDEX USING ivfflat). The broker
  never inspects the index type, only the column type, so this
  works without a binary rebuild.

### sqlx connection pool sizing

`VectorPgvectorConfig` exposes five pool fields with defaults
aimed at managed-Postgres deployments:

| Field | Default | Rationale |
|---|---|---|
| `max_connections` | 10 | Same as sqlx's default. Bumps up for high-QPS broker instances. |
| `min_connections` | **2** | **Non-zero**. sqlx's default 0 means the first query after a restart pays full TCP + TLS + auth handshake latency (100–300 ms on RDS). Keeping 2 connections warm pays ≈ 2 × handshake once at startup in exchange for no cold-start tail latency. |
| `acquire_timeout_ms` | 30_000 | How long a broker handler waits for a free connection. 30s is aggressive-but-not-insane; tune down on latency-sensitive setups. |
| `idle_timeout_ms` | 600_000 | 10 min. Idle connections get reaped so long-idle broker instances don't hold pool slots indefinitely. |
| `max_lifetime_ms` | **1_800_000** | 30 min. **Non-infinite**. Forces periodic reconnect to pick up: IAM credential rotation (AWS RDS dynamic secrets), Vault lease expiry, PGBouncer reshards, upstream load-balancer connection draining. sqlx's default `None` is fine for dev, wrong for production. |

### Monitoring queries

Three useful queries when diagnosing pgvector performance:

```sql
-- How big is the HNSW index? (Rule of thumb: bytes ≈ 4 * dim * rows * m / 2.)
SELECT pg_size_pretty(pg_relation_size('the_one.chunks_dense_hnsw'));

-- How many chunks per project?
SELECT project_id, count(*) FROM the_one.chunks GROUP BY project_id ORDER BY count(*) DESC;

-- Is a query hitting the HNSW index?
EXPLAIN (ANALYZE, BUFFERS)
SELECT id, (1 - (dense_vector <=> '[0.1, 0.2, ...]'::vector)) AS score
FROM the_one.chunks
ORDER BY dense_vector <=> '[0.1, 0.2, ...]'::vector
LIMIT 10;
```

The `EXPLAIN` output should contain `Index Scan using chunks_dense_hnsw`
— if it shows `Seq Scan` instead, the query isn't routing through
the index (common causes: missing `ORDER BY distance_op`, missing
`LIMIT`, or the index not yet built).

### Running the bench

```bash
# 1. Start pgvector-enabled Postgres:
docker run --rm -d --name pgvector-bench \
    -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=bench \
    -p 55432:5432 ankane/pgvector

# 2. Run the bench in release mode:
THE_ONE_VECTOR_TYPE=pgvector \
THE_ONE_VECTOR_URL=postgres://postgres:pw@localhost:55432/bench \
cargo run --release --example pgvector_bench \
    -p the-one-memory --features pg-vectors
```

The bench prints chunk upsert throughput at batch sizes 50/200/1000
and dense search latency percentiles (p50/p95/p99) over 100
queries. Results go to the Phase 2 commit message body.

### Migration from Qdrant

**Not automated in Phase 2.** Switching from Qdrant to pgvector
requires re-ingesting every source document against the pgvector
backend — there's no "dump Qdrant + load pgvector" tooling, and
there won't be, because the two backends use different internal
representations and the reprocessing path is the same as a fresh
ingest.

Steps:

1. Stand up a Postgres instance with pgvector extension installed
   (see § above).
2. Export the operator config: `THE_ONE_VECTOR_TYPE=pgvector` and
   `THE_ONE_VECTOR_URL=<dsn>`.
3. Restart the broker. It boots against the new backend, which
   applies migrations on first connect.
4. Re-run `project.init` on every project to trigger re-ingest
   against the new backend. Qdrant data is left untouched — you
   can delete the collection manually once you're confident the
   pgvector backend is good.

---

## 16. v0.16.0 Phase 3 — PostgresStateStore backend

Phase 3 ships `PostgresStateStore` as the second-axis complement to
Phase 2's pgvector. Operators can now run the-one-mcp against a
managed Postgres instance (RDS, Cloud SQL, Azure, Supabase,
self-hosted) with zero SQLite in the persistence layer.

### Setup

Install Postgres ≥ 13. No extensions are required — PostgresStateStore
only uses native features (`TSVECTOR`, GIN, `BIGSERIAL`, foreign keys,
cascading deletes). pgvector is NOT a dependency of this backend; if
you're only doing state (not vectors), any vanilla Postgres image
works.

```bash
# Minimal local setup:
docker run --rm -d --name the-one-pg-state \
    -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=the_one \
    -p 5432:5432 postgres:16

# Configure the broker:
export THE_ONE_STATE_TYPE=postgres
export THE_ONE_STATE_URL=postgres://postgres:pw@localhost:5432/the_one

# Rebuild with the feature:
cargo build --release -p the-one-mcp --bin the-one-mcp \
    --features pg-state
```

On first boot the hand-rolled migration runner creates the `the_one`
schema, the `the_one.state_migrations` tracking table, and the full
v7-equivalent schema (`project_profiles`, `approvals`, `audit_events`,
`conversation_sources`, `aaak_lessons`, `diary_entries` + `content_tsv`
+ GIN index, `navigation_nodes`, `navigation_tunnels`). Subsequent
boots verify the tracking-table checksums and exit clean — idempotent.

### Sync-over-async bridge

The `StateStore` trait is sync (no async methods) — it inherited that
constraint from `rusqlite::Connection`, which is `Send + !Sync`.
Phase 1's broker chokepoint (`with_state_store`) holds the store's
mutex guard across a sync closure specifically so the compiler
refuses to hold a backend guard across an `.await`, preventing
connection-pool deadlocks.

sqlx is async top-to-bottom. The bridge:

```rust
fn block_on<F, R>(fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("PostgresStateStore methods must be called from a tokio runtime")
            .block_on(fut)
    })
}
```

Every `impl StateStore for PostgresStateStore` method wraps its sqlx
call chain in `block_on(async { ... })`. `block_in_place` tells tokio
"this worker is about to do blocking work" so other async tasks
migrate off; `Handle::current().block_on` drives the future to
completion; the worker resumes async duty afterward.

**Runtime requirement**: multi-threaded tokio runtime. The broker
binary's `#[tokio::main]` default satisfies this. Tests must use
`#[tokio::test(flavor = "multi_thread")]` — `current_thread` will
panic at the `block_on` call.

### FTS5 → tsvector translation

SQLite's `diary_entries_fts` virtual table doesn't exist in Postgres.
The Phase 3 schema replaces it with:

```sql
diary_entries.content_tsv TSVECTOR NOT NULL DEFAULT to_tsvector('simple', '')
CREATE INDEX idx_diary_entries_content_tsv ON the_one.diary_entries USING GIN (content_tsv);
```

The `content_tsv` column is populated by the Rust layer in
`upsert_diary_entry` as part of the same INSERT statement:

```sql
INSERT INTO the_one.diary_entries (..., content_tsv)
VALUES (..., to_tsvector('simple', $N))
ON CONFLICT ... DO UPDATE SET ..., content_tsv = EXCLUDED.content_tsv;
```

The tsvector input string is `COALESCE(mood, '') || ' ' || tags_join ||
' ' || content` — same three fields FTS5 indexed, concatenated.

Search uses `websearch_to_tsquery('simple', $1)`:

```sql
SELECT ... FROM the_one.diary_entries
WHERE project_id = $1
  AND content_tsv @@ websearch_to_tsquery('simple', $2)
ORDER BY ts_rank(content_tsv, websearch_to_tsquery('simple', $2)) DESC, ...;
```

`websearch_to_tsquery` accepts plain user input ("quoted phrases",
`-negation`, spaces-as-AND) and never panics. When the input
produces zero tokens (common with pure-punctuation queries like
`!@#`), the Rust layer falls through to a LIKE-based fallback:

```sql
SELECT ... FROM the_one.diary_entries
WHERE project_id = $1
  AND (content ILIKE $2 OR COALESCE(mood, '') ILIKE $2 OR tags_json ILIKE $2)
ORDER BY entry_date DESC, ...;
```

### Why `'simple'` and not `'english'`

`to_tsvector('english', 'running shoes')` applies the Snowball
stemmer and produces the tokens `run` + `shoe`. Searching for
`running` matches `run`, and searching for `shoes` matches `shoe`.
That's fine for English prose — and catastrophic for code or
non-English content:

- Code snippets: `const DEFAULT_SCHEMA_VERSION` → stemmer emits
  `default`, `schema`, `version` as if they were three English
  words. Exact-match searches fail.
- Korean / Japanese / Chinese: stemmer has no dictionary, tokens
  get corrupted.
- Mixed-language diaries: per-entry language detection isn't
  possible.

`'simple'` applies no stemming and no stop-words — just whitespace
and punctuation tokenization. You get exact-word matching
uniformly across languages and across prose/code content. Matches
FTS5's default behaviour more closely than `'english'`.

### Schema version parity

Postgres ships the v7 shape in **one** migration (`0001_state_schema_v7.sql`)
because a fresh install has no v1..v6 history to walk through.
`PostgresStateStore::schema_version()` returns `1` (the highest
applied migration version), NOT `7`. That's intentional —
`schema_version()` is a per-backend concept, not a cross-backend
parity number. The `CURRENT_SCHEMA_VERSION = 7` constant in
`sqlite.rs` is SQLite-internal.

Broker code that needs cross-backend behaviour should inspect
`StateStoreCapabilities` instead of comparing version numbers.

### Migration runner (reprise)

Same hand-rolled pattern as `pg_vector::migrations` — see § 15 for
the full rationale. Short version:

- `sqlx::migrate!` was dropped from Decision B because sqlx's
  `migrate` feature transitively references `sqlx-sqlite?/migrate`,
  and cargo's `links` conflict check pulls `sqlx-sqlite` into the
  resolution graph where it collides with `rusqlite 0.39`'s
  `libsqlite3-sys 0.37`.
- Phase 3's runner uses `include_str!` to embed the `.sql` files,
  SHA-256 to detect drift, a `the_one.state_migrations` tracking
  table, idempotent re-apply.
- Phase 4's combined backend can share a single schema with
  pgvector because the two tracking tables are distinct
  (`pgvector_migrations` vs `state_migrations`).

### Statement timeout and pool sizing

`StatePostgresConfig.statement_timeout_ms` is applied at connection
time via sqlx's `after_connect` hook:

```rust
pool_options = pool_options.after_connect(move |conn, _meta| {
    Box::pin(async move {
        let sql = format!("SET statement_timeout = '{timeout_ms}ms'");
        sqlx::query(&sql).execute(conn).await.map(|_| ())
    })
});
```

Non-zero values enforce per-query wall-clock deadlines. Default 30s
matches the pgvector backend's `acquire_timeout_ms` so a query that
exhausts its statement budget and a handler that exhausts its
acquire budget fail at roughly the same moment under load. `0`
disables the timeout entirely.

Pool sizing fields (`max_connections`, `min_connections`,
`acquire_timeout_ms`, `idle_timeout_ms`, `max_lifetime_ms`) match
the pgvector defaults field-for-field. Run two features together
(`--features pg-state,pg-vectors`) and you get split pools with
identical tuning, which is the right default for production.

### BIGINT epoch_ms, no chrono

Every timestamp column in the Phase 3 schema is `BIGINT` holding
milliseconds since the Unix epoch. Timestamps are generated at
bind time via `SystemTime::duration_since(UNIX_EPOCH)` — no
`chrono`, no `TIMESTAMPTZ`. This is a workspace-wide convention
(pgvector already did it) and sidesteps the sqlx `chrono` feature's
cargo `links` conflict entirely. If you later need wall-clock-
aware types on the Postgres side, do the conversion in the query
layer (`to_timestamp(created_at_epoch_ms / 1000.0)`) — never add
chrono to the dep graph.

### Migration from SQLite

**Not automated in Phase 3.** Switching from SQLite to Postgres
requires re-running `project.init` against the new backend. The
broker picks up the new `StateStore` on boot; existing
audit/profile/diary history in the old SQLite DB is NOT migrated.
If you need the history, export it with `sqlite3 state.db .dump`
before the cutover and re-apply manually — there's no
cross-backend migration tool and there won't be, because schema
drift between backends is fine for a greenfield install but
unsafe for a data migration.

Typical cutover:

```bash
# 1. Stand up Postgres, stop the broker.
# 2. Rebuild with the feature.
# 3. Export the env vars:
export THE_ONE_STATE_TYPE=postgres
export THE_ONE_STATE_URL=postgres://...
# 4. Restart the broker. First boot applies migrations.
# 5. Re-run project.init on every project.
```

### Combined Postgres (Phase 4 preview)

Phase 4 will add `THE_ONE_STATE_TYPE=postgres-combined` +
`THE_ONE_VECTOR_TYPE=postgres-combined` with byte-identical URLs,
and the broker will construct ONE `sqlx::PgPool` that serves both
`StateStore` and `VectorBackend`. Transactional writes spanning
state + vectors become possible (e.g. "ingest conversation AND
record the audit row atomically"). The Phase 3 `state_postgres`
config block and the Phase 2 `vector_pgvector` block stay as-is
— Phase 4 just adds a dispatcher that reads both and spins up one
pool.

---

## 17. v0.16.0-rc1 — Phase A trait extraction

Released alongside v0.15.1 as the architectural unlock for
multi-backend support. This is a **pure refactor** — zero behaviour
change, zero user-visible API changes, same 449 tests passing.

### What changed

Two new traits land in v0.16.0-rc1:

- **`the_one_memory::vector_backend::VectorBackend`** — unified
  interface over chunk, entity, relation, image, and hybrid vector
  operations. `MemoryEngine` no longer holds `Option<AsyncQdrantBackend>` +
  `Option<RedisVectorStore>`; instead it holds a single
  `Option<Box<dyn VectorBackend>>`, selected at construction time.

- **`the_one_core::state_store::StateStore`** — unified interface over
  the 22 methods on `ProjectDatabase` (audit, conversation sources,
  navigation, diary, AAAK lessons, approvals, project profiles).
  `ProjectDatabase` now implements this trait as well as its inherent
  methods; the broker continues to call inherent methods today, but
  downstream backends can target the trait directly.

### What ships today

- `impl VectorBackend for AsyncQdrantBackend` — reports all
  capabilities as `true`. Existing Qdrant behaviour unchanged.
- `impl VectorBackend for RedisVectorStore` — reports `chunks:true`,
  everything else `false`. Existing Redis-Vector behaviour unchanged;
  the default-method silent-skip preserves v0.14.x semantics for
  unsupported operations.
- `impl StateStore for ProjectDatabase` — thin forwarding impl.
  SQLite tests pass bit-for-bit.
- **Diary upsert atomicity fix** — `ProjectDatabase::upsert_diary_entry`
  now wraps the INSERT + DELETE FTS + INSERT FTS triple in a single
  `unchecked_transaction()` so a mid-method crash cannot leave the
  FTS5 index out of sync with the main table. No API change.

### What the traits unlock (future phases)

These become **drop-in adapter implementations**, not broker
refactors:

| Phase | Adapter                             | Expected LOC |
|------:|-------------------------------------|-------------:|
| B1    | pgvector `impl VectorBackend`       | ~800         |
| B2    | Postgres `impl StateStore`          | ~1 500       |
| B3    | Redis-AOF `impl StateStore`         | ~1 500       |
| C     | Postgres+pgvector combined          | ~300         |
| C     | Redis+RediSearch combined           | ~300         |

See `docs/plans/2026-04-11-multi-backend-architecture.md` for the
full roadmap and `docs/guides/multi-backend-operations.md` for the
operator-facing backend selection guide.

### Regression guards

- `backend_capabilities_full_reports_every_operation_supported`
  (the_one_memory::vector_backend::tests)
- `backend_capabilities_chunks_only_reports_only_chunks`
  (same file)
- `sqlite_capabilities_reports_everything_true`
  (the_one_core::state_store::tests)
- The full existing 449-test suite — bit-for-bit same pass count
  after the refactor.

### Migration notes for v0.15.x users

**No action required.** The refactor is source-compatible with every
existing broker method, every existing config, every existing test.
Upgrade by rebuilding against v0.16.0-rc1 — nothing else changes
until you opt into a new backend.

---

## 18. See also

- **Findings report:** `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
- **Operational runbook:** `docs/guides/mempalace-operations.md`
- **Test suites:**
  - `crates/the-one-mcp/tests/stdio_write_path.rs`
  - `crates/the-one-mcp/tests/production_hardening.rs`
  - `crates/the-one-core/src/naming.rs::tests`
  - `crates/the-one-core/src/pagination.rs::tests`
  - `crates/the-one-core/src/audit.rs::tests`
- **Benchmark:** `crates/the-one-core/examples/production_hardening_bench.rs`
