//! Shared input sanitisers for user-supplied names, identifiers, and action keys.
//!
//! The-one-mcp accepts wing/hall/room/project names and identifiers across many
//! broker entry points. Prior to v0.15.0 each entry point used ad-hoc validation
//! (or none at all), which left two classes of bugs latent:
//!
//! 1. Path-traversal or filesystem-unfriendly characters in user-supplied names
//!    could slip through and later interact badly with downstream code that
//!    does `path.join(wing_name)` or uses the name as part of a filename.
//! 2. SQLite/FTS integrations could silently accept absurdly long names that
//!    bloat indexes and degrade query latency.
//!
//! This module is the single source of truth. Every broker write path MUST
//! call the appropriate sanitiser on incoming names before persisting them.
//!
//! # Charset policy (rationale)
//!
//! `sanitize_name`: allows `[A-Za-z0-9 ._\-]`. Covers human-readable wing names
//! like `"ops incidents"`, slugs like `"auth-migration"`, and dotted identifiers
//! like `"v1.2.3"`. Rejects `/`, `\`, null bytes, newlines, and tabs. Rejects
//! leading/trailing dot (to keep names from shadowing hidden files). Rejects
//! the literal sequence `..` to defeat path traversal even inside the charset.
//!
//! `sanitize_project_id`: stricter — `[A-Za-z0-9_\-]{1,64}`. Project IDs are
//! used as SQLite primary keys and appear in filesystem paths; spaces and dots
//! are too loose.
//!
//! `sanitize_action_key`: tool action identifiers — `[A-Za-z0-9_.:\-]{1,128}`.
//! Colons and dots are allowed because the existing broker uses
//! `"tool.run:danger"`-style keys. Spaces are rejected.

use crate::error::CoreError;

/// Maximum allowed length of a human-readable name (wing, hall, room, entity).
pub const MAX_NAME_LENGTH: usize = 128;

/// Maximum allowed length of a project identifier.
pub const MAX_PROJECT_ID_LENGTH: usize = 64;

/// Maximum allowed length of an action key.
pub const MAX_ACTION_KEY_LENGTH: usize = 128;

/// Validate a human-readable name used for wings, halls, rooms, entity labels,
/// etc. Returns a trimmed copy on success, `CoreError::InvalidRequest` on failure.
///
/// Rules:
/// - must be non-empty after trimming
/// - length ≤ [`MAX_NAME_LENGTH`]
/// - no null bytes, newlines, tabs
/// - no `/`, `\`, or the literal sequence `..`
/// - no leading/trailing dot (avoids shadowing hidden files if the name ever
///   flows into a path component)
/// - allowed charset: ASCII letters, digits, space, `.`, `_`, `-`, `:`
///
/// The `:` is included because existing hook / namespace conventions use
/// colons as separators (e.g. `"hook:precompact"`, `"event:stop"`). The
/// ASCII-only policy is deliberate: names flow into SQLite indexes, log
/// lines, and (rarely) filesystem paths; supporting the full Unicode
/// repertoire here would require consistent NFKC normalization and case
/// folding at every call site, which is error-prone. If a caller needs
/// non-ASCII, they should store a display name alongside a sanitized slug.
pub fn sanitize_name(value: &str, field: &str) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not be empty"
        )));
    }
    if trimmed.len() > MAX_NAME_LENGTH {
        return Err(CoreError::InvalidRequest(format!(
            "{field} exceeds maximum length of {MAX_NAME_LENGTH} characters",
        )));
    }
    if trimmed.contains('\0') {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not contain null bytes"
        )));
    }
    if trimmed.contains('\n') || trimmed.contains('\r') || trimmed.contains('\t') {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not contain control whitespace"
        )));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not contain path separators"
        )));
    }
    if trimmed.contains("..") {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not contain '..'"
        )));
    }
    if trimmed.starts_with('.') || trimmed.ends_with('.') {
        return Err(CoreError::InvalidRequest(format!(
            "{field} must not start or end with '.'"
        )));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | ':'))
    {
        return Err(CoreError::InvalidRequest(format!(
            "{field} contains unsupported characters (allowed: letters, digits, space, '.', '_', '-', ':')",
        )));
    }
    Ok(trimmed.to_string())
}

