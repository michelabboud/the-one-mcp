use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use the_one_core::backup::{backup_project_state, restore_project_state, BackupResult};
use the_one_core::config::{
    update_project_config, AppConfig, MemoryPalaceProfilePreset, ProjectConfigUpdate,
    RuntimeOverrides,
};
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
    memory_palace_profile: Option<String>,
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

fn memory_palace_profile_from_config(config: &AppConfig) -> Option<MemoryPalaceProfilePreset> {
    match (
        config.memory_palace_enabled,
        config.memory_palace_hooks_enabled,
        config.memory_palace_aaak_enabled,
        config.memory_palace_diary_enabled,
        config.memory_palace_navigation_enabled,
    ) {
        (false, false, false, false, false) => Some(MemoryPalaceProfilePreset::Off),
        (true, false, false, false, false) => Some(MemoryPalaceProfilePreset::Core),
        (true, true, true, true, true) => Some(MemoryPalaceProfilePreset::Full),
        _ => None,
    }
}

fn memory_palace_profile_label(config: &AppConfig) -> &'static str {
    memory_palace_profile_from_config(config)
        .map(|preset| preset.as_str())
        .unwrap_or("custom")
}

fn memory_palace_profile_select_value(config: &AppConfig) -> Option<&'static str> {
    memory_palace_profile_from_config(config).map(|preset| preset.as_str())
}

fn render_memory_palace_profile_card(config: &AppConfig) -> String {
    let profile_label = memory_palace_profile_label(config);
    let badge_class = if profile_label == "custom" {
        "warn"
    } else {
        "ok"
    };
    let enabled_badge = if config.memory_palace_enabled {
        "ok"
    } else {
        "err"
    };
    let hooks_badge = if config.memory_palace_hooks_enabled {
        "ok"
    } else {
        "err"
    };
    let aaak_badge = if config.memory_palace_aaak_enabled {
        "ok"
    } else {
        "err"
    };
    let diary_badge = if config.memory_palace_diary_enabled {
        "ok"
    } else {
        "err"
    };
    let navigation_badge = if config.memory_palace_navigation_enabled {
        "ok"
    } else {
        "err"
    };

    format!(
        r#"<section class="card profile-card">
  <h3>MemPalace profile</h3>
  <p class="muted">Active preset: <span class="badge {badge_class}">{profile_label}</span></p>
  <p class="muted">Expanded flags:
    <code>enabled={enabled}</code>
    <code>hooks={hooks}</code>
    <code>aaak={aaak}</code>
    <code>diary={diary}</code>
    <code>navigation={navigation}</code>
  </p>
  <ul class="profile-flag-list">
    <li><code>memory_palace_enabled</code> <span class="badge {enabled_badge}">{enabled_text}</span></li>
    <li><code>memory_palace_hooks_enabled</code> <span class="badge {hooks_badge}">{hooks_text}</span></li>
    <li><code>memory_palace_aaak_enabled</code> <span class="badge {aaak_badge}">{aaak_text}</span></li>
    <li><code>memory_palace_diary_enabled</code> <span class="badge {diary_badge}">{diary_text}</span></li>
    <li><code>memory_palace_navigation_enabled</code> <span class="badge {navigation_badge}">{navigation_text}</span></li>
  </ul>
  <p class="muted">Use the config select below to switch between <code>off</code>, <code>core</code>, and <code>full</code>.</p>
</section>"#,
        enabled = config.memory_palace_enabled,
        hooks = config.memory_palace_hooks_enabled,
        aaak = config.memory_palace_aaak_enabled,
        diary = config.memory_palace_diary_enabled,
        navigation = config.memory_palace_navigation_enabled,
        enabled_text = if config.memory_palace_enabled {
            "enabled"
        } else {
            "disabled"
        },
        hooks_text = if config.memory_palace_hooks_enabled {
            "enabled"
        } else {
            "disabled"
        },
        aaak_text = if config.memory_palace_aaak_enabled {
            "enabled"
        } else {
            "disabled"
        },
        diary_text = if config.memory_palace_diary_enabled {
            "enabled"
        } else {
            "disabled"
        },
        navigation_text = if config.memory_palace_navigation_enabled {
            "enabled"
        } else {
            "disabled"
        },
    )
}

fn render_memory_palace_profile_select(config: &AppConfig) -> String {
    let profile_select_value = memory_palace_profile_select_value(config);
    let preserve_selected = if profile_select_value.is_none() {
        "selected"
    } else {
        ""
    };
    let profile_off_selected = if matches!(profile_select_value, Some("off")) {
        "selected"
    } else {
        ""
    };
    let profile_core_selected = if matches!(profile_select_value, Some("core")) {
        "selected"
    } else {
        ""
    };
    let profile_full_selected = if matches!(profile_select_value, Some("full")) {
        "selected"
    } else {
        ""
    };

    format!(
        r#"<div class="form-grid"><label class="field">MemPalace Profile<select name="memory_palace_profile"><option value="" {preserve_selected}>preserve current custom state</option><option value="off" {profile_off_selected}>off</option><option value="core" {profile_core_selected}>core</option><option value="full" {profile_full_selected}>full</option></select></label></div>"#
    )
}

fn render_limits_form(config: &AppConfig) -> String {
    let limits = &config.limits;
    format!(
        r#"<h3 style="margin-top:16px">Limits</h3><p class="muted">Configurable limits with validation bounds. Environment variables override these values.</p><div class="form-grid"><label class="field">Max Tool Suggestions (1-20)<input type="number" name="limit_max_tool_suggestions" value="{max_tool_suggestions}" min="1" max="20"></label><label class="field">Max Search Hits (1-50)<input type="number" name="limit_max_search_hits" value="{max_search_hits}" min="1" max="50"></label><label class="field">Max Raw Section Bytes (1024-102400)<input type="number" name="limit_max_raw_section_bytes" value="{max_raw_section_bytes}" min="1024" max="102400"></label><label class="field">Max Enabled Families (1-50)<input type="number" name="limit_max_enabled_families" value="{max_enabled_families}" min="1" max="50"></label><label class="field">Max Doc Size Bytes (1024-1048576)<input type="number" name="limit_max_doc_size_bytes" value="{max_doc_size_bytes}" min="1024" max="1048576"></label><label class="field">Max Managed Docs (1-10000)<input type="number" name="limit_max_managed_docs" value="{max_managed_docs}" min="1" max="10000"></label><label class="field">Max Embedding Batch Size (1-256)<input type="number" name="limit_max_embedding_batch_size" value="{max_embedding_batch_size}" min="1" max="256"></label><label class="field">Max Chunk Tokens (64-4096)<input type="number" name="limit_max_chunk_tokens" value="{max_chunk_tokens}" min="64" max="4096"></label><label class="field">Max Nano Timeout Ms (100-30000)<input type="number" name="limit_max_nano_timeout_ms" value="{max_nano_timeout_ms}" min="100" max="30000"></label><label class="field">Max Nano Retries (0-10)<input type="number" name="limit_max_nano_retries" value="{max_nano_retries}" min="0" max="10"></label><label class="field">Max Nano Providers (1-5)<input type="number" name="limit_max_nano_providers" value="{max_nano_providers}" min="1" max="5"></label><label class="field">Search Score Threshold (0.0-1.0)<input type="number" name="limit_search_score_threshold" value="{search_score_threshold}" min="0" max="1" step="0.01"></label></div>"#,
        max_tool_suggestions = limits.max_tool_suggestions,
        max_search_hits = limits.max_search_hits,
        max_raw_section_bytes = limits.max_raw_section_bytes,
        max_enabled_families = limits.max_enabled_families,
        max_doc_size_bytes = limits.max_doc_size_bytes,
        max_managed_docs = limits.max_managed_docs,
        max_embedding_batch_size = limits.max_embedding_batch_size,
        max_chunk_tokens = limits.max_chunk_tokens,
        max_nano_timeout_ms = limits.max_nano_timeout_ms,
        max_nano_retries = limits.max_nano_retries,
        max_nano_providers = limits.max_nano_providers,
        search_score_threshold = limits.search_score_threshold,
    )
}

