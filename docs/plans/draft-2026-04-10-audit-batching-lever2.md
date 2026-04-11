# Audit Log Batching (Lever 2) — Implementation Plan [DRAFT, SUPERSEDED]

> ⚠️ **This draft has been superseded by
> `2026-04-10-audit-batching-lever2.md` (v2).**
>
> Retained for review trail. The v2 document resolves the issues
> listed in **§ 16 Open issues** at the end of this file. Do NOT
> implement from this draft — use the v2 plan. Use this file only
> to see the history and the rejected-alternatives section which
> was not duplicated in v2.

**Status:** Draft — superseded by v2 on 2026-04-10.
**Owner:** unassigned
**Prerequisite:** v0.15.1 (Lever 1, `synchronous=NORMAL`) — already shipped.
**Estimated effort:** 1–2 days of careful implementation + 1 day of
soak testing.
**Estimated impact:** additional 5–10× throughput improvement on top
of Lever 1 (from ~11 k rows/sec to ~50–100 k rows/sec per project).

---

## 1. Problem statement

After shipping Lever 1 (`synchronous=NORMAL`) the audit-log write path
runs at ~85µs per row, down from ~5 ms. For a broker servicing even
1 000 state-changing calls per minute, total audit overhead is now
~85 ms/min — 0.14% of wall clock. **This plan is therefore documented,
not scheduled.** It describes the design we would ship *if* we ever
observe `record_audit` as a real bottleneck in a flamegraph.

### What would trigger implementation

Any one of:

1. `record_audit` appears above 1% of CPU in a production flamegraph
   under realistic load.
2. A deployment targets > 10 000 state-changing broker calls per
   second sustained (e.g. a multi-tenant SaaS exposing `the-one-mcp`
   to dozens of concurrent CLI clients).
3. Broker p99 latency regresses and the regression traces back to
   audit-log writes blocking on WAL checkpoints.

Absent any of these, Lever 1 is sufficient and adding Lever 2 would
be overengineering.

### What "batching" means here

Instead of writing one audit row per broker method call synchronously:

```rust
// v0.15.x: synchronous write, blocks on SQLite
db.record_audit(&AuditRecord::ok("memory.diary.add", params))?;
```

The broker enqueues the record on an async channel and returns
immediately:

```rust
// v0.16 (Lever 2): fire-and-forget enqueue
self.audit_batcher.queue(project_root, project_id, AuditRecord::ok(...));
```

A background tokio task drains the channel, groups records by
`(project_root, project_id)`, and flushes each group in a single
`BEGIN IMMEDIATE; INSERT ...; INSERT ...; COMMIT;` — amortising the
SQLite prepared-statement overhead across N records.

---

## 2. Goals and non-goals

### Goals

1. **Amortise audit-write overhead**: cut per-row audit latency from
   ~85µs to < 10µs amortised.
2. **Never block broker request paths**: `queue_audit` returns in
   O(1) without touching SQLite.
3. **Preserve ordering within a single `(project_root, project_id)`**:
   audit rows for the same project must land in the order they were
   queued. Ordering across projects is irrelevant because every query
   filters by `project_id`.
4. **Bounded memory**: the batcher holds at most `MAX_QUEUED` records
   in flight per project. Exceeding the cap applies backpressure
   (blocking `queue_audit` with a warning) rather than silently
   dropping records.
5. **Graceful shutdown**: on broker drop, flush everything before
   returning control. No "this program terminated while audit rows
   were still in memory" scenarios.
6. **Observable**: new metrics `audit_batcher_queue_depth`,
   `audit_batcher_flushes_total`, `audit_batcher_flush_latency_ms`,
   `audit_batcher_dropped_on_shutdown_total`.
7. **Zero impact on existing tests**: the synchronous `record_audit`
   API continues to work for tests and tools that need deterministic
   persistence-before-return semantics.

### Non-goals

1. **Cross-project batching**: SQLite connections are per-project, so
   batching across projects would require multiple connections anyway
   and wouldn't actually reduce fsync count. Skip.
2. **Persistence of the in-memory queue**: if the broker process
   dies, in-flight records are lost. This is a deliberate trade-off
   — see § 6.
3. **Applying the same pattern to docs/diary/navigation writes**:
   those are already user-visible writes that clients expect to be
   persistent when the call returns. Only audit rows are safe to
   defer. Lever 2 is audit-only.
4. **Replacing the synchronous `record_audit` API**: it stays. The
   new API is an additional pathway, not a replacement.

---

## 3. Architecture

### 3.1 Component diagram

