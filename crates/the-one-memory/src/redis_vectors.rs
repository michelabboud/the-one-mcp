use std::sync::Arc;

use the_one_redis::search::{
    CreateOptions, DistanceMetric, Query, SchemaField, SearchReply, VectorAlgorithm,
};
use the_one_redis::{PoolConfig, RedisPool};
use tokio::sync::OnceCell;

#[derive(Debug)]
pub struct RedisVectorStore {
    pool: RedisPool,
    index_name: String,
    embedding_dim: usize,
    started: Arc<OnceCell<()>>,
}

impl Clone for RedisVectorStore {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            index_name: self.index_name.clone(),
            embedding_dim: self.embedding_dim,
            // SHARE the OnceCell across clones rather than allocating a
            // fresh empty one. Arc::clone is a refcount bump; Arc::new
            // would defeat the whole point of the guard and force every
            // clone to re-run startup() (FT.CREATE, schema migration).
            // Reached via the combined-Redis shared-pool cache in the
            // broker, which clones the store per call site.
            started: Arc::clone(&self.started),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisChunkRecord {
    pub chunk_id: String,
    pub source_path: String,
    pub heading: String,
    pub chunk_index: usize,
    pub content: String,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisSearchResult {
    pub chunk_id: String,
    pub score: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedisPersistenceInfo {
    pub aof_enabled: bool,
    pub rdb_bgsave_in_progress: bool,
    pub rdb_last_bgsave_status_ok: bool,
    pub rdb_last_save_time: Option<i64>,
}

impl RedisPersistenceInfo {
    pub fn is_persistent(&self) -> bool {
        self.aof_enabled
            || self.rdb_bgsave_in_progress
            || (self.rdb_last_bgsave_status_ok && self.rdb_last_save_time.unwrap_or_default() > 0)
    }
}

impl RedisVectorStore {
    pub fn new(
        pool: RedisPool,
        index_name: impl Into<String>,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        let index_name = validate_index_name(index_name.into())?;
        validate_embedding_dim(embedding_dim)?;

        Ok(Self {
            pool,
            index_name,
            embedding_dim,
            started: Arc::new(OnceCell::new()),
        })
    }

    pub async fn from_url(
        redis_url: &str,
        index_name: impl Into<String>,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        if redis_url.trim().is_empty() {
            return Err("redis_url must not be empty".to_string());
        }

        let pool = RedisPool::new(PoolConfig::from_url(redis_url))
            .await
            .map_err(|e| format!("invalid redis_url: {e}"))?;

        Self::new(pool, index_name, embedding_dim)
    }

    pub async fn new_for_test(
        redis_url: &str,
        index_name: &str,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        Self::from_url(redis_url, index_name, embedding_dim).await
    }

    pub fn pool(&self) -> &RedisPool {
        &self.pool
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }

    pub fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    pub fn chunk_key(&self, chunk_id: &str) -> String {
        format!("mem:{}:chunk:{}", self.index_name, chunk_id)
    }

    pub fn source_index_key(&self, source_path: &str) -> String {
        format!(
            "mem:{}:src:{}",
            self.index_name,
            source_path_hash(source_path)
        )
    }

    pub fn chunk_key_prefix(&self) -> String {
        format!("mem:{}:chunk:", self.index_name)
    }

    pub fn index_schema(&self) -> String {
        format!(
            "FT.CREATE {index} ON HASH PREFIX 1 {prefix} SCHEMA \
             chunk_id TEXT \
             source_path TAG \
             heading TEXT \
             chunk_index NUMERIC \
             content TEXT \
             embedding VECTOR HNSW 6 TYPE FLOAT32 DIM {dim} DISTANCE_METRIC COSINE",
            index = self.index_name,
            prefix = self.chunk_key_prefix(),
            dim = self.embedding_dim
        )
    }

    pub fn parse_persistence_info(text: &str) -> Result<RedisPersistenceInfo, String> {
        let mut info = RedisPersistenceInfo::default();

        for line in text.lines() {
            let Some((raw_key, raw_value)) = line.split_once(':') else {
                continue;
            };

            let key = raw_key.trim();
            let value = raw_value.trim();

            match key {
                "aof_enabled" => info.aof_enabled = parse_boolish(value)?,
                "rdb_bgsave_in_progress" => info.rdb_bgsave_in_progress = parse_boolish(value)?,
                "rdb_last_bgsave_status" => {
                    info.rdb_last_bgsave_status_ok = value.eq_ignore_ascii_case("ok")
                }
                "rdb_last_save_time" => {
                    info.rdb_last_save_time = value
                        .parse::<i64>()
                        .map(Some)
                        .map_err(|e| format!("invalid rdb_last_save_time value: {e}"))?;
                }
                _ => {}
            }
        }

        Ok(info)
    }

    pub async fn verify_persistence(&self) -> Result<(), String> {
        self.ensure_started().await?;

        let info: String = self
            .pool
            .raw_cmd(&["INFO", "persistence"])
            .await
            .map_err(|e| format!("failed to read Redis persistence info: {e}"))?;

        let parsed = Self::parse_persistence_info(&info)?;
        if parsed.is_persistent() {
            return Ok(());
        }

        Err("redis persistence is required but neither AOF nor RDB appears enabled".to_string())
    }

    pub async fn ensure_index(&self) -> Result<(), String> {
        self.ensure_started().await?;

        let schema = vec![
            SchemaField::Text {
                name: "chunk_id".into(),
            },
            SchemaField::Tag {
                name: "source_path".into(),
                separator: None,
            },
            SchemaField::Text {
                name: "heading".into(),
            },
            SchemaField::Numeric {
                name: "chunk_index".into(),
                sortable: false,
            },
            SchemaField::Text {
                name: "content".into(),
            },
            SchemaField::Vector {
                name: "embedding".into(),
                algorithm: VectorAlgorithm::Hnsw,
                dim: self.embedding_dim,
                distance_metric: DistanceMetric::Cosine,
                initial_cap: None,
            },
        ];

        let options = CreateOptions {
            on_json: false,
            prefixes: vec![self.chunk_key_prefix()],
        };

        match self
            .pool
            .search()
            .ft_create(&self.index_name, &options, &schema)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let message = err.to_string();
                if message.contains("Index already exists") || message.contains("already exists") {
                    Ok(())
                } else {
                    Err(format!("failed to create Redis index: {message}"))
                }
            }
        }
    }

    pub async fn upsert_chunks(&self, records: &[RedisChunkRecord]) -> Result<usize, String> {
        if records.is_empty() {
            return Ok(0);
        }

        self.ensure_index().await?;

        let mut written = 0usize;
        for record in records {
            if record.vector.len() != self.embedding_dim {
                return Err(format!(
                    "chunk {} embedding dimension mismatch: expected {}, got {}",
                    record.chunk_id,
                    self.embedding_dim,
                    record.vector.len()
                ));
            }

            let key = self.chunk_key(&record.chunk_id);
            let embedding_bytes = vector_to_bytes(&record.vector);
            let chunk_index_str = record.chunk_index.to_string();
            let fields: [(&str, &[u8]); 6] = [
                ("chunk_id", record.chunk_id.as_bytes()),
                ("source_path", record.source_path.as_bytes()),
                ("heading", record.heading.as_bytes()),
                ("chunk_index", chunk_index_str.as_bytes()),
                ("content", record.content.as_bytes()),
                ("embedding", embedding_bytes.as_slice()),
            ];

            self.pool
                .hashes()
                .hset_multi(&key, &fields)
                .await
                .map_err(|e| format!("redis hset failed for {key}: {e}"))?;

            let source_key = self.source_index_key(&record.source_path);
            self.pool
                .sets()
                .sadd(&source_key, key.as_str())
                .await
                .map_err(|e| format!("redis sadd failed for {source_key}: {e}"))?;
            written += 1;
        }

        Ok(written)
    }

    pub async fn delete_by_source_path(&self, source_path: &str) -> Result<usize, String> {
        self.ensure_started().await?;

        let source_key = self.source_index_key(source_path);
        let members: Vec<String> = self
            .pool
            .sets()
            .smembers(&source_key)
            .await
            .map_err(|e| format!("redis smembers failed for {source_key}: {e}"))?;

        if members.is_empty() {
            return Ok(0);
        }

        let mut deleted_total: u64 = 0;
        for member in &members {
            let del = self
                .pool
                .keys()
                .del(member)
                .await
                .map_err(|e| format!("redis del failed: {e}"))?;
            deleted_total += del;
        }

        let _ = self
            .pool
            .keys()
            .del(&source_key)
            .await
            .map_err(|e| format!("redis del failed for source index: {e}"))?;

        Ok(deleted_total as usize)
    }

    pub async fn search_chunks(
        &self,
        query_vector: &[f32],
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<RedisSearchResult>, String> {
        if top_k == 0 {
            return Ok(Vec::new());
        }

        self.ensure_index().await?;

        if query_vector.len() != self.embedding_dim {
            return Err(format!(
                "query embedding dimension mismatch: expected {}, got {}",
                self.embedding_dim,
                query_vector.len()
            ));
        }

        let raw_query = format!("*=>[KNN {} @embedding $BLOB AS score]", top_k);
        let query_blob = vector_to_bytes(query_vector);
        let query = Query::new(raw_query)
            .return_fields(["score"])
            .param("BLOB", query_blob)
            .limit(0, top_k)
            .dialect(2);

        let reply = self
            .pool
            .search()
            .ft_search(&self.index_name, &query)
            .await
            .map_err(|e| format!("redis vector search failed: {e}"))?;

        parse_chunk_reply(&reply, &self.chunk_key_prefix(), score_threshold)
    }

    // ── Entity / relation support (Phase 7) ─────────────────────────

    fn entity_key(&self, entity_id: &str) -> String {
        format!("mem:{}:entity:{}", self.index_name, entity_id)
    }

    fn entity_key_prefix(&self) -> String {
        format!("mem:{}:entity:", self.index_name)
    }

    fn entity_index_name(&self) -> String {
        format!("{}_entities", self.index_name)
    }

    fn relation_key(&self, relation_id: &str) -> String {
        format!("mem:{}:relation:{}", self.index_name, relation_id)
    }

    fn relation_key_prefix(&self) -> String {
        format!("mem:{}:relation:", self.index_name)
    }

    fn relation_index_name(&self) -> String {
        format!("{}_relations", self.index_name)
    }

    async fn ensure_entity_index(&self) -> Result<(), String> {
        self.ensure_started().await?;

        let idx_name = self.entity_index_name();
        let prefix = self.entity_key_prefix();

        let schema = vec![
            SchemaField::Text {
                name: "name".into(),
            },
            SchemaField::Tag {
                name: "entity_type".into(),
                separator: None,
            },
            SchemaField::Text {
                name: "description".into(),
            },
            SchemaField::Vector {
                name: "embedding".into(),
                algorithm: VectorAlgorithm::Hnsw,
                dim: self.embedding_dim,
                distance_metric: DistanceMetric::Cosine,
                initial_cap: None,
            },
        ];

        let options = CreateOptions {
            on_json: false,
            prefixes: vec![prefix],
        };

        match self
            .pool
            .search()
            .ft_create(&idx_name, &options, &schema)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("already exists") {
                    Ok(())
                } else {
                    Err(format!("entity index create: {msg}"))
                }
            }
        }
    }