fn render_config_save_script(config: &AppConfig) -> String {
    let limits = &config.limits;
    format!(
        r#"<script>const f=document.getElementById('cfg');const out=document.getElementById('result');const currentLimits={{max_tool_suggestions:{max_tool_suggestions},max_search_hits:{max_search_hits},max_raw_section_bytes:{max_raw_section_bytes},max_enabled_families:{max_enabled_families},max_doc_size_bytes:{max_doc_size_bytes},max_managed_docs:{max_managed_docs},max_embedding_batch_size:{max_embedding_batch_size},max_chunk_tokens:{max_chunk_tokens},max_nano_timeout_ms:{max_nano_timeout_ms},max_nano_retries:{max_nano_retries},max_nano_providers:{max_nano_providers},search_score_threshold:{search_score_threshold}}};const readInt=(name,fallback)=>{{const value=Number.parseInt(f.elements.namedItem(name)?.value ?? '',10);return Number.isNaN(value)?fallback:value;}};const readFloat=(name,fallback)=>{{const value=Number.parseFloat(f.elements.namedItem(name)?.value ?? '');return Number.isNaN(value)?fallback:value;}};f.addEventListener('submit',async(e)=>{{e.preventDefault();const d=new FormData(f);const payload={{provider:d.get('provider'),nano_provider:d.get('nano_provider'),nano_model:d.get('nano_model'),qdrant_url:d.get('qdrant_url'),qdrant_ca_cert_path:d.get('qdrant_ca_cert_path'),qdrant_strict_auth:d.get('qdrant_strict_auth')==='on',qdrant_tls_insecure:d.get('qdrant_tls_insecure')==='on'}};const profile=d.get('memory_palace_profile');if(profile==='off'||profile==='core'||profile==='full')payload.memory_palace_profile=profile;const apiKey=d.get('qdrant_api_key');if(apiKey&&apiKey.trim().length>0)payload.qdrant_api_key=apiKey;payload.limits={{max_tool_suggestions:readInt('limit_max_tool_suggestions',currentLimits.max_tool_suggestions),max_search_hits:readInt('limit_max_search_hits',currentLimits.max_search_hits),max_raw_section_bytes:readInt('limit_max_raw_section_bytes',currentLimits.max_raw_section_bytes),max_enabled_families:readInt('limit_max_enabled_families',currentLimits.max_enabled_families),max_doc_size_bytes:readInt('limit_max_doc_size_bytes',currentLimits.max_doc_size_bytes),max_managed_docs:readInt('limit_max_managed_docs',currentLimits.max_managed_docs),max_embedding_batch_size:readInt('limit_max_embedding_batch_size',currentLimits.max_embedding_batch_size),max_chunk_tokens:readInt('limit_max_chunk_tokens',currentLimits.max_chunk_tokens),max_nano_timeout_ms:readInt('limit_max_nano_timeout_ms',currentLimits.max_nano_timeout_ms),max_nano_retries:readInt('limit_max_nano_retries',currentLimits.max_nano_retries),max_nano_providers:readInt('limit_max_nano_providers',currentLimits.max_nano_providers),search_score_threshold:readFloat('limit_search_score_threshold',currentLimits.search_score_threshold)}};const res=await fetch('/api/config',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify(payload)}});const body=await res.json();out.textContent=res.ok?'Saved: '+(body.path||'ok'):'Error: '+(body.error||'unknown');}});</script>"#,
        max_tool_suggestions = limits.max_tool_suggestions,
        max_search_hits = limits.max_search_hits,
        max_raw_section_bytes = limits.max_raw_section_bytes,
        max_enabled_families = limits.max_enabled_families,
        max_doc_size_bytes = limits.max_doc_size_bytes,
        max_managed_docs = limits.max_managed_docs,
        max_embedding_batch_size = limits.max_embedding_batch_size,
        max_chunk_tokens = limits.max_chunk_tokens,
        max_nano_timeout_ms = limits.max_nano_timeout_ms,
        max_nano_retries = limits.max_nano_retries,
        max_nano_providers = limits.max_nano_providers,
        search_score_threshold = limits.search_score_threshold,
    )
}

/// Render the admin UI landing page served at `/`.
///
/// This is a static marketing / orientation page that explains what
/// the-one-mcp is, exposes quick links to the other admin routes via a
/// top nav, and points users at GitHub, docs, and issues. It does NOT
/// hit the broker (no metrics, no project state) so it works even when
/// the project directory is in a weird state.
pub fn render_home_html(project_root: &str, project_id: &str) -> String {
    let pkg_version = env!("CARGO_PKG_VERSION");
    let project_root = html_escape(project_root);
    let project_id = html_escape(project_id);
    format!(
        r##"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>the-one-mcp — Admin UI</title>
<style>{styles}{home_styles}</style>
</head><body>
<header>
  <h1>the-one-mcp</h1>
  <nav>
    <a href="/">Home</a>
    <a href="/dashboard">Dashboard</a>
    <a href="/config">Config</a>
    <a href="/audit">Audit</a>
    <a href="/images">Images</a>
    <a href="/swagger">API Docs</a>
  </nav>
</header>
<main>
  <section class="hero">
    <h2>Semantic memory for your AI coding assistant</h2>
    <p class="lead">
      A Rust-native MCP broker that gives Claude Code, Gemini CLI, OpenCode, and Codex
      unlimited, searchable memory of your entire project — docs, code, even screenshots.
    </p>
    <p class="version-line">
      Running version <code>v{version}</code> · Project <code>{project_id}</code> · Root <code>{project_root}</code>
    </p>
  </section>

  <section class="grid">
    <article class="card">
      <h3>Features</h3>
      <ul class="feature-list">
        <li><strong>17 MCP tools</strong> + 3 resource types (<code>docs</code>, <code>project</code>, <code>catalog</code>)</li>
        <li><strong>Hybrid search</strong> — dense embeddings + SPLADE sparse vectors</li>
        <li><strong>Tree-sitter code chunker</strong> for 13 languages</li>
        <li><strong>184 curated CLI tools</strong> across 10 languages</li>
        <li><strong>Auto-reindex</strong> watcher for markdown + images</li>
        <li><strong>Backup / restore</strong> via <code>maintain: backup</code></li>
        <li><strong>Observability</strong> — 15 metrics counters via <code>observe</code></li>
      </ul>
    </article>

    <article class="card">
      <h3>Admin sections</h3>
      <ul class="link-list">
        <li><a href="/dashboard">Dashboard</a> — health, metrics, provider pool</li>
        <li><a href="/config">Config</a> — 5-layer config viewer + editor</li>
        <li><a href="/audit">Audit</a> — append-only event log</li>
        <li><a href="/images">Images</a> — indexed image gallery</li>
        <li><a href="/swagger">API Docs</a> — OpenAPI / Swagger for the broker</li>
        <li><a href="/api/health">/api/health</a> — JSON health probe</li>
      </ul>
    </article>

    <article class="card">
      <h3>Project & community</h3>
      <ul class="link-list">
        <li><a href="https://github.com/michelabboud/the-one-mcp" target="_blank" rel="noopener">GitHub repository</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/issues" target="_blank" rel="noopener">Report an issue</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/pulls" target="_blank" rel="noopener">Pull requests</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/releases" target="_blank" rel="noopener">Releases</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/CHANGELOG.md" target="_blank" rel="noopener">Changelog</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/CONTRIBUTING.md" target="_blank" rel="noopener">Contributing</a></li>
      </ul>
    </article>

    <article class="card">
      <h3>Documentation</h3>
      <ul class="link-list">
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/INSTALL.md" target="_blank" rel="noopener">Install guide</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/quickstart.md" target="_blank" rel="noopener">Quickstart</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/api-reference.md" target="_blank" rel="noopener">API reference</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/mcp-resources.md" target="_blank" rel="noopener">MCP Resources guide</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/code-chunking.md" target="_blank" rel="noopener">Code chunking (tree-sitter)</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/hybrid-search.md" target="_blank" rel="noopener">Hybrid search</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/backup-restore.md" target="_blank" rel="noopener">Backup &amp; restore</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/observability.md" target="_blank" rel="noopener">Observability</a></li>
        <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/troubleshooting.md" target="_blank" rel="noopener">Troubleshooting</a></li>
      </ul>
    </article>
  </section>

  <section class="card cta">
    <h3>Quick start</h3>
    <p>Install on a new machine:</p>
    <pre><code>curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash</code></pre>
    <p class="muted">Auto-detects your OS, downloads the right binary, registers with Claude Code / Gemini CLI / OpenCode / Codex.</p>
  </section>
</main>
<footer class="home-footer">
  <p>
    the-one-mcp · Apache-2.0 ·
    <a href="https://github.com/michelabboud/the-one-mcp" target="_blank" rel="noopener">github.com/michelabboud/the-one-mcp</a>
  </p>
</footer>
</body></html>"##,
        styles = base_styles(),
        home_styles = home_styles(),
        version = pkg_version,
        project_root = project_root,
        project_id = project_id,
    )
}

