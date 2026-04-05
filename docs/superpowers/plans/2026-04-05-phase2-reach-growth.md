# Phase 2: Reach & Growth (v0.10.0)

**Scope:** Expand what the project can do for users (MCP resources API), what the catalog knows (200+ new tools), and how people discover it (landing page).

**Prerequisites:**
- Phase 1 complete (v0.9.0 shipped with tree-sitter + benchmarks)
- Read `2026-04-05-SESSION-HANDOFF.md` first
- Verify baseline: `git log --oneline -5 && cargo test --workspace 2>&1 | grep "^test result"`

**Deliverables:**
- v0.10.0 release: MCP resources API + catalog expansion (~200 tools)
- GitHub Pages landing site at `michelabboud.github.io/the-one-mcp`

---

## Task 2.1: MCP Resources API

### Why

The MCP protocol has three primitives: **tools**, **resources**, and **prompts**. The-one-mcp currently only exposes tools. Claude Code (and other MCP clients) can use resources as first-class context — users can `@`-reference them, drag them into conversations, browse them in the UI.

Currently: to access an indexed doc, an LLM must call `memory.search` (lossy) or `docs.get` (knows the path). Neither is ideal for browsing or reference.

With resources: the LLM can list all docs, read one directly, and the client UI can show them as browsable entries alongside files in the editor.

### MCP Resources protocol

Required MCP methods (read the official MCP spec for details):

1. **`resources/list`** — list all available resources (returns array of `{uri, name, description, mimeType}`)
2. **`resources/read`** — fetch a specific resource by URI (returns array of `{uri, mimeType, text}`)
3. **`resources/subscribe`** (optional) — client wants change notifications
4. **`resources/unsubscribe`** (optional)
5. **`notifications/resources/list_changed`** (optional server-initiated)

### What to expose as resources

**Default resource set** (always available when a project is initialized):

1. **Managed docs** — every file in `<project>/.the-one/docs/` gets a `the-one://docs/<path>` URI
2. **External docs** — if `external_docs_root` is set, its files get `the-one://external/<path>` URIs
3. **Project profile** — `the-one://project/profile` returns the profile JSON
4. **Tool catalog snapshot** — `the-one://catalog/enabled` returns enabled tools for this project

**Optional (gate behind config):**
5. Images — `the-one://images/<hash>` for each indexed image (only if `image_embedding_enabled`)
6. Code chunks — `the-one://chunks/<chunk_id>` for direct chunk access

### URI scheme

`the-one://<resource_type>/<identifier>`

Resource types: `docs`, `external`, `project`, `catalog`, `images`, `chunks`.

### Files to create

- `crates/the-one-mcp/src/resources.rs` — resource URI parsing, listing, reading
- `schemas/mcp/v1beta/resources.list.request.schema.json`
- `schemas/mcp/v1beta/resources.list.response.schema.json`
- `schemas/mcp/v1beta/resources.read.request.schema.json`
- `schemas/mcp/v1beta/resources.read.response.schema.json`
- `docs/guides/mcp-resources.md` — user guide

### Files to modify

- `crates/the-one-mcp/src/transport/jsonrpc.rs` — add `resources/list`, `resources/read` method handlers alongside existing `tools/list`, `tools/call`
- `crates/the-one-mcp/src/broker.rs` — add `resources_list(project_root, project_id)` and `resources_read(project_root, project_id, uri)` methods
- `crates/the-one-mcp/src/bin/the-one-mcp.rs` — the `initialize` handshake response needs to advertise the `resources` capability
- `crates/the-one-mcp/src/lib.rs` — add schemas to expected list (35 to 39)
- `README.md`, `PROGRESS.md`, `CHANGELOG.md`

### Implementation pattern

Create `McpResource`, `ResourcesListResponse`, `ResourcesReadRequest`, `ResourceContent`, `ResourcesReadResponse` structs in `resources.rs` following the `memory.search` pattern from existing API types.

Add a `parse_uri(uri: &str) -> Option<(String, String)>` helper that splits `the-one://type/identifier` into `(type, identifier)`.

In the broker:

