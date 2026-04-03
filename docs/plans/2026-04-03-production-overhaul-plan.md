# Production Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform the-one-mcp from a library with stubs into a production-grade MCP server with real transport, embeddings, document management, LLM routing, and configurable limits.

**Architecture:** The overhaul preserves the existing 8-crate workspace structure but makes the broker async, replaces all stubs with real implementations, adds three MCP transports (stdio/SSE/streamable HTTP), integrates fastembed-rs for local embeddings, builds a managed document system with soft-delete, and adds an intelligent nano LLM provider pool with health checks.

**Tech Stack:** Rust 2021, tokio (async), axum (HTTP), fastembed (ONNX embeddings), reqwest (async HTTP), clap (CLI), serde/serde_json, rusqlite (bundled), qdrant HTTP API, tracing

**Spec:** `docs/specs/2026-04-03-production-overhaul-design.md`

---

## File Structure Map

### New Files
```
crates/the-one-core/src/limits.rs              — Configurable limits with validation bounds
crates/the-one-core/src/docs_manager.rs        — Managed docs CRUD + soft-delete + auto-sync
crates/the-one-memory/src/embeddings.rs         — EmbeddingProvider trait + fastembed + API impls
crates/the-one-memory/src/chunker.rs            — Smart markdown chunking (heading-aware, paragraph-safe)
crates/the-one-memory/src/qdrant.rs             — Async Qdrant HTTP backend
crates/the-one-router/src/provider_pool.rs      — Multi-provider pool with health, routing policies
crates/the-one-router/src/health.rs             — Per-provider health tracking + cooldown
crates/the-one-mcp/src/transport/mod.rs         — Transport trait
crates/the-one-mcp/src/transport/stdio.rs       — Stdio JSON-RPC transport
crates/the-one-mcp/src/transport/sse.rs         — SSE HTTP transport
crates/the-one-mcp/src/transport/stream.rs      — Streamable HTTP transport
crates/the-one-mcp/src/transport/jsonrpc.rs     — JSON-RPC 2.0 types + dispatch
crates/the-one-mcp/src/transport/tools.rs       — MCP tool definitions (tools/list schemas)
crates/the-one-mcp/src/bin/the-one-mcp.rs       — Main binary with clap CLI
```

### Modified Files
```
Cargo.toml                                      — Add workspace deps: tokio, clap, fastembed, async-trait
crates/the-one-core/Cargo.toml                  — Add sha2 (already there), temp-env dev-dep
crates/the-one-core/src/lib.rs                  — Add limits, docs_manager modules
crates/the-one-core/src/error.rs                — Add new error variants
crates/the-one-core/src/config.rs               — Add embedding, nano_providers, limits, external_docs_root fields
crates/the-one-core/src/policy.rs               — Use configurable limits from config
crates/the-one-memory/Cargo.toml                — Switch to async reqwest, add fastembed, sha2, tokio
crates/the-one-memory/src/lib.rs                — Refactor to async, use new embeddings/chunker/qdrant modules
crates/the-one-router/Cargo.toml                — Add reqwest, tokio for async provider calls
crates/the-one-router/src/lib.rs                — Make routing async
crates/the-one-router/src/providers.rs           — Replace stubs with OpenAI-compatible HTTP provider
crates/the-one-mcp/Cargo.toml                   — Add tokio, clap, axum, tokio-util, async-trait
crates/the-one-mcp/src/lib.rs                   — Add transport module
crates/the-one-mcp/src/api.rs                   — Add new request/response types for docs CRUD, config.update, docs.reindex
crates/the-one-mcp/src/broker.rs                — Make async, integrate docs_manager, new provider pool
crates/the-one-mcp/src/adapter_core.rs          — Make async
crates/the-one-ui/Cargo.toml                    — Ensure async compat
crates/the-one-ui/src/lib.rs                    — Update for async broker, add limits config UI
crates/the-one-claude/src/lib.rs                — Make async
crates/the-one-codex/src/lib.rs                 — Make async
```

---

## Phase 1: Foundation — Configurable Limits + Fix Broken Test + Error Types

These are dependency-free changes that everything else builds on.

### Task 1: Fix the failing config test

**Files:**
- Modify: `crates/the-one-core/Cargo.toml`
- Modify: `crates/the-one-core/src/config.rs:420-443`

- [ ] **Step 1: Add temp-env dev dependency**

In `crates/the-one-core/Cargo.toml`, add under `[dev-dependencies]`:
```toml
temp-env = "0.3"
```

- [ ] **Step 2: Fix the test to isolate env vars**

Replace the test `test_update_project_config_persists_provider_and_nano_settings` in `crates/the-one-core/src/config.rs` with:

```rust
#[test]
fn test_update_project_config_persists_provider_and_nano_settings() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    fs::create_dir_all(&project_root).expect("project root should exist");

    let global = temp.path().join("global");
    fs::create_dir_all(&global).expect("global dir");

    temp_env::with_vars(
        [
            ("THE_ONE_HOME", Some(global.display().to_string().as_str())),
            ("THE_ONE_PROVIDER", None::<&str>),
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
```

Also fix the first test similarly by wrapping its body with `temp_env::with_vars`.

- [ ] **Step 3: Run tests to verify fix**

Run: `cargo test -p the-one-core`
Expected: All 19 tests pass, 0 failures.

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-core/
git commit -m "fix: isolate config tests with temp-env to prevent env var pollution"
```

### Task 2: Add configurable limits to core

**Files:**
- Create: `crates/the-one-core/src/limits.rs`
- Modify: `crates/the-one-core/src/lib.rs`
- Modify: `crates/the-one-core/src/policy.rs`

- [ ] **Step 1: Write failing test for limits with validation**

Create `crates/the-one-core/src/limits.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurableLimits {
    pub max_tool_suggestions: usize,
    pub max_search_hits: usize,
    pub max_raw_section_bytes: usize,
    pub max_enabled_families: usize,
    pub max_doc_size_bytes: usize,
    pub max_managed_docs: usize,
    pub max_embedding_batch_size: usize,
    pub max_chunk_tokens: usize,
    pub max_nano_timeout_ms: u64,
    pub max_nano_retries: u8,
    pub max_nano_providers: usize,
    pub search_score_threshold: f32,
}

