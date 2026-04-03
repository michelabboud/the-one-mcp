use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInitRequest {
    pub project_root: String,
    pub project_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateRequest {
    pub project_root: String,
    pub update: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateResponse {
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigExportRequest, ConfigExportResponse, DocsListRequest, MemoryFetchChunkRequest,
        MemorySearchRequest, ProjectInitRequest, ToolRunRequest,
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
}