```
     ┌──────────────────────────────────────────────┐
     │                   McpBroker                   │
     │  ┌────────────────────────────────────────┐  │
     │  │  memory_ingest_conversation / etc.     │  │
     │  │  (broker request path — runs on tokio  │  │
     │  │   worker threads, MUST NOT block)      │  │
     │  └──────────────┬─────────────────────────┘  │
     │                 │ queue_audit(project, rec)  │
     │                 ▼                             │
     │  ┌────────────────────────────────────────┐  │
     │  │  AuditBatcher::queue                    │  │
     │  │  - O(1), non-blocking                   │  │
     │  │  - increments queue_depth metric        │  │
     │  │  - on full queue: await with timeout    │  │
     │  └──────────────┬─────────────────────────┘  │
     │                 │ mpsc::Sender<BatcherMsg>    │
     └─────────────────┼────────────────────────────┘
                       │
                       ▼
        ┌──────────────────────────────────────┐
        │  BatcherTask (spawned once per       │
        │  broker at construction)             │
        │                                       │
        │  loop {                               │
        │    select {                           │
        │      msg = rx.recv() => accumulate    │
        │      _ = interval.tick() => flush_all │
        │      shutdown = shutdown_rx.recv() => │
        │        flush_all; break               │
        │    }                                  │
        │    if bucket_size >= BATCH_MAX {      │
        │      flush(project)                   │
        │    }                                  │
        │  }                                    │
        └──────────────┬───────────────────────┘
                       │ holds HashMap<ProjectKey, Vec<AuditRecord>>
                       │
                       ▼
        ┌──────────────────────────────────────┐
        │  flush(project) {                     │
        │    db = cached_db(project);          │
        │    db.transaction(|tx| {              │
        │       for rec in bucket {             │
        │          tx.execute(INSERT, ...);     │
        │       }                               │
        │    });                                │
        │  }                                    │
        └──────────────────────────────────────┘
```

### 3.2 Key data structures

```rust
// crates/the-one-core/src/audit.rs (additions)

/// Opaque handle the broker uses to enqueue audit records. Cheap to
/// clone; internally holds an `Arc<AuditBatcherInner>`.
#[derive(Clone)]
pub struct AuditBatcher {
    inner: Arc<AuditBatcherInner>,
}

struct AuditBatcherInner {
    sender: mpsc::Sender<BatcherMsg>,
    shutdown: tokio::sync::Notify,
    /// Exposed so McpBroker::metrics_snapshot can report it.
    queue_depth: AtomicU64,
    flushes_total: AtomicU64,
    dropped_on_shutdown: AtomicU64,
    flush_latency_ms_total: AtomicU64,
}

enum BatcherMsg {
    Record {
        key: ProjectKey,
        record: AuditRecord,
    },
    /// Synchronous flush — waits until all currently queued records
    /// for `key` have been written to SQLite, then resolves `tx`.
    Flush {
        key: ProjectKey,
        tx: oneshot::Sender<Result<(), CoreError>>,
    },
    /// Final shutdown — drain every bucket, then close.
    Shutdown {
        tx: oneshot::Sender<FlushReport>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectKey {
    pub project_root: PathBuf,
    pub project_id: String,
}

#[derive(Debug, Clone)]
pub struct AuditBatcherConfig {
    /// Maximum records queued per project before `queue` applies
    /// backpressure.
    pub max_queue_depth_per_project: usize,
    /// Flush when any bucket reaches this many pending records.
    pub batch_max_rows: usize,
    /// Flush every bucket at least this often, regardless of size.
    pub flush_interval: Duration,
    /// How long `queue` will wait on a full channel before returning
    /// an error. Prevents caller starvation under sustained overload.
    pub queue_timeout: Duration,
}

impl Default for AuditBatcherConfig {
    fn default() -> Self {
        Self {
            max_queue_depth_per_project: 10_000,
            batch_max_rows: 256,
            flush_interval: Duration::from_millis(250),
            queue_timeout: Duration::from_millis(100),
        }
    }
}
```

### 3.3 Concurrency model

- **One batcher task per broker**, not per project. SQLite access
  within that task is single-threaded, so no per-project mutex is
  needed. Per-project buckets are a `HashMap<ProjectKey, Vec<AuditRecord>>`
  owned exclusively by the task.
- **The channel is `tokio::sync::mpsc::channel(capacity)`**. Capacity
  is `max_queue_depth_per_project * 8` to give the task headroom
  before backpressure triggers.
- **Writers (broker methods) use `sender.try_send` first**; on
  `TrySendError::Full` they fall back to `sender.send_timeout(msg,
  queue_timeout)`. On timeout they log a warning and return a
  `CoreError::Transport("audit queue saturated")` that the caller
  can choose to propagate or ignore. See § 5.
- **The batcher task runs `tokio::select!` over: channel receive,
  interval tick, shutdown notify**. On each channel message it
  appends to the bucket and checks the `batch_max_rows` threshold.
  On each interval tick it flushes every bucket that has pending
  records. On shutdown it flushes every bucket exactly once then
  exits the loop.

### 3.4 Project database caching

The batcher task opens each `ProjectDatabase` lazily on first flush
and caches it in a `HashMap<ProjectKey, ProjectDatabase>`. Entries
are evicted via LRU (max 32 projects) to bound file-descriptor usage.

This cache is **private to the batcher task** — not shared with the
rest of the broker, which continues to open `ProjectDatabase` per
call. Sharing the cache would introduce a mutex between the request
path and the batcher task, which defeats the whole purpose of
batching.