impl Default for ConfigurableLimits {
    fn default() -> Self {
        Self {
            max_tool_suggestions: 5,
            max_search_hits: 5,
            max_raw_section_bytes: 24 * 1024,
            max_enabled_families: 12,
            max_doc_size_bytes: 100 * 1024,
            max_managed_docs: 500,
            max_embedding_batch_size: 64,
            max_chunk_tokens: 512,
            max_nano_timeout_ms: 2_000,
            max_nano_retries: 3,
            max_nano_providers: 5,
            search_score_threshold: 0.3,
        }
    }
}

struct Bound<T> {
    floor: T,
    ceiling: T,
}

impl ConfigurableLimits {
    /// Clamp all values to valid bounds, logging warnings for out-of-range values.
    pub fn validated(mut self) -> Self {
        self.max_tool_suggestions = clamp_usize(self.max_tool_suggestions, 1, 50, "max_tool_suggestions");
        self.max_search_hits = clamp_usize(self.max_search_hits, 1, 100, "max_search_hits");
        self.max_raw_section_bytes = clamp_usize(self.max_raw_section_bytes, 1024, 1_048_576, "max_raw_section_bytes");
        self.max_enabled_families = clamp_usize(self.max_enabled_families, 1, 100, "max_enabled_families");
        self.max_doc_size_bytes = clamp_usize(self.max_doc_size_bytes, 1024, 10_485_760, "max_doc_size_bytes");
        self.max_managed_docs = clamp_usize(self.max_managed_docs, 10, 10_000, "max_managed_docs");
        self.max_embedding_batch_size = clamp_usize(self.max_embedding_batch_size, 1, 256, "max_embedding_batch_size");
        self.max_chunk_tokens = clamp_usize(self.max_chunk_tokens, 64, 2_048, "max_chunk_tokens");
        self.max_nano_timeout_ms = clamp_u64(self.max_nano_timeout_ms, 100, 10_000, "max_nano_timeout_ms");
        self.max_nano_retries = clamp_u8(self.max_nano_retries, 0, 10, "max_nano_retries");
        self.max_nano_providers = clamp_usize(self.max_nano_providers, 1, 10, "max_nano_providers");
        self.search_score_threshold = clamp_f32(self.search_score_threshold, 0.0, 1.0, "search_score_threshold");
        self
    }
}

fn clamp_usize(value: usize, floor: usize, ceiling: usize, name: &str) -> usize {
    if value < floor {
        tracing::warn!("{name} clamped from {value} to floor {floor}");
        return floor;
    }
    if value > ceiling {
        tracing::warn!("{name} clamped from {value} to ceiling {ceiling}");
        return ceiling;
    }
    value
}

fn clamp_u64(value: u64, floor: u64, ceiling: u64, name: &str) -> u64 {
    if value < floor {
        tracing::warn!("{name} clamped from {value} to floor {floor}");
        return floor;
    }
    if value > ceiling {
        tracing::warn!("{name} clamped from {value} to ceiling {ceiling}");
        return ceiling;
    }
    value
}

fn clamp_u8(value: u8, floor: u8, ceiling: u8, name: &str) -> u8 {
    if value < floor {
        tracing::warn!("{name} clamped from {value} to floor {floor}");
        return floor;
    }
    if value > ceiling {
        tracing::warn!("{name} clamped from {value} to ceiling {ceiling}");
        return ceiling;
    }
    value
}

