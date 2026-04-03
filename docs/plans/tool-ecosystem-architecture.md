# Tool Ecosystem Architecture

**Date:** 2026-04-03
**Status:** Active — Layers 1-4, 7 in progress. Layers 5-6 future.

## Vision

The-One MCP manages a curated catalog of thousands of developer tools, LSPs, and MCP servers. It shows each developer only what's relevant to their project, installed on their system, and enabled for their CLI — while maintaining a comprehensive knowledge base of the best tools ever built.

## The Funnel

```
CATALOG (thousands)         Every quality tool, curated with rich metadata
    ↓ filter by project profile
RELEVANT (hundreds)         Tools that match your languages/frameworks/infra
    ↓ filter by installed state
AVAILABLE (dozens)          Tools actually present on your system
    ↓ filter by enabled state + CLI
ENABLED (few)               Tools the LLM can execute right now
```

The LLM never sees the full catalog. Token efficiency by design.

## Seven Layers

### Layer 1: Official Marketplaces (auto-synced)

Aggregate tool/MCP listings from each CLI's official registry:
- Claude Code plugin marketplace
- Gemini CLI extensions
- OpenCode plugin registry
- modelcontextprotocol.io MCP server registry

Each entry tagged with which CLIs support it. Auto-refreshed from official sources.

**Status:** Buildable now (JSON files, GitHub Actions for periodic sync)

### Layer 2: Curated Known-Good Tools (community-maintained)

The core catalog — thousands of tools organized by:
- Language (Rust, Python, JS, Go, Java, C++, ...)
- Category (testing, security, CI/CD, databases, cloud, ...)
- Type (CLI tool, LSP, MCP server, framework, library)

Each with LLM-optimized metadata:
```json
{
  "id": "cargo-audit",
  "name": "cargo-audit",
  "type": "cli",
  "category": ["qa", "security"],
  "languages": ["rust"],
  "frameworks": [],
  "description": "Audit Cargo.lock for crates with known security vulnerabilities",
  "when_to_use": "Before releases, in CI, after dependency updates",
  "what_it_finds": "CVEs in dependencies, yanked crates, unmaintained packages",
  "install": {
    "command": "cargo install cargo-audit",
    "package_manager": "cargo",
    "binary_name": "cargo-audit"
  },
  "run": {
    "command": "cargo audit",
    "args_template": "cargo audit {flags}",
    "common_flags": ["--json", "--deny warnings"]
  },
  "risk_level": "low",
  "requires": ["cargo"],
  "cli_support": ["claude", "gemini", "opencode", "codex"],
  "tags": ["security", "dependencies", "audit", "cve", "supply-chain"],
  "github": "https://github.com/rustsec/rustsec",
  "docs": "https://rustsec.org",
  "security": {
    "verified": true,
    "last_checked": "2026-04-03",
    "github_stars": 1200,
    "last_commit": "2026-03-15",
    "cve_count": 0,
    "archived": false
  },
  "ratings": {
    "quality": 4.8,
    "reliability": 4.9,
    "community_votes": 342
  }
}
```

**Status:** Buildable now (JSON files in repo, PRs for additions)

### Layer 3: User Custom Tools (per-machine, per-CLI)

```
~/.the-one/registry/
├── custom.json              # User's shared tools (all CLIs)
├── custom-claude.json       # Claude Code only
├── custom-gemini.json       # Gemini CLI only
├── custom-opencode.json     # OpenCode only
└── custom-codex.json        # Codex only
```

Same JSON schema as Layer 2. Never overwritten by catalog updates. User owns these files.

**Status:** Built (install.sh creates them)

### Layer 4: Community Contributions (GitHub-based)

Two contribution paths:

**Path A: GitHub Pages Submission Form (low friction)**
A static GitHub Pages site with a tool submission form. User fills in fields → form generates a JSON entry → auto-creates a GitHub Issue or PR via GitHub API. No git knowledge required.

**Path B: Direct Pull Request (developer-friendly)**
1. Fork the repo
2. Add tool entry to appropriate catalog file
3. Open PR
4. GitHub Actions validates:
   - JSON schema compliance
   - Required fields present (id, name, type, category, description, install, run)
   - No duplicate IDs
   - GitHub URL resolves (if provided)
   - Security check passes (Layer 7)
5. Maintainer reviews and merges
6. Next `install.sh --update` or `project.refresh` picks up the new tool

Contribution template:
```bash
# tools/catalog/languages/rust.json — add your tool
{
  "id": "your-tool-id",
  "name": "your-tool",
  "type": "cli",
  "category": ["testing"],
  "languages": ["rust"],
  "description": "What it does",
  "when_to_use": "When to use it",
  "install": { "command": "cargo install your-tool" },
  "run": { "command": "your-tool run" },
  "github": "https://github.com/you/your-tool"
}
```

**Status:** Buildable now (PR template, GitHub Actions validator)

### Layer 5: Community Marketplace (FUTURE — needs backend)

A web-based marketplace where users can:
- Browse tools by category, language, rating
- Submit tools without PRs (web form → auto-PR)
- Rate and review tools (1-5 stars + text review)
- See install counts and usage analytics
- Follow tool authors for updates

Tech stack: web frontend + API + database (Supabase/Postgres)

**Status:** Future — requires backend infrastructure

### Layer 6: Community Markets (FUTURE — needs backend)

