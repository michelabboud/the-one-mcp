# Multi-Backend Architecture — Phase A (Combined A1 + A2)

**Date:** 2026-04-11
**Status:** Phase A complete — shipped in commit `5ff9872` as part of the bundled v0.15.0 + v0.15.1 + v0.16.0-rc1 release. Phases B/C (the actual backend implementations) are tracked as Phases 1–7 in the execution plan (see **Related** below).
**Target release:** v0.16.0 (trait refactor, no behaviour change).
**Supersedes:** the Phase A section of `2026-04-11-next-steps-expansion.md`
which only covered the vector layer.
**Related:** `docs/plans/2026-04-11-resume-phase1-onwards.md` — self-contained execution plan for Phases 1–7 (broker call-site migration through v0.16.0 release), including the four-var `THE_ONE_{STATE,VECTOR}_{TYPE,URL}` backend selection scheme, fail-loud startup validation, and the `postgres-combined` / `redis-combined` single-pool dispatch pattern decided in a subsequent brainstorming session.
**Authorisation:** user selected option B ("plan A1 + A2 together and
implement both today") in session 2026-04-11.

---

## 1. Goal

Make the-one-mcp's persistence layer backend-agnostic along **two
orthogonal axes**:

1. **Vector storage** (dense/sparse embeddings, chunk/entity/relation/image/hybrid search).
2. **Relational state store** (audit events, conversation sources, navigation, diary, AAAK, approvals, project profiles).

After this refactor, adding pgvector, Postgres state, or Redis-with-AOF
is a matter of implementing one of two traits in a new file, not
editing the broker or `MemoryEngine`.

## 2. Non-goals

- Implementing pgvector, Postgres, or Redis-AOF backends themselves
  (those are separate B-phase tickets).
- Any user-visible behaviour change. Every existing test must still
  pass bit-for-bit.
- Cross-backend consistency guarantees (we already document that the
  audit log has ~1s of exposure to OS crashes; that envelope doesn't
  change).

## 3. Findings from surface mapping

### 3.1 StateStore surface (from `ProjectDatabase`)

- **32 total pub methods**, **18 called by the broker** (the rest are
  observability/metadata — still included for completeness).
- **Zero rusqlite leakage** in public signatures — every method returns
  `CoreError` + plain domain structs. **This is the critical finding**:
  the refactor is a clean trait extraction, not an ownership rewrite.
- All paginated methods already use `the_one_core::pagination::PageRequest`
  and `Page<T>`.
- SQLite-specific surface to abstract:
  - FTS5 virtual table (`diary_entries_fts`) — Postgres will use
    `tsvector` + `to_tsquery` or `pg_trgm`.
  - `strftime('%s','now') * 1000` timestamp generation — Postgres uses
    `(extract(epoch from now()) * 1000)::bigint`.
  - `ON CONFLICT DO UPDATE` — **portable** (Postgres 9.5+ supports
    identical syntax).
  - `PRAGMA journal_mode/synchronous` — SQLite-only; the StateStore
    trait does not expose these (they're init-time concerns).
  - SQLITE_MAX_VARIABLE_NUMBER chunking in `list_navigation_tunnels_for_nodes`
    — SQLite-specific workaround; Postgres has 32 K parameters so it's
    unnecessary there.

### 3.2 VectorBackend surface (from `MemoryEngine`)

- **16 dispatch sites** across 8 methods in `crates/the-one-memory/src/lib.rs`.
- **13 unique operations**:
  chunk upsert (dense + hybrid), chunk search (dense + hybrid), chunk
  delete, entity collection/upsert/search, relation collection/upsert/search,
  image operations, persistence verification.
- **Qdrant supports all 13**; Redis supports only chunk upsert/search/delete
  + persistence verification.
- Current fallback for Redis-unsupported ops: silent `Ok(0)` / `Vec::new()`.
  This is implicit and must become explicit via the trait's default
  implementations returning `Err("unsupported")`.

### 3.3 `upsert_diary_entry` is not atomic

Three sequential `execute()` calls:

1. `INSERT OR REPLACE INTO diary_entries ...`
2. `DELETE FROM diary_entries_fts WHERE ...`
3. `INSERT INTO diary_entries_fts ...`

If the process crashes between #1 and #3, the FTS index is out of sync.
The StateStore trait should either (a) expose an explicit transaction
API or (b) document that the FTS index is eventually consistent and
fix the implementation with `unchecked_transaction()`. **Decision: fix
in SqliteStateStore as part of this refactor.** The trait doesn't need
a transaction primitive because no other method batches writes.

## 4. Architecture — two traits, optionally combined

### 4.1 `trait VectorBackend`

Located in new file `crates/the-one-memory/src/vector_backend.rs`.

```rust
use async_trait::async_trait;

#[async_trait]
pub trait VectorBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    // ── Chunks (required for every backend) ────────────────────────
    async fn ensure_collection(&self, dims: usize) -> Result<(), String>;
    async fn ensure_hybrid_collection(&self, dims: usize) -> Result<(), String> {
        self.ensure_collection(dims).await
    }
    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String>;
    async fn upsert_hybrid_chunks(&self, points: Vec<HybridVectorPoint>)
        -> Result<(), String> {
        let _ = points;
        Err("hybrid chunk upsert not supported by this backend".into())
    }
    async fn search_chunks(
        &self,
        dense: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<VectorHit>, String>;
    async fn search_chunks_hybrid(
        &self,
        dense: Vec<f32>,
        sparse: SparseVector,
        top_k: usize,
        threshold: f32,
    ) -> Result<HybridHits, String> {
        let _ = (dense, sparse, top_k, threshold);
        Err("hybrid search not supported by this backend".into())
    }
    async fn delete_by_source_path(&self, source_path: &str) -> Result<(), String>;

    // ── Entities (default: unsupported) ────────────────────────────
    async fn ensure_entity_collection(&self, dims: usize) -> Result<(), String> {
        let _ = dims;
        Ok(()) // best-effort no-op
    }
    async fn upsert_entity_points(&self, points: Vec<EntityPoint>) -> Result<(), String> {
        let _ = points;
        Ok(()) // silent skip mirrors v0.14.x semantics
    }
    async fn search_entities(
        &self,
        query: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<EntityHit>, String> {
        let _ = (query, top_k, threshold);
        Ok(Vec::new()) // empty-results fallback mirrors v0.14.x semantics
    }

    // ── Relations (default: unsupported) ───────────────────────────
    async fn ensure_relation_collection(&self, dims: usize) -> Result<(), String> {
        let _ = dims;
        Ok(())
    }
    async fn upsert_relation_points(&self, points: Vec<RelationPoint>)
        -> Result<(), String> {
        let _ = points;
        Ok(())
    }
    async fn search_relations(
        &self,
        query: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<RelationHit>, String> {
        let _ = (query, top_k, threshold);
        Ok(Vec::new())
    }

    // ── Persistence verification (default: always succeeds) ───────
    async fn verify_persistence(&self) -> Result<(), String> {
        Ok(())
    }
}
```

**Why default-Ok for unsupported ops instead of default-Err:** preserves
v0.14.x behaviour bit-for-bit. The Redis backend today silently no-ops
on entity/relation/image upserts and returns empty results on searches.
Changing that to `Err("unsupported")` would break `MemoryEngine` call
sites that currently ignore the error. We preserve semantics now; a
future PR can tighten the contract.

### 4.2 `trait StateStore`

Located in new file `crates/the-one-core/src/state_store.rs`.

```rust
use std::path::Path;
use crate::audit::AuditRecord;
use crate::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode,
    MemoryNavigationTunnel,
};
use crate::error::CoreError;
use crate::pagination::{Page, PageRequest};
use crate::storage::sqlite::{AuditEvent, ConversationSourceRecord};

pub trait StateStore: Send + Sync {
    // ── Metadata ───────────────────────────────────────────────────
    fn project_id(&self) -> &str;
    fn schema_version(&self) -> Result<i64, CoreError>;

    // ── Project profiles ───────────────────────────────────────────
    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError>;
    fn latest_project_profile(&self) -> Result<Option<String>, CoreError>;

    // ── Approvals ──────────────────────────────────────────────────
    fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError>;
    fn is_approved(
        &self,
        action_key: &str,
        scope: ApprovalScope,
    ) -> Result<bool, CoreError>;

    // ── Audit ──────────────────────────────────────────────────────
    fn record_audit_event(
        &self,
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), CoreError>;
    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError>;
    fn audit_event_count_for_project(&self) -> Result<u64, CoreError>;
    fn list_audit_events_paged(
        &self,
        req: &PageRequest,
    ) -> Result<Page<AuditEvent>, CoreError>;
    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError>;

    // ── Conversation sources ───────────────────────────────────────
    fn upsert_conversation_source(
        &self,
        record: &ConversationSourceRecord,
    ) -> Result<(), CoreError>;
    fn list_conversation_sources(
        &self,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSourceRecord>, CoreError>;

    // ── AAAK lessons ───────────────────────────────────────────────
    fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError>;
    fn list_aaak_lessons(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError>;
    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError>;

    // ── Diary ──────────────────────────────────────────────────────
    fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError>;
    fn list_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError>;
    fn search_diary_entries_in_range(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError>;

    // ── Navigation ─────────────────────────────────────────────────
    fn upsert_navigation_node(
        &self,
        node: &MemoryNavigationNode,
    ) -> Result<(), CoreError>;
    fn get_navigation_node(
        &self,
        node_id: &str,
    ) -> Result<Option<MemoryNavigationNode>, CoreError>;
    fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError>;
    fn upsert_navigation_tunnel(
        &self,
        tunnel: &MemoryNavigationTunnel,
    ) -> Result<(), CoreError>;
    fn list_navigation_tunnels_paged(
        &self,
        node_id: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationTunnel>, CoreError>;
    fn list_navigation_tunnels_for_nodes(
        &self,
        node_ids: &[String],
        limit: usize,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError>;

    // Capability reporting.
    fn capabilities(&self) -> StateStoreCapabilities;
}

#[derive(Debug, Clone, Copy)]
pub struct StateStoreCapabilities {
    pub name: &'static str,
    pub fts: bool,           // true for SQLite/Postgres, false for Redis
    pub transactions: bool,  // true for SQLite/Postgres, limited on Redis
    pub durable: bool,       // true for WAL SQLite / Postgres / Redis AOF
}
```

### 4.3 Combined backends

For Postgres+pgvector or Redis+RediSearch single-instance deployments,
a backend struct simply implements **both traits**. The broker then
holds:

```rust
pub struct McpBroker {
    // Was: HashMap<String, MemoryEngine>
    memory_by_project: Arc<RwLock<HashMap<String, MemoryEngine>>>,
    // Was: opened per-call via ProjectDatabase::open
    // Now: opened per-project lazily via state_store_factory
    state_by_project: RwLock<HashMap<String, Arc<dyn StateStore>>>,
    // ...
}
```

A combined backend factory can return both trait objects backed by the
same underlying connection — preserving transactional consistency
across state + vector writes when the implementation chooses to.

## 5. Implementation order (risk-minimising)

Phase A1 is smaller and lower risk. Phase A2 is larger and touches the
broker's critical path. Execute in this order:

1. **A1.1** — Create `vector_backend.rs` with trait + types.
2. **A1.2** — `impl VectorBackend for AsyncQdrantBackend` (forwarding
   to existing methods).
3. **A1.3** — `impl VectorBackend for RedisVectorStore` (chunks-only;
   default impls for the rest).
4. **A1.4** — Refactor `MemoryEngine` to hold
   `Option<Box<dyn VectorBackend>>`. Update all 16 dispatch sites.
5. **A1.5** — Refactor `McpBroker::build_memory_engine` to use a
   factory.
6. **Checkpoint 1** — full validation (fmt, clippy, tests, bench).
7. **A2.1** — Create `state_store.rs` with trait + capabilities.
8. **A2.2** — `impl StateStore for ProjectDatabase` (forwarding to
   existing methods, zero behaviour change).
9. **A2.3** — Fix `upsert_diary_entry` atomicity via
   `unchecked_transaction`.
10. **A2.4** — Update the broker to hold and route through
    `Arc<dyn StateStore>` via a cache. Use `SqliteStateStore` as the
    only factory target.
11. **Checkpoint 2** — full validation.
12. **A2.5** — Update all 18+ broker call sites to use the trait
    instead of direct `ProjectDatabase` method calls. One-to-one
    mechanical replacement.
13. **Checkpoint 3** — full validation + manual smoke test.

## 6. Acceptance criteria

1. `cargo fmt --check` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test --workspace` passes with ≥446 tests (same as before)
   + new capability tests.
4. `cargo run --release --example production_hardening_bench -p the-one-core`
   produces numbers in the same ballpark as pre-refactor.
5. Manual: broker constructs with Qdrant + SQLite, ingests a
   conversation, search returns results.
6. New tests:
   - `test_qdrant_backend_reports_full_capabilities`
   - `test_redis_backend_reports_chunks_only_capabilities` (gated on
     `redis-vectors` feature)
   - `test_sqlite_state_store_roundtrip_matches_project_database`
7. CHANGELOG entry for v0.16.0-rc1.

## 7. Migration path for future backends

Adding pgvector:
1. New file `crates/the-one-memory/src/pg_vector.rs`.
2. `impl VectorBackend for PgVectorBackend { … }` — ~800 LOC.
3. Add `pg-vectors` feature to `the-one-memory/Cargo.toml`.
4. Add `vector_backend == "pgvector"` branch in `build_memory_engine`.
5. Done.

Adding Postgres state store:
1. New file `crates/the-one-core/src/state_store/postgres.rs`.
2. `impl StateStore for PostgresStateStore { … }` — ~1500 LOC (FTS
   translation layer is the bulk).
3. Add `pg-state` feature to `the-one-core/Cargo.toml`.
4. Add `state_backend == "postgres"` branch in `StateStoreFactory`.
5. Done.

Combined Postgres+pgvector:
1. New file `crates/the-one-core/src/backend/postgres_combined.rs`.
2. One struct holding a `sqlx::PgPool`.
3. `impl VectorBackend` + `impl StateStore` on the same struct.
4. `BackendFactory` returns the same `Arc<T>` for both trait
   lookups, preserving transactional consistency.
5. Done.

## 8. Risks and mitigations

| risk | mitigation |
|------|------------|
| Behaviour drift in `MemoryEngine` refactor | Full test suite before + after; bit-for-bit pass count match. |
| `async_trait` + `Send + Sync` lifetime issues | `async_trait` already in workspace; similar patterns used elsewhere. |
| Broker call sites break during A2.5 | Implement StateStore impl first, then migrate call sites one by one with `cargo check` after each. |
| Feature flag interactions | `redis-vectors` gates Redis impl; when disabled, the only VectorBackend impl is Qdrant. Test both builds. |
| `upsert_diary_entry` FTS atomicity fix breaks existing tests | Existing tests only observe the committed state, not interim. Transaction wrap is a strict improvement. |
| Broker now holds `Arc<dyn StateStore>` cache — new lifecycle concerns | Cache is an in-memory HashMap; entries live as long as the broker. No external resources to clean up because SQLite Connection is dropped when the Arc is dropped. |

## 9. File inventory

### New files

- `crates/the-one-memory/src/vector_backend.rs` (~250 LOC)
- `crates/the-one-core/src/state_store.rs` (~300 LOC)
- `crates/the-one-mcp/tests/backend_traits.rs` (~200 LOC, new integration tests)

### Modified files

- `crates/the-one-memory/src/lib.rs` (~±400 LOC: field swap + dispatch updates)
- `crates/the-one-memory/src/qdrant.rs` (+100 LOC: trait impl block)
- `crates/the-one-memory/src/redis_vectors.rs` (+60 LOC: trait impl block)
- `crates/the-one-core/src/lib.rs` (+1 module export)
- `crates/the-one-core/src/storage/sqlite.rs` (+150 LOC: `impl StateStore for ProjectDatabase` + diary atomicity fix)
- `crates/the-one-mcp/src/broker.rs` (~±400 LOC: state_by_project cache + call site migration)
- `docs/plans/2026-04-11-next-steps-expansion.md` (scope section)
- `CHANGELOG.md` (v0.16.0-rc1 entry)
- `CLAUDE.md` (new conventions)

### Total LOC delta
Roughly +2 000 / -700 LOC. Net +1 300 LOC for the trait extraction.
Most of that is the new files; the dispatch sites become shorter.
