//! Stream operations: `XADD`, `XREAD`, `XREADGROUP`, `XAUTOCLAIM`,
//! `XACK`, `XPENDING`, `XLEN`, `XDEL`, `XGROUP CREATE`.
//!
//! ## Why this module exists
//!
//! On `fred` 10, both `XREADGROUP` and `XAUTOCLAIM` had typed-convert
//! bugs where populated responses failed to decode (commits `80c437ba`
//! and `c0b7e27f` worked around them with manual RESP2 parsers inline
//! in `crates/the-one-memory/src/rag/queue.rs`). On `redis-rs` 1.2 those
//! same operations decode cleanly because the `streams` feature ships
//! typed `StreamReadReply`/`StreamPendingReply`/etc. structs that match
//! the actual response shape. **The manual parsers go away when the-one-mcp
//! migrates from fred to this module.**
//!
//! See `tests/streams_nil.rs` for the regression test that locks the
//! correct nil-handling contract in CI.

use std::collections::HashMap;
use std::time::Duration;

use redis::streams::{
    StreamAutoClaimOptions, StreamAutoClaimReply, StreamClaimReply, StreamMaxlen,
    StreamPendingReply, StreamReadOptions, StreamReadReply,
};
use redis::AsyncCommands;
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for stream operations. Returned by [`RedisPool::streams`].
pub struct StreamsOps<'a> {
    pool: &'a RedisPool,
}

/// One stream entry as decoded from a `XREADGROUP` / `XAUTOCLAIM`
/// reply. Fields are flattened into a `HashMap<String, String>`.
#[derive(Debug, Clone)]
pub struct StreamEntry {
    /// The stream key the entry came from.
    pub stream: String,
    /// The entry's auto-generated ID (e.g. `1700000000000-0`).
    pub id: String,
    /// Field-value pairs. Both sides decoded as UTF-8 strings; callers
    /// that need bytes can drop down to the raw `redis::Value` API.
    pub fields: HashMap<String, String>,
}

