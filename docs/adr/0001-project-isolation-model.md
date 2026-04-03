# ADR-0001: Project Isolation Model

Date: 2026-04-03
Status: Accepted

## Context

The system must support single-user, multi-project operation with strict data separation.

## Decision

- Each project has isolated local state in `<repo>/.the-one/`.
- Each project has its own SQLite database and RAG storage scope.
- Global scope is limited to tool/capability metadata.

## Consequences

- Strong protection against cross-project data leakage.
- Simpler backup/restore per project.
- Slight increase in storage overhead due to per-project persistence.
