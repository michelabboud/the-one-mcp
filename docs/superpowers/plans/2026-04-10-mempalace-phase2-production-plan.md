# MemPalace Phase 2 Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the remaining MemPalace capabilities in production-grade form: AAAK compression + auto-teach, drawers/closets/tunnels navigation primitives, diary memory tools, and a single on/off profile switch.

**Architecture:** Extend the existing conversation-memory stack in `the-one-core` + `the-one-mcp` by adding explicit data contracts, SQLite persistence, broker APIs, and MCP tool surface. Keep all behavior opt-in via config profile controls, with deterministic defaults and strict runtime gating. Reuse existing ingest/search/wake-up pathways where possible, adding only bounded, test-backed primitives.

**Tech Stack:** Rust workspace (`the-one-core`, `the-one-memory`, `the-one-mcp`, `the-one-ui`), SQLite migrations/state, JSON-RPC MCP transport, serde contracts, tokio async tests.

---

## File Structure

- Modify: `crates/the-one-core/src/config.rs`
  - Add MemPalace profile preset model and switch helpers.
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
  - Add persistence for AAAK lessons, navigation graph (drawers/closets/tunnels), diary entries.
- Modify: `crates/the-one-core/src/contracts.rs`
  - Add shared enums/structs for AAAK, navigation primitives, diary schema.
- Modify: `crates/the-one-mcp/src/api.rs`
  - Add MCP request/response contracts for new tools/actions.
- Modify: `crates/the-one-mcp/src/broker.rs`
  - Implement broker methods and runtime gating for all new capabilities.
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
  - Add dispatch routes for new maintain/config actions and diary/navigation tools.
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
  - Add tool definitions and maintain action enums/docs.
- Modify: `crates/the-one-memory/src/conversation.rs`
  - Add AAAK compression dialect helpers and auto-teach extraction.
- Modify: `crates/the-one-ui/src/lib.rs`
  - Add UI/API controls for profile switching and inspection.
- Modify: `schemas/mcp/v1beta/*.json`
  - Add/update schema definitions for new tool contracts.
- Modify: `README.md`, `docs/guides/conversation-memory.md`, `docs/guides/api-reference.md`, `CHANGELOG.md`, `PROGRESS.md`
  - Document behavior, examples, feature flags, and safety constraints.

## Task 1: Single Switch Profile (MemPalace Preset)

**Files:**
- Modify: `crates/the-one-core/src/config.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-core/src/config.rs` (tests module)
- Test: `crates/the-one-mcp/src/broker.rs` (tests module)

- [ ] **Step 1: Write failing config tests for profile preset behavior**
  - Add tests for:
    - applying `mempalace_full` sets all required flags (`memory_palace_enabled`, hooks, AAAK, diary, navigation).
    - applying `mempalace_off` disables all MemPalace features.
  - Run: `cargo test -p the-one-core mempalace_profile`
  - Expected: FAIL (preset not implemented yet).

- [ ] **Step 2: Add profile model + config apply logic**
  - Implement in `config.rs`:
    - `enum MemoryPalaceProfilePreset { Off, Core, Full }`
    - `ProjectConfigUpdate` field: `memory_palace_profile: Option<String>`
    - deterministic mapping from preset to concrete feature flags.

- [ ] **Step 3: Add broker-level config action for profile switching**
  - In `broker.rs` + `jsonrpc.rs`, add `config` action:
    - `profile.set` with params: `{ project_root, profile }`.
  - Ensure this action writes explicit concrete flags to project config (no implicit runtime-only state).

- [ ] **Step 4: Expose tool metadata**
  - Update `tools.rs` to document `config: profile.set`.
  - Include valid values: `off`, `core`, `full`.

- [ ] **Step 5: Run targeted tests**
  - Run:
    - `cargo test -p the-one-core mempalace_profile`
    - `cargo test -p the-one-mcp profile_set`
  - Expected: PASS.

- [ ] **Step 6: Commit**
  - Run:
    - `git add crates/the-one-core/src/config.rs crates/the-one-mcp/src/{api.rs,broker.rs,transport/jsonrpc.rs,transport/tools.rs}`
    - `git commit -m "feat: add mempalace profile preset switch"`

## Task 2: AAAK Compression Dialect + Auto-Teach Protocol