/// Extra styles for the landing page only. Kept separate from
/// `base_styles()` so dashboard/config/audit/images pages stay visually
/// consistent and the home page gets a bit of hero polish.
fn home_styles() -> &'static str {
    ".hero{background:#fff;border:1px solid #dbe3ef;border-radius:12px;padding:28px 28px 24px;margin-bottom:20px;box-shadow:0 4px 10px rgba(2,6,23,.06)}\
     .hero h2{margin:0 0 10px 0;font-size:1.6rem;color:#0b3d91}\
     .hero .lead{margin:0 0 14px 0;font-size:1.08rem;color:#1e293b;max-width:780px;line-height:1.55}\
     .hero .version-line{margin:0;color:#475569;font-size:.9rem}\
     .hero .version-line code{background:#eef2f8;padding:2px 6px;border-radius:4px;font-size:.88rem}\
     .feature-list,.link-list{margin:0;padding-left:20px;line-height:1.65}\
     .feature-list li,.link-list li{margin-bottom:4px}\
     .link-list a{color:#0b3d91;text-decoration:none}\
     .link-list a:hover{text-decoration:underline}\
     .cta{margin-top:20px}\
     .cta pre{background:#0f172a;color:#e2e8f0;padding:14px 16px;border-radius:8px;overflow-x:auto;margin:8px 0}\
     .cta pre code{background:transparent;color:inherit;padding:0}\
     .home-footer{padding:28px 20px;text-align:center;color:#475569;border-top:1px solid #dbe3ef;margin-top:24px}\
     .home-footer a{color:#0b3d91;text-decoration:none}\
     .home-footer a:hover{text-decoration:underline}"
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
    "body{font-family:ui-sans-serif,system-ui,-apple-system,Segoe UI,sans-serif;background:#f4f7fb;color:#0f172a;margin:0}header{background:linear-gradient(120deg,#0b3d91,#14532d);color:#fff;padding:20px}h1{margin:0 0 10px 0}nav a{color:#fff;text-decoration:none;margin-right:16px;font-weight:600}main{padding:20px}h2{margin-top:0}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:16px}.card{background:#fff;border:1px solid #dbe3ef;border-radius:12px;padding:16px;box-shadow:0 4px 10px rgba(2,6,23,.06)}table{width:100%;border-collapse:collapse;background:#fff}th,td{border:1px solid #dbe3ef;padding:8px;text-align:left}th{background:#e8eef8}code{white-space:pre-wrap;word-break:break-word}.form-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px}.field{display:flex;flex-direction:column;gap:6px}.field input,.field select{padding:8px;border:1px solid #cbd5e1;border-radius:8px}.actions{margin-top:12px;display:flex;gap:8px}.btn{border:0;border-radius:8px;padding:10px 14px;font-weight:700;cursor:pointer}.btn.primary{background:#14532d;color:#fff}.muted{color:#475569;font-size:.95rem}.badge{display:inline-block;padding:2px 8px;border-radius:999px;font-size:.78rem;font-weight:700;background:#eef2f8;color:#334155}.badge.ok{background:#dcfce7;color:#166534}.badge.warn{background:#fef3c7;color:#92400e}.badge.err{background:#fee2e2;color:#991b1b}.profile-card{display:grid;gap:10px}.profile-flag-list{margin:0;padding-left:20px;display:grid;gap:6px}.profile-flag-list li{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.profile-flag-list code{background:#eef2f8;padding:2px 6px;border-radius:4px}"
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ---------------------------------------------------------------------------
// Project registry — tracks "known projects" on this machine for multi-project
// UI support (v0.13.0). Backed by ~/.the-one/projects.json, updated whenever a
// project is initialized through the broker or visited through the admin UI.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectRegistryEntry {
    pub project_root: String,
    pub project_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub last_seen_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<ProjectRegistryEntry>,
}

impl ProjectRegistry {
    /// Load the registry from `~/.the-one/projects.json`. Returns an empty
    /// registry if the file does not exist yet.
    pub fn load() -> Self {
        let Some(path) = Self::registry_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Save the registry back to disk (best-effort; errors are logged and
    /// swallowed so UI pages never fail because of a missing home dir).
    pub fn save(&self) {
        let Some(path) = Self::registry_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Upsert an entry keyed on `{project_root, project_id}` and bump its
    /// `last_seen_epoch` to now. Called from every admin UI handler so the
    /// list naturally self-populates as the user browses.
    pub fn touch(&mut self, project_root: &str, project_id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Some(entry) = self
            .projects
            .iter_mut()
            .find(|e| e.project_root == project_root && e.project_id == project_id)
        {
            entry.last_seen_epoch = now;
            return;
        }
        self.projects.push(ProjectRegistryEntry {
            project_root: project_root.to_string(),
            project_id: project_id.to_string(),
            label: None,
            last_seen_epoch: now,
        });
    }

    fn registry_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".the-one").join("projects.json"))
    }
}

// ---------------------------------------------------------------------------
// Shared layout helpers
// ---------------------------------------------------------------------------

/// The list of nav items. Exposed as a const so adding a new page only needs
/// one edit here plus the handler itself.
const NAV_ITEMS: &[(&str, &str)] = &[
    ("/", "Home"),
    ("/dashboard", "Dashboard"),
    ("/ingest", "Ingest"),
    ("/graph", "Graph"),
    ("/images", "Images"),
    ("/config", "Config"),
    ("/audit", "Audit"),
    ("/swagger", "API"),
];

/// Render the shared top navigation bar with the active link highlighted and
/// a project switcher dropdown. Used by every admin UI page.
pub fn render_nav(
    active_path: &str,
    current_project_id: &str,
    registry: &ProjectRegistry,
) -> String {
    let mut html = String::from("<nav class=\"topnav\"><div class=\"brand\"><a href=\"/\">the-one-mcp</a></div><div class=\"nav-links\">");
    for (path, label) in NAV_ITEMS {
        let cls = if *path == active_path {
            " class=\"active\""
        } else {
            ""
        };
        html.push_str(&format!("<a href=\"{}\"{}>{}</a>", path, cls, label));
    }
    let current_escaped = html_escape(current_project_id);
    let known_projects = registry.projects.len();
    html.push_str(&format!(
        "</div><div class=\"project-switcher\"><span class=\"label\">Project</span><span class=\"value\">{}</span><span class=\"count\">{} known</span></div></nav>",
        current_escaped, known_projects
    ));
    html
}

/// Wrap a body HTML fragment in the full admin UI page shell with shared
/// styles, head, and nav bar. Callers only have to build their `main` content.
pub fn render_page_shell(
    title: &str,
    active_path: &str,
    current_project_id: &str,
    registry: &ProjectRegistry,
    body: &str,
) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title} — the-one-mcp</title><style>{styles}{shell_styles}</style></head><body>{nav}<main>{body}</main><footer class=\"page-footer\">the-one-mcp · v{version} · <a href=\"https://github.com/michelabboud/the-one-mcp\" target=\"_blank\" rel=\"noopener\">GitHub</a></footer></body></html>",
        title = html_escape(title),
        styles = base_styles(),
        shell_styles = shell_styles(),
        nav = render_nav(active_path, current_project_id, registry),
        body = body,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// Additional styles for the shared page shell (top nav, project switcher,
