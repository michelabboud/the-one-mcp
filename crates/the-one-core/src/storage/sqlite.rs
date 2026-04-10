use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};

use crate::contracts::ApprovalScope;
use crate::error::CoreError;

const CURRENT_SCHEMA_VERSION: i64 = 2;

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

    pub fn record_audit_event(
        &self,
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), CoreError> {
        self.conn.execute(
            "
            INSERT INTO audit_events(project_id, event_type, payload_json, created_at_epoch_ms)
            VALUES (?1, ?2, ?3, CAST(strftime('%s','now') AS INTEGER) * 1000)
            ",
            params![self.project_id, event_type, payload_json],
        )?;
        Ok(())
    }

    pub fn audit_event_count(&self) -> Result<i64, CoreError> {
        let count = self
            .conn
            .query_row("SELECT COUNT(*) FROM audit_events", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        let safe_limit = limit.min(200) as i64;
        let mut stmt = self.conn.prepare(
            "
            SELECT id, project_id, event_type, payload_json, created_at_epoch_ms
            FROM audit_events
            WHERE project_id = ?1
            ORDER BY id DESC
            LIMIT ?2
            ",
        )?;

        let mut rows = stmt.query(params![self.project_id, safe_limit])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            events.push(AuditEvent {
                id: row.get(0)?,
                project_id: row.get(1)?,
                event_type: row.get(2)?,
                payload_json: row.get(3)?,
                created_at_epoch_ms: row.get(4)?,
            });
        }

        Ok(events)
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

    if current > CURRENT_SCHEMA_VERSION {
        return Err(CoreError::UnsupportedSchemaVersion(current.to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::contracts::ApprovalScope;

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
        assert_eq!(db.schema_version().expect("schema version query"), 2);
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
}