- `resources_list()` walks `<project>/.the-one/docs/` with `walkdir`, adds one resource per file. Also adds the project profile and catalog snapshot entries.
- `resources_read()` dispatches on resource type:
  - `docs` — read the file (**reject `..` in identifier for path traversal safety**)
  - `project` — serialize the current profile
  - `catalog` — query the SQLite `enabled_tools` table
  - Return `CoreError::InvalidRequest` for unknown types

### Dispatch wiring

In `transport/jsonrpc.rs`, extend the top-level `dispatch` match:

```
"resources/list" => handle_resources_list(broker, id, request.params).await,
"resources/read" => handle_resources_read(broker, id, request.params).await,
```

Add handler functions that extract params, call broker methods, serialize responses.

### Initialize handshake update

In `handle_initialize`, add `resources` to the `capabilities` object:

```
"capabilities": {
    "tools": {},
    "resources": {
        "subscribe": false,
        "listChanged": false
    }
}
```

### Tests

- `test_resources_list_empty_project` — should return project profile + catalog at minimum
- `test_resources_list_with_managed_docs` — should include each file under `.the-one/docs/`
- `test_resources_read_managed_doc` — reads a known file, verifies content
- `test_resources_read_rejects_path_traversal` — `the-one://docs/../../etc/passwd` returns error
- `test_resources_list_via_jsonrpc` — full dispatch path
- `test_resources_read_unknown_uri_returns_error`

Target: 272 + 6 new tests in Task 2.1.

### Token cost impact

**Zero impact on per-session token cost for tools.** Resources are NOT sent in the `tools/list` response — they are fetched separately. Clients that use resources pay their own cost on `resources/list` which happens once per session.

