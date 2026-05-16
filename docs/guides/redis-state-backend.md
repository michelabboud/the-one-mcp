# Redis State Backend Guide

> **Status:** First-class — shipped in **v0.16.0-phase5** (`1dbf6a5`,
> tag `v0.16.0-phase5`).
> **Substrate refreshed** in **v0.17.0** — all Redis traffic now routes
> through the [the-one-redis facade](the-one-redis-facade.md).
>
> **Cargo feature:** `redis-state` (off by default — operators opt in at
> build time).
>
> **Env-var activation:** `THE_ONE_STATE_TYPE=redis` (or
> `redis-combined` for shared-pool mode) + `THE_ONE_STATE_URL=<dsn>`.

`RedisStateStore` is the Redis-backed third state-store backend
alongside SQLite (default) and Postgres (Phase 3). All 26 `StateStore`
trait methods land on Redis: project profiles via `HSET` objects;
audit events via Redis Streams; time-ordered listings via sorted sets
(`ZADD` / `ZREVRANGE`); diary FTS via the **RediSearch** module
(`FT.CREATE` / `FT.SEARCH`).

Two durability modes ship in v0.16.0 Phase 5:

- **Cache mode** (`require_aof=false`) — volatile. Data is lost on
  process or host restart. Useful for ephemeral dev environments.
- **Persistent mode** (`require_aof=true`) — boot-time AOF verification.
  The backend connects, runs `CONFIG GET appendonly`, and refuses to
  start unless `aof_enabled:1` is returned. This protects production
  deployments from accidentally pointing at a cache instance.

This guide covers installation, both durability modes, the FTS5 →
RediSearch translation, the schema migration runner, and
troubleshooting. For the full backend-selection scheme, see
[multi-backend-operations.md](multi-backend-operations.md). For the
combined Redis+RediSearch mode (shared pool), see
[combined-redis-backend.md](combined-redis-backend.md). For the
sibling Redis vector backend, see
[redis-vector-backend.md](redis-vector-backend.md).

---

## 1. Setup

### Persistent mode (production)

Requires Redis **8.0+** (or Redis 7 with the RediSearch module
loaded) with **AOF enabled**.

```bash
# Minimal local setup with AOF:
docker run --rm -d --name the-one-redis-state \
    -p 6379:6379 \
    redis/redis-stack-server:latest \
    redis-server --appendonly yes --appendfsync everysec

# Verify AOF is on (this is the same check the backend runs at startup):
redis-cli CONFIG GET appendonly
# Expected: 1) "appendonly"  2) "yes"

# Verify RediSearch is loaded:
redis-cli MODULE LIST | head
# Expected: includes "search"

# Configure the broker:
export THE_ONE_STATE_TYPE=redis
export THE_ONE_STATE_URL=redis://localhost:6379

# Rebuild with the feature:
cargo build --release -p the-one-mcp --bin the-one-mcp \
    --features redis-state
```

Add (optional, all fields have production-sane defaults) in
`<project>/.the-one/config.json`:

```json
{
  "state_redis": {
    "key_prefix": "the_one",
    "require_aof": true,
    "max_connections": 10,
    "min_connections": 2,
    "acquire_timeout_ms": 30000,
    "idle_timeout_ms": 600000
  }
}
```

### Cache mode (dev / ephemeral)

```bash
# Same Redis image without AOF:
docker run --rm -d -p 6379:6379 redis/redis-stack-server:latest

export THE_ONE_STATE_TYPE=redis
export THE_ONE_STATE_URL=redis://localhost:6379

# Set require_aof=false in config (or just omit state_redis — defaults
# to true; for cache mode you MUST explicitly opt out):
```

```json
{
  "state_redis": { "require_aof": false }
}
```

`require_aof: false` lets the broker boot against a cache Redis. Use
only for dev — your audit events, diary entries, and approvals will
not survive a restart.

---

## 2. Key layout

`RedisStateStore` lays out keys under the configured `key_prefix`
(default `the_one`):

