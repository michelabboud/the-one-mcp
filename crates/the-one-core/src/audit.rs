//! Structured audit outcomes for state-changing broker operations.
//!
//! Prior to v0.15.0 the-one-mcp only recorded audit events for tool-run
//! approvals, and the event table had no dedicated outcome column. This module
//! defines the shape of a proper audit record — `operation`, `params` (the
//! redacted input), `outcome`, and `error_kind` — so every write-path in the
//! broker can produce an identical, machine-parseable audit trail.
//!
//! The schema migration (v7) in `storage::sqlite` adds the `outcome` and
//! `error_kind` columns. Legacy v0.14.x rows default to `outcome='unknown'`
//! and `error_kind=NULL` so downstream dashboards can filter them out.
//!
//! # Why not a full WAL?
//!
//! A write-ahead log would need to replay operations deterministically, which
//! means storing the full (often sensitive) input. We intentionally do not do
//! that — the audit log is an *observability* artefact, not a rollback log.
//! For rollback we use backups (`maintain: backup`) and SQLite WAL checkpoints.
//! See `docs/reviews/2026-04-10-mempalace-comparative-audit.md` for the
//! mempalace comparison that motivated this split.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Outcome of a state-changing broker operation, as recorded in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Operation committed successfully.
    Ok,
    /// Operation was rejected or failed. See `error_kind` for the category.
    Error,
    /// Legacy row from before schema v7 — never written by current code.
    Unknown,
}

impl AuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditOutcome::Ok => "ok",
            AuditOutcome::Error => "error",
            AuditOutcome::Unknown => "unknown",
        }
    }

    /// Parse a stored outcome string back into the enum. Returns
    /// [`AuditOutcome::Unknown`] for legacy or unexpected values so callers
    /// don't need to handle errors — the audit log's "unknown" bucket is
    /// the designated home for rows that predate schema v7.
    ///
    /// Intentionally named `parse_value` (not `from_str`) to avoid clashing
    /// with the `std::str::FromStr` trait, which callers would not expect
    /// on an infallible parse.
    pub fn parse_value(value: &str) -> Self {
        match value {
            "ok" => AuditOutcome::Ok,
            "error" => AuditOutcome::Error,
            _ => AuditOutcome::Unknown,
        }
    }
}

/// A structured audit log entry written by every state-changing broker call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Canonical operation name, e.g. `"memory.ingest_conversation"`.
    pub operation: &'static str,
    /// JSON-encoded, redacted parameters (no secrets, no large payloads).
    pub params_json: String,
    /// Outcome of the operation.
    pub outcome: AuditOutcome,
    /// Coarse error category when `outcome == Error`. Examples:
    /// `"invalid_request"`, `"sqlite"`, `"io"`, `"policy_denied"`.
    /// Safe to log — no inner error details.
    pub error_kind: Option<&'static str>,
}

impl AuditRecord {
    pub fn ok(operation: &'static str, params_json: impl Into<String>) -> Self {
        Self {
            operation,
            params_json: params_json.into(),
            outcome: AuditOutcome::Ok,
            error_kind: None,
        }
    }

    pub fn error(operation: &'static str, params_json: impl Into<String>, err: &CoreError) -> Self {
        Self {
            operation,
            params_json: params_json.into(),
            outcome: AuditOutcome::Error,
            error_kind: Some(error_kind_label(err)),
        }
    }
}

/// Map a [`CoreError`] variant to a short, stable, publicly-safe kind label.
/// These labels go into the audit log and the client-facing error envelope;
/// they must never leak inner messages, paths, or SQL text.
pub fn error_kind_label(err: &CoreError) -> &'static str {
    match err {
        CoreError::Io(_) => "io",
        CoreError::Json(_) => "json",
        CoreError::Sqlite(_) => "sqlite",
        CoreError::InvalidProjectConfig(_) => "invalid_project_config",
        CoreError::PolicyDenied(_) => "policy_denied",
        CoreError::UnsupportedSchemaVersion(_) => "unsupported_schema_version",
        CoreError::Embedding(_) => "embedding",
        CoreError::Transport(_) => "transport",
        CoreError::Provider(_) => "provider",
        CoreError::Document(_) => "document",
        CoreError::Catalog(_) => "catalog",
        CoreError::NotEnabled(_) => "not_enabled",
        CoreError::InvalidRequest(_) => "invalid_request",
        CoreError::Postgres(_) => "postgres",
        CoreError::Redis(_) => "redis",
    }
}

/// Render a compact JSON payload for an audit record. Uses `serde_json::json!`
/// under the hood; helper exists to keep call sites short.
pub fn params_json(value: serde_json::Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_roundtrip() {
        assert_eq!(AuditOutcome::parse_value("ok"), AuditOutcome::Ok);
        assert_eq!(AuditOutcome::parse_value("error"), AuditOutcome::Error);
        assert_eq!(AuditOutcome::parse_value("wat"), AuditOutcome::Unknown);
        assert_eq!(AuditOutcome::Ok.as_str(), "ok");
    }

    #[test]
    fn error_kind_labels_cover_every_variant() {
        // Sanity: every match arm returns a non-empty label. Static test of
        // every variant lives in `crates/the-one-mcp/tests/production_hardening.rs`.
        assert_eq!(
            error_kind_label(&CoreError::InvalidRequest("x".into())),
            "invalid_request"
        );
        assert_eq!(
            error_kind_label(&CoreError::PolicyDenied("x".into())),
            "policy_denied"
        );
    }

    #[test]
    fn ok_record_has_no_error_kind() {
        let rec = AuditRecord::ok("memory.ingest_conversation", "{}");
        assert_eq!(rec.outcome, AuditOutcome::Ok);
        assert!(rec.error_kind.is_none());
    }

    #[test]
    fn error_record_carries_kind() {
        let err = CoreError::InvalidRequest("bad".into());
        let rec = AuditRecord::error("memory.diary.add", "{}", &err);
        assert_eq!(rec.outcome, AuditOutcome::Error);
        assert_eq!(rec.error_kind, Some("invalid_request"));
    }
}
