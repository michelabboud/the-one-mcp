# Fred Removal + Audit Fix Catalogue

**Date:** 2026-05-16
**Status:** Plan — pending implementation start (authorisation: user requested
"copy the fix code from mai and refactor fred").
**Target release:** v0.16.2 (fixes) + v0.17.0 (fred removal + facade).
**Related:**
- `docs/guides/production-hardening-v0.15.md` — prior hardening pass.
- `/home/michel/projects/mai/docs/guides/fred-retrospective.md` — MAI's 11-bug
  catalogue and the rationale for replacing fred with redis-rs behind a facade.
- `/home/michel/projects/mai/crates/mai-redis/` — the facade we are porting
  (owned by the same maintainer, copy authorised).

---

## 1. Goal

Two outcomes, sequenced:

1. **Land the surgical fixes** surfaced by the v0.16.1 post-GA audit (Section 5).
   Targets v0.16.2. Independent of any client swap.
2. **Replace `fred` with a `the-one-redis` facade** built on `redis-rs 1.2`,
   ported from MAI's `mai-redis` crate. Targets v0.17.0. Closes the window
   where our fred footprint is still small enough to migrate cheaply.

## 2. Non-goals

- Adding new Redis features (cluster, sentinel, TLS variants).
- Changing the public JSON-RPC surface of the broker.
- Backporting facade work to v0.16.x — the facade lives behind `redis-state`
  and `redis-vectors` feature gates, so v0.16.2 still ships fred-backed binaries.
- Re-litigating the Phase 2–4 decisions (BIGINT epoch_ms, Decision D deferred).

## 3. Audit summary (post-v0.16.1)

Source: four parallel Explore agents on 2026-05-16, cross-checked against the
git history through commit `682e06f`.

### 3.1 What's already healthy

- **Input sanitization** (`the_one_core::naming`) is applied at every write
  entry point including the four new backends. The StateStore trait funnels
  everything through the same chokepoints — Phases 3/5 inherited the discipline
  for free.
- **Cursor pagination** (`PageRequest::decode`) is used uniformly. No list
  endpoint silently truncates.
- **Audit log** with `outcome` + `error_kind` is implemented for SQLite,
  Postgres, and Redis StateStore variants.
- **Wire-level error sanitization** (`public_error_message` + `corr=<id>`) is
  applied uniformly to JSON-RPC error responses.
- **Error labels** include `postgres` and `redis` variants — Phase 3/5 errors
  surface to clients as short kind labels, never raw text.

### 3.2 What needs work

Covered in Sections 4 and 5 below.

## 4. Cross-reference against MAI's fred bug catalogue

MAI hit 11 fred bugs over 24 hours during their migration. Map against our
actual surface:

| MAI bug | Our exposure | Notes |
|---|---|---|
| **#1** BLPOP returns `Err(Timeout)` on nil | ✅ N/A | We don't use BLPOP. |
| **#2** XREADGROUP RESP2 map/array mismatch | ✅ N/A | We use XREVRANGE for audit, not consumer groups. |
| **#3** XAUTOCLAIM same wire-shape mismatch | ✅ N/A | We don't claim pending entries. |
| **#4** 30 s implicit cap on every command | **🔴 HIT — silent** | No `PerformanceConfig` / `ConnectionConfig` overrides anywhere in our tree. Every `XADD` / `FT.SEARCH` / `HSET` is on fred defaults: 10 s timeout × up to 3 retries. **Audit XADD on a slow Redis can produce duplicate rows.** FT.SEARCH on a cold index can bail spuriously. |
| **#5** tower-sessions-redis-store hard-couples to fred | ✅ N/A | No session storage in the-one-mcp. |
| **#6** 14-month stale upstream | 🔴 structural | Same crate, same maintenance posture, no patches since 2025-02-27. |
| **#7** Custom Value walkers proliferate | 🟡 starting | Three walkers in `storage/redis.rs:89/122/997`, plus CustomCommand value-walking in `redis_vectors.rs`. Smaller than MAI's seven copies but the same pattern. |
| **#8** Pub/sub 5-step setup | ✅ N/A | We don't subscribe. |
| **#9** XADD turbofish hell | 🟡 yes | `xadd::<(), _, _, _, _>(&key, false, None, "*", fields)` at `redis.rs:417, 438`. |
| **#10** SET with trailing `None, None, false` | 🟡 yes | `set::<(), _, _>(&key, val.as_str(), None, None, false)` at `redis.rs:333, 373`. |
| **#11** `expire` with `i64` seconds | ✅ N/A | We don't use EXPIRE. |

