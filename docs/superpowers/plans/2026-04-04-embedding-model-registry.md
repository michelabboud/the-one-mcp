# Embedding Model Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hardcoded embedding model defaults with a TOML-based model registry, make quality tier the default, add interactive model selection to the installer, and create maintenance scripts for tracking upstream model updates.

**Architecture:** Two TOML registry files (`models/local-models.toml`, `models/api-models.toml`) embedded in the binary via `include_str!`. A new `models_registry` module in `the-one-memory` parses them and exposes typed model lists. The installer reads model metadata to render an interactive selection table. Two maintenance scripts check for upstream changes.

**Tech Stack:** Rust, `toml` crate (new dependency), `fastembed` 4.x, bash

**Spec:** `docs/superpowers/specs/2026-04-04-embedding-model-registry-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `models/local-models.toml` | Local model registry — source of truth for all fastembed models |
| Create | `models/api-models.toml` | API model registry — OpenAI, Voyage, Cohere + extensibility |
| Create | `crates/the-one-memory/src/models_registry.rs` | Parse TOML registries, expose `LocalModel`, `ApiProvider`, `list_local_models()`, `list_api_models()` |
| Create | `scripts/update-local-models.sh` | Maintenance: check fastembed crate for new models |
| Create | `scripts/update-api-models.sh` | Maintenance: check API providers for new models |
| Modify | `Cargo.toml` | Add `toml` workspace dependency |
| Modify | `crates/the-one-memory/Cargo.toml` | Add `toml` dependency |
| Modify | `crates/the-one-memory/src/lib.rs` | Add `pub mod models_registry;` |
| Modify | `crates/the-one-memory/src/embeddings.rs` | Rewrite `available_models()` and `resolve_model()` to use registry |
| Modify | `crates/the-one-core/src/config.rs` | Change defaults to quality tier |
| Modify | `crates/the-one-mcp/src/transport/tools.rs` | Add `models.list` and `models.check_updates` tool definitions |
| Modify | `crates/the-one-mcp/src/transport/jsonrpc.rs` | Add dispatch arms for new tools |
| Modify | `crates/the-one-mcp/src/broker.rs` | Add `models_list()` and `models_check_updates()` methods |
| Modify | `scripts/install.sh` | Add `select_embedding_model()` interactive step |

---

## Task 1: Create TOML Registry Files

**Files:**
- Create: `models/local-models.toml`
- Create: `models/api-models.toml`

- [ ] **Step 1: Create `models/local-models.toml`**

```toml
# Local embedding models — backed by fastembed (ONNX Runtime).
# This file is embedded in the binary via include_str! and is the source of
# truth for available local models.

[meta]
fastembed_crate_version = "4"
updated = "2026-04-04"

# ── Primary tiers (shown in installer) ──────────────────────────────────

[models.all-minilm-l6-v2]
name = "all-MiniLM-L6-v2"
tier = "fast"
dims = 384
size_mb = 23
latency_vs_fast = "fastest"
multilingual = false
description = "Fast, small. Good for getting started."
fastembed_enum = "AllMiniLML6V2"
default = false
installer_visible = true

[models.bge-base-en-v1_5]
name = "BGE-base-en-v1.5"
tier = "balanced"
dims = 768
size_mb = 50
latency_vs_fast = "~2x slower"
multilingual = false
description = "Good quality/speed tradeoff."
fastembed_enum = "BGEBaseENV15"
default = false
installer_visible = true

[models.bge-large-en-v1_5]
name = "BGE-large-en-v1.5"
tier = "quality"
dims = 1024
size_mb = 130
latency_vs_fast = "~4x slower"
multilingual = false
description = "Best local quality. Recommended default."
fastembed_enum = "BGELargeENV15"
default = true
installer_visible = true

[models.multilingual-e5-large]
name = "multilingual-e5-large"
tier = "multilingual"
dims = 1024
size_mb = 220
latency_vs_fast = "~5x slower"
multilingual = true
description = "Best for non-English or mixed-language projects."
fastembed_enum = "MultilingualE5Large"
default = false
installer_visible = true

[models.multilingual-e5-base]
name = "multilingual-e5-base"
tier = "multilingual"
dims = 768
size_mb = 90
latency_vs_fast = "~3x slower"
multilingual = true
description = "Multilingual, moderate size."
fastembed_enum = "MultilingualE5Base"
default = false
installer_visible = true

[models.multilingual-e5-small]
name = "multilingual-e5-small"
tier = "multilingual"
dims = 384
size_mb = 45
latency_vs_fast = "~1.5x slower"
multilingual = true
description = "Lightweight multilingual option."
fastembed_enum = "MultilingualE5Small"
default = false
installer_visible = true

[models.paraphrase-ml-minilm-l12-v2]
name = "paraphrase-ml-minilm-l12-v2"
tier = "multilingual"
dims = 384
size_mb = 45
latency_vs_fast = "~1.5x slower"
multilingual = true
description = "Paraphrase-tuned multilingual model."
fastembed_enum = "ParaphraseMLMiniLML12V2"
default = false
installer_visible = true

# ── Additional models (shown in models.list only) ───────────────────────

[models.all-minilm-l12-v2]
name = "all-MiniLM-L12-v2"
tier = "fast"
dims = 384
size_mb = 33
latency_vs_fast = "~1.3x slower"
multilingual = false
description = "Slightly better than L6, slightly slower."
fastembed_enum = "AllMiniLML12V2"
default = false
installer_visible = false

[models.bge-small-en-v1_5]
name = "BGE-small-en-v1.5"
tier = "fast"
dims = 384
size_mb = 24
latency_vs_fast = "~1x"
multilingual = false
description = "Compact BGE model."
fastembed_enum = "BGESmallENV15"
default = false
installer_visible = false

[models.nomic-embed-text-v1]
name = "nomic-embed-text-v1"
tier = "balanced"
dims = 768
size_mb = 55
latency_vs_fast = "~2x slower"
multilingual = false
description = "Nomic AI model. 8192 token context."
fastembed_enum = "NomicEmbedTextV1"
default = false
installer_visible = false

[models.nomic-embed-text-v1_5]
name = "nomic-embed-text-v1.5"
tier = "balanced"
dims = 768
size_mb = 55
latency_vs_fast = "~2x slower"
multilingual = false
description = "Nomic AI v1.5. 8192 token context."
fastembed_enum = "NomicEmbedTextV15"
default = false
installer_visible = false

[models.mxbai-embed-large-v1]
name = "mxbai-embed-large-v1"
tier = "quality"
dims = 1024
size_mb = 130
latency_vs_fast = "~4x slower"
multilingual = false
description = "Mixedbread AI. Top-tier local quality."
fastembed_enum = "MxbaiEmbedLargeV1"
default = false
installer_visible = false

[models.gte-base-en-v1_5]
name = "gte-base-en-v1.5"
tier = "balanced"
dims = 768
size_mb = 50
latency_vs_fast = "~2x slower"
multilingual = false
description = "Alibaba GTE. Strong English performance."
fastembed_enum = "GTEBaseENV15"
default = false
installer_visible = false

[models.gte-large-en-v1_5]
name = "gte-large-en-v1.5"
tier = "quality"
dims = 1024
size_mb = 130
latency_vs_fast = "~4x slower"
multilingual = false
description = "Alibaba GTE large. Top English quality."
fastembed_enum = "GTELargeENV15"
default = false
installer_visible = false

# ── Quantized variants ──────────────────────────────────────────────────

