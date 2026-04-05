//! Embedding model registry — parsed from TOML files embedded in the binary.
//!
//! Four registries are bundled:
//! - **local-models.toml**: fastembed-backed ONNX embedding models (offline).
//! - **api-models.toml**: OpenAI-compatible API providers.
//! - **rerank-models.toml**: fastembed-backed ONNX reranker models (offline).
//! - **image-models.toml**: fastembed-backed ONNX image embedding models (offline).
//!
//! API models can be extended at runtime by merging a user-supplied TOML file
//! (typically `~/.the-one/api-models.toml`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;

// ── Embedded TOML sources ────────────────────────────────────────────────────

const LOCAL_MODELS_TOML: &str = include_str!("../../../models/local-models.toml");
const API_MODELS_TOML: &str = include_str!("../../../models/api-models.toml");
const RERANK_MODELS_TOML: &str = include_str!("../../../models/rerank-models.toml");
const IMAGE_MODELS_TOML: &str = include_str!("../../../models/image-models.toml");

// ── Local model types ────────────────────────────────────────────────────────

/// Metadata section of local-models.toml — parsed but `updated` field is
/// informational only.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct LocalRegistryMeta {
    fastembed_crate_version: String,
    updated: String,
}

/// A single local embedding model entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocalModel {
    /// Human-readable display name.
    pub name: String,
    /// Tier label: "fast" | "balanced" | "quality" | "multilingual".
    pub tier: String,
    /// Output embedding dimensionality.
    pub dims: usize,
    /// Approximate model download size in MB.
    pub size_mb: u32,
    /// Qualitative latency hint relative to the fastest model.
    pub latency_vs_fast: String,
    /// Whether the model handles non-English text well.
    pub multilingual: bool,
    /// One-line description shown to users.
    pub description: String,
    /// Rust enum variant name in the `fastembed` crate.
    pub fastembed_enum: String,
    /// Whether this is the recommended default model.
    pub default: bool,
    /// Whether to show this model in the interactive installer.
    pub installer_visible: bool,
    /// Whether this model is deprecated and should not be used.
    #[serde(default)]
    pub deprecated: bool,
    /// Optional status note for deprecation or maintenance tracking.
    #[serde(default)]
    pub status: Option<String>,
}

/// Raw deserialization wrapper for local-models.toml.
#[derive(Debug, Deserialize)]
struct LocalRegistryFile {
    #[allow(dead_code)]
    meta: LocalRegistryMeta,
    models: HashMap<String, LocalModel>,
}

// ── API model types ──────────────────────────────────────────────────────────

/// A single model entry within an API provider.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiModel {
    /// Human-readable name (may equal the BTreeMap key).
    pub name: String,
    /// Output embedding dimensionality.
    pub dims: usize,
    /// Whether the model handles multiple languages.
    pub multilingual: bool,
    /// One-line description shown to users.
    pub description: String,
    /// Whether this is the recommended default within its provider.
    pub default: bool,
}

/// A provider entry containing one or more API models.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiProvider {
    /// Human-readable provider name.
    pub name: String,
    /// API base URL.
    pub base_url: String,
    /// Environment variable that holds the API key.
    pub auth_env: String,
    /// Link to provider docs.
    pub docs_url: String,
    /// Models offered by this provider, keyed by model id.
    pub models: BTreeMap<String, ApiModel>,
}

// ── Reranker model types ─────────────────────────────────────────────────────

/// A single reranker model entry from `rerank-models.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RerankModel {
    /// Human-readable display name.
    pub name: String,
    /// Approximate model download size in MB.
    pub size_mb: u32,
    /// Whether the model handles non-English text well.
    pub multilingual: bool,
    /// One-line description shown to users.
    pub description: String,
    /// Rust enum variant name in the `fastembed` crate.
    pub fastembed_enum: String,
    /// Whether this is the recommended default reranker.
    pub default: bool,
}

/// Raw deserialization wrapper for rerank-models.toml.
#[derive(Debug, Deserialize)]
struct RerankRegistryFile {
    #[allow(dead_code)]
    meta: LocalRegistryMeta,
    models: HashMap<String, RerankModel>,
}

// ── Image model types ────────────────────────────────────────────────────────

