//! Redis connection pool. The single sanctioned construction point for a
//! Redis client in the-one-mcp.
//!
//! ## Why a facade pool
//!
//! `redis-rs` 1.2 ships [`DEFAULT_RESPONSE_TIMEOUT = Some(500ms)`][1] on
//! every connection. That default fires before BLPOP/BRPOP/BLMPOP can
//! return — converting our 5-second blocking-pop into a guaranteed
//! `Err(Io: timed out)` after 500ms — and silently races real LPUSH-ed
//! data against a client-side timeout that the server doesn't know about.
//! The POC at `tools/redis-blpop-poc/` reproduces this in 60 lines.
//!
//! This module disables that default at construction so no caller anywhere
//! else in the-one-mcp has to remember the magic line.
//!
//! [1]: https://docs.rs/redis/1.2.0/src/redis/client.rs.html

use std::sync::Arc;

use redis::aio::MultiplexedConnection;
use redis::AsyncConnectionConfig;
use tracing::{info, instrument};

use crate::error::{RedisError, RedisResult};
use crate::{hashes, keys, lists, pubsub, search, sets, sorted_sets, streams, timeseries};

/// User-facing pool configuration. Build with [`PoolConfig::from_url`].
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Redis URL, e.g. `redis://127.0.0.1:6379` or `redis://:pass@host:6379/0`.
    pub url: String,
}

impl PoolConfig {
    /// Build a config from a Redis URL.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

/// the-one-mcp's Redis pool. Cheaply cloneable — internally an `Arc` over a
/// multiplexed connection.
///
/// ## Lifecycle
///
/// `RedisPool::new` opens one [`redis::Client`] and pre-warms one
/// multiplexed connection so that init errors (bad URL, server
/// unreachable, auth failure) surface synchronously rather than on the
/// first command. The pre-warmed connection is reused for every
/// non-pubsub call site — it multiplexes safely across tasks.
///
/// `subscribe()` opens a separate dedicated connection because pub/sub
/// monopolises a connection's read half (Redis spec).
#[derive(Clone)]
pub struct RedisPool {
    inner: Arc<RedisPoolInner>,
}

/// Manual `Debug` — the underlying `redis::Client` and
/// `MultiplexedConnection` don't implement it, so a `derive(Debug)` on
/// the pool would cascade the constraint to every caller. We print a
/// single-line placeholder; none of the internal state would be safe to
/// dump (it holds credentials in the URL).
impl std::fmt::Debug for RedisPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisPool").finish_non_exhaustive()
    }
}

struct RedisPoolInner {
    client: redis::Client,
    /// The shared multiplexed connection used by every non-pubsub call.
    /// Cloning a `MultiplexedConnection` is cheap (it shares a single
    /// underlying tokio task that drives the socket) so we hand out
    /// clones rather than handing out a connection-per-call.
    conn: MultiplexedConnection,
}

impl RedisPool {
    /// Open a pool. Pre-warms one multiplexed connection — surfaces init
    /// errors synchronously.
    #[instrument(skip(config), fields(url = %redact_password(&config.url)))]
    pub async fn new(config: PoolConfig) -> RedisResult<Self> {
        let client = redis::Client::open(config.url.as_str()).map_err(RedisError::PoolInit)?;
        let conn = client
            .get_multiplexed_async_connection_with_config(&Self::connection_config())
            .await
            .map_err(RedisError::PoolInit)?;
        info!("the-one-redis pool connected");
        Ok(Self {
            inner: Arc::new(RedisPoolInner { client, conn }),
        })
    }

    /// The connection configuration applied to every non-pubsub
    /// connection. **`response_timeout` is set to `None` deliberately**
    /// (see module docs).
    pub(crate) fn connection_config() -> AsyncConnectionConfig {
        AsyncConnectionConfig::default().set_response_timeout(None)
    }