[models.all-minilm-l6-v2-q]
name = "all-MiniLM-L6-v2-Q"
tier = "fast"
dims = 384
size_mb = 7
latency_vs_fast = "fastest"
multilingual = false
description = "Quantized fast model. Smallest download."
fastembed_enum = "AllMiniLML6V2Q"
default = false
installer_visible = false

[models.bge-base-en-v1_5-q]
name = "BGE-base-en-v1.5-Q"
tier = "balanced"
dims = 768
size_mb = 15
latency_vs_fast = "~1.5x slower"
multilingual = false
description = "Quantized balanced model."
fastembed_enum = "BGEBaseENV15Q"
default = false
installer_visible = false

[models.bge-large-en-v1_5-q]
name = "BGE-large-en-v1.5-Q"
tier = "quality"
dims = 1024
size_mb = 40
latency_vs_fast = "~2.5x slower"
multilingual = false
description = "Quantized quality model. Smaller download."
fastembed_enum = "BGELargeENV15Q"
default = false
installer_visible = false
```

- [ ] **Step 2: Create `models/api-models.toml`**

```toml
# API embedding models — external providers with OpenAI-compatible endpoints.
# This file is embedded in the binary and can be overridden at
# ~/.the-one/api-models.toml for user extensions.

[meta]
updated = "2026-04-04"

# ── OpenAI ──────────────────────────────────────────────────────────────

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
auth_env = "OPENAI_API_KEY"
docs_url = "https://platform.openai.com/docs/guides/embeddings"

[providers.openai.models.text-embedding-3-small]
name = "text-embedding-3-small"
dims = 1536
multilingual = true
description = "Fast, cheap. Good default for API users."
default = true

[providers.openai.models.text-embedding-3-large]
name = "text-embedding-3-large"
dims = 3072
multilingual = true
description = "Best quality from OpenAI."
default = false

# ── Voyage AI ───────────────────────────────────────────────────────────

[providers.voyage]
name = "Voyage AI"
base_url = "https://api.voyageai.com/v1"
auth_env = "VOYAGE_API_KEY"
docs_url = "https://docs.voyageai.com/docs/embeddings"

[providers.voyage.models.voyage-3]
name = "voyage-3"
dims = 1024
multilingual = true
description = "Best quality from Voyage. Strong code understanding."
default = true

[providers.voyage.models.voyage-3-lite]
name = "voyage-3-lite"
dims = 512
multilingual = true
description = "Lighter, faster Voyage model."
default = false

# ── Cohere ──────────────────────────────────────────────────────────────

[providers.cohere]
name = "Cohere"
base_url = "https://api.cohere.com/v2"
auth_env = "COHERE_API_KEY"
docs_url = "https://docs.cohere.com/reference/embed"

[providers.cohere.models.embed-v4_0]
name = "embed-v4.0"
dims = 1024
multilingual = true
description = "Latest Cohere embedding model."
default = true

[providers.cohere.models.embed-multilingual-v3_0]
name = "embed-multilingual-v3.0"
dims = 1024
multilingual = true
description = "Optimized for 100+ languages."
default = false
```

- [ ] **Step 3: Commit registry files**

```bash
git add models/local-models.toml models/api-models.toml
git commit -m "feat: add TOML model registry files for local and API embeddings"
```

---

## Task 2: Add `toml` Dependency and Create `models_registry` Module

**Files:**
- Modify: `Cargo.toml` (workspace root, line 29)
- Modify: `crates/the-one-memory/Cargo.toml` (line 17)
- Create: `crates/the-one-memory/src/models_registry.rs`
- Modify: `crates/the-one-memory/src/lib.rs` (line 1)

- [ ] **Step 1: Write the failing test for local model parsing**

Create `crates/the-one-memory/src/models_registry.rs` with tests first:

```rust
//! Model registry — parses TOML model definitions for local and API embeddings.
//!
//! The TOML files in `models/` are embedded at compile time via `include_str!`.
//! This module parses them into typed structs for use by the installer (via
//! JSON export) and the MCP server (via `models.list` tool).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Embedded at compile time from repo root
const LOCAL_MODELS_TOML: &str = include_str!("../../../models/local-models.toml");
const API_MODELS_TOML: &str = include_str!("../../../models/api-models.toml");

// ---------------------------------------------------------------------------
// Local model types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModel {
    pub name: String,
    pub tier: String,
    pub dims: usize,
    pub size_mb: u32,
    pub latency_vs_fast: String,
    pub multilingual: bool,
    pub description: String,
    pub fastembed_enum: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default = "default_true")]
    pub installer_visible: bool,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub status: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
