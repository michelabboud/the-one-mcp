# Multimodal Image Support + Reranking Implementation Plan (v0.6.0)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Ship v0.6.0 with three bundled features: fastembed 5.x migration, cross-encoder reranking, and full image embedding/search with OCR.

**Architecture:** Bump fastembed 4→5.13. Add `RerankerProvider` trait that wraps `fastembed::TextRerank`. Add `ImageEmbeddingProvider` trait that wraps `fastembed::ImageEmbedding`. Separate Qdrant collection per project for images. OCR via optional `tesseract` feature. Feature flags gate compile-time inclusion; runtime config toggles gate per-project activation.

**Tech Stack:** Rust, fastembed 5.x, tesseract (optional), image crate, tokio, Qdrant.

---

## Phase 1: fastembed 5.x Migration

### Task 1: Bump fastembed to 5.13 and fix API drift

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/the-one-memory/src/embeddings.rs`

- [ ] **Step 1: Bump version in workspace Cargo.toml**

Change `fastembed = "4"` to `fastembed = "5.13"` in the workspace `[workspace.dependencies]` section.

- [ ] **Step 2: Run cargo build to discover API drift**

Run: `cargo build -p the-one-memory 2>&1 | head -80`

Capture all errors. Common v4→v5 changes to watch for:
- `InitOptions` constructor signature changes
- `EmbeddingModel` enum variant renames
- Return types on `embed()` method
- `with_show_download_progress` removed/moved

- [ ] **Step 3: Fix API drift in `enum_from_name`**

Add the 6 previously-stubbed variants that now exist in fastembed 5.x:
```rust
"BGEM3" => fastembed::EmbeddingModel::BGEM3,
"JinaEmbeddingsV2BaseEN" => fastembed::EmbeddingModel::JinaEmbeddingsV2BaseEN,
"SnowflakeArcticEmbedM" => fastembed::EmbeddingModel::SnowflakeArcticEmbedM,
"AllMpnetBaseV2" => fastembed::EmbeddingModel::AllMpnetBaseV2,
"EmbeddingGemma300M" => fastembed::EmbeddingModel::EmbeddingGemma300M,
"SnowflakeArcticEmbedMQ" => fastembed::EmbeddingModel::SnowflakeArcticEmbedMQ,
```

- [ ] **Step 4: Fix FastEmbedProvider::new if InitOptions signature changed**

Check whether `InitOptions::new(model)` still exists or if it's now `InitOptions::new().with_model(model)` or similar. Update accordingly.

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p the-one-memory -- --no-capture`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/the-one-memory/src/embeddings.rs
git commit -m "feat: bump fastembed 4→5.13, wire 6 new text model variants"
```

---

## Phase 2: Reranking

### Task 2: Add reranker model registry

**Files:**
- Create: `models/rerank-models.toml`
- Modify: `crates/the-one-memory/src/models_registry.rs`

- [ ] **Step 1: Create rerank-models.toml**

```toml
[meta]
fastembed_crate_version = "5"
updated = "2026-04-05"

[models.jina-reranker-v2-base-multilingual]
name = "jina-reranker-v2-base-multilingual"
size_mb = 280
multilingual = true
description = "Multilingual cross-encoder. Recommended default."
fastembed_enum = "JINARerankerV2BaseMultilingual"
default = true

[models.jina-reranker-v1-base-en]
name = "jina-reranker-v1-base-en"
size_mb = 140
multilingual = false
description = "English-only reranker."
fastembed_enum = "JINARerankerV1BaseEn"
default = false

[models.jina-reranker-v1-turbo-en]
name = "jina-reranker-v1-turbo-en"
size_mb = 60
multilingual = false
description = "Fastest English reranker."
fastembed_enum = "JINARerankerV1TurboEn"
default = false

[models.bge-reranker-base]
name = "bge-reranker-base"
size_mb = 280
multilingual = false
description = "BAAI BGE reranker baseline."
fastembed_enum = "BGERerankerBase"
default = false

[models.bge-reranker-v2-m3]
name = "bge-reranker-v2-m3"
size_mb = 560
multilingual = true
description = "Highest quality, multilingual."
fastembed_enum = "BGERerankerV2M3"
default = false
```

- [ ] **Step 2: Add `RerankModel` struct and parser to models_registry.rs**

Add after `LocalModel` struct:
```rust
const RERANK_MODELS_TOML: &str = include_str!("../../../models/rerank-models.toml");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RerankModel {
    pub name: String,
    pub size_mb: u32,
    pub multilingual: bool,
    pub description: String,
    pub fastembed_enum: String,
    pub default: bool,
}

#[derive(Debug, Deserialize)]
struct RerankRegistryFile {
    #[allow(dead_code)]
    meta: LocalRegistryMeta,
    models: HashMap<String, RerankModel>,
}