Trade-off: if both the batcher and a broker method write to the same
project simultaneously, they're using two different SQLite
connections. That's fine — WAL mode supports multiple writers
serialised by SQLite internally — but it means the broker method
might briefly contend with a batch flush on the project's WAL lock.
In practice this is invisible at the latencies we care about.

---

## 4. API changes

### 4.1 Public surface additions

```rust
// the-one-core/src/audit.rs

impl AuditBatcher {
    /// Build a new batcher and spawn its background task. The task
    /// will run until the returned handle is dropped, at which point
    /// `Drop` triggers a synchronous final flush.
    pub fn spawn(config: AuditBatcherConfig) -> Self;

    /// Non-blocking enqueue. Returns `Err(Transport)` only if the
    /// queue is saturated AND the `queue_timeout` expires.
    pub async fn queue(
        &self,
        project_root: &Path,
        project_id: &str,
        record: AuditRecord,
    ) -> Result<(), CoreError>;

    /// Block until every currently-queued record for the given
    /// project has been written. Used by tests and by shutdown.
    pub async fn flush(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<(), CoreError>;

    /// Snapshot of per-batcher metrics for observability endpoints.
    pub fn metrics(&self) -> AuditBatcherMetrics;
}

/// Returned by `AuditBatcher::metrics`. Mirrors the existing
/// BrokerMetrics pattern.
#[derive(Debug, Clone, Serialize)]
pub struct AuditBatcherMetrics {
    pub queue_depth: u64,
    pub flushes_total: u64,
    pub rows_flushed_total: u64,
    pub dropped_on_shutdown: u64,
    pub flush_latency_ms_total: u64,
    pub flush_latency_ms_avg: u64,
}
```

### 4.2 Broker wiring

```rust
// the-one-mcp/src/broker.rs

pub struct McpBroker {
    // … existing fields …
    audit_batcher: AuditBatcher,
}

impl McpBroker {
    pub fn new_with_policy(policy: PolicyEngine) -> Self {
        // … existing setup …
        let audit_batcher = AuditBatcher::spawn(AuditBatcherConfig::default());
        Self { /* … */ audit_batcher }
    }
}

// Callsites change from:
//     db.record_audit(&record)?;
// to:
//     self.audit_batcher
//         .queue(project_root, project_id, record)
//         .await?;
//
// The synchronous `record_audit` path stays available for callers
// that need deterministic "row is on disk when this returns"
// semantics — primarily integration tests.
```

### 4.3 Metrics snapshot

```rust
// MetricsSnapshotResponse (existing type) gets three new fields:
pub audit_queue_depth: u64,
pub audit_flushes_total: u64,
pub audit_flush_latency_ms_avg: u64,
```

Mark them `#[serde(default)]` so deserialisers written against older
versions keep working.

### 4.4 Config plumbing

New section in `config.rs`:

```toml
[audit_batcher]
max_queue_depth_per_project = 10000  # default
batch_max_rows = 256                  # default
flush_interval_ms = 250               # default
queue_timeout_ms = 100                # default
```

All four keys live in `AppConfig`. The broker reads them when it
spawns the batcher. A project-level override is *not* supported —
the batcher is global to the broker, not per-project, so a
per-project config would be ignored or misleading.

---

## 5. Failure modes and mitigations

### 5.1 Broker process crash between `queue` and `flush`

**Scenario:** broker methods call `queue_audit`, the batcher task
buffers 200 records, then the process gets SIGKILL.

**Consequence:** those 200 audit rows are lost. The corresponding
data writes (diary, navigation, etc.) already landed in SQLite
synchronously, so there's a brief window where data exists without
audit coverage.

**Mitigation:**
1. Document this explicitly in the guide.
2. The upper bound on the window is `batch_max_rows * flush_interval`
   = 256 rows × 250ms = ~64 rows/sec of unaudited data in the worst
   case.
3. Operators who need stronger guarantees can either:
   - Use the synchronous `record_audit` path instead (no batching).
   - Reduce `flush_interval_ms` at the cost of throughput.
   - Reduce `batch_max_rows` to 1 (effectively disabling batching).
4. **We do NOT add a persistent queue.** That would require a
   second write-ahead log that's *itself* subject to the fsync
   penalty we're trying to avoid. It's turtles.

### 5.2 Batcher task panics mid-flush

**Scenario:** SQLite throws an error mid-batch. The `tx.execute`
call returns `Err`.

**Mitigation:**
1. Wrap the flush in a `catch_unwind` (via `tokio::task::spawn_blocking`
   for the SQLite portion) so a panic inside rusqlite doesn't kill
   the batcher task.
2. On flush error, log the error with correlation ID, increment a
   `audit_flush_errors_total` counter, and **keep the records in
   the bucket**. The next flush retries them.
3. If the same bucket fails to flush three times in a row, drop the
   bucket, increment `audit_dropped_flush_failures_total`, and emit
   a `tracing::error!` with full context.
4. Add a unit test that fakes a SQLite error and verifies the
   retry-then-drop behaviour.

### 5.3 Queue saturation under write storm