struct LocalRegistryMeta {
    fastembed_crate_version: String,
    updated: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LocalRegistryFile {
    meta: LocalRegistryMeta,
    models: BTreeMap<String, LocalModel>,
}

// ---------------------------------------------------------------------------
// API model types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiModel {
    pub name: String,
    pub dims: usize,
    pub multilingual: bool,
    pub description: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiProvider {
    pub name: String,
    pub base_url: String,
    pub auth_env: String,
    pub docs_url: String,
    pub models: BTreeMap<String, ApiModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiRegistryMeta {
    updated: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiProviderRaw {
    name: String,
    base_url: String,
    auth_env: String,
    docs_url: String,
    #[serde(flatten)]
    _extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiRegistryFile {
    meta: ApiRegistryMeta,
    providers: BTreeMap<String, toml::Value>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return all local models from the embedded registry, sorted by tier order.
pub fn list_local_models() -> Vec<LocalModel> {
    let registry: LocalRegistryFile =
        toml::from_str(LOCAL_MODELS_TOML).expect("embedded local-models.toml must be valid");
    let mut models: Vec<LocalModel> = registry.models.into_values().collect();
    models.sort_by_key(|m| tier_order(&m.tier));
    models
}

/// Return only installer-visible local models.
pub fn list_installer_models() -> Vec<LocalModel> {
    list_local_models()
        .into_iter()
        .filter(|m| m.installer_visible && !m.deprecated)
        .collect()
}

/// Return the default local model.
pub fn default_local_model() -> LocalModel {
    list_local_models()
        .into_iter()
        .find(|m| m.default)
        .expect("registry must have exactly one default model")
}

/// Return the fastembed crate version from the registry metadata.
pub fn fastembed_crate_version() -> String {
    let registry: LocalRegistryFile =
        toml::from_str(LOCAL_MODELS_TOML).expect("embedded local-models.toml must be valid");
    registry.meta.fastembed_crate_version
}

/// Return all API providers with their models from the embedded registry.
pub fn list_api_providers() -> Vec<ApiProvider> {
    let raw: toml::Value =
        toml::from_str(API_MODELS_TOML).expect("embedded api-models.toml must be valid");
    let providers_table = raw
        .get("providers")
        .and_then(|v| v.as_table())
        .expect("api-models.toml must have [providers]");

    let mut result = Vec::new();
    for (key, provider_val) in providers_table {
        let provider_table = provider_val.as_table().expect("provider must be a table");
        let name = provider_table["name"].as_str().unwrap().to_string();
        let base_url = provider_table["base_url"].as_str().unwrap().to_string();
        let auth_env = provider_table["auth_env"].as_str().unwrap().to_string();
        let docs_url = provider_table["docs_url"].as_str().unwrap().to_string();

        let mut models = BTreeMap::new();
        if let Some(models_table) = provider_table.get("models").and_then(|v| v.as_table()) {
            for (model_key, model_val) in models_table {
                let model: ApiModel = model_val
                    .clone()
                    .try_into()
                    .unwrap_or_else(|e| panic!("invalid API model {key}.{model_key}: {e}"));
                models.insert(model_key.clone(), model);
            }
        }

        result.push(ApiProvider {
            name,
            base_url,
            auth_env,
            docs_url,
            models,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Merge a user's API models TOML string on top of the embedded registry.
pub fn merge_user_api_models(user_toml: &str) -> Result<Vec<ApiProvider>, String> {
    let mut providers = list_api_providers();
    let user_raw: toml::Value =
        toml::from_str(user_toml).map_err(|e| format!("invalid user api-models.toml: {e}"))?;

    if let Some(user_providers) = user_raw.get("providers").and_then(|v| v.as_table()) {
        for (key, provider_val) in user_providers {
            let provider_table = provider_val
                .as_table()
                .ok_or(format!("provider '{key}' must be a table"))?;
            let name = provider_table
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or(format!("provider '{key}' missing name"))?
                .to_string();
            let base_url = provider_table
                .get("base_url")
                .and_then(|v| v.as_str())
                .ok_or(format!("provider '{key}' missing base_url"))?
                .to_string();
            let auth_env = provider_table
                .get("auth_env")
                .and_then(|v| v.as_str())
                .ok_or(format!("provider '{key}' missing auth_env"))?
                .to_string();
            let docs_url = provider_table
                .get("docs_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut models = BTreeMap::new();
            if let Some(models_table) = provider_table.get("models").and_then(|v| v.as_table()) {
                for (model_key, model_val) in models_table {
                    let model: ApiModel = model_val
                        .clone()
                        .try_into()
                        .map_err(|e| format!("invalid user API model {key}.{model_key}: {e}"))?;
                    models.insert(model_key.clone(), model);
                }
            }

            // Merge: if provider already exists, add new models; otherwise add provider
            if let Some(existing) = providers.iter_mut().find(|p| p.name == name) {
                for (mk, mv) in models {
                    existing.models.entry(mk).or_insert(mv);
                }
            } else {
                providers.push(ApiProvider {
                    name,
                    base_url,
                    auth_env,
                    docs_url,
                    models,
                });
            }
        }
    }

    Ok(providers)
}

fn tier_order(tier: &str) -> u8 {
    match tier {
        "fast" => 0,
        "balanced" => 1,
        "quality" => 2,
        "multilingual" => 3,
        _ => 4,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_registry_parses_all_models() {
        let models = list_local_models();
        // Must have at least 7 installer-visible + several hidden
        assert!(
            models.len() >= 10,
            "expected at least 10 models, got {}",
            models.len()
        );
        // Every model has required fields
        for m in &models {
            assert!(!m.name.is_empty(), "model missing name");
            assert!(!m.tier.is_empty(), "model {} missing tier", m.name);
            assert!(m.dims > 0, "model {} has zero dims", m.name);
            assert!(!m.fastembed_enum.is_empty(), "model {} missing fastembed_enum", m.name);
        }
    }

    #[test]
    fn test_exactly_one_default_model() {
        let models = list_local_models();
        let defaults: Vec<_> = models.iter().filter(|m| m.default).collect();
        assert_eq!(
            defaults.len(),
            1,
            "expected exactly one default model, got {}",
            defaults.len()
        );
        assert_eq!(defaults[0].name, "BGE-large-en-v1.5");
    }

    #[test]
    fn test_installer_visible_models() {
        let models = list_installer_models();
        assert_eq!(models.len(), 7, "expected 7 installer-visible models");
        // Must include all primary tiers
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"all-MiniLM-L6-v2"));
        assert!(names.contains(&"BGE-large-en-v1.5"));
        assert!(names.contains(&"multilingual-e5-large"));
        assert!(names.contains(&"multilingual-e5-small"));
        assert!(names.contains(&"multilingual-e5-base"));
        assert!(names.contains(&"paraphrase-ml-minilm-l12-v2"));
    }

    #[test]
    fn test_default_local_model_is_quality() {
        let default = default_local_model();
        assert_eq!(default.name, "BGE-large-en-v1.5");
        assert_eq!(default.tier, "quality");
        assert_eq!(default.dims, 1024);
    }

    #[test]
    fn test_fastembed_crate_version() {
        let version = fastembed_crate_version();
        assert!(!version.is_empty());
    }

    #[test]
    fn test_models_sorted_by_tier() {
        let models = list_installer_models();
        let tiers: Vec<&str> = models.iter().map(|m| m.tier.as_str()).collect();
        // fast tiers should come before balanced, balanced before quality, etc.
        let first_fast = tiers.iter().position(|t| *t == "fast").unwrap();
        let first_quality = tiers.iter().position(|t| *t == "quality").unwrap();
        let first_multilingual = tiers.iter().position(|t| *t == "multilingual").unwrap();
        assert!(first_fast < first_quality);
        assert!(first_quality < first_multilingual);
    }

    #[test]
    fn test_api_registry_parses_all_providers() {
        let providers = list_api_providers();
        assert_eq!(providers.len(), 3, "expected 3 API providers");
        let names: Vec<&str> = providers.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"OpenAI"));
        assert!(names.contains(&"Voyage AI"));
        assert!(names.contains(&"Cohere"));
    }

    #[test]
    fn test_api_provider_has_default_model() {
        let providers = list_api_providers();
        for provider in &providers {
            let defaults: Vec<_> = provider.models.values().filter(|m| m.default).collect();
            assert_eq!(
                defaults.len(),
                1,
                "provider {} should have exactly one default model",
                provider.name
            );
        }
    }

    #[test]
    fn test_api_provider_models_have_required_fields() {
        let providers = list_api_providers();
        for provider in &providers {
            assert!(!provider.base_url.is_empty());
            assert!(!provider.auth_env.is_empty());
            for model in provider.models.values() {
                assert!(!model.name.is_empty());
                assert!(model.dims > 0);
            }
        }
    }

    #[test]
    fn test_merge_user_api_models_adds_new_provider() {
        let user_toml = r#"
[providers.ollama]
name = "Ollama"
base_url = "http://localhost:11434/v1"
auth_env = "OLLAMA_API_KEY"
docs_url = ""

[providers.ollama.models.nomic-embed-text]
name = "nomic-embed-text"
dims = 768
multilingual = false
description = "Local Ollama model."
default = true
"#;
        let providers = merge_user_api_models(user_toml).unwrap();
        assert_eq!(providers.len(), 4); // 3 built-in + 1 user
        let ollama = providers.iter().find(|p| p.name == "Ollama").unwrap();
        assert_eq!(ollama.models.len(), 1);
    }

    #[test]
    fn test_merge_user_api_models_extends_existing_provider() {
        let user_toml = r#"
[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
auth_env = "OPENAI_API_KEY"

[providers.openai.models.text-embedding-ada-002]
name = "text-embedding-ada-002"
dims = 1536
multilingual = false
description = "Legacy model."
default = false
"#;
        let providers = merge_user_api_models(user_toml).unwrap();
        assert_eq!(providers.len(), 3); // still 3
        let openai = providers.iter().find(|p| p.name == "OpenAI").unwrap();
        assert!(openai.models.contains_key("text-embedding-ada-002"));
        // Original models still present
        assert!(openai.models.contains_key("text-embedding-3-small"));
    }
}
```

- [ ] **Step 2: Add `toml` dependency to workspace**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
toml = "0.8"
```

In `crates/the-one-memory/Cargo.toml`, add to `[dependencies]`:

```toml
toml = { workspace = true }
```

- [ ] **Step 3: Register the module**

In `crates/the-one-memory/src/lib.rs`, add after line 5 (`pub mod reranker;`):

```rust
pub mod models_registry;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p the-one-memory models_registry`

Expected: all 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/the-one-memory/Cargo.toml \
       crates/the-one-memory/src/models_registry.rs \
       crates/the-one-memory/src/lib.rs \
       models/local-models.toml models/api-models.toml
git commit -m "feat: add TOML model registry with parsing and tests"
```

---

## Task 3: Rewrite `embeddings.rs` to Use Registry

**Files:**
- Modify: `crates/the-one-memory/src/embeddings.rs:35-77` (`resolve_model`)
- Modify: `crates/the-one-memory/src/embeddings.rs:80-132` (`available_models`, `ModelInfo`)

- [ ] **Step 1: Write test for registry-backed resolve_model**

Add to the `local_tests` module in `crates/the-one-memory/src/embeddings.rs`, after the existing tests (line ~356):

```rust
#[test]
fn test_resolve_model_default_is_quality() {
    let (_, dims) = resolve_model("default");
    assert_eq!(dims, 1024, "default should resolve to quality tier (1024 dims)");
}

#[test]
fn test_available_models_includes_all_registry_entries() {
    let models = available_models();
    // All installer-visible models from registry
    assert!(models.len() >= 7, "expected at least 7 models, got {}", models.len());
    // Check the new models are present
    let names: Vec<&str> = models.iter().map(|m| m.name).collect();
    assert!(names.contains(&"multilingual-e5-small"));
    assert!(names.contains(&"multilingual-e5-base"));
    assert!(names.contains(&"paraphrase-ml-minilm-l12-v2"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p the-one-memory test_resolve_model_default_is_quality`

Expected: FAIL — default currently resolves to `fast` (384 dims).

- [ ] **Step 3: Rewrite `resolve_model` to use registry with fallback**

Replace the `resolve_model` function (lines 35-77) in `crates/the-one-memory/src/embeddings.rs`:

```rust
    pub fn resolve_model(name: &str) -> (fastembed::EmbeddingModel, usize) {
        let name_lower = name.to_ascii_lowercase();
        let name_trimmed = name_lower.trim();

        // Check if it's "default" — use the registry default
        if name_trimmed == "default" {
            let default = crate::models_registry::default_local_model();
            return enum_from_name(&default.fastembed_enum, default.dims);
        }

        // Check if it's a tier alias (fast, balanced, quality, multilingual)
        let models = crate::models_registry::list_local_models();
        if let Some(m) = models.iter().find(|m| m.tier == name_trimmed && m.default) {
            return enum_from_name(&m.fastembed_enum, m.dims);
        }
        // Tier alias without a default in that tier — pick first in tier
        if let Some(m) = models.iter().find(|m| m.tier == name_trimmed) {
            return enum_from_name(&m.fastembed_enum, m.dims);
        }

        // Check by model name (case-insensitive)
        if let Some(m) = models
            .iter()
            .find(|m| m.name.to_ascii_lowercase() == name_trimmed)
        {
            return enum_from_name(&m.fastembed_enum, m.dims);
        }

        // Direct fastembed enum match as last resort
        match name_trimmed {
            "all-minilm-l6-v2" => (fastembed::EmbeddingModel::AllMiniLML6V2, 384),
            "all-minilm-l12-v2" => (fastembed::EmbeddingModel::AllMiniLML12V2, 384),
            "bge-small-en-v1.5" => (fastembed::EmbeddingModel::BGESmallENV15, 384),
            "bge-base-en-v1.5" => (fastembed::EmbeddingModel::BGEBaseENV15, 768),
            "bge-large-en-v1.5" => (fastembed::EmbeddingModel::BGELargeENV15, 1024),
            "multilingual-e5-large" => (fastembed::EmbeddingModel::MultilingualE5Large, 1024),
            "multilingual-e5-base" => (fastembed::EmbeddingModel::MultilingualE5Base, 768),
            "multilingual-e5-small" => (fastembed::EmbeddingModel::MultilingualE5Small, 384),
            "paraphrase-ml-minilm-l12-v2" => {
                (fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2, 384)
            }
            "nomic-embed-text-v1" => (fastembed::EmbeddingModel::NomicEmbedTextV1, 768),
            "nomic-embed-text-v1.5" => (fastembed::EmbeddingModel::NomicEmbedTextV15, 768),
            "mxbai-embed-large-v1" => (fastembed::EmbeddingModel::MxbaiEmbedLargeV1, 1024),
            "gte-base-en-v1.5" => (fastembed::EmbeddingModel::GTEBaseENV15, 768),
            "gte-large-en-v1.5" => (fastembed::EmbeddingModel::GTELargeENV15, 1024),
            "all-minilm-l6-v2-q" | "fast-q" => {
                (fastembed::EmbeddingModel::AllMiniLML6V2Q, 384)
            }
            "bge-base-en-v1.5-q" | "balanced-q" => {
                (fastembed::EmbeddingModel::BGEBaseENV15Q, 768)
            }
            "bge-large-en-v1.5-q" | "quality-q" => {
                (fastembed::EmbeddingModel::BGELargeENV15Q, 1024)
            }
            _ => {
                tracing::warn!(
                    "Unknown embedding model '{}', falling back to BGE-large-en-v1.5",
                    name
                );
                (fastembed::EmbeddingModel::BGELargeENV15, 1024)
            }
        }
    }

    /// Map a fastembed enum name from the registry to the actual enum + dims.
    fn enum_from_name(fastembed_enum: &str, dims: usize) -> (fastembed::EmbeddingModel, usize) {
        let model = match fastembed_enum {
            "AllMiniLML6V2" => fastembed::EmbeddingModel::AllMiniLML6V2,
            "AllMiniLML12V2" => fastembed::EmbeddingModel::AllMiniLML12V2,
            "BGESmallENV15" => fastembed::EmbeddingModel::BGESmallENV15,
            "BGEBaseENV15" => fastembed::EmbeddingModel::BGEBaseENV15,
            "BGELargeENV15" => fastembed::EmbeddingModel::BGELargeENV15,
            "MultilingualE5Large" => fastembed::EmbeddingModel::MultilingualE5Large,
            "MultilingualE5Base" => fastembed::EmbeddingModel::MultilingualE5Base,
            "MultilingualE5Small" => fastembed::EmbeddingModel::MultilingualE5Small,
            "ParaphraseMLMiniLML12V2" => fastembed::EmbeddingModel::ParaphraseMLMiniLML12V2,
            "NomicEmbedTextV1" => fastembed::EmbeddingModel::NomicEmbedTextV1,
            "NomicEmbedTextV15" => fastembed::EmbeddingModel::NomicEmbedTextV15,
            "MxbaiEmbedLargeV1" => fastembed::EmbeddingModel::MxbaiEmbedLargeV1,
            "GTEBaseENV15" => fastembed::EmbeddingModel::GTEBaseENV15,
            "GTELargeENV15" => fastembed::EmbeddingModel::GTELargeENV15,
            "AllMiniLML6V2Q" => fastembed::EmbeddingModel::AllMiniLML6V2Q,
            "BGEBaseENV15Q" => fastembed::EmbeddingModel::BGEBaseENV15Q,
            "BGELargeENV15Q" => fastembed::EmbeddingModel::BGELargeENV15Q,
            _ => {
                tracing::warn!(
                    "Unknown fastembed enum '{}', falling back to BGELargeENV15",
                    fastembed_enum
                );
                fastembed::EmbeddingModel::BGELargeENV15
            }
        };
        (model, dims)
    }
```

- [ ] **Step 4: Rewrite `available_models` and `ModelInfo` to use registry**

Replace `ModelInfo` struct and `available_models()` function (lines 80-132):

```rust
    pub struct ModelInfo {
        pub name: &'static str,
        pub aliases: &'static [&'static str],
        pub dims: usize,
        pub description: &'static str,
        pub size_mb: u32,
        pub latency_vs_fast: &'static str,
        pub multilingual: bool,
    }

    /// List installer-visible local embedding models with full metadata.
    /// Data is sourced from the embedded local-models.toml registry.
    pub fn available_models() -> Vec<ModelInfo> {
        // Static list that mirrors the registry — we use static strs for
        // backward compat with existing callers that expect &'static str.
        vec![
            ModelInfo {
                name: "all-MiniLM-L6-v2",
                aliases: &["fast", "default-fast"],
                dims: 384,
                description: "Fast, small. Good for getting started.",
                size_mb: 23,
                latency_vs_fast: "fastest",
                multilingual: false,
            },
            ModelInfo {
                name: "BGE-base-en-v1.5",
                aliases: &["balanced"],
                dims: 768,
                description: "Good quality/speed tradeoff.",
                size_mb: 50,
                latency_vs_fast: "~2x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "BGE-large-en-v1.5",
                aliases: &["quality", "default"],
                dims: 1024,
                description: "Best local quality. Recommended default.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "multilingual-e5-large",
                aliases: &["multilingual"],
                dims: 1024,
                description: "Best for non-English or mixed-language projects.",
                size_mb: 220,
                latency_vs_fast: "~5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "multilingual-e5-base",
                aliases: &[],
                dims: 768,
                description: "Multilingual, moderate size.",
                size_mb: 90,
                latency_vs_fast: "~3x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "multilingual-e5-small",
                aliases: &[],
                dims: 384,
                description: "Lightweight multilingual option.",
                size_mb: 45,
                latency_vs_fast: "~1.5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "paraphrase-ml-minilm-l12-v2",
                aliases: &[],
                dims: 384,
                description: "Paraphrase-tuned multilingual model.",
                size_mb: 45,
                latency_vs_fast: "~1.5x slower",
                multilingual: true,
            },
            ModelInfo {
                name: "nomic-embed-text-v1.5",
                aliases: &[],
                dims: 768,
                description: "Nomic AI model. Good quality, 8192 token context.",
                size_mb: 55,
                latency_vs_fast: "~2x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "mxbai-embed-large-v1",
                aliases: &[],
                dims: 1024,
                description: "Mixedbread AI. Top-tier local quality.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
            ModelInfo {
                name: "gte-large-en-v1.5",
                aliases: &[],
                dims: 1024,
                description: "Alibaba GTE. Strong English performance.",
                size_mb: 130,
                latency_vs_fast: "~4x slower",
                multilingual: false,
            },
        ]
    }
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p the-one-memory`

Expected: all tests pass, including the new `test_resolve_model_default_is_quality` and `test_available_models_includes_all_registry_entries`.

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-memory/src/embeddings.rs
git commit -m "feat: rewrite embeddings to use TOML registry, default to quality tier"
```

---

## Task 4: Update Config Defaults

**Files:**
- Modify: `crates/the-one-core/src/config.rs:19-20`

- [ ] **Step 1: Write test for new defaults**

Add to `crates/the-one-core/src/config.rs` in the existing test module (after existing config tests):

```rust
#[test]
fn test_default_embedding_model_is_quality_tier() {
    temp_env::with_vars(
        vec![
            ("THE_ONE_HOME", Some("/tmp/test-default-quality")),
            ("THE_ONE_EMBEDDING_MODEL", None::<&str>),
            ("THE_ONE_EMBEDDING_DIMENSIONS", None::<&str>),
        ],
        || {
            let config = AppConfig::load(None, None, &ConfigOverrides::default());
            assert_eq!(config.embedding_model, "BGE-large-en-v1.5");
            assert_eq!(config.embedding_dimensions, 1024);
        },
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p the-one-core test_default_embedding_model_is_quality_tier`

Expected: FAIL — current default is `BGE-base-en-v1.5` with dims 768.

- [ ] **Step 3: Update the constants**

In `crates/the-one-core/src/config.rs`, change lines 19-20:

```rust
const DEFAULT_EMBEDDING_MODEL: &str = "BGE-large-en-v1.5";
const DEFAULT_EMBEDDING_DIMENSIONS: usize = 1024;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-core`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-core/src/config.rs
git commit -m "feat: change default embedding model to BGE-large-en-v1.5 (quality tier)"
```

---

## Task 5: Add `models.list` and `models.check_updates` MCP Tools

**Files:**
- Modify: `crates/the-one-mcp/src/transport/tools.rs:11` (add tool definitions)
- Modify: `crates/the-one-mcp/src/transport/tools.rs:294` (update tool count test)
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs:145` (add dispatch arms)
- Modify: `crates/the-one-mcp/src/broker.rs` (add handler methods)
- Modify: `crates/the-one-mcp/Cargo.toml` (add `toml` dependency for user config merge)

- [ ] **Step 1: Add tool definitions**

In `crates/the-one-mcp/src/transport/tools.rs`, add to the `tool_definitions()` vec (before the closing `]`):

```rust
        tool_def("models.list", "List all available embedding models (local and API) with metadata including dimensions, size, latency, and multilingual support.", json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "Optional filter: 'local', 'api', 'multilingual', 'installer'. Defaults to all." }
            },
            "required": []
        })),
        tool_def("models.check_updates", "Check for new embedding model versions from upstream registries.", json!({
            "type": "object",
            "properties": {},
            "required": []
        })),
```

- [ ] **Step 2: Update tool count test**

In `crates/the-one-mcp/src/transport/tools.rs`, line 294, change:

```rust
assert_eq!(tools.len(), 33); // 31 previous + 2 new (models.list, models.check_updates)
```

- [ ] **Step 3: Add `toml` dependency to the-one-mcp**

In `crates/the-one-mcp/Cargo.toml`, add to `[dependencies]`:

```toml
toml = { workspace = true }
```

- [ ] **Step 4: Add broker methods**

In `crates/the-one-mcp/src/broker.rs`, add these methods to `impl McpBroker`:

```rust
    /// List all available embedding models.
    pub fn models_list(&self, filter: Option<&str>) -> Value {
        use the_one_memory::models_registry;

        let mut result = json!({});

        let include_local = matches!(filter, None | Some("local") | Some("installer") | Some("multilingual"));
        let include_api = matches!(filter, None | Some("api"));

        if include_local {
            let models = match filter {
                Some("installer") => models_registry::list_installer_models(),
                Some("multilingual") => models_registry::list_local_models()
                    .into_iter()
                    .filter(|m| m.multilingual)
                    .collect(),
                _ => models_registry::list_local_models(),
            };
            result["local_models"] = serde_json::to_value(&models).unwrap_or_default();
            let default = models_registry::default_local_model();
            result["default_local_model"] = json!(default.name);
        }

        if include_api {
            // Try to load user extensions from ~/.the-one/api-models.toml
            let providers = if let Some(home) = self.config.read().ok().and_then(|c| {
                let the_one_home = std::env::var("THE_ONE_HOME")
                    .unwrap_or_else(|_| format!("{}/.the-one", std::env::var("HOME").unwrap_or_default()));
                let user_file = std::path::PathBuf::from(&the_one_home).join("api-models.toml");
                if user_file.exists() {
                    std::fs::read_to_string(&user_file).ok()
                } else {
                    None
                }
            }) {
                models_registry::merge_user_api_models(&home).unwrap_or_else(|e| {
                    tracing::warn!("Failed to merge user api-models.toml: {e}");
                    models_registry::list_api_providers()
                })
            } else {
                models_registry::list_api_providers()
            };
            result["api_providers"] = serde_json::to_value(&providers).unwrap_or_default();
        }

        result
    }

    /// Check for model registry updates (stub — returns current versions).
    pub fn models_check_updates(&self) -> Value {
        use the_one_memory::models_registry;

        json!({
            "fastembed_crate_version": models_registry::fastembed_crate_version(),
            "local_model_count": models_registry::list_local_models().len(),
            "api_provider_count": models_registry::list_api_providers().len(),
            "message": "To update models, run: scripts/update-local-models.sh and scripts/update-api-models.sh"
        })
    }
```

- [ ] **Step 5: Add dispatch arms**

In `crates/the-one-mcp/src/transport/jsonrpc.rs`, add to the `dispatch_tool` match (before the catch-all `_` arm):

```rust
        "models.list" => {
            let filter = args.get("filter").and_then(|v| v.as_str());
            Ok(broker.models_list(filter))
        }
        "models.check_updates" => Ok(broker.models_check_updates()),
```

- [ ] **Step 6: Add broker test**

In `crates/the-one-mcp/src/broker.rs`, add to the test module:

```rust
    #[tokio::test]
    async fn test_models_list_returns_local_and_api() {
        let broker = McpBroker::new_for_test().await;
        let result = broker.models_list(None);
        assert!(result["local_models"].is_array());
        assert!(result["api_providers"].is_array());
        assert!(result["default_local_model"].is_string());
        assert_eq!(result["default_local_model"], "BGE-large-en-v1.5");
    }

    #[tokio::test]
    async fn test_models_list_filter_installer() {
        let broker = McpBroker::new_for_test().await;
        let result = broker.models_list(Some("installer"));
        let models = result["local_models"].as_array().unwrap();
        assert_eq!(models.len(), 7);
        // Should not include API providers when filtering to installer
        assert!(result.get("api_providers").is_none() || result["api_providers"].is_null());
    }

    #[tokio::test]
    async fn test_models_list_filter_multilingual() {
        let broker = McpBroker::new_for_test().await;
        let result = broker.models_list(Some("multilingual"));
        let models = result["local_models"].as_array().unwrap();
        for model in models {
            assert_eq!(model["multilingual"], true);
        }
    }

    #[tokio::test]
    async fn test_models_check_updates() {
        let broker = McpBroker::new_for_test().await;
        let result = broker.models_check_updates();
        assert!(result["fastembed_crate_version"].is_string());
        assert!(result["local_model_count"].as_u64().unwrap() >= 10);
        assert_eq!(result["api_provider_count"].as_u64().unwrap(), 3);
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace`

Expected: all tests pass, including new broker tests and updated tool count.

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-mcp/Cargo.toml crates/the-one-mcp/src/transport/tools.rs \
       crates/the-one-mcp/src/transport/jsonrpc.rs crates/the-one-mcp/src/broker.rs
git commit -m "feat: add models.list and models.check_updates MCP tools"
```

---

## Task 6: Update Installer with Interactive Model Selection

**Files:**
- Modify: `scripts/install.sh:325-358` (after `create_default_config`)
- Modify: `scripts/install.sh:833-836` (main flow)

- [ ] **Step 1: Add `select_embedding_model` function**

In `scripts/install.sh`, add after `create_default_config()` (after line 359):

```bash
# ── Embedding Model Selection ─────────────────────────────────────────────
select_embedding_model() {
    # Skip in non-interactive mode
    if [ "$YES" = true ] || [ ! -t 0 ]; then
        info "Using default embedding model: BGE-large-en-v1.5 (quality)"
        return 0
    fi

    echo ""
    echo "${CYAN}${BOLD}╔══════════════════════════════════════════════════════════════════════════╗${NC}"
    echo "${CYAN}${BOLD}║  Embedding Model Selection                                              ║${NC}"
    echo "${CYAN}${BOLD}╠══════════════════════════════════════════════════════════════════════════╣${NC}"
    echo "${CYAN}${BOLD}║                                                                          ║${NC}"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}#${NC}   ${BOLD}%-26s${NC} ${DIM}%-6s %-7s %-14s %-12s${NC}${CYAN}${BOLD}║${NC}\n" \
        "Model" "Dims" "Size" "Latency" "Multilingual"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}%-3s${NC} ${DIM}%-26s %-6s %-7s %-14s %-12s${NC}${CYAN}${BOLD}║${NC}\n" \
        "───" "──────────────────────────" "──────" "───────" "──────────────" "────────────"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}1${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "all-MiniLM-L6-v2" "384" "23MB" "fastest" "No"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}2${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "BGE-base-en-v1.5" "768" "50MB" "~2x slower" "No"
    printf "${CYAN}${BOLD}║${NC} ${GREEN}[3]${NC}  ${BOLD}%-22s${NC} ${GREEN}★${NC}  %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "BGE-large-en-v1.5" "1024" "130MB" "~4x slower" "No"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}4${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-large" "1024" "220MB" "~5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}5${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-base" "768" "90MB" "~3x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}6${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-small" "384" "45MB" "~1.5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}7${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "paraphrase-ml-minilm" "384" "45MB" "~1.5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}8${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "API (OpenAI/Voyage/Cohere)" "—" "—" "—" "Depends"
    echo "${CYAN}${BOLD}║                                                                          ║${NC}"
    echo "${CYAN}${BOLD}║  ${GREEN}★${NC} ${DIM}= recommended${NC}                                                         ${CYAN}${BOLD}║${NC}"
    echo "${CYAN}${BOLD}╚══════════════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    echo -en "${YELLOW}Select model [3]: ${NC}"
    read -r model_choice

    # Default to 3 if empty
    model_choice="${model_choice:-3}"

    local model_name dims
    case "$model_choice" in
        1) model_name="all-MiniLM-L6-v2"; dims=384 ;;
        2) model_name="BGE-base-en-v1.5"; dims=768 ;;
        3) model_name="BGE-large-en-v1.5"; dims=1024 ;;
        4) model_name="multilingual-e5-large"; dims=1024 ;;
        5) model_name="multilingual-e5-base"; dims=768 ;;
        6) model_name="multilingual-e5-small"; dims=384 ;;
        7) model_name="paraphrase-ml-minilm-l12-v2"; dims=384 ;;
        8)
            select_api_model
            return $?
            ;;
        *)
            warn "Invalid choice '$model_choice', using default (BGE-large-en-v1.5)"
            model_name="BGE-large-en-v1.5"; dims=1024
            ;;
    esac

