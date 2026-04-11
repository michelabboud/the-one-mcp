//! Backend selection env var parser (v0.16.0 Phase 2).
//!
//! This module owns the four-variable surface through which operators
//! tell the broker which state and vector backends to use:
//!
//! ```text
//! THE_ONE_STATE_TYPE    — sqlite | postgres | redis | postgres-combined | redis-combined
//! THE_ONE_STATE_URL     — connection string (may carry credentials)
//! THE_ONE_VECTOR_TYPE   — qdrant | pgvector | redis-vectors | postgres-combined | redis-combined
//! THE_ONE_VECTOR_URL    — connection string (may carry credentials)
//! ```
//!
//! [`BackendSelection::from_env`] reads the four vars, parses them
//! into typed enums, and enforces every rule in § 3 of the backend
//! selection scheme (see `docs/plans/2026-04-11-resume-phase1-onwards.md`).
//! Any inconsistency fails loud at startup as
//! [`CoreError::InvalidProjectConfig`] — which the v0.15.0 error
//! sanitizer passes through verbatim to the operator.
//!
//! ## Parse order
//!
//! First-match fail, NOT collect-all. If an operator sets both an
//! unknown TYPE and a missing URL, they see ONE error, not a combined
//! list. The order is deterministic and documented so the operator
//! knows what to expect:
//!
//! 1. `THE_ONE_STATE_TYPE`  — must be a known enum value (or unset)
//! 2. `THE_ONE_STATE_URL`   — required iff `STATE_TYPE` is set and != sqlite
//! 3. `THE_ONE_VECTOR_TYPE` — must be a known enum value (or unset)
//! 4. `THE_ONE_VECTOR_URL`  — required iff `VECTOR_TYPE` is set and != qdrant
//! 5. Cross-axis asymmetry  — one TYPE set, other unset → fail
//! 6. Combined matching     — if either TYPE ends in `-combined`, both must match
//! 7. Combined URL equality — if both TYPEs are `*-combined`, URLs must be byte-identical
//!
//! ## Why first-match, not collect-all
//!
//! Collecting multiple errors sounds friendlier but (a) one error
//! usually cascades (an unknown TYPE makes the URL check meaningless),
//! (b) doubles the test matrix for every combination, and (c) breaks
//! the v0.15.0 "one `corr=<id>` per error" invariant by needing a
//! plural envelope.
//!
//! ## Test isolation
//!
//! Every test in this module wraps its env var mutation in
//! `temp_env::with_vars([...], || { ... })`. Never use
//! `std::env::set_var` directly — parallel `cargo test` runs will
//! poison each other.

use std::env;

use crate::error::CoreError;

// ---------------------------------------------------------------------------
// Env var names (single source of truth)
// ---------------------------------------------------------------------------

pub const ENV_STATE_TYPE: &str = "THE_ONE_STATE_TYPE";
pub const ENV_STATE_URL: &str = "THE_ONE_STATE_URL";
pub const ENV_VECTOR_TYPE: &str = "THE_ONE_VECTOR_TYPE";
pub const ENV_VECTOR_URL: &str = "THE_ONE_VECTOR_URL";

// ---------------------------------------------------------------------------
// Closed enums
// ---------------------------------------------------------------------------

/// State-store backend choice. Closed enum — parsing an unknown value
/// fails loud at startup with the full list of known values in the
/// error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTypeChoice {
    /// `THE_ONE_STATE_TYPE` unset, or explicitly = `"sqlite"`.
    /// Default; the 95% deployment. No connection URL required.
    Sqlite,
    /// `THE_ONE_STATE_TYPE = "postgres"` — split-pool Postgres state
    /// store. Ships in Phase 3.
    Postgres,
    /// `THE_ONE_STATE_TYPE = "redis"` — Redis state store, cache or
    /// persistent mode selected via `config.json`'s `[state.redis]`
    /// section. Ships in Phase 5.
    Redis,
    /// `THE_ONE_STATE_TYPE = "postgres-combined"` — state and vectors
    /// share ONE `sqlx::PgPool` for transactional consistency. Ships
    /// in Phase 4.
    PostgresCombined,
    /// `THE_ONE_STATE_TYPE = "redis-combined"` — state and vectors
    /// share ONE `fred::Client`. Ships in Phase 6.
    RedisCombined,
}

