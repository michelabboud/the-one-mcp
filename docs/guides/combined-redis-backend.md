# Combined Redis+RediSearch Backend

**Target version:** v0.16.0-phase6+; substrate refreshed in v0.17.0
**Feature flags:** `redis-state` + `redis-vectors` (both required)
**Activation:** `THE_ONE_STATE_TYPE=redis-combined`
              + `THE_ONE_VECTOR_TYPE=redis-combined`
              + byte-identical `THE_ONE_STATE_URL` and
                `THE_ONE_VECTOR_URL`

Phase 6 of the v0.16.0 multi-backend roadmap. Ships the second
*combined single-pool* backend (after Phase 4's Postgres combined):
one shared `RedisPool` — from the v0.17.0
[the-one-redis facade](the-one-redis-facade.md) — serving both the
`StateStore` trait role (audit, diary, navigation, approvals, AAAK
lessons, conversation sources, project profiles) **and** the
`VectorBackend` trait role (chunks, entities, relations) against a
single Redis instance.

> Looking for the split variant where state and vector each get their
> own pool (and can even live on different Redis instances)?
> See [`redis-state-backend.md`](redis-state-backend.md) and
> [`redis-vector-backend.md`](redis-vector-backend.md) — both still
> ship and are the right pick when you want different credentials,
> different AOF policies, or different deployment lifecycles per
> trait role.

---

## 1. When to pick combined over split

| Priority                                            | Pick          |
|-----------------------------------------------------|---------------|
| One credential to rotate, one IAM token, one TLS cert | **combined** |
| One AOF / RDB window covers state AND vectors       | **combined**  |
| Lowest connection footprint                         | **combined**  |
| Operational unity (one Redis to monitor, one OOM page)  | **combined** |
| Different AOF policy per trait role                 | split         |
| State on a managed Redis, vectors on Redis Stack    | split         |
| Vector-heavy workload that would starve state queries | split       |
| Different memory budgets per role                   | split         |

The dominant reason to pick combined is **operational unity**: one
Redis is one credential, one health check, one PITR target. The
dominant reason to pick split is **independence**: two Redis
instances are two memory budgets, two failure domains, and two sets
of knobs.

---

## 2. What "combined" actually means

A combined Redis deployment does **not** merge the two trait roles
into a new named backend type. Under the hood, the broker still holds
a `RedisStateStore` in its state cache and a `RedisVectorStore` in
its vector cache — they just share a **single underlying `RedisPool`**
instead of each constructing its own.

This is the same refined Option Y pattern Phase 4 used for Postgres:

```
McpBroker
├── state_by_project: { project_id → Arc<Mutex<Box<dyn StateStore>>> }
│                            │
│                            ▼  RedisStateStore::from_pool(pool, ...)
│
├── memory_by_project: { project_id → Arc<RwLock<HashMap<.., MemoryEngine>>> }
│                            │
│                            ▼  RedisVectorStore::new(pool, ...)
│
└── combined_redis_client_by_project: { project_id → RedisPool }   ← shared cache
                            │
                            ▼
                       fred::Client (pre-v0.17.0)
                       RedisPool (v0.17.0+, internally Arc-counted)
```

Both `RedisStateStore::from_pool` and `RedisVectorStore::new` receive
**clones** of the same `RedisPool`. The pool's internal connection
manager is `Arc`-counted, so `pool.clone()` is a cheap refcount bump.
Both sub-backends hold a handle to the same underlying connection
set.

`★ Insight ─────────────────────────────────────`
- Pre-v0.17.0 this worked because `fred::Client` was internally
  `Arc`-counted. Post-v0.17.0 the facade's `RedisPool` retains the
  same cheap-clone semantics by wrapping
  `redis::aio::ConnectionManager` (also `Arc`-counted). The combined
  pattern survives the substrate swap without a redesign.
- Why a cache, not a static? Project isolation: each project gets its
  own pool keyed on `{canonical_root}::{project_id}`, so two different
  projects can target different Redis URLs.
`─────────────────────────────────────────────`

---

## 3. Activation

Three things together:

1. **Build with both features**:
   ```bash
   cargo build --release -p the-one-mcp --bin the-one-mcp \
       --features redis-state,redis-vectors
   ```

2. **Set the four env vars** (both `TYPE` axes must be
   `redis-combined`; both `URL` axes must be byte-identical):
   ```bash
   export THE_ONE_STATE_TYPE=redis-combined
   export THE_ONE_VECTOR_TYPE=redis-combined
   export THE_ONE_STATE_URL=redis://localhost:6379
   export THE_ONE_VECTOR_URL=redis://localhost:6379   # byte-identical
   ```

3. **Ensure the Redis has both AOF and RediSearch**:
   ```bash
   redis-cli CONFIG GET appendonly         # → "yes"
   redis-cli MODULE LIST | grep search     # → search module loaded
   ```

   Redis 8.0+ ships RediSearch in core. For older Redis, use the
   Redis Stack image:
   ```bash
   docker run --rm -d -p 6379:6379 \
       redis/redis-stack-server:latest \
       redis-server --appendonly yes --appendfsync everysec
   ```

If any of the three conditions are wrong, the broker fails loud at
startup with an `InvalidProjectConfig` carrying the exact offending
value (see env-var parser at
`crates/the-one-core/src/config/backend_selection.rs`).

---

## 4. The byte-identical URL constraint

The env-var parser enforces that when **either** axis is set to
`*-combined`, **both** axes must be:

- Set to the same `*-combined` value (no mixing combined with split)
- Carry byte-identical URLs (no `localhost` ≠ `127.0.0.1`, no
  whitespace, no trailing slash differences)

Why byte-identical? It's the simplest invariant the parser can check
without an authoritative URL parser, and it matches operator intent
("I want one Redis"). If you need two different Redis instances, use
the split mode.

If the URLs differ by even one character, startup fails with:

```
InvalidProjectConfig: THE_ONE_STATE_URL and THE_ONE_VECTOR_URL must be
byte-identical when THE_ONE_STATE_TYPE=redis-combined and
THE_ONE_VECTOR_TYPE=redis-combined.
state=redis://localhost:6379
vector=redis://127.0.0.1:6379
```

---

## 5. Pool-sizing rule (state side wins)

On combined deployments, the **state-side** config wins for pool
sizing:

- `state_redis.max_connections`, `min_connections`, `acquire_timeout_ms`,
  `idle_timeout_ms` all apply to the shared pool.
- `state_redis.require_aof` triggers the AOF check at startup.
- `vector_redis`'s pool-sizing fields are **ignored** on the shared
  pool — they would have applied in split mode but don't here.

HNSW tuning (`hnsw_m`, `hnsw_ef_construction`, `hnsw_ef_runtime`) is
NOT a pool setting — those still come from `vector_redis` because
they're migration-time + query-time settings.

Why state wins: the state side has the larger surface (26 trait
methods including blocking audit reads), so its pool needs are
typically the bigger constraint. Operators sizing for combined mode
should treat the state pool budget as the binding constraint and
verify it's enough headroom for vector queries on top.

---

## 6. Activation walkthrough

```bash
# 1. Start Redis with AOF + RediSearch:
docker run --rm -d --name the-one-redis \
    -p 6379:6379 \
    redis/redis-stack-server:latest \
    redis-server --appendonly yes --appendfsync everysec

# 2. Configure env:
export THE_ONE_STATE_TYPE=redis-combined
export THE_ONE_VECTOR_TYPE=redis-combined
export THE_ONE_STATE_URL=redis://localhost:6379
export THE_ONE_VECTOR_URL=redis://localhost:6379

# 3. Build with both features:
cargo build --release -p the-one-mcp --bin the-one-mcp \
    --features redis-state,redis-vectors

# 4. Run:
target/release/the-one-mcp serve

# Expected startup log lines:
# INFO RedisStateStore: AOF verified (aof_enabled=1)
# INFO RedisVectorStore: FT.CREATE the_one_chunks_<project> OK
# INFO McpBroker: backend selection = state=redis-combined vector=redis-combined
```

---

## 7. What you get vs split mode

| Aspect | Split (`redis` + `redis-vectors`) | Combined (`redis-combined`) |
|---|---|---|
| Number of Redis instances | 1 or 2 (operator choice) | exactly 1 |
| Number of `RedisPool` instances per project | 2 | **1 (shared)** |
| Number of `redis-cli AUTH` credentials | 1 or 2 | 1 |
| AOF rewrite frequency | 1 or 2 instances to coordinate | 1 to coordinate |
| Memory budget | independent | shared (one OOM page) |
| FT.SEARCH on the same instance as state HSETs | optional | required |
| Connection footprint | `state.max + vector.max` | `state.max` only |
| Fail-loud env validation | URLs can differ | URLs must be byte-identical |

---

## 8. The substrate (v0.17.0 facade)

All Redis commands go through the [the-one-redis facade](the-one-redis-facade.md).
The shared `RedisPool` is constructed once per project on the cold
path and cached on `McpBroker::combined_redis_client_by_project`.

The facade's **load-bearing fix** — `response_timeout = None` in
`pool::connection_config` — applies uniformly to both trait roles on
the combined pool. Blocking commands on the state side
(`XREADGROUP` audit tail) and long FT.SEARCH calls on the vector side
both honour server-side timeouts instead of being silently capped at
500 ms.

Pre-v0.17.0 the shared client type was `fred::Client`. In v0.17.0
the migration replaced this with the facade's `RedisPool`. Operator-
visible config and behaviour are unchanged; the substrate is
different.

---

## 9. Cross-trait operations

Phase 6 ships with **no** `begin_combined_tx()` cross-trait
transaction method. Considered but deferred — no call site needed it,
and Redis transactions (`MULTI`/`EXEC`) don't compose cleanly across
the two trait roles since they share a connection pool, not a
single connection. If a future workload needs atomic state+vector
updates, the design would be a method on the combined backend type
(itself currently not introduced — see the Option Y rationale
above) that pulls a single connection from the pool, runs
`MULTI`/`EXEC` on it, and returns.

---

## 10. Migration: split → combined

There is **no automated tool**. The migration is:

1. Run the broker against the split-pool Redis once to drain pending
   work and let the audit stream stabilize.
2. Stop the broker.
3. Re-export the env vars to `redis-combined` mode with byte-identical
   URLs (the state URL becomes the shared URL).
4. Restart the broker.

Since both modes use the **same data layout** under the same
`key_prefix`, no data migration is necessary. The broker simply picks
up the existing keys via the shared pool.

The only caveat: if you previously ran with **different** Redis
instances for state vs vectors, you need to first move the data into
one instance (`MIGRATE` or dump/restore) before flipping to combined.

---

## 11. Testing

Integration tests live in
`crates/the-one-mcp/tests/redis_combined_roundtrip.rs` (5 tests
covering shared-pool identity, AOF verification, diary FTS, vector
search, and combined-mode shutdown ordering). Gated on:

- Cargo features `all(redis-state, redis-vectors)` via `#[cfg]`
- Env vars `THE_ONE_STATE_TYPE=redis-combined` +
  `THE_ONE_VECTOR_TYPE=redis-combined` + byte-identical URLs

Skip gracefully via `return` when env vars are missing.

```bash
THE_ONE_STATE_TYPE=redis-combined \
THE_ONE_VECTOR_TYPE=redis-combined \
THE_ONE_STATE_URL=redis://localhost:6379 \
THE_ONE_VECTOR_URL=redis://localhost:6379 \
    cargo test -p the-one-mcp --features redis-state,redis-vectors \
        --test redis_combined_roundtrip -- --test-threads=1
```

---

## 12. Troubleshooting

### Startup fails: "byte-identical URLs required"

Your two URLs differ — even by whitespace or scheme. Make them
literally byte-identical:

```bash
diff <(echo -n "$THE_ONE_STATE_URL") <(echo -n "$THE_ONE_VECTOR_URL")
# Expected: no output (no diff)
```

### Startup fails: "AOF verification failed"

`require_aof=true` (the default) requires Redis to have AOF on.
Either:

- Enable AOF: `redis-cli CONFIG SET appendonly yes` then add
  `appendonly yes` to `redis.conf` for persistence across restart.
- Set `state_redis.require_aof=false` to opt into cache mode (dev
  only).

### Vector queries are slow on the shared pool

Tune in this order:

1. Increase `state_redis.max_connections` (state side wins on
   combined). Default is 10; production workloads with vector
   queries on top often want 20–40.
2. Tune HNSW `hnsw_ef_runtime` in `vector_redis` (lower = faster,
   less accurate; default 10).
3. If consistently saturated, consider switching to split mode so
   vector and state get independent pool budgets.

### Shutdown hangs

The broker drains the shared-pool cache before clearing state
caches. If shutdown hangs, check that all sessions have ended and no
long FT.SEARCH calls are in flight. v0.17.0's `response_timeout =
None` means client-side won't preempt — the server-side timeout (or
`SHUTDOWN NOSAVE` on Redis) is the contract.

### Pre-v0.17.0 phantom timeouts at 500 ms

Upgrade. The facade overrides redis-rs's default. See the
[upgrade guide § v0.17.0](upgrade-guide.md#upgrading-to-v0170-from-v0161--v0162).

---

## 13. Related guides

- [the-one-redis facade](the-one-redis-facade.md) — the substrate
- [redis-state-backend.md](redis-state-backend.md) — split-pool state
- [redis-vector-backend.md](redis-vector-backend.md) — split-pool
  vectors
- [combined-postgres-backend.md](combined-postgres-backend.md) —
  Phase 4 sibling (same refined Option Y pattern)
- [multi-backend-operations.md](multi-backend-operations.md) — full
  matrix
- [configuration.md](configuration.md) — every config field