**Scenario:** a client hammers the broker with 100k operations/sec,
faster than the batcher can flush. The channel fills up.

**Mitigation:**
1. `queue` uses `send_timeout(msg, queue_timeout)`. On timeout it
   returns `CoreError::Transport("audit queue saturated")`.
2. The broker logs a warning including the queue depth.
3. Callers decide whether to propagate or swallow. For audit writes
   we would swallow (increment a `audit_queue_timeouts_total`
   counter) because losing an audit row is better than failing the
   underlying memory write.
4. Alert: monitor `audit_queue_depth` and fire if it's consistently
   > 80% of capacity.

### 5.4 Graceful shutdown dropping in-flight records

**Scenario:** broker Drop impl runs, the batcher task receives
shutdown, but the final flush is still running when the runtime
itself is torn down.

**Mitigation:**
1. `AuditBatcher::spawn` returns a handle whose `Drop` impl sends a
   `Shutdown` message on the channel and then **blocks the current
   thread** on the oneshot response with a 10-second timeout.
2. If the timeout expires, log an error with the number of dropped
   records and let the program continue exiting.
3. The broker's `bin/the-one-mcp.rs` installs a SIGTERM handler that
   explicitly awaits `broker.shutdown().await` *before* the tokio
   runtime shuts down, giving the batcher a clean exit path.
4. `FlushReport` (returned by shutdown) reports the total flushed +
   dropped so operators can see it in the log.

### 5.5 Clock skew / interval tick drift

**Scenario:** the `tokio::time::interval` falls behind because the
batcher task is busy flushing. Records can sit longer than
`flush_interval_ms` before the next tick.

**Mitigation:**
1. Use `interval.set_missed_tick_behavior(Burst)` so missed ticks
   fire immediately.
2. Track "time since last flush" per bucket explicitly — if it
   exceeds `2 * flush_interval`, log a warning and force-flush.

### 5.6 Batcher task starves the request path

**Scenario:** the batcher task runs inside the same tokio runtime as
the broker request path. If the flush is synchronous (rusqlite) and
takes too long, it blocks an executor thread.

**Mitigation:**
1. Wrap the SQLite flush in `tokio::task::spawn_blocking`. The
   batcher task's `select!` loop stays async; only the actual
   INSERT work runs on the blocking pool.
2. Document the choice. If a flush takes > 100ms consistently, the
   operator should increase the tokio blocking pool size.
3. Benchmark under sustained load as part of the test plan.

### 5.7 Cache eviction under many projects

**Scenario:** a deployment serves 1 000 distinct projects. The
batcher's DB cache holds at most 32. Projects #33+ churn the
cache, re-opening `ProjectDatabase` every flush.

**Mitigation:**
1. Track `audit_db_cache_evictions_total` so the operator can see it.
2. Under heavy churn, reopening `ProjectDatabase` is still < 5ms
   and runs on the blocking pool, so the batcher task doesn't
   stall.
3. If a deployment actually hits this, make `cache_size` a config
   knob.

---

## 6. Testing plan

### 6.1 Unit tests (in `the-one-core::audit`)

1. `test_batcher_queues_and_flushes_on_tick` — queue 5 records,
   wait > `flush_interval`, assert SQLite has 5 rows.
2. `test_batcher_flushes_on_batch_max_rows` — queue `batch_max_rows`
   records rapidly, assert flush fires without waiting for tick.
3. `test_batcher_preserves_order_within_project` — queue 100 records
   with sequential IDs, flush, read them back, assert order.
4. `test_batcher_backpressures_on_saturation` — fill the channel,
   attempt another `queue`, assert it returns
   `Transport("audit queue saturated")` after the timeout.
5. `test_batcher_explicit_flush_blocks_until_done` — queue N
   records, call `flush(project)`, read SQLite immediately after,
   assert all rows are present.
6. `test_batcher_isolation_across_projects` — queue records for
   projects A and B, flush A only, assert A has rows but B doesn't.
7. `test_batcher_survives_sqlite_error_and_retries` — inject a
   failure on the first flush, assert the records retry on the next
   flush and land successfully.
8. `test_batcher_drops_bucket_after_3_consecutive_failures` — mock
   persistent SQLite error, assert drop-then-continue behaviour.
9. `test_batcher_shutdown_flushes_everything_queued` — queue 50
   records, call `shutdown`, assert all 50 are on disk before
   `shutdown` resolves.
10. `test_batcher_shutdown_timeout_reports_dropped_count` — force
    a shutdown with a pathologically-fast timeout, verify
    `FlushReport` reports non-zero dropped count.
11. `test_batcher_metrics_are_monotonic` — flush twice, assert
    counters only increase.

### 6.2 Integration tests

1. Extend `tests/stdio_write_path.rs` with a new test
   `stdio_audit_batching_rows_land_after_flush` — drive 20 writes
   via stdio, call the new `observe.audit_batcher_flush` admin
   action, verify rows landed.
2. Add `tests/production_hardening.rs::lever2_audit_queue_depth_metric_updates`
   — verify the metric is reported via `metrics_snapshot`.

