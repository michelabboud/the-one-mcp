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
}
