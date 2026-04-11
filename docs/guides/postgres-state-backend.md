# Postgres State Backend Guide

> **Status:** First-class — shipped in **v0.16.0-phase3** (`f010ed6`,
> tag `v0.16.0-phase3`).
>
> **Cargo feature:** `pg-state` (off by default — operators opt in at
> build time).
>
> **Env-var activation:** `THE_ONE_STATE_TYPE=postgres` +
> `THE_ONE_STATE_URL=<dsn>`.

`PostgresStateStore` is the second-axis complement to Phase 2's
pgvector. Operators running managed Postgres can now run the-one-mcp
with **zero SQLite on the state axis** — project profiles, approvals,
audit events, conversation sources, AAAK lessons, diary entries, and
navigation nodes/tunnels all live in Postgres.

This guide covers installation, the sync-over-async bridge, FTS
translation, schema parity, and the migration path. For the full
backend-selection scheme across axes, see the
[multi-backend operations guide](multi-backend-operations.md). For
the config fields and env-var validation rules, see
[configuration.md](configuration.md#multi-backend-selection-v0160).
For the sibling pgvector backend, see
[pgvector-backend.md](pgvector-backend.md).

---

## 1. Setup

Install Postgres ≥ 13. **No extensions are required** — `PostgresStateStore`
only uses native features (`TSVECTOR`, GIN, `BIGSERIAL`, foreign keys,
cascading deletes). pgvector is NOT a dependency of this backend; if
you're only doing state (not vectors), any vanilla Postgres image
works. If you plan to run pgvector too, use `ankane/pgvector` which
includes both.

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

Tune via `<project>/.the-one/config.json` (every field has a
production-sane default, so the block is optional):

```json
{
  "state_postgres": {
    "schema": "the_one",
    "statement_timeout_ms": 30000,
    "max_connections": 10,
    "min_connections": 2,
    "acquire_timeout_ms": 30000,
    "idle_timeout_ms": 600000,
    "max_lifetime_ms": 1800000
  }
}
```

On first boot the hand-rolled migration runner creates the `the_one`
schema, the `the_one.state_migrations` tracking table, and the full
v7-equivalent schema (`project_profiles`, `approvals`, `audit_events`,
`conversation_sources`, `aaak_lessons`, `diary_entries` + `content_tsv`
+ GIN index, `navigation_nodes`, `navigation_tunnels`). Subsequent
boots verify the tracking-table checksums and exit clean —
idempotent.

### Verify in Postgres

After first boot:

```sql
\dn the_one                              -- schema exists
\dt the_one.*                            -- 8 tables + state_migrations
\di the_one.*                            -- includes diary_entries_content_tsv GIN
SELECT * FROM the_one.state_migrations ORDER BY version;
-- → 2 rows, versions 0..1, one SHA-256 checksum each
```

---

## 2. Sync-over-async bridge

The `StateStore` trait is **sync** by design (no async methods) —
it inherited that constraint from `rusqlite::Connection`, which is
`Send + !Sync`. Phase 1's broker chokepoint (`with_state_store`)
holds the store's mutex guard across a **sync closure**
specifically so the compiler refuses to hold a backend guard
across an `.await` — which prevents the classic connection-pool
deadlock pattern on any pooled backend.

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
migrate off the worker; `Handle::current().block_on` drives the
future to completion; the worker resumes async duty afterward.

**Runtime requirement**: multi-threaded tokio runtime. The broker
binary's `#[tokio::main]` default satisfies this. Tests must use
`#[tokio::test(flavor = "multi_thread")]` — `current_thread` will
panic at the `block_on` call with
`"can call blocking only when running on the multi-threaded runtime"`.

### Why not change the trait to async?

Making `StateStore` async would force `tokio::sync::Mutex` on the
broker's state-store cache (since `std::sync::Mutex` guards are
`!Send` and can't cross `.await`). But `tokio::sync::Mutex` guards
CAN cross `.await`, which reintroduces the connection-pool deadlock
pattern for SQLite (where `rusqlite::Connection` is `!Sync` and the
mutex must serialize access). The async trait would need per-backend
guard semantics, which is more complex than the sync-trait +
block_in_place pattern.

The sync trait + block_in_place pattern is the pragmatic middle
ground: SQLite stays fast (no unnecessary async wrapping), Postgres
gets full sqlx async semantics (via the bridge), and the compiler
enforces the anti-deadlock invariant on both.

---

## 3. FTS5 → tsvector translation

SQLite's `diary_entries_fts` virtual table doesn't exist in Postgres.
The Phase 3 schema replaces it with a `content_tsv TSVECTOR` column
on the `diary_entries` table + a GIN index:

```sql
diary_entries.content_tsv TSVECTOR NOT NULL DEFAULT to_tsvector('simple', '')
CREATE INDEX idx_diary_entries_content_tsv ON the_one.diary_entries USING GIN (content_tsv);
```

### Population

The `content_tsv` column is populated by the Rust layer in
`upsert_diary_entry` as part of the same INSERT statement:

```sql
INSERT INTO the_one.diary_entries (..., content_tsv)
VALUES (..., to_tsvector('simple', $N))
ON CONFLICT ... DO UPDATE SET ..., content_tsv = EXCLUDED.content_tsv;
```

The tsvector input string is `COALESCE(mood, '') || ' ' || tags_join
|| ' ' || content` — same three fields FTS5 indexed, concatenated.
The whole upsert is wrapped in a sqlx transaction so the main row +
derived `content_tsv` commit atomically. This is the Phase 3
equivalent of the Phase 0 SQLite atomicity fix.

### Search

```sql
SELECT ... FROM the_one.diary_entries
WHERE project_id = $1
  AND content_tsv @@ websearch_to_tsquery('simple', $2)
ORDER BY ts_rank(content_tsv, websearch_to_tsquery('simple', $2)) DESC, ...;
```

`websearch_to_tsquery` accepts **plain user input**:

- `"quoted phrases"` → exact phrase match
- `-negation` → exclude term
- `term1 term2` → AND
- `term1 OR term2` → OR

It **never panics** on malformed input. When the input produces zero
tokens (common with pure-punctuation queries like `!@#`), the Rust
layer falls through to a LIKE-based fallback:

```sql
SELECT ... FROM the_one.diary_entries
WHERE project_id = $1
  AND (content ILIKE $2 OR COALESCE(mood, '') ILIKE $2 OR tags_json ILIKE $2)
ORDER BY entry_date DESC, ...;
```

---

## 4. Why `'simple'` and not `'english'`

`to_tsvector('english', 'running shoes')` applies the Snowball stemmer
and produces the tokens `run` + `shoe`. Searching for `running` matches
`run`, and searching for `shoes` matches `shoe`. That's fine for
English prose — and **catastrophic** for code or non-English content:

- **Code snippets**: `const DEFAULT_SCHEMA_VERSION` → stemmer emits
  `default`, `schema`, `version` as if they were three English words.
  Exact-match searches on `DEFAULT_SCHEMA_VERSION` fail.
- **Korean / Japanese / Chinese**: stemmer has no dictionary, tokens
  get corrupted.
- **Mixed-language diaries**: per-entry language detection isn't
  possible; choosing one stemmer poisons the others.

`'simple'` applies **no stemming and no stop-words** — just
whitespace and punctuation tokenization. You get exact-word matching
uniformly across languages and across prose/code content. It matches
FTS5's default behaviour more closely than `'english'`.

If you specifically need English stemming for prose content, open an
issue — we could add a per-project `diary_fts_config` field that lets
operators opt into `'english'` (or another language config) at project
init time. No deployments have asked for it yet.

---

## 5. Schema version parity

Postgres ships the v7 shape in **one** migration (`0001_state_schema_v7.sql`)
because a fresh Postgres install has no v1..v6 history to walk
through. `PostgresStateStore::schema_version()` returns **`1`** (the
highest applied migration version), NOT `7`. That's intentional —
`schema_version()` is a per-backend concept, not a cross-backend
parity number.

| Backend | `schema_version()` on v0.16.0-phase3 |
|---|---|
| SQLite (`ProjectDatabase`) | `7` — reflects the 7-step incremental history |
| Postgres (`PostgresStateStore`) | `1` — reflects the Phase 3 migration tracking |

Broker code that needs cross-backend behaviour should inspect
`StateStoreCapabilities` instead of comparing version numbers:

```rust
let caps = store.capabilities();
if caps.fts {
    // both sqlite and postgres set this true
    store.search_diary_entries_in_range(query, None, None, 20)?
}
```

The `CURRENT_SCHEMA_VERSION = 7` constant in `sqlite.rs` is
SQLite-internal.

---

## 6. Migration runner (reprise)

Same hand-rolled pattern as `pg_vector::migrations` — see the
[pgvector backend guide](pgvector-backend.md#3-migration-ownership-model)
for the full rationale. Short version:

- `sqlx::migrate!` was dropped from Decision B because sqlx's
  `migrate` feature transitively references `sqlx-sqlite?/migrate`,
  and cargo's `links` conflict check pulls `sqlx-sqlite` into the
  resolution graph where it collides with `rusqlite 0.39`'s
  `libsqlite3-sys 0.37`.
- Phase 3's runner uses `include_str!` to embed `.sql` files,
  SHA-256 checksums to detect drift, a `the_one.state_migrations`
  tracking table, and idempotent re-apply.
- Phase 4's combined backend can share a single schema with
  pgvector because the two tracking tables are **distinct**
  (`pgvector_migrations` vs `state_migrations`).

### Files shipped

```
crates/the-one-core/migrations/postgres-state/
├── 0000_state_migrations_table.sql  -- hand-rolled tracking table
└── 0001_state_schema_v7.sql         -- full v7 schema in one pass
```

---

## 7. Statement timeout and pool sizing

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

Non-zero values enforce per-query wall-clock deadlines — important
on managed Postgres where a runaway query can monopolize a
connection slot. **Default 30s** matches the pgvector backend's
`acquire_timeout_ms` so a query exhausting its statement budget and
a handler exhausting its acquire budget fail at roughly the same
moment under load. `0` disables the timeout entirely.

Pool-sizing fields (`max_connections`, `min_connections`,
`acquire_timeout_ms`, `idle_timeout_ms`, `max_lifetime_ms`) match the
pgvector defaults field-for-field. Run two features together
(`--features pg-state,pg-vectors`) and you get split pools with
identical tuning, which is the right default for production.

See [pgvector-backend.md § 7](pgvector-backend.md#7-sqlx-connection-pool-sizing)
for the per-field rationale — the same logic applies here.

---

## 8. BIGINT epoch_ms, no chrono

Every timestamp column in the Phase 3 schema is `BIGINT` holding
milliseconds since the Unix epoch. Timestamps are generated at bind
time via `SystemTime::duration_since(UNIX_EPOCH)` — **no `chrono`,
no `TIMESTAMPTZ`**. This is a workspace-wide convention (pgvector
already did it) and sidesteps the sqlx `chrono` feature's cargo
`links` conflict entirely.

If you later need wall-clock-aware types on the Postgres side, do
the conversion in the query layer:

```sql
SELECT to_timestamp(created_at_epoch_ms / 1000.0) FROM the_one.audit_events;
```

Never add `chrono` to the dep graph.

---

## 9. Migration from SQLite

**Not automated in Phase 3.** Switching from SQLite to Postgres
requires re-running `project.init` against the new backend. The
broker picks up the new `StateStore` on boot; **existing
audit/profile/diary history in the old SQLite DB is NOT migrated**.

If you need the history, export it manually before the cutover:

```bash
# Dump the old SQLite state
sqlite3 ~/.the-one/projects/<project>/state.db .dump > state-backup.sql

# Or export per table to CSV
sqlite3 ~/.the-one/projects/<project>/state.db \
    -header -csv \
    "SELECT * FROM diary_entries" > diary-backup.csv
```

Re-apply to Postgres with your preferred import tooling
(`psql -f`, `\COPY`, etc.). Note that field types differ in places
(`INTEGER PRIMARY KEY AUTOINCREMENT` → `BIGSERIAL`, etc.), so a
direct `sqlite3 .dump | psql` pipe won't work without transformation.

There's **no cross-backend migration tool** and there won't be,
because schema drift between backends is fine for a greenfield
install but unsafe for a data migration. The strict guarantee is:
re-ingest your sources against the new backend and the broker's
behaviour is identical.

### Typical cutover

```bash
# 1. Stand up Postgres, stop the broker.
# 2. Rebuild with the feature.
cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-state

# 3. Export the env vars:
export THE_ONE_STATE_TYPE=postgres
export THE_ONE_STATE_URL=postgres://...

# 4. Restart the broker. First boot applies migrations.
./target/release/the-one-mcp serve

# 5. Re-run project.init on every project.
```

---

## 10. Integration testing

The integration test suite at
`crates/the-one-core/tests/postgres_state_roundtrip.rs` covers all
26 trait methods with 11 tests:

- metadata + bootstrap
- migration runner idempotency
- project profile CRUD
- approvals with scope isolation
- audit (legacy + structured + paginated list)
- conversation sources with wing filter + upsert replace
- AAAK lessons upsert/list/delete
- diary FTS + LIKE fallback + upsert atomicity
- navigation nodes + tunnels + `list_tunnels_for_nodes`
- cross-project upsert rejection

Tests are gated on the **production env surface**:

```bash
docker run --rm -d --name the-one-pg-state \
    -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=the_one_test \
    -p 55433:5432 postgres:16

THE_ONE_STATE_TYPE=postgres \
THE_ONE_STATE_URL=postgres://postgres:pw@localhost:55433/the_one_test \
cargo test -p the-one-core --features pg-state \
    --test postgres_state_roundtrip -- --test-threads=1
```

`--test-threads=1` is **required** — every test drops and recreates
the `the_one` schema, so parallel tests would race. When the env
vars aren't set, the tests skip gracefully via `return` in
`matching_env()` — no panic, no error, zero spam in CI.

---

## 11. Phase 4 combined preview

Phase 4 will add `THE_ONE_STATE_TYPE=postgres-combined` +
`THE_ONE_VECTOR_TYPE=postgres-combined` with byte-identical URLs.
When both are set, the broker will construct **one** `sqlx::PgPool`
that serves both `StateStore` AND `VectorBackend` trait roles.
Transactional writes spanning state + vectors become possible (e.g.
"ingest conversation AND record the audit row atomically in one
transaction").

The Phase 3 `state_postgres` config block and the Phase 2
`vector_pgvector` block stay as-is — Phase 4 just adds a dispatcher
that reads both and spins up one pool. The distinct tracking tables
(`pgvector_migrations` vs `state_migrations`) are precisely what lets
Phase 4 share one schema without the two hand-rolled runners
colliding on version numbers.

---

## 12. Coverage gap caveat

The Phase 3 resume plan asked for "run the existing broker
integration tests against a `PostgresStateStore`-backed broker."
In practice, the existing broker integration tests construct
`ProjectDatabase::open` directly in tempdirs, and porting them to
drive a live Postgres container would require significant
test-harness rework beyond Phase 3's scope.

**What shipped instead**: 11 PostgresStateStore-specific integration
tests covering every trait method behaviourally (see § 10). This
gives full trait-surface coverage but doesn't exercise the broker's
full handler pipeline against Postgres. If you hit a discrepancy
between SQLite and Postgres behaviour at the broker layer, please
open an issue — Phase 4 or Phase 7 will revisit this.

---

## 13. See also

- [Configuration guide](configuration.md#multi-backend-selection-v0160)
  — env vars + validation rules + field tables
- [Multi-backend operations](multi-backend-operations.md) — deployment
  matrix across state + vector axes
- [pgvector backend](pgvector-backend.md) — Phase 2 sibling guide,
  same sqlx + hand-rolled migration pattern
- [Architecture guide](architecture.md#multi-backend-architecture-v0160)
  — trait surface, broker cache, factory dispatcher, sync-over-async
  bridge rationale
- [Production hardening v0.15.md](production-hardening-v0.15.md) §§
  14–18 — the v0.15/v0.16 feature history
