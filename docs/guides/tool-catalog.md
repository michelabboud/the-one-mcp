# Tool Catalog Guide

> The tool catalog is the central registry of developer tools, LSPs, and MCP servers managed
> by the-one-mcp. It helps you discover, install, and run the right tools for your project
> without drowning in noise.
>
> **v0.14.0 update:** catalog expanded to **365 tools** across 10 languages
> + 8 cross-language categories (+248 new from the baseline 28). Every
> language file and every category file is now populated with curated,
> schema-validated entries.

## What Is the Tool Catalog?

The tool catalog is a curated, searchable database of developer tools organized by language,
category, and type. It covers CLI tools, language servers (LSPs), MCP servers, and more —
each entry enriched with metadata that helps the LLM make smart recommendations.

Key properties:

- **Curated quality bar** — every entry has been reviewed for correctness, security, and
  usefulness. Automated GitHub Actions checks enforce schema compliance and flag archived
  or unmaintained repos.
- **LLM-optimized metadata** — each tool carries `description`, `when_to_use`, and
  `what_it_finds` fields written specifically for LLM consumption, not just human readers.
- **Project-aware filtering** — the catalog knows your project's languages and frameworks
  and surfaces only what's relevant. You never see a Python linter in a Rust project.
- **Offline-capable** — the catalog is stored locally in SQLite at `~/.the-one/catalog.db`.
  No network required after initial setup.

## Catalog Architecture

### Source of Truth: `tools/catalog/` in the Repository

The canonical catalog lives in the `tools/catalog/` directory of the the-one-mcp repository,
organized into subdirectories:

```
tools/catalog/
├── _schema.json           # JSON Schema for all entries
├── _index.json            # Catalog metadata and version
├── _changelog.json        # Change history
├── languages/             # Language-specific tools (10 files, 294 total)
│   ├── rust.json          # 24 tools
│   ├── python.json        # 50 tools
│   ├── javascript.json    # 50 tools (covers TypeScript too)
│   ├── go.json            # 40 tools
│   ├── java.json          # 40 tools
│   ├── kotlin.json        # 15 tools
│   ├── ruby.json          # 20 tools
│   ├── php.json           # 20 tools
│   ├── swift.json         # 15 tools
│   └── cpp.json           # 20 tools (covers C too)
├── categories/            # Cross-language category tools
│   ├── security.json
│   ├── testing.json
│   ├── ci-cd.json
│   ├── cloud.json
│   ├── databases.json
│   ├── docs.json
│   ├── monitoring.json
│   └── automation.json
└── mcps/                  # MCP server entries
    ├── official.json
    └── community.json
```

### Local Storage: SQLite at `~/.the-one/catalog.db`

When `setup` (action: `project`) runs (or `maintain` with `action: tool.refresh`), the broker imports
all catalog JSON files into a local SQLite database at `~/.the-one/catalog.db`. This database
has two search mechanisms:

- **FTS5 full-text search** — keyword search over name, description, tags, and `when_to_use`
- **Qdrant semantic search** — embedding-based similarity search for natural-language queries
  (falls back to FTS5 if Qdrant is unavailable)

The database also tracks:
- **System inventory** — which tools are installed on your machine (detected via `which`)
- **Enabled state** — which tools are active for a specific CLI and project root
- **Catalog metadata** — version and last-refresh timestamp

### The Discovery Funnel

Token efficiency is a first-class concern. The LLM never sees the full catalog — only what's
relevant to the current project:

```
CATALOG (thousands)
    |  filter by project languages/frameworks
    v
RELEVANT (hundreds)
    |  filter by system inventory (which)
    v
AVAILABLE (dozens)
    |  filter by enabled state + CLI
    v
ENABLED (few)         ← what the LLM can run right now
```

## Tool States

Every tool in the catalog has one of four states, evaluated in context of the current project
and CLI:

| State | Meaning |
|-------|---------|
| `recommended` | In catalog but not installed on your system |
| `available` | Installed on your system but not enabled for this CLI/project |
| `enabled` | Installed and active for the current CLI and project |

State is per-CLI and per-project-root. The same tool can be enabled for Claude Code but not
for Gemini CLI, or enabled in one project but not another.