**Translation:** Bug #4 is a live correctness hazard. Bugs #6/#7/#9/#10 are
ergonomic, and they motivate the facade move while the surface is still
manageable.

## 5. Surgical fix catalogue (v0.16.2)

Independent of the fred swap — these fixes ship on the current crate. Each
includes file, line, severity, and the one-line patch shape.

### 5.1 Critical

**F1 — RediSearch query escaping is incomplete**
- File: `crates/the-one-core/src/storage/redis.rs:741`
- Symptom: a diary search containing `*` or `,` becomes a wildcard or a
  multi-term split. Affects diary FTS.
- Fix: extend the escape table to include `*` and `,`. Centralise via a
  `redis_query_escape(input: &str) -> String` helper (deduplicates the same
  escape logic in `redis_vectors.rs`).
- Test: feed `query="file*"` and `query="a,b"` and assert no spurious hits.

**F2 — AOF check duplicated and silently defaults to "off"**
- Files: `crates/the-one-mcp/src/broker.rs:618-625` (inline) and
  `crates/the-one-core/src/storage/redis.rs::verify_aof` (same parse logic).
- Symptom: on malformed `INFO persistence` output, both copies return `false`.
  A `require_aof=true` deployment can silently boot in cache mode.
- Fix: collapse to one helper in `storage/redis.rs` returning
  `Result<bool, CoreError::Redis>`. Treat parse failure as a real error,
  not as `aof_enabled=false`. Broker's combined-Redis path calls the helper.
- Test: feed a synthetic malformed `INFO` body and assert `Err(Redis(_))`.

### 5.2 Should-fix

**F3 — `RedisVectorStore::Clone` defeats the `OnceCell` guard**
- File: `crates/the-one-memory/src/redis_vectors.rs:21-29`
- Symptom: `Clone` constructs `Arc::new(OnceCell::new())` instead of
  `Arc::clone(&self.started)`. Each clone re-runs `startup()` (FT.CREATE etc.).
  Reached via the combined-Redis shared-client cache.
- Fix: `started: Arc::clone(&self.started)`.
- Test: clone the store, call `ensure_started()` twice, assert the inner
  client only saw one `FT.CREATE` round-trip.

**F4 — `serde_json::to_value(resp).unwrap_or(Value::Null)` swallows errors**
- File: `crates/the-one-mcp/src/transport/jsonrpc.rs:226, 266`
- Symptom: on serialization failure the client receives `result: null` with
  no error envelope. Logs only show the corr-id.
- Fix: map the serialization error into an internal-error JSON-RPC envelope
  carrying the corr-id, same as every other error path.
- Test: inject a response type that fails to serialize (e.g. NaN float) and
  assert the response is `{ "error": { "code": -32603, ... } }`.

**F5 — `request.image_base64.as_deref().unwrap()` in a request path**
- File: `crates/the-one-mcp/src/broker.rs:5071`
- Symptom: relies on a non-local `is_some()` guard. Brittle if the upstream
  contract drifts.
- Fix: pattern-match the `Option` once and bind, return
  `CoreError::InvalidRequest("image_base64 is required")` on None.

### 5.3 Nice-to-have

**F6 — `.unwrap_or_default()` masks corrupt JSON in Redis vectors**
- File: `crates/the-one-memory/src/redis_vectors.rs:60, 796, 1020`
- Symptom: malformed `source_chunks` JSON silently parses as `Vec::new()`.
- Fix: log a warning at WARN level before falling back, or treat as error.

**F7 — Key-namespace invariants undocumented**
- File: `crates/the-one-core/src/storage/redis.rs:225-230`
- Symptom: `{prefix}:{project_id}:{suffix}` is constructed by concatenation.
  If a project_id ever contains a `:` (today blocked by `sanitize_project_id`,
  but the invariant lives in two places).
- Fix: route all key construction through a single `key(suffix: &str) -> String`
  method on the store, document the format invariant in one place.

## 6. Migration plan — fred → `the-one-redis` facade

### 6.1 Scope