    /// Borrow a fresh handle to the shared multiplexed connection.
    /// Cheap clone — internal tokio task is shared.
    /// Issue an arbitrary Redis command — escape hatch for commands
    /// the typed handle modules don't wrap yet.
    ///
    /// `args` is passed positionally: the first element is the command
    /// name (e.g. `"ACL"`), the rest are its arguments. The reply is
    /// decoded into `V: FromRedisValue`, so callers pick the shape.
    ///
    /// Every direct use is a hint that the facade should grow a typed
    /// wrapper. Keep this confined to truly one-off commands
    /// (server-admin ACL/CLIENT/DEBUG calls, version probes, etc.) — if
    /// a command appears in 3+ call sites, promote it.
    #[tracing::instrument(skip(self, args), level = "trace", fields(argc = args.len()))]
    pub async fn raw_cmd<V>(&self, args: &[&str]) -> RedisResult<V>
    where
        V: redis::FromRedisValue,
    {
        let mut conn = self.conn();
        let (head, rest) = args
            .split_first()
            .ok_or_else(|| RedisError::ReplyParse("raw_cmd: empty args".into()))?;
        let mut cmd = redis::cmd(head);
        for arg in rest {
            cmd.arg(*arg);
        }
        cmd.query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    pub(crate) fn conn(&self) -> MultiplexedConnection {
        self.inner.conn.clone()
    }

    /// Open a fresh, single-task `MultiplexedConnection` that is NOT
    /// shared with the pool's long-lived multiplexed connection.
    ///
    /// **Required for blocking commands** (`BLPOP`, `BRPOP`, `BLMOVE`,
    /// `XREADGROUP` with `BLOCK > 0`, `XREAD` with `BLOCK > 0`, `WAIT`,
    /// etc.). Redis processes commands per-connection serially, so a
    /// blocking command sent on the shared multiplexed connection
    /// queues every subsequent non-blocking call (HGET, HSET,
    /// FT.SEARCH, …) behind it — up to the BLOCK timeout. The fix is
    /// to give every blocking call its own short-lived connection so
    /// the read-loop on `inner.conn` is never head-of-line-blocked.
    ///
    /// **The footgun this fixes (v0.1.363):** prior to this method,
    /// `lists::blpop`, `streams::xreadgroup`, and `streams::xread` all
    /// used `pool.conn()` (the shared multiplexed connection). With
    /// the SelfQA control listener cycling `XREADGROUP BLOCK 5000ms`
    /// and N RAG queue workers cycling the same, every Redis call in
    /// the system saw 0–5 s of head-of-line wait at random. Tail
    /// latency of `self_info` (a one-shot HLEN) hit the SelfQA 5 s
    /// deadline reliably; voice attach hit 28 s in the worst case.
    /// Investigation: `docs/plans/2026-05-16-fred-removal-and-bug-fixes.md`.
    ///
    /// **Cost:** each call opens a TCP connection (sub-ms in our
    /// localhost setup; ~1 ms in production). For long-lived workers
    /// (RAG queue, SelfQA control listener) the connection should be
    /// acquired ONCE outside the loop and reused. For ad-hoc blocking
    /// calls (BLPOP with timeout) acquiring per-call is fine.
    ///
    /// **Returns** `MultiplexedConnection` (not the lower-level
    /// `Connection`) because (a) `redis-rs` 1.2's bare `Connection` is
    /// not `Send` and most callers need to await across thread
    /// boundaries, and (b) using a fresh `MultiplexedConnection` per
    /// caller still gives us per-connection sequencing without
    /// blocking everyone else.
    pub(crate) async fn dedicated_conn(&self) -> RedisResult<MultiplexedConnection> {
        self.inner
            .client
            .get_multiplexed_async_connection_with_config(&Self::connection_config())
            .await
            .map_err(RedisError::PoolInit)
    }

    /// Borrow the underlying [`redis::Client`] — used only by the pubsub
    /// module which needs to construct a [`redis::aio::PubSub`] from a
    /// fresh connection.
    pub(crate) fn client(&self) -> &redis::Client {
        &self.inner.client
    }

    // ── Domain-namespaced operation handles ────────────────────────────
    //
    // Each one is zero-cost (just a `&self` borrow). Splits the API
    // surface into Redis-shaped categories so call-site code reads as
    // `pool.lists().blpop(...)` rather than 80 methods on the pool.

    /// String / KV operations: `GET`, `SET`, `DEL`, `EXISTS`, `EXPIRE`,
    /// `GETDEL`, `SCAN`, `INCR`.
    pub fn keys(&self) -> keys::KeysOps<'_> {
        keys::KeysOps::new(self)
    }

