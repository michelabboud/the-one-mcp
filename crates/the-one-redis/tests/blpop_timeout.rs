//! Regression test: `BLPOP` on a non-existent key with a finite deadline
//! must return `Ok(None)` after the deadline elapses — NEVER an error.
//!
//! This locks the contract that the original bug (autonomy recording
//! `Failure("action failed")` when the RAG queue's worker took longer
//! than the canary deadline) cannot regress. If a future redis-rs upgrade
//! changes the BLPOP semantics back to fred-style timeout-as-error, this
//! test fails loudly in CI before the change ships.
//!
//! ## Running
//!
//! ```sh
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p the-one-redis -- --include-ignored
//! ```
//!
//! Defaults to `redis://127.0.0.1:6379` if `REDIS_URL` is unset.
//! Marked `#[ignore]` so the test only runs when explicitly requested or
//! against a CI environment that has Redis available.

use std::time::{Duration, Instant};

use the_one_redis::{PoolConfig, RedisPool};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn blpop_returns_ok_none_on_timeout() {
    let pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new");

    let key = format!("the-one-redis-test:blpop-timeout:{}", uuid::Uuid::new_v4());
    let started = Instant::now();
    let res = pool
        .lists()
        .blpop(&key, Duration::from_secs(1))
        .await
        .expect("blpop must return Ok on deadline expiry, NEVER Err");
    let elapsed = started.elapsed();

    assert!(
        res.is_none(),
        "blpop on non-existent key after deadline must be Ok(None); got Ok(Some({res:?}))"
    );
    // Sanity: the wait should be close to the deadline. If it returned
    // in <500ms the response_timeout default is leaking through (we set
    // it to None in pool.rs — this test would catch a regression).
    assert!(
        elapsed >= Duration::from_millis(900),
        "blpop returned in {elapsed:?} — suspiciously fast, did response_timeout leak?"
    );
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn blpop_returns_ok_some_when_lpush_lands() {
    // Two SEPARATE pools — Redis serialises commands per connection on
    // the server side, so a BLPOP on one connection blocks the
    // connection's slot. An LPUSH issued on the same multiplexed
    // connection would queue behind the BLPOP and never land. Production
    // is fine because the RAG queue producer and the RagWorker run in
    // different processes (different connections).
    let waiter_pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new (waiter)");
    let pusher_pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new (pusher)");

    let key = format!(
        "the-one-redis-test:blpop-late-push:{}",
        uuid::Uuid::new_v4()
    );
    let key_for_push = key.clone();

    let pusher = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        pusher_pool
            .lists()
            .lpush(&key_for_push, "hello")
            .await
            .expect("lpush");
    });

    let res = waiter_pool
        .lists()
        .blpop(&key, Duration::from_secs(5))
        .await
        .expect("blpop must succeed when LPUSH lands within the deadline");

    let _ = pusher.await;
    let (got_key, got_value) = res.expect("blpop must return Some when value arrives");
    assert_eq!(got_key, key);
    assert_eq!(got_value, "hello");
}

#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn blpop_returns_ok_some_immediately_on_populated_key() {
    let pool = RedisPool::new(PoolConfig::from_url(redis_url()))
        .await
        .expect("RedisPool::new");

    let key = format!("the-one-redis-test:blpop-pop:{}", uuid::Uuid::new_v4());
    pool.lists().lpush(&key, "value").await.expect("lpush");

    let started = Instant::now();
    let res = pool
        .lists()
        .blpop(&key, Duration::from_secs(1))
        .await
        .expect("blpop on populated key must succeed");
    let elapsed = started.elapsed();

    let (got_key, got_value) = res.expect("blpop must return Some immediately");
    assert_eq!(got_key, key);
    assert_eq!(got_value, "value");
    // Should return well under 100ms — there's no blocking to do.
    assert!(
        elapsed < Duration::from_millis(100),
        "blpop on populated key took {elapsed:?} — should be near-instant"
    );
}
