//! Regression tests for sentinel-migration call sites.
//!
//! Locks the contracts of three methods added during the sentinel
//! migration (`keys::set_ex_nx`, `sorted_sets::zremrangebyrank`,
//! `streams::xread`) so future redis-rs upgrades don't silently change
//! their behaviour.
//!
//! Run with a live Redis 8 instance:
//! ```sh
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p the-one-redis sentinel_ops -- --include-ignored
//! ```

use std::time::Duration;
use the_one_redis::{PoolConfig, RedisPool};

/// Helper: build a pool from `REDIS_URL` env var or skip the test.
async fn pool() -> Option<RedisPool> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    RedisPool::new(PoolConfig::from_url(url)).await.ok()
}

/// `set_ex_nx` returns `true` on first write and `false` on second
/// (lock already held) without touching the stored value.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn set_ex_nx_returns_true_first_false_second() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:lock:{}", uuid::Uuid::new_v4());

    // First SET NX — should succeed.
    let first = pool
        .keys()
        .set_ex_nx(&key, "locked", Duration::from_secs(10))
        .await
        .expect("set_ex_nx must not error");
    assert!(first, "first set_ex_nx must return true (lock acquired)");

    // Second SET NX — key already exists.
    let second = pool
        .keys()
        .set_ex_nx(&key, "locked", Duration::from_secs(10))
        .await
        .expect("set_ex_nx must not error");
    assert!(!second, "second set_ex_nx must return false (already held)");

    // Cleanup.
    let _ = pool.keys().del(&key).await;
}

/// `set_ex_nx` sets a TTL — the key must expire and a subsequent
/// acquisition must succeed.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn set_ex_nx_respects_ttl() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:lock_ttl:{}", uuid::Uuid::new_v4());

    // Set with 1 second TTL.
    let acquired = pool
        .keys()
        .set_ex_nx(&key, "locked", Duration::from_secs(1))
        .await
        .expect("set_ex_nx must not error");
    assert!(acquired);

    // Wait for expiry.
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Should be acquirable again.
    let re_acquired = pool
        .keys()
        .set_ex_nx(&key, "locked", Duration::from_secs(10))
        .await
        .expect("set_ex_nx must not error");
    assert!(
        re_acquired,
        "must be able to re-acquire lock after TTL expiry"
    );

    let _ = pool.keys().del(&key).await;
}

/// `zremrangebyrank` removes entries at the expected rank range and
/// returns the count removed.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn zremrangebyrank_trims_to_latest_n() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:health_hist:{}", uuid::Uuid::new_v4());

    // Add 5 members with distinct scores.
    for i in 0_u64..5 {
        pool.sorted_sets()
            .zadd(&key, i as f64, format!("entry:{i}"))
            .await
            .expect("zadd must not error");
    }

    // Trim to keep latest 3 (ranks 2..=4 — scores 2,3,4).
    // Remove rank 0..=-3 = rank 0 and 1.
    let removed = pool
        .sorted_sets()
        .zremrangebyrank(&key, 0, -4)
        .await
        .expect("zremrangebyrank must not error");
    assert_eq!(removed, 2, "must have removed exactly 2 entries");

    let remaining: Vec<String> = pool
        .sorted_sets()
        .zrange(&key, 0, -1)
        .await
        .expect("zrange must not error");
    assert_eq!(
        remaining.len(),
        3,
        "must have exactly 3 entries remaining after trim"
    );

    let _ = pool.keys().del(&key).await;
}

