//! String / KV operations: `GET`, `SET`, `DEL`, `EXISTS`, `EXPIRE`,
//! `GETDEL`, `SCAN`, `INCR`, `TTL`.

use std::time::Duration;

use redis::{AsyncCommands, FromRedisValue, ToRedisArgs, ToSingleRedisArg};
use tracing::instrument;

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Existence predicate for [`KeysOps::set_with_exat`]. Mirrors the
/// `[NX|XX]` clause in the `SET` command grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetPredicate {
    /// SET unconditionally (no predicate).
    Always,
    /// SET NX — only when the key does **not** already exist.
    IfMissing,
    /// SET XX — only when the key **already** exists.
    IfExisting,
}

impl SetPredicate {
    fn as_tag(self) -> Option<&'static str> {
        match self {
            Self::Always => None,
            Self::IfMissing => Some("NX"),
            Self::IfExisting => Some("XX"),
        }
    }
}

/// Handle for KV / string operations. Returned by [`RedisPool::keys`].
///
/// Zero-cost — internally just borrows the pool.
pub struct KeysOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> KeysOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `GET key`. Returns `None` if the key doesn't exist.
    #[instrument(skip(self), level = "trace")]
    pub async fn get<V>(&self, key: &str) -> RedisResult<Option<V>>
    where
        V: FromRedisValue,
    {
        let mut conn = self.pool.conn();
        conn.get(key).await.map_err(RedisError::Command)
    }

    /// `SET key value`. Overwrites any existing value.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn set<V>(&self, key: &str, value: V) -> RedisResult<()>
    where
        V: ToRedisArgs + ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        let _: () = conn.set(key, value).await.map_err(RedisError::Command)?;
        Ok(())
    }

    /// `SET key value EX seconds`. Set with TTL in one round-trip.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn set_ex<V>(&self, key: &str, value: V, ttl: Duration) -> RedisResult<()>
    where
        V: ToRedisArgs + ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        let _: () = conn
            .set_ex(key, value, ttl.as_secs().max(1))
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `DEL key`. Returns the number of keys actually deleted (0 or 1
    /// for a single key).
    #[instrument(skip(self), level = "trace")]
    pub async fn del(&self, key: &str) -> RedisResult<u64> {
        let mut conn = self.pool.conn();
        conn.del(key).await.map_err(RedisError::Command)
    }

    /// `EXISTS key`. `true` if the key is set.
    #[instrument(skip(self), level = "trace")]
    pub async fn exists(&self, key: &str) -> RedisResult<bool> {
        let mut conn = self.pool.conn();
        conn.exists(key).await.map_err(RedisError::Command)
    }

    /// `EXPIRE key seconds`. Returns `true` if TTL was set.
    #[instrument(skip(self), level = "trace")]
    pub async fn expire(&self, key: &str, ttl: Duration) -> RedisResult<bool> {
        let mut conn = self.pool.conn();
        let secs = ttl.as_secs() as i64;
        let res: i64 = conn.expire(key, secs).await.map_err(RedisError::Command)?;
        Ok(res == 1)
    }

    /// `TTL key`. Returns `None` for keys with no TTL or that don't exist.
    #[instrument(skip(self), level = "trace")]
    pub async fn ttl(&self, key: &str) -> RedisResult<Option<Duration>> {
        let mut conn = self.pool.conn();
        let secs: i64 = conn.ttl(key).await.map_err(RedisError::Command)?;
        // -2 = key does not exist; -1 = key has no TTL
        if secs < 0 {
            Ok(None)
        } else {
            Ok(Some(Duration::from_secs(secs as u64)))
        }
    }

    /// `GETDEL key`. Atomic get + delete. Returns the previous value or
    /// `None` if the key didn't exist.
    #[instrument(skip(self), level = "trace")]
    pub async fn getdel<V>(&self, key: &str) -> RedisResult<Option<V>>
    where
        V: FromRedisValue,
    {
        let mut conn = self.pool.conn();
        // redis-rs 1.x: getdel returns Option-shaped via type inference.
        redis::cmd("GETDEL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `INCR key`. Returns the new value.
    #[instrument(skip(self), level = "trace")]
    pub async fn incr(&self, key: &str) -> RedisResult<i64> {
        let mut conn = self.pool.conn();
        conn.incr(key, 1_i64).await.map_err(RedisError::Command)
    }

    /// `SET key value EXAT unix_secs [NX|XX]` — set with an *absolute*
    /// expiration time (seconds since Unix epoch) plus an optional
    /// existence predicate.
    ///
    /// - `predicate = SetPredicate::Always` — classic SET.
    /// - `predicate = SetPredicate::IfMissing` — SET NX, returns `true`
    ///   only when the key did not previously exist.
    /// - `predicate = SetPredicate::IfExisting` — SET XX, returns `true`
    ///   only when the key already existed.
    ///
    /// Designed for session stores and similar patterns where the TTL is
    /// a calendar deadline rather than a window — e.g. `expires_at`
    /// taken from a typed session record. `EXAT` was added in Redis 6.2.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn set_with_exat<V>(
        &self,
        key: &str,
        value: V,
        exat_unix_secs: i64,
        predicate: SetPredicate,
    ) -> RedisResult<bool>
    where
        V: ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("SET");
        cmd.arg(key).arg(value).arg("EXAT").arg(exat_unix_secs);
        if let Some(tag) = predicate.as_tag() {
            cmd.arg(tag);
        }
        // SET returns bulk "OK" on success, nil when NX/XX predicate
        // fails. redis-rs maps to Some(()) / None.
        let result: Option<()> = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(result.is_some())
    }

    /// `SET key value EX seconds NX` — Set with TTL only when the key does
    /// **not** already exist. Returns `true` if the key was set (lock
    /// acquired), `false` if the key already existed (lock held by
    /// another caller).
    ///
    /// This is the canonical Redis distributed-lock primitive. The TTL
    /// ensures automatic release if the lock-holder crashes.
    #[instrument(skip(self, value), level = "trace")]
    pub async fn set_ex_nx<V>(&self, key: &str, value: V, ttl: Duration) -> RedisResult<bool>
    where
        V: ToRedisArgs + Send + Sync,
    {
        let mut conn = self.pool.conn();
        // SET NX returns `OK` (bulk string) when the key was set, and nil
        // when the key already existed. redis-rs maps these to `Some(())`
        // and `None` respectively when decoded as `Option<()>`.
        let result: Option<()> = redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .arg("NX")
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(result.is_some())
    }

    /// `TYPE key`. Returns the Redis type name (`"string"`, `"list"`,
    /// `"set"`, `"zset"`, `"hash"`, `"stream"`, or `"none"` for missing).
    #[instrument(skip(self), level = "trace")]
    pub async fn key_type(&self, key: &str) -> RedisResult<String> {
        let mut conn = self.pool.conn();
        redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `STRLEN key`. Length of the string stored at `key`; 0 if missing.
    #[instrument(skip(self), level = "trace")]
    pub async fn strlen(&self, key: &str) -> RedisResult<i64> {
        let mut conn = self.pool.conn();
        conn.strlen(key).await.map_err(RedisError::Command)
    }

    /// `DBSIZE`. Total number of keys in the currently-selected database.
    #[instrument(skip(self), level = "trace")]
    pub async fn dbsize(&self) -> RedisResult<i64> {
        let mut conn = self.pool.conn();
        redis::cmd("DBSIZE")
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `MGET key1 key2 ...`. Batch get. The returned vec mirrors `keys`
    /// in order; each slot is `None` when the corresponding key is
    /// missing.
    ///
    /// Preferred over N sequential `get`s when N > 3 — one round-trip
    /// beats N pipeline round-trips for small batches.
    #[instrument(skip(self), level = "trace", fields(n = keys.len()))]
    pub async fn mget<V>(&self, keys: &[&str]) -> RedisResult<Vec<Option<V>>>
    where
        V: FromRedisValue,
    {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.pool.conn();
        // redis-rs types mget-on-single-key differently than mget-on-many
        // — on one key it returns V, on many it returns Vec<V>. Pin to
        // Vec by always going through the command builder.
        let mut cmd = redis::cmd("MGET");
        for k in keys {
            cmd.arg(*k);
        }
        cmd.query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `MSET key1 value1 key2 value2 ...`. Atomic batch set — all keys
    /// are set in a single step (server-side), so they either all
    /// succeed or all fail. No TTL support; use a pipeline of
    /// `set_ex` when each key needs its own TTL.
    #[instrument(skip(self, pairs), level = "trace", fields(n = pairs.len()))]
    pub async fn mset(&self, pairs: &[(&str, &[u8])]) -> RedisResult<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("MSET");
        for (k, v) in pairs {
            cmd.arg(*k).arg(*v);
        }
        let _: () = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `RENAME source destination`. Atomically renames `source` to
    /// `destination`, overwriting any existing key at `destination`.
    ///
    /// Errors if `source` does not exist. Callers that want the
    /// `RENAMENX` (no-overwrite) variant should open a targeted method
    /// when the need arises — this wrapper is the `RENAME` convention
    /// used by write-then-swap atomicity patterns.
    #[instrument(skip(self), level = "trace")]
    pub async fn rename(&self, source: &str, destination: &str) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let _: () = redis::cmd("RENAME")
            .arg(source)
            .arg(destination)
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `SCAN MATCH pattern COUNT count` — collected into a Vec.
    ///
    /// Caps at `limit` keys to bound memory. Pass a large `limit` only
    /// when you know the keyspace is small.
    #[instrument(skip(self), level = "trace")]
    pub async fn scan_match(&self, pattern: &str, limit: usize) -> RedisResult<Vec<String>> {
        let mut conn = self.pool.conn();
        let mut out = Vec::new();
        let mut iter: redis::AsyncIter<String> = conn
            .scan_match(pattern)
            .await
            .map_err(RedisError::Command)?;
        use futures::StreamExt;
        while let Some(item) = iter.next().await {
            // redis-rs 1.x AsyncIter yields Result per item — propagate
            // the first error we hit rather than silently dropping it.
            let key = item.map_err(RedisError::Command)?;
            out.push(key);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }
}
