use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("json failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid project configuration: {0}")]
    InvalidProjectConfig(String),
    #[error("policy denied action: {0}")]
    PolicyDenied(String),
    #[error("unsupported schema version: {0}")]
    UnsupportedSchemaVersion(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("document error: {0}")]
    Document(String),
    #[error("catalog error: {0}")]
    Catalog(String),
    #[error("feature not enabled: {0}")]
    NotEnabled(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// v0.16.0 Phase 3 — `PostgresStateStore` operation failed. Covers
    /// sqlx connection/pool errors, query errors, migration failures,
    /// and any other runtime backend error. Constructed as a `String`
    /// because sqlx's error types are not `#[from]`-friendly without
    /// adding sqlx as an unconditional dep of this crate. The wire-
    /// level sanitizer maps this to the short label `"postgres"` so
    /// internal error text never leaks to clients.
    #[error("postgres failure: {0}")]
    Postgres(String),
    /// v0.16.0 Phase 5 — `RedisStateStore` operation failed. Same
    /// pattern as `Postgres(String)` — wraps fred client errors as
    /// strings so the crate doesn't need to expose fred types
    /// unconditionally. Wire-level sanitizer maps this to `"redis"`.
    #[error("redis failure: {0}")]
    Redis(String),
}
