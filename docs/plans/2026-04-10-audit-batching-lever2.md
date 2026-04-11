# Audit Log Batching (Lever 2) — Implementation Plan v2

**Status:** Ready for implementation (not yet scheduled).
**Owner:** unassigned
**Prerequisite:** v0.15.1 (Lever 1, `synchronous=NORMAL`) — shipped.
**Supersedes:** `draft-2026-04-10-audit-batching-lever2.md` (same date).
**Estimated effort:** 1 350 LOC, ~25 engineer-hours (v2 adds ~4 hours
over the draft for the extra safety work).
**Target release:** v0.17.0 (behind a feature flag), default-on in v0.18.0.

---

## Quick reference

| Question                               | v2 answer |
|-----------------------------------------|-----------|
| Drop audit rows on persistent failure?  | **No.** Poison-queue to `audit.poison.jsonl`. |
| Queue saturation behavior?              | **Degrade to synchronous writes** (Lever 1 mode). |
| Global memory cap?                      | **Yes** — `max_queue_depth_total = 50 000`. |
| Per-project backpressure tracking?      | `Arc<DashMap<ProjectKey, AtomicUsize>>` hot path. |
| SQLite flush threading?                 | `spawn_blocking` with take/reinsert + staging buffer. |
| Test-time error injection?              | `trait AuditSink` + `FailingSink` under `cfg(test)`. |
| Feature flag mechanism?                 | `audit_batcher_enabled: bool` in `AppConfig`. |
| Shutdown sequence?                      | Explicit 6-step protocol, `McpBroker::shutdown()`. |
| Decision to implement now?              | **No.** Document ready for when Lever 1 stops being enough. |

---

## 1. Trigger conditions

Same as the draft: implement only when one of these holds.

1. `record_audit` > 1% of broker CPU in a production flamegraph.
2. Deployment sustains > 10 000 state-changing calls/sec.
3. Broker p99 regresses and traces to audit writes.

Lever 1 already delivers ~11 000 audit writes/sec single-threaded
(85 µs/row). For any current or foreseeable the-one-mcp deployment
Lever 1 is sufficient.

## 2. Non-goals

See draft § 2. Unchanged in v2.

## 3. What the draft got right (preserved)

- **Architecture sketch at § 3.1 of the draft** — one batcher task,
  per-project buckets, channel-based enqueue. Still correct.
- **Rollout phasing at § 8 of the draft** — feature flag → canary →
  default-on → remove. v2 keeps it, only swapping the canary
  mechanism (§ 11 below).
- **Rejected alternatives at § 9 of the draft** — 6 wrong answers
  identified and rejected. v2 does not re-litigate them. If a reader
  wonders "why not just X", consult the draft.
- **Non-goals at § 2 of the draft** — unchanged.
- **Observability metric list at § 7 of the draft** — 10 metrics,
  v2 adds 3 more (see § 8 below).

The remainder of this document covers only the parts that changed.

---

## 4. Safety resolutions

### 4.1 S1 — Poison queue for persistent flush failures

**Decision:** when a bucket fails to flush 3 times in a row, **do
not drop it**. Write it to a poison queue file.

**File format:**
```
.the-one/audit.poison.jsonl
```
Line-delimited JSON, one record per line. Each line is:
```json
{
  "timestamp": "2026-04-10T14:23:45.123Z",
  "project_id": "my-project",
  "record": {
    "operation": "memory.ingest_conversation",
    "params_json": "{...}",
    "outcome": "ok",
    "error_kind": null
  },
  "drop_reason": "sqlite_error: database is locked (3 retries)",
  "retry_count": 3
}
```

**Rotation:**
- File rotated when it exceeds 100 MB.
- Rotation format: `audit.poison.jsonl.1`, `.2`, `.3`, with the oldest
  deleted when a fourth is needed. Max 300 MB total.
- New metric `audit_batcher_poison_rotations_total` increments on
  every rotation.
- New metric `audit_batcher_poison_rows_total` increments on every
  appended line.

**Recovery:**
- A new admin action `maintain: action: replay_audit_poison` reads
  the file line by line, parses each entry, and attempts a
  synchronous `db.record_audit(record)`. Successful replays are
  removed from the file; failures stay.
- Document this in the production hardening guide as the operator
  recovery path.

**Implementation sketch:**
```rust
// crates/the-one-core/src/audit/poison.rs  (new)

pub struct PoisonQueue {
    path: PathBuf,
    max_bytes: u64,
    max_generations: usize,
}

impl PoisonQueue {
    pub fn append(
        &self,
        project_id: &str,
        record: &AuditRecord,
        drop_reason: &str,
        retry_count: u32,
    ) -> Result<(), CoreError> {
        self.maybe_rotate()?;
        let entry = serde_json::json!({
            "timestamp": current_iso8601(),
            "project_id": project_id,
            "record": record,
            "drop_reason": drop_reason,
            "retry_count": retry_count,
        });
        let line = serde_json::to_string(&entry)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
    fn maybe_rotate(&self) -> Result<(), CoreError> { /* … */ }
}
```

**Why file-based instead of a separate SQLite table:**
- The poison queue exists because the primary SQLite store is
  failing. Writing to SQLite to record SQLite failures is the worst
  form of turtles. Appending to a file works even when SQLite is
  locked, corrupted, or out of disk quota.
- JSONL is trivially replayable, `grep`-pable, and obvious to an
  operator who opens it in an editor.

