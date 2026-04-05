# MCP Tool Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce MCP tool surface from 33 to 15 tools (~52% token savings) by merging overlapping work tools and multiplexing admin tools.

**Architecture:** The transport layer (`tools.rs`, `jsonrpc.rs`) changes tool definitions and dispatch. Broker methods stay intact — new dispatch code calls existing handlers internally. New API types route multiplexed actions to existing typed requests.

**Tech Stack:** Rust, serde_json, tokio, JSON Schema draft 2020-12

---

### Task 1: Add new API types in `api.rs`

Add the request types for merged and multiplexed tools. Existing request/response types remain — they're used internally by the new dispatch.

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs:227-341` (after Docs CRUD types)

- [ ] **Step 1: Write test for new API types**

Add to the existing `mod tests` block in `api.rs`:

```rust
#[test]
fn test_docs_save_request_roundtrip() {
    let save = DocsSaveRequest {
        project_root: "/tmp/repo".to_string(),
        project_id: "p1".to_string(),
        path: "guide.md".to_string(),
        content: "# Guide".to_string(),
        tags: Some(vec!["howto".to_string()]),
    };
    let json = serde_json::to_string(&save).expect("serialize");
    let decoded: DocsSaveRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.project_root, "/tmp/repo");
    assert_eq!(decoded.tags, Some(vec!["howto".to_string()]));

    // tags omitted
    let no_tags: DocsSaveRequest =
        serde_json::from_str(r#"{"project_root":"/tmp","project_id":"p","path":"a.md","content":"x"}"#)
            .expect("deserialize without tags");
    assert_eq!(no_tags.tags, None);
}

