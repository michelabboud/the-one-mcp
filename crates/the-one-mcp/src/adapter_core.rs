use std::path::Path;

use the_one_core::error::CoreError;

use crate::api::{
    AuditEventsRequest, AuditEventsResponse, ConfigExportResponse, ProjectInitRequest,
    ProjectInitResponse, ProjectRefreshRequest, ProjectRefreshResponse, ToolRunRequest,
    ToolRunResponse,
};
use crate::broker::McpBroker;

pub struct AdapterCore {
    broker: McpBroker,
}

impl AdapterCore {
    pub fn new(broker: McpBroker) -> Self {
        Self { broker }
    }

    pub async fn project_init(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectInitResponse, CoreError> {
        self.broker
            .project_init(ProjectInitRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
            })
            .await
    }

    pub async fn project_refresh(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectRefreshResponse, CoreError> {
        self.broker
            .project_refresh(ProjectRefreshRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
            })
            .await
    }

    pub async fn config_export(
        &self,
        project_root: &Path,
    ) -> Result<ConfigExportResponse, CoreError> {
        self.broker.config_export(project_root).await
    }

    pub async fn audit_events(
        &self,
        project_root: &Path,
        project_id: &str,
        limit: usize,
    ) -> Result<AuditEventsResponse, CoreError> {
        self.broker
            .audit_events(AuditEventsRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
                limit,
            })
            .await
    }

    pub async fn ingest_docs(
        &self,
        project_root: &Path,
        project_id: &str,
        docs_root: &Path,
    ) -> Result<usize, CoreError> {
        self.broker
            .ingest_docs(project_root, project_id, docs_root)
            .await
    }

    pub async fn tool_run(
        &self,
        project_root: &Path,
        project_id: &str,
        request: ToolRunRequest,
    ) -> Result<ToolRunResponse, CoreError> {
        self.broker
            .tool_run(project_root, project_id, request)
            .await
    }
}