    async fn ensure_relation_index(&self) -> Result<(), String> {
        self.ensure_started().await?;

        let idx_name = self.relation_index_name();
        let prefix = self.relation_key_prefix();

        let schema = vec![
            SchemaField::Tag {
                name: "source".into(),
                separator: None,
            },
            SchemaField::Tag {
                name: "target".into(),
                separator: None,
            },
            SchemaField::Tag {
                name: "relation_type".into(),
                separator: None,
            },
            SchemaField::Text {
                name: "description".into(),
            },
            SchemaField::Vector {
                name: "embedding".into(),
                algorithm: VectorAlgorithm::Hnsw,
                dim: self.embedding_dim,
                distance_metric: DistanceMetric::Cosine,
                initial_cap: None,
            },
        ];

        let options = CreateOptions {
            on_json: false,
            prefixes: vec![prefix],
        };

        match self
            .pool
            .search()
            .ft_create(&idx_name, &options, &schema)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("already exists") {
                    Ok(())
                } else {
                    Err(format!("relation index create: {msg}"))
                }
            }
        }
    }

    async fn ensure_started(&self) -> Result<(), String> {
        // redis-rs's MultiplexedConnection is initialized eagerly when
        // RedisPool::new completes (Pool pre-warms one connection at
        // construction time), so there's no init() step to gate on.
        // The OnceCell is retained for API compatibility — anything
        // we'd want to do once-per-store can hook in here later
        // (e.g. RediSearch capability probe).
        self.started.get_or_init(|| async {}).await;
        Ok(())
    }
}