fn clamp_f32(value: f32, floor: f32, ceiling: f32, name: &str) -> f32 {
    if value < floor {
        tracing::warn!("{name} clamped from {value} to floor {floor}");
        return floor;
    }
    if value > ceiling {
        tracing::warn!("{name} clamped from {value} to ceiling {ceiling}");
        return ceiling;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_are_within_bounds() {
        let limits = ConfigurableLimits::default().validated();
        assert_eq!(limits.max_tool_suggestions, 5);
        assert_eq!(limits.max_search_hits, 5);
        assert_eq!(limits.max_raw_section_bytes, 24 * 1024);
        assert_eq!(limits.max_enabled_families, 12);
        assert_eq!(limits.max_doc_size_bytes, 100 * 1024);
        assert_eq!(limits.max_managed_docs, 500);
        assert_eq!(limits.max_embedding_batch_size, 64);
        assert_eq!(limits.max_chunk_tokens, 512);
        assert_eq!(limits.max_nano_timeout_ms, 2_000);
        assert_eq!(limits.max_nano_retries, 3);
        assert_eq!(limits.max_nano_providers, 5);
        assert!((limits.search_score_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_out_of_bounds_values_are_clamped() {
        let limits = ConfigurableLimits {
            max_tool_suggestions: 999,
            max_search_hits: 0,
            max_raw_section_bytes: 1,
            max_enabled_families: 200,
            max_doc_size_bytes: 999_999_999,
            max_managed_docs: 1,
            max_embedding_batch_size: 1000,
            max_chunk_tokens: 10,
            max_nano_timeout_ms: 99_999,
            max_nano_retries: 100,
            max_nano_providers: 0,
            search_score_threshold: 5.0,
        }
        .validated();

        assert_eq!(limits.max_tool_suggestions, 50);
        assert_eq!(limits.max_search_hits, 1);
        assert_eq!(limits.max_raw_section_bytes, 1024);
        assert_eq!(limits.max_enabled_families, 100);
        assert_eq!(limits.max_doc_size_bytes, 10_485_760);
        assert_eq!(limits.max_managed_docs, 10);
        assert_eq!(limits.max_embedding_batch_size, 256);
        assert_eq!(limits.max_chunk_tokens, 64);
        assert_eq!(limits.max_nano_timeout_ms, 10_000);
        assert_eq!(limits.max_nano_retries, 10);
        assert_eq!(limits.max_nano_providers, 1);
        assert!((limits.search_score_threshold - 1.0).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 2: Add limits module to lib.rs**

Add to `crates/the-one-core/src/lib.rs`:
```rust
pub mod limits;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p the-one-core limits`
Expected: 2 tests pass.

- [ ] **Step 4: Update PolicyEngine to use ConfigurableLimits**

Replace `crates/the-one-core/src/policy.rs` — swap out `PolicyLimits` for `ConfigurableLimits`:

```rust
use crate::contracts::RiskLevel;
use crate::error::CoreError;
use crate::limits::ConfigurableLimits;

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    limits: ConfigurableLimits,
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self {
            limits: ConfigurableLimits::default().validated(),
        }
    }
}

impl PolicyEngine {
    pub fn new(limits: ConfigurableLimits) -> Self {
        Self {
            limits: limits.validated(),
        }
    }

    pub fn limits(&self) -> &ConfigurableLimits {
        &self.limits
    }

    pub fn clamp_suggestions(&self, requested: usize) -> usize {
        requested.min(self.limits.max_tool_suggestions)
    }

    pub fn clamp_search_hits(&self, requested: usize) -> usize {
        requested.min(self.limits.max_search_hits)
    }

    pub fn clamp_doc_bytes(&self, requested: usize) -> usize {
        requested.min(self.limits.max_raw_section_bytes)
    }

    pub fn validate_enabled_families_count(&self, count: usize) -> Result<(), CoreError> {
        if count <= self.limits.max_enabled_families {
            return Ok(());
        }
        Err(CoreError::PolicyDenied(format!(
            "enabled families exceed policy limit: {} > {}",
            count, self.limits.max_enabled_families
        )))
    }

    pub fn requires_approval(&self, risk_level: RiskLevel) -> bool {
        matches!(risk_level, RiskLevel::High)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_clamps_values_to_limits() {
        let engine = PolicyEngine::new(ConfigurableLimits {
            max_tool_suggestions: 3,
            max_search_hits: 2,
            max_raw_section_bytes: 1024,
            max_enabled_families: 1,
            ..ConfigurableLimits::default()
        });

        assert_eq!(engine.clamp_suggestions(10), 3);
        assert_eq!(engine.clamp_search_hits(10), 2);
        assert_eq!(engine.clamp_doc_bytes(9999), 1024);
        assert!(engine.validate_enabled_families_count(2).is_err());
    }
}
```

- [ ] **Step 5: Fix compilation across workspace**

The old `PolicyLimits` type is referenced in `broker.rs`. Update `crates/the-one-mcp/src/broker.rs` — replace all `PolicyLimits` with `ConfigurableLimits` and update imports. The `McpBroker::new_with_policy` already takes `PolicyEngine` so only test code that constructs `PolicyLimits` directly needs updating.

- [ ] **Step 6: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/the-one-core/src/limits.rs crates/the-one-core/src/lib.rs crates/the-one-core/src/policy.rs crates/the-one-mcp/src/broker.rs
git commit -m "feat: add ConfigurableLimits with validation bounds, replace PolicyLimits"
```

### Task 3: Extend config system with limits, embedding, and nano provider pool fields

**Files:**
- Modify: `crates/the-one-core/src/config.rs`
- Modify: `crates/the-one-core/src/error.rs`

- [ ] **Step 1: Add new error variants**

Add to `CoreError` in `crates/the-one-core/src/error.rs`:

```rust
#[error("embedding error: {0}")]
Embedding(String),
#[error("transport error: {0}")]
Transport(String),
#[error("provider error: {0}")]
Provider(String),
#[error("document error: {0}")]
Document(String),
```

- [ ] **Step 2: Add NanoProviderConfig and embedding fields to FileConfig and AppConfig**

In `crates/the-one-core/src/config.rs`, add a new struct and extend config:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NanoProviderEntry {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NanoRoutingPolicy {
    #[serde(rename = "priority")]
    Priority,
    #[serde(rename = "round_robin")]
    RoundRobin,
    #[serde(rename = "latency")]
    Latency,
}

impl Default for NanoRoutingPolicy {
    fn default() -> Self { Self::Priority }
}
```

Add to `AppConfig`:
```rust
pub embedding_provider: String,          // "local" or "api"
pub embedding_model: String,             // "all-MiniLM-L6-v2"
pub embedding_api_base_url: Option<String>,
pub embedding_api_key: Option<String>,
pub embedding_dimensions: usize,
pub nano_providers: Vec<NanoProviderEntry>,
pub nano_routing_policy: NanoRoutingPolicy,
pub external_docs_root: Option<PathBuf>,
pub limits: ConfigurableLimits,
```

Add matching fields to `FileConfig`, `RuntimeOverrides`, `ProjectConfigUpdate`. Add defaults:
```rust
const DEFAULT_EMBEDDING_PROVIDER: &str = "local";
const DEFAULT_EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";
const DEFAULT_EMBEDDING_DIMENSIONS: usize = 384;
```

Add env var reading in `apply_env_layer` for `THE_ONE_EMBEDDING_PROVIDER`, `THE_ONE_EMBEDDING_MODEL`, `THE_ONE_EMBEDDING_API_BASE_URL`, `THE_ONE_EMBEDDING_API_KEY`, `THE_ONE_EMBEDDING_DIMENSIONS`, `THE_ONE_EXTERNAL_DOCS_ROOT`, and all `THE_ONE_LIMIT_*` vars.

- [ ] **Step 3: Write test for new config fields**

```rust
#[test]
fn test_config_loads_embedding_and_limits_from_project_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_root = temp.path().join("repo");
    let project_state = project_root.join(".the-one");
    let global = temp.path().join("global");
    fs::create_dir_all(&project_state).unwrap();
    fs::create_dir_all(&global).unwrap();

    fs::write(
        project_state.join("config.json"),
        r#"{
            "embedding_provider": "api",
            "embedding_model": "text-embedding-3-small",
            "embedding_dimensions": 1536,
            "limits": {
                "max_search_hits": 10,
                "max_chunk_tokens": 1024
            }
        }"#,
    ).unwrap();

    temp_env::with_vars(
        [
            ("THE_ONE_HOME", Some(global.display().to_string().as_str())),
            ("THE_ONE_PROVIDER", None::<&str>),
        ],
        || {
            let config = AppConfig::load(&project_root, RuntimeOverrides::default()).unwrap();
            assert_eq!(config.embedding_provider, "api");
            assert_eq!(config.embedding_model, "text-embedding-3-small");
            assert_eq!(config.embedding_dimensions, 1536);
            assert_eq!(config.limits.max_search_hits, 10);
            assert_eq!(config.limits.max_chunk_tokens, 1024);
            // Defaults preserved for unset fields
            assert_eq!(config.limits.max_tool_suggestions, 5);
        },
    );
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-core`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-core/
git commit -m "feat: extend config with embedding, nano provider pool, limits, and external docs fields"
```

---

## Phase 2: Production RAG — Embeddings + Chunker + Async Qdrant

### Task 4: Smart markdown chunker

**Files:**
- Create: `crates/the-one-memory/src/chunker.rs`

- [ ] **Step 1: Write failing tests for chunker**

Create `crates/the-one-memory/src/chunker.rs` with tests first:

```rust
/// Chunk metadata carried through the RAG pipeline.
#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub id: String,
    pub source_path: String,
    pub heading_hierarchy: Vec<String>,
    pub chunk_index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub content_hash: String,
    pub content: String,
}

/// Split markdown into semantic chunks respecting heading hierarchy,
/// paragraph boundaries, and code blocks. Never splits mid-paragraph
/// or mid-code-block.
pub fn chunk_markdown(source_path: &str, content: &str, max_chunk_tokens: usize) -> Vec<ChunkMeta> {
    todo!()
}

/// Estimate token count from text (rough: 1 token ~= 4 chars).
fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_by_headings() {
        let md = "# Intro\nHello world\n\n## Details\nSome details here\n\n## Outro\nBye\n";
        let chunks = chunk_markdown("test.md", md, 512);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].heading_hierarchy, vec!["Intro"]);
        assert_eq!(chunks[1].heading_hierarchy, vec!["Details"]);
        assert_eq!(chunks[2].heading_hierarchy, vec!["Outro"]);
        assert!(chunks[0].content.contains("Hello world"));
    }

    #[test]
    fn test_large_section_splits_on_paragraphs() {
        let big_paragraph = "word ".repeat(600); // ~3000 chars = ~750 tokens
        let md = format!("# Big\n{big_paragraph}\n\nSecond paragraph here.\n");
        let chunks = chunk_markdown("test.md", &md, 512);
        assert!(chunks.len() >= 2, "should split large section");
        // Each chunk should be <= max_chunk_tokens
        for c in &chunks {
            assert!(estimate_tokens(&c.content) <= 600, "chunk too big: {}", c.content.len());
        }
    }

    #[test]
    fn test_code_blocks_not_split() {
        let md = "# Code\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n";
        let chunks = chunk_markdown("test.md", md, 512);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("fn main()"));
    }

    #[test]
    fn test_no_headings_single_chunk() {
        let md = "Just some text without headings.\n\nAnother paragraph.\n";
        let chunks = chunk_markdown("test.md", md, 512);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_content_hash_differs_for_different_content() {
        let chunks1 = chunk_markdown("a.md", "# A\nfoo", 512);
        let chunks2 = chunk_markdown("a.md", "# A\nbar", 512);
        assert_ne!(chunks1[0].content_hash, chunks2[0].content_hash);
    }

    #[test]
    fn test_nested_headings_preserve_hierarchy() {
        let md = "# Top\n## Sub\n### Deep\nContent\n";
        let chunks = chunk_markdown("test.md", md, 512);
        let deep = chunks.iter().find(|c| c.heading_hierarchy.contains(&"Deep".to_string()));
        assert!(deep.is_some());
        assert_eq!(deep.unwrap().heading_hierarchy, vec!["Top", "Sub", "Deep"]);
    }
}
```

- [ ] **Step 2: Implement chunk_markdown**

Fill in the implementation: parse headings with regex `^(#{1,6})\s+(.+)$`, track heading hierarchy stack, split sections that exceed max_chunk_tokens on double-newline paragraph boundaries, compute SHA-256 content_hash per chunk, track byte_offset and byte_length.

- [ ] **Step 3: Run tests**

Run: `cargo test -p the-one-memory chunker`
Expected: All 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-memory/src/chunker.rs
git commit -m "feat: smart markdown chunker with heading hierarchy and paragraph-safe splitting"
```

### Task 5: Embedding provider trait + fastembed + API implementations

**Files:**
- Create: `crates/the-one-memory/src/embeddings.rs`
- Modify: `crates/the-one-memory/Cargo.toml`
- Modify: `Cargo.toml` (workspace deps)

- [ ] **Step 1: Add dependencies**

In workspace `Cargo.toml`, add:
```toml
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", "net", "signal", "sync", "io-util", "io-std"] }
fastembed = "4"
async-trait = "0.1"
```

In `crates/the-one-memory/Cargo.toml`:
```toml
[dependencies]
fastembed = { workspace = true }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
sha2 = "0.11"
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true
```

Note: Remove `blocking` feature from reqwest.

- [ ] **Step 2: Write EmbeddingProvider trait and implementations**

Create `crates/the-one-memory/src/embeddings.rs`:

```rust
use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, String> {
        let results = self.embed_batch(&[text.to_string()]).await?;
        results.into_iter().next().ok_or_else(|| "empty result".to_string())
    }
}

/// Local embedding via fastembed-rs (ONNX, offline, free).
pub struct FastEmbedProvider {
    model: fastembed::TextEmbedding,
    dims: usize,
}

impl FastEmbedProvider {
    pub fn new(model_name: &str) -> Result<Self, String> {
        // fastembed model initialization runs in blocking context
        let model = fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(Self::parse_model(model_name))
                .with_show_download_progress(false),
        ).map_err(|e| format!("fastembed init: {e}"))?;

        let dims = match model_name {
            "all-MiniLM-L6-v2" => 384,
            "BGE-small-en-v1.5" => 384,
            _ => 384,
        };

        Ok(Self { model, dims })
    }

    fn parse_model(name: &str) -> fastembed::EmbeddingModel {
        match name {
            "BGE-small-en-v1.5" => fastembed::EmbeddingModel::BGESmallENV15,
            _ => fastembed::EmbeddingModel::AllMiniLML6V2,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FastEmbedProvider {
    fn name(&self) -> &str { "fastembed-local" }
    fn dimensions(&self) -> usize { self.dims }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let texts = texts.to_vec();
        let model = self.model.clone();
        tokio::task::spawn_blocking(move || {
            model.embed(texts, None)
                .map_err(|e| format!("fastembed embed: {e}"))
        })
        .await
        .map_err(|e| format!("join error: {e}"))?
    }
}

/// API-based embedding via OpenAI-compatible endpoint.
pub struct ApiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dims: usize,
}

impl ApiEmbeddingProvider {
    pub fn new(base_url: &str, api_key: Option<&str>, model: &str, dims: usize) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = api_key {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {key}").parse().expect("valid header"),
            );
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("client build");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dims,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    fn name(&self) -> &str { "api-embedding" }
    fn dimensions(&self) -> usize { self.dims }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self.client
            .post(format!("{}/embeddings", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("embedding API request: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("embedding API {status}: {text}"));
        }

        let json: serde_json::Value = resp.json().await
            .map_err(|e| format!("embedding API parse: {e}"))?;

        let data = json["data"].as_array()
            .ok_or("missing 'data' array in response")?;

        let mut results = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item["embedding"].as_array()
                .ok_or("missing 'embedding' in response item")?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            results.push(embedding);
        }

        Ok(results)
    }
}
```

- [ ] **Step 3: Write tests**

Add tests at bottom of embeddings.rs:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fastembed_produces_384_dim_vectors() {
        let provider = FastEmbedProvider::new("all-MiniLM-L6-v2").expect("init");
        assert_eq!(provider.dimensions(), 384);
        let result = provider.embed_single("hello world").await.expect("embed");
        assert_eq!(result.len(), 384);
        // Verify it's not all zeros
        assert!(result.iter().any(|&v| v != 0.0));
    }

    #[tokio::test]
    async fn test_fastembed_batch_embedding() {
        let provider = FastEmbedProvider::new("all-MiniLM-L6-v2").expect("init");
        let texts = vec!["hello".to_string(), "world".to_string()];
        let results = provider.embed_batch(&texts).await.expect("embed");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 384);
        assert_eq!(results[1].len(), 384);
        // Different texts should produce different vectors
        assert_ne!(results[0], results[1]);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-memory embeddings`
Expected: 2 tests pass (first run downloads the model, ~30MB).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/the-one-memory/
git commit -m "feat: add production embedding providers - fastembed local + OpenAI-compatible API"
```

### Task 6: Async Qdrant HTTP backend

**Files:**
- Create: `crates/the-one-memory/src/qdrant.rs`

- [ ] **Step 1: Write async Qdrant backend**

Create `crates/the-one-memory/src/qdrant.rs` with:
- `AsyncQdrantBackend` struct with `reqwest::Client`, `base_url`, `collection_name`
- `QdrantOptions` struct: `api_key`, `ca_cert_path`, `tls_insecure`
- Methods: `new()`, `ensure_collection(dims)`, `upsert_points(ids, vectors, payloads)`, `search(query_vector, top_k, score_threshold)`, `delete_by_source_path(path)`
- All methods async, return `Result<T, String>`
- Collection auto-creation with cosine distance and HNSW parameters

- [ ] **Step 2: Write tests with httpmock**

Test: ensure_collection sends correct PUT, search sends correct POST, API key header is included.

- [ ] **Step 3: Run tests**

Run: `cargo test -p the-one-memory qdrant`
Expected: Tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-memory/src/qdrant.rs
git commit -m "feat: async Qdrant HTTP backend with collection management and scored search"
```

### Task 7: Refactor MemoryEngine to async with real embeddings

**Files:**
- Modify: `crates/the-one-memory/src/lib.rs`

- [ ] **Step 1: Rewrite MemoryEngine as async**

Restructure `lib.rs`:
- Keep it as the public API module
- Import from `chunker`, `embeddings`, `qdrant`
- Make `MemoryEngine` async with `async fn` methods
- `MemoryEngine::new_local(model_name)` → fastembed + local Qdrant file
- `MemoryEngine::new_qdrant_http(base_url, project_id, options, embedding_provider)` → remote Qdrant
- `MemoryEngine::new_api_embedding(embedding_base_url, api_key, model, dims, qdrant_base_url, project_id, qdrant_options)` → API embeddings + remote Qdrant
- `ingest_markdown_tree()` → async, uses new chunker, batch embedding
- `search()` → async, embeds query, searches Qdrant
- All old hash-based embedding code removed

- [ ] **Step 2: Update module declarations**

```rust
pub mod chunker;
pub mod embeddings;
pub mod qdrant;
```

- [ ] **Step 3: Update existing tests to async**

Convert all tests to `#[tokio::test]`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-memory`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-memory/
git commit -m "feat: async MemoryEngine with fastembed, smart chunking, and real Qdrant search"
```

---

## Phase 3: Nano LLM Provider Pool

### Task 8: Provider health tracking

**Files:**
- Create: `crates/the-one-router/src/health.rs`

- [ ] **Step 1: Implement ProviderHealth with cooldown logic**

```rust
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub status: ProviderStatus,
    pub last_check_epoch_ms: u64,
    pub consecutive_failures: u32,
    pub cooldown_until_epoch_ms: u64,
    pub latency_samples: Vec<u64>,   // rolling window, last 20
    pub total_calls: u64,
    pub total_errors: u64,
}

impl ProviderHealth {
    pub fn new() -> Self { /* Unknown status, zero counters */ }
    pub fn p50_latency_ms(&self) -> u64 { /* median of latency_samples */ }
    pub fn record_success(&mut self, latency_ms: u64) { /* reset to healthy, push sample */ }
    pub fn record_failure(&mut self) { /* increment failures, compute cooldown */ }
    pub fn is_available(&self) -> bool { /* healthy or unknown, and not in cooldown */ }
    fn cooldown_duration_ms(consecutive_failures: u32) -> u64 {
        match consecutive_failures {
            0 => 0,
            1 => 5_000,
            2 => 15_000,
            _ => 60_000,
        }
    }
}
```

- [ ] **Step 2: Write tests**

Test cooldown progression, p50 calculation, success resets, availability checks.

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-router/src/health.rs
git commit -m "feat: provider health tracking with cooldown strategy and latency rolling window"
```

### Task 9: OpenAI-compatible provider + provider pool

**Files:**
- Create: `crates/the-one-router/src/provider_pool.rs`
- Modify: `crates/the-one-router/src/providers.rs`
- Modify: `crates/the-one-router/Cargo.toml`

- [ ] **Step 1: Add async deps to router**

In `crates/the-one-router/Cargo.toml`:
```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
tokio = { workspace = true }
async-trait = { workspace = true }
serde_json.workspace = true
```

- [ ] **Step 2: Replace stub providers with real OpenAI-compatible HTTP provider**

In `crates/the-one-router/src/providers.rs`, replace all stubs with:

```rust
pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: std::time::Duration,
    client: reqwest::Client,
}
```

Implement `classify()` as async: POST to `{base_url}/chat/completions` with the classification prompt, parse single-word response, map to `RequestIntent`.

- [ ] **Step 3: Build ProviderPool with routing policies**

Create `crates/the-one-router/src/provider_pool.rs`:

```rust
pub struct ProviderPool {
    providers: Vec<(OpenAiCompatibleProvider, Mutex<ProviderHealth>)>,
    policy: NanoRoutingPolicy,
    round_robin_index: AtomicUsize,
}

impl ProviderPool {
    pub fn new(entries: Vec<NanoProviderEntry>, policy: NanoRoutingPolicy) -> Self;
    pub async fn classify(&self, request: &str) -> PoolClassifyResult;
    fn select_priority(&self) -> Vec<usize>;    // indices in config order, skip unavailable
    fn select_round_robin(&self) -> Vec<usize>; // rotate, skip unavailable
    fn select_latency(&self) -> Vec<usize>;     // sort by p50, skip unavailable
}

pub struct PoolClassifyResult {
    pub intent: Option<RequestIntent>,
    pub provider_used: Option<String>,
    pub latency_ms: u64,
    pub fallback_to_rules: bool,
    pub attempts: u8,
    pub last_error: Option<String>,
}
```

TCP connect check before classify: `tokio::net::TcpStream::connect` with 50ms timeout.

- [ ] **Step 4: Write tests**

Test priority routing, round-robin rotation, fallback to rules when all providers down, cooldown behavior.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-router/
git commit -m "feat: OpenAI-compatible nano provider with pool routing (priority/round_robin/latency)"
```

### Task 10: Make Router async and integrate provider pool

**Files:**
- Modify: `crates/the-one-router/src/lib.rs`

- [ ] **Step 1: Convert Router to async**

Make `route_with_provider_budget` async. Replace the old `NanoProvider` trait usage with `ProviderPool`. The `Router` struct gets an optional `ProviderPool`.

- [ ] **Step 2: Update tests to async**

Convert all router tests to `#[tokio::test]`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p the-one-router`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-router/
git commit -m "feat: async Router with ProviderPool integration"
```

---

## Phase 4: Managed Documents System

### Task 11: DocsManager with CRUD + soft-delete + auto-sync

**Files:**
- Create: `crates/the-one-core/src/docs_manager.rs`
- Modify: `crates/the-one-core/src/lib.rs`

- [ ] **Step 1: Write DocsManager**

```rust
pub struct DocsManager {
    managed_root: PathBuf,   // <project>/.the-one/docs/
    trash_root: PathBuf,     // <project>/.the-one/docs/.trash/
}

pub struct DocEntry {
    pub path: String,          // relative to managed_root
    pub size_bytes: u64,
    pub modified_epoch_ms: u64,
}

pub struct SyncReport {
    pub new: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

impl DocsManager {
    pub fn new(project_root: &Path) -> Result<Self, CoreError>;
    pub fn create(&self, relative_path: &str, content: &str) -> Result<PathBuf, CoreError>;
    pub fn update(&self, relative_path: &str, content: &str) -> Result<PathBuf, CoreError>;
    pub fn delete(&self, relative_path: &str) -> Result<(), CoreError>;  // soft-delete to .trash/
    pub fn get(&self, relative_path: &str) -> Result<String, CoreError>;
    pub fn get_section(&self, relative_path: &str, heading: &str, max_bytes: usize) -> Result<Option<String>, CoreError>;
    pub fn list(&self) -> Result<Vec<DocEntry>, CoreError>;
    pub fn move_doc(&self, from: &str, to: &str) -> Result<(), CoreError>;
    pub fn trash_list(&self) -> Result<Vec<DocEntry>, CoreError>;
    pub fn trash_restore(&self, relative_path: &str) -> Result<(), CoreError>;
    pub fn trash_empty(&self) -> Result<(), CoreError>;
    pub fn scan_changes(&self) -> Result<SyncReport, CoreError>;  // for auto-sync

    // Validation
    fn validate_path(relative_path: &str) -> Result<(), CoreError>;  // no .., must be .md
    fn validate_doc_size(&self, content: &str, max_bytes: usize) -> Result<(), CoreError>;
    fn validate_doc_count(&self, max_docs: usize) -> Result<(), CoreError>;
}
```

- [ ] **Step 2: Implement all methods**

Key behaviors:
- `validate_path`: reject `..`, non-`.md`, empty, and non-alphanumeric-hyphen-underscore-dot-slash chars
- `create`: `fs::create_dir_all` for subdirectories, write file, error if exists
- `update`: error if doesn't exist, write file
- `delete`: move to `.trash/` preserving directory structure
- `trash_restore`: move back from `.trash/`, error if target exists
- `trash_empty`: `fs::remove_dir_all` on `.trash/` contents
- `scan_changes`: walk directory, compare mtime against last known state

- [ ] **Step 3: Write comprehensive tests**

Test: create + get, update + get, delete + trash_list + restore, trash_empty, path traversal rejection, move, list ordering, doc size validation, doc count validation.

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-core docs_manager`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-core/src/docs_manager.rs crates/the-one-core/src/lib.rs
git commit -m "feat: DocsManager with CRUD, soft-delete to .trash, auto-sync scanning"
```

---

## Phase 5: Async Broker Overhaul

### Task 12: Make McpBroker async

**Files:**
- Modify: `crates/the-one-mcp/Cargo.toml`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/adapter_core.rs`
- Modify: `crates/the-one-mcp/src/api.rs`

- [ ] **Step 1: Add async dependencies to the-one-mcp**

In `crates/the-one-mcp/Cargo.toml`:
```toml
[dependencies]
tokio.workspace = true
async-trait.workspace = true
# ... existing deps
```

- [ ] **Step 2: Convert broker to async**

In `broker.rs`:
- `memory_by_project: tokio::sync::RwLock<HashMap<...>>`
- `session_approvals: tokio::sync::RwLock<HashSet<...>>`
- All public methods become `pub async fn`
- SQLite calls wrapped: `tokio::task::spawn_blocking(move || { ... }).await`
- Memory engine operations are already async from Task 7
- Router calls are already async from Task 10

- [ ] **Step 3: Add new API types for docs CRUD**

In `api.rs`, add:
```rust
pub struct DocsCreateRequest { pub project_root: String, pub project_id: String, pub path: String, pub content: String }
pub struct DocsCreateResponse { pub path: String }
pub struct DocsUpdateRequest { pub project_root: String, pub project_id: String, pub path: String, pub content: String }
pub struct DocsUpdateResponse { pub path: String }
pub struct DocsDeleteRequest { pub project_root: String, pub project_id: String, pub path: String }
pub struct DocsDeleteResponse { pub deleted: bool }
pub struct DocsMoveRequest { pub project_root: String, pub project_id: String, pub from: String, pub to: String }
pub struct DocsMoveResponse { pub from: String, pub to: String }
pub struct DocsTrashListRequest { pub project_root: String, pub project_id: String }
pub struct DocsTrashListResponse { pub entries: Vec<DocEntry> }
pub struct DocsTrashRestoreRequest { pub project_root: String, pub project_id: String, pub path: String }
pub struct DocsTrashRestoreResponse { pub restored: bool }
pub struct DocsTrashEmptyRequest { pub project_root: String, pub project_id: String }
pub struct DocsTrashEmptyResponse { pub emptied: bool }
pub struct DocsReindexRequest { pub project_root: String, pub project_id: String }
pub struct DocsReindexResponse { pub new: usize, pub updated: usize, pub removed: usize, pub unchanged: usize }
pub struct ConfigUpdateRequest { pub project_root: String, pub update: serde_json::Value }
pub struct ConfigUpdateResponse { pub path: String }
```

- [ ] **Step 4: Add broker methods for docs CRUD, config.update, docs.reindex**

Add to `McpBroker`:
```rust
pub async fn docs_create(&self, request: DocsCreateRequest) -> Result<DocsCreateResponse, CoreError>;
pub async fn docs_update(&self, request: DocsUpdateRequest) -> Result<DocsUpdateResponse, CoreError>;
pub async fn docs_delete(&self, request: DocsDeleteRequest) -> Result<DocsDeleteResponse, CoreError>;
pub async fn docs_move(&self, request: DocsMoveRequest) -> Result<DocsMoveResponse, CoreError>;
pub async fn docs_trash_list(&self, request: DocsTrashListRequest) -> Result<DocsTrashListResponse, CoreError>;
pub async fn docs_trash_restore(&self, request: DocsTrashRestoreRequest) -> Result<DocsTrashRestoreResponse, CoreError>;
pub async fn docs_trash_empty(&self, request: DocsTrashEmptyRequest) -> Result<DocsTrashEmptyResponse, CoreError>;
pub async fn docs_reindex(&self, request: DocsReindexRequest) -> Result<DocsReindexResponse, CoreError>;
pub async fn config_update(&self, request: ConfigUpdateRequest) -> Result<ConfigUpdateResponse, CoreError>;
```

- [ ] **Step 5: Update adapter_core.rs to async**

Make all `AdapterCore` methods `pub async fn`.

- [ ] **Step 6: Convert all tests to async**

All `#[test]` → `#[tokio::test]`.

- [ ] **Step 7: Run tests**

Run: `cargo test -p the-one-mcp`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-mcp/
git commit -m "feat: async McpBroker with docs CRUD, config.update, reindex, and provider pool"
```

### Task 13: Update adapters and UI for async

**Files:**
- Modify: `crates/the-one-claude/src/lib.rs`
- Modify: `crates/the-one-codex/src/lib.rs`
- Modify: `crates/the-one-ui/src/lib.rs`
- Modify: `crates/the-one-ui/Cargo.toml`

- [ ] **Step 1: Make Claude adapter async**

All methods become `pub async fn`, delegate to async `AdapterCore`.

- [ ] **Step 2: Make Codex adapter async**

Same pattern as Claude.

- [ ] **Step 3: Update UI for async broker**

In `the-one-ui`, all handler functions already run in async context (axum). Update broker calls to `.await`. Add limits section to config page HTML. Add `POST /api/config` handler for limits updates.

- [ ] **Step 4: Run workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-claude/ crates/the-one-codex/ crates/the-one-ui/
git commit -m "feat: async adapters and UI with limits configuration"
```

---

## Phase 6: MCP Transport Layer

### Task 14: JSON-RPC types and MCP tool definitions

**Files:**
- Create: `crates/the-one-mcp/src/transport/mod.rs`
- Create: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Create: `crates/the-one-mcp/src/transport/tools.rs`
- Modify: `crates/the-one-mcp/src/lib.rs`

- [ ] **Step 1: Create transport module**

`transport/mod.rs`:
```rust
pub mod jsonrpc;
pub mod tools;
pub mod stdio;
pub mod sse;
pub mod stream;

use async_trait::async_trait;
use std::sync::Arc;
use crate::broker::McpBroker;
use the_one_core::error::CoreError;

#[async_trait]
pub trait Transport: Send + Sync {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError>;
}
```

- [ ] **Step 2: Implement JSON-RPC 2.0 types**

`transport/jsonrpc.rs`:
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,  // "2.0"
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Dispatch a JSON-RPC request to the broker and return a response.
pub async fn dispatch(broker: &McpBroker, request: JsonRpcRequest) -> JsonRpcResponse;
```

The `dispatch` function matches on `request.method`:
- `"initialize"` → return server info + capabilities
- `"notifications/initialized"` → no-op, return empty
- `"tools/list"` → return tool definitions from `tools.rs`
- `"tools/call"` → extract tool name + arguments, dispatch to broker method

- [ ] **Step 3: Define all 24 MCP tool schemas**

`transport/tools.rs`:
```rust
pub fn tool_definitions() -> Vec<serde_json::Value>;
```

Returns JSON array of MCP tool definitions with `name`, `description`, and `inputSchema` for each of the 24 tools.

- [ ] **Step 4: Write tests**

Test dispatch for `initialize`, `tools/list`, and a sample `tools/call` (e.g., `config.export`).

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-mcp/src/transport/ crates/the-one-mcp/src/lib.rs
git commit -m "feat: JSON-RPC 2.0 dispatch layer with 24 MCP tool definitions"
```

### Task 15: Stdio transport

**Files:**
- Create: `crates/the-one-mcp/src/transport/stdio.rs`

- [ ] **Step 1: Implement StdioTransport**

```rust
pub struct StdioTransport;

#[async_trait]
impl Transport for StdioTransport {
    async fn run(&self, broker: Arc<McpBroker>) -> Result<(), CoreError> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let reader = tokio::io::BufReader::new(stdin);
        // Read newline-delimited JSON from stdin
        // For each line: parse as JsonRpcRequest, dispatch, write response + newline to stdout
        // EOF = clean shutdown
    }
}
```

- [ ] **Step 2: Write test**

Test with a mock stdin/stdout pair: send `initialize` request, verify response.

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-mcp/src/transport/stdio.rs
git commit -m "feat: stdio MCP transport for Claude Code / Codex integration"
```

### Task 16: SSE transport

**Files:**
- Create: `crates/the-one-mcp/src/transport/sse.rs`

- [ ] **Step 1: Implement SseTransport**

axum HTTP server with:
- `POST /message` → accept JSON-RPC request, dispatch, return response
- `GET /sse` → SSE stream for server-initiated messages (notifications)
- Session management via random session IDs in headers

- [ ] **Step 2: Write integration test**

Test: HTTP POST to `/message` with `initialize` request, verify JSON-RPC response.

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-mcp/src/transport/sse.rs
git commit -m "feat: SSE HTTP transport for web client MCP access"
```

### Task 17: Streamable HTTP transport

**Files:**
- Create: `crates/the-one-mcp/src/transport/stream.rs`

- [ ] **Step 1: Implement StreamableHttpTransport**

axum HTTP server with:
- `POST /mcp` → accept JSON-RPC, if `Accept: text/event-stream` return SSE stream, otherwise return JSON
- Supports bidirectional communication per MCP streamable HTTP spec

- [ ] **Step 2: Write integration test**

Test: POST to `/mcp` with JSON accept, verify JSON-RPC response.

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-mcp/src/transport/stream.rs
git commit -m "feat: streamable HTTP transport per MCP spec"
```

### Task 18: Main binary with clap CLI

**Files:**
- Create: `crates/the-one-mcp/src/bin/the-one-mcp.rs`
- Modify: `crates/the-one-mcp/Cargo.toml`

- [ ] **Step 1: Add clap dependency**

In workspace `Cargo.toml`:
```toml
clap = { version = "4", features = ["derive"] }
```

In `crates/the-one-mcp/Cargo.toml`:
```toml
clap.workspace = true
```

And add binary declaration:
```toml
[[bin]]
name = "the-one-mcp"
path = "src/bin/the-one-mcp.rs"
```

- [ ] **Step 2: Implement CLI**

```rust
use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(name = "the-one-mcp", about = "The One MCP broker server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Serve {
        #[arg(long, default_value = "stdio")]
        transport: TransportKind,
        #[arg(long, default_value = "3000")]
        port: u16,
        #[arg(long)]
        project_root: Option<String>,
        #[arg(long)]
        project_id: Option<String>,
    },
}

#[derive(Clone, ValueEnum)]
enum TransportKind { Stdio, Sse, Stream }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Init telemetry, load config, create broker, start transport
}
```

- [ ] **Step 3: Verify binary builds and runs**

Run: `cargo build -p the-one-mcp --bin the-one-mcp`
Run: `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | cargo run -p the-one-mcp --bin the-one-mcp -- serve`
Expected: JSON-RPC response with server capabilities.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/the-one-mcp/
git commit -m "feat: the-one-mcp binary with serve command supporting stdio/sse/stream transports"
```

