# Multi-Backend Operations Guide

**Target version:** v0.16.0-phase4+
**Audience:** operators deploying the-one-mcp in production.

As of v0.16.0-rc1, the-one-mcp's persistence layer is split into two
orthogonal abstractions so you can mix and match backends without
touching code:

1. **Vector storage** — chunk embeddings, entity vectors, relation
   vectors, image embeddings, hybrid dense+sparse search.
2. **Relational state store** — audit events, conversation sources,
   navigation, diary, AAAK lessons, approvals, project profiles.

This guide is the operator's reference for picking the right
combination.

---

## 1. Backend matrix (what's shipped vs what's planned)

### Vector backends

| Backend        | Status (v0.16.0-phase4) | Capabilities                                             | Feature flag       |
|----------------|-------------------------|----------------------------------------------------------|--------------------|
| **Qdrant**     | First-class             | chunks, hybrid, entities, relations, images              | default            |
| **pgvector (split)**   | **First-class (v0.16.0 Phase 2)** | chunks, entities, relations (hybrid = Decision D, deferred) | `pg-vectors`       |
| **pgvector (combined)** | **First-class (v0.16.0 Phase 4)** | same as split; shares one `sqlx::PgPool` with `PostgresStateStore` | `pg-state,pg-vectors` |
| **Redis-Vector** | Second-class          | chunks only (+ persistence check)                        | `redis-vectors`    |
| **In-memory**  | Fallback                | keyword search only                                      | always available   |

### State store backends

| Backend        | Status (v0.16.0-phase4)   | Capabilities                        | Feature flag       |
|----------------|---------------------------|-------------------------------------|--------------------|
| **SQLite**     | First-class               | FTS5, transactions, WAL             | default            |
| **Postgres (split)**   | **First-class (v0.16.0 Phase 3)** | tsvector FTS, full ACID, BIGINT epoch_ms | `pg-state` |
| **Postgres (combined)** | **First-class (v0.16.0 Phase 4)** | same as split; shares one `sqlx::PgPool` with `PgVectorBackend` | `pg-state,pg-vectors` |
| **Redis-AOF**  | Planned (Phase 5)         | RedisJSON + persistence             | `redis-state` (future) |
| **Redis cache**| Planned (Phase 5)         | volatile, fast                      | `redis-state` (future) |

### Combined single-connection backends

| Backend                    | Status              | Benefit                               |
|----------------------------|---------------------|---------------------------------------|
| SQLite + Qdrant sidecar    | First-class today   | Default deployment                    |
| SQLite + Redis-Vector      | Supported today     | Low-latency small deployments         |
| **Postgres + pgvector**    | **First-class (v0.16.0 Phase 4)** | One DB, one `sqlx::PgPool`, one credential, one backup target |
| **Redis + RediSearch + AOF** | Planned (Phase 6) | One Redis, everything in one process  |

---

## 2. Config reference

### SQLite + Qdrant (default)

```json
{
  "vector_backend": "qdrant",
  "qdrant_url": "http://localhost:6333",
  "qdrant_api_key": null,
  "qdrant_strict_auth": false
}
```

State store: SQLite is implicit — `state.db` lives at
`<project_root>/.the-one/state.db`.

### SQLite + Redis-Vector

```json
{
  "vector_backend": "redis",
  "redis_url": "redis://localhost:6379",
  "redis_index_name": "the_one_memories",
  "redis_persistence_required": true
}
```

Requirements:
- Build with `--features redis-vectors`
- `embedding_provider: "local"` (API embeddings not yet supported on Redis)
- If `redis_persistence_required = true`, the Redis instance must
  have AOF enabled (`appendonly yes`) or the broker refuses to start
  the memory engine for that project.

### SQLite + pgvector (v0.16.0 Phase 2, split-pool)

> **Full operational details in the standalone guide**:
> [pgvector-backend.md](pgvector-backend.md).


Operators running managed Postgres can use pgvector for vector
storage while keeping the v0.15.x SQLite state store. This is the
simplest pgvector deployment: no state migration, no combined-
transaction semantics.

Set the env vars (per § 1 of the backend selection scheme —
secrets live in env, tuning in config.json):

