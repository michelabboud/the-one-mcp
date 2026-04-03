# ADR-0003: MCP Public Contract Versioning

Date: 2026-04-03
Status: Accepted

## Context

The MCP public interface will evolve during implementation and must remain compatible for clients.

## Decision

- Version MCP tool schemas at the contract level.
- Start with `v1beta` namespace for rapid iteration.
- Freeze to `v1` after compatibility and acceptance suites pass.
- Breaking changes require a deprecation period across at least two minor releases.

## Consequences

- Lower risk of adapter breakage.
- Additional CI effort for schema compatibility tests.
