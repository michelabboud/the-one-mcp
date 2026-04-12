# Combined Postgres+pgvector Backend

**Target version:** v0.16.0-phase4+
**Feature flags:** `pg-state` + `pg-vectors` (both)
**Activation:** `THE_ONE_STATE_TYPE=postgres-combined`
              + `THE_ONE_VECTOR_TYPE=postgres-combined`
              + byte-identical `THE_ONE_STATE_URL` and
                `THE_ONE_VECTOR_URL`

Phase 4 of the v0.16.0 multi-backend roadmap. Ships the first
*combined single-pool* backend: one `sqlx::PgPool` serving both the
`StateStore` trait role (audit, diary, navigation, approvals, AAAK
lessons, conversation sources, project profiles) **and** the
`VectorBackend` trait role (chunks, entities, relations) against a
single Postgres database.

> Looking for the split-pool variant where state and vectors each
> get their own pool (and can even live on different databases)?
> See [`postgres-state-backend.md`](postgres-state-backend.md) and
> [`pgvector-backend.md`](pgvector-backend.md) — both still ship
> with Phase 4 and are the right pick when credentials / pool
> budgets / upgrade cadences need to be independent.

---

## 1. When to pick combined over split

| Priority                                            | Pick          |
|-----------------------------------------------------|---------------|
| One credential to rotate, one IAM role, one pgbouncer entry | **combined** |
| One PITR backup window covers state AND vectors     | **combined**  |
| Cross-trait transactional guarantees (future, Phase 4.5+)   | **combined** |
| Independent `statement_timeout` for state vs vectors | split        |
| Different `max_connections` budgets per trait role  | split         |
| State in a managed DB, vectors in a self-hosted DB  | split         |
| Vector-heavy workload that would starve state queries | split       |

The dominant reason to pick combined is **operational unity**: one
pool is one credential, one health check, one backup target, and one
place to reason about connection-pool math. The dominant reason to
pick split is **independence**: two pools are two budgets, two
timeouts, and two sets of knobs that can't interfere with each other.

---

## 2. What "combined" actually means

A combined deployment does **not** merge the two trait roles into a
new named backend type. Under the hood, the broker still holds a
`PostgresStateStore` in its state cache and a `PgVectorBackend`
inside its memory engine — the only difference from split-pool is
that both backends were constructed from the same `sqlx::PgPool`
handle (via their `from_pool(...)` constructors), and the broker
caches that pool once per project in a dedicated
`combined_pg_pool_by_project` map.

Consequences:

- **Both Phase 2 + Phase 3 migration runners apply** to the shared
  pool at first boot. They use distinct tracking tables
  (`the_one.pgvector_migrations` for the vector side,
  `the_one.state_migrations` for the state side) and do not
  interfere with each other's version history.
- **The pgvector extension preflight runs once** on the shared pool.
  Managed-provider install errors (Supabase / RDS / Cloud SQL /
  Azure / self-hosted) surface exactly as they do on the split-pool
  pgvector path.
- **Cross-trait atomicity is NOT a pool-level guarantee.** Broker
  handlers still acquire state-store access and memory-engine access
  separately. The combined pool gives you the *opportunity* to write
  a future transaction primitive that spans both trait roles, but
  Phase 4 does not add one — every handler is still responsible for
  ordering its writes. Cross-trait consistency deliberately stays a
  per-call-site concern until a load-bearing handler demands
  otherwise.

---

## 3. Activation

Production env surface:

```bash
export THE_ONE_STATE_TYPE=postgres-combined
export THE_ONE_VECTOR_TYPE=postgres-combined
export THE_ONE_STATE_URL='postgres://user:password@db.internal/the_one?sslmode=require'
export THE_ONE_VECTOR_URL='postgres://user:password@db.internal/the_one?sslmode=require'
```

Both `*_TYPE` variables **must** equal `postgres-combined`
simultaneously, and both `*_URL` variables **must** be byte-identical
(same query params, same trailing whitespace, same everything). The
env parser (`BackendSelection::from_env`) enforces these rules at
broker startup — a mismatch fails loud as
`CoreError::InvalidProjectConfig` before the broker accepts any
traffic, matching rules 6 + 7 in
`crates/the-one-core/src/config/backend_selection.rs`.

Build with both features:

```bash
cargo build --release -p the-one-mcp --bin the-one-mcp \
    --features pg-state,pg-vectors
```

Omitting either feature compiles the broker without the combined
dispatcher; starting with `postgres-combined` in that case returns
a pointed `InvalidProjectConfig` error telling the operator exactly
which feature is missing and how to rebuild.

