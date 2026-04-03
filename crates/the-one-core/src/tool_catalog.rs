use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::tool_catalog_schema;

// ---------------------------------------------------------------------------
// Types deserialised from catalog JSON files
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogInstall {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub binary_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRun {
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogToolEntry {
    pub id: String,
    pub name: String,
    #[serde(default = "default_tool_type")]
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub when_to_use: String,
    #[serde(default)]
    pub what_it_finds: String,
    #[serde(default)]
    pub install: Option<CatalogInstall>,
    #[serde(default)]
    pub run: Option<CatalogRun>,
    #[serde(default)]
    pub risk_level: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub github: String,
    #[serde(default = "default_trust_level")]
    pub trust_level: String,
}

fn default_tool_type() -> String {
    "cli".to_string()
}
fn default_trust_level() -> String {
    "community".to_string()
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub description: String,
    pub category: Vec<String>,
    pub state: String,
    pub source: String,
    pub trust_level: String,
    pub install_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFullInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub category: Vec<String>,
    pub languages: Vec<String>,
    pub description: String,
    pub when_to_use: String,
    pub what_it_finds: String,
    pub install_command: String,
    pub run_command: String,
    pub risk_level: String,
    pub tags: Vec<String>,
    pub github: String,
    pub trust_level: String,
    pub source: String,
    pub updated_at: u64,
    pub installed_path: Option<String>,
    pub installed_version: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SuggestResult {
    pub enabled: Vec<ToolSummary>,
    pub available: Vec<ToolSummary>,
    pub recommended: Vec<ToolSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub description: String,
    pub score: f64,
    pub state: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// ToolCatalog
// ---------------------------------------------------------------------------

pub struct ToolCatalog {
    conn: Connection,
}

impl ToolCatalog {
    /// Open (or create) the catalog database inside `catalog_dir`.
    pub fn open(catalog_dir: &Path) -> Result<Self, CoreError> {
        std::fs::create_dir_all(catalog_dir).map_err(CoreError::Io)?;
        let db_path = catalog_dir.join("catalog.db");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        // Create regular tables.
        conn.execute_batch(tool_catalog_schema::CREATE_TOOLS_TABLE)?;
        conn.execute_batch(tool_catalog_schema::CREATE_SYSTEM_INVENTORY)?;
        conn.execute_batch(tool_catalog_schema::CREATE_ENABLED_TOOLS)?;
        conn.execute_batch(tool_catalog_schema::CREATE_CATALOG_META)?;

        // FTS5 virtual table — the IF NOT EXISTS is embedded in the SQL.
        // Some older rusqlite builds surface a benign error when the table
        // already exists; guard with a sqlite_master check.
        let fts_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tools_fts'",
            [],
            |r| r.get(0),
        )?;
        if !fts_exists {
            conn.execute_batch(tool_catalog_schema::CREATE_TOOLS_FTS)?;
        }

        // Triggers — safe to re-create (IF NOT EXISTS in the SQL).
        conn.execute_batch(tool_catalog_schema::CREATE_FTS_TRIGGER_INSERT)?;
        conn.execute_batch(tool_catalog_schema::CREATE_FTS_TRIGGER_UPDATE)?;
        conn.execute_batch(tool_catalog_schema::CREATE_FTS_TRIGGER_DELETE)?;

        Ok(Self { conn })
    }

    // -- Import ---------------------------------------------------------------

    /// Bulk-upsert a slice of entries into the tools table.
    pub fn import_tools(
        &self,
        entries: &[CatalogToolEntry],
        source: &str,
    ) -> Result<usize, CoreError> {
        let now = epoch_millis() as i64;
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO tools (id, name, type, category, languages, description,
                     when_to_use, what_it_finds, install_command, run_command,
                     risk_level, tags, github, trust_level, source, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                 ON CONFLICT(id) DO UPDATE SET
                     name=excluded.name, type=excluded.type,
                     category=excluded.category, languages=excluded.languages,
                     description=excluded.description, when_to_use=excluded.when_to_use,
                     what_it_finds=excluded.what_it_finds,
                     install_command=excluded.install_command,
                     run_command=excluded.run_command, risk_level=excluded.risk_level,
                     tags=excluded.tags, github=excluded.github,
                     trust_level=excluded.trust_level, source=excluded.source,
                     updated_at=excluded.updated_at",
            )?;
            for e in entries {
                let install_cmd = e
                    .install
                    .as_ref()
                    .map(|i| i.command.as_str())
                    .unwrap_or("");
                let run_cmd = e.run.as_ref().map(|r| r.command.as_str()).unwrap_or("");
                stmt.execute(params![
                    e.id,
                    e.name,
                    e.tool_type,
                    serde_json::to_string(&e.category).unwrap_or_default(),
                    serde_json::to_string(&e.languages).unwrap_or_default(),
                    e.description,
                    e.when_to_use,
                    e.what_it_finds,
                    install_cmd,
                    run_cmd,
                    e.risk_level,
                    serde_json::to_string(&e.tags).unwrap_or_default(),
                    e.github,
                    e.trust_level,
                    source,
                    now,
                ])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    /// Read a JSON file containing an array of `CatalogToolEntry` and import.
    pub fn import_from_file(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<usize, CoreError> {
        let data = std::fs::read_to_string(path)?;
        let entries: Vec<CatalogToolEntry> = serde_json::from_str(&data)?;
        self.import_tools(&entries, source)
    }

    /// Walk `languages/`, `categories/`, and `mcps/` subdirectories of
    /// `catalog_dir` and import every `.json` file found.
    pub fn import_catalog_dir(&self, catalog_dir: &Path) -> Result<usize, CoreError> {
        let mut total = 0usize;
        for subdir in &["languages", "categories", "mcps"] {
            let dir = catalog_dir.join(subdir);
            if !dir.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    total += self.import_from_file(&path, "catalog")?;
                }
            }
        }
        Ok(total)
    }

    // -- Queries --------------------------------------------------------------

    pub fn tool_count(&self) -> Result<u64, CoreError> {
        let c: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM tools", [], |r| r.get(0))?;
        Ok(c as u64)
    }

    pub fn catalog_version(&self) -> Result<Option<String>, CoreError> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM catalog_meta WHERE key='version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn set_catalog_version(&self, v: &str) -> Result<(), CoreError> {
        self.conn.execute(
            "INSERT INTO catalog_meta (key, value) VALUES ('version', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![v],
        )?;
        Ok(())
    }

    /// Retrieve full information about a single tool, joined with inventory.
    pub fn get_tool(&self, id: &str) -> Result<Option<ToolFullInfo>, CoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT t.id, t.name, t.type, t.category, t.languages,
                        t.description, t.when_to_use, t.what_it_finds,
                        t.install_command, t.run_command, t.risk_level,
                        t.tags, t.github, t.trust_level, t.source, t.updated_at,
                        si.path, si.version
                 FROM tools t
                 LEFT JOIN system_inventory si ON si.binary_name = t.id
                 WHERE t.id = ?1",
                params![id],
                |r| {
                    Ok(ToolFullInfoRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        tool_type: r.get(2)?,
                        category_json: r.get(3)?,
                        languages_json: r.get(4)?,
                        description: r.get(5)?,
                        when_to_use: r.get(6)?,
                        what_it_finds: r.get(7)?,
                        install_command: r.get(8)?,
                        run_command: r.get(9)?,
                        risk_level: r.get(10)?,
                        tags_json: r.get(11)?,
                        github: r.get(12)?,
                        trust_level: r.get(13)?,
                        source: r.get(14)?,
                        updated_at: r.get::<_, i64>(15)? as u64,
                        inv_path: r.get(16)?,
                        inv_version: r.get(17)?,
                    })
                },
            )
            .optional()?;

        Ok(row.map(|r| r.into_full_info()))
    }

    /// Filtered suggestion query, grouped by enabled / available / recommended.
    #[allow(clippy::too_many_arguments)]
    pub fn suggest(
        &self,
        languages: &[String],
        category: Option<&str>,
        tool_type: Option<&str>,
        cli: &str,
        project_root: &str,
        limit: u32,
    ) -> Result<SuggestResult, CoreError> {
        let mut where_clauses: Vec<String> = Vec::new();
        let mut bind_values: Vec<String> = Vec::new();

        // Language overlap filter — match any of the requested languages.
        if !languages.is_empty() {
            let mut lang_or: Vec<String> = Vec::new();
            for lang in languages {
                lang_or.push(format!(
                    "t.languages LIKE '%\"{}\":%' OR t.languages LIKE '%\"{}\",%' OR t.languages LIKE '%\"{}\"]%'",
                    lang, lang, lang
                ));
            }
            where_clauses.push(format!("({})", lang_or.join(" OR ")));
        }
        if let Some(cat) = category {
            where_clauses.push(format!(
                "(t.category LIKE '%\"{}\":%' OR t.category LIKE '%\"{}\",%' OR t.category LIKE '%\"{}\"]%')",
                cat, cat, cat
            ));
        }
        if let Some(tt) = tool_type {
            where_clauses.push("t.type = ?".to_string());
            bind_values.push(tt.to_string());
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let sql = format!(
            "SELECT t.id, t.name, t.type, t.description, t.category,
                    t.source, t.trust_level, t.install_command,
                    CASE WHEN e.tool_id IS NOT NULL THEN 'enabled' ELSE 'available' END AS state
             FROM tools t
             LEFT JOIN enabled_tools e
                 ON e.tool_id = t.id AND e.cli = ?1 AND e.project_root = ?2
             {}
             ORDER BY state DESC, t.name ASC
             LIMIT ?3",
            where_sql
        );

        let mut stmt = self.conn.prepare(&sql)?;

        // Build dynamic params — first 3 are cli, project_root, limit.
        let mut param_idx = 0u32;
        let base_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(cli.to_string()),
            Box::new(project_root.to_string()),
            Box::new(limit),
        ];

        // Additional bind values from tool_type filter.
        let all_params: Vec<Box<dyn rusqlite::types::ToSql>> = base_params
            .into_iter()
            .chain(bind_values.into_iter().map(|v| {
                param_idx += 1;
                Box::new(v) as Box<dyn rusqlite::types::ToSql>
            }))
            .collect();

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |r| {
            Ok(ToolSummaryRow {
                id: r.get(0)?,
                name: r.get(1)?,
                tool_type: r.get(2)?,
                description: r.get(3)?,
                category_json: r.get(4)?,
                source: r.get(5)?,
                trust_level: r.get(6)?,
                install_command: r.get(7)?,
                state: r.get(8)?,
            })
        })?;

        let mut result = SuggestResult::default();
        for row in rows {
            let row = row?;
            let summary = row.into_summary();
            match summary.state.as_str() {
                "enabled" => result.enabled.push(summary),
                _ => result.recommended.push(summary),
            }
        }
        Ok(result)
    }

    /// Full-text search over the FTS5 index.
    pub fn search_fts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchResult>, CoreError> {
        // Sanitise the query for FTS5 — wrap each token in double-quotes.
        let sanitised: String = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT f.id, t.name, t.description, rank, t.source
             FROM tools_fts f
             JOIN tools t ON t.id = f.id
             WHERE tools_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![sanitised, limit], |r| {
            Ok(SearchResult {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                score: r.get(3)?,
                state: String::new(),
                source: r.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // -- System inventory -----------------------------------------------------

    /// For each tool in the catalog, run `which <binary_name>` and record
    /// path + version in the system_inventory table.
    pub fn scan_system_inventory(&self) -> Result<u64, CoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, install_command FROM tools")?;
        let tool_ids: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let now = epoch_millis() as i64;
        let tx = self.conn.unchecked_transaction()?;
        let mut found = 0u64;

        for (binary_name, _install_cmd) in &tool_ids {
            // Derive the actual binary name: use the id (e.g. "cargo-audit").
            let output = std::process::Command::new("which")
                .arg(binary_name)
                .output();
            match output {
                Ok(o) if o.status.success() => {
                    let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    tx.execute(
                        "INSERT INTO system_inventory (binary_name, path, version, last_checked)
                         VALUES (?1, ?2, '', ?3)
                         ON CONFLICT(binary_name) DO UPDATE SET
                             path=excluded.path, last_checked=excluded.last_checked",
                        params![binary_name, path, now],
                    )?;
                    found += 1;
                }
                _ => {
                    // Not found — remove stale entry if any.
                    tx.execute(
                        "DELETE FROM system_inventory WHERE binary_name = ?1",
                        params![binary_name],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(found)
    }

    // -- Enable / disable -----------------------------------------------------

    pub fn enable_tool(
        &self,
        tool_id: &str,
        cli: &str,
        project_root: &str,
    ) -> Result<(), CoreError> {
        let now = epoch_millis() as i64;
        self.conn.execute(
            "INSERT INTO enabled_tools (tool_id, cli, project_root, enabled_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(tool_id, cli, project_root) DO UPDATE SET enabled_at=excluded.enabled_at",
            params![tool_id, cli, project_root, now],
        )?;
        Ok(())
    }

    pub fn disable_tool(
        &self,
        tool_id: &str,
        cli: &str,
        project_root: &str,
    ) -> Result<(), CoreError> {
        self.conn.execute(
            "DELETE FROM enabled_tools WHERE tool_id=?1 AND cli=?2 AND project_root=?3",
            params![tool_id, cli, project_root],
        )?;
        Ok(())
    }

    pub fn is_enabled(
        &self,
        tool_id: &str,
        cli: &str,
        project_root: &str,
    ) -> Result<bool, CoreError> {
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM enabled_tools
             WHERE tool_id=?1 AND cli=?2 AND project_root=?3",
            params![tool_id, cli, project_root],
            |r| r.get(0),
        )?;
        Ok(exists)
    }

    // -- User tools -----------------------------------------------------------

    /// Returns (tool_id, text_to_embed) for all tools.
    /// The text combines description, when_to_use, what_it_finds, and tags
    /// to create a rich embedding target for semantic search.
    pub fn all_tool_descriptions(&self) -> Result<Vec<(String, String)>, CoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, description, when_to_use, what_it_finds, tags FROM tools"
        ).map_err(|e| CoreError::Catalog(format!("all_tool_descriptions prepare: {e}")))?;

        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let description: String = row.get(1)?;
            let when_to_use: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
            let what_it_finds: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
            let tags_json: String = row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "[]".to_string());
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

            let text = format!(
                "{description} {when_to_use} {what_it_finds} {}",
                tags.join(" ")
            ).trim().to_string();

            Ok((id, text))
        }).map_err(|e| CoreError::Catalog(format!("all_tool_descriptions query: {e}")))?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Import a single user-provided tool entry with source='user'.
    pub fn add_user_tool(&self, entry: &CatalogToolEntry) -> Result<(), CoreError> {
        self.import_tools(std::slice::from_ref(entry), "user")?;
        Ok(())
    }

    /// Remove a tool only if its source is 'user'.
    pub fn remove_user_tool(&self, tool_id: &str) -> Result<bool, CoreError> {
        let changed = self.conn.execute(
            "DELETE FROM tools WHERE id = ?1 AND source = 'user'",
            params![tool_id],
        )?;
        Ok(changed > 0)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Raw row for get_tool query.
struct ToolFullInfoRow {
    id: String,
    name: String,
    tool_type: String,
    category_json: String,
    languages_json: String,
    description: String,
    when_to_use: String,
    what_it_finds: String,
    install_command: String,
    run_command: String,
    risk_level: String,
    tags_json: String,
    github: String,
    trust_level: String,
    source: String,
    updated_at: u64,
    inv_path: Option<String>,
    inv_version: Option<String>,
}

impl ToolFullInfoRow {
    fn into_full_info(self) -> ToolFullInfo {
        let installed = self.inv_path.is_some();
        ToolFullInfo {
            id: self.id,
            name: self.name,
            tool_type: self.tool_type,
            category: json_vec(&self.category_json),
            languages: json_vec(&self.languages_json),
            description: self.description,
            when_to_use: self.when_to_use,
            what_it_finds: self.what_it_finds,
            install_command: self.install_command,
            run_command: self.run_command,
            risk_level: self.risk_level,
            tags: json_vec(&self.tags_json),
            github: self.github,
            trust_level: self.trust_level,
            source: self.source,
            updated_at: self.updated_at,
            installed_path: self.inv_path,
            installed_version: self.inv_version,
            state: if installed {
                "installed".to_string()
            } else {
                "available".to_string()
            },
        }
    }
}

/// Raw row for suggest query.
struct ToolSummaryRow {
    id: String,
    name: String,
    tool_type: String,
    description: String,
    category_json: String,
    source: String,
    trust_level: String,
    install_command: String,
    state: String,
}

impl ToolSummaryRow {
    fn into_summary(self) -> ToolSummary {
        ToolSummary {
            id: self.id,
            name: self.name,
            tool_type: self.tool_type,
            description: self.description,
            category: json_vec(&self.category_json),
            state: self.state,
            source: self.source,
            trust_level: self.trust_level,
            install_command: self.install_command,
        }
    }
}

fn json_vec(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_entry(id: &str, name: &str, langs: Vec<&str>, cats: Vec<&str>) -> CatalogToolEntry {
        CatalogToolEntry {
            id: id.to_string(),
            name: name.to_string(),
            tool_type: "cli".to_string(),
            category: cats.into_iter().map(String::from).collect(),
            languages: langs.into_iter().map(String::from).collect(),
            description: format!("{} description", name),
            when_to_use: format!("use {} when auditing", name),
            what_it_finds: format!("{} finds issues", name),
            install: Some(CatalogInstall {
                command: format!("cargo install {}", id),
                binary_name: id.to_string(),
            }),
            run: Some(CatalogRun {
                command: format!("{} check", id),
            }),
            risk_level: "low".to_string(),
            tags: vec!["audit".to_string(), "security".to_string()],
            github: format!("https://github.com/example/{}", id),
            trust_level: "official".to_string(),
        }
    }

    #[test]
    fn test_import_and_count() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![
            sample_entry("tool-a", "Tool A", vec!["rust"], vec!["security"]),
            sample_entry("tool-b", "Tool B", vec!["python"], vec!["lint"]),
        ];
        let n = cat.import_tools(&entries, "catalog").unwrap();
        assert_eq!(n, 2);
        assert_eq!(cat.tool_count().unwrap(), 2);
    }

    #[test]
    fn test_get_tool_by_id() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![sample_entry(
            "cargo-audit",
            "Cargo Audit",
            vec!["rust"],
            vec!["security"],
        )];
        cat.import_tools(&entries, "catalog").unwrap();

        let info = cat.get_tool("cargo-audit").unwrap().expect("tool exists");
        assert_eq!(info.id, "cargo-audit");
        assert_eq!(info.name, "Cargo Audit");
        assert_eq!(info.tool_type, "cli");
        assert_eq!(info.languages, vec!["rust"]);
        assert_eq!(info.category, vec!["security"]);
        assert_eq!(info.trust_level, "official");
        assert_eq!(info.source, "catalog");
        assert_eq!(info.state, "available");
    }

    #[test]
    fn test_suggest_filters_by_language() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![
            sample_entry("cargo-audit", "Cargo Audit", vec!["rust"], vec!["security"]),
            sample_entry("bandit", "Bandit", vec!["python"], vec!["security"]),
        ];
        cat.import_tools(&entries, "catalog").unwrap();

        let result = cat
            .suggest(
                &["rust".to_string()],
                None,
                None,
                "claude",
                "/tmp/proj",
                10,
            )
            .unwrap();

        // Only the rust tool should appear.
        let all_ids: Vec<&str> = result
            .enabled
            .iter()
            .chain(result.available.iter())
            .chain(result.recommended.iter())
            .map(|s| s.id.as_str())
            .collect();
        assert!(all_ids.contains(&"cargo-audit"), "rust tool present");
        assert!(!all_ids.contains(&"bandit"), "python tool filtered out");
    }

    #[test]
    fn test_fts_search() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![
            sample_entry("cargo-audit", "Cargo Audit", vec!["rust"], vec!["security"]),
            sample_entry("prettier", "Prettier", vec!["javascript"], vec!["format"]),
        ];
        cat.import_tools(&entries, "catalog").unwrap();

        let results = cat.search_fts("audit", 10).unwrap();
        assert!(!results.is_empty(), "FTS returned results");
        assert_eq!(results[0].id, "cargo-audit");
    }

    #[test]
    fn test_enable_disable_tool() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![sample_entry(
            "cargo-audit",
            "Cargo Audit",
            vec!["rust"],
            vec!["security"],
        )];
        cat.import_tools(&entries, "catalog").unwrap();

        assert!(!cat.is_enabled("cargo-audit", "claude", "/proj").unwrap());
        cat.enable_tool("cargo-audit", "claude", "/proj").unwrap();
        assert!(cat.is_enabled("cargo-audit", "claude", "/proj").unwrap());
        cat.disable_tool("cargo-audit", "claude", "/proj").unwrap();
        assert!(!cat.is_enabled("cargo-audit", "claude", "/proj").unwrap());
    }

    #[test]
    fn test_add_and_remove_user_tool() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();

        let entry = sample_entry("my-tool", "My Tool", vec!["rust"], vec!["custom"]);
        cat.add_user_tool(&entry).unwrap();

        let info = cat.get_tool("my-tool").unwrap().expect("exists");
        assert_eq!(info.source, "user");

        let removed = cat.remove_user_tool("my-tool").unwrap();
        assert!(removed);
        assert!(cat.get_tool("my-tool").unwrap().is_none());
    }

    fn test_catalog(dir: &Path) -> ToolCatalog {
        ToolCatalog::open(dir).unwrap()
    }

    fn sample_tool(id: &str, lang: &str) -> CatalogToolEntry {
        CatalogToolEntry {
            id: id.to_string(),
            name: id.to_string(),
            tool_type: "cli".to_string(),
            category: vec!["security".to_string()],
            languages: vec![lang.to_string()],
            description: "test tool".to_string(),
            when_to_use: "when testing".to_string(),
            what_it_finds: "finds bugs".to_string(),
            install: None,
            run: None,
            risk_level: "low".to_string(),
            tags: vec!["test".to_string()],
            github: String::new(),
            trust_level: "community".to_string(),
        }
    }

    #[test]
    fn test_all_tool_descriptions() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[sample_tool("my-tool", "rust")], "catalog").unwrap();
        let descs = cat.all_tool_descriptions().unwrap();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].0, "my-tool");
        assert!(descs[0].1.contains("test tool"));
    }

    #[test]
    fn test_remove_refuses_catalog_tools() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();

        let entries = vec![sample_entry(
            "cargo-audit",
            "Cargo Audit",
            vec!["rust"],
            vec!["security"],
        )];
        cat.import_tools(&entries, "catalog").unwrap();

        let removed = cat.remove_user_tool("cargo-audit").unwrap();
        assert!(!removed, "catalog tool should not be removable");
        assert!(cat.get_tool("cargo-audit").unwrap().is_some());
    }

    #[test]
    fn test_suggest_groups_by_state() {
        let tmp = TempDir::new().unwrap();
        let cat = ToolCatalog::open(tmp.path()).unwrap();
        let entries = vec![
            sample_entry("tool-enabled", "Enabled Tool", vec!["rust"], vec!["security"]),
            sample_entry("tool-other", "Other Tool", vec!["rust"], vec!["security"]),
        ];
        cat.import_tools(&entries, "catalog").unwrap();
        cat.enable_tool("tool-enabled", "claude", "/proj").unwrap();

        let result = cat
            .suggest(
                &["rust".to_string()],
                None,
                None,
                "claude",
                "/proj",
                10,
            )
            .unwrap();

        let enabled_ids: Vec<&str> = result.enabled.iter().map(|s| s.id.as_str()).collect();
        let recommended_ids: Vec<&str> =
            result.recommended.iter().map(|s| s.id.as_str()).collect();

        assert!(
            enabled_ids.contains(&"tool-enabled"),
            "enabled tool in enabled group"
        );
        assert!(
            recommended_ids.contains(&"tool-other"),
            "non-enabled tool in recommended group"
        );
        assert!(
            !enabled_ids.contains(&"tool-other"),
            "non-enabled not in enabled"
        );
    }
}
