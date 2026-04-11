use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use the_one_core::audit::error_kind_label;
use the_one_core::error::CoreError;

use crate::api::*;
use crate::broker::McpBroker;

/// Monotonically-increasing correlation ID stamped on every JSON-RPC error
/// response. Operators can grep the server log for this ID to find the full,
/// un-sanitised error context. The ID resets on process restart.
static CORRELATION_ID: AtomicU64 = AtomicU64::new(1);

fn next_correlation_id() -> String {
    let n = CORRELATION_ID.fetch_add(1, Ordering::Relaxed);
    format!("corr-{n:08x}")
}

/// Convert a [`CoreError`] into a client-safe JSON-RPC error message.
///
/// Prior to v0.15.0 every error path passed `e.to_string()` straight into
/// the client response, which leaked rusqlite schema details, absolute
/// paths, and other internals. This function is the chokepoint that
/// sanitises them.
///
/// The public message is the short, stable `error_kind_label` — e.g.
/// `"sqlite"`, `"io"`, `"invalid_request"`. A correlation ID is appended so
/// operators can match the response to the matching `tracing::error!` line
/// in the server log, which still contains the full details.
///
/// Exception: [`CoreError::InvalidRequest`], [`CoreError::NotEnabled`],
/// [`CoreError::PolicyDenied`], and [`CoreError::InvalidProjectConfig`]
/// carry deliberately-crafted human-readable messages (e.g. "wing must not
/// be empty"). These are safe to pass through verbatim — they do not
/// contain paths, schema, or secrets — and they tell the client what to fix.
pub(crate) fn public_error_message(err: &CoreError) -> (i32, String) {
    let correlation = next_correlation_id();
    tracing::error!(
        target: "the_one_mcp::jsonrpc",
        correlation_id = %correlation,
        kind = %error_kind_label(err),
        error = %err,
        "request failed"
    );
    let kind = error_kind_label(err);
    let code = match err {
        CoreError::InvalidRequest(_) | CoreError::InvalidProjectConfig(_) => INVALID_PARAMS,
        _ => INTERNAL_ERROR,
    };
    let public_detail = match err {
        // Safe to surface — these carry user-facing messages by design.
        CoreError::InvalidRequest(msg)
        | CoreError::NotEnabled(msg)
        | CoreError::PolicyDenied(msg)
        | CoreError::InvalidProjectConfig(msg) => msg.clone(),
        // Everything else: redacted label only.
        _ => kind.to_string(),
    };
    (
        code,
        format!("{public_detail} (kind={kind}, corr={correlation})"),
    )
}

/// Convert a `Result<T, CoreError>` into either the `Ok` value or a
/// pre-sanitised `(code, message)` pair — helper for call sites that need
/// to propagate errors as Strings.
pub(crate) fn map_core_error_to_string(err: &CoreError) -> String {
    let (_, msg) = public_error_message(err);
    msg
}

/// Convert a `serde_json::Error` (from serializing our own broker response
/// types) into a generic client-safe message. Serialization errors on
/// controlled types are effectively unreachable, but if one slips through
/// we log it with a correlation ID and return a short, generic label
/// instead of exposing serde internals like "trailing characters at line X".
pub(crate) fn serialize_error_to_string(err: serde_json::Error) -> String {
    let correlation = next_correlation_id();
    tracing::error!(
        target: "the_one_mcp::jsonrpc",
        correlation_id = %correlation,
        error = %err,
        "response serialization failed"
    );
    format!("response serialization failed (corr={correlation})")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

/// Standard JSON-RPC error codes
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

/// Dispatch a JSON-RPC request to the broker.
pub async fn dispatch(broker: &McpBroker, request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    match request.method.as_str() {
        "initialize" => handle_initialize(id),
        "notifications/initialized" => JsonRpcResponse::success(id, Value::Null),
        "tools/list" => handle_tools_list(id),
        "tools/call" => handle_tools_call(broker, id, request.params).await,
        "resources/list" => handle_resources_list(broker, id, request.params).await,
        "resources/read" => handle_resources_read(broker, id, request.params).await,
        _ => JsonRpcResponse::error(
            id,
            METHOD_NOT_FOUND,
            format!("method not found: {}", request.method),
        ),
    }
}

fn handle_initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "resources": {
                    "subscribe": false,
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "the-one-mcp",
                "version": crate::MCP_SCHEMA_VERSION
            }
        }),
    )
}