---

## 4. Configuration blocks

Combined deployments **reuse the two existing config sections**.
There is no `[combined.postgres]` block — the combined adapter is a
dispatch optimization, not a new config surface.

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
  },
  "vector_pgvector": {
    "schema": "the_one",
    "hnsw_m": 16,
    "hnsw_ef_construction": 100,
    "hnsw_ef_search": 40,
    "max_connections": 10,
    "min_connections": 2,
    "acquire_timeout_ms": 30000,
    "idle_timeout_ms": 600000,
    "max_lifetime_ms": 1800000
  }
}
```

### State config wins on pool sizing

When both axes route through `postgres-combined`, the shared pool is
constructed from **the state config's pool-sizing fields**:

| Field                  | Source                      |
|------------------------|-----------------------------|
| `max_connections`      | `state_postgres`            |
| `min_connections`      | `state_postgres`            |
| `acquire_timeout_ms`   | `state_postgres`            |
| `idle_timeout_ms`      | `state_postgres`            |
| `max_lifetime_ms`      | `state_postgres`            |
| `statement_timeout_ms` | `state_postgres`            |
| `schema`               | `state_postgres` (both migration runners hardcode `the_one`) |

The corresponding fields on `vector_pgvector` are **ignored** on the
combined path. Operators migrating from split-pool should verify
their state-side pool is sized for the combined workload (state
operations are higher-frequency than vector operations in every
handler profile we've measured, so the state budget is the right
anchor — but it may need to be bumped to accommodate the additional
vector traffic).

HNSW build-time parameters (`hnsw_m`, `hnsw_ef_construction`) still
come from `vector_pgvector` because they're migration-time settings
baked into the index — changing them after first boot requires
re-building the HNSW indexes manually, the same way the split-pool
path works.

`hnsw_ef_search` (the query-time recall knob) also still comes from
`vector_pgvector` because it's applied per-search via a transaction-
local `SET LOCAL hnsw.ef_search = ...` — that's runtime, not
migration-time.

### statement_timeout asymmetry

The shared pool wires an `after_connect` hook that runs
`SET statement_timeout = '<state_postgres.statement_timeout_ms>ms'`
on every freshly-checked-out connection. This means **vector
queries inherit the state-side timeout**, even though the
split-pool pgvector path doesn't have an equivalent hook at all.

Practical impact: if you're migrating from split-pool pgvector
(no statement timeout) to combined (state-side timeout applies),
set `state_postgres.statement_timeout_ms` high enough to accommodate
your slowest vector search. Default (30 s) is comfortable for
corpora under ~5M vectors on typical hardware; larger installs
should tune upward after measuring p99 search latency under load.

---

## 5. Topology

### Single-node, self-hosted

```
┌─────────────────────────────────────────────┐
│ the-one-mcp broker                          │
│                                             │
│  ┌────────────┐  ┌───────────────┐          │
│  │ state_by_  │  │ memory_by_    │          │
│  │ project    │  │ project       │          │
│  │ cache      │  │ cache         │          │
│  └─────┬──────┘  └────┬──────────┘          │
│        │              │                     │
│        ▼              ▼                     │
│  PostgresStateStore   PgVectorBackend       │
│  (from_pool clone)    (from_pool clone)     │
│        │              │                     │
│        └──────┬───────┘                     │
│               ▼                             │
│     combined_pg_pool_by_project             │
│     (cached sqlx::PgPool, per project)      │
└──────────────┬──────────────────────────────┘
               │
               ▼
   ┌────────────────────────────────┐
   │ Postgres 16 + pgvector         │
   │  schema: the_one                │
   │  ├─ state_migrations            │
   │  ├─ pgvector_migrations         │
   │  ├─ audit_events                │
   │  ├─ diary_entries (+ tsvector)  │
   │  ├─ chunks (vector(1024))       │
   │  ├─ entities / relations        │
   │  └─ ... (v7 state shape)        │
   └────────────────────────────────┘
```

### Managed Postgres (RDS / Supabase / Cloud SQL / Azure)

Same topology — the managed instance just sits behind a pgbouncer or
the provider's connection pooler. Put **one** pgbouncer entry in
front of the combined deployment's URL; both trait roles fan out
from the same pool, so the pooler sees a single origin application.

---

## 6. Verifying the deployment

After the broker starts, inspect the shared pool:

```sql
-- Both migration tracking tables should exist side by side.
SELECT version, description, applied_at
FROM the_one.state_migrations
ORDER BY version;

