use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

const DEFAULT_PROVIDER: &str = "local";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_QDRANT_URL: &str = "http://127.0.0.1:6334";
const DEFAULT_NANO_PROVIDER: &str = "rules";
const DEFAULT_NANO_MODEL: &str = "none";
const DEFAULT_QDRANT_TLS_INSECURE: bool = false;
const DEFAULT_QDRANT_STRICT_AUTH: bool = true;

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
}
