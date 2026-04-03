# The-One MCP Sprint Plan

Last updated: 2026-04-03
Plan source: `docs/plans/the-one-mcp-implementation-plan.md`

## Sprint Strategy

- Sprint length: 2 weeks.
- Team model: five parallel lanes (Platform, Intelligence, Knowledge, Interfaces, Reliability).
- Definition of done per sprint:
  - Code merged with tests.
  - CI green (`check`, `fmt --check`, `clippy`, `test`).
  - Docs and ADR updates included for architectural deltas.

## Sprint 1 - Stage 0 Foundations

Objectives:

- Finalize crate boundaries, contracts, ADR baselines.
- Stand up workspace skeleton and CI quality gates.
- Add `v1beta` MCP schema placeholders and error taxonomy scaffolding.

Tasks:

1. Platform
   - Workspace setup and crate scaffolding.
   - Shared config and error crate/module baseline.
2. Interfaces
   - `v1beta` tool schema directories and placeholder JSON schemas.
3. Reliability
   - CI workflow with `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo test`.
4. Architecture
   - ADRs for isolation, approval policy, and MCP versioning.

Acceptance:

- Workspace builds cleanly.
- All crates compile.
- CI passes on every PR.

## Sprint 2 - Stage 1 Core Platform Foundations

Objectives:

- Implement layered config loading.
- Add SQLite manager with migrations and WAL mode.
- Establish `.the-one/` local filesystem contract.

Tasks:

1. Platform
   - Config precedence engine.
   - DB manager and migration runner.
   - Project state filesystem bootstrap.
2. Reliability
   - Structured logging and tracing bootstrap.

Acceptance:

- New project can be initialized with local manifests and migrated DB.

## Sprint 3 - Stage 2 Isolation and State Lifecycle

Objectives:

- Enforce per-project isolation boundaries.
- Implement manifest atomic writes and rollback behavior.

Tasks:

1. Platform
   - Request context carries `project_id` and roots.
   - Guardrails for cross-project access.
2. Reliability
   - Isolation stress tests with concurrent project operations.

Acceptance:

- Cross-project leakage tests are green.

## Sprint 4 - Stage 3 Profiler and Fingerprint Engine

Objectives:

- Build detector framework and fingerprinting.
- Implement `project.init` and `project.refresh` cache reuse.

Tasks:

1. Intelligence
   - File signal detector modules.
   - Fingerprint hash generation and change classification.
2. Interfaces
   - Wire profile outputs to API contracts.

Acceptance:

- Unchanged fingerprint path avoids full re-profile.

## Sprint 5 - Stage 4 Registry and Policy

Objectives:

- Implement global capability catalog and visibility logic.
- Implement risk policy + approval model.

Tasks:

1. Intelligence
   - Capability metadata model + query API.
   - Policy evaluator and hard limits.
2. Interfaces
   - Approval prompt behavior for interactive mode.
3. Reliability
   - Headless deny-by-default and approval persistence tests.

Acceptance:

- Policy matrix suite passes.

## Sprint 6 - Stage 5 Docs and RAG Plane

Objectives:

- Implement ingestion/chunking and Qdrant integration.
- Expose retrieval and raw-doc precision APIs.

Tasks:

1. Knowledge
   - Ingestion pipeline and metadata storage.
   - Qdrant project isolation and indexing.
2. Interfaces
   - `memory.search`, `memory.fetch_chunk`, `docs.*` endpoints.
3. Reliability
   - Bounded response and idempotent reindex tests.

Acceptance:

- Retrieval APIs pass correctness and latency checks.

## Sprint 7 - Stage 6 Router (Rules + Nano)

Objectives:

- Implement rules-first router and optional nano backends.

Tasks:

1. Intelligence
   - Rule scoring engine and family ranking.
   - Nano provider abstraction for API/Ollama/LM Studio.
2. Reliability
   - Fallback determinism and timeout/retry tests.

Acceptance:

- Rules baseline always functional; nano path optional and safe.

## Sprint 8 - Stage 7 MCP Surface and Compatibility

Objectives:

- Deliver stable `v1beta` MCP surface.
- Add schema drift and contract compatibility checks.

Tasks:

1. Interfaces
   - Implement `project.*`, `memory.*`, `docs.*`, `tool.*`, `config.export`.
2. Reliability
   - Contract test suite and schema diff checks.

Acceptance:

- Contract suite passes and no breaking schema changes.

## Sprint 9 - Stage 8 Dual Satellites

Objectives:

- Integrate both Claude and Codex adapters in parity mode.

Tasks:

1. Interfaces
   - Shared adapter core.
   - Claude-specific packaging/config.
   - Codex-specific packaging/config.
2. Reliability
   - Cross-client parity scenario suite.

Acceptance:

- Same core workflows pass in both clients.

## Sprint 10 - Stage 9 UI, Ops, Hardening

Objectives:

- Ship embedded admin UI.
- Add manual backup/restore and full observability.
- Complete reliability hardening and release readiness.

Tasks:

1. Interfaces
   - Embedded UI for profile/config/capability/maintenance.
2. Platform
   - Manual snapshot/restore flows.
3. Reliability
   - Dashboards, trace propagation, audit browsing.
   - Soak/fault/crash recovery tests and release criteria.

Acceptance:

- Release candidate sign-off completed.

## Parallel Work Allocation

- Platform lane: Sprints 1-3, 10 (heaviest dependency path).
- Intelligence lane: Sprints 4-5, 7.
- Knowledge lane: Sprint 6.
- Interfaces lane: Sprints 1, 4, 6, 8, 9, 10.
- Reliability lane: every sprint with increasing depth.

## Milestone Map

- M1: end Sprint 3 (isolation stable).
- M2: end Sprint 6 (profile + policy + retrieval stable).
- M3: end Sprint 8 (`v1beta` MCP stable).
- M4: end Sprint 9 (Claude+Codex parity).
- M5: end Sprint 10 (production-ready release candidate).
