# Multi-Backend Operations Guide

**Target version:** v0.16.0-rc1+
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

| Backend        | Status (v0.16.0-phase2) | Capabilities                                             | Feature flag       |
|----------------|-------------------------|----------------------------------------------------------|--------------------|
| **Qdrant**     | First-class             | chunks, hybrid, entities, relations, images              | default            |
| **pgvector**   | **First-class (v0.16.0 Phase 2)** | chunks, entities, relations (hybrid = Decision D, deferred) | `pg-vectors`       |
| **Redis-Vector** | Second-class          | chunks only (+ persistence check)                        | `redis-vectors`    |
| **In-memory**  | Fallback                | keyword search only                                      | always available   |

### State store backends

| Backend        | Status (v0.16.0-rc1) | Capabilities                        | Feature flag       |
|----------------|----------------------|-------------------------------------|--------------------|
| **SQLite**     | First-class          | FTS5, transactions, WAL             | default            |
| **Postgres**   | Planned (Phase B2)   | tsvector FTS, full ACID             | `pg-state` (future) |
| **Redis-AOF**  | Planned (Phase B3)   | RedisJSON + persistence             | `redis-state` (future) |
| **Redis cache**| Planned (Phase B3)   | volatile, fast                      | `redis-state` (future) |

### Combined single-connection backends

| Backend                    | Status              | Benefit                               |
|----------------------------|---------------------|---------------------------------------|
| SQLite + Qdrant sidecar    | First-class today   | Default deployment                    |
| SQLite + Redis-Vector      | Supported today     | Low-latency small deployments         |
| **Postgres + pgvector**    | Planned (Phase C)   | One DB, transactional state+vector    |
| **Redis + RediSearch + AOF** | Planned (Phase C) | One Redis, everything in one process  |

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

### Planned: Postgres + pgvector (combined, Phase 4)

```bash
# Planned Phase 4:
export THE_ONE_STATE_TYPE=postgres-combined
export THE_ONE_STATE_URL=postgres://user:password@db.internal/the_one
export THE_ONE_VECTOR_TYPE=postgres-combined
export THE_ONE_VECTOR_URL=postgres://user:password@db.internal/the_one  # byte-identical
```

When both TYPEs are `postgres-combined` and the URLs match
byte-for-byte, the broker constructs a single `sqlx::PgPool` that
serves both `StateStore` and `VectorBackend` — writes to state
and vectors can commit in one transaction. Available in Phase 4.

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
| Existing Postgres stack | Postgres + pgvector (future)     | Colocate with your other data, transactional   |
| Existing Redis stack  | Redis + RediSearch (future)        | One service to run, microsecond latencies      |
| Large corpus (100M+)  | Qdrant                             | Dedicated vector DB scales better than pgvector past ~10M |
| Small corpus (<1M)    | pgvector or Redis-Vector (future)  | Cheaper to operate, no sidecar                 |
| Regulated / ACID critical | Postgres + pgvector (future)  | Single ACID commit across state AND vectors    |

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

### SQLite → Postgres (planned)

1. Drain the watcher and shut down the broker cleanly.
2. Run the (planned) `maintain: action: migrate_state` tool with
   `from: sqlite, to: postgres`. This dumps the SQLite tables and
   re-inserts them into the Postgres schema using the same
   `StateStore` trait on both sides.
3. Update config: set `state_backend: "postgres"` and
   `postgres_url: ...`
4. Restart. The broker opens the Postgres pool, runs the same
   migrations, and every subsequent write goes to Postgres.

### Qdrant → pgvector (planned)

1. Keep both running during migration.
2. Run `maintain: action: migrate_vectors` with
   `from: qdrant, to: pgvector`.
3. The broker exports each Qdrant collection in batches and upserts
   into pgvector via the `VectorBackend` trait — no re-embedding
   needed if dimensions match.
4. Update `vector_backend: "pgvector"` in config and restart.

### Redis-Vector → Redis-Combined (planned)

Redis-Vector already uses RediSearch under the hood. Migrating to
the combined backend just means pointing state operations at the
same Redis URL and enabling `redis_persistence_required` on a Redis
instance with `appendonly yes`.

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
Yes for vectors (the embeddings are in the old backend). No for state
(the migration tool copies tables verbatim).

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
    │    └─ Yes → Postgres + pgvector (once shipped, Phase B2+B1)
    │              otherwise SQLite + Qdrant
    │
    ├─ Do you already run Redis?
    │    └─ Yes → Redis + RediSearch (once combined mode ships, Phase C)
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

- `docs/plans/2026-04-11-multi-backend-architecture.md` — the full
  architecture plan (Phases A1/A2/B/C).
- `docs/guides/production-hardening-v0.15.md` — v0.15.x durability
  trade-offs + Lever 1 rationale.
- `docs/guides/mempalace-operations.md` — memory palace configuration
  (orthogonal to backend choice).
- `docs/reviews/2026-04-10-mempalace-comparative-audit.md` — why the
  traits exist (mempalace audit findings).
- `crates/the-one-memory/src/vector_backend.rs` — the VectorBackend
  trait source.
- `crates/the-one-core/src/state_store.rs` — the StateStore trait
  source.
