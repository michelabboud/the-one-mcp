# Tool Catalog Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the curated tool catalog into the MCP broker with SQLite storage, Qdrant semantic search, system inventory scanning, per-CLI enabled state, and 6 new MCP tools (30 total).

**Architecture:** A new `ToolCatalog` module in the-one-core owns the SQLite `catalog.db` and provides query/mutate methods. The broker wraps it with async methods and project-profile-aware filtering. Qdrant stores tool embeddings in a global `the_one_tools` collection alongside the per-project document collections. System inventory is scanned via `which` on init. Updates use a changelog-based diff mechanism.

**Tech Stack:** Rust 2021, rusqlite (bundled, FTS5), tokio, fastembed, Qdrant HTTP, serde_json

**Spec:** `docs/specs/2026-04-03-tool-catalog-integration-design.md`

---

## File Structure

### New Files
```
crates/the-one-core/src/tool_catalog.rs       — ToolCatalog: SQLite catalog.db, CRUD, queries, system scan
crates/the-one-core/src/tool_catalog_schema.rs — SQL schema constants + migration
tools/catalog/_changelog.json                  — Version diff log (empty initial)
.github/workflows/catalog-build.yml            — CI: validate catalog, build snapshots
```

### Modified Files
```
crates/the-one-core/src/lib.rs                 — Add tool_catalog, tool_catalog_schema modules
crates/the-one-core/src/error.rs               — Add Catalog error variant
crates/the-one-core/Cargo.toml                 — (no new deps — rusqlite already present)
crates/the-one-mcp/src/api.rs                  — New request/response types for 6 new tools
crates/the-one-mcp/src/broker.rs               — New broker methods, ToolCatalog integration
crates/the-one-mcp/src/transport/tools.rs      — 6 new tool definitions (30 total)
crates/the-one-mcp/src/transport/jsonrpc.rs    — Dispatch for 6 new tools
```

---

## Phase 1: SQLite Catalog Foundation

### Task 1: Catalog SQLite schema and migration

**Files:**
- Create: `crates/the-one-core/src/tool_catalog_schema.rs`
- Modify: `crates/the-one-core/src/lib.rs`

- [ ] **Step 1: Create schema module with SQL constants**

Create `crates/the-one-core/src/tool_catalog_schema.rs`:

```rust
pub const CATALOG_SCHEMA_VERSION: i64 = 1;

pub const CREATE_TOOLS_TABLE: &str = "
CREATE TABLE IF NOT EXISTS tools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '[]',
    languages TEXT NOT NULL DEFAULT '[]',
    frameworks TEXT DEFAULT '[]',
    description TEXT NOT NULL,
    when_to_use TEXT,
    what_it_finds TEXT,
    install_command TEXT NOT NULL,
    install_package_manager TEXT,
    install_binary_name TEXT,
    run_command TEXT NOT NULL,
    run_args_template TEXT,
    run_common_flags TEXT DEFAULT '[]',
    risk_level TEXT DEFAULT 'low',
    requires TEXT DEFAULT '[]',
    cli_support TEXT DEFAULT '[]',
    tags TEXT DEFAULT '[]',
    github TEXT,
    docs TEXT,
    trust_level TEXT DEFAULT 'community',
    source TEXT DEFAULT 'catalog',
    catalog_version INTEGER,
    updated_at INTEGER NOT NULL
)";

pub const CREATE_TOOLS_FTS: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS tools_fts USING fts5(
    id, name, description, when_to_use, what_it_finds, tags,
    content='tools', content_rowid='rowid'
)";

pub const CREATE_FTS_TRIGGER_INSERT: &str = "
CREATE TRIGGER IF NOT EXISTS tools_ai AFTER INSERT ON tools BEGIN
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END";

pub const CREATE_FTS_TRIGGER_UPDATE: &str = "
CREATE TRIGGER IF NOT EXISTS tools_au AFTER UPDATE ON tools BEGIN
    DELETE FROM tools_fts WHERE rowid = old.rowid;
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END";

pub const CREATE_FTS_TRIGGER_DELETE: &str = "
CREATE TRIGGER IF NOT EXISTS tools_ad AFTER DELETE ON tools BEGIN
    DELETE FROM tools_fts WHERE rowid = old.rowid;
END";

pub const CREATE_SYSTEM_INVENTORY: &str = "
CREATE TABLE IF NOT EXISTS system_inventory (
    binary_name TEXT PRIMARY KEY,
    path TEXT,
    version TEXT,
    last_checked INTEGER NOT NULL
)";

pub const CREATE_ENABLED_TOOLS: &str = "
CREATE TABLE IF NOT EXISTS enabled_tools (
    tool_id TEXT NOT NULL,
    cli TEXT NOT NULL,
    project_root TEXT DEFAULT '',
    enabled_at INTEGER NOT NULL,
    PRIMARY KEY (tool_id, cli, project_root)
)";

pub const CREATE_CATALOG_META: &str = "
CREATE TABLE IF NOT EXISTS catalog_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
)";
```