SELECT version, description, applied_at
FROM the_one.pgvector_migrations
ORDER BY version;

-- The pgvector extension must be installed.
SELECT extname, extversion FROM pg_extension WHERE extname = 'vector';

-- Confirm a single application is connected (your broker).
SELECT application_name, count(*)
FROM pg_stat_activity
WHERE datname = current_database()
GROUP BY application_name;
```

Expected:

- `state_migrations` populated with rows for versions 0 + 1 (the
  Phase 3 bootstrap + schema v7 migration).
- `pgvector_migrations` populated with rows for versions 0–4 (the
  Phase 2 bootstrap + schema + chunks/entities/relations tables).
- `vector` extension installed (version ≥ 0.5.0 for HNSW).
- One application row in `pg_stat_activity` for the broker, with
  `count` matching the combined pool's `max_connections` budget
  (minus any idle/terminated connections).

---

## 7. Migration from split-pool Postgres

If you're already running `THE_ONE_STATE_TYPE=postgres` +
`THE_ONE_VECTOR_TYPE=pgvector` against the same database (two pools,
same DSN), switching to combined is a zero-data-copy change:

1. Verify both env vars point at the **same** database. If they
   don't, migrate first to point at one database (you'll need to
   drop the unused schema). The combined path refuses to start if
   the URLs don't match byte-identically.
2. Shut down the broker cleanly.
3. Change both `*_TYPE` env vars to `postgres-combined`.
4. Restart. The two migration runners are idempotent — they detect
   the already-applied state and exit clean without running any
   schema changes. The combined pool replaces the two split pools
   seamlessly.

No data migration; just a dispatcher change. Both the state schema
(`the_one.diary_entries`, `audit_events`, etc.) and the pgvector
schema (`the_one.chunks`, `entities`, `relations`) continue to serve
the same data — the broker just reaches them through one pool
instead of two.

## 8. Migration from split-pool Postgres + DIFFERENT databases

If your current split-pool deployment has state and vectors on
**different** databases, combined-pool is **not** a seamless
upgrade. You have to pick one database as the winner and move the
other side's data in. The-one-mcp does not ship tooling for this;
the canonical path is:

1. Pick the database you want to keep.
2. Use `pg_dump --schema=the_one` on the losing database, then
   `pg_restore` into the winning database under a temporary
   schema name.
3. Manually `INSERT INTO the_one.xxx SELECT * FROM tmp.xxx` for
   every table you want to carry over.
4. `DROP SCHEMA tmp CASCADE` and verify the counts match.
5. Flip the env vars to `postgres-combined` and restart.

Audit trails, diary entries, and AAAK lessons are preserved by this
dump/restore flow because they're plain tables; chunks need to be
re-embedded only if the `hnsw.m` or `hnsw.ef_construction` values
differ between the source and target installs (the HNSW indexes are
rebuilt either way on the restore side).

---

## 9. Integration tests

Phase 4's test suite lives at
`crates/the-one-mcp/tests/postgres_combined_roundtrip.rs`, gated on
`all(feature = "pg-state", feature = "pg-vectors")`. The suite
gracefully skips via early `return` when the env vars aren't set,
so running `cargo test --features pg-state,pg-vectors` without a
live Postgres doesn't fail — it just counts the file's tests as
passing without executing any assertions.

To actually exercise the suite against a live database:

```bash
docker run --rm -d --name the-one-pg-combined \
    -e POSTGRES_PASSWORD=pw \
    -e POSTGRES_DB=the_one_combined_test \
    -p 55434:5432 ankane/pgvector

THE_ONE_STATE_TYPE=postgres-combined \
THE_ONE_VECTOR_TYPE=postgres-combined \
THE_ONE_STATE_URL=postgres://postgres:pw@localhost:55434/the_one_combined_test \
THE_ONE_VECTOR_URL=postgres://postgres:pw@localhost:55434/the_one_combined_test \
cargo test -p the-one-mcp --features pg-state,pg-vectors \
    --test postgres_combined_roundtrip -- --test-threads=1