/// `sets::sadd` + `sismember` + `smembers` round-trip. Locks the basic
/// set contract workspace session modules require for admin /
/// participant membership tracking.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn sets_sadd_and_membership_checks() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:set:{}", uuid::Uuid::new_v4());

    let added = pool
        .sets()
        .sadd(&key, "alice")
        .await
        .expect("sadd must not error");
    assert_eq!(added, 1, "first sadd returns 1 (new member)");

    let readded = pool
        .sets()
        .sadd(&key, "alice")
        .await
        .expect("sadd must not error");
    assert_eq!(readded, 0, "second sadd of same member returns 0");

    pool.sets()
        .sadd(&key, "bob")
        .await
        .expect("sadd must not error");

    let is_member = pool
        .sets()
        .sismember(&key, "alice")
        .await
        .expect("sismember must not error");
    assert!(is_member, "alice must be a member after sadd");

    let not_member = pool
        .sets()
        .sismember(&key, "charlie")
        .await
        .expect("sismember must not error");
    assert!(!not_member, "charlie must not be a member");

    let card = pool.sets().scard(&key).await.expect("scard must not error");
    assert_eq!(card, 2, "scard must reflect the 2 unique members");

    let mut members: Vec<String> = pool
        .sets()
        .smembers(&key)
        .await
        .expect("smembers must not error");
    members.sort();
    assert_eq!(members, vec!["alice".to_string(), "bob".to_string()]);

    let removed = pool
        .sets()
        .srem(&key, "alice")
        .await
        .expect("srem must not error");
    assert_eq!(removed, 1, "srem returns 1 when the member was present");

    let _ = pool.keys().del(&key).await;
}

/// `zrevrange` returns members in descending-score order — the contract
/// the scoring module's `get_recent_scores` relies on to return newest-first.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn zrevrange_returns_members_newest_first() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:zrev:{}", uuid::Uuid::new_v4());

    // Add 3 members with ascending scores 100, 200, 300 (simulating
    // timestamps — lowest = oldest).
    pool.sorted_sets()
        .zadd(&key, 100.0, "oldest")
        .await
        .expect("zadd must not error");
    pool.sorted_sets()
        .zadd(&key, 200.0, "middle")
        .await
        .expect("zadd must not error");
    pool.sorted_sets()
        .zadd(&key, 300.0, "newest")
        .await
        .expect("zadd must not error");

    // `ZREVRANGE 0 -1` must return newest first: 300, 200, 100.
    let members: Vec<String> = pool
        .sorted_sets()
        .zrevrange(&key, 0, -1)
        .await
        .expect("zrevrange must not error");
    assert_eq!(
        members,
        vec![
            "newest".to_string(),
            "middle".to_string(),
            "oldest".to_string(),
        ],
        "zrevrange must return members in descending-score order"
    );

    // Limit to top 2 — must be the 2 newest.
    let top2: Vec<String> = pool
        .sorted_sets()
        .zrevrange(&key, 0, 1)
        .await
        .expect("zrevrange must not error");
    assert_eq!(
        top2,
        vec!["newest".to_string(), "middle".to_string()],
        "zrevrange 0..1 must return the 2 highest-score members"
    );

    let _ = pool.keys().del(&key).await;
}

/// `streams::xread` returns an empty vec on timeout (no error) and
/// returns entries when they exist.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn xread_returns_ok_none_on_timeout() {
    let pool = pool().await.expect("Redis must be reachable");
    let stream = format!("the-one-redis:test:events:{}", uuid::Uuid::new_v4());

    // Stream doesn't exist — should return empty vec, not an error.
    let entries = pool
        .streams()
        .xread(&stream, "0-0", 10, Duration::from_millis(100))
        .await
        .expect("xread must not error on empty/missing stream");
    assert!(
        entries.is_empty(),
        "xread on missing stream must return empty vec (not error)"
    );
}

/// `timeseries::create` is idempotent: a second call on an existing
/// series silently succeeds rather than propagating the `key already
/// exists` error. Locks the contract workspace migrations rely on
/// for bootstrap.
#[tokio::test]
#[ignore = "requires live Redis with TimeSeries module"]
async fn timeseries_create_is_idempotent() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:ts:create:{}", uuid::Uuid::new_v4());
    let opts = the_one_redis::timeseries::CreateOptions::default();

    pool.timeseries()
        .create(&key, &opts)
        .await
        .expect("first ts.create must succeed");

    // Second call on an existing series must succeed silently.
    pool.timeseries()
        .create(&key, &opts)
        .await
        .expect("second ts.create must not error (idempotent bootstrap)");

    let _ = pool.keys().del(&key).await;
}