- [ ] **Step 2: Add module to lib.rs**

Add to `crates/the-one-core/src/lib.rs`:
```rust
pub mod tool_catalog;
pub mod tool_catalog_schema;
```

- [ ] **Step 3: Add error variant**

Add to `CoreError` in `crates/the-one-core/src/error.rs`:
```rust
#[error("catalog error: {0}")]
Catalog(String),
```

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-core/
git commit -m "feat: tool catalog SQLite schema with FTS5 and auto-sync triggers"
```

### Task 2: ToolCatalog struct with DB init and import

**Files:**
- Create: `crates/the-one-core/src/tool_catalog.rs`

- [ ] **Step 1: Write the ToolCatalog struct and types**

Create `crates/the-one-core/src/tool_catalog.rs`:

```rust
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::tool_catalog_schema::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogToolEntry {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub category: Vec<String>,
    pub languages: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub what_it_finds: Option<String>,
    pub install: CatalogInstall,
    pub run: CatalogRun,
    #[serde(default = "default_risk_level")]
    pub risk_level: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub cli_support: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub github: Option<String>,
    #[serde(default)]
    pub docs: Option<String>,
    #[serde(default)]
    pub security: Option<CatalogSecurity>,
}

fn default_risk_level() -> String { "low".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogInstall {
    pub command: String,
    #[serde(default)]
    pub package_manager: Option<String>,
    #[serde(default)]
    pub binary_name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRun {
    pub command: String,
    #[serde(default)]
    pub args_template: Option<String>,
    #[serde(default)]
    pub common_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSecurity {
    #[serde(default)]
    pub trust_level: Option<String>,
    #[serde(default)]
    pub last_checked: Option<String>,
}

/// Result from tool.suggest — grouped by install state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestResult {
    pub enabled: Vec<ToolSummary>,
    pub available: Vec<ToolSummary>,
    pub recommended: Vec<ToolSummary>,
}

/// Compact tool info for suggest/list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub name: String,
    pub tool_type: String,
    pub description: String,
    pub category: Vec<String>,
    pub state: String,           // enabled | available | recommended
    pub source: String,          // catalog | user
    pub trust_level: String,
    pub install_command: Option<String>,
}

/// Full tool info for tool.info responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFullInfo {
    pub id: String,
    pub name: String,
    pub tool_type: String,
    pub category: Vec<String>,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub description: String,
    pub when_to_use: Option<String>,
    pub what_it_finds: Option<String>,
    pub install_command: String,
    pub install_binary_name: Option<String>,
    pub run_command: String,
    pub run_common_flags: Vec<String>,
    pub risk_level: String,
    pub requires: Vec<String>,
    pub github: Option<String>,
    pub docs: Option<String>,
    pub trust_level: String,
    pub source: String,
    pub state: String,
    pub installed_path: Option<String>,
    pub installed_version: Option<String>,
}

/// Result from tool.search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub description: String,
    pub score: f32,
    pub state: String,
    pub source: String,
}

pub struct ToolCatalog {
    conn: Connection,
    db_path: PathBuf,
}

impl ToolCatalog {
    /// Open or create the catalog database.
    pub fn open(catalog_dir: &Path) -> Result<Self, CoreError> {
        std::fs::create_dir_all(catalog_dir)?;
        let db_path = catalog_dir.join("catalog.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| CoreError::Catalog(format!("open catalog.db: {e}")))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")
            .map_err(|e| CoreError::Catalog(format!("pragmas: {e}")))?;

        // Run migrations
        conn.execute(CREATE_TOOLS_TABLE, [])
            .map_err(|e| CoreError::Catalog(format!("create tools: {e}")))?;
        conn.execute(CREATE_SYSTEM_INVENTORY, [])
            .map_err(|e| CoreError::Catalog(format!("create inventory: {e}")))?;
        conn.execute(CREATE_ENABLED_TOOLS, [])
            .map_err(|e| CoreError::Catalog(format!("create enabled: {e}")))?;
        conn.execute(CREATE_CATALOG_META, [])
            .map_err(|e| CoreError::Catalog(format!("create meta: {e}")))?;

        // FTS — create separately (virtual tables can't use IF NOT EXISTS in all SQLite versions)
        let has_fts: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tools_fts'",
            [], |row| row.get(0),
        ).unwrap_or(false);
        if !has_fts {
            conn.execute_batch(CREATE_TOOLS_FTS)
                .map_err(|e| CoreError::Catalog(format!("create fts: {e}")))?;
            conn.execute_batch(CREATE_FTS_TRIGGER_INSERT)
                .map_err(|e| CoreError::Catalog(format!("fts trigger insert: {e}")))?;
            conn.execute_batch(CREATE_FTS_TRIGGER_UPDATE)
                .map_err(|e| CoreError::Catalog(format!("fts trigger update: {e}")))?;
            conn.execute_batch(CREATE_FTS_TRIGGER_DELETE)
                .map_err(|e| CoreError::Catalog(format!("fts trigger delete: {e}")))?;
        }

