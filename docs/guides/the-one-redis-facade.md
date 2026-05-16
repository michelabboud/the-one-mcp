# the-one-redis — Redis Facade Crate (v0.17.0+)

> The single chokepoint for every Redis command the workspace makes.
> Built on `redis-rs 1.2`. Wholesale port of the sibling project
> `mai-redis` on 2026-05-16. **Replaces `fred 10` entirely.**

---

## 1. Why this crate exists

Direct `redis-rs` usage works but exposes the workspace to three
classes of foot-guns:

1. **Driver quirks**. `redis::aio::ConnectionManager` defaults
   `response_timeout` to **500 ms**. That's fine for `GET`/`SET`, fatal
   for `BLPOP`/`XREADGROUP`/long `FT.SEARCH` where the server-side
   blocking timeout is the contract and the client-side cap silently
   shadows it. The workspace silently hit this quirk on the prior
   `fred 10` substrate; MAI's retrospective at
   `~/projects/mai/docs/guides/fred-retrospective.md` catalogues it as
   fred bug #4 (one of 11 bugs that motivated MAI's substrate swap).
2. **Untyped replies**. `FT.SEARCH` returns nested `Vec<Value>` that
   the caller has to decode by hand. Easy to get wrong, hard to test,
   noisy at call sites.
3. **Cycles**. If every Redis-using crate depends on `redis-rs`
   directly, a future workspace-wide upgrade or substrate swap is a
   per-call-site edit.

`the-one-redis` solves all three: typed surface, single override of
the dangerous default, and one place to swap drivers.

`★ Design principle ────────────────────────────`
- **Domain-shaped API on top of `redis-rs`, NOT a `redis-rs` clone.**
  The facade exposes `RedisStateStore` patterns (`hset`, `hgetall`,
  `xrevrange`, `ft_search`) directly — no leaky `redis::cmd("HSET")`
  call sites in the rest of the workspace.
- **Cycle-free.** The facade depends on `redis = 1.2`, `tokio`,
  `thiserror`, `tracing`, `futures`, `serde`, `serde_json`,
  `async-trait`. **No `the-one-core` dep** — that lets
  `the-one-core`'s `redis-state` feature pull this crate in without
  forming a dep cycle.
- **Boundary-mapped errors.** `RedisError` is the facade's own enum.
  Callers map at the boundary: `.map_err(|e| CoreError::Redis(e.to_string()))`.
`─────────────────────────────────────────────`

## 2. Provenance: where the code comes from

This crate is a **wholesale port** of `mai-redis` (sibling project,
same maintainer). The port happened on **2026-05-16** as one atomic
migration, replacing `fred 10` in three modules:

- `crates/the-one-core/src/storage/redis.rs` — `RedisStateStore`
- `crates/the-one-memory/src/redis_vectors.rs` — `RedisVectorStore`
  (chunks + entities + relations)
- `crates/the-one-mcp/src/broker.rs` — combined-Redis shared-pool
  cache

