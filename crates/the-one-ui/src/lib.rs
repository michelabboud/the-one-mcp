use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
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
    limits: Option<the_one_core::limits::ConfigurableLimits>,
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
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>The-One Admin</title><style>{}</style></head><body><header><h1>The-One Admin Dashboard</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/images\">Images</a><a href=\"/swagger\">Swagger</a></nav></header><main><section class=\"grid\"><article class=\"card\"><h2>Config</h2><p><strong>Provider</strong>: {}</p><p><strong>Nano Provider</strong>: {}</p><p><strong>Nano Model</strong>: {}</p><p><strong>Qdrant URL</strong>: {}</p><p><strong>Qdrant Auth</strong>: {}</p></article><article class=\"card\"><h2>Metrics</h2><p>project_init_calls: {}</p><p>project_refresh_calls: {}</p><p>memory_search_calls: {}</p><p>tool_run_calls: {}</p><p>router_fallback_calls: {}</p><p>router_provider_error_calls: {}</p></article><article class=\"card\"><h2>Audit</h2><p>recent_audit_events: {}</p><p><a href=\"/audit\">Open audit explorer</a></p></article><article class=\"card\"><h2>Provider Pool</h2><p class=\"muted\">Nano provider health status is available via the <code>metrics.snapshot</code> MCP tool.</p><p class=\"muted\">Use <code>config.update</code> to manage provider pool entries and routing policy.</p><table><tr><th>Field</th><th>Value</th></tr><tr><td>Routing</td><td>See metrics.snapshot</td></tr><tr><td>Providers</td><td>See metrics.snapshot</td></tr><tr><td>Status</td><td>Provider health available via metrics.snapshot API</td></tr></table></article></section></main></body></html>",
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
        .route("/images", get(images_page_handler))
        .route("/images/thumbnail/{hash}", get(images_thumbnail_handler))
        .route("/api/images", get(api_images_handler))
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
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Swagger UI</title><link rel=\"stylesheet\" href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\"><style>{}</style></head><body><header><h1>Swagger UI</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/images\">Images</a><a href=\"/swagger\">Swagger</a></nav></header><main><div id=\"swagger-ui\"></div></main><script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\"></script><script>window.ui=SwaggerUIBundle({{url:'/api/swagger',dom_id:'#swagger-ui',deepLinking:true,presets:[SwaggerUIBundle.presets.apis]}});</script></body></html>",
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
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Audit Events</title><style>{}</style></head><body><header><h1>Audit Events</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/images\">Images</a><a href=\"/swagger\">Swagger</a></nav></header><main><table><tr><th>ts</th><th>type</th><th>payload</th></tr>{}</table></main></body></html>",
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
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Config</title><style>{}</style></head><body><header><h1>Runtime Config</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/images\">Images</a><a href=\"/swagger\">Swagger</a></nav></header><main><p class=\"muted\">Edit and save project configuration. Environment variables still override these values.</p><form id=\"cfg\" class=\"card\"><div class=\"form-grid\"><label class=\"field\">Provider<select name=\"provider\"><option value=\"local\" {}>local</option><option value=\"hosted\" {}>hosted</option></select></label><label class=\"field\">Nano Provider<select name=\"nano_provider\"><option value=\"rules\" {}>rules</option><option value=\"api\" {}>api</option><option value=\"ollama\" {}>ollama</option><option value=\"lmstudio\" {}>lmstudio</option></select></label><label class=\"field\">Nano Model<input name=\"nano_model\" value=\"{}\"></label><label class=\"field\">Qdrant URL<input name=\"qdrant_url\" value=\"{}\"></label><label class=\"field\">Qdrant API Key<input name=\"qdrant_api_key\" value=\"\" placeholder=\"leave empty to keep current\"></label><label class=\"field\">Qdrant CA Cert Path<input name=\"qdrant_ca_cert_path\" value=\"{}\"></label><label class=\"field\">Qdrant Strict Auth<input type=\"checkbox\" name=\"qdrant_strict_auth\" {}></label><label class=\"field\">Qdrant TLS Insecure<input type=\"checkbox\" name=\"qdrant_tls_insecure\" {}></label></div><h3 style=\"margin-top:16px\">Limits</h3><p class=\"muted\">Configurable limits with validation bounds. Environment variables override these values.</p><div class=\"form-grid\"><label class=\"field\">Max Tool Suggestions (1-20)<input type=\"number\" name=\"limit_max_tool_suggestions\" value=\"5\" min=\"1\" max=\"20\"></label><label class=\"field\">Max Search Hits (1-50)<input type=\"number\" name=\"limit_max_search_hits\" value=\"5\" min=\"1\" max=\"50\"></label><label class=\"field\">Max Raw Section Bytes (1024-102400)<input type=\"number\" name=\"limit_max_raw_section_bytes\" value=\"24576\" min=\"1024\" max=\"102400\"></label><label class=\"field\">Max Enabled Families (1-50)<input type=\"number\" name=\"limit_max_enabled_families\" value=\"12\" min=\"1\" max=\"50\"></label><label class=\"field\">Max Doc Size Bytes (1024-1048576)<input type=\"number\" name=\"limit_max_doc_size_bytes\" value=\"102400\" min=\"1024\" max=\"1048576\"></label><label class=\"field\">Max Managed Docs (1-10000)<input type=\"number\" name=\"limit_max_managed_docs\" value=\"500\" min=\"1\" max=\"10000\"></label><label class=\"field\">Max Embedding Batch Size (1-256)<input type=\"number\" name=\"limit_max_embedding_batch_size\" value=\"64\" min=\"1\" max=\"256\"></label><label class=\"field\">Max Chunk Tokens (64-4096)<input type=\"number\" name=\"limit_max_chunk_tokens\" value=\"512\" min=\"64\" max=\"4096\"></label><label class=\"field\">Max Nano Timeout Ms (100-30000)<input type=\"number\" name=\"limit_max_nano_timeout_ms\" value=\"2000\" min=\"100\" max=\"30000\"></label><label class=\"field\">Max Nano Retries (0-10)<input type=\"number\" name=\"limit_max_nano_retries\" value=\"3\" min=\"0\" max=\"10\"></label><label class=\"field\">Max Nano Providers (1-5)<input type=\"number\" name=\"limit_max_nano_providers\" value=\"5\" min=\"1\" max=\"5\"></label><label class=\"field\">Search Score Threshold (0.0-1.0)<input type=\"number\" name=\"limit_search_score_threshold\" value=\"0.3\" min=\"0\" max=\"1\" step=\"0.01\"></label></div><div class=\"actions\"><button class=\"btn primary\" type=\"submit\">Save Config</button></div><p id=\"result\" class=\"muted\"></p></form></main><script>const f=document.getElementById('cfg');const out=document.getElementById('result');f.addEventListener('submit',async(e)=>{{e.preventDefault();const d=new FormData(f);const payload={{provider:d.get('provider'),nano_provider:d.get('nano_provider'),nano_model:d.get('nano_model'),qdrant_url:d.get('qdrant_url'),qdrant_ca_cert_path:d.get('qdrant_ca_cert_path'),qdrant_strict_auth:d.get('qdrant_strict_auth')==='on',qdrant_tls_insecure:d.get('qdrant_tls_insecure')==='on'}};const apiKey=d.get('qdrant_api_key');if(apiKey&&apiKey.trim().length>0)payload.qdrant_api_key=apiKey;payload.limits={{max_tool_suggestions:parseInt(d.get('limit_max_tool_suggestions'))||5,max_search_hits:parseInt(d.get('limit_max_search_hits'))||5,max_raw_section_bytes:parseInt(d.get('limit_max_raw_section_bytes'))||24576,max_enabled_families:parseInt(d.get('limit_max_enabled_families'))||12,max_doc_size_bytes:parseInt(d.get('limit_max_doc_size_bytes'))||102400,max_managed_docs:parseInt(d.get('limit_max_managed_docs'))||500,max_embedding_batch_size:parseInt(d.get('limit_max_embedding_batch_size'))||64,max_chunk_tokens:parseInt(d.get('limit_max_chunk_tokens'))||512,max_nano_timeout_ms:parseInt(d.get('limit_max_nano_timeout_ms'))||2000,max_nano_retries:parseInt(d.get('limit_max_nano_retries'))||3,max_nano_providers:parseInt(d.get('limit_max_nano_providers'))||5,search_score_threshold:parseFloat(d.get('limit_search_score_threshold'))||0.3}};const res=await fetch('/api/config',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify(payload)}});const body=await res.json();out.textContent=res.ok?'Saved: '+(body.path||'ok'):'Error: '+(body.error||'unknown');}});</script></body></html>",
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