/// Validate a project identifier (primary key in SQLite, appears in paths).
///
/// Rules:
/// - non-empty, length ≤ [`MAX_PROJECT_ID_LENGTH`]
/// - allowed charset: `[A-Za-z0-9_\-]`
/// - no leading dash, no trailing dash
pub fn sanitize_project_id(value: &str) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidRequest(
            "project_id must not be empty".to_string(),
        ));
    }
    if trimmed.len() > MAX_PROJECT_ID_LENGTH {
        return Err(CoreError::InvalidRequest(format!(
            "project_id exceeds maximum length of {MAX_PROJECT_ID_LENGTH} characters",
        )));
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        return Err(CoreError::InvalidRequest(
            "project_id must not start or end with '-'".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(CoreError::InvalidRequest(
            "project_id contains unsupported characters (allowed: letters, digits, '_', '-')"
                .to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate a tool action key (e.g. `"tool.run:danger"`).
///
/// Rules:
/// - non-empty, length ≤ [`MAX_ACTION_KEY_LENGTH`]
/// - allowed charset: `[A-Za-z0-9_.:\-]`
/// - no whitespace, no path separators, no `..`
pub fn sanitize_action_key(value: &str) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidRequest(
            "action_key must not be empty".to_string(),
        ));
    }
    if trimmed.len() > MAX_ACTION_KEY_LENGTH {
        return Err(CoreError::InvalidRequest(format!(
            "action_key exceeds maximum length of {MAX_ACTION_KEY_LENGTH} characters",
        )));
    }
    if trimmed.contains("..") {
        return Err(CoreError::InvalidRequest(
            "action_key must not contain '..'".to_string(),
        ));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(CoreError::InvalidRequest(
            "action_key must not contain whitespace".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
    {
        return Err(CoreError::InvalidRequest(
            "action_key contains unsupported characters (allowed: letters, digits, '_', '.', ':', '-')"
                .to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Optional variant of [`sanitize_name`] — returns `Ok(None)` when input is
/// `None`, otherwise delegates.
pub fn sanitize_optional_name(
    value: Option<&str>,
    field: &str,
) -> Result<Option<String>, CoreError> {
    match value {
        Some(v) => sanitize_name(v, field).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reasonable_names() {
        assert_eq!(sanitize_name("ops", "wing").unwrap(), "ops");
        assert_eq!(
            sanitize_name("auth-migration", "room").unwrap(),
            "auth-migration"
        );
        assert_eq!(
            sanitize_name("Postmortem 2026-04", "hall").unwrap(),
            "Postmortem 2026-04"
        );
        assert_eq!(sanitize_name("v1.2.3", "room").unwrap(), "v1.2.3");
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(sanitize_name("  hello  ", "wing").unwrap(), "hello");
    }

    #[test]
    fn rejects_empty_and_blank() {
        assert!(sanitize_name("", "wing").is_err());
        assert!(sanitize_name("   ", "wing").is_err());
    }

    #[test]
    fn rejects_length_overflow() {
        let huge = "a".repeat(MAX_NAME_LENGTH + 1);
        assert!(sanitize_name(&huge, "wing").is_err());
    }

    #[test]
    fn rejects_null_bytes() {
        assert!(sanitize_name("foo\0bar", "wing").is_err());
    }

    #[test]
    fn rejects_control_whitespace() {
        assert!(sanitize_name("foo\nbar", "wing").is_err());
        assert!(sanitize_name("foo\tbar", "wing").is_err());
        assert!(sanitize_name("foo\rbar", "wing").is_err());
    }

    #[test]
    fn rejects_path_separators() {
        assert!(sanitize_name("foo/bar", "wing").is_err());
        assert!(sanitize_name("foo\\bar", "wing").is_err());
    }

    #[test]
    fn rejects_dot_dot_sequence() {
        assert!(sanitize_name("..", "wing").is_err());
        assert!(sanitize_name("foo..bar", "wing").is_err());
        assert!(sanitize_name("..foo", "wing").is_err());
    }

    #[test]
    fn rejects_leading_or_trailing_dot() {
        assert!(sanitize_name(".hidden", "wing").is_err());
        assert!(sanitize_name("trailing.", "wing").is_err());
    }

    #[test]
    fn rejects_non_ascii() {
        assert!(sanitize_name("café", "wing").is_err());
        assert!(sanitize_name("日本語", "wing").is_err());
    }

    #[test]
    fn rejects_weird_punctuation() {
        assert!(sanitize_name("foo!bar", "wing").is_err());
        assert!(sanitize_name("foo@bar", "wing").is_err());
        assert!(sanitize_name("foo'bar", "wing").is_err());
        assert!(sanitize_name("foo#bar", "wing").is_err());
    }

    #[test]
    fn accepts_colon_for_namespaced_names() {
        assert_eq!(
            sanitize_name("hook:precompact", "hall").unwrap(),
            "hook:precompact"
        );
        assert_eq!(sanitize_name("event:stop", "room").unwrap(), "event:stop");
    }

    #[test]
    fn project_id_charset_is_strict() {
        assert!(sanitize_project_id("ops-2026").is_ok());
        assert!(sanitize_project_id("a_b").is_ok());
        // spaces not allowed in project ids
        assert!(sanitize_project_id("has space").is_err());
        // dots not allowed either
        assert!(sanitize_project_id("v1.2").is_err());
        // leading dash
        assert!(sanitize_project_id("-foo").is_err());
        assert!(sanitize_project_id("foo-").is_err());
        // length
        let huge = "a".repeat(MAX_PROJECT_ID_LENGTH + 1);
        assert!(sanitize_project_id(&huge).is_err());
    }

    #[test]
    fn action_key_accepts_colon_and_dot() {
        assert!(sanitize_action_key("tool.run:danger").is_ok());
        assert!(sanitize_action_key("docs.create").is_ok());
        assert!(sanitize_action_key("memory.ingest_conversation").is_ok());
    }

    #[test]
    fn action_key_rejects_whitespace_and_traversal() {
        assert!(sanitize_action_key("tool run").is_err());
        assert!(sanitize_action_key("..danger").is_err());
        assert!(sanitize_action_key("tool..run").is_err());
    }

    #[test]
    fn sanitize_optional_passes_none_through() {
        assert_eq!(sanitize_optional_name(None, "wing").unwrap(), None);
        assert_eq!(
            sanitize_optional_name(Some("ops"), "wing").unwrap(),
            Some("ops".to_string())
        );
        assert!(sanitize_optional_name(Some(""), "wing").is_err());
    }
}