New crate at `crates/the-one-redis/` — **wholesale port** of
`~/projects/mai/crates/mai-redis/`. User directive 2026-05-16: take mai-redis
whole. Every module ports over, even those with no current call site, so:

- Future capability (lists, pubsub, sorted_sets, timeseries) is available with
  zero further facade work.
- The crate stays a near-mirror of mai-redis, so future bug fixes in either
  project can be cherry-picked across with minimal conflict resolution.
- Dead code is acceptable cost (~few KB compiled, gated by feature flags
  upstream if we want to slim later).

| mai-redis module | Port? | Notes |
|---|---|---|
| `lib.rs` | ✅ | Strip MAI-specific doc strings, rename crate to `the_one_redis`. |
| `error.rs` | ✅ | Replace `impl From<RedisError> for MaiError` with `impl From<RedisError> for the_one_core::CoreError`. |
| `pool.rs` | ✅ | **The `set_response_timeout(None)` fix is the load-bearing line.** Add `info_persistence() -> AofStatus` helper (mai-redis has it on the search module — we promote to pool because the AOF check is a deployment-readiness concern, not a search concern). |
| `keys.rs` | ✅ | Full surface. |
| `hashes.rs` | ✅ | Full surface. |
| `streams.rs` | ✅ | Full surface including XREADGROUP/XAUTOCLAIM — even though we don't use them now, MAI's typed parsers are exactly the workaround we'd need later. |
| `search.rs` | ✅ | Full surface including FT.CREATE / FT.SEARCH / FT.INFO / FT.ALTER / FT.DROPINDEX. |
| `lists.rs` | ✅ | Full surface (BLPOP etc.). Locks the Ok(None)-on-timeout contract. |
| `pubsub.rs` | ✅ | Full surface. Carries the 5-step-collapsed-to-1 ergonomic win. |
| `sets.rs` | ✅ | Full surface. |
| `sorted_sets.rs` | ✅ | Full surface. |
| `timeseries.rs` | ✅ | Full surface. Requires `redistimeseries` module on the target server — same constraint as mai-redis. |