    /// List operations: `LPUSH`, `LPOP`, `RPUSH`, **`BLPOP`**, `LRANGE`,
    /// `LLEN`. The blocking variants honour the documented
    /// `Ok(None)`-on-timeout contract.
    pub fn lists(&self) -> lists::ListsOps<'_> {
        lists::ListsOps::new(self)
    }

    /// Stream operations: `XADD`, `XREAD`, `XREADGROUP`, `XAUTOCLAIM`,
    /// `XACK`, `XPENDING`, `XLEN`, `XDEL`, `XGROUP CREATE`.
    pub fn streams(&self) -> streams::StreamsOps<'_> {
        streams::StreamsOps::new(self)
    }

    /// Hash operations: `HGET`, `HSET`, `HGETALL`, `HDEL`, `HKEYS`.
    pub fn hashes(&self) -> hashes::HashesOps<'_> {
        hashes::HashesOps::new(self)
    }

    /// Sorted set operations: `ZADD`, `ZREM`, `ZRANGE`, `ZSCORE`.
    pub fn sorted_sets(&self) -> sorted_sets::SortedSetsOps<'_> {
        sorted_sets::SortedSetsOps::new(self)
    }

    /// Unordered set operations: `SADD`, `SREM`, `SMEMBERS`,
    /// `SISMEMBER`, `SCARD`.
    pub fn sets(&self) -> sets::SetsOps<'_> {
        sets::SetsOps::new(self)
    }

    /// Pub/sub operations: `PUBLISH` and `SUBSCRIBE`. `SUBSCRIBE` opens a
    /// dedicated connection (Redis pub/sub spec).
    pub fn pubsub(&self) -> pubsub::PubSubOps<'_> {
        pubsub::PubSubOps::new(self)
    }

    /// RediSearch operations: `FT.CREATE`, `FT.SEARCH`, `FT.INFO`,
    /// `FT.ALTER`, `FT.DROPINDEX`. RediSearch is built into Redis 8 core
    /// (no separate module load needed).
    pub fn search(&self) -> search::SearchOps<'_> {
        search::SearchOps::new(self)
    }

    /// Redis TimeSeries operations: `TS.CREATE`, `TS.ADD`, `TS.GET`,
    /// `TS.RANGE`. Requires the `redistimeseries` module loaded on the
    /// target Redis (bundled with Redis Stack / the-one-mcp's Redis target).
    pub fn timeseries(&self) -> timeseries::TimeSeriesOps<'_> {
        timeseries::TimeSeriesOps::new(self)
    }
}

/// Mask the password segment of a Redis URL for safe tracing output.
fn redact_password(url: &str) -> String {
    // redis://[username]:[password]@host:port/db
    if let Some(at) = url.find('@') {
        if let Some(colon_after_scheme) = url.find("://") {
            let scheme_end = colon_after_scheme + 3;
            // Find the colon between username and password
            if let Some(pw_colon) = url[scheme_end..at].find(':') {
                let pw_start = scheme_end + pw_colon + 1;
                let mut redacted = String::with_capacity(url.len());
                redacted.push_str(&url[..pw_start]);
                redacted.push_str("***");
                redacted.push_str(&url[at..]);
                return redacted;
            }
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password_in_url() {
        assert_eq!(
            redact_password("redis://user:secret@host:6379/0"),
            "redis://user:***@host:6379/0"
        );
    }

    #[test]
    fn leaves_passwordless_url_alone() {
        assert_eq!(
            redact_password("redis://127.0.0.1:6379"),
            "redis://127.0.0.1:6379"
        );
    }
}
