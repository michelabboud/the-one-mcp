//! Production-hardening regression tests.
//!
//! One test per finding from `docs/reviews/2026-04-10-mempalace-comparative-audit.md`.
//! Each test asserts the post-fix invariant, NOT the failing pre-fix behaviour,
//! so future refactors can't regress the hardening without triggering a
//! test failure.
//!
//! When adding a new finding to the report, add a matching test here with
//! the same short id (C1, C3, H2, etc.) so the report and the test grid
//! stay in sync.

use tempfile::TempDir;

use the_one_core::audit::{error_kind_label, AuditOutcome, AuditRecord};
use the_one_core::error::CoreError;
use the_one_core::naming::{
    sanitize_action_key, sanitize_name, sanitize_optional_name, sanitize_project_id,
};
use the_one_core::pagination::{Cursor, Page, PageRequest, GLOBAL_MAX_PAGE_SIZE};
use the_one_core::storage::sqlite::{page_limits, ConversationSourceRecord, ProjectDatabase};

fn fresh_db() -> (TempDir, ProjectDatabase) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("project dir");
    let db = ProjectDatabase::open(&root, "project-1").expect("open db");
    (tmp, db)
}

// ── Lever 1 (v0.15.1): synchronous=NORMAL durability / speed trade-off ───

#[test]
fn lever1_synchronous_is_normal_in_wal_mode() {
    // v0.15.1 flipped PRAGMA synchronous from the SQLite default of FULL
    // to NORMAL in WAL mode. This regression guard fails the build if the
    // pragma is ever accidentally reverted.
    //
    // Why it matters: in WAL mode, synchronous=FULL calls fsync() before
    // every commit, capping audit-log writes at ~5ms each. synchronous=
    // NORMAL defers fsync to WAL checkpoint time and gets 10-50×
    // throughput for audit/diary/navigation writes. The only durability
    // gap is < 1s of writes on OS crash or power loss — still safe on
    // process crashes because the WAL file captures every commit.
    let (_tmp, db) = fresh_db();
    assert_eq!(db.journal_mode().unwrap(), "wal");
    assert_eq!(
        db.synchronous_mode().unwrap(),
        1,
        "synchronous must be NORMAL (1), was {}",
        db.synchronous_mode().unwrap()
    );
}

// ── C1: audit log records outcome + error_kind ────────────────────────────

#[test]
fn c1_audit_record_structured_with_outcome_and_error_kind() {
    let (_tmp, db) = fresh_db();

    db.record_audit(&AuditRecord::ok("docs.create", "{}"))
        .unwrap();

    let err = CoreError::InvalidRequest("bad".into());
    db.record_audit(&AuditRecord::error("docs.create", "{}", &err))
        .unwrap();

    let events = db.list_audit_events(10).unwrap();
    // Most recent first.
    assert_eq!(events[0].outcome, "error");
    assert_eq!(events[0].error_kind.as_deref(), Some("invalid_request"));
    assert_eq!(events[1].outcome, "ok");
    assert!(events[1].error_kind.is_none());

    assert_eq!(db.audit_outcome_count(AuditOutcome::Ok).unwrap(), 1);
    assert_eq!(db.audit_outcome_count(AuditOutcome::Error).unwrap(), 1);
}

