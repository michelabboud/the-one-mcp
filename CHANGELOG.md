# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

### Added

- Complete implementation guide: `docs/guides/the-one-mcp-complete-guide.md`
- Quickstart guide: `docs/guides/quickstart.md`
- Embedded UI runtime endpoints for dashboard/health/audit/config and config update API
- Interactive Swagger UI page (`/swagger`) in addition to raw OpenAPI JSON (`/api/swagger`)
- Editable config UX on `/config` backed by `POST /api/config`
- Embedded swagger support in MCP (`embed-swagger` feature, default enabled)
- Swagger asset: `schemas/mcp/v1beta/openapi.swagger.json`
- Release gate script + CI release-gate job
- Router hard-bound telemetry fields and provider error tracking
- Remote Qdrant strict-auth enforcement and auth/TLS config knobs
- Qdrant HTTP backend tests and router soak tests

### Changed

- Expanded MCP config export contract to include Qdrant auth/TLS/strict mode visibility
- Expanded memory search response contract with route and telemetry metadata
- Strengthened schema validation/tests to enforce schema inventory and metadata consistency

### Security

- Enforced fail-closed behavior for remote Qdrant when strict auth is enabled and API key is missing