#[test]
fn test_tool_find_request_roundtrip() {
    let find = ToolFindRequest {
        project_root: "/tmp/repo".to_string(),
        project_id: "p1".to_string(),
        mode: "search".to_string(),
        filter: None,
        query: Some("linter".to_string()),
        cli: None,
        max: None,
    };
    let json = serde_json::to_string(&find).expect("serialize");
    let decoded: ToolFindRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.mode, "search");
    assert_eq!(decoded.query, Some("linter".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p the-one-mcp test_docs_save_request_roundtrip -- --no-capture`
Expected: FAIL — `DocsSaveRequest` not found

- [ ] **Step 3: Add the new types**

Add these types right after the `DocsReindexResponse` struct (line ~329) in `api.rs`:

```rust
// ---------------------------------------------------------------------------
// Merged tool types
// ---------------------------------------------------------------------------

/// docs.save — upsert: create if missing, update if exists
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsSaveRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsSaveResponse {
    pub path: String,
    pub created: bool,
}

/// tool.find — unified discovery (list / suggest / search)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFindRequest {
    pub project_root: String,
    pub project_id: String,
    pub mode: String,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub cli: Option<String>,
    #[serde(default)]
    pub max: Option<usize>,
}
```

- [ ] **Step 4: Update test imports**

Update the test imports at the top of the `mod tests` block to include the new types:

```rust
use super::{
    ConfigExportRequest, ConfigExportResponse, DocsSaveRequest, DocsListRequest,
    MemoryFetchChunkRequest, MemorySearchRequest, ProjectInitRequest, ToolFindRequest,
    ToolRunRequest,
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p the-one-mcp test_docs_save_request_roundtrip test_tool_find_request_roundtrip -- --no-capture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-mcp/src/api.rs
git commit -m "feat: add DocsSaveRequest and ToolFindRequest API types for tool consolidation"
```

---

### Task 2: Rewrite tool definitions in `tools.rs`

Replace all 33 tool definitions with the new 15. This is a full rewrite of the `tool_definitions()` function.

**Files:**
- Modify: `crates/the-one-mcp/src/transport/tools.rs` (full file)

- [ ] **Step 1: Update the tool count test expectation**

Change the test at the bottom of `tools.rs`:

```rust
#[test]
fn test_tool_definitions_count() {
    let tools = tool_definitions();
    assert_eq!(tools.len(), 15); // 11 work + 4 admin (setup, config, maintain, observe)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p the-one-mcp test_tool_definitions_count -- --no-capture`
Expected: FAIL — `assert_eq!(33, 15)`

- [ ] **Step 3: Rewrite `tool_definitions()` — work tools (11)**

Replace the entire `tool_definitions()` body with:

```rust
pub fn tool_definitions() -> Vec<Value> {
    vec![
        // ── Work tools (11) ─────────────────────────────────────────
        tool_def("memory.search", "Semantic search over indexed project documentation chunks.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "query": { "type": "string", "description": "Natural-language search query" },
                "top_k": { "type": "integer", "description": "Maximum number of results to return (default 5)", "default": 5 }
            },
            "required": ["project_root", "project_id", "query"]
        })),
        tool_def("memory.fetch_chunk", "Fetch the full content of a specific memory chunk by ID.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "id": { "type": "string", "description": "Chunk ID returned from memory.search" }
            },
            "required": ["project_root", "project_id", "id"]
        })),
        tool_def("docs.list", "List all indexed documentation paths for a project.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("docs.get", "Retrieve a document or a specific section. Omit 'section' for the full document.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path to the document" },
                "section": { "type": "string", "description": "Heading text to extract (omit for full document)" },
                "max_bytes": { "type": "integer", "description": "Maximum bytes for section extraction (default 24576)", "default": 24576 }
            },
            "required": ["project_root", "project_id", "path"]
        })),
        tool_def("docs.save", "Create or update a managed document (upsert). Creates if path doesn't exist, updates if it does.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path for the document" },
                "content": { "type": "string", "description": "Markdown content to write" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags (replaces existing on update)" }
            },
            "required": ["project_root", "project_id", "path", "content"]
        })),
        tool_def("docs.delete", "Soft-delete a managed document (moves to trash).", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path to the document to delete" }
            },
            "required": ["project_root", "project_id", "path"]
        })),
        tool_def("docs.move", "Rename or move a managed document.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "from": { "type": "string", "description": "Current relative path" },
                "to": { "type": "string", "description": "New relative path" }
            },
            "required": ["project_root", "project_id", "from", "to"]
        })),
        tool_def("tool.find", "Unified tool discovery. Modes: 'list' (by state filter), 'suggest' (smart recommendations), 'search' (query-based).", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "mode": { "type": "string", "description": "Discovery mode", "enum": ["list", "suggest", "search"] },
                "filter": { "type": "string", "description": "For 'list' mode: filter by state", "enum": ["enabled", "available", "recommended", "all"] },
                "query": { "type": "string", "description": "For 'suggest'/'search' modes: natural-language query" },
                "cli": { "type": "string", "description": "CLI client name (optional)" },
                "max": { "type": "integer", "description": "Maximum results (default 5)", "default": 5 }
            },
            "required": ["project_root", "project_id", "mode"]
        })),
        tool_def("tool.info", "Get full metadata for a specific tool including install state and version.", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to query" }
            },
            "required": ["tool_id"]
        })),
        tool_def("tool.install", "Install a tool by running its install command, then auto-enable it.", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to install" },
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["tool_id", "project_root", "project_id"]
        })),
        tool_def("tool.run", "Request approval and run a tool action, respecting policy and approval scopes.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "action_key": { "type": "string", "description": "Action key identifying the tool action to run" },
                "interactive": { "type": "boolean", "description": "Whether the user can be prompted for approval", "default": false },
                "approval_scope": { "type": "string", "description": "Scope of approval: once, session, or forever", "enum": ["once", "session", "forever"], "default": "once" }
            },
            "required": ["project_root", "project_id", "action_key"]
        })),

        // ── Admin tools (4 multiplexed) ─────────────────────────────
        tool_def("setup", "Project initialization and profile management. Actions: 'project' (init), 'refresh' (re-scan), 'profile' (get profile).", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["project", "refresh", "profile"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters",
                    "properties": {
                        "project_root": { "type": "string", "description": "Absolute path to the project root" },
                        "project_id": { "type": "string", "description": "Unique project identifier" }
                    },
                    "required": ["project_root", "project_id"]
                }
            },
            "required": ["action", "params"]
        })),
        tool_def("config", "Configuration, custom tools, and embedding models. Actions: 'export', 'update', 'tool.add', 'tool.remove', 'models.list', 'models.check'.", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["export", "update", "tool.add", "tool.remove", "models.list", "models.check"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters. 'export': {project_root}. 'update': {project_root, update}. 'tool.add': {id, name, description, install_command, run_command, ...}. 'tool.remove': {tool_id}. 'models.list': {filter?}. 'models.check': {}."
                }
            },
            "required": ["action"]
        })),
        tool_def("maintain", "Housekeeping: re-indexing, tool enable/disable, catalog refresh, trash management. Actions: 'reindex', 'tool.enable', 'tool.disable', 'tool.refresh', 'trash.list', 'trash.restore', 'trash.empty'.", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["reindex", "tool.enable", "tool.disable", "tool.refresh", "trash.list", "trash.restore", "trash.empty"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters. Most actions need {project_root, project_id}. 'tool.enable': {project_root, family}. 'tool.disable': {tool_id, project_root, project_id}. 'trash.restore': {project_root, project_id, path}."
                }
            },
            "required": ["action"]
        })),
        tool_def("observe", "Metrics and audit log access. Actions: 'metrics' (broker counters), 'events' (audit log).", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["metrics", "events"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters. 'metrics': {}. 'events': {project_root, project_id, limit?}."
                }
            },
            "required": ["action"]
        })),
    ]
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p the-one-mcp test_tool_definitions_count test_tool_definitions_all_have_required_fields -- --no-capture`
Expected: PASS (15 tools, all have name/description/inputSchema)

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-mcp/src/transport/tools.rs
git commit -m "feat: consolidate 33 tool definitions down to 15 (11 work + 4 admin)"
```

---

### Task 3: Rewrite dispatch logic in `jsonrpc.rs`

Replace the `dispatch_tool()` match arms to handle the new tool names. Merged tools route internally to existing broker methods. Multiplexed tools extract `action` and `params`, then dispatch to existing broker methods.

**Files:**
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs:145-563` (the `dispatch_tool` function)

- [ ] **Step 1: Write tests for merged tool dispatch**

Add to the `mod tests` block in `jsonrpc.rs`:

```rust
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
    assert!(response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS);
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
    assert!(response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS);
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
    assert!(response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS);
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
    assert!(response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS);
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
    assert!(response.error.is_none() || response.error.as_ref().unwrap().code != INVALID_PARAMS);
}

#[tokio::test]
async fn test_dispatch_observe_metrics() {
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p the-one-mcp test_dispatch_docs_get_full test_dispatch_docs_save test_dispatch_tool_find test_dispatch_setup_action test_dispatch_observe_metrics test_dispatch_unknown_action -- --no-capture`
Expected: FAIL — unknown tool errors

- [ ] **Step 3: Rewrite `dispatch_tool()` function**

Replace the entire `dispatch_tool` function (lines 145-563) with:

```rust
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
            // If section is provided, delegate to get_section; otherwise full doc
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
            // Upsert: try update first, if it fails with not-found, create
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
                    let state = args.get("filter").and_then(|v| v.as_str()).map(String::from);
                    let request = ToolListRequest {
                        state,
                        project_root: project_root.to_string(),
                        project_id: project_id.to_string(),
                    };
                    let result = broker.tool_list(request).await.map_err(|e| e.to_string())?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
                }
                "suggest" => {
                    let query = args["query"].as_str().ok_or("missing query for suggest mode")?;
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
                    let query = args["query"].as_str().ok_or("missing query for search mode")?;
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
    let params = args.get("params").cloned().unwrap_or(Value::Object(Default::default()));
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
    let params = args.get("params").cloned().unwrap_or(Value::Object(Default::default()));
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
    let params = args.get("params").cloned().unwrap_or(Value::Object(Default::default()));
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
            let family = params["family"]
                .as_str()
                .ok_or("missing params.family")?;
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
            let path = params["path"]
                .as_str()
                .ok_or("missing params.path")?;
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
    let params = args.get("params").cloned().unwrap_or(Value::Object(Default::default()));
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
```

- [ ] **Step 4: Update existing `test_dispatch_tools_list` test**

Change the tools count assertion:

```rust
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
    assert_eq!(tools, 15);
}
```

- [ ] **Step 5: Update existing `test_dispatch_metrics_snapshot` test**

The metrics tool is now `observe` with action `metrics`:

```rust
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
```

- [ ] **Step 6: Run all jsonrpc tests**

Run: `cargo test -p the-one-mcp -- jsonrpc --no-capture`
Expected: All PASS

- [ ] **Step 7: Commit**

```bash
git add crates/the-one-mcp/src/transport/jsonrpc.rs
git commit -m "feat: rewrite dispatch_tool for 15-tool surface with multiplexed admin"
```

---

### Task 4: Update schema files

Remove old schema files, add new ones for merged/multiplexed tools, update the test expectations in `lib.rs`.

**Files:**
- Modify: `schemas/mcp/v1beta/` (add/remove JSON files)
- Modify: `crates/the-one-mcp/src/lib.rs:24-88` (expected schema list)

- [ ] **Step 1: Remove obsolete schema files**

```bash
cd /home/michel/projects/the-one-mcp/schemas/mcp/v1beta

# Merged into docs.get (section param)
rm docs.get_section.request.schema.json docs.get_section.response.schema.json

# Merged into docs.save
rm docs.create.request.schema.json docs.create.response.schema.json
rm docs.update.request.schema.json docs.update.response.schema.json

# Merged into tool.find
rm tool.list.request.schema.json tool.list.response.schema.json
rm tool.suggest.request.schema.json tool.suggest.response.schema.json
rm tool.search.request.schema.json tool.search.response.schema.json

# Moved into setup
rm project.init.request.schema.json project.init.response.schema.json
rm project.refresh.request.schema.json project.refresh.response.schema.json
rm project.profile.get.request.schema.json project.profile.get.response.schema.json

# Moved into config
rm config.export.request.schema.json config.export.response.schema.json
rm config.update.request.schema.json config.update.response.schema.json
rm tool.add.request.schema.json tool.add.response.schema.json
rm tool.remove.request.schema.json tool.remove.response.schema.json

# Moved into maintain
rm docs.reindex.request.schema.json docs.reindex.response.schema.json
rm tool.enable.request.schema.json tool.enable.response.schema.json
rm tool.disable.request.schema.json tool.disable.response.schema.json
rm tool.update.request.schema.json tool.update.response.schema.json
rm docs.trash.list.request.schema.json docs.trash.list.response.schema.json
rm docs.trash.restore.request.schema.json docs.trash.restore.response.schema.json
rm docs.trash.empty.request.schema.json docs.trash.empty.response.schema.json

# Moved into observe
rm metrics.snapshot.request.schema.json metrics.snapshot.response.schema.json
rm audit.events.request.schema.json audit.events.response.schema.json

# Moved into config (models)
# Note: check if models schemas exist first — they may not
rm -f models.list.request.schema.json models.list.response.schema.json
rm -f models.check_updates.request.schema.json models.check_updates.response.schema.json
```

- [ ] **Step 2: Create new schema files for merged tools**

Create `docs.save.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.docs.save.request",
  "type": "object",
  "properties": {
    "project_root": { "type": "string" },
    "project_id": { "type": "string" },
    "path": { "type": "string" },
    "content": { "type": "string" },
    "tags": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["project_root", "project_id", "path", "content"]
}
```

Create `docs.save.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.docs.save.response",
  "type": "object",
  "properties": {
    "path": { "type": "string" },
    "created": { "type": "boolean" }
  },
  "required": ["path", "created"]
}
```

Create `tool.find.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.tool.find.request",
  "type": "object",
  "properties": {
    "project_root": { "type": "string" },
    "project_id": { "type": "string" },
    "mode": { "type": "string", "enum": ["list", "suggest", "search"] },
    "filter": { "type": "string", "enum": ["enabled", "available", "recommended", "all"] },
    "query": { "type": "string" },
    "cli": { "type": "string" },
    "max": { "type": "integer" }
  },
  "required": ["project_root", "project_id", "mode"]
}
```

Create `tool.find.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.tool.find.response",
  "type": "object",
  "description": "Response varies by mode: list returns {tools}, suggest returns {suggestions}, search returns {matches}"
}
```

Create `setup.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.setup.request",
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["project", "refresh", "profile"] },
    "params": { "type": "object" }
  },
  "required": ["action", "params"]
}
```

Create `setup.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.setup.response",
  "type": "object",
  "description": "Response varies by action"
}
```

Create `config.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.config.request",
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["export", "update", "tool.add", "tool.remove", "models.list", "models.check"] },
    "params": { "type": "object" }
  },
  "required": ["action"]
}
```

Create `config.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.config.response",
  "type": "object",
  "description": "Response varies by action"
}
```

Create `maintain.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.maintain.request",
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["reindex", "tool.enable", "tool.disable", "tool.refresh", "trash.list", "trash.restore", "trash.empty"] },
    "params": { "type": "object" }
  },
  "required": ["action"]
}
```

Create `maintain.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.maintain.response",
  "type": "object",
  "description": "Response varies by action"
}
```

Create `observe.request.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.observe.request",
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["metrics", "events"] },
    "params": { "type": "object" }
  },
  "required": ["action"]
}
```

Create `observe.response.schema.json`:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.observe.response",
  "type": "object",
  "description": "Response varies by action"
}
```

- [ ] **Step 3: Update the docs.get schema to include optional section**

Update `docs.get.request.schema.json` to add `section` and `max_bytes` properties:
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "the-one.mcp.v1beta.docs.get.request",
  "type": "object",
  "properties": {
    "project_root": { "type": "string" },
    "project_id": { "type": "string" },
    "path": { "type": "string" },
    "section": { "type": "string" },
    "max_bytes": { "type": "integer" }
  },
  "required": ["project_root", "project_id", "path"]
}
```

- [ ] **Step 4: Update `lib.rs` expected schema list**

Replace the `expected` array in `test_v1beta_schema_files_exist_and_are_valid_json` (lines 24-88 of `lib.rs`):

```rust
let expected = [
    // Work tools
    "docs.delete.request.schema.json",
    "docs.delete.response.schema.json",
    "docs.get.request.schema.json",
    "docs.get.response.schema.json",
    "docs.list.request.schema.json",
    "docs.list.response.schema.json",
    "docs.move.request.schema.json",
    "docs.move.response.schema.json",
    "docs.save.request.schema.json",
    "docs.save.response.schema.json",
    "memory.fetch_chunk.request.schema.json",
    "memory.fetch_chunk.response.schema.json",
    "memory.search.request.schema.json",
    "memory.search.response.schema.json",
    "tool.find.request.schema.json",
    "tool.find.response.schema.json",
    "tool.info.request.schema.json",
    "tool.info.response.schema.json",
    "tool.install.request.schema.json",
    "tool.install.response.schema.json",
    "tool.run.request.schema.json",
    "tool.run.response.schema.json",
    // Admin tools (multiplexed)
    "setup.request.schema.json",
    "setup.response.schema.json",
    "config.request.schema.json",
    "config.response.schema.json",
    "maintain.request.schema.json",
    "maintain.response.schema.json",
    "observe.request.schema.json",
    "observe.response.schema.json",
    // OpenAPI
    "openapi.swagger.json",
]
.into_iter()
.map(str::to_string)
.collect::<HashSet<_>>();
```

- [ ] **Step 5: Run schema validation tests**

Run: `cargo test -p the-one-mcp test_v1beta -- --no-capture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add schemas/mcp/v1beta/ crates/the-one-mcp/src/lib.rs
git commit -m "feat: update JSON schemas for 15-tool surface"
```

---

### Task 5: Update CLAUDE.md and run full validation

Update documentation references and run the full test suite.

**Files:**
- Modify: `CLAUDE.md` (tool count references)

- [ ] **Step 1: Update CLAUDE.md tool count**

Find and replace `33 MCP tools` with `15 MCP tools` and `33 tools` with `15 tools` in the relevant lines. Update the last line that mentions tool counts:

Change:
```
- 33 MCP tools (see `crates/the-one-mcp/src/transport/tools.rs`), 174 tests, 63 schemas, 28 catalog entries
```
To:
```
- 15 MCP tools (see `crates/the-one-mcp/src/transport/tools.rs`), 174+ tests, 31 schemas, 28 catalog entries
```

- [ ] **Step 2: Run `cargo fmt --check`**

Run: `cargo fmt --check`
Expected: No formatting issues (or run `cargo fmt` to fix)

- [ ] **Step 3: Run `cargo clippy`**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 4: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for 15-tool MCP surface"
```

---

### Task 6: Verify token reduction

Quick sanity check that the consolidation achieved the target savings.

- [ ] **Step 1: Count tool definitions**

Run: `cargo test -p the-one-mcp test_tool_definitions_count -- --no-capture`
Expected: `assert_eq!(tools.len(), 15)` PASS

- [ ] **Step 2: Build release binary**

Run: `cargo build --release -p the-one-mcp --bin the-one-mcp`
Expected: Build succeeds

- [ ] **Step 3: Verify tools/list output**

Run: `echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -p the-one-mcp --bin the-one-mcp -- serve 2>/dev/null | head -1 | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'Tools: {len(d[\"result\"][\"tools\"])}'); [print(f'  {t[\"name\"]}') for t in d['result']['tools']]"`
Expected: 15 tools listed
