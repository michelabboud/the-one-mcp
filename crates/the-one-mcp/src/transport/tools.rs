use serde_json::{json, Value};

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        // ── Work tools (11) ─────────────────────────────────────────
        tool_def("memory.search", "Semantic search over indexed project documentation chunks.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "query": { "type": "string", "description": "Natural-language search query" },
                "top_k": { "type": "integer", "description": "Maximum number of results to return (default 5)", "default": 5 },
                "wing": { "type": "string", "description": "Optional palace wing filter for conversation memory" },
                "hall": { "type": "string", "description": "Optional palace hall filter for conversation memory" },
                "room": { "type": "string", "description": "Optional palace room filter for conversation memory" }
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
        tool_def("memory.search_images", "Semantic search over indexed project images. Supply either 'query' (natural-language text) or 'image_base64' (base64-encoded PNG/JPEG/WebP) to find similar images. Exactly one must be provided.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "query": { "type": "string", "description": "Text query (mutually exclusive with image_base64)" },
                "image_base64": { "type": "string", "description": "Base64-encoded image bytes (mutually exclusive with query)" },
                "top_k": { "type": "integer", "description": "Maximum number of results (default 5)", "default": 5 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("memory.ingest_image", "Manually index an image file. Extracts OCR text (if enabled) and generates a thumbnail.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Absolute or project-relative path to the image" },
                "caption": { "type": "string", "description": "Optional user-provided caption" }
            },
            "required": ["project_root", "project_id", "path"]
        })),
        tool_def("memory.ingest_conversation", "Import a conversation export and index it as verbatim memory with optional palace metadata.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Absolute or project-relative path to the transcript export" },
                "format": { "type": "string", "description": "Transcript format", "enum": ["openai_messages", "claude_transcript", "generic_jsonl"] },
                "wing": { "type": "string", "description": "Optional palace wing" },
                "hall": { "type": "string", "description": "Optional palace hall" },
                "room": { "type": "string", "description": "Optional palace room" }
            },
            "required": ["project_root", "project_id", "path", "format"]
        })),
        tool_def("memory.wake_up", "Build a compact context pack from recent high-signal conversation memory.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "wing": { "type": "string", "description": "Optional palace wing filter" },
                "hall": { "type": "string", "description": "Optional palace hall filter" },
                "room": { "type": "string", "description": "Optional palace room filter" },
                "max_items": { "type": "integer", "description": "Maximum items to include (default 12)", "default": 12 }
            },
            "required": ["project_root", "project_id"]
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
        tool_def("maintain", "Housekeeping: re-indexing, tool enable/disable, catalog refresh, trash management, image management, and hook-based memory capture. Actions: 'reindex', 'tool.enable', 'tool.disable', 'tool.refresh', 'trash.list', 'trash.restore', 'trash.empty', 'images.rescan', 'images.clear', 'images.delete', 'memory.capture_hook'.", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["reindex", "tool.enable", "tool.disable", "tool.refresh", "trash.list", "trash.restore", "trash.empty", "images.rescan", "images.clear", "images.delete", "memory.capture_hook"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters. Most actions need {project_root, project_id}. 'tool.enable': {project_root, family}. 'tool.disable': {tool_id, project_root, project_id}. 'trash.restore': {project_root, project_id, path}. 'images.delete': {project_root, project_id, path}. 'memory.capture_hook': {project_root, project_id, path, format, event, wing?, hall?, room?} where event is 'stop' or 'precompact'."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_include_conversation_memory_tools() {
        let tools = tool_definitions();
        let names: Vec<String> = tools
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();

        assert!(names.contains(&"memory.ingest_conversation".to_string()));
        assert!(names.contains(&"memory.wake_up".to_string()));
    }

    #[test]
    fn memory_search_tool_definition_exposes_palace_filters() {
        let memory_search = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "memory.search")
            .expect("memory.search tool should exist");
        let properties = &memory_search["inputSchema"]["properties"];

        assert!(properties["wing"].is_object());
        assert!(properties["hall"].is_object());
        assert!(properties["room"].is_object());
    }

    #[test]
    fn test_tool_definitions_count() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 19); // 15 work + 4 admin (setup, config, maintain, observe)
    }

    #[test]
    fn test_tool_definitions_all_have_required_fields() {
        for tool in tool_definitions() {
            assert!(tool["name"].is_string(), "tool missing name");
            assert!(tool["description"].is_string(), "tool missing description");
            assert!(tool["inputSchema"].is_object(), "tool missing inputSchema");
        }
    }
}