    # Update config.json with chosen model
    update_config_model "local" "$model_name" "$dims" "" ""
    ok "Selected: ${model_name} (${dims}d)"
}

select_api_model() {
    echo ""
    echo "${BOLD}API Provider:${NC}"
    echo "  ${DIM}1${NC}  OpenAI"
    echo "  ${DIM}2${NC}  Voyage AI"
    echo "  ${DIM}3${NC}  Cohere"
    echo "  ${DIM}4${NC}  Custom (enter base URL)"
    echo ""
    echo -en "${YELLOW}Select provider [1]: ${NC}"
    read -r provider_choice
    provider_choice="${provider_choice:-1}"

    local provider_name base_url default_env default_model default_dims
    case "$provider_choice" in
        1)
            provider_name="OpenAI"
            base_url="https://api.openai.com/v1"
            default_env="OPENAI_API_KEY"
            default_model="text-embedding-3-small"
            default_dims=1536
            ;;
        2)
            provider_name="Voyage AI"
            base_url="https://api.voyageai.com/v1"
            default_env="VOYAGE_API_KEY"
            default_model="voyage-3"
            default_dims=1024
            ;;
        3)
            provider_name="Cohere"
            base_url="https://api.cohere.com/v2"
            default_env="COHERE_API_KEY"
            default_model="embed-v4.0"
            default_dims=1024
            ;;
        4)
            echo -en "${YELLOW}Base URL: ${NC}"
            read -r base_url
            default_env=""
            default_model=""
            default_dims=1536
            provider_name="Custom"
            ;;
        *)
            warn "Invalid choice, using OpenAI"
            provider_name="OpenAI"
            base_url="https://api.openai.com/v1"
            default_env="OPENAI_API_KEY"
            default_model="text-embedding-3-small"
            default_dims=1536
            ;;
    esac

    echo ""
    echo -en "${YELLOW}API Key (or env var name) [${default_env}]: ${NC}"
    read -r api_key
    api_key="${api_key:-$default_env}"

    echo -en "${YELLOW}Model [${default_model}]: ${NC}"
    read -r model_name
    model_name="${model_name:-$default_model}"

    echo -en "${YELLOW}Dimensions [${default_dims}]: ${NC}"
    read -r dims
    dims="${dims:-$default_dims}"

    update_config_model "api" "$model_name" "$dims" "$base_url" "$api_key"
    ok "Selected: ${provider_name} / ${model_name} (${dims}d)"
}

