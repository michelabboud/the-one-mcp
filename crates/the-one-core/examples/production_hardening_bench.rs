//! Production-hardening micro-benchmarks (v0.15.0).
//!
//! Measures the throughput/latency impact of the v0.15.0 hardening pass:
//!
//! 1. **Audit log throughput** — `record_audit` latency at 1k, 10k rows.
//! 2. **List pagination scaling** — `list_audit_events_paged` time for
//!    page 1 and page N at 10k, 100k rows.
//! 3. **Diary list pagination** — same thing for diary entries.
//! 4. **Navigation tunnel SQL-side filter** — compares the new
//!    `list_navigation_tunnels_for_nodes` against the naive "load all +
//!    filter in Rust" approach at 10k tunnels.
//!
//! Prints a markdown-ish summary to stdout. No criterion dependency — the
//! the-one-mcp workspace intentionally avoids heavyweight bench runners.
//!
//! # How to run
//!
//! ```bash
//! cargo run --release --example production_hardening_bench -p the-one-core
//! ```
//!
//! The benchmark uses a throwaway tempdir SQLite DB so it's safe to run
//! anywhere. Expected runtime: ~10s in release mode, ~60s in debug.
//!
//! # See also (v0.16.0 Phase 2)
//!
//! The pgvector backend has its own throughput/latency bench at
//! `crates/the-one-memory/examples/pgvector_bench.rs`. It lives in
//! `the-one-memory` because `the-one-core` deliberately does not
//! depend on the vector crate (the dependency arrow points the other
//! way). Run it with:
//!
//! ```bash
//! THE_ONE_VECTOR_TYPE=pgvector \
//! THE_ONE_VECTOR_URL=postgres://... \
//! cargo run --release --example pgvector_bench \
//!     -p the-one-memory --features pg-vectors
//! ```

use std::collections::HashSet;
use std::time::Instant;

use the_one_core::contracts::{
    MemoryNavigationNode, MemoryNavigationNodeKind, MemoryNavigationTunnel,
};
use the_one_core::pagination::{Cursor, PageRequest};
use the_one_core::storage::sqlite::{page_limits, ProjectDatabase};

fn fresh_db() -> (tempfile::TempDir, ProjectDatabase) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("project dir");
    let db = ProjectDatabase::open(&root, "bench").expect("open db");
    (tmp, db)
}

fn fmt_ms(nanos: u128) -> String {
    if nanos < 1_000 {
        format!("{nanos}ns")
    } else if nanos < 1_000_000 {
        format!("{:.2}µs", nanos as f64 / 1_000.0)
    } else if nanos < 1_000_000_000 {
        format!("{:.2}ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", nanos as f64 / 1_000_000_000.0)
    }
}

fn bench_audit_throughput() {
    println!("\n## Audit log throughput\n");
    println!("| rows | record_audit total | per-row  | paged list (page 1) |");
    println!("|-----:|-------------------:|---------:|--------------------:|");
    for &n in &[1_000usize, 10_000usize] {
        let (_tmp, db) = fresh_db();

        let t0 = Instant::now();
        for i in 0..n {
            db.record_audit_event("tool_run", &format!("{{\"i\":{i}}}"))
                .expect("record audit");
        }
        let write_nanos = t0.elapsed().as_nanos();

        let req =
            PageRequest::decode(100, None, 100, page_limits::AUDIT_EVENTS_MAX).expect("page req");
        let t1 = Instant::now();
        let page = db.list_audit_events_paged(&req).expect("list");
        let read_nanos = t1.elapsed().as_nanos();
        assert_eq!(page.items.len(), 100);

        println!(
            "| {:>4} | {:>18} | {:>8} | {:>19} |",
            n,
            fmt_ms(write_nanos),
            fmt_ms(write_nanos / n as u128),
            fmt_ms(read_nanos)
        );
    }
}

fn bench_pagination_deep_pages() {
    println!("\n## List pagination depth (audit_events)\n");
    println!("| rows  | offset | latency  |");
    println!("|------:|-------:|---------:|");
    let (_tmp, db) = fresh_db();
    for i in 0..10_000usize {
        db.record_audit_event("tool_run", &format!("{{\"i\":{i}}}"))
            .unwrap();
    }
    for &offset in &[0u64, 100, 1_000, 5_000, 9_900] {
        let cursor_string;
        let cursor_opt: Option<&str> = if offset == 0 {
            None
        } else {
            let cursor = Cursor::from_offset(offset);
            cursor_string = cursor.0;
            Some(cursor_string.as_str())
        };
        let req = PageRequest::decode(100, cursor_opt, 100, page_limits::AUDIT_EVENTS_MAX).unwrap();
        let t0 = Instant::now();
        let page = db.list_audit_events_paged(&req).unwrap();
        let nanos = t0.elapsed().as_nanos();
        println!("| {:>5} | {:>6} | {:>8} |", 10_000, offset, fmt_ms(nanos));
        // Sanity: every page has 100 items except the tail.
        assert!(
            page.items.len() == 100 || offset + 100 > 10_000,
            "unexpected page size at offset {offset}"
        );
    }
}

