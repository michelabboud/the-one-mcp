use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::limits::ConfigurableLimits;

const DEFAULT_PROVIDER: &str = "local";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_QDRANT_URL: &str = "http://127.0.0.1:6334";
const DEFAULT_NANO_PROVIDER: &str = "rules";
const DEFAULT_NANO_MODEL: &str = "none";
const DEFAULT_QDRANT_TLS_INSECURE: bool = false;
const DEFAULT_QDRANT_STRICT_AUTH: bool = true;
const DEFAULT_EMBEDDING_PROVIDER: &str = "local";
const DEFAULT_EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";
const DEFAULT_EMBEDDING_DIMENSIONS: usize = 384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NanoProviderKind {
    RulesOnly,
    Api,
    Ollama,
    LmStudio,
}

impl NanoProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RulesOnly => "rules",
            Self::Api => "api",
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
        }
    }
}

impl FromStr for NanoProviderKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "api" => Ok(Self::Api),
            "ollama" => Ok(Self::Ollama),
            "lmstudio" => Ok(Self::LmStudio),
            "rules" => Ok(Self::RulesOnly),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NanoProviderEntry {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NanoRoutingPolicy {
    #[default]
    #[serde(rename = "priority")]
    Priority,
    #[serde(rename = "round_robin")]
    RoundRobin,
    #[serde(rename = "latency")]
    Latency,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    pub provider: Option<String>,
    pub log_level: Option<String>,
    pub qdrant_url: Option<String>,
    pub nano_provider: Option<String>,
    pub nano_model: Option<String>,
    pub qdrant_api_key: Option<String>,
    pub qdrant_ca_cert_path: Option<String>,
    pub qdrant_tls_insecure: Option<bool>,
    pub qdrant_strict_auth: Option<bool>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_api_base_url: Option<String>,
    pub embedding_api_key: Option<String>,
    pub embedding_dimensions: Option<usize>,
    pub nano_providers: Option<Vec<NanoProviderEntry>>,
    pub nano_routing_policy: Option<NanoRoutingPolicy>,
    pub external_docs_root: Option<String>,
    pub limits: Option<ConfigurableLimits>,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectConfigUpdate {
    pub provider: Option<String>,
    pub log_level: Option<String>,
    pub qdrant_url: Option<String>,
    pub nano_provider: Option<String>,
    pub nano_model: Option<String>,
    pub qdrant_api_key: Option<String>,
    pub qdrant_ca_cert_path: Option<String>,
    pub qdrant_tls_insecure: Option<bool>,
    pub qdrant_strict_auth: Option<bool>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_api_base_url: Option<String>,
    pub embedding_api_key: Option<String>,
    pub embedding_dimensions: Option<usize>,
    pub nano_providers: Option<Vec<NanoProviderEntry>>,
    pub nano_routing_policy: Option<NanoRoutingPolicy>,
    pub external_docs_root: Option<String>,
    pub limits: Option<ConfigurableLimits>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub project_root: PathBuf,
    pub project_state_dir: PathBuf,
    pub global_state_dir: PathBuf,
    pub provider: String,
    pub log_level: String,
    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_ca_cert_path: Option<PathBuf>,
    pub qdrant_tls_insecure: bool,
    pub qdrant_strict_auth: bool,
    pub nano_provider: NanoProviderKind,
    pub nano_model: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_api_base_url: Option<String>,
    pub embedding_api_key: Option<String>,
    pub embedding_dimensions: usize,
    pub nano_providers: Vec<NanoProviderEntry>,
    pub nano_routing_policy: NanoRoutingPolicy,
    pub external_docs_root: Option<PathBuf>,
    pub limits: ConfigurableLimits,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct FileConfig {
    provider: Option<String>,
    log_level: Option<String>,
    qdrant_url: Option<String>,
    qdrant_api_key: Option<String>,
    qdrant_ca_cert_path: Option<String>,
    qdrant_tls_insecure: Option<bool>,
    qdrant_strict_auth: Option<bool>,
    nano_provider: Option<String>,
    nano_model: Option<String>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    embedding_api_base_url: Option<String>,
    embedding_api_key: Option<String>,
    embedding_dimensions: Option<usize>,
    nano_providers: Option<Vec<NanoProviderEntry>>,
    nano_routing_policy: Option<NanoRoutingPolicy>,
    external_docs_root: Option<String>,
    limits: Option<ConfigurableLimits>,
}

impl AppConfig {
    pub fn load(project_root: &Path, runtime: RuntimeOverrides) -> Result<Self, CoreError> {
        let normalized_project_root = normalize_existing_dir(project_root)?;
        let global_state_dir = global_state_dir()?;
        let project_state_dir = normalized_project_root.join(".the-one");

        let mut merged = FileConfig {
            provider: Some(DEFAULT_PROVIDER.to_string()),
            log_level: Some(DEFAULT_LOG_LEVEL.to_string()),
            qdrant_url: Some(DEFAULT_QDRANT_URL.to_string()),
            qdrant_api_key: None,
            qdrant_ca_cert_path: None,
            qdrant_tls_insecure: Some(DEFAULT_QDRANT_TLS_INSECURE),
            qdrant_strict_auth: Some(DEFAULT_QDRANT_STRICT_AUTH),
            nano_provider: Some(DEFAULT_NANO_PROVIDER.to_string()),
            nano_model: Some(DEFAULT_NANO_MODEL.to_string()),
            embedding_provider: Some(DEFAULT_EMBEDDING_PROVIDER.to_string()),
            embedding_model: Some(DEFAULT_EMBEDDING_MODEL.to_string()),
            embedding_api_base_url: None,
            embedding_api_key: None,
            embedding_dimensions: Some(DEFAULT_EMBEDDING_DIMENSIONS),
            nano_providers: None,
            nano_routing_policy: None,
            external_docs_root: None,
            limits: None,
        };

        apply_file_layer(&global_state_dir.join("config.json"), &mut merged)?;
        apply_file_layer(&project_state_dir.join("config.json"), &mut merged)?;
        apply_env_layer(&mut merged);
        apply_runtime_layer(runtime, &mut merged);

        Ok(Self {
            project_root: normalized_project_root,
            project_state_dir,
            global_state_dir,
            provider: merged
                .provider
                .unwrap_or_else(|| DEFAULT_PROVIDER.to_string()),
            log_level: merged
                .log_level
                .unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string()),
            qdrant_url: merged
                .qdrant_url
                .unwrap_or_else(|| DEFAULT_QDRANT_URL.to_string()),
            qdrant_api_key: merged.qdrant_api_key,
            qdrant_ca_cert_path: merged.qdrant_ca_cert_path.map(PathBuf::from),
            qdrant_tls_insecure: merged
                .qdrant_tls_insecure
                .unwrap_or(DEFAULT_QDRANT_TLS_INSECURE),
            qdrant_strict_auth: merged
                .qdrant_strict_auth
                .unwrap_or(DEFAULT_QDRANT_STRICT_AUTH),
            nano_provider: merged
                .nano_provider
                .as_deref()
                .unwrap_or(DEFAULT_NANO_PROVIDER)
                .parse::<NanoProviderKind>()
                .unwrap_or(NanoProviderKind::RulesOnly),
            nano_model: merged
                .nano_model
                .unwrap_or_else(|| DEFAULT_NANO_MODEL.to_string()),
            embedding_provider: merged
                .embedding_provider
                .unwrap_or_else(|| DEFAULT_EMBEDDING_PROVIDER.to_string()),
            embedding_model: merged
                .embedding_model
                .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string()),
            embedding_api_base_url: merged.embedding_api_base_url,
            embedding_api_key: merged.embedding_api_key,
            embedding_dimensions: merged
                .embedding_dimensions
                .unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS),
            nano_providers: merged.nano_providers.unwrap_or_default(),
            nano_routing_policy: merged.nano_routing_policy.unwrap_or_default(),
            external_docs_root: merged.external_docs_root.map(PathBuf::from),
            limits: merged.limits.map(|l| l.validated()).unwrap_or_default(),
        })
    }
}