fn validate_embedding_dim(embedding_dim: usize) -> Result<(), String> {
    if embedding_dim == 0 {
        return Err("embedding_dim must be greater than zero".to_string());
    }

    Ok(())
}

fn validate_index_name(index_name: String) -> Result<String, String> {
    if index_name.is_empty() {
        return Err("index_name must not be empty".to_string());
    }

    if !index_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-'))
    {
        return Err("index_name must use only ASCII letters, digits, ':', '_' or '-'".to_string());
    }

    Ok(index_name)
}

fn parse_boolish(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("invalid boolean value: {other}")),
    }
}

fn source_path_hash(source_path: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Parse a chunks-search reply where the KNN expression aliases its
/// distance to the `score` field (`AS score`). The reply shape is the
/// standard FT.SEARCH `[total, key, [score, X], key, [score, X], ...]`.
fn parse_chunk_reply(
    reply: &SearchReply,
    chunk_prefix: &str,
    score_threshold: f32,
) -> Result<Vec<RedisSearchResult>, String> {
    let mut results = Vec::new();
    for hit in &reply.hits {
        let raw_score: f32 = hit
            .fields
            .iter()
            .find(|(name, _)| name == "score")
            .map(|(_, v)| v.parse().unwrap_or(0.0))
            .unwrap_or(0.0);
        let score = 1.0 - raw_score;
        if score >= score_threshold {
            let chunk_id = hit
                .key
                .strip_prefix(chunk_prefix)
                .map(str::to_string)
                .unwrap_or_else(|| hit.key.clone());
            results.push(RedisSearchResult { chunk_id, score });
        }
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

fn parse_entity_reply(reply: &SearchReply, threshold: f32) -> Result<Vec<EntityHit>, String> {
    let mut results = Vec::new();
    for hit in &reply.hits {
        let get = |name: &str| -> String {
            hit.fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let raw_score: f32 = get("score").parse().unwrap_or(0.0);
        let score = 1.0 - raw_score;
        if score >= threshold {
            let raw_chunks = get("source_chunks");
            let source_chunks: Vec<String> = match serde_json::from_str(&raw_chunks) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        entity_key = %hit.key,
                        error = %e,
                        "corrupt source_chunks JSON on entity hit — falling back to empty list",
                    );
                    Vec::new()
                }
            };
            results.push(EntityHit {
                name: get("name"),
                entity_type: get("entity_type"),
                description: get("description"),
                source_chunks,
                score,
            });
        }
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}

fn parse_relation_reply(reply: &SearchReply, threshold: f32) -> Result<Vec<RelationHit>, String> {
    let mut results = Vec::new();
    for hit in &reply.hits {
        let get = |name: &str| -> String {
            hit.fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let raw_score: f32 = get("score").parse().unwrap_or(0.0);
        let score = 1.0 - raw_score;
        if score >= threshold {
            let raw_chunks = get("source_chunks");
            let source_chunks: Vec<String> = match serde_json::from_str(&raw_chunks) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        relation_key = %hit.key,
                        error = %e,
                        "corrupt source_chunks JSON on relation hit — falling back to empty list",
                    );
                    Vec::new()
                }
            };
            results.push(RelationHit {
                source: get("source"),
                target: get("target"),
                relation_type: get("relation_type"),
                description: get("description"),
                source_chunks,
                score,
            });
        }
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}

// ---------------------------------------------------------------------------
// VectorBackend trait impl (v0.16.0 Phase A1)
// ---------------------------------------------------------------------------
//
// Redis-Vector supports chunk upsert/search/delete + persistence verification.
// Entity / relation / image / hybrid operations use the trait's default
// implementations, which preserve v0.14.x silent-skip semantics:
//   - write ops return Ok(())
//   - read ops return Ok(Vec::new())
//   - hybrid search returns Err so callers fall back to dense-only
//
// `upsert_chunks` requires the `content` field on `VectorPoint` to be `Some`
// because Redis stores the content as a searchable hash field. Callers that
// route to a Redis backend without a content field will see a clear error.

use crate::vector_backend::{
    BackendCapabilities, EntityHit, EntityPoint, RelationHit, RelationPoint, VectorBackend,
    VectorHit, VectorPoint,
};
use async_trait::async_trait;

#[async_trait]
impl VectorBackend for RedisVectorStore {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            name: "redis-vectors",
            chunks: true,
            hybrid: false,
            entities: true,
            relations: true,
            images: false,
            persistence_verifiable: true,
        }
    }

    async fn ensure_collection(&self, _dims: usize) -> Result<(), String> {
        // Redis uses a single global index whose dims were fixed at
        // RedisVectorStore construction. ensure_index creates it if missing;
        // the dims argument is ignored (verified against self.embedding_dim
        // inside search_chunks).
        self.ensure_index().await
    }

    async fn upsert_chunks(&self, points: Vec<VectorPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        // Redis needs the full content text (it's stored as a searchable
        // hash field). Reject up-front if any point is missing it so the
        // error is actionable rather than silently corrupt.
        let records: Result<Vec<RedisChunkRecord>, String> = points
            .into_iter()
            .map(|p| {
                let content = p.content.ok_or_else(|| {
                    "RedisVectorStore.upsert_chunks: VectorPoint.content is required (Redis stores content for FT.SEARCH)"
                        .to_string()
                })?;
                Ok(RedisChunkRecord {
                    chunk_id: p.payload.chunk_id,
                    source_path: p.payload.source_path,
                    heading: p.payload.heading,
                    chunk_index: p.payload.chunk_index,
                    content,
                    vector: p.vector,
                })
            })
            .collect();
        let records = records?;
        self.upsert_chunks(&records).await?;
        Ok(())
    }

    async fn search_chunks(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<VectorHit>, String> {
        let redis_results = self
            .search_chunks(&query_vector, top_k, score_threshold)
            .await?;
        // Redis returns only (chunk_id, score). source_path/heading/chunk_index
        // are empty strings/zeros here; callers look those up via
        // MemoryEngine::by_id which has the full metadata.
        Ok(redis_results
            .into_iter()
            .map(|r| VectorHit {
                chunk_id: r.chunk_id,
                source_path: String::new(),
                heading: String::new(),
                chunk_index: 0,
                score: r.score,
            })
            .collect())
    }

    async fn delete_by_source_path(&self, source_path: &str) -> Result<(), String> {
        // Redis's delete_by_source_path returns the number of deleted keys;
        // we discard it (the trait's signature matches Qdrant's `Result<()>`).
        self.delete_by_source_path(source_path).await?;
        Ok(())
    }

    async fn verify_persistence(&self) -> Result<(), String> {
        self.verify_persistence().await
    }

    // ── Entity operations (Phase 7) ───────────────────────────────

    async fn ensure_entity_collection(&self, _dims: usize) -> Result<(), String> {
        self.ensure_entity_index().await
    }

    async fn upsert_entities(&self, points: Vec<EntityPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        self.ensure_entity_index().await?;

        for p in &points {
            if p.vector.len() != self.embedding_dim {
                return Err(format!(
                    "entity {} dim mismatch: expected {}, got {}",
                    p.id,
                    self.embedding_dim,
                    p.vector.len()
                ));
            }
            let key = self.entity_key(&p.id);
            let emb = vector_to_bytes(&p.vector);
            let source_chunks_json =
                serde_json::to_string(&p.source_chunks).unwrap_or_else(|_| "[]".to_string());
            let fields: [(&str, &[u8]); 5] = [
                ("name", p.name.as_bytes()),
                ("entity_type", p.entity_type.as_bytes()),
                ("description", p.description.as_bytes()),
                ("source_chunks", source_chunks_json.as_bytes()),
                ("embedding", emb.as_slice()),
            ];
            self.pool
                .hashes()
                .hset_multi(&key, &fields)
                .await
                .map_err(|e| format!("redis hset entity {}: {e}", p.id))?;
        }
        Ok(())
    }

    async fn search_entities(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<EntityHit>, String> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        self.ensure_entity_index().await?;

        let idx = self.entity_index_name();
        let raw_query = format!("*=>[KNN {} @embedding $BLOB AS score]", top_k);
        let query_blob = vector_to_bytes(&query_vector);
        let query = Query::new(raw_query)
            .return_fields([
                "name",
                "entity_type",
                "description",
                "source_chunks",
                "score",
            ])
            .param("BLOB", query_blob)
            .limit(0, top_k)
            .dialect(2);

        let reply = self
            .pool
            .search()
            .ft_search(&idx, &query)
            .await
            .map_err(|e| format!("redis entity search: {e}"))?;

        parse_entity_reply(&reply, score_threshold)
    }

    // ── Relation operations (Phase 7) ─────────────────────────────

    async fn ensure_relation_collection(&self, _dims: usize) -> Result<(), String> {
        self.ensure_relation_index().await
    }

    async fn upsert_relations(&self, points: Vec<RelationPoint>) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        self.ensure_relation_index().await?;

        for p in &points {
            if p.vector.len() != self.embedding_dim {
                return Err(format!(
                    "relation {} dim mismatch: expected {}, got {}",
                    p.id,
                    self.embedding_dim,
                    p.vector.len()
                ));
            }
            let key = self.relation_key(&p.id);
            let emb = vector_to_bytes(&p.vector);
            let source_chunks_json =
                serde_json::to_string(&p.source_chunks).unwrap_or_else(|_| "[]".to_string());
            let fields: [(&str, &[u8]); 6] = [
                ("source", p.source.as_bytes()),
                ("target", p.target.as_bytes()),
                ("relation_type", p.relation_type.as_bytes()),
                ("description", p.description.as_bytes()),
                ("source_chunks", source_chunks_json.as_bytes()),
                ("embedding", emb.as_slice()),
            ];
            self.pool
                .hashes()
                .hset_multi(&key, &fields)
                .await
                .map_err(|e| format!("redis hset relation {}: {e}", p.id))?;
        }
        Ok(())
    }

    async fn search_relations(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<RelationHit>, String> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        self.ensure_relation_index().await?;

        let idx = self.relation_index_name();
        let raw_query = format!("*=>[KNN {} @embedding $BLOB AS score]", top_k);
        let query_blob = vector_to_bytes(&query_vector);
        let query = Query::new(raw_query)
            .return_fields([
                "source",
                "target",
                "relation_type",
                "description",
                "source_chunks",
                "score",
            ])
            .param("BLOB", query_blob)
            .limit(0, top_k)
            .dialect(2);

        let reply = self
            .pool
            .search()
            .ft_search(&idx, &query)
            .await
            .map_err(|e| format!("redis relation search: {e}"))?;

        parse_relation_reply(&reply, score_threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::RedisVectorStore;

    fn test_store(index: &str, dim: usize) -> RedisVectorStore {
        // Tests don't connect to Redis — they only exercise pure
        // constructor + schema logic. The pool is built but never
        // dispatched to; if any test starts hitting the network this
        // assertion will surface immediately.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            RedisVectorStore::new_for_test("redis://127.0.0.1:6379", index, dim)
                .await
                .expect("store")
        })
    }

    #[test]
    fn redis_vector_store_builds_index_schema_with_hnsw() {
        let store = test_store("the_one_memories", 1024);

        let schema = store.index_schema();
        assert!(schema.contains("VECTOR"));
        assert!(schema.contains("HNSW"));
        assert!(schema.contains("source_path"));
        assert!(schema.contains("content"));
        assert!(schema.contains("DIM 1024"));
    }

    #[test]
    fn redis_vector_store_capabilities_include_entities_and_relations() {
        use crate::vector_backend::VectorBackend;
        let store = test_store("the_one_memories", 1024);
        let caps = store.capabilities();
        assert!(caps.chunks);
        assert!(caps.entities);
        assert!(caps.relations);
        assert!(!caps.hybrid);
        assert!(!caps.images);
    }

    #[tokio::test]
    async fn redis_vector_store_rejects_invalid_index_name() {
        match RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the one memories", 1024)
            .await
        {
            Ok(_) => panic!("invalid index name should be rejected"),
            Err(err) => assert!(err.contains("index_name")),
        }
    }

    #[tokio::test]
    async fn redis_vector_store_rejects_zero_embedding_dim() {
        match RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 0).await
        {
            Ok(_) => panic!("zero embedding dimensions should be rejected"),
            Err(err) => assert!(err.contains("embedding_dim")),
        }
    }

    #[test]
    fn parse_persistence_info_accepts_aof_enabled_state() {
        let parsed = RedisVectorStore::parse_persistence_info(
            "aof_enabled:1\nrdb_last_save_time:0\nrdb_last_bgsave_status:err\n",
        )
        .expect("parse");

        assert!(parsed.aof_enabled);
        assert!(parsed.is_persistent());
    }

    #[test]
    fn parse_persistence_info_accepts_rdb_state() {
        let parsed = RedisVectorStore::parse_persistence_info(
            "aof_enabled:0\nrdb_last_save_time:1710000000\nrdb_last_bgsave_status:ok\n",
        )
        .expect("parse");

        assert!(!parsed.aof_enabled);
        assert!(parsed.rdb_last_bgsave_status_ok);
        assert!(parsed.is_persistent());
    }

    #[test]
    fn parse_persistence_info_rejects_misformatted_boolean_values() {
        let err = RedisVectorStore::parse_persistence_info("aof_enabled:maybe\n")
            .expect_err("invalid boolean should fail");
        assert!(err.contains("invalid boolean value"));
    }
}
