use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use the_one_core::backup::{backup_project_state, restore_project_state, BackupResult};
use the_one_core::config::{update_project_config, ProjectConfigUpdate};
use the_one_mcp::api::{
    AuditEventsRequest, AuditEventsResponse, ConfigExportResponse, MetricsSnapshotResponse,
    ProjectInitRequest, ProjectInitResponse, ProjectProfileGetRequest, ProjectProfileGetResponse,
    ProjectRefreshRequest, ProjectRefreshResponse, ToolEnableRequest, ToolEnableResponse,
    ToolRunRequest,
};
use the_one_mcp::broker::McpBroker;
use the_one_mcp::swagger::embedded_swagger_json;

pub fn ui_module_name() -> &'static str {
    "the-one-ui"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiRuntimeConfig {
    pub project_root: PathBuf,
    pub project_id: String,
    pub bind_addr: String,
}

#[derive(Debug, Default, Deserialize)]
struct UiFileConfig {
    project_root: Option<String>,
    project_id: Option<String>,
    ui_bind: Option<String>,
}

pub fn resolve_ui_runtime_config(
    cwd: &Path,
) -> Result<UiRuntimeConfig, the_one_core::error::CoreError> {
    let mut merged = UiFileConfig {
        project_root: None,
        project_id: Some("default".to_string()),
        ui_bind: Some("127.0.0.1:8787".to_string()),
    };

    let global_path = ui_global_config_path()?;
    apply_ui_config_layer(&global_path, &mut merged)?;

    if let Ok(project_root_env) = std::env::var("THE_ONE_PROJECT_ROOT") {
        merged.project_root = Some(project_root_env);
    }

    let mut project_root = merged
        .project_root
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());
    if !project_root.is_absolute() {
        project_root = cwd.join(project_root);
    }

    let project_config_path = project_root.join(".the-one").join("config.json");
    apply_ui_config_layer(&project_config_path, &mut merged)?;

    if let Ok(project_root_env) = std::env::var("THE_ONE_PROJECT_ROOT") {
        merged.project_root = Some(project_root_env);
    }
    if let Ok(project_id_env) = std::env::var("THE_ONE_PROJECT_ID") {
        merged.project_id = Some(project_id_env);
    }
    if let Ok(ui_bind_env) = std::env::var("THE_ONE_UI_BIND") {
        merged.ui_bind = Some(ui_bind_env);
    }

    let mut resolved_project_root = merged
        .project_root
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());
    if !resolved_project_root.is_absolute() {
        resolved_project_root = cwd.join(resolved_project_root);
    }

    Ok(UiRuntimeConfig {
        project_root: resolved_project_root,
        project_id: merged.project_id.unwrap_or_else(|| "default".to_string()),
        bind_addr: merged
            .ui_bind
            .unwrap_or_else(|| "127.0.0.1:8787".to_string()),
    })
}

fn ui_global_config_path() -> Result<PathBuf, the_one_core::error::CoreError> {
    if let Ok(path) = std::env::var("THE_ONE_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(the_one_core::error::CoreError::InvalidProjectConfig(
                "THE_ONE_HOME must be absolute".to_string(),
            ));
        }
        return Ok(path.join("config.json"));
    }

    let home = std::env::var("HOME").map_err(|_| {
        the_one_core::error::CoreError::InvalidProjectConfig(
            "HOME is not set and THE_ONE_HOME not provided".to_string(),
        )
    })?;
    Ok(PathBuf::from(home).join(".the-one").join("config.json"))
}

fn apply_ui_config_layer(
    file_path: &Path,
    merged: &mut UiFileConfig,
) -> Result<(), the_one_core::error::CoreError> {
    if !file_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(file_path)?;
    let layer: UiFileConfig = serde_json::from_str(&content)?;
    if layer.project_root.is_some() {
        merged.project_root = layer.project_root;
    }
    if layer.project_id.is_some() {
        merged.project_id = layer.project_id;
    }
    if layer.ui_bind.is_some() {
        merged.ui_bind = layer.ui_bind;
    }
    Ok(())
}

pub struct AdminUi {
    broker: McpBroker,
}