/// page footer, dark-mode tokens). Stacked on top of `base_styles()`.
fn shell_styles() -> &'static str {
    ":root{--accent:#0b3d91;--accent2:#14532d;--bg-soft:#f4f7fb;--card:#fff;--border:#dbe3ef;--text:#0f172a;--muted:#475569}\
     @media (prefers-color-scheme:dark){:root{--bg-soft:#0b0d11;--card:#131720;--border:#1f2530;--text:#e6e9ef;--muted:#9aa4b5;--accent:#7aa2ff;--accent2:#9bb8ff}}\
     body{background:var(--bg-soft);color:var(--text)}\
     .card{background:var(--card);border-color:var(--border);color:var(--text)}\
     table{background:var(--card);color:var(--text)}th{background:var(--border)}th,td{border-color:var(--border)}\
     nav.topnav{background:linear-gradient(120deg,var(--accent),var(--accent2));color:#fff;padding:10px 20px;display:flex;align-items:center;gap:20px;flex-wrap:wrap;position:sticky;top:0;z-index:10;box-shadow:0 2px 8px rgba(0,0,0,.12)}\
     nav.topnav .brand a{color:#fff;text-decoration:none;font-weight:800;font-size:1.08rem;letter-spacing:-.01em}\
     nav.topnav .nav-links{display:flex;gap:6px;flex:1;flex-wrap:wrap}\
     nav.topnav .nav-links a{color:rgba(255,255,255,.86);text-decoration:none;padding:7px 12px;border-radius:6px;font-weight:500;font-size:.94rem;transition:background .12s}\
     nav.topnav .nav-links a:hover{background:rgba(255,255,255,.12);color:#fff}\
     nav.topnav .nav-links a.active{background:rgba(255,255,255,.2);color:#fff;font-weight:700}\
     nav.topnav .project-switcher{display:flex;align-items:center;gap:8px}\
     nav.topnav .project-switcher .label{font-size:.82rem;color:rgba(255,255,255,.78);text-transform:uppercase;letter-spacing:.04em}\
     nav.topnav .project-switcher .value{padding:6px 10px;border-radius:6px;border:1px solid rgba(255,255,255,.2);background:rgba(255,255,255,.1);color:#fff;font-weight:600;max-width:220px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
     nav.topnav .project-switcher .count{font-size:.78rem;color:rgba(255,255,255,.75)}\
     .page-footer{padding:24px 20px;text-align:center;color:var(--muted);font-size:.88rem;border-top:1px solid var(--border);margin-top:40px}\
     .page-footer a{color:var(--accent);text-decoration:none}\
     .page-footer a:hover{text-decoration:underline}\
     h1{font-size:1.6rem;margin:0 0 6px;letter-spacing:-.01em}\
     h2{font-size:1.25rem;margin:0 0 10px;letter-spacing:-.005em}\
     h3{font-size:1.02rem;margin:0 0 8px}\
     .page-header{margin-bottom:20px}\
     .page-header h1{margin-bottom:4px}\
     .page-header .subtitle{color:var(--muted);font-size:.95rem}\
     .stat-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:14px;margin-bottom:24px}\
     .stat{background:var(--card);border:1px solid var(--border);border-radius:10px;padding:16px}\
     .stat .label{font-size:.78rem;text-transform:uppercase;letter-spacing:.04em;color:var(--muted);margin-bottom:4px}\
     .stat .value{font-size:1.7rem;font-weight:700;line-height:1;letter-spacing:-.02em;color:var(--accent)}\
     .stat .hint{font-size:.82rem;color:var(--muted);margin-top:4px}\
     .empty-state{background:var(--card);border:2px dashed var(--border);border-radius:12px;padding:40px 30px;text-align:center;color:var(--muted)}\
     .empty-state h3{color:var(--text);margin-bottom:8px}\
     .empty-state .cta{margin-top:14px}\
     .bar-chart{display:flex;flex-direction:column;gap:8px;margin:8px 0}\
     .bar-row{display:grid;grid-template-columns:160px 1fr auto;gap:10px;align-items:center;font-size:.88rem}\
     .bar-row .name{color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}\
     .bar-row .track{background:var(--bg-soft);height:14px;border-radius:7px;overflow:hidden;border:1px solid var(--border)}\
     .bar-row .fill{background:linear-gradient(90deg,var(--accent),var(--accent2));height:100%;border-radius:7px}\
     .bar-row .num{font-variant-numeric:tabular-nums;font-weight:600;color:var(--text)}\
     .badge{display:inline-block;padding:2px 8px;border-radius:12px;font-size:.78rem;font-weight:600;background:var(--bg-soft);color:var(--muted);border:1px solid var(--border)}\
     .badge.ok{background:#dcfce7;color:#166534;border-color:#86efac}\
     .badge.warn{background:#fef3c7;color:#92400e;border-color:#fcd34d}\
     .badge.err{background:#fee2e2;color:#991b1b;border-color:#fca5a5}\
     @media (max-width:720px){nav.topnav{flex-direction:column;align-items:flex-start}nav.topnav .nav-links{overflow-x:auto;white-space:nowrap}}"
}

// ---------------------------------------------------------------------------
// Home / landing page (replaces inline render in v0.12.x)
// ---------------------------------------------------------------------------

pub fn render_home_page_v2(
    project_root: &str,
    project_id: &str,
    registry: &ProjectRegistry,
) -> String {
    let project_root_esc = html_escape(project_root);
    let project_id_esc = html_escape(project_id);
    let body = format!(
        r##"<section class="hero card">
  <h1>Semantic memory for your AI coding assistant</h1>
  <p class="lead">A Rust-native MCP broker that gives Claude Code, Gemini CLI, OpenCode, and Codex unlimited, searchable memory of your entire project — docs, code, even screenshots.</p>
  <p class="version-line">Running <code>v{version}</code> · Project <code>{pid}</code> · Root <code>{root}</code></p>
</section>

<section class="grid">
  <article class="card">
    <h3>What this server does</h3>
    <ul class="feature-list">
      <li><strong>17 MCP tools</strong> + 3 resource types (<code>docs</code>, <code>project</code>, <code>catalog</code>)</li>
      <li><strong>Hybrid retrieval</strong> — dense + sparse (SPLADE) + optional cross-encoder rerank</li>
      <li><strong>Graph RAG</strong> (latent) — entity/relation extraction with naive/local/global/hybrid modes</li>
      <li><strong>Tree-sitter code chunker</strong> for 13 languages</li>
      <li><strong>184 curated CLI tools</strong> across 10 languages</li>
      <li><strong>Auto-reindex</strong> watcher for markdown + images</li>
      <li><strong>Backup / restore</strong> via <code>maintain: backup</code></li>
      <li><strong>Observability</strong> — 15 metrics counters via <code>observe</code></li>
    </ul>
  </article>

  <article class="card">
    <h3>Admin sections</h3>
    <ul class="link-list">
      <li><a href="/dashboard">Dashboard</a> — health, metrics, provider pool</li>
      <li><a href="/ingest">Ingest</a> — upload docs, images, code</li>
      <li><a href="/graph">Graph</a> — entity / relation explorer</li>
      <li><a href="/images">Images</a> — indexed image gallery</li>
      <li><a href="/config">Config</a> — 5-layer config viewer + editor</li>
      <li><a href="/audit">Audit</a> — append-only event log</li>
      <li><a href="/swagger">API Docs</a> — OpenAPI / Swagger</li>
      <li><a href="/api/health">/api/health</a> — JSON health probe</li>
    </ul>
  </article>

  <article class="card">
    <h3>Project &amp; community</h3>
    <ul class="link-list">
      <li><a href="https://github.com/michelabboud/the-one-mcp" target="_blank" rel="noopener">GitHub repository</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/issues" target="_blank" rel="noopener">Report an issue</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/pulls" target="_blank" rel="noopener">Pull requests</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/releases" target="_blank" rel="noopener">Releases</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/CHANGELOG.md" target="_blank" rel="noopener">Changelog</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/CONTRIBUTING.md" target="_blank" rel="noopener">Contributing</a></li>
    </ul>
  </article>

  <article class="card">
    <h3>Documentation</h3>
    <ul class="link-list">
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/INSTALL.md" target="_blank" rel="noopener">Install guide</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/quickstart.md" target="_blank" rel="noopener">Quickstart</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/api-reference.md" target="_blank" rel="noopener">API reference</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/mcp-resources.md" target="_blank" rel="noopener">MCP Resources</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/code-chunking.md" target="_blank" rel="noopener">Code chunking</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/hybrid-search.md" target="_blank" rel="noopener">Hybrid search</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/graph-rag.md" target="_blank" rel="noopener">Graph RAG</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/backup-restore.md" target="_blank" rel="noopener">Backup &amp; restore</a></li>
      <li><a href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/observability.md" target="_blank" rel="noopener">Observability</a></li>
    </ul>
  </article>
</section>

<section class="card cta">
  <h3>Quick start</h3>
  <p>Install on a new machine:</p>
  <pre><code>curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash</code></pre>
  <p class="muted">Auto-detects your OS, downloads the right binary, registers with Claude Code / Gemini CLI / OpenCode / Codex.</p>
</section>"##,
        version = env!("CARGO_PKG_VERSION"),
        pid = project_id_esc,
        root = project_root_esc,
    );
    let home_only_styles = "\
        <style>\
        .hero{padding:28px 28px 24px;margin-bottom:20px}\
        .hero h1{font-size:1.9rem;color:var(--accent);margin-bottom:10px}\
        .hero .lead{margin:0 0 14px 0;font-size:1.08rem;color:var(--text);max-width:780px;line-height:1.55}\
        .hero .version-line{margin:0;color:var(--muted);font-size:.9rem}\
        .hero .version-line code{background:var(--bg-soft);padding:2px 6px;border-radius:4px;font-size:.88rem;border:1px solid var(--border)}\
        .feature-list,.link-list{margin:0;padding-left:20px;line-height:1.65}\
        .feature-list li,.link-list li{margin-bottom:4px}\
        .link-list a{color:var(--accent);text-decoration:none}\
        .link-list a:hover{text-decoration:underline}\
        .cta{margin-top:20px}\
        .cta pre{background:#0f172a;color:#e2e8f0;padding:14px 16px;border-radius:8px;overflow-x:auto;margin:8px 0}\
        .cta pre code{background:transparent;color:inherit;padding:0}\
        </style>\
    ";
    let body_with_styles = format!("{home_only_styles}{body}");
    render_page_shell("Home", "/", project_id, registry, &body_with_styles)
}

// ---------------------------------------------------------------------------
// /graph page — entity / relation explorer with Sigma.js viz
// ---------------------------------------------------------------------------

pub fn render_graph_page(
    current_project_id: &str,
    entity_count: usize,
    relation_count: usize,
    top_types: &[(String, usize)],
    registry: &ProjectRegistry,
) -> String {
    let body = if entity_count == 0 {
        String::from(
            r#"<div class="page-header"><h1>Graph RAG Explorer</h1><p class="subtitle">Entity-relation knowledge graph extracted from your project docs.</p></div>
<div class="empty-state">
  <h3>No graph data yet</h3>
  <p>The knowledge graph is empty. Graph RAG extraction runs on top of your existing memory index and identifies entities (people, technologies, concepts) and the relationships between them.</p>
  <p>To populate the graph:</p>
  <ol style="text-align:left;display:inline-block;margin:10px auto">
    <li>Enable extraction in config: <code>graph_enabled: true</code></li>
    <li>Point it at an LLM: set <code>graph_extraction_model</code> and <code>graph_extraction_base_url</code></li>
    <li>Trigger extraction via <code>maintain: action: graph.extract</code></li>
  </ol>
  <p class="cta"><a class="btn primary" href="https://github.com/michelabboud/the-one-mcp/blob/main/docs/guides/graph-rag.md" target="_blank" rel="noopener">Read the Graph RAG guide</a></p>
</div>"#,
        )
    } else {
        let mut top_types_html = String::new();
        let max = top_types.iter().map(|(_, n)| *n).max().unwrap_or(1);
        for (ty, count) in top_types {
            let pct = (*count as f64 / max as f64 * 100.0) as u32;
            top_types_html.push_str(&format!(
                "<div class=\"bar-row\"><span class=\"name\">{}</span><div class=\"track\"><div class=\"fill\" style=\"width:{}%\"></div></div><span class=\"num\">{}</span></div>",
                html_escape(ty), pct, count
            ));
        }
        format!(
            r##"<div class="page-header"><h1>Graph RAG Explorer</h1><p class="subtitle">Entity-relation knowledge graph extracted from your project docs.</p></div>
<div class="stat-grid">
  <div class="stat"><div class="label">Entities</div><div class="value">{entities}</div><div class="hint">Unique entities in the knowledge graph</div></div>
  <div class="stat"><div class="label">Relations</div><div class="value">{relations}</div><div class="hint">Directed edges between entities</div></div>
  <div class="stat"><div class="label">Density</div><div class="value">{density}</div><div class="hint">Avg relations per entity</div></div>
</div>

<div class="grid">
  <article class="card">
    <h3>Top entity types</h3>
    <div class="bar-chart">{types_html}</div>
  </article>
  <article class="card">
    <h3>Graph visualization</h3>
    <p class="muted">Force-directed layout rendered from <code>/api/graph</code>. Click + drag to pan. Zero external dependencies.</p>
    <canvas id="graph-canvas" style="width:100%;height:420px;background:var(--bg-soft);border:1px solid var(--border);border-radius:8px;cursor:grab"></canvas>
    <p class="muted" style="margin-top:10px"><a href="/api/graph">Download raw JSON: /api/graph</a></p>
    <script>
    (function(){{
      var C=document.getElementById('graph-canvas');if(!C)return;
      var ctx=C.getContext('2d');var W,H;
      function resize(){{C.width=C.offsetWidth;C.height=C.offsetHeight||420;W=C.width;H=C.height;}}
      resize();window.addEventListener('resize',resize);
      var palette={{person:'#e11d48',organization:'#2563eb',location:'#059669',technology:'#7c3aed',concept:'#d97706',event:'#db2777'}};
      fetch('/api/graph').then(function(r){{return r.json();}}).then(function(data){{
        if(!data.nodes||data.nodes.length===0){{
          ctx.font='14px sans-serif';ctx.fillStyle='#9aa4b5';ctx.textAlign='center';
          ctx.fillText('No graph data — run graph.extract first',W/2,H/2);return;
        }}
        // Init positions randomly
        var nodes=data.nodes.map(function(n,i){{return{{id:n.id,label:n.label,type:n.type,x:Math.random()*W,y:Math.random()*H,vx:0,vy:0}};}});
        var byId={{}};nodes.forEach(function(n){{byId[n.id]=n;}});
        var edges=data.edges.filter(function(e){{return byId[e.source]&&byId[e.target];}});
        // Force simulation: repulsion + attraction + center gravity
        function tick(){{
          var k=0.01,repel=500,damping=0.85;
          for(var i=0;i<nodes.length;i++){{
            for(var j=i+1;j<nodes.length;j++){{
              var dx=nodes[j].x-nodes[i].x,dy=nodes[j].y-nodes[i].y;
              var d2=dx*dx+dy*dy+1;var f=repel/d2;
              nodes[i].vx-=dx*f;nodes[i].vy-=dy*f;
              nodes[j].vx+=dx*f;nodes[j].vy+=dy*f;
            }}
          }}
          for(var e=0;e<edges.length;e++){{
            var s=byId[edges[e].source],t=byId[edges[e].target];if(!s||!t)continue;
            var dx=t.x-s.x,dy=t.y-s.y,d=Math.sqrt(dx*dx+dy*dy)+1;
            var f=k*(d-120);
            s.vx+=dx/d*f;s.vy+=dy/d*f;
            t.vx-=dx/d*f;t.vy-=dy/d*f;
          }}
          for(var i=0;i<nodes.length;i++){{
            nodes[i].vx+=(W/2-nodes[i].x)*0.001;
            nodes[i].vy+=(H/2-nodes[i].y)*0.001;
            nodes[i].vx*=damping;nodes[i].vy*=damping;
            nodes[i].x+=nodes[i].vx;nodes[i].y+=nodes[i].vy;
            nodes[i].x=Math.max(20,Math.min(W-20,nodes[i].x));
            nodes[i].y=Math.max(20,Math.min(H-20,nodes[i].y));
          }}
        }}
        function draw(){{
          ctx.clearRect(0,0,W,H);
          // Edges
          ctx.strokeStyle='rgba(150,160,180,0.3)';ctx.lineWidth=1;
          for(var e=0;e<edges.length;e++){{
            var s=byId[edges[e].source],t=byId[edges[e].target];if(!s||!t)continue;
            ctx.beginPath();ctx.moveTo(s.x,s.y);ctx.lineTo(t.x,t.y);ctx.stroke();
          }}
          // Nodes
          for(var i=0;i<nodes.length;i++){{
            var n=nodes[i];
            ctx.beginPath();ctx.arc(n.x,n.y,6,0,Math.PI*2);
            ctx.fillStyle=palette[n.type]||'#6b7280';ctx.fill();
            ctx.strokeStyle='rgba(255,255,255,0.6)';ctx.lineWidth=1.5;ctx.stroke();
          }}
          // Labels (only if < 80 nodes to avoid clutter)
          if(nodes.length<80){{
            ctx.font='11px ui-sans-serif,sans-serif';ctx.textAlign='left';ctx.textBaseline='middle';
            for(var i=0;i<nodes.length;i++){{
              var n=nodes[i];
              ctx.fillStyle='var(--text,#0f172a)';
              ctx.fillText(n.label,n.x+10,n.y);
            }}
          }}
        }}
        // Run 200 ticks then render (fast convergence)
        for(var t=0;t<200;t++)tick();
        draw();
        // Optional: continue animating on interaction
        var animating=false;
        C.addEventListener('click',function(){{
          if(animating)return;animating=true;
          var frame=0;
          function animate(){{
            tick();draw();frame++;
            if(frame<60)requestAnimationFrame(animate);else animating=false;
          }}
          animate();
        }});
      }}).catch(function(err){{
        ctx.font='13px sans-serif';ctx.fillStyle='#ef4444';ctx.textAlign='center';
        ctx.fillText('Failed to load graph: '+err.message,W/2,H/2);
      }});
    }})();
    </script>
  </article>
</div>

<section class="card" style="margin-top:20px">
  <h3>Query modes</h3>
  <table>
    <thead><tr><th>Mode</th><th>What it searches</th><th>Best for</th></tr></thead>
    <tbody>
      <tr><td><code>naive</code></td><td>Pure vector search over chunks</td><td>Free-text semantic queries</td></tr>
      <tr><td><code>local</code></td><td>Entity-focused graph walk</td><td>"What is X?" queries targeting specific entities</td></tr>
      <tr><td><code>global</code></td><td>Relation-focused traversal</td><td>"How does X relate to Y?" thematic queries</td></tr>
      <tr><td><code>hybrid</code> (default)</td><td>Vector + graph fused</td><td>General-purpose; best overall quality</td></tr>
    </tbody>
  </table>
</section>"##,
            entities = entity_count,
            relations = relation_count,
            density = if entity_count > 0 {
                format!("{:.1}", relation_count as f64 / entity_count as f64)
            } else {
                "0".to_string()
            },
            types_html = top_types_html,
        )
    };
    render_page_shell("Graph", "/graph", current_project_id, registry, &body)
}

// ---------------------------------------------------------------------------
// /ingest page — multi-format upload UI
// ---------------------------------------------------------------------------

pub fn render_ingest_page(current_project_id: &str, registry: &ProjectRegistry) -> String {
    let body = r##"<div class="page-header"><h1>Ingest content</h1><p class="subtitle">Add documents, code, and images to the project's semantic index.</p></div>

<div class="grid">
  <article class="card">
    <h3>📝 Markdown doc</h3>
    <p class="muted">Save a markdown file into <code>.the-one/docs/</code> and chunk it into the memory index.</p>
    <form id="md-form" class="form-grid">
      <label class="field"><span>Path (relative)</span><input name="path" placeholder="notes/2026-04-06.md" required></label>
      <label class="field"><span>Content</span><textarea name="content" rows="8" style="font-family:ui-monospace,SFMono-Regular,Menlo,monospace;padding:10px;border:1px solid var(--border);border-radius:6px;background:var(--bg-soft);color:var(--text)" required placeholder="# Title&#10;&#10;Body content..."></textarea></label>
      <div class="actions"><button class="btn primary" type="submit">Save &amp; index</button></div>
      <p id="md-result" class="muted"></p>
    </form>
  </article>

  <article class="card">
    <h3>🖼️ Image file</h3>
    <p class="muted">Embed an image path already on disk into the project's image collection. Supports .png, .jpg, .jpeg, .webp.</p>
    <form id="img-form" class="form-grid">
      <label class="field"><span>Absolute or relative path</span><input name="path" placeholder="/abs/path/to/screenshot.png" required></label>
      <label class="field"><span>Caption (optional)</span><input name="caption" placeholder="Architecture diagram v3"></label>
      <div class="actions"><button class="btn primary" type="submit">Embed image</button></div>
      <p id="img-result" class="muted"></p>
    </form>
  </article>

  <article class="card">
    <h3>💻 Code file</h3>
    <p class="muted">Index a source file with the tree-sitter chunker. Supported extensions: <code>.rs .py .ts .tsx .js .jsx .go .c .cpp .java .kt .php .rb .swift .zig</code></p>
    <form id="code-form" class="form-grid">
      <label class="field"><span>Path (relative to project root)</span><input name="path" placeholder="src/main.rs" required></label>
      <label class="field"><span>Content</span><textarea name="content" rows="8" style="font-family:ui-monospace,SFMono-Regular,Menlo,monospace;padding:10px;border:1px solid var(--border);border-radius:6px;background:var(--bg-soft);color:var(--text)" required></textarea></label>
      <div class="actions"><button class="btn primary" type="submit">Chunk &amp; index</button></div>
      <p id="code-result" class="muted"></p>
    </form>
  </article>

  <article class="card">
    <h3>🔄 Reindex project</h3>
    <p class="muted">Force a full re-scan of all managed docs. Useful after a backup restore or when you've edited files outside the watcher's view.</p>
    <div class="actions"><button id="reindex-btn" class="btn primary">Trigger reindex</button></div>
    <p id="reindex-result" class="muted"></p>
  </article>
</div>

<script>
function wire(formId, resultId, payloadFn, endpoint){
  var f=document.getElementById(formId);
  var out=document.getElementById(resultId);
  f.addEventListener('submit',async function(e){
    e.preventDefault();
    out.textContent='Working...';
    try{
      var payload=payloadFn(new FormData(f));
      var res=await fetch(endpoint,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(payload)});
      var body=await res.json().catch(function(){return {};});
      out.textContent=res.ok?('✅ '+(body.message||JSON.stringify(body))):('❌ '+(body.error||('HTTP '+res.status)));
      if(res.ok)f.reset();
    }catch(err){out.textContent='❌ '+err.message;}
  });
}
wire('md-form','md-result',function(d){return {path:d.get('path'),content:d.get('content')};},'/api/ingest/markdown');
wire('img-form','img-result',function(d){return {path:d.get('path'),caption:d.get('caption')||null};},'/api/ingest/image');
wire('code-form','code-result',function(d){return {path:d.get('path'),content:d.get('content')};},'/api/ingest/code');
document.getElementById('reindex-btn').addEventListener('click',async function(){
  var out=document.getElementById('reindex-result');
  out.textContent='Reindexing...';
  try{
    var res=await fetch('/api/ingest/reindex',{method:'POST'});
    var body=await res.json().catch(function(){return {};});
    out.textContent=res.ok?('✅ '+(body.message||'Reindex complete')):('❌ '+(body.error||'failed'));
  }catch(e){out.textContent='❌ '+e.message;}
});
</script>"##;
    render_page_shell("Ingest", "/ingest", current_project_id, registry, body)
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
        .route("/", get(home_handler))
        .route("/dashboard", get(dashboard_handler))
        .route("/audit", get(audit_page_handler))
        .route("/config", get(config_page_handler))
        .route("/swagger", get(swagger_ui_page_handler))
        .route("/images", get(images_page_handler))
        .route("/ingest", get(ingest_page_handler))
        .route("/graph", get(graph_page_handler))
        .route("/images/thumbnail/{hash}", get(images_thumbnail_handler))
        .route("/api/images", get(api_images_handler))
        .route("/api/health", get(health_handler))
        .route("/api/swagger", get(swagger_handler))
        .route("/api/config", post(config_update_handler))
        .route("/api/projects", get(api_projects_handler))
        .route("/api/models", get(api_models_handler))
        .route("/api/graph", get(api_graph_handler))
        .route("/api/ingest/markdown", post(api_ingest_markdown_handler))
        .route("/api/ingest/image", post(api_ingest_image_handler))
        .route("/api/ingest/code", post(api_ingest_code_handler))
        .route("/api/ingest/reindex", post(api_ingest_reindex_handler))
        .route("/api/graph/extract", post(api_graph_extract_handler))
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

async fn home_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let mut registry = ProjectRegistry::load();
    registry.touch(&state.project_root.display().to_string(), &state.project_id);
    registry.save();
    Html(render_home_page_v2(
        &state.project_root.display().to_string(),
        &state.project_id,
        &registry,
    ))
    .into_response()
}

async fn ingest_page_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let registry = ProjectRegistry::load();
    Html(render_ingest_page(&state.project_id, &registry)).into_response()
}