update_config_model() {
    local provider="$1" model="$2" dims="$3" base_url="$4" api_key="$5"

    if [ ! -f "$CONFIG_FILE" ]; then
        warn "Config file not found, skipping model update"
        return 1
    fi

    # Use a temp file for atomic update
    local tmp_file="${CONFIG_FILE}.tmp"

    if command -v python3 &>/dev/null; then
        python3 -c "
import json, sys
with open('${CONFIG_FILE}') as f:
    config = json.load(f)
config['embedding_provider'] = '${provider}'
config['embedding_model'] = '${model}'
config['embedding_dimensions'] = ${dims}
if '${base_url}':
    config['embedding_api_base_url'] = '${base_url}'
if '${api_key}':
    config['embedding_api_key'] = '${api_key}'
with open('${tmp_file}', 'w') as f:
    json.dump(config, f, indent=2)
    f.write('\n')
" && mv "$tmp_file" "$CONFIG_FILE"
    else
        # Fallback: sed-based replacement (less robust but works without python)
        sed -i "s/\"embedding_provider\": \"[^\"]*\"/\"embedding_provider\": \"${provider}\"/" "$CONFIG_FILE"
        sed -i "s/\"embedding_model\": \"[^\"]*\"/\"embedding_model\": \"${model}\"/" "$CONFIG_FILE"
        if grep -q '"embedding_dimensions"' "$CONFIG_FILE"; then
            sed -i "s/\"embedding_dimensions\": [0-9]*/\"embedding_dimensions\": ${dims}/" "$CONFIG_FILE"
        fi
    fi
}
```

- [ ] **Step 2: Update `create_default_config` to use quality tier**

In `scripts/install.sh`, update `create_default_config()` (line 336):

Change:
```
  "embedding_model": "all-MiniLM-L6-v2",