Rename mechanics (mechanical, applied by `sed` during the port):
- `mai_redis` → `the_one_redis` (crate / module references)
- `mai-redis` → `the-one-redis` (Cargo.toml package name + path)
- `MaiError` → `CoreError` (with the `Redis(String)` variant we already added)
- `MaiRedisError` / `RedisError` (the facade's own type) → keep `RedisError`
- `mai-types` workspace dep → drop, replace with `the-one-core` for error coupling
- `docs/guides/redis-facade.md` reference → update to point at this plan

### 6.2 Public API shape (mirrors mai-redis exactly)

```rust
let pool = the_one_redis::Pool::new(
    the_one_redis::PoolConfig::from_url("redis://127.0.0.1:6379")
).await?;

// Strings
pool.keys().set("foo", "bar").await?;
let v: Option<String> = pool.keys().get("foo").await?;

// Hashes
pool.hashes().hset("h", "field", "value").await?;
let m: HashMap<String, String> = pool.hashes().hgetall("h").await?;

// Streams (audit log)
pool.streams().xadd("audit", &[("op","x"),("outcome","ok")], Some(10_000)).await?;
let entries = pool.streams().xrevrange("audit", "+", "-", Some(100)).await?;

// RediSearch — typed schema + Query builder
let schema = SchemaBuilder::new()
    .add_text("content").add_tag("project_id").add_vector_hnsw("embedding", 1024)
    .build();
pool.search().ft_create("idx", schema).await?;

let query = Query::knn("*=>[KNN 10 @embedding $vec AS score]")
    .param("vec", vec_bytes)
    .dialect(2);
let reply: SearchReply = pool.search().ft_search("idx", &query).await?;
```

### 6.3 Pool construction (THE fix)

Verbatim port from `mai-redis/src/pool.rs:98-100`:

```rust
pub(crate) fn connection_config() -> AsyncConnectionConfig {
    AsyncConnectionConfig::default().set_response_timeout(None)
}
```

Both the shared multiplexed connection AND `dedicated_conn()` apply this.
The line replaces all of the workarounds documented in MAI Bug #4.

### 6.4 Per-call-site migration

#### Site 1 — `crates/the-one-core/src/storage/redis.rs` (~1000 LOC)

Today:
```rust
use fred::clients::Client;
use fred::interfaces::{ClientLike, KeysInterface, HashesInterface, StreamsInterface, ...};
use fred::types::config::Config;
use fred::types::redisearch::{FtCreateOptions, IndexKind, SearchSchema, SearchSchemaKind};
use fred::types::{InfoKind, Value};

// 333:   conn.set::<(), _, _>(&key, val.as_str(), None, None, false)
// 417:   conn.xadd::<(), _, _, _, _>(&key, false, None, "*", fields)
// 465:   conn.xrevrange(&key, "+", "-", Some(count as u64))
// 507:   conn.hset::<(), _, _>(&key, fields)
// 614:   conn.hget(&hk, "json")
// 741:   FT.SEARCH escape table
```

After:
```rust
use the_one_redis::{Pool, PoolConfig};

// pool.keys().set(&key, val.as_str()).await?;
// pool.streams().xadd(&key, &fields, None).await?;
// pool.streams().xrevrange(&key, "+", "-", Some(count)).await?;
// pool.hashes().hset_multi(&key, &fields).await?;
// pool.hashes().hget::<String>(&hk, "json").await?;
// pool.search().ft_search(&idx, &query).await?;
```

Other changes:
- `RedisStateConfig::open()` constructs a `Pool` instead of `fred::clients::Client`.
- `verify_aof()` calls `pool.info_persistence()` instead of parsing `INFO` text.
- Three local Value walkers (`val_to_string`, `parse_conv_hash`,
  `parse_audit_stream`) collapse into typed `FromRedisValue` decoding +
  one `parse_audit_entry(StreamEntry) -> AuditEvent` helper.

#### Site 2 — `crates/the-one-memory/src/redis_vectors.rs` (~1100 LOC)

Today:
```rust
use fred::clients::Client;
use fred::interfaces::{ClientLike, KeysInterface, HashesInterface};
use fred::types::config::Config;
use fred::types::redisearch::{FtCreateOptions, IndexKind, SearchSchema, SearchSchemaKind};
use fred::types::{ClusterHash, CustomCommand, InfoKind, Map, Value};

// 402:   CustomCommand::new("FT.SEARCH", ClusterHash::FirstKey, false)  // chunks
// 1050:  CustomCommand::new("FT.SEARCH", ...)                            // entities
// 1141:  CustomCommand::new("FT.SEARCH", ...)                            // relations
```

After:
```rust
use the_one_redis::{Pool, search::{Query, SchemaBuilder, SearchReply}};

// pool.search().ft_search("chunks_idx", &query).await?;
// pool.search().ft_search("entities_idx", &query).await?;
// pool.search().ft_search("relations_idx", &query).await?;
```

CustomCommand goes away entirely — first-class typed FT.SEARCH covers all
three call sites. Fix F3 (Clone+OnceCell) lands in the same diff.

#### Site 3 — `crates/the-one-mcp/src/broker.rs` (~50 LOC)

Today:
```rust
combined_redis_client_by_project: RwLock<HashMap<String, fred::clients::Client>>,

let config = fred::types::config::Config::from_url(url)?;
let client = fred::clients::Client::new(config, None, None, None);
client.init().await?;
let info = client.info(Some(fred::types::InfoKind::Persistence)).await?;
```

After:
```rust
combined_redis_client_by_project: RwLock<HashMap<String, the_one_redis::Pool>>,

let pool = the_one_redis::Pool::new(
    the_one_redis::PoolConfig::from_url(url)
).await?;
let aof = pool.info_persistence().await?;
```

`shutdown()` calls `pool.close().await` (graceful drain).

### 6.5 Workspace + feature gates

**User directive 2026-05-16: 100% of fred must be out of the code, completely.**
That includes transitive references — no third-party crate may pull fred via
a dep chain. Verification commands run at each migration step and gate
merge:

```bash
# Fail if any source file still references fred
grep -rn "fred::\|use fred\|fred = " crates/ Cargo.toml | grep -v "docs/\|//"
# Expected: no matches

# Fail if cargo tree shows fred anywhere
cargo tree -e all --workspace --all-features | grep -i fred
# Expected: no matches (post-step-6)

# Fail if Cargo.lock still mentions fred
grep -c "^name = \"fred" Cargo.lock
# Expected: 0
```

Root `Cargo.toml`:
```toml
[workspace.dependencies]
# REMOVE entirely:
# fred = { version = "10", features = [...] }

# ADD:
the-one-redis = { path = "crates/the-one-redis" }
```

Per-crate Cargo.toml updates (every line touching fred is replaced):
```toml
# crates/the-one-core/Cargo.toml
[features]
redis-state = ["dep:the-one-redis", "dep:tokio"]   # was: ["dep:fred", "dep:tokio"]

[dependencies]
the-one-redis = { workspace = true, optional = true }
# REMOVE: fred = { workspace = true, optional = true }

# crates/the-one-memory/Cargo.toml
[features]
redis-vectors = ["dep:the-one-redis"]              # was: ["dep:fred"]

[dependencies]
the-one-redis = { workspace = true, optional = true }
# REMOVE: fred = { workspace = true, optional = true }

# crates/the-one-mcp/Cargo.toml
[features]
redis-state    = ["the-one-core/redis-state",    "dep:the-one-redis"]
redis-vectors  = ["the-one-memory/redis-vectors", "dep:the-one-redis"]

[dependencies]
the-one-redis = { workspace = true, optional = true }
# REMOVE: fred = { workspace = true, optional = true }
```

CLAUDE.md note removal: the line about `fred` `i-streams` feature added in
the Phase 5 section is updated to reference `the-one-redis` instead, with a
note that "fred was removed in v0.17.0 — see `docs/plans/2026-05-16-fred-removal-and-bug-fixes.md`".

Comments and identifiers: any inline comment mentioning fred is updated to
match (e.g., "fred surfaces nil-timeout" → "the-one-redis surfaces …"). Search
order: code first, then doc strings, then markdown that references behaviour
(not history).

## 7. Test plan

### 7.1 Unit tests inside `the-one-redis`

Port from `mai-redis/tests/`:
- `pool_response_timeout.rs` — assert `connection_config().response_timeout == None`.
- `search_escape.rs` — assert `redis_query_escape("a*b,c") == "a\\*b\\,c"` (covers F1).
- `pool_redact.rs` — password redaction (already inline in mai-redis pool.rs).

### 7.2 Live-Redis integration tests

New file `crates/the-one-redis/tests/sentinel_ops.rs`:
- Gated on `THE_ONE_REDIS_URL` env var (skip via `return` if unset).
- `#[ignore]` by default; CI runs with `cargo test -- --include-ignored`.
- Test cases:
  - `hset_hget_roundtrip` — basic happy path.
  - `xadd_xrevrange_roundtrip` — audit pattern.
  - `ft_create_search_knn` — vector schema + KNN query against synthetic
    embeddings.
  - `slow_command_not_retried` — sleep-debug an `EVAL` for 12 s, assert the
    facade waits the full deadline rather than retrying. **Locks the fix for
    Bug #4.**
  - `aof_check_propagates_parse_error` — feed a malformed `INFO` via a mock
    or skip if not feasible.

### 7.3 Migrated-crate regression tests

Re-run existing integration suites against the migrated code:
- `cargo test -p the-one-core --features redis-state` — Phase 5 round-trips
  (8 tests).
- `cargo test -p the-one-memory --features redis-vectors` — Phase 7 vector
  parity (subset of 21 tests).
- `cargo test -p the-one-mcp --features redis-state,redis-vectors` — Phase 6
  combined backend (subset of 13 tests).

All currently skip when `THE_ONE_STATE_TYPE` / `THE_ONE_VECTOR_TYPE` env vars
are unset — they run unchanged under the new substrate.

### 7.4 CI changes (orthogonal but high-leverage)

Track separately, but the moment we wire `the-one-redis`, the CI gap from the
v0.16.1 audit becomes the binding constraint:

- Add `services.redis` to `.github/workflows/ci.yml` using the
  `redis/redis-stack:latest` image (ships RediSearch).
- Export `THE_ONE_STATE_URL=redis://localhost:6379` /
  `THE_ONE_VECTOR_URL=redis://localhost:6379` for the redis matrix cell.
- Matrix dimension: `features ∈ { "", "pg-state", "pg-state,pg-vectors", "redis-state", "redis-state,redis-vectors" }`.

Not in scope for v0.17.0 if it slips. Tracked as a separate task in
`/route-language-task` follow-ups.

## 8. Rollout sequence

| # | Step | Target version | Gating |
|---|---|---|---|
| 1 | Ship F1–F5 surgical fixes on the existing fred substrate | v0.16.2 | Tests pass; release-gate clean. |
| 2 | Create `crates/the-one-redis` from mai-redis port | v0.17.0-rc1 | Unit tests pass; `cargo test -p the-one-redis` green. |
| 3 | Migrate `storage/redis.rs` | v0.17.0-rc1 | Phase 5 integration tests pass against `THE_ONE_REDIS_URL`. |
| 4 | Migrate `redis_vectors.rs` | v0.17.0-rc1 | Phase 7 integration tests pass. |
| 5 | Migrate `broker.rs` shared cache | v0.17.0-rc1 | Phase 6 integration tests pass; `cargo tree | grep fred` empty. |
| 6 | Drop fred from workspace + per-crate Cargo.toml | v0.17.0-rc1 | All three `grep`s from §6.5 return empty. `release-gate.sh` clean. `Cargo.lock` regenerated and committed. |
| 7 | Port sentinel tests (`slow_command_not_retried` etc.) | v0.17.0 | Locks Bug #4 contract in CI. |
| 8 | Add CI Redis service + feature matrix | v0.17.0 or v0.17.1 | Unblocks the broader v0.16.1 CI gap. |
| 9 | **Final fred-purity audit** | v0.17.0 release tag | `cargo tree -e all --workspace --all-features \| grep -i fred` returns empty. Manual scan of comments / doc strings for any "fred" mentions in non-historical contexts. Commit lock file. |

Steps 2–6 ship as a single PR with one commit per step (so revert granularity
is per-call-site, not all-or-nothing).

## 9. Task tracker

Tasks created in the session task list on 2026-05-16. See `TaskList`.

| Task # | Subject | Blocks | BlockedBy |
|---|---|---|---|
| 6 | Create the-one-redis facade crate skeleton (wholesale port) | 1, 4, 2 | — |
| 1 | Migrate storage/redis.rs to facade | 5 | 6 |
| 4 | Migrate redis_vectors.rs to facade | 5 | 6 |
| 5 | Migrate broker.rs combined-Redis client cache | 3 | 6, 1, 4 |
| 3 | Drop fred from workspace and clean Cargo.toml | — | 1, 4, 5 |
| 2 | Port mai-redis sentinel tests | — | 6 |

**Note 2026-05-16:** Task 6 is to be a *wholesale* port — all 10 mai-redis
modules, not the trimmed 7-module subset. Update the task description before
starting work.

**Note 2026-05-16:** Task 3 includes the three fred-purity grep gates from
§6.5 plus a final `cargo tree -e all` audit. Add a task #9 ("fred-purity
audit") before tagging v0.17.0, after Task 3 is complete.

The surgical fixes (F1–F7) are not yet in the task list — they are batched
under v0.16.2 and ship before the facade work begins. Create them when
Step 1 (Section 8) starts.

## 10. Open questions

None require resolution before implementation begins. Three forward
preferences locked from the v0.16.1 session:

- **No chrono.** All BIGINT epoch_ms timestamps stay as `i64` / `u64`. The
  facade follows the same convention; `streams.rs`'s `StreamEntry.id` is a
  `String` (the Redis-generated `<ms>-<seq>` format).
- **No backwards-compat shim** for fred users. The facade is feature-gated,
  and the `redis-state` / `redis-vectors` features bring it in. There is no
  "use fred or facade" toggle.
- **No new public JSON-RPC tools.** This is a refactor, not a feature.

## 11. References

- [`mai-redis/src/pool.rs`](../../mai/crates/mai-redis/src/pool.rs) — exact
  fix code being ported.
- [`mai-redis/src/lib.rs`](../../mai/crates/mai-redis/src/lib.rs) — public
  surface shape.
- [`mai-redis/src/search.rs`](../../mai/crates/mai-redis/src/search.rs) —
  the FT.* typed wrapper; replaces our `CustomCommand` walls.
- [`mai-redis/tests/sentinel_ops.rs`](../../mai/crates/mai-redis/tests/sentinel_ops.rs)
  — sentinel test pattern.
- [`fred-retrospective.md`](../../../mai/docs/guides/fred-retrospective.md) —
  the upstream "why we left" document.
- [`production-hardening-v0.15.md`](../guides/production-hardening-v0.15.md) —
  the hardening conventions our facade must respect (corr-ids, error labels,
  sanitization, pagination).