---

## Phase 7: Integration + Polish

### Task 19: Update JSON schemas for new tools

**Files:**
- Create/modify: `schemas/mcp/v1beta/docs.create.*.schema.json` (and all other new tool schemas)

- [ ] **Step 1: Add schema files for all 10 new tools**

Create JSON Schema files for: `docs.create`, `docs.update`, `docs.delete`, `docs.move`, `docs.trash.list`, `docs.trash.restore`, `docs.trash.empty`, `docs.reindex`, `config.update`, each with request and response schemas.

- [ ] **Step 2: Update schema validation test**

Update `test_v1beta_schema_files_exist_and_are_valid_json` in `crates/the-one-mcp/src/lib.rs` to expect the new schema count.

- [ ] **Step 3: Run tests**

Run: `cargo test -p the-one-mcp test_v1beta`
Expected: Schema validation passes with new count.

- [ ] **Step 4: Commit**

```bash
git add schemas/ crates/the-one-mcp/src/lib.rs
git commit -m "feat: add v1beta JSON schemas for docs CRUD, trash, reindex, and config.update tools"
```

### Task 20: Update admin UI with limits + new endpoints

**Files:**
- Modify: `crates/the-one-ui/src/lib.rs`

- [ ] **Step 1: Add limits section to config page**

Add form fields for all 12 configurable limits with labels, current values, and floor/ceiling hints.

