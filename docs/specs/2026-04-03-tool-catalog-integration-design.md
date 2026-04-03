# Tool Catalog Integration — Design Spec

**Date:** 2026-04-03
**Status:** Approved
**Depends on:** Production overhaul (v0.2.0), tool ecosystem architecture

---

## Goal

Integrate the curated tool catalog into the MCP broker so that `tool.suggest` returns project-aware, install-state-aware, CLI-aware recommendations from thousands of tools — with semantic search powered by the same fastembed + Qdrant infrastructure used for document RAG.

## Non-Goals

- Community marketplace with user accounts (Layer 5-6, future)
- Web UI for catalog browsing (future)
- Auto-installation without user consent

---

## Architecture Overview

```
GitHub (source of truth)
  tools/catalog/*.json          Curated tool definitions
  _changelog.json               Per-version diffs
  _index.json                   Version + metadata
  GitHub Release assets:
    catalog-snapshot-vN.db      Pre-built SQLite (all tools + metadata)
    catalog-vectors-vN.snapshot Pre-built Qdrant vectors (all embeddings)

Install / First Run:
  Download pre-built DB + vectors → import → ready instantly
  Zero embedding cost on user's machine

Updates (tool.update or project.refresh):
  Fetch _changelog.json → compute diff since local version
  Download only changed entries → upsert SQLite + re-embed into Qdrant

Runtime:
  tool.suggest → Qdrant payload filter (language/category) + project profile
  tool.search  → Qdrant semantic search OR SQLite FTS5 fallback
  tool.add     → Insert into SQLite (source='user') + embed into Qdrant
```

---

## Storage Layout

```
~/.the-one/
├── catalog.db                     SQLite: tools, FTS, inventory, enabled state, metadata
├── config.json                    Global config
├── registry/
│   └── custom/                    User tools (JSON, user-editable, imported into DB)
│       ├── custom.json            Shared across all CLIs
│       ├── custom-claude.json
│       ├── custom-gemini.json
│       ├── custom-opencode.json
│       └── custom-codex.json
└── .fastembed_cache/              ONNX model cache

Qdrant collections:
  the_one_tools                    GLOBAL — all tool embeddings
  the_one_{project_id}             PER-PROJECT — document chunk embeddings
```

---

## SQLite Schema: catalog.db

```sql
CREATE TABLE tools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,              -- cli | lsp | mcp | framework | library | extension
    category TEXT NOT NULL,          -- JSON array: ["qa", "security"]
    languages TEXT NOT NULL,         -- JSON array: ["rust"] or [] for language-agnostic
    frameworks TEXT DEFAULT '[]',    -- JSON array: ["docker", "kubernetes"]
    description TEXT NOT NULL,
    when_to_use TEXT,
    what_it_finds TEXT,
    install_command TEXT NOT NULL,
    install_package_manager TEXT,
    install_binary_name TEXT,
    run_command TEXT NOT NULL,
    run_args_template TEXT,
    run_common_flags TEXT,           -- JSON array
    risk_level TEXT DEFAULT 'low',   -- low | medium | high
    requires TEXT DEFAULT '[]',      -- JSON array of prerequisites
    cli_support TEXT DEFAULT '[]',   -- JSON array: ["claude","gemini","opencode","codex"]
    tags TEXT DEFAULT '[]',          -- JSON array for search
    github TEXT,
    docs TEXT,
    trust_level TEXT DEFAULT 'community', -- verified | community | unverified | deprecated | warning
    source TEXT DEFAULT 'catalog',   -- catalog | user
    catalog_version INTEGER,         -- version when this entry was last updated
    updated_at INTEGER NOT NULL      -- epoch ms
);

-- Full-text search (fallback when Qdrant unavailable)
CREATE VIRTUAL TABLE tools_fts USING fts5(
    id, name, description, when_to_use, what_it_finds, tags,
    content='tools', content_rowid='rowid'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER tools_ai AFTER INSERT ON tools BEGIN
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END;

CREATE TRIGGER tools_au AFTER UPDATE ON tools BEGIN
    DELETE FROM tools_fts WHERE rowid = old.rowid;
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END;

CREATE TRIGGER tools_ad AFTER DELETE ON tools BEGIN
    DELETE FROM tools_fts WHERE rowid = old.rowid;
END;

-- System inventory: what's actually installed on this machine
CREATE TABLE system_inventory (
    binary_name TEXT PRIMARY KEY,
    path TEXT,                       -- result of `which`
    version TEXT,                    -- result of `<binary> --version`
    last_checked INTEGER NOT NULL    -- epoch ms
);

-- Enabled tools per CLI per project
CREATE TABLE enabled_tools (
    tool_id TEXT NOT NULL,
    cli TEXT NOT NULL,               -- claude | gemini | opencode | codex | default
    project_root TEXT DEFAULT '',    -- empty string = global, path = project-specific
    enabled_at INTEGER NOT NULL,
    PRIMARY KEY (tool_id, cli, project_root)
);

-- Catalog version tracking
CREATE TABLE catalog_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Keys: version, last_updated, last_checked, source_url
```

