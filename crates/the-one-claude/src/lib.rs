use std::path::Path;

use the_one_mcp::adapter_core::AdapterCore;
use the_one_mcp::api::{
    AuditEventsResponse, ConfigExportResponse, ProjectInitResponse, ProjectRefreshResponse,
    ToolRunRequest, ToolRunResponse,
};
use the_one_mcp::broker::McpBroker;

pub fn adapter_name() -> &'static str {
    "claude"
}

pub struct ClaudeAdapter {
    core: AdapterCore,
}

impl ClaudeAdapter {
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

    use the_one_codex::CodexAdapter;
    use the_one_mcp::api::ToolRunRequest;
    use the_one_mcp::broker::McpBroker;

    use super::ClaudeAdapter;

    #[tokio::test]
    async fn test_claude_adapter_project_init() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let adapter = ClaudeAdapter::new(McpBroker::new());
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

    #[tokio::test]
    async fn test_claude_codex_parity_for_core_flow() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let claude = ClaudeAdapter::new(McpBroker::new());
        let codex = CodexAdapter::new(McpBroker::new());

        let claude_init = claude
            .project_init(&project_root, "project-1")
            .await
            .expect("claude init should work");
        let codex_init = codex
            .project_init(&project_root, "project-1")
            .await
            .expect("codex init should work");
        assert_eq!(claude_init.project_id, codex_init.project_id);

        let claude_refresh = claude
            .project_refresh(&project_root, "project-1")
            .await
            .expect("claude refresh should work");
        let codex_refresh = codex
            .project_refresh(&project_root, "project-1")
            .await
            .expect("codex refresh should work");
        assert_eq!(claude_refresh.mode, codex_refresh.mode);

        let claude_config = claude
            .config_export(&project_root)
            .await
            .expect("claude config should work");
        let codex_config = codex
            .config_export(&project_root)
            .await
            .expect("codex config should work");
        assert_eq!(claude_config.provider, codex_config.provider);
        assert_eq!(claude_config.nano_provider, codex_config.nano_provider);

        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("guide.md"), "# Intro\nhello").expect("doc write should succeed");
        let claude_ingested = claude
            .ingest_docs(&project_root, "project-1", &docs)
            .await
            .expect("claude ingest should work");
        let codex_ingested = codex
            .ingest_docs(&project_root, "project-1", &docs)
            .await
            .expect("codex ingest should work");
        assert_eq!(claude_ingested, codex_ingested);

        let claude_headless = claude
            .tool_run(
                &project_root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .await
            .expect("claude tool run should work");
        let codex_headless = codex
            .tool_run(
                &project_root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .await
            .expect("codex tool run should work");
        assert_eq!(claude_headless.allowed, codex_headless.allowed);
    }
}