fn seed_navigation_tunnels(db: &ProjectDatabase, nodes: usize, tunnels: usize) {
    // Create `nodes` drawer nodes.
    for i in 0..nodes {
        db.upsert_navigation_node(&MemoryNavigationNode {
            node_id: format!("drawer:n{i}"),
            project_id: "bench".into(),
            kind: MemoryNavigationNodeKind::Drawer,
            label: format!("n{i}"),
            parent_node_id: None,
            wing: Some(format!("w{i}")),
            hall: None,
            room: None,
            updated_at_epoch_ms: 1,
        })
        .unwrap();
    }
    // Create `tunnels` linking random pairs — for deterministic bench we
    // use (i, (i+1) % nodes).
    for i in 0..tunnels {
        let from = format!("drawer:n{}", i % nodes);
        let to = format!("drawer:n{}", (i + 1) % nodes);
        if from == to {
            continue;
        }
        // Normalize to satisfy CHECK(from < to).
        let (lo, hi) = if from < to { (from, to) } else { (to, from) };
        let _ = db.upsert_navigation_tunnel(&MemoryNavigationTunnel {
            tunnel_id: format!("t{i}"),
            project_id: "bench".into(),
            from_node_id: lo,
            to_node_id: hi,
            updated_at_epoch_ms: 1,
        });
    }
}

fn bench_navigation_tunnel_sql_vs_rust_filter() {
    println!("\n## Navigation tunnel filter: SQL vs client-side\n");
    println!("| nodes | tunnels | SQL helper | Rust filter (legacy) |");
    println!("|------:|--------:|-----------:|---------------------:|");

    let (_tmp, db) = fresh_db();
    seed_navigation_tunnels(&db, 500, 10_000);

    // Target set: the first 20 nodes.
    let targets: Vec<String> = (0..20).map(|i| format!("drawer:n{i}")).collect();

    // 1. SQL-filtered helper (v0.15.0 path).
    let t0 = Instant::now();
    let sql_results = db
        .list_navigation_tunnels_for_nodes(&targets, page_limits::NAVIGATION_TUNNELS_MAX)
        .unwrap();
    let sql_nanos = t0.elapsed().as_nanos();

    // 2. "Legacy" path — load all tunnels, filter in Rust. This is what
    //    memory_navigation_list used to do in v0.14.x before we pushed the
    //    filter into SQL.
    let t1 = Instant::now();
    let all = db.list_navigation_tunnels(None).unwrap();
    let targets_set: HashSet<&str> = targets.iter().map(String::as_str).collect();
    let rust_results: Vec<_> = all
        .into_iter()
        .filter(|t| {
            targets_set.contains(t.from_node_id.as_str())
                || targets_set.contains(t.to_node_id.as_str())
        })
        .collect();
    let rust_nanos = t1.elapsed().as_nanos();

    println!(
        "| {:>5} | {:>7} | {:>10} | {:>20} |",
        500,
        10_000,
        fmt_ms(sql_nanos),
        fmt_ms(rust_nanos)
    );
    println!(
        "\n    SQL helper returned {} rows, Rust filter returned {} rows",
        sql_results.len(),
        rust_results.len()
    );
}

fn bench_diary_pagination() {
    println!("\n## Diary list pagination\n");
    println!("| rows   | page 1 latency |");
    println!("|-------:|---------------:|");
    use the_one_core::contracts::DiaryEntry;
    let (_tmp, db) = fresh_db();
    for i in 0..10_000usize {
        db.upsert_diary_entry(&DiaryEntry {
            entry_id: format!("e{i}"),
            project_id: "bench".into(),
            entry_date: "2026-04-10".into(),
            mood: None,
            tags: vec![],
            content: format!("entry {i}"),
            created_at_epoch_ms: i as i64,
            updated_at_epoch_ms: i as i64,
        })
        .unwrap();
    }

    let req = PageRequest::decode(100, None, 100, page_limits::DIARY_ENTRIES_MAX).unwrap();
    let t0 = Instant::now();
    let page = db.list_diary_entries_paged(None, None, &req).unwrap();
    let nanos = t0.elapsed().as_nanos();
    println!("| {:>6} | {:>14} |", 10_000, fmt_ms(nanos));
    assert_eq!(page.items.len(), 100);
}

fn main() {
    println!("# the-one-mcp production hardening benchmarks");
    println!(
        "\nBuild: {}",
        if cfg!(debug_assertions) {
            "debug (run with --release for realistic numbers)"
        } else {
            "release"
        }
    );

    bench_audit_throughput();
    bench_pagination_deep_pages();
    bench_diary_pagination();
    bench_navigation_tunnel_sql_vs_rust_filter();

    println!("\n(end of bench)");
}
