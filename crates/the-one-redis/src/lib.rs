//! the-one-mcp's Redis facade — the **only** crate that talks to Redis directly.
//!
//! ## Why this exists
//!
//! the-one-mcp accumulated three different one-off workarounds for `fred` quirks in
//! a single week (XREADGROUP nil-decode panic, XAUTOCLAIM same shape,
//! BLPOP-returns-`Err(Timeout)`-not-`Ok(None)`). Each lived in a different
//! file in a different shape, and the next blocking-command call site
//! would have shipped the bug fresh. This crate stops the bleeding by
//! making fred-style quirks impossible to leak past one boundary.
//!
//! ## Substrate
//!
//! Built on **`redis-rs` 1.2** ([crates.io](https://crates.io/crates/redis)).
//! Picked over `fred` and `rustis` after empirical comparison — see
//! `tools/redis-blpop-poc/` for the side-by-side BLPOP semantics test that
//! drove the choice. `redis-rs` ships one footgun of its own
//! ([`DEFAULT_RESPONSE_TIMEOUT = 500ms`][1]) that this facade neutralises
//! so callers never have to know about it.
//!
//! [1]: https://docs.rs/redis/1.2.0/redis/struct.AsyncConnectionConfig.html#method.set_response_timeout
//!
//! ## API shape
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use the_one_redis::{RedisPool, PoolConfig};
//! use std::time::Duration;
//!
//! let pool = RedisPool::new(PoolConfig::from_url("redis://127.0.0.1:6379")).await?;
//!
//! // Strings
//! pool.keys().set("foo", "bar").await?;
//! let v: Option<String> = pool.keys().get("foo").await?;
//!
//! // Blocking lists — Ok(None) on timeout, NEVER Err(Timeout)
//! let popped: Option<(String, String)> =
//!     pool.lists().blpop("key", Duration::from_secs(5)).await?;
//!
//! // Streams
//! pool.streams().xadd("stream", &[("k", "v")], None).await?;
//!
//! // RediSearch — typed FT.* command construction
//! let query = the_one_redis::search::Query::new("*");
//! let hits = pool.search().ft_search("idx", &query).await?;
//! # Ok(()) }
//! ```
//!
//! See `docs/guides/redis-facade.md` for the architectural rationale and
//! the migration plan from direct fred usage.

#![warn(missing_docs)]

pub mod error;
pub mod hashes;
pub mod keys;
pub mod lists;
pub mod pool;
pub mod pubsub;
pub mod search;
pub mod sets;
pub mod sorted_sets;
pub mod streams;
pub mod timeseries;

// Re-exports of the most-used public types so callers can write
// `use the_one_redis::{RedisPool, RedisError, RedisResult};` without
// reaching into individual modules.
pub use error::{RedisError, RedisResult};
pub use pool::{PoolConfig, RedisPool};

// Re-export `redis::Value` because parsers downstream of `ft_search`
// inspect raw values; keeping this in our public surface lets callers
// avoid taking a direct redis-rs dep just to name the type.
pub use redis::Value;
