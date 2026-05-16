//! Pub/sub operations: `PUBLISH` and `SUBSCRIBE`.
//!
//! ## Connection topology
//!
//! Redis pub/sub monopolises a connection's read half. Subscribers MUST
//! use a dedicated connection separate from the multiplexed one used for
//! ordinary commands. This module hides that detail by opening a fresh
//! connection inside [`PubSubOps::subscribe`] and returning a
//! [`SubscriberStream`] that owns it.
//!
//! Publish, by contrast, is a normal request/reply command and runs on
//! the shared multiplexed connection.

use futures::Stream;
use redis::aio::PubSub;
use redis::AsyncCommands;
use tracing::{debug, instrument};

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for pub/sub operations. Returned by [`RedisPool::pubsub`].
pub struct PubSubOps<'a> {
    pool: &'a RedisPool,
}

impl<'a> PubSubOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `PUBLISH channel message`. Returns the number of subscribers that
    /// received the message.
    #[instrument(skip(self, message), level = "trace", fields(channel = %channel))]
    pub async fn publish<M>(&self, channel: &str, message: M) -> RedisResult<u64>
    where
        M: redis::ToRedisArgs + redis::ToSingleRedisArg + Send + Sync,
    {
        let mut conn = self.pool.conn();
        conn.publish(channel, message)
            .await
            .map_err(RedisError::Command)
    }

    /// `SUBSCRIBE channel [channel ...]` — opens a dedicated connection
    /// and subscribes to all channels in one round-trip. Returns a
    /// [`SubscriberStream`] that yields incoming messages until dropped.
    ///
    /// Subscriber connections **cannot** be reused for normal commands.
    /// If the caller needs both, hold a separate `RedisPool` clone for
    /// the publisher side.
    #[instrument(skip(self), level = "debug", fields(channel_count = channels.len()))]
    pub async fn subscribe(&self, channels: &[&str]) -> RedisResult<SubscriberStream> {
        let conn = self
            .pool
            .client()
            .get_async_pubsub()
            .await
            .map_err(RedisError::PoolInit)?;
        let mut pubsub = conn;
        for ch in channels {
            pubsub.subscribe(*ch).await.map_err(RedisError::Command)?;
        }
        debug!(channels = ?channels, "the-one-redis pubsub subscribed");
        Ok(SubscriberStream { inner: pubsub })
    }

    /// `PSUBSCRIBE pattern [pattern ...]` — opens a dedicated connection
    /// and pattern-subscribes in one round-trip. Returns a
    /// [`SubscriberStream`]; [`PubSubMessage::channel`] on received messages
    /// is the actual published channel (not the pattern).
    ///
    /// Same rules as [`subscribe`](Self::subscribe): dedicated connection,
    /// cannot run normal commands.
    #[instrument(skip(self), level = "debug", fields(pattern_count = patterns.len()))]
    pub async fn subscribe_patterns(&self, patterns: &[&str]) -> RedisResult<SubscriberStream> {
        let conn = self
            .pool
            .client()
            .get_async_pubsub()
            .await
            .map_err(RedisError::PoolInit)?;
        let mut pubsub = conn;
        for pat in patterns {
            pubsub.psubscribe(*pat).await.map_err(RedisError::Command)?;
        }
        debug!(patterns = ?patterns, "the-one-redis pubsub pattern-subscribed");
        Ok(SubscriberStream { inner: pubsub })
    }
}

/// Active subscription. Yields messages via [`SubscriberStream::recv`]
/// until the underlying connection is dropped.
///
/// Wraps `redis::aio::PubSub` so callers don't import upstream types
/// directly and so we can swap the substrate later.
pub struct SubscriberStream {
    inner: PubSub,
}

impl SubscriberStream {
    /// Wait for the next message on any subscribed channel. Returns
    /// `None` when the connection is closed.
    pub async fn recv(&mut self) -> Option<PubSubMessage> {
        use futures::StreamExt;
        let mut stream = self.inner.on_message();
        let msg = stream.next().await?;
        let payload = match msg.get_payload::<String>() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "pubsub message decode failed; skipping");
                return None;
            }
        };
        Some(PubSubMessage {
            channel: msg.get_channel_name().to_string(),
            payload,
        })
    }

    /// Subscribe to additional channels on the existing connection.
    pub async fn add_channel(&mut self, channel: &str) -> RedisResult<()> {
        self.inner
            .subscribe(channel)
            .await
            .map_err(RedisError::Command)
    }

    /// Unsubscribe from a channel without dropping the connection.
    pub async fn remove_channel(&mut self, channel: &str) -> RedisResult<()> {
        self.inner
            .unsubscribe(channel)
            .await
            .map_err(RedisError::Command)
    }

    /// Borrow the underlying tokio Stream of raw `redis::Msg` values.
    /// Use when you need byte payloads, channel patterns, or the lower-
    /// level [`redis::Msg`] type rather than the simplified shape of
    /// [`PubSubMessage`].
    pub fn raw_stream(&mut self) -> impl Stream<Item = redis::Msg> + '_ {
        self.inner.on_message()
    }
}

/// One pub/sub message delivered to a [`SubscriberStream`].
#[derive(Debug, Clone)]
pub struct PubSubMessage {
    /// The channel the message was published on.
    pub channel: String,
    /// UTF-8 decoded payload. For binary payloads use
    /// [`SubscriberStream::raw_stream`] instead.
    pub payload: String,
}
