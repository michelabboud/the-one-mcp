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
        tool_def("project.init", "Initialize a project for the-one-mcp tracking. Creates the .the-one state directory and stores the initial project profile.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("project.refresh", "Refresh the project profile, recomputing if the fingerprint has changed.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("project.profile.get", "Retrieve the latest stored project profile JSON.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
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
        tool_def("docs.get", "Retrieve the full content of a documentation file.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path to the document" }
            },
            "required": ["project_root", "project_id", "path"]
        })),
        tool_def("docs.get_section", "Extract a specific section from a document by heading.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path to the document" },
                "heading": { "type": "string", "description": "Section heading to extract" },
                "max_bytes": { "type": "integer", "description": "Maximum bytes to return (default 24576)", "default": 24576 }
            },
            "required": ["project_root", "project_id", "path", "heading"]
        })),
        tool_def("docs.create", "Create a new managed documentation file.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path for the new document" },
                "content": { "type": "string", "description": "Markdown content to write" }
            },
            "required": ["project_root", "project_id", "path", "content"]
        })),
        tool_def("docs.update", "Update the content of an existing managed document.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path to the document" },
                "content": { "type": "string", "description": "New markdown content" }
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
        tool_def("docs.trash.list", "List documents in the trash.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("docs.trash.restore", "Restore a document from the trash.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Relative path of the trashed document to restore" }
            },
            "required": ["project_root", "project_id", "path"]
        })),
        tool_def("docs.trash.empty", "Permanently delete all documents in the trash.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("docs.reindex", "Re-ingest all managed and external documents into the memory index.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("tool.list", "List tools by state: enabled, available, recommended, or all", json!({
            "type": "object",
            "properties": {
                "state": { "type": "string", "description": "Filter by tool state", "enum": ["enabled", "available", "recommended", "all"] },
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("tool.add", "Add a custom tool to your local catalog", json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Unique tool identifier" },
                "name": { "type": "string", "description": "Human-readable tool name" },
                "description": { "type": "string", "description": "Tool description" },
                "install_command": { "type": "string", "description": "Shell command to install the tool" },
                "run_command": { "type": "string", "description": "Shell command to run the tool" },
                "tool_type": { "type": "string", "description": "Type of tool (default: cli)", "default": "cli" },
                "category": { "type": "array", "items": { "type": "string" }, "description": "Tool categories" },
                "languages": { "type": "array", "items": { "type": "string" }, "description": "Supported languages" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Searchable tags" },
                "github": { "type": "string", "description": "GitHub repository URL" },
                "risk_level": { "type": "string", "description": "Risk level: low, medium, high" },
                "cli": { "type": "string", "description": "CLI binary name" }
            },
            "required": ["id", "name", "description", "install_command", "run_command"]
        })),
        tool_def("tool.remove", "Remove a user-added custom tool from the local catalog (cannot remove curated catalog tools)", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to remove" }
            },
            "required": ["tool_id"]
        })),
        tool_def("tool.disable", "Disable a tool for the current CLI and project", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to disable" },
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["tool_id", "project_root", "project_id"]
        })),
        tool_def("tool.install", "Install a tool by running its install command, then auto-enable it", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to install" },
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" }
            },
            "required": ["tool_id", "project_root", "project_id"]
        })),
        tool_def("tool.info", "Get full metadata for a specific tool including install state and version", json!({
            "type": "object",
            "properties": {
                "tool_id": { "type": "string", "description": "Tool ID to query" }
            },
            "required": ["tool_id"]
        })),
        tool_def("tool.update", "Refresh the tool catalog from source and re-scan system inventory", json!({
            "type": "object",
            "properties": {},
            "required": []
        })),
        tool_def("tool.suggest", "Get smart tool recommendations filtered by project profile — returns tools grouped as enabled, available (installed but not enabled), and recommended (not installed)", json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural-language description of desired capability" },
                "max": { "type": "integer", "description": "Maximum number of suggestions (default 5)", "default": 5 }
            },
            "required": ["query"]
        })),
        tool_def("tool.search", "Search the capability registry for tools matching a query.", json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "max": { "type": "integer", "description": "Maximum number of results (default 5)", "default": 5 }
            },
            "required": ["query"]
        })),
        tool_def("tool.enable", "Enable a tool family for a project by adding it to the overrides manifest.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "family": { "type": "string", "description": "Tool family name to enable" }
            },
            "required": ["project_root", "family"]
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
        tool_def("config.export", "Export the current project configuration.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" }
            },
            "required": ["project_root"]
        })),
        tool_def("config.update", "Update project configuration fields.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "update": { "type": "object", "description": "JSON object with configuration fields to update" }
            },
            "required": ["project_root", "update"]
        })),
        tool_def("metrics.snapshot", "Return current in-memory metrics counters for the broker.", json!({
            "type": "object",
            "properties": {},
            "required": []
        })),
        tool_def("audit.events", "List recent audit events for a project.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "limit": { "type": "integer", "description": "Maximum number of events to return (default 50)", "default": 50 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("models.list", "List all available embedding models (local and API) with metadata including dimensions, size, latency, and multilingual support.", json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "Optional filter: 'local', 'api', 'multilingual', 'installer'. Defaults to all." }
            },
            "required": []
        })),
        tool_def("models.check_updates", "Check for new embedding model versions from upstream registries.", json!({
            "type": "object",
            "properties": {},
            "required": []
        })),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_count() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 33); // 24 original + 7 new (tool.list, tool.add, tool.remove, tool.disable, tool.install, tool.info, tool.update) + 2 models (models.list, models.check_updates)
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