- [ ] **Step 2: Add provider pool status to dashboard**

Show nano provider health status (name, status, latency, calls, errors) in the dashboard.

- [ ] **Step 3: Run UI tests**

Run: `cargo test -p the-one-ui`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/the-one-ui/
git commit -m "feat: admin UI with limits configuration and provider pool health dashboard"
```

### Task 21: Update release gate and CI

**Files:**
- Modify: `scripts/release-gate.sh`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Update release gate script**

Add new critical test targets: transport dispatch, embedding provider, docs manager, provider pool.

- [ ] **Step 2: Update CI to build the binary**

Add step: `cargo build --release -p the-one-mcp --bin the-one-mcp`

- [ ] **Step 3: Run release gate locally**

Run: `bash scripts/release-gate.sh`
Expected: All gates pass.

- [ ] **Step 4: Commit**

```bash
git add scripts/ .github/
git commit -m "feat: update release gate and CI for production overhaul"
```

### Task 22: Update docs and version

**Files:**
- Modify: `docs/guides/the-one-mcp-complete-guide.md`
- Modify: `docs/guides/quickstart.md`
- Modify: `README.md`
- Modify: `PROGRESS.md`
- Modify: `CHANGELOG.md`
- Modify: `VERSION`

- [ ] **Step 1: Update complete guide**

Document: new config fields (embedding, nano_providers, limits, external_docs_root), new tools (docs CRUD, trash, reindex, config.update), transport modes, provider pool configuration.

- [ ] **Step 2: Update quickstart**

Add: binary usage (`the-one-mcp serve`), Claude Code integration (`claude mcp add`), basic config example.

- [ ] **Step 3: Update README**

Add transport modes, embedding info, managed docs, current capabilities.

- [ ] **Step 4: Update PROGRESS, CHANGELOG, VERSION**

VERSION: `v0.2.0`
CHANGELOG: document all changes.
PROGRESS: update stage status.

- [ ] **Step 5: Commit**

```bash
git add docs/ README.md PROGRESS.md CHANGELOG.md VERSION
git commit -m "docs: update guides, quickstart, README for v0.2.0 production overhaul"
```

### Task 23: Final integration test + tag

- [ ] **Step 1: Run full workspace validation**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
bash scripts/release-gate.sh
```

All must pass.

- [ ] **Step 2: Tag release**

```bash
git tag -a v0.2.0 -m "Production overhaul: transport, RAG, docs management, provider pool, configurable limits"
git push origin main --tags
```