**Regression guards:**
- `test_poison_queue_appends_dropped_bucket` — fake a persistent
  flush failure, assert the poison file exists with N lines.
- `test_poison_queue_rotates_at_max_bytes` — seed to near-max, write
  one more, assert rotation.
- `test_replay_audit_poison_drains_file_on_success` — happy path.

### 4.2 S2 — Saturation degrades to synchronous writes

**Decision:** when the batcher queue is saturated AND the
`queue_timeout` expires, `queue_audit` does NOT return an error to
the broker. It falls back to a synchronous write against the same
`ProjectDatabase` the broker already has open.

**API shape:**
```rust
// the-one-core/src/audit/batcher.rs

/// Outcome of a queue attempt.
pub enum QueueOutcome {
    /// Record was accepted into the async batcher.
    Queued,
    /// Batcher is saturated — caller should write this record
    /// synchronously via `db.record_audit(&record)` to avoid losing it.
    /// `reason` is a short label for metrics.
    FallbackToSync { reason: &'static str },
}

impl AuditBatcher {
    /// Non-blocking enqueue. Never returns an error that the broker
    /// would propagate to the client. A saturated queue returns
    /// `FallbackToSync` so the caller can drop to Lever-1 mode.
    pub async fn queue(
        &self,
        project_root: &Path,
        project_id: &str,
        record: AuditRecord,
    ) -> QueueOutcome;
}
```

**Broker call-site pattern (one per state-changing method):**
```rust
// broker.rs — memory_ingest_conversation et al.
let audit = AuditRecord::ok("memory.ingest_conversation", params);
match self
    .audit_batcher
    .queue(project_root, &request.project_id, audit.clone())
    .await
{
    QueueOutcome::Queued => {}
    QueueOutcome::FallbackToSync { reason } => {
        tracing::warn!(
            target: "the_one_mcp::audit_batcher",
            reason,
            "audit queue saturated; falling back to synchronous write"
        );
        self.metrics
            .audit_batcher_fallback_sync_total
            .fetch_add(1, Ordering::Relaxed);
        if let Err(err) = db.record_audit(&audit) {
            // Synchronous write also failed — last resort:
            // write to poison queue directly.
            self.audit_batcher.poison_direct(&request.project_id, &audit, &format!("sync_fallback_failed: {err}"));
        }
    }
}
```

**What "saturated" means in practice:**
- Global: total queued records across all projects ≥ `max_queue_depth_total`.
- Per-project: a single project's bucket ≥ `max_queue_depth_per_project`.
- Time-based: `queue_timeout` elapsed waiting for channel capacity.

**Trade-off:**
- Under saturation, audit writes slow to Lever-1 speed (~85 µs/row).
- That's ~10 000× slower than the batched path but still fast enough
  to keep up with most write storms.
- If even Lever-1 can't keep up, the broker's request path itself
  becomes the bottleneck — audit was never the limiter.
- Zero data loss in all cases except process crash during the
  synchronous write, which is the same risk the broker already
  carries for every other SQLite write.

**Regression guards:**
- `test_saturation_degrades_to_sync_not_error` — fill the queue,
  verify the broker method succeeds via the sync path.
- `test_saturation_metric_increments` — verify
  `audit_batcher_fallback_sync_total` counts correctly.
- `test_sync_fallback_failure_hits_poison_queue` — make both the
  batcher AND the direct SQLite call fail, verify the record lands
  in the poison file.

### 4.3 S3 — Global memory bound

**Decision:** add `max_queue_depth_total = 50 000` to the config.
Tracked via `Arc<AtomicUsize>` shared between `queue()` and the
batcher task. Crossing the bound triggers the S2 `FallbackToSync`
path with `reason = "global_depth"`.

**Why 50 000:**
- At the default ~200 bytes per `AuditRecord` (JSON params + metadata),
  50 000 records ≈ 10 MB RSS. Safe on any container >= 256 MB.
- 50 000 records ÷ 256 batch_max_rows ÷ 4 flushes/sec = ~49 seconds
  of buffering headroom at the default flush cadence. Ops have time
  to react to depth-nearing-cap alerts.
- Operators who need more can raise it explicitly; the default
  protects the 99% case.

**Implementation:**
```rust
struct AuditBatcherInner {
    sender: mpsc::Sender<BatcherMsg>,
    global_depth: AtomicUsize,          // ← NEW
    per_project_depth: DashMap<ProjectKey, AtomicUsize>, // ← NEW, see § 5.1
    poison: Arc<PoisonQueue>,           // ← NEW for S1
    max_global: usize,
    max_per_project: usize,
    queue_timeout: Duration,
    metrics: Arc<BatcherMetrics>,
}
```

### 4.4 S4 — Correct loss-window math

**Real worst case:**
```
max_rows_in_memory = min(
    max_queue_depth_total,
    throughput_per_sec * flush_interval_sec + batch_max_rows
)
```

On a kernel panic during a write storm, you lose `max_rows_in_memory`
records. At default config:
- `flush_interval = 250 ms`, `batch_max_rows = 256`,
  `max_queue_depth_total = 50 000`