// ── Image gallery ──────────────────────────────────────────────────────────

/// A single row from the `managed_images` SQLite table.
#[derive(Debug, Clone)]
struct ManagedImageRow {
    path: String,
    hash: String,
    caption: Option<String>,
    ocr_text: Option<String>,
    thumbnail_path: Option<String>,
}

/// Validate that `hash` is a hex string (32–64 characters, lowercase hex only).
/// This prevents path-traversal via the thumbnail route.
fn is_valid_image_hash(hash: &str) -> bool {
    let len = hash.len();
    (32..=64).contains(&len) && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Query the `managed_images` table from the project SQLite database.
/// Returns `None` if the table does not exist (image embeddings never enabled).
fn query_managed_images(project_root: &Path) -> Result<Option<Vec<ManagedImageRow>>, String> {
    let db_path = project_root.join(".the-one").join("state.db");
    if !db_path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("db open: {e}"))?;

    // Check if the table exists — it is only created when image-embeddings are enabled.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='managed_images'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    if !table_exists {
        return Ok(None);
    }

    let mut stmt = conn
        .prepare("SELECT path, hash, caption, ocr_text, thumbnail_path FROM managed_images ORDER BY path")
        .map_err(|e| format!("prepare: {e}"))?;

    let rows: Result<Vec<ManagedImageRow>, _> = stmt
        .query_map([], |row| {
            Ok(ManagedImageRow {
                path: row.get(0)?,
                hash: row.get(1)?,
                caption: row.get(2)?,
                ocr_text: row.get(3)?,
                thumbnail_path: row.get(4)?,
            })
        })
        .map_err(|e| format!("query: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row: {e}"));

    Ok(Some(rows?))
}