**Files:**
- Modify: `crates/the-one-memory/src/conversation.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-core/src/contracts.rs`
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-memory/src/conversation.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing AAAK parser/compressor tests**
  - Add tests for:
    - lossless parse/serialize of AAAK envelope.
    - deterministic compression for repeated dialogue motifs.
    - safe fallback to verbatim when confidence is low.
  - Run: `cargo test -p the-one-memory aaak`
  - Expected: FAIL.

- [ ] **Step 2: Implement AAAK dialect contracts**
  - In `contracts.rs`, define:
    - `AaakLesson`, `AaakPattern`, `AaakCompressionResult`, `AaakTeachOutcome`.
  - In `api.rs`, add requests/responses for:
    - `memory.aaak.compress`
    - `memory.aaak.teach`
    - `memory.aaak.list_lessons`

- [ ] **Step 3: Persist AAAK lessons**
  - In `sqlite.rs`, add migration + DAO methods:
    - `upsert_aaak_lesson`
    - `list_aaak_lessons(project_id, limit)`
    - `delete_aaak_lesson(id)` (soft-delete optional if pattern used elsewhere).
  - Add tests for migration and roundtrip behavior.

- [ ] **Step 4: Implement auto-teach in ingest flow**
  - In `broker.rs`, update conversation ingest/hook capture flow:
    - when AAAK enabled, extract reusable patterns and write lessons.
    - guard by quality threshold and max lessons per ingest batch.
    - never discard original transcript memory.

- [ ] **Step 5: Expose MCP tool surface**
  - Wire new calls in `jsonrpc.rs` and definitions in `tools.rs`.
  - Add schema updates in `schemas/mcp/v1beta`.

- [ ] **Step 6: Run targeted tests**
  - Run:
    - `cargo test -p the-one-memory aaak`
    - `cargo test -p the-one-core aaak_lesson`
    - `cargo test -p the-one-mcp aaak`
  - Expected: PASS.

- [ ] **Step 7: Commit**
  - `git add` relevant files
  - `git commit -m "feat: add aaak compression and auto-teach protocol"`

## Task 3: Drawers / Closets / Tunnels Navigation Primitives

**Files:**
- Modify: `crates/the-one-core/src/contracts.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing navigation graph tests**
  - Add tests for:
    - create/list drawer
    - create/list closet under drawer
    - tunnel link between two nodes
    - traversal query returns deterministic path ordering.
  - Run: `cargo test -p the-one-core navigation_primitive`
  - Expected: FAIL.

- [ ] **Step 2: Add contracts and persistence**
  - In `contracts.rs` define node/link types.
  - In `sqlite.rs` add tables + DAO methods for navigation nodes and tunnel edges.
  - Include uniqueness constraints to prevent duplicate links.

- [ ] **Step 3: Add broker APIs**
  - Implement:
    - `memory.navigation.upsert_node`
    - `memory.navigation.link_tunnel`
    - `memory.navigation.list`
    - `memory.navigation.traverse`
  - Validate references and return typed errors on missing node IDs.

- [ ] **Step 4: Expose transport/tool definitions**
  - Add dispatch in `jsonrpc.rs`.
  - Add definitions and examples in `tools.rs`.

- [ ] **Step 5: Integrate with wake-up/search metadata**
  - Map existing `wing/hall/room` to node identifiers when present.
  - Keep backward compatibility for existing palace metadata.

- [ ] **Step 6: Run targeted tests**
  - Run:
    - `cargo test -p the-one-core navigation_primitive`
    - `cargo test -p the-one-mcp navigation`
  - Expected: PASS.

- [ ] **Step 7: Commit**
  - `git add` relevant files
  - `git commit -m "feat: add drawers closets tunnels navigation primitives"`

## Task 4: Diary-Specific Memory Tools and Flows

**Files:**
- Modify: `crates/the-one-core/src/contracts.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing diary flow tests**
  - Add tests for:
    - create diary entry with tags/mood/date
    - list by date range
    - search diary entries
    - summarize recent diary entries.
  - Run: `cargo test -p the-one-mcp diary`
  - Expected: FAIL.

- [ ] **Step 2: Add diary schema + storage**
  - In `contracts.rs`: `DiaryEntry`, `DiarySummary`.
  - In `sqlite.rs`: `diary_entries` table + indexes (`project_id`, `entry_date`, FTS text if needed).

