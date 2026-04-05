# Custom Tools Guide

> Custom tools let you extend the tool catalog with your own scripts, internal CLIs, or
> project-specific commands. They live alongside the curated catalog and are never
> overwritten by catalog updates.

## Two Ways to Add Custom Tools

### 1. Via the MCP (LLM does it)

Ask the LLM to register a tool using the `config` MCP tool with `action: tool.add`.
This is the fastest path — no file editing required:

```json
{
  "tool": "config",
  "params": {
    "action": "tool.add",
    "params": {
      "id": "my-linter",
      "name": "my-linter",
      "description": "Internal linter for company style rules",
      "install_command": "pip install my-linter",
      "run_command": "my-linter check .",
      "risk_level": "low",
      "tags": ["lint", "internal"]
    }
  }
}
```

The broker writes the entry to `~/.the-one/registry/custom.json` (shared across all CLIs)
and immediately enables it for the current session.

To remove a tool added this way:

```json
{
  "tool": "config",
  "params": {
    "action": "tool.remove",
    "params": { "tool_id": "my-linter" }
  }
}
```

Note: `tool.remove` only works on user-added tools (source `"user"`). It refuses to remove
tools that came from the curated catalog.

### 2. Via JSON Files (manual)

Edit the registry JSON files directly. This is useful for batch additions, scripting, or
managing tools under version control alongside your dotfiles.

## Custom Tool File Locations

All custom tool files live in `~/.the-one/registry/`:

```
~/.the-one/registry/
├── custom.json           # Shared across all CLIs
├── custom-claude.json    # Claude Code only
├── custom-gemini.json    # Gemini CLI only
├── custom-opencode.json  # OpenCode only
└── custom-codex.json     # Codex only
```

Files are loaded in order: the per-CLI file is merged on top of `custom.json`. A tool
defined in `custom-claude.json` is invisible to Gemini CLI.

Use `custom.json` for tools that make sense everywhere. Use per-CLI files for tools that
are CLI-specific — for example, a tool that wraps Claude's native capabilities, or a
Gemini-only workflow.

The install script (`scripts/install.sh`) creates empty stubs for all these files on
first run.

## JSON Schema for Custom Tools

Custom tool entries use the same schema as catalog entries (`tools/catalog/_schema.json`).
All fields:

```json
{
  "id": "my-tool",               // Required. Lowercase, hyphenated. Must be unique.
  "name": "my-tool",             // Required. Display name.
  "type": "cli",                 // Required. cli | lsp | mcp | framework | library | extension
  "category": ["testing"],       // Required. Array of category strings.
  "description": "...",          // Required. One-line summary for LLM consumption.
  "install": {
    "command": "pip install ...", // Required. Shell command to install.
    "binary_name": "my-tool"     // Optional. Binary to scan for in system inventory.
  },
  "run": {
    "command": "my-tool run .",   // Required. Shell command to run.
    "args_template": "my-tool {flags} {path}", // Optional. Parameterized template.
    "common_flags": ["--verbose"]  // Optional. Flags shown in tool.info.
  },
  "languages": ["python"],        // Optional. Languages this tool targets.
  "frameworks": ["django"],       // Optional. Relevant frameworks.
  "when_to_use": "...",           // Optional. LLM guidance on when to suggest this.
  "what_it_finds": "...",         // Optional. What the tool outputs or detects.
  "risk_level": "low",            // Optional. low | medium | high. Default: low.
  "requires": ["python"],         // Optional. Prerequisites that must be installed.
  "tags": ["lint", "internal"],   // Optional. Search keywords.
  "github": "https://...",        // Optional. Source repository URL.
  "docs": "https://..."           // Optional. Documentation URL.
}
```

## Example: Add a Custom Linter

A company-internal Python style checker, shared across all CLIs:

**`~/.the-one/registry/custom.json`**

```json
[
  {
    "id": "acme-linter",
    "name": "acme-linter",
    "type": "cli",
    "category": ["lint", "qa"],
    "languages": ["python"],
    "description": "ACME Corp internal Python style checker with company-specific rules",
    "when_to_use": "Before committing Python code in any ACME project",
    "what_it_finds": "Style violations, import ordering issues, internal API misuse",
    "install": {
      "command": "pip install acme-linter --extra-index-url https://pypi.acme.internal",
      "binary_name": "acme-linter"
    },
    "run": {
      "command": "acme-linter check .",
      "common_flags": ["--strict", "--format json"]
    },
    "risk_level": "low",
    "tags": ["lint", "internal", "acme", "python"]
  }
]
```

## Example: Add a Custom Test Runner

A project-specific test harness registered only for Claude Code:

**`~/.the-one/registry/custom-claude.json`**

