//! Regression test for the v0.1.363 head-of-line-blocking fix.
//!
//! ## What this locks
//!
//! Before v0.1.363, blocking commands (BLPOP, XREADGROUP-with-BLOCK,
//! XREAD-with-BLOCK) used the shared `MultiplexedConnection` returned by
//! [`RedisPool::conn`]. Redis processes commands per-connection serially,
//! so any HGET / HSET / FT.SEARCH / etc. issued on the same pool while
//! the blocking call was in flight would queue behind it for up to the
//! BLOCK timeout — typically 5 seconds in the-one-mcp's deployment (SelfQA
//! control listener cycles `XREADGROUP BLOCK 5000ms` continuously, plus
//! per-tenant RAG queue workers do the same).
//!
//! The fix in `lists::blpop` / `streams::xreadgroup` / `streams::xread`
//! is to acquire a **dedicated** connection via
//! [`RedisPool::dedicated_conn`] for blocking calls. This test asserts
//! the contract end-to-end: a long-running blocking call on one task
//! must NOT delay a concurrent non-blocking call on the SAME pool.
//!
//! ## Investigation
//!
//! `docs/plans/2026-05-16-fred-removal-and-bug-fixes.md`
//!
//! ## Running
//!
//! ```sh
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p the-one-redis -- --include-ignored
//! ```

use std::time::{Duration, Instant};

use the_one_redis::{PoolConfig, RedisPool};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

/// **THE** regression test: an in-flight BLPOP on one task must not
/// head-of-line-block an HGET/HLEN on another task using the same pool.
///
/// Before v0.1.363, the HLEN call would queue behind the BLPOP's BLOCK
/// window — observed as ~5 s tail latency on every Redis call in the
/// process whenever a blocking command was active. After the fix, BLPOP
/// runs on a dedicated connection and the HLEN returns within tens of
/// milliseconds regardless of the blocking call's BLOCK timeout.
#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn blpop_in_flight_does_not_delay_concurrent_hlen() {
    let pool = std::sync::Arc::new(
        RedisPool::new(PoolConfig::from_url(redis_url()))
            .await
            .expect("RedisPool::new"),
    );

    // Set up a hash with a known size so HLEN has something to count.
    let hash_key = format!(
        "the-one-redis-test:dedicated-conn:hash:{}",
        uuid::Uuid::new_v4()
    );
    pool.hashes()
        .hset(&hash_key, "field-a", "1")
        .await
        .expect("hset to seed test hash");

    // Pick a list key that no one else is writing to — BLPOP will sit
    // on it for the full 5s window.
    let blpop_key = format!(
        "the-one-redis-test:dedicated-conn:blpop:{}",
        uuid::Uuid::new_v4()
    );

    // Spawn BLPOP with a 5-second BLOCK on one task. Pre-v0.1.363 this
    // would occupy the shared multiplexed connection for the full 5 s.
    let blpop_pool = std::sync::Arc::clone(&pool);
    let blpop_started = Instant::now();
    let blpop_handle = tokio::spawn(async move {
        let res = blpop_pool
            .lists()
            .blpop(&blpop_key, Duration::from_secs(5))
            .await
            .expect("blpop must return Ok on deadline");
        (res, blpop_started.elapsed())
    });

    // Give the BLPOP task a moment to actually issue the command on the
    // wire. Without this, the HLEN below could race ahead of the BLPOP
    // and trivially pass even on broken code.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now issue HLEN on the same pool. This is the contract: must
    // return within tens of milliseconds, NOT the BLPOP timeout.
    let hlen_started = Instant::now();
    let hlen_result = pool
        .hashes()
        .hlen(&hash_key)
        .await
        .expect("hlen must succeed concurrently with in-flight blpop");
    let hlen_elapsed = hlen_started.elapsed();

    // Cleanup
    let _ = pool.keys().del(&hash_key).await;

    assert_eq!(
        hlen_result, 1,
        "hlen returned wrong count for the seeded hash"
    );

    // The fix's actual assertion: HLEN must complete fast even with
    // BLPOP in flight on the same pool. Pre-v0.1.363 this would take
    // ~5 s. After the fix it should be < 100 ms (round-trip latency
    // to localhost Redis is typically sub-ms; we leave headroom for
    // CI variance and the connection-acquisition overhead in the
    // concurrent case).
    assert!(
        hlen_elapsed < Duration::from_millis(500),
        "hlen took {hlen_elapsed:?} while a BLPOP was in flight — \
         this indicates blocking commands are still using the shared \
         multiplexed connection (v0.1.363 regression). Expected: < 500 ms."
    );

    // Wait for BLPOP to finish naturally and assert it did wait the
    // full timeout (sanity check: confirms BLPOP actually issued and
    // wasn't somehow short-circuited).
    let (blpop_result, blpop_elapsed) = blpop_handle.await.expect("blpop task panicked");
    assert!(
        blpop_result.is_none(),
        "blpop on unused key must return Ok(None) on timeout"
    );
    assert!(
        blpop_elapsed >= Duration::from_secs(4),
        "blpop returned in {blpop_elapsed:?} — should have waited \
         the full 5 s timeout"
    );
}

