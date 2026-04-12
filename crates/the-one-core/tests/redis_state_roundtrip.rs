//! Live integration tests for `RedisStateStore` (v0.16.0 Phase 5).
//!
//! Gated on `redis-state` feature + `THE_ONE_STATE_TYPE=redis` +
//! `THE_ONE_STATE_URL`. Skip gracefully via `return` when env vars
//! aren't set.
//!
//! ```bash
//! docker run --rm -d --name the-one-redis-state \
//!     -p 16379:6379 redis/redis-stack
//! THE_ONE_STATE_TYPE=redis \
//! THE_ONE_STATE_URL=redis://localhost:16379 \
//! cargo test -p the-one-core --features redis-state \
//!     --test redis_state_roundtrip -- --test-threads=1
//! ```

#![cfg(feature = "redis-state")]

use the_one_core::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationNodeKind,
    MemoryNavigationTunnel,
};
use the_one_core::state_store::StateStore;
use the_one_core::storage::redis::{RedisStateConfig, RedisStateStore};

fn matching_env() -> Option<String> {
    if std::env::var("THE_ONE_STATE_TYPE").ok().as_deref() != Some("redis") {
        return None;
    }
    let url = std::env::var("THE_ONE_STATE_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

fn test_config() -> RedisStateConfig {
    RedisStateConfig {
        require_aof: false,
        ..RedisStateConfig::default()
    }
}

async fn fresh_store(project_id: &str) -> Option<RedisStateStore> {
    let url = matching_env()?;
    // Flush the test prefix keys (best-effort cleanup).
    let config = test_config();
    let store = RedisStateStore::new(&config, &url, project_id)
        .await
        .expect("RedisStateStore::new");
    Some(store)
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_metadata() {
    let Some(store) = fresh_store("meta").await else {
        return;
    };
    assert_eq!(store.project_id(), "meta");
    assert_eq!(store.capabilities().name, "redis");
    assert!(store.capabilities().fts);
    assert!(!store.capabilities().transactions);
    assert_eq!(store.schema_version().unwrap(), 1);
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_profile_roundtrip() {
    let Some(store) = fresh_store("profile").await else {
        return;
    };
    assert_eq!(store.latest_project_profile().unwrap(), None);
    store
        .upsert_project_profile(r#"{"languages":["rust"]}"#)
        .unwrap();
    let p = store.latest_project_profile().unwrap().unwrap();
    assert!(p.contains("\"rust\""));
    store
        .upsert_project_profile(r#"{"languages":["go"]}"#)
        .unwrap();
    let p2 = store.latest_project_profile().unwrap().unwrap();
    assert!(p2.contains("\"go\""));
    assert!(!p2.contains("\"rust\""));
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_approval_roundtrip() {
    let Some(store) = fresh_store("approval").await else {
        return;
    };
    assert!(!store
        .is_approved("test_action", ApprovalScope::Session)
        .unwrap());
    store
        .set_approval("test_action", ApprovalScope::Session, true)
        .unwrap();
    assert!(store
        .is_approved("test_action", ApprovalScope::Session)
        .unwrap());
    store
        .set_approval("test_action", ApprovalScope::Session, false)
        .unwrap();
    assert!(!store
        .is_approved("test_action", ApprovalScope::Session)
        .unwrap());
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_audit_roundtrip() {
    let Some(store) = fresh_store("audit").await else {
        return;
    };
    store
        .record_audit_event("test.event", r#"{"key":"value"}"#)
        .unwrap();
    let count = store.audit_event_count_for_project().unwrap();
    assert!(count >= 1);
    let events = store.list_audit_events(10).unwrap();
    assert!(!events.is_empty());
    assert_eq!(events[0].event_type, "test.event");
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_diary_roundtrip() {
    let Some(store) = fresh_store("diary").await else {
        return;
    };
    let entry = DiaryEntry {
        entry_id: "d1".to_string(),
        project_id: "diary".to_string(),
        entry_date: "2026-04-12".to_string(),
        mood: Some("productive".to_string()),
        tags: vec!["test".to_string()],
        content: "Phase 5 Redis state store".to_string(),
        created_at_epoch_ms: 1712880000000,
        updated_at_epoch_ms: 1712880000000,
    };
    store.upsert_diary_entry(&entry).unwrap();
    let list = store.list_diary_entries(None, None, 10).unwrap();
    assert!(list.iter().any(|e| e.entry_id == "d1"));
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_aaak_roundtrip() {
    let Some(store) = fresh_store("aaak").await else {
        return;
    };
    let lesson = AaakLesson {
        lesson_id: "l1".to_string(),
        project_id: "aaak".to_string(),
        pattern_key: "test_pattern".to_string(),
        role: "assistant".to_string(),
        canonical_text: "always test your code".to_string(),
        occurrence_count: 3,
        confidence_percent: 85,
        source_transcript_path: None,
        updated_at_epoch_ms: 1712880000000,
    };
    store.upsert_aaak_lesson(&lesson).unwrap();
    let lessons = store.list_aaak_lessons("aaak", 10).unwrap();
    assert!(lessons.iter().any(|l| l.lesson_id == "l1"));
    assert!(store.delete_aaak_lesson("l1").unwrap());
    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_state_navigation_roundtrip() {
    let Some(store) = fresh_store("nav").await else {
        return;
    };
    let node = MemoryNavigationNode {
        node_id: "n1".to_string(),
        project_id: "nav".to_string(),
        kind: MemoryNavigationNodeKind::Room,
        label: "Test Room".to_string(),
        parent_node_id: None,
        wing: Some("west".to_string()),
        hall: None,
        room: Some("test".to_string()),
        updated_at_epoch_ms: 1712880000000,
    };
    store.upsert_navigation_node(&node).unwrap();
    let fetched = store.get_navigation_node("n1").unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().label, "Test Room");
    store.close().await;
}