---

## Qdrant: the_one_tools Collection

```
Collection: the_one_tools
  Vector: 384 dimensions (fastembed all-MiniLM-L6-v2) or matching configured model
  Distance: Cosine
  HNSW: m=16, ef_construct=100

Each point:
  id:      hash of tool.id (u64)
  vector:  embedding of "{description} {when_to_use} {what_it_finds} {tags joined}"
  payload:
    tool_id:    "cargo-audit"
    type:       "cli"
    category:   ["qa", "security"]
    languages:  ["rust"]
    tags:       ["security", "audit", "cve"]
    trust_level: "verified"
    source:     "catalog"
```

---

## MCP Tool Surface (Updated)

### Existing (unchanged)
| Tool | Description |
|------|-------------|
| `tool.run` | Execute an enabled tool with approval gate |

### Updated
| Tool | Description |
|------|-------------|
| `tool.suggest` | Smart recommendations from catalog + user tools, filtered by project profile, grouped by install state (enabled / available / recommended) |
| `tool.search` | Free-text or semantic search across catalog + user tools, unscoped |
| `tool.enable` | Activate a tool for current CLI / project |
| `tool.list` | List tools by state: enabled, available, recommended, all |

### New
| Tool | Description |
|------|-------------|
| `tool.add` | Add a custom tool definition locally (user source) |
| `tool.remove` | Remove a user-added custom tool |
| `tool.disable` | Deactivate a tool for current CLI / project |
| `tool.install` | Run install command, update system inventory, auto-enable |
| `tool.info` | Full metadata for a specific tool by ID |
| `tool.update` | Refresh catalog from GitHub + re-scan system inventory |

---

## Request / Response Types

### tool.suggest

```json
// Request
{
  "project_root": "/home/user/myproject",
  "project_id": "myproject",
  "category": "qa",           // optional filter
  "type": "cli",              // optional: cli | lsp | mcp
  "limit": 10                 // optional, per group
}

// Response
{
  "project_languages": ["rust"],
  "project_frameworks": ["docker", "github-actions"],
  "enabled": [
    { "id": "cargo-clippy", "name": "cargo clippy", "type": "cli", "description": "...", "category": ["lint","qa"] }
  ],
  "available": [
    { "id": "cargo-audit", "name": "cargo audit", "type": "cli", "description": "...", "install_state": "installed", "category": ["qa","security"] }
  ],
  "recommended": [
    { "id": "cargo-deny", "name": "cargo deny", "type": "cli", "description": "...", "install_command": "cargo install cargo-deny", "category": ["qa","security"] }
  ]
}
```

### tool.search