### 6.3 Benchmarks

Update `examples/production_hardening_bench.rs` with a new section:

```
## Audit log throughput (Lever 2: async batching)

| rows  | queue() total | amortized per-row | flush latency |
|------:|--------------:|------------------:|--------------:|
| 10000 |          XXms |              XXµs |          XXms |
```

Target: amortized per-row < 10µs under default config. If we don't
hit that, tune `batch_max_rows` up.

### 6.4 Stress / soak tests

These run only manually via a feature flag:

1. **1 M rows over 60 seconds**: enqueue 1M records at a steady
   rate, verify every record lands in SQLite, no records are
   dropped, queue depth stays under capacity.
2. **Concurrent projects**: 50 projects writing simultaneously at
   different rates, verify per-project ordering preserved.
3. **Panic injection**: every 1 000 flushes force a synthetic
   rusqlite error, verify retry+drop+continue works for 10 000 flushes.
4. **Shutdown under load**: while queue is 80% full, issue shutdown,
   verify final flush lands everything.
5. **SQLite WAL churn**: check that WAL file size stays bounded
   under sustained load (batcher should trigger checkpoints).

---

## 7. Observability

### 7.1 Metrics

New counters exposed via `BrokerMetrics` (already `Arc`-wrapped so
the batcher task can share it):

| metric                                  | type     | description                                             |
|------------------------------------------|----------|---------------------------------------------------------|
| `audit_batcher_queue_depth`              | gauge    | Current records buffered across all projects            |
| `audit_batcher_max_project_depth`        | gauge    | Max depth seen in any single project bucket             |
| `audit_batcher_flushes_total`            | counter  | Total number of batch flushes executed                  |
| `audit_batcher_rows_flushed_total`       | counter  | Total rows persisted via the batcher                    |
| `audit_batcher_flush_latency_ms_total`   | counter  | Cumulative flush latency for averaging                  |
| `audit_batcher_flush_errors_total`       | counter  | Flushes that returned a SQLite error                    |
| `audit_batcher_dropped_flush_failures`   | counter  | Buckets dropped after 3 consecutive flush failures      |
| `audit_batcher_queue_timeouts_total`     | counter  | `queue()` calls that gave up waiting on a full channel  |
| `audit_batcher_dropped_on_shutdown`      | counter  | Records lost because shutdown timeout expired           |
| `audit_batcher_db_cache_evictions_total` | counter  | LRU evictions from the batcher's connection cache       |

### 7.2 Structured logging

Every flush emits:

```rust
tracing::debug!(
    target: "the_one_mcp::audit_batcher",
    project_id = %key.project_id,
    rows = batch.len(),
    latency_ms = elapsed.as_millis(),
    "audit batch flushed"
);
```

Every error emits at `warn!` with a correlation ID.

### 7.3 Alerting recommendations

Sample PromQL-like expressions (the broker exposes Prometheus via
the existing observability layer):

- `rate(audit_batcher_queue_timeouts_total[5m]) > 0` — queue is
  saturated, callers are timing out.
- `audit_batcher_queue_depth / audit_batcher_max_project_depth > 0.8`
  — close to capacity, consider increasing `max_queue_depth_per_project`.
- `rate(audit_batcher_flush_errors_total[5m]) > 0.1` — persistent
  flush failures, investigate SQLite health.
- `rate(audit_batcher_dropped_flush_failures[1h]) > 0` — **data
  loss**, investigate immediately.

---

## 8. Rollout plan

### 8.1 Phased adoption

**Phase 0: Feature-flagged, default off.**
- Add `audit_batcher_enabled: bool = false` to `AppConfig`.
- When false, broker methods continue to call the synchronous
  `record_audit`. When true, they use `queue_audit`.
- Zero risk for existing deployments.

**Phase 1: Default-on in `v0.17.0-rc1` under a `--canary` flag on
the binary.**
- `scripts/install.sh --canary` opts into the new path.
- Dogfood for 1 week on the maintainer's own workspace.
- Soak test at 10× normal write rate to exercise backpressure.

**Phase 2: Default-on in v0.17.0.**
- Flip the config default to `true`.
- Release notes include the durability disclaimer + rollback
  instructions (set `audit_batcher_enabled = false` in
  `~/.the-one/config.json`).
- Keep the synchronous path in the codebase for at least two
  release cycles.

**Phase 3: Remove the synchronous path in v0.18.0 if no issues reported.**
- Only if phase 2 has run for at least 4 weeks with no regression
  reports.
- Even then, keep `record_audit_sync` as a crate-internal API for
  tests.

### 8.2 Rollback story

1. Set `audit_batcher_enabled = false` in project or global config.
2. Restart the broker.
3. All new writes go through the synchronous path; any still-queued
   records in the batcher get flushed by shutdown then the task
   exits.
4. No data migration needed.

### 8.3 Deprecation of `record_audit` (synchronous)

Not planned. The synchronous path remains available for:

- Integration tests that need deterministic read-after-write
  semantics.
