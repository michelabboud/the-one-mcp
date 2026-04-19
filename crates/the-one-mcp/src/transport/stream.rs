use async_trait::async_trait;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::post,
    Router as AxumRouter,
};
use std::convert::Infallible;
use std::sync::Arc;

use super::jsonrpc::{dispatch, JsonRpcRequest};
use super::Transport;
use crate::broker::McpBroker;
use the_one_core::error::CoreError;

pub struct StreamableHttpTransport {
    pub port: u16,
}

#[async_trait]
impl Transport for StreamableHttpTransport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError> {
        let app = AxumRouter::new()
            .route("/mcp", post(handle_mcp))
            .with_state(broker);

        let addr = format!("127.0.0.1:{}", self.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| CoreError::Transport(format!("stream bind failed: {e}")))?;

        tracing::info!("Streamable HTTP transport listening on {addr}");
        axum::serve(listener, app)
            .await
            .map_err(|e| CoreError::Transport(format!("stream serve failed: {e}")))?;

        Ok(())
    }
}

async fn handle_mcp(
    State(broker): State<Arc<McpBroker>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // JSON-RPC 2.0 §4.1: notifications produce no response. MCP's HTTP
    // streamable mapping returns 202 Accepted with an empty body in that
    // case.
    let Some(response) = dispatch(&broker, request).await else {
        return StatusCode::ACCEPTED.into_response();
    };

    // Check if client requested SSE streaming
    let accepts_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false);

    if accepts_sse {
        // Return as SSE event
        let json = serde_json::to_string(&response).unwrap_or_default();
        let stream = tokio_stream::once(Ok::<_, Infallible>(Event::default().data(json)));
        Sse::new(stream).into_response()
    } else {
        // Return as plain JSON
        Json(response).into_response()
    }
}
