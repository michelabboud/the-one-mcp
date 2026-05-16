//! Regression test: the pool's connection config must keep
//! `response_timeout = None` so blocking commands aren't capped at
//! `redis-rs`'s 500ms `DEFAULT_RESPONSE_TIMEOUT`.
//!
//! ## Why this test exists
//!
//! `redis-rs` 1.2 ships
//! [`DEFAULT_RESPONSE_TIMEOUT = Some(Duration::from_millis(500))`][1] on
//! every connection. If we ever forget to override it (e.g. someone
//! "cleans up" the explicit `set_response_timeout(None)` in
//! `pool.rs::connection_config`), every BLPOP/BRPOP in the-one-mcp will fire at
//! 500ms regardless of the deadline the caller passed — silently
//! racing real LPUSH-ed data against a client-side timeout the server
//! doesn't know about.
//!
//! This test verifies that BLPOP with a 1.5-second deadline waits the
//! full 1.5 seconds (not 500ms). If the response_timeout regresses, the
//! BLPOP returns in ~500ms and the assertion fails.
//!
//! [1]: https://docs.rs/redis/1.2.0/src/redis/client.rs.html
//!
//! Run with `cargo test -p the-one-redis -- --include-ignored`.

use std::time::{Duration, Instant};

use the_one_redis::{PoolConfig, RedisPool};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn blpop_waits_full_deadline_not_clamped_to_500ms() {
    let pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new");

    let key = format!(
        "the-one-redis-test:response-timeout:{}",
        uuid::Uuid::new_v4()
    );

    let started = Instant::now();
    let res = pool
        .lists()
        .blpop(&key, Duration::from_millis(1500))
        .await
        .expect("blpop");
    let elapsed = started.elapsed();

    assert!(res.is_none(), "blpop must return None on deadline expiry");
    // The cap that bites us is exactly 500ms. If the test fails because
    // elapsed < 1.4s, response_timeout regressed and is being honoured
    // somewhere in the connection setup.
    assert!(
        elapsed >= Duration::from_millis(1400),
        "blpop returned in {elapsed:?} — DEFAULT_RESPONSE_TIMEOUT regression: \
         pool.rs::connection_config must set_response_timeout(None)"
    );
    // Upper bound is generous because cargo runs integration tests in
    // parallel and the BLPOP can queue on the same single-thread Redis.
    // The interesting assertion is the LOWER bound (the regression guard).
    assert!(
        elapsed < Duration::from_secs(10),
        "blpop took {elapsed:?} — extreme outlier suggesting a real hang"
    );
}