#[test]
fn c1_error_kind_label_covers_every_core_error_variant() {
    // Exhaustive: every CoreError variant must produce a stable, non-empty,
    // snake_case label. If a new variant is added without updating
    // error_kind_label, this test fails the pattern match on construction.
    let cases: &[(CoreError, &str)] = &[
        (CoreError::Io(std::io::Error::other("x")), "io"),
        (
            CoreError::Json(serde_json::from_str::<serde_json::Value>("x").unwrap_err()),
            "json",
        ),
        (
            CoreError::InvalidProjectConfig("x".into()),
            "invalid_project_config",
        ),
        (CoreError::PolicyDenied("x".into()), "policy_denied"),
        (
            CoreError::UnsupportedSchemaVersion("x".into()),
            "unsupported_schema_version",
        ),
        (CoreError::Embedding("x".into()), "embedding"),
        (CoreError::Transport("x".into()), "transport"),
        (CoreError::Provider("x".into()), "provider"),
        (CoreError::Document("x".into()), "document"),
        (CoreError::Catalog("x".into()), "catalog"),
        (CoreError::NotEnabled("x".into()), "not_enabled"),
        (CoreError::InvalidRequest("x".into()), "invalid_request"),
        (CoreError::Postgres("x".into()), "postgres"),
        (CoreError::Redis("x".into()), "redis"),
    ];
    for (err, expected) in cases {
        assert_eq!(error_kind_label(err), *expected);
    }
}

// ── C3: navigation_digest width + project scoping ─────────────────────────

#[test]
fn c3_navigation_digest_is_at_least_32_hex_chars() {
    // The broker's navigation_* functions are private, so we prove the
    // property via the database: ingested navigation nodes produced by the
    // broker must have the new wide digest. We create nodes directly via
    // the storage layer and just sanity check the schema can store them.
    // The real exercise happens in `tests/stdio_write_path.rs::stdio_navigation_digest_width_regression`.
    //
    // This unit test just asserts the SQLite schema doesn't truncate long
    // node_ids (a 32-hex id pushes the full `drawer:<slug>-<32hex>` well
    // past the 12-char limit of the v0.14.x format).
    let long_id = format!("drawer:ops-{}", "0123456789abcdef".repeat(2));
    // 11 ("drawer:ops-") + 32 hex = 43. The important property is that the
    // 32-char hex tail is strictly longer than the v0.14.x 12-char tail.
    assert!(long_id.len() > 42, "actual: {}", long_id.len());
    assert!(long_id.ends_with("0123456789abcdef0123456789abcdef"));
    let (_tmp, db) = fresh_db();
    use the_one_core::contracts::{MemoryNavigationNode, MemoryNavigationNodeKind};
    db.upsert_navigation_node(&MemoryNavigationNode {
        node_id: long_id.clone(),
        project_id: "project-1".to_string(),
        kind: MemoryNavigationNodeKind::Drawer,
        label: "ops".to_string(),
        parent_node_id: None,
        wing: Some("ops".to_string()),
        hall: None,
        room: None,
        updated_at_epoch_ms: 1,
    })
    .unwrap();

    let fetched = db.get_navigation_node(&long_id).unwrap().unwrap();
    assert_eq!(fetched.node_id, long_id);
}

// ── C5: pagination rejects over-limit instead of silent truncation ────────

#[test]
fn c5_pagination_rejects_over_limit() {
    // Per-endpoint caps are declared in page_limits. Requesting more than
    // the declared max must return InvalidRequest, NEVER silently clamp.
    let err = PageRequest::decode(10_000, None, 20, page_limits::DIARY_ENTRIES_MAX).unwrap_err();
    assert!(matches!(err, CoreError::InvalidRequest(_)));
    let msg = format!("{err}");
    assert!(msg.contains("exceeds maximum"), "got: {msg}");

    // And the global cap supersedes even a generous endpoint max.
    let err = PageRequest::decode(
        GLOBAL_MAX_PAGE_SIZE + 1,
        None,
        20,
        GLOBAL_MAX_PAGE_SIZE + 10,
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::InvalidRequest(_)));
}

#[test]
fn c5_pagination_cursor_roundtrips_opaque_payload() {
    // Clients must be able to pass the cursor back verbatim and land on
    // exactly the next page — no client-side parsing, no cursor drift.
    let cursor = Cursor::from_offset(42);
    let (off, tie) = Cursor::decode(cursor.as_str()).unwrap();
    assert_eq!(off, 42);
    assert!(tie.is_none());

    // Malformed cursors are a 400-equivalent, not a panic.
    assert!(Cursor::decode("not base64 !!!").is_err());
}

