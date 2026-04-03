use std::path::Path;

use the_one_mcp::adapter_core::AdapterCore;
use the_one_mcp::api::{
    AuditEventsResponse, ConfigExportResponse, ProjectInitResponse, ProjectRefreshResponse,
    ToolRunRequest, ToolRunResponse,
};
use the_one_mcp::broker::McpBroker;

pub fn adapter_name() -> &'static str {
    "codex"
}

pub struct CodexAdapter {
    core: AdapterCore,
}

impl CodexAdapter {
    pub fn new(broker: McpBroker) -> Self {
        Self {
            core: AdapterCore::new(broker),
        }
    }

    pub async fn project_init(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectInitResponse, the_one_core::error::CoreError> {
        self.core.project_init(project_root, project_id).await
    }

    pub async fn project_refresh(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectRefreshResponse, the_one_core::error::CoreError> {
        self.core.project_refresh(project_root, project_id).await
    }

    pub async fn config_export(
        &self,
        project_root: &Path,
    ) -> Result<ConfigExportResponse, the_one_core::error::CoreError> {
        self.core.config_export(project_root).await
    }

    pub async fn audit_events(
        &self,
        project_root: &Path,
        project_id: &str,
        limit: usize,
    ) -> Result<AuditEventsResponse, the_one_core::error::CoreError> {
        self.core.audit_events(project_root, project_id, limit).await
    }

    pub async fn ingest_docs(
        &self,
        project_root: &Path,
        project_id: &str,
        docs_root: &Path,
    ) -> Result<usize, the_one_core::error::CoreError> {
        self.core.ingest_docs(project_root, project_id, docs_root).await
    }

    pub async fn tool_run(
        &self,
        project_root: &Path,
        project_id: &str,
        request: ToolRunRequest,
    ) -> Result<ToolRunResponse, the_one_core::error::CoreError> {
        self.core.tool_run(project_root, project_id, request).await
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use the_one_mcp::broker::McpBroker;

    use super::CodexAdapter;

    #[tokio::test]
    async fn test_codex_adapter_project_init() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let adapter = CodexAdapter::new(McpBroker::new());
        let response = adapter
            .project_init(&project_root, "project-1")
            .await
            .expect("init should succeed");
        assert_eq!(response.project_id, "project-1");

        let refresh = adapter
            .project_refresh(&project_root, "project-1")
            .await
            .expect("refresh should succeed");
        assert_eq!(refresh.project_id, "project-1");

        let config = adapter
            .config_export(&project_root)
            .await
            .expect("config export should succeed");
        assert_eq!(config.provider, "local");

        let events = adapter
            .audit_events(&project_root, "project-1", 10)
            .await
            .expect("audit events should load");
        assert!(events.events.is_empty());
    }
}