async fn graph_page_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let registry = ProjectRegistry::load();
    // Load knowledge graph stats from the persisted file if any.
    let (entity_count, relation_count, top_types) = load_graph_stats(&state.project_root);
    Html(render_graph_page(
        &state.project_id,
        entity_count,
        relation_count,
        &top_types,
        &registry,
    ))
    .into_response()
}

/// Read `<project_root>/.the-one/knowledge_graph.json` if present and return
/// (entity_count, relation_count, top_entity_types). Safe fallback to zeroes
/// on any error — the graph file is optional and may not exist yet.
fn load_graph_stats(project_root: &Path) -> (usize, usize, Vec<(String, usize)>) {
    let graph_path = project_root.join(".the-one").join("knowledge_graph.json");
    let Ok(text) = std::fs::read_to_string(&graph_path) else {
        return (0, 0, Vec::new());
    };
    let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
        return (0, 0, Vec::new());
    };
    let entities = value
        .get("entities")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let relations = value
        .get("relations")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let entity_count = entities.len();

    // Count entity types and return top 8
    let mut type_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for e in &entities {
        if let Some(ty) = e.get("entity_type").and_then(|v| v.as_str()) {
            *type_counts.entry(ty.to_string()).or_insert(0) += 1;
        }
    }
    let mut top: Vec<(String, usize)> = type_counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(8);

    (entity_count, relations, top)
}