Multiple curated "markets" or "collections":
- "Essential Rust Toolbox" (community-curated list)
- "Security First" (all security-focused tools)
- "Startup Stack" (tools for fast-moving teams)
- "Enterprise Compliance" (tools for regulated environments)

Users can create, share, and subscribe to markets.

**Status:** Future — requires backend + auth

### Layer 7: Security Verification (GitHub-based)

Every tool with a `github` field gets automatically checked:

**On submission (GitHub Actions):**
- Repo exists and is not archived
- Has recent commits (< 1 year)
- No known CVEs in GitHub Security Advisories
- License is OSI-approved
- Has minimum star count (configurable, e.g., > 10 for community, > 100 for curated)

**Periodic re-check (GitHub Actions cron):**
- Weekly scan of all catalog entries
- Flag newly archived repos
- Flag repos with new CVE advisories
- Flag repos with no commits in 12+ months
- Update `security.last_checked` timestamp

**Trust levels:**
```
verified     — Official tool, well-known, regularly checked
community    — Community-submitted, passed validation, >50 stars
unverified   — Community-submitted, passed schema, low star count
deprecated   — Repo archived or unmaintained (>12 months)
warning      — Known security issues
```

**Status:** Buildable now (GitHub Actions for validation + cron re-check)

## Tool Lifecycle in the MCP

### Discovery Flow

```
User: "I need to check my code for security issues"
  ↓
LLM calls: tool.suggest({ query: "security", category: "qa" })
  ↓
Broker:
  1. Load catalog → filter by project profile (Rust)
  2. Filter by category ("qa/security")
  3. Check installed state (which cargo-audit, which trivy, ...)
  4. Check enabled state for current CLI
  5. Return structured response
  ↓
LLM sees:
  ENABLED:  cargo-clippy (running)
  AVAILABLE: cargo-audit (installed, not enabled), trivy (installed)
  RECOMMENDED: cargo-deny, semgrep, cargo-geiger (not installed)
  ↓
LLM decides: "Let me enable cargo-audit and run it"
  tool.enable("cargo-audit")
  tool.run("cargo-audit")
  ↓
Results → LLM analyzes → reports to user
```

### MCP Tools for Tool Lifecycle

| Tool | Description |
|------|-------------|
| `tool.suggest` | Search catalog filtered by project profile, return categorized results |
| `tool.search` | Free-text search across entire catalog |
| `tool.enable` | Activate a tool for current CLI session/project |
| `tool.disable` | Deactivate a tool |
| `tool.install` | Run install command for a tool, add to system inventory |
| `tool.run` | Execute an enabled tool with arguments |
| `tool.info` | Get full metadata for a specific tool |
| `tool.list` | List tools by state: enabled / available / recommended |
| `tool.update` | Re-scan system for installed tools, refresh catalog |

### State Storage

```
~/.the-one/
├── registry/
│   ├── catalog/               # Downloaded from GitHub (auto-updated)
│   │   ├── _index.json
│   │   ├── markets/
│   │   ├── languages/
│   │   ├── categories/
│   │   └── mcps/
│   ├── system-inventory.json  # Auto-scanned installed tools
│   ├── enabled/
│   │   ├── default.json       # Enabled for all CLIs
│   │   ├── claude.json
│   │   ├── gemini.json
│   │   ├── opencode.json
│   │   └── codex.json
│   └── custom/
│       ├── custom.json
│       ├── custom-claude.json
│       └── ...
```

### Auto-Refresh

| Trigger | Action |
|---------|--------|
| `install.sh` | Download full catalog from GitHub |
| `project.init` | Scan system inventory, match to catalog |
| `project.refresh` | Re-scan if fingerprint changed |
| `tool.update` | Force re-scan system + pull latest catalog |
| Weekly cron (if configured) | Pull latest catalog silently |

## Catalog Size Targets

| Phase | Tool Count | Timeline |
|-------|-----------|----------|
| Seed | ~200 | Now — covers top languages + categories |
| Growth | ~1,000 | 3 months — community PRs + market scraping |
| Mature | ~5,000 | 6 months — automated discovery + community |
| Scale | ~10,000+ | 12 months — marketplace + multiple markets |

## Implementation Phases

### Phase A: Catalog Structure + Seed (NOW)
- Define JSON schema for tool entries
- Create catalog directory structure
- Seed 200 tools covering: Rust, Python, JS/TS, Go, Java
- Seed 30 MCPs from modelcontextprotocol.io
- Seed 15 LSPs for common languages
- Create GitHub Actions for PR validation

### Phase B: Detection + Lifecycle (NOW)
- System inventory scanner (which + version detection)
- Enable/disable per CLI in broker
- tool.suggest with funnel filtering
- tool.install execution
- tool.list by state

### Phase C: Community Contributions (NEXT)
- PR template for tool submissions
- GitHub Actions validator (schema + security)
- Periodic security re-check cron
- CONTRIBUTING.md with instructions

### Phase D: Marketplace (FUTURE)
- Web frontend for browsing
- Rating/review system
- User accounts + submissions
- Analytics + install counts

## Security Policy

1. **No tool is auto-installed** — user must explicitly `tool.install`
2. **No tool is auto-enabled** — user must explicitly `tool.enable`
3. **High-risk tools require approval** — flagged by risk_level
4. **All executions are audited** — audit.events tracks every tool.run
5. **Catalog entries are validated** — schema + security checks on every PR
6. **Periodic re-verification** — weekly cron checks GitHub security advisories
7. **Trust levels displayed** — user sees verified/community/unverified/warning