| Data type | Redis structure | Key shape |
|---|---|---|
| Project profiles | `HSET` object | `{prefix}:{project_id}:profile` |
| Approvals | `HSET` object + ZADD index | `{prefix}:{project_id}:approval:{id}` + `{prefix}:{project_id}:approvals_by_time` |
| Audit events | Redis Stream | `{prefix}:{project_id}:audit` |
| Conversation sources | `HSET` object + ZADD index | `{prefix}:{project_id}:conv:{id}` + `{prefix}:{project_id}:convs_by_time` |
| AAAK lessons | `HSET` object + ZADD index + RediSearch tag | `{prefix}:{project_id}:lesson:{id}` |
| Diary entries | `HSET` object + ZADD index + RediSearch FT index | `{prefix}:{project_id}:diary:{id}` |
| Navigation nodes | `HSET` object | `{prefix}:{project_id}:nav_node:{id}` |
| Navigation tunnels | `HSET` object | `{prefix}:{project_id}:nav_tunnel:{from}:{to}` |

The single `:audit` stream per project is intentional: a stream gives
strict total ordering, server-side trimming via `MAXLEN`, and a clean
read pattern via `XREVRANGE` (added to the facade in v0.17.0 for
exactly this).

---

## 3. FTS5 → RediSearch translation

SQLite's FTS5 module supports `MATCH` queries over `content` columns.
On Postgres we translated that to `tsvector` + `GIN`. On Redis the
analog is **RediSearch** — `FT.CREATE` to declare an index over hash
fields, `FT.SEARCH` to run text + tag queries.

Diary entries get a RediSearch index named
`{prefix}_{project_id}_diary`:

```
FT.CREATE the_one_demo_diary
    ON HASH PREFIX 1 the_one:demo:diary:
    SCHEMA
      content   TEXT WEIGHT 1.0
      tags      TAG SEPARATOR |
      wing      TAG SEPARATOR |
      hall      TAG SEPARATOR |
      room      TAG SEPARATOR |
      ts        NUMERIC SORTABLE
```

Query for diary search:

```
FT.SEARCH the_one_demo_diary "@content:(<query>) @tags:{<tag>}" LIMIT 0 10 SORTBY ts DESC
```

**F1 v0.16.2 fix**: the tag-query escaper in `redis_query_escape`
covers `*` and `,` (added in v0.16.2). Diary tags or palace metadata
containing reserved RediSearch tokens are properly neutralised.

---

## 4. The substrate (v0.17.0 facade)

All Redis commands go through the [the-one-redis facade](the-one-redis-facade.md).
The load-bearing detail: the facade's
`pool::connection_config` sets `response_timeout = None`, so blocking
calls (`BLPOP`, `XREADGROUP`, long `FT.SEARCH`) honour their server-
side timeouts instead of being silently capped at 500 ms.

`RedisStateStore` exposes two constructors:

```rust
// Split mode — own pool, standalone backend:
pub async fn RedisStateStore::new(
    config: &RedisStateConfig,
    url: &str,
    project_id: &str,
) -> Result<Self, CoreError>;

// Combined mode — share an existing pool with RedisVectorStore:
pub async fn RedisStateStore::from_pool(
    pool: RedisPool,
    config: &RedisStateConfig,
    project_id: &str,
) -> Result<Self, CoreError>;
```

`from_pool` is what the combined-Redis cache uses
(`McpBroker::combined_redis_client_by_project`). See
[combined-redis-backend.md](combined-redis-backend.md).

---

## 5. The 26 `StateStore` trait methods

Every method delegates to the appropriate Redis primitive:

| Trait method | Redis primitive |
|---|---|
| `get_project_profile` / `set_project_profile` | `HGETALL` / `HSET` |
| `record_audit` | `XADD` to `:audit` stream |
| `list_audit_events` | **`XREVRANGE`** (newest-first; the facade extension) |
| `add_approval` / `get_approval` | `HSET` + `ZADD` / `HGETALL` |
| `list_approvals` | `ZREVRANGE` + batched `HGETALL` |
| `add_conversation_source` / `list_conversation_sources` | `HSET` + `ZADD` / `ZREVRANGE` |
| `record_aaak_lesson` / `list_aaak_lessons` | `HSET` + `ZADD` + `FT.SEARCH` (tag) |
| `upsert_diary_entry` / `search_diary` | `HSET` + `FT.SEARCH` |
| `upsert_navigation_node` / `get_navigation_node` | `HSET` / `HGETALL` |
| `link_navigation_tunnel` / `list_navigation_tunnels` | `HSET` / `KEYS` + `HGETALL` |

The full list of 26 methods (including paged variants) is in
`crates/the-one-core/src/state_store.rs`.

---

## 6. Schema migration runner