// ---------------------------------------------------------------------------
// API handlers for the multi-project + ingest + graph UI features
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ProjectsListResponse {
    projects: Vec<ProjectRegistryEntry>,
    current_project_id: String,
}

async fn api_projects_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let registry = ProjectRegistry::load();
    Json(ProjectsListResponse {
        projects: registry.projects,
        current_project_id: state.project_id.clone(),
    })
}

#[derive(Serialize)]
struct ModelEntry {
    id: String,
    name: String,
    tier: String,
    dims: usize,
    size_mb: usize,
    description: String,
}

#[derive(Serialize)]
struct ModelsResponse {
    local: Vec<ModelEntry>,
    current_embedding_model: String,
}

async fn api_models_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    use the_one_core::config::{AppConfig, RuntimeOverrides};
    use the_one_memory::models_registry::list_local_models;
    let models = list_local_models();
    let local: Vec<ModelEntry> = models
        .into_iter()
        .map(|m| ModelEntry {
            // LocalModel doesn't carry a separate id field; the `name` field
            // (e.g. "all-MiniLM-L6-v2") IS the identifier used throughout the
            // config + embedding provider layers.
            id: m.name.clone(),
            name: m.name,
            tier: m.tier,
            dims: m.dims,
            size_mb: m.size_mb as usize,
            description: m.description,
        })
        .collect();
    // Read the active embedding model directly from the resolved AppConfig —
    // ConfigExportResponse doesn't include it yet, and we want the same
    // 5-layer resolution as everything else.
    let current = AppConfig::load(&state.project_root, RuntimeOverrides::default())
        .map(|c| c.embedding_model)
        .unwrap_or_default();
    Json(ModelsResponse {
        local,
        current_embedding_model: current,
    })
}

#[derive(Serialize, Deserialize)]
struct GraphNode {
    id: String,
    label: String,
    #[serde(rename = "type")]
    node_type: String,
}

#[derive(Serialize, Deserialize)]
struct GraphEdge {
    source: String,
    target: String,
    #[serde(rename = "type")]
    edge_type: String,
}

#[derive(Serialize)]
struct GraphJsonResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    entity_count: usize,
    relation_count: usize,
}

async fn api_graph_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let graph_path = state
        .project_root
        .join(".the-one")
        .join("knowledge_graph.json");
    let text = match std::fs::read_to_string(&graph_path) {
        Ok(t) => t,
        Err(_) => {
            return Json(GraphJsonResponse {
                nodes: Vec::new(),
                edges: Vec::new(),
                entity_count: 0,
                relation_count: 0,
            })
            .into_response();
        }
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let entities = value
        .get("entities")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let relations = value
        .get("relations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let nodes: Vec<GraphNode> = entities
        .iter()
        .filter_map(|e| {
            Some(GraphNode {
                id: e.get("name")?.as_str()?.to_string(),
                label: e.get("name")?.as_str()?.to_string(),
                node_type: e
                    .get("entity_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("entity")
                    .to_string(),
            })
        })
        .collect();
    let edges: Vec<GraphEdge> = relations
        .iter()
        .filter_map(|r| {
            Some(GraphEdge {
                source: r.get("source")?.as_str()?.to_string(),
                target: r.get("target")?.as_str()?.to_string(),
                edge_type: r
                    .get("relation_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("relation")
                    .to_string(),
            })
        })
        .collect();

    Json(GraphJsonResponse {
        entity_count: nodes.len(),
        relation_count: edges.len(),
        nodes,
        edges,
    })
    .into_response()
}

// ---- Ingest API handlers --------------------------------------------------

#[derive(Deserialize)]
struct IngestMarkdownPayload {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct IngestImagePayload {
    path: String,
    caption: Option<String>,
}

#[derive(Deserialize)]
struct IngestCodePayload {
    path: String,
    content: String,
}

#[derive(Serialize)]
struct ApiMessageResponse {
    message: String,
}

async fn api_ingest_markdown_handler(
    State(state): State<EmbeddedUiState>,
    Json(payload): Json<IngestMarkdownPayload>,
) -> impl IntoResponse {
    // Safety: reject absolute paths and ..
    let path = payload.path.trim();
    if path.is_empty() || path.contains("..") || Path::new(path).is_absolute() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid path"})),
        )
            .into_response();
    }
    let docs_dir = state.project_root.join(".the-one").join("docs");
    let full = docs_dir.join(path);
    if let Some(parent) = full.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("create dir: {e}")})),
            )
                .into_response();
        }
    }
    if let Err(e) = std::fs::write(&full, payload.content.as_bytes()) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("write: {e}")})),
        )
            .into_response();
    }
    Json(ApiMessageResponse {
        message: format!(
            "Saved to {}. Watcher will index it shortly (or run reindex).",
            full.display()
        ),
    })
    .into_response()
}

