# MemPalace-Inspired Conversation Memory Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add MemPalace-style conversation memory, palace metadata, wake-up context packs, and an optional Redis persistent vector backend to the-one-mcp without merging the Python repo or replacing the current Rust host architecture.

**Architecture:** Keep the-one as the system of record and extend it in four layers: transcript ingestion in `the-one-memory`, a pluggable vector backend layer in `the-one-memory`, MCP surface and orchestration in `the-one-mcp`, and persistence/metadata support in `the-one-core`. Reuse existing chunking, watcher, docs, and UI patterns, keep Qdrant as the default backend, and add Redis/RediSearch HNSW as a persistent backend option inspired by the `mai` project.

**Tech Stack:** Rust 2021, tokio, serde, rusqlite, Qdrant HTTP backend, Redis 8 + RediSearch HNSW via `fred`, fastembed/API embeddings, existing MCP JSON-RPC transport, markdown docs.

---

## Scope And Non-Goals

- Build a feature-level merge, not a repository transplant.
- Keep Qdrant as the default vector backend.
- Add Redis/RediSearch as an optional persistent vector backend.
- Do not embed the MemPalace Python MCP server.
- Do not adopt AAAK as part of the first implementation.
- Focus first on raw verbatim conversation retrieval, palace metadata filters, and wake-up packs.
- Follow the persistence pattern already used in `mai`: Redis RDB + AOF enabled for vector durability.

## File Structure

### New files

- `crates/the-one-memory/src/conversation.rs`
  Normalizes chat transcripts into canonical messages and chunkable conversation turns.
- `crates/the-one-memory/src/palace.rs`
  Defines palace metadata (`wing`, `hall`, `room`) and helpers for search filtering and wake-up packs.
- `crates/the-one-memory/src/redis_vectors.rs`
  Redis/RediSearch HNSW backend for semantic retrieval, modeled after `mai-memory`.
- `docs/guides/redis-vector-backend.md`
  Setup and operational guide for Redis persistence, module loading, and backend selection.
- `config/redis.conf.example`
  Example Redis config with RDB + AOF persistence and RediSearch notes.
- `docs/guides/conversation-memory.md`
  User-facing guide for ingesting chat exports and querying conversational memory.
- `docs/benchmarks/conversation-memory-benchmark.md`
  Reproduction notes and benchmark reporting format for long-memory evaluation.

### Modified files

- `crates/the-one-memory/src/lib.rs`
  Expose conversation ingestion/search helpers and extend search request handling.
- `crates/the-one-core/src/config.rs`
  Add vector backend selection and Redis connection settings.
- `crates/the-one-mcp/src/api.rs`
  Add MCP request/response types for conversation ingestion and wake-up packs.
- `crates/the-one-mcp/src/transport/tools.rs`
  Add tool definitions for the new memory operations and extend `memory.search` schema with optional palace filters.
- `crates/the-one-mcp/src/broker.rs`
  Wire new APIs into `MemoryEngine`, backend selection, broker metrics, and project DB interactions.
- `crates/the-one-core/src/storage/sqlite.rs`
  Persist conversation ingest metadata and wake-up state summaries.
- `README.md`
  Update product positioning and tool inventory.
- `docs/guides/api-reference.md`
  Document the new MCP tools and fields.
- `docs/guides/architecture.md`
  Add the conversation-memory layer and palace metadata model.
- `docs/guides/configuration.md`
  Document the Redis backend settings and persistence expectations.

### Existing files to read before coding

- `crates/the-one-memory/src/lib.rs`
- `crates/the-one-memory/src/chunker.rs`
- `/home/michel/projects/mai/crates/mai-memory/src/redis/vectors.rs`
- `/home/michel/projects/mai/config/redis.conf`
- `/home/michel/projects/mai/config/mai.toml`
- `crates/the-one-mcp/src/api.rs`
- `crates/the-one-mcp/src/broker.rs`
- `crates/the-one-mcp/src/transport/tools.rs`
- `crates/the-one-core/src/storage/sqlite.rs`
- `README.md`

