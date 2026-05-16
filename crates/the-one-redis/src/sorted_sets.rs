//! Sorted set operations: `ZADD`, `ZREM`, `ZRANGE`, `ZSCORE`, `ZCARD`.

use redis::AsyncCommands;
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for sorted set operations. Returned by [`RedisPool::sorted_sets`].
pub struct SortedSetsOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> SortedSetsOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `ZADD key score member`. Returns 1 if the member is new, 0 if it
    /// already existed (and the score was updated).
    #[instrument(skip(self, member), level = "trace")]
    pub async fn zadd<M>(&self, key: &str, score: f64, member: M) -> RedisResult<u8>
    where
        M: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.zadd(key, member, score)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZREM key member`. Returns the count of members removed.
    #[instrument(skip(self, member), level = "trace")]
    pub async fn zrem<M>(&self, key: &str, member: M) -> RedisResult<u64>
    where
        M: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.zrem(key, member).await.map_err(RedisError::Command)
    }

    /// `ZRANGE key start stop`. Use `0..-1` for the whole set.
    #[instrument(skip(self), level = "trace")]
    pub async fn zrange<V>(&self, key: &str, start: isize, stop: isize) -> RedisResult<Vec<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.zrange(key, start, stop)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZRANGE key start stop WITHSCORES`.
    #[instrument(skip(self), level = "trace")]
    pub async fn zrange_with_scores(
        &self,
        key: &str,
        start: isize,
        stop: isize,
    ) -> RedisResult<Vec<(String, f64)>> {
        let mut conn = self.pool.conn();
        conn.zrange_withscores(key, start, stop)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZREVRANGE key start stop`. Like [`zrange`] but returns members in
    /// descending score order.
    ///
    /// Typical use: a history sorted set scored by timestamp — `zrevrange
    /// 0 N-1` returns the `N` most recent entries newest-first without a
    /// client-side sort.
    ///
    /// [`zrange`]: Self::zrange
    #[instrument(skip(self), level = "trace")]
    pub async fn zrevrange<V>(&self, key: &str, start: isize, stop: isize) -> RedisResult<Vec<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.zrevrange(key, start, stop)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZSCORE key member`. `None` if the member is missing.
    #[instrument(skip(self, member), level = "trace")]
    pub async fn zscore<M>(&self, key: &str, member: M) -> RedisResult<Option<f64>>
    where
        M: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.zscore(key, member).await.map_err(RedisError::Command)
    }

    /// `ZCARD key`. Returns the cardinality.
    #[instrument(skip(self), level = "trace")]
    pub async fn zcard(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.zcard(key).await.map_err(RedisError::Command)
    }

    /// `ZINCRBY key delta member`. Atomically adds `delta` to the score of
    /// `member` inside sorted set `key`. If the member does not exist it is
    /// created with score `delta`. Returns the new score as a `f64`.
    ///
    /// Typical use: feature-usage counters, trending leaderboards.
    #[instrument(skip(self, member), level = "trace")]
    pub async fn zincrby<M>(&self, key: &str, delta: f64, member: M) -> RedisResult<f64>
    where
        M: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.zincr(key, member, delta)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZRANGEBYSCORE key min max [WITHSCORES] [LIMIT offset count]`.
    ///
    /// `min` / `max` are passed as `impl ToRedisArgs` so callers can use
    /// numeric literals, `"-inf"` / `"+inf"`, exclusive `(score` notation,
    /// or the typed `redis::Score*` helpers. `with_scores` appends
    /// `WITHSCORES` — when true the result shape is `[(member, score), …]`
    /// and callers must decode it as such (typed `Vec<(String, f64)>`).
    /// When false, only the member list is returned.
    ///
    /// `limit` pages the reply via `LIMIT offset count`. Pass `None` for
    /// the whole range.
    ///
    /// This is the typed surface for both result shapes — callers pick the
    /// decoded form at the generic `V` parameter. For the common
    /// `(member, score)` shape use [`Self::zrangebyscore_with_scores`].
    #[instrument(skip(self, min, max), level = "trace")]
    pub async fn zrangebyscore<V, Min, Max>(
        &self,
        key: &str,
        min: Min,
        max: Max,
        with_scores: bool,
        limit: Option<(isize, isize)>,
    ) -> RedisResult<Vec<V>>
    where
        V: redis::FromRedisValue,
        Min: redis::ToRedisArgs + Send + Sync,
        Max: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("ZRANGEBYSCORE");
        cmd.arg(key).arg(min).arg(max);
        if with_scores {
            cmd.arg("WITHSCORES");
        }
        if let Some((offset, count)) = limit {
            cmd.arg("LIMIT").arg(offset).arg(count);
        }
        cmd.query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZREMRANGEBYSCORE key min max`. Remove all members in the sorted
    /// set with scores inside the inclusive `[min, max]` range (use `(x`
    /// for exclusive). Returns the number of members removed.
    ///
    /// Typical use: drop due entries from a time-sorted work queue after
    /// processing.
    #[instrument(skip(self, min, max), level = "trace")]
    pub async fn zremrangebyscore<Min, Max>(
        &self,
        key: &str,
        min: Min,
        max: Max,
    ) -> RedisResult<u64>
    where
        Min: redis::ToRedisArgs + Send + Sync,
        Max: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        redis::cmd("ZREMRANGEBYSCORE")
            .arg(key)
            .arg(min)
            .arg(max)
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `ZREMRANGEBYRANK key start stop`. Remove all members in the sorted
    /// set with rank between `start` and `stop` (inclusive, 0-based,
    /// negative indices count from the tail). Returns the number of
    /// elements removed.
    ///
    /// Typical use: trim a history sorted set to at most N entries after
    /// each write. Pass `start=0, stop=-(N+1)` to remove everything
    /// except the latest N entries (rank 0 is lowest score).
    #[instrument(skip(self), level = "trace")]
    pub async fn zremrangebyrank(&self, key: &str, start: isize, stop: isize) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.zremrangebyrank(key, start, stop)
            .await
            .map_err(RedisError::Command)
    }
}