#[derive(Clone)]
struct EmbeddedUiState {
    admin: Arc<AdminUi>,
    project_root: PathBuf,
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct ConfigUpdatePayload {
    provider: Option<String>,
    nano_provider: Option<String>,
    nano_model: Option<String>,
    qdrant_url: Option<String>,
    qdrant_api_key: Option<String>,
    qdrant_ca_cert_path: Option<String>,
    qdrant_tls_insecure: Option<bool>,
    qdrant_strict_auth: Option<bool>,
}

pub struct EmbeddedUiRuntime {
    pub listen_addr: std::net::SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl EmbeddedUiRuntime {
    pub fn shutdown(self) {
        let _ = self.shutdown.send(());
    }
}

pub struct AdminHealthReport {
    pub config: ConfigExportResponse,
    pub metrics: MetricsSnapshotResponse,
    pub recent_audit_events: usize,
}

pub fn render_dashboard_html(report: &AdminHealthReport) -> String {
    let provider = html_escape(&report.config.provider);
    let nano_provider = html_escape(&report.config.nano_provider);
    let nano_model = html_escape(&report.config.nano_model);
    let qdrant_url = html_escape(&report.config.qdrant_url);

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>The-One Admin</title><style>{}</style></head><body><header><h1>The-One Admin Dashboard</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/swagger\">Swagger</a></nav></header><main><section class=\"grid\"><article class=\"card\"><h2>Config</h2><p><strong>Provider</strong>: {}</p><p><strong>Nano Provider</strong>: {}</p><p><strong>Nano Model</strong>: {}</p><p><strong>Qdrant URL</strong>: {}</p><p><strong>Qdrant Auth</strong>: {}</p></article><article class=\"card\"><h2>Metrics</h2><p>project_init_calls: {}</p><p>project_refresh_calls: {}</p><p>memory_search_calls: {}</p><p>tool_run_calls: {}</p><p>router_fallback_calls: {}</p><p>router_provider_error_calls: {}</p></article><article class=\"card\"><h2>Audit</h2><p>recent_audit_events: {}</p><p><a href=\"/audit\">Open audit explorer</a></p></article></section></main></body></html>",
        base_styles(),
        provider,
        nano_provider,
        nano_model,
        qdrant_url,
        report.config.qdrant_auth_configured,
        report.metrics.project_init_calls,
        report.metrics.project_refresh_calls,
        report.metrics.memory_search_calls,
        report.metrics.tool_run_calls,
        report.metrics.router_fallback_calls,
        report.metrics.router_provider_error_calls,
        report.recent_audit_events,
    )
}

fn base_styles() -> &'static str {
    "body{font-family:ui-sans-serif,system-ui,-apple-system,Segoe UI,sans-serif;background:#f4f7fb;color:#0f172a;margin:0}header{background:linear-gradient(120deg,#0b3d91,#14532d);color:#fff;padding:20px}h1{margin:0 0 10px 0}nav a{color:#fff;text-decoration:none;margin-right:16px;font-weight:600}main{padding:20px}h2{margin-top:0}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:16px}.card{background:#fff;border:1px solid #dbe3ef;border-radius:12px;padding:16px;box-shadow:0 4px 10px rgba(2,6,23,.06)}table{width:100%;border-collapse:collapse;background:#fff}th,td{border:1px solid #dbe3ef;padding:8px;text-align:left}th{background:#e8eef8}code{white-space:pre-wrap;word-break:break-word}.form-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px}.field{display:flex;flex-direction:column;gap:6px}.field input,.field select{padding:8px;border:1px solid #cbd5e1;border-radius:8px}.actions{margin-top:12px;display:flex;gap:8px}.btn{border:0;border-radius:8px;padding:10px 14px;font-weight:700;cursor:pointer}.btn.primary{background:#14532d;color:#fff}.muted{color:#475569;font-size:.95rem}"
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub async fn start_embedded_ui_runtime(
    admin: Arc<AdminUi>,
    project_root: PathBuf,
    project_id: String,
    bind_addr: std::net::SocketAddr,
) -> Result<EmbeddedUiRuntime, the_one_core::error::CoreError> {
    let state = EmbeddedUiState {
        admin,
        project_root,
        project_id,
    };

    let app = Router::new()
        .route("/dashboard", get(dashboard_handler))
        .route("/audit", get(audit_page_handler))
        .route("/config", get(config_page_handler))
        .route("/swagger", get(swagger_ui_page_handler))
        .route("/api/health", get(health_handler))
        .route("/api/swagger", get(swagger_handler))
        .route("/api/config", post(config_update_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(the_one_core::error::CoreError::Io)?;
    let listen_addr = listener
        .local_addr()
        .map_err(the_one_core::error::CoreError::Io)?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Ok(EmbeddedUiRuntime {
        listen_addr,
        shutdown: shutdown_tx,
    })
}

async fn dashboard_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    match state
        .admin
        .health_report(&state.project_root, &state.project_id)
        .await
    {
        Ok(report) => Html(render_dashboard_html(&report)).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard error: {err}"),
        )
            .into_response(),
    }
}

async fn health_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    match state
        .admin
        .health_report(&state.project_root, &state.project_id)
        .await
    {
        Ok(report) => Json(serde_json::json!({
            "schema_version": report.config.schema_version,
            "provider": report.config.provider,
            "nano_provider": report.config.nano_provider,
            "metrics": report.metrics,
            "recent_audit_events": report.recent_audit_events,
        }))
        .into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn swagger_handler() -> impl IntoResponse {
    if let Some(swagger) = embedded_swagger_json() {
        return (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            swagger.to_string(),
        )
            .into_response();
    }

    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error":"embedded swagger is disabled"})),
    )
        .into_response()
}

async fn swagger_ui_page_handler() -> impl IntoResponse {
    if embedded_swagger_json().is_none() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            "embedded swagger is disabled at compile time",
        )
            .into_response();
    }

    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Swagger UI</title><link rel=\"stylesheet\" href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\"><style>{}</style></head><body><header><h1>Swagger UI</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/swagger\">Swagger</a></nav></header><main><div id=\"swagger-ui\"></div></main><script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\"></script><script>window.ui=SwaggerUIBundle({{url:'/api/swagger',dom_id:'#swagger-ui',deepLinking:true,presets:[SwaggerUIBundle.presets.apis]}});</script></body></html>",
        base_styles()
    ))
    .into_response()
}

