# Production Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the remaining production-readiness gaps in Redis backend support, model update checks, MCP resources, and wake-up filtering so the shipped surfaces have no stubs, no placeholders, and no intentionally incomplete runtime behavior.

**Architecture:** Complete the unfinished seams instead of adding more compatibility shims. Redis becomes a real `MemoryEngine` backend with explicit persistence validation, operator-facing update checks become a true capability with structured status, resource reads become backed by SQLite state, and wake-up retrieval becomes consistent with the same palace metadata model used by ingest and search.

**Tech Stack:** Rust 2021, tokio, fred/RediSearch, fastembed or API embeddings, rusqlite, serde/serde_json, tracing, MCP JSON-RPC transport

---

## File Map

- Modify: `crates/the-one-memory/src/lib.rs`
- Modify: `crates/the-one-memory/src/redis_vectors.rs`
- Modify: `crates/the-one-memory/Cargo.toml`
- Modify: `crates/the-one-core/src/config.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/resources.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Modify: `docs/guides/redis-vector-backend.md`
- Modify: `docs/guides/configuration.md`
- Modify: `docs/guides/architecture.md`
- Modify: `docs/guides/conversation-memory.md`
- Modify: `docs/guides/mcp-resources.md`
- Modify: `docs/guides/api-reference.md`
- Test in place:
  - `crates/the-one-memory/src/lib.rs`
  - `crates/the-one-memory/src/redis_vectors.rs`
  - `crates/the-one-core/src/config.rs`
  - `crates/the-one-core/src/storage/sqlite.rs`
  - `crates/the-one-mcp/src/broker.rs`
  - `crates/the-one-mcp/src/resources.rs`

### Task 1: Finish the Redis-backed `MemoryEngine`

**Files:**
- Modify: `crates/the-one-memory/src/lib.rs`
- Modify: `crates/the-one-memory/src/redis_vectors.rs`
- Modify: `crates/the-one-memory/Cargo.toml`
- Test: `crates/the-one-memory/src/lib.rs`
- Test: `crates/the-one-memory/src/redis_vectors.rs`

- [ ] **Step 1: Write the failing Redis engine construction tests**

Add tests that prove the engine exposes a real Redis-backed backend instead of
silently degrading to local-only memory.

```rust
#[cfg(feature = "redis-vectors")]
#[tokio::test]
async fn redis_memory_engine_reports_redis_backend() {
    let engine = MemoryEngine::new_with_redis(
        "BAAI/bge-small-en-v1.5",
        512,
        RedisEngineConfig {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            index_name: "the_one_memories_test".to_string(),
            persistence_required: false,
        },
    )
    .await
    .expect("redis engine should construct");

    assert_eq!(engine.vector_backend_name(), "redis");
    assert!(engine.redis_backend().is_some());
}

#[cfg(feature = "redis-vectors")]
#[tokio::test]
async fn redis_memory_engine_rejects_invalid_index_names() {
    let err = MemoryEngine::new_with_redis(
        "BAAI/bge-small-en-v1.5",
        512,
        RedisEngineConfig {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            index_name: "bad index".to_string(),
            persistence_required: false,
        },
    )
    .await
    .expect_err("invalid index name must fail");

    assert!(err.contains("index"));
}
```

- [ ] **Step 2: Run the targeted Redis tests to verify they fail**

Run: `cargo test -p the-one-memory redis_memory_engine_reports_redis_backend -- --nocapture`
Expected: FAIL because `MemoryEngine::new_with_redis` and backend inspection do
not exist yet.

- [ ] **Step 3: Add a first-class Redis engine config and constructor**

Introduce a dedicated config type and backend enum in `lib.rs` rather than
smuggling Redis through a local-only engine.

```rust
#[derive(Clone, Debug)]
pub struct RedisEngineConfig {
    pub redis_url: String,
    pub index_name: String,
    pub persistence_required: bool,
}

#[derive(Clone)]
enum VectorBackend {
    Local,
    Qdrant(AsyncQdrantBackend),
    #[cfg(feature = "redis-vectors")]
    Redis(Arc<RedisVectorStore>),
}