/// `timeseries::add` → `get` round-trips a value with a server-clock
/// timestamp. Locks the (timestamp, value) tuple decoding.
#[tokio::test]
#[ignore = "requires live Redis with TimeSeries module"]
async fn timeseries_add_and_get_roundtrips() {
    use std::time::Duration;

    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:ts:latency:{}", uuid::Uuid::new_v4());

    pool.timeseries()
        .create(&key, &the_one_redis::timeseries::CreateOptions::default())
        .await
        .expect("ts.create must succeed");

    pool.timeseries()
        .add(&key, 42.5, Duration::from_secs(3600))
        .await
        .expect("ts.add must succeed");

    let latest = pool
        .timeseries()
        .get(&key)
        .await
        .expect("ts.get must not error");
    let (ts, val) = latest.expect("latest sample must be Some");
    assert!(ts > 0, "timestamp must be positive (server clock)");
    assert!(
        (val - 42.5).abs() < 1e-9,
        "value must round-trip exactly, got {val}"
    );

    let _ = pool.keys().del(&key).await;
}

/// `timeseries::get` on a non-existent series returns `Ok(None)` rather
/// than bubbling the `no such key` error — this is the "missing series =
/// no data" contract domain TimeSeries wrappers relied on.
#[tokio::test]
#[ignore = "requires live Redis with TimeSeries module"]
async fn timeseries_get_returns_none_for_missing_key() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:ts:missing:{}", uuid::Uuid::new_v4());

    let got = pool
        .timeseries()
        .get(&key)
        .await
        .expect("ts.get must not error on missing key");
    assert!(
        got.is_none(),
        "missing series must return None, got: {got:?}"
    );
}

/// `streams::xread` returns entries already in the stream when reading
/// from `"0-0"`.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn xread_returns_existing_entries() {
    let pool = pool().await.expect("Redis must be reachable");
    let stream = format!("the-one-redis:test:events:{}", uuid::Uuid::new_v4());

    // Pre-populate the stream.
    pool.streams()
        .xadd(
            &stream,
            &[("event_type", "selfqa_result"), ("grade", "A")],
            None,
        )
        .await
        .expect("xadd must not error");

    // Reading from beginning should return that entry immediately.
    let entries = pool
        .streams()
        .xread(&stream, "0-0", 10, Duration::from_millis(500))
        .await
        .expect("xread must not error");

    assert_eq!(entries.len(), 1, "must return 1 entry from stream");
    assert_eq!(
        entries[0].fields.get("grade").map(|s| s.as_str()),
        Some("A"),
        "entry grade field must round-trip through xread"
    );

    let _ = pool.keys().del(&stream).await;
}

/// `pubsub().subscribe_patterns(&["the-one-redis:test:ps:*"])` receives messages
/// published on matching channels, with the `PubSubMessage.channel` set
/// to the actual published channel (not the pattern).
///
/// Added during external session storage — the broadcast listener pattern
/// subscribes to `mai:broadcast:*` and relies on the delivered channel
/// name to route messages per user.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn subscribe_patterns_receives_matching_messages() {
    let pool = pool().await.expect("Redis must be reachable");
    let marker = uuid::Uuid::new_v4();
    let pattern = format!("the-one-redis:test:ps:{marker}:*");
    let channel = format!("the-one-redis:test:ps:{marker}:42");

    let mut sub = pool
        .pubsub()
        .subscribe_patterns(&[pattern.as_str()])
        .await
        .expect("subscribe_patterns must not error");

    // Publish after subscribe is established. The SUBSCRIBE confirmation
    // has already round-tripped by the time subscribe_patterns returns, so
    // no sleep is needed here.
    let publisher = pool.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = publisher.pubsub().publish(&channel, "hello").await;
    });

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("subscribe_patterns must receive message within 2s")
        .expect("subscribe_patterns stream must not close");

    assert!(
        msg.channel.ends_with(":42"),
        "channel must be the actual published channel, got {}",
        msg.channel
    );
    assert_eq!(msg.payload, "hello", "payload must round-trip");
}