        Ok(Self { conn, db_path })
    }

    /// Import tools from a JSON array (catalog or user).
    pub fn import_tools(&self, entries: &[CatalogToolEntry], source: &str) -> Result<usize, CoreError> {
        let now = epoch_ms();
        let tx = self.conn.unchecked_transaction()
            .map_err(|e| CoreError::Catalog(format!("begin tx: {e}")))?;

        let mut count = 0;
        for entry in entries {
            let trust = entry.security.as_ref()
                .and_then(|s| s.trust_level.clone())
                .unwrap_or_else(|| "community".to_string());

            tx.execute(
                "INSERT OR REPLACE INTO tools (id, name, type, category, languages, frameworks, \
                 description, when_to_use, what_it_finds, install_command, install_package_manager, \
                 install_binary_name, run_command, run_args_template, run_common_flags, risk_level, \
                 requires, cli_support, tags, github, docs, trust_level, source, updated_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
                params![
                    entry.id, entry.name, entry.tool_type,
                    serde_json::to_string(&entry.category).unwrap_or_default(),
                    serde_json::to_string(&entry.languages).unwrap_or_default(),
                    serde_json::to_string(&entry.frameworks).unwrap_or_default(),
                    entry.description,
                    entry.when_to_use,
                    entry.what_it_finds,
                    entry.install.command,
                    entry.install.package_manager,
                    entry.install.binary_name,
                    entry.run.command,
                    entry.run.args_template,
                    serde_json::to_string(&entry.run.common_flags).unwrap_or_default(),
                    entry.risk_level,
                    serde_json::to_string(&entry.requires).unwrap_or_default(),
                    serde_json::to_string(&entry.cli_support).unwrap_or_default(),
                    serde_json::to_string(&entry.tags).unwrap_or_default(),
                    entry.github,
                    entry.docs,
                    trust,
                    source,
                    now as i64,
                ],
            ).map_err(|e| CoreError::Catalog(format!("upsert tool {}: {e}", entry.id)))?;
            count += 1;
        }

        tx.commit().map_err(|e| CoreError::Catalog(format!("commit: {e}")))?;
        Ok(count)
    }

    /// Import from a JSON file path.
    pub fn import_from_file(&self, path: &Path, source: &str) -> Result<usize, CoreError> {
        if !path.exists() { return Ok(0); }
        let content = std::fs::read_to_string(path)?;
        let entries: Vec<CatalogToolEntry> = serde_json::from_str(&content)
            .map_err(|e| CoreError::Catalog(format!("parse {}: {e}", path.display())))?;
        self.import_tools(&entries, source)
    }

    /// Import all catalog files from a directory (languages/, categories/, mcps/).
    pub fn import_catalog_dir(&self, catalog_dir: &Path) -> Result<usize, CoreError> {
        let mut total = 0;
        for subdir in &["languages", "categories", "mcps", "markets"] {
            let dir = catalog_dir.join(subdir);
            if !dir.exists() { continue; }
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().map_or(false, |e| e == "json") {
                    total += self.import_from_file(&path, "catalog")?;
                }
            }
        }
        Ok(total)
    }

    /// Get total tool count.
    pub fn tool_count(&self) -> Result<i64, CoreError> {
        self.conn.query_row("SELECT COUNT(*) FROM tools", [], |row| row.get(0))
            .map_err(|e| CoreError::Catalog(format!("count: {e}")))
    }

    /// Get catalog version from meta.
    pub fn catalog_version(&self) -> Result<i64, CoreError> {
        self.conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM catalog_meta WHERE key='version'",
            [], |row| row.get(0),
        ).unwrap_or(Ok(0)).map_err(|e| CoreError::Catalog(format!("version: {e}")))
    }

    /// Set catalog version in meta.
    pub fn set_catalog_version(&self, version: i64) -> Result<(), CoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES ('version', ?1)",
            params![version.to_string()],
        ).map_err(|e| CoreError::Catalog(format!("set version: {e}")))?;
        Ok(())
    }

    /// Get a single tool by ID with full info.
    pub fn get_tool(&self, id: &str) -> Result<Option<ToolFullInfo>, CoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.*, si.path, si.version FROM tools t \
             LEFT JOIN system_inventory si ON t.install_binary_name = si.binary_name \
             WHERE t.id = ?1"
        ).map_err(|e| CoreError::Catalog(format!("prepare: {e}")))?;

        let result = stmt.query_row(params![id], |row| {
            Ok(ToolFullInfo {
                id: row.get("id")?,
                name: row.get("name")?,
                tool_type: row.get("type")?,
                category: parse_json_array(row.get::<_, String>("category")?),
                languages: parse_json_array(row.get::<_, String>("languages")?),
                frameworks: parse_json_array(row.get::<_, String>("frameworks")?),
                description: row.get("description")?,
                when_to_use: row.get("when_to_use")?,
                what_it_finds: row.get("what_it_finds")?,
                install_command: row.get("install_command")?,
                install_binary_name: row.get("install_binary_name")?,
                run_command: row.get("run_command")?,
                run_common_flags: parse_json_array(row.get::<_, String>("run_common_flags")?),
                risk_level: row.get("risk_level")?,
                requires: parse_json_array(row.get::<_, String>("requires")?),
                github: row.get("github")?,
                docs: row.get("docs")?,
                trust_level: row.get("trust_level")?,
                source: row.get("source")?,
                state: String::new(), // filled by caller
                installed_path: row.get(24)?,
                installed_version: row.get(25)?,
            })
        });

        match result {
            Ok(mut tool) => {
                tool.state = if tool.installed_path.is_some() { "available".to_string() } else { "recommended".to_string() };
                Ok(Some(tool))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Catalog(format!("get_tool: {e}"))),
        }
    }

    /// Suggest tools filtered by languages/category, grouped by state.
    pub fn suggest(
        &self,
        languages: &[String],
        category: Option<&str>,
        tool_type: Option<&str>,
        cli: &str,
        project_root: &str,
        limit: usize,
    ) -> Result<SuggestResult, CoreError> {
        // Build dynamic WHERE clause
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // Language filter: tools that match any project language OR are language-agnostic
        if !languages.is_empty() {
            let lang_conditions: Vec<String> = languages.iter().enumerate().map(|(i, _)| {
                format!("languages LIKE ?{}", param_values.len() + i + 1)
            }).collect();
            for lang in languages {
                param_values.push(Box::new(format!("%\"{lang}\"%")));
            }
            conditions.push(format!("(languages = '[]' OR {})", lang_conditions.join(" OR ")));
        }

        if let Some(cat) = category {
            param_values.push(Box::new(format!("%\"{cat}\"%")));
            conditions.push(format!("category LIKE ?{}", param_values.len()));
        }
        if let Some(tt) = tool_type {
            param_values.push(Box::new(tt.to_string()));
            conditions.push(format!("type = ?{}", param_values.len()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT t.id, t.name, t.type, t.description, t.category, t.trust_level, t.source, \
             t.install_command, si.path IS NOT NULL as is_installed \
             FROM tools t \
             LEFT JOIN system_inventory si ON t.install_binary_name = si.binary_name \
             {where_clause} \
             ORDER BY t.trust_level ASC, t.name ASC"
        );

        let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)
            .map_err(|e| CoreError::Catalog(format!("suggest prepare: {e}")))?;

        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            let id: String = row.get(0)?;
            let is_installed: bool = row.get(8)?;
            Ok(ToolSummary {
                id: id.clone(),
                name: row.get(1)?,
                tool_type: row.get(2)?,
                description: row.get(3)?,
                category: parse_json_array(row.get::<_, String>(4)?),
                trust_level: row.get(5)?,
                source: row.get(6)?,
                install_command: if is_installed { None } else { Some(row.get::<_, String>(7)?) },
                state: String::new(), // set below
            })
        }).map_err(|e| CoreError::Catalog(format!("suggest query: {e}")))?;

        let mut enabled_set = std::collections::HashSet::new();
        {
            let mut estmt = self.conn.prepare(
                "SELECT tool_id FROM enabled_tools WHERE (cli = ?1 OR cli = 'default') AND (project_root = ?2 OR project_root = '')"
            ).map_err(|e| CoreError::Catalog(format!("enabled query: {e}")))?;
            let erows = estmt.query_map(params![cli, project_root], |row| row.get::<_, String>(0))
                .map_err(|e| CoreError::Catalog(format!("enabled: {e}")))?;
            for row in erows {
                if let Ok(id) = row { enabled_set.insert(id); }
            }
        }

        let mut result = SuggestResult {
            enabled: Vec::new(),
            available: Vec::new(),
            recommended: Vec::new(),
        };

        for row in rows {
            if let Ok(mut tool) = row {
                let is_installed = tool.install_command.is_none();
                if enabled_set.contains(&tool.id) {
                    tool.state = "enabled".to_string();
                    if result.enabled.len() < limit { result.enabled.push(tool); }
                } else if is_installed {
                    tool.state = "available".to_string();
                    if result.available.len() < limit { result.available.push(tool); }
                } else {
                    tool.state = "recommended".to_string();
                    if result.recommended.len() < limit { result.recommended.push(tool); }
                }
            }
        }

        Ok(result)
    }

    /// Full-text search using FTS5.
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, CoreError> {
        let fts_query = query.split_whitespace()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");

        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, t.description, t.source, \
             rank * -1.0 as score, \
             si.path IS NOT NULL as is_installed \
             FROM tools_fts fts \
             JOIN tools t ON t.id = fts.id \
             LEFT JOIN system_inventory si ON t.install_binary_name = si.binary_name \
             WHERE tools_fts MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2"
        ).map_err(|e| CoreError::Catalog(format!("fts prepare: {e}")))?;

        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            let is_installed: bool = row.get(5)?;
            Ok(SearchResult {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                source: row.get(3)?,
                score: row.get(4)?,
                state: if is_installed { "available".to_string() } else { "recommended".to_string() },
            })
        }).map_err(|e| CoreError::Catalog(format!("fts query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(r) = row { results.push(r); }
        }
        Ok(results)
    }

    /// Scan system for installed tools using `which`.
    pub fn scan_system_inventory(&self) -> Result<usize, CoreError> {
        let now = epoch_ms();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT install_binary_name FROM tools WHERE install_binary_name IS NOT NULL"
        ).map_err(|e| CoreError::Catalog(format!("scan prepare: {e}")))?;

        let binaries: Vec<String> = stmt.query_map([], |row| row.get(0))
            .map_err(|e| CoreError::Catalog(format!("scan query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut found = 0;
        for binary in &binaries {
            let output = std::process::Command::new("which").arg(binary).output();
            if let Ok(out) = output {
                if out.status.success() {
                    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let version = std::process::Command::new(binary)
                        .arg("--version")
                        .output()
                        .ok()
                        .map(|v| String::from_utf8_lossy(&v.stdout).lines().next().unwrap_or("").to_string());

                    self.conn.execute(
                        "INSERT OR REPLACE INTO system_inventory (binary_name, path, version, last_checked) VALUES (?1, ?2, ?3, ?4)",
                        params![binary, path, version, now as i64],
                    ).map_err(|e| CoreError::Catalog(format!("inventory upsert: {e}")))?;
                    found += 1;
                }
            }
        }
        Ok(found)
    }

    /// Enable a tool for a CLI + project.
    pub fn enable_tool(&self, tool_id: &str, cli: &str, project_root: &str) -> Result<(), CoreError> {
        let now = epoch_ms();
        self.conn.execute(
            "INSERT OR REPLACE INTO enabled_tools (tool_id, cli, project_root, enabled_at) VALUES (?1, ?2, ?3, ?4)",
            params![tool_id, cli, project_root, now as i64],
        ).map_err(|e| CoreError::Catalog(format!("enable: {e}")))?;
        Ok(())
    }

    /// Disable a tool for a CLI + project.
    pub fn disable_tool(&self, tool_id: &str, cli: &str, project_root: &str) -> Result<(), CoreError> {
        self.conn.execute(
            "DELETE FROM enabled_tools WHERE tool_id = ?1 AND cli = ?2 AND project_root = ?3",
            params![tool_id, cli, project_root],
        ).map_err(|e| CoreError::Catalog(format!("disable: {e}")))?;
        Ok(())
    }

    /// Check if a tool is enabled.
    pub fn is_enabled(&self, tool_id: &str, cli: &str, project_root: &str) -> Result<bool, CoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM enabled_tools WHERE tool_id = ?1 AND (cli = ?2 OR cli = 'default') AND (project_root = ?3 OR project_root = '')",
            params![tool_id, cli, project_root],
            |row| row.get(0),
        ).map_err(|e| CoreError::Catalog(format!("is_enabled: {e}")))?;
        Ok(count > 0)
    }

    /// Add a user-defined tool.
    pub fn add_user_tool(&self, entry: &CatalogToolEntry) -> Result<(), CoreError> {
        self.import_tools(&[entry.clone()], "user")?;
        Ok(())
    }

    /// Remove a user-defined tool. Refuses to remove catalog tools.
    pub fn remove_user_tool(&self, tool_id: &str) -> Result<bool, CoreError> {
        let affected = self.conn.execute(
            "DELETE FROM tools WHERE id = ?1 AND source = 'user'",
            params![tool_id],
        ).map_err(|e| CoreError::Catalog(format!("remove: {e}")))?;
        // Also remove from enabled
        let _ = self.conn.execute(
            "DELETE FROM enabled_tools WHERE tool_id = ?1",
            params![tool_id],
        );
        Ok(affected > 0)
    }
}