/// A single image embedding model entry from `image-models.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageModel {
    /// Human-readable display name.
    pub name: String,
    /// Output embedding dimensionality.
    pub dims: usize,
    /// Approximate model download size in MB.
    pub size_mb: u32,
    /// One-line description shown to users.
    pub description: String,
    /// Rust enum variant name in the `fastembed` crate.
    pub fastembed_enum: String,
    /// Whether this is the recommended default image model.
    pub default: bool,
    /// Optional paired text model enum variant (for joint text+image search).
    #[serde(default)]
    pub paired_text_model: String,
}

/// Raw deserialization wrapper for image-models.toml.
#[derive(Debug, Deserialize)]
struct ImageRegistryFile {
    #[allow(dead_code)]
    meta: LocalRegistryMeta,
    models: HashMap<String, ImageModel>,
}

// ── Tier ordering ────────────────────────────────────────────────────────────

fn tier_order(tier: &str) -> u8 {
    match tier {
        "fast" => 0,
        "balanced" => 1,
        "quality" => 2,
        "multilingual" => 3,
        _ => 4,
    }
}

// ── Parsing helpers ──────────────────────────────────────────────────────────

fn parse_local_registry(src: &str) -> Result<HashMap<String, LocalModel>, String> {
    let file: LocalRegistryFile =
        toml::from_str(src).map_err(|e| format!("failed to parse local-models.toml: {e}"))?;
    Ok(file.models)
}

fn parse_rerank_registry(src: &str) -> Result<HashMap<String, RerankModel>, String> {
    let file: RerankRegistryFile =
        toml::from_str(src).map_err(|e| format!("failed to parse rerank-models.toml: {e}"))?;
    Ok(file.models)
}

