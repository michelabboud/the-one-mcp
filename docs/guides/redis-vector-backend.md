# Redis Vector Backend

The-one keeps Qdrant as the default vector backend. Redis is an optional
backend surface for teams that want RediSearch HNSW indexes backed by Redis
persistence expectations and stable index naming.

## Persistence requirements

- Enable both RDB snapshots and AOF.
- Use `appendfsync everysec` or a stricter policy in environments where
  vector durability matters.
- If `redis_persistence_required` is enabled in the-one config, treat missing
  persistence as a startup-time misconfiguration instead of a best-effort mode.

See [`config/redis.conf.example`](../../config/redis.conf.example) for a minimal
baseline.

## RediSearch requirement

- The Redis backend depends on RediSearch for `FT.CREATE`, `FT.SEARCH`, and
  the HNSW vector field.
- Redis Stack already includes RediSearch.
- Module-based Redis installs must load the RediSearch module before the-one
  connects.

## Backend selection

Use the existing project config file at `.the-one/config.json` to switch
backends:

```json
{
  "vector_backend": "redis",
  "redis_url": "redis://127.0.0.1:6379",
  "redis_index_name": "the_one_memories",
  "redis_persistence_required": true
}
```

Expected behavior:

- Omit `vector_backend` to stay on the default `qdrant` backend.
- Set `vector_backend` to `redis` only when RediSearch is available.
- Keep `redis_index_name` stable for a given deployment so the same HNSW
  index is reused across restarts.
- Leave `redis_url` unset unless Redis is the selected backend.

Persistence expectations:

- Redis durability is your responsibility. Keep both RDB and AOF enabled if
  you expect vectors to survive process or host restarts.
- `redis_persistence_required: true` expresses that expectation in config even
  when Redis is managed outside this repository.
- Reusing the same `redis_index_name` against a wiped Redis instance means the
  index name is reused, not that vectors are restored automatically. Restore
  from Redis persistence or reindex your project.

## Recommended setup flow

1. Start Redis with RediSearch available.
2. Enable both RDB snapshots and AOF.
3. Put the Redis backend fields in `.the-one/config.json`.
4. Reindex after switching backends so vectors are regenerated for the target
   store.

Minimal example:

```json
{
  "vector_backend": "redis",
  "redis_url": "redis://127.0.0.1:6379",
  "redis_index_name": "the_one_memories",
  "redis_persistence_required": true,
  "embedding_provider": "local"
}
```

## Runtime behavior

With `vector_backend: "redis"` and local embeddings enabled, the broker builds
a Redis-backed `MemoryEngine` and stores chunk vectors in the configured
RediSearch index. If `redis_persistence_required` is true, the engine verifies
Redis persistence state before Redis-backed ingest/search operations.

As of v0.14.2 there is no silent fallback to local-only vectors when Redis is
explicitly selected. Redis misconfiguration fails fast so runtime behavior
matches backend selection.

`vector_backend: "redis"` is currently paired with local embeddings. If
`embedding_provider` is set to `api`, broker initialization returns a
configuration error so this mismatch fails fast.