| sustained throughput | rows in memory (typical) | rows lost on panic |
|----------------------|-------------------------:|-------------------:|
|    100 rows/sec      |  ~25 + 256 = **281**     |  281 |
|  1 000 rows/sec      |  ~250 + 256 = **506**    |  506 |
|  5 000 rows/sec      |  ~1 250 + 256 = **1 506** | 1 506 |
| 20 000 rows/sec      |  min(5 256, 50 000) = **5 256** | 5 256 |
| 100 000 rows/sec     | capped at **50 000** (saturation triggers S2) | 50 000 + sync-fallback losses |

The draft's "64 rows/sec" was simply wrong arithmetic. Real numbers
above are what the production hardening guide will publish.

---

## 5. Completeness resolutions

### 5.1 C1 — Per-project backpressure mechanism

**Decision:** use `Arc<DashMap<ProjectKey, AtomicUsize>>` shared
between `queue()` (on the request path) and the batcher task (on the
flush path).

**Why DashMap:** lock-free reads via sharded hashing, safe concurrent
mutation, Arc-clonable. Depth is a `AtomicUsize` inside each entry so
we never hold the DashMap write lock during the hot path.

**Hot path (`queue`):**
```rust
pub async fn queue(
    &self,
    project_root: &Path,
    project_id: &str,
    record: AuditRecord,
) -> QueueOutcome {
    let key = ProjectKey::new(project_root, project_id);

    // Global depth check (1 atomic load).
    let global = self.inner.global_depth.load(Ordering::Relaxed);
    if global >= self.inner.max_global {
        self.inner.metrics.fallback_sync_global.fetch_add(1, Relaxed);
        return QueueOutcome::FallbackToSync { reason: "global_depth" };
    }

    // Per-project depth check (1 DashMap read + 1 atomic load).
    let entry = self
        .inner
        .per_project_depth
        .entry(key.clone())
        .or_insert_with(|| AtomicUsize::new(0));
    let per_proj = entry.load(Ordering::Relaxed);
    if per_proj >= self.inner.max_per_project {
        self.inner.metrics.fallback_sync_project.fetch_add(1, Relaxed);
        return QueueOutcome::FallbackToSync { reason: "project_depth" };
    }

    // Increment before send so the next caller sees the slot taken.
    entry.fetch_add(1, Ordering::Relaxed);
    self.inner.global_depth.fetch_add(1, Ordering::Relaxed);
    drop(entry);   // release DashMap ref

    // Actual send — mpsc::send is async and bounded.
    match tokio::time::timeout(
        self.inner.queue_timeout,
        self.inner.sender.send(BatcherMsg::Record { key, record }),
    ).await {
        Ok(Ok(())) => QueueOutcome::Queued,
        Ok(Err(_closed)) | Err(_timeout) => {
            // Roll back the counters we incremented above.
            self.rollback_depth(&key);
            self.inner.metrics.fallback_sync_timeout.fetch_add(1, Relaxed);
            QueueOutcome::FallbackToSync { reason: "queue_timeout" }
        }
    }
}
```

**Decrement on flush (batcher task):**
```rust
// inside flush_bucket(...)
let flushed = batch.len();
match sink.flush_records(&batch) {
    Ok(()) => {
        self.inner.per_project_depth
            .get(&key).map(|e| e.fetch_sub(flushed, Relaxed));
        self.inner.global_depth.fetch_sub(flushed, Ordering::Relaxed);
    }
    Err(e) => { /* retry path — see § 5.2 */ }
}
```

**Hot path cost budget:**
- 1 atomic load (global_depth)
- 1 DashMap read (amortised O(1))
- 1 atomic load (project_depth)
- 1 atomic fetch_add (project_depth)
- 1 atomic fetch_add (global_depth)
- 1 async send (amortised ~200 ns on bounded channel)

Total: ~500 ns per `queue` call. Safe for the broker's hot path.

### 5.2 C2 — `spawn_blocking` + Connection ownership (the fiddly bit)

**Decision:** flush runs inside `tokio::task::spawn_blocking`, with a
take/reinsert pattern and a staging buffer to preserve ordering.

**Bucket structure:**
```rust
struct Bucket {
    /// Primary queue — the batcher task appends here until a flush
    /// starts, then swaps with `staging`.
    primary: Vec<AuditRecord>,
    /// Staging — receives new records while a flush is in progress.
    staging: Vec<AuditRecord>,
    /// True if a flush is currently in flight for this bucket. New
    /// records go to `staging` when this is set.
    flush_in_flight: bool,
    /// Last time the bucket was flushed (for interval-driven flushes).
    last_flush: Instant,
    /// Consecutive flush failure count (for the 3-strike poison path).
    consecutive_failures: u32,
    /// The project DB, moved in and out of spawn_blocking during flush.
    /// `None` iff currently being used by a spawn_blocking task.
    db: Option<ProjectDatabase>,
}
```