pub fn update_project_config(
    project_root: &Path,
    update: ProjectConfigUpdate,
) -> Result<PathBuf, CoreError> {
    let normalized_project_root = normalize_existing_dir(project_root)?;
    let project_state_dir = normalized_project_root.join(".the-one");
    fs::create_dir_all(&project_state_dir)?;
    let config_path = project_state_dir.join("config.json");

    let mut merged = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        serde_json::from_str::<FileConfig>(&content)?
    } else {
        FileConfig::default()
    };

    if update.provider.is_some() {
        merged.provider = update.provider;
    }
    if update.log_level.is_some() {
        merged.log_level = update.log_level;
    }
    if update.qdrant_url.is_some() {
        merged.qdrant_url = update.qdrant_url;
    }
    if update.qdrant_api_key.is_some() {
        merged.qdrant_api_key = update.qdrant_api_key;
    }
    if update.qdrant_ca_cert_path.is_some() {
        merged.qdrant_ca_cert_path = update.qdrant_ca_cert_path;
    }
    if update.qdrant_tls_insecure.is_some() {
        merged.qdrant_tls_insecure = update.qdrant_tls_insecure;
    }
    if update.qdrant_strict_auth.is_some() {
        merged.qdrant_strict_auth = update.qdrant_strict_auth;
    }
    if update.nano_provider.is_some() {
        merged.nano_provider = update.nano_provider;
    }
    if update.nano_model.is_some() {
        merged.nano_model = update.nano_model;
    }
    if update.embedding_provider.is_some() {
        merged.embedding_provider = update.embedding_provider;
    }
    if update.embedding_model.is_some() {
        merged.embedding_model = update.embedding_model;
    }
    if update.embedding_api_base_url.is_some() {
        merged.embedding_api_base_url = update.embedding_api_base_url;
    }
    if update.embedding_api_key.is_some() {
        merged.embedding_api_key = update.embedding_api_key;
    }
    if update.embedding_dimensions.is_some() {
        merged.embedding_dimensions = update.embedding_dimensions;
    }
    if update.nano_providers.is_some() {
        merged.nano_providers = update.nano_providers;
    }
    if update.nano_routing_policy.is_some() {
        merged.nano_routing_policy = update.nano_routing_policy;
    }
    if update.external_docs_root.is_some() {
        merged.external_docs_root = update.external_docs_root;
    }
    if update.limits.is_some() {
        merged.limits = update.limits;
    }

    let tmp_path = project_state_dir.join("config.json.tmp");
    let payload = serde_json::to_vec_pretty(&merged)?;
    fs::write(&tmp_path, payload)?;
    fs::rename(&tmp_path, &config_path)?;

    Ok(config_path)
}