impl MemoryEngine {
    #[cfg(feature = "redis-vectors")]
    pub async fn new_with_redis(
        embedding_model: &str,
        max_chunk_tokens: usize,
        redis: RedisEngineConfig,
    ) -> Result<Self, String> {
        let embedding_provider = Arc::new(EmbeddingProvider::new(embedding_model)?);
        let dims = embedding_provider.dimensions();
        let store = RedisVectorStore::connect(&redis.redis_url, &redis.index_name, dims).await?;
        if redis.persistence_required {
            store.verify_persistence().await?;
        }
        store.ensure_index().await?;

        Ok(Self {
            chunks: Vec::new(),
            embedding_provider,
            max_chunk_tokens,
            vector_backend: VectorBackend::Redis(Arc::new(store)),
            ..Self::empty_state(max_chunk_tokens)
        })
    }
}
```

- [ ] **Step 4: Route ingest/search/upsert through the Redis backend**

Add Redis-aware code paths in the same places that currently branch for Qdrant.
Do not add duplicate ingest logic.

```rust
match &self.vector_backend {
    VectorBackend::Local => {}
    VectorBackend::Qdrant(qdrant) => {
        qdrant.upsert_points(points).await?;
    }
    #[cfg(feature = "redis-vectors")]
    VectorBackend::Redis(redis) => {
        redis.upsert_chunks(&points).await?;
    }
}
```

```rust
let results = match &self.vector_backend {
    VectorBackend::Local => self.search_local_only(query, top_k)?,
    VectorBackend::Qdrant(qdrant) => self.search_qdrant(qdrant, query, top_k).await?,
    #[cfg(feature = "redis-vectors")]
    VectorBackend::Redis(redis) => self.search_redis(redis, query, top_k).await?,
};
```

- [ ] **Step 5: Add persistence verification to `RedisVectorStore`**

Turn the existing Redis helper into an operator-safe store that checks AOF/RDB
state when persistence is required.

```rust
impl RedisVectorStore {
    pub async fn verify_persistence(&self) -> Result<(), String> {
        let info = self.client.custom_raw("INFO", vec!["persistence"]).await?;
        let text = String::from_utf8(info).map_err(|e| e.to_string())?;

        let aof_enabled = text.contains("aof_enabled:1");
        let rdb_enabled = text.contains("rdb_bgsave_in_progress:")
            || text.contains("rdb_last_save_time:");

        if aof_enabled || rdb_enabled {
            Ok(())
        } else {
            Err("redis persistence is required but neither AOF nor RDB appears enabled".to_string())
        }
    }
}
```

- [ ] **Step 6: Add Redis integration tests for index creation and persistence validation**

Add focused tests around schema generation and persistence parsing.

```rust
#[cfg(feature = "redis-vectors")]
#[tokio::test]
async fn verify_persistence_accepts_aof_enabled_info() {
    let store = RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 512)
        .expect("store");

    let ok = store.parse_persistence_info(
        "aof_enabled:1\nrdb_last_save_time:1710000000\n",
    );

    assert!(ok.is_ok());
}
```

- [ ] **Step 7: Run Redis memory tests**

Run: `cargo test -p the-one-memory redis -- --nocapture`
Expected: PASS for Redis-specific constructor, validation, and store tests.

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-memory/src/lib.rs crates/the-one-memory/src/redis_vectors.rs crates/the-one-memory/Cargo.toml
git commit -m "feat: add production redis memory backend"
```

### Task 2: Wire the broker to the real Redis backend

**Files:**
- Modify: `crates/the-one-core/src/config.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Test: `crates/the-one-core/src/config.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing broker tests for Redis backend selection**

```rust
#[tokio::test]
async fn broker_builds_redis_memory_engine_when_redis_backend_selected() {
    let project = temp_project_with_config(serde_json::json!({
        "vector_backend": "redis",
        "redis_url": "redis://127.0.0.1:6379",
        "redis_index_name": "the_one_memories",
        "redis_persistence_required": false,
        "embedding_provider": "local"
    }));

    let broker = McpBroker::new(project.path()).await.expect("broker");
    let engine = broker
        .debug_build_memory_engine_for_test(project.path(), "proj-test")
        .expect("engine");

    assert_eq!(engine.vector_backend_name(), "redis");
}

#[tokio::test]
async fn broker_rejects_redis_backend_when_required_persistence_is_missing() {
    let project = temp_project_with_config(serde_json::json!({
        "vector_backend": "redis",
        "redis_url": "redis://127.0.0.1:6379",
        "redis_persistence_required": true,
        "embedding_provider": "local"
    }));

    let broker = McpBroker::new(project.path()).await.expect("broker");
    let err = broker
        .debug_build_memory_engine_for_test(project.path(), "proj-test")
        .expect_err("missing persistence must fail");

    assert!(err.to_string().contains("persistence"));
}
```

- [ ] **Step 2: Run the broker Redis tests to verify they fail**

Run: `cargo test -p the-one-mcp broker_builds_redis_memory_engine_when_redis_backend_selected -- --nocapture`
Expected: FAIL because the broker still falls back to local-only memory.