**Flush logic (batcher task, async):**
```rust
async fn flush_bucket(
    &self,
    key: ProjectKey,
    buckets: Arc<Mutex<HashMap<ProjectKey, Bucket>>>,
) {
    // Critical section 1: swap primary into a local vec, take the DB.
    let (batch, db_opt) = {
        let mut guard = buckets.lock().await;
        let bucket = match guard.get_mut(&key) {
            Some(b) if !b.primary.is_empty() && !b.flush_in_flight => b,
            _ => return,
        };
        bucket.flush_in_flight = true;
        let batch = std::mem::take(&mut bucket.primary);
        let db = bucket.db.take();  // may be None if bucket is fresh
        (batch, db)
    };

    // Lazy-open the DB on first flush, or if the cache evicted it.
    let db = match db_opt {
        Some(db) => db,
        None => match ProjectDatabase::open(&key.project_root, &key.project_id) {
            Ok(db) => db,
            Err(e) => {
                self.reinsert_on_failure(&key, &buckets, batch, &e).await;
                return;
            }
        },
    };

    // Actual SQLite work happens on the blocking pool. The closure
    // owns `db` and `batch`, returning them both on completion so we
    // can reinsert cleanly.
    let result = tokio::task::spawn_blocking(move || {
        let flush_result = db.record_audit_batch(&batch);
        (db, batch, flush_result)
    })
    .await;

    let (db, batch, flush_result) = match result {
        Ok(tuple) => tuple,
        Err(join_err) => {
            // spawn_blocking panic — this is the fallback for an
            // unexpected panic inside rusqlite. Treat as flush failure.
            tracing::error!(error = ?join_err, "audit batch flush panicked");
            self.reinsert_on_panic(&key, &buckets).await;
            return;
        }
    };

    // Critical section 2: merge staging into primary, reinsert DB,
    // clear flush_in_flight.
    let mut guard = buckets.lock().await;
    let bucket = guard.get_mut(&key).expect("bucket was present");
    bucket.db = Some(db);
    bucket.flush_in_flight = false;
    bucket.last_flush = Instant::now();

    match flush_result {
        Ok(_) => {
            bucket.consecutive_failures = 0;
            // Anything arrived during the flush?
            if !bucket.staging.is_empty() {
                bucket.primary.append(&mut bucket.staging);
            }
            // Decrement per-project + global depth counters.
            self.decrement_depth(&key, batch.len());
        }
        Err(e) => {
            bucket.consecutive_failures += 1;
            if bucket.consecutive_failures >= 3 {
                // Poison queue path — write ALL batch records and
                // clear the bucket. Also pull staging so no poison
                // rows get silently buffered.
                for rec in &batch {
                    let _ = self.inner.poison.append(
                        &key.project_id, rec, &format!("{e}"), 3
                    );
                }
                for rec in bucket.staging.drain(..) {
                    let _ = self.inner.poison.append(
                        &key.project_id, &rec, "collateral_on_3rd_strike", 3
                    );
                }
                self.decrement_depth(&key, batch.len());
                self.inner.metrics.poisoned_total.fetch_add(batch.len() as u64, Relaxed);
                bucket.consecutive_failures = 0;
            } else {
                // Retry: put the batch back at the front of primary.
                let mut requeued = batch;
                requeued.append(&mut bucket.primary);
                bucket.primary = requeued;
            }
        }
    }
}
```

**New method on `ProjectDatabase`:**
```rust
// crates/the-one-core/src/storage/sqlite.rs

/// Write a batch of audit records in a single transaction.
/// Used by the Lever 2 batcher. The sync `record_audit` method is
/// equivalent to calling this with a single-record slice.
pub fn record_audit_batch(
    &self,
    records: &[AuditRecord],
) -> Result<(), CoreError> {
    let tx = self.conn.unchecked_transaction()?;
    let mut stmt = tx.prepare_cached(
        "INSERT INTO audit_events(project_id, event_type, payload_json, outcome, error_kind, created_at_epoch_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, CAST(strftime('%s','now') AS INTEGER) * 1000)"
    )?;
    for rec in records {
        stmt.execute(params![
            self.project_id,
            rec.operation,
            rec.params_json,
            rec.outcome.as_str(),
            rec.error_kind,
        ])?;
    }
    drop(stmt);
    tx.commit()?;
    Ok(())
}
```

**Why `unchecked_transaction`:** `rusqlite::Connection` stores the
Transaction as a borrow, which conflicts with passing the connection
through `spawn_blocking`. `unchecked_transaction` returns a
non-borrowing guard and we manually commit. Standard pattern for
rusqlite + async.

**Ordering guarantee:** within a single `ProjectKey`, records are
appended to `bucket.primary` by the batcher task in the order they
arrive on the channel (FIFO). During a flush, the primary buffer is
swapped out whole and written in-order inside one transaction. New
records queued during the flush go to `bucket.staging`, which gets
appended to primary post-flush. So the on-disk order is always the
queue order. **Guaranteed.**

### 5.3 C3 — Test-time error injection via `trait AuditSink`

**Decision:** introduce a minimal trait so tests can swap in a
failing implementation without touching rusqlite.

```rust
// crates/the-one-core/src/audit/sink.rs (new)

/// The interface the batcher uses to persist a flushed bucket.
/// Real implementation delegates to `ProjectDatabase::record_audit_batch`.
/// Tests can swap in a `FailingSink` to exercise retry/poison paths.
pub trait AuditSink: Send + 'static {
    fn flush(&mut self, records: &[AuditRecord]) -> Result<(), CoreError>;
}

/// Real sink backed by a ProjectDatabase.
pub struct SqliteSink {
    db: ProjectDatabase,
}

impl AuditSink for SqliteSink {
    fn flush(&mut self, records: &[AuditRecord]) -> Result<(), CoreError> {
        self.db.record_audit_batch(records)
    }
}

#[cfg(test)]
pub struct FailingSink {
    pub fail_next_n: std::sync::atomic::AtomicU32,
    pub records_written: std::sync::Mutex<Vec<AuditRecord>>,
}

#[cfg(test)]
impl AuditSink for FailingSink {
    fn flush(&mut self, records: &[AuditRecord]) -> Result<(), CoreError> {
        let remaining = self.fail_next_n.load(Ordering::Relaxed);
        if remaining > 0 {
            self.fail_next_n.store(remaining - 1, Ordering::Relaxed);
            return Err(CoreError::Sqlite(rusqlite::Error::ExecuteReturnedResults));
        }
        self.records_written.lock().unwrap().extend_from_slice(records);
        Ok(())
    }
}
```