/// Render an image card for the gallery.
fn render_image_card(row: &ManagedImageRow) -> String {
    let escaped_path = html_escape(&row.path);
    let filename = row.path.rsplit('/').next().unwrap_or(&row.path).to_string();
    let caption_text = row
        .caption
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&filename);
    let caption = html_escape(caption_text);
    let ocr_preview = row
        .ocr_text
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect::<String>();
    let ocr = html_escape(&ocr_preview);
    let hash = html_escape(&row.hash);

    format!(
        "<div class=\"img-card\">\
            <img src=\"/images/thumbnail/{hash}\" alt=\"{escaped_path}\" loading=\"lazy\">\
            <div class=\"img-caption\">{caption}</div>\
            <div class=\"img-path\">{escaped_path}</div>\
            <div class=\"img-ocr\">{ocr}</div>\
        </div>"
    )
}

/// Render the full images gallery HTML page.
fn render_images_html(images: &Option<Vec<ManagedImageRow>>) -> String {
    let styles = base_styles();
    let extra_styles = "\
        .img-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:20px;margin-top:16px}\
        .img-card{background:#fff;border:1px solid #dbe3ef;border-radius:12px;padding:12px;box-shadow:0 4px 10px rgba(2,6,23,.06)}\
        .img-card img{width:100%;height:180px;object-fit:contain;background:#eee;border-radius:4px}\
        .img-caption{font-size:13px;margin-top:8px;font-weight:600;color:#0f172a}\
        .img-path{font-size:11px;color:#64748b;margin-top:4px;word-break:break-all}\
        .img-ocr{font-size:11px;color:#94a3b8;margin-top:4px;max-height:40px;overflow:hidden}\
        .img-count{color:#475569;font-size:.95rem;margin-bottom:8px}\
        .img-empty{text-align:center;padding:60px 20px;color:#64748b}\
        .img-empty h2{margin-bottom:12px}\
        .img-empty code{background:#f1f5f9;padding:2px 6px;border-radius:4px;font-size:.9rem}";

    let body = match images {
        None => "<div class=\"img-empty\">\
                <h2>No indexed images</h2>\
                <p>The <code>managed_images</code> table does not exist yet.</p>\
                <p>Enable image embeddings in your project config, then run \
                <code>memory.ingest_images</code> to index images.</p>\
            </div>"
            .to_string(),
        Some(rows) if rows.is_empty() => "<div class=\"img-empty\">\
                <h2>No indexed images</h2>\
                <p>Image embeddings are enabled but no images have been indexed yet.</p>\
                <p>Run <code>memory.ingest_images</code> to index images from your project.</p>\
            </div>"
            .to_string(),
        Some(rows) => {
            let count = rows.len();
            let cards: String = rows.iter().map(render_image_card).collect();
            format!(
                "<p class=\"img-count\">{count} indexed image{s}</p>\
                <div class=\"img-grid\">{cards}</div>",
                s = if count == 1 { "" } else { "s" }
            )
        }
    };

    format!(
        "<!doctype html><html><head>\
            <meta charset=\"utf-8\">\
            <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
            <title>Images &mdash; the-one-mcp</title>\
            <style>{styles}{extra_styles}</style>\
        </head><body>\
            <header>\
                <h1>Image Gallery</h1>\
                <nav>\
                    <a href=\"/dashboard\">Dashboard</a>\
                    <a href=\"/config\">Config</a>\
                    <a href=\"/audit\">Audit</a>\
                    <a href=\"/images\">Images</a>\
                    <a href=\"/swagger\">Swagger</a>\
                </nav>\
            </header>\
            <main>{body}</main>\
        </body></html>"
    )
}