- [ ] **Step 3: Replace the Redis fallback branch in `build_memory_engine`**

Use the new `MemoryEngine::new_with_redis` constructor directly.

```rust
let mut engine = if vector_backend == "redis" {
    let redis_url = config.redis_url.clone().ok_or_else(|| {
        CoreError::InvalidProjectConfig(
            "vector_backend 'redis' requires redis_url".to_string(),
        )
    })?;

    let redis_index_name = config
        .redis_index_name
        .clone()
        .unwrap_or_else(|| "the_one_memories".to_string());

    MemoryEngine::new_with_redis(
        &config.embedding_model,
        max_chunk_tokens,
        RedisEngineConfig {
            redis_url,
            index_name: redis_index_name,
            persistence_required: config.redis_persistence_required,
        },
    )
    .await
    .map_err(CoreError::Embedding)?
} else if config.embedding_provider == "api" {
    // existing API path
```

- [ ] **Step 4: Remove Redis fallback warnings and replace them with real telemetry**

```rust
tracing::info!(
    "vector_backend=redis active for project {project_id} using index {redis_index_name}"
);
```

- [ ] **Step 5: Add config tests for Redis defaults and env overrides**

```rust
#[test]
fn config_defaults_vector_backend_to_qdrant() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.vector_backend, "qdrant");
    assert_eq!(cfg.redis_index_name.as_deref(), None);
}
```

- [ ] **Step 6: Run broker and config tests**

Run: `cargo test -p the-one-core config_parses_redis_vector_backend_settings -- --nocapture`
Expected: PASS

Run: `cargo test -p the-one-mcp redis_backend -- --nocapture`
Expected: PASS for Redis backend construction and validation cases.

- [ ] **Step 7: Commit**

```bash
git add crates/the-one-core/src/config.rs crates/the-one-mcp/src/broker.rs
git commit -m "feat: wire broker to redis memory backend"
```

### Task 3: Replace `models.check` with a real update check

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing tests for structured model update status**

```rust
#[tokio::test]
async fn models_check_reports_no_updates_when_registries_match() {
    let broker = test_broker();
    let result = broker
        .models_check_updates_with(test_model_update_source_matching())
        .await
        .expect("check");

    assert_eq!(result["status"], "up_to_date");
    assert_eq!(result["updates_available"], 0);
}

#[tokio::test]
async fn models_check_reports_provider_errors_without_panicking() {
    let broker = test_broker();
    let result = broker
        .models_check_updates_with(test_model_update_source_failing("timeout"))
        .await
        .expect("check");

    assert_eq!(result["status"], "degraded");
    assert_eq!(result["sources"][0]["status"], "error");
}
```

- [ ] **Step 2: Run the model update tests to verify they fail**

Run: `cargo test -p the-one-mcp models_check_reports_no_updates_when_registries_match -- --nocapture`
Expected: FAIL because no async or source-backed update check exists.

- [ ] **Step 3: Introduce a real update source abstraction**

```rust
#[async_trait::async_trait]
trait ModelUpdateSource: Send + Sync {
    async fn fetch_local_registry_versions(&self) -> Result<RegistrySnapshot, CoreError>;
    async fn fetch_api_registry_versions(&self) -> Result<RegistrySnapshot, CoreError>;
}

#[derive(Debug, Serialize)]
struct RegistrySnapshot {
    source: String,
    version: String,
    entries: Vec<String>,
}
```

- [ ] **Step 4: Implement a real comparison result**

```rust
#[derive(Debug, Serialize)]
struct ModelUpdateCheckResult {
    status: String,
    updates_available: usize,
    sources: Vec<ModelUpdateSourceStatus>,
}

#[derive(Debug, Serialize)]
struct ModelUpdateSourceStatus {
    source: String,
    status: String,
    current_version: String,
    latest_version: Option<String>,
    added_models: Vec<String>,
    removed_models: Vec<String>,
    error: Option<String>,
}
```

- [ ] **Step 5: Replace the stubbed broker method**

```rust
pub async fn models_check_updates(&self) -> Result<serde_json::Value, CoreError> {
    let result = self
        .models_check_updates_with(DefaultModelUpdateSource::new())
        .await?;
    serde_json::to_value(result).map_err(CoreError::from)
}
```

- [ ] **Step 6: Update JSON-RPC dispatch if the method becomes async**

```rust
"models.check" => Ok(serde_json::to_value(
    broker.models_check_updates().await?
)?),
```

- [ ] **Step 7: Run model update tests**