async fn handle_resources_list(
    broker: &McpBroker,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
    let params = params.unwrap_or(Value::Object(Default::default()));
    let project_root = params
        .get("project_root")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let project_id = params
        .get("project_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if project_root.is_empty() || project_id.is_empty() {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "resources/list requires project_root and project_id".to_string(),
        );
    }

    match broker
        .resources_list(std::path::Path::new(project_root), project_id)
        .await
    {
        Ok(resp) => JsonRpcResponse::success(id, serde_json::to_value(resp).unwrap_or(Value::Null)),
        Err(e) => {
            let (code, msg) = public_error_message(&e);
            JsonRpcResponse::error(id, code, msg)
        }
    }
}

async fn handle_resources_read(
    broker: &McpBroker,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "missing params".to_string());
        }
    };
    let project_root = params
        .get("project_root")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let project_id = params
        .get("project_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
    if project_root.is_empty() || project_id.is_empty() || uri.is_empty() {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "resources/read requires project_root, project_id, and uri".to_string(),
        );
    }

    match broker
        .resources_read(std::path::Path::new(project_root), project_id, uri)
        .await
    {
        Ok(resp) => JsonRpcResponse::success(id, serde_json::to_value(resp).unwrap_or(Value::Null)),
        Err(e) => {
            let (code, msg) = public_error_message(&e);
            JsonRpcResponse::error(id, code, msg)
        }
    }
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "tools": super::tools::tool_definitions()
        }),
    )
}

async fn handle_tools_call(
    broker: &McpBroker,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "missing params".to_string());
        }
    };

    let tool_name = match params["name"].as_str() {
        Some(n) => n,
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "missing tool name".to_string());
        }
    };

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    match dispatch_tool(broker, tool_name, arguments).await {
        Ok(result) => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result).unwrap_or_default()
                }]
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, e),
    }
}

