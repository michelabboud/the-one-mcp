//! Redis TimeSeries operations: `TS.CREATE`, `TS.ADD`, `TS.GET`, `TS.RANGE`.
//!
//! ## Context
//!
//! Redis TimeSeries is shipped as a module in Redis Stack. the-one-mcp's Redis 8
//! image includes it by default; external Redis deployments need to load
//! the `redistimeseries` module. All commands are first-class once the
//! module is loaded.
//!
//! `redis-rs` 1.2 doesn't ship typed TS.* wrappers, so this module owns
//! them. The surface mirrors the shape domain TimeSeries wrappers
//! needs (`ts:llm:latency:*`, `ts:cache:hits`, etc.) plus the generic
//! range-query pattern. Typed reply shapes (timestamp + value) are
//! decoded inside this module — callers get `(u64, f64)` tuples, not
//! raw `redis::Value`.

use std::time::Duration;

use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for Redis TimeSeries operations. Returned by
/// [`RedisPool::timeseries`].
pub struct TimeSeriesOps<'a> {
    pool: &'a RedisPool,
}

/// Options for `TS.CREATE`. Duplicate-policy defaults to `LAST` so
/// concurrent writers converge on the most recent value rather than
/// returning errors.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// Retention window. `None` = keep forever.
    pub retention: Option<Duration>,
    /// Optional `LABELS k v [k v ...]` key-value pairs. Empty = no labels.
    pub labels: Vec<(String, String)>,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            retention: Some(Duration::from_secs(7 * 24 * 60 * 60)),
            labels: Vec::new(),
        }
    }
}

impl<'a> TimeSeriesOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `TS.CREATE key [RETENTION ms] DUPLICATE_POLICY LAST [LABELS ...]`.
    ///
    /// Returns `Ok(())` on first creation. When the series already exists
    /// Redis returns an error which this method silently swallows —
    /// callers want idempotent bootstrap. Any other error is propagated.
    #[instrument(skip(self, opts), level = "debug")]
    pub async fn create(&self, key: &str, opts: &CreateOptions) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("TS.CREATE");
        cmd.arg(key);
        if let Some(ret) = opts.retention {
            cmd.arg("RETENTION").arg(ret.as_millis() as u64);
        }
        cmd.arg("DUPLICATE_POLICY").arg("LAST");
        if !opts.labels.is_empty() {
            cmd.arg("LABELS");
            for (k, v) in &opts.labels {
                cmd.arg(k).arg(v);
            }
        }
        let res: redis::RedisResult<()> = cmd.query_async(&mut conn).await;
        match res {
            Ok(()) => Ok(()),
            // `TSDB: key already exists` — ignore, matches the pre-facade behaviour
            // of domain TimeSeries wrappers which discarded TS.CREATE errors.
            Err(e) if e.to_string().contains("key already exists") => Ok(()),
            Err(e) => Err(RedisError::Command(e)),
        }
    }

    /// `TS.ADD key * value RETENTION ms ON_DUPLICATE LAST`.
    ///
    /// Uses `*` for the timestamp, letting Redis use its server clock.
    /// `retention` is applied per-add so series that weren't
    /// pre-created still get a bounded retention window.
    #[instrument(skip(self), level = "trace")]
    pub async fn add(&self, key: &str, value: f64, retention: Duration) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let _: redis::Value = redis::cmd("TS.ADD")
            .arg(key)
            .arg("*")
            .arg(value.to_string())
            .arg("RETENTION")
            .arg(retention.as_millis() as u64)
            .arg("ON_DUPLICATE")
            .arg("LAST")
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `TS.GET key`. Returns the latest `(timestamp_ms, value)` or `None`
    /// when the series doesn't exist or has no data.
    ///
    /// Errors from Redis are swallowed and returned as `Ok(None)` —
    /// matches the pre-facade call-site behaviour which treats a missing
    /// series as "no data" rather than a hard failure.
    #[instrument(skip(self), level = "trace")]
    pub async fn get(&self, key: &str) -> RedisResult<Option<(u64, f64)>> {
        let mut conn = self.pool.conn();
        let res: redis::RedisResult<redis::Value> =
            redis::cmd("TS.GET").arg(key).query_async(&mut conn).await;
        match res {
            Ok(redis::Value::Array(arr)) if arr.len() == 2 => {
                let ts = parse_timestamp(&arr[0])?;
                let val = parse_float(&arr[1])?;
                Ok(Some((ts, val)))
            }
            Ok(_) => Ok(None),
            // Missing series or module not loaded — treat as no data.
            Err(_) => Ok(None),
        }
    }

    /// `TS.RANGE key from to`. Inclusive timestamp bounds in milliseconds.
    /// Pass `to_ms = None` for `+` (up to the latest sample).
    ///
    /// Returns an empty Vec when the series doesn't exist (rather than
    /// propagating the Redis error) — matches the pre-facade call-site
    /// behaviour.
    #[instrument(skip(self), level = "trace")]
    pub async fn range(
        &self,
        key: &str,
        from_ms: u64,
        to_ms: Option<u64>,
    ) -> RedisResult<Vec<(u64, f64)>> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("TS.RANGE");
        cmd.arg(key).arg(from_ms);
        match to_ms {
            Some(ms) => {
                cmd.arg(ms);
            }
            None => {
                cmd.arg("+");
            }
        }
        let res: redis::RedisResult<redis::Value> = cmd.query_async(&mut conn).await;
        let pairs = match res {
            Ok(redis::Value::Array(arr)) => arr,
            Ok(_) => return Ok(Vec::new()),
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::with_capacity(pairs.len());
        for pair in pairs {
            if let redis::Value::Array(inner) = pair {
                if inner.len() == 2 {
                    if let (Ok(ts), Ok(val)) = (parse_timestamp(&inner[0]), parse_float(&inner[1]))
                    {
                        out.push((ts, val));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Parse a Redis reply element as a millisecond timestamp.
fn parse_timestamp(val: &redis::Value) -> RedisResult<u64> {
    match val {
        redis::Value::Int(n) => Ok(*n as u64),
        redis::Value::BulkString(b) => {
            let s = std::str::from_utf8(b)
                .map_err(|e| RedisError::ReplyParse(format!("invalid utf8 timestamp: {e}")))?;
            s.parse::<u64>()
                .map_err(|e| RedisError::ReplyParse(format!("invalid timestamp '{s}': {e}")))
        }
        redis::Value::SimpleString(s) => s
            .parse::<u64>()
            .map_err(|e| RedisError::ReplyParse(format!("invalid timestamp '{s}': {e}"))),
        other => Err(RedisError::ReplyParse(format!(
            "unexpected timestamp type: {other:?}"
        ))),
    }
}

/// Parse a Redis reply element as a floating-point value.
fn parse_float(val: &redis::Value) -> RedisResult<f64> {
    match val {
        redis::Value::Double(f) => Ok(*f),
        redis::Value::Int(n) => Ok(*n as f64),
        redis::Value::BulkString(b) => {
            let s = std::str::from_utf8(b)
                .map_err(|e| RedisError::ReplyParse(format!("invalid utf8 float: {e}")))?;
            s.parse::<f64>()
                .map_err(|e| RedisError::ReplyParse(format!("invalid float '{s}': {e}")))
        }
        redis::Value::SimpleString(s) => s
            .parse::<f64>()
            .map_err(|e| RedisError::ReplyParse(format!("invalid float '{s}': {e}"))),
        other => Err(RedisError::ReplyParse(format!(
            "unexpected float type: {other:?}"
        ))),
    }
}
