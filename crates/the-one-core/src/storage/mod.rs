pub mod sqlite;

// v0.16.0 Phase 3 — PostgresStateStore backend. Gated on `pg-state`
// feature; off by default. See `crates/the-one-core/Cargo.toml` for
// the sqlx feature-narrowing rationale (same migrate+chrono conflict
// that Phase 2's pgvector backend worked around).
#[cfg(feature = "pg-state")]
pub mod postgres;
