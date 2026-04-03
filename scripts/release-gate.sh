#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Contract gate
cargo test -p the-one-mcp test_v1beta_schema_files_exist_and_are_valid_json
cargo test -p the-one-mcp test_v1beta_schema_ids_and_draft_are_consistent

# Reliability drill gate
cargo test -p the-one-core test_manual_restore_recovers_sqlite_and_qdrant_tree
cargo test -p the-one-mcp test_project_refresh_soak_keeps_cached_mode_when_unchanged
cargo test -p the-one-mcp test_remote_qdrant_strict_auth_rejects_missing_api_key
cargo test -p the-one-ui test_embedded_ui_runtime_serves_dashboard_and_health