impl StateTypeChoice {
    /// Every known value, in documentation order. Used to build error
    /// messages when parsing fails.
    pub const KNOWN: &'static [&'static str] = &[
        "sqlite",
        "postgres",
        "redis",
        "postgres-combined",
        "redis-combined",
    ];

    /// String form of this choice — the literal env var value that
    /// would select it. Used for error messages and debugging.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::Redis => "redis",
            Self::PostgresCombined => "postgres-combined",
            Self::RedisCombined => "redis-combined",
        }
    }

    /// True iff this is a `*-combined` variant. Used to enforce the
    /// cross-axis matching rule.
    pub const fn is_combined(&self) -> bool {
        matches!(self, Self::PostgresCombined | Self::RedisCombined)
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "sqlite" => Some(Self::Sqlite),
            "postgres" => Some(Self::Postgres),
            "redis" => Some(Self::Redis),
            "postgres-combined" => Some(Self::PostgresCombined),
            "redis-combined" => Some(Self::RedisCombined),
            _ => None,
        }
    }
}

/// Vector backend choice. Closed enum, same parse-or-fail rules as
/// [`StateTypeChoice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorTypeChoice {
    /// Default. `THE_ONE_VECTOR_TYPE` unset, or = `"qdrant"`. No URL
    /// requirement (qdrant URL lives in `config.json` per legacy
    /// convention).
    Qdrant,
    /// `THE_ONE_VECTOR_TYPE = "pgvector"` — the Phase 2 addition.
    /// Requires `THE_ONE_VECTOR_URL`.
    Pgvector,
    /// `THE_ONE_VECTOR_TYPE = "redis-vectors"` — existing RedisVector
    /// backend, now addressable via the unified scheme. Requires
    /// `THE_ONE_VECTOR_URL`.
    RedisVectors,
    /// `THE_ONE_VECTOR_TYPE = "postgres-combined"` — pair of
    /// `PostgresCombined` on state axis.
    PostgresCombined,
    /// `THE_ONE_VECTOR_TYPE = "redis-combined"` — pair of
    /// `RedisCombined` on state axis.
    RedisCombined,
}