pub fn list_rerank_models() -> Vec<RerankModel> {
    let file: RerankRegistryFile = toml::from_str(RERANK_MODELS_TOML)
        .expect("embedded rerank-models.toml is valid");
    file.models.into_values().collect()
}

pub fn default_rerank_model() -> RerankModel {
    list_rerank_models()
        .into_iter()
        .find(|m| m.default)
        .expect("rerank registry must have exactly one default")
}
```

- [ ] **Step 3: Add tests**

```rust
#[test]
fn test_rerank_models_parses_without_error() {
    let models = list_rerank_models();
    assert!(!models.is_empty());
}

#[test]
fn test_default_rerank_model_is_jina_v2() {
    let model = default_rerank_model();
    assert_eq!(model.fastembed_enum, "JINARerankerV2BaseMultilingual");
    assert!(model.default);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p the-one-memory rerank -- --no-capture`

- [ ] **Step 5: Commit**

```bash
git add models/rerank-models.toml crates/the-one-memory/src/models_registry.rs
git commit -m "feat: add rerank model registry with 5 cross-encoder models"
```

---

### Task 3: Add RerankerProvider trait and FastEmbedReranker

**Files:**
- Create: `crates/the-one-memory/src/reranker.rs`
- Modify: `crates/the-one-memory/src/lib.rs`

- [ ] **Step 1: Create reranker.rs**

```rust
//! Cross-encoder reranking — improves retrieval quality by re-scoring a
//! shortlist of bi-encoder results with a query-aware model.

use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct RerankHit {
    pub index: usize,
    pub score: f32,
}

#[async_trait]
pub trait RerankerProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankHit>, String>;
}

#[cfg(feature = "local-embeddings")]
mod local {
    use super::*;
    use std::sync::Arc;

    fn rerank_enum_from_name(name: &str) -> fastembed::RerankerModel {
        match name {
            "JINARerankerV2BaseMultilingual" => fastembed::RerankerModel::JINARerankerV2BaseMultilingual,
            "JINARerankerV1BaseEn" => fastembed::RerankerModel::JINARerankerV1BaseEn,
            "JINARerankerV1TurboEn" => fastembed::RerankerModel::JINARerankerV1TurboEn,
            "BGERerankerBase" => fastembed::RerankerModel::BGERerankerBase,
            "BGERerankerV2M3" => fastembed::RerankerModel::BGERerankerV2M3,
            _ => {
                tracing::warn!(
                    "Unknown reranker enum '{}', falling back to JINARerankerV2BaseMultilingual",
                    name
                );
                fastembed::RerankerModel::JINARerankerV2BaseMultilingual
            }
        }
    }

    pub fn resolve_rerank_model(name: &str) -> fastembed::RerankerModel {
        let name_lower = name.to_ascii_lowercase();
        if name_lower == "default" {
            let default = crate::models_registry::default_rerank_model();
            return rerank_enum_from_name(&default.fastembed_enum);
        }
        let models = crate::models_registry::list_rerank_models();
        if let Some(m) = models.iter().find(|m| m.name.to_ascii_lowercase() == name_lower) {
            return rerank_enum_from_name(&m.fastembed_enum);
        }
        rerank_enum_from_name(name)
    }

    pub struct FastEmbedReranker {
        model: Arc<fastembed::TextRerank>,
        model_name: String,
    }

    impl FastEmbedReranker {
        pub fn new(model_name: &str) -> Result<Self, String> {
            let model_enum = resolve_rerank_model(model_name);
            let model = fastembed::TextRerank::try_new(
                fastembed::RerankInitOptions::new(model_enum)
                    .with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed reranker init failed: {e}"))?;
            Ok(Self {
                model: Arc::new(model),
                model_name: model_name.to_string(),
            })
        }

        pub fn model_name(&self) -> &str {
            &self.model_name
        }
    }

    #[async_trait]
    impl RerankerProvider for FastEmbedReranker {
        fn name(&self) -> &str {
            "fastembed-reranker"
        }

        async fn rerank(
            &self,
            query: &str,
            documents: &[String],
            top_k: usize,
        ) -> Result<Vec<RerankHit>, String> {
            let query = query.to_string();
            let documents = documents.to_vec();
            let model = Arc::clone(&self.model);
            tokio::task::spawn_blocking(move || {
                let results = model
                    .rerank(&query, documents.iter().collect::<Vec<_>>(), true, None)
                    .map_err(|e| format!("fastembed rerank failed: {e}"))?;
                let mut hits: Vec<RerankHit> = results
                    .into_iter()
                    .map(|r| RerankHit {
                        index: r.index,
                        score: r.score,
                    })
                    .collect();
                hits.truncate(top_k);
                Ok(hits)
            })
            .await
            .map_err(|e| format!("tokio join error: {e}"))?
        }
    }
}

#[cfg(feature = "local-embeddings")]
pub use local::{resolve_rerank_model, FastEmbedReranker};

#[cfg(test)]
mod tests {
    #[cfg(feature = "local-embeddings")]
    mod local_tests {
        use super::super::*;
        use super::super::local::*;