async fn images_page_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let project_root = state.project_root.clone();
    let result = tokio::task::spawn_blocking(move || query_managed_images(&project_root)).await;

    match result {
        Ok(Ok(images)) => Html(render_images_html(&images)).into_response(),
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("images query error: {e}"),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("task error: {e}"),
        )
            .into_response(),
    }
}

async fn images_thumbnail_handler(
    State(state): State<EmbeddedUiState>,
    AxumPath(hash): AxumPath<String>,
) -> impl IntoResponse {
    // Security: reject any hash that is not a lowercase hex string of length 32-64.
    if !is_valid_image_hash(&hash) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid hash format").into_response();
    }

    let thumb_path = state
        .project_root
        .join(".the-one")
        .join("thumbnails")
        .join(format!("{hash}.webp"));

    match tokio::fs::read(&thumb_path).await {
        Ok(bytes) => (
            axum::http::StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "image/webp"),
                (axum::http::header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (axum::http::StatusCode::NOT_FOUND, "thumbnail not found").into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("thumbnail read error: {e}"),
        )
            .into_response(),
    }
}

async fn api_images_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let project_root = state.project_root.clone();
    let result = tokio::task::spawn_blocking(move || query_managed_images(&project_root)).await;

    match result {
        Ok(Ok(images)) => {
            let rows = images.unwrap_or_default();
            let json_rows: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let hash = r.hash.clone();
                    serde_json::json!({
                        "path": r.path,
                        "hash": hash,
                        "caption": r.caption,
                        "ocr_text": r.ocr_text,
                        "thumbnail_path": r.thumbnail_path,
                        "thumbnail_url": format!("/images/thumbnail/{hash}"),
                    })
                })
                .collect();
            Json(serde_json::json!({ "images": json_rows })).into_response()
        }
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
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
            limits: payload.limits,
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
        is_valid_image_hash, query_managed_images, render_dashboard_html, render_images_html,
        resolve_ui_runtime_config, start_embedded_ui_runtime, AdminUi,
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

    #[test]
    fn test_images_route_empty_state() {
        // When no state.db exists, query returns None → empty-state HTML.
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");

        let images = query_managed_images(&project).expect("query should not error");
        assert!(images.is_none(), "no state.db → should return None");

        let html = render_images_html(&images);
        assert!(html.contains("managed_images"), "should show table name");
        assert!(html.contains("Image Gallery"), "should have page title");
    }

    #[test]
    fn test_images_route_with_data() {
        use rusqlite::Connection;

        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let state_dir = project.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");

        // Create a real SQLite DB with managed_images table.
        let db_path = state_dir.join("state.db");
        let conn = Connection::open(&db_path).expect("db should open");
        conn.execute_batch(
            "CREATE TABLE managed_images (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_epoch INTEGER NOT NULL,
                caption TEXT,
                ocr_text TEXT,
                thumbnail_path TEXT,
                indexed_at_epoch INTEGER NOT NULL
            );",
        )
        .expect("create table should succeed");
        conn.execute(
            "INSERT INTO managed_images VALUES (
                'docs/auth-flow.png', 'abc123def456abc123def456abc123de', 1024, 0,
                'Auth flow diagram', 'User → Auth → DB', NULL, 0
            )",
            [],
        )
        .expect("insert should succeed");
        drop(conn);

        let images = query_managed_images(&project)
            .expect("query should not error")
            .expect("table exists → should return Some");

        assert_eq!(images.len(), 1, "should find one image");
        assert_eq!(images[0].path, "docs/auth-flow.png");
        assert_eq!(images[0].hash, "abc123def456abc123def456abc123de");
        assert_eq!(images[0].caption.as_deref(), Some("Auth flow diagram"));
        assert_eq!(images[0].ocr_text.as_deref(), Some("User → Auth → DB"));

        let html = render_images_html(&Some(images));
        assert!(html.contains("auth-flow.png"), "should show filename");
        assert!(html.contains("Auth flow diagram"), "should show caption");
        assert!(html.contains("img-grid"), "should render grid");
        assert!(
            html.contains("/images/thumbnail/abc123def456abc123def456abc123de"),
            "should have thumbnail URL"
        );
    }

    #[test]
    fn test_thumbnail_hash_validation_rejects_path_traversal() {
        // Reject path traversal and invalid characters.
        assert!(
            !is_valid_image_hash("../etc/passwd"),
            "path traversal must be rejected"
        );
        assert!(
            !is_valid_image_hash("../../secret"),
            "path traversal must be rejected"
        );
        assert!(!is_valid_image_hash("abc/def"), "slash must be rejected");
        assert!(
            !is_valid_image_hash("abc DEF"),
            "uppercase must be rejected"
        );
        assert!(!is_valid_image_hash("ABCDEF"), "uppercase must be rejected");
        assert!(!is_valid_image_hash(""), "empty string must be rejected");
        assert!(
            !is_valid_image_hash("abc"),
            "too short must be rejected (< 32)"
        );
        assert!(
            !is_valid_image_hash(&"a".repeat(65)),
            "too long must be rejected (> 64)"
        );
        // Accept valid lowercase hex strings.
        assert!(
            is_valid_image_hash(&"a".repeat(32)),
            "32-char hex should be valid"
        );
        assert!(
            is_valid_image_hash(&"f".repeat(64)),
            "64-char hex should be valid"
        );
        assert!(
            is_valid_image_hash("abc123def456abc123def456abc123de"),
            "mixed hex should be valid"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_thumbnail_route_404_for_missing_file() {
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
        let url = format!(
            "http://{}/images/thumbnail/{}",
            runtime.listen_addr,
            "a".repeat(32)
        );
        let resp = client.get(url).send().await.expect("request should work");
        assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

        // Path traversal via URL is rejected with 400.
        let bad_url = format!(
            "http://{}/images/thumbnail/../etc/passwd",
            runtime.listen_addr
        );
        let bad_resp = client
            .get(bad_url)
            .send()
            .await
            .expect("request should work");
        // Axum may return 404 for unmatched path or 400 from our handler.
        assert!(
            bad_resp.status() == reqwest::StatusCode::NOT_FOUND
                || bad_resp.status() == reqwest::StatusCode::BAD_REQUEST,
            "path traversal must not return 200"
        );

        runtime.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_images_page_and_api_serve_correctly() {
        use rusqlite::Connection as RusqliteConn;

        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let state_dir = project.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

        // Seed managed_images table.
        let db_path = state_dir.join("state.db");
        let conn = RusqliteConn::open(&db_path).expect("db should open");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS managed_images (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_epoch INTEGER NOT NULL,
                caption TEXT,
                ocr_text TEXT,
                thumbnail_path TEXT,
                indexed_at_epoch INTEGER NOT NULL
            );",
        )
        .expect("create table should succeed");
        conn.execute(
            "INSERT INTO managed_images VALUES (
                'images/logo.png', 'deadbeefdeadbeefdeadbeefdeadbeef', 512, 0,
                'Company logo', 'ACME Corp', NULL, 0
            )",
            [],
        )
        .expect("insert should succeed");
        drop(conn);

        let admin = Arc::new(AdminUi::new(McpBroker::new()));
        let runtime = start_embedded_ui_runtime(
            admin,
            project.clone(),
            "project-1".to_string(),
            "127.0.0.1:0".parse().expect("addr should parse"),
        )
        .await
        .expect("runtime should start");

        let client = reqwest::Client::new();

        // /images HTML page
        let html = client
            .get(format!("http://{}/images", runtime.listen_addr))
            .send()
            .await
            .expect("images request should work")
            .text()
            .await
            .expect("images body should read");
        assert!(html.contains("Image Gallery"), "should contain page title");
        assert!(html.contains("logo.png"), "should contain image filename");
        assert!(html.contains("Company logo"), "should contain caption");
        assert!(html.contains("ACME Corp"), "should contain OCR text");

        // /api/images JSON endpoint
        let json: serde_json::Value = client
            .get(format!("http://{}/api/images", runtime.listen_addr))
            .send()
            .await
            .expect("api/images request should work")
            .json()
            .await
            .expect("api/images body should parse");
        assert!(json["images"].is_array(), "should have images array");
        assert_eq!(
            json["images"].as_array().unwrap().len(),
            1,
            "should have one image"
        );
        let first = &json["images"][0];
        assert_eq!(first["path"], "images/logo.png");
        assert_eq!(first["hash"], "deadbeefdeadbeefdeadbeefdeadbeef");
        assert!(
            first["thumbnail_url"]
                .as_str()
                .unwrap()
                .starts_with("/images/thumbnail/"),
            "thumbnail_url should point to /images/thumbnail/"
        );

        runtime.shutdown();
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
