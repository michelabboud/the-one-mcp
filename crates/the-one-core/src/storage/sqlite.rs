use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};

use crate::audit::{AuditOutcome, AuditRecord};
use crate::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationNodeKind,
    MemoryNavigationTunnel,
};
use crate::error::CoreError;
use crate::pagination::{Page, PageRequest};

const CURRENT_SCHEMA_VERSION: i64 = 7;

/// Per-endpoint max page sizes. These are enforced at the broker layer via
/// [`PageRequest::decode`] — every list/search endpoint MUST declare a
/// non-silent cap. Prior to v0.15.0 these were silent `clamp`s; clients had
/// no way to know they had been truncated.
pub mod page_limits {
    pub const AUDIT_EVENTS_MAX: usize = 500;
    pub const AUDIT_EVENTS_DEFAULT: usize = 50;

    pub const CONVERSATION_SOURCES_MAX: usize = 500;
    pub const CONVERSATION_SOURCES_DEFAULT: usize = 50;

    pub const DIARY_ENTRIES_MAX: usize = 500;
    pub const DIARY_ENTRIES_DEFAULT: usize = 20;

    pub const AAAK_LESSONS_MAX: usize = 500;
    pub const AAAK_LESSONS_DEFAULT: usize = 20;

    pub const NAVIGATION_NODES_MAX: usize = 1_000;
    pub const NAVIGATION_NODES_DEFAULT: usize = 100;

    pub const NAVIGATION_TUNNELS_MAX: usize = 1_000;
    pub const NAVIGATION_TUNNELS_DEFAULT: usize = 200;
}

