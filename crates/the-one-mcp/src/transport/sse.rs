use async_trait::async_trait;
use axum::{
    extract::State,
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{get, post},
    Router as AxumRouter,
};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::jsonrpc::{dispatch, JsonRpcRequest};
use super::Transport;
use crate::broker::McpBroker;
use the_one_core::error::CoreError;

#[derive(Clone)]
struct SseState {
    broker: Arc<McpBroker>,
    tx: broadcast::Sender<String>,
}

pub struct SseTransport {
    pub port: u16,
}

#[async_trait]
impl Transport for SseTransport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError> {
        let (tx, _) = broadcast::channel(100);
        let state = SseState { broker, tx };

        let app = AxumRouter::new()
            .route("/message", post(handle_message))
            .route("/sse", get(handle_sse))
            .with_state(state);

        let addr = format!("127.0.0.1:{}", self.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| CoreError::Transport(format!("SSE bind failed: {e}")))?;

        tracing::info!("SSE transport listening on {addr}");
        axum::serve(listener, app)
            .await
            .map_err(|e| CoreError::Transport(format!("SSE serve failed: {e}")))?;

        Ok(())
    }
}

async fn handle_message(
    State(state): State<SseState>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = dispatch(&state.broker, request).await;
    Json(response)
}

async fn handle_sse(
    State(state): State<SseState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|msg: Result<String, _>| msg.ok())
        .map(|msg| Ok(Event::default().data(msg)));
    Sse::new(stream)
}
