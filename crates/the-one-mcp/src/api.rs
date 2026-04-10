use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInitRequest {
    pub project_root: String,
    pub project_id: String,
}

// ---------------------------------------------------------------------------
// Backup / restore (v0.12.0, Task 3.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupRequest {
    pub project_root: String,
    pub project_id: String,
    pub output_path: String,
    #[serde(default = "default_true")]
    pub include_images: bool,
    #[serde(default)]
    pub include_qdrant_local: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupResponse {
    pub output_path: String,
    pub size_bytes: u64,
    pub file_count: usize,
    pub manifest_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreRequest {
    pub backup_path: String,
    pub target_project_root: String,
    pub target_project_id: String,
    #[serde(default)]
    pub overwrite_existing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreResponse {
    pub restored_files: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInitResponse {
    pub project_id: String,
    pub profile_version: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRefreshRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRefreshResponse {
    pub project_id: String,
    pub mode: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchRequest {
    pub project_root: String,
    pub project_id: String,
    pub query: String,
    pub top_k: usize,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub hall: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySearchItem {
    pub id: String,
    pub source_path: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySearchResponse {
    pub hits: Vec<MemorySearchItem>,
    pub route: String,
    pub rationale: String,
    pub provider_path: String,
    pub confidence_percent: u8,
    pub fallback_used: bool,
    pub timeout_ms_bound: u64,
    pub retries_bound: u8,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryConversationFormat {
    #[serde(rename = "openai_messages")]
    OpenAiMessages,
    #[serde(rename = "claude_transcript")]
    ClaudeTranscript,
    #[serde(rename = "generic_jsonl")]
    GenericJsonl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIngestConversationRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub format: MemoryConversationFormat,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIngestConversationResponse {
    pub ingested_chunks: usize,
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWakeUpRequest {
    pub project_root: String,
    pub project_id: String,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
    pub max_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWakeUpResponse {
    pub summary: String,
    pub facts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryHookEvent {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "precompact")]
    PreCompact,
}

impl MemoryHookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::PreCompact => "precompact",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryCaptureHookRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub format: MemoryConversationFormat,
    pub event: MemoryHookEvent,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub hall: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryCaptureHookResponse {
    pub event: String,
    pub ingested_chunks: usize,
    pub source_path: String,
    pub wing: String,
    pub hall: String,
    pub room: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFetchChunkRequest {
    pub project_root: String,
    pub project_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFetchChunkResponse {
    pub id: String,
    pub source_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsListResponse {
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsGetResponse {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsGetSectionRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub heading: String,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsListRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsGetRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsGetSectionResponse {
    pub path: String,
    pub heading: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSuggestRequest {
    pub query: String,
    pub max: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSuggestItem {
    pub id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSuggestResponse {
    pub suggestions: Vec<ToolSuggestItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSearchResponse {
    pub matches: Vec<ToolSuggestItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSearchRequest {
    pub query: String,
    pub max: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRunRequest {
    pub action_key: String,
    pub interactive: bool,
    pub approval_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEnableRequest {
    pub project_root: String,
    pub family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEnableResponse {
    pub enabled_families: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRunResponse {
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigExportResponse {
    pub schema_version: String,
    pub provider: String,
    pub log_level: String,
    pub qdrant_url: String,
    pub qdrant_auth_configured: bool,
    pub qdrant_ca_cert_path: Option<String>,
    pub qdrant_tls_insecure: bool,
    pub qdrant_strict_auth: bool,
    pub nano_provider: String,
    pub nano_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigExportRequest {
    pub project_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfileGetRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfileGetResponse {
    pub project_id: String,
    pub profile_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsSnapshotResponse {
    pub project_init_calls: u64,
    pub project_refresh_calls: u64,
    pub memory_search_calls: u64,
    pub tool_run_calls: u64,
    pub router_fallback_calls: u64,
    pub router_decision_latency_ms_total: u64,
    pub router_provider_error_calls: u64,
    // v0.12.0: observability deep dive — new fields.
    //
    // These are `serde(default)` so older clients deserializing a newer
    // snapshot still get a zero value if they were generated from a binary
    // that did not track the field yet.
    #[serde(default)]
    pub memory_search_latency_ms_total: u64,
    #[serde(default)]
    pub memory_search_latency_avg_ms: u64,
    #[serde(default)]
    pub image_search_calls: u64,
    #[serde(default)]
    pub image_ingest_calls: u64,
    #[serde(default)]
    pub resources_list_calls: u64,
    #[serde(default)]
    pub resources_read_calls: u64,
    #[serde(default)]
    pub watcher_events_processed: u64,
    #[serde(default)]
    pub watcher_events_failed: u64,
    #[serde(default)]
    pub qdrant_errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventItem {
    pub id: i64,
    pub project_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub created_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventsRequest {
    pub project_root: String,
    pub project_id: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventsResponse {
    pub events: Vec<AuditEventItem>,
}

// ---------------------------------------------------------------------------
// Docs CRUD types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsCreateRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsCreateResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsUpdateRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsUpdateResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsDeleteRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsDeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsMoveRequest {
    pub project_root: String,
    pub project_id: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsMoveResponse {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashListRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashListResponse {
    pub entries: Vec<the_one_core::docs_manager::DocEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashRestoreRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashRestoreResponse {
    pub restored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashEmptyRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsTrashEmptyResponse {
    pub emptied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsReindexRequest {
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsReindexResponse {
    pub new: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateRequest {
    pub project_root: String,
    pub update: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateResponse {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Tool lifecycle types
// ---------------------------------------------------------------------------

fn default_tool_type() -> String {
    "cli".to_string()
}

// tool.add
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAddRequest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_tool_type")]
    pub tool_type: String,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub description: String,
    pub install_command: String,
    pub run_command: String,
    #[serde(default)]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub github: Option<String>,
    #[serde(default)]
    pub cli: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAddResponse {
    pub added: bool,
    pub id: String,
}

// tool.remove
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRemoveRequest {
    pub tool_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRemoveResponse {
    pub removed: bool,
}

// tool.disable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDisableRequest {
    pub tool_id: String,
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDisableResponse {
    pub disabled: bool,
}

// tool.install
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstallRequest {
    pub tool_id: String,
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstallResponse {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub auto_enabled: bool,
    pub output: String,
}

// tool.info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfoRequest {
    pub tool_id: String,
}
// Response: ToolFullInfo from the_one_core::tool_catalog (already serializable)

// tool.update (catalog refresh)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateResponse {
    pub catalog_version_before: Option<String>,
    pub catalog_version_after: Option<String>,
    pub tools_added: usize,
    pub tools_updated: usize,
    pub system_tools_found: u64,
}

// tool.list (with state filter)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListRequest {
    #[serde(default)]
    pub state: Option<String>,
    pub project_root: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListResponse {
    pub tools: Vec<the_one_core::tool_catalog::ToolSummary>,
}

// ---------------------------------------------------------------------------
// Image search / ingest types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchRequest {
    pub project_root: String,
    pub project_id: String,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub image_base64: Option<String>,
    pub top_k: usize,
}

/// Note: cannot derive `Eq` because `score` is `f32`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchHit {
    pub id: String,
    pub source_path: String,
    pub thumbnail_path: Option<String>,
    pub caption: Option<String>,
    pub ocr_text: Option<String>,
    pub score: f32,
}

/// Note: cannot derive `Eq` because `hits` contains `f32` scores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchResponse {
    pub hits: Vec<ImageSearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageIngestRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageIngestResponse {
    pub path: String,
    pub dims: usize,
    pub ocr_extracted: bool,
    pub thumbnail_generated: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigExportRequest, ConfigExportResponse, DocsListRequest, DocsSaveRequest,
        ImageIngestRequest, ImageSearchRequest, MemoryConversationFormat, MemoryFetchChunkRequest,
        MemoryIngestConversationRequest, MemorySearchRequest, ProjectInitRequest, ToolFindRequest,
        ToolRunRequest,
    };

    #[test]
    fn test_api_request_roundtrip_serialization() {
        let init = ProjectInitRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "project-1".to_string(),
        };
        let json = serde_json::to_string(&init).expect("serialize should succeed");
        let decoded: ProjectInitRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, init);

        let search = MemorySearchRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "project-1".to_string(),
            query: "search docs".to_string(),
            top_k: 5,
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: Some("auth".to_string()),
        };
        let json = serde_json::to_string(&search).expect("serialize should succeed");
        let decoded: MemorySearchRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, search);

        let docs = DocsListRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "project-1".to_string(),
        };
        let json = serde_json::to_string(&docs).expect("serialize should succeed");
        let decoded: DocsListRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, docs);

        let fetch = MemoryFetchChunkRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "project-1".to_string(),
            id: "chunk-1".to_string(),
        };
        let json = serde_json::to_string(&fetch).expect("serialize should succeed");
        let decoded: MemoryFetchChunkRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, fetch);
    }

    #[test]
    fn memory_ingest_conversation_request_roundtrip() {
        let req = MemoryIngestConversationRequest {
            project_root: "/tmp/project".to_string(),
            project_id: "proj-1".to_string(),
            path: "exports/auth.json".to_string(),
            format: MemoryConversationFormat::OpenAiMessages,
            wing: Some("proj-auth".to_string()),
            hall: Some("hall_facts".to_string()),
            room: Some("auth-migration".to_string()),
        };
        let json = serde_json::to_string(&req).expect("serialize should succeed");
        let decoded: MemoryIngestConversationRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, req);
        assert!(json.contains(r#""format":"openai_messages""#));
    }

    #[test]
    fn test_api_response_roundtrip_serialization() {
        let config = ConfigExportResponse {
            schema_version: "v1beta".to_string(),
            provider: "local".to_string(),
            log_level: "info".to_string(),
            qdrant_url: "http://127.0.0.1:6334".to_string(),
            qdrant_auth_configured: false,
            qdrant_ca_cert_path: None,
            qdrant_tls_insecure: false,
            qdrant_strict_auth: true,
            nano_provider: "rules".to_string(),
            nano_model: "none".to_string(),
        };
        let json = serde_json::to_string(&config).expect("serialize should succeed");
        let decoded: ConfigExportResponse =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, config);

        let tool = ToolRunRequest {
            action_key: "tool.run:danger".to_string(),
            interactive: false,
            approval_scope: None,
        };
        let json = serde_json::to_string(&tool).expect("serialize should succeed");
        let decoded: ToolRunRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, tool);

        let config_request = ConfigExportRequest {
            project_root: "/tmp/repo".to_string(),
        };
        let json = serde_json::to_string(&config_request).expect("serialize should succeed");
        let decoded: ConfigExportRequest =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(decoded, config_request);
    }

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
        let no_tags: DocsSaveRequest = serde_json::from_str(
            r#"{"project_root":"/tmp","project_id":"p","path":"a.md","content":"x"}"#,
        )
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

    #[test]
    fn test_image_search_request_with_base64() {
        let req = ImageSearchRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "p1".to_string(),
            query: None,
            image_base64: Some("aGVsbG8=".to_string()),
            top_k: 5,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let decoded: ImageSearchRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.image_base64.as_deref(), Some("aGVsbG8="));
        assert_eq!(decoded.query, None);
    }

    #[test]
    fn test_image_ingest_request_roundtrip() {
        let req = ImageIngestRequest {
            project_root: "/tmp/repo".to_string(),
            project_id: "p1".to_string(),
            path: "/tmp/repo/screenshot.png".to_string(),
            caption: Some("App screenshot".to_string()),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let decoded: ImageIngestRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, req);

        // Without caption
        let no_caption: ImageIngestRequest = serde_json::from_str(
            r#"{"project_root":"/tmp","project_id":"p","path":"/img.png","caption":null}"#,
        )
        .expect("deserialize without caption");
        assert_eq!(no_caption.caption, None);
    }
}