#[derive(Debug)]
pub struct ProjectDatabase {
    project_id: String,
    db_path: PathBuf,
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: i64,
    pub project_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub outcome: String,
    pub error_kind: Option<String>,
    pub created_at_epoch_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSourceRecord {
    pub project_id: String,
    pub transcript_path: String,
    pub memory_path: String,
    pub format: String,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
    pub message_count: usize,
}

impl ProjectDatabase {
    pub fn open(project_root: &Path, project_id: &str) -> Result<Self, CoreError> {
        if !project_root.exists() || !project_root.is_dir() {
            return Err(CoreError::InvalidProjectConfig(format!(
                "invalid project root: {}",
                project_root.display()
            )));
        }

        let state_dir = project_root.join(".the-one");
        fs::create_dir_all(&state_dir)?;
        let db_path = state_dir.join("state.db");

        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // v0.15.1 (production hardening Lever 1): switch from the default
        // `synchronous=FULL` to `synchronous=NORMAL`. In WAL mode this means
        // fsync() is called only at checkpoint time, not on every commit.
        //
        // Durability trade-off:
        // - Process crash: SAFE. The WAL file captures every committed
        //   transaction; recovery on reopen reads it back.
        // - OS crash or power loss: the last few committed transactions
        //   that are still in the OS page cache (typically < 1s of writes)
        //   can be lost.
        //
        // For the-one-mcp's workload (audit logs, diary entries, navigation
        // nodes) this is the right trade-off: losing < 1s of writes on a
        // power-cut is acceptable because the data that would have been
        // audited lives in the same page-cache window, so the audit and
        // the data stay consistent. Process crashes — by far the more
        // common failure mode — remain fully safe.
        //
        // Measured impact (crates/the-one-core/examples/production_hardening_bench.rs):
        //   record_audit: 5.33ms/row  →  ~100-500µs/row  (10-50×)
        //
        // See docs/guides/production-hardening-v0.15.md § Lever 1 for the
        // rationale and the full trade-off analysis.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        run_migrations(&conn)?;

        Ok(Self {
            project_id: project_id.to_string(),
            db_path,
            conn,
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn journal_mode(&self) -> Result<String, CoreError> {
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        Ok(mode)
    }

    /// Return the current `PRAGMA synchronous` setting as the integer SQLite
    /// uses internally: 0=OFF, 1=NORMAL, 2=FULL, 3=EXTRA. v0.15.1 expects
    /// NORMAL (1) because FULL's per-commit fsync made audit writes 10-50×
    /// slower than necessary in WAL mode.
    pub fn synchronous_mode(&self) -> Result<i64, CoreError> {
        let mode: i64 = self
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        Ok(mode)
    }

    pub fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO project_profiles(project_id, profile_json, updated_at_epoch_ms)
            VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ON CONFLICT(project_id)
            DO UPDATE SET
                profile_json = excluded.profile_json,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![self.project_id, profile_json],
        )?;
        Ok(())
    }

    pub fn profile_count(&self) -> Result<i64, CoreError> {
        let count = self
            .conn
            .query_row("SELECT COUNT(*) FROM project_profiles", [], |row| {
                row.get(0)
            })?;
        Ok(count)
    }

    pub fn latest_project_profile(&self) -> Result<Option<String>, CoreError> {
        let mut stmt = self.conn.prepare(
            "
            SELECT profile_json
            FROM project_profiles
            WHERE project_id = ?1
            LIMIT 1
            ",
        )?;
        let mut rows = stmt.query([&self.project_id])?;
        if let Some(row) = rows.next()? {
            let profile: String = row.get(0)?;
            return Ok(Some(profile));
        }
        Ok(None)
    }

    pub fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO approvals(project_id, action_key, scope, approved, created_at_epoch_ms)
            VALUES (?1, ?2, ?3, ?4, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ON CONFLICT(project_id, action_key, scope)
            DO UPDATE SET
                approved = excluded.approved,
                created_at_epoch_ms = excluded.created_at_epoch_ms
            ",
            params![
                self.project_id,
                action_key,
                approval_scope_to_str(scope),
                if approved { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    pub fn is_approved(&self, action_key: &str, scope: ApprovalScope) -> Result<bool, CoreError> {
        let mut stmt = self.conn.prepare(
            "
            SELECT approved
            FROM approvals
            WHERE project_id = ?1 AND action_key = ?2 AND scope = ?3
            LIMIT 1
            ",
        )?;
        let mut rows = stmt.query(params![
            self.project_id,
            action_key,
            approval_scope_to_str(scope)
        ])?;
        if let Some(row) = rows.next()? {
            let approved: i64 = row.get(0)?;
            return Ok(approved == 1);
        }
        Ok(false)
    }

    pub fn schema_version(&self) -> Result<i64, CoreError> {
        let version = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    /// Record an audit event with a raw event type and payload. Legacy entry
    /// point preserved for back-compat; new code should prefer
    /// [`ProjectDatabase::record_audit`], which enforces the structured
    /// outcome + error_kind columns introduced in schema v7.
    pub fn record_audit_event(
        &self,
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO audit_events(
                project_id, event_type, payload_json, outcome, error_kind, created_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, 'unknown', NULL, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            params![self.project_id, event_type, payload_json],
        )?;
        Ok(())
    }

    /// Record a structured audit entry — the preferred API since v0.15.0.
    ///
    /// Every state-changing broker method should call this exactly once per
    /// attempt, passing the operation name, redacted params, and outcome.
    /// The audit log becomes a stable observability artefact: `outcome='error'`
    /// rows with a given `error_kind` can be counted and alerted on.
    pub fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO audit_events(
                project_id, event_type, payload_json, outcome, error_kind, created_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            params![
                self.project_id,
                record.operation,
                record.params_json,
                record.outcome.as_str(),
                record.error_kind,
            ],
        )?;
        Ok(())
    }

    pub fn audit_event_count(&self) -> Result<i64, CoreError> {
        let count = self
            .conn
            .query_row("SELECT COUNT(*) FROM audit_events", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn audit_event_count_for_project(&self) -> Result<u64, CoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE project_id = ?1",
            params![self.project_id],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    /// List audit events with cursor-based pagination. Callers MUST supply
    /// a validated [`PageRequest`] — the SQL layer no longer silently
    /// truncates. Construct the request via
    /// `PageRequest::decode(limit, cursor, AUDIT_EVENTS_DEFAULT, AUDIT_EVENTS_MAX)`.
    pub fn list_audit_events_paged(
        &self,
        req: &PageRequest,
    ) -> Result<Page<AuditEvent>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let mut stmt = self.conn.prepare(
            "
            SELECT id, project_id, event_type, payload_json, outcome, error_kind, created_at_epoch_ms
            FROM audit_events
            WHERE project_id = ?1
            ORDER BY id DESC
            LIMIT ?2 OFFSET ?3
            ",
        )?;

        let mut rows = stmt.query(params![self.project_id, fetch, offset])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            events.push(AuditEvent {
                id: row.get(0)?,
                project_id: row.get(1)?,
                event_type: row.get(2)?,
                payload_json: row.get(3)?,
                outcome: row.get(4)?,
                error_kind: row.get(5)?,
                created_at_epoch_ms: row.get(6)?,
            });
        }

        let total = self.audit_event_count_for_project()?;
        Ok(Page::from_peek(events, req.limit, req.offset, Some(total)))
    }

    /// Legacy, non-paginated list_audit_events — retained for back-compat
    /// with tests written against v0.14.x. New code should use
    /// [`list_audit_events_paged`]. Unlike the v0.14.x version this does
    /// NOT silently truncate — it returns whatever the caller asked for, up
    /// to the declared page max.
    pub fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::AUDIT_EVENTS_DEFAULT,
            page_limits::AUDIT_EVENTS_MAX,
        )?;
        let page = self.list_audit_events_paged(&req)?;
        Ok(page.items)
    }

    /// Count audit events for this project that match a given outcome.
    /// Used by observability dashboards to track error rates per operation.
    pub fn audit_outcome_count(&self, outcome: AuditOutcome) -> Result<u64, CoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE project_id = ?1 AND outcome = ?2",
            params![self.project_id, outcome.as_str()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    pub fn upsert_conversation_source(
        &self,
        record: &ConversationSourceRecord,
    ) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO conversation_sources(
                project_id,
                transcript_path,
                memory_path,
                format,
                wing,
                hall,
                room,
                message_count,
                updated_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ON CONFLICT(project_id, transcript_path)
            DO UPDATE SET
                memory_path = excluded.memory_path,
                format = excluded.format,
                wing = excluded.wing,
                hall = excluded.hall,
                room = excluded.room,
                message_count = excluded.message_count,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![
                self.project_id,
                record.transcript_path,
                record.memory_path,
                record.format,
                record.wing,
                record.hall,
                record.room,
                record.message_count as i64,
            ],
        )?;
        Ok(())
    }

    pub fn list_conversation_sources(
        &self,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSourceRecord>, CoreError> {
        let safe_limit = limit.max(1).min(i64::MAX as usize) as i64;
        let mut sql = String::from(
            "
            SELECT project_id, transcript_path, memory_path, format, wing, hall, room, message_count
            FROM conversation_sources
            WHERE project_id = ?1
            ",
        );
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;

        if let Some(wing) = wing {
            sql.push_str(&format!(" AND wing = ?{bind_index}"));
            bind_values.push(wing.to_string().into());
            bind_index += 1;
        }
        if let Some(hall) = hall {
            sql.push_str(&format!(" AND hall = ?{bind_index}"));
            bind_values.push(hall.to_string().into());
            bind_index += 1;
        }
        if let Some(room) = room {
            sql.push_str(&format!(" AND room = ?{bind_index}"));
            bind_values.push(room.to_string().into());
            bind_index += 1;
        }

        sql.push_str(&format!(
            "
            ORDER BY updated_at_epoch_ms DESC, transcript_path ASC
            LIMIT ?{bind_index}
            "
        ));
        bind_values.push(safe_limit.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut query = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut rows = Vec::new();
        while let Some(row) = query.next()? {
            rows.push(ConversationSourceRecord {
                project_id: row.get(0)?,
                transcript_path: row.get(1)?,
                memory_path: row.get(2)?,
                format: row.get(3)?,
                wing: row.get(4)?,
                hall: row.get(5)?,
                room: row.get(6)?,
                message_count: row.get::<_, i64>(7)? as usize,
            });
        }

        Ok(rows)
    }

    pub fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO aaak_lessons(
                lesson_id,
                project_id,
                pattern_key,
                role,
                canonical_text,
                occurrence_count,
                confidence_percent,
                source_transcript_path,
                updated_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(lesson_id)
            DO UPDATE SET
                pattern_key = excluded.pattern_key,
                role = excluded.role,
                canonical_text = excluded.canonical_text,
                occurrence_count = excluded.occurrence_count,
                confidence_percent = excluded.confidence_percent,
                source_transcript_path = excluded.source_transcript_path,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![
                lesson.lesson_id,
                lesson.project_id,
                lesson.pattern_key,
                lesson.role,
                lesson.canonical_text,
                lesson.occurrence_count as i64,
                lesson.confidence_percent as i64,
                lesson.source_transcript_path,
                lesson.updated_at_epoch_ms,
            ],
        )?;
        Ok(())
    }

    /// Upsert a diary entry AND its FTS5 index atomically (v0.16.0 fix).
    ///
    /// Pre-v0.16.0 this method issued three separate `execute()` calls:
    /// INSERT into `diary_entries`, DELETE from `diary_entries_fts`, INSERT
    /// into `diary_entries_fts`. If the process crashed between statements
    /// the FTS index was left out of sync with the main table — entries
    /// would appear in `list_diary_entries` but not `search_diary_entries`,
    /// or vice-versa.
    ///
    /// v0.16.0 wraps all three statements in a single `unchecked_transaction`
    /// so the commit is atomic: either the whole entry lands (main +
    /// re-indexed FTS row) or nothing does. `unchecked_transaction` is the
    /// standard rusqlite pattern for transactions that must outlive a
    /// borrow of `&self` (the alternative, `transaction()`, requires
    /// `&mut self` and returns a guard that holds the borrow — which
    /// doesn't compose with our impl of `StateStore`).
    pub fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError> {
        if entry.project_id != self.project_id {
            return Err(CoreError::InvalidRequest(format!(
                "diary entry project_id {} does not match database project {}",
                entry.project_id, self.project_id
            )));
        }

        let tags_json = serde_json::to_string(&entry.tags)?;
        let tags_search_text = entry.tags.join(" ");

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "
            INSERT INTO diary_entries(
                entry_id,
                project_id,
                entry_date,
                mood,
                tags_json,
                content,
                created_at_epoch_ms,
                updated_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(project_id, entry_id)
            DO UPDATE SET
                entry_date = excluded.entry_date,
                mood = excluded.mood,
                tags_json = excluded.tags_json,
                content = excluded.content,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![
                entry.entry_id,
                self.project_id,
                entry.entry_date,
                entry.mood,
                tags_json,
                entry.content,
                entry.created_at_epoch_ms,
                entry.updated_at_epoch_ms,
            ],
        )?;
        tx.execute(
            "
            DELETE FROM diary_entries_fts
            WHERE project_id = ?1 AND entry_id = ?2
            ",
            params![self.project_id, entry.entry_id],
        )?;
        tx.execute(
            "
            INSERT INTO diary_entries_fts(project_id, entry_id, entry_date, mood, tags, content)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                self.project_id,
                entry.entry_id,
                entry.entry_date,
                entry.mood,
                tags_search_text,
                entry.content,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Legacy list_diary_entries — retained for back-compat with v0.14.x.
    /// Unlike the v0.14.x version this no longer silently truncates: the
    /// limit is validated against `page_limits::DIARY_ENTRIES_MAX` and
    /// invalid requests return `InvalidRequest`.
    pub fn list_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::DIARY_ENTRIES_DEFAULT,
            page_limits::DIARY_ENTRIES_MAX,
        )?;
        Ok(self
            .list_diary_entries_paged(start_date, end_date, &req)?
            .items)
    }

    /// Cursor-paginated variant — preferred entry point since v0.15.0.
    pub fn list_diary_entries_paged(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<DiaryEntry>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let mut sql = String::from(
            "
            SELECT entry_id, project_id, entry_date, mood, tags_json, content,
                   created_at_epoch_ms, updated_at_epoch_ms
            FROM diary_entries
            WHERE project_id = ?1
            ",
        );
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;

        if let Some(start_date) = start_date {
            sql.push_str(&format!(" AND entry_date >= ?{bind_index}"));
            bind_values.push(start_date.to_string().into());
            bind_index += 1;
        }
        if let Some(end_date) = end_date {
            sql.push_str(&format!(" AND entry_date <= ?{bind_index}"));
            bind_values.push(end_date.to_string().into());
            bind_index += 1;
        }

        sql.push_str(&format!(
            "
            ORDER BY entry_date DESC, updated_at_epoch_ms DESC, entry_id ASC
            LIMIT ?{bind_index} OFFSET ?{}
            ",
            bind_index + 1
        ));
        bind_values.push(fetch.into());
        bind_values.push(offset.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            entries.push(diary_entry_from_row(row)?);
        }

        let total = self.count_diary_entries(start_date, end_date)?;
        Ok(Page::from_peek(entries, req.limit, req.offset, Some(total)))
    }

    fn count_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
    ) -> Result<u64, CoreError> {
        let mut sql = String::from("SELECT COUNT(*) FROM diary_entries WHERE project_id = ?1");
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;
        if let Some(start_date) = start_date {
            sql.push_str(&format!(" AND entry_date >= ?{bind_index}"));
            bind_values.push(start_date.to_string().into());
            bind_index += 1;
        }
        if let Some(end_date) = end_date {
            sql.push_str(&format!(" AND entry_date <= ?{bind_index}"));
            bind_values.push(end_date.to_string().into());
        }
        let count: i64 =
            self.conn
                .query_row(&sql, params_from_iter(bind_values.iter()), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn search_diary_entries(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        self.search_diary_entries_in_range(query, None, None, limit)
    }

    /// Legacy search_diary_entries_in_range — validates the limit and
    /// delegates to the paginated implementation.
    pub fn search_diary_entries_in_range(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::DIARY_ENTRIES_DEFAULT,
            page_limits::DIARY_ENTRIES_MAX,
        )?;
        Ok(self
            .search_diary_entries_paged(query, start_date, end_date, &req)?
            .items)
    }

    /// Paginated search for diary entries. Uses FTS5 when available and falls
    /// back to LIKE on FTS5 failures.
    pub fn search_diary_entries_paged(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<DiaryEntry>, CoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Page::final_page(Vec::new(), Some(0)));
        }

        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;

        let raw =
            match self.search_diary_entries_with_fts(query, start_date, end_date, fetch, offset) {
                Ok(entries) => entries,
                Err(CoreError::Sqlite(_)) => {
                    self.search_diary_entries_with_like(query, start_date, end_date, fetch, offset)?
                }
                Err(other) => return Err(other),
            };

        Ok(Page::from_peek(raw, req.limit, req.offset, None))
    }

    fn search_diary_entries_with_fts(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let mut sql = String::from(
            "
            SELECT d.entry_id, d.project_id, d.entry_date, d.mood, d.tags_json, d.content,
                   d.created_at_epoch_ms, d.updated_at_epoch_ms
            FROM diary_entries_fts
            JOIN diary_entries d
              ON d.project_id = diary_entries_fts.project_id
             AND d.entry_id = diary_entries_fts.entry_id
            WHERE diary_entries_fts MATCH ?1
              AND d.project_id = ?2
            ",
        );
        let mut bind_values: Vec<SqlValue> =
            vec![query.to_string().into(), self.project_id.clone().into()];
        let mut bind_index = 3;

        if let Some(start_date) = start_date {
            sql.push_str(&format!(" AND d.entry_date >= ?{bind_index}"));
            bind_values.push(start_date.to_string().into());
            bind_index += 1;
        }
        if let Some(end_date) = end_date {
            sql.push_str(&format!(" AND d.entry_date <= ?{bind_index}"));
            bind_values.push(end_date.to_string().into());
            bind_index += 1;
        }

        sql.push_str(&format!(
            "
            ORDER BY d.entry_date DESC, d.updated_at_epoch_ms DESC, d.entry_id ASC
            LIMIT ?{bind_index} OFFSET ?{}
            ",
            bind_index + 1
        ));
        bind_values.push(limit.into());
        bind_values.push(offset.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            entries.push(diary_entry_from_row(row)?);
        }
        Ok(entries)
    }

    fn search_diary_entries_with_like(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let mut sql = String::from(
            "
            SELECT entry_id, project_id, entry_date, mood, tags_json, content,
                   created_at_epoch_ms, updated_at_epoch_ms
            FROM diary_entries
            WHERE project_id = ?1
              AND (
                  lower(content) LIKE ?2
                  OR lower(COALESCE(mood, '')) LIKE ?2
                  OR lower(tags_json) LIKE ?2
              )
            ",
        );
        let mut bind_values: Vec<SqlValue> = vec![
            self.project_id.clone().into(),
            format!("%{}%", query.to_ascii_lowercase()).into(),
        ];
        let mut bind_index = 3;

        if let Some(start_date) = start_date {
            sql.push_str(&format!(" AND entry_date >= ?{bind_index}"));
            bind_values.push(start_date.to_string().into());
            bind_index += 1;
        }
        if let Some(end_date) = end_date {
            sql.push_str(&format!(" AND entry_date <= ?{bind_index}"));
            bind_values.push(end_date.to_string().into());
            bind_index += 1;
        }

        sql.push_str(&format!(
            "
            ORDER BY entry_date DESC, updated_at_epoch_ms DESC, entry_id ASC
            LIMIT ?{bind_index} OFFSET ?{}
            ",
            bind_index + 1
        ));
        bind_values.push(limit.into());
        bind_values.push(offset.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            entries.push(diary_entry_from_row(row)?);
        }
        Ok(entries)
    }

    /// Legacy list_aaak_lessons — validates the limit and delegates to the
    /// paginated implementation. Unlike v0.14.x this rejects over-limit
    /// requests instead of silently truncating to 200.
    pub fn list_aaak_lessons(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::AAAK_LESSONS_DEFAULT,
            page_limits::AAAK_LESSONS_MAX,
        )?;
        Ok(self.list_aaak_lessons_paged(project_id, &req)?.items)
    }