async fn audit_page_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    match state
        .admin
        .view_audit_events(&state.project_root, &state.project_id, 50)
        .await
    {
        Ok(events) => {
            let rows = events
                .events
                .into_iter()
                .map(|event| {
                    let payload = html_escape(&event.payload_json);
                    format!(
                        "<tr><td>{}</td><td>{}</td><td><code>{}</code></td></tr>",
                        event.created_at_epoch_ms, event.event_type, payload
                    )
                })
                .collect::<Vec<_>>()
                .join("");
            Html(format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Audit Events</title><style>{}</style></head><body><header><h1>Audit Events</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/swagger\">Swagger</a></nav></header><main><table><tr><th>ts</th><th>type</th><th>payload</th></tr>{}</table></main></body></html>",
                base_styles(),
                rows
            ))
            .into_response()
        }
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("audit error: {err}"),
        )
            .into_response(),
    }
}

async fn config_page_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    match state
        .admin
        .health_report(&state.project_root, &state.project_id)
        .await
    {
        Ok(report) => {
            let nano_model = html_escape(&report.config.nano_model);
            let qdrant_url = html_escape(&report.config.qdrant_url);
            let ca_path = html_escape(
                &report
                    .config
                    .qdrant_ca_cert_path
                    .clone()
                    .unwrap_or_default(),
            );
            let strict_checked = if report.config.qdrant_strict_auth {
                "checked"
            } else {
                ""
            };
            let insecure_checked = if report.config.qdrant_tls_insecure {
                "checked"
            } else {
                ""
            };

            Html(format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Config</title><style>{}</style></head><body><header><h1>Runtime Config</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/swagger\">Swagger</a></nav></header><main><p class=\"muted\">Edit and save project configuration. Environment variables still override these values.</p><form id=\"cfg\" class=\"card\"><div class=\"form-grid\"><label class=\"field\">Provider<select name=\"provider\"><option value=\"local\" {}>local</option><option value=\"hosted\" {}>hosted</option></select></label><label class=\"field\">Nano Provider<select name=\"nano_provider\"><option value=\"rules\" {}>rules</option><option value=\"api\" {}>api</option><option value=\"ollama\" {}>ollama</option><option value=\"lmstudio\" {}>lmstudio</option></select></label><label class=\"field\">Nano Model<input name=\"nano_model\" value=\"{}\"></label><label class=\"field\">Qdrant URL<input name=\"qdrant_url\" value=\"{}\"></label><label class=\"field\">Qdrant API Key<input name=\"qdrant_api_key\" value=\"\" placeholder=\"leave empty to keep current\"></label><label class=\"field\">Qdrant CA Cert Path<input name=\"qdrant_ca_cert_path\" value=\"{}\"></label><label class=\"field\">Qdrant Strict Auth<input type=\"checkbox\" name=\"qdrant_strict_auth\" {}></label><label class=\"field\">Qdrant TLS Insecure<input type=\"checkbox\" name=\"qdrant_tls_insecure\" {}></label></div><div class=\"actions\"><button class=\"btn primary\" type=\"submit\">Save Config</button></div><p id=\"result\" class=\"muted\"></p></form></main><script>const f=document.getElementById('cfg');const out=document.getElementById('result');f.addEventListener('submit',async(e)=>{{e.preventDefault();const d=new FormData(f);const payload={{provider:d.get('provider'),nano_provider:d.get('nano_provider'),nano_model:d.get('nano_model'),qdrant_url:d.get('qdrant_url'),qdrant_ca_cert_path:d.get('qdrant_ca_cert_path'),qdrant_strict_auth:d.get('qdrant_strict_auth')==='on',qdrant_tls_insecure:d.get('qdrant_tls_insecure')==='on'}};const apiKey=d.get('qdrant_api_key');if(apiKey&&apiKey.trim().length>0)payload.qdrant_api_key=apiKey;const res=await fetch('/api/config',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify(payload)}});const body=await res.json();out.textContent=res.ok?'Saved: '+(body.path||'ok'):'Error: '+(body.error||'unknown');}});</script></body></html>",
                base_styles(),
                if report.config.provider == "local" { "selected" } else { "" },
                if report.config.provider == "hosted" { "selected" } else { "" },
                if report.config.nano_provider == "rules" { "selected" } else { "" },
                if report.config.nano_provider == "api" { "selected" } else { "" },
                if report.config.nano_provider == "ollama" { "selected" } else { "" },
                if report.config.nano_provider == "lmstudio" { "selected" } else { "" },
                nano_model,
                qdrant_url,
                ca_path,
                strict_checked,
                insecure_checked,
            ))
            .into_response()
        }
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("config page error: {err}"),
        )
            .into_response(),
    }
}

