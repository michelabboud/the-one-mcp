# The-One MCP Implementation Plan

Last updated: 2026-04-03
Status: Locked for full delivery through Phase 9

## 1) Scope and Decisions

This plan operationalizes the architecture in `docs/plans/the-one-mcp-architecture-prompt.md`.

Locked product decisions:

- Delivery target: complete all phases through production hardening (Phase 9).
- Client support: Claude Code and Codex are first-class from the beginning.
- Routing: ship both rules-first routing and optional nano model routing.
- Model/provider support: local and hosted providers are both supported.
- Runtime bias: embedded/local service for storage and retrieval by default.
- High-risk tool execution approvals:
  - Interactive mode: approve once, always this session, or always forever.
  - Headless mode: deny by default unless previously approved policy exists.
- Isolation model:
  - Per project: dedicated folder, SQLite DB, RAG store/collection, config/overrides.
  - Global: tool and capability catalog only.
- Observability: full logs, metrics, traces, and audit trail.
- Embedded admin UI: required in MVP path (not deferred).
- Backup/restore: manual flows now, no scheduler yet.
- API compatibility: version public MCP schema as `v1beta` first, then freeze to `v1`.

## 2) Performance and Reliability Targets

Targets to enforce during implementation and release validation:

- `project.init` cold p95 <= 4s (medium project).
- `project.refresh` with unchanged fingerprint p95 <= 500ms.
- `memory.search` (top-k <= 5) p95 <= 700ms in local mode.
- `docs.get_section` p95 <= 250ms.
- Router decision p95 <= 120ms (rules-only), <= 300ms (rules+nano).
- MCP response defaults:
  - max hits by default: 5.
  - max raw section payload by default: 24 KB.
- Broker startup ready time <= 2s cold.
- No control-plane data loss on crash (SQLite WAL + transactions).

## 3) Delivery Structure

Execution runs across five parallel lanes:

1. Platform (config, persistence, isolation, migrations)
2. Intelligence (profiler, policy, routing)
3. Knowledge (docs ingestion, chunking, retrieval)
4. Interfaces (MCP surface, adapters, embedded UI)
5. Reliability (observability, resiliency, test quality)

High-level dependency chain:

`Stage 0 -> Stage 1 -> Stage 2 -> (Stage 3 + Stage 4 + Stage 5 in parallel) -> Stage 6 -> Stage 7 -> Stage 8 -> Stage 9`

## 4) Stage 0 - Program Setup (Gate A)

Goal: establish contracts, boundaries, and CI guardrails.

Tasks:

1. Architecture baseline
   - Define crate ownership and boundaries.
   - Freeze core entities (`ProjectProfile`, `Capability`, `RouteDecision`, `PolicyDecision`).
   - Create initial ADRs for isolation, approvals, and versioning.
2. Repository and CI bootstrap
   - Workspace checks (`check`, `fmt`, `clippy`, `test`) in CI.
   - Release profile build job and artifact validation.
3. Contract skeleton
   - Create MCP tool schema stubs under `v1beta`.
   - Define error taxonomy and cross-crate conversion rules.

Parallel work:

- CI bootstrap and schema generation can run in parallel after crate list is fixed.

Exit criteria:

- Skeleton compiles and tests pass in CI.
- ADRs are merged and referenced by implementation tasks.

## 5) Stage 1 - Core Platform Foundations (Gate B)

Goal: implement shared infrastructure and local state foundations.

Tasks:

1. Config system
   - Layer precedence: defaults -> global -> project -> env -> runtime.
   - Path normalization and validation for project roots.
2. Persistence foundations
   - Per-project SQLite manager with WAL and migrations.
   - Global registry persistence for tools/capability metadata.
3. Project filesystem contract
   - Create and validate `.the-one/` layout.
   - Add manifest schema version checks.
4. Shared telemetry bootstrap
   - Structured logs, trace context propagation, base metrics.

Parallel work:

- Config and telemetry run in parallel.
- Persistence and filesystem contract run in parallel once config paths are stable.