Like the Postgres backend, `RedisStateStore` ships a hand-rolled
migration runner under `the_one_core::storage::redis::migrations`.
First boot creates the necessary RediSearch indexes (`FT.CREATE` is
idempotent via `IF NOT EXISTS` semantics in our wrapper). Subsequent
boots verify and exit clean.

Migrations track in a Redis hash at `{prefix}:_state_migrations`:

```
HGETALL the_one:_state_migrations
1) "0000_init"
2) "applied=2026-04-19T13:22:01Z;version=7"
```

If you ever need to wipe and re-apply (dev only):

```bash
redis-cli DEL the_one:_state_migrations
redis-cli FT.DROPINDEX the_one_demo_diary  # repeat per project
```

---

## 7. AOF enforcement details

When `require_aof=true`, startup runs:

```
CONFIG GET appendonly
```

…and refuses to proceed unless the reply is `aof_enabled:1` (or
literal `yes`). The `verify_aof(&RedisPool)` helper in
`storage/redis.rs` is wrapped in `Result<(), CoreError>` and
propagates parse errors as `CoreError::Redis("aof verification: …")`
so the failure surfaces with a clear correlation ID in the audit
log.

In v0.17.0 the helper takes `&RedisPool` (was `&fred::Client`) — see
the [upgrade guide § v0.17.0](upgrade-guide.md#upgrading-to-v0170-from-v0161--v0162)
for the signature delta.

---

## 8. Pool sizing

The defaults are:

- `max_connections: 10`
- `min_connections: 2`
- `acquire_timeout_ms: 30000` (30s)
- `idle_timeout_ms: 600000` (10 min)

For combined-mode deployments (Phase 6), `state_redis` pool sizing
**wins** — the `vector_redis` URL is still required and must be
byte-identical, but its pool-sizing fields are ignored on the shared
pool. See [combined-redis-backend.md](combined-redis-backend.md).

---

## 9. Testing

Integration tests live in `crates/the-one-core/tests/redis_state_roundtrip.rs`
(7 tests covering project profile, audit, approvals, diary FTS, AAAK
lessons, navigation). Env-gated on `THE_ONE_STATE_TYPE=redis` +
`THE_ONE_STATE_URL`. Skip via `return` when env vars are missing, so
the suite stays green by default and runs only when a Redis
container is provisioned.

```bash
# With a running Redis container at localhost:6379:
THE_ONE_STATE_TYPE=redis \
    THE_ONE_STATE_URL=redis://localhost:6379 \
    cargo test -p the-one-core --features redis-state --test redis_state_roundtrip -- --test-threads=1
```

The `--test-threads=1` matters: tests share the same Redis namespace
and use a `wipe()` helper between cases.

---

## 10. Troubleshooting

### "AOF verification failed" at startup

`CONFIG GET appendonly` returned anything other than `yes`. Either:

- Enable AOF on Redis: `redis-cli CONFIG SET appendonly yes` then
  trigger a rewrite via `BGREWRITEAOF`. Note this is **runtime
  only** — also add `appendonly yes` to `redis.conf` so it survives
  restart.
- Or set `state_redis.require_aof=false` if you intentionally want
  cache mode (dev only).

### "RediSearch module not loaded"

Redis < 8.0 needs the module loaded explicitly. Either:

- Use Redis 8.0+ (RediSearch is in core).
- Use the Redis Stack image: `redis/redis-stack-server:latest`.
- Load manually: `redis-server --loadmodule /path/to/redisearch.so`.

### `FT.SEARCH` returns empty for diary search

Likely the FT index wasn't created. Index creation is lazy on first
diary write. Run:

```
FT._LIST
```

You should see `the_one_<project_id>_diary` per project. If the
index isn't there, ingest at least one diary entry and re-check.

### 500 ms phantom timeouts (pre-v0.17.0)

Upgrade. The v0.17.0 facade sets `response_timeout = None`. See the
[upgrade guide § v0.17.0](upgrade-guide.md#upgrading-to-v0170-from-v0161--v0162).

---

## 11. Related guides

- [the-one-redis facade](the-one-redis-facade.md) — the substrate
- [redis-vector-backend.md](redis-vector-backend.md) — Phase 7 sibling
- [combined-redis-backend.md](combined-redis-backend.md) — shared
  pool for state + vectors
- [postgres-state-backend.md](postgres-state-backend.md) — Phase 3
  cross-axis sibling
- [multi-backend-operations.md](multi-backend-operations.md) — full
  matrix
- [configuration.md](configuration.md) — every config field