```json
// Request
{
  "query": "check dependencies for security issues",
  "type": "cli",              // optional filter
  "limit": 10
}

// Response
{
  "results": [
    { "id": "cargo-audit", "name": "cargo audit", "score": 0.92, "description": "...", "state": "available", "source": "catalog" },
    { "id": "snyk", "name": "Snyk CLI", "score": 0.87, "description": "...", "state": "not_installed", "source": "catalog" },
    { "id": "my-custom-checker", "name": "...", "score": 0.71, "description": "...", "state": "enabled", "source": "user" }
  ]
}
```

### tool.add

```json
// Request
{
  "id": "my-custom-tool",
  "name": "My Custom Tool",
  "type": "cli",
  "category": ["testing"],
  "languages": ["rust"],
  "description": "Runs custom validation on our Rust project",
  "install": { "command": "cargo install my-tool" },
  "run": { "command": "my-tool check" },
  "cli": "claude"             // optional: add to specific CLI, default = all
}

// Response
{ "added": true, "id": "my-custom-tool" }
```

### tool.install

```json
// Request
{
  "tool_id": "cargo-deny",
  "project_root": "/home/user/myproject",
  "project_id": "myproject"
}

// Response
{
  "installed": true,
  "binary_path": "/home/user/.cargo/bin/cargo-deny",
  "version": "0.14.3",
  "auto_enabled": true,
  "output": "... install stdout ..."
}
```

### tool.info

```json
// Request
{ "tool_id": "cargo-audit" }

// Response — full metadata entry
{
  "id": "cargo-audit",
  "name": "cargo audit",
  "type": "cli",
  "category": ["qa", "security"],
  "languages": ["rust"],
  "description": "Audit Cargo.lock for crates with known security vulnerabilities",
  "when_to_use": "Before releases, in CI, after dependency updates",
  "what_it_finds": "CVEs in dependencies, yanked crates, unmaintained packages",
  "install": { "command": "cargo install cargo-audit", "binary_name": "cargo-audit" },
  "run": { "command": "cargo audit", "common_flags": ["--json", "--deny warnings"] },
  "risk_level": "low",
  "requires": ["cargo"],
  "github": "https://github.com/rustsec/rustsec",
  "trust_level": "verified",
  "source": "catalog",
  "state": "available",
  "installed_path": "/home/user/.cargo/bin/cargo-audit",
  "installed_version": "0.20.0"
}
```

### tool.list

```json
// Request
{
  "state": "enabled",         // enabled | available | recommended | all
  "project_root": "/home/user/myproject",
  "project_id": "myproject"
}

// Response
{
  "tools": [
    { "id": "cargo-clippy", "name": "cargo clippy", "state": "enabled", "source": "catalog" },
    { "id": "my-tool", "name": "My Tool", "state": "enabled", "source": "user" }
  ]
}
```

### tool.update

```json
// Request (no params needed)
{}

// Response
{
  "catalog_version_before": 45,
  "catalog_version_after": 47,
  "tools_added": 20,
  "tools_updated": 5,
  "tools_deprecated": 1,
  "system_inventory_scanned": 142,
  "system_tools_found": 38
}
```

---

## Data Flow: Install

```
1. Download catalog-snapshot-vN.db from GitHub Release
   → Copy to ~/.the-one/catalog.db
   → Contains all tools + FTS index, zero embedding needed

2. Download catalog-vectors-vN.snapshot from GitHub Release
   → Import into Qdrant "the_one_tools" collection
   → Contains all pre-computed embeddings

3. Scan system inventory
   → For each tool in catalog: `which <binary_name>`
   → Store results in system_inventory table

4. Import user custom tools
   → Read ~/.the-one/registry/custom/*.json
   → Upsert into tools table with source='user'
   → Embed into Qdrant

5. Set catalog_meta: version=N, last_checked=now
```

---

## Data Flow: Diff Update

