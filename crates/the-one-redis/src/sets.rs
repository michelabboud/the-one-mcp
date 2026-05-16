//! Unordered set operations: `SADD`, `SREM`, `SMEMBERS`, `SISMEMBER`,
//! `SCARD`.
//!
//! Added during external session storage, which stores per-chat
//! membership lists (admins, participants) as Redis sets.

use redis::AsyncCommands;
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for set operations. Returned by [`RedisPool::sets`].
pub struct SetsOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> SetsOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `SADD key member`. Returns 1 if the member was new, 0 if it was
    /// already present.
    #[instrument(skip(self, member), level = "trace")]
    pub async fn sadd<M>(&self, key: &str, member: M) -> RedisResult<u8>
    where
        M: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.sadd(key, member).await.map_err(RedisError::Command)
    }

    /// `SREM key member`. Returns the count of members actually removed
    /// (0 or 1 for a single member).
    #[instrument(skip(self, member), level = "trace")]
    pub async fn srem<M>(&self, key: &str, member: M) -> RedisResult<u64>
    where
        M: redis::ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.srem(key, member).await.map_err(RedisError::Command)
    }

    /// `SMEMBERS key`. Returns all members of the set; empty Vec for
    /// missing keys.
    #[instrument(skip(self), level = "trace")]
    pub async fn smembers<V>(&self, key: &str) -> RedisResult<Vec<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.smembers(key).await.map_err(RedisError::Command)
    }

    /// `SISMEMBER key member`. `true` if the member is in the set.
    #[instrument(skip(self, member), level = "trace")]
    pub async fn sismember<M>(&self, key: &str, member: M) -> RedisResult<bool>
    where
        M: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.sismember(key, member)
            .await
            .map_err(RedisError::Command)
    }

    /// `SCARD key`. Returns the set cardinality; 0 for missing keys.
    #[instrument(skip(self), level = "trace")]
    pub async fn scard(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.scard(key).await.map_err(RedisError::Command)
    }
}
