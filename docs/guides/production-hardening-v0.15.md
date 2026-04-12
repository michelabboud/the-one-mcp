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

## 15. v0.16.0 Phase 2 — pgvector backend (moved to standalone guide)

Phase 2 ships `PgVectorBackend` as the first real alternative vector
backend after the Phase A trait extraction — operators running managed
Postgres can co-locate their vectors with their relational data instead
of standing up a separate Qdrant service.

**The full operational content has moved to its own guide:**
**[docs/guides/pgvector-backend.md](pgvector-backend.md)**. It covers:

- Installing the `vector` extension on Supabase, AWS RDS / Aurora,
  Google Cloud SQL, Azure Flexible Server, and self-hosted Postgres
- The defensive `preflight_vector_extension` 3-query probe
- Hand-rolled migration runner rationale (cargo `links` conflict vs
  `sqlx::migrate!`) and SHA-256 checksum drift detection
- Decision C: `dim=1024` hardcoded in the migration SQL
- HNSW tuning (`m`, `ef_construction`, `ef_search`) with three
  ownership models
- HNSW vs IVFFlat trade-offs
- sqlx connection pool sizing (`min_connections=2`, `max_lifetime=30min`)
- Monitoring queries (`EXPLAIN ANALYZE`, index sizing)
- Running the throughput bench
- Migration from Qdrant
- Decision D (hybrid search) deferral

**Configuration reference for the `vector_pgvector` config block
lives in [configuration.md](configuration.md#vector-backend--pgvector-v0160-phase-2).**

Commit `91ff224`, tag `v0.16.0-phase2`. Cargo feature `pg-vectors`
(off by default).

---

## 16. v0.16.0 Phase 3 — PostgresStateStore backend (moved to standalone guide)

Phase 3 ships `PostgresStateStore` as the second-axis complement to
Phase 2's pgvector — operators can now run the-one-mcp against managed
Postgres with zero SQLite in the persistence layer.

**The full operational content has moved to its own guide:**
**[docs/guides/postgres-state-backend.md](postgres-state-backend.md)**.
It covers:

- Setup (any vanilla Postgres ≥ 13 — no extensions needed)
- The sync-over-async bridge via `tokio::task::block_in_place` +
  `Handle::current().block_on` (and why the `StateStore` trait stays
  sync)
- FTS5 → tsvector translation with `content_tsv TSVECTOR` + GIN
- Why `websearch_to_tsquery('simple', ...)` not `'english'`
- Schema v7 parity in one migration (fresh Postgres installs have no
  v1..v6 history)
- Hand-rolled migration runner reusing the Phase 2 pattern
- Statement timeout via sqlx's `after_connect` hook and pool sizing
- BIGINT epoch_ms convention (no chrono, permanent)
- Migration path from SQLite (re-ingest, not dump+load)
- The 11 integration tests in `tests/postgres_state_roundtrip.rs`
- Combined Phase 4 cross-reference to the standalone guide

**Configuration reference for the `state_postgres` config block
lives in [configuration.md](configuration.md#state-store--postgres-v0160-phase-3).**

Commit `f010ed6`, tag `v0.16.0-phase3`. Cargo feature `pg-state`
(off by default, composable with `pg-vectors`).

---

## 16b. v0.16.0 Phase 4 — combined Postgres+pgvector backend (moved to standalone guide)

Phase 4 is the first *combined single-pool* backend on the
multi-backend roadmap. One `sqlx::PgPool` serves BOTH the
`StateStore` trait role (everything Phase 3 shipped) AND the
`VectorBackend` trait role (everything Phase 2 shipped) against a
single Postgres database. The operational benefit is one credential
to rotate, one pgbouncer entry, one PITR backup window, and one set
of IAM grants — without introducing a new named backend type or new
trait methods.

**The full operational content has moved to its own guide:**
**[docs/guides/combined-postgres-backend.md](combined-postgres-backend.md)**.
It covers:

- When to pick combined over split (decision matrix by priority)
- What "combined" actually means (dispatcher + shared pool, no new
  type, no new trait methods)
- Activation via `THE_ONE_{STATE,VECTOR}_TYPE=postgres-combined` with
  byte-identical URLs
- The "refined Option Y" architecture: `PgVectorBackend::from_pool`
  and `PostgresStateStore::from_pool` sync wrapper constructors +
  `McpBroker::combined_pg_pool_by_project` shared-pool cache +
  `postgres_combined::build_shared_pool` (the sole cold-path entry)
- The "state config wins" pool-sizing rule and its rationale
- `statement_timeout` inheritance on vector queries (split-pool
  pgvector had no equivalent hook)
- Topology diagrams, verification queries, migration paths from
  split-pool (same-DB = zero-data-copy, different-DB = manual
  `pg_dump`/`pg_restore`)
- What Phase 4 deliberately does NOT ship: no `begin_combined_tx()`
  trait method (no call site needed it), no named
  `PostgresCombinedBackend` type, no automated split → combined
  migration tool

**Configuration**: combined deployments reuse the existing
`state_postgres` and `vector_pgvector` config sections — there is
NO `[combined_postgres]` section. See
[configuration.md § Multi-Backend Selection](configuration.md#multi-backend-selection-v0160)
for the inline note.

Commit `<pending>`, tag `v0.16.0-phase4`. Cargo features
`pg-state,pg-vectors` (the combined dispatcher activates whenever
both features are on and both env vars select `postgres-combined`
— no new feature flag).

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

## 18. Redis StateStore (Phase 5 — v0.16.0-phase5)

Phase 5 ships a full `RedisStateStore` implementing all 26
`StateStore` trait methods. Two modes: cache (`require_aof=false`)
and persistent (`require_aof=true`). New `CoreError::Redis(String)`
variant with `"redis"` label in `error_kind_label`. See
[multi-backend-operations.md](multi-backend-operations.md) for the
complete backend matrix and config examples.

## 19. Combined Redis+RediSearch (Phase 6 — v0.16.0-phase6)

Phase 6 ships the combined Redis+RediSearch backend following the
same refined Option Y pattern as Phase 4's Postgres combined. One
`fred::Client` shared between `RedisStateStore` and
`RedisVectorStore`. See
[multi-backend-operations.md](multi-backend-operations.md) for
activation via `redis-combined`.

## 20. Redis-Vector entity/relation parity (Phase 7 — v0.16.0 GA)

Phase 7 closes the capability gap on `RedisVectorStore`: entities
and relations are now supported (was chunks-only). Each type gets
its own RediSearch index. Images remain unsupported on Redis
(tracked for v0.16.1). Decision D (pgvector hybrid search) is
deferred to post-GA.

---

## 21. See also

- **Findings report:** `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
- **Operational runbook:** `docs/guides/mempalace-operations.md`
- **Test suites:**
  - `crates/the-one-mcp/tests/stdio_write_path.rs`
  - `crates/the-one-mcp/tests/production_hardening.rs`
  - `crates/the-one-core/src/naming.rs::tests`
  - `crates/the-one-core/src/pagination.rs::tests`
  - `crates/the-one-core/src/audit.rs::tests`
- **Benchmark:** `crates/the-one-core/examples/production_hardening_bench.rs`