impl<'a> StreamsOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `XADD key * field value [field value ...]` with optional MAXLEN.
    ///
    /// Returns the auto-generated entry ID.
    #[instrument(skip(self, fields), level = "trace", fields(field_count = fields.len()))]
    pub async fn xadd(
        &self,
        key: &str,
        fields: &[(&str, &str)],
        maxlen_approx: Option<u64>,
    ) -> RedisResult<String> {
        let mut conn = self.pool.conn();
        let id: String = if let Some(cap) = maxlen_approx {
            // `MAXLEN ~ N` — approximate trim, O(1) amortised.
            conn.xadd_maxlen(key, StreamMaxlen::Approx(cap as usize), "*", fields)
                .await
                .map_err(RedisError::Command)?
        } else {
            conn.xadd(key, "*", fields)
                .await
                .map_err(RedisError::Command)?
        };
        Ok(id)
    }

    /// `XLEN key`. Number of entries on the stream.
    #[instrument(skip(self), level = "trace")]
    pub async fn xlen(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.xlen(key).await.map_err(RedisError::Command)
    }

    /// `XACK key group id`. Acknowledges a single entry.
    #[instrument(skip(self), level = "trace")]
    pub async fn xack(&self, key: &str, group: &str, id: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.xack(key, group, &[id])
            .await
            .map_err(RedisError::Command)
    }

    /// `XGROUP CREATE key group id MKSTREAM` — idempotent; ignores
    /// `BUSYGROUP` (already exists).
    ///
    /// `start_id` is typically `"$"` (read only new entries) or `"0"`
    /// (read all existing entries).
    #[instrument(skip(self), level = "debug")]
    pub async fn xgroup_create(&self, key: &str, group: &str, start_id: &str) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let res: redis::RedisResult<()> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(key)
            .arg(group)
            .arg(start_id)
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await;
        match res {
            Ok(()) => Ok(()),
            // BUSYGROUP = group already exists; treat as success because
            // the bootstrap is idempotent.
            Err(e) if e.to_string().contains("BUSYGROUP") => Ok(()),
            Err(e) => Err(RedisError::Command(e)),
        }
    }

    /// `XREADGROUP GROUP group consumer COUNT count BLOCK block_ms STREAMS key >`.
    ///
    /// Returns up to `count` newly-arrived entries. Returns an empty Vec
    /// when the block timeout elapses with no new data — **never an error**.
    /// This is the contract the manual RESP2 parser in
    /// `crates/the-one-memory/src/rag/queue.rs` was wrestling with on fred;
    /// `redis-rs` 1.2's typed `StreamReadReply` handles the nil case
    /// natively.
    #[instrument(skip(self), level = "debug", fields(stream = %stream, group = %group, consumer = %consumer))]
    pub async fn xreadgroup(
        &self,
        group: &str,
        consumer: &str,
        stream: &str,
        count: usize,
        block: Duration,
    ) -> RedisResult<Vec<StreamEntry>> {
        // v0.1.363: blocking commands MUST use a dedicated connection
        // (not the shared multiplexed one) so they don't head-of-line-
        // block every other Redis call in the process during their
        // BLOCK window. See `RedisPool::dedicated_conn` doc for the
        // full rationale + the investigation link. We always use a
        // dedicated connection here even when `block == Duration::ZERO`
        // — keeps the invariant simple ("any method with a `block`
        // parameter uses dedicated_conn"). Cost when block=0 is a
        // single sub-millisecond TCP open, negligible against any
        // real query work.
        let mut conn = self.pool.dedicated_conn().await?;
        let opts = StreamReadOptions::default()
            .group(group, consumer)
            .count(count)
            .block(block.as_millis() as usize);
        let reply: Option<StreamReadReply> = conn
            .xread_options(&[stream], &[">"], &opts)
            .await
            .map_err(RedisError::Command)?;
        Ok(flatten_read_reply(reply))
    }

    /// `XAUTOCLAIM key group consumer min_idle_time start COUNT count`.
    ///
    /// Reclaims entries pending longer than `min_idle` from any consumer
    /// in the group, transferring them to `consumer`. Returns the
    /// reclaimed entries flattened to [`StreamEntry`].
    ///
    /// The cursor (next `start` to pass on the following call) is
    /// returned in the second tuple element. When all pending entries
    /// have been swept this is `"0-0"`.
    #[instrument(skip(self), level = "debug")]
    pub async fn xautoclaim(
        &self,
        key: &str,
        group: &str,
        consumer: &str,
        min_idle: Duration,
        start: &str,
        count: usize,
    ) -> RedisResult<(Vec<StreamEntry>, String)> {
        let mut conn = self.pool.conn();
        let opts = StreamAutoClaimOptions::default().count(count);
        let reply: StreamAutoClaimReply = conn
            .xautoclaim_options(
                key,
                group,
                consumer,
                min_idle.as_millis() as usize,
                start,
                opts,
            )
            .await
            .map_err(RedisError::Command)?;
        let entries: Vec<StreamEntry> = reply
            .claimed
            .into_iter()
            .map(|raw| StreamEntry {
                stream: key.to_string(),
                id: raw.id,
                fields: stream_value_map(raw.map),
            })
            .collect();
        Ok((entries, reply.next_stream_id))
    }

    /// `XCLAIM key group consumer min_idle_time id`. Single-id reclaim.
    #[instrument(skip(self), level = "trace")]
    pub async fn xclaim(
        &self,
        key: &str,
        group: &str,
        consumer: &str,
        min_idle: Duration,
        id: &str,
    ) -> RedisResult<Vec<StreamEntry>> {
        let mut conn = self.pool.conn();
        let reply: StreamClaimReply = conn
            .xclaim(key, group, consumer, min_idle.as_millis() as usize, &[id])
            .await
            .map_err(RedisError::Command)?;
        Ok(reply
            .ids
            .into_iter()
            .map(|raw| StreamEntry {
                stream: key.to_string(),
                id: raw.id,
                fields: stream_value_map(raw.map),
            })
            .collect())
    }

    /// `XPENDING key group` — short form. Returns the count of pending
    /// entries.
    #[instrument(skip(self), level = "trace")]
    pub async fn xpending_count(&self, key: &str, group: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        let reply: StreamPendingReply = conn
            .xpending(key, group)
            .await
            .map_err(RedisError::Command)?;
        Ok(match reply {
            StreamPendingReply::Empty => 0,
            StreamPendingReply::Data(d) => d.count as u64,
            // `StreamPendingReply` is marked non_exhaustive upstream —
            // any future variant we don't recognise is treated as no
            // pending entries (safe fallback for an observability call).
            _ => 0,
        })
    }

    /// `XDEL key id`. Returns the number of entries actually deleted.
    #[instrument(skip(self), level = "trace")]
    pub async fn xdel(&self, key: &str, id: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.xdel(key, &[id]).await.map_err(RedisError::Command)
    }

    /// `XREVRANGE key end start [COUNT n]` — read entries in reverse
    /// chronological order. `end = "+"` and `start = "-"` reads from
    /// newest to oldest across the full stream. Used by audit-log
    /// pagination paths that need newest-first ordering without setting
    /// up a consumer group.
    ///
    /// Fields are decoded as UTF-8 strings and flattened into
    /// [`StreamEntry::fields`].
    #[instrument(skip(self), level = "debug", fields(stream = %key, count = ?count))]
    pub async fn xrevrange(
        &self,
        key: &str,
        end: &str,
        start: &str,
        count: Option<usize>,
    ) -> RedisResult<Vec<StreamEntry>> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("XREVRANGE");
        cmd.arg(key).arg(end).arg(start);
        if let Some(n) = count {
            cmd.arg("COUNT").arg(n);
        }
        let raw: redis::Value = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(parse_xrange_reply(key, raw))
    }

    /// `XREAD [COUNT count] [BLOCK block_ms] STREAMS key id` — non-group
    /// consumer read. Returns up to `count` entries newer than `since_id`
    /// on the stream. Blocks for up to `block` before returning an empty
    /// `Vec` (never an error on timeout).
    ///
    /// Use `since_id = "0-0"` to read from the beginning of the stream.
    /// Use `since_id = "$"` to read only new entries (race-prone on first
    /// call — prefer capturing the last-known ID before the producer sends
    /// and passing that instead).
    ///
    /// Fields are decoded as UTF-8 strings and flattened into
    /// [`StreamEntry::fields`].
    #[instrument(skip(self), level = "debug", fields(stream = %key, since = %since_id))]
    pub async fn xread(
        &self,
        key: &str,
        since_id: &str,
        count: usize,
        block: Duration,
    ) -> RedisResult<Vec<StreamEntry>> {
        // v0.1.363: blocking commands MUST use a dedicated connection
        // (not the shared multiplexed one) — see `xreadgroup` above
        // and `RedisPool::dedicated_conn` for the full rationale.
        let mut conn = self.pool.dedicated_conn().await?;
        let opts = StreamReadOptions::default()
            .count(count)
            .block(block.as_millis() as usize);
        let reply: Option<StreamReadReply> = conn
            .xread_options(&[key], &[since_id], &opts)
            .await
            .map_err(RedisError::Command)?;
        Ok(flatten_read_reply(reply))
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn flatten_read_reply(reply: Option<StreamReadReply>) -> Vec<StreamEntry> {
    let Some(reply) = reply else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stream_key in reply.keys {
        for raw in stream_key.ids {
            out.push(StreamEntry {
                stream: stream_key.key.clone(),
                id: raw.id,
                fields: stream_value_map(raw.map),
            });
        }
    }
    out
}

fn stream_value_map(map: HashMap<String, redis::Value>) -> HashMap<String, String> {
    map.into_iter()
        .filter_map(|(k, v)| value_to_string(v).map(|s| (k, s)))
        .collect()
}

fn value_to_string(v: redis::Value) -> Option<String> {
    match v {
        redis::Value::SimpleString(s) => Some(s),
        redis::Value::BulkString(bytes) => String::from_utf8(bytes).ok(),
        redis::Value::Int(i) => Some(i.to_string()),
        redis::Value::Double(f) => Some(f.to_string()),
        _ => None,
    }
}

/// Parse an `XRANGE` / `XREVRANGE` reply into [`StreamEntry`] values.
///
/// Reply shape per Redis spec:
/// ```text
/// 1) 1) "<id>"                  ← entry ID (BulkString)
///    2) 1) "<field1>"           ← flat alternating field/value array
///       2) "<value1>"
///       3) "<field2>"
///       4) "<value2>"
/// 2) 1) "<id>"
///    2) 1) ...
/// ```
fn parse_xrange_reply(stream_key: &str, reply: redis::Value) -> Vec<StreamEntry> {
    let redis::Value::Array(entries) = reply else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let redis::Value::Array(parts) = entry else {
            continue;
        };
        let mut iter = parts.into_iter();
        let Some(id) = iter.next().and_then(value_to_string) else {
            continue;
        };
        let Some(redis::Value::Array(field_pairs)) = iter.next() else {
            out.push(StreamEntry {
                stream: stream_key.to_string(),
                id,
                fields: HashMap::new(),
            });
            continue;
        };
        let mut fields = HashMap::with_capacity(field_pairs.len() / 2);
        let mut pair_iter = field_pairs.into_iter();
        while let (Some(k), Some(v)) = (pair_iter.next(), pair_iter.next()) {
            if let (Some(k), Some(v)) = (value_to_string(k), value_to_string(v)) {
                fields.insert(k, v);
            }
        }
        out.push(StreamEntry {
            stream: stream_key.to_string(),
            id,
            fields,
        });
    }
    out
}
