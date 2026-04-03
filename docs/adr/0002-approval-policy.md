# ADR-0002: High-Risk Tool Approval Policy

Date: 2026-04-03
Status: Accepted

## Context

High-risk tools require explicit user consent with different runtime modes.

## Decision

- Interactive mode supports approval scopes:
  - once
  - session
  - forever
- Headless mode denies high-risk actions unless a persisted approval exists.

## Consequences

- Safer default execution behavior.
- Additional complexity for policy persistence and scope resolution.