    /// Cursor-paginated list of AAAK lessons. Preferred since v0.15.0.
    pub fn list_aaak_lessons_paged(
        &self,
        project_id: &str,
        req: &PageRequest,
    ) -> Result<Page<AaakLesson>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let mut stmt = self.conn.prepare(
            "
            SELECT lesson_id, project_id, pattern_key, role, canonical_text, occurrence_count,
                   confidence_percent, source_transcript_path, updated_at_epoch_ms
            FROM aaak_lessons
            WHERE project_id = ?1
            ORDER BY confidence_percent DESC, occurrence_count DESC, pattern_key ASC
            LIMIT ?2 OFFSET ?3
            ",
        )?;

        let mut rows = stmt.query(params![project_id, fetch, offset])?;
        let mut lessons = Vec::new();
        while let Some(row) = rows.next()? {
            lessons.push(AaakLesson {
                lesson_id: row.get(0)?,
                project_id: row.get(1)?,
                pattern_key: row.get(2)?,
                role: row.get(3)?,
                canonical_text: row.get(4)?,
                occurrence_count: row.get::<_, i64>(5)? as usize,
                confidence_percent: row.get::<_, i64>(6)? as u8,
                source_transcript_path: row.get(7)?,
                updated_at_epoch_ms: row.get(8)?,
            });
        }

        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM aaak_lessons WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;

        Ok(Page::from_peek(
            lessons,
            req.limit,
            req.offset,
            Some(total.max(0) as u64),
        ))
    }

    pub fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError> {
        let deleted = self.conn.execute(
            "
            DELETE FROM aaak_lessons
            WHERE lesson_id = ?1 AND project_id = ?2
            ",
            params![lesson_id, self.project_id],
        )?;
        Ok(deleted > 0)
    }

    pub fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError> {
        self.ensure_navigation_project_scope(&node.project_id)?;
        self.conn.execute(
            "
            INSERT INTO navigation_nodes(
                node_id,
                project_id,
                kind,
                label,
                parent_node_id,
                wing,
                hall,
                room,
                updated_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(project_id, node_id)
            DO UPDATE SET
                kind = excluded.kind,
                label = excluded.label,
                parent_node_id = excluded.parent_node_id,
                wing = excluded.wing,
                hall = excluded.hall,
                room = excluded.room,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![
                node.node_id,
                self.project_id,
                node.kind.as_str(),
                node.label,
                node.parent_node_id,
                node.wing,
                node.hall,
                node.room,
                node.updated_at_epoch_ms,
            ],
        )?;
        Ok(())
    }

    pub fn get_navigation_node(
        &self,
        node_id: &str,
    ) -> Result<Option<MemoryNavigationNode>, CoreError> {
        let mut stmt = self.conn.prepare(
            "
            SELECT node_id, project_id, kind, label, parent_node_id, wing, hall, room,
                   updated_at_epoch_ms
            FROM navigation_nodes
            WHERE project_id = ?1 AND node_id = ?2
            LIMIT 1
            ",
        )?;
        let mut rows = stmt.query(params![self.project_id, node_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(navigation_node_from_row(row)?));
        }
        Ok(None)
    }

    /// Legacy list_navigation_nodes — validates the limit and delegates.
    pub fn list_navigation_nodes(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryNavigationNode>, CoreError> {
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::NAVIGATION_NODES_DEFAULT,
            page_limits::NAVIGATION_NODES_MAX,
        )?;
        Ok(self
            .list_navigation_nodes_paged(parent_node_id, kind, &req)?
            .items)
    }

    /// Cursor-paginated list of navigation nodes. Preferred since v0.15.0.
    pub fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let mut sql = String::from(
            "
            SELECT node_id, project_id, kind, label, parent_node_id, wing, hall, room,
                   updated_at_epoch_ms
            FROM navigation_nodes
            WHERE project_id = ?1
            ",
        );
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;

        if let Some(parent_node_id) = parent_node_id {
            sql.push_str(&format!(" AND parent_node_id = ?{bind_index}"));
            bind_values.push(parent_node_id.to_string().into());
            bind_index += 1;
        }
        if let Some(kind) = kind {
            sql.push_str(&format!(" AND kind = ?{bind_index}"));
            bind_values.push(kind.to_string().into());
            bind_index += 1;
        }

        sql.push_str(&format!(
            " ORDER BY kind ASC, label ASC, node_id ASC LIMIT ?{bind_index} OFFSET ?{}",
            bind_index + 1
        ));
        bind_values.push(fetch.into());
        bind_values.push(offset.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(navigation_node_from_row(row)?);
        }

        let total = self.count_navigation_nodes(parent_node_id, kind)?;
        Ok(Page::from_peek(nodes, req.limit, req.offset, Some(total)))
    }