- Admin operations where losing the audit row on crash is
  unacceptable (e.g. `maintain: backup` / `restore`).
- Emergency disable of the batcher without restarting.

---

## 9. Alternative approaches considered

### 9.1 Rewrite audit writes via `spawn_blocking` but keep them synchronous

**Idea:** instead of batching, just move the synchronous
`db.record_audit` call off the async executor via
`tokio::task::spawn_blocking`.

**Rejected because:**
- This doesn't reduce fsync count — still one commit per audit row.
- Lever 1 (`synchronous=NORMAL`) already eliminates the fsync bottleneck.
- The only remaining cost is prepared-statement overhead, which is
  measured in single-digit microseconds per call.
- Adds `spawn_blocking` overhead (~10µs) that would partly cancel
  the gain.

Net effect: ~zero. Not worth the complexity.

### 9.2 Use `rusqlite` transaction-per-call with manual `BEGIN IMMEDIATE; … COMMIT;`

**Idea:** same as today but explicit transaction boundaries.

**Rejected because:**
- rusqlite's `execute` already wraps each call in an implicit
  transaction. No gain.

### 9.3 Store audit rows in a separate SQLite DB per project

**Idea:** move audit rows out of `state.db` into `audit.db` so
audit writes don't contend with data writes on the main WAL.

**Rejected because:**
- WAL mode already gives readers + one writer concurrency on a
  single file.
- Two files doubles the fsync cost, not halves it.
- Complicates backup/restore.

### 9.4 Use a different storage engine for audit (RocksDB, sled, LMDB)

**Idea:** audit logs are append-only; a log-structured store would
be faster.

**Rejected because:**
- Adds a multi-MB dependency for a workload where Lever 1 already
  fits comfortably in the performance envelope.
- Backup/restore has to handle two file formats.
- Operators have to learn a second storage engine's recovery
  semantics.
- Not justified unless audit writes become > 10% of broker wall
  clock, which Lever 1 makes implausible.

### 9.5 Ship audit rows over UDP to an external collector

**Idea:** fire-and-forget audit rows to a syslog/Loki/Fluent Bit
endpoint, skip local SQLite entirely.

**Rejected because:**
- Breaks the local-first, air-gap-friendly guarantee of
  the-one-mcp.
- Adds an external dependency the operator must run.
- Loses the integration with `observe: audit_events` which works
  on the same DB.

### 9.6 Use `tokio::sync::Mutex<Connection>` in the broker

**Idea:** cache a `Mutex<Connection>` per project in the broker
itself and serialise writes through it. No separate batcher task.

**Rejected because:**
- Doesn't batch: still one commit per audit row.
- Introduces mutex contention on the request path — the opposite
  of what we want.
- Doesn't buy anything over the current "open per call" pattern
  once Lever 1 is in place.

---

## 10. Open questions

These need answers BEFORE starting implementation:

1. **Config location**: project-level (`.the-one/config.json`) or
   global (`~/.the-one/config.json`) or both? Batcher state is
   global to the broker, so a per-project setting would be
   misleading. Recommendation: global-only.

2. **Metric exposure path**: do we surface the new metrics via the
   existing `observe: metrics` action, a new
   `observe: audit_batcher` action, or both? Recommendation: add
   fields to the existing `MetricsSnapshotResponse` + a dedicated
   `observe: audit_batcher` admin action for detailed inspection
   (queue depth per project, cache contents).

3. **Flush on project eviction from DB cache**: when the LRU
   evicts a project's cached `ProjectDatabase`, should we force a
   final flush for that project first? Recommendation: yes, always.
   Otherwise an idle project could lose buffered records at
   eviction time.

4. **Interaction with file watcher**: the file watcher already
   spawns its own tokio task and calls broker methods. Those calls
   will now go through the batcher. Does that introduce a cycle?
   Recommendation: audit — I think it's fine because the batcher
   task never calls back into the broker, but confirm before
   shipping.

5. **Backpressure policy**: should a saturated queue cause
   `queue_audit` to return an error (current plan), silently drop
   the record (alternative), or synchronously write through
   (degrades to Lever 1 mode)? Recommendation: return an error,
   let the caller decide. Provides observability via
   `audit_queue_timeouts_total`.

6. **Benchmark target**: what per-row latency target justifies
   shipping this? Recommendation: require at least 5× improvement
   over Lever 1 baseline under realistic load (so ~17µs per row
   amortised). If the real number is less than 3×, the complexity
   isn't worth it.

---

## 11. Pre-implementation checklist

Before opening a PR, confirm:

- [ ] Lever 1 has been in production for at least 2 weeks without
      issue.
- [ ] A real flamegraph shows `record_audit` above 1% of broker CPU
      under production load.
- [ ] The answers to all six open questions in § 10 are documented
      and approved.
- [ ] The test plan in § 6 has been reviewed by a second engineer.
- [ ] `scripts/release-gate.sh` is updated with the new tests.
- [ ] `docs/guides/production-hardening-v0.15.md` has a new § 16
      cross-referencing this plan document.