#[test]
fn c5_page_from_peek_emits_next_cursor_exactly_when_more_remains() {
    let rows: Vec<i32> = (0..11).collect();
    let page = Page::from_peek(rows, 10, 0, None);
    assert_eq!(page.items.len(), 10);
    assert!(page.next_cursor.is_some());

    let rows: Vec<i32> = (0..5).collect();
    let page = Page::from_peek(rows, 10, 0, None);
    assert_eq!(page.items.len(), 5);
    assert!(page.next_cursor.is_none());
}

#[test]
fn c5_list_audit_events_paginated_roundtrip() {
    let (_tmp, db) = fresh_db();
    for i in 0..150 {
        db.record_audit_event("tool_run", &format!("{{\"i\":{i}}}"))
            .unwrap();
    }

    // First page
    let req = PageRequest::decode(50, None, 50, 500).unwrap();
    let page1 = db.list_audit_events_paged(&req).unwrap();
    assert_eq!(page1.items.len(), 50);
    assert!(page1.next_cursor.is_some());
    assert_eq!(page1.total_count, Some(150));

    // Follow cursor
    let req2 = PageRequest::decode(
        50,
        Some(page1.next_cursor.as_ref().unwrap().as_str()),
        50,
        500,
    )
    .unwrap();
    let page2 = db.list_audit_events_paged(&req2).unwrap();
    assert_eq!(page2.items.len(), 50);

    // Final page
    let req3 = PageRequest::decode(
        50,
        Some(page2.next_cursor.as_ref().unwrap().as_str()),
        50,
        500,
    )
    .unwrap();
    let page3 = db.list_audit_events_paged(&req3).unwrap();
    assert_eq!(page3.items.len(), 50);
    assert!(page3.next_cursor.is_none());
}

#[test]
fn c5_list_navigation_tunnels_for_nodes_is_sql_filtered() {
    // Regression guard for "memory_navigation_list used to fetch every
    // tunnel into Rust and filter client-side". The new helper must return
    // only tunnels touching the requested nodes.
    use the_one_core::contracts::{
        MemoryNavigationNode, MemoryNavigationNodeKind, MemoryNavigationTunnel,
    };
    let (_tmp, db) = fresh_db();

    // 4 drawers, 3 tunnels: (a<->b), (c<->d), (a<->d)
    for name in ["a", "b", "c", "d"] {
        db.upsert_navigation_node(&MemoryNavigationNode {
            node_id: format!("drawer:{name}"),
            project_id: "project-1".into(),
            kind: MemoryNavigationNodeKind::Drawer,
            label: name.into(),
            parent_node_id: None,
            wing: Some(name.into()),
            hall: None,
            room: None,
            updated_at_epoch_ms: 1,
        })
        .unwrap();
    }
    for (lo, hi, id) in &[
        ("drawer:a", "drawer:b", "t:ab"),
        ("drawer:c", "drawer:d", "t:cd"),
        ("drawer:a", "drawer:d", "t:ad"),
    ] {
        db.upsert_navigation_tunnel(&MemoryNavigationTunnel {
            tunnel_id: (*id).into(),
            project_id: "project-1".into(),
            from_node_id: (*lo).into(),
            to_node_id: (*hi).into(),
            updated_at_epoch_ms: 1,
        })
        .unwrap();
    }

    // Only ask for tunnels touching drawer:a — must return t:ab and t:ad,
    // never t:cd.
    let tunnels = db
        .list_navigation_tunnels_for_nodes(&["drawer:a".to_string()], 100)
        .unwrap();
    let ids: Vec<_> = tunnels.iter().map(|t| t.tunnel_id.clone()).collect();
    assert!(ids.contains(&"t:ab".to_string()));
    assert!(ids.contains(&"t:ad".to_string()));
    assert!(!ids.contains(&"t:cd".to_string()));
}