### Verification

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --no-default-features --features the-one-ui/embed-swagger --all-targets -- -D warnings
THE_ONE_EMBEDDING_MODEL=all-MiniLM-L6-v2 cargo test --workspace 2>&1 | grep "^test result"
```

Manual test via stdio:
```
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | target/release/the-one-mcp serve
echo '{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{"project_root":"'$PWD'","project_id":"test"}}' | target/release/the-one-mcp serve
```

### Docs

- `docs/guides/mcp-resources.md` — ~150 lines covering URI scheme, list/read patterns, client integration examples
- `docs/guides/api-reference.md` — add a new section for resources (they are not tools, so they get their own section)
- `docs/guides/architecture.md` — mention the resources layer

---

## Task 2.2: Catalog Expansion (~200 new tools)

### Why

Current state: 28 curated tools in `tools/catalog/` (mostly Rust + security). User-contributed: 125 total. `tool.find` is currently useful only for Rust projects with security focus.

Expanding to Python, JavaScript/TypeScript, Go, and Java ecosystems would make `tool.find` useful for 90%+ of developer projects.

### Target coverage

**Python (~50 tools):**
- Linters: ruff, flake8, pylint, mypy, pyright, black, isort, autopep8
- Testing: pytest, tox, nose2, hypothesis, pytest-cov, coverage
- Formatters: black, isort, yapf, autoflake
- Security: bandit, safety, pip-audit, semgrep-py
- Build/deps: poetry, uv, pdm, pipenv, rye, hatch
- Docs: sphinx, mkdocs, pydoctor
- Profiling: cProfile, py-spy, memory_profiler, line_profiler
- REPL/notebooks: ipython, jupyter, jupytext
- Env mgmt: pyenv, asdf

**JavaScript/TypeScript (~50 tools):**
- Linters: eslint, tslint, biome, standard
- Formatters: prettier, biome, dprint
- Type checkers: tsc, flow
- Testing: jest, vitest, mocha, playwright, cypress, testing-library
- Build: webpack, rollup, esbuild, vite, swc, parcel, turbopack
- Package managers: npm, yarn, pnpm, bun
- Runtime: node, deno, bun
- Docs: typedoc, jsdoc, storybook
- Security: npm audit, snyk, socket

**Go (~40 tools):**
- Linters: golangci-lint, staticcheck, revive, gosec
- Formatters: gofmt, goimports, gofumpt
- Testing: go test, testify, ginkgo, gomock
- Build: goreleaser, ko
- Security: gosec, govulncheck, nancy
- Profiling: pprof, trace
- Docs: godoc, pkgsite

**Java / Kotlin (~40 tools):**
- Build: maven, gradle, bazel
- Linters: checkstyle, pmd, spotbugs, detekt, ktlint
- Testing: junit, testng, mockito, spock
- Formatters: google-java-format, ktlint, spotless
- Security: dependency-check, snyk, spotbugs
- Docs: javadoc, dokka

### Tool entry schema

Follow the existing `tools/catalog/_schema.json`. Each tool entry has:
- `id` — unique identifier (e.g., "ruff")
- `name` — human-readable name
- `description` — one-line description
- `when_to_use` — LLM hint for when this is the right tool
- `what_it_finds` — what output the user should expect
- `languages` — array of language tags
- `categories` — array like ["lint", "format"]
- `tags` — searchable tags
- `install_command` — shell command to install
- `run_command` — shell command to run
- `risk_level` — "low" | "medium" | "high"
- `github` — repo URL

### Files to create

Check existing layout first with `ls tools/catalog/`. Current convention likely groups per-language:
- `tools/catalog/languages/python.json`
- `tools/catalog/languages/javascript.json`
- `tools/catalog/languages/typescript.json`
- `tools/catalog/languages/go.json`
- `tools/catalog/languages/java.json`
- `tools/catalog/languages/kotlin.json`

Also:
- `tools/catalog/categories/linting.json` (cross-language)
- `tools/catalog/categories/testing.json`
- `tools/catalog/categories/formatting.json`
- `tools/catalog/categories/package-management.json`

### Validation

Each tool entry must:
1. Validate against `tools/catalog/_schema.json`
2. Have a unique `id`
3. Have accurate install/run commands (tested on at least one platform)
4. Use allowed values for `risk_level` (low/medium/high)
5. Reference real, currently maintained GitHub repos

Write `scripts/validate-catalog.sh` that iterates all JSON files and validates each.

### Import into the SQLite catalog

The catalog is imported into `~/.the-one/catalog.db` via `tool.refresh` (in the `maintain` multiplexed tool). After adding new JSON files, running `maintain: action: tool.refresh` will re-import.

For tests, add a unit test per language file that parses it and verifies minimum count:

```
#[test]
fn test_catalog_python_tools_parse() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().join("tools/catalog/languages/python.json");
    let content = std::fs::read_to_string(&path).expect("read python.json");
    let tools: Vec<ToolDefinition> = serde_json::from_str(&content).expect("parse");
    assert!(tools.len() >= 40);
    assert!(tools.iter().any(|t| t.id == "ruff"));
    assert!(tools.iter().any(|t| t.id == "pytest"));
}
```

### Approach for gathering tools

Tedious but mechanical. Split into sub-tasks:

1. **Research phase** — for each language, compile from:
   - Awesome lists (awesome-python, awesome-go, etc.)
   - Popular VS Code extensions
   - Build system docs (package.json, pyproject.toml, go.mod conventions)
2. **Drafting phase** — create JSON entries in batches of 10
3. **Validation phase** — ensure install/run commands work
4. **Import phase** — run `tool.refresh`, verify SQLite, spot-check semantic search

### Docs

- Update `tool-catalog.md` guide with new counts
- Update `README.md` stats: "28 curated tools" to "~230 curated tools"
- Update `CONTRIBUTING.md` with the JSON schema and submission process

---

## Task 2.3: Landing Page via GitHub Pages

### Why

Current state: README is dev-focused, plain text. No visual identity. No way for a non-developer to understand "what is this, why should I care, how do I try it".

A landing page would:
- Explain the value proposition in 10 seconds
- Show actual usage via a short video/GIF
- Display benchmark numbers (from Phase 1)
- Drive installs via the curl one-liner
- Link to the catalog browser
- Build credibility

### Scope

One static HTML page hosted at `https://michelabboud.github.io/the-one-mcp`. No JavaScript frameworks — keep it minimal.

### Structure

```
docs-site/
  index.html           # Landing page
  style.css            # Minimal styles
  og-image.png         # Social share image
  favicon.ico
  demo.gif             # 30-second usage demo
  catalog-browser/     # Optional: static catalog browser
    index.html
    tools.json         # Exported from catalog.db
```

### Content sections