async fn api_ingest_image_handler(
    State(state): State<EmbeddedUiState>,
    Json(payload): Json<IngestImagePayload>,
) -> impl IntoResponse {
    use the_one_mcp::api::ImageIngestRequest;
    match state
        .admin
        .broker()
        .image_ingest(ImageIngestRequest {
            project_root: state.project_root.display().to_string(),
            project_id: state.project_id.clone(),
            path: payload.path.clone(),
            caption: payload.caption,
        })
        .await
    {
        Ok(resp) => Json(ApiMessageResponse {
            message: format!(
                "Embedded {} (dims={}, ocr={}, thumb={})",
                resp.path, resp.dims, resp.ocr_extracted, resp.thumbnail_generated
            ),
        })
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_ingest_code_handler(
    State(state): State<EmbeddedUiState>,
    Json(payload): Json<IngestCodePayload>,
) -> impl IntoResponse {
    // Code ingest reuses the docs.save + reindex path. We write the file into
    // .the-one/docs/code/<path> so it gets picked up by the chunker dispatcher
    // (which routes .rs/.py/etc through the tree-sitter chunker automatically).
    let path = payload.path.trim();
    if path.is_empty() || path.contains("..") || Path::new(path).is_absolute() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid path"})),
        )
            .into_response();
    }
    let target = state
        .project_root
        .join(".the-one")
        .join("docs")
        .join("code")
        .join(path);
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("create dir: {e}")})),
            )
                .into_response();
        }
    }
    if let Err(e) = std::fs::write(&target, payload.content.as_bytes()) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("write: {e}")})),
        )
            .into_response();
    }
    Json(ApiMessageResponse {
        message: format!(
            "Saved {} for tree-sitter chunking. Run reindex to pick it up.",
            target.display()
        ),
    })
    .into_response()
}

async fn api_graph_extract_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    match state
        .admin
        .broker()
        .graph_extract(&state.project_root, &state.project_id)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_ingest_reindex_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    use the_one_mcp::api::DocsReindexRequest;
    match state
        .admin
        .broker()
        .docs_reindex(DocsReindexRequest {
            project_root: state.project_root.display().to_string(),
            project_id: state.project_id.clone(),
        })
        .await
    {
        Ok(resp) => Json(ApiMessageResponse {
            message: format!(
                "Reindex complete: {} new, {} updated, {} removed, {} unchanged",
                resp.new, resp.updated, resp.removed, resp.unchanged
            ),
        })
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn dashboard_handler(State(state): State<EmbeddedUiState>) -> impl IntoResponse {
    let registry = ProjectRegistry::load();
    match state
        .admin
        .health_report(&state.project_root, &state.project_id)
        .await
    {
        Ok(report) => {
            let (graph_entities, graph_relations, _) = load_graph_stats(&state.project_root);
            Html(render_dashboard_page_v2(
                &state.project_id,
                &report,
                graph_entities,
                graph_relations,
                &registry,
            ))
            .into_response()
        }
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("dashboard error: {err}"),
        )
            .into_response(),
    }
}

