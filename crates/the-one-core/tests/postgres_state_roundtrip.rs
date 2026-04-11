//! Live-database integration tests for `PostgresStateStore` (v0.16.0 Phase 3).
//!
//! Parallel to `crates/the-one-memory/tests/pgvector_roundtrip.rs`.
//! Same gating strategy: the whole suite reads the production env
//! surface `THE_ONE_STATE_TYPE=postgres` + `THE_ONE_STATE_URL`, skips
//! gracefully via `return` when either is absent, and does NOT use
//! `_TEST`-suffixed shadow vars.
//!
//! ## Running locally
//!
//! ```bash
//! docker run --rm -d --name the-one-pg-state \
//!     -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=the_one_state_test \
//!     -p 55433:5432 postgres:16
//! THE_ONE_STATE_TYPE=postgres \
//! THE_ONE_STATE_URL=postgres://postgres:pw@localhost:55433/the_one_state_test \
//! cargo test -p the-one-core --features pg-state \
//!     --test postgres_state_roundtrip -- --test-threads=1
//! ```
//!
//! `--test-threads=1` is required because every test drops and
//! re-creates the `the_one` schema at the start of the run — two
//! parallel tests would race on the schema.
//!
//! ## Why a plain `postgres:16` image (not `ankane/pgvector`)
//!
//! PostgresStateStore does NOT use the `vector` extension. It only
//! touches `TSVECTOR` + GIN (native Postgres). A vanilla Postgres
//! image works fine. If you run the Phase 2 pgvector tests against
//! the same instance, use `ankane/pgvector:16` which includes both.

#![cfg(feature = "pg-state")]