    fn count_navigation_nodes(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<u64, CoreError> {
        let mut sql = String::from("SELECT COUNT(*) FROM navigation_nodes WHERE project_id = ?1");
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;
        if let Some(parent_node_id) = parent_node_id {
            sql.push_str(&format!(" AND parent_node_id = ?{bind_index}"));
            bind_values.push(parent_node_id.to_string().into());
            bind_index += 1;
        }
        if let Some(kind) = kind {
            sql.push_str(&format!(" AND kind = ?{bind_index}"));
            bind_values.push(kind.to_string().into());
        }
        let count: i64 =
            self.conn
                .query_row(&sql, params_from_iter(bind_values.iter()), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn upsert_navigation_tunnel(
        &self,
        tunnel: &MemoryNavigationTunnel,
    ) -> Result<(), CoreError> {
        self.ensure_navigation_project_scope(&tunnel.project_id)?;
        self.conn.execute(
            "
            INSERT INTO navigation_tunnels(
                tunnel_id,
                project_id,
                from_node_id,
                to_node_id,
                updated_at_epoch_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(project_id, from_node_id, to_node_id)
            DO UPDATE SET
                tunnel_id = excluded.tunnel_id,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms
            ",
            params![
                tunnel.tunnel_id,
                self.project_id,
                tunnel.from_node_id,
                tunnel.to_node_id,
                tunnel.updated_at_epoch_ms,
            ],
        )?;
        Ok(())
    }

    /// Legacy list_navigation_tunnels — DEPRECATED, use
    /// [`list_navigation_tunnels_paged`] or
    /// [`list_navigation_tunnels_for_nodes`] instead. This version used to
    /// return every tunnel in the project with no limit at all; it now
    /// returns at most `NAVIGATION_TUNNELS_MAX` rows and emits a warning.
    pub fn list_navigation_tunnels(
        &self,
        node_id: Option<&str>,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError> {
        let req = PageRequest::decode(
            0,
            None,
            page_limits::NAVIGATION_TUNNELS_MAX,
            page_limits::NAVIGATION_TUNNELS_MAX,
        )?;
        Ok(self.list_navigation_tunnels_paged(node_id, &req)?.items)
    }

    /// Cursor-paginated list of navigation tunnels. Preferred since v0.15.0.
    pub fn list_navigation_tunnels_paged(
        &self,
        node_id: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationTunnel>, CoreError> {
        let fetch = req.fetch_limit() as i64;
        let offset = req.offset as i64;
        let mut sql = String::from(
            "
            SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms
            FROM navigation_tunnels
            WHERE project_id = ?1
            ",
        );
        let mut bind_values: Vec<SqlValue> = vec![self.project_id.clone().into()];
        let mut bind_index = 2;
        if let Some(node_id) = node_id {
            sql.push_str(&format!(
                " AND (from_node_id = ?{bind_index} OR to_node_id = ?{bind_index})"
            ));
            bind_values.push(node_id.to_string().into());
            bind_index += 1;
        }
        sql.push_str(&format!(
            " ORDER BY from_node_id ASC, to_node_id ASC, tunnel_id ASC LIMIT ?{bind_index} OFFSET ?{}",
            bind_index + 1
        ));
        bind_values.push(fetch.into());
        bind_values.push(offset.into());

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        let mut tunnels = Vec::new();
        while let Some(row) = rows.next()? {
            tunnels.push(MemoryNavigationTunnel {
                tunnel_id: row.get(0)?,
                project_id: row.get(1)?,
                from_node_id: row.get(2)?,
                to_node_id: row.get(3)?,
                updated_at_epoch_ms: row.get(4)?,
            });
        }

        let total = self.count_navigation_tunnels(node_id)?;
        Ok(Page::from_peek(tunnels, req.limit, req.offset, Some(total)))
    }

    /// SQL-side filter: fetch every tunnel that touches any of the given
    /// node ids. Used by `memory_navigation_list` to avoid loading every
    /// tunnel in the project and filtering in Rust. The caller supplies an
    /// explicit limit that is validated against
    /// `page_limits::NAVIGATION_TUNNELS_MAX`.
    ///
    /// Implementation: uses a `WHERE from_node_id IN (...) OR to_node_id IN (...)`
    /// clause with dynamically-bound parameters. SQLite's default SQLITE_MAX_VARIABLE_NUMBER
    /// is 999, so we chunk the input by 400 (leaving headroom) and UNION the results.
    pub fn list_navigation_tunnels_for_nodes(
        &self,
        node_ids: &[String],
        limit: usize,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let req = PageRequest::decode(
            limit,
            None,
            page_limits::NAVIGATION_TUNNELS_DEFAULT,
            page_limits::NAVIGATION_TUNNELS_MAX,
        )?;

        // Deduplicate and chunk — avoids SQLITE_MAX_VARIABLE_NUMBER and ensures
        // stable ordering across chunk boundaries.
        let mut unique: Vec<&String> = node_ids.iter().collect();
        unique.sort();
        unique.dedup();

        const CHUNK: usize = 400;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<MemoryNavigationTunnel> = Vec::new();

        for chunk in unique.chunks(CHUNK) {
            let placeholders: String = (0..chunk.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "
                SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms
                FROM navigation_tunnels
                WHERE project_id = ?1
                  AND (from_node_id IN ({ph}) OR to_node_id IN ({ph}))
                ORDER BY from_node_id ASC, to_node_id ASC, tunnel_id ASC
                LIMIT ?{lim}
                ",
                ph = placeholders,
                lim = chunk.len() + 2
            );
            let mut bind_values: Vec<SqlValue> = Vec::with_capacity(chunk.len() + 2);
            bind_values.push(self.project_id.clone().into());
            for id in chunk {
                bind_values.push((*id).clone().into());
            }
            bind_values.push((req.limit as i64).into());

            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
            while let Some(row) = rows.next()? {
                let tunnel = MemoryNavigationTunnel {
                    tunnel_id: row.get(0)?,
                    project_id: row.get(1)?,
                    from_node_id: row.get(2)?,
                    to_node_id: row.get(3)?,
                    updated_at_epoch_ms: row.get(4)?,
                };
                if seen.insert(tunnel.tunnel_id.clone()) {
                    out.push(tunnel);
                }
                if out.len() >= req.limit {
                    break;
                }
            }
            if out.len() >= req.limit {
                break;
            }
        }

        out.sort_by(|a, b| {
            a.from_node_id
                .cmp(&b.from_node_id)
                .then(a.to_node_id.cmp(&b.to_node_id))
                .then(a.tunnel_id.cmp(&b.tunnel_id))
        });
        out.truncate(req.limit);
        Ok(out)
    }

    fn count_navigation_tunnels(&self, node_id: Option<&str>) -> Result<u64, CoreError> {
        let count: i64 = if let Some(node_id) = node_id {
            self.conn.query_row(
                "SELECT COUNT(*) FROM navigation_tunnels
                 WHERE project_id = ?1 AND (from_node_id = ?2 OR to_node_id = ?2)",
                params![self.project_id, node_id],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM navigation_tunnels WHERE project_id = ?1",
                params![self.project_id],
                |row| row.get(0),
            )?
        };
        Ok(count.max(0) as u64)
    }

    fn ensure_navigation_project_scope(&self, project_id: &str) -> Result<(), CoreError> {
        if project_id != self.project_id {
            return Err(CoreError::InvalidRequest(format!(
                "navigation record project_id '{}' does not match database scope '{}'",
                project_id, self.project_id
            )));
        }
        Ok(())
    }
}

fn navigation_kind_from_str(value: &str) -> Result<MemoryNavigationNodeKind, CoreError> {
    match value {
        "drawer" => Ok(MemoryNavigationNodeKind::Drawer),
        "closet" => Ok(MemoryNavigationNodeKind::Closet),
        "room" => Ok(MemoryNavigationNodeKind::Room),
        other => Err(CoreError::InvalidProjectConfig(format!(
            "unsupported navigation node kind: {other}"
        ))),
    }
}

fn navigation_node_from_row(row: &rusqlite::Row<'_>) -> Result<MemoryNavigationNode, CoreError> {
    Ok(MemoryNavigationNode {
        node_id: row.get(0)?,
        project_id: row.get(1)?,
        kind: navigation_kind_from_str(&row.get::<_, String>(2)?)?,
        label: row.get(3)?,
        parent_node_id: row.get(4)?,
        wing: row.get(5)?,
        hall: row.get(6)?,
        room: row.get(7)?,
        updated_at_epoch_ms: row.get(8)?,
    })
}

fn diary_entry_from_row(row: &rusqlite::Row<'_>) -> Result<DiaryEntry, CoreError> {
    let tags_json: String = row.get(4)?;
    Ok(DiaryEntry {
        entry_id: row.get(0)?,
        project_id: row.get(1)?,
        entry_date: row.get(2)?,
        mood: row.get(3)?,
        tags: serde_json::from_str(&tags_json)?,
        content: row.get(5)?,
        created_at_epoch_ms: row.get(6)?,
        updated_at_epoch_ms: row.get(7)?,
    })
}

fn approval_scope_to_str(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Session => "session",
        ApprovalScope::Forever => "forever",
    }
}

fn run_migrations(conn: &Connection) -> Result<(), CoreError> {
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at_epoch_ms INTEGER NOT NULL
        )
        ",
        [],
    )?;

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if current < 1 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS project_profiles (
                project_id TEXT PRIMARY KEY,
                profile_json TEXT NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS approvals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id TEXT NOT NULL,
                action_key TEXT NOT NULL,
                scope TEXT NOT NULL,
                approved INTEGER NOT NULL,
                created_at_epoch_ms INTEGER NOT NULL,
                UNIQUE(project_id, action_key, scope)
            );

            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at_epoch_ms INTEGER NOT NULL
            );
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [1],
        )?;
    }