fn parse_json_array(s: String) -> Vec<String> {
    serde_json::from_str(&s).unwrap_or_default()
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_catalog(dir: &Path) -> ToolCatalog {
        ToolCatalog::open(dir).expect("open catalog")
    }

    fn sample_tool(id: &str, lang: &str) -> CatalogToolEntry {
        CatalogToolEntry {
            id: id.to_string(),
            name: id.to_string(),
            tool_type: "cli".to_string(),
            category: vec!["test".to_string()],
            languages: vec![lang.to_string()],
            frameworks: vec![],
            description: format!("A test tool: {id}"),
            when_to_use: Some("during testing".to_string()),
            what_it_finds: None,
            install: CatalogInstall { command: format!("install {id}"), package_manager: None, binary_name: Some(id.to_string()), url: None },
            run: CatalogRun { command: format!("{id} run"), args_template: None, common_flags: vec![] },
            risk_level: "low".to_string(),
            requires: vec![],
            cli_support: vec!["claude".to_string()],
            tags: vec!["test".to_string()],
            github: None,
            docs: None,
            security: None,
        }
    }

    #[test]
    fn test_import_and_count() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        let tools = vec![sample_tool("tool-a", "rust"), sample_tool("tool-b", "python")];
        let imported = cat.import_tools(&tools, "catalog").unwrap();
        assert_eq!(imported, 2);
        assert_eq!(cat.tool_count().unwrap(), 2);
    }

    #[test]
    fn test_get_tool_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[sample_tool("cargo-test", "rust")], "catalog").unwrap();
        let tool = cat.get_tool("cargo-test").unwrap();
        assert!(tool.is_some());
        let tool = tool.unwrap();
        assert_eq!(tool.id, "cargo-test");
        assert_eq!(tool.languages, vec!["rust"]);
    }

    #[test]
    fn test_suggest_filters_by_language() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[
            sample_tool("rust-tool", "rust"),
            sample_tool("python-tool", "python"),
        ], "catalog").unwrap();
        let result = cat.suggest(&["rust".to_string()], None, None, "claude", "", 10).unwrap();
        // rust-tool should be in recommended (not installed), python-tool should not
        assert!(result.recommended.iter().any(|t| t.id == "rust-tool"));
        assert!(!result.recommended.iter().any(|t| t.id == "python-tool"));
    }

    #[test]
    fn test_fts_search() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[sample_tool("cargo-audit", "rust")], "catalog").unwrap();
        let results = cat.search_fts("audit", 5).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "cargo-audit");
    }

    #[test]
    fn test_enable_disable_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[sample_tool("my-tool", "rust")], "catalog").unwrap();

        assert!(!cat.is_enabled("my-tool", "claude", "").unwrap());
        cat.enable_tool("my-tool", "claude", "").unwrap();
        assert!(cat.is_enabled("my-tool", "claude", "").unwrap());
        cat.disable_tool("my-tool", "claude", "").unwrap();
        assert!(!cat.is_enabled("my-tool", "claude", "").unwrap());
    }

    #[test]
    fn test_add_and_remove_user_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.add_user_tool(&sample_tool("my-custom", "rust")).unwrap();
        assert_eq!(cat.tool_count().unwrap(), 1);

        let tool = cat.get_tool("my-custom").unwrap().unwrap();
        assert_eq!(tool.source, "user");

        assert!(cat.remove_user_tool("my-custom").unwrap());
        assert_eq!(cat.tool_count().unwrap(), 0);
    }

    #[test]
    fn test_remove_refuses_catalog_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[sample_tool("catalog-tool", "rust")], "catalog").unwrap();
        assert!(!cat.remove_user_tool("catalog-tool").unwrap()); // returns false — not removed
        assert_eq!(cat.tool_count().unwrap(), 1); // still there
    }

    #[test]
    fn test_import_from_real_catalog_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        let catalog_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/catalog");
        if catalog_dir.exists() {
            let count = cat.import_catalog_dir(&catalog_dir).unwrap();
            assert!(count > 0, "should import tools from catalog dir");
        }
    }

    #[test]
    fn test_suggest_groups_by_state() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = test_catalog(tmp.path());
        cat.import_tools(&[
            sample_tool("tool-a", "rust"),
            sample_tool("tool-b", "rust"),
        ], "catalog").unwrap();
        cat.enable_tool("tool-a", "claude", "").unwrap();

        let result = cat.suggest(&["rust".to_string()], None, None, "claude", "", 10).unwrap();
        assert!(result.enabled.iter().any(|t| t.id == "tool-a"));
        assert!(result.recommended.iter().any(|t| t.id == "tool-b"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p the-one-core tool_catalog`
Expected: All 8 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-core/
git commit -m "feat: ToolCatalog with SQLite storage, FTS5 search, system scan, enable/disable per CLI"
```

---

## Phase 2: Broker Integration + New API Types

### Task 3: Add API types for 6 new tools

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`

- [ ] **Step 1: Add new request/response types**

Add to `api.rs`:

```rust
// tool.add
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAddRequest {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default = "default_cli")]
    pub tool_type: String,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub description: String,
    pub install_command: String,
    pub run_command: String,
    #[serde(default)]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub github: Option<String>,
    #[serde(default)]
    pub cli: Option<String>,
}
fn default_cli() -> String { "cli".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAddResponse { pub added: bool, pub id: String }

// tool.remove
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRemoveRequest { pub tool_id: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRemoveResponse { pub removed: bool }

// tool.disable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDisableRequest {
    pub tool_id: String,
    pub project_root: String,
    pub project_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDisableResponse { pub disabled: bool }

// tool.install
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstallRequest {
    pub tool_id: String,
    pub project_root: String,
    pub project_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstallResponse {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub auto_enabled: bool,
    pub output: String,
}

// tool.info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfoRequest { pub tool_id: String }
// Response uses ToolFullInfo from tool_catalog

// tool.update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateRequest {}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateResponse {
    pub catalog_version_before: i64,
    pub catalog_version_after: i64,
    pub tools_added: usize,
    pub tools_updated: usize,
    pub system_tools_found: usize,
}

// Updated tool.suggest — now returns grouped results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSuggestResponseV2 {
    pub project_languages: Vec<String>,
    pub enabled: Vec<the_one_core::tool_catalog::ToolSummary>,
    pub available: Vec<the_one_core::tool_catalog::ToolSummary>,
    pub recommended: Vec<the_one_core::tool_catalog::ToolSummary>,
}

// Updated tool.list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListRequest {
    pub state: Option<String>,    // enabled | available | recommended | all
    pub project_root: String,
    pub project_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListResponse {
    pub tools: Vec<the_one_core::tool_catalog::ToolSummary>,
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/the-one-mcp/src/api.rs
git commit -m "feat: API types for tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update"
```

### Task 4: Add broker methods for catalog tools

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/Cargo.toml` (if needed)

- [ ] **Step 1: Add ToolCatalog to McpBroker**

Add a `catalog: tokio::sync::RwLock<Option<ToolCatalog>>` field to `McpBroker`. Initialize it lazily on first use. The catalog is global (not per-project).

```rust
use the_one_core::tool_catalog::{ToolCatalog, CatalogToolEntry, CatalogInstall, CatalogRun};
```

Add a private method:
```rust
async fn with_catalog<R>(&self, f: impl FnOnce(&ToolCatalog) -> R) -> Result<R, CoreError> {
    // Lazy init: open catalog.db in global state dir on first access
    // Cache in self.catalog RwLock
}
```

- [ ] **Step 2: Implement tool.add broker method**

```rust
pub async fn tool_add(&self, request: ToolAddRequest) -> Result<ToolAddResponse, CoreError> {
    let entry = CatalogToolEntry {
        id: request.id.clone(),
        name: request.name,
        tool_type: request.tool_type,
        category: request.category,
        languages: request.languages,
        // ... map all fields
        install: CatalogInstall { command: request.install_command, .. },
        run: CatalogRun { command: request.run_command, .. },
    };
    self.with_catalog(|cat| cat.add_user_tool(&entry)).await??;
    Ok(ToolAddResponse { added: true, id: request.id })
}
```

- [ ] **Step 3: Implement remaining 5 broker methods**

Implement: `tool_remove`, `tool_disable`, `tool_install`, `tool_info`, `tool_update`. Each wraps ToolCatalog methods via `with_catalog`. `tool_install` executes the install command via `tokio::process::Command`.

- [ ] **Step 4: Update tool_suggest to use catalog**

Replace the old CapabilityRegistry-based `tool_suggest` with catalog-based. Load project profile (languages), call `catalog.suggest(...)`, return grouped response.

- [ ] **Step 5: Update tool_search to use catalog FTS**

Replace old implementation with `catalog.search_fts(query, limit)`.

- [ ] **Step 6: Add tests**

Test: tool_add + tool_info, tool_suggest returns grouped, tool_enable + tool_disable, tool_remove refuses catalog tools.

- [ ] **Step 7: Run tests**

Run: `cargo test -p the-one-mcp`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-mcp/
git commit -m "feat: broker catalog integration — tool.add/remove/disable/install/info/update"
```

---

## Phase 3: Transport Layer + Catalog Bootstrap

### Task 5: Add tool definitions and dispatch for new tools

**Files:**
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`

- [ ] **Step 1: Add 6 new tool definitions to tools.rs**

Add definitions for: `tool.add`, `tool.remove`, `tool.disable`, `tool.install`, `tool.info`, `tool.update`. Update the `tool.suggest` and `tool.search` descriptions to reflect catalog-backed behavior. Update `tool.list` to accept `state` filter.

Total: 30 tools.

- [ ] **Step 2: Add dispatch cases to jsonrpc.rs**

Add to `dispatch_tool` match:

```rust
"tool.add" => { ... }
"tool.remove" => { ... }
"tool.disable" => { ... }
"tool.install" => { ... }
"tool.info" => { ... }
"tool.update" => { ... }
```

- [ ] **Step 3: Update tool count in schema validation test**

Update `test_v1beta_schema_files_exist_and_are_valid_json` if it checks tool count.
Update `test_dispatch_tools_list` to expect 30 tools.

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-mcp`
Expected: All tests pass, tools/list returns 30 tools.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-mcp/src/transport/
git commit -m "feat: transport dispatch for 6 new catalog tools (30 total)"
```

### Task 6: Catalog bootstrap on project.init

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Load catalog files on first project.init**

In the broker's `project_init` method, after project state is created, initialize the catalog:

```rust
// Bootstrap catalog if not yet loaded
self.with_catalog(|cat| {
    if cat.tool_count()? == 0 {
        // Import from bundled catalog files
        let catalog_dir = find_catalog_dir(); // tools/catalog/ or ~/.the-one/catalog/
        cat.import_catalog_dir(&catalog_dir)?;
        cat.scan_system_inventory()?;
    }
    Ok::<(), CoreError>(())
}).await??;
```

- [ ] **Step 2: Import user custom tools on init**

After catalog import, import user custom tools:
```rust
let custom_dir = global_state_dir.join("registry/custom");
cat.import_from_file(&custom_dir.join("custom.json"), "user")?;
// Per-CLI custom
let cli_custom = custom_dir.join(format!("custom-{cli}.json"));
cat.import_from_file(&cli_custom, "user")?;
```

- [ ] **Step 3: Add test for catalog bootstrap**

Test: project_init populates catalog with tools from the catalog directory.

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-mcp`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-mcp/src/broker.rs
git commit -m "feat: catalog bootstrap on project.init — imports catalog files + scans system"
```

---

## Phase 4: Qdrant Semantic Search for Tools

### Task 7: Embed tools into Qdrant the_one_tools collection

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: After catalog import, embed tools into Qdrant**

In the catalog bootstrap code, after importing tools:

```rust
// Embed tool descriptions into Qdrant for semantic search
if let Ok(embedding_provider) = self.build_embedding_provider().await {
    let tools = cat.all_tool_descriptions()?; // returns Vec<(id, text_to_embed)>
    let texts: Vec<String> = tools.iter().map(|(_, text)| text.clone()).collect();
    let vectors = embedding_provider.embed_batch(&texts).await?;

    let qdrant = AsyncQdrantBackend::new(&config.qdrant_url, "tools", qdrant_options)?;
    qdrant.ensure_collection(embedding_provider.dimensions()).await?;

    let points: Vec<QdrantPoint> = tools.iter().zip(vectors).map(|((id, _), vector)| {
        QdrantPoint { id: id.clone(), vector, payload: /* tool metadata */ }
    }).collect();
    qdrant.upsert_points(points).await?;
}
```

- [ ] **Step 2: Add all_tool_descriptions to ToolCatalog**

```rust
pub fn all_tool_descriptions(&self) -> Result<Vec<(String, String)>, CoreError> {
    // Returns (id, "description when_to_use what_it_finds tags")
    // This is the text that gets embedded
}
```

- [ ] **Step 3: Add semantic search method to broker**

```rust
async fn search_tools_semantic(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, CoreError> {
    // Embed query → search the_one_tools collection → map results
}
```

- [ ] **Step 4: Update tool_search to try semantic first, fall back to FTS**

```rust
pub async fn tool_search(&self, request: ToolSearchRequest) -> ToolSearchResponse {
    // Try Qdrant semantic search first
    // Fall back to SQLite FTS5 if Qdrant unavailable
}
```

- [ ] **Step 5: Add test for semantic search fallback**

Test: search returns results via FTS when Qdrant is not running.

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-mcp/src/broker.rs crates/the-one-core/src/tool_catalog.rs
git commit -m "feat: Qdrant semantic search for tool catalog with FTS5 fallback"
```

---

## Phase 5: Changelog, Schemas, CI, Docs

### Task 8: Create changelog file and update schemas

**Files:**
- Create: `tools/catalog/_changelog.json`
- Create: `schemas/mcp/v1beta/tool.add.*.schema.json` (+ other new tool schemas)
- Modify: `crates/the-one-mcp/src/lib.rs` (schema count if checked)

- [ ] **Step 1: Create initial changelog**

Create `tools/catalog/_changelog.json`:
```json
[
  {
    "version": 1,
    "date": "2026-04-03",
    "added": [],
    "updated": [],
    "removed": [],
    "deprecated": [],
    "notes": "Initial catalog release"
  }
]
```

- [ ] **Step 2: Create JSON schemas for new tools**

Create request/response schemas for: `tool.add`, `tool.remove`, `tool.disable`, `tool.install`, `tool.info`, `tool.update`, `tool.list` (updated).

- [ ] **Step 3: Update schema count in tests**

Update expected schema file count in validation test.

- [ ] **Step 4: Commit**

```bash
git add tools/catalog/_changelog.json schemas/ crates/the-one-mcp/src/lib.rs
git commit -m "feat: catalog changelog + v1beta schemas for 6 new tool lifecycle methods"
```

### Task 9: Update documentation

**Files:**
- Modify: `README.md`, `AGENTS.md`, `CLAUDE.md`, `CHANGELOG.md`
- Modify: `docs/guides/the-one-mcp-complete-guide.md`
- Modify: `docs/guides/quickstart.md`

- [ ] **Step 1: Update all docs**

Update: tool count (30), catalog system description, tool.suggest grouped response, tool lifecycle (add/remove/enable/disable/install/run), semantic search, system inventory, per-CLI custom tools.

- [ ] **Step 2: Commit**

```bash
git add README.md AGENTS.md CLAUDE.md CHANGELOG.md docs/
git commit -m "docs: update for tool catalog integration — 30 tools, semantic search, system inventory"
```

### Task 10: Final validation + tag

- [ ] **Step 1: Full workspace validation**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
```

- [ ] **Step 2: Commit and tag**

```bash
git tag -a v0.3.0 -m "Tool catalog: SQLite + Qdrant semantic search, 30 MCP tools, system inventory"
git push origin main --tags
```
