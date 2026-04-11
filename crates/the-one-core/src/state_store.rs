//! Backend-agnostic relational state trait (Phase A2, v0.16.0).
//!
//! Prior to v0.16.0, the broker held a `ProjectDatabase` (SQLite) directly
//! and called its concrete methods everywhere. This module abstracts those
//! 18+ methods behind [`StateStore`] so alternative backends (Postgres,
//! Redis-with-AOF, combined Postgres+pgvector, combined Redis+RediSearch)
//! can be plugged in without touching the broker.
//!
//! ## Design principles
//!
//! 1. **Zero behaviour change for SQLite.** The existing `ProjectDatabase`
//!    implements this trait verbatim; every method forwards to the existing
//!    inherent method. Tests pass bit-for-bit.
//!
//! 2. **Trait lives in `the-one-core`** alongside `ProjectDatabase` itself
//!    so there's no circular crate dependency. Downstream crates (Postgres
//!    impl, Redis impl) depend on `the-one-core` and implement the trait
//!    in their own module.
//!
//! 3. **All methods return [`CoreError`].** The existing `ProjectDatabase`
//!    API already uses `CoreError` for every pub method — no rusqlite
//!    types leak in signatures, so the trait extraction is clean.
//!
//! 4. **Paginated methods use [`PageRequest`] / [`Page<T>`].** Already the
//!    convention in v0.15.0; no change here.
//!
//! 5. **Capability reporting** via [`StateStoreCapabilities`] so callers
//!    can detect which features each backend supports (FTS5, transactions,
//!    durability) and degrade gracefully.
//!
//! ## What this trait does NOT expose
//!
//! - `PRAGMA` introspection (`journal_mode`, `synchronous_mode`) — SQLite
//!   specific. Tests that check these call the concrete `ProjectDatabase`
//!   directly.
//! - `db_path()` — SQLite-specific concept. A Postgres impl has a
//!   connection URL instead.
//! - Raw rusqlite `Connection` access — intentionally walled off.

use crate::audit::AuditRecord;
use crate::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationTunnel,
};
use crate::error::CoreError;
use crate::pagination::{Page, PageRequest};
use crate::storage::sqlite::{AuditEvent, ConversationSourceRecord};

/// Static capability report for a state-store backend.
#[derive(Debug, Clone, Copy)]
pub struct StateStoreCapabilities {
    /// Short name for logs and metrics ("sqlite", "postgres", "redis-aof").
    pub name: &'static str,
    /// Backend supports full-text search on diary entries.
    /// SQLite uses FTS5; Postgres uses tsvector/tsquery. Redis impls may
    /// set this to false and fall back to LIKE-style matching.
    pub fts: bool,
    /// Backend supports multi-statement ACID transactions. True for SQLite,
    /// Postgres. False for pure Redis (Redis only supports MULTI/EXEC which
    /// is more limited).
    pub transactions: bool,
    /// Backend persists data durably (WAL, AOF, remote commit, etc.).
    pub durable: bool,
    /// Backend reports its schema version. SQLite uses
    /// `schema_migrations` table; Postgres/Redis backends should mirror
    /// this so the broker can refuse to boot against an unsupported
    /// version.
    pub schema_versioned: bool,
}

impl StateStoreCapabilities {
    /// Helper for the SQLite backend — every capability is true.
    pub const fn sqlite() -> Self {
        Self {
            name: "sqlite",
            fts: true,
            transactions: true,
            durable: true,
            schema_versioned: true,
        }
    }
}

/// Unified interface for any project-state persistence backend.
///
/// Implemented today by [`crate::storage::sqlite::ProjectDatabase`]; designed
/// so Postgres and Redis-with-AOF backends can be added as new files without
/// touching the broker.
///
/// # Method ordering
///
/// Methods are grouped by domain: metadata, profiles, approvals, audit,
/// conversation sources, AAAK, diary, navigation. The trait does not
/// expose every `ProjectDatabase` method — only the ones called from the
/// broker or tests. Observability/debug methods like `audit_event_count`
/// stay as inherent impls on `ProjectDatabase` since they don't need to
/// be cross-backend.
///
/// # Send only, not Sync
///
/// `rusqlite::Connection` is `Send + !Sync`, so the trait only requires
/// `Send`. The broker wraps each `Arc<dyn StateStore>` in a
/// `Mutex` (or owns it exclusively per project) to get cross-task
/// sharing. Backends that CAN be `Sync` (e.g. a connection-pool wrapper)
/// are free to implement `Sync` additionally — callers can't observe the
/// difference through this trait alone.
pub trait StateStore: Send {
    // ── Metadata ───────────────────────────────────────────────────────

    /// Return the project identifier this store is scoped to.
    fn project_id(&self) -> &str;

    /// Return the schema version currently applied to the backend.
    fn schema_version(&self) -> Result<i64, CoreError>;

    /// Static capability report.
    fn capabilities(&self) -> StateStoreCapabilities;

    // ── Project profiles ───────────────────────────────────────────────

    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError>;
    fn latest_project_profile(&self) -> Result<Option<String>, CoreError>;

    // ── Approvals ──────────────────────────────────────────────────────

    fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError>;
    fn is_approved(&self, action_key: &str, scope: ApprovalScope) -> Result<bool, CoreError>;

    // ── Audit ──────────────────────────────────────────────────────────

    /// Legacy entry point — writes `outcome='unknown'` for back-compat.
    /// New code should prefer [`StateStore::record_audit`].
    fn record_audit_event(&self, event_type: &str, payload_json: &str) -> Result<(), CoreError>;

    /// Preferred structured audit write (v0.15.0+).
    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError>;

    fn audit_event_count_for_project(&self) -> Result<u64, CoreError>;

    fn list_audit_events_paged(&self, req: &PageRequest) -> Result<Page<AuditEvent>, CoreError>;

    /// Legacy non-paginated wrapper (delegates to paginated).
    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError>;

    // ── Conversation sources ───────────────────────────────────────────

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

    // ── AAAK lessons ───────────────────────────────────────────────────

    fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError>;

    fn list_aaak_lessons(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError>;

    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError>;

    // ── Diary ──────────────────────────────────────────────────────────

    /// Upsert a diary entry. Implementations that support ACID transactions
    /// MUST write the main entry and any FTS index atomically (v0.15.0
    /// production hardening fix for SQLite: wrap the DELETE+INSERT pair in
    /// a transaction with the main INSERT).
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

    // ── Navigation ─────────────────────────────────────────────────────

    fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError>;

    fn get_navigation_node(&self, node_id: &str)
        -> Result<Option<MemoryNavigationNode>, CoreError>;

    fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError>;

    fn upsert_navigation_tunnel(&self, tunnel: &MemoryNavigationTunnel) -> Result<(), CoreError>;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_capabilities_reports_everything_true() {
        let caps = StateStoreCapabilities::sqlite();
        assert_eq!(caps.name, "sqlite");
        assert!(caps.fts);
        assert!(caps.transactions);
        assert!(caps.durable);
        assert!(caps.schema_versioned);
    }
}