Exit criteria:

- New project initialization creates valid local folders/manifests and migrated DB.

## 6) Stage 2 - Isolation and State Lifecycle (Gate C)

Goal: guarantee strict project-level data isolation.

Tasks:

1. Isolation enforcement
   - Carry `project_id` and project root through all operations.
   - Add guardrails preventing cross-project reads/writes.
2. Manifest lifecycle and atomicity
   - Implement `project.json`, `overrides.json`, `fingerprint.json`, `pointers.json`.
   - Ensure atomic writes with rollback on failure.
3. Global vs local split enforcement
   - Keep catalog global.
   - Keep profile/RAG/cache/approvals local to each project.

Parallel work:

- Isolation enforcement and manifest lifecycle can proceed together.

Exit criteria:

- Multi-project concurrency tests confirm zero state leakage.

## 7) Stage 3 - Profiler and Fingerprint Engine (Gate D)

Goal: project-aware behavior with cache-safe refresh logic.

Tasks:

1. Signal detectors
   - Detect language/framework/build/CI/infra/cloud/risk markers.
2. Fingerprint engine
   - Hash high-signal files and classify change magnitude.
3. Init and refresh workflow
   - Implement `project.init` and `project.refresh` fast path.
4. Override protection
   - Preserve user overrides during profile regeneration.

Parallel work:

- Signal detectors and fingerprint engine in parallel.
- Override protection can start while init/refresh orchestration is integrated.

Exit criteria:

- Unchanged fingerprint path skips full init.
- User overrides are preserved across re-init.

## 8) Stage 4 - Capability Registry and Policy Engine (Gate E)

Goal: global capability catalog with enforceable safety and budget controls.

Tasks:

1. Capability catalog
   - Implement metadata model and indexing/query APIs.
   - Support visibility modes: `core`, `project`, `dormant`.
2. Policy engine
   - Enforce limits on suggestions, payload size, and enabled families.
   - Implement risk-tier gating rules.
3. Approval workflow
   - Interactive approvals: once/session/forever.
   - Headless behavior: deny unless persisted approval exists.
4. Policy persistence
   - Session and long-lived approval storage with project scope.

Parallel work:

- Catalog and policy rules in parallel.
- Approval workflow and persistence in parallel once policy interfaces are fixed.

Exit criteria:

- Approval matrix tests pass for interactive and headless modes.

## 9) Stage 5 - Docs and RAG Plane (Gate F)

Goal: retrieval and precision doc access with bounded outputs.

Tasks:

1. Ingestion pipeline
   - Source discovery, chunking, metadata extraction, idempotent updates.
2. Vector backend integration
   - Embedded/local Qdrant project isolation strategy.
   - Embedding provider abstraction (local and hosted).
3. Retrieval APIs
   - `memory.search` and `memory.fetch_chunk` with top-k defaults.
4. Raw markdown access APIs
   - `docs.list`, `docs.get`, `docs.get_section` with strict bounds.

Parallel work:

- Ingestion pipeline and vector integration in parallel.
- Retrieval and raw-doc APIs can proceed in parallel after interfaces are stable.

Exit criteria:

- Reindex is idempotent and bounded-output constraints are enforced.

## 10) Stage 6 - Router (Rules + Nano) (Gate G)

Goal: robust request routing with deterministic fallback behavior.

Tasks:

1. Rules-first router
   - Intent classification, retrieval decision, tool-family ranking.
2. Nano provider abstraction
   - Hosted/local providers, timeout/retry budgets, model selection.
3. Hybrid orchestration
   - Rules baseline always available, nano optional enhancement.
   - Fallback to rules when nano fails or is disabled.
4. Decision telemetry
   - Capture rationale, confidence, latency, and provider path.

Parallel work:

- Rules engine and nano abstraction in parallel.
- Telemetry work runs alongside hybrid orchestration integration.

Exit criteria:

- Route decision tests pass with deterministic fallback and latency budgets.

## 11) Stage 7 - MCP Broker Surface and Versioning (Gate H)