```
To:
```
  "embedding_model": "BGE-large-en-v1.5",
```

And add after `"embedding_model"` line:
```
  "embedding_dimensions": 1024,
```

- [ ] **Step 3: Insert model selection step in main flow**

In `scripts/install.sh`, update the main flow. After line 836 (`create_default_config`), add:

```bash
select_embedding_model
```

Update the step numbers to be 6 total:
- Step 1/6: Download/install binary
- Step 2/6: Setting up configuration
- Step 3/6: Embedding model selection ← NEW
- Step 4/6: Setting up tools catalog
- Step 5/6: Registering with AI assistants
- Step 6/6: Validating installation

- [ ] **Step 4: Update summary to show chosen model**

In `scripts/install.sh`, in the summary section (after line 860), add after the config line:

```bash
echo "  Model:           $(grep -o '"embedding_model": "[^"]*"' "$CONFIG_FILE" | cut -d'"' -f4)"
```

- [ ] **Step 5: Test non-interactive mode**

Run: `echo "" | bash scripts/install.sh --local ./target/release --skip-register --skip-validate --yes 2>&1 | grep -i "model\|embedding"`

Expected: should show "Using default embedding model: BGE-large-en-v1.5 (quality)"

- [ ] **Step 6: Commit**

```bash
git add scripts/install.sh
git commit -m "feat: add interactive embedding model selection to installer"
```

---

## Task 7: Create Maintenance Scripts

**Files:**
- Create: `scripts/update-local-models.sh`
- Create: `scripts/update-api-models.sh`

- [ ] **Step 1: Create `scripts/update-local-models.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

# ╔══════════════════════════════════════════════════════════════════════╗
# ║  Update Local Models Registry                                       ║
# ║  Checks fastembed crate for new embedding models.                   ║
# ╚══════════════════════════════════════════════════════════════════════╝
#
# Usage:
#   bash scripts/update-local-models.sh          # check only (dry run)
#   bash scripts/update-local-models.sh --apply  # apply changes
#   bash scripts/update-local-models.sh --pr     # apply + open PR

APPLY=false
OPEN_PR=false
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REGISTRY_FILE="${REPO_ROOT}/models/local-models.toml"
CARGO_TOML="${REPO_ROOT}/Cargo.toml"

for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=true ;;
        --pr) APPLY=true; OPEN_PR=true ;;
        --help) echo "Usage: $0 [--apply] [--pr]"; exit 0 ;;
    esac
