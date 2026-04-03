# v1beta Upgrade Notes

Last updated: 2026-04-03

## Highlights

- Added project-scoped memory request contracts for `memory.search` and `docs.get_section`.
- Added `docs.list` and `docs.get` request schemas with explicit project context.
- Added router observability counters in metrics snapshot:
  - `router_fallback_calls`
  - `router_decision_latency_ms_total`
- Added admin restore workflow and health report aggregation.

## Operator Actions

1. Update any client payload builders to include `project_root` and `project_id` where required.
2. Ensure monitoring collectors accept the new router metric fields.
3. Validate backup/restore flow in staging before production rollout.

## Compatibility

- Schema namespace remains `v1beta`.
- Changes are additive for existing response objects except where request contracts were tightened with explicit project context.