async fn dispatch_tool(broker: &McpBroker, tool_name: &str, args: Value) -> Result<Value, String> {
    match tool_name {
        // ── Work tools ──────────────────────────────────────────
        "memory.search" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let query = args["query"].as_str().ok_or("missing query")?;
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;
            let wing = args
                .get("wing")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let hall = args
                .get("hall")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let room = args
                .get("room")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let result = broker
                .memory_search(MemorySearchRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    query: query.to_string(),
                    top_k,
                    wing,
                    hall,
                    room,
                })
                .await;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.fetch_chunk" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let chunk_id = args["id"].as_str().ok_or("missing id")?;
            let result = broker
                .memory_fetch_chunk(MemoryFetchChunkRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    id: chunk_id.to_string(),
                })
                .await;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.ingest_conversation" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let format =
                parse_memory_conversation_format(args["format"].as_str().ok_or("missing format")?)?;
            let wing = args
                .get("wing")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let hall = args
                .get("hall")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let room = args
                .get("room")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let result = broker
                .memory_ingest_conversation(MemoryIngestConversationRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    format,
                    wing,
                    hall,
                    room,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.aaak.compress" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let format =
                parse_memory_conversation_format(args["format"].as_str().ok_or("missing format")?)?;
            let result = broker
                .memory_aaak_compress(MemoryAaakCompressRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    format,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.aaak.teach" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let format =
                parse_memory_conversation_format(args["format"].as_str().ok_or("missing format")?)?;
            let result = broker
                .memory_aaak_teach(MemoryAaakTeachRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    format,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.aaak.list_lessons" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let limit = args["limit"].as_u64().unwrap_or(20) as usize;
            let result = broker
                .memory_aaak_list_lessons(MemoryAaakListLessonsRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    limit,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.diary.add" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let entry_date = args["entry_date"].as_str().ok_or("missing entry_date")?;
            let content = args["content"].as_str().ok_or("missing content")?;
            let tags = match args.get("tags") {
                None => Vec::new(),
                Some(Value::Array(items)) => {
                    let mut tags = Vec::with_capacity(items.len());
                    for item in items {
                        let tag = item.as_str().ok_or("tags must be an array of strings")?;
                        tags.push(tag.to_string());
                    }
                    tags
                }
                Some(_) => return Err("tags must be an array of strings".to_string()),
            };
            let result = broker
                .memory_diary_add(MemoryDiaryAddRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    entry_date: entry_date.to_string(),
                    mood: args
                        .get("mood")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    tags,
                    content: content.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.diary.list" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let max_results = args["max_results"].as_u64().unwrap_or(20) as usize;
            let result = broker
                .memory_diary_list(MemoryDiaryListRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    start_date: args
                        .get("start_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    end_date: args
                        .get("end_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    max_results,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.diary.search" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let query = args["query"].as_str().ok_or("missing query")?;
            let max_results = args["max_results"].as_u64().unwrap_or(20) as usize;
            let result = broker
                .memory_diary_search(MemoryDiarySearchRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    query: query.to_string(),
                    start_date: args
                        .get("start_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    end_date: args
                        .get("end_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    max_results,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.diary.summarize" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let max_summary_items = args["max_summary_items"].as_u64().unwrap_or(12) as usize;
            let result = broker
                .memory_diary_summarize(MemoryDiarySummarizeRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    start_date: args
                        .get("start_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    end_date: args
                        .get("end_date")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    max_summary_items,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.navigation.upsert_node" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let node_id = args["node_id"].as_str().ok_or("missing node_id")?;
            let kind = args["kind"].as_str().ok_or("missing kind")?;
            let label = args["label"].as_str().ok_or("missing label")?;
            let result = broker
                .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    node_id: node_id.to_string(),
                    kind: kind.to_string(),
                    label: label.to_string(),
                    parent_node_id: args
                        .get("parent_node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    wing: args
                        .get("wing")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    hall: args
                        .get("hall")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    room: args
                        .get("room")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.navigation.link_tunnel" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let from_node_id = args["from_node_id"]
                .as_str()
                .ok_or("missing from_node_id")?;
            let to_node_id = args["to_node_id"].as_str().ok_or("missing to_node_id")?;
            let result = broker
                .memory_navigation_link_tunnel(MemoryNavigationLinkTunnelRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    from_node_id: from_node_id.to_string(),
                    to_node_id: to_node_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.navigation.list" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let limit = args["limit"].as_u64().unwrap_or(100) as usize;
            let result = broker
                .memory_navigation_list(MemoryNavigationListRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    parent_node_id: args
                        .get("parent_node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    kind: args
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    limit,
                    cursor: args
                        .get("cursor")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.navigation.traverse" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let start_node_id = args["start_node_id"]
                .as_str()
                .ok_or("missing start_node_id")?;
            let max_depth = args["max_depth"].as_u64().unwrap_or(8) as usize;
            let result = broker
                .memory_navigation_traverse(MemoryNavigationTraverseRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    start_node_id: start_node_id.to_string(),
                    max_depth,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.wake_up" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let wing = args
                .get("wing")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let hall = args
                .get("hall")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let room = args
                .get("room")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let max_items = args["max_items"].as_u64().unwrap_or(12) as usize;
            let result = broker
                .memory_wake_up(MemoryWakeUpRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    wing,
                    hall,
                    room,
                    max_items,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "docs.list" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let result = broker
                .docs_list(DocsListRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "docs.get" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            if let Some(heading) = args.get("section").and_then(|v| v.as_str()) {
                let max_bytes = args["max_bytes"].as_u64().unwrap_or(24576) as usize;
                let result = broker
                    .docs_get_section(DocsGetSectionRequest {
                        project_root: project_root.to_string(),
                        project_id: project_id.to_string(),
                        path: path.to_string(),
                        heading: heading.to_string(),
                        max_bytes,
                    })
                    .await;
                serde_json::to_value(result).map_err(serialize_error_to_string)
            } else {
                let result = broker
                    .docs_get(DocsGetRequest {
                        project_root: project_root.to_string(),
                        project_id: project_id.to_string(),
                        path: path.to_string(),
                    })
                    .await;
                serde_json::to_value(result).map_err(serialize_error_to_string)
            }
        }
        "docs.save" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let content = args["content"].as_str().ok_or("missing content")?;
            let update_result = broker
                .docs_update(DocsUpdateRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    content: content.to_string(),
                })
                .await;
            match update_result {
                Ok(r) => serde_json::to_value(DocsSaveResponse {
                    path: r.path,
                    created: false,
                })
                .map_err(serialize_error_to_string),
                Err(_) => {
                    let result = broker
                        .docs_create(DocsCreateRequest {
                            project_root: project_root.to_string(),
                            project_id: project_id.to_string(),
                            path: path.to_string(),
                            content: content.to_string(),
                        })
                        .await
                        .map_err(|e| map_core_error_to_string(&e))?;
                    serde_json::to_value(DocsSaveResponse {
                        path: result.path,
                        created: true,
                    })
                    .map_err(serialize_error_to_string)
                }
            }
        }
        "docs.delete" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let result = broker
                .docs_delete(DocsDeleteRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "docs.move" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let from = args["from"].as_str().ok_or("missing from")?;
            let to = args["to"].as_str().ok_or("missing to")?;
            let result = broker
                .docs_move(DocsMoveRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    from: from.to_string(),
                    to: to.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.find" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let mode = args["mode"].as_str().ok_or("missing mode")?;
            match mode {
                "list" => {
                    let state = args
                        .get("filter")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let request = ToolListRequest {
                        state,
                        project_root: project_root.to_string(),
                        project_id: project_id.to_string(),
                    };
                    let result = broker
                        .tool_list(request)
                        .await
                        .map_err(|e| map_core_error_to_string(&e))?;
                    serde_json::to_value(result).map_err(serialize_error_to_string)
                }
                "suggest" => {
                    let query = args["query"]
                        .as_str()
                        .ok_or("missing query for suggest mode")?;
                    let max = args["max"].as_u64().unwrap_or(5) as usize;
                    let result = broker
                        .tool_suggest(ToolSuggestRequest {
                            query: query.to_string(),
                            max,
                        })
                        .await;
                    serde_json::to_value(result).map_err(serialize_error_to_string)
                }
                "search" => {
                    let query = args["query"]
                        .as_str()
                        .ok_or("missing query for search mode")?;
                    let max = args["max"].as_u64().unwrap_or(5) as usize;
                    let result = broker
                        .tool_search(ToolSearchRequest {
                            query: query.to_string(),
                            max,
                        })
                        .await;
                    serde_json::to_value(result).map_err(serialize_error_to_string)
                }
                _ => Err(format!("unknown tool.find mode: {mode}")),
            }
        }
        "tool.info" => {
            let tool_id = args["tool_id"].as_str().ok_or("missing tool_id")?;
            let result = broker
                .tool_info(ToolInfoRequest {
                    tool_id: tool_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.install" => {
            let request = serde_json::from_value::<ToolInstallRequest>(args)
                .map_err(|e| format!("invalid tool.install params: {e}"))?;
            let result = broker
                .tool_install(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.run" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let action_key = args["action_key"].as_str().ok_or("missing action_key")?;
            let interactive = args["interactive"].as_bool().unwrap_or(false);
            let scope_str = args["approval_scope"].as_str().unwrap_or("once");
            let result = broker
                .tool_run(
                    Path::new(project_root),
                    project_id,
                    ToolRunRequest {
                        action_key: action_key.to_string(),
                        interactive,
                        approval_scope: Some(scope_str.to_string()),
                    },
                )
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.search_images" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let query = args.get("query").and_then(|v| v.as_str()).map(String::from);
            let image_base64 = args
                .get("image_base64")
                .and_then(|v| v.as_str())
                .map(String::from);
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;
            let result = broker
                .image_search(ImageSearchRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    query,
                    image_base64,
                    top_k,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.ingest_image" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let caption = args
                .get("caption")
                .and_then(|v| v.as_str())
                .map(String::from);
            let result = broker
                .image_ingest(ImageIngestRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    caption,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }

        // ── Multiplexed admin tools ─────────────────────────────
        "setup" => dispatch_setup(broker, args).await,
        "config" => dispatch_config(broker, args).await,
        "maintain" => dispatch_maintain(broker, args).await,
        "observe" => dispatch_observe(broker, args).await,

        _ => Err(format!("unknown tool: {tool_name}")),
    }
}

fn parse_memory_conversation_format(value: &str) -> Result<MemoryConversationFormat, String> {
    match value {
        "openai_messages" => Ok(MemoryConversationFormat::OpenAiMessages),
        "claude_transcript" => Ok(MemoryConversationFormat::ClaudeTranscript),
        "generic_jsonl" => Ok(MemoryConversationFormat::GenericJsonl),
        other => Err(format!("invalid format: {other}")),
    }
}

async fn dispatch_setup(broker: &McpBroker, args: Value) -> Result<Value, String> {
    let action = args["action"].as_str().ok_or("missing action")?;
    let params = args
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let project_root = params["project_root"]
        .as_str()
        .ok_or("missing params.project_root")?;
    let project_id = params["project_id"]
        .as_str()
        .ok_or("missing params.project_id")?;
    match action {
        "project" => {
            let result = broker
                .project_init(ProjectInitRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "refresh" => {
            let result = broker
                .project_refresh(ProjectRefreshRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "profile" => {
            let result = broker
                .project_profile_get(ProjectProfileGetRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        _ => Err(format!("unknown setup action: {action}")),
    }
}

async fn dispatch_config(broker: &McpBroker, args: Value) -> Result<Value, String> {
    let action = args["action"].as_str().ok_or("missing action")?;
    let params = args
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    match action {
        "export" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let result = broker
                .config_export(Path::new(project_root))
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "update" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let update = params
                .get("update")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let result = broker
                .config_update(ConfigUpdateRequest {
                    project_root: project_root.to_string(),
                    update,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "profile.set" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let profile = params["profile"].as_str().ok_or("missing params.profile")?;
            let result = broker
                .config_update(ConfigUpdateRequest {
                    project_root: project_root.to_string(),
                    update: serde_json::json!({
                        "memory_palace_profile": profile
                    }),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.add" => {
            let request = serde_json::from_value::<ToolAddRequest>(params)
                .map_err(|e| format!("invalid tool.add params: {e}"))?;
            let result = broker
                .tool_add(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.remove" => {
            let request = serde_json::from_value::<ToolRemoveRequest>(params)
                .map_err(|e| format!("invalid tool.remove params: {e}"))?;
            let result = broker
                .tool_remove(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "models.list" => {
            let filter = params.get("filter").and_then(|v| v.as_str());
            Ok(broker.models_list(filter))
        }
        "models.check" => Ok(broker.models_check_updates()),
        _ => Err(format!("unknown config action: {action}")),
    }
}

async fn dispatch_maintain(broker: &McpBroker, args: Value) -> Result<Value, String> {
    let action = args["action"].as_str().ok_or("missing action")?;
    let params = args
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    match action {
        "reindex" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let result = broker
                .docs_reindex(DocsReindexRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.enable" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let family = params["family"].as_str().ok_or("missing params.family")?;
            let result = broker
                .tool_enable(ToolEnableRequest {
                    project_root: project_root.to_string(),
                    family: family.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.disable" => {
            let request = serde_json::from_value::<ToolDisableRequest>(params)
                .map_err(|e| format!("invalid tool.disable params: {e}"))?;
            let result = broker
                .tool_disable(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "tool.refresh" => {
            let result = broker
                .tool_catalog_update()
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "trash.list" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let result = broker
                .docs_trash_list(DocsTrashListRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "trash.restore" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let path = params["path"].as_str().ok_or("missing params.path")?;
            let result = broker
                .docs_trash_restore(DocsTrashRestoreRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "trash.empty" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let result = broker
                .docs_trash_empty(DocsTrashEmptyRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "images.rescan" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            broker
                .image_rescan(Path::new(project_root), project_id)
                .await
                .map_err(|e| map_core_error_to_string(&e))
        }
        "images.clear" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            broker
                .image_clear(Path::new(project_root), project_id)
                .await
                .map_err(|e| map_core_error_to_string(&e))
        }
        "images.delete" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let path = params["path"].as_str().ok_or("missing params.path")?;
            broker
                .image_delete(Path::new(project_root), project_id, path)
                .await
                .map_err(|e| map_core_error_to_string(&e))
        }
        // v0.13.0: Graph RAG extraction + stats
        "graph.extract" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let result = broker
                .graph_extract(Path::new(project_root), project_id)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "graph.stats" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            Ok(broker
                .graph_stats(Path::new(project_root), project_id)
                .await)
        }
        // v0.12.0: backup / restore
        "backup" => {
            let request: crate::api::BackupRequest = serde_json::from_value(params)
                .map_err(|e| format!("invalid backup params: {e}"))?;
            let result = broker
                .backup_project(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "restore" => {
            let request: crate::api::RestoreRequest = serde_json::from_value(params)
                .map_err(|e| format!("invalid restore params: {e}"))?;
            let result = broker
                .restore_project(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "memory.capture_hook" => {
            let request: crate::api::MemoryCaptureHookRequest = serde_json::from_value(params)
                .map_err(|e| format!("invalid memory.capture_hook params: {e}"))?;
            let result = broker
                .memory_capture_hook(request)
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        _ => Err(format!("unknown maintain action: {action}")),
    }
}

async fn dispatch_observe(broker: &McpBroker, args: Value) -> Result<Value, String> {
    let action = args["action"].as_str().ok_or("missing action")?;
    let params = args
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    match action {
        "metrics" => {
            let result = broker.metrics_snapshot();
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        "events" => {
            let project_root = params["project_root"]
                .as_str()
                .ok_or("missing params.project_root")?;
            let project_id = params["project_id"]
                .as_str()
                .ok_or("missing params.project_id")?;
            let limit = params["limit"].as_u64().unwrap_or(50) as usize;
            let result = broker
                .audit_events(AuditEventsRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    limit,
                })
                .await
                .map_err(|e| map_core_error_to_string(&e))?;
            serde_json::to_value(result).map_err(serialize_error_to_string)
        }
        _ => Err(format!("unknown observe action: {action}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use the_one_core::config::{AppConfig, RuntimeOverrides};

    #[tokio::test]
    async fn test_dispatch_initialize() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "initialize".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "the-one-mcp");
        // v0.10.0 — initialize must advertise the resources capability
        assert!(
            result["capabilities"]["resources"].is_object(),
            "initialize should advertise resources capability"
        );
    }

    #[tokio::test]
    async fn test_dispatch_resources_list() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(10.into())),
            method: "resources/list".to_string(),
            params: Some(serde_json::json!({
                "project_root": root.display().to_string(),
                "project_id": "resources-test",
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none(),
            "resources/list dispatch errored: {:?}",
            response.error
        );
        let result = response.result.expect("result");
        let arr = result["resources"]
            .as_array()
            .expect("resources should be array");
        // Empty project still yields at least the project/profile and
        // catalog/enabled defaults.
        assert!(arr.len() >= 2);
    }

    #[tokio::test]
    async fn test_dispatch_resources_list_missing_params() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(11.into())),
            method: "resources/list".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_dispatch_resources_read_rejects_path_traversal() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(12.into())),
            method: "resources/read".to_string(),
            params: Some(serde_json::json!({
                "project_root": root.display().to_string(),
                "project_id": "t",
                "uri": "the-one://docs/../../etc/passwd",
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_some(),
            "path traversal should be rejected"
        );
    }

    #[tokio::test]
    async fn test_dispatch_tools_list() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(2.into())),
            method: "tools/list".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_none());
        let tools = response.result.unwrap()["tools"].as_array().unwrap().len();
        assert_eq!(tools, crate::transport::tools::tool_definitions().len());
    }

    #[tokio::test]
    async fn test_dispatch_unknown_method() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(3.into())),
            method: "nonexistent".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_dispatch_tools_call_missing_params() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(4.into())),
            method: "tools/call".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_dispatch_tools_call_missing_tool_name() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(5.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({})),
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_dispatch_tools_call_unknown_tool() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(6.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "nonexistent.tool",
                "arguments": {}
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn test_dispatch_notifications_initialized() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn test_dispatch_metrics_snapshot() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(7.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "observe",
                "arguments": {
                    "action": "metrics"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_dispatch_docs_get_full() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(10.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "docs.get",
                "arguments": {
                    "project_root": "/tmp/nonexistent",
                    "project_id": "test",
                    "path": "README.md"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        // Will fail at broker level (no project), but should not be "unknown tool"
        assert!(
            response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS
        );
    }

    #[tokio::test]
    async fn test_dispatch_docs_get_with_section() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(11.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "docs.get",
                "arguments": {
                    "project_root": "/tmp/nonexistent",
                    "project_id": "test",
                    "path": "README.md",
                    "section": "Installation"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS
        );
    }

    #[tokio::test]
    async fn test_dispatch_docs_save() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(12.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "docs.save",
                "arguments": {
                    "project_root": "/tmp/nonexistent",
                    "project_id": "test",
                    "path": "notes.md",
                    "content": "# Notes"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS
        );
    }

    #[tokio::test]
    async fn test_dispatch_config_profile_set() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir should exist");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(24.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "config",
                "arguments": {
                    "action": "profile.set",
                    "params": {
                        "project_root": root.display().to_string(),
                        "profile": "full"
                    }
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none(),
            "config profile.set dispatch errored: {:?}",
            response.error
        );

        let config =
            AppConfig::load(&root, RuntimeOverrides::default()).expect("config should load");
        assert!(config.memory_palace_enabled);
        assert!(config.memory_palace_hooks_enabled);
        assert!(config.memory_palace_aaak_enabled);
        assert!(config.memory_palace_diary_enabled);
        assert!(config.memory_palace_navigation_enabled);
    }

    #[tokio::test]
    async fn test_dispatch_config_profile_set_rejects_invalid_profile() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir should exist");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(25.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "config",
                "arguments": {
                    "action": "profile.set",
                    "params": {
                        "project_root": root.display().to_string(),
                        "profile": "bad-profile"
                    }
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_some(),
            "expected invalid profile to produce an error response"
        );
        let error = response.error.expect("error should exist");
        assert_eq!(error.code, INTERNAL_ERROR);
        assert!(
            error.message.contains("invalid memory palace profile"),
            "unexpected error message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_ingest_conversation() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir should exist");
        let transcript_path = root.join("dispatch-transcript.json");
        std::fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Dispatch ingest transcript for task 6."}
            ]"#,
        )
        .expect("transcript should be written");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(21.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.ingest_conversation",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "path": transcript_path.display().to_string(),
                    "format": "openai_messages",
                    "wing": "ops",
                    "room": "auth"
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none(),
            "memory.ingest_conversation dispatch errored: {:?}",
            response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_aaak_tools() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        std::fs::create_dir_all(&state_dir).expect("state dir should exist");
        std::fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_aaak_enabled":true}"#,
        )
        .expect("config should be written");

        let transcript_path = root.join("aaak-dispatch-transcript.json");
        std::fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."},
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."}
            ]"#,
        )
        .expect("transcript should be written");

        let compress_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(26.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.aaak.compress",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "path": transcript_path.display().to_string(),
                    "format": "openai_messages"
                }
            })),
        };
        let compress_response = dispatch(&broker, compress_request).await;
        assert!(
            compress_response.error.is_none(),
            "memory.aaak.compress dispatch errored: {:?}",
            compress_response.error
        );

        let teach_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(27.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.aaak.teach",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "path": transcript_path.display().to_string(),
                    "format": "openai_messages"
                }
            })),
        };
        let teach_response = dispatch(&broker, teach_request).await;
        assert!(
            teach_response.error.is_none(),
            "memory.aaak.teach dispatch errored: {:?}",
            teach_response.error
        );

        let list_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(28.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.aaak.list_lessons",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "limit": 10
                }
            })),
        };
        let list_response = dispatch(&broker, list_request).await;
        assert!(
            list_response.error.is_none(),
            "memory.aaak.list_lessons dispatch errored: {:?}",
            list_response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_diary_tools() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        std::fs::create_dir_all(&state_dir).expect("state dir should exist");
        std::fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config should be written");

        let add_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(34.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.diary.add",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "entry_date": "2026-04-10",
                    "mood": "focused",
                    "tags": ["release", "auth"],
                    "content": "Validated the release checklist and auth migration."
                }
            })),
        };
        let add_response = dispatch(&broker, add_request).await;
        assert!(
            add_response.error.is_none(),
            "memory.diary.add dispatch errored: {:?}",
            add_response.error
        );

        let list_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(35.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.diary.list",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "start_date": "2026-04-01",
                    "end_date": "2026-04-30",
                    "max_results": 10
                }
            })),
        };
        let list_response = dispatch(&broker, list_request).await;
        assert!(
            list_response.error.is_none(),
            "memory.diary.list dispatch errored: {:?}",
            list_response.error
        );

        let search_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(36.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.diary.search",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "query": "release",
                    "max_results": 10
                }
            })),
        };
        let search_response = dispatch(&broker, search_request).await;
        assert!(
            search_response.error.is_none(),
            "memory.diary.search dispatch errored: {:?}",
            search_response.error
        );

        let summarize_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(37.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.diary.summarize",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "start_date": "2026-04-01",
                    "end_date": "2026-04-30",
                    "max_summary_items": 6
                }
            })),
        };
        let summarize_response = dispatch(&broker, summarize_request).await;
        assert!(
            summarize_response.error.is_none(),
            "memory.diary.summarize dispatch errored: {:?}",
            summarize_response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_diary_add_rejects_non_string_tags() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        std::fs::create_dir_all(&state_dir).expect("state dir should exist");
        std::fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config should be written");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(38.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.diary.add",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "entry_date": "2026-04-10",
                    "tags": ["release", 7],
                    "content": "This should fail."
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some(), "expected invalid tags to error");
        let error = response.error.expect("error should exist");
        assert_eq!(error.code, INTERNAL_ERROR);
        assert!(
            error.message.contains("tags"),
            "unexpected error message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_navigation_tools() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        std::fs::create_dir_all(&state_dir).expect("state dir should exist");
        std::fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config should be written");

        let upsert_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(29.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.navigation.upsert_node",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "node_id": "drawer:ops",
                    "kind": "drawer",
                    "label": "Operations",
                    "wing": "ops"
                }
            })),
        };
        let upsert_response = dispatch(&broker, upsert_request).await;
        assert!(
            upsert_response.error.is_none(),
            "memory.navigation.upsert_node dispatch errored: {:?}",
            upsert_response.error
        );

        let second_upsert_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(30.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.navigation.upsert_node",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "node_id": "drawer:platform",
                    "kind": "drawer",
                    "label": "Platform",
                    "wing": "platform"
                }
            })),
        };
        let second_upsert_response = dispatch(&broker, second_upsert_request).await;
        assert!(
            second_upsert_response.error.is_none(),
            "second navigation upsert dispatch errored: {:?}",
            second_upsert_response.error
        );

        let link_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(31.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.navigation.link_tunnel",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "from_node_id": "drawer:platform",
                    "to_node_id": "drawer:ops"
                }
            })),
        };
        let link_response = dispatch(&broker, link_request).await;
        assert!(
            link_response.error.is_none(),
            "memory.navigation.link_tunnel dispatch errored: {:?}",
            link_response.error
        );

        let list_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(32.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.navigation.list",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "limit": 10
                }
            })),
        };
        let list_response = dispatch(&broker, list_request).await;
        assert!(
            list_response.error.is_none(),
            "memory.navigation.list dispatch errored: {:?}",
            list_response.error
        );

        let traverse_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(33.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.navigation.traverse",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "start_node_id": "drawer:ops",
                    "max_depth": 2
                }
            })),
        };
        let traverse_response = dispatch(&broker, traverse_request).await;
        assert!(
            traverse_response.error.is_none(),
            "memory.navigation.traverse dispatch errored: {:?}",
            traverse_response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_memory_wake_up() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir should exist");
        let transcript_path = root.join("wake-up-dispatch-transcript.json");
        std::fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Wake-up dispatch transcript for task 6."}
            ]"#,
        )
        .expect("transcript should be written");

        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "test-project".to_string(),
                path: transcript_path.display().to_string(),
                format: MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: None,
                room: Some("auth".to_string()),
            })
            .await
            .expect("conversation ingest should succeed");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(22.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "memory.wake_up",
                "arguments": {
                    "project_root": root.display().to_string(),
                    "project_id": "test-project",
                    "wing": "ops",
                    "max_items": 4
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none(),
            "memory.wake_up dispatch errored: {:?}",
            response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_maintain_memory_capture_hook() {
        let broker = McpBroker::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        std::fs::create_dir_all(&state_dir).expect("state dir should exist");
        std::fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_hooks_enabled":true}"#,
        )
        .expect("config should be written");

        let transcript_path = root.join("hook-dispatch-transcript.json");
        std::fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Dispatch maintain memory.capture_hook test."}
            ]"#,
        )
        .expect("transcript should be written");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(23.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "maintain",
                "arguments": {
                    "action": "memory.capture_hook",
                    "params": {
                        "project_root": root.display().to_string(),
                        "project_id": "test-project",
                        "path": transcript_path.display().to_string(),
                        "format": "openai_messages",
                        "event": "stop"
                    }
                }
            })),
        };

        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none(),
            "maintain memory.capture_hook dispatch errored: {:?}",
            response.error
        );
    }

    #[tokio::test]
    async fn test_dispatch_tool_find() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(13.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "tool.find",
                "arguments": {
                    "project_root": "/tmp/nonexistent",
                    "project_id": "test",
                    "mode": "search",
                    "query": "linter"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS
        );
    }

    #[tokio::test]
    async fn test_dispatch_setup_action() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(14.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "setup",
                "arguments": {
                    "action": "profile",
                    "params": {
                        "project_root": "/tmp/nonexistent",
                        "project_id": "test"
                    }
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(
            response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS
        );
    }

    #[tokio::test]
    async fn test_dispatch_observe_metrics_via_observe() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(15.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "observe",
                "arguments": {
                    "action": "metrics"
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_dispatch_unknown_action() {
        let broker = McpBroker::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(16.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "setup",
                "arguments": {
                    "action": "nonexistent",
                    "params": {}
                }
            })),
        };
        let response = dispatch(&broker, request).await;
        assert!(response.error.is_some());
    }
}