fn global_state_dir() -> Result<PathBuf, CoreError> {
    if let Ok(path) = env::var("THE_ONE_HOME") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err(CoreError::InvalidProjectConfig(
            "THE_ONE_HOME must be absolute".to_string(),
        ));
    }

    let home = env::var("HOME").map_err(|_| {
        CoreError::InvalidProjectConfig("HOME is not set and THE_ONE_HOME not provided".to_string())
    })?;

    Ok(PathBuf::from(home).join(".the-one"))
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf, CoreError> {
    if !path.exists() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project root does not exist: {}",
            path.display()
        )));
    }

    let canonical = fs::canonicalize(path)?;
    if !canonical.is_dir() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project root is not a directory: {}",
            canonical.display()
        )));
    }

    Ok(canonical)
}

fn apply_file_layer(path: &Path, merged: &mut FileConfig) -> Result<(), CoreError> {
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(path)?;
    let layer: FileConfig = serde_json::from_str(&content)?;
    merge(merged, layer);
    Ok(())
}

fn apply_env_layer(merged: &mut FileConfig) {
    if let Ok(provider) = env::var("THE_ONE_PROVIDER") {
        merged.provider = Some(provider);
    }
    if let Ok(log_level) = env::var("THE_ONE_LOG_LEVEL") {
        merged.log_level = Some(log_level);
    }
    if let Ok(qdrant_url) = env::var("THE_ONE_QDRANT_URL") {
        merged.qdrant_url = Some(qdrant_url);
    }
    if let Ok(qdrant_api_key) = env::var("THE_ONE_QDRANT_API_KEY") {
        merged.qdrant_api_key = Some(qdrant_api_key);
    }
    if let Ok(qdrant_ca_cert_path) = env::var("THE_ONE_QDRANT_CA_CERT_PATH") {
        merged.qdrant_ca_cert_path = Some(qdrant_ca_cert_path);
    }
    if let Ok(qdrant_tls_insecure) = env::var("THE_ONE_QDRANT_TLS_INSECURE") {
        merged.qdrant_tls_insecure = parse_bool_env(&qdrant_tls_insecure);
    }
    if let Ok(qdrant_strict_auth) = env::var("THE_ONE_QDRANT_STRICT_AUTH") {
        merged.qdrant_strict_auth = parse_bool_env(&qdrant_strict_auth);
    }
    if let Ok(nano_provider) = env::var("THE_ONE_NANO_PROVIDER") {
        merged.nano_provider = Some(nano_provider);
    }
    if let Ok(nano_model) = env::var("THE_ONE_NANO_MODEL") {
        merged.nano_model = Some(nano_model);
    }
    if let Ok(v) = env::var("THE_ONE_EMBEDDING_PROVIDER") {
        merged.embedding_provider = Some(v);
    }
    if let Ok(v) = env::var("THE_ONE_EMBEDDING_MODEL") {
        merged.embedding_model = Some(v);
    }
    if let Ok(v) = env::var("THE_ONE_EMBEDDING_API_BASE_URL") {
        merged.embedding_api_base_url = Some(v);
    }
    if let Ok(v) = env::var("THE_ONE_EMBEDDING_API_KEY") {
        merged.embedding_api_key = Some(v);
    }
    if let Ok(v) = env::var("THE_ONE_EMBEDDING_DIMENSIONS") {
        if let Ok(d) = v.parse::<usize>() {
            merged.embedding_dimensions = Some(d);
        }
    }
    if let Ok(v) = env::var("THE_ONE_EXTERNAL_DOCS_ROOT") {
        merged.external_docs_root = Some(v);
    }
    // Limit env vars
    apply_limit_env_vars(merged);
}

