use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::Transport;
use super::jsonrpc::{dispatch, JsonRpcRequest};
use crate::broker::McpBroker;
use the_one_core::error::CoreError;

pub struct StdioTransport;

#[async_trait]
impl Transport for StdioTransport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let error_response = super::jsonrpc::JsonRpcResponse::error(
                        None,
                        -32700,
                        format!("parse error: {e}"),
                    );
                    let response_json =
                        serde_json::to_string(&error_response).unwrap_or_default();
                    let _ = stdout.write_all(response_json.as_bytes()).await;
                    let _ = stdout.write_all(b"\n").await;
                    let _ = stdout.flush().await;
                    continue;
                }
            };

            let response = dispatch(&broker, request).await;
            let response_json = serde_json::to_string(&response).unwrap_or_else(|_| {
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialization error"}}"#
                    .to_string()
            });

            let _ = stdout.write_all(response_json.as_bytes()).await;
            let _ = stdout.write_all(b"\n").await;
            let _ = stdout.flush().await;
        }

        Ok(())
    }
}