- [ ] `docs/guides/mempalace-operations.md` has a new
      "Audit batching configuration" subsection.

---

## 12. File inventory

Planned files for the implementation:

### New

- `crates/the-one-core/src/audit/batcher.rs` — `AuditBatcher`,
  `AuditBatcherInner`, `BatcherMsg`, config struct, task loop.
- `crates/the-one-core/src/audit/batcher_tests.rs` — unit tests
  listed in § 6.1 (11 tests).
- `crates/the-one-core/benches/audit_batcher_bench.rs` — standalone
  benchmark wrapping the existing production_hardening_bench.

### Modified

- `crates/the-one-core/src/audit.rs` — convert to a `mod` directory
  with `mod.rs` re-exporting `batcher`.
- `crates/the-one-core/src/lib.rs` — export `AuditBatcher`.
- `crates/the-one-core/src/config.rs` — new config section.
- `crates/the-one-mcp/src/broker.rs` — spawn the batcher in `new`,
  switch every `db.record_audit(...)` call site to
  `self.audit_batcher.queue(...)`, add shutdown hook.
- `crates/the-one-mcp/src/api.rs` — extend `MetricsSnapshotResponse`.
- `crates/the-one-mcp/src/bin/the-one-mcp.rs` — SIGTERM → broker
  shutdown → batcher final flush.
- `crates/the-one-mcp/tests/stdio_write_path.rs` — new integration
  test.
- `crates/the-one-mcp/tests/production_hardening.rs` — new
  regression test.
- `crates/the-one-core/examples/production_hardening_bench.rs` —
  new "Lever 2" section measuring batched vs direct throughput.
- `docs/guides/production-hardening-v0.15.md` — document the new
  flag, durability semantics, rollback.
- `docs/guides/mempalace-operations.md` — operator tuning guide.

### Deleted

Nothing. The synchronous path stays as a crate-internal safety net.

---

## 13. Estimated complexity breakdown

| area                                     | LOC  | hours |
|------------------------------------------|-----:|------:|
| `AuditBatcher` + task + config           |  350 |   4   |
| Broker wiring + metric plumbing          |  150 |   2   |
| Unit tests (§ 6.1 × 11)                  |  400 |   4   |
| Integration tests (§ 6.2 × 2)            |  120 |   1   |
| Benchmark extension                      |   80 |   0.5 |
| Documentation (this file + 2 guides)     |  200 |   1.5 |
| Rollout work (feature flag, canary, etc) |   50 |   1   |
| Code review + iterate                    |    - |   3   |
| Soak / stress testing (§ 6.4)            |    - |   4   |
| **Total**                                |1 350 |  21   |

At 6 productive hours per engineer-day: **~3.5 engineer-days**.

---

## 14. What to do if this plan bit-rots

If more than 6 months elapse without implementation, re-validate:

1. Has Lever 1 remained sufficient? Check the broker's actual
   `record_audit` wall-clock percentage.
2. Has the tokio ecosystem shipped a better primitive (e.g. a
   built-in channel batcher)?
3. Have we adopted any new persistence backend that makes this
   plan moot (e.g. Redis WAL, Kafka)?
4. Has the audit schema grown columns that invalidate the batch
   INSERT shape?

Update this file with the date, findings, and decision before
starting implementation.

---

## 15. See also

- **Lever 1 (shipped v0.15.1)**:
  `docs/guides/production-hardening-v0.15.md` § 14
- **Findings report**:
  `docs/reviews/2026-04-10-mempalace-comparative-audit.md`
- **Existing audit module**:
  `crates/the-one-core/src/audit.rs`
- **Benchmark**:
  `crates/the-one-core/examples/production_hardening_bench.rs`
- **Broker metrics wiring** (pattern to follow for the new counters):
  `crates/the-one-mcp/src/broker.rs` — `BrokerMetrics` struct and
  `metrics_snapshot` method.

---

## 16. Open issues — why this draft was superseded

Self-review of this document on 2026-04-10 (same day it was written)
identified four safety gaps and six completeness gaps that would have
either (a) blocked an implementer on days 1-2 with unresolved technical
ambiguities, or (b) shipped production bugs on rollout. The resolutions
for each issue live in the v2 document
(`2026-04-10-audit-batching-lever2.md`). This section preserves the
critique exactly as written so future maintainers can see what was
wrong with the first cut.

### Safety issues

**S1. 3-strike retry-then-drop creates silent data loss.**
§ 5.2 says: *"if the same bucket fails to flush three times in a row,
drop the bucket"*. That quietly deletes up to 256 audit rows. For a
log whose entire purpose is post-incident forensics, this is the exact
failure mode the log is supposed to prevent. The counter doesn't help —
by the time an operator looks at it, the context is lost.

**Fix in v2:** dropped buckets are written to a poison queue file
(`.the-one/audit.poison.jsonl`) before being cleared from memory. File
is line-delimited JSON, rotated at 100 MB, with a recovery tool
that replays it into `audit_events`.

