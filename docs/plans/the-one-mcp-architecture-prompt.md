# The-One MCP Architecture Prompt

You are an expert systems architect and senior Rust engineer. Your job is to design and help implement **the-one**, a central MCP-based broker for multiple AI coding CLIs such as **Claude Code** and **Codex**.

This document defines the intended architecture, design goals, constraints, and implementation direction. Treat it as the source-of-truth prompt for planning, architecture, implementation, and refinement.

---

## 1. High-level vision

Build **one central MCP system** called **the-one** that acts as:

- a **tool broker**
- a **memory broker**
- a **RAG/search broker**
- a **project profiler**
- a **router** for skills, agents, plugins, local CLI tools, and MCP-backed tools
- a **configuration and maintenance hub**

The goal is to avoid loading huge amounts of tools, memory, instructions, or context into Claude Code, Codex, or other CLIs up front.

Instead:

- expose a **small always-available core**
- detect the active project
- suggest or expose only what the project needs
- keep heavy data in external storage
- keep retrieval on demand
- support dynamic tool discovery / suggestion
- minimize token/context waste

This is a **single-user, multi-project** system.

---

## 2. Core architectural philosophy

### The main principle

**Anything always loaded is expensive.**

Therefore:

- keep always-loaded instructions tiny
- keep memory files tiny
- move workflows into skills or on-demand docs
- keep RAG outside the live context window
- expose tools progressively
- use the router to choose minimal capabilities

### The system should prefer

1. local preprocessing / filtering
2. skills / playbooks
3. narrow MCP exposure
4. dynamic tool suggestion
5. RAG search only when needed
6. raw doc access only when needed

---

## 3. Multi-CLI strategy

Design for **multiple AI CLIs from day one**.

### Recommended shape

Build:

- **one shared backend/core**
- **one central MCP server**
- **one Claude Code satellite/adaptor**
- **one Codex satellite/adaptor**

### Do NOT build

- two separate backends
- two separate memory systems
- two separate registries
- two separate RAG systems

### Rationale

Claude Code and Codex overlap heavily, but their extension surfaces differ.

Use:

- one shared engine
- one shared MCP surface
- thin CLI-specific adapters

---

## 4. Major components

### 4.1 `the-one-core`

Shared business logic.

Responsibilities:

- project profiling
- registry of capabilities
- routing / tool-family ranking
- memory/RAG ingestion
- search and retrieval
- policy engine
- cache invalidation
- config generation
- maintenance orchestration

### 4.2 `the-one-mcp`

The central MCP broker server.

It should expose a **small public surface**, not a flat dump of all tools.

Suggested MCP tools/resources/prompts:

- `project.init`
- `project.refresh`
- `project.profile.get`
- `memory.search`
- `memory.fetch_chunk`
- `docs.list`
- `docs.get`
- `docs.get_section`
- `tool.search`
- `tool.suggest`
- `tool.enable`
- `tool.run`
- `config.export`

### 4.3 CLI satellites

#### Claude Code satellite

Contains:

- plugin packaging
- MCP server connection config
- Claude hooks
- Claude-specific skill wrappers
- minimal Claude guidance

#### Codex satellite

Contains:

- plugin packaging
- MCP server connection config
- Codex-specific skill wrappers
- minimal `AGENTS.md`
- tool-search-friendly behavior

### 4.4 Optional local web UI

A web UI embedded into the Rust binary.

Responsibilities:

- configure models/providers
- manage enabled tool families
- inspect project profile
- inspect memory/RAG state
- run maintenance tasks
- trigger init / refresh / reindex
- export configs
- view logs

The UI is for humans. The MCP is for machine consumption.

---

## 5. Storage strategy

Use **both global and per-project storage**.

### 5.1 Global storage: `~/.the-one/`

This is the heavy store and true system home.

Store here:

- SQLite database
- Qdrant data
- embeddings cache
- global registry
- model/provider config
- logs
- backups
- shared cache files
- maintenance metadata

Example:

