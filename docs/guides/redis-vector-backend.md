# Redis Vector Backend

> Optional vector backend backed by **Redis + RediSearch HNSW** indexes,
> implemented over the v0.17.0 [the-one-redis facade](the-one-redis-facade.md).
> Default vector backend remains **Qdrant**.

## When to choose Redis

Use this backend when:

- Your operations team already runs Redis-first infrastructure and adding
  a second persistence layer (Qdrant) is operationally expensive.
- You're deploying the **combined** mode (Phase 6 / v0.17.0): a single
  Redis serves both `RedisStateStore` and `RedisVectorStore` via one
  shared `RedisPool`. See [combined-redis-backend.md](combined-redis-backend.md).
- You want the entire broker's state + vectors backed by Redis AOF for a
  single PITR and snapshot window.

Stay on **Qdrant default** when:

- You need image vectors. Redis-Vector image support is tracked
  post-v0.17.0; the current backend supports chunks + entities +
  relations.
- You need pgvector hybrid (sparse + dense). Decision D is deferred.

## Capabilities (v0.16.0 Phase 7 / v0.17.0)

| Capability | Supported |
|---|---|
| Chunks (dense KNN) | ✅ |
| Entities | ✅ (Phase 7) |
| Relations | ✅ (Phase 7) |
| Images | ❌ (tracked post-v0.17.0) |
| Hybrid (dense + sparse) | ❌ |
| Persistence (RDB + AOF) | ✅ enforced via `redis_persistence_required` |

Each type has its own RediSearch index named after the project +
collection, e.g. `the_one_chunks_<project_id>`,
`the_one_entities_<project_id>`, `the_one_relations_<project_id>`.

## Substrate (v0.17.0)

All Redis commands route through the **the-one-redis** facade crate
(`crates/the-one-redis/`). The facade was added in v0.17.0 as a
wholesale port of MAI's `mai-redis` on `redis-rs 1.2`, replacing the
prior `fred 10` substrate. **Load-bearing fix**:
`the_one_redis::pool::connection_config` sets
`response_timeout = None`, removing redis-rs's default 500 ms cap on
blocking commands (`BLPOP`, long `FT.SEARCH`, etc.).

See [the-one-redis-facade.md](the-one-redis-facade.md) for the
architectural rationale, sentinel-test suite, and error-mapping
pattern at the boundary.

## Persistence requirements

- Enable both **RDB snapshots** and **AOF**.
- Use `appendfsync everysec` or stricter where vector durability matters.
- If `redis_persistence_required` is `true` in config, the backend
  verifies persistence at startup and refuses to boot otherwise. This
  protects against accidentally running production data on a cache
  Redis.

Minimal `redis.conf` baseline lives at
[`config/redis.conf.example`](../../config/redis.conf.example).

## RediSearch requirement

- Redis **8.0+** ships RediSearch in core — no module load step.
- For older Redis 7 / Redis Stack, load the RediSearch module before
  the broker connects.
- the-one-mcp targets Redis 8.6.2 in CI.

## Backend selection — env vars (v0.16.0+)

```bash
# Split-pool: Redis vector + SQLite or Postgres state
export THE_ONE_VECTOR_TYPE=redis-vectors
export THE_ONE_VECTOR_URL=redis://127.0.0.1:6379

cargo build --release -p the-one-mcp --bin the-one-mcp \
    --features redis-vectors
```

For **combined** mode (one Redis for state + vectors), see
[combined-redis-backend.md](combined-redis-backend.md) and use
`THE_ONE_VECTOR_TYPE=redis-combined` with matching state-side env
vars.

## Backend selection — legacy `config.json`

Still supported for projects that haven't moved to env-var selection:

```json
{
  "vector_backend": "redis",
  "redis_url": "redis://127.0.0.1:6379",
  "redis_index_name": "the_one_memories",
  "redis_persistence_required": true
}
```

Field expectations:

- Omit `vector_backend` to stay on the default `qdrant` backend.
- Set `vector_backend` to `redis` only when RediSearch is available.
- Keep `redis_index_name` stable for a given deployment so the same
  HNSW index is reused across restarts.
- Leave `redis_url` unset unless Redis is the selected backend.

## Persistence expectations on restore

- Redis durability is your responsibility. Keep both RDB and AOF
  enabled if you expect vectors to survive process or host restarts.
- `redis_persistence_required: true` expresses that expectation in
  config even when Redis is managed outside this repository.
- **Reusing the same `redis_index_name` against a wiped Redis instance
  means the index name is reused, not that vectors are restored
  automatically.** Restore from Redis persistence or reindex.

## Recommended setup flow

1. Start Redis 8.0+ (or Redis 7 with RediSearch module loaded).
2. Enable both RDB snapshots and AOF in `redis.conf`.
3. Set the env vars (or `config.json` fields) for the Redis vector
   backend.
4. Rebuild the broker with `--features redis-vectors` (or
   `redis-state,redis-vectors` for combined mode).
5. Reindex (`maintain: reindex`) after switching backends so vectors
   are regenerated for the target store.

## Runtime behavior

With `vector_backend: "redis"` (or `THE_ONE_VECTOR_TYPE=redis-vectors`)
and local embeddings enabled, the broker builds a Redis-backed
`MemoryEngine` and stores chunk/entity/relation vectors in the
configured RediSearch indexes. If `redis_persistence_required` is
true, the engine verifies Redis persistence state before any
ingest/search operation.

As of v0.14.2 there is **no silent fallback** to local-only vectors
when Redis is explicitly selected. Redis misconfiguration fails fast
so runtime behavior matches backend selection.

`vector_backend: "redis"` is currently paired with local embeddings.
If `embedding_provider` is set to `api`, broker initialization
returns a configuration error so this mismatch fails fast.

## Diagnostic playbook

If `RedisVectorStore` startup fails or vector searches return nothing:

1. **Persistence check failure** — `aof_enabled:1` is required when
   `redis_persistence_required=true`. Check `CONFIG GET appendonly`
   on the Redis instance.
2. **Missing FT index** — Run `FT._LIST` on the Redis CLI; the
   broker creates indexes lazily on first ingest. Check that the
   project ID matches.
3. **HNSW parameters** — Tune via `hnsw_m`, `hnsw_ef_construction`,
   `hnsw_ef_runtime` in `vector_redis` config (defaults: 16, 200,
   10). See [configuration.md](configuration.md) Redis vector
   section.
4. **500 ms phantom timeouts on v0.16.x** — Upgrade to v0.17.0; the
   facade removes the redis-rs `response_timeout` cap. See the
   [upgrade guide](upgrade-guide.md#upgrading-to-v0170-from-v0161--v0162)
   v0.17.0 section.

## Related guides

- [the-one-redis facade](the-one-redis-facade.md) — substrate layer
- [redis-state-backend.md](redis-state-backend.md) — Redis as
  `StateStore` (Phase 5)
- [combined-redis-backend.md](combined-redis-backend.md) — both
  trait roles, one shared `RedisPool` (Phase 6 / v0.17.0)
- [multi-backend-operations.md](multi-backend-operations.md) —
  deployment matrix
- [configuration.md](configuration.md) — every config field
