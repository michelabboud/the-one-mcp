//! List operations including the **safe** `BLPOP` wrapper.
//!
//! ## The BLPOP contract this module enforces
//!
//! `redis-rs`, `fred`, and most Redis clients in most languages share a
//! confusing trap with blocking commands: a *server-side timeout* (the
//! BLPOP `timeout` arg the docs talk about) returns nil over the wire,
//! which the typed convert layer surfaces as some flavour of error
//! rather than `Ok(None)`. Concretely:
//!
//! - `redis-rs` 1.2: `Err(Io: timed out)` if the connection's
//!   `response_timeout` fires (we set it to `None` in `pool.rs`), then
//!   nil → `Ok(None)` only when the typed return is `Option<T>`.
//! - `fred` 10: `Err(ErrorKind::Timeout, "Request timed out.")` always.
//!
//! This module guarantees the documented Redis semantics: *deadline
//! elapsed → `Ok(None)`, data arrived → `Ok(Some((key, value)))`,
//! anything else → `Err(...)`. Domain code never has to handle the
//! library-specific timeout error variant.

use std::time::Duration;

use redis::AsyncCommands;
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for list operations. Returned by [`RedisPool::lists`].
pub struct ListsOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> ListsOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `LPUSH key value`. Returns the new length of the list.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn lpush<V>(&self, key: &str, value: V) -> RedisResult<i64>
    where
        V: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.lpush(key, value).await.map_err(RedisError::Command)
    }

    /// `RPUSH key value`. Returns the new length of the list.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn rpush<V>(&self, key: &str, value: V) -> RedisResult<i64>
    where
        V: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.rpush(key, value).await.map_err(RedisError::Command)
    }

    /// `LPOP key`. Non-blocking. Returns `None` if the list is empty.
    #[instrument(skip(self), level = "trace")]
    pub async fn lpop<V>(&self, key: &str) -> RedisResult<Option<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.lpop(key, None).await.map_err(RedisError::Command)
    }

    /// `RPOP key`. Non-blocking. Returns `None` if the list is empty.
    #[instrument(skip(self), level = "trace")]
    pub async fn rpop<V>(&self, key: &str) -> RedisResult<Option<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.rpop(key, None).await.map_err(RedisError::Command)
    }

    /// `LLEN key`. Returns the list length, 0 for missing keys.
    #[instrument(skip(self), level = "trace")]
    pub async fn llen(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.llen(key).await.map_err(RedisError::Command)
    }

    /// **`BLPOP key timeout`** with the documented `Ok(None)`-on-timeout
    /// semantics.
    ///
    /// Returns:
    /// - `Ok(Some((key, value)))` when an element was popped before the
    ///   deadline elapsed.
    /// - `Ok(None)` when `timeout` elapsed without any LPUSH on the key.
    /// - `Err(_)` for genuine errors (connection lost, decode failure,
    ///   server replied with the wrong shape).
    ///
    /// Pass `Duration::ZERO` to block indefinitely (Redis convention).
    ///
    /// ## Why this method exists
    ///
    /// Without this wrapper, the canary that proves the RAG queue's
    /// round-trip works would surface its 3-second deadline as
    /// `Failure("action failed")` to the autonomy engine — masking a
    /// real signal (worker took too long) under a fake one (BLPOP
    /// returned an error). See `tools/redis-blpop-poc/` for the
    /// empirical evidence and `crates/the-one-memory/src/rag/queue.rs` for
    /// the original call site.
    #[instrument(skip(self), level = "debug", fields(key = %key, timeout_ms = timeout.as_millis() as u64))]
    pub async fn blpop(
        &self,
        key: &str,
        timeout: Duration,
    ) -> RedisResult<Option<(String, String)>> {
        // v0.1.363: blocking commands MUST use a dedicated connection
        // (not the shared multiplexed one) so they don't head-of-line-
        // block every other Redis call in the process during their
        // BLOCK window. See `RedisPool::dedicated_conn` doc for the
        // full rationale + the investigation link.
        let mut conn = self.pool.dedicated_conn().await?;
        // `redis-rs` 1.2 already returns `Ok(None)` on nil-timeout when
        // the typed return is `Option<T>` — UNLIKE fred 10. Our pool
        // disables the 500ms `response_timeout` default so the BLPOP
        // call can wait the full caller-supplied deadline without a
        // client-side cut-off racing the server.
        //
        // The cast to `f64` matches Redis's BLPOP timeout protocol
        // (Redis 6.0+ accepts a double for sub-second resolution).
        let timeout_secs = timeout.as_secs_f64();
        let res: Option<(String, String)> = conn
            .blpop(key, timeout_secs)
            .await
            .map_err(RedisError::Command)?;
        Ok(res)
    }

    /// `LRANGE key start stop`. `0..-1` returns the whole list.
    #[instrument(skip(self), level = "trace")]
    pub async fn lrange<V>(&self, key: &str, start: isize, stop: isize) -> RedisResult<Vec<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.lrange(key, start, stop)
            .await
            .map_err(RedisError::Command)
    }

    /// `LTRIM key start stop`. Keep only the elements in the `[start,
    /// stop]` inclusive range (0-based, negative indices count from the
    /// tail).
    ///
    /// Typical use: capped history lists — `ltrim(key, 0, N-1)` after
    /// each `lpush` keeps at most `N` latest entries.
    #[instrument(skip(self), level = "trace")]
    pub async fn ltrim(&self, key: &str, start: isize, stop: isize) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let _: () = conn
            .ltrim(key, start, stop)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `LREM key count value`. Remove up to `|count|` occurrences of
    /// `value` from the list. Positive `count` scans head→tail, negative
    /// scans tail→head, `0` removes all occurrences. Returns the number
    /// of elements actually removed.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn lrem<V>(&self, key: &str, count: isize, value: V) -> RedisResult<u64>
    where
        V: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.lrem(key, count, value)
            .await
            .map_err(RedisError::Command)
    }
}