- [ ] **Step 3: Implement broker diary operations**
  - Add:
    - `memory.diary.add`
    - `memory.diary.list`
    - `memory.diary.search`
    - `memory.diary.summarize`
  - Reuse existing memory engine extraction for summaries where possible.

- [ ] **Step 4: Add MCP transport + schemas**
  - Update `jsonrpc.rs`, `tools.rs`, and JSON schemas.

- [ ] **Step 5: Add gating + limits**
  - Require MemPalace enabled for diary tools.
  - Add bounded defaults (`max_results`, `max_summary_items`) tied to limits config.

- [ ] **Step 6: Run targeted tests**
  - Run:
    - `cargo test -p the-one-core diary_entries`
    - `cargo test -p the-one-mcp diary`
  - Expected: PASS.

- [ ] **Step 7: Commit**
  - `git add` relevant files
  - `git commit -m "feat: add diary memory tools and flows"`

## Task 5: UI Preset Control + End-to-End Docs

**Files:**
- Modify: `crates/the-one-ui/src/lib.rs`
- Modify: `README.md`
- Modify: `docs/guides/conversation-memory.md`
- Modify: `docs/guides/api-reference.md`
- Modify: `CHANGELOG.md`
- Modify: `PROGRESS.md`
- Test: `crates/the-one-ui/src/lib.rs`

- [ ] **Step 1: Write failing UI/API preset tests**
  - Add tests for:
    - set profile from UI endpoint
    - reflect active profile in config view.
  - Run: `cargo test -p the-one-ui mempalace_profile`
  - Expected: FAIL.

- [ ] **Step 2: Add admin UI profile control**
  - Add API route + simple control widget to apply `off/core/full`.
  - Show currently active preset and expanded flag state.

- [ ] **Step 3: Update docs with exact examples**
  - Add command examples for:
    - `config: profile.set`
    - `maintain: memory.capture_hook`
    - AAAK + diary + navigation calls.

- [ ] **Step 4: Run targeted UI/docs verification**
  - Run:
    - `cargo test -p the-one-ui mempalace_profile`
    - `rg -n "TODO|TBD|placeholder|stub" README.md docs/guides/conversation-memory.md docs/guides/api-reference.md`
  - Expected: PASS; no placeholder markers.

- [ ] **Step 5: Commit**
  - `git add` relevant files
  - `git commit -m "feat: add ui preset controls and complete mempalace docs"`

## Task 6: Full Release-Grade Verification

**Files:**
- Modify: none (verification + report only)
- Create: `docs/reviews/2026-04-10-mempalace-phase2-verification.md`

- [ ] **Step 1: Run full format/lint/test gates**
  - Run:
    - `cargo fmt --check`
    - `cargo clippy --workspace --all-targets -- -D warnings`
    - `cargo test --workspace`
  - Expected: all PASS.

- [ ] **Step 2: Run focused no-placeholder/no-stub scan**
  - Run:
    - `rg -n "TODO|TBD|placeholder|stub" crates docs | head -200`
  - Expected: no unresolved markers in new feature paths.

- [ ] **Step 3: Produce verification report**
  - Write:
    - feature checklist with pass/fail
    - executed commands + result summary
    - residual risks and explicit “none known” if clean.

- [ ] **Step 4: Commit verification artifacts**
  - `git add docs/reviews/2026-04-10-mempalace-phase2-verification.md CHANGELOG.md PROGRESS.md`
  - `git commit -m "docs: add mempalace phase2 verification report"`

## Spec Coverage Check

- AAAK compression dialect / auto-teach protocol: covered by Task 2.
- Drawers/closets/tunnels primitives: covered by Task 3.
- Diary-specific memory tools: covered by Task 4.
- Single on/off profile command/UI preset: covered by Task 1 + Task 5.

No uncovered requirement remains for the stated scope.

## Placeholder Scan

- No `TBD`, `TODO`, “implement later”, or “similar to previous task” placeholders are present in this plan.

## Type Consistency Check

- Profile names are consistent: `off`, `core`, `full`.
- Hook events are consistent: `stop`, `precompact`.
- Navigation primitive labels are consistent across contracts/storage/API: drawers, closets, tunnels.

Plan complete and saved to `docs/superpowers/plans/2026-04-10-mempalace-phase2-production-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
