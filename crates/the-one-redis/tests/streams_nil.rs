//! Regression test: `XREADGROUP` with a finite block timeout on an empty
//! stream must return an empty `Vec` — NOT panic, NOT raise a parse
//! error, NOT propagate a typed-convert failure.
//!
//! ## Why this test exists
//!
//! On `fred` 10, this exact case caused the `XREADGROUP response decode
//! failed: Parse Error: Cannot convert to map` bug fixed by commit
//! `80c437ba` (manual RESP2 parser inline in the worker loop). The same
//! shape bit `XAUTOCLAIM` (commit `c0b7e27f`).
//!
//! `redis-rs` 1.2 with the `streams` feature returns a typed
//! `StreamReadReply` whose `Option`-shaped wrapper handles nil cleanly.
//! This test locks that contract — if a future redis-rs version changes
//! the typed convert behaviour, the test fails before it ships.
//!
//! Run with `cargo test -p the-one-redis -- --include-ignored`.

use std::time::Duration;

use the_one_redis::{PoolConfig, RedisPool};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn xreadgroup_returns_empty_on_idle() {
    let pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new");

    let stream = format!("the-one-redis-test:stream:{}", uuid::Uuid::new_v4());
    let group = "test-group";
    let consumer = "test-consumer";

    pool.streams()
        .xgroup_create(&stream, group, "$")
        .await
        .expect("xgroup_create");

    let entries = pool
        .streams()
        .xreadgroup(group, consumer, &stream, 1, Duration::from_millis(200))
        .await
        .expect("xreadgroup must NOT error on idle timeout — must return empty Vec");

    assert!(
        entries.is_empty(),
        "xreadgroup on idle stream must return empty Vec; got {entries:?}"
    );

    // Cleanup
    let _ = pool.keys().del(&stream).await;
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn xautoclaim_returns_empty_when_no_pending() {
    let pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new");

    let stream = format!("the-one-redis-test:autoclaim:{}", uuid::Uuid::new_v4());
    let group = "test-group";
    let consumer = "test-consumer";

    pool.streams()
        .xgroup_create(&stream, group, "$")
        .await
        .expect("xgroup_create");

    // No pending entries — XAUTOCLAIM should return (Vec::new(), "0-0").
    let (entries, next) = pool
        .streams()
        .xautoclaim(&stream, group, consumer, Duration::from_secs(60), "0-0", 10)
        .await
        .expect("xautoclaim must NOT error on empty pending — must return empty Vec");

    assert!(
        entries.is_empty(),
        "xautoclaim with no pending must return empty entries; got {entries:?}"
    );
    assert_eq!(
        next, "0-0",
        "xautoclaim cursor should be 0-0 when scan completes"
    );

    let _ = pool.keys().del(&stream).await;
}
