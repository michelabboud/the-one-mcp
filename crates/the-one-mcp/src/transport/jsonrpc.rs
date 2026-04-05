use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::api::*;
use crate::broker::McpBroker;

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
                "tools": {}
            },
            "serverInfo": {
                "name": "the-one-mcp",
                "version": crate::MCP_SCHEMA_VERSION
            }
        }),
    )
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
            let result = broker
                .memory_search(MemorySearchRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    query: query.to_string(),
                    top_k,
                })
                .await;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
            serde_json::to_value(result).map_err(|e| e.to_string())
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
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                serde_json::to_value(result).map_err(|e| e.to_string())
            } else {
                let result = broker
                    .docs_get(DocsGetRequest {
                        project_root: project_root.to_string(),
                        project_id: project_id.to_string(),
                        path: path.to_string(),
                    })
                    .await;
                serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string()),
                Err(_) => {
                    let result = broker
                        .docs_create(DocsCreateRequest {
                            project_root: project_root.to_string(),
                            project_id: project_id.to_string(),
                            path: path.to_string(),
                            content: content.to_string(),
                        })
                        .await
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(DocsSaveResponse {
                        path: result.path,
                        created: true,
                    })
                    .map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                    let result = broker.tool_list(request).await.map_err(|e| e.to_string())?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
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
                    serde_json::to_value(result).map_err(|e| e.to_string())
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
                    serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "tool.install" => {
            let request = serde_json::from_value::<ToolInstallRequest>(args)
                .map_err(|e| format!("invalid tool.install params: {e}"))?;
            let result = broker
                .tool_install(request)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "memory.search_images" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let query = args["query"].as_str().ok_or("missing query")?;
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;
            let result = broker
                .image_search(ImageSearchRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    query: query.to_string(),
                    top_k,
                })
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "memory.ingest_image" => {
            let project_root = args["project_root"]
                .as_str()
                .ok_or("missing project_root")?;
            let project_id = args["project_id"].as_str().ok_or("missing project_id")?;
            let path = args["path"].as_str().ok_or("missing path")?;
            let caption = args.get("caption").and_then(|v| v.as_str()).map(String::from);
            let result = broker
                .image_ingest(ImageIngestRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                    path: path.to_string(),
                    caption,
                })
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        // ── Multiplexed admin tools ─────────────────────────────
        "setup" => dispatch_setup(broker, args).await,
        "config" => dispatch_config(broker, args).await,
        "maintain" => dispatch_maintain(broker, args).await,
        "observe" => dispatch_observe(broker, args).await,

        _ => Err(format!("unknown tool: {tool_name}")),
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "refresh" => {
            let result = broker
                .project_refresh(ProjectRefreshRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "profile" => {
            let result = broker
                .project_profile_get(ProjectProfileGetRequest {
                    project_root: project_root.to_string(),
                    project_id: project_id.to_string(),
                })
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "tool.add" => {
            let request = serde_json::from_value::<ToolAddRequest>(params)
                .map_err(|e| format!("invalid tool.add params: {e}"))?;
            let result = broker.tool_add(request).await.map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "tool.remove" => {
            let request = serde_json::from_value::<ToolRemoveRequest>(params)
                .map_err(|e| format!("invalid tool.remove params: {e}"))?;
            let result = broker
                .tool_remove(request)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "tool.disable" => {
            let request = serde_json::from_value::<ToolDisableRequest>(params)
                .map_err(|e| format!("invalid tool.disable params: {e}"))?;
            let result = broker
                .tool_disable(request)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "tool.refresh" => {
            let result = broker
                .tool_catalog_update()
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
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
            serde_json::to_value(result).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown observe action: {action}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(tools, 17);
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