```text
~/.the-one/
  state.db
  qdrant/
  cache/
  logs/
  models/
  registry/
  backups/
```

### 5.2 Project-local stub: `<repo>/.the-one/`

This is the light project-specific manifest/cache.

Store here:

- `project.json`
- `overrides.json`
- `fingerprint.json`
- `pointers.json`
- optional project template/export file

Example:

```text
my-repo/
  .the-one/
    project.json
    overrides.json
    fingerprint.json
    pointers.json
```

### 5.3 Storage rationale

Do not store all heavy RAG/index data in every repo.

That would:

- bloat repos
- duplicate embeddings
- complicate backups
- make clones heavy
- create Git hygiene issues

Do not keep all project-specific steering only in the home directory either.

That would:

- reduce portability
- hide per-project configuration
- make bootstrapping less transparent

### Final recommendation

- **heavy truth lives in `~/.the-one/`**
- **project-specific steering lives in `<repo>/.the-one/`**

---

## 6. Database and retrieval stack

### 6.1 SQLite

Use SQLite as the **control plane database**.

Store:

- project profiles
- overrides
- capability metadata
- registry metadata
- routing history
- cache indexes
- maintenance task state
- local settings
- audit trail
- per-project generation/version info

#### SQLite mode

- enable **WAL mode**
- use `PRAGMA data_version` or equivalent invalidation strategy when needed

SQLite is appropriate because this system is:

- single-user
- multi-project
- local-first
- low-ops

### 6.2 Qdrant

Use Qdrant as the **retrieval plane**.

Store:

- document chunks
- dense vectors
- sparse vectors / hybrid retrieval signals if used
- chunk metadata
- collection-level project segmentation

Use Qdrant for:

- semantic search
- hybrid retrieval
- top-k candidate generation
- RAG retrieval over docs, runbooks, notes, and structured memory

### 6.3 Memory cache

Use an **in-process memory cache** in the Rust service.

Cache:

- hot project profiles
- enabled tool-family maps
- hot doc/chunk metadata
- top memory search results
- selected raw markdown sections
- hot routing decisions

#### Desired behavior

Once data is read from SQLite, keep it in memory until there is a relevant write.

Recommended model:

- read-through cache
- write-through or write-invalidate
- per-project / per-entity invalidation
- optional LRU + TTL for larger objects

Do not cache everything forever.

---

## 7. RAG and docs strategy

The MCP should manage **all docs + RAG**.

### Canonical rule

The-one owns ingestion, indexing, and retrieval.

### It must expose two access modes

#### A. Retrieval mode

- semantic search
- filtered search
- chunk fetch
- snippet-based retrieval
- small, high-signal results

#### B. Raw document mode

- access the original plain Markdown file
- return a specific section
- return a bounded range
- list related docs

### Why both are needed

RAG alone is not enough.

Sometimes the model needs:

- exact wording
- headings / structure
- frontmatter
- code fences
- nearby surrounding context
- the original markdown source

### Design rule

- **RAG for discovery**
- **raw markdown access for precision**

### Important constraint

Do not dump giant files by default.

Prefer:

- top 3-5 hits
- section-level access
- bounded content windows
- explicit full-doc fetch only when truly needed

---

## 8. Router and nano LLM

The-one should include a **router**.

### Router duties

- classify the current request
- decide whether memory search is needed
- rank tool families
- decide whether to use local CLI, skill, MCP tool, plugin action, or agent
- decide whether dormant tools should be suggested
- set risk mode

### Nano LLM policy

A nano model is allowed and recommended, but only as a **router/classifier**, not as a full second agent.

It should:

- classify intent
- rank capability families
- choose local vs remote path
- decide whether retrieval is necessary
- help with dynamic suggestion

It should NOT:

- ramble
- do large reasoning traces
- become a second assistant
- recursively orchestrate the whole system

### Provider model

Support multiple backends:

- API provider
- local provider via Ollama
- local provider via LM Studio
- rules-only fallback

### Strong recommendation

Ship with:

