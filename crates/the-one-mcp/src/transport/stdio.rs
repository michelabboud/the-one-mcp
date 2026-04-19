//! Stdio JSON-RPC transport.
//!
//! Reads one JSON-RPC request per line from stdin, dispatches it to the
//! broker, and writes the response to stdout with a trailing newline. The
//! transport loop is abstracted into [`serve_pipe`] so tests can drive it
//! with in-memory pipes instead of spawning a subprocess.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::jsonrpc::{dispatch, JsonRpcRequest, JsonRpcResponse};
use super::Transport;
use crate::broker::McpBroker;
use the_one_core::error::CoreError;

pub struct StdioTransport;

/// Parse one JSON-RPC line into a request, or produce an error response for
/// the caller to emit. Extracted so both [`serve_pipe`] and
/// [`StdioTransport::run`] can share the parsing logic and both have a test
/// surface.
///
/// The `Err` variant is boxed to keep `Result<_, _>` small (clippy::result_large_err).
#[allow(clippy::result_large_err)]
pub(crate) fn parse_line(line: &str) -> Result<JsonRpcRequest, Box<JsonRpcResponse>> {
    serde_json::from_str::<JsonRpcRequest>(line).map_err(|e| {
        Box::new(JsonRpcResponse::error(
            None,
            -32700,
            format!("parse error: {e}"),
        ))
    })
}

/// Drive the JSON-RPC dispatch loop against arbitrary async pipes. Consumed
/// by [`StdioTransport::run`] and by the integration tests in
/// `tests/stdio_write_path.rs`.
///
/// This function is intentionally a free function so integration tests can
/// feed it in-memory pipes without spawning subprocesses. Each request is
/// read as one line from `reader`, dispatched to the broker, and the
/// single-line response is written to `writer` followed by `\n`.
pub async fn serve_pipe<R, W>(
    broker: Arc<McpBroker>,
    reader: R,
    mut writer: W,
) -> Result<(), CoreError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request = match parse_line(&line) {
            Ok(r) => r,
            Err(err) => {
                write_response(&mut writer, &err).await;
                continue;
            }
        };
        // parse_line boxed the error variant; the happy path continues with
        // the unboxed request as before.

        // Notifications return None — per JSON-RPC 2.0 §4.1 we must NOT
        // write any frame back, otherwise strict clients (e.g. Claude Code's
        // Zod-validated stdio transport) close the connection.
        if let Some(response) = dispatch(&broker, request).await {
            write_response(&mut writer, &response).await;
        }
    }

    Ok(())
}

async fn write_response<W: AsyncWrite + Unpin>(writer: &mut W, response: &JsonRpcResponse) {
    let response_json = serde_json::to_string(response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialization error"}}"#.to_string()
    });
    let _ = writer.write_all(response_json.as_bytes()).await;
    let _ = writer.write_all(b"\n").await;
    let _ = writer.flush().await;
}

#[async_trait]
impl Transport for StdioTransport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        serve_pipe(broker, BufReader::new(stdin), stdout).await
    }
}