The bucket then holds `Box<dyn AuditSink>` instead of
`Option<ProjectDatabase>`. The batcher task owns the sink for the
bucket's lifetime. `spawn_blocking` now takes the sink:
```rust
let (sink, batch, result) = tokio::task::spawn_blocking(move || {
    let r = sink.flush(&batch);
    (sink, batch, r)
}).await?;
```

**Regression guards using `FailingSink`:**
- `test_batcher_retries_on_single_failure` — fail_next_n=1, verify
  2nd flush succeeds.
- `test_batcher_poisons_on_3_consecutive_failures` — fail_next_n=3,
  verify poison queue has N entries.
- `test_batcher_staging_records_flush_after_retry` — queue 10 records,
  fail first flush, queue 5 more during retry, verify all 15 land.

### 5.4 C4 — Shutdown sequencing

**Decision:** the broker gets a new `shutdown() -> impl Future<Output
= ShutdownReport>` method that sequences teardown explicitly.

**6-step protocol:**
```rust
impl McpBroker {
    pub async fn shutdown(&self) -> ShutdownReport {
        // Step 1: refuse new requests.
        self.shutting_down.store(true, Ordering::Release);
        tracing::info!("broker entering shutdown");

        // Step 2: drain in-flight broker methods. Each method checks
        // `shutting_down` at entry and returns a `ServiceUnavailable`
        // if set, so after a short quiesce no new writes start.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Step 3: stop the file watcher. The watcher holds a
        // shutdown_rx oneshot that we fire here. watcher_join_handle
        // resolves when the task has fully exited.
        if let Some(shutdown_tx) = self.watcher_shutdown.lock().await.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(handle) = self.watcher_join.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }

        // Step 4: drain the audit batcher. Everything already queued
        // must land before we proceed.
        let batcher_report = self.audit_batcher.shutdown(Duration::from_secs(10)).await;

        // Step 5: flush any remaining synchronous state (e.g. docs
        // manager caches). Idempotent.
        let docs_flushed = self.docs_by_project.read().await.len();

        // Step 6: return a report. The broker is NOT dropped here —
        // that's the caller's responsibility. Separating shutdown
        // from drop lets tests assert the report without relying on
        // Drop running to completion.
        ShutdownReport {
            watcher_stopped: true,
            batcher: batcher_report,
            docs_flushed,
        }
    }
}
```

**Broker method guard:**
```rust
// Every state-changing broker method gets a guard at the top.
pub async fn memory_ingest_conversation(&self, request: ...) -> ... {
    if self.shutting_down.load(Ordering::Acquire) {
        return Err(CoreError::Transport(
            "broker shutting down".to_string()
        ));
    }
    // ...existing body...
}
```

**SIGTERM wiring in `bin/the-one-mcp.rs`:**
```rust
async fn main() -> Result<()> {
    let broker = Arc::new(McpBroker::new());
    let transport = StdioTransport;

    let serve = tokio::spawn({
        let broker = broker.clone();
        async move { transport.run(broker).await }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("SIGINT received, shutting down");
        }
        res = serve => {
            tracing::info!("transport loop exited: {:?}", res);
        }
    }

    let report = broker.shutdown().await;
    tracing::info!(?report, "broker shutdown complete");
    Ok(())
}
```

**Regression guards:**
- `test_shutdown_refuses_new_requests` — call `shutdown()` then a
  broker method, assert `Transport` error.
- `test_shutdown_drains_batcher_before_returning` — queue 100
  records, call shutdown, verify all 100 land.
- `test_shutdown_timeout_reports_dropped` — force a 0-ms timeout,
  verify `ShutdownReport` shows non-zero dropped.
- `test_shutdown_stops_file_watcher` — integration test with a file
  watcher active.

### 5.5 C5 — Feature flag via config

**Decision:** `AppConfig::audit_batcher_enabled: bool`.

```rust
// the-one-core/src/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    // …existing fields…
    pub audit_batcher_enabled: bool,
}

const DEFAULT_AUDIT_BATCHER_ENABLED: bool = false; // v0.17
```

**Broker construction:**
```rust
impl McpBroker {
    pub fn new_with_policy(policy: PolicyEngine) -> Self {
        let config = AppConfig::load_global().unwrap_or_default();
        let audit_mode = if config.audit_batcher_enabled {
            AuditMode::Batched(AuditBatcher::spawn(
                AuditBatcherConfig::from_app_config(&config),
            ))
        } else {
            AuditMode::Synchronous
        };
        // …rest of construction…
    }
}

enum AuditMode {
    Synchronous,
    Batched(AuditBatcher),
}
```