fn apply_limit_env_vars(merged: &mut FileConfig) {
    let mut limits = merged.limits.clone().unwrap_or_default();
    let mut any_set = merged.limits.is_some();

    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_TOOL_SUGGESTIONS") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_tool_suggestions = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_SEARCH_HITS") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_search_hits = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_raw_section_bytes = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_ENABLED_FAMILIES") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_enabled_families = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_DOC_SIZE_BYTES") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_doc_size_bytes = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_MANAGED_DOCS") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_managed_docs = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_EMBEDDING_BATCH_SIZE") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_embedding_batch_size = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_CHUNK_TOKENS") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_chunk_tokens = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_NANO_TIMEOUT_MS") {
        if let Ok(n) = v.parse::<u64>() {
            limits.max_nano_timeout_ms = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_NANO_RETRIES") {
        if let Ok(n) = v.parse::<u8>() {
            limits.max_nano_retries = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_MAX_NANO_PROVIDERS") {
        if let Ok(n) = v.parse::<usize>() {
            limits.max_nano_providers = n;
            any_set = true;
        }
    }
    if let Ok(v) = env::var("THE_ONE_LIMIT_SEARCH_SCORE_THRESHOLD") {
        if let Ok(n) = v.parse::<f32>() {
            limits.search_score_threshold = n;
            any_set = true;
        }
    }

    if any_set {
        merged.limits = Some(limits);
    }
}

fn apply_runtime_layer(runtime: RuntimeOverrides, merged: &mut FileConfig) {
    if runtime.provider.is_some() {
        merged.provider = runtime.provider;
    }
    if runtime.log_level.is_some() {
        merged.log_level = runtime.log_level;
    }
    if runtime.qdrant_url.is_some() {
        merged.qdrant_url = runtime.qdrant_url;
    }
    if runtime.qdrant_api_key.is_some() {
        merged.qdrant_api_key = runtime.qdrant_api_key;
    }
    if runtime.qdrant_ca_cert_path.is_some() {
        merged.qdrant_ca_cert_path = runtime.qdrant_ca_cert_path;
    }
    if runtime.qdrant_tls_insecure.is_some() {
        merged.qdrant_tls_insecure = runtime.qdrant_tls_insecure;
    }
    if runtime.qdrant_strict_auth.is_some() {
        merged.qdrant_strict_auth = runtime.qdrant_strict_auth;
    }
    if runtime.nano_provider.is_some() {
        merged.nano_provider = runtime.nano_provider;
    }
    if runtime.nano_model.is_some() {
        merged.nano_model = runtime.nano_model;
    }
    if runtime.embedding_provider.is_some() {
        merged.embedding_provider = runtime.embedding_provider;
    }
    if runtime.embedding_model.is_some() {
        merged.embedding_model = runtime.embedding_model;
    }
    if runtime.embedding_api_base_url.is_some() {
        merged.embedding_api_base_url = runtime.embedding_api_base_url;
    }
    if runtime.embedding_api_key.is_some() {
        merged.embedding_api_key = runtime.embedding_api_key;
    }
    if runtime.embedding_dimensions.is_some() {
        merged.embedding_dimensions = runtime.embedding_dimensions;
    }
    if runtime.nano_providers.is_some() {
        merged.nano_providers = runtime.nano_providers;
    }
    if runtime.nano_routing_policy.is_some() {
        merged.nano_routing_policy = runtime.nano_routing_policy;
    }
    if runtime.external_docs_root.is_some() {
        merged.external_docs_root = runtime.external_docs_root;
    }
    if runtime.limits.is_some() {
        merged.limits = runtime.limits;
    }
}

fn merge(base: &mut FileConfig, overlay: FileConfig) {
    if overlay.provider.is_some() {
        base.provider = overlay.provider;
    }
    if overlay.log_level.is_some() {
        base.log_level = overlay.log_level;
    }
    if overlay.qdrant_url.is_some() {
        base.qdrant_url = overlay.qdrant_url;
    }
    if overlay.qdrant_api_key.is_some() {
        base.qdrant_api_key = overlay.qdrant_api_key;
    }
    if overlay.qdrant_ca_cert_path.is_some() {
        base.qdrant_ca_cert_path = overlay.qdrant_ca_cert_path;
    }
    if overlay.qdrant_tls_insecure.is_some() {
        base.qdrant_tls_insecure = overlay.qdrant_tls_insecure;
    }
    if overlay.qdrant_strict_auth.is_some() {
        base.qdrant_strict_auth = overlay.qdrant_strict_auth;
    }
    if overlay.nano_provider.is_some() {
        base.nano_provider = overlay.nano_provider;
    }
    if overlay.nano_model.is_some() {
        base.nano_model = overlay.nano_model;
    }
    if overlay.embedding_provider.is_some() {
        base.embedding_provider = overlay.embedding_provider;
    }
    if overlay.embedding_model.is_some() {
        base.embedding_model = overlay.embedding_model;
    }
    if overlay.embedding_api_base_url.is_some() {
        base.embedding_api_base_url = overlay.embedding_api_base_url;
    }
    if overlay.embedding_api_key.is_some() {
        base.embedding_api_key = overlay.embedding_api_key;
    }
    if overlay.embedding_dimensions.is_some() {
        base.embedding_dimensions = overlay.embedding_dimensions;
    }
    if overlay.nano_providers.is_some() {
        base.nano_providers = overlay.nano_providers;
    }
    if overlay.nano_routing_policy.is_some() {
        base.nano_routing_policy = overlay.nano_routing_policy;
    }
    if overlay.external_docs_root.is_some() {
        base.external_docs_root = overlay.external_docs_root;
    }
    if overlay.limits.is_some() {
        base.limits = overlay.limits;
    }
}

fn parse_bool_env(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        update_project_config, AppConfig, NanoProviderKind, ProjectConfigUpdate, RuntimeOverrides,
    };
    use crate::limits::ConfigurableLimits;

    #[test]
    fn test_config_precedence_runtime_overrides_env_project_global_defaults() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        let project_state_dir = project_root.join(".the-one");
        let global_state_dir = temp.path().join("global");

        fs::create_dir_all(&project_state_dir).expect("project state dir should exist");
        fs::create_dir_all(&global_state_dir).expect("global state dir should exist");

        fs::write(
            global_state_dir.join("config.json"),
            r#"{"provider":"global-provider","log_level":"warn"}"#,
        )
        .expect("global config write should succeed");
        fs::write(
            project_state_dir.join("config.json"),
            r#"{"provider":"project-provider","qdrant_url":"http://project:6334"}"#,
        )
        .expect("project config write should succeed");

        let global_home = global_state_dir.display().to_string();
        temp_env::with_vars(
            [
                ("THE_ONE_HOME", Some(global_home.as_str())),
                ("THE_ONE_PROVIDER", Some("env-provider")),
                ("THE_ONE_LOG_LEVEL", None),
                ("THE_ONE_QDRANT_URL", None),
                ("THE_ONE_NANO_PROVIDER", None),
                ("THE_ONE_NANO_MODEL", None),
                ("THE_ONE_QDRANT_API_KEY", None),
                ("THE_ONE_QDRANT_CA_CERT_PATH", None),
                ("THE_ONE_QDRANT_TLS_INSECURE", None),
                ("THE_ONE_QDRANT_STRICT_AUTH", None),
                ("THE_ONE_EMBEDDING_PROVIDER", None),
                ("THE_ONE_EMBEDDING_MODEL", None),
                ("THE_ONE_EMBEDDING_API_BASE_URL", None),
                ("THE_ONE_EMBEDDING_API_KEY", None),
                ("THE_ONE_EMBEDDING_DIMENSIONS", None),
                ("THE_ONE_EXTERNAL_DOCS_ROOT", None),
                ("THE_ONE_LIMIT_MAX_TOOL_SUGGESTIONS", None),
                ("THE_ONE_LIMIT_MAX_SEARCH_HITS", None),
                ("THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES", None),
                ("THE_ONE_LIMIT_MAX_ENABLED_FAMILIES", None),
                ("THE_ONE_LIMIT_MAX_DOC_SIZE_BYTES", None),
                ("THE_ONE_LIMIT_MAX_MANAGED_DOCS", None),
                ("THE_ONE_LIMIT_MAX_EMBEDDING_BATCH_SIZE", None),
                ("THE_ONE_LIMIT_MAX_CHUNK_TOKENS", None),
                ("THE_ONE_LIMIT_MAX_NANO_TIMEOUT_MS", None),
                ("THE_ONE_LIMIT_MAX_NANO_RETRIES", None),
                ("THE_ONE_LIMIT_MAX_NANO_PROVIDERS", None),
                ("THE_ONE_LIMIT_SEARCH_SCORE_THRESHOLD", None),
            ],
            || {
                let config = AppConfig::load(
                    &project_root,
                    RuntimeOverrides {
                        provider: Some("runtime-provider".to_string()),
                        log_level: None,
                        qdrant_url: None,
                        nano_provider: Some("api".to_string()),
                        nano_model: Some("gpt-nano".to_string()),
                        qdrant_api_key: None,
                        qdrant_ca_cert_path: None,
                        qdrant_tls_insecure: None,
                        qdrant_strict_auth: None,
                        ..RuntimeOverrides::default()
                    },
                )
                .expect("config should load");

                assert_eq!(config.provider, "runtime-provider");
                assert_eq!(config.log_level, "warn");
                assert_eq!(config.qdrant_url, "http://project:6334");
                assert_eq!(config.nano_provider, NanoProviderKind::Api);
                assert_eq!(config.nano_model, "gpt-nano");
            },
        );
    }

    #[test]
    fn test_update_project_config_persists_provider_and_nano_settings() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        let global_state_dir = temp.path().join("global");

        fs::create_dir_all(&project_root).expect("project root should exist");
        fs::create_dir_all(&global_state_dir).expect("global state dir should exist");

        let global_home = global_state_dir.display().to_string();
        temp_env::with_vars(
            [
                ("THE_ONE_HOME", Some(global_home.as_str())),
                ("THE_ONE_PROVIDER", None),
                ("THE_ONE_LOG_LEVEL", None),
                ("THE_ONE_QDRANT_URL", None),
                ("THE_ONE_NANO_PROVIDER", None),
                ("THE_ONE_NANO_MODEL", None),
                ("THE_ONE_QDRANT_API_KEY", None),
                ("THE_ONE_QDRANT_CA_CERT_PATH", None),
                ("THE_ONE_QDRANT_TLS_INSECURE", None),
                ("THE_ONE_QDRANT_STRICT_AUTH", None),
                ("THE_ONE_EMBEDDING_PROVIDER", None),
                ("THE_ONE_EMBEDDING_MODEL", None),
                ("THE_ONE_EMBEDDING_API_BASE_URL", None),
                ("THE_ONE_EMBEDDING_API_KEY", None),
                ("THE_ONE_EMBEDDING_DIMENSIONS", None),
                ("THE_ONE_EXTERNAL_DOCS_ROOT", None),
                ("THE_ONE_LIMIT_MAX_TOOL_SUGGESTIONS", None),
                ("THE_ONE_LIMIT_MAX_SEARCH_HITS", None),
                ("THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES", None),
                ("THE_ONE_LIMIT_MAX_ENABLED_FAMILIES", None),
                ("THE_ONE_LIMIT_MAX_DOC_SIZE_BYTES", None),
                ("THE_ONE_LIMIT_MAX_MANAGED_DOCS", None),
                ("THE_ONE_LIMIT_MAX_EMBEDDING_BATCH_SIZE", None),
                ("THE_ONE_LIMIT_MAX_CHUNK_TOKENS", None),
                ("THE_ONE_LIMIT_MAX_NANO_TIMEOUT_MS", None),
                ("THE_ONE_LIMIT_MAX_NANO_RETRIES", None),
                ("THE_ONE_LIMIT_MAX_NANO_PROVIDERS", None),
                ("THE_ONE_LIMIT_SEARCH_SCORE_THRESHOLD", None),
            ],
            || {
                update_project_config(
                    &project_root,
                    ProjectConfigUpdate {
                        provider: Some("hosted".to_string()),
                        nano_provider: Some("ollama".to_string()),
                        nano_model: Some("tiny".to_string()),
                        ..ProjectConfigUpdate::default()
                    },
                )
                .expect("update should succeed");

                let config = AppConfig::load(&project_root, RuntimeOverrides::default())
                    .expect("config should load");
                assert_eq!(config.provider, "hosted");
                assert_eq!(config.nano_provider, NanoProviderKind::Ollama);
                assert_eq!(config.nano_model, "tiny");
            },
        );
    }

    #[test]
    fn test_config_loads_embedding_and_limits_from_project_config() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        let project_state_dir = project_root.join(".the-one");
        let global_state_dir = temp.path().join("global");

        fs::create_dir_all(&project_state_dir).expect("project state dir should exist");
        fs::create_dir_all(&global_state_dir).expect("global state dir should exist");

        fs::write(
            project_state_dir.join("config.json"),
            r#"{
                "embedding_provider": "openai",
                "embedding_model": "text-embedding-3-small",
                "embedding_dimensions": 256,
                "limits": {
                    "max_tool_suggestions": 5,
                    "max_search_hits": 10,
                    "max_raw_section_bytes": 24576,
                    "max_enabled_families": 12,
                    "max_doc_size_bytes": 102400,
                    "max_managed_docs": 500,
                    "max_embedding_batch_size": 64,
                    "max_chunk_tokens": 256,
                    "max_nano_timeout_ms": 2000,
                    "max_nano_retries": 3,
                    "max_nano_providers": 5,
                    "search_score_threshold": 0.3
                }
            }"#,
        )
        .expect("project config write should succeed");

        let global_home = global_state_dir.display().to_string();
        temp_env::with_vars(
            [
                ("THE_ONE_HOME", Some(global_home.as_str())),
                ("THE_ONE_PROVIDER", None),
                ("THE_ONE_LOG_LEVEL", None),
                ("THE_ONE_QDRANT_URL", None),
                ("THE_ONE_NANO_PROVIDER", None),
                ("THE_ONE_NANO_MODEL", None),
                ("THE_ONE_QDRANT_API_KEY", None),
                ("THE_ONE_QDRANT_CA_CERT_PATH", None),
                ("THE_ONE_QDRANT_TLS_INSECURE", None),
                ("THE_ONE_QDRANT_STRICT_AUTH", None),
                ("THE_ONE_EMBEDDING_PROVIDER", None),
                ("THE_ONE_EMBEDDING_MODEL", None),
                ("THE_ONE_EMBEDDING_API_BASE_URL", None),
                ("THE_ONE_EMBEDDING_API_KEY", None),
                ("THE_ONE_EMBEDDING_DIMENSIONS", None),
                ("THE_ONE_EXTERNAL_DOCS_ROOT", None),
                ("THE_ONE_LIMIT_MAX_TOOL_SUGGESTIONS", None),
                ("THE_ONE_LIMIT_MAX_SEARCH_HITS", None),
                ("THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES", None),
                ("THE_ONE_LIMIT_MAX_ENABLED_FAMILIES", None),
                ("THE_ONE_LIMIT_MAX_DOC_SIZE_BYTES", None),
                ("THE_ONE_LIMIT_MAX_MANAGED_DOCS", None),
                ("THE_ONE_LIMIT_MAX_EMBEDDING_BATCH_SIZE", None),
                ("THE_ONE_LIMIT_MAX_CHUNK_TOKENS", None),
                ("THE_ONE_LIMIT_MAX_NANO_TIMEOUT_MS", None),
                ("THE_ONE_LIMIT_MAX_NANO_RETRIES", None),
                ("THE_ONE_LIMIT_MAX_NANO_PROVIDERS", None),
                ("THE_ONE_LIMIT_SEARCH_SCORE_THRESHOLD", None),
            ],
            || {
                let config = AppConfig::load(&project_root, RuntimeOverrides::default())
                    .expect("config should load");

                // Embedding fields from project config
                assert_eq!(config.embedding_provider, "openai");
                assert_eq!(config.embedding_model, "text-embedding-3-small");
                assert_eq!(config.embedding_dimensions, 256);

                // Limits from project config
                assert_eq!(config.limits.max_search_hits, 10);
                assert_eq!(config.limits.max_chunk_tokens, 256);

                // Default limit preserved when set in config to default value
                let defaults = ConfigurableLimits::default();
                assert_eq!(
                    config.limits.max_tool_suggestions,
                    defaults.max_tool_suggestions
                );
            },
        );
    }
}