The mai-redis source was copied module-for-module with four
documented deviations (see [Deviations from upstream](#5-deviations-from-upstream)).
The diff is **~+58 LOC net** over upstream mai-redis, almost entirely
driven by adding `XREVRANGE` (needed by `RedisStateStore::list_audit_events`).

Migration plan: [`docs/plans/2026-05-16-fred-removal-and-bug-fixes.md`](../plans/2026-05-16-fred-removal-and-bug-fixes.md).

## 3. Module layout

```
crates/the-one-redis/src/
├── pool.rs           # RedisPool (the load-bearing module)
├── keys.rs           # GET/SET/DEL/MGET/MSET/KEYS/SCAN + TTL family
├── hashes.rs         # HSET/HGET/HGETALL/HMGET/HEXISTS/HINCRBY
├── lists.rs          # LPUSH/RPUSH/LPOP/RPOP/BLPOP/BRPOP/LRANGE/LLEN
├── sets.rs           # SADD/SMEMBERS/SISMEMBER/SREM/SCARD
├── sorted_sets.rs    # ZADD/ZRANGE/ZREVRANGE/ZRANGEBYSCORE/ZREMRANGEBYRANK/ZSCORE/ZINCRBY
├── streams.rs        # XADD/XREAD/XREADGROUP/XAUTOCLAIM/XACK/XPENDING/XLEN/XDEL/XRANGE/XREVRANGE
├── pubsub.rs         # PUBLISH/SUBSCRIBE/PSUBSCRIBE (dedicated connection)
├── timeseries.rs     # TS.CREATE/TS.ADD/TS.RANGE
├── search.rs         # RediSearch typed surface (FT.CREATE/FT.SEARCH/FT.ALTER)
├── error.rs          # RedisError + RedisResult<T>
└── lib.rs            # module re-exports
```

Total: **12 modules, ~2,700 LOC, 27 sentinel tests.**

## 4. The load-bearing fix: `response_timeout = None`

The single most important line in the whole crate lives in
`pool.rs`:

```rust
pub(crate) fn connection_config() -> AsyncConnectionConfig {
    AsyncConnectionConfig::default().set_response_timeout(None)
}
```

**Why this matters**: redis-rs's `AsyncConnectionConfig::default()`
sets `response_timeout = Duration::from_millis(500)`. That's a
client-side **hard cap** on every reply. For `GET`/`SET`/`HGET` it's
fine. For:

- `BLPOP key 10` (server-side 10-second blocking) — the client cancels
  the future after 500 ms, surfaces a phantom `Timeout`, and Redis is
  left with an orphaned blocked client until the server-side timeout
  hits.
- `XREADGROUP GROUP grp consumer COUNT n BLOCK 30000` — same story.
  Audit-log tails on the prior fred substrate were silently capped at
  500 ms.
- `FT.SEARCH index "*=>[KNN ...]"` against a 10M-vector index — the
  search completes server-side in ~2s, but the client gives up at
  500 ms with `Timeout`. The caller never sees the result.

Setting `response_timeout = None` means **server-side blocking
timeouts are the contract**. No silent client-side shadowing. This is
not a performance optimization — it's a correctness fix.

The sentinel test `tests/response_timeout_default.rs` locks this
contract in CI. If a future `redis-rs` upgrade or refactor flips the
default back, the test fails loud.

## 5. Deviations from upstream (mai-redis)

Four intentional changes, all documented in source:

| Change | Where | Why |
|---|---|---|
| Rebrand `mai-redis` → `the-one-redis`, version `0.1.0` → `0.16.2` | `Cargo.toml` + doc comments | Project identity |
| **Dropped `From<RedisError> for MaiError`** + `use mai_types::error::MaiError` | `error.rs` (−3 LOC) | Cycle-break: `the-one-core` has optional dep on this facade via the `redis-state` feature, so the facade cannot depend on `the-one-core`. Callers map at the boundary |
| **Dropped deps**: `mai-types`, `metrics`, `chrono`, `uuid` | `Cargo.toml` | `mai-types` is the cycle. `chrono` violates the workspace rule (cargo `links` conflict with `rusqlite 0.39`). `metrics`/`uuid` weren't reached by any code path migrated |
| **Added `xrevrange` + `parse_xrange_reply`** (+80 LOC) | `streams.rs` | Needed by `RedisStateStore::list_audit_events` for newest-first audit pagination without a consumer group — mai-redis only ever used `XREAD`/`XREADGROUP` |

Other small diffs (rustfmt rewraps in `hashes.rs`, `pubsub.rs`,
`search.rs`, `sorted_sets.rs`) are pure formatting drift between
toolchains — zero behavioural change.

## 6. Cycle-break: why no `the-one-core` dep

The facade is pulled in by **multiple** workspace crates through
feature flags:

- `the-one-core/Cargo.toml` has `the-one-redis = { ..., optional = true }`
  behind the `redis-state` feature (gates `RedisStateStore`).
- `the-one-memory/Cargo.toml` has the same setup behind `redis-vectors`
  (gates `RedisVectorStore`).
- `the-one-mcp/Cargo.toml` has it as a direct optional dep behind both
  features (gates the combined-mode shared-pool wiring in `broker.rs`).

If `the-one-redis` depended on `the-one-core` for its error type (so
`RedisError` could carry a `CoreError` variant), the graph would be:

```
the-one-core (with redis-state) ─→ the-one-redis ─→ the-one-core  ❌ cycle
```

Cargo would refuse to compile. The fix is to keep the facade
independent: `RedisError` is its own enum, and **callers** map at the
boundary:

```rust
// In the-one-core/src/storage/redis.rs:
let value: String = pool.hashes().hget(key, field)
    .await
    .map_err(|e| CoreError::Redis(e.to_string()))?;
```

Trade-off: ~30 call sites across the workspace each carry one extra
`.map_err(...)` line. In exchange the facade is reusable by any
future workspace crate without dep-graph surgery.

`★ Insight ─────────────────────────────────────`
- The boundary-mapping pattern is **structurally** equivalent to
  what `the-one-memory` already does for its `MemoryEngine` (which
  returns `Result<T, String>` internally and gets mapped to
  `CoreError::Embedding` at the broker boundary). The facade just
  picks a typed error enum instead of `String`.
`─────────────────────────────────────────────`

## 7. The typed RediSearch surface

`redis-rs 1.2` doesn't ship typed `FT.*` wrappers (unlike fred's
`i-redisearch` feature or `rustis`'s built-in support). `search.rs`
owns the typed surface for the workspace.

```rust
use the_one_redis::search::{Query, SchemaField, VectorAlgorithm, DistanceMetric};

// FT.CREATE with HNSW vector schema:
pool.search().ft_create(
    "the_one_chunks_demo",
    &FtCreateOptions {
        on_json: false,
        prefixes: vec!["chunk:demo:".into()],
    },
    &[
        SchemaField::Text { name: "content".into(), weight: None, sortable: false },
        SchemaField::Tag { name: "project_id".into(), sortable: true, separator: Some('|') },
        SchemaField::Vector {
            name: "embedding".into(),
            algorithm: VectorAlgorithm::Hnsw,
            dim: 1024,
            distance_metric: DistanceMetric::Cosine,
            initial_cap: None,
        },
    ],
).await?;

// FT.SEARCH with KNN:
let raw_query = format!("(@project_id:{{demo}})=>[KNN {} @embedding $BLOB AS score]", top_k);
let query = Query::new(&raw_query)
    .return_fields(["score"])
    .param("BLOB", query_blob)
    .limit(0, top_k)
    .dialect(2);

let reply = pool.search().ft_search("the_one_chunks_demo", &query).await?;
for hit in &reply.hits {
    println!("{} → score={}", hit.id, hit.fields.get("score").unwrap_or(&"".into()));
}
```

The `Query` builder hides RediSearch's RESP-shaped argument list and
the binary `PARAMS BLOB` quoting. `SearchReply` decodes the typed
reply into `Vec<SearchHit>`.

## 8. Sentinel-test suite (CI contract)

Five sentinel tests live in `crates/the-one-redis/tests/`. Each
guards one specific contract — they're worth understanding as a unit:

| Test | Guards |
|---|---|
| `response_timeout_default.rs` | The `None` override. Without this, every blocking command silently caps at 500 ms. |
| `blpop_timeout.rs` | `BLPOP key 1` returns `Ok(None)` on server-side timeout, not `Err(Timeout)`. (Fred bug #4 manifestation.) |
| `dedicated_conn_for_blocking_commands.rs` | Pub/sub and blocking commands get dedicated connections — they don't starve the multiplexed connection. |
| `sentinel_ops.rs` | Redis Sentinel failover ops (`SENTINEL master`, `SENTINEL get-master-addr-by-name`). |
| `streams_nil.rs` | `XREADGROUP` empty-reply decode. Fred's typed convert had a bug where populated responses sometimes failed; redis-rs 1.2 handles the nil case natively. |

All 27 tests across the suite are env-gated on `THE_ONE_REDIS_URL`
and skip gracefully via `return` when no Redis is present. CI cells
that have a Redis service container running execute the full suite.

## 9. Migration audit: three purity gates

After the fred → the-one-redis migration, three gates are run on
every release to confirm fred is fully gone:

```bash
# Gate 1: source-level reference
grep -rn 'fred::\|use fred\|fred = ' crates/ Cargo.toml | grep -v "docs/\|//\|#"
# Expected: empty

# Gate 2: resolved dep graph
cargo tree -e all --workspace --all-features | grep fred
# Expected: empty

# Gate 3: lockfile
grep -c '^name = "fred' Cargo.lock
# Expected: 0
```

If any gate returns non-empty/non-zero on a release branch, the
release is held until the regression is identified and the gate
returns clean. See `docs/plans/2026-05-16-fred-removal-and-bug-fixes.md
§ Purity gates` for the rationale.

## 10. Public API summary (for crate users)

```rust
use the_one_redis::pool::RedisPool;

// Construct (async; sets response_timeout=None internally)
let pool = RedisPool::from_url("redis://localhost:6379").await?;

// Access typed sub-clients
pool.keys()         // string KV
pool.hashes()       // HSET/HGET
pool.lists()        // LPUSH/BLPOP
pool.sets()         // SADD/SMEMBERS
pool.sorted_sets()  // ZADD/ZRANGE
pool.streams()      // XADD/XREAD/XREVRANGE
pool.pubsub()       // SUBSCRIBE/PUBLISH
pool.timeseries()   // TS.*
pool.search()       // FT.CREATE/FT.SEARCH

// Error mapping at the boundary
match pool.hashes().hget::<String>("foo", "bar").await {
    Ok(Some(v)) => /* ... */,
    Ok(None) => /* missing field */,
    Err(e) => return Err(CoreError::Redis(e.to_string())),
}
```

## 11. Troubleshooting

### "Connection timeout after 500ms" on blocking commands

You're on v0.16.x or earlier. Upgrade to v0.17.0 — the facade
overrides this default. See the
[upgrade guide § v0.17.0](upgrade-guide.md#upgrading-to-v0170-from-v0161--v0162).

### `cargo build` fails with "cyclic package dependency"

You added a `the-one-core` dep to the facade. **Don't.** The cycle-
break is the architectural choice. Map errors at the boundary via
`.map_err(|e| CoreError::Redis(e.to_string()))` in the caller.

### FT.SEARCH returns empty

Three common causes:

1. Index doesn't exist yet. `FT.CREATE` is lazy on first ingest; run
   `FT._LIST` on the Redis CLI to verify.
2. Project ID mismatch. Index naming is per-project
   (`the_one_chunks_<project_id>`). Check your config.
3. `dialect(2)` missing from the `Query`. KNN requires DIALECT 2;
   the builder defaults it.

### Sentinel tests don't run

They're env-gated. Set `THE_ONE_REDIS_URL=redis://localhost:6379`
(or your Sentinel master) and run:

```bash
cargo test -p the-one-redis --features sentinel-tests   # if feature gated
# or just
THE_ONE_REDIS_URL=redis://localhost:6379 cargo test -p the-one-redis
```

## 12. Related guides

- [redis-state-backend.md](redis-state-backend.md) — `RedisStateStore`
  (Phase 5 / v0.17.0): cache + persistent modes
- [redis-vector-backend.md](redis-vector-backend.md) —
  `RedisVectorStore` (Phase 7 / v0.17.0): chunks/entities/relations
- [combined-redis-backend.md](combined-redis-backend.md) — combined
  state + vector (Phase 6 / v0.17.0): one shared `RedisPool`
- [multi-backend-operations.md](multi-backend-operations.md) — full
  backend matrix
- [architecture.md](architecture.md) — workspace layout, dep graph
- [Migration plan](../plans/2026-05-16-fred-removal-and-bug-fixes.md)