/// `zincrby` creates the member on first call and increments the score
/// on the second. Used by tool-promotion code paths.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn zincrby_creates_then_increments() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:zincrby:{}", uuid::Uuid::new_v4());

    let first = pool
        .sorted_sets()
        .zincrby(&key, 3.5, "alice")
        .await
        .expect("zincrby first call must succeed");
    assert!(
        (first - 3.5).abs() < f64::EPSILON,
        "first zincrby must return the new score 3.5, got {first}"
    );

    let second = pool
        .sorted_sets()
        .zincrby(&key, 1.5, "alice")
        .await
        .expect("zincrby second call must succeed");
    assert!(
        (second - 5.0).abs() < f64::EPSILON,
        "second zincrby must return cumulative score 5.0, got {second}"
    );

    let _ = pool.keys().del(&key).await;
}

/// `zrangebyscore` returns members in the `[min, max]` score range and
/// respects the `LIMIT offset count` page.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn zrangebyscore_returns_inclusive_range_with_limit() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:zrbs:{}", uuid::Uuid::new_v4());

    // Seed 5 members with scores 1..=5.
    for (m, s) in [("a", 1.0), ("b", 2.0), ("c", 3.0), ("d", 4.0), ("e", 5.0)] {
        let _ = pool.sorted_sets().zadd(&key, s, m).await.unwrap();
    }

    // Without LIMIT — full range [2, 4] should return [b, c, d].
    let all: Vec<String> = pool
        .sorted_sets()
        .zrangebyscore(&key, 2.0, 4.0, false, None)
        .await
        .expect("zrangebyscore must not error");
    assert_eq!(
        all,
        vec!["b", "c", "d"],
        "inclusive range must be 3 elements"
    );

    // With LIMIT 1 2 — skip first, take two: [c, d].
    let paged: Vec<String> = pool
        .sorted_sets()
        .zrangebyscore(&key, 2.0, 4.0, false, Some((1, 2)))
        .await
        .expect("zrangebyscore with limit must not error");
    assert_eq!(
        paged,
        vec!["c", "d"],
        "LIMIT 1 2 must skip first and take 2"
    );

    let _ = pool.keys().del(&key).await;
}

/// `hincrby` creates the field on first call and increments on the
/// second. Returns the new value each time.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn hincrby_creates_then_increments() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:hincrby:{}", uuid::Uuid::new_v4());

    let first = pool
        .hashes()
        .hincrby(&key, "requests", 10)
        .await
        .expect("hincrby first call");
    assert_eq!(
        first, 10,
        "first hincrby must return the delta as new value"
    );

    let second = pool
        .hashes()
        .hincrby(&key, "requests", -3)
        .await
        .expect("hincrby negative call");
    assert_eq!(second, 7, "hincrby must support negative deltas");

    let _ = pool.keys().del(&key).await;
}

/// `ltrim` keeps only the specified range of a list.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn ltrim_caps_list_to_latest_n() {
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:ltrim:{}", uuid::Uuid::new_v4());

    // Head-pushed list ends up in order 5, 4, 3, 2, 1.
    for v in ["1", "2", "3", "4", "5"] {
        let _ = pool.lists().lpush(&key, v).await.unwrap();
    }
    assert_eq!(pool.lists().llen(&key).await.unwrap(), 5);

    // Keep only the latest 3 (indices 0..=2).
    pool.lists()
        .ltrim(&key, 0, 2)
        .await
        .expect("ltrim must not error");

    let remaining: Vec<String> = pool.lists().lrange(&key, 0, -1).await.unwrap();
    assert_eq!(
        remaining,
        vec!["5", "4", "3"],
        "ltrim must retain head range"
    );

    let _ = pool.keys().del(&key).await;
}