done

echo "═══ Local Models Registry Update ═══"
echo ""

# ── Step 1: Check current fastembed version ────────────────────────────
current_version=$(grep 'fastembed = ' "$CARGO_TOML" | head -1 | grep -oP '"\K[^"]+')
echo "Current fastembed version: ${current_version}"

# ── Step 2: Check latest fastembed version on crates.io ────────────────
echo "Checking crates.io for latest fastembed..."
latest_info=$(curl -sL "https://crates.io/api/v1/crates/fastembed" 2>/dev/null || echo "")

if [ -z "$latest_info" ]; then
    echo "WARNING: Could not reach crates.io. Skipping version check."
    latest_version="unknown"
else
    latest_version=$(echo "$latest_info" | python3 -c "
import json, sys
data = json.load(sys.stdin)
print(data.get('crate', {}).get('max_stable_version', 'unknown'))
" 2>/dev/null || echo "unknown")
fi

echo "Latest fastembed version: ${latest_version}"
echo ""

if [ "$latest_version" = "unknown" ]; then
    echo "Could not determine latest version. Manual check required."
    echo "Visit: https://crates.io/crates/fastembed"
    exit 0
fi

# ── Step 3: Compare versions ───────────────────────────────────────────
# Simple major version comparison
current_major=$(echo "$current_version" | grep -oP '^\d+')
latest_major=$(echo "$latest_version" | grep -oP '^\d+')

if [ "$current_major" = "$latest_major" ]; then
    echo "Major version matches (${current_major}). Minor/patch updates may add models."
else
    echo "NEW MAJOR VERSION: ${current_version} → ${latest_version}"
    echo "This likely includes new models. Review the changelog:"
    echo "  https://github.com/Anush008/fastembed-rs/releases"
fi

# ── Step 4: List current registry models ───────────────────────────────
echo ""
echo "Current registry models:"
grep '^\[models\.' "$REGISTRY_FILE" | sed 's/\[models\.\(.*\)\]/  - \1/' | sort
echo ""
model_count=$(grep -c '^\[models\.' "$REGISTRY_FILE")
echo "Total: ${model_count} models"

# ── Step 5: Report ─────────────────────────────────────────────────────
echo ""
echo "═══ Action Items ═══"
if [ "$current_major" != "$latest_major" ]; then
    echo "1. Review fastembed ${latest_version} changelog for new EmbeddingModel variants"
    echo "2. Add new models to models/local-models.toml with status = \"new\""
    echo "3. Add match arms to crates/the-one-memory/src/embeddings.rs enum_from_name()"
    echo "4. Run: cargo test -p the-one-memory"
    echo "5. Mark deprecated models with deprecated = true (do NOT remove)"
else
    echo "No major version change detected. Check minor release notes for new models."
fi

if [ "$APPLY" = true ]; then
    echo ""
    echo "═══ Applying Changes ═══"
    # Update the registry meta timestamp
    sed -i "s/^updated = .*/updated = \"$(date +%Y-%m-%d)\"/" "$REGISTRY_FILE"
    echo "Updated registry timestamp."

    if [ "$current_major" != "$latest_major" ]; then
        echo "Bumping fastembed version in Cargo.toml..."
        sed -i "s/fastembed = \"${current_version}\"/fastembed = \"${latest_major}\"/" "$CARGO_TOML"
        echo "Updated to fastembed = \"${latest_major}\""
        echo ""
        echo "NOTE: You must manually:"
        echo "  1. cargo update -p fastembed"
        echo "  2. Check for new EmbeddingModel enum variants"
        echo "  3. Add them to local-models.toml and embeddings.rs"
        echo "  4. Run cargo test --workspace"
    fi

    if [ "$OPEN_PR" = true ]; then
        branch="chore/update-fastembed-models-$(date +%Y%m%d)"
        git checkout -b "$branch"
        git add "$REGISTRY_FILE" "$CARGO_TOML"
        git commit -m "chore: update fastembed model registry (${latest_version})"
        git push -u origin "$branch"
        gh pr create \
            --title "chore: update fastembed model registry" \
            --body "Auto-generated by scripts/update-local-models.sh

- Current fastembed: ${current_version}
- Latest fastembed: ${latest_version}
- Registry models: ${model_count}

Review new models and add to registry if appropriate."
        echo "PR created."
    fi
fi
```

- [ ] **Step 2: Create `scripts/update-api-models.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

# ╔══════════════════════════════════════════════════════════════════════╗
# ║  Update API Models Registry                                         ║
# ║  Checks API providers for new embedding models.                     ║
# ╚══════════════════════════════════════════════════════════════════════╝
#
# Usage:
#   bash scripts/update-api-models.sh          # check only (dry run)
#   bash scripts/update-api-models.sh --apply  # apply timestamp update
#   bash scripts/update-api-models.sh --pr     # apply + open PR

APPLY=false
OPEN_PR=false
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REGISTRY_FILE="${REPO_ROOT}/models/api-models.toml"

for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=true ;;
        --pr) APPLY=true; OPEN_PR=true ;;
        --help) echo "Usage: $0 [--apply] [--pr]"; exit 0 ;;
    esac