Run: `cargo test -p the-one-mcp models_check -- --nocapture`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-mcp/src/api.rs crates/the-one-mcp/src/broker.rs crates/the-one-mcp/src/transport/jsonrpc.rs
git commit -m "feat: replace model update stub with real checks"
```

### Task 4: Implement real `catalog/enabled` resource reads

**Files:**
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-mcp/src/resources.rs`
- Modify: `crates/the-one-mcp/src/transport/jsonrpc.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/resources.rs`

- [ ] **Step 1: Write failing tests for non-empty enabled tool resources**

```rust
#[test]
fn test_read_resource_catalog_enabled_returns_enabled_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    seed_enabled_tool(&root, "tool.find", "codex");

    let resp = read_resource_with_client(&root, "codex", "the-one://catalog/enabled")
        .expect("read");

    assert!(resp.contents[0].text.contains("tool.find"));
}
```

- [ ] **Step 2: Run the resource tests to verify they fail**

Run: `cargo test -p the-one-mcp test_read_resource_catalog_enabled_returns_enabled_tools -- --nocapture`
Expected: FAIL because the resource always returns `[]`.

- [ ] **Step 3: Add a storage read for enabled tools**

```rust
pub fn list_enabled_tools(
    &self,
    cli: &str,
    project_root: &Path,
) -> Result<Vec<String>, CoreError> {
    let conn = self.connection()?;
    let mut stmt = conn.prepare(
        "SELECT tool_id
           FROM enabled_tools
          WHERE cli = ?1 AND project_root = ?2
          ORDER BY tool_id ASC",
    )?;

    let rows = stmt.query_map(params![cli, project_root.to_string_lossy()], |row| row.get(0))?;
    rows.collect::<Result<Vec<String>, _>>().map_err(CoreError::from)
}
```

- [ ] **Step 4: Thread client context into resource reads**

Replace the placeholder implementation with a real SQLite-backed lookup.

```rust
fn read_catalog_resource(
    project_root: &Path,
    client_name: &str,
    identifier: &str,
    uri: &str,
) -> Result<ResourcesReadResponse, CoreError> {
    if identifier != "enabled" {
        return Err(CoreError::InvalidProjectConfig(format!(
            "unknown catalog resource: {identifier}"
        )));
    }

    let db = SqliteStorage::open(project_root)?;
    let enabled = db.list_enabled_tools(client_name, project_root)?;
    let text = serde_json::to_string(&enabled)?;

    Ok(ResourcesReadResponse {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: "application/json".to_string(),
            text,
        }],
    })
}
```

- [ ] **Step 5: Update JSON-RPC request handling to pass client identity**

```rust
let client_name = broker.client_name().unwrap_or("unknown");
let resp = read_resource_with_client(project_root, client_name, &uri)?;
```

- [ ] **Step 6: Add empty and non-empty resource tests**

```rust
#[test]
fn test_read_resource_catalog_enabled_returns_empty_array_when_no_tools_enabled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let resp = read_resource_with_client(&root, "codex", "the-one://catalog/enabled")
        .expect("read");
    assert_eq!(resp.contents[0].text, "[]");
}
```

- [ ] **Step 7: Run resource tests**

Run: `cargo test -p the-one-mcp resources -- --nocapture`
Expected: PASS with real enabled-tool payloads.

- [ ] **Step 8: Commit**

```bash
git add crates/the-one-core/src/storage/sqlite.rs crates/the-one-mcp/src/resources.rs crates/the-one-mcp/src/transport/jsonrpc.rs
git commit -m "feat: back catalog resource with enabled tools"
```

### Task 5: Make wake-up filtering consistent with full palace metadata

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write failing tests for hall and room wake-up filters**

```rust
#[tokio::test]
async fn memory_wake_up_can_filter_by_wing_hall_and_room() {
    let broker = seeded_broker_with_two_palace_conversations().await;

    let response = broker
        .memory_wake_up(MemoryWakeUpRequest {
            project_root: broker.project_root().display().to_string(),
            project_id: "proj-auth".to_string(),
            wing: Some("ops".to_string()),
            hall: Some("incidents".to_string()),
            room: Some("auth".to_string()),
            max_items: Some(10),
        })
        .await
        .expect("wake up");

    assert!(response.facts.iter().all(|fact| fact.contains("auth")));
    assert!(!response.facts.iter().any(|fact| fact.contains("payments")));
}
```

- [ ] **Step 2: Run the wake-up tests to verify they fail**

Run: `cargo test -p the-one-mcp memory_wake_up_can_filter_by_wing_hall_and_room -- --nocapture`
Expected: FAIL because `memory.wake_up` only filters by `wing`.