/// Same shape as the BLPOP test above but for `XREADGROUP-with-BLOCK`.
/// SelfQA's control listener cycles this continuously in production,
/// so it's the most-active blocking caller in the system. If this
/// regresses, every Redis call in the-one-mcp gets 5 s tail latency again.
#[tokio::test]
#[ignore = "requires live Redis; run with --include-ignored"]
async fn xreadgroup_in_flight_does_not_delay_concurrent_hlen() {
    let pool = std::sync::Arc::new(
        RedisPool::new(PoolConfig::from_url(redis_url()))
            .await
            .expect("RedisPool::new"),
    );

    let hash_key = format!(
        "the-one-redis-test:xreadgroup-vs-hlen:hash:{}",
        uuid::Uuid::new_v4()
    );
    pool.hashes()
        .hset(&hash_key, "f", "v")
        .await
        .expect("hset to seed hash");

    let stream_key = format!(
        "the-one-redis-test:xreadgroup-vs-hlen:stream:{}",
        uuid::Uuid::new_v4()
    );
    let group = "test-group";
    let consumer = "test-consumer";

    // Create the stream + consumer group. XREADGROUP errors otherwise.
    // We use `xadd` to materialize the stream, then create the group
    // starting at "$" so we don't immediately consume that seed entry.
    pool.streams()
        .xadd(&stream_key, &[("seed", "1")], None)
        .await
        .expect("xadd to materialize stream");
    pool.streams()
        .xgroup_create(&stream_key, group, "$")
        .await
        .expect("xgroup_create");

    // Spawn XREADGROUP with 5s BLOCK on one task.
    let xread_pool = std::sync::Arc::clone(&pool);
    let xread_stream = stream_key.clone();
    let xread_started = Instant::now();
    let xread_handle = tokio::spawn(async move {
        let res = xread_pool
            .streams()
            .xreadgroup(group, consumer, &xread_stream, 1, Duration::from_secs(5))
            .await
            .expect("xreadgroup must return Ok on idle timeout");
        (res, xread_started.elapsed())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let hlen_started = Instant::now();
    let hlen_result = pool
        .hashes()
        .hlen(&hash_key)
        .await
        .expect("hlen must succeed concurrently with in-flight xreadgroup");
    let hlen_elapsed = hlen_started.elapsed();

    assert_eq!(hlen_result, 1);
    assert!(
        hlen_elapsed < Duration::from_millis(500),
        "hlen took {hlen_elapsed:?} while xreadgroup was in flight — \
         v0.1.363 regression: blocking xreadgroup is using the shared \
         multiplexed connection again. Expected: < 500 ms."
    );

    // Wait for xreadgroup to finish BEFORE cleaning up the stream key
    // (deleting the key mid-xreadgroup would error the task out with
    // NOGROUP, which is a test-isolation bug, not a production bug).
    let (xread_entries, xread_elapsed) = xread_handle.await.expect("xreadgroup task panicked");

    // Cleanup AFTER xreadgroup completes.
    let _ = pool.keys().del(&hash_key).await;
    let _ = pool.keys().del(&stream_key).await;

    assert!(
        xread_entries.is_empty(),
        "xreadgroup on idle stream after $ must return empty Vec"
    );
    assert!(
        xread_elapsed >= Duration::from_secs(4),
        "xreadgroup returned in {xread_elapsed:?} — should have waited the full 5s"
    );
}