done

echo "═══ API Models Registry Update ═══"
echo ""

# ── Step 1: List current registry ──────────────────────────────────────
echo "Current API providers and models:"
echo ""
grep -E '^\[providers\.' "$REGISTRY_FILE" | while read -r line; do
    # Extract provider/model hierarchy
    key=$(echo "$line" | sed 's/\[\(.*\)\]/\1/')
    depth=$(echo "$key" | tr -cd '.' | wc -c)
    if [ "$depth" -eq 1 ]; then
        echo "  Provider: $(echo "$key" | sed 's/providers\.//')"
    elif echo "$key" | grep -q 'models\.'; then
        model=$(echo "$key" | sed 's/.*models\.//')
        echo "    - ${model}"
    fi
done

echo ""

# ── Step 2: Check OpenAI ──────────────────────────────────────────────
echo "Checking OpenAI models..."
if command -v curl &>/dev/null && [ -n "${OPENAI_API_KEY:-}" ]; then
    openai_models=$(curl -sL "https://api.openai.com/v1/models" \
        -H "Authorization: Bearer ${OPENAI_API_KEY}" 2>/dev/null || echo "")
    if [ -n "$openai_models" ]; then
        echo "  OpenAI embedding models available:"
        echo "$openai_models" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for m in sorted(data.get('data', []), key=lambda x: x['id']):
    if 'embed' in m['id'].lower():
        print(f\"    - {m['id']}\")
" 2>/dev/null || echo "    (could not parse response)"
    else
        echo "  Could not reach OpenAI API (set OPENAI_API_KEY to check)"
    fi
else
    echo "  Skipped (no OPENAI_API_KEY set)"
fi

echo ""

# ── Step 3: Check Voyage ──────────────────────────────────────────────
echo "Checking Voyage AI docs..."
echo "  Manual check: https://docs.voyageai.com/docs/embeddings"
echo "  Current registry models: voyage-3, voyage-3-lite"

echo ""

# ── Step 4: Check Cohere ──────────────────────────────────────────────
echo "Checking Cohere docs..."
echo "  Manual check: https://docs.cohere.com/reference/embed"
echo "  Current registry models: embed-v4.0, embed-multilingual-v3.0"

echo ""

# ── Step 5: Report ─────────────────────────────────────────────────────
echo "═══ Action Items ═══"
echo "1. Check each provider's docs for new models"
echo "2. Add new models to models/api-models.toml"
echo "3. Run: cargo test -p the-one-memory models_registry"
echo "4. Mark deprecated models (do NOT remove)"

if [ "$APPLY" = true ]; then
    echo ""
    echo "═══ Applying Changes ═══"
    sed -i "s/^updated = .*/updated = \"$(date +%Y-%m-%d)\"/" "$REGISTRY_FILE"
    echo "Updated registry timestamp."

    if [ "$OPEN_PR" = true ]; then
        branch="chore/update-api-models-$(date +%Y%m%d)"
        git checkout -b "$branch"
        git add "$REGISTRY_FILE"
        git commit -m "chore: update API model registry"
        git push -u origin "$branch"
        gh pr create \
            --title "chore: update API model registry" \
            --body "Auto-generated by scripts/update-api-models.sh

Review new models and add to registry if appropriate."
        echo "PR created."
    fi
fi
```

- [ ] **Step 3: Make scripts executable**

```bash
chmod +x scripts/update-local-models.sh scripts/update-api-models.sh
```

- [ ] **Step 4: Test dry run**

Run: `bash scripts/update-local-models.sh`

Expected: Shows current fastembed version, latest version, model list, and action items. No files changed.

- [ ] **Step 5: Commit**

```bash
git add scripts/update-local-models.sh scripts/update-api-models.sh
git commit -m "feat: add model registry maintenance scripts"
```

---

## Task 8: Full Integration Validation

**Files:** None (validation only)

- [ ] **Step 1: Run fmt check**

Run: `cargo fmt --check`

Expected: no formatting issues.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: no warnings.

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`

Expected: all tests pass (145+ existing + ~15 new).

- [ ] **Step 4: Verify installer non-interactive**

Run: `bash scripts/install.sh --yes --local ./target/release --skip-register --skip-validate 2>&1 | grep -i "model\|embedding"`

Expected: "Using default embedding model: BGE-large-en-v1.5 (quality)"

- [ ] **Step 5: Verify maintenance script**

Run: `bash scripts/update-local-models.sh`

Expected: clean output with model list and action items.

- [ ] **Step 6: Final commit and tag**

```bash
git tag v0.4.0
git push origin main --tags
```
