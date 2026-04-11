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
        // ── Work tools ───────────────────────────────────────────────
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
        tool_def("memory.aaak.compress", "Compress a transcript into the AAAK dialect with deterministic motif references and lossless payload output.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Absolute or project-relative path to the transcript export" },
                "format": { "type": "string", "description": "Transcript format", "enum": ["openai_messages", "claude_transcript", "generic_jsonl"] }
            },
            "required": ["project_root", "project_id", "path", "format"]
        })),
        tool_def("memory.aaak.teach", "Extract reusable AAAK patterns from a transcript and persist them as project lessons.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "path": { "type": "string", "description": "Absolute or project-relative path to the transcript export" },
                "format": { "type": "string", "description": "Transcript format", "enum": ["openai_messages", "claude_transcript", "generic_jsonl"] }
            },
            "required": ["project_root", "project_id", "path", "format"]
        })),
        tool_def("memory.aaak.list_lessons", "List persisted AAAK lessons for the current project.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "limit": { "type": "integer", "description": "Maximum number of lessons to return (default 20)", "default": 20 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("memory.diary.add", "Create or refresh a structured diary memory entry with date, mood, tags, and freeform content.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "entry_date": { "type": "string", "description": "Diary date in YYYY-MM-DD format" },
                "mood": { "type": "string", "description": "Optional mood label for the entry" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags for later search and filtering" },
                "content": { "type": "string", "description": "Diary entry body text" }
            },
            "required": ["project_root", "project_id", "entry_date", "content"]
        })),
        tool_def("memory.diary.list", "List diary entries for the current project, optionally filtered by an inclusive date range.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "start_date": { "type": "string", "description": "Optional inclusive lower bound in YYYY-MM-DD format" },
                "end_date": { "type": "string", "description": "Optional inclusive upper bound in YYYY-MM-DD format" },
                "max_results": { "type": "integer", "description": "Maximum number of diary entries to return (default 20)", "default": 20 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("memory.diary.search", "Search diary entries by natural-language terms across content, mood, and tags.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "query": { "type": "string", "description": "Search query for diary content, mood, or tags" },
                "start_date": { "type": "string", "description": "Optional inclusive lower bound in YYYY-MM-DD format" },
                "end_date": { "type": "string", "description": "Optional inclusive upper bound in YYYY-MM-DD format" },
                "max_results": { "type": "integer", "description": "Maximum number of results to return (default 20)", "default": 20 }
            },
            "required": ["project_root", "project_id", "query"]
        })),
        tool_def("memory.diary.summarize", "Build a compact summary of recent diary entries using deterministic fact extraction.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "start_date": { "type": "string", "description": "Optional inclusive lower bound in YYYY-MM-DD format" },
                "end_date": { "type": "string", "description": "Optional inclusive upper bound in YYYY-MM-DD format" },
                "max_summary_items": { "type": "integer", "description": "Maximum summary highlight count (default 12)", "default": 12 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("memory.navigation.upsert_node", "Create or update a MemPalace navigation node such as a drawer, closet, or room.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "node_id": { "type": "string", "description": "Stable navigation node identifier" },
                "kind": { "type": "string", "description": "Navigation node type", "enum": ["drawer", "closet", "room"] },
                "label": { "type": "string", "description": "Human-readable node label" },
                "parent_node_id": { "type": "string", "description": "Optional parent node identifier" },
                "wing": { "type": "string", "description": "Optional wing metadata for compatibility with existing palace filters" },
                "hall": { "type": "string", "description": "Optional hall metadata for compatibility with existing palace filters" },
                "room": { "type": "string", "description": "Optional room metadata for compatibility with existing palace filters" }
            },
            "required": ["project_root", "project_id", "node_id", "kind", "label"]
        })),
        tool_def("memory.navigation.link_tunnel", "Create or refresh a tunnel link between two existing navigation nodes.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "from_node_id": { "type": "string", "description": "One end of the tunnel" },
                "to_node_id": { "type": "string", "description": "The other end of the tunnel" }
            },
            "required": ["project_root", "project_id", "from_node_id", "to_node_id"]
        })),
        tool_def("memory.navigation.list", "List navigation nodes and nearby tunnel metadata for the current project.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "parent_node_id": { "type": "string", "description": "Optional parent node filter" },
                "kind": { "type": "string", "description": "Optional node kind filter", "enum": ["drawer", "closet", "room"] },
                "limit": { "type": "integer", "description": "Maximum number of nodes to return (default 100)", "default": 100 }
            },
            "required": ["project_root", "project_id"]
        })),
        tool_def("memory.navigation.traverse", "Traverse the MemPalace navigation graph from a starting node using deterministic path ordering.", json!({
            "type": "object",
            "properties": {
                "project_root": { "type": "string", "description": "Absolute path to the project root" },
                "project_id": { "type": "string", "description": "Unique project identifier" },
                "start_node_id": { "type": "string", "description": "Starting node identifier" },
                "max_depth": { "type": "integer", "description": "Maximum traversal depth (default 8)", "default": 8 }
            },
            "required": ["project_root", "project_id", "start_node_id"]
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
        tool_def("config", "Configuration, custom tools, embedding models, and MemPalace profiles. Actions: 'export', 'update', 'profile.set', 'tool.add', 'tool.remove', 'models.list', 'models.check'.", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Operation to perform", "enum": ["export", "update", "profile.set", "tool.add", "tool.remove", "models.list", "models.check"] },
                "params": {
                    "type": "object",
                    "description": "Action-specific parameters. 'export': {project_root}. 'update': {project_root, update}. 'profile.set': {project_root, profile} where profile is one of off, core, full (aliases mempalace_off, mempalace_core, mempalace_full also accepted). 'tool.add': {id, name, description, install_command, run_command, ...}. 'tool.remove': {tool_id}. 'models.list': {filter?}. 'models.check': {}."
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
        assert!(names.contains(&"memory.aaak.compress".to_string()));
        assert!(names.contains(&"memory.aaak.teach".to_string()));
        assert!(names.contains(&"memory.aaak.list_lessons".to_string()));
        assert!(names.contains(&"memory.diary.add".to_string()));
        assert!(names.contains(&"memory.diary.list".to_string()));
        assert!(names.contains(&"memory.diary.search".to_string()));
        assert!(names.contains(&"memory.diary.summarize".to_string()));
        assert!(names.contains(&"memory.navigation.upsert_node".to_string()));
        assert!(names.contains(&"memory.navigation.link_tunnel".to_string()));
        assert!(names.contains(&"memory.navigation.list".to_string()));
        assert!(names.contains(&"memory.navigation.traverse".to_string()));
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
    fn navigation_tool_definitions_expose_graph_tools() {
        let tools = tool_definitions();
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(names.contains(&"memory.navigation.upsert_node"));
        assert!(names.contains(&"memory.navigation.link_tunnel"));
        assert!(names.contains(&"memory.navigation.list"));
        assert!(names.contains(&"memory.navigation.traverse"));
    }

    #[test]
    fn diary_tool_definitions_expose_diary_tools() {
        let tools = tool_definitions();
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(names.contains(&"memory.diary.add"));
        assert!(names.contains(&"memory.diary.list"));
        assert!(names.contains(&"memory.diary.search"));
        assert!(names.contains(&"memory.diary.summarize"));
    }

    #[test]
    fn test_tool_definitions_count() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 30); // 26 work + 4 admin (setup, config, maintain, observe)
    }

    #[test]
    fn test_tool_definitions_all_have_required_fields() {
        for tool in tool_definitions() {
            assert!(tool["name"].is_string(), "tool missing name");
            assert!(tool["description"].is_string(), "tool missing description");
            assert!(tool["inputSchema"].is_object(), "tool missing inputSchema");
        }
    }

    /// M5 (mempalace comparative audit): no tool description may contain
    /// imperative directives aimed at the AI client (e.g. "always call X
    /// first", "on wake-up do Y"). Mempalace's `PALACE_PROTOCOL` baked such
    /// directives into response data, which is a form of self-prompt-
    /// injection — it tightly couples storage semantics to LLM behaviour
    /// and breaks when the agent's own system prompt conflicts.
    ///
    /// This test is a hygiene gate: new tool descriptions must stay
    /// descriptive ("semantic search over indexed documentation") rather
    /// than imperative ("you MUST call this before responding").
    #[test]
    fn test_tool_descriptions_are_descriptive_not_imperative() {
        // Case-insensitive substrings that should never appear in a tool
        // description. We tolerate mention of these words in inputSchema
        // (where they describe parameter semantics) — only the description
        // field is checked.
        const FORBIDDEN: &[&str] = &[
            "you must",
            "always call",
            "never call",
            "on wake-up",
            "before responding",
            "do not guess",
            "this protocol ensures",
        ];

        for tool in tool_definitions() {
            let name = tool["name"].as_str().unwrap_or("<anonymous>");
            let description = tool["description"].as_str().unwrap_or("").to_lowercase();
            for needle in FORBIDDEN {
                assert!(
                    !description.contains(needle),
                    "tool '{name}' description contains imperative directive '{needle}' — \
                     descriptions must stay descriptive, not instruct the AI. \
                     See M5 in docs/reviews/2026-04-10-mempalace-comparative-audit.md."
                );
            }
        }
    }
}