- [ ] **Step 3: Extend the wake-up request and DB query path**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWakeUpRequest {
    pub project_root: String,
    pub project_id: String,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
    pub max_items: Option<usize>,
}
```

```rust
pub fn list_conversation_sources(
    &self,
    wing: Option<&str>,
    hall: Option<&str>,
    room: Option<&str>,
    limit: usize,
) -> Result<Vec<ConversationSourceRecord>, CoreError> {
    // build WHERE clause with all supplied filters
}
```

- [ ] **Step 4: Update broker wake-up logic to use all palace filters**

```rust
let sources = db.list_conversation_sources(
    request.wing.as_deref(),
    request.hall.as_deref(),
    request.room.as_deref(),
    request.max_items.unwrap_or(10),
)?;
```

- [ ] **Step 5: Add tests for partial and full palace filtering**

```rust
#[tokio::test]
async fn memory_wake_up_room_filter_excludes_other_rooms() {
    // seed auth + pager rooms, query auth, assert pager excluded
}
```

- [ ] **Step 6: Run wake-up tests**

Run: `cargo test -p the-one-mcp wake_up -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/the-one-mcp/src/api.rs crates/the-one-core/src/storage/sqlite.rs crates/the-one-mcp/src/broker.rs
git commit -m "feat: add full palace filtering to wake-up packs"
```

### Task 6: Remove all incomplete-product language from the docs

**Files:**
- Modify: `docs/guides/redis-vector-backend.md`
- Modify: `docs/guides/configuration.md`
- Modify: `docs/guides/architecture.md`
- Modify: `docs/guides/conversation-memory.md`
- Modify: `docs/guides/mcp-resources.md`
- Modify: `docs/guides/api-reference.md`
- Test: documentation consistency via grep

- [ ] **Step 1: Update Redis docs to describe the real backend**

Replace the temporary integration note with production behavior.

```md
## Runtime behavior

When `vector_backend` is set to `redis`, the broker constructs a Redis-backed
memory engine, verifies persistence if required, and stores vector data in the
configured RediSearch index.
```

- [ ] **Step 2: Update MCP resources docs to remove the empty-array disclaimer**

```md
### Enabled tools (`catalog/enabled`)

Returns the enabled tool IDs for the active CLI and project context.
```

- [ ] **Step 3: Update conversation-memory docs to describe full palace filtering**

```md
Notes:

- Wake-up filtering supports `wing`, `hall`, and `room`.
- Omitting any field widens the match to all values for that dimension.
```

- [ ] **Step 4: Run grep checks to ensure incomplete wording is gone**

Run: `rg -n "currently empty|follow-up|under active integration|rejected today|wing only today|stub" docs/guides README.md crates/the-one-mcp/src/broker.rs`
Expected: no matches in the updated production surfaces.

- [ ] **Step 5: Commit**

```bash
git add docs/guides/redis-vector-backend.md docs/guides/configuration.md docs/guides/architecture.md docs/guides/conversation-memory.md docs/guides/mcp-resources.md docs/guides/api-reference.md
git commit -m "docs: align guides with production-ready behavior"
```

### Task 7: Full verification pass

**Files:**
- Verify workspace state only

- [ ] **Step 1: Run formatting**

Run: `cargo fmt --check`
Expected: PASS

- [ ] **Step 2: Run linting**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 4: Run targeted regression checks for the hardened surfaces**

Run: `cargo test -p the-one-memory redis -- --nocapture`
Expected: PASS

Run: `cargo test -p the-one-mcp models_check -- --nocapture`
Expected: PASS

Run: `cargo test -p the-one-mcp resources -- --nocapture`
Expected: PASS

Run: `cargo test -p the-one-mcp wake_up -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run final incompleteness scan**

Run: `rg -n "stub|placeholder|follow-up|under active integration|currently empty|rejected today|not added yet" crates docs README.md`
Expected: no matches in live production code or guides for the hardened areas

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: complete production hardening follow-up"
```

## Self-Review

Spec coverage:

- Redis backend runtime gap: covered by Tasks 1-2
- `models.check` stub: covered by Task 3
- `catalog/enabled` placeholder resource: covered by Task 4
- Wake-up filter inconsistency: covered by Task 5
- Documentation drift and incomplete wording: covered by Task 6
- End-to-end production verification: covered by Task 7

Placeholder scan:

- No `TODO`, `TBD`, "similar to above", or "implement later" markers remain in
  tasks.
- Every task includes exact file paths and runnable commands.

Type consistency:

- `RedisEngineConfig`, `vector_backend_name`, and `models_check_updates` naming
  is consistent across tasks.
- Palace filter fields are `wing`, `hall`, and `room` throughout.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-09-production-hardening-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