async fn config_update_handler(
    State(state): State<EmbeddedUiState>,
    Json(payload): Json<ConfigUpdatePayload>,
) -> impl IntoResponse {
    if let Some(provider) = payload.provider.as_deref() {
        if provider != "local" && provider != "hosted" {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":"provider must be local or hosted"})),
            )
                .into_response();
        }
    }

    if let Some(qdrant_url) = payload.qdrant_url.as_deref() {
        let remote = qdrant_url.starts_with("https://")
            || (qdrant_url.starts_with("http://")
                && !qdrant_url.contains("localhost")
                && !qdrant_url.starts_with("http://127.0.0.1"));
        let strict = payload.qdrant_strict_auth.unwrap_or(true);
        let has_api_key = payload
            .qdrant_api_key
            .as_deref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        if remote && strict && !has_api_key {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error":"remote qdrant with strict auth requires qdrant_api_key"
                })),
            )
                .into_response();
        }
    }

    let result = update_project_config(
        &state.project_root,
        ProjectConfigUpdate {
            provider: payload.provider,
            qdrant_url: payload.qdrant_url,
            qdrant_api_key: payload.qdrant_api_key,
            qdrant_ca_cert_path: payload.qdrant_ca_cert_path,
            qdrant_tls_insecure: payload.qdrant_tls_insecure,
            qdrant_strict_auth: payload.qdrant_strict_auth,
            nano_provider: payload.nano_provider,
            nano_model: payload.nano_model,
            ..ProjectConfigUpdate::default()
        },
    );

    match result {
        Ok(path) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"saved": true, "path": path.display().to_string()})),
        )
            .into_response(),
        Err(err) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"saved": false, "error": err.to_string()})),
        )
            .into_response(),
    }
}