**Broker call-site pattern:**
```rust
// In each state-changing method:
match &self.audit_mode {
    AuditMode::Synchronous => {
        if let Err(e) = db.record_audit(&audit) {
            tracing::warn!(error = %e, "audit write failed (sync mode)");
        }
    }
    AuditMode::Batched(batcher) => {
        match batcher.queue(project_root, project_id, audit.clone()).await {
            QueueOutcome::Queued => {}
            QueueOutcome::FallbackToSync { reason: _ } => {
                if let Err(e) = db.record_audit(&audit) {
                    tracing::warn!(error = %e, "audit write failed (fallback path)");
                }
            }
        }
    }
}
```

Rollout phasing:
- **v0.17.0:** ship the code with `audit_batcher_enabled = false`.
  New tests cover both modes. Default behaviour identical to v0.16.
- **v0.17.1:** operators opt in via `audit_batcher_enabled = true` in
  their config.
- **v0.18.0:** flip the default to `true`. Include rollback
  instructions in release notes.
- **v0.19.0:** remove `AuditMode::Synchronous` from broker methods
  (the underlying `ProjectDatabase::record_audit` stays for tests
  and for the fallback path).

### 5.6 C6 — CI test matrix

**Decision:** dedicated tests exercise both modes explicitly, with a
small helper that constructs the broker in each mode.

```rust
// tests/production_hardening.rs (extended)

fn broker_with_audit_mode(batcher_enabled: bool) -> (TempDir, McpBroker) {
    let tmp = tempfile::tempdir().unwrap();
    // Write a config.json that forces the mode.
    let config = format!(
        r#"{{"audit_batcher_enabled": {batcher_enabled}}}"#
    );
    std::fs::create_dir_all(tmp.path().join(".the-one")).unwrap();
    std::fs::write(tmp.path().join(".the-one/config.json"), config).unwrap();
    std::env::set_var("THE_ONE_PROJECT_ROOT", tmp.path());
    let broker = McpBroker::new();
    (tmp, broker)
}

#[tokio::test]
async fn lever2_audit_writes_work_in_both_modes() {
    for enabled in [false, true] {
        let (_tmp, broker) = broker_with_audit_mode(enabled);
        // ...exercise the write path, assert audit rows appear...
    }
}
```

**Why not a full matrix on every test:** running all 491 existing
tests twice would nearly double CI time. The batcher only affects
audit-writing paths, so only tests that actually touch audit need the
matrix treatment. Existing stdio / unit tests run once with the
default (`false`).

---

## 6. Changed data structures (full picture)

```rust
// the-one-core/src/audit/batcher.rs

#[derive(Clone)]
pub struct AuditBatcher {
    inner: Arc<Inner>,
}

struct Inner {
    config: AuditBatcherConfig,
    sender: mpsc::Sender<BatcherMsg>,
    shutdown_tx: tokio::sync::Notify,
    shutdown_done: tokio::sync::Notify,

    // S3: global memory bound
    global_depth: AtomicUsize,
    // C1: per-project depth for backpressure
    per_project_depth: DashMap<ProjectKey, AtomicUsize>,
    // S1: poison queue for 3-strike drops
    poison: Arc<PoisonQueue>,
    // Observability
    metrics: Arc<BatcherMetrics>,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditBatcherConfig {
    pub max_queue_depth_total: usize,        // default 50 000 (S3)
    pub max_queue_depth_per_project: usize,  // default 10 000
    pub batch_max_rows: usize,               // default 256
    pub flush_interval: Duration,            // default 250 ms
    pub queue_timeout: Duration,             // default 100 ms
    pub db_cache_size: usize,                // default 32
    pub poison_path: PathBuf,                // ~/.the-one/audit.poison.jsonl
    pub poison_max_bytes: u64,               // default 100 MB
    pub poison_max_generations: usize,       // default 3
    pub consecutive_failure_threshold: u32,  // default 3
}

#[derive(Debug, Default)]
struct BatcherMetrics {
    // Existing from draft
    queue_depth: AtomicU64,
    flushes_total: AtomicU64,
    rows_flushed_total: AtomicU64,
    flush_latency_ms_total: AtomicU64,
    flush_errors_total: AtomicU64,
    db_cache_evictions_total: AtomicU64,
    // S1 additions
    poisoned_total: AtomicU64,
    poison_rotations_total: AtomicU64,
    // S2 additions — split by reason so alerts are actionable
    fallback_sync_global: AtomicU64,
    fallback_sync_project: AtomicU64,
    fallback_sync_timeout: AtomicU64,
    // Shutdown accounting
    dropped_on_shutdown: AtomicU64,
}

enum BatcherMsg {
    Record { key: ProjectKey, record: AuditRecord },
    Flush { key: ProjectKey, tx: oneshot::Sender<Result<(), CoreError>> },
    Shutdown { tx: oneshot::Sender<ShutdownReport> },
}

pub enum QueueOutcome {
    Queued,
    FallbackToSync { reason: &'static str },
}

#[derive(Debug, Clone)]
pub struct ShutdownReport {
    pub rows_flushed_on_shutdown: u64,
    pub rows_dropped_on_shutdown: u64,
    pub poisoned_on_shutdown: u64,
    pub took: Duration,
}
```

---

## 7. Updated testing plan

Eleven unit tests in `audit/batcher.rs`, three integration tests in
`tests/production_hardening.rs`, one extended stdio test, plus the
poison-queue tests.

### 7.1 Unit tests (batcher.rs)

