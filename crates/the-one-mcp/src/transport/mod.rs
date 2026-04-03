pub mod jsonrpc;
pub mod sse;
pub mod stdio;
pub mod stream;
pub mod tools;

use async_trait::async_trait;
use std::sync::Arc;

use crate::broker::McpBroker;
use the_one_core::error::CoreError;

#[async_trait]
pub trait Transport: Send + Sync {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError>;
}
