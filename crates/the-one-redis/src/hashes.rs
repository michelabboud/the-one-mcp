//! Hash operations: `HGET`, `HSET`, `HGETALL`, `HDEL`, `HKEYS`, `HEXISTS`.

use std::collections::HashMap;

use redis::AsyncCommands;
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for hash operations. Returned by [`RedisPool::hashes`].
pub struct HashesOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> HashesOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `HSET key field value`. Returns 1 if the field was new, 0 if it
    /// was overwritten.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn hset<V>(&self, key: &str, field: &str, value: V) -> RedisResult<u8>
    where
        V: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.hset(key, field, value)
            .await
            .map_err(RedisError::Command)
    }

    /// `HSET key field1 value1 field2 value2 ...` — variadic multi-field
    /// set in one round-trip. Returns the number of fields that were
    /// newly created (existing fields are overwritten but not counted).
    ///
    /// Useful when a single logical write has many columns. Values can
    /// mix strings, integers, and binary bytes — each implements
    /// `ToRedisArgs` on redis-rs. Order of pairs is preserved.
    #[instrument(skip(self, fields), level = "trace", fields(field_count = fields.len()))]
    pub async fn hset_multi(&self, key: &str, fields: &[(&str, &[u8])]) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("HSET");
        cmd.arg(key);
        for (name, value) in fields {
            cmd.arg(*name).arg(*value);
        }
        cmd.query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `HGET key field`. `None` for missing field.
    #[instrument(skip(self), level = "trace")]
    pub async fn hget<V>(&self, key: &str, field: &str) -> RedisResult<Option<V>>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.hget(key, field).await.map_err(RedisError::Command)
    }

    /// `HMGET key field1 field2 ...` — variadic multi-field read in one
    /// round-trip. Returns a vector of `Option<V>` aligned 1:1 with
    /// `fields`, where `None` indicates the field was absent in the
    /// hash (or the hash itself does not exist). Empty `fields` returns
    /// an empty `Vec` without contacting Redis.
    ///
    /// Preferred over N sequential `hget`s when N > 1 — one round-trip
    /// beats N pipeline round-trips for small batches and matches the
    /// `keys().mget()` ergonomics for the string-KV layer.
    #[instrument(skip(self), level = "trace", fields(field_count = fields.len()))]
    pub async fn hmget<V>(&self, key: &str, fields: &[&str]) -> RedisResult<Vec<Option<V>>>
    where
        V: redis::FromRedisValue,
    {
        if fields.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.pool.conn();
        // Build the command manually so the result type is always
        // `Vec<Option<V>>` regardless of fields.len() (redis-rs's
        // single-vs-many-arg overload returns different shapes
        // otherwise — same gotcha that `mget` documents).
        let mut cmd = redis::cmd("HMGET");
        cmd.arg(key);
        for f in fields {
            cmd.arg(*f);
        }
        cmd.query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `HGETALL key`. Returns the full field-value map. Empty map for
    /// missing keys.
    #[instrument(skip(self), level = "trace")]
    pub async fn hgetall(&self, key: &str) -> RedisResult<HashMap<String, String>> {
        let mut conn = self.pool.conn();
        conn.hgetall(key).await.map_err(RedisError::Command)
    }

    /// `HDEL key field`. Returns the number of fields actually removed.
    #[instrument(skip(self), level = "trace")]
    pub async fn hdel(&self, key: &str, field: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.hdel(key, field).await.map_err(RedisError::Command)
    }

    /// `HKEYS key`. Returns the list of field names.
    #[instrument(skip(self), level = "trace")]
    pub async fn hkeys(&self, key: &str) -> RedisResult<Vec<String>> {
        let mut conn = self.pool.conn();
        conn.hkeys(key).await.map_err(RedisError::Command)
    }

    /// `HEXISTS key field`. `true` if the field exists.
    #[instrument(skip(self), level = "trace")]
    pub async fn hexists(&self, key: &str, field: &str) -> RedisResult<bool> {
        let mut conn = self.pool.conn();
        conn.hexists(key, field).await.map_err(RedisError::Command)
    }

    /// `HINCRBY key field delta`. Atomically increments the integer
    /// value of hash field `field` by `delta` (negative values decrement).
    /// Creates the field with value `delta` if it did not exist. Returns
    /// the new value.
    ///
    /// Typical use: per-user counters (requests, tokens, errors) keyed by
    /// one hash per user.
    #[instrument(skip(self), level = "trace")]
    pub async fn hincrby(&self, key: &str, field: &str, delta: i64) -> RedisResult<i64> {
        let mut conn = self.pool.conn();
        conn.hincr(key, field, delta)
            .await
            .map_err(RedisError::Command)
    }

    /// `HLEN key`. Returns the number of fields in the hash; 0 for
    /// missing keys.
    #[instrument(skip(self), level = "trace")]
    pub async fn hlen(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.hlen(key).await.map_err(RedisError::Command)
    }
}