```

`--test-threads=1` is mandatory — every test drops and recreates
the `the_one` schema at the start of its run, and parallel tests
would race on the reset. Use the same `ankane/pgvector` image you
use for the Phase 2 standalone pgvector tests; combined reuses all
of Phase 2's schema shapes plus Phase 3's tsvector tables, so one
image works for all three test suites.

Coverage as of Phase 4:

1. **`combined_build_shared_pool_runs_both_migrations`** — asserts
   both tracking tables are populated after one
   `build_shared_pool` call, and the `vector` extension is
   installed via the preflight.
2. **`combined_build_shared_pool_is_idempotent`** — two back-to-back
   builds against the same DB must both succeed (checksum drift
   detection in both runners exits cleanly).
3. **`combined_build_shared_pool_rejects_wrong_dim`** — a stub
   embedding provider reporting `dim=384` against a `dim=1024`
   schema must fail with a clear `InvalidProjectConfig` mentioning
   both numbers.
4. **`combined_state_and_vector_share_pool_and_persist`** — the
   end-to-end roundtrip: state write + vector upsert + vector
   search, all through distinct trait-role adapters constructed
   from clones of the same `PgPool`.
5. **`combined_from_pool_constructors_do_not_run_migrations`** —
   pins the contract that `PostgresStateStore::from_pool` and
   `PgVectorBackend::from_pool` are pure wrapper constructors;
   calling them against a raw pool that hasn't been prepared by
   `build_shared_pool` must fail at first query time, not silently
   succeed.

Additional unit tests for the config mirror helpers live inline in
`crates/the-one-mcp/src/postgres_combined.rs` and run
unconditionally (under feature gate) without a live Postgres.

---

## 10. What Phase 4 does NOT ship

- **`begin_combined_tx()` trait method.** The original Phase 4 plan
  flagged this as a possible addition; during implementation, no
  load-bearing broker handler was found that needed cross-trait
  atomicity through a dedicated transaction primitive. The
  combined pool gives you the *infrastructure* to add one later
  (any handler can open a sqlx transaction against its shared
  `PgPool` handle), but no trait-level abstraction ships in Phase
  4. If you hit a call site that needs it, flag it and the trait
  will follow the same "evidence first" rule that Phase 2's
  Decision D (hybrid search on pgvector) did.
- **Automated split → combined migration tool.** Moving between
  deployment shapes is a manual env-var swap + restart; there's no
  `maintain: migrate_to_combined` action.
- **Combined backend *type*.** There is no
  `PostgresCombinedBackend` struct. The combined path is a
  dispatcher decision in the broker's factory methods, not a new
  named type that implements both traits. A future Phase 6
  `RedisCombinedBackend` may introduce a named type for Redis if
  the Redis API makes a wrapper struct cleaner — but that's a
  Phase 6 choice, not a precedent Phase 4 set.
- **Cross-database combined.** The parser enforces URL byte-
  equality; you cannot mix two different databases into one
  combined pool. The combined adapter is strictly "one URL, two
  trait roles," not "two URLs, one logical pool."

---

## 11. See also

- [`multi-backend-operations.md`](multi-backend-operations.md) —
  operator reference for the full backend matrix, the shipped
  combined backends table, and the decision flowchart.
- [`pgvector-backend.md`](pgvector-backend.md) — Phase 2 standalone
  guide (split-pool pgvector). Explains the HNSW tuning knobs, the
  per-provider install paths, the `preflight_vector_extension`
  targeted errors, and the Decision C dim lock at 1024.
- [`postgres-state-backend.md`](postgres-state-backend.md) — Phase 3
  standalone guide (split-pool Postgres state). Explains the sync-
  over-async bridge, the FTS5 → tsvector translation, the `simple`
  vs `english` tokenizer choice, and the schema v7 parity notes.
- [`configuration.md § Multi-Backend Selection`](configuration.md#multi-backend-selection-v0160)
  — env var surface + validation rules + per-backend config
  tables (combined is a pointer to this file).
- [`architecture.md § Multi-Backend Architecture`](architecture.md#multi-backend-architecture-v0160)
  — trait surface, broker cache, factory dispatcher, cross-phase
  relationship table with Phase 4 marked shipped.
- `docs/plans/2026-04-11-multi-backend-architecture.md` § 4.3 —
  the "combined adapter" design sketch that shaped Phase 4's
  refined Option Y choice (no named combined type).
- `docs/plans/2026-04-11-resume-phase1-onwards.md` § Phase 4 DONE —
  the shipped-vs-planned diff covering every scope deviation
  (file location, no named type, mirror helper extraction).
- `crates/the-one-mcp/src/postgres_combined.rs` — the combined
  adapter source, including `build_shared_pool` and the two mirror
  helpers.
- `crates/the-one-memory/src/pg_vector.rs`'s `PgVectorBackend::from_pool`
  and `crates/the-one-core/src/storage/postgres.rs`'s
  `PostgresStateStore::from_pool` — the pair of `from_pool`
  constructors Phase 4 added on the existing split-pool backends.
