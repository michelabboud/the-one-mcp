//! Error types for `the-one-redis`. Wraps `redis::RedisError` so callers
//! never have to import the upstream error type directly, and provides
//! a `From<RedisError> for CoreError` conversion so the rest of the
//! workspace can continue to bubble errors through `?` without ceremony.

use the_one_core::error::CoreError;
use thiserror::Error;

/// Result alias used throughout the facade.
pub type RedisResult<T> = std::result::Result<T, RedisError>;

/// Domain-shaped Redis error. Wraps `redis::RedisError` and adds a few
/// facade-specific variants so callers can pattern-match on conditions
/// that the raw upstream type buries.
#[derive(Debug, Error)]
pub enum RedisError {
    /// The Redis server was reachable but the command failed.
    #[error("Redis command failed: {0}")]
    Command(#[source] redis::RedisError),

    /// Pool / connection setup failed.
    #[error("Redis pool init failed: {0}")]
    PoolInit(#[source] redis::RedisError),

    /// A typed conversion of the response failed.
    /// E.g. expected `Option<(String, String)>` from `BLPOP` but the
    /// reply did not match.
    #[error("Redis response decode failed for {context}: {source}")]
    Decode {
        /// Free-form description of where the decode happened — used in
        /// the error message and tracing.
        context: String,
        /// The underlying `redis-rs` error.
        #[source]
        source: redis::RedisError,
    },

    /// A blocking command (BLPOP, BRPOP, BLMPOP, BZPOPMIN, BZPOPMAX) was
    /// asked to block but the wait elapsed without data.
    ///
    /// **This is normally not surfaced** — the facade converts the timeout
    /// into `Ok(None)` for the typed `Option<T>` returns of those methods
    /// (the documented semantics). It exists for callers that want to
    /// distinguish "deadline expired" from "server replied with no data
    /// of the expected shape" via the `match` API on the lower-level
    /// `*_raw` helpers.
    #[error("Redis blocking command timed out after {timeout_ms}ms")]
    BlockingTimeout {
        /// The deadline the caller passed.
        timeout_ms: u64,
    },

    /// Reply-parse failure — the command succeeded on the server but the
    /// facade couldn't decode the response into its typed shape. Used by
    /// RediSearch's nested array replies, RedisTimeSeries's tuple
    /// replies, and any future module with a non-trivial reply format.
    #[error("Redis reply parse failed: {0}")]
    ReplyParse(String),
}

impl From<RedisError> for CoreError {
    fn from(value: RedisError) -> Self {
        CoreError::Redis(value.to_string())
    }
}

/// Convenience: convert `redis::RedisError` directly. Most internal call
/// sites use this via `.map_err(RedisError::Command)?` or via `?` after
/// adopting `From`.
impl From<redis::RedisError> for RedisError {
    fn from(value: redis::RedisError) -> Self {
        Self::Command(value)
    }
}