        #[test]
        fn test_resolve_rerank_model_default() {
            let model = resolve_rerank_model("default");
            // Just verify no panic — enum matching is what matters
            let _ = model;
        }

        #[test]
        fn test_resolve_rerank_model_by_name() {
            let _ = resolve_rerank_model("jina-reranker-v1-turbo-en");
        }

        #[tokio::test]
        async fn test_rerank_reorders_documents() {
            let reranker = FastEmbedReranker::new("default").expect("init");
            let docs = vec![
                "The weather is nice today.".to_string(),
                "Rust is a systems programming language.".to_string(),
                "Cats are cute animals.".to_string(),
            ];
            let hits = reranker
                .rerank("what language should I use for a low-level compiler?", &docs, 3)
                .await
                .expect("rerank");
            assert_eq!(hits.len(), 3);
            // The Rust doc should rank highest
            assert_eq!(hits[0].index, 1);
        }
    }
}
```

NOTE: The `fastembed::TextRerank::rerank` method signature may differ in 5.x. If compilation fails, consult fastembed 5.x docs and adjust: might be `rerank(query, documents, return_documents)` or similar.

- [ ] **Step 2: Export from lib.rs**

Add to `crates/the-one-memory/src/lib.rs`:
```rust
pub mod reranker;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p the-one-memory`

- [ ] **Step 4: Run reranker tests (download is slow on first run)**

Run: `cargo test -p the-one-memory reranker -- --no-capture`

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-memory/src/reranker.rs crates/the-one-memory/src/lib.rs
git commit -m "feat: add RerankerProvider trait and FastEmbedReranker"
```

---

### Task 4: Integrate reranker into MemoryEngine

**Files:**
- Modify: `crates/the-one-memory/src/lib.rs` (MemoryEngine struct and methods)
- Modify: `crates/the-one-core/src/config.rs` (add rerank config fields)

- [ ] **Step 1: Add rerank config fields**

In `crates/the-one-core/src/config.rs`, add to `ProjectConfig`:
```rust
#[serde(default = "default_rerank_enabled")]
pub rerank_enabled: bool,
#[serde(default = "default_rerank_model")]
pub rerank_model: String,
#[serde(default = "default_rerank_multiplier")]
pub rerank_fetch_multiplier: usize,
```

With default functions:
```rust
fn default_rerank_enabled() -> bool { false }  // opt-in for now
fn default_rerank_model() -> String { "default".to_string() }
fn default_rerank_multiplier() -> usize { 4 }
```

- [ ] **Step 2: Add reranker field to MemoryEngine**

Modify `crates/the-one-memory/src/lib.rs`:
```rust
pub struct MemoryEngine {
    // ... existing fields ...
    #[cfg(feature = "local-embeddings")]
    reranker: Option<Arc<dyn reranker::RerankerProvider>>,
    rerank_multiplier: usize,
}
```

- [ ] **Step 3: Add builder method**

```rust
impl MemoryEngine {
    #[cfg(feature = "local-embeddings")]
    pub fn with_reranker(
        mut self,
        reranker: Arc<dyn reranker::RerankerProvider>,
        multiplier: usize,
    ) -> Self {
        self.reranker = Some(reranker);
        self.rerank_multiplier = multiplier;
        self
    }
}
```

- [ ] **Step 4: Modify search method to rerank**

In the existing `search` method (or whatever it's called), wrap with rerank logic:
```rust
pub async fn search(&self, query_vec: Vec<f32>, query_text: &str, top_k: usize) 
    -> Result<Vec<SearchHit>, String> 
{
    let fetch_k = if self.reranker.is_some() {
        top_k * self.rerank_multiplier
    } else {
        top_k
    };
    
    let initial = self.qdrant_search(query_vec, fetch_k).await?;
    
    #[cfg(feature = "local-embeddings")]
    if let Some(reranker) = &self.reranker {
        let docs: Vec<String> = initial.iter().map(|h| h.content.clone()).collect();
        let reranked = reranker.rerank(query_text, &docs, top_k).await?;
        return Ok(reranked.into_iter().map(|r| initial[r.index].clone()).collect());
    }
    
    Ok(initial.into_iter().take(top_k).collect())
}
```

NOTE: The actual `search` method signature in lib.rs may differ — read it first and adapt.

- [ ] **Step 5: Tests**

Add integration test that sets up a MemoryEngine with reranker and verifies results change ordering.

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-memory/src/lib.rs crates/the-one-core/src/config.rs
git commit -m "feat: integrate reranker into MemoryEngine search pipeline"
```

---

## Phase 3: Image Infrastructure

### Task 5: Add image model registry

**Files:**
- Create: `models/image-models.toml`
- Modify: `crates/the-one-memory/src/models_registry.rs`

- [ ] **Step 1: Create image-models.toml**

```toml
[meta]
fastembed_crate_version = "5"
updated = "2026-04-05"

[models.nomic-embed-vision-v1_5]
name = "nomic-embed-vision-v1.5"
dims = 768
size_mb = 700
description = "Pairs with nomic-embed-text-v1.5 for unified text+image search."
fastembed_enum = "NomicEmbedVisionV15"
default = true
paired_text_model = "NomicEmbedTextV15"

[models.clip-vit-b-32]
name = "clip-ViT-B-32-vision"
dims = 512
size_mb = 350
description = "CLIP — industry standard image encoder."
fastembed_enum = "ClipVitB32"
default = false
paired_text_model = ""

[models.resnet50]
name = "resnet50-onnx"
dims = 2048
size_mb = 95
description = "Pure image features, no text pairing."
fastembed_enum = "Resnet50"
default = false
paired_text_model = ""

[models.unicom-vit-b-16]
name = "Unicom-ViT-B-16"
dims = 768
size_mb = 350
description = "Fine-grained classification."
fastembed_enum = "UnicomVitB16"
default = false
paired_text_model = ""

[models.unicom-vit-b-32]
name = "Unicom-ViT-B-32"
dims = 512
size_mb = 175
description = "Lighter Unicom variant."
fastembed_enum = "UnicomVitB32"
default = false
paired_text_model = ""
```

- [ ] **Step 2: Add ImageModel struct and parser**

In `models_registry.rs`:
```rust
const IMAGE_MODELS_TOML: &str = include_str!("../../../models/image-models.toml");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageModel {
    pub name: String,
    pub dims: usize,
    pub size_mb: u32,
    pub description: String,
    pub fastembed_enum: String,
    pub default: bool,
    #[serde(default)]
    pub paired_text_model: String,
}

#[derive(Debug, Deserialize)]
struct ImageRegistryFile {
    #[allow(dead_code)]
    meta: LocalRegistryMeta,
    models: HashMap<String, ImageModel>,
}

pub fn list_image_models() -> Vec<ImageModel> {
    let file: ImageRegistryFile = toml::from_str(IMAGE_MODELS_TOML)
        .expect("embedded image-models.toml is valid");
    file.models.into_values().collect()
}

pub fn default_image_model() -> ImageModel {
    list_image_models()
        .into_iter()
        .find(|m| m.default)
        .expect("image registry must have exactly one default")
}
```

- [ ] **Step 3: Tests**

```rust
#[test]
fn test_image_models_parses() {
    let models = list_image_models();
    assert_eq!(models.len(), 5);
}

#[test]
fn test_default_image_is_nomic_vision() {
    let model = default_image_model();
    assert_eq!(model.fastembed_enum, "NomicEmbedVisionV15");
    assert_eq!(model.dims, 768);
}
```

- [ ] **Step 4: Commit**

```bash
git add models/image-models.toml crates/the-one-memory/src/models_registry.rs
git commit -m "feat: add image model registry with 5 fastembed image models"
```

---

### Task 6: Add ImageEmbeddingProvider trait and FastEmbedImageProvider

**Files:**
- Create: `crates/the-one-memory/src/image_embeddings.rs`
- Modify: `crates/the-one-memory/src/lib.rs`
- Modify: `crates/the-one-memory/Cargo.toml` (add `image-embeddings` feature)

- [ ] **Step 1: Add feature flag to Cargo.toml**

```toml
[features]
default = ["local-embeddings"]
local-embeddings = ["dep:fastembed"]
image-embeddings = ["local-embeddings", "dep:image"]

[dependencies]
image = { version = "0.25", optional = true }
```

- [ ] **Step 2: Create image_embeddings.rs**

```rust
use async_trait::async_trait;
use std::path::{Path, PathBuf};

#[async_trait]
pub trait ImageEmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String>;
    async fn embed_batch(&self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>, String>;
}

#[cfg(feature = "image-embeddings")]
mod local {
    use super::*;
    use std::sync::Arc;

    fn image_enum_from_name(name: &str) -> fastembed::ImageEmbeddingModel {
        match name {
            "NomicEmbedVisionV15" => fastembed::ImageEmbeddingModel::NomicEmbedVisionV15,
            "ClipVitB32" => fastembed::ImageEmbeddingModel::ClipVitB32,
            "Resnet50" => fastembed::ImageEmbeddingModel::Resnet50,
            "UnicomVitB16" => fastembed::ImageEmbeddingModel::UnicomVitB16,
            "UnicomVitB32" => fastembed::ImageEmbeddingModel::UnicomVitB32,
            _ => {
                tracing::warn!(
                    "Unknown image embedding enum '{}', falling back to NomicEmbedVisionV15",
                    name
                );
                fastembed::ImageEmbeddingModel::NomicEmbedVisionV15
            }
        }
    }

    pub fn resolve_image_model(name: &str) -> (fastembed::ImageEmbeddingModel, usize) {
        let name_lower = name.to_ascii_lowercase();
        if name_lower == "default" {
            let default = crate::models_registry::default_image_model();
            return (image_enum_from_name(&default.fastembed_enum), default.dims);
        }
        let models = crate::models_registry::list_image_models();
        if let Some(m) = models.iter().find(|m| m.name.to_ascii_lowercase() == name_lower) {
            return (image_enum_from_name(&m.fastembed_enum), m.dims);
        }
        let default = crate::models_registry::default_image_model();
        (image_enum_from_name(&default.fastembed_enum), default.dims)
    }

    pub struct FastEmbedImageProvider {
        model: Arc<fastembed::ImageEmbedding>,
        dims: usize,
        model_name: String,
    }

    impl FastEmbedImageProvider {
        pub fn new(model_name: &str) -> Result<Self, String> {
            let (model_enum, dims) = resolve_image_model(model_name);
            let model = fastembed::ImageEmbedding::try_new(
                fastembed::ImageInitOptions::new(model_enum)
                    .with_show_download_progress(false),
            )
            .map_err(|e| format!("fastembed image init failed: {e}"))?;
            Ok(Self {
                model: Arc::new(model),
                dims,
                model_name: model_name.to_string(),
            })
        }
    }

    #[async_trait]
    impl ImageEmbeddingProvider for FastEmbedImageProvider {
        fn name(&self) -> &str {
            "fastembed-image"
        }

        fn dimensions(&self) -> usize {
            self.dims
        }

        async fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String> {
            let results = self.embed_batch(&[path.to_path_buf()]).await?;
            results.into_iter().next().ok_or_else(|| "empty".to_string())
        }

        async fn embed_batch(&self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>, String> {
            let paths = paths.to_vec();
            let model = Arc::clone(&self.model);
            tokio::task::spawn_blocking(move || {
                model
                    .embed(paths, None)
                    .map_err(|e| format!("fastembed image embed failed: {e}"))
            })
            .await
            .map_err(|e| format!("tokio join: {e}"))?
        }
    }
}

#[cfg(feature = "image-embeddings")]
pub use local::{resolve_image_model, FastEmbedImageProvider};
```

- [ ] **Step 3: Export from lib.rs**

Add: `pub mod image_embeddings;`

- [ ] **Step 4: Tests**

```rust
#[cfg(feature = "image-embeddings")]
#[tokio::test]
async fn test_image_provider_embeds_fixture() {
    let provider = FastEmbedImageProvider::new("default").expect("init");
    assert_eq!(provider.dimensions(), 768);
    let result = provider
        .embed_image(Path::new("tests/fixtures/images/tiny.png"))
        .await
        .expect("embed");
    assert_eq!(result.len(), 768);
}
```

Requires test fixture at `crates/the-one-memory/tests/fixtures/images/tiny.png` — create with:
```bash
# Tiny 1×1 red PNG (89 bytes)
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDAT\x08\x99c\xf8\xcf\xc0\x00\x00\x00\x03\x00\x01\x5e\xf3+\xea\x00\x00\x00\x00IEND\xaeB`\x82' > crates/the-one-memory/tests/fixtures/images/tiny.png
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/the-one-memory/Cargo.toml crates/the-one-memory/src/image_embeddings.rs crates/the-one-memory/src/lib.rs crates/the-one-memory/tests/fixtures/
git commit -m "feat: add ImageEmbeddingProvider trait and FastEmbedImageProvider"
```

---

### Task 7: Image ingest module (file walker + hash detection)

**Files:**
- Create: `crates/the-one-memory/src/image_ingest.rs`

- [ ] **Step 1: Create image_ingest.rs**

```rust
//! Image file discovery and change detection.

use std::fs;
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};

pub const DEFAULT_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone)]
pub struct DiscoveredImage {
    pub path: PathBuf,
    pub hash: String,
    pub size_bytes: u64,
    pub mtime_epoch: i64,
}

/// Walk a directory recursively and return all image files matching the given extensions.
pub fn discover_images(root: &Path, extensions: &[&str]) -> Vec<DiscoveredImage> {
    let mut results = Vec::new();
    walk(root, extensions, &mut results);
    results
}

fn walk(dir: &Path, extensions: &[&str], out: &mut Vec<DiscoveredImage>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip .trash and .git
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            walk(&path, extensions, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_ascii_lowercase();
            if extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower)) {
                if let Ok(metadata) = fs::metadata(&path) {
                    if let Ok(hash) = hash_file(&path) {
                        let mtime_epoch = metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        out.push(DiscoveredImage {
                            path,
                            hash,
                            size_bytes: metadata.len(),
                            mtime_epoch,
                        });
                    }
                }
            }
        }
    }
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_images_finds_fixtures() {
        let root = Path::new("tests/fixtures/images");
        let images = discover_images(root, DEFAULT_IMAGE_EXTENSIONS);
        assert!(!images.is_empty(), "should find at least the tiny.png fixture");
    }

    #[test]
    fn test_discover_images_filters_extensions() {
        let root = Path::new("tests/fixtures/images");
        let images = discover_images(root, &["jpg"]);
        // tiny.png should not match
        assert!(!images.iter().any(|i| i.path.extension().unwrap() == "png"));
    }

    #[test]
    fn test_hash_deterministic() {
        let root = Path::new("tests/fixtures/images");
        let images1 = discover_images(root, DEFAULT_IMAGE_EXTENSIONS);
        let images2 = discover_images(root, DEFAULT_IMAGE_EXTENSIONS);
        for (a, b) in images1.iter().zip(images2.iter()) {
            assert_eq!(a.hash, b.hash);
        }
    }
}
```

- [ ] **Step 2: Export + commit**

```bash
# Add pub mod image_ingest; to lib.rs
git add crates/the-one-memory/src/image_ingest.rs crates/the-one-memory/src/lib.rs
git commit -m "feat: add image discovery and hash-based change detection"
```

---

### Task 8: Thumbnail generation

**Files:**
- Create: `crates/the-one-memory/src/thumbnail.rs`

- [ ] **Step 1: Create thumbnail.rs**

```rust
//! Thumbnail generation for indexed images.

#[cfg(feature = "image-embeddings")]
mod gen {
    use image::imageops::FilterType;
    use std::path::Path;

    /// Generate a thumbnail at the given max dimension, preserving aspect ratio.
    /// Writes as WebP to `output_path`.
    pub fn generate_thumbnail(
        input: &Path,
        output_path: &Path,
        max_dim: u32,
    ) -> Result<(), String> {
        let img = image::open(input).map_err(|e| format!("image open: {e}"))?;
        let thumbnail = img.resize(max_dim, max_dim, FilterType::Lanczos3);
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        thumbnail
            .save(output_path)
            .map_err(|e| format!("thumbnail save: {e}"))?;
        Ok(())
    }
}

#[cfg(feature = "image-embeddings")]
pub use gen::generate_thumbnail;

#[cfg(test)]
#[cfg(feature = "image-embeddings")]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_thumbnail_generation() {
        let input = Path::new("tests/fixtures/images/tiny.png");
        let output = Path::new("/tmp/the-one-mcp-test-thumb.webp");
        let _ = std::fs::remove_file(output);
        generate_thumbnail(input, output, 256).expect("generate");
        assert!(output.exists());
        std::fs::remove_file(output).ok();
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/the-one-memory/src/thumbnail.rs crates/the-one-memory/src/lib.rs
git commit -m "feat: add thumbnail generation via image crate"
```

---

### Task 9: OCR module (tesseract wrapper, feature-gated)

**Files:**
- Create: `crates/the-one-memory/src/ocr.rs`
- Modify: `crates/the-one-memory/Cargo.toml` (add image-ocr feature)

- [ ] **Step 1: Add feature to Cargo.toml**

```toml
[features]
image-ocr = ["image-embeddings", "dep:tesseract"]

[dependencies]
tesseract = { version = "0.15", optional = true }
```

- [ ] **Step 2: Create ocr.rs**

```rust
//! OCR text extraction from images via tesseract.

#[cfg(feature = "image-ocr")]
mod ocr_impl {
    use std::path::Path;

    pub fn extract_text(image_path: &Path, language: &str) -> Result<String, String> {
        let path_str = image_path
            .to_str()
            .ok_or_else(|| "image path is not valid UTF-8".to_string())?;
        let mut tess = tesseract::Tesseract::new(None, Some(language))
            .map_err(|e| format!("tesseract init: {e}"))?;
        tess = tess
            .set_image(path_str)
            .map_err(|e| format!("set_image: {e}"))?;
        let text = tess
            .get_text()
            .map_err(|e| format!("get_text: {e}"))?;
        Ok(text.trim().to_string())
    }
}

#[cfg(feature = "image-ocr")]
pub use ocr_impl::extract_text;

/// No-op stub when OCR feature is disabled.
#[cfg(not(feature = "image-ocr"))]
pub fn extract_text(_: &std::path::Path, _: &str) -> Result<String, String> {
    Err("OCR not enabled at compile time (feature image-ocr)".to_string())
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-memory/Cargo.toml crates/the-one-memory/src/ocr.rs crates/the-one-memory/src/lib.rs
git commit -m "feat: add OCR module with tesseract (feature-gated)"
```

---

### Task 10: Image Qdrant collection support

**Files:**
- Modify: `crates/the-one-memory/src/qdrant.rs`

- [ ] **Step 1: Add image collection methods**

Add to `QdrantBackend` (or equivalent):
```rust
pub async fn create_image_collection(&self, project_id: &str, dims: usize) -> Result<(), String> {
    let collection = format!("the_one_images_{project_id}");
    self.create_collection(&collection, dims).await
}

pub async fn upsert_image_points(
    &self,
    project_id: &str,
    points: Vec<ImagePoint>,
) -> Result<(), String> {
    let collection = format!("the_one_images_{project_id}");
    self.upsert_points_generic(&collection, points).await
}

pub async fn search_images(
    &self,
    project_id: &str,
    query_vec: Vec<f32>,
    top_k: usize,
) -> Result<Vec<ImageSearchResult>, String> {
    let collection = format!("the_one_images_{project_id}");
    self.search_generic(&collection, query_vec, top_k).await
}
```

- [ ] **Step 2: Define ImagePoint and ImageSearchResult**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub source_path: String,
    pub file_size: u64,
    pub mtime_epoch: i64,
    pub caption: Option<String>,
    pub ocr_text: Option<String>,
    pub thumbnail_path: Option<String>,
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/the-one-memory/src/qdrant.rs
git commit -m "feat: add Qdrant methods for image collection management"
```

---

## Phase 4: MCP Integration

### Task 11: API types for image search/ingest

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`

- [ ] **Step 1: Add new types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchRequest {
    pub project_root: String,
    pub project_id: String,
    pub query: String,
    pub top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchHit {
    pub id: String,
    pub source_path: String,
    pub thumbnail_path: Option<String>,
    pub caption: Option<String>,
    pub ocr_text: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSearchResponse {
    pub hits: Vec<ImageSearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageIngestRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageIngestResponse {
    pub path: String,
    pub dims: usize,
    pub ocr_extracted: bool,
    pub thumbnail_generated: bool,
}
```

- [ ] **Step 2: Tests + commit**

```bash
git add crates/the-one-mcp/src/api.rs
git commit -m "feat: add API types for image search and ingest"
```

---

### Task 12: Broker methods for image search/ingest

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Add image_search and image_ingest methods**

```rust
impl McpBroker {
    pub async fn image_search(&self, request: ImageSearchRequest) 
        -> Result<ImageSearchResponse, CoreError> 
    {
        // Get memory engine for project
        // Check if image embeddings are enabled
        // Embed query using TEXT encoder (same space as Nomic vision)
        // Search Qdrant image collection
        // Return hits
    }

    pub async fn image_ingest(&self, request: ImageIngestRequest) 
        -> Result<ImageIngestResponse, CoreError> 
    {
        // Validate path exists and has allowed extension
        // Hash file for change detection
        // Embed image
        // OCR if enabled
        // Generate thumbnail if enabled
        // Upsert to Qdrant
        // Store metadata in SQLite
    }
}
```

This task has the most novel logic — detailed implementation left to the implementer using the primitives from Tasks 5-10.

- [ ] **Step 2: Tests + commit**

---

### Task 13: Add memory.search_images and memory.ingest_image tools

**Files:**
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`

- [ ] **Step 1: Add tool definitions**

Add two new `tool_def` entries to `tool_definitions()`:
```rust
tool_def("memory.search_images", "Semantic search over indexed project images. Finds screenshots, diagrams, and photos matching a natural-language query.", json!({
    "type": "object",
    "properties": {
        "project_root": { "type": "string" },
        "project_id": { "type": "string" },
        "query": { "type": "string", "description": "Natural-language search query" },
        "top_k": { "type": "integer", "default": 5 }
    },
    "required": ["project_root", "project_id", "query"]
})),
tool_def("memory.ingest_image", "Manually index an image file with optional caption. Extracts OCR text and generates a thumbnail.", json!({
    "type": "object",
    "properties": {
        "project_root": { "type": "string" },
        "project_id": { "type": "string" },
        "path": { "type": "string", "description": "Absolute or project-relative path to the image" },
        "caption": { "type": "string", "description": "Optional user-provided caption" }
    },
    "required": ["project_root", "project_id", "path"]
})),
```

Update the count test: 15 → 17.

- [ ] **Step 2: Add dispatch arms**

Add to `dispatch_tool` in `jsonrpc.rs`:
```rust
"memory.search_images" => { /* extract args, call broker.image_search */ }
"memory.ingest_image" => { /* extract args, call broker.image_ingest */ }
```

- [ ] **Step 3: Tests + commit**

---

### Task 14: Add maintain actions for image management

**Files:**
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`

- [ ] **Step 1: Extend dispatch_maintain with image actions**

```rust
"images.rescan" => {
    // Trigger full image re-ingestion
}
"images.delete" => {
    // Delete a specific image from index
}
"images.clear" => {
    // Clear all images from index
}
```

Update the `maintain` tool schema to include these new actions in its enum.

- [ ] **Step 2: Tests + commit**

---

### Task 15: Config fields, limits, env vars

**Files:**
- Modify: `crates/the-one-core/src/config.rs`
- Modify: `crates/the-one-core/src/limits.rs`

- [ ] **Step 1: Add to ProjectConfig**

```rust
#[serde(default)]
pub image_embedding_enabled: bool,
#[serde(default = "default_image_model")]
pub image_embedding_model: String,
#[serde(default)]
pub image_ocr_enabled: bool,
#[serde(default = "default_ocr_language")]
pub image_ocr_language: String,
#[serde(default = "default_thumbnail_enabled")]
pub image_thumbnail_enabled: bool,
```

- [ ] **Step 2: Add limits**

```rust
pub max_image_size_bytes: u64,        // default 10 MB
pub max_images_per_project: usize,    // default 500
pub max_image_search_hits: usize,     // default 5
pub image_search_score_threshold: f32, // default 0.25
```

- [ ] **Step 3: Add env vars**

`THE_ONE_IMAGE_EMBEDDING_ENABLED`, `THE_ONE_IMAGE_EMBEDDING_MODEL`, `THE_ONE_IMAGE_OCR_ENABLED`, etc.

- [ ] **Step 4: Tests + commit**

---

### Task 16: Extend setup:refresh for auto image ingestion

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs` (project_refresh method)

- [ ] **Step 1: Modify project_refresh to also scan for images**

After the existing markdown scan, if `config.image_embedding_enabled`:
1. Walk `<project>/.the-one/images/` and `<project>/.the-one/docs/` for images
2. Diff against SQLite `managed_images` table
3. Ingest new/changed images
4. Remove vanished images from Qdrant

- [ ] **Step 2: Tests + commit**

---

### Task 17: JSON schemas

**Files:**
- Create: `schemas/mcp/v1beta/memory.search_images.request.schema.json`
- Create: `schemas/mcp/v1beta/memory.search_images.response.schema.json`
- Create: `schemas/mcp/v1beta/memory.ingest_image.request.schema.json`
- Create: `schemas/mcp/v1beta/memory.ingest_image.response.schema.json`
- Modify: `crates/the-one-mcp/src/lib.rs` (expected schema list)

Standard pattern — see existing schemas. Update expected list to include 4 new files. Total schemas: 31 → 35.

Commit: `feat: add JSON schemas for image tools`

---

## Phase 5: Docs + Release

### Task 18: User guides

**Files:**
- Create: `docs/guides/image-search.md`
- Create: `docs/guides/reranking.md`

Write user-facing guides for each feature. Include:
- What it does
- How to enable it (feature flag + config)
- Example usage
- Performance characteristics
- Troubleshooting

Commit: `docs: add image search and reranking user guides`

---

### Task 19: Update README, CHANGELOG, PROGRESS, CLAUDE.md

**Files:**
- Modify: `README.md` (new features section, stats)
- Modify: `CHANGELOG.md` (v0.6.0 entry)
- Modify: `PROGRESS.md` (v0.6.0 column)
- Modify: `CLAUDE.md` (tool count 15→17, architecture)
- Modify: `VERSION` (v0.5.0 → v0.6.0)

CHANGELOG entry template:
```markdown
## [0.6.0] - 2026-04-05

### Added
- Cross-encoder reranking for memory.search (jina-reranker-v2 default, already cached)
- Image embedding and semantic search via fastembed ImageEmbedding
- 5 image models: Nomic Vision, CLIP, Resnet50, Unicom ViT-B/16, Unicom ViT-B/32
- OCR text extraction from images via tesseract (feature-gated)
- Thumbnail generation for indexed images
- 2 new MCP tools: memory.search_images, memory.ingest_image
- 3 new maintain actions: images.rescan, images.delete, images.clear
- 6 previously-stubbed text models now working: BGE-M3, ModernBertEmbedLarge, JinaEmbeddingsV2BaseEN, SnowflakeArcticEmbedM, AllMpnetBaseV2, EmbeddingGemma300M

### Changed
- BREAKING: fastembed bumped from 4 to 5.13 (internal API drift fixed)
- memory.search now optionally reranks top-N candidates for improved precision
- MCP tool count: 15 → 17

### Dependencies
- fastembed 5.13
- image 0.25 (optional, image-embeddings feature)
- tesseract 0.15 (optional, image-ocr feature)
```

Commit: `docs: v0.6.0 — multimodal images, reranking, CHANGELOG`

---

### Task 20: Final validation + release

- [ ] **Step 1: Full CI pipeline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p the-one-mcp --bin the-one-mcp
```

- [ ] **Step 2: Feature matrix build test**

```bash
# Default build
cargo build --release
# Without image support
cargo build --release --no-default-features --features local-embeddings
# With OCR
cargo build --release --features image-ocr
```

- [ ] **Step 3: Tag and push**

```bash
git tag -a v0.6.0 -m "v0.6.0: Multimodal images + reranking"
git push origin main --tags
```

- [ ] **Step 4: Trigger cross-platform release**

```bash
echo "y" | bash scripts/build.sh release v0.6.0
```

Monitor with: `gh run list --workflow release.yml --limit 1`