impl AdminUi {
    pub fn new(broker: McpBroker) -> Self {
        Self { broker }
    }

    pub async fn trigger_project_refresh(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectRefreshResponse, the_one_core::error::CoreError> {
        self.broker
            .project_refresh(ProjectRefreshRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
            })
            .await
    }

    pub async fn trigger_project_init(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<ProjectInitResponse, the_one_core::error::CoreError> {
        self.broker
            .project_init(ProjectInitRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
            })
            .await
    }

    pub fn trigger_manual_backup(
        &self,
        project_root: &Path,
        backup_root: &Path,
    ) -> Result<BackupResult, the_one_core::error::CoreError> {
        backup_project_state(project_root, backup_root)
    }

    pub fn trigger_manual_restore(
        &self,
        project_root: &Path,
        backup_root: &Path,
    ) -> Result<(), the_one_core::error::CoreError> {
        restore_project_state(project_root, backup_root)
    }

    pub async fn trigger_reindex_docs(
        &self,
        project_root: &Path,
        project_id: &str,
        docs_root: &Path,
    ) -> Result<usize, the_one_core::error::CoreError> {
        self.broker
            .ingest_docs(project_root, project_id, docs_root)
            .await
    }

    pub async fn view_project_profile(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<Option<ProjectProfileGetResponse>, the_one_core::error::CoreError> {
        self.broker
            .project_profile_get(ProjectProfileGetRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
            })
            .await
    }

    pub async fn enable_capability_family(
        &self,
        project_root: &Path,
        family: &str,
    ) -> Result<ToolEnableResponse, the_one_core::error::CoreError> {
        self.broker
            .tool_enable(ToolEnableRequest {
                project_root: project_root.display().to_string(),
                family: family.to_string(),
            })
            .await
    }

    pub fn set_provider_model_config(
        &self,
        project_root: &Path,
        provider: Option<String>,
        nano_provider: Option<String>,
        nano_model: Option<String>,
    ) -> Result<(), the_one_core::error::CoreError> {
        let _ = update_project_config(
            project_root,
            ProjectConfigUpdate {
                provider,
                nano_provider,
                nano_model,
                ..ProjectConfigUpdate::default()
            },
        )?;
        Ok(())
    }

    pub fn set_qdrant_security_config(
        &self,
        project_root: &Path,
        qdrant_url: Option<String>,
        qdrant_api_key: Option<String>,
        qdrant_ca_cert_path: Option<String>,
        qdrant_tls_insecure: Option<bool>,
        qdrant_strict_auth: Option<bool>,
    ) -> Result<(), the_one_core::error::CoreError> {
        let _ = update_project_config(
            project_root,
            ProjectConfigUpdate {
                qdrant_url,
                qdrant_api_key,
                qdrant_ca_cert_path,
                qdrant_tls_insecure,
                qdrant_strict_auth,
                ..ProjectConfigUpdate::default()
            },
        )?;
        Ok(())
    }