1. `test_queue_and_tick_flush` — happy path.
2. `test_batch_max_rows_triggers_flush` — size-based flush.
3. `test_ordering_within_project_preserved` — 100 records, verify order.
4. `test_saturation_global_falls_back_to_sync` — fill global, verify reason.
5. `test_saturation_project_falls_back_to_sync` — fill one project, others unaffected.
6. `test_queue_timeout_falls_back_to_sync` — channel send blocked > timeout.
7. `test_explicit_flush_blocks_until_done` — Flush msg round-trip.
8. `test_isolation_across_projects` — A and B don't mix.
9. `test_retry_once_on_single_failure` — `FailingSink` fail_next_n=1.
10. `test_poison_on_3_consecutive_failures` — fail_next_n=3, verify poison queue.
11. `test_staging_buffer_during_flush` — queue while flush is in flight, verify ordering.
12. `test_shutdown_drains_queued` — final flush on shutdown.
13. `test_shutdown_timeout_reports_dropped` — fast timeout, verify dropped count.
14. `test_metrics_are_monotonic` — counters only go up.

### 7.2 Integration tests (production_hardening.rs)

1. `lever2_batched_mode_persists_audit_via_stdio` — set `audit_batcher_enabled=true`, drive a write through stdio, verify row landed.
2. `lever2_fallback_to_sync_on_saturation` — saturate the queue via a test-configured low cap, verify writes still succeed via fallback.
3. `lever2_poison_queue_created_on_persistent_failure` — inject persistent sink failure, verify `.the-one/audit.poison.jsonl` exists.
4. `lever2_shutdown_drains_batcher_before_exit` — queue 100 records, shutdown, verify all persist.

### 7.3 Poison queue tests (poison.rs)

1. `test_poison_append_creates_file` — cold start.
2. `test_poison_rotation_at_max_bytes` — seed near cap, append, verify rotation.
3. `test_poison_rotation_deletes_oldest_at_max_generations` — 4th generation triggers delete.
4. `test_replay_audit_poison_succeeds` — happy path recovery.
5. `test_replay_audit_poison_leaves_failed_rows` — partial recovery.

### 7.4 Stress / soak (manual, feature-flagged)

Same as draft § 6.4. Adds:
- `stress_saturation_recovery` — saturate for 10 seconds, unthrottle,
  verify queue drains within 5 s.
- `stress_poison_under_flaky_sqlite` — 1% random flush failure, run
  1 M records, verify poison file contains only the failed ones.

---

## 8. Observability — full metric list

Additions over draft § 7.1:

| metric                                  | type     | reason                         |
|------------------------------------------|----------|--------------------------------|
| `audit_batcher_global_depth`             | gauge    | S3 — global memory tracking    |
| `audit_batcher_fallback_sync_global`     | counter  | S2 — cause breakdown           |
| `audit_batcher_fallback_sync_project`    | counter  | S2 — cause breakdown           |
| `audit_batcher_fallback_sync_timeout`    | counter  | S2 — cause breakdown           |
| `audit_batcher_poisoned_total`           | counter  | S1 — poison path exercised     |
| `audit_batcher_poison_rotations_total`   | counter  | S1 — poison file size          |
| `audit_batcher_staging_merge_total`      | counter  | C2 — flush contention indicator |

Alerts:
- `rate(audit_batcher_poisoned_total[5m]) > 0` → **page on-call**
  (real data loss event).
- `rate(audit_batcher_fallback_sync_global[5m]) > 0.01` → warning
  (global memory bound being hit).
- `audit_batcher_global_depth / 50_000 > 0.8` → warning (cap nearing).

---

## 9. File inventory (v2)

### New

- `crates/the-one-core/src/audit/mod.rs` — promote `audit.rs` to module.
- `crates/the-one-core/src/audit/batcher.rs` — batcher struct, task, config.
- `crates/the-one-core/src/audit/poison.rs` — poison queue.
- `crates/the-one-core/src/audit/sink.rs` — `AuditSink` trait + sqlite impl.
- `crates/the-one-core/benches/audit_batcher_bench.rs` — batch-vs-sync bench.
- `crates/the-one-mcp/tests/audit_batcher_scenarios.rs` — integration tests.

### Modified

- `crates/the-one-core/src/audit.rs` → moved to `audit/mod.rs`, re-exports.
- `crates/the-one-core/src/lib.rs` — export `AuditBatcher`, `AuditSink`.
- `crates/the-one-core/src/config.rs` — add audit batcher fields.
- `crates/the-one-core/src/storage/sqlite.rs` — add `record_audit_batch`.
- `crates/the-one-mcp/src/broker.rs` — `audit_mode` field, `shutdown()`.
- `crates/the-one-mcp/src/api.rs` — `MetricsSnapshotResponse` extensions.
- `crates/the-one-mcp/src/bin/the-one-mcp.rs` — SIGTERM handler.
- `crates/the-one-mcp/tests/stdio_write_path.rs` — add batched-mode tests.
- `crates/the-one-mcp/tests/production_hardening.rs` — add Lever 2 guards.
- `crates/the-one-core/examples/production_hardening_bench.rs` — batcher section.
- `docs/guides/production-hardening-v0.15.md` — § 16 Lever 2 operational guide.
- `docs/guides/mempalace-operations.md` — tuning knobs.
- `CLAUDE.md` — brief mention in the conventions block.

### Dependencies to add

- `dashmap = "6"` (for `per_project_depth`)

Everything else uses existing workspace deps.

---

## 10. Complexity estimate (v2)