## Trust Levels

Each catalog entry carries a `trust_level` that indicates how thoroughly it has been vetted:

| Trust Level | Meaning |
|-------------|---------|
| `verified` | Official tool, well-known project, regularly security-checked |
| `community` | Community-submitted, passed validation, reasonable star count |
| `unverified` | Community-submitted, passed schema validation, low star count |
| `deprecated` | Repo archived or no commits in 12+ months |
| `warning` | Known security advisories or active CVEs |

## Browsing the Catalog

All catalog interaction goes through the `tool.find` MCP tool, which supports three modes.

### List All Tools

```json
{
  "tool": "tool.find",
  "params": {
    "project_root": "/path/to/project",
    "project_id": "my-project",
    "mode": "list",
    "filter": "all"
  }
}
```

Filter values for `list` mode:

| Filter | Returns |
|--------|---------|
| `all` | Every tool in the catalog |
| `enabled` | Tools currently enabled for this CLI/project |
| `available` | Tools installed but not yet enabled |
| `recommended` | Tools in catalog but not installed (curated picks) |

### Keyword Search

Use `mode: "search"` with a natural-language or keyword query. The broker runs Qdrant
semantic search first, falls back to FTS5 keyword search if semantic search is unavailable:

```json
{
  "tool": "tool.find",
  "params": {
    "project_root": "/path/to/project",
    "project_id": "my-project",
    "mode": "search",
    "query": "audit dependencies for CVEs",
    "max": 10
  }
}
```

### Smart Recommendations

Use `mode: "suggest"` to get recommendations tailored to your project's detected languages
and frameworks. Results are grouped into `enabled`, `available`, and `recommended` buckets:

```json
{
  "tool": "tool.find",
  "params": {
    "project_root": "/path/to/project",
    "project_id": "my-project",
    "mode": "suggest",
    "query": "security scanning"
  }
}
```

Example response structure:

```json
{
  "enabled": [
    { "id": "cargo-clippy", "name": "cargo clippy", "state": "enabled", ... }
  ],
  "available": [
    { "id": "cargo-audit", "name": "cargo-audit", "state": "available", ... }
  ],
  "recommended": [
    { "id": "cargo-deny", "name": "cargo-deny", "state": "recommended", ... },
    { "id": "semgrep", "name": "semgrep", "state": "recommended", ... }
  ]
}
```

### Get Full Tool Metadata

Use `tool.info` to retrieve complete metadata for a specific tool, including install command,
run command, risk level, GitHub link, and detected installation path:

```json
{
  "tool": "tool.info",
  "params": {
    "tool_id": "cargo-audit"
  }
}
```

## Tool Lifecycle

### 1. Discover

```
tool.find mode="suggest"  →  project-aware recommendations
tool.find mode="search"   →  keyword/semantic search
tool.find mode="list"     →  list by state
```

### 2. Install

Use `tool.install` to run the tool's install command and automatically enable it:

```json
{
  "tool": "tool.install",
  "params": {
    "tool_id": "cargo-audit",
    "project_root": "/path/to/project",
    "project_id": "my-project"
  }
}
```

After install, the broker runs `which <binary>` to confirm the binary is present, adds it to
the system inventory, and enables it for the current CLI and project.

### 3. Enable / Disable

For tools that are already installed but not enabled:

```json
{ "tool": "maintain", "params": { "action": "tool.enable",
  "params": { "project_root": "/path/to/project", "family": "cargo-audit" } } }

{ "tool": "maintain", "params": { "action": "tool.disable",
  "params": { "tool_id": "cargo-audit", "project_root": "/path/to/project",
               "project_id": "my-project" } } }
```

### 4. Run

Use `tool.run` to execute an enabled tool. The policy engine evaluates the tool's `risk_level`
before execution:

```json
{
  "tool": "tool.run",
  "params": {
    "project_root": "/path/to/project",
    "project_id": "my-project",
    "action_key": "cargo-audit",
    "interactive": true,
    "approval_scope": "session"
  }
}
```