    pub async fn view_audit_events(
        &self,
        project_root: &Path,
        project_id: &str,
        limit: usize,
    ) -> Result<AuditEventsResponse, the_one_core::error::CoreError> {
        self.broker
            .audit_events(AuditEventsRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
                limit,
            })
            .await
    }

    pub async fn health_report(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<AdminHealthReport, the_one_core::error::CoreError> {
        let config = self.broker.config_export(project_root).await?;
        let metrics = self.broker.metrics_snapshot();
        let events = self
            .broker
            .audit_events(AuditEventsRequest {
                project_root: project_root.display().to_string(),
                project_id: project_id.to_string(),
                limit: 20,
            })
            .await?;

        Ok(AdminHealthReport {
            config,
            metrics,
            recent_audit_events: events.events.len(),
        })
    }

    pub async fn simulate_tool_run_for_audit(
        &self,
        project_root: &Path,
        project_id: &str,
        action_key: &str,
    ) -> Result<(), the_one_core::error::CoreError> {
        let _ = self
            .broker
            .tool_run(
                project_root,
                project_id,
                ToolRunRequest {
                    action_key: action_key.to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use the_one_mcp::broker::McpBroker;
    use the_one_mcp::swagger::swagger_embedded_enabled;

    use super::{
        render_dashboard_html, resolve_ui_runtime_config, start_embedded_ui_runtime, AdminUi,
    };

    #[test]
    fn test_resolve_ui_runtime_config_uses_global_project_env_precedence() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let cwd = temp.path().join("cwd");
        let project = temp.path().join("project");
        fs::create_dir_all(&cwd).expect("cwd dir should exist");
        fs::create_dir_all(project.join(".the-one")).expect("project state dir should exist");

        let global = temp.path().join("global-home");
        fs::create_dir_all(&global).expect("global dir should exist");
        fs::write(
            global.join("config.json"),
            format!(
                "{{\"project_root\":\"{}\",\"project_id\":\"global-id\",\"ui_bind\":\"127.0.0.1:8788\"}}",
                project.display()
            ),
        )
        .expect("global config should write");
        fs::write(
            project.join(".the-one").join("config.json"),
            "{\"project_id\":\"project-id\",\"ui_bind\":\"127.0.0.1:8799\"}",
        )
        .expect("project config should write");

        std::env::set_var("THE_ONE_HOME", global.display().to_string());
        let resolved = resolve_ui_runtime_config(&cwd).expect("config should resolve");
        assert_eq!(resolved.project_root, project);
        assert_eq!(resolved.project_id, "project-id");
        assert_eq!(resolved.bind_addr, "127.0.0.1:8799");

        std::env::set_var("THE_ONE_PROJECT_ID", "env-id");
        std::env::set_var("THE_ONE_UI_BIND", "127.0.0.1:9900");
        let resolved = resolve_ui_runtime_config(&cwd).expect("config should resolve");
        assert_eq!(resolved.project_id, "env-id");
        assert_eq!(resolved.bind_addr, "127.0.0.1:9900");

        std::env::remove_var("THE_ONE_PROJECT_ID");
        std::env::remove_var("THE_ONE_UI_BIND");
        std::env::remove_var("THE_ONE_HOME");
    }

    #[test]
    fn test_manual_backup_flow() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let state = project.join(".the-one");
        fs::create_dir_all(state.join("qdrant")).expect("qdrant dir should exist");
        fs::write(state.join("state.db"), "db").expect("db write should succeed");

        let ui = AdminUi::new(McpBroker::new());
        let backup_root = temp.path().join("backup");
        let result = ui
            .trigger_manual_backup(&project, &backup_root)
            .expect("backup should succeed");
        assert!(result.sqlite_backup_path.exists());

        fs::write(state.join("state.db"), "db-v2").expect("db write should succeed");
        ui.trigger_manual_restore(&project, &backup_root)
            .expect("restore should succeed");
        let restored = fs::read_to_string(state.join("state.db")).expect("db should read");
        assert_eq!(restored, "db");
    }

    #[tokio::test]
    async fn test_audit_events_view_flow() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");

        let ui = AdminUi::new(McpBroker::new());
        ui.simulate_tool_run_for_audit(&project, "project-1", "tool.run:danger")
            .await
            .expect("simulate tool run should succeed");

        let events = ui
            .view_audit_events(&project, "project-1", 10)
            .await
            .expect("audit events should load");
        assert!(!events.events.is_empty());

        let report = ui
            .health_report(&project, "project-1")
            .await
            .expect("health report should load");
        assert!(report.recent_audit_events >= 1);
        assert_eq!(report.config.schema_version, "v1beta");

        let html = render_dashboard_html(&report);
        assert!(html.contains("The-One Admin Dashboard"));
        assert!(html.contains("recent_audit_events"));
    }

    #[tokio::test]
    async fn test_admin_ui_project_ops_and_config_flow() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");
        fs::write(docs.join("guide.md"), "# Intro\nhello").expect("doc write should work");

        let ui = AdminUi::new(McpBroker::new());
        let init = ui
            .trigger_project_init(&project, "project-1")
            .await
            .expect("init should work");
        assert_eq!(init.project_id, "project-1");

        let ingested = ui
            .trigger_reindex_docs(&project, "project-1", &docs)
            .await
            .expect("reindex should work");
        assert!(ingested >= 1);

        let profile = ui
            .view_project_profile(&project, "project-1")
            .await
            .expect("profile should load");
        assert!(profile.is_some());

        let enabled = ui
            .enable_capability_family(&project, "docs")
            .await
            .expect("enable should work");
        assert!(enabled.enabled_families.contains(&"docs".to_string()));

        ui.set_provider_model_config(
            &project,
            Some("hosted".to_string()),
            Some("api".to_string()),
            Some("gpt-nano".to_string()),
        )
        .expect("set config should work");

        let report = ui
            .health_report(&project, "project-1")
            .await
            .expect("report should work");
        assert_eq!(report.config.provider, "hosted");
        assert_eq!(report.config.nano_provider, "api");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_embedded_ui_runtime_serves_dashboard_and_health() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

        let admin = Arc::new(AdminUi::new(McpBroker::new()));
        admin
            .trigger_project_init(&project, "project-1")
            .await
            .expect("init should work");

        let runtime = start_embedded_ui_runtime(
            admin,
            project.clone(),
            "project-1".to_string(),
            "127.0.0.1:0".parse().expect("addr should parse"),
        )
        .await
        .expect("runtime should start");

        let client = reqwest::Client::new();
        let dashboard_url = format!("http://{}/dashboard", runtime.listen_addr);
        let health_url = format!("http://{}/api/health", runtime.listen_addr);
        let audit_url = format!("http://{}/audit", runtime.listen_addr);
        let config_url = format!("http://{}/config", runtime.listen_addr);
        let api_config_url = format!("http://{}/api/config", runtime.listen_addr);
        let swagger_url = format!("http://{}/api/swagger", runtime.listen_addr);
        let swagger_page_url = format!("http://{}/swagger", runtime.listen_addr);

        let dashboard = client
            .get(dashboard_url)
            .send()
            .await
            .expect("dashboard request should work")
            .text()
            .await
            .expect("dashboard body should read");
        assert!(dashboard.contains("The-One Admin Dashboard"));

        let health = client
            .get(health_url)
            .send()
            .await
            .expect("health request should work")
            .json::<serde_json::Value>()
            .await
            .expect("health body should parse");
        assert_eq!(health["schema_version"], "v1beta");

        let audit = client
            .get(audit_url)
            .send()
            .await
            .expect("audit request should work")
            .text()
            .await
            .expect("audit body should read");
        assert!(audit.contains("Audit Events"));

        let config = client
            .get(config_url)
            .send()
            .await
            .expect("config request should work")
            .text()
            .await
            .expect("config body should read");
        assert!(config.contains("qdrant_url"));

        let update = client
            .post(api_config_url)
            .json(&serde_json::json!({
                "provider": "hosted",
                "nano_provider": "api",
                "nano_model": "gpt-nano",
                "qdrant_url": "https://qdrant.example.com:6333",
                "qdrant_api_key": "secret",
                "qdrant_strict_auth": true
            }))
            .send()
            .await
            .expect("config update should work")
            .json::<serde_json::Value>()
            .await
            .expect("config update body should parse");
        assert_eq!(update["saved"], true);

        let swagger = client
            .get(swagger_url)
            .send()
            .await
            .expect("swagger request should work");
        if swagger_embedded_enabled() {
            assert_eq!(swagger.status(), reqwest::StatusCode::OK);

            let swagger_page = client
                .get(swagger_page_url)
                .send()
                .await
                .expect("swagger page should work")
                .text()
                .await
                .expect("swagger page body should read");
            assert!(swagger_page.contains("SwaggerUIBundle"));
        } else {
            assert_eq!(swagger.status(), reqwest::StatusCode::NOT_FOUND);
        }

        runtime.shutdown();
    }
}