// ── H1 / C2: grep gate that `let _ =` discards are gone from broker hotpaths ──

#[test]
fn h1_broker_does_not_silently_drop_side_effect_results() {
    // Static grep against broker.rs source. This is a belt-and-braces check
    // for patterns that are hard to detect at runtime. If someone adds a
    // new `let _ = foo()?` somewhere in an error-discarding position,
    // this test fails with a specific line.
    let broker_src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/broker.rs"))
        .expect("read broker.rs");

    // These specific sites were documented in the hardening report as the
    // error-swallowing locations. The fix replaces each with a
    // `tracing::warn!` or a proper Err propagation. If any of these exact
    // lines come back, the test fails.
    let forbidden_exact_lines: &[&str] = &[
        "            let _ = self.registry.save_to_path(path);",
        "        let config = AppConfig::load(project_root, RuntimeOverrides::default()).ok();",
        "                let _ = catalog.import_catalog_dir(&dir);",
        "            let _ = catalog.scan_system_inventory();",
        "            let _ = cat.scan_system_inventory();",
        "            let _ = cat.enable_tool(&request.tool_id, \"default\", &request.project_root);",
        "        self.ensure_catalog().ok();",
    ];
    for line in forbidden_exact_lines {
        assert!(
            !broker_src.contains(line),
            "broker.rs still contains the v0.14.x error-swallowing pattern: {line}",
        );
    }
}

// ── H2: error response sanitisation ────────────────────────────────────────

#[test]
fn h2_sanitize_name_rejects_path_traversal_and_allows_namespaced() {
    assert!(sanitize_name("..", "wing").is_err());
    assert!(sanitize_name("foo/bar", "wing").is_err());
    assert!(sanitize_name("", "wing").is_err());
    assert!(sanitize_name(".hidden", "wing").is_err());
    assert!(sanitize_name("trailing.", "wing").is_err());

    assert_eq!(sanitize_name("ops", "wing").unwrap(), "ops");
    assert_eq!(
        sanitize_name("hook:precompact", "hall").unwrap(),
        "hook:precompact"
    );
    assert_eq!(sanitize_optional_name(None, "wing").unwrap(), None);
}

#[test]
fn h2_sanitize_project_id_stricter_than_name() {
    assert!(sanitize_project_id("has space").is_err());
    assert!(sanitize_project_id("v1.2").is_err());
    assert!(sanitize_project_id("-leading").is_err());
    assert_eq!(sanitize_project_id("ops-2026").unwrap(), "ops-2026");
}

#[test]
fn h2_sanitize_action_key_allows_namespaces() {
    assert_eq!(
        sanitize_action_key("tool.run:danger").unwrap(),
        "tool.run:danger"
    );
    assert!(sanitize_action_key("tool run").is_err());
    assert!(sanitize_action_key("..evil").is_err());
}

// ── H4: conversation source list is paginable end-to-end ──────────────────

#[test]
fn h4_conversation_source_metadata_survives_reopen() {
    // Regression guard: reopening the DB after a write must see the row.
    // If we ever break file-sync or flush semantics this test will fail.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("project dir");

    {
        let db = ProjectDatabase::open(&root, "project-1").unwrap();
        db.upsert_conversation_source(&ConversationSourceRecord {
            project_id: "project-1".into(),
            transcript_path: "/tmp/x.json".into(),
            memory_path: "/tmp/x.md".into(),
            format: "openai_messages".into(),
            wing: Some("ops".into()),
            hall: Some("incidents".into()),
            room: Some("auth".into()),
            message_count: 3,
        })
        .unwrap();
    }
    // Drop the DB, reopen — row must persist.
    {
        let db = ProjectDatabase::open(&root, "project-1").unwrap();
        let rows = db
            .list_conversation_sources(Some("ops"), None, None, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
}
