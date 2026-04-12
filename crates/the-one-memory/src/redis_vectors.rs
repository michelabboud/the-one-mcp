use std::convert::TryInto;
use std::sync::Arc;

use fred::clients::Client;
use fred::interfaces::{
    ClientLike, HashesInterface, KeysInterface, RediSearchInterface, SetsInterface,
};
use fred::types::config::Config;
use fred::types::redisearch::{FtCreateOptions, IndexKind, SearchSchema, SearchSchemaKind};
use fred::types::{ClusterHash, CustomCommand, InfoKind, Map, Value};
use tokio::sync::OnceCell;

#[derive(Debug)]
pub struct RedisVectorStore {
    client: Client,
    index_name: String,
    embedding_dim: usize,
    started: Arc<OnceCell<()>>,
}

impl Clone for RedisVectorStore {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            index_name: self.index_name.clone(),
            embedding_dim: self.embedding_dim,
            started: Arc::new(OnceCell::new()),
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
        client: Client,
        index_name: impl Into<String>,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        let index_name = validate_index_name(index_name.into())?;
        validate_embedding_dim(embedding_dim)?;

        Ok(Self {
            client,
            index_name,
            embedding_dim,
            started: Arc::new(OnceCell::new()),
        })
    }

    pub fn from_url(
        redis_url: &str,
        index_name: impl Into<String>,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        if redis_url.trim().is_empty() {
            return Err("redis_url must not be empty".to_string());
        }

        let config = Config::from_url(redis_url).map_err(|e| format!("invalid redis_url: {e}"))?;
        let client = Client::new(config, None, None, None);

        Self::new(client, index_name, embedding_dim)
    }

    pub fn new_for_test(
        redis_url: &str,
        index_name: &str,
        embedding_dim: usize,
    ) -> Result<Self, String> {
        Self::from_url(redis_url, index_name, embedding_dim)
    }

    pub fn client(&self) -> &Client {
        &self.client
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
            .client
            .info::<String>(Some(InfoKind::Persistence))
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
            SearchSchema {
                field_name: "chunk_id".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: true,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "source_path".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: true,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "heading".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: true,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "chunk_index".into(),
                alias: None,
                kind: SearchSchemaKind::Numeric {
                    sortable: false,
                    unf: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "content".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: false,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "embedding".into(),
                alias: None,
                kind: SearchSchemaKind::Custom {
                    name: "VECTOR".into(),
                    arguments: vec![
                        "HNSW".into(),
                        6usize
                            .try_into()
                            .map_err(|e| format!("vector schema error: {e}"))?,
                        "TYPE".into(),
                        "FLOAT32".into(),
                        "DIM".into(),
                        self.embedding_dim
                            .try_into()
                            .map_err(|e| format!("vector schema error: {e}"))?,
                        "DISTANCE_METRIC".into(),
                        "COSINE".into(),
                    ],
                },
            },
        ];

        let options = FtCreateOptions {
            on: Some(IndexKind::Hash),
            prefixes: vec![self.chunk_key_prefix().into()],
            skipinitialscan: true,
            ..Default::default()
        };

        match self
            .client
            .ft_create::<Value, _>(&self.index_name, options, schema)
            .await
        {
            Ok(_) => Ok(()),
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
            let mut fields = Map::new();
            fields.insert("chunk_id".into(), record.chunk_id.clone().into());
            fields.insert("source_path".into(), record.source_path.clone().into());
            fields.insert("heading".into(), record.heading.clone().into());
            fields.insert("chunk_index".into(), (record.chunk_index as i64).into());
            fields.insert("content".into(), record.content.clone().into());
            let embedding_bytes = vector_to_bytes(&record.vector);
            fields.insert("embedding".into(), embedding_bytes.as_slice().into());

            let _: Value = self
                .client
                .hset(&key, fields)
                .await
                .map_err(|e| format!("redis hset failed for {key}: {e}"))?;

            let source_key = self.source_index_key(&record.source_path);
            let _: Value = self
                .client
                .sadd(&source_key, vec![key.clone()])
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
            .client
            .smembers(&source_key)
            .await
            .map_err(|e| format!("redis smembers failed for {source_key}: {e}"))?;

        if members.is_empty() {
            return Ok(0);
        }

        let deleted: i64 = self
            .client
            .del(members.clone())
            .await
            .map_err(|e| format!("redis del failed: {e}"))?;

        let _: i64 = self
            .client
            .del(vec![source_key])
            .await
            .map_err(|e| format!("redis del failed for source index: {e}"))?;

        Ok(deleted.max(0) as usize)
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
        let command = CustomCommand::new("FT.SEARCH", ClusterHash::FirstKey, false);
        let args: Vec<Value> = vec![
            self.index_name.clone().into(),
            raw_query.into(),
            "PARAMS".into(),
            2i64.into(),
            "BLOB".into(),
            query_blob.as_slice().into(),
            "NOCONTENT".into(),
            "WITHSCORES".into(),
            "LIMIT".into(),
            0i64.into(),
            top_k
                .try_into()
                .map_err(|e| format!("invalid top_k: {e}"))?,
            "DIALECT".into(),
            2i64.into(),
        ];

        let value: Value = self
            .client
            .custom::<Value, _>(command, args)
            .await
            .map_err(|e| format!("redis vector search failed: {e}"))?;

        parse_search_results(value, &self.chunk_key_prefix(), score_threshold)
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
            SearchSchema {
                field_name: "name".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: true,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "entity_type".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "description".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: false,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "embedding".into(),
                alias: None,
                kind: SearchSchemaKind::Custom {
                    name: "VECTOR".into(),
                    arguments: vec![
                        "HNSW".into(),
                        6usize.try_into().map_err(|e| format!("{e}"))?,
                        "TYPE".into(),
                        "FLOAT32".into(),
                        "DIM".into(),
                        self.embedding_dim.try_into().map_err(|e| format!("{e}"))?,
                        "DISTANCE_METRIC".into(),
                        "COSINE".into(),
                    ],
                },
            },
        ];

        let options = FtCreateOptions {
            on: Some(IndexKind::Hash),
            prefixes: vec![prefix.into()],
            skipinitialscan: true,
            ..Default::default()
        };

        match self
            .client
            .ft_create::<Value, _>(&idx_name, options, schema)
            .await
        {
            Ok(_) => Ok(()),
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
            SearchSchema {
                field_name: "source".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "target".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "relation_type".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "description".into(),
                alias: None,
                kind: SearchSchemaKind::Text {
                    sortable: false,
                    unf: false,
                    nostem: false,
                    phonetic: None,
                    weight: None,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
            SearchSchema {
                field_name: "embedding".into(),
                alias: None,
                kind: SearchSchemaKind::Custom {
                    name: "VECTOR".into(),
                    arguments: vec![
                        "HNSW".into(),
                        6usize.try_into().map_err(|e| format!("{e}"))?,
                        "TYPE".into(),
                        "FLOAT32".into(),
                        "DIM".into(),
                        self.embedding_dim.try_into().map_err(|e| format!("{e}"))?,
                        "DISTANCE_METRIC".into(),
                        "COSINE".into(),
                    ],
                },
            },
        ];

        let options = FtCreateOptions {
            on: Some(IndexKind::Hash),
            prefixes: vec![prefix.into()],
            skipinitialscan: true,
            ..Default::default()
        };

        match self
            .client
            .ft_create::<Value, _>(&idx_name, options, schema)
            .await
        {
            Ok(_) => Ok(()),
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
        self.started
            .get_or_try_init(|| async {
                self.client
                    .init()
                    .await
                    .map_err(|e| format!("failed to initialize Redis client: {e}"))?;
                Ok(())
            })
            .await
            .map(|_| ())
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

fn parse_search_results(
    value: Value,
    chunk_prefix: &str,
    score_threshold: f32,
) -> Result<Vec<RedisSearchResult>, String> {
    let Value::Array(mut values) = value else {
        return Err("unexpected Redis search response shape".to_string());
    };

    if values.is_empty() {
        return Ok(Vec::new());
    }

    values.remove(0);

    let mut results = Vec::new();
    let mut i = 0usize;
    while i < values.len() {
        let (key_value, score_value, consumed) = if let Some(Value::Array(row)) = values.get(i) {
            if row.len() >= 2 {
                (row[0].clone(), row[1].clone(), 1usize)
            } else {
                return Err("unexpected nested Redis search row".to_string());
            }
        } else if i + 1 < values.len() {
            (values[i].clone(), values[i + 1].clone(), 2usize)
        } else {
            return Err("unexpected trailing Redis search row".to_string());
        };

        let key = key_value
            .convert::<String>()
            .map_err(|e| format!("failed to decode Redis key: {e}"))?;
        let raw_score = parse_score(score_value)?;
        let score = 1.0 - raw_score;
        if score >= score_threshold {
            let chunk_id = key
                .strip_prefix(chunk_prefix)
                .map(str::to_string)
                .unwrap_or(key.clone());
            results.push(RedisSearchResult { chunk_id, score });
        }

        i += consumed;
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

fn parse_entity_results(
    value: Value,
    prefix: &str,
    threshold: f32,
) -> Result<Vec<EntityHit>, String> {
    let Value::Array(mut vals) = value else {
        return Ok(Vec::new());
    };
    if vals.is_empty() {
        return Ok(Vec::new());
    }
    vals.remove(0); // count

    let mut results = Vec::new();
    let mut i = 0;
    while i + 2 < vals.len() {
        let key_str = vals[i].clone().convert::<String>().unwrap_or_default();
        let score = 1.0 - parse_score(vals[i + 1].clone())?;
        let fields = &vals[i + 2];

        if score >= threshold {
            let get = |name: &str| -> String {
                if let Value::Array(arr) = fields {
                    let mut j = 0;
                    while j + 1 < arr.len() {
                        if let Some(k) = arr[j].as_str() {
                            if k == name {
                                return arr[j + 1].clone().convert::<String>().unwrap_or_default();
                            }
                        }
                        j += 2;
                    }
                }
                String::new()
            };
            let _ = key_str.strip_prefix(prefix);
            let source_chunks: Vec<String> =
                serde_json::from_str(&get("source_chunks")).unwrap_or_default();
            results.push(EntityHit {
                name: get("name"),
                entity_type: get("entity_type"),
                description: get("description"),
                source_chunks,
                score,
            });
        }
        i += 3;
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}

fn parse_relation_results(
    value: Value,
    prefix: &str,
    threshold: f32,
) -> Result<Vec<RelationHit>, String> {
    let Value::Array(mut vals) = value else {
        return Ok(Vec::new());
    };
    if vals.is_empty() {
        return Ok(Vec::new());
    }
    vals.remove(0);

    let mut results = Vec::new();
    let mut i = 0;
    while i + 2 < vals.len() {
        let key_str = vals[i].clone().convert::<String>().unwrap_or_default();
        let score = 1.0 - parse_score(vals[i + 1].clone())?;
        let fields = &vals[i + 2];

        if score >= threshold {
            let get = |name: &str| -> String {
                if let Value::Array(arr) = fields {
                    let mut j = 0;
                    while j + 1 < arr.len() {
                        if let Some(k) = arr[j].as_str() {
                            if k == name {
                                return arr[j + 1].clone().convert::<String>().unwrap_or_default();
                            }
                        }
                        j += 2;
                    }
                }
                String::new()
            };
            let _ = key_str.strip_prefix(prefix);
            let source_chunks: Vec<String> =
                serde_json::from_str(&get("source_chunks")).unwrap_or_default();
            results.push(RelationHit {
                source: get("source"),
                target: get("target"),
                relation_type: get("relation_type"),
                description: get("description"),
                source_chunks,
                score,
            });
        }
        i += 3;
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}

fn parse_score(value: Value) -> Result<f32, String> {
    if let Some(score) = value.as_f64() {
        return Ok(score as f32);
    }

    let text = value
        .convert::<String>()
        .map_err(|e| format!("failed to decode Redis score: {e}"))?;
    text.parse::<f32>()
        .map_err(|e| format!("failed to parse Redis score '{text}': {e}"))
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
            let mut fields = Map::new();
            fields.insert("name".into(), p.name.clone().into());
            fields.insert("entity_type".into(), p.entity_type.clone().into());
            fields.insert("description".into(), p.description.clone().into());
            fields.insert(
                "source_chunks".into(),
                serde_json::to_string(&p.source_chunks)
                    .unwrap_or_else(|_| "[]".to_string())
                    .into(),
            );
            let emb = vector_to_bytes(&p.vector);
            fields.insert("embedding".into(), emb.as_slice().into());

            let _: Value = self
                .client
                .hset(&key, fields)
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
        let prefix = self.entity_key_prefix();
        let raw_query = format!("*=>[KNN {} @embedding $BLOB AS score]", top_k);
        let query_blob = vector_to_bytes(&query_vector);
        let command = CustomCommand::new("FT.SEARCH", ClusterHash::FirstKey, false);
        let args: Vec<Value> = vec![
            idx.into(),
            raw_query.into(),
            "PARAMS".into(),
            2i64.into(),
            "BLOB".into(),
            query_blob.as_slice().into(),
            "RETURN".into(),
            4i64.into(),
            "name".into(),
            "entity_type".into(),
            "description".into(),
            "source_chunks".into(),
            "WITHSCORES".into(),
            "LIMIT".into(),
            0i64.into(),
            (top_k as i64).into(),
            "DIALECT".into(),
            2i64.into(),
        ];

        let value: Value = self
            .client
            .custom::<Value, _>(command, args)
            .await
            .map_err(|e| format!("redis entity search: {e}"))?;

        parse_entity_results(value, &prefix, score_threshold)
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
            let mut fields = Map::new();
            fields.insert("source".into(), p.source.clone().into());
            fields.insert("target".into(), p.target.clone().into());
            fields.insert("relation_type".into(), p.relation_type.clone().into());
            fields.insert("description".into(), p.description.clone().into());
            fields.insert(
                "source_chunks".into(),
                serde_json::to_string(&p.source_chunks)
                    .unwrap_or_else(|_| "[]".to_string())
                    .into(),
            );
            let emb = vector_to_bytes(&p.vector);
            fields.insert("embedding".into(), emb.as_slice().into());

            let _: Value = self
                .client
                .hset(&key, fields)
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
        let prefix = self.relation_key_prefix();
        let raw_query = format!("*=>[KNN {} @embedding $BLOB AS score]", top_k);
        let query_blob = vector_to_bytes(&query_vector);
        let command = CustomCommand::new("FT.SEARCH", ClusterHash::FirstKey, false);
        let args: Vec<Value> = vec![
            idx.into(),
            raw_query.into(),
            "PARAMS".into(),
            2i64.into(),
            "BLOB".into(),
            query_blob.as_slice().into(),
            "RETURN".into(),
            5i64.into(),
            "source".into(),
            "target".into(),
            "relation_type".into(),
            "description".into(),
            "source_chunks".into(),
            "WITHSCORES".into(),
            "LIMIT".into(),
            0i64.into(),
            (top_k as i64).into(),
            "DIALECT".into(),
            2i64.into(),
        ];

        let value: Value = self
            .client
            .custom::<Value, _>(command, args)
            .await
            .map_err(|e| format!("redis relation search: {e}"))?;

        parse_relation_results(value, &prefix, score_threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::RedisVectorStore;

    #[test]
    fn redis_vector_store_builds_index_schema_with_hnsw() {
        let store =
            RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 1024)
                .expect("store");

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
        let store =
            RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 1024)
                .expect("store");
        let caps = store.capabilities();
        assert!(caps.chunks);
        assert!(caps.entities);
        assert!(caps.relations);
        assert!(!caps.hybrid);
        assert!(!caps.images);
    }

    #[test]
    fn redis_vector_store_rejects_invalid_index_name() {
        match RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the one memories", 1024) {
            Ok(_) => panic!("invalid index name should be rejected"),
            Err(err) => assert!(err.contains("index_name")),
        }
    }

    #[test]
    fn redis_vector_store_rejects_zero_embedding_dim() {
        match RedisVectorStore::new_for_test("redis://127.0.0.1:6379", "the_one_memories", 0) {
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
