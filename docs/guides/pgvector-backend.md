# pgvector Backend Guide

> **Status:** First-class — shipped in **v0.16.0-phase2** (`91ff224`,
> tag `v0.16.0-phase2`).
>
> **Cargo feature:** `pg-vectors` (off by default — operators opt in
> at build time).
>
> **Env-var activation:** `THE_ONE_VECTOR_TYPE=pgvector` +
> `THE_ONE_VECTOR_URL=<dsn>`.

`PgVectorBackend` is the first real alternative `VectorBackend` after
the Phase A trait extraction. Operators running managed Postgres can
co-locate their vectors with their relational data instead of
standing up a separate Qdrant service. It implements chunks +
entities + relations on pgvector (hybrid search is Decision D,
deferred to Phase 2.5).

This guide covers installation, tuning, operational concerns, and the
migration-ownership model. For the full backend-selection scheme and
the deployment matrix across axes, see the
[multi-backend operations guide](multi-backend-operations.md). For the
config fields and env-var validation rules, see
[configuration.md](configuration.md#multi-backend-selection-v0160).

---

## 1. Installing the `vector` extension

`PgVectorBackend::new` runs `preflight_vector_extension` before any
migration. It performs three defensive checks and produces a targeted
error message for each of the five common managed-Postgres
environments when it can't find the extension:

### Supabase

pgvector is **pre-installed on every project**. No action required.
The preflight query sees `vector` in `pg_extension` and returns `Ok(())`.

### AWS RDS / Aurora Postgres

`vector` ships with RDS Postgres ≥ 15.3 but is **not installed by
default**. Steps:

1. Edit your instance parameter group, add `vector` to
   `shared_preload_libraries`.
2. Reboot the instance.
3. Connect as `rds_superuser` once and run `CREATE EXTENSION vector;`.

Subsequent broker startups see the installed extension and skip the
`CREATE`.

### Google Cloud SQL for PostgreSQL

Set the database flag `cloudsql.enable_pgvector` to `on` on the
instance. Unlike RDS, Cloud SQL doesn't require a separate
`CREATE EXTENSION` step once the flag is set.

### Azure Database for PostgreSQL Flexible Server

Add `vector` to the server parameter `azure.extensions`, then connect
as any member of `azure_pg_admin` for the one-time
`CREATE EXTENSION vector;`.

### Self-hosted Postgres

Install the pgvector package for your distribution (`apt install
postgresql-16-pgvector` on Debian/Ubuntu, `brew install pgvector` on
macOS, or build from source per the upstream README), restart
Postgres, then connect as a superuser once to
`CREATE EXTENSION vector;`.

### The defensive preflight in detail

The runtime preflight runs three queries:

1. `SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')`
   — is the extension already installed? (Supabase path)
2. `SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector')`
   — is it available to install? (RDS/Cloud SQL/Azure path)
3. `CREATE EXTENSION IF NOT EXISTS vector` — actually install it.

If step 2 returns false, the broker refuses to start with an error
listing all five installation paths above. If step 3 fails, the error
names the likely culprit (usually "connecting role lacks CREATE
privilege on this database — connect as superuser once").

No silent fallbacks, no cryptic sqlx errors. The three probe queries
cover every permission model cleanly.

---

## 2. Building and running

Rebuild the broker with the `pg-vectors` Cargo feature (off by default):

```bash
cargo build --release -p the-one-mcp --bin the-one-mcp --features pg-vectors
```

Export the env vars (secrets live in env, never in config.json):

```bash
export THE_ONE_VECTOR_TYPE=pgvector
export THE_ONE_VECTOR_URL=postgres://user:password@db.internal:5432/the_one
```

Tune via `<project>/.the-one/config.json` (every field has a
production-sane default, so the block is optional):

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

First boot applies migrations automatically and verifies the
embedding provider's dim matches the schema's hardcoded `dim=1024`.

---

## 3. Migration-ownership model

Phase 2 uses a **hand-rolled migration runner** at
`the_one_memory::pg_vector::migrations` instead of `sqlx::migrate!`.

### Why not `sqlx::migrate!`?

sqlx's `migrate` feature transitively references
`sqlx-sqlite?/migrate` via cargo weak-dep syntax. In theory, weak deps
should only activate when the parent dep is already activated. In
practice, cargo's `links` conflict check evaluates weak deps as
"possibly activates" and pulls `sqlx-sqlite` into the resolution
graph. `sqlx-sqlite`'s `libsqlite3-sys ^0.30.1` requirement collides
with `rusqlite 0.39`'s `libsqlite3-sys ^0.37.0`, which is already in
the workspace. Result: the workspace refuses to compile with both.

Dropping the `migrate` feature sidesteps the conflict entirely. See
the `pg-vectors` feature comment in
`crates/the-one-memory/Cargo.toml` for the full bisection.

### What the runner does

- Embeds every `.sql` file in
  `crates/the-one-memory/migrations/pgvector/` via `include_str!` at
  compile time. No runtime file reads, no deployment footprint.
- Applies migration 0 (the tracking table itself) unconditionally —
  the body is `CREATE TABLE IF NOT EXISTS`, so re-running is safe.
- For migrations 1..N, checks `the_one.pgvector_migrations` for an
  existing row at that version. If present, it **rehashes the
  embedded file and compares SHA-256 checksums**; drift (someone
  edited a `.sql` file post-ship) refuses to continue with a clear
  error. If absent, it applies the migration in one
  `raw_sql().execute()` call and inserts a tracking row.
- Exposes `list_applied(&pool)` for observability and integration
  tests.

### Files shipped

```
crates/the-one-memory/migrations/pgvector/
├── 0000_migrations_table.sql           -- hand-rolled tracking table
├── 0001_extension_and_schema.sql       -- CREATE EXTENSION + schema
├── 0002_chunks_table.sql               -- chunks + HNSW index
├── 0003_entities_table.sql             -- entities + HNSW index
└── 0004_relations_table.sql            -- relations + HNSW index
```

### Checksum drift detection

This is the guarantee `sqlx::migrate!` provides for free and that
most hand-rolled runners lose. Phase 2 catches it with one extra
`SELECT checksum` per migration per startup — negligible cost for the
safety it buys. If you ever edit `0002_chunks_table.sql` after
shipping, the next startup against an already-migrated database will
fail with a precise error pointing at the version + description that
drifted.

### Coexistence with `PostgresStateStore` (Phase 3)

Phase 3's state-store runner uses a **distinct tracking table**
(`the_one.state_migrations` vs pgvector's `the_one.pgvector_migrations`)
so the Phase 4 combined deployment (`postgres-combined` TYPE on
both axes) shares one schema without collision. Both runners are
idempotent and coexist cleanly — see
[`combined-postgres-backend.md`](combined-postgres-backend.md).

---

## 4. Vector dimension is locked at 1024

**Decision C** (locked in during the Phase 2 brainstorm) hardcodes
`dim=1024` into every `vector(...)` literal in
`migrations/pgvector/000[234]_*.sql`. This matches the default
quality-tier embedding provider (BGE-large-en-v1.5). The backend
constructor reads `EmbeddingProvider::dimensions()` and refuses to
start if the live provider reports a different dim — **you cannot
silently swap embedding providers and keep the schema**.

Reasons this is a feature, not a limitation:

1. **Changing an embedding provider changes the vector space.** Even
   if the dim matched numerically, re-using old vectors with a new
   provider produces semantically incoherent search results. Forcing
   a schema migration makes the rebuild deliberate.
2. **Combined Postgres+pgvector (shipped in Phase 4)** shares
   this schema with `PostgresStateStore` via a single
   `sqlx::PgPool`. The migration-tracked schema makes the
   combined path's zero-data-copy split → combined transition
   possible — the combined adapter reuses the same migrations
   already applied by the split-pool Phase 2/3 builds.
3. **If you need multi-dim support later**, that's a new migration
   file (`0005_reshape_chunks_dim.sql`) with a documented downtime
   step — not a silent config toggle.

---

## 5. HNSW tuning

Phase 2 ships HNSW indexes with `m = 16` and `ef_construction = 100`
baked into the migration SQL. These are the pgvector defaults and the
Qdrant defaults — a safe starting point for corpora up to ~10 million
chunks on 1024-dim vectors.

### Three tunables, three ownership models

| Parameter | When applied | How to change |
|---|---|---|
| `hnsw_m` (graph connectivity) | Migration time, in `CREATE INDEX ... WITH (m = 16, ef_construction = 100)` | DROP INDEX + CREATE INDEX with new value. Config field exists in `vector_pgvector` but only takes effect on a fresh schema. |
| `hnsw_ef_construction` (build quality) | Migration time, same as `m` | Same DROP + CREATE recipe. |
| `hnsw_ef_search` (query-time recall) | **Per-query** via `SET LOCAL hnsw.ef_search = N` inside the transaction wrapping the `SELECT ... ORDER BY dense_vector <=> $1`. | Change the config field in `vector_pgvector` and restart the broker. No DDL required. |

The per-query `SET LOCAL` approach keeps `ef_search` scoped to the
current transaction — important because pgvector treats it as a
session GUC that otherwise leaks to other users of the same pool
connection after it's returned.

### Manual retune recipe for `m` / `ef_construction`

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

Running this on a large index takes minutes to hours depending on
row count and will block INSERTs; plan for it during a maintenance
window.

---

## 6. HNSW vs IVFFlat

pgvector supports two index types: HNSW (default, higher recall) and
IVFFlat (lower memory, worse recall on small datasets). Phase 2 ships
**HNSW only** because:

- **Recall matters more than memory** at the-one-mcp scale. Even a
  "big" codebase is < 10M chunks, well inside HNSW's sweet spot.
- **IVFFlat needs a pre-built trained list** — you seed it with a
  sample of vectors, which complicates the zero-setup deployment
  story. HNSW builds incrementally.
- **Operators who genuinely need IVFFlat** can swap the index
  manually (`DROP INDEX + CREATE INDEX ... USING ivfflat`). The
  broker never inspects the index type, only the column type, so
  this works without a binary rebuild.

---

## 7. sqlx connection pool sizing

`VectorPgvectorConfig` exposes five pool fields with defaults aimed at
managed-Postgres deployments:

| Field | Default | Rationale |
|---|---|---|
| `max_connections` | 10 | Same as sqlx's default. Bump for high-QPS broker instances. |
| `min_connections` | **2** | **Non-zero**. sqlx's default 0 means the first query after a restart pays full TCP + TLS + auth handshake latency (100–300 ms on RDS). Keeping 2 connections warm pays ≈ 2 × handshake once at startup in exchange for no cold-start tail latency. |
| `acquire_timeout_ms` | 30_000 | How long a broker handler waits for a free connection. 30s is aggressive-but-not-insane; tune down on latency-sensitive setups. |
| `idle_timeout_ms` | 600_000 | 10 min. Idle connections get reaped so long-idle broker instances don't hold pool slots indefinitely. |
| `max_lifetime_ms` | **1_800_000** | 30 min. **Non-infinite**. Forces periodic reconnect to pick up: IAM credential rotation (AWS RDS dynamic secrets), Vault lease expiry, PGBouncer reshards, upstream load-balancer connection draining. sqlx's default `None` is fine for dev, wrong for production. |

---

## 8. Monitoring queries

Three useful queries when diagnosing pgvector performance:

```sql
-- How big is the HNSW index? (Rule of thumb: bytes ≈ 4 * dim * rows * m / 2.)
SELECT pg_size_pretty(pg_relation_size('the_one.chunks_dense_hnsw'));

-- How many chunks per project?
SELECT project_id, count(*)
FROM the_one.chunks
GROUP BY project_id
ORDER BY count(*) DESC;

-- Is a query hitting the HNSW index?
EXPLAIN (ANALYZE, BUFFERS)
SELECT id, (1 - (dense_vector <=> '[0.1, 0.2, ...]'::vector)) AS score
FROM the_one.chunks
ORDER BY dense_vector <=> '[0.1, 0.2, ...]'::vector
LIMIT 10;
```

The `EXPLAIN` output should contain `Index Scan using chunks_dense_hnsw`
— if it shows `Seq Scan` instead, the query isn't routing through the
index. Common causes: missing `ORDER BY distance_op`, missing `LIMIT`,
or the index not yet built.

---

## 9. Running the bench

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
and dense search latency percentiles (p50/p95/p99) over 100 queries.

---

## 10. Migration from Qdrant

**Not automated in Phase 2.** Switching from Qdrant to pgvector
requires re-ingesting every source document against the pgvector
backend — there's no "dump Qdrant + load pgvector" tooling, and there
won't be, because the two backends use different internal
representations and the reprocessing path is the same as a fresh
ingest.

Steps:

1. Stand up a Postgres instance with the `vector` extension installed
   (see § 1).
2. Export the operator config: `THE_ONE_VECTOR_TYPE=pgvector` and
   `THE_ONE_VECTOR_URL=<dsn>`.
3. Rebuild with `--features pg-vectors`.
4. Restart the broker. It boots against the new backend, which runs
   the preflight and applies migrations on first connect.
5. Re-run `project.init` on every project to trigger re-ingest
   against the new backend. Qdrant data is left untouched — you can
   delete the collection manually once you're confident the pgvector
   backend is good.

---

## 11. Hybrid search (Decision D — deferred)

`PgVectorBackend::upsert_hybrid_chunks` and
`search_chunks_hybrid` return
`Err("pgvector hybrid chunk upsert deferred to Phase 2.5 (Decision D)")`.
The backend's `capabilities().hybrid` is `false`. The broker logs a
warning and falls back to dense-only search when
`hybrid_search_enabled = true` is configured on a pgvector
deployment.

Phase 2.5 will land hybrid search after benchmark comparison of two
candidate implementations:

- **α**: `tsvector` + GIN on a computed content column, fused with
  dense cosine scores via linear combination.
- **β**: `sparse_vector_indices` + `sparse_vector_values` columns
  (already present on the chunks schema) used for manual
  inner-product rewrite.

The schema already carries the sparse columns so Phase 2.5 doesn't
need a schema migration to ship β.

---

## 12. Combined Postgres+pgvector (shipped in v0.16.0 Phase 4)

Phase 4 is live. Setting `THE_ONE_STATE_TYPE=postgres-combined` +
`THE_ONE_VECTOR_TYPE=postgres-combined` with byte-identical URLs
makes the broker construct **one** `sqlx::PgPool` that serves both
the `StateStore` trait role and the `VectorBackend` trait role
against a single Postgres database. The operational win is one
credential to rotate, one pgbouncer entry, one PITR backup window,
and one set of IAM grants — not a new named backend type or new
trait methods.

This pgvector guide stays focused on the **split-pool** shape
(separate pool for vectors, potentially on a different database
from state) because the tuning knobs are the same on both paths.
The only asymmetry is that on the combined path, this config's
pool-sizing fields (`max_connections`, `min_connections`, the
timeout fields) are **ignored** — the state config's pool-sizing
wins. HNSW tuning (`hnsw_m`, `hnsw_ef_construction`, `hnsw_ef_search`)
still applies on both paths because those are migration- and
query-time settings, not pool settings.

The distinct tracking tables (`pgvector_migrations` vs
`state_migrations`) are what let Phase 4 share one schema without
the two hand-rolled runners stepping on each other's versions.
Both runners are idempotent, so a split-pool → combined migration
against the same database is a zero-data-copy dispatcher swap.

Full operational reference — topology, the "state config wins"
rule, verification queries, migration paths, and what Phase 4
deliberately does NOT ship — lives in the Phase 4 standalone
guide: **[combined-postgres-backend.md](combined-postgres-backend.md)**.

---

## 13. See also

- [Configuration guide](configuration.md#multi-backend-selection-v0160)
  — env vars + validation rules + field tables
- [Multi-backend operations](multi-backend-operations.md) — deployment
  matrix across state + vector axes
- [Postgres state backend](postgres-state-backend.md) — Phase 3
  sibling guide, same sqlx + hand-rolled migration pattern
- [Architecture guide](architecture.md#multi-backend-architecture-v0160)
  — trait surface, broker cache, factory dispatcher
- [Production hardening v0.15.md](production-hardening-v0.15.md) §§
  14–18 — the v0.15/v0.16 feature history