See the [policy gate section](#the-policy-gate) for details on approval scopes.

### The Policy Gate

Every `tool.run` call passes through the policy engine before execution. The engine checks
the tool's `risk_level`:

| Risk Level | Default Behavior |
|------------|-----------------|
| `low` | Auto-approved, runs immediately |
| `medium` | Auto-approved (configurable) |
| `high` | Requires explicit approval |

When approval is required, the `approval_scope` parameter determines how long the approval
lasts:

| Scope | Duration |
|-------|---------|
| `once` | Approval consumed after one run |
| `session` | Approved for the entire current session |
| `forever` | Permanently approved (stored in state) |

In headless mode (no interactive terminal), `high`-risk tools are denied unless a prior
`forever` or `session` approval exists.

## Updating the Catalog

To pull the latest catalog from GitHub and re-scan your system for newly installed tools:

```json
{
  "tool": "maintain",
  "params": {
    "action": "tool.refresh",
    "params": { "project_root": "/path/to/project", "project_id": "my-project" }
  }
}
```

This performs two operations:
1. Downloads the latest `tools/catalog/` JSON files from the repository
2. Re-runs the system inventory scan (`which <binary>` for every catalog entry)

The catalog refresh also triggers automatically on `setup` (action: `refresh`) if the project
fingerprint has changed since the last scan.

## System Inventory

The broker maintains a system inventory by running `which <binary_name>` for every tool
in the catalog. This is how it distinguishes `recommended` (not installed) from `available`
(installed but not enabled) states.

The inventory is updated:
- On `setup` (action: `project`) — first run for a project
- On `setup` (action: `refresh`) when the fingerprint changes
- On `tool.install` (after a successful install)
- On `maintain` with `action: tool.refresh` (explicit refresh)

## Contributing Tools

The catalog is community-maintained. Contributions are accepted via GitHub pull requests.

### Required Fields

Every entry must satisfy the JSON Schema in `tools/catalog/_schema.json`. Required fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique identifier, lowercase hyphenated (e.g., `cargo-audit`) |
| `name` | string | Display name |
| `type` | string | One of: `cli`, `lsp`, `mcp`, `framework`, `library`, `extension` |
| `category` | array | Categories: `build`, `test`, `lint`, `qa`, `security`, `ci-cd`, `database`, `cloud`, `docs`, `automation`, `monitoring` |
| `description` | string | One-line description for LLM consumption |
| `install.command` | string | Shell command to install the tool |
| `run.command` | string | Shell command to run the tool |

### Recommended Fields

| Field | Type | Description |
|-------|------|-------------|
| `languages` | array | Languages this tool targets (empty = language-agnostic) |
| `when_to_use` | string | Guidance for the LLM on when to suggest this tool |
| `what_it_finds` | string | What issues or results this tool produces |
| `risk_level` | string | `low`, `medium`, or `high` |
| `tags` | array | Keywords for search |
| `github` | string | GitHub repository URL |

### Example Entry

```json
{
  "id": "cargo-audit",
  "name": "cargo-audit",
  "type": "cli",
  "category": ["qa", "security"],
  "languages": ["rust"],
  "description": "Audit Cargo.lock for crates with known security vulnerabilities",
  "when_to_use": "Before releases, in CI, after dependency updates",
  "what_it_finds": "CVEs in dependencies, yanked crates, unmaintained packages",
  "install": {
    "command": "cargo install cargo-audit",
    "binary_name": "cargo-audit"
  },
  "run": {
    "command": "cargo audit",
    "common_flags": ["--json", "--deny warnings"]
  },
  "risk_level": "low",
  "tags": ["security", "dependencies", "audit", "cve"],
  "github": "https://github.com/rustsec/rustsec"
}
```

### Submission Workflow

1. Fork the repository
2. Add your entry to the appropriate file under `tools/catalog/languages/` or
   `tools/catalog/categories/`. Create a new file if no appropriate one exists.
3. Validate locally: `cat tools/catalog/languages/rust.json | python3 -m json.tool`
4. Open a pull request. GitHub Actions will:
   - Validate JSON schema compliance
   - Check for duplicate IDs
   - Verify required fields are present
   - Confirm the GitHub URL resolves (if provided)
5. A maintainer reviews and merges. The new tool becomes available on the next
   `maintain tool.refresh` run.

See `CONTRIBUTING.md` for the full contribution guide.