fn parse_api_registry(src: &str) -> Result<Vec<(String, ApiProvider)>, String> {
    let value: toml::Value =
        toml::from_str(src).map_err(|e| format!("failed to parse api-models.toml: {e}"))?;

    let providers_table = value
        .get("providers")
        .and_then(|v| v.as_table())
        .ok_or("api-models.toml missing [providers] section")?;

    let mut providers = Vec::new();
    for (provider_key, provider_val) in providers_table {
        let tbl = provider_val
            .as_table()
            .ok_or_else(|| format!("provider `{provider_key}` is not a table"))?;

        let name = tbl
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(provider_key)
            .to_string();
        let base_url = tbl
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let auth_env = tbl
            .get("auth_env")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let docs_url = tbl
            .get("docs_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let models_table = tbl
            .get("models")
            .and_then(|v| v.as_table())
            .ok_or_else(|| format!("provider `{provider_key}` has no [models] section"))?;

        let mut models = BTreeMap::new();
        for (model_key, model_val) in models_table {
            let mt = model_val
                .as_table()
                .ok_or_else(|| format!("model `{model_key}` is not a table"))?;

            let model_name = mt
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(model_key)
                .to_string();
            let dims = mt.get("dims").and_then(|v| v.as_integer()).unwrap_or(0) as usize;
            let multilingual = mt
                .get("multilingual")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let description = mt
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let default = mt.get("default").and_then(|v| v.as_bool()).unwrap_or(false);

            models.insert(
                model_key.clone(),
                ApiModel {
                    name: model_name,
                    dims,
                    multilingual,
                    description,
                    default,
                },
            );
        }

        providers.push((
            provider_key.clone(),
            ApiProvider {
                name,
                base_url,
                auth_env,
                docs_url,
                models,
            },
        ));
    }

    Ok(providers)
}

fn parse_image_registry(src: &str) -> Result<HashMap<String, ImageModel>, String> {
    let file: ImageRegistryFile =
        toml::from_str(src).map_err(|e| format!("failed to parse image-models.toml: {e}"))?;
    Ok(file.models)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Return all local models from the embedded registry, sorted by tier order.
///
/// # Panics
///
/// Panics at startup if the embedded TOML is malformed. This indicates a
/// build-time error that must be fixed before shipping.
pub fn list_local_models() -> Vec<LocalModel> {
    let mut models: Vec<LocalModel> = parse_local_registry(LOCAL_MODELS_TOML)
        .expect("embedded local-models.toml is valid")
        .into_values()
        .collect();
    models.sort_by_key(|m| tier_order(&m.tier));
    models
}

/// Return only the models that should appear in the interactive installer
/// (those with `installer_visible = true` and `deprecated = false`),
/// sorted by tier order.
pub fn list_installer_models() -> Vec<LocalModel> {
    let mut models: Vec<LocalModel> = list_local_models()
        .into_iter()
        .filter(|m| m.installer_visible && !m.deprecated)
        .collect();
    models.sort_by_key(|m| tier_order(&m.tier));
    models
}

/// Return the definition of the default local model (the one with `default = true`).
///
/// # Panics
///
/// Panics if the registry contains no default — this is a compile-time
/// configuration error that must be fixed before shipping.
pub fn default_local_model() -> LocalModel {
    list_local_models()
        .into_iter()
        .find(|m| m.default)
        .expect("registry must have exactly one default model")
}

/// Return the `fastembed_crate_version` field from the embedded meta section.
///
/// Used to detect when the fastembed crate was bumped and the registry needs
/// review.
pub fn fastembed_crate_version() -> String {
    // Parse just the meta section via a minimal struct
    #[derive(Deserialize)]
    struct JustMeta {
        meta: LocalRegistryMeta,
    }
    let parsed: JustMeta =
        toml::from_str(LOCAL_MODELS_TOML).expect("embedded local-models.toml is valid");
    parsed.meta.fastembed_crate_version
}

/// Return all rerank models from the embedded registry.
///
/// # Panics
///
/// Panics at startup if the embedded TOML is malformed. This indicates a
/// build-time error that must be fixed before shipping.
pub fn list_rerank_models() -> Vec<RerankModel> {
    parse_rerank_registry(RERANK_MODELS_TOML)
        .expect("embedded rerank-models.toml is valid")
        .into_values()
        .collect()
}

/// Return the default rerank model (the one with `default = true`).
///
/// # Panics
///
/// Panics if the registry contains no default — this is a compile-time
/// configuration error that must be fixed before shipping.
pub fn default_rerank_model() -> RerankModel {
    list_rerank_models()
        .into_iter()
        .find(|m| m.default)
        .expect("rerank registry must have exactly one default model")
}

/// Return all API providers from the embedded registry.
///
/// # Panics
///
/// Panics at startup if the embedded TOML is malformed.
pub fn list_api_providers() -> Vec<ApiProvider> {
    parse_api_registry(API_MODELS_TOML)
        .expect("embedded api-models.toml is valid")
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

/// Merge a user-supplied TOML string (from `~/.the-one/api-models.toml`) into
/// the base provider list from the embedded registry. User entries override
/// built-in providers with the same key; unknown keys are appended.
///
/// Returns an error string if the user TOML is malformed.
pub fn merge_user_api_models(user_toml: &str) -> Result<Vec<ApiProvider>, String> {
    let base = parse_api_registry(API_MODELS_TOML).expect("embedded api-models.toml is valid");

    if user_toml.trim().is_empty() {
        return Ok(base.into_iter().map(|(_, p)| p).collect());
    }

    let user_providers = parse_api_registry(user_toml)?;

    // Keyed map from the base so we can merge by key
    let mut map: BTreeMap<String, ApiProvider> = base.into_iter().collect();

    for (provider_key, provider) in user_providers {
        map.entry(provider_key)
            .and_modify(|existing| {
                // Merge: user models override existing models with the same key
                for (model_key, model) in &provider.models {
                    existing.models.insert(model_key.clone(), model.clone());
                }
                // Also update provider-level fields if user supplied them
                if !provider.base_url.is_empty() {
                    existing.base_url = provider.base_url.clone();
                }
                if !provider.auth_env.is_empty() {
                    existing.auth_env = provider.auth_env.clone();
                }
                if !provider.docs_url.is_empty() {
                    existing.docs_url = provider.docs_url.clone();
                }
            })
            .or_insert(provider);
    }

    Ok(map.into_values().collect())
}

/// Return all image embedding models from the embedded registry.
///
/// # Panics
///
/// Panics at startup if the embedded TOML is malformed. This indicates a
/// build-time error that must be fixed before shipping.
pub fn list_image_models() -> Vec<ImageModel> {
    parse_image_registry(IMAGE_MODELS_TOML)
        .expect("embedded image-models.toml is valid")
        .into_values()
        .collect()
}

/// Return the default image embedding model (the one with `default = true`).
///
/// # Panics
///
/// Panics if the registry contains no default — this is a compile-time
/// configuration error that must be fixed before shipping.
pub fn default_image_model() -> ImageModel {
    list_image_models()
        .into_iter()
        .find(|m| m.default)
        .expect("image registry must have exactly one default model")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Local model registry tests ───────────────────────────────────────────

    #[test]
    fn test_local_models_parses_without_error() {
        // Should not panic
        let models = list_local_models();
        assert!(
            !models.is_empty(),
            "registry must contain at least one model"
        );
    }

    #[test]
    fn test_local_models_has_expected_count() {
        let models = list_local_models();
        // The TOML file defines 19 models (7 primary + 7 additional + 3 quantized +
        // 2 more additional = 19 total)
        assert!(
            models.len() >= 10,
            "expected at least 10 local models, got {}",
            models.len()
        );
    }

    #[test]
    fn test_default_local_model_exists() {
        // Should not panic
        let model = default_local_model();
        assert!(
            model.default,
            "default model's `default` field must be true"
        );
        assert!(
            !model.name.is_empty(),
            "default model name must not be empty"
        );
    }

    #[test]
    fn test_default_local_model_is_bge_large() {
        let model = default_local_model();
        assert_eq!(model.fastembed_enum, "BGELargeENV15");
        assert_eq!(model.dims, 1024);
        assert_eq!(model.tier, "quality");
    }

    #[test]
    fn test_installer_models_count() {
        let installer = list_installer_models();
        assert_eq!(
            installer.len(),
            7,
            "expected 7 installer-visible models, got {}",
            installer.len()
        );
    }

    #[test]
    fn test_installer_models_are_visible_subset() {
        let all = list_local_models();
        let installer = list_installer_models();

        // All installer models must be in the full set (match by fastembed_enum as unique id)
        let all_enums: std::collections::HashSet<&str> =
            all.iter().map(|m| m.fastembed_enum.as_str()).collect();
        for model in &installer {
            assert!(
                all_enums.contains(model.fastembed_enum.as_str()),
                "installer model `{}` not found in full registry",
                model.fastembed_enum
            );
        }

        // Installer models must all have installer_visible = true
        for model in &installer {
            assert!(
                model.installer_visible,
                "installer model `{}` has installer_visible = false",
                model.name
            );
        }

        // Installer models must all have deprecated = false
        for model in &installer {
            assert!(
                !model.deprecated,
                "installer model `{}` has deprecated = true",
                model.name
            );
        }

        // Non-visible models must NOT appear
        let installer_enums: std::collections::HashSet<&str> = installer
            .iter()
            .map(|m| m.fastembed_enum.as_str())
            .collect();
        for model in &all {
            if !model.installer_visible {
                assert!(
                    !installer_enums.contains(model.fastembed_enum.as_str()),
                    "non-visible model `{}` appeared in installer list",
                    model.fastembed_enum
                );
            }
        }
    }

    #[test]
    fn test_installer_models_sorted_by_tier() {
        let installer = list_installer_models();
        for window in installer.windows(2) {
            let a = &window[0];
            let b = &window[1];
            assert!(
                tier_order(&a.tier) <= tier_order(&b.tier),
                "installer models not sorted by tier: {} ({}) before {} ({})",
                a.name,
                a.tier,
                b.name,
                b.tier
            );
        }
    }

    #[test]
    fn test_local_models_sorted_by_tier() {
        let models = list_local_models();
        for window in models.windows(2) {
            let a = &window[0];
            let b = &window[1];
            assert!(
                tier_order(&a.tier) <= tier_order(&b.tier),
                "local models not sorted by tier: {} ({}) before {} ({})",
                a.name,
                a.tier,
                b.name,
                b.tier
            );
        }
    }

    #[test]
    fn test_local_model_fields_are_populated() {
        let models = list_local_models();
        for model in &models {
            assert!(
                !model.name.is_empty(),
                "model `{}` has empty name",
                model.fastembed_enum
            );
            assert!(
                !model.tier.is_empty(),
                "model `{}` has empty tier",
                model.fastembed_enum
            );
            assert!(
                model.dims > 0,
                "model `{}` has zero dims",
                model.fastembed_enum
            );
            assert!(
                model.size_mb > 0,
                "model `{}` has zero size_mb",
                model.fastembed_enum
            );
            assert!(
                !model.fastembed_enum.is_empty(),
                "model has empty fastembed_enum"
            );
            assert!(
                !model.description.is_empty(),
                "model `{}` has empty description",
                model.fastembed_enum
            );
        }
    }

    #[test]
    fn test_fastembed_crate_version() {
        let version = fastembed_crate_version();
        assert_eq!(
            version, "4",
            "expected fastembed version 4, got `{version}`"
        );
    }

    #[test]
    fn test_tiers_are_valid() {
        let valid_tiers = ["fast", "balanced", "quality", "multilingual"];
        let models = list_local_models();
        for model in &models {
            assert!(
                valid_tiers.contains(&model.tier.as_str()),
                "model `{}` has unexpected tier `{}`",
                model.fastembed_enum,
                model.tier
            );
        }
    }

    #[test]
    fn test_exactly_one_default_local_model() {
        let models = list_local_models();
        let defaults: Vec<&str> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| m.fastembed_enum.as_str())
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "expected exactly 1 default local model, got {}: {:?}",
            defaults.len(),
            defaults
        );
    }

    // ── Reranker model registry tests ───────────────────────────────────────

    #[test]
    fn test_rerank_models_parses_without_error() {
        let models = list_rerank_models();
        assert_eq!(models.len(), 4, "expected 4 rerank models");
    }

    #[test]
    fn test_default_rerank_model_is_jina_v2() {
        let model = default_rerank_model();
        assert_eq!(model.fastembed_enum, "JINARerankerV2BaseMultiligual");
        assert!(model.default);
        assert!(model.multilingual);
    }

    #[test]
    fn test_exactly_one_default_rerank_model() {
        let models = list_rerank_models();
        let defaults: Vec<_> = models.iter().filter(|m| m.default).collect();
        assert_eq!(defaults.len(), 1);
    }

    #[test]
    fn test_rerank_model_fields_populated() {
        for model in list_rerank_models() {
            assert!(!model.name.is_empty());
            assert!(model.size_mb > 0);
            assert!(!model.description.is_empty());
            assert!(!model.fastembed_enum.is_empty());
        }
    }

    // ── Image model registry tests ───────────────────────────────────────────

    #[test]
    fn test_image_models_parses_without_error() {
        let models = list_image_models();
        assert!(
            !models.is_empty(),
            "image registry must contain at least one model"
        );
    }

    #[test]
    fn test_default_image_model_exists() {
        let model = default_image_model();
        assert!(
            model.default,
            "default image model's `default` field must be true"
        );
        assert!(
            !model.name.is_empty(),
            "default image model name must not be empty"
        );
    }

    #[test]
    fn test_exactly_one_default_image_model() {
        let models = list_image_models();
        let defaults: Vec<&str> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| m.fastembed_enum.as_str())
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "expected exactly 1 default image model, got {}: {:?}",
            defaults.len(),
            defaults
        );
    }

    #[test]
    fn test_image_model_fields_populated() {
        for model in list_image_models() {
            assert!(!model.name.is_empty(), "image model has empty name");
            assert!(model.size_mb > 0, "image model `{}` has zero size_mb", model.name);
            assert!(!model.description.is_empty(), "image model `{}` has empty description", model.name);
            assert!(!model.fastembed_enum.is_empty(), "image model `{}` has empty fastembed_enum", model.name);
            assert!(model.dims > 0, "image model `{}` has zero dims", model.name);
        }
    }

    // ── API model registry tests ─────────────────────────────────────────────

    #[test]
    fn test_api_providers_parses_without_error() {
        let providers = list_api_providers();
        assert!(
            !providers.is_empty(),
            "registry must contain at least one provider"
        );
    }

    #[test]
    fn test_api_providers_has_expected_count() {
        let providers = list_api_providers();
        assert_eq!(
            providers.len(),
            3,
            "expected 3 API providers, got {}",
            providers.len()
        );
    }

    #[test]
    fn test_api_providers_has_expected_providers() {
        let providers = list_api_providers();
        // Verify by name since we no longer have a key field
        let names: Vec<&str> = providers.iter().map(|p| p.name.as_str()).collect();
        for expected in &["OpenAI", "Voyage AI", "Cohere"] {
            assert!(
                names.contains(expected),
                "expected provider `{expected}` not found; got: {names:?}"
            );
        }
    }

    #[test]
    fn test_api_provider_fields_are_populated() {
        let providers = list_api_providers();
        for provider in &providers {
            assert!(
                !provider.name.is_empty(),
                "provider `{}` has empty name",
                provider.name
            );
            assert!(
                !provider.base_url.is_empty(),
                "provider `{}` has empty base_url",
                provider.name
            );
            assert!(
                !provider.auth_env.is_empty(),
                "provider `{}` has empty auth_env",
                provider.name
            );
            assert!(
                !provider.models.is_empty(),
                "provider `{}` has no models",
                provider.name
            );
        }
    }

    #[test]
    fn test_api_provider_each_has_exactly_one_default() {
        let providers = list_api_providers();
        for provider in &providers {
            let defaults: Vec<&str> = provider
                .models
                .iter()
                .filter(|(_, m)| m.default)
                .map(|(k, _)| k.as_str())
                .collect();
            assert_eq!(
                defaults.len(),
                1,
                "provider `{}` must have exactly 1 default model, got {}: {:?}",
                provider.name,
                defaults.len(),
                defaults
            );
        }
    }

    #[test]
    fn test_api_model_dims_are_nonzero() {
        let providers = list_api_providers();
        for provider in &providers {
            for (key, model) in &provider.models {
                assert!(
                    model.dims > 0,
                    "provider `{}` model `{}` has zero dims",
                    provider.name,
                    key
                );
            }
        }
    }

    #[test]
    fn test_openai_default_model() {
        let providers = list_api_providers();
        let openai = providers
            .iter()
            .find(|p| p.name == "OpenAI")
            .expect("OpenAI provider must exist");
        let default_model = openai
            .models
            .get("text-embedding-3-small")
            .expect("text-embedding-3-small must exist");
        assert!(
            default_model.default,
            "text-embedding-3-small must be the default"
        );
        assert_eq!(default_model.dims, 1536);
    }

    // ── merge_user_api_models tests ──────────────────────────────────────────

    #[test]
    fn test_merge_empty_user_toml_returns_base_unchanged() {
        let base = list_api_providers();
        let base_len = base.len();
        let merged = merge_user_api_models("").expect("empty merge must succeed");
        assert_eq!(merged.len(), base_len);
    }

    #[test]
    fn test_merge_adds_new_provider() {
        let base = list_api_providers();
        let base_len = base.len();
        let user_toml = r#"
[providers.custom]
name = "Custom Provider"
base_url = "https://api.example.com/v1"
auth_env = "CUSTOM_API_KEY"
docs_url = "https://docs.example.com"

[providers.custom.models.custom-embed-v1]
name = "custom-embed-v1"
dims = 512
multilingual = false
description = "A custom embedding model."
default = true
"#;
        let merged = merge_user_api_models(user_toml).expect("merge must succeed");
        assert_eq!(merged.len(), base_len + 1);
        let custom = merged
            .iter()
            .find(|p| p.name == "Custom Provider")
            .expect("custom provider must be present after merge");
        assert_eq!(custom.models.len(), 1);
        let model = custom
            .models
            .get("custom-embed-v1")
            .expect("custom-embed-v1 must exist");
        assert_eq!(model.dims, 512);
    }

    #[test]
    fn test_merge_overrides_existing_provider_model() {
        let user_toml = r#"
[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
auth_env = "OPENAI_API_KEY"
docs_url = "https://platform.openai.com/docs/guides/embeddings"

[providers.openai.models.text-embedding-3-small]
name = "text-embedding-3-small"
dims = 2048
multilingual = true
description = "Overridden dims."
default = true
"#;
        let merged = merge_user_api_models(user_toml).expect("merge must succeed");
        let openai = merged
            .iter()
            .find(|p| p.name == "OpenAI")
            .expect("openai must still exist");
        let small = openai
            .models
            .get("text-embedding-3-small")
            .expect("text-embedding-3-small must still exist");
        assert_eq!(small.dims, 2048, "user override must take effect");
    }

    #[test]
    fn test_merge_invalid_toml_returns_error() {
        let result = merge_user_api_models("not valid toml ::::");
        assert!(result.is_err(), "invalid TOML must return an error");
    }
}