```
1. Fetch _index.json from GitHub → get remote version
2. If remote version == local version → done
3. Fetch _changelog.json → find entries between local and remote version
4. Collect all added/updated/deprecated tool IDs from changelog
5. Fetch full entries for those IDs from catalog JSON files
6. Upsert into SQLite tools table
7. Re-embed only changed entries → upsert into Qdrant
8. Mark deprecated entries with trust_level='deprecated'
9. Update catalog_meta: version=remote, last_checked=now
10. Re-scan system inventory for any newly added tools
```

---

## Data Flow: tool.suggest

```
1. Load project profile (languages, frameworks) from project DB
2. Determine current CLI from clientInfo
3. Query SQLite:
   - Filter: languages overlap with project languages OR languages is []
   - Filter: category matches request (if specified)
   - Filter: type matches request (if specified)
4. For each result, determine state:
   - enabled: EXISTS in enabled_tools for this CLI + project
   - available: EXISTS in system_inventory (installed but not enabled)
   - recommended: NOT in system_inventory (not installed)
5. Group by state, sort each group by trust_level then name
6. Clamp each group to limit
7. Return grouped response
```

---

## Data Flow: tool.search (semantic)

```
1. Embed query using fastembed
2. Search Qdrant "the_one_tools" collection:
   - vector similarity
   - optional payload filter: type, languages
   - top_k = limit
3. For each result:
   - Fetch full entry from SQLite by tool_id
   - Determine install state from system_inventory
   - Determine enabled state from enabled_tools
4. Return results with scores and state
```

---

## Data Flow: tool.search (FTS fallback, no Qdrant)

```
1. Query SQLite FTS5: tools_fts MATCH '<query>'
2. Rank by BM25 score
3. Enrich with install/enabled state
4. Return results
```

---

## Data Flow: tool.install

```
1. Look up tool by ID in SQLite
2. Check risk_level → if high, require approval (same policy as tool.run)
3. Execute install.command via subprocess
4. Capture stdout/stderr
5. Run `which <binary_name>` → update system_inventory
6. Run `<binary_name> --version` → store version
7. Auto-enable for current CLI + project
8. Record audit event
9. Return result with binary path, version, output
```

---

## GitHub CI: Catalog Build Pipeline

Triggered on push to `tools/catalog/**`:

```yaml
- Validate all JSON files against _schema.json
- Detect changed tool IDs (git diff)
- Auto-set updated_at on changed entries
- Auto-append to _changelog.json
- Auto-increment version in _index.json
- Build catalog-snapshot-vN.db:
    - Import all catalog JSON into SQLite
    - Build FTS5 index
- Build catalog-vectors-vN.snapshot:
    - Embed all tool descriptions with fastembed
    - Export Qdrant collection snapshot
- Attach both to GitHub Release
- Update _index.json total_tools count
```

---

## Fallback Strategy

| Component | Available | Fallback |
|-----------|-----------|----------|
| Qdrant | Running | Semantic search on tool descriptions |
| Qdrant | Not running | SQLite FTS5 text search |
| Pre-built DB | Downloaded | Instant import, no local embedding |
| Pre-built DB | Download fails | Import from catalog JSON files + embed locally |
| System inventory | `which` works | Accurate install state |
| System inventory | `which` fails | All tools shown as "not installed" |

---

## New Files

```
crates/the-one-core/src/tool_catalog.rs      — ToolCatalog struct, SQLite operations, system scan
crates/the-one-mcp/src/api.rs                — New request/response types
crates/the-one-mcp/src/broker.rs             — New broker methods
crates/the-one-mcp/src/transport/tools.rs    — Updated tool definitions (30 tools total)
crates/the-one-mcp/src/transport/jsonrpc.rs  — Dispatch for new tools
tools/catalog/_changelog.json                — Version diff log
.github/workflows/catalog-build.yml          — CI for catalog validation + snapshot
```

---

## Updated MCP Tool Count

Existing: 24 tools
New: 6 tools (tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update)
Total: **30 MCP tools**
