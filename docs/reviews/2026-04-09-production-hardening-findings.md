# Production Hardening Findings

Date: 2026-04-09

## Status Update (2026-04-10)

All four blocking findings in this audit were addressed in v0.14.2.
See [2026-04-10-production-hardening-verification.md](/home/michel/projects/the-one-mcp/docs/reviews/2026-04-10-production-hardening-verification.md)
for implementation and verification evidence.

## Scope

This audit covers the production-readiness gaps still present after the
conversation-memory and Redis vector backend integration work. The goal is to
identify every concrete "not fully shipped" behavior that must be fixed before
we can honestly describe this surface as production-grade, with no stubs, no
placeholders, and no intentionally incomplete runtime paths.

## Verification Performed

- `cargo check --workspace`
- `cargo test -p the-one-mcp test_dispatch_memory_ingest_conversation -- --nocapture`
- `cargo test -p the-one-mcp test_memory_wake_up_reloads_persisted_conversations_after_broker_restart -- --nocapture`
- `cargo test -p the-one-core config_parses_redis_vector_backend_settings -- --nocapture`
- `cargo test -p the-one-mcp resources -- --nocapture`
- `rg -n "TODO|TBD|FIXME|XXX|stub|placeholder|not implemented|unimplemented!|todo!|panic!\\(|IMPLEMENT ME|coming soon|temporary|hardcoded" crates docs README.md`
- `rg -n "deferred|follow-up|under active integration|currently empty|currently|not yet|today|remains under active|wiring" crates README.md docs/guides docs/benchmarks`

## Findings

### 1. Redis vector backend is configuration-only, not end-to-end functional

Severity: high

The broker accepts `vector_backend = "redis"` configuration, validates the
Redis URL and index name, and then explicitly falls back to local-only memory
instead of constructing a Redis-backed `MemoryEngine`.

Evidence:

- [broker.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs#L170)
- [broker.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs#L200)
- [redis-vector-backend.md](/home/michel/projects/the-one-mcp/docs/guides/redis-vector-backend.md#L78)
- [configuration.md](/home/michel/projects/the-one-mcp/docs/guides/configuration.md#L122)
- [architecture.md](/home/michel/projects/the-one-mcp/docs/guides/architecture.md#L268)

Current behavior:

- Local embeddings + Redis config: accepted, but broker warns and constructs
  local-only memory.
- API embeddings + Redis config: rejected.
- The docs are honest about this, which confirms this is not an accidental bug
  but an unfinished product seam.

Why this blocks production:

- Operators cannot rely on Redis persistence, index durability, or RediSearch
  retrieval semantics even when the configuration says Redis is the active
  vector backend.
- The runtime behavior does not match the advertised backend selection
  contract.
- Recovery, backup, performance, and operational expectations are materially
  different between local-only memory and Redis-backed vector storage.

Required fix:

- Add a real Redis-backed `MemoryEngine` construction path.
- Support both local-embedding and API-embedding ingest/search flows.
- Validate Redis persistence settings against live server state when
  `redis_persistence_required = true`.
- Remove the local-only fallback path for `vector_backend = "redis"`.
- Update docs only after the real runtime path exists.

### 2. `models.check` is still a live stub

Severity: medium

The broker exposes a `models.check` action, but the implementation itself is
documented as a stub and only returns current local registry counts plus a
message telling the operator to run update scripts manually.

Evidence:

- [broker.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs#L2290)
- [jsonrpc.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/transport/jsonrpc.rs#L688)

Current behavior:

- No upstream fetch
- No comparison against current registry entries
- No structured "up to date / update available / source unreachable" result
- No provider-specific status

Why this blocks production:

- A production-facing "check updates" command must either perform a real check
  or not exist.
- Stubbed operator surfaces create false confidence and lead to manual drift.

Required fix:

- Replace the stub with a real update-check implementation.
- Define a stable response model with per-source status, error reporting, and
  recommended next actions.
- Add tests for success, no-update, and network/source failure cases.

### 3. `the-one://catalog/enabled` is intentionally unimplemented

Severity: medium

The MCP resources surface exposes `the-one://catalog/enabled`, but the resource
handler returns `[]` unconditionally and explicitly says that wiring to the
SQLite `enabled_tools` table is a future follow-up. There is also a test that
locks in the empty-array behavior.

Evidence:

- [resources.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/resources.rs#L268)
- [resources.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/resources.rs#L398)
- [mcp-resources.md](/home/michel/projects/the-one-mcp/docs/guides/mcp-resources.md#L74)
- [api-reference.md](/home/michel/projects/the-one-mcp/docs/guides/api-reference.md#L2151)

Why this blocks production:

- The resource advertises data that does not exist.
- Clients consuming MCP resources cannot trust the catalog surface.
- The docs correctly describe a limitation, but the limitation is still a real
  incomplete feature.

Required fix:

- Read enabled tools from SQLite for the active project and client context.
- Define the exact resource payload shape.
- Add integration tests covering empty, non-empty, and client-scoped results.
- Remove the "currently empty array" documentation once the resource is real.

### 4. Wake-up packs only filter by `wing`

Severity: medium

The conversation wake-up flow currently supports filtering by `wing` only. The
public docs call this out explicitly, which means the feature is knowingly
partial even though conversation ingestion and search now model `wing`, `hall`,
and `room`.

Evidence:

- [conversation-memory.md](/home/michel/projects/the-one-mcp/docs/guides/conversation-memory.md#L105)
- [api.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/api.rs#L136)
- [broker.rs](/home/michel/projects/the-one-mcp/crates/the-one-mcp/src/broker.rs#L1214)

Why this blocks production:

- The palace model is now tri-level (`wing`, `hall`, `room`) for ingest and
  search, but wake-up retrieval ignores two of those dimensions.
- Users cannot build precise session revival behavior for a hall- or
  room-specific context pack.
- This creates surprising inconsistency between `memory.search` and
  `memory.wake_up`.

Required fix:

- Extend the wake-up request and broker/database query path to support `hall`
  and `room`.
- Define deterministic precedence and matching semantics.
- Add tests for `wing`-only, `wing+hall`, `wing+room`, and full
  `wing+hall+room` filtering.

## Non-Findings

These surfaced during the scan but should not be treated as production blockers
 for this hardening pass:

- Placeholder text in HTML form inputs in the UI
- Test-only `panic!` and fake data helpers
- Historical plan/spec/review documents that mention earlier stubs
- Optional feature-disabled OCR stub behavior in
  [ocr.rs](/home/michel/projects/the-one-mcp/crates/the-one-memory/src/ocr.rs),
  because that path is an explicit disabled-feature contract rather than a live
  production feature pretending to be complete

## Exit Criteria

Do not declare this work production-grade until all of the following are true:

- Redis backend selection creates and uses a real Redis-backed vector engine
- `models.check` performs a real check and no longer contains stub behavior
- `the-one://catalog/enabled` returns real enabled tool data
- `memory.wake_up` supports the full palace filter model used by ingest/search
- Guides and API reference no longer describe these surfaces as "today",
  "currently", "follow-up", or "under active integration"