```json
[
  {
    "id": "integration-tests",
    "name": "Integration Test Suite",
    "type": "cli",
    "category": ["testing"],
    "languages": [],
    "description": "Run the full integration test suite against a local stack",
    "when_to_use": "After feature work, before raising a PR. Requires Docker running.",
    "what_it_finds": "Integration failures, regressions in end-to-end user flows",
    "install": {
      "command": "echo 'No install needed — bundled with repo'",
      "binary_name": "bash"
    },
    "run": {
      "command": "bash scripts/integration-tests.sh",
      "args_template": "bash scripts/integration-tests.sh {flags}",
      "common_flags": ["--parallel", "--fail-fast"]
    },
    "risk_level": "medium",
    "requires": ["docker", "bash"],
    "tags": ["testing", "integration", "docker"]
  }
]
```

## Example: Add an MCP Server

Register an MCP server that the LLM can invoke:

**`~/.the-one/registry/custom.json`** (add to the array)

```json
{
  "id": "mcp-filesystem-extended",
  "name": "Filesystem MCP (Extended)",
  "type": "mcp",
  "category": ["automation"],
  "description": "Extended filesystem MCP with archive support and directory diffing",
  "when_to_use": "When working with archives (.zip, .tar) or comparing directory trees",
  "install": {
    "command": "npm install -g @acme/mcp-filesystem-extended"
  },
  "run": {
    "command": "mcp-filesystem-extended"
  },
  "risk_level": "medium",
  "tags": ["mcp", "filesystem", "archives"]
}
```

## Security: The Policy Gate

Every custom tool execution passes through the policy engine. The `risk_level` field
controls how the gate behaves:

| Risk Level | Default Behavior |
|------------|-----------------|
| `low` | Auto-approved — runs immediately |
| `medium` | Auto-approved by default (configurable to require approval) |
| `high` | Requires explicit user approval before each run |

When a tool requires approval, the `tool.run` call must include:
- `interactive: true` — allows the broker to prompt the user
- `approval_scope` — how long the approval persists

### Approval Scopes

```json
{
  "tool": "tool.run",
  "params": {
    "project_root": "/path/to/project",
    "project_id": "my-project",
    "action_key": "my-high-risk-tool",
    "interactive": true,
    "approval_scope": "session"
  }
}
```

| Scope | Meaning |
|-------|---------|
| `once` | Approve this single run only |
| `session` | Approve for the rest of the current session |
| `forever` | Permanently approve (written to session state) |

### Headless Mode

When running without an interactive terminal (CI, automated agents), `high`-risk tools are
denied by default unless a `session` or `forever` approval already exists from a prior
interactive session. Set `risk_level: "medium"` or lower for tools that should run
unattended.

## Advanced: Per-CLI Specialization

Per-CLI custom tool files let you register different tools — or different `run` commands —
for different coding assistants.

### Example: Rust-specific tool for Claude Code only

You use `cargo-add` to add dependencies interactively, but only when Claude Code is driving:

**`~/.the-one/registry/custom-claude.json`**

```json
[
  {
    "id": "cargo-add",
    "name": "cargo-add",
    "type": "cli",
    "category": ["build"],
    "languages": ["rust"],
    "description": "Add a dependency to Cargo.toml from the command line",
    "when_to_use": "When the user asks to add a Rust dependency without editing Cargo.toml manually",
    "install": {
      "command": "cargo install cargo-edit",
      "binary_name": "cargo-add"
    },
    "run": {
      "command": "cargo add",
      "args_template": "cargo add {crate_name}"
    },
    "risk_level": "low",
    "tags": ["rust", "dependencies", "cargo"]
  }
]
```

### Example: Different run commands per CLI

The same tool might need different arguments depending on which CLI is running it.
Register it twice — once per CLI file — with different `run.command` values:

**`~/.the-one/registry/custom-gemini.json`**
```json
[{ "id": "my-tool", "run": { "command": "my-tool --gemini-mode" }, ... }]
```

**`~/.the-one/registry/custom-claude.json`**
```json
[{ "id": "my-tool", "run": { "command": "my-tool --claude-mode" }, ... }]
```

## Removing Custom Tools

### Via MCP

```json
{
  "tool": "config",
  "params": {
    "action": "tool.remove",
    "params": { "tool_id": "my-linter" }
  }
}
```

This removes the entry from the database. It does not modify the JSON file on disk.

### Via JSON File

Open the relevant file in `~/.the-one/registry/` and delete the entry from the array.
Run `maintain` with `action: tool.refresh` to sync the database:

```json
{
  "tool": "maintain",
  "params": {
    "action": "tool.refresh",
    "params": { "project_root": "/path/to/project", "project_id": "my-project" }
  }
}
```

## Viewing All Custom Tools

List only user-added tools by filtering for the `user` source (via `tool.find list`).
Or inspect the file directly:

```bash
cat ~/.the-one/registry/custom.json
cat ~/.the-one/registry/custom-claude.json
```

Custom tools appear in `tool.find` results alongside catalog tools, distinguished by
`"source": "user"` in their metadata. The LLM sees them identically to catalog tools.