Goal: expose the minimal stable public interface with compatibility controls.

Tasks:

1. Tool surface implementation
   - `project.*`, `memory.*`, `docs.*`, `tool.*`, `config.export`.
2. Compatibility/versioning
   - Publish schemas as `v1beta`.
   - Add compatibility tests and schema drift checks.
3. Safety wrappers
   - Enforce policy and approval checks before high-risk tool execution.
4. End-to-end integration tests
   - Full request path from MCP entrypoint through policy/router/retrieval/tool.

Parallel work:

- Tool families can be implemented in parallel by API group.
- Versioning and safety wrappers can proceed in parallel.

Exit criteria:

- Public contract suite passes with no breaking changes.

## 12) Stage 8 - Claude and Codex Satellites (Gate I)

Goal: parity integration for both clients on one shared backend.

Tasks:

1. Shared adapter core
   - Common transport, config ingestion, normalization layer.
2. Claude satellite
   - Package integration, connection config, minimal guidance/hooks.
3. Codex satellite
   - Package integration, connection config, AGENTS-aligned behavior.
4. Cross-client parity suite
   - Same scenarios validated against Claude and Codex adapters.

Parallel work:

- Claude and Codex satellite tasks run in parallel after shared adapter contracts are fixed.

Exit criteria:

- Feature parity checklist passes for both clients.

## 13) Stage 9 - Embedded UI, Ops, and Hardening (Release Gate)

Goal: production readiness with operability and reliability.

Tasks:

1. Embedded UI
   - Project profile views, capability toggles, provider/model config.
   - Maintenance actions: init, refresh, reindex, backup.
2. Manual backup and restore
   - SQLite snapshots and Qdrant snapshots via command and UI trigger.
3. Full observability
   - Metrics dashboards, distributed traces, audit event exploration.
4. Reliability hardening
   - Soak tests, failure injection, crash recovery, migration stress tests.
5. Release readiness
   - Packaging, upgrade notes, operator docs, runbooks.

Parallel work:

- Embedded UI, backup/restore, and observability can run in parallel.
- Reliability hardening and release readiness finalize after full integration.

Exit criteria:

- SLOs met, restore drills pass, and release candidate sign-off complete.

## 14) Milestones and Checkpoints

- M1 (post Stage 2): isolated project state and DB layout are stable.
- M2 (post Stages 3,4,5): profiler, policy, and retrieval core complete.
- M3 (post Stage 7): MCP `v1beta` public surface stable.
- M4 (post Stage 8): Claude and Codex parity achieved.
- M5 (post Stage 9): production-ready release candidate.

## 15) Acceptance Test Matrix (Condensed)

Core acceptance suites required before release:

1. Isolation suite
   - Multi-project read/write segregation.
   - No leakage of approvals, profiles, retrieval data.
2. Compatibility suite
   - MCP schema contract checks and `v1beta` drift detection.
3. Policy and approval suite
   - Interactive scope handling and headless deny-by-default.
4. Retrieval suite
   - Deterministic bounded results for `memory.*` and `docs.*` APIs.
5. Router suite
   - Rules-only, rules+nano, and fallback behavior.
6. Adapter parity suite
   - Equivalent outcomes for Claude and Codex flows.
7. Reliability suite
   - Crash recovery, migration safety, backup/restore drills.

## 16) Risks and Mitigations

- Risk: contract churn across rapid feature work.
  - Mitigation: enforce schema compatibility tests on every PR.
- Risk: cross-project data leakage under concurrency.
  - Mitigation: explicit project context guards and dedicated stress tests.
- Risk: provider instability (nano/embedding backends).
  - Mitigation: timeout budgets, retries, and deterministic rules fallback.
- Risk: UI scope creep.
  - Mitigation: UI remains operations/config focused and reuses backend APIs.

## 17) Immediate Next Steps

1. Create Stage 0 ADR set and workspace skeleton.
2. Define `v1beta` MCP schema contracts for all planned tool groups.
3. Implement Stage 1 foundation tasks with CI quality gates enabled.