use the_one_core::audit::{AuditOutcome, AuditRecord};
use the_one_core::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationNodeKind,
    MemoryNavigationTunnel,
};
use the_one_core::pagination::PageRequest;
use the_one_core::state_store::StateStore;
use the_one_core::storage::postgres::{PostgresStateConfig, PostgresStateStore};
use the_one_core::storage::sqlite::{page_limits, ConversationSourceRecord};

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Returns the state-store connection URL iff the production env
/// vars are set correctly for this test run. Tests skip gracefully
/// via `return` when the env vars don't match.
fn matching_env() -> Option<String> {
    if std::env::var("THE_ONE_STATE_TYPE").ok().as_deref() != Some("postgres") {
        return None;
    }
    let url = std::env::var("THE_ONE_STATE_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

/// Drop + recreate the `the_one` schema so each test starts clean.
async fn reset_schema(url: &str) -> Result<(), String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .map_err(|e| format!("reset connect: {e}"))?;
    sqlx::query("DROP SCHEMA IF EXISTS the_one CASCADE")
        .execute(&pool)
        .await
        .map_err(|e| format!("drop schema: {e}"))?;
    pool.close().await;
    Ok(())
}

/// Tiny pool config — avoids exhausting max_connections on dev Postgres.
fn test_config() -> PostgresStateConfig {
    PostgresStateConfig {
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_ms: 5_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_timeout_ms: 10_000,
        ..PostgresStateConfig::default()
    }
}

/// Construct the store for a freshly-reset database. Helper that
/// collapses the boilerplate at the start of every test.
async fn fresh_store(project_id: &str) -> Option<PostgresStateStore> {
    let url = matching_env()?;
    reset_schema(&url).await.expect("reset_schema");
    Some(
        PostgresStateStore::new(&test_config(), &url, project_id)
            .await
            .expect("PostgresStateStore::new"),
    )
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_metadata_and_bootstrap() {
    let Some(store) = fresh_store("meta").await else {
        return;
    };
    assert_eq!(store.project_id(), "meta");
    assert_eq!(store.capabilities().name, "postgres");
    assert!(store.capabilities().fts);
    assert!(store.capabilities().transactions);
    assert!(store.capabilities().durable);
    assert!(store.capabilities().schema_versioned);

    // Schema version reflects the number of applied migrations
    // (which is 2 after the Phase 3 bootstrap — migration 0 + 1).
    let version = store.schema_version().expect("schema_version");
    assert_eq!(version, 1, "latest Phase 3 migration version is 1");

    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_migration_runner_is_idempotent() {
    let Some(url) = matching_env() else { return };
    reset_schema(&url).await.expect("reset");
    // First apply.
    let store1 = PostgresStateStore::new(&test_config(), &url, "idempotent")
        .await
        .expect("new 1");
    store1.close().await;
    // Second apply — should verify checksums and exit clean.
    let store2 = PostgresStateStore::new(&test_config(), &url, "idempotent")
        .await
        .expect("new 2");
    store2.close().await;
}

// ---------------------------------------------------------------------------
// Project profiles
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_project_profile_roundtrip() {
    let Some(store) = fresh_store("profile").await else {
        return;
    };
    assert_eq!(store.latest_project_profile().unwrap(), None);
    store
        .upsert_project_profile(r#"{"languages":["rust"],"frameworks":["tokio"]}"#)
        .unwrap();
    let fetched = store.latest_project_profile().unwrap().unwrap();
    assert!(fetched.contains("\"rust\""));
    assert!(fetched.contains("\"tokio\""));
    // Upsert with a different body should replace, not append.
    store
        .upsert_project_profile(r#"{"languages":["go"]}"#)
        .unwrap();
    let fetched2 = store.latest_project_profile().unwrap().unwrap();
    assert!(fetched2.contains("\"go\""));
    assert!(!fetched2.contains("\"rust\""));
    store.close().await;
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_approvals_set_check_and_scoped() {
    let Some(store) = fresh_store("appr").await else {
        return;
    };
    // Not approved by default.
    assert!(!store.is_approved("shell.run", ApprovalScope::Once).unwrap());
    assert!(!store
        .is_approved("shell.run", ApprovalScope::Session)
        .unwrap());

    // Approve at Session scope — Once + Forever remain unapproved.
    store
        .set_approval("shell.run", ApprovalScope::Session, true)
        .unwrap();
    assert!(store
        .is_approved("shell.run", ApprovalScope::Session)
        .unwrap());
    assert!(!store.is_approved("shell.run", ApprovalScope::Once).unwrap());
    assert!(!store
        .is_approved("shell.run", ApprovalScope::Forever)
        .unwrap());

    // Upsert to "denied" — the row still exists, is_approved is now false.
    store
        .set_approval("shell.run", ApprovalScope::Session, false)
        .unwrap();
    assert!(!store
        .is_approved("shell.run", ApprovalScope::Session)
        .unwrap());
    store.close().await;
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_audit_record_and_list_paginated() {
    let Some(store) = fresh_store("audit").await else {
        return;
    };
    // Legacy entry point — should still write with outcome='unknown'.
    store
        .record_audit_event("legacy_event", r#"{"note":"legacy"}"#)
        .unwrap();

    // Modern structured API.
    for i in 0..5 {
        let record = AuditRecord {
            operation: "memory.ingest_conversation",
            params_json: format!(r#"{{"i":{i}}}"#),
            outcome: if i % 2 == 0 {
                AuditOutcome::Ok
            } else {
                AuditOutcome::Error
            },
            error_kind: if i % 2 == 0 { None } else { Some("sqlite") },
        };
        store.record_audit(&record).unwrap();
    }

    let count = store.audit_event_count_for_project().unwrap();
    assert_eq!(count, 6, "1 legacy + 5 structured");

    // Paginate — request 2 per page.
    let req = PageRequest::decode(
        2,
        None,
        page_limits::AUDIT_EVENTS_DEFAULT,
        page_limits::AUDIT_EVENTS_MAX,
    )
    .unwrap();
    let page1 = store.list_audit_events_paged(&req).unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some(), "should have next page");
    assert_eq!(page1.total_count, Some(6));

    // Legacy wrapper returns the flat vec.
    let all = store.list_audit_events(10).unwrap();
    assert_eq!(all.len(), 6);
    // ORDER BY id DESC means the most recent (structured i=4) is first.
    assert!(all[0].event_type.starts_with("memory.ingest_conversation"));
    // Legacy event was written first → last in the DESC list.
    let last = all.last().unwrap();
    assert_eq!(last.event_type, "legacy_event");
    assert_eq!(last.outcome, "unknown");

    store.close().await;
}

// ---------------------------------------------------------------------------
// Conversation sources
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_conversation_sources_upsert_list_filter() {
    let Some(store) = fresh_store("conv").await else {
        return;
    };
    for (idx, wing) in ["ideas", "ideas", "logs"].iter().enumerate() {
        let record = ConversationSourceRecord {
            project_id: "conv".to_string(),
            transcript_path: format!("/tmp/t{idx}.json"),
            memory_path: format!("mem://t{idx}"),
            format: "claude".to_string(),
            wing: Some(wing.to_string()),
            hall: None,
            room: None,
            message_count: idx + 1,
        };
        store.upsert_conversation_source(&record).unwrap();
    }

    let all = store
        .list_conversation_sources(None, None, None, 10)
        .unwrap();
    assert_eq!(all.len(), 3);

    let ideas_only = store
        .list_conversation_sources(Some("ideas"), None, None, 10)
        .unwrap();
    assert_eq!(ideas_only.len(), 2);
    for r in &ideas_only {
        assert_eq!(r.wing.as_deref(), Some("ideas"));
    }

    // Upsert the same transcript path → replaces, doesn't duplicate.
    let replacement = ConversationSourceRecord {
        project_id: "conv".to_string(),
        transcript_path: "/tmp/t0.json".to_string(),
        memory_path: "mem://updated".to_string(),
        format: "claude".to_string(),
        wing: Some("ideas".to_string()),
        hall: None,
        room: None,
        message_count: 99,
    };
    store.upsert_conversation_source(&replacement).unwrap();
    let after = store
        .list_conversation_sources(None, None, None, 10)
        .unwrap();
    assert_eq!(after.len(), 3, "upsert replaces, no new row");
    let updated = after
        .iter()
        .find(|r| r.transcript_path == "/tmp/t0.json")
        .unwrap();
    assert_eq!(updated.memory_path, "mem://updated");
    assert_eq!(updated.message_count, 99);

    store.close().await;
}

// ---------------------------------------------------------------------------
// AAAK lessons
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_aaak_lessons_upsert_list_delete() {
    let Some(store) = fresh_store("aaak").await else {
        return;
    };
    for i in 0..3 {
        let lesson = AaakLesson {
            lesson_id: format!("lesson-{i}"),
            project_id: "aaak".to_string(),
            pattern_key: format!("pattern-{i}"),
            role: "user".to_string(),
            canonical_text: format!("text-{i}"),
            occurrence_count: (i + 1) * 2,
            confidence_percent: 80 + i as u8,
            source_transcript_path: Some(format!("/tmp/ts-{i}.json")),
            updated_at_epoch_ms: 1_700_000_000_000 + i as i64,
        };
        store.upsert_aaak_lesson(&lesson).unwrap();
    }

    let lessons = store.list_aaak_lessons("aaak", 10).unwrap();
    assert_eq!(lessons.len(), 3);
    // ORDER BY confidence DESC — lesson-2 (82%) first, lesson-0 (80%) last.
    assert_eq!(lessons[0].lesson_id, "lesson-2");
    assert_eq!(lessons[2].lesson_id, "lesson-0");

    // Delete one.
    let deleted = store.delete_aaak_lesson("lesson-1").unwrap();
    assert!(deleted);
    let post_delete = store.list_aaak_lessons("aaak", 10).unwrap();
    assert_eq!(post_delete.len(), 2);

    // Delete a non-existent lesson returns false.
    assert!(!store.delete_aaak_lesson("nope").unwrap());

    store.close().await;
}

// ---------------------------------------------------------------------------
// Diary + FTS
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_diary_upsert_list_and_fts_search() {
    let Some(store) = fresh_store("diary").await else {
        return;
    };

    let entries = vec![
        DiaryEntry {
            entry_id: "e1".to_string(),
            project_id: "diary".to_string(),
            entry_date: "2026-04-10".to_string(),
            mood: Some("focused".to_string()),
            tags: vec!["rust".to_string(), "tokio".to_string()],
            content: "Wrestled with the borrow checker on a new lifetime annotation.".to_string(),
            created_at_epoch_ms: 1_000,
            updated_at_epoch_ms: 1_000,
        },
        DiaryEntry {
            entry_id: "e2".to_string(),
            project_id: "diary".to_string(),
            entry_date: "2026-04-11".to_string(),
            mood: Some("calm".to_string()),
            tags: vec!["postgres".to_string(), "sqlx".to_string()],
            content: "Wired the pgvector backend into the broker factory.".to_string(),
            created_at_epoch_ms: 2_000,
            updated_at_epoch_ms: 2_000,
        },
        DiaryEntry {
            entry_id: "e3".to_string(),
            project_id: "diary".to_string(),
            entry_date: "2026-04-09".to_string(),
            mood: None,
            tags: vec!["cooking".to_string()],
            content: "Tried a new pasta recipe, unrelated to code.".to_string(),
            created_at_epoch_ms: 500,
            updated_at_epoch_ms: 500,
        },
    ];
    for e in &entries {
        store.upsert_diary_entry(e).unwrap();
    }

    // List — ORDER BY entry_date DESC.
    let all = store.list_diary_entries(None, None, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].entry_id, "e2"); // 2026-04-11
    assert_eq!(all[2].entry_id, "e3"); // 2026-04-09

    // Range filter.
    let filtered = store
        .list_diary_entries(Some("2026-04-10"), Some("2026-04-11"), 10)
        .unwrap();
    assert_eq!(filtered.len(), 2);

    // FTS search via tsvector.
    let borrow = store
        .search_diary_entries_in_range("borrow checker", None, None, 10)
        .unwrap();
    assert!(
        borrow.iter().any(|e| e.entry_id == "e1"),
        "search should find e1 for 'borrow checker': {borrow:?}"
    );

    // Tag-field match.
    let tokio_tag = store
        .search_diary_entries_in_range("tokio", None, None, 10)
        .unwrap();
    assert!(tokio_tag.iter().any(|e| e.entry_id == "e1"));

    // LIKE fallback: pure punctuation produces an empty tsquery, so
    // the LIKE path runs. We search for a substring that appears only
    // in e3.
    let pasta = store
        .search_diary_entries_in_range("pasta recipe", None, None, 10)
        .unwrap();
    assert!(
        pasta.iter().any(|e| e.entry_id == "e3"),
        "pasta recipe should land on e3"
    );

    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_diary_upsert_is_atomic_on_update() {
    // Regression test for the Phase 0 atomicity fix: re-upserting an
    // entry must leave both the main row and the tsvector consistent.
    let Some(store) = fresh_store("atomic").await else {
        return;
    };
    let v1 = DiaryEntry {
        entry_id: "a".to_string(),
        project_id: "atomic".to_string(),
        entry_date: "2026-04-11".to_string(),
        mood: None,
        tags: vec!["initial".to_string()],
        content: "original words".to_string(),
        created_at_epoch_ms: 1,
        updated_at_epoch_ms: 1,
    };
    store.upsert_diary_entry(&v1).unwrap();
    let v2 = DiaryEntry {
        content: "replaced payload text".to_string(),
        updated_at_epoch_ms: 2,
        ..v1.clone()
    };
    store.upsert_diary_entry(&v2).unwrap();

    // FTS must now find the new content and NOT the old.
    let new_hits = store
        .search_diary_entries_in_range("replaced payload", None, None, 10)
        .unwrap();
    assert_eq!(new_hits.len(), 1);
    assert_eq!(new_hits[0].entry_id, "a");
    let old_hits = store
        .search_diary_entries_in_range("original words", None, None, 10)
        .unwrap();
    assert!(
        old_hits.iter().all(|e| e.entry_id != "a"),
        "old FTS row should be gone: {old_hits:?}"
    );

    store.close().await;
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_navigation_nodes_and_tunnels() {
    let Some(store) = fresh_store("nav").await else {
        return;
    };

    // Build a small tree: drawer → closet → room.
    let drawer = MemoryNavigationNode {
        node_id: "drawer-a".to_string(),
        project_id: "nav".to_string(),
        kind: MemoryNavigationNodeKind::Drawer,
        label: "ideas".to_string(),
        parent_node_id: None,
        wing: Some("ideas".to_string()),
        hall: None,
        room: None,
        updated_at_epoch_ms: 100,
    };
    let closet = MemoryNavigationNode {
        node_id: "closet-b".to_string(),
        project_id: "nav".to_string(),
        kind: MemoryNavigationNodeKind::Closet,
        label: "active".to_string(),
        parent_node_id: Some("drawer-a".to_string()),
        wing: Some("ideas".to_string()),
        hall: Some("active".to_string()),
        room: None,
        updated_at_epoch_ms: 200,
    };
    let room = MemoryNavigationNode {
        node_id: "room-c".to_string(),
        project_id: "nav".to_string(),
        kind: MemoryNavigationNodeKind::Room,
        label: "phase3".to_string(),
        parent_node_id: Some("closet-b".to_string()),
        wing: Some("ideas".to_string()),
        hall: Some("active".to_string()),
        room: Some("phase3".to_string()),
        updated_at_epoch_ms: 300,
    };

    store.upsert_navigation_node(&drawer).unwrap();
    store.upsert_navigation_node(&closet).unwrap();
    store.upsert_navigation_node(&room).unwrap();

    let fetched = store.get_navigation_node("room-c").unwrap().unwrap();
    assert_eq!(fetched.label, "phase3");
    assert_eq!(fetched.kind, MemoryNavigationNodeKind::Room);

    // Filtered listing — everything under closet-b.
    let req = PageRequest::decode(
        0,
        None,
        page_limits::NAVIGATION_NODES_DEFAULT,
        page_limits::NAVIGATION_NODES_MAX,
    )
    .unwrap();
    let under_closet = store
        .list_navigation_nodes_paged(Some("closet-b"), None, &req)
        .unwrap();
    assert_eq!(under_closet.items.len(), 1);
    assert_eq!(under_closet.items[0].node_id, "room-c");

    // Tunnels — drawer ↔ room + closet ↔ room.
    let t1 = MemoryNavigationTunnel {
        tunnel_id: "t1".to_string(),
        project_id: "nav".to_string(),
        from_node_id: "drawer-a".to_string(),
        to_node_id: "room-c".to_string(),
        updated_at_epoch_ms: 400,
    };
    let t2 = MemoryNavigationTunnel {
        tunnel_id: "t2".to_string(),
        project_id: "nav".to_string(),
        from_node_id: "closet-b".to_string(),
        to_node_id: "room-c".to_string(),
        updated_at_epoch_ms: 500,
    };
    store.upsert_navigation_tunnel(&t1).unwrap();
    store.upsert_navigation_tunnel(&t2).unwrap();

    let all_tunnels = store.list_navigation_tunnels_paged(None, &req).unwrap();
    assert_eq!(all_tunnels.items.len(), 2);

    let touching_room = store
        .list_navigation_tunnels_paged(Some("room-c"), &req)
        .unwrap();
    assert_eq!(touching_room.items.len(), 2);

    let from_drawer_or_closet = store
        .list_navigation_tunnels_for_nodes(&["drawer-a".to_string(), "closet-b".to_string()], 10)
        .unwrap();
    assert_eq!(from_drawer_or_closet.len(), 2);

    store.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_state_navigation_rejects_cross_project_upsert() {
    let Some(store) = fresh_store("scoped").await else {
        return;
    };
    let other = MemoryNavigationNode {
        node_id: "n1".to_string(),
        project_id: "OTHER-project".to_string(), // wrong scope
        kind: MemoryNavigationNodeKind::Drawer,
        label: "x".to_string(),
        parent_node_id: None,
        wing: None,
        hall: None,
        room: None,
        updated_at_epoch_ms: 1,
    };
    let err = store.upsert_navigation_node(&other);
    assert!(err.is_err(), "cross-project upsert should be rejected");
    store.close().await;
}