    if current < 2 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS conversation_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id TEXT NOT NULL,
                transcript_path TEXT NOT NULL,
                memory_path TEXT NOT NULL,
                format TEXT NOT NULL,
                wing TEXT,
                hall TEXT,
                room TEXT,
                message_count INTEGER NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL,
                UNIQUE(project_id, transcript_path)
            );

            CREATE INDEX IF NOT EXISTS idx_conversation_sources_project_wing_updated
            ON conversation_sources(project_id, wing, updated_at_epoch_ms DESC);
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [2],
        )?;
    }

    if current < 3 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS aaak_lessons (
                lesson_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                pattern_key TEXT NOT NULL,
                role TEXT NOT NULL,
                canonical_text TEXT NOT NULL,
                occurrence_count INTEGER NOT NULL,
                confidence_percent INTEGER NOT NULL,
                source_transcript_path TEXT,
                updated_at_epoch_ms INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_aaak_lessons_project_confidence
            ON aaak_lessons(project_id, confidence_percent DESC, occurrence_count DESC);
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [3],
        )?;
    }

    if current < 4 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS navigation_nodes (
                node_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                label TEXT NOT NULL,
                parent_node_id TEXT,
                wing TEXT,
                hall TEXT,
                room TEXT,
                updated_at_epoch_ms INTEGER NOT NULL,
                PRIMARY KEY(project_id, node_id),
                FOREIGN KEY(project_id, parent_node_id)
                    REFERENCES navigation_nodes(project_id, node_id)
                    ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_navigation_nodes_project_parent_kind_label
            ON navigation_nodes(project_id, parent_node_id, kind, label, node_id);

            CREATE TABLE IF NOT EXISTS navigation_tunnels (
                tunnel_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                from_node_id TEXT NOT NULL,
                to_node_id TEXT NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL,
                CHECK (from_node_id <> to_node_id),
                PRIMARY KEY(project_id, tunnel_id),
                FOREIGN KEY(project_id, from_node_id)
                    REFERENCES navigation_nodes(project_id, node_id)
                    ON DELETE CASCADE,
                FOREIGN KEY(project_id, to_node_id)
                    REFERENCES navigation_nodes(project_id, node_id)
                    ON DELETE CASCADE,
                UNIQUE(project_id, from_node_id, to_node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_navigation_tunnels_project_endpoints
            ON navigation_tunnels(project_id, from_node_id, to_node_id, tunnel_id);
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [4],
        )?;
    }

    if current < 5 {
        conn.execute_batch(
            "
            PRAGMA foreign_keys = OFF;

            CREATE TABLE navigation_nodes_v2 (
                node_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                label TEXT NOT NULL,
                parent_node_id TEXT,
                wing TEXT,
                hall TEXT,
                room TEXT,
                updated_at_epoch_ms INTEGER NOT NULL,
                PRIMARY KEY(project_id, node_id),
                FOREIGN KEY(project_id, parent_node_id)
                    REFERENCES navigation_nodes_v2(project_id, node_id)
                    ON DELETE CASCADE
            );

            INSERT INTO navigation_nodes_v2(
                node_id, project_id, kind, label, parent_node_id, wing, hall, room,
                updated_at_epoch_ms
            )
            SELECT node_id, project_id, kind, label, parent_node_id, wing, hall, room,
                   updated_at_epoch_ms
            FROM navigation_nodes;

            CREATE TABLE navigation_tunnels_v2 (
                tunnel_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                from_node_id TEXT NOT NULL,
                to_node_id TEXT NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL,
                CHECK (from_node_id <> to_node_id),
                PRIMARY KEY(project_id, tunnel_id),
                FOREIGN KEY(project_id, from_node_id)
                    REFERENCES navigation_nodes_v2(project_id, node_id)
                    ON DELETE CASCADE,
                FOREIGN KEY(project_id, to_node_id)
                    REFERENCES navigation_nodes_v2(project_id, node_id)
                    ON DELETE CASCADE,
                UNIQUE(project_id, from_node_id, to_node_id)
            );

            INSERT OR IGNORE INTO navigation_tunnels_v2(
                tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms
            )
            SELECT tunnel_id, project_id, from_node_id, to_node_id, updated_at_epoch_ms
            FROM navigation_tunnels;

            DROP TABLE navigation_tunnels;
            DROP TABLE navigation_nodes;

            ALTER TABLE navigation_nodes_v2 RENAME TO navigation_nodes;
            ALTER TABLE navigation_tunnels_v2 RENAME TO navigation_tunnels;

            CREATE INDEX IF NOT EXISTS idx_navigation_nodes_project_parent_kind_label
            ON navigation_nodes(project_id, parent_node_id, kind, label, node_id);

            CREATE INDEX IF NOT EXISTS idx_navigation_tunnels_project_endpoints
            ON navigation_tunnels(project_id, from_node_id, to_node_id, tunnel_id);

            PRAGMA foreign_keys = ON;
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [5],
        )?;
    }

    if current < 6 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS diary_entries (
                entry_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                entry_date TEXT NOT NULL,
                mood TEXT,
                tags_json TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at_epoch_ms INTEGER NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL,
                PRIMARY KEY(project_id, entry_id)
            );

            CREATE INDEX IF NOT EXISTS idx_diary_entries_project_date
            ON diary_entries(project_id, entry_date DESC, updated_at_epoch_ms DESC, entry_id);

            CREATE VIRTUAL TABLE IF NOT EXISTS diary_entries_fts USING fts5(
                project_id UNINDEXED,
                entry_id UNINDEXED,
                entry_date UNINDEXED,
                mood,
                tags,
                content
            );
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [6],
        )?;
    }

    if current < 7 {
        // Schema v7 (v0.15.0 production hardening): add structured outcome
        // columns to audit_events so every state-changing broker operation
        // can record a proper AuditRecord instead of opaque payload_json.
        //
        // Both new columns are nullable / default so existing rows from
        // v0.14.x keep working — they get `outcome='unknown'` and
        // `error_kind=NULL` which downstream dashboards can filter out.
        //
        // We also add an index on (project_id, outcome) so error-rate queries
        // stay cheap as the audit log grows.
        conn.execute_batch(
            "
            ALTER TABLE audit_events ADD COLUMN outcome TEXT NOT NULL DEFAULT 'unknown';
            ALTER TABLE audit_events ADD COLUMN error_kind TEXT;

            CREATE INDEX IF NOT EXISTS idx_audit_events_project_outcome
            ON audit_events(project_id, outcome, created_at_epoch_ms DESC);

            CREATE INDEX IF NOT EXISTS idx_audit_events_project_event
            ON audit_events(project_id, event_type, created_at_epoch_ms DESC);
            ",
        )?;

        conn.execute(
            "
            INSERT INTO schema_migrations(version, applied_at_epoch_ms)
            VALUES (?1, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            [7],
        )?;
    }

    if current > CURRENT_SCHEMA_VERSION {
        return Err(CoreError::UnsupportedSchemaVersion(current.to_string()));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// StateStore trait impl (v0.16.0 Phase A2)
// ---------------------------------------------------------------------------
//
// Forwards every trait method to the corresponding inherent method on
// `ProjectDatabase`. Zero behaviour change for SQLite — the trait exists so
// alternative backends (Postgres, Redis-with-AOF) can be swapped in at the
// broker level without touching the 18+ call sites.

use crate::state_store::{StateStore, StateStoreCapabilities};

impl StateStore for ProjectDatabase {
    fn project_id(&self) -> &str {
        ProjectDatabase::project_id(self)
    }

    fn schema_version(&self) -> Result<i64, CoreError> {
        ProjectDatabase::schema_version(self)
    }

    fn capabilities(&self) -> StateStoreCapabilities {
        StateStoreCapabilities::sqlite()
    }

    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError> {
        ProjectDatabase::upsert_project_profile(self, profile_json)
    }

    fn latest_project_profile(&self) -> Result<Option<String>, CoreError> {
        ProjectDatabase::latest_project_profile(self)
    }

    fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError> {
        ProjectDatabase::set_approval(self, action_key, scope, approved)
    }

    fn is_approved(&self, action_key: &str, scope: ApprovalScope) -> Result<bool, CoreError> {
        ProjectDatabase::is_approved(self, action_key, scope)
    }

    fn record_audit_event(&self, event_type: &str, payload_json: &str) -> Result<(), CoreError> {
        ProjectDatabase::record_audit_event(self, event_type, payload_json)
    }

    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError> {
        ProjectDatabase::record_audit(self, record)
    }

    fn audit_event_count_for_project(&self) -> Result<u64, CoreError> {
        ProjectDatabase::audit_event_count_for_project(self)
    }

    fn list_audit_events_paged(&self, req: &PageRequest) -> Result<Page<AuditEvent>, CoreError> {
        ProjectDatabase::list_audit_events_paged(self, req)
    }

    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        ProjectDatabase::list_audit_events(self, limit)
    }

    fn upsert_conversation_source(
        &self,
        record: &ConversationSourceRecord,
    ) -> Result<(), CoreError> {
        ProjectDatabase::upsert_conversation_source(self, record)
    }

    fn list_conversation_sources(
        &self,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSourceRecord>, CoreError> {
        ProjectDatabase::list_conversation_sources(self, wing, hall, room, limit)
    }

    fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError> {
        ProjectDatabase::upsert_aaak_lesson(self, lesson)
    }

    fn list_aaak_lessons(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError> {
        ProjectDatabase::list_aaak_lessons(self, project_id, limit)
    }

    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError> {
        ProjectDatabase::delete_aaak_lesson(self, lesson_id)
    }

    fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError> {
        ProjectDatabase::upsert_diary_entry(self, entry)
    }

    fn list_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        ProjectDatabase::list_diary_entries(self, start_date, end_date, limit)
    }

    fn search_diary_entries_in_range(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        ProjectDatabase::search_diary_entries_in_range(self, query, start_date, end_date, limit)
    }

    fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError> {
        ProjectDatabase::upsert_navigation_node(self, node)
    }

    fn get_navigation_node(
        &self,
        node_id: &str,
    ) -> Result<Option<MemoryNavigationNode>, CoreError> {
        ProjectDatabase::get_navigation_node(self, node_id)
    }

    fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError> {
        ProjectDatabase::list_navigation_nodes_paged(self, parent_node_id, kind, req)
    }

    fn upsert_navigation_tunnel(&self, tunnel: &MemoryNavigationTunnel) -> Result<(), CoreError> {
        ProjectDatabase::upsert_navigation_tunnel(self, tunnel)
    }

    fn list_navigation_tunnels_paged(
        &self,
        node_id: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationTunnel>, CoreError> {
        ProjectDatabase::list_navigation_tunnels_paged(self, node_id, req)
    }

    fn list_navigation_tunnels_for_nodes(
        &self,
        node_ids: &[String],
        limit: usize,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError> {
        ProjectDatabase::list_navigation_tunnels_for_nodes(self, node_ids, limit)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::contracts::{
        AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationNodeKind,
        MemoryNavigationTunnel,
    };
    use crate::error::CoreError;

    use super::{ConversationSourceRecord, ProjectDatabase};

    #[test]
    fn test_project_db_uses_wal_and_migrates() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");

        assert_eq!(db.project_id(), "project-1");
        assert!(db
            .db_path()
            .to_string_lossy()
            .ends_with(".the-one/state.db"));
        assert_eq!(db.journal_mode().expect("journal mode query"), "wal");
        assert_eq!(db.schema_version().expect("schema version query"), 7);
        // v0.15.1 Lever 1: synchronous must be NORMAL (1) in WAL mode. If
        // this regresses to FULL (2) or EXTRA (3), audit-log write
        // throughput drops by 10-50×. See production-hardening-v0.15.md.
        assert_eq!(
            db.synchronous_mode().expect("synchronous mode query"),
            1,
            "expected PRAGMA synchronous=NORMAL (1) after v0.15.1 Lever 1"
        );
    }

    #[test]
    fn test_audit_write_throughput_under_normal_sync() {
        // Smoke test for v0.15.1 Lever 1: writes should complete in a
        // realistic wall-clock budget. We don't assert a hard latency floor
        // (CI runners vary) but we confirm 100 audit writes finish under
        // 5s — which would be impossible at 5ms/row under FULL sync
        // because that alone would take 0.5s just on fsync, plus overhead.
        //
        // This is a behavioral guard against accidentally switching back
        // to synchronous=FULL. A pure pragma check (above) would miss the
        // case where the pragma is set but ignored for some reason.
        use std::time::Instant;
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir");
        let db = ProjectDatabase::open(&project_root, "project-1").expect("db");

        let t0 = Instant::now();
        for i in 0..100 {
            db.record_audit_event("tool_run", &format!("{{\"i\":{i}}}"))
                .expect("audit write");
        }
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_secs() < 5,
            "100 audit writes took {elapsed:?} — suspiciously slow, \
             did synchronous=NORMAL regress?"
        );
    }

    #[test]
    fn test_project_db_isolation_across_projects() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_a = temp.path().join("repo-a");
        let project_b = temp.path().join("repo-b");

        fs::create_dir_all(&project_a).expect("project a dir should exist");
        fs::create_dir_all(&project_b).expect("project b dir should exist");

        let db_a = ProjectDatabase::open(&project_a, "project-a").expect("db a should open");
        let db_b = ProjectDatabase::open(&project_b, "project-b").expect("db b should open");

        db_a.upsert_project_profile("{\"name\":\"a\"}")
            .expect("insert a should succeed");

        assert_eq!(db_a.profile_count().expect("count a"), 1);
        assert_eq!(db_b.profile_count().expect("count b"), 0);
        assert_ne!(db_a.db_path(), db_b.db_path());
    }

    #[test]
    fn test_approval_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.set_approval("tool.run:danger", ApprovalScope::Forever, true)
            .expect("approval write should succeed");

        let approved = db
            .is_approved("tool.run:danger", ApprovalScope::Forever)
            .expect("approval read should succeed");
        assert!(approved);
    }

    #[test]
    fn test_audit_event_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.record_audit_event("tool_run", "{\"allowed\":true}")
            .expect("audit write should succeed");

        assert_eq!(db.audit_event_count().expect("count should work"), 1);

        let events = db.list_audit_events(10).expect("list should work");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "tool_run");
        // Legacy record_audit_event writes 'unknown' for back-compat.
        assert_eq!(events[0].outcome, "unknown");
        assert!(events[0].error_kind.is_none());
    }

    #[test]
    fn test_audit_record_structured_roundtrip() {
        use crate::audit::{AuditOutcome, AuditRecord};
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");

        db.record_audit(&AuditRecord::ok("docs.create", "{\"path\":\"notes/a.md\"}"))
            .expect("ok audit write should succeed");

        let err = CoreError::InvalidRequest("nope".into());
        db.record_audit(&AuditRecord::error("docs.create", "{\"path\":\"\"}", &err))
            .expect("error audit write should succeed");

        let events = db.list_audit_events(10).expect("list should work");
        assert_eq!(events.len(), 2);
        // Most-recent first.
        assert_eq!(events[0].outcome, "error");
        assert_eq!(events[0].error_kind.as_deref(), Some("invalid_request"));
        assert_eq!(events[1].outcome, "ok");
        assert!(events[1].error_kind.is_none());

        assert_eq!(
            db.audit_outcome_count(AuditOutcome::Ok).unwrap(),
            1,
            "one ok"
        );
        assert_eq!(
            db.audit_outcome_count(AuditOutcome::Error).unwrap(),
            1,
            "one error"
        );
    }

    #[test]
    fn test_audit_events_pagination() {
        use crate::pagination::{Cursor, PageRequest};
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).unwrap();
        let db = ProjectDatabase::open(&project_root, "project-1").unwrap();
        for i in 0..120 {
            db.record_audit_event("tool_run", &format!("{{\"i\":{i}}}"))
                .unwrap();
        }

        let req = PageRequest::decode(50, None, 50, 500).unwrap();
        let page = db.list_audit_events_paged(&req).unwrap();
        assert_eq!(page.items.len(), 50);
        assert!(page.next_cursor.is_some());
        assert_eq!(page.total_count, Some(120));

        let next_cursor = page.next_cursor.unwrap();
        let req2 = PageRequest::decode(50, Some(next_cursor.as_str()), 50, 500).unwrap();
        let page2 = db.list_audit_events_paged(&req2).unwrap();
        assert_eq!(page2.items.len(), 50);
        assert!(page2.next_cursor.is_some());

        let (off, _) = Cursor::decode(page2.next_cursor.unwrap().as_str()).unwrap();
        assert_eq!(off, 100);

        let req3 = PageRequest::decode(50, Some(&Cursor::from_offset(100).0), 50, 500).unwrap();
        let page3 = db.list_audit_events_paged(&req3).unwrap();
        assert_eq!(page3.items.len(), 20);
        assert!(page3.next_cursor.is_none());
    }

    #[test]
    fn test_list_audit_events_rejects_over_limit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).unwrap();
        let db = ProjectDatabase::open(&project_root, "project-1").unwrap();
        // AUDIT_EVENTS_MAX = 500 — any bigger number must be rejected, not
        // silently truncated.
        let err = db.list_audit_events(5_000).unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }

    #[test]
    fn test_conversation_source_metadata_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_conversation_source(&ConversationSourceRecord {
            project_id: "project-1".to_string(),
            transcript_path: "/tmp/transcript.json".to_string(),
            memory_path: "/tmp/conversations/transcript.md".to_string(),
            format: "openai_messages".to_string(),
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: Some("auth".to_string()),
            message_count: 3,
        })
        .expect("conversation metadata write should succeed");

        let rows = db
            .list_conversation_sources(Some("ops"), None, None, 10)
            .expect("conversation metadata read should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_path, "/tmp/conversations/transcript.md");
        assert_eq!(rows[0].hall.as_deref(), Some("incidents"));
    }

    #[test]
    fn test_conversation_source_metadata_can_list_more_than_200_rows() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        for index in 0..250 {
            db.upsert_conversation_source(&ConversationSourceRecord {
                project_id: "project-1".to_string(),
                transcript_path: format!("/tmp/transcript-{index}.json"),
                memory_path: format!("/tmp/conversations/transcript-{index}.md"),
                format: "openai_messages".to_string(),
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some(format!("room-{index}")),
                message_count: 1,
            })
            .expect("conversation metadata write should succeed");
        }

        let rows = db
            .list_conversation_sources(Some("ops"), None, None, 250)
            .expect("conversation metadata read should succeed");
        assert_eq!(rows.len(), 250);
    }

    #[test]
    fn test_conversation_source_metadata_filters_by_hall_and_room() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_conversation_source(&ConversationSourceRecord {
            project_id: "project-1".to_string(),
            transcript_path: "/tmp/auth.json".to_string(),
            memory_path: "/tmp/conversations/auth.md".to_string(),
            format: "openai_messages".to_string(),
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: Some("auth".to_string()),
            message_count: 1,
        })
        .expect("auth conversation write should succeed");
        db.upsert_conversation_source(&ConversationSourceRecord {
            project_id: "project-1".to_string(),
            transcript_path: "/tmp/pager.json".to_string(),
            memory_path: "/tmp/conversations/pager.md".to_string(),
            format: "openai_messages".to_string(),
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: Some("pager".to_string()),
            message_count: 1,
        })
        .expect("pager conversation write should succeed");

        let rows = db
            .list_conversation_sources(Some("ops"), Some("incidents"), Some("auth"), 10)
            .expect("filtered conversation metadata read should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].room.as_deref(), Some("auth"));
    }

    #[test]
    fn test_aaak_lesson_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        let lesson = AaakLesson {
            lesson_id: "lesson-auth-refresh".to_string(),
            project_id: "project-1".to_string(),
            pattern_key: "aaak-auth-refresh".to_string(),
            role: "assistant".to_string(),
            canonical_text: "Refresh tokens were failing in staging due to issuer drift."
                .to_string(),
            occurrence_count: 2,
            confidence_percent: 80,
            source_transcript_path: Some("/tmp/auth-transcript.json".to_string()),
            updated_at_epoch_ms: 1_712_000_000_000,
        };

        db.upsert_aaak_lesson(&lesson)
            .expect("aaak lesson write should succeed");

        let rows = db
            .list_aaak_lessons("project-1", 10)
            .expect("aaak lesson read should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], lesson);

        let deleted = db
            .delete_aaak_lesson("lesson-auth-refresh")
            .expect("aaak lesson delete should succeed");
        assert!(deleted);
        assert!(db
            .list_aaak_lessons("project-1", 10)
            .expect("aaak lesson list should succeed")
            .is_empty());
    }

    #[test]
    fn test_aaak_lesson_isolation_across_projects() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db_project_a =
            ProjectDatabase::open(&project_root, "project-a").expect("db project-a should open");
        let db_project_b =
            ProjectDatabase::open(&project_root, "project-b").expect("db project-b should open");

        let lesson = AaakLesson {
            lesson_id: "lesson-shared-id".to_string(),
            project_id: "project-a".to_string(),
            pattern_key: "aaak-project-a".to_string(),
            role: "assistant".to_string(),
            canonical_text: "Project A only pattern".to_string(),
            occurrence_count: 2,
            confidence_percent: 78,
            source_transcript_path: Some("/tmp/project-a.json".to_string()),
            updated_at_epoch_ms: 1_712_000_000_000,
        };
        db_project_a
            .upsert_aaak_lesson(&lesson)
            .expect("project-a lesson write should succeed");

        let a_rows = db_project_a
            .list_aaak_lessons("project-a", 10)
            .expect("project-a list should succeed");
        assert_eq!(a_rows.len(), 1);

        let b_rows = db_project_b
            .list_aaak_lessons("project-b", 10)
            .expect("project-b list should succeed");
        assert!(
            b_rows.is_empty(),
            "project-b should not see project-a lessons"
        );

        let deleted_from_b = db_project_b
            .delete_aaak_lesson("lesson-shared-id")
            .expect("project-b delete should succeed");
        assert!(
            !deleted_from_b,
            "project-b should not be able to delete project-a lesson"
        );

        let still_exists_in_a = db_project_a
            .list_aaak_lessons("project-a", 10)
            .expect("project-a list should succeed");
        assert_eq!(still_exists_in_a.len(), 1);
    }

    #[test]
    fn test_diary_entries_roundtrip_with_tags_mood_and_date() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        let entry = DiaryEntry {
            entry_id: "diary-2026-04-10-one".to_string(),
            project_id: "project-1".to_string(),
            entry_date: "2026-04-10".to_string(),
            mood: Some("focused".to_string()),
            tags: vec!["release".to_string(), "auth".to_string()],
            content: "Finished the production rollout checklist and validated the auth migration."
                .to_string(),
            created_at_epoch_ms: 1_775_000_000_000,
            updated_at_epoch_ms: 1_775_000_000_000,
        };

        db.upsert_diary_entry(&entry)
            .expect("diary entry write should succeed");

        let rows = db
            .list_diary_entries(Some("2026-04-01"), Some("2026-04-30"), 10)
            .expect("diary entry list should succeed");
        assert_eq!(rows, vec![entry]);
    }

    #[test]
    fn test_diary_entries_list_by_date_range() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        for (entry_id, entry_date) in [
            ("diary-2026-04-08", "2026-04-08"),
            ("diary-2026-04-09", "2026-04-09"),
            ("diary-2026-04-10", "2026-04-10"),
        ] {
            db.upsert_diary_entry(&DiaryEntry {
                entry_id: entry_id.to_string(),
                project_id: "project-1".to_string(),
                entry_date: entry_date.to_string(),
                mood: None,
                tags: vec!["ops".to_string()],
                content: format!("Entry for {entry_date}"),
                created_at_epoch_ms: 1_775_000_000_000,
                updated_at_epoch_ms: 1_775_000_000_000,
            })
            .expect("diary entry write should succeed");
        }

        let rows = db
            .list_diary_entries(Some("2026-04-09"), Some("2026-04-10"), 10)
            .expect("diary entry range list should succeed");
        let entry_dates = rows
            .iter()
            .map(|entry| entry.entry_date.as_str())
            .collect::<Vec<_>>();
        assert_eq!(entry_dates, vec!["2026-04-10", "2026-04-09"]);
    }

    #[test]
    fn test_diary_entries_search_matches_content_and_tags() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_diary_entry(&DiaryEntry {
            entry_id: "diary-release".to_string(),
            project_id: "project-1".to_string(),
            entry_date: "2026-04-10".to_string(),
            mood: Some("relieved".to_string()),
            tags: vec!["release".to_string(), "auth".to_string()],
            content: "Finished the release train after fixing the token refresh issue.".to_string(),
            created_at_epoch_ms: 1_775_000_000_000,
            updated_at_epoch_ms: 1_775_000_000_000,
        })
        .expect("diary entry write should succeed");
        db.upsert_diary_entry(&DiaryEntry {
            entry_id: "diary-notes".to_string(),
            project_id: "project-1".to_string(),
            entry_date: "2026-04-11".to_string(),
            mood: None,
            tags: vec!["meeting".to_string()],
            content: "Captured team notes for the Monday sync.".to_string(),
            created_at_epoch_ms: 1_775_000_000_100,
            updated_at_epoch_ms: 1_775_000_000_100,
        })
        .expect("diary entry write should succeed");

        let rows = db
            .search_diary_entries("release", 10)
            .expect("diary entry search should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry_id, "diary-release");
    }

    #[test]
    fn test_diary_entries_refresh_preserves_created_at() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_diary_entry(&DiaryEntry {
            entry_id: "diary-2026-04-10".to_string(),
            project_id: "project-1".to_string(),
            entry_date: "2026-04-10".to_string(),
            mood: Some("focused".to_string()),
            tags: vec!["release".to_string()],
            content: "Initial release note".to_string(),
            created_at_epoch_ms: 1_775_000_000_000,
            updated_at_epoch_ms: 1_775_000_000_000,
        })
        .expect("initial diary write should succeed");
        db.upsert_diary_entry(&DiaryEntry {
            entry_id: "diary-2026-04-10".to_string(),
            project_id: "project-1".to_string(),
            entry_date: "2026-04-10".to_string(),
            mood: Some("relieved".to_string()),
            tags: vec!["release".to_string(), "auth".to_string()],
            content: "Updated release note after auth verification".to_string(),
            created_at_epoch_ms: 1_775_999_999_999,
            updated_at_epoch_ms: 1_776_000_000_000,
        })
        .expect("refresh diary write should succeed");

        let rows = db
            .list_diary_entries(Some("2026-04-10"), Some("2026-04-10"), 10)
            .expect("diary entry list should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].content,
            "Updated release note after auth verification"
        );
        assert_eq!(rows[0].created_at_epoch_ms, 1_775_000_000_000);
        assert_eq!(rows[0].updated_at_epoch_ms, 1_776_000_000_000);
    }

    #[test]
    fn test_navigation_primitive_create_and_list_drawer() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_navigation_node(&MemoryNavigationNode {
            node_id: "drawer:ops".to_string(),
            project_id: "project-1".to_string(),
            kind: MemoryNavigationNodeKind::Drawer,
            label: "Operations".to_string(),
            parent_node_id: None,
            wing: Some("ops".to_string()),
            hall: None,
            room: None,
            updated_at_epoch_ms: 1_712_000_000_000,
        })
        .expect("drawer upsert should succeed");

        let drawers = db
            .list_navigation_nodes(None, Some("drawer"), 10)
            .expect("drawer list should succeed");
        assert_eq!(drawers.len(), 1);
        assert_eq!(drawers[0].node_id, "drawer:ops");
        assert_eq!(drawers[0].kind, MemoryNavigationNodeKind::Drawer);
        assert_eq!(drawers[0].wing.as_deref(), Some("ops"));
    }

    #[test]
    fn test_navigation_primitive_create_and_list_closet_under_drawer() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        db.upsert_navigation_node(&MemoryNavigationNode {
            node_id: "drawer:ops".to_string(),
            project_id: "project-1".to_string(),
            kind: MemoryNavigationNodeKind::Drawer,
            label: "Operations".to_string(),
            parent_node_id: None,
            wing: Some("ops".to_string()),
            hall: None,
            room: None,
            updated_at_epoch_ms: 1_712_000_000_000,
        })
        .expect("drawer upsert should succeed");
        db.upsert_navigation_node(&MemoryNavigationNode {
            node_id: "closet:ops:incidents".to_string(),
            project_id: "project-1".to_string(),
            kind: MemoryNavigationNodeKind::Closet,
            label: "Incidents".to_string(),
            parent_node_id: Some("drawer:ops".to_string()),
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: None,
            updated_at_epoch_ms: 1_712_000_000_100,
        })
        .expect("closet upsert should succeed");

        let closets = db
            .list_navigation_nodes(Some("drawer:ops"), Some("closet"), 10)
            .expect("closet list should succeed");
        assert_eq!(closets.len(), 1);
        assert_eq!(closets[0].node_id, "closet:ops:incidents");
        assert_eq!(closets[0].parent_node_id.as_deref(), Some("drawer:ops"));
    }

    #[test]
    fn test_navigation_primitive_tunnel_link_is_unique() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db = ProjectDatabase::open(&project_root, "project-1").expect("db should open");
        for node in [
            MemoryNavigationNode {
                node_id: "drawer:ops".to_string(),
                project_id: "project-1".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Operations".to_string(),
                parent_node_id: None,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_000,
            },
            MemoryNavigationNode {
                node_id: "drawer:platform".to_string(),
                project_id: "project-1".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Platform".to_string(),
                parent_node_id: None,
                wing: Some("platform".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_001,
            },
        ] {
            db.upsert_navigation_node(&node)
                .expect("node upsert should succeed");
        }

        let tunnel = MemoryNavigationTunnel {
            tunnel_id: "tunnel:drawer:ops:drawer:platform".to_string(),
            project_id: "project-1".to_string(),
            from_node_id: "drawer:ops".to_string(),
            to_node_id: "drawer:platform".to_string(),
            updated_at_epoch_ms: 1_712_000_000_100,
        };
        db.upsert_navigation_tunnel(&tunnel)
            .expect("tunnel upsert should succeed");
        db.upsert_navigation_tunnel(&tunnel)
            .expect("duplicate tunnel upsert should succeed");

        let tunnels = db
            .list_navigation_tunnels(Some("drawer:ops"))
            .expect("tunnel list should succeed");
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].from_node_id, "drawer:ops");
        assert_eq!(tunnels[0].to_node_id, "drawer:platform");
    }

    #[test]
    fn test_navigation_primitive_same_node_id_is_isolated_per_project() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db_project_a =
            ProjectDatabase::open(&project_root, "project-a").expect("db project-a should open");
        let db_project_b =
            ProjectDatabase::open(&project_root, "project-b").expect("db project-b should open");

        db_project_a
            .upsert_navigation_node(&MemoryNavigationNode {
                node_id: "drawer:shared".to_string(),
                project_id: "project-a".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Project A Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-a".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_000,
            })
            .expect("project-a node upsert should succeed");
        db_project_b
            .upsert_navigation_node(&MemoryNavigationNode {
                node_id: "drawer:shared".to_string(),
                project_id: "project-b".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Project B Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-b".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_001,
            })
            .expect("project-b node upsert should succeed");

        let node_a = db_project_a
            .get_navigation_node("drawer:shared")
            .expect("project-a node fetch should succeed")
            .expect("project-a node should exist");
        let node_b = db_project_b
            .get_navigation_node("drawer:shared")
            .expect("project-b node fetch should succeed")
            .expect("project-b node should exist");

        assert_eq!(node_a.label, "Project A Drawer");
        assert_eq!(node_b.label, "Project B Drawer");
        assert_eq!(node_a.project_id, "project-a");
        assert_eq!(node_b.project_id, "project-b");
    }

    #[test]
    fn test_navigation_primitive_parent_and_tunnel_are_project_scoped() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");

        let db_project_a =
            ProjectDatabase::open(&project_root, "project-a").expect("db project-a should open");
        let db_project_b =
            ProjectDatabase::open(&project_root, "project-b").expect("db project-b should open");

        db_project_a
            .upsert_navigation_node(&MemoryNavigationNode {
                node_id: "drawer:shared".to_string(),
                project_id: "project-a".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Project A Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-a".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_000,
            })
            .expect("project-a drawer upsert should succeed");
        db_project_b
            .upsert_navigation_node(&MemoryNavigationNode {
                node_id: "drawer:local".to_string(),
                project_id: "project-b".to_string(),
                kind: MemoryNavigationNodeKind::Drawer,
                label: "Project B Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-b".to_string()),
                hall: None,
                room: None,
                updated_at_epoch_ms: 1_712_000_000_001,
            })
            .expect("project-b drawer upsert should succeed");

        let parent_err = db_project_b
            .upsert_navigation_node(&MemoryNavigationNode {
                node_id: "closet:cross-project".to_string(),
                project_id: "project-b".to_string(),
                kind: MemoryNavigationNodeKind::Closet,
                label: "Cross Project Closet".to_string(),
                parent_node_id: Some("drawer:shared".to_string()),
                wing: Some("ops-b".to_string()),
                hall: Some("incidents".to_string()),
                room: None,
                updated_at_epoch_ms: 1_712_000_000_100,
            })
            .expect_err("cross-project parent should fail");
        assert!(
            parent_err.to_string().contains("FOREIGN KEY"),
            "unexpected parent scoping error: {parent_err}"
        );

        let tunnel_err = db_project_b
            .upsert_navigation_tunnel(&MemoryNavigationTunnel {
                tunnel_id: "tunnel:cross-project".to_string(),
                project_id: "project-b".to_string(),
                from_node_id: "drawer:local".to_string(),
                to_node_id: "drawer:shared".to_string(),
                updated_at_epoch_ms: 1_712_000_000_101,
            })
            .expect_err("cross-project tunnel should fail");
        assert!(
            tunnel_err.to_string().contains("FOREIGN KEY"),
            "unexpected tunnel scoping error: {tunnel_err}"
        );
    }
}