1. **Hero** — Title, tagline, curl install command, download link, demo gif
2. **Features** — 6-tile grid: unlimited memory, hybrid search, tool catalog, multimodal, code chunking, auto-reindex
3. **Install** — one-command install + supported platforms
4. **Benchmarks** — table from Phase 1 benchmarks/results.md
5. **Catalog** — link to catalog browser
6. **Footer** — Apache-2.0, GitHub link

### Styling approach

Use a minimal CSS library like Pico.css or simple.css for instant polish. No JS frameworks. Hand-rolled CSS in `style.css` with:
- CSS variables for colors
- Grid for feature tiles
- Responsive breakpoints at 768px and 1024px
- Dark mode via `prefers-color-scheme`

### Demo recording

Create `demo.gif`:
1. Install asciinema: `brew install asciinema`
2. Install agg (asciinema to gif): `cargo install --git https://github.com/asciinema/agg`
3. Record: `asciinema rec demo.cast`
4. Inside the recording:
   - Start Claude Code
   - Ask: "Search my docs for how the watcher works"
   - Show `memory.search` results
   - Ask: "What linting tools do we have for this project?"
   - Show `tool.find` results
5. Convert: `agg demo.cast demo.gif`
6. Optimize: `gifsicle -O3 --lossy=80 demo.gif -o demo.gif` (target under 2MB)

### GitHub Pages setup

1. Create `docs-site/` directory in repo root
2. Push to main
3. GitHub repo Settings to Pages
4. Source: Deploy from a branch to `main` to `/docs-site`
5. Custom domain (optional)
6. Verify at `https://michelabboud.github.io/the-one-mcp/`

### Catalog browser (stretch)

Small static HTML that loads `tools.json` (exported from `catalog.db`) and provides client-side search/filter.

Export script:
```
sqlite3 ~/.the-one/catalog.db "SELECT json_object('id', id, 'name', name, 'description', description) FROM tools" > docs-site/catalog-browser/tools.json
```

Client-side search uses vanilla DOM APIs (createElement, textContent — **do NOT use innerHTML with user input**, XSS risk).

### Verification

```
python3 -m http.server -d docs-site 8000
```

Open `http://localhost:8000` and check:
- Page loads in under 1 second
- Mobile responsive (Chrome DevTools device mode)
- All links work
- Demo gif plays
- No console errors

### Update README

Add badge:
```
[![Landing page](https://img.shields.io/badge/landing-michelabboud.github.io-blue)](https://michelabboud.github.io/the-one-mcp)
```

---

## Phase 2 Release Sequence

### v0.10.0 (Tasks 2.1 + 2.2)

Bundle MCP resources API + catalog expansion. These are the user-facing changes.

CHANGELOG entry:
```
## [0.10.0] - YYYY-MM-DD

### Added
- MCP resources API: `resources/list`, `resources/read`. Expose managed docs, project profile, and catalog as native MCP resources.
- the-one:// URI scheme for resource addressing
- Catalog expansion: ~200 new tools covering Python, JavaScript/TypeScript, Go, Java, Kotlin ecosystems
- docs/guides/mcp-resources.md

### Changed
- Initialize handshake advertises `resources` capability
- Catalog tool count: 28 -> ~230
```

Release:
```
echo "v0.10.0" > VERSION
git add -A
git commit -m "feat: v0.10.0 — MCP resources + catalog expansion"
git tag -a v0.10.0 -m "v0.10.0: MCP resources + catalog expansion"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.10.0
```

### Landing page (Task 2.3)

Lands to `main` + GitHub Pages. NOT tied to a version release. Can ship independently.

```
git add docs-site/
git commit -m "docs: add landing page at michelabboud.github.io/the-one-mcp"
git push origin main
```

---

## Phase 2 success criteria

- [ ] v0.10.0 ships 6/6 platforms
- [ ] `resources/list` and `resources/read` JSON-RPC methods work
- [ ] Claude Code shows indexed docs as resources in its UI (manual verification)
- [ ] Catalog has 200+ entries across 5+ languages
- [ ] Landing page live at `https://michelabboud.github.io/the-one-mcp`
- [ ] Demo GIF plays inline on landing page
- [ ] Benchmarks from Phase 1 visible on landing page
- [ ] Tests still green (target: ~340 after Task 2.1 adds ~6 resource tests + catalog parsing tests)