- **rules-first routing**
- nano model as optional enhancement
- local or remote model selectable by config

Never make the nano model the only brain.

---

## 9. Capability registry

You have a real catalog of over 100 existing things across:

- skills
- agents
- plugins
- MCPs
- local CLI tools

Do not represent them as a flat list.

Build a **capability catalog**.

### Each capability should have metadata such as

- `id`
- `title`
- `type` (`skill`, `agent`, `mcp_tool`, `plugin_action`, `cli`)
- `family`
- `project_tags`
- `language_tags`
- `risk_level`
- `cost_hint`
- `description`
- `trigger_hints`
- `provider`
- `install_state`
- `enabled_state`
- `visibility_mode`

### Visibility modes

#### `core`
Always available.

#### `project`
Enabled after project init/profile.

#### `dormant`
Hidden until explicitly requested or suggested by router.

This is essential.

---

## 10. Project init and profile caching

The-one should run a project **init** phase.

### Init should inspect signals such as

- top-level files
- language markers
- framework markers
- build/test files
- package managers
- Docker files
- CI workflows
- infra markers
- cloud/vendor markers
- dangerous or production indicators

### Init should produce

- project type
- languages
- frameworks
- tooling stack
- CI/CD hints
- infra hints
- risk profile
- recommended tool families
- hidden/dormant families

### Cache the result

Store init output in the project-local `.the-one/` folder.

This avoids re-running full init every session.

### Cache discipline

Treat project profile files as **cache/manifest**, not as prompt memory.

### Use fingerprinting

The system should compute a fingerprint from relevant project files, such as:

- `package.json`
- lockfiles
- `pyproject.toml`
- `go.mod`
- `Cargo.toml`
- `.github/workflows/*`
- `Dockerfile*`
- Terraform files
- other high-signal files

On session/init/startup:

- if fingerprint unchanged: reuse project profile
- if fingerprint changed: rerun init and rewrite profile

### Separate detected state from user choices

Use separate files for:

- detected project state
- user overrides/customizations

Do not let re-init erase user customizations.

---

## 11. Memory files strategy (AGENTS.md / CLAUDE.md)

Memory files should be treated as **boot instructions only**.

### Rules

- keep them tiny
- only store things needed in almost every session
- do not stuff operational docs or runbooks into them

### Suitable content

- personal defaults
- repo build/test/lint commands
- dangerous-path warnings
- short architecture facts
- style/risk preferences

### Unsuitable content

- long tutorials
- giant tool catalogs
- deployment playbooks
- cloud runbooks
- troubleshooting guides
- large configuration references

### Important distinction

Use:

- memory files for stable, always-relevant instructions
- `.the-one/*.json` for machine state and cache
- docs/skills for procedures and workflows

### Additional note for Codex

A plain reference inside `AGENTS.md` to another `.md` file should be treated as a hint, not assumed to auto-load that file.

### Additional note for Claude Code

A special import syntax may exist in Claude-specific instruction files, but imported content still costs startup context and should be used carefully.

---

## 12. Claude Code adapter guidance

Claude Code should integrate via a dedicated satellite.

### Claude-specific pieces

- plugin package
- MCP connection config
- minimal Claude memory/instructions
- Claude skills
- hooks where useful

### Hook philosophy

Use hooks only where they help, such as:

- bootstrap / session awareness
- tool interception/steering
- context minimization

Do NOT rely on hooks alone as the architecture.

The central MCP remains the main source of truth.

Claude satellite should be thin.

---

## 13. Codex adapter guidance

Codex should integrate via a dedicated satellite.

### Codex-specific pieces

- plugin package
- MCP connection config
- minimal `AGENTS.md`
- Codex-specific skills
- use dynamic tool loading / tool search style behavior where possible

Do not assume Codex behaves exactly like Claude Code.

Its integration surface differs.

Again, keep the satellite thin.

---

## 14. Embedded web UI inside the Rust binary

Yes, embed a website inside the Rust binary.

### Purpose

Provide a local admin/configuration UI.