| area                                 | LOC  | hours |
|---------------------------------------|-----:|------:|
| Batcher + config + task loop          |  420 |  5.0 |
| Poison queue module                   |  120 |  1.5 |
| `AuditSink` trait + impls             |   80 |  0.5 |
| `record_audit_batch` SQL method       |   40 |  0.25 |
| Broker wiring + audit_mode enum       |  180 |  2.0 |
| `McpBroker::shutdown` + watcher hook  |  120 |  1.5 |
| Unit tests (14)                       |  450 |  4.0 |
| Integration tests (4)                 |  180 |  1.5 |
| Poison queue tests (5)                |  120 |  1.0 |
| Benchmark section                     |   80 |  0.5 |
| Documentation (this + 2 guides)       |  250 |  2.0 |
| Rollout (feature flag, canary config) |   50 |  1.0 |
| Code review + iterate                 |    — |  4.0 |
| Stress / soak testing                 |    — |  4.0 |
| **Total**                             |2 090 |  28.75 |

At 6 productive hours/day: **~4.8 engineer-days**, up from the draft's
3.5. The extra time pays for the safety work (poison queue, fallback,
global cap, shutdown sequencing) that makes the plan actually safe.

---

## 11. Rollout (v2 sequence)

1. **v0.17.0** — ship all code, feature flag default `false`, no
   behaviour change for existing users. Tests exercise both modes.
2. **v0.17.1** — opt-in via config. Document the tuning knobs and
   the rollback procedure.
3. **v0.17.2** — dogfood on maintainer workspace for 1 week under
   realistic write load. Watch `audit_batcher_poisoned_total` and
   the fallback counters.
4. **v0.18.0** — flip default to `true`. Release notes highlight
   the rollback switch and the durability envelope.
5. **v0.19.0** (at earliest, 4+ weeks after v0.18.0, only if no
   regressions reported) — remove `AuditMode::Synchronous` from
   broker methods. Keep `record_audit` on `ProjectDatabase` for
   tests and the fallback path.

Rollback at any point: set `audit_batcher_enabled = false` and
restart the broker. Any in-flight records flush on shutdown before
the batcher task exits.

---

## 12. Acceptance criteria (implementation)

Before merging the Lever 2 PR:

1. All existing tests pass with `audit_batcher_enabled` at both values.
2. All 23 new tests (14 unit + 4 integration + 5 poison) pass.
3. `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. Benchmark shows **≥ 5× amortised speedup** over Lever 1 baseline
   under default config. If less, tune `batch_max_rows` and re-run.
5. Stress test (1 M rows/60 s) completes without a single record lost
   (checked via total input count vs `audit_events` + poison queue count).
6. Shutdown test: 100 records queued, SIGTERM, all 100 land on disk.
7. Poison queue test: force 3 consecutive sink failures, verify file
   exists, contains N entries, metric incremented.
8. Flame-graph verification: `record_audit` wall-clock percentage
   drops below Lever 1 baseline AND the batcher task doesn't appear
   in the top 20 hot spots on the request path.
9. Release notes drafted covering: opt-in flag, durability envelope,
   rollback, poison queue location.

---

## 13. Open questions — still deliberately unresolved

These are not blockers for implementation but need explicit answers
during PR review, not code.

1. **Poison queue location**: inside the per-project `.the-one/` dir
   or in `~/.the-one/audit.poison.jsonl`? Per-project is simpler for
   recovery; global is simpler for operators. Recommendation:
   **per-project** — matches the existing state isolation model.

2. **Should `replay_audit_poison` run synchronously or via the
   batcher?** If via the batcher, a sink failure during replay would
   re-poison the records, creating a loop. Recommendation:
   **synchronous replay**. Breaks the loop explicitly.

3. **Should `record_audit` (sync path) also write to the poison
   queue on failure?** Today it returns an error to the broker which
   logs and continues. For consistency, arguably yes. Recommendation:
   **no** — the sync path is already "best effort", and funnelling
   both paths into the same poison file would obscure which came from
   where. Add a `source` field to the poison JSON if we change this.

4. **Metric cardinality**: per-project `global_depth` over DashMap
   is a metric-per-project situation if exposed via Prometheus. For
   100+ projects this floods the /metrics endpoint. Recommendation:
   expose only aggregate metrics via the public snapshot; keep per-
   project depths behind `observe: audit_batcher` admin action.

5. **Test pollution via `THE_ONE_PROJECT_ROOT` env var**: the test
   helper in § 5.6 uses a global env var. If two tests run in
   parallel, they race. Recommendation: use a `test_utils::EnvGuard`
   with `temp_env::with_vars` like the existing config tests.

Resolve these at PR review.

---

## 14. See also

- **Draft**: `draft-2026-04-10-audit-batching-lever2.md` — the first
  cut. Read its § 9 for the rejected alternatives, § 16 for the
  self-critique that motivated this v2.
- **Lever 1 (shipped)**: `docs/guides/production-hardening-v0.15.md`
  § 14.
- **Findings report**:
  `docs/reviews/2026-04-10-mempalace-comparative-audit.md`.
- **Audit module**: `crates/the-one-core/src/audit.rs`.
- **Benchmark**: `crates/the-one-core/examples/production_hardening_bench.rs`.
- **Broker metrics pattern**:
  `crates/the-one-mcp/src/broker.rs::BrokerMetrics`.