**S2. Backpressure policy is ambiguous → either user-visible writes
break or audit rows vanish silently.**
§ 5.3 says: *"Callers decide whether to propagate or swallow."* There
are only two real choices and both are bad:
- Propagate → every broker write fails when the audit queue is full.
- Swallow → audit rows disappear during overload, which is when they
  are needed most.

**Fix in v2:** on saturation, degrade to synchronous (`Lever 1`-mode)
audit writes for the duration. Costs some latency, loses nothing.

**S3. No overall memory bound.**
§ 3.2 specifies `max_queue_depth_per_project = 10_000` with no global
cap. 100 projects × 10k records × ~200 bytes = **200 MB of buffered
state** in a worst-case multi-tenant deployment. On a 2 GB container
this is a real risk.

**Fix in v2:** added `max_queue_depth_total` (default 50 000) that
applies across all projects; crossing it triggers the S2 synchronous
degradation path.

**S4. Loss-window math is wrong.**
§ 5.1 claims *"64 rows/sec of unaudited data in the worst case"*. The
real worst case is `max(batch_max_rows, throughput × flush_interval)`.
At 5 k writes/sec with a 250 ms flush interval that's 1 250 rows, not
64. "Acceptable" framing in the draft was based on wrong arithmetic.

**Fix in v2:** corrected math in § 5.1, plus a tuning table showing
worst-case loss at various throughputs.

### Completeness issues

**C1. Per-project backpressure has no actual mechanism.**
§ 3.2 mentions `max_queue_depth_per_project` but doesn't say how
`queue()` checks it. The channel is a single `mpsc::Sender<BatcherMsg>`
that doesn't know which project a message belongs to. Options exist
(shared DashMap, oneshot round-trip, broker-side cache), plan picks
none.

**Fix in v2:** explicit decision — `Arc<DashMap<ProjectKey, AtomicUsize>>`
shared between broker and batcher. `queue()` increments on enqueue,
batcher decrements on flush. Hot path cost is one atomic load + one
atomic increment.

**C2. `spawn_blocking` + `rusqlite::Connection` ownership is
hand-waved.**
§ 5.6 says "wrap the SQLite flush in `tokio::task::spawn_blocking`".
`spawn_blocking` takes ownership of its closure. The batcher task owns
`HashMap<ProjectKey, ProjectDatabase>`. Moving the DB into a blocking
task and back requires a `take`/reinsert dance or `Arc<Mutex<...>>`
wrapping — neither is specified, and both change the concurrency
story.

**Fix in v2:** concrete pattern sketched — bucket holds
`Mutex<Option<ProjectDatabase>>`, flush takes Some, hands to
spawn_blocking, receives back, reinserts. New records during flush
go into a staging `Vec` on the bucket that gets drained post-flush.

**C3. SQLite error injection for tests #7 and #8 has no mechanism.**
§ 6.1 says tests should "inject a failure on the first flush". There's
no way to do that against real rusqlite today.

**Fix in v2:** introduce a small `trait AuditSink` inside the batcher
module with one method (`fn flush(&self, records: &[AuditRecord])`).
Real impl wraps `ProjectDatabase`; test-only impl in `cfg(test)` has
a configurable fail-next counter. Adds ~30 LOC of abstraction and
makes the tests trivial.

**C4. Shutdown ordering with the existing file watcher is not
sequenced.**
§ 5.4 mentions clean shutdown but the-one-mcp already has a file
watcher task (`maybe_spawn_watcher`) that calls broker methods. After
Lever 2, those calls route audit rows through the batcher. The
shutdown sequence must be: refuse new requests → drain in-flight →
stop watcher → stop batcher → drop broker. The plan doesn't specify
this.

**Fix in v2:** explicit 6-step shutdown protocol in § 5.5, with a new
`McpBroker::shutdown() -> impl Future` method signature and a
`ShutdownGuard` that the bin's SIGTERM handler awaits.

**C5. The `--canary` rollout flag doesn't exist in the-one-mcp.**
§ 8.1 says `scripts/install.sh --canary` enables the new path. There's
no canary infrastructure today.

**Fix in v2:** rollout uses an `audit_batcher_enabled: bool` AppConfig
field (default `false` in v0.16, flip to `true` in v0.17, remove the
flag in v0.18). No CLI changes.

**C6. No concrete answer for the CI test matrix under a feature flag.**
§ 8 acknowledges the flag creates two code paths but doesn't commit to
how tests cover both.

**Fix in v2:** dedicated tests for the batcher path + a one-line env
var override (`THE_ONE_AUDIT_BATCHER=1`) on the existing
`stdio_write_path` integration tests so the stdio path is exercised
under both flag states. Doubles only the 9 integration tests, not
every unit test.

### Good parts of this draft that v2 preserves verbatim

- Architecture at § 3.1 (one batcher task, per-project buckets).
- Rollout phasing at § 8.
- Alternative approaches considered at § 9 (all six rejections).
- Non-goals at § 2.
- Observability metrics and alert recommendations at § 7.
- The decision to NOT implement right now at § 1.

Those sections were correct on first draft and are not re-litigated in
v2. Read them here; v2 points at them.
