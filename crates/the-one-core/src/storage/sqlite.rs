use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::contracts::ApprovalScope;
use crate::error::CoreError;

const CURRENT_SCHEMA_VERSION: i64 = 1;

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

    if current > CURRENT_SCHEMA_VERSION {
        return Err(CoreError::UnsupportedSchemaVersion(current.to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::contracts::ApprovalScope;

    use super::ProjectDatabase;

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
        assert_eq!(db.schema_version().expect("schema version query"), 1);
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
}