impl VectorTypeChoice {
    pub const KNOWN: &'static [&'static str] = &[
        "qdrant",
        "pgvector",
        "redis-vectors",
        "postgres-combined",
        "redis-combined",
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Qdrant => "qdrant",
            Self::Pgvector => "pgvector",
            Self::RedisVectors => "redis-vectors",
            Self::PostgresCombined => "postgres-combined",
            Self::RedisCombined => "redis-combined",
        }
    }

    pub const fn is_combined(&self) -> bool {
        matches!(self, Self::PostgresCombined | Self::RedisCombined)
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "qdrant" => Some(Self::Qdrant),
            "pgvector" => Some(Self::Pgvector),
            "redis-vectors" => Some(Self::RedisVectors),
            "postgres-combined" => Some(Self::PostgresCombined),
            "redis-combined" => Some(Self::RedisCombined),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Result struct
// ---------------------------------------------------------------------------

/// Resolved backend selection. Parsed once at broker construction
/// (not per-call) and stashed on `McpBroker` for the lifetime of the
/// process. Layer 4 of the five-layer config stack.
#[derive(Debug, Clone)]
pub struct BackendSelection {
    pub state: StateTypeChoice,
    pub vector: VectorTypeChoice,
    /// Connection URL for the state backend. `None` iff
    /// `state == Sqlite` (which derives its path from
    /// `THE_ONE_PROJECT_ROOT`). `Some` for everything else.
    pub state_url: Option<String>,
    /// Connection URL for the vector backend. `None` iff
    /// `vector == Qdrant` (which reads its URL from `config.json`).
    /// `Some` for everything else.
    pub vector_url: Option<String>,
}

impl BackendSelection {
    /// The zero-configuration default: `(Sqlite, Qdrant)` with no
    /// URLs. This is what [`from_env`] returns when all four env
    /// vars are unset.
    ///
    /// [`from_env`]: Self::from_env
    pub const fn default_sqlite_qdrant() -> Self {
        Self {
            state: StateTypeChoice::Sqlite,
            vector: VectorTypeChoice::Qdrant,
            state_url: None,
            vector_url: None,
        }
    }

    /// Parse the four env vars per the rules in the module docs.
    ///
    /// Returns `Err(CoreError::InvalidProjectConfig(...))` on any
    /// rule violation, with a message crafted to appear verbatim in
    /// the operator's log (v0.15.0 error sanitizer pass-through
    /// invariant).
    pub fn from_env() -> Result<Self, CoreError> {
        let state_type_raw = env::var(ENV_STATE_TYPE).ok();
        let state_url_raw = env::var(ENV_STATE_URL).ok();
        let vector_type_raw = env::var(ENV_VECTOR_TYPE).ok();
        let vector_url_raw = env::var(ENV_VECTOR_URL).ok();

        // If every knob is unset, the broker uses the v0.15.x default.
        // This is the 95% deployment path; keep it silent and fast.
        if state_type_raw.is_none()
            && state_url_raw.is_none()
            && vector_type_raw.is_none()
            && vector_url_raw.is_none()
        {
            return Ok(Self::default_sqlite_qdrant());
        }

        // 1. Parse STATE_TYPE.
        let state: Option<StateTypeChoice> = state_type_raw
            .as_deref()
            .map(|v| {
                StateTypeChoice::parse(v)
                    .ok_or_else(|| unknown_type_err(ENV_STATE_TYPE, v, StateTypeChoice::KNOWN))
            })
            .transpose()?;

        // 2. STATE_URL is required iff STATE_TYPE is set AND != sqlite.
        let state_url = match state {
            Some(StateTypeChoice::Sqlite) | None => state_url_raw.clone(),
            Some(t) => Some(required_url(
                ENV_STATE_TYPE,
                t.as_str(),
                ENV_STATE_URL,
                state_url_raw.clone(),
            )?),
        };

        // 3. Parse VECTOR_TYPE.
        let vector: Option<VectorTypeChoice> = vector_type_raw
            .as_deref()
            .map(|v| {
                VectorTypeChoice::parse(v)
                    .ok_or_else(|| unknown_type_err(ENV_VECTOR_TYPE, v, VectorTypeChoice::KNOWN))
            })
            .transpose()?;

        // 4. VECTOR_URL is required iff VECTOR_TYPE is set AND != qdrant.
        let vector_url = match vector {
            Some(VectorTypeChoice::Qdrant) | None => vector_url_raw.clone(),
            Some(t) => Some(required_url(
                ENV_VECTOR_TYPE,
                t.as_str(),
                ENV_VECTOR_URL,
                vector_url_raw.clone(),
            )?),
        };

        // 5. Cross-axis asymmetry. Once the operator has set either
        //    TYPE explicitly (we got here because at least one of the
        //    four env vars was set), BOTH TYPEs must be set. This is
        //    the rule that prevents "I set STATE_TYPE=postgres and
        //    the broker silently picked Qdrant for my vectors."
        match (state, vector) {
            (Some(_), Some(_)) => {}
            (Some(s), None) => {
                return Err(asymmetry_err(ENV_STATE_TYPE, s.as_str(), ENV_VECTOR_TYPE));
            }
            (None, Some(v)) => {
                return Err(asymmetry_err(ENV_VECTOR_TYPE, v.as_str(), ENV_STATE_TYPE));
            }
            (None, None) => {
                // This branch is only reachable if the operator set
                // _URL without _TYPE — which is itself a silent
                // misconfiguration. Fail loud.
                return Err(CoreError::InvalidProjectConfig(
                    "THE_ONE_STATE_URL or THE_ONE_VECTOR_URL is set but neither \
                     THE_ONE_STATE_TYPE nor THE_ONE_VECTOR_TYPE is set; set at least \
                     one TYPE explicitly or unset every THE_ONE_* env var to use the \
                     default sqlite + qdrant backend."
                        .to_string(),
                ));
            }
        }

        let state = state.expect("both TYPEs Some by match above");
        let vector = vector.expect("both TYPEs Some by match above");

        // 6. Combined matching. If EITHER side is `*-combined`, BOTH
        //    must be the same combined value. Mixing `postgres-combined`
        //    with `redis-combined` is nonsensical — different tech
        //    stacks.
        if state.is_combined() || vector.is_combined() {
            if state.as_str() != vector.as_str() {
                return Err(CoreError::InvalidProjectConfig(format!(
                    "Combined backends must match: {ENV_STATE_TYPE}={state} requires \
                     {ENV_VECTOR_TYPE}={state} (got {ENV_VECTOR_TYPE}={vector})",
                    state = state.as_str(),
                    vector = vector.as_str(),
                )));
            }

            // 7. Combined URL equality. When both TYPEs are the same
            //    `*-combined` value, both URLs must be byte-identical
            //    so the broker can instantiate ONE connection pool
            //    that serves both trait roles.
            let s_url = state_url
                .as_deref()
                .expect("combined state always sets URL");
            let v_url = vector_url
                .as_deref()
                .expect("combined vector always sets URL");
            if s_url != v_url {
                return Err(CoreError::InvalidProjectConfig(format!(
                    "Combined {state}: {ENV_STATE_URL} and {ENV_VECTOR_URL} must be \
                     byte-identical; got state_url={s_url} vs vector_url={v_url}",
                    state = state.as_str(),
                )));
            }
        }

        // 8. Non-combined + same URL → allowed, silent. Operators
        //    sometimes want split pools sharing a host for separate
        //    credential rotation or statement-timeout isolation.
        //    No rule fires here by design.

        Ok(Self {
            state,
            vector,
            state_url,
            vector_url,
        })
    }
}

// ---------------------------------------------------------------------------
// Error message builders
// ---------------------------------------------------------------------------

fn unknown_type_err(var: &str, value: &str, known: &[&str]) -> CoreError {
    CoreError::InvalidProjectConfig(format!(
        "Unknown {var}={value}; expected one of: {}",
        known.join(", ")
    ))
}

fn required_url(
    type_var: &str,
    type_value: &str,
    url_var: &str,
    maybe_url: Option<String>,
) -> Result<String, CoreError> {
    match maybe_url {
        Some(ref u) if !u.trim().is_empty() => Ok(u.clone()),
        _ => Err(CoreError::InvalidProjectConfig(format!(
            "{type_var}={type_value} requires {url_var} to be set"
        ))),
    }
}

fn asymmetry_err(set_var: &str, set_value: &str, missing_var: &str) -> CoreError {
    CoreError::InvalidProjectConfig(format!(
        "{set_var}={set_value} set but {missing_var} is unset; both axes must be \
         explicit when either is overridden."
    ))
}

// ---------------------------------------------------------------------------
// Tests — all isolated via temp_env::with_vars
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: run [`BackendSelection::from_env`] inside a
    /// `temp_env::with_vars` block so env mutations don't poison
    /// parallel test runs.
    fn with_vars<F, R>(pairs: Vec<(&'static str, Option<&'static str>)>, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        temp_env::with_vars(pairs, f)
    }

    #[test]
    fn both_unset_defaults_silently() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, None),
                (ENV_STATE_URL, None),
                (ENV_VECTOR_TYPE, None),
                (ENV_VECTOR_URL, None),
            ],
            || {
                let sel = BackendSelection::from_env().expect("defaults should parse");
                assert_eq!(sel.state, StateTypeChoice::Sqlite);
                assert_eq!(sel.vector, VectorTypeChoice::Qdrant);
                assert!(sel.state_url.is_none());
                assert!(sel.vector_url.is_none());
            },
        );
    }

    #[test]
    fn only_vector_type_set_fails_loud() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, None),
                (ENV_STATE_URL, None),
                (ENV_VECTOR_TYPE, Some("pgvector")),
                (ENV_VECTOR_URL, Some("postgres://localhost/test")),
            ],
            || {
                let err = BackendSelection::from_env().expect_err("one-side-only should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig, got {err:?}");
                };
                assert!(
                    msg.contains("THE_ONE_VECTOR_TYPE=pgvector"),
                    "err missing vector type hint: {msg}"
                );
                assert!(
                    msg.contains("THE_ONE_STATE_TYPE is unset"),
                    "err missing state-unset hint: {msg}"
                );
            },
        );
    }

    #[test]
    fn only_state_type_set_fails_loud() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres")),
                (ENV_STATE_URL, Some("postgres://localhost/test")),
                (ENV_VECTOR_TYPE, None),
                (ENV_VECTOR_URL, None),
            ],
            || {
                let err = BackendSelection::from_env().expect_err("state-only should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                assert!(msg.contains("THE_ONE_STATE_TYPE=postgres"), "msg={msg}");
                assert!(msg.contains("THE_ONE_VECTOR_TYPE is unset"), "msg={msg}");
            },
        );
    }

    #[test]
    fn unknown_type_fails_with_enum_list() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("sqlite")),
                (ENV_VECTOR_TYPE, Some("pgsql")),
                (ENV_VECTOR_URL, Some("postgres://localhost/test")),
                (ENV_STATE_URL, None),
            ],
            || {
                let err =
                    BackendSelection::from_env().expect_err("unknown vector type should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                // The error must echo the bad value AND every known
                // value so the operator sees exactly what's allowed.
                assert!(msg.contains("pgsql"), "msg={msg}");
                for known in VectorTypeChoice::KNOWN {
                    assert!(
                        msg.contains(known),
                        "err missing known value {known}: {msg}"
                    );
                }
            },
        );
    }

    #[test]
    fn type_without_url_fails() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres")),
                (ENV_STATE_URL, None),
                (ENV_VECTOR_TYPE, Some("pgvector")),
                (ENV_VECTOR_URL, Some("postgres://localhost/test")),
            ],
            || {
                let err = BackendSelection::from_env().expect_err("missing state URL should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                assert!(msg.contains("THE_ONE_STATE_TYPE=postgres"), "msg={msg}");
                assert!(msg.contains("THE_ONE_STATE_URL"), "msg={msg}");
            },
        );
    }

    #[test]
    fn combined_mismatch_fails() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres-combined")),
                (ENV_STATE_URL, Some("postgres://host/db")),
                (ENV_VECTOR_TYPE, Some("qdrant")),
                (ENV_VECTOR_URL, None),
            ],
            || {
                let err = BackendSelection::from_env()
                    .expect_err("postgres-combined + qdrant should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                assert!(msg.contains("Combined backends must match"), "msg={msg}");
                assert!(msg.contains("postgres-combined"), "msg={msg}");
            },
        );
    }

    #[test]
    fn combined_url_mismatch_fails() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres-combined")),
                (ENV_STATE_URL, Some("postgres://host-a/db")),
                (ENV_VECTOR_TYPE, Some("postgres-combined")),
                (ENV_VECTOR_URL, Some("postgres://host-b/db")),
            ],
            || {
                let err =
                    BackendSelection::from_env().expect_err("mismatched combined URLs should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                // Both URLs must be echoed back so the operator can
                // spot-diff them in the log.
                assert!(msg.contains("postgres://host-a/db"), "msg={msg}");
                assert!(msg.contains("postgres://host-b/db"), "msg={msg}");
                assert!(msg.contains("byte-identical"), "msg={msg}");
            },
        );
    }

    #[test]
    fn both_non_combined_same_url_allowed() {
        // Operator wants split pools sharing a host (separate
        // credential rotation, statement-timeout isolation).
        // Allowed, silent.
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres")),
                (ENV_STATE_URL, Some("postgres://shared-host/db")),
                (ENV_VECTOR_TYPE, Some("pgvector")),
                (ENV_VECTOR_URL, Some("postgres://shared-host/db")),
            ],
            || {
                let sel = BackendSelection::from_env()
                    .expect("split pools on same host should be allowed");
                assert_eq!(sel.state, StateTypeChoice::Postgres);
                assert_eq!(sel.vector, VectorTypeChoice::Pgvector);
                assert_eq!(sel.state_url.as_deref(), Some("postgres://shared-host/db"));
                assert_eq!(sel.vector_url.as_deref(), Some("postgres://shared-host/db"));
            },
        );
    }

    #[test]
    fn cross_combined_tech_fails() {
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres-combined")),
                (ENV_STATE_URL, Some("postgres://host/db")),
                (ENV_VECTOR_TYPE, Some("redis-combined")),
                (ENV_VECTOR_URL, Some("redis://host")),
            ],
            || {
                let err = BackendSelection::from_env()
                    .expect_err("postgres-combined vs redis-combined should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                assert!(msg.contains("Combined backends must match"), "msg={msg}");
                assert!(msg.contains("postgres-combined"), "msg={msg}");
                assert!(msg.contains("redis-combined"), "msg={msg}");
            },
        );
    }

    #[test]
    fn postgres_combined_matched_urls_parses_clean() {
        // Positive control for the combined-matching rules: when
        // both TYPEs and both URLs align, the parser succeeds.
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres-combined")),
                (ENV_STATE_URL, Some("postgres://shared/db")),
                (ENV_VECTOR_TYPE, Some("postgres-combined")),
                (ENV_VECTOR_URL, Some("postgres://shared/db")),
            ],
            || {
                let sel = BackendSelection::from_env().expect("should parse");
                assert_eq!(sel.state, StateTypeChoice::PostgresCombined);
                assert_eq!(sel.vector, VectorTypeChoice::PostgresCombined);
            },
        );
    }

    #[test]
    fn pgvector_split_pool_parses_clean() {
        // Positive control for the Phase 2 primary deployment:
        // sqlite state + pgvector vectors, pgvector URL set.
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("sqlite")),
                (ENV_STATE_URL, None),
                (ENV_VECTOR_TYPE, Some("pgvector")),
                (ENV_VECTOR_URL, Some("postgres://localhost/thenone")),
            ],
            || {
                let sel = BackendSelection::from_env().expect("should parse");
                assert_eq!(sel.state, StateTypeChoice::Sqlite);
                assert_eq!(sel.vector, VectorTypeChoice::Pgvector);
                assert!(sel.state_url.is_none());
                assert_eq!(
                    sel.vector_url.as_deref(),
                    Some("postgres://localhost/thenone")
                );
            },
        );
    }

    #[test]
    fn empty_url_string_treated_as_missing() {
        // An empty `THE_ONE_STATE_URL=""` export (common shell
        // mistake) should not satisfy the URL-required rule.
        with_vars(
            vec![
                (ENV_STATE_TYPE, Some("postgres")),
                (ENV_STATE_URL, Some("")),
                (ENV_VECTOR_TYPE, Some("pgvector")),
                (ENV_VECTOR_URL, Some("postgres://localhost/test")),
            ],
            || {
                let err = BackendSelection::from_env().expect_err("empty URL should fail");
                let CoreError::InvalidProjectConfig(msg) = err else {
                    panic!("expected InvalidProjectConfig");
                };
                assert!(msg.contains("THE_ONE_STATE_URL"), "msg={msg}");
            },
        );
    }
}