### What it should do

- configure providers/models
- enable/disable capability families
- inspect project profile
- inspect docs/RAG state
- view logs
- trigger init / refresh / reindex
- perform maintenance tasks
- export configuration files
- write to SQLite
- manage backups

### What it should NOT do

- replace the MCP protocol
- become the only control path
- leak giant config state into prompt memory

### UI hosting model

Run local HTTP on loopback from the same Rust binary.

Recommended shape:

- one binary
- embedded static assets
- local REST/JSON handlers
- no separate deployment burden for the UI

---

## 15. Maintenance and management operations

The-one should support maintenance operations such as:

- project init
- project refresh
- reindex docs
- prune stale cache
- backup SQLite
- snapshot/backup Qdrant
- validate registry entries
- inspect orphaned references
- export/import settings
- rebuild project profile
- health checks

These should be invokable via:

- local UI
- CLI/admin command
- possibly MCP admin tools if safe

---

## 16. Policy and safety boundaries

The system should implement hard limits such as:

- maximum suggested tools per step
- maximum enabled tool families per project
- maximum search hits returned by default
- maximum raw document size returned by default
- confirmation/deny policy for high-risk tools
- stale-profile detection and refresh policy
- timeout/retry policy for providers and tools

The system must be cost-aware and token-aware.

---

## 17. Suggested Rust workspace/module layout

```text
the-one/
  core/
    registry/
    profiler/
    router/
    memory/
    docs/
    policy/
    cache/
    schemas/
  mcp-server/
  ui/
  adapters/
    claude/
      plugin/
      hooks/
      skills/
      CLAUDE.md
    codex/
      plugin/
      skills/
      AGENTS.md
  examples/
  docs/
```

### Suggested internal crates/modules

- `the-one-core`
- `the-one-mcp`
- `the-one-router`
- `the-one-memory`
- `the-one-registry`
- `the-one-ui`
- `the-one-claude`
- `the-one-codex`

---

## 18. Implementation priorities

### Phase 1

- define schemas
- build SQLite control DB
- build project profiler
- build capability registry
- implement `.the-one` local manifest logic

### Phase 2

- bring up Qdrant
- implement ingestion and chunking
- implement `memory.search`
- implement `docs.get` / `docs.get_section`

### Phase 3

- implement in-memory cache and invalidation
- implement router
- add rules-first ranking
- add nano-LLM provider abstraction

### Phase 4

- build central MCP broker
- expose small public MCP surface
- test against one CLI first, then the second

### Phase 5

- add Claude satellite
- add Codex satellite
- keep adapters thin

### Phase 6

- embed local web UI
- add maintenance tasks
- add backup/restore
- refine policy and budgets

---

## 19. Non-goals and anti-patterns

Do NOT:

- preload hundreds of tools directly into the client if avoidable
- store all project docs in always-loaded memory files
- make the nano model a full second assistant
- depend entirely on hooks for correctness
- duplicate the heavy store inside each repo
- dump giant raw docs or giant search payloads by default
- create two separate backends for Claude and Codex

---

## 20. Final condensed directive

Design and implement **the-one** as:

- a **Rust-based central MCP broker**
- with **SQLite** as the control/state database
- with **Qdrant** as the RAG/vector backend
- with an **in-memory cache** for hot reads
- with a **project-local `.the-one/` manifest/cache layer**
- with a **global `~/.the-one/` heavy storage layer**
- with a **rules-first router** plus optional **nano LLM**
- with **RAG for discovery** and **raw markdown access for precision**
- with **one shared backend** and **thin satellites for Claude Code and Codex**
- with an optional **embedded local web UI** for configuration and maintenance

Optimize for:

- token efficiency
- low startup context
- progressive tool exposure
- project-aware behavior
- local-first developer ergonomics
- future extensibility

When making architecture or implementation decisions, prefer:

- simpler operations
- explicit metadata
- small public surfaces
- strong invalidation/caching discipline
- separation of heavy state from prompt-visible state