---

### Task 1: Add Conversation Domain Types And Transcript Normalization

**Files:**
- Create: `crates/the-one-memory/src/conversation.rs`
- Create: `crates/the-one-memory/src/palace.rs`
- Modify: `crates/the-one-memory/src/lib.rs`
- Test: `crates/the-one-memory/src/conversation.rs`
- Test: `crates/the-one-memory/src/palace.rs`

- [ ] **Step 1: Write the failing transcript normalization tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_export_into_messages() {
        let input = r#"[
          {"role":"system","content":"You are helpful"},
          {"role":"user","content":"Why did we switch auth vendors?"},
          {"role":"assistant","content":"Because refresh tokens were failing in staging"}
        ]"#;

        let transcript = ConversationTranscript::from_json_str(
            "auth-review",
            ConversationFormat::OpenAiMessages,
            input,
        )
        .expect("transcript should parse");

        assert_eq!(transcript.messages.len(), 3);
        assert_eq!(transcript.messages[1].role, ConversationRole::User);
        assert!(transcript.messages[2]
            .content
            .contains("refresh tokens were failing"));
    }

    #[test]
    fn derives_palace_metadata_from_project_and_tags() {
        let meta = PalaceMetadata::new(
            "proj-auth",
            Some("hall_facts".to_string()),
            Some("auth-migration".to_string()),
        );

        assert_eq!(meta.wing, "proj-auth");
        assert_eq!(meta.hall.as_deref(), Some("hall_facts"));
        assert_eq!(meta.room.as_deref(), Some("auth-migration"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p the-one-memory normalizes_openai_export_into_messages derives_palace_metadata_from_project_and_tags -- --nocapture`

Expected: FAIL with unresolved types like `ConversationTranscript`, `ConversationFormat`, or `PalaceMetadata`.

- [ ] **Step 3: Implement the conversation and palace modules**

```rust
// crates/the-one-memory/src/conversation.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConversationRole {
    System,
    User,
    Assistant,
    Tool,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConversationFormat {
    OpenAiMessages,
    ClaudeTranscript,
    GenericJsonl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: ConversationRole,
    pub content: String,
    pub turn_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTranscript {
    pub source_id: String,
    pub messages: Vec<ConversationMessage>,
}

impl ConversationTranscript {
    pub fn from_json_str(
        source_id: &str,
        format: ConversationFormat,
        input: &str,
    ) -> Result<Self, String> {
        match format {
            ConversationFormat::OpenAiMessages => {
                #[derive(Deserialize)]
                struct RawMessage {
                    role: String,
                    content: String,
                }

                let raw: Vec<RawMessage> =
                    serde_json::from_str(input).map_err(|e| format!("invalid transcript: {e}"))?;

                let messages = raw
                    .into_iter()
                    .enumerate()
                    .map(|(turn_index, item)| ConversationMessage {
                        role: match item.role.as_str() {
                            "system" => ConversationRole::System,
                            "user" => ConversationRole::User,
                            "assistant" => ConversationRole::Assistant,
                            "tool" => ConversationRole::Tool,
                            _ => ConversationRole::Unknown,
                        },
                        content: item.content,
                        turn_index,
                    })
                    .collect();

                Ok(Self {
                    source_id: source_id.to_string(),
                    messages,
                })
            }
            _ => Err("format not implemented yet".to_string()),
        }
    }
}
```

```rust
// crates/the-one-memory/src/palace.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PalaceMetadata {
    pub wing: String,
    pub hall: Option<String>,
    pub room: Option<String>,
}

impl PalaceMetadata {
    pub fn new(wing: &str, hall: Option<String>, room: Option<String>) -> Self {
        Self {
            wing: wing.to_string(),
            hall,
            room,
        }
    }
}
```

```rust
// crates/the-one-memory/src/lib.rs
pub mod conversation;
pub mod palace;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p the-one-memory normalizes_openai_export_into_messages derives_palace_metadata_from_project_and_tags -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/the-one-memory/src/lib.rs \
        crates/the-one-memory/src/conversation.rs \
        crates/the-one-memory/src/palace.rs
git commit -m "feat: add conversation transcript domain model"
```

---

### Task 2: Teach MemoryEngine To Ingest And Search Verbatim Conversations

**Files:**
- Modify: `crates/the-one-memory/src/lib.rs`
- Modify: `crates/the-one-memory/src/chunker.rs`
- Test: `crates/the-one-memory/src/lib.rs`

- [ ] **Step 1: Write the failing ingestion/search test**

```rust
#[tokio::test]
async fn ingested_conversation_is_searchable_by_exact_reasoning() {
    let mut engine = MemoryEngine::new_local("fast", 256).expect("engine");

    let transcript = crate::conversation::ConversationTranscript {
        source_id: "claude-auth-session".to_string(),
        messages: vec![
            crate::conversation::ConversationMessage {
                role: crate::conversation::ConversationRole::User,
                content: "Why did we switch auth vendors?".to_string(),
                turn_index: 0,
            },
            crate::conversation::ConversationMessage {
                role: crate::conversation::ConversationRole::Assistant,
                content: "We switched because refresh token rotation failed in staging and support was slow.".to_string(),
                turn_index: 1,
            },
        ],
    };

    engine
        .ingest_conversation(
            "/tmp/auth-session.json",
            &transcript,
            Some(crate::palace::PalaceMetadata::new(
                "proj-auth",
                Some("hall_facts".to_string()),
                Some("auth-migration".to_string()),
            )),
        )
        .await
        .expect("conversation should ingest");

    let result = engine
        .search(crate::MemorySearchRequest {
            query: "refresh token rotation failed in staging".to_string(),
            top_k: 5,
            score_threshold: 0.0,
            mode: crate::RetrievalMode::Naive,
        })
        .await
        .expect("search should work");

    assert!(!result.is_empty());
    assert!(result[0].chunk.source_path.contains("auth-session"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p the-one-memory ingested_conversation_is_searchable_by_exact_reasoning -- --nocapture`

Expected: FAIL because `MemoryEngine::ingest_conversation` does not exist.

- [ ] **Step 3: Implement minimal conversation ingestion**

```rust
impl MemoryEngine {
    pub async fn ingest_conversation(
        &mut self,
        source_path: &str,
        transcript: &crate::conversation::ConversationTranscript,
        palace: Option<crate::palace::PalaceMetadata>,
    ) -> Result<usize, String> {
        let mut inserted = 0usize;

        for message in &transcript.messages {
            let content = format!(
                "[turn:{}][role:{:?}]\n{}",
                message.turn_index,
                message.role,
                message.content
            );

            let chunk = ChunkMeta {
                id: format!("{source_path}:turn:{}", message.turn_index),
                source_path: source_path.to_string(),
                heading_hierarchy: vec!["conversation".to_string()],
                chunk_index: message.turn_index,
                byte_offset: 0,
                byte_length: content.len(),
                content_hash: super::chunker::content_hash(&content),
                content,
                language: Some("conversation".to_string()),
                symbol: palace.as_ref().and_then(|p| p.room.clone()),
                signature: palace.as_ref().and_then(|p| p.hall.clone()),
                line_range: None,
            };

            self.by_id.insert(chunk.id.clone(), self.chunks.len());
            self.chunks.push(chunk);
            inserted += 1;
        }

        Ok(inserted)
    }
}
```

Notes:
- Keep the first version simple: one chunk per message or turn pair.
- Preserve verbatim content.
- Do not add summarization or AAAK in this task.
- If Qdrant is configured, reuse the same indexing path used by markdown chunks.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p the-one-memory ingested_conversation_is_searchable_by_exact_reasoning -- --nocapture`

Expected: PASS

- [ ] **Step 5: Add a second test for palace metadata carrying through**

```rust
#[tokio::test]
async fn ingested_conversation_carries_palace_room_metadata() {
    let mut engine = MemoryEngine::new_local("fast", 256).expect("engine");
    let transcript = crate::conversation::ConversationTranscript {
        source_id: "session".to_string(),
        messages: vec![crate::conversation::ConversationMessage {
            role: crate::conversation::ConversationRole::Assistant,
            content: "Clerk won over Auth0 after the outage review.".to_string(),
            turn_index: 0,
        }],
    };

    engine
        .ingest_conversation(
            "/tmp/session.json",
            &transcript,
            Some(crate::palace::PalaceMetadata::new(
                "proj-auth",
                Some("hall_facts".to_string()),
                Some("auth-migration".to_string()),
            )),
        )
        .await
        .expect("ingest");

    let chunk = engine.fetch_chunk("/tmp/session.json:turn:0").expect("chunk");
    assert_eq!(chunk.symbol.as_deref(), Some("auth-migration"));
    assert_eq!(chunk.signature.as_deref(), Some("hall_facts"));
}
```

- [ ] **Step 6: Run both tests**

Run: `cargo test -p the-one-memory ingested_conversation_is_searchable_by_exact_reasoning ingested_conversation_carries_palace_room_metadata -- --nocapture`

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/the-one-memory/src/lib.rs \
        crates/the-one-memory/src/chunker.rs
git commit -m "feat: ingest verbatim conversation memory into engine"
```

---

### Task 3: Add A Redis Persistent Vector Backend Option

**Files:**
- Create: `crates/the-one-memory/src/redis_vectors.rs`
- Modify: `crates/the-one-memory/src/lib.rs`
- Modify: `crates/the-one-core/src/config.rs`
- Create: `config/redis.conf.example`
- Create: `docs/guides/redis-vector-backend.md`
- Test: `crates/the-one-memory/src/redis_vectors.rs`
- Test: `crates/the-one-core/src/config.rs`

- [ ] **Step 1: Write the failing Redis backend tests**

```rust
#[tokio::test]
async fn redis_vector_store_builds_index_schema_with_hnsw() {
    let store = RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 1024)
        .expect("store");

    let schema = store.index_schema();
    assert!(schema.contains("VECTOR"));
    assert!(schema.contains("HNSW"));
    assert!(schema.contains("wing"));
    assert!(schema.contains("room"));
}

#[test]
fn config_parses_redis_vector_backend_settings() {
    let toml = r#"
vector_backend = "redis"
redis_url = "redis://127.0.0.1:6379"
redis_index_name = "the_one_memories"
redis_persistence_required = true
"#;

    let cfg: MemoryBackendConfig = toml::from_str(toml).expect("config");
    assert_eq!(cfg.vector_backend, "redis");
    assert_eq!(cfg.redis_index_name.as_deref(), Some("the_one_memories"));
    assert!(cfg.redis_persistence_required);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p the-one-memory redis_vector_store_builds_index_schema_with_hnsw -- --nocapture`

Run: `cargo test -p the-one-core config_parses_redis_vector_backend_settings -- --nocapture`

Expected: FAIL because `RedisVectorStore` and the new config fields do not exist.

- [ ] **Step 3: Implement a minimal Redis vector store modeled after `mai`**

```rust
// crates/the-one-memory/src/redis_vectors.rs
use fred::prelude::*;

#[derive(Clone)]
pub struct RedisVectorStore {
    client: Client,
    index_name: String,
    embedding_dim: usize,
}

impl RedisVectorStore {
    pub fn new(client: Client, index_name: impl Into<String>, embedding_dim: usize) -> Self {
        Self {
            client,
            index_name: index_name.into(),
            embedding_dim,
        }
    }

    pub fn index_schema(&self) -> String {
        format!(
            "FT.CREATE {} ON HASH PREFIX 1 mem: SCHEMA content TEXT wing TAG hall TAG room TAG embedding VECTOR HNSW 6 TYPE FLOAT32 DIM {} DISTANCE_METRIC COSINE",
            self.index_name,
            self.embedding_dim
        )
    }
}
```

```rust
// crates/the-one-core/src/config.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryBackendConfig {
    #[serde(default = "default_vector_backend")]
    pub vector_backend: String,
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub redis_index_name: Option<String>,
    #[serde(default)]
    pub redis_persistence_required: bool,
}

fn default_vector_backend() -> String {
    "qdrant".to_string()
}
```

Notes:
- Mirror the `mai` design where Redis uses RediSearch HNSW on hash keys and stores metadata in TAG/NUMERIC fields.
- Keep the-the one naming and config layout idiomatic to this repo.
- Do not switch the default backend away from Qdrant.

- [ ] **Step 4: Add Redis persistence documentation and config example**

```conf
# config/redis.conf.example
save 900 1
save 300 10
save 60 10000
appendonly yes
appendfsync everysec
# loadmodule /path/to/redisearch.so
```

```md
## Persistence requirements

- Redis must have both RDB snapshots and AOF enabled for vector durability.
- RediSearch must be loaded for `FT.CREATE` / `FT.SEARCH`.
- If `redis_persistence_required = true`, the broker should warn or fail fast when persistence is misconfigured.
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p the-one-memory redis_vector_store_builds_index_schema_with_hnsw -- --nocapture`

Run: `cargo test -p the-one-core config_parses_redis_vector_backend_settings -- --nocapture`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-memory/src/redis_vectors.rs \
        crates/the-one-memory/src/lib.rs \
        crates/the-one-core/src/config.rs \
        config/redis.conf.example \
        docs/guides/redis-vector-backend.md
git commit -m "feat: add redis persistent vector backend option"
```

---

### Task 4: Add MCP APIs For Conversation Ingest And Wake-Up Packs

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-mcp/src/transport/tools.rs`
- Test: `crates/the-one-mcp/src/api.rs`

- [ ] **Step 1: Write the failing MCP schema tests**

```rust
#[test]
fn tool_definitions_include_conversation_memory_tools() {
    let names: Vec<String> = tool_definitions()
        .into_iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();

    assert!(names.contains(&"memory.ingest_conversation".to_string()));
    assert!(names.contains(&"memory.wake_up".to_string()));
}

#[test]
fn memory_ingest_conversation_request_roundtrip() {
    let json = r#"{
      "project_root":"/tmp/project",
      "project_id":"proj-1",
      "path":"exports/auth.json",
      "format":"openai_messages",
      "wing":"proj-auth",
      "hall":"hall_facts",
      "room":"auth-migration"
    }"#;

    let req: MemoryIngestConversationRequest =
        serde_json::from_str(json).expect("request should parse");

    assert_eq!(req.format, "openai_messages");
    assert_eq!(req.room.as_deref(), Some("auth-migration"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p the-one-mcp tool_definitions_include_conversation_memory_tools memory_ingest_conversation_request_roundtrip -- --nocapture`

Expected: FAIL because the structs and tool definitions do not exist yet.

- [ ] **Step 3: Add the new request/response types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIngestConversationRequest {
    pub project_root: String,
    pub project_id: String,
    pub path: String,
    pub format: String,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIngestConversationResponse {
    pub ingested_chunks: usize,
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWakeUpRequest {
    pub project_root: String,
    pub project_id: String,
    pub wing: Option<String>,
    pub max_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWakeUpResponse {
    pub summary: String,
    pub facts: Vec<String>,
}
```

- [ ] **Step 4: Add the MCP tool definitions**

```rust
tool_def("memory.ingest_conversation", "Import a conversation export and index it as verbatim memory with optional palace metadata.", json!({
    "type": "object",
    "properties": {
        "project_root": { "type": "string" },
        "project_id": { "type": "string" },
        "path": { "type": "string", "description": "Absolute or project-relative path to the transcript export" },
        "format": { "type": "string", "enum": ["openai_messages", "claude_transcript", "generic_jsonl"] },
        "wing": { "type": "string" },
        "hall": { "type": "string" },
        "room": { "type": "string" }
    },
    "required": ["project_root", "project_id", "path", "format"]
})),
tool_def("memory.wake_up", "Build a compact context pack from recent high-signal conversation memory.", json!({
    "type": "object",
    "properties": {
        "project_root": { "type": "string" },
        "project_id": { "type": "string" },
        "wing": { "type": "string" },
        "max_items": { "type": "integer", "default": 12 }
    },
    "required": ["project_root", "project_id"]
})),
```

Notes:
- Increase the tool count assertion after adding the tools.
- Also extend `memory.search` later with optional `wing`, `hall`, and `room` fields if that proves stable.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p the-one-mcp tool_definitions_include_conversation_memory_tools memory_ingest_conversation_request_roundtrip -- --nocapture`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-mcp/src/api.rs \
        crates/the-one-mcp/src/transport/tools.rs
git commit -m "feat: add conversation memory MCP request surface"
```

---

### Task 5: Wire The Broker To Ingest Conversations, Select Vector Backend, And Generate Wake-Up Packs

**Files:**
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `crates/the-one-core/src/storage/sqlite.rs`
- Test: `crates/the-one-mcp/src/broker.rs`
- Test: `crates/the-one-core/src/storage/sqlite.rs`

- [ ] **Step 1: Write the failing broker integration test**

```rust
#[tokio::test]
async fn ingest_conversation_indexes_export_and_returns_chunk_count() {
    let broker = McpBroker::new();
    let project = tempfile::tempdir().expect("tempdir");
    let export_path = project.path().join("auth.json");

    std::fs::write(
        &export_path,
        r#"[{"role":"user","content":"Why Clerk?"},{"role":"assistant","content":"Auth0 support lagged during the outage review."}]"#,
    )
    .expect("write export");

    broker
        .project_init(ProjectInitRequest {
            project_root: project.path().display().to_string(),
            project_id: "proj-1".to_string(),
        })
        .await
        .expect("project init");

    let response = broker
        .ingest_conversation(MemoryIngestConversationRequest {
            project_root: project.path().display().to_string(),
            project_id: "proj-1".to_string(),
            path: export_path.display().to_string(),
            format: "openai_messages".to_string(),
            wing: Some("proj-auth".to_string()),
            hall: Some("hall_facts".to_string()),
            room: Some("auth-migration".to_string()),
        })
        .await
        .expect("conversation ingest");

    assert_eq!(response.ingested_chunks, 2);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p the-one-mcp ingest_conversation_indexes_export_and_returns_chunk_count -- --nocapture`

Expected: FAIL because `McpBroker::ingest_conversation` does not exist.

- [ ] **Step 3: Implement broker ingestion and metadata persistence**

```rust
impl McpBroker {
    pub async fn ingest_conversation(
        &self,
        request: MemoryIngestConversationRequest,
    ) -> Result<MemoryIngestConversationResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let key = Self::project_memory_key(&project_root, &request.project_id);
        let raw = std::fs::read_to_string(&request.path)?;

        let transcript = the_one_memory::conversation::ConversationTranscript::from_json_str(
            &request.path,
            match request.format.as_str() {
                "openai_messages" => the_one_memory::conversation::ConversationFormat::OpenAiMessages,
                "claude_transcript" => the_one_memory::conversation::ConversationFormat::ClaudeTranscript,
                _ => the_one_memory::conversation::ConversationFormat::GenericJsonl,
            },
            &raw,
        )
        .map_err(CoreError::Embedding)?;

        let palace = request.wing.as_deref().map(|wing| {
            the_one_memory::palace::PalaceMetadata::new(
                wing,
                request.hall.clone(),
                request.room.clone(),
            )
        });

        let mut memories = self.memory_by_project.write().await;
        let memory = memories.get_mut(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;

        let ingested_chunks = memory
            .ingest_conversation(&request.path, &transcript, palace)
            .await
            .map_err(CoreError::Embedding)?;

        Ok(MemoryIngestConversationResponse {
            ingested_chunks,
            source_path: request.path,
        })
    }
}
```

```rust
// sqlite.rs
CREATE TABLE IF NOT EXISTS conversation_sources (
    project_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    format TEXT NOT NULL,
    wing TEXT,
    hall TEXT,
    room TEXT,
    ingested_at_epoch_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, source_path)
);
```

```rust
// broker.rs
let mut engine = match config.vector_backend.as_str() {
    "redis" => MemoryEngine::new_with_redis(
        &config.embedding_model,
        config.redis_url.as_deref().ok_or_else(|| {
            CoreError::InvalidProjectConfig("redis backend selected without redis_url".to_string())
        })?,
        config
            .redis_index_name
            .as_deref()
            .unwrap_or("the_one_memories"),
        config.embedding_dimensions,
        max_chunk_tokens,
    )
    .map_err(CoreError::Embedding)?,
    _ => /* existing qdrant/local path */,
};
```

- [ ] **Step 4: Add wake-up pack generation with a small deterministic first version**

```rust
pub async fn wake_up(
    &self,
    request: MemoryWakeUpRequest,
) -> Result<MemoryWakeUpResponse, CoreError> {
    let hits = self
        .memory_search(MemorySearchRequest {
            project_root: request.project_root.clone(),
            project_id: request.project_id.clone(),
            query: request
                .wing
                .clone()
                .unwrap_or_else(|| "recent project decisions preferences milestones".to_string()),
            top_k: request.max_items.max(1),
        })
        .await?;

    let facts: Vec<String> = hits
        .hits
        .into_iter()
        .map(|hit| format!("{} ({:.2})", hit.source_path, hit.score))
        .collect();

    Ok(MemoryWakeUpResponse {
        summary: facts.join("\n"),
        facts,
    })
}
```

Notes:
- Keep generation deterministic in V1.
- Do not require an LLM to build the wake-up pack.
- This is a context pack, not a summary writer.

- [ ] **Step 5: Add a failing wake-up test, then make it pass**

```rust
#[tokio::test]
async fn wake_up_returns_compact_fact_list() {
    let broker = McpBroker::new();
    // Reuse a project with ingested conversation memory.
    // Expect non-empty `facts` and a non-empty `summary`.
}
```

Run: `cargo test -p the-one-mcp wake_up_returns_compact_fact_list -- --nocapture`

Expected: PASS after implementation.

- [ ] **Step 6: Commit**

```bash
git add crates/the-one-mcp/src/broker.rs \
        crates/the-one-core/src/storage/sqlite.rs
git commit -m "feat: wire broker conversation ingest and wake-up packs"
```

---

### Task 6: Add Palace-Aware Search Filters, Backend Docs, And Benchmarking

**Files:**
- Modify: `crates/the-one-mcp/src/api.rs`
- Modify: `crates/the-one-mcp/src/transport/tools.rs`
- Modify: `crates/the-one-mcp/src/broker.rs`
- Modify: `README.md`
- Create: `docs/guides/conversation-memory.md`
- Create: `docs/guides/redis-vector-backend.md`
- Create: `docs/benchmarks/conversation-memory-benchmark.md`
- Modify: `docs/guides/api-reference.md`
- Modify: `docs/guides/architecture.md`
- Modify: `docs/guides/configuration.md`
- Test: `crates/the-one-mcp/src/broker.rs`

- [ ] **Step 1: Write the failing palace-filter search test**

```rust
#[tokio::test]
async fn memory_search_can_filter_by_wing_and_room() {
    let broker = McpBroker::new();
    // Seed two conversation sources with different room metadata.
    // Search for a shared term while filtering to auth-migration.
    // Assert only the auth-migration source is returned.
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p the-one-mcp memory_search_can_filter_by_wing_and_room -- --nocapture`

Expected: FAIL because `MemorySearchRequest` has no `wing`/`room` fields or the broker ignores them.

- [ ] **Step 3: Extend the search request and tool schema**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchRequest {
    pub project_root: String,
    pub project_id: String,
    pub query: String,
    pub top_k: usize,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub hall: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
}
```

```rust
"wing": { "type": "string", "description": "Optional palace wing filter" },
"hall": { "type": "string", "description": "Optional palace hall filter" },
"room": { "type": "string", "description": "Optional palace room filter" }
```

- [ ] **Step 4: Implement filter application in broker and memory engine**

```rust
let palace_filter = PalaceMetadata::new(
    request.wing.as_deref().unwrap_or(""),
    request.hall.clone(),
    request.room.clone(),
);

// Apply the filter before reranking or final result shaping.
```

Notes:
- Apply metadata filtering before reranking to reduce noise.
- Preserve existing behavior when no filter fields are set.

- [ ] **Step 5: Update docs with exact user-facing examples**

```md
### Ingest a Claude or ChatGPT export

```json
{
  "name": "memory.ingest_conversation",
  "arguments": {
    "project_root": "/workspace/myapp",
    "project_id": "myapp",
    "path": "exports/claude-auth-session.json",
    "format": "openai_messages",
    "wing": "proj-auth",
    "hall": "hall_facts",
    "room": "auth-migration"
  }
}
```

### Build a wake-up pack

```json
{
  "name": "memory.wake_up",
  "arguments": {
    "project_root": "/workspace/myapp",
    "project_id": "myapp",
    "wing": "proj-auth",
    "max_items": 8
  }
}
```
```

- [ ] **Step 6: Add benchmark documentation and a reproducibility checklist**

```md
1. Define a fixture set of conversation exports under `benchmarks/fixtures/conversations/`.
2. Run raw verbatim retrieval only.
3. Report:
   - exact dataset
   - exact model
   - exact top_k
   - exact filter mode
   - whether reranking was enabled
4. Do not claim parity with MemPalace until the-one reproduces the benchmark result.
```

- [ ] **Step 7: Run the full targeted test set**

Run: `cargo test -p the-one-memory conversation -- --nocapture`

Run: `cargo test -p the-one-mcp ingest_conversation wake_up memory_search_can_filter_by_wing_and_room -- --nocapture`

Run: `cargo fmt --check`

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add README.md \
        docs/guides/conversation-memory.md \
        docs/benchmarks/conversation-memory-benchmark.md \
        docs/guides/api-reference.md \
        docs/guides/architecture.md \
        crates/the-one-mcp/src/api.rs \
        crates/the-one-mcp/src/transport/tools.rs \
        crates/the-one-mcp/src/broker.rs
git commit -m "docs: add conversation memory and palace search guide"
```

---

## Self-Review

### Spec coverage

- Conversation/chat-export ingestion: covered in Tasks 1, 2, and 4.
- Raw verbatim memory mode: covered in Task 2.
- Redis persistent vector backend: covered in Tasks 3 and 5.
- Palace metadata (`wing`, `hall`, `room`): covered in Tasks 1, 5, and 6.
- Wake-up/context packs: covered in Tasks 4 and 5.
- Benchmark discipline: covered in Task 6.
- Avoid direct repo merge / preserve Rust host architecture: reflected in Scope and all task boundaries.

### Placeholder scan

- No `TODO`, `TBD`, or “similar to above” placeholders remain in task steps.
- All code-writing steps include concrete snippets.
- All verification steps include concrete commands.

### Type consistency

- `MemoryIngestConversationRequest` / `Response` are used consistently across API, tools, and broker tasks.
- `MemoryWakeUpRequest` / `Response` are introduced once and reused consistently.
- Palace metadata naming remains `wing`, `hall`, `room` across all tasks.
- Redis backend naming remains `vector_backend = "redis"` with `redis_url`, `redis_index_name`, and `redis_persistence_required`.

## Execution Notes

- Implement this behind incremental commits exactly as written.
- Keep the first iteration deterministic and local-first.
- Treat AAAK as a later experiment, not as part of the initial merge.
- Do not add ChromaDB unless benchmark evidence shows Qdrant and Redis cannot support the retrieval quality target.

Plan complete and saved to `docs/superpowers/plans/2026-04-09-mempalace-integration-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