/// `mget` returns values in the order of the input keys with `None` for
/// missing keys, preserving the slot layout.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn mget_preserves_order_and_missing_slots() {
    let pool = pool().await.expect("Redis must be reachable");
    let prefix = format!("the-one-redis:test:mget:{}", uuid::Uuid::new_v4());
    let k1 = format!("{prefix}:a");
    let k2 = format!("{prefix}:b");
    let k3 = format!("{prefix}:c");

    pool.keys().set(&k1, "alpha").await.unwrap();
    // k2 intentionally not set.
    pool.keys().set(&k3, "gamma").await.unwrap();

    let got: Vec<Option<String>> = pool
        .keys()
        .mget(&[k1.as_str(), k2.as_str(), k3.as_str()])
        .await
        .expect("mget must not error");
    assert_eq!(
        got,
        vec![Some("alpha".into()), None, Some("gamma".into())],
        "mget must preserve order and use None for missing keys"
    );

    for k in [&k1, &k3] {
        let _ = pool.keys().del(k).await;
    }
}

/// `rename` atomically renames a key, including its value.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn rename_moves_value_to_new_key() {
    let pool = pool().await.expect("Redis must be reachable");
    let src = format!("the-one-redis:test:rename:src:{}", uuid::Uuid::new_v4());
    let dst = format!("the-one-redis:test:rename:dst:{}", uuid::Uuid::new_v4());

    pool.keys().set(&src, "payload").await.unwrap();
    pool.keys()
        .rename(&src, &dst)
        .await
        .expect("rename must succeed when source exists");

    let from_src: Option<String> = pool.keys().get(&src).await.unwrap();
    let from_dst: Option<String> = pool.keys().get(&dst).await.unwrap();
    assert_eq!(
        from_src, None,
        "source key must no longer exist after rename"
    );
    assert_eq!(
        from_dst.as_deref(),
        Some("payload"),
        "destination must hold the original value"
    );

    let _ = pool.keys().del(&dst).await;
}

/// `set_with_exat` with `IfMissing` / `IfExisting` predicates implements
/// the SET-NX and SET-XX semantics the session store relies on. Locks
/// the round-trip contract against future redis-rs regressions.
#[tokio::test]
#[ignore = "requires live Redis"]
async fn set_with_exat_respects_nx_xx_predicates() {
    use the_one_redis::keys::SetPredicate;
    let pool = pool().await.expect("Redis must be reachable");
    let key = format!("the-one-redis:test:setexat:{}", uuid::Uuid::new_v4());
    // Expire ~10 minutes in the future.
    let exat = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("post-epoch")
        .as_secs()
        + 600) as i64;

    // First NX — key does not exist → should set.
    let first = pool
        .keys()
        .set_with_exat(&key, "alpha", exat, SetPredicate::IfMissing)
        .await
        .expect("set_with_exat NX must not error");
    assert!(first, "first NX must succeed");

    // Second NX — key now exists → must NOT set.
    let second = pool
        .keys()
        .set_with_exat(&key, "beta", exat, SetPredicate::IfMissing)
        .await
        .expect("set_with_exat NX must not error");
    assert!(!second, "second NX must be rejected");

    // Value untouched — still "alpha".
    let got: Option<String> = pool.keys().get(&key).await.expect("get");
    assert_eq!(got.as_deref(), Some("alpha"), "NX must not overwrite");

    // XX — key exists, should succeed and overwrite.
    let third = pool
        .keys()
        .set_with_exat(&key, "gamma", exat, SetPredicate::IfExisting)
        .await
        .expect("set_with_exat XX must not error");
    assert!(third, "XX must succeed when key exists");
    let got_after: Option<String> = pool.keys().get(&key).await.expect("get");
    assert_eq!(got_after.as_deref(), Some("gamma"), "XX must overwrite");

    // XX on a missing key — must NOT set.
    let missing = format!("the-one-redis:test:setexat:{}", uuid::Uuid::new_v4());
    let fourth = pool
        .keys()
        .set_with_exat(&missing, "delta", exat, SetPredicate::IfExisting)
        .await
        .expect("set_with_exat XX must not error");
    assert!(!fourth, "XX must be rejected when key is missing");
    let got_missing: Option<String> = pool.keys().get(&missing).await.expect("get");
    assert!(got_missing.is_none(), "XX must not create the key");

    let _ = pool.keys().del(&key).await;
}