```bash
export THE_ONE_VECTOR_TYPE=pgvector
export THE_ONE_VECTOR_URL=postgres://user:password@db.internal:5432/the_one
# STATE_TYPE/STATE_URL intentionally unset → defaults to sqlite
```

Then tune via `<project>/.the-one/config.json`:

```json
{
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

Every field has a production-sane default — the block above is
only needed if you want to override specific values. Rebuild the
broker with `cargo build --release -p the-one-mcp --bin the-one-mcp
--features pg-vectors` (the feature is off by default).

**Startup sequence on first boot:**

1. `BackendSelection::from_env` parses `THE_ONE_VECTOR_TYPE=pgvector`
   + `THE_ONE_VECTOR_URL=...`. If either is missing or asymmetric
   (only one side set), startup fails loud with a targeted error.
2. `PgVectorBackend::new` opens the sqlx pool with the configured
   min/max/lifetime settings.
3. `preflight_vector_extension` checks `pg_extension` for the
   `vector` extension. If absent and available, it runs
   `CREATE EXTENSION`. If absent and unavailable, it refuses to
   start with per-provider installation instructions (see
   production-hardening-v0.15.md § 15).
4. The hand-rolled migration runner applies
   `migrations/pgvector/000[0-4]_*.sql` in order, tracking applied
   versions in `the_one.pgvector_migrations` with SHA-256 checksums
   for drift detection.
5. The backend verifies `EmbeddingProvider::dimensions() == 1024`
   (Decision C) — mismatched dim means wrong embedding provider
   and refuses to start.

**What you'll see in Postgres after boot:**

```sql
\dn the_one                              -- schema exists
\dt the_one.*                            -- chunks, entities, relations, pgvector_migrations
\di the_one.chunks_dense_hnsw            -- HNSW index present
SELECT * FROM the_one.pgvector_migrations ORDER BY version;
-- → 5 rows, versions 0..4, one SHA-256 checksum each
```

Everything else (search, upsert, delete-by-source-path) is
transparent — the broker routes through the same `VectorBackend`
trait calls the existing Qdrant path uses.

### Postgres state + Qdrant (v0.16.0 Phase 3, split-pool)

> **Full operational details in the standalone guide**:
> [postgres-state-backend.md](postgres-state-backend.md).


Use the Phase 3 `PostgresStateStore` for state while keeping the
default Qdrant backend for vectors. This gives operators an
ACID-capable state store on managed Postgres without standing up
pgvector yet — useful for teams that already have Postgres but
are still evaluating vector-DB options.

```bash
# Env vars: pick Postgres for state, leave vector unset (defaults to qdrant).
# Both TYPE+URL must be set together for the state axis per § 3 of
# the backend selection scheme.
export THE_ONE_STATE_TYPE=postgres
export THE_ONE_STATE_URL=postgres://user:password@db.internal:5432/the_one
# VECTOR_TYPE unset → defaults to qdrant.
```

```json
{
  "qdrant_url": "http://qdrant.internal:6334",
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

Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp
--features pg-state` (off by default).

**First-boot sequence**:

1. `BackendSelection::from_env` parses
   `THE_ONE_STATE_TYPE=postgres` and `THE_ONE_STATE_URL`. If either
   is missing, startup fails loud with a targeted error.
2. `PostgresStateStore::new` opens the sqlx pool with the
   configured min/max/lifetime settings and runs `SET
   statement_timeout` on every freshly-checked-out connection.
3. The hand-rolled migration runner applies
   `migrations/postgres-state/000[01]_*.sql`, tracking versions in
   `the_one.state_migrations` with SHA-256 drift detection.
4. The Qdrant path takes over for vectors, unchanged.

### Postgres state + pgvector (v0.16.0 Phase 3, split-pool, two pools)

Split-pool Postgres on **both** axes — separate pools for state and
vectors, potentially different DSNs, different credentials, different
statement timeouts. Use this shape when you need independent pool
budgets, independent credential rotation, or separate databases for
state and vectors. If you're on one database and want operational
unity (one credential, one pgbouncer entry, one backup target), the
combined variant ships in Phase 4 — see the next subsection.

```bash
export THE_ONE_STATE_TYPE=postgres
export THE_ONE_STATE_URL=postgres://state_user:pw@db.internal/the_one_state
export THE_ONE_VECTOR_TYPE=pgvector
export THE_ONE_VECTOR_URL=postgres://vec_user:pw@db.internal/the_one_vec
```

Both `[state_postgres]` and `[vector_pgvector]` blocks in
`config.json` apply. They're independent — tuning one doesn't
affect the other. This is the deployment shape for operators who
want Postgres-backed state AND pgvector but with **separate
credential rotation and pool budgets** for each axis.

Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp
--features pg-state,pg-vectors`.

### Postgres + pgvector (combined, v0.16.0 Phase 4, one pool)

```bash
export THE_ONE_STATE_TYPE=postgres-combined
export THE_ONE_STATE_URL=postgres://user:password@db.internal/the_one
export THE_ONE_VECTOR_TYPE=postgres-combined
export THE_ONE_VECTOR_URL=postgres://user:password@db.internal/the_one  # byte-identical
```

When both TYPEs are `postgres-combined` and the URLs match
byte-for-byte, the broker constructs a single `sqlx::PgPool` that
serves both the `StateStore` trait role and the `VectorBackend`
trait role — state writes and vector writes flow through the same
connection pool, with one credential to rotate, one set of IAM
grants, one pgbouncer entry, and one PITR backup window covering
everything.

Configuration reuses both existing blocks — there is no
`[combined.postgres]` section. When combined, the **state
config's pool sizing wins**: `state_postgres.max_connections`,
`min_connections`, the timeout fields, and `statement_timeout_ms`
all apply to the shared pool. `vector_pgvector`'s corresponding
pool fields are ignored on this path; its HNSW tuning
(`hnsw_m`, `hnsw_ef_construction`, `hnsw_ef_search`) still
applies because those are migration-time + query-time settings,
not pool settings.

Rebuild: `cargo build --release -p the-one-mcp --bin the-one-mcp
--features pg-state,pg-vectors`. The combined dispatcher is
automatically active whenever both features are on and the env
vars select `postgres-combined` for both axes.

Full operational reference — topology diagrams, migration paths
from split-pool, verification queries, the "state config wins"
rationale, and the scope of what Phase 4 does NOT ship (no
cross-trait transaction primitive yet, no automated split →
combined migration tool) — is in the standalone
[`combined-postgres-backend.md`](combined-postgres-backend.md)
guide.

### Planned: Redis + RediSearch + AOF (combined)

```json
{
  "vector_backend": "redis",
  "state_backend": "redis",
  "redis_url": "redis://localhost:6379",
  "redis_persistence_required": true
}
```

Same pattern: when both point at Redis, one `fred::Client` handles
both roles.

---

## 3. Capability reporting

Every backend reports what it supports via a static `capabilities()`
method. You can inspect the active backend through the `observe`
tool once the broker is running.

### `the_one_memory::vector_backend::BackendCapabilities`

```rust
pub struct BackendCapabilities {
    pub name: &'static str,      // "qdrant", "redis-vectors", "pgvector"
    pub chunks: bool,
    pub hybrid: bool,
    pub entities: bool,
    pub relations: bool,
    pub images: bool,
    pub persistence_verifiable: bool,
}
```

Today:
- `qdrant` → all `true` (except `persistence_verifiable: false`)
- `redis-vectors` → only `chunks: true`, everything else `false`

### `the_one_core::state_store::StateStoreCapabilities`

```rust
pub struct StateStoreCapabilities {
    pub name: &'static str,      // "sqlite", "postgres", "redis-aof"
    pub fts: bool,               // SQLite FTS5, Postgres tsvector, etc.
    pub transactions: bool,      // ACID multi-statement
    pub durable: bool,           // WAL, AOF, remote commit
    pub schema_versioned: bool,  // tracks schema_migrations
}
```

Today:
- `sqlite` → all `true`

---

## 4. Trade-off matrix

Operator decision guide — pick based on your priorities:

| Priority              | Best pick                          | Why                                             |
|-----------------------|------------------------------------|-------------------------------------------------|
| Single-machine, minimal ops | SQLite + Qdrant sidecar      | Default; works offline; one state file        |
| Lowest latency reads  | SQLite + Redis-Vector              | Redis in-memory search is sub-millisecond       |
| Maximum durability    | SQLite + Qdrant (with Qdrant backups) | WAL + Qdrant snapshots, no single point of failure |
| Existing Postgres stack, operational unity | Postgres + pgvector (combined, Phase 4) | One credential, one pool, one backup target |
| Existing Postgres stack, independent budgets | Postgres + pgvector (split, Phase 2+3) | Tune state and vectors separately |
| Existing Redis stack  | Redis + RediSearch (future)        | One service to run, microsecond latencies      |
| Large corpus (100M+)  | Qdrant                             | Dedicated vector DB scales better than pgvector past ~10M |
| Small corpus (<1M)    | pgvector or Redis-Vector (future)  | Cheaper to operate, no sidecar                 |
| Regulated / ACID critical | Postgres + pgvector (combined, Phase 4) | Shared pool is the foundation for cross-trait transactions (Phase 4.5+) |

---

## 5. Durability semantics

| Backend        | Crash safety                                | Notes |
|----------------|---------------------------------------------|-------|
| SQLite (WAL, synchronous=NORMAL) | Safe against process crash. OS crash can lose < 1s of writes. | v0.15.1 default. |
| Qdrant         | Safe against process crash. Depends on Qdrant's own persistence config. | Configure via Qdrant operator. |
| Redis-Vector (AOF appendfsync=everysec) | Safe against process crash. OS crash can lose < 1s. | Set `redis_persistence_required: true`. |
| Redis (no AOF) | Volatile. Data lost on restart.             | Use only when you explicitly want a cache. |
| Postgres (planned) | Safe against OS crash via fsync on commit. | Default Postgres behaviour. |

If your workload cannot tolerate the < 1s loss window on SQLite,
the-one-mcp supports overriding with `PRAGMA synchronous=FULL`
(see production-hardening-v0.15.md § 14 for the tradeoff).

---

## 6. Migration paths

> **No automated cross-backend migration tooling ships in v0.16.0.**
> This was an explicit non-goal — see `PROGRESS.md` ("Deferred /
> Not on the v0.16.0 roadmap"). Operators choose a backend at init
> time; switching later is manual re-ingestion against the new
> backend. Schema drift between backends is fine for a greenfield
> install but unsafe for a data migration — which is why no
> `maintain: migrate_state` tool exists and there's no plan to add
> one in the v0.16.0 line.

### SQLite → Postgres (manual, shipped in v0.16.0 Phase 3)

1. Stand up Postgres ≥ 13 (any vanilla image — no extensions
   required for `PostgresStateStore`).
2. Drain the watcher and shut down the broker cleanly.
3. **Optional**: export the old SQLite history if you need it as a
   reference. There's no import path; the dump is for your records
   only.
   ```bash
   sqlite3 ~/.the-one/projects/<project>/state.db .dump > state-backup.sql
   ```
4. Rebuild the broker with the feature:
   ```bash
   cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-state
   ```
5. Export env vars:
   ```bash
   export THE_ONE_STATE_TYPE=postgres
   export THE_ONE_STATE_URL=postgres://user:pw@db.internal/the_one
   ```
6. Restart the broker. First boot applies the Phase 3 migrations
   (`0000_state_migrations_table.sql` + `0001_state_schema_v7.sql`).
7. Re-run `project.init` on every project to rehydrate metadata
   against the new backend. Audit history, diary entries, and AAAK
   lessons from the SQLite DB are NOT carried over — they exist
   only in the backup.

Full guide + per-provider install notes:
**[postgres-state-backend.md](postgres-state-backend.md)**.

### Qdrant → pgvector (manual, shipped in v0.16.0 Phase 2)

1. Stand up Postgres ≥ 13 with the `vector` extension available
   (Supabase ships it; AWS RDS / Cloud SQL / Azure / self-hosted
   each have different install paths — see the standalone guide).
2. Rebuild with the feature:
   ```bash
   cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-vectors
   ```
3. Export env vars:
   ```bash
   export THE_ONE_VECTOR_TYPE=pgvector
   export THE_ONE_VECTOR_URL=postgres://user:pw@db.internal/the_one
   ```
4. Restart the broker. `preflight_vector_extension` runs a
   defensive 3-query probe with targeted per-provider error
   messages; if the `vector` extension isn't installed, startup
   fails loud with the install steps for your environment.
5. Re-run `project.init` on every project to re-ingest source
   documents against the pgvector backend. Existing Qdrant data is
   untouched — delete the Qdrant collection manually once you're
   confident the new backend is good.

**The vector dimension is locked at 1024** (Decision C, matching
BGE-large-en-v1.5 quality tier). If your existing Qdrant collection
uses a different dim, you'll need to re-embed anyway — no
batch-copy shortcut.

Full guide + HNSW tuning + per-provider install:
**[pgvector-backend.md](pgvector-backend.md)**.

### Split-pool Postgres → Combined Postgres (shipped in v0.16.0 Phase 4)

`postgres-combined` ships in Phase 4 as a dispatcher change — one
`sqlx::PgPool` serving both the `StateStore` and `VectorBackend`
trait roles instead of two separate pools. If your current
split-pool Postgres deployment already points both sides at the
**same** database, the migration is zero data copy:

1. Verify `THE_ONE_STATE_URL` and `THE_ONE_VECTOR_URL` are
   byte-identical. They can already share a schema because the
   tracking tables (`the_one.state_migrations` and
   `the_one.pgvector_migrations`) are distinct by design.
2. Shut down the broker cleanly.
3. Change both env var TYPEs to `postgres-combined`.
4. Restart. The broker constructs one shared pool instead of two.
   Both migration runners are idempotent and exit cleanly against
   the already-migrated schema. No schema changes, no data copy.

If your split-pool deployment uses **different** databases for
state and vectors, the combined path won't start — it refuses to
proceed when the URLs don't match byte-identically. You have to
pick one database as the winner and manually migrate the other
side's data in via `pg_dump` / `pg_restore`; the standalone
combined guide has the exact commands. See
[`combined-postgres-backend.md § 8`](combined-postgres-backend.md#8-migration-from-split-pool-postgres--different-databases).

### Redis migrations (Phases 5 + 6, pending)

Redis state store ships in Phase 5 (three durability modes: cache,
persistent with AOF, combined). Combined Redis+RediSearch ships in
Phase 6. Migration paths for those will follow the same
re-ingestion pattern as SQLite → Postgres — no automated cross-
backend tooling.

---

## 7. Observability per backend

Metrics and audit events behave identically across backends because
the broker only talks through the traits. Use `observe: metrics` and
`observe: audit_events` the same way you would with the default.

Backend-specific health checks:
- **Qdrant**: HTTP `/healthz` on the Qdrant URL
- **Redis**: `INFO persistence` + `FT._LIST` for index status
- **SQLite**: `PRAGMA integrity_check` via the backup action
- **Postgres** (future): `SELECT 1` + `pg_stat_activity`

---

## 8. Frequently asked

**Q: Can I run Postgres + SQLite at the same time (state in Postgres, vectors in SQLite)?**
No — SQLite isn't a vector backend. The supported mixes are the four
rows in the "combined backends" table above, plus any combination
where `vector_backend` is an external service (Qdrant / Redis-Vector
/ pgvector) and `state_backend` is SQLite.

**Q: Can I run Redis cache-mode for state and Qdrant for vectors?**
Yes (once Redis state mode ships in Phase B3). This gives you a
volatile state store — audit events and diary entries survive only
until the Redis instance restarts. Useful for ephemeral
experimentation but not production.

**Q: Does changing backends require re-indexing?**
Yes for vectors when the target is a different physical backend
(e.g. Qdrant → pgvector: the embeddings live in the old backend
and must be regenerated against the new one). No for state changes
that target the same physical backend (e.g. split-pool Postgres →
combined Postgres: the broker just switches from two pools to one,
no schema change). No for state when moving SQLite → Postgres on
the same project: the migration tool copies tables verbatim.

**Q: Can I point two brokers at the same Postgres instance for HA?**
Not yet. The current broker design assumes exclusive access to the
state store. Multi-broker HA is a future feature (would need
distributed locking for SQLite PROJECTS as well, so it's not
backend-specific).

**Q: Can pgvector scale to 100 million vectors?**
At ~10 million vectors per table, pgvector HNSW starts losing to
Qdrant on p99 latency. The crossover depends on your HNSW `m` and
`ef_search` params. If your corpus grows past ~10M, plan to
migrate to Qdrant or shard across Postgres instances.

---

## 9. Backend selection flowchart

```
Start here
    │
    ├─ Do you already run Postgres?
    │    ├─ Yes, one DB, want operational unity
    │    │    → Postgres + pgvector (combined, Phase 4)
    │    ├─ Yes, multiple DBs or independent pool budgets
    │    │    → Postgres + pgvector (split, Phase 2+3)
    │    └─ No → SQLite + Qdrant (default)
    │
    ├─ Do you already run Redis?
    │    └─ Yes → Redis + RediSearch (once combined mode ships, Phase 6)
    │              otherwise SQLite + Redis-Vector
    │
    ├─ Corpus > 10M vectors?
    │    └─ Yes → SQLite + Qdrant (Qdrant scales better)
    │
    ├─ Single-machine, offline, minimal ops?
    │    └─ Yes → SQLite + Qdrant sidecar (default)
    │
    └─ Otherwise → SQLite + Qdrant (default is the safest pick)
```

---

## 10. See also

### Backend-specific guides (v0.16.0)

- **[pgvector-backend.md](pgvector-backend.md)** — Phase 2 standalone
  guide. Per-provider install (Supabase/RDS/Cloud SQL/Azure/self-
  hosted), defensive preflight, hand-rolled migration runner,
  HNSW tuning, sqlx pool sizing, monitoring queries, Decision D
  deferral notes.
- **[postgres-state-backend.md](postgres-state-backend.md)** — Phase 3
  standalone guide. Sync-over-async bridge rationale, FTS5 →
  tsvector translation, `'simple'` vs `'english'` tokenizer
  choice, schema v7 parity, 11 integration tests.
- **[combined-postgres-backend.md](combined-postgres-backend.md)** — Phase 4
  standalone guide. One `sqlx::PgPool` serving both trait roles,
  the "state config wins" pool-sizing rule, migration paths from
  split-pool Postgres, verification queries, integration test
  suite at `crates/the-one-mcp/tests/postgres_combined_roundtrip.rs`,
  and the list of things Phase 4 deliberately does NOT ship
  (no cross-trait transaction primitive yet, no named combined
  backend type).
- `docs/guides/redis-vector-backend.md` — Redis/RediSearch backend
  and persistence expectations.

### Configuration + architecture

- **[configuration.md § Multi-Backend Selection (v0.16.0+)](configuration.md#multi-backend-selection-v0160)**
  — env var surface + validation rules + per-backend config tables.
- **[architecture.md § Multi-Backend Architecture (v0.16.0+)](architecture.md#multi-backend-architecture-v0160)**
  — trait surface, broker cache, factory dispatcher, cross-phase
  relationship table.
- `docs/plans/2026-04-11-multi-backend-architecture.md` — the full
  architecture plan (Phases A1/A2/B/C).
- `docs/guides/production-hardening-v0.15.md` — v0.15.x durability
  trade-offs + Lever 1 rationale (§§ 15 + 16 now redirect to the
  standalone backend guides above).

### Plans + trait source

- `docs/plans/2026-04-11-resume-phase1-onwards.md` — canonical Phase
  1–7 execution plan with DONE blocks for Phases 0–4.
- `docs/plans/2026-04-11-resume-phase4-prompt.md` — standalone resume
  prompt that drove the Phase 4 session (kept in tree as a
  reference for the decision trail behind the refined Option Y
  architecture).
- `docs/guides/mempalace-operations.md` — memory palace configuration
  (orthogonal to backend choice).
- `docs/reviews/2026-04-10-mempalace-comparative-audit.md` — why the
  traits exist (mempalace audit findings).
- `crates/the-one-memory/src/vector_backend.rs` — the VectorBackend
  trait source.
- `crates/the-one-core/src/state_store.rs` — the StateStore trait
  source.
- `crates/the-one-core/src/config/backend_selection.rs` — the env
  var parser source (v0.16.0 Phase 2).