/// LightRAG-inspired dashboard with stat grid, metrics bar chart, and graph
/// summary. Replaces the v0.12.x 4-card format.
pub fn render_dashboard_page_v2(
    current_project_id: &str,
    report: &AdminHealthReport,
    graph_entities: usize,
    graph_relations: usize,
    registry: &ProjectRegistry,
) -> String {
    let m = &report.metrics;
    let c = &report.config;

    let counters: Vec<(&str, u64)> = vec![
        ("memory.search", m.memory_search_calls),
        ("tool.run", m.tool_run_calls),
        ("project.init", m.project_init_calls),
        ("project.refresh", m.project_refresh_calls),
        ("image.search", m.image_search_calls),
        ("image.ingest", m.image_ingest_calls),
        ("resources.list", m.resources_list_calls),
        ("resources.read", m.resources_read_calls),
    ];
    let max_counter = counters.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
    let mut bar_chart = String::new();
    for (name, val) in &counters {
        let pct = (*val as f64 / max_counter as f64 * 100.0) as u32;
        bar_chart.push_str(&format!(
            "<div class=\"bar-row\"><span class=\"name\">{}</span><div class=\"track\"><div class=\"fill\" style=\"width:{}%\"></div></div><span class=\"num\">{}</span></div>",
            name, pct, val
        ));
    }

    let watcher_total = m.watcher_events_processed + m.watcher_events_failed;
    let watcher_badge = if watcher_total == 0 {
        "<span class=\"badge\">idle</span>"
    } else if m.watcher_events_failed == 0 {
        "<span class=\"badge ok\">healthy</span>"
    } else if m.watcher_events_failed * 10 < m.watcher_events_processed {
        "<span class=\"badge warn\">some failures</span>"
    } else {
        "<span class=\"badge err\">degraded</span>"
    };
    let qdrant_badge = if m.qdrant_errors == 0 {
        "<span class=\"badge ok\">ok</span>"
    } else {
        "<span class=\"badge err\">errors</span>"
    };
    let graph_badge = if graph_entities == 0 {
        "<span class=\"badge\">empty</span>"
    } else {
        "<span class=\"badge ok\">populated</span>"
    };

    let body = format!(
        r##"<div class="page-header"><h1>Dashboard</h1><p class="subtitle">Health, metrics, and feature status for project <code>{pid}</code></p></div>

<div class="stat-grid">
  <div class="stat"><div class="label">Memory searches</div><div class="value">{search_calls}</div><div class="hint">Avg latency: {avg_ms} ms</div></div>
  <div class="stat"><div class="label">Tool runs</div><div class="value">{tool_runs}</div><div class="hint">Total invocations since startup</div></div>
  <div class="stat"><div class="label">Graph entities</div><div class="value">{graph_e}</div><div class="hint">{graph_badge} · {graph_r} relations</div></div>
  <div class="stat"><div class="label">Watcher events</div><div class="value">{watcher_ok}</div><div class="hint">{watcher_badge} · {watcher_fail} failed</div></div>
  <div class="stat"><div class="label">Qdrant errors</div><div class="value">{qdrant_errs}</div><div class="hint">{qdrant_badge}</div></div>
  <div class="stat"><div class="label">Audit events</div><div class="value">{audit}</div><div class="hint">Recent events in the log</div></div>
</div>

<div class="grid">
  <article class="card">
    <h3>Tool call distribution</h3>
    <p class="muted">Relative invocation counts since the broker started. Resets on restart.</p>
    <div class="bar-chart">{bar_chart}</div>
  </article>

  <article class="card">
    <h3>Runtime config</h3>
    <table>
      <tr><th>Provider</th><td>{provider}</td></tr>
      <tr><th>Nano provider</th><td>{nano_provider}</td></tr>
      <tr><th>Nano model</th><td>{nano_model}</td></tr>
      <tr><th>Qdrant URL</th><td><code>{qdrant_url}</code></td></tr>
      <tr><th>Qdrant auth</th><td>{qdrant_auth}</td></tr>
    </table>
    <p class="muted" style="margin-top:12px"><a href="/config">Edit config →</a></p>
  </article>

  <article class="card" id="embedding-card">
    <h3>Embedding model</h3>
    <p class="muted">Active local or API embedding model for this project.</p>
    <div id="embedding-current" style="font-weight:700;font-size:1.1rem;color:var(--accent);margin:8px 0">Loading…</div>
    <p class="muted"><a href="/api/models">See all available models</a></p>
  </article>

  <article class="card">
    <h3>Graph RAG status</h3>
    <table>
      <tr><th>Enabled</th><td id="graph-enabled-cell">—</td></tr>
      <tr><th>Entities</th><td>{graph_e}</td></tr>
      <tr><th>Relations</th><td>{graph_r}</td></tr>
    </table>
    <p class="muted" style="margin-top:12px"><a href="/graph">Open graph explorer →</a></p>
  </article>
</div>

<script>
// Populate via DOM methods only — no innerHTML with dynamic data.
fetch('/api/models').then(function(r){{return r.json();}}).then(function(j){{
  var el=document.getElementById('embedding-current');
  if(el){{el.textContent=j.current_embedding_model||'(none configured)';}}
}}).catch(function(){{}});
fetch('/api/graph').then(function(r){{return r.json();}}).then(function(j){{
  var cell=document.getElementById('graph-enabled-cell');
  if(!cell)return;
  var badge=document.createElement('span');
  badge.className='badge '+(j.entity_count>0?'ok':'');
  badge.textContent=(j.entity_count>0)?'populated':'empty';
  cell.textContent='';
  cell.appendChild(badge);
}}).catch(function(){{}});
</script>"##,
        pid = html_escape(current_project_id),
        search_calls = m.memory_search_calls,
        avg_ms = m.memory_search_latency_avg_ms,
        tool_runs = m.tool_run_calls,
        graph_e = graph_entities,
        graph_r = graph_relations,
        graph_badge = graph_badge,
        watcher_ok = m.watcher_events_processed,
        watcher_fail = m.watcher_events_failed,
        watcher_badge = watcher_badge,
        qdrant_errs = m.qdrant_errors,
        qdrant_badge = qdrant_badge,
        audit = report.recent_audit_events,
        bar_chart = bar_chart,
        provider = html_escape(&c.provider),
        nano_provider = html_escape(&c.nano_provider),
        nano_model = html_escape(&c.nano_model),
        qdrant_url = html_escape(&c.qdrant_url),
        qdrant_auth = if c.qdrant_auth_configured {
            "configured"
        } else {
            "none"
        },
    );
    render_page_shell(
        "Dashboard",
        "/dashboard",
        current_project_id,
        registry,
        &body,
    )
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
    let runtime_config = match AppConfig::load(&state.project_root, RuntimeOverrides::default()) {
        Ok(config) => config,
        Err(err) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("config load error: {err}"),
            )
                .into_response();
        }
    };

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
            let profile_card = render_memory_palace_profile_card(&runtime_config);
            let profile_select = render_memory_palace_profile_select(&runtime_config);
            let limits_form = render_limits_form(&runtime_config);
            let config_save_script = render_config_save_script(&runtime_config);

            Html(format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Config</title><style>{}</style></head><body><header><h1>Runtime Config</h1><nav><a href=\"/dashboard\">Dashboard</a><a href=\"/config\">Config</a><a href=\"/audit\">Audit</a><a href=\"/images\">Images</a><a href=\"/swagger\">Swagger</a></nav></header><main><p class=\"muted\">Edit and save project configuration. Environment variables still override these values.</p>{profile_card}<form id=\"cfg\" class=\"card\"><div class=\"form-grid\"><label class=\"field\">Provider<select name=\"provider\"><option value=\"local\" {}>local</option><option value=\"hosted\" {}>hosted</option></select></label><label class=\"field\">Nano Provider<select name=\"nano_provider\"><option value=\"rules\" {}>rules</option><option value=\"api\" {}>api</option><option value=\"ollama\" {}>ollama</option><option value=\"lmstudio\" {}>lmstudio</option></select></label><label class=\"field\">Nano Model<input name=\"nano_model\" value=\"{}\"></label><label class=\"field\">Qdrant URL<input name=\"qdrant_url\" value=\"{}\"></label><label class=\"field\">Qdrant API Key<input name=\"qdrant_api_key\" value=\"\" placeholder=\"leave empty to keep current\"></label><label class=\"field\">Qdrant CA Cert Path<input name=\"qdrant_ca_cert_path\" value=\"{}\"></label><label class=\"field\">Qdrant Strict Auth<input type=\"checkbox\" name=\"qdrant_strict_auth\" {}></label><label class=\"field\">Qdrant TLS Insecure<input type=\"checkbox\" name=\"qdrant_tls_insecure\" {}></label></div><h3 style=\"margin-top:16px\">MemPalace</h3><p class=\"muted\">Switch the conversation memory preset with one save.</p>{profile_select}<h3 style=\"margin-top:16px\">Limits</h3><p class=\"muted\">Configurable limits with validation bounds. Environment variables override these values.</p>{limits_form}<div class=\"actions\"><button class=\"btn primary\" type=\"submit\">Save Config</button></div><p id=\"result\" class=\"muted\"></p></form></main>{config_save_script}</body></html>",
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
                profile_card = profile_card,
                profile_select = profile_select,
                limits_form = limits_form,
                config_save_script = config_save_script,
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

    let memory_palace_profile = payload
        .memory_palace_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(|profile| profile.to_string());

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
            memory_palace_profile,
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

    /// Passthrough accessor to the underlying broker. Used by the
    /// v0.13.0 ingest / graph UI handlers that need to call broker
    /// methods directly.
    pub fn broker(&self) -> &McpBroker {
        &self.broker
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
        assert!(html.contains("Dashboard"));
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
    async fn test_mempalace_profile_config_view_reflects_active_preset() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

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
        let config_html = client
            .get(format!("http://{}/config", runtime.listen_addr))
            .send()
            .await
            .expect("config request should work")
            .text()
            .await
            .expect("config body should read");

        assert!(
            config_html.contains("MemPalace profile"),
            "config page should show the profile control card"
        );
        assert!(
            config_html.contains("Active preset"),
            "config page should expose the active profile summary"
        );
        assert!(
            config_html.contains("core"),
            "default MemPalace preset should resolve to core"
        );
        assert!(
            config_html.contains("memory_palace_enabled"),
            "config page should show expanded flag state"
        );

        let runtime_config = the_one_core::config::AppConfig::load(
            &project,
            the_one_core::config::RuntimeOverrides::default(),
        )
        .expect("config should load");
        assert!(runtime_config.memory_palace_enabled);
        assert!(!runtime_config.memory_palace_hooks_enabled);
        assert!(!runtime_config.memory_palace_aaak_enabled);
        assert!(!runtime_config.memory_palace_diary_enabled);
        assert!(!runtime_config.memory_palace_navigation_enabled);

        runtime.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_mempalace_profile_api_update_switches_preset() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

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
        let update = client
            .post(format!("http://{}/api/config", runtime.listen_addr))
            .json(&serde_json::json!({
                "memory_palace_profile": "full"
            }))
            .send()
            .await
            .expect("config update should work")
            .json::<serde_json::Value>()
            .await
            .expect("config update body should parse");
        assert_eq!(update["saved"], true);

        let runtime_config = the_one_core::config::AppConfig::load(
            &project,
            the_one_core::config::RuntimeOverrides::default(),
        )
        .expect("config should load");
        assert!(runtime_config.memory_palace_enabled);
        assert!(runtime_config.memory_palace_hooks_enabled);
        assert!(runtime_config.memory_palace_aaak_enabled);
        assert!(runtime_config.memory_palace_diary_enabled);
        assert!(runtime_config.memory_palace_navigation_enabled);

        let config_html = client
            .get(format!("http://{}/config", runtime.listen_addr))
            .send()
            .await
            .expect("config request should work")
            .text()
            .await
            .expect("config body should read");

        assert!(
            config_html.contains("Active preset"),
            "config page should expose the active profile summary"
        );
        assert!(
            config_html.contains(">full<"),
            "config page should show the active full profile"
        );
        assert!(
            config_html.contains("memory_palace_navigation_enabled"),
            "config page should reflect the expanded flag state"
        );

        runtime.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_mempalace_custom_profile_survives_unrelated_save() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

        let custom_update = super::ProjectConfigUpdate {
            memory_palace_enabled: Some(true),
            memory_palace_hooks_enabled: Some(true),
            memory_palace_aaak_enabled: Some(false),
            memory_palace_diary_enabled: Some(false),
            memory_palace_navigation_enabled: Some(false),
            ..Default::default()
        };
        super::update_project_config(&project, custom_update)
            .expect("custom profile update should work");

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
        let config_html = client
            .get(format!("http://{}/config", runtime.listen_addr))
            .send()
            .await
            .expect("config request should work")
            .text()
            .await
            .expect("config body should read");
        assert!(
            config_html.contains("Active preset: <span class=\"badge warn\">custom</span>"),
            "custom MemPalace state should be visible in the config card"
        );
        assert!(
            config_html.contains("preserve current custom state"),
            "custom MemPalace state should preserve the profile selector"
        );

        let update = client
            .post(format!("http://{}/api/config", runtime.listen_addr))
            .json(&serde_json::json!({
                "provider": "hosted"
            }))
            .send()
            .await
            .expect("config update should work")
            .json::<serde_json::Value>()
            .await
            .expect("config update body should parse");
        assert_eq!(update["saved"], true);

        let runtime_config = the_one_core::config::AppConfig::load(
            &project,
            the_one_core::config::RuntimeOverrides::default(),
        )
        .expect("config should load");
        assert!(runtime_config.memory_palace_enabled);
        assert!(runtime_config.memory_palace_hooks_enabled);
        assert!(!runtime_config.memory_palace_aaak_enabled);
        assert!(!runtime_config.memory_palace_diary_enabled);
        assert!(!runtime_config.memory_palace_navigation_enabled);

        runtime.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_mempalace_limits_render_and_survive_unrelated_save() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("project dir should exist");
        fs::write(project.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should work");

        let custom_limits = the_one_core::limits::ConfigurableLimits {
            max_search_hits: 17,
            max_nano_retries: 0,
            search_score_threshold: 0.0,
            ..Default::default()
        };

        let limit_update = super::ProjectConfigUpdate {
            limits: Some(custom_limits.clone()),
            ..Default::default()
        };
        super::update_project_config(&project, limit_update).expect("limit update should work");

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
        let config_html = client
            .get(format!("http://{}/config", runtime.listen_addr))
            .send()
            .await
            .expect("config request should work")
            .text()
            .await
            .expect("config body should read");
        assert!(
            config_html.contains("name=\"limit_max_search_hits\" value=\"17\""),
            "config page should render the current non-default integer limit"
        );
        assert!(
            config_html.contains("name=\"limit_max_nano_retries\" value=\"0\""),
            "config page should render zero-valued limits instead of defaulting them"
        );
        assert!(
            config_html.contains("name=\"limit_search_score_threshold\" value=\"0\""),
            "config page should render zero-valued float limits instead of defaulting them"
        );

        let update = client
            .post(format!("http://{}/api/config", runtime.listen_addr))
            .json(&serde_json::json!({
                "provider": "hosted"
            }))
            .send()
            .await
            .expect("config update should work")
            .json::<serde_json::Value>()
            .await
            .expect("config update body should parse");
        assert_eq!(update["saved"], true);

        let runtime_config = the_one_core::config::AppConfig::load(
            &project,
            the_one_core::config::RuntimeOverrides::default(),
        )
        .expect("config should load");
        assert_eq!(runtime_config.limits.max_search_hits, 17);
        assert_eq!(runtime_config.limits.max_nano_retries, 0);
        assert!((runtime_config.limits.search_score_threshold - 0.0).abs() < f32::EPSILON);

        runtime.shutdown();
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
        assert!(dashboard.contains("Dashboard"));

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
