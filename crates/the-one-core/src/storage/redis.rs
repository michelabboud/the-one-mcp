//! Redis StateStore backend (v0.16.0 Phase 5).
//!
//! Two durability modes:
//!
//! - **Cache** (`require_aof = false`): no persistence check, data
//!   is lost on Redis restart.
//! - **Persistent** (`require_aof = true`): startup calls
//!   `INFO persistence` and refuses to boot if `aof_enabled:0`.
//!
//! ## Data model
//!
//! All keys live under `{prefix}:{project_id}:` with type suffixes.
//! Objects are stored as HSET with a single `json` field containing
//! serialized JSON. Audit uses Redis Streams (XADD/XREVRANGE).
//! Diary entries use HSET with individual fields so RediSearch can
//! index `content` for FTS.
//!
//! ## Sync bridge
//!
//! Same `tokio::task::block_in_place` + `Handle::current().block_on`
//! pattern as the Postgres backend.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use fred::clients::Client;
use fred::interfaces::{
    ClientLike, HashesInterface, KeysInterface, RediSearchInterface, SetsInterface,
    SortedSetsInterface, StreamsInterface,
};
use fred::types::config::Config;
use fred::types::redisearch::{
    FtCreateOptions, FtSearchOptions, IndexKind, SearchSchema, SearchSchemaKind,
};
use fred::types::{InfoKind, Value};

use crate::audit::AuditRecord;
use crate::contracts::{
    AaakLesson, ApprovalScope, DiaryEntry, MemoryNavigationNode, MemoryNavigationTunnel,
};
use crate::error::CoreError;
use crate::pagination::{Page, PageRequest};
use crate::state_store::{StateStore, StateStoreCapabilities};
use crate::storage::sqlite::{AuditEvent, ConversationSourceRecord};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RedisStateConfig {
    pub prefix: String,
    pub require_aof: bool,
    pub db_number: u8,
}

impl Default for RedisStateConfig {
    fn default() -> Self {
        Self {
            prefix: "the_one_state".to_string(),
            require_aof: false,
            db_number: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn block_on<F, R>(fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn redis_err(ctx: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::Redis(format!("{ctx}: {e}"))
}

fn val_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        Value::Bytes(b) => String::from_utf8_lossy(b).to_string(),
        Value::Integer(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Simple FNV-1a hash for generating deterministic conversation keys.
fn md5_lite(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in input.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn map_get(m: &fred::types::Map, k: &str) -> String {
    // fred::types::Map::inner() consumes self, so clone. Maps are
    // small (single-digit field counts per hash) so this is cheap.
    m.clone()
        .inner()
        .into_iter()
        .find(|(mk, _)| {
            let b = mk.as_bytes();
            std::str::from_utf8(b).unwrap_or("") == k
        })
        .map(|(_, v)| val_to_string(&v))
        .unwrap_or_default()
}

fn parse_conv_hash(vals: &Value) -> Option<ConversationSourceRecord> {
    let m = match vals {
        Value::Map(m) if !m.is_empty() => m,
        _ => return None,
    };

    let wing_raw = map_get(m, "wing");
    let hall_raw = map_get(m, "hall");
    let room_raw = map_get(m, "room");

    Some(ConversationSourceRecord {
        project_id: map_get(m, "project_id"),
        transcript_path: map_get(m, "transcript_path"),
        memory_path: map_get(m, "memory_path"),
        format: map_get(m, "format"),
        wing: if wing_raw.is_empty() {
            None
        } else {
            Some(wing_raw)
        },
        hall: if hall_raw.is_empty() {
            None
        } else {
            Some(hall_raw)
        },
        room: if room_raw.is_empty() {
            None
        } else {
            Some(room_raw)
        },
        message_count: map_get(m, "message_count").parse().unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// RedisStateStore
// ---------------------------------------------------------------------------

pub struct RedisStateStore {
    client: Client,
    project_id: String,
    prefix: String,
    config: RedisStateConfig,
}

impl std::fmt::Debug for RedisStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisStateStore")
            .field("project_id", &self.project_id)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl RedisStateStore {
    pub async fn new(
        config: &RedisStateConfig,
        url: &str,
        project_id: &str,
    ) -> Result<Self, CoreError> {
        let redis_config = Config::from_url(url).map_err(|e| redis_err("invalid url", e))?;
        let client = Client::new(redis_config, None, None, None);
        client.init().await.map_err(|e| redis_err("connect", e))?;

        let prefix = format!("{}:{}", config.prefix, project_id);

        if config.require_aof {
            verify_aof(&client).await?;
        }

        let store = Self {
            client,
            project_id: project_id.to_string(),
            prefix,
            config: config.clone(),
        };

        store.ensure_diary_index().await?;
        Ok(store)
    }

    /// Phase 6 — build from an already-connected client.
    pub async fn from_client(
        client: Client,
        config: &RedisStateConfig,
        project_id: &str,
    ) -> Result<Self, CoreError> {
        let prefix = format!("{}:{}", config.prefix, project_id);
        let store = Self {
            client,
            project_id: project_id.to_string(),
            prefix,
            config: config.clone(),
        };
        store.ensure_diary_index().await?;
        Ok(store)
    }

    pub async fn close(&self) {
        let _ = self.client.quit().await;
    }

    fn key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.prefix)
    }

    async fn ensure_diary_index(&self) -> Result<(), CoreError> {
        let index_name = self.key("diary_idx");
        let prefix = format!("{}:diary:", self.prefix);

        // If FT.INFO succeeds, the index exists.
        if self.client.ft_info::<Value, _>(&index_name).await.is_ok() {
            return Ok(());
        }

        let opts = FtCreateOptions {
            on: Some(IndexKind::Hash),
            prefixes: vec![prefix.into()],
            ..Default::default()
        };

        let schema = vec![
            SearchSchema {
                field_name: "content".into(),
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
                field_name: "entry_date".into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: true,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            },
        ];

        self.client
            .ft_create::<(), _>(&index_name, opts, schema)
            .await
            .map_err(|e| redis_err("FT.CREATE diary_idx", e))?;
        Ok(())
    }
}

async fn verify_aof(client: &Client) -> Result<(), CoreError> {
    let info: String = client
        .info(Some(InfoKind::Persistence))
        .await
        .map_err(|e| redis_err("INFO persistence", e))?;

    let aof_enabled = info
        .lines()
        .find(|l| l.starts_with("aof_enabled:"))
        .and_then(|l| l.strip_prefix("aof_enabled:"))
        .map(|v| v.trim() == "1")
        .unwrap_or(false);

    if !aof_enabled {
        return Err(CoreError::InvalidProjectConfig(
            "THE_ONE_STATE_TYPE=redis with require_aof=true, but Redis reports \
             aof_enabled:0. Configure Redis with `appendonly yes` or set \
             state_redis.require_aof=false for cache mode."
                .to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// StateStore impl
// ---------------------------------------------------------------------------

impl StateStore for RedisStateStore {
    fn project_id(&self) -> &str {
        &self.project_id
    }

    fn schema_version(&self) -> Result<i64, CoreError> {
        Ok(1)
    }

    fn capabilities(&self) -> StateStoreCapabilities {
        StateStoreCapabilities {
            name: "redis",
            fts: true,
            transactions: false,
            durable: self.config.require_aof,
            schema_versioned: false,
        }
    }

    // ── Profiles ──────────────────────────────────────────────────

    fn upsert_project_profile(&self, profile_json: &str) -> Result<(), CoreError> {
        let key = self.key("profile");
        let val = profile_json.to_string();
        block_on(async {
            self.client
                .set::<(), _, _>(&key, val.as_str(), None, None, false)
                .await
                .map_err(|e| redis_err("SET profile", e))
        })
    }

    fn latest_project_profile(&self) -> Result<Option<String>, CoreError> {
        let key = self.key("profile");
        block_on(async {
            let v: Value = self
                .client
                .get(&key)
                .await
                .map_err(|e| redis_err("GET profile", e))?;
            match v {
                Value::Null => Ok(None),
                Value::String(s) => Ok(Some(s.to_string())),
                Value::Bytes(b) => Ok(Some(String::from_utf8_lossy(&b).to_string())),
                _ => Ok(None),
            }
        })
    }

    // ── Approvals ─────────────────────────────────────────────────

    fn set_approval(
        &self,
        action_key: &str,
        scope: ApprovalScope,
        approved: bool,
    ) -> Result<(), CoreError> {
        let scope_str = match scope {
            ApprovalScope::Once => "once",
            ApprovalScope::Session => "session",
            ApprovalScope::Forever => "forever",
        };
        let key = self.key(&format!("approval:{action_key}:{scope_str}"));
        block_on(async {
            if approved {
                self.client
                    .set::<(), _, _>(&key, "1", None, None, false)
                    .await
                    .map_err(|e| redis_err("SET approval", e))
            } else {
                self.client
                    .del::<(), _>(&key)
                    .await
                    .map_err(|e| redis_err("DEL approval", e))
            }
        })
    }

    fn is_approved(&self, action_key: &str, scope: ApprovalScope) -> Result<bool, CoreError> {
        let scope_str = match scope {
            ApprovalScope::Once => "once",
            ApprovalScope::Session => "session",
            ApprovalScope::Forever => "forever",
        };
        let key = self.key(&format!("approval:{action_key}:{scope_str}"));
        block_on(async {
            let exists: bool = self
                .client
                .exists(&key)
                .await
                .map_err(|e| redis_err("EXISTS approval", e))?;
            Ok(exists)
        })
    }

    // ── Audit ─────────────────────────────────────────────────────

    fn record_audit_event(&self, event_type: &str, payload_json: &str) -> Result<(), CoreError> {
        let key = self.key("audit");
        let ts = now_epoch_ms().to_string();
        let fields: Vec<(&str, &str)> = vec![
            ("event_type", event_type),
            ("payload", payload_json),
            ("outcome", "unknown"),
            ("error_kind", ""),
            ("ts", &ts),
            ("project_id", &self.project_id),
        ];
        block_on(async {
            self.client
                .xadd::<(), _, _, _, _>(&key, false, None, "*", fields)
                .await
                .map_err(|e| redis_err("XADD audit", e))
        })
    }

    fn record_audit(&self, record: &AuditRecord) -> Result<(), CoreError> {
        let key = self.key("audit");
        let ts = now_epoch_ms().to_string();
        let outcome = record.outcome.as_str();
        let ek = record.error_kind.unwrap_or("");
        let fields: Vec<(&str, &str)> = vec![
            ("event_type", record.operation),
            ("payload", &record.params_json),
            ("outcome", outcome),
            ("error_kind", ek),
            ("ts", &ts),
            ("project_id", &self.project_id),
        ];
        block_on(async {
            self.client
                .xadd::<(), _, _, _, _>(&key, false, None, "*", fields)
                .await
                .map_err(|e| redis_err("XADD audit", e))
        })
    }

    fn audit_event_count_for_project(&self) -> Result<u64, CoreError> {
        let key = self.key("audit");
        block_on(async {
            let len: u64 = self
                .client
                .xlen(&key)
                .await
                .map_err(|e| redis_err("XLEN audit", e))?;
            Ok(len)
        })
    }

    fn list_audit_events_paged(&self, req: &PageRequest) -> Result<Page<AuditEvent>, CoreError> {
        let key = self.key("audit");
        let limit = req.limit;
        // Use offset-based pagination via XREVRANGE with COUNT.
        // For simplicity, fetch all and skip to offset.
        block_on(async {
            let count = (req.offset as usize) + limit + 1;
            let entries: Value = self
                .client
                .xrevrange(&key, "+", "-", Some(count as u64))
                .await
                .map_err(|e| redis_err("XREVRANGE audit", e))?;

            let all = parse_audit_stream(&entries);
            let offset = req.offset as usize;
            let slice: Vec<AuditEvent> = all.into_iter().skip(offset).take(limit + 1).collect();

            let page = Page::from_peek(slice, limit, req.offset, None);
            Ok(page)
        })
    }

    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        let key = self.key("audit");
        block_on(async {
            let entries: Value = self
                .client
                .xrevrange(&key, "+", "-", Some(limit as u64))
                .await
                .map_err(|e| redis_err("XREVRANGE audit", e))?;
            Ok(parse_audit_stream(&entries))
        })
    }

    // ── Conversation sources ──────────────────────────────────────

    fn upsert_conversation_source(
        &self,
        record: &ConversationSourceRecord,
    ) -> Result<(), CoreError> {
        // Use a hash of (transcript_path + format) as the unique key.
        let conv_id = format!(
            "{:x}",
            md5_lite(&format!("{}:{}", record.transcript_path, record.format))
        );
        let hk = self.key(&format!("conv:{conv_id}"));
        let idx = self.key("conv_idx");
        let ts = now_epoch_ms() as f64;

        block_on(async {
            self.client
                .hset::<(), _, _>(
                    &hk,
                    [
                        ("project_id", record.project_id.as_str()),
                        ("transcript_path", record.transcript_path.as_str()),
                        ("memory_path", record.memory_path.as_str()),
                        ("format", record.format.as_str()),
                        ("wing", record.wing.as_deref().unwrap_or("")),
                        ("hall", record.hall.as_deref().unwrap_or("")),
                        ("room", record.room.as_deref().unwrap_or("")),
                        ("message_count", &record.message_count.to_string()),
                    ],
                )
                .await
                .map_err(|e| redis_err("HSET conv", e))?;
            self.client
                .zadd::<(), _, _>(&idx, None, None, false, false, (ts, conv_id.as_str()))
                .await
                .map_err(|e| redis_err("ZADD conv_idx", e))?;
            Ok(())
        })
    }

    fn list_conversation_sources(
        &self,
        _wing: Option<&str>,
        _hall: Option<&str>,
        _room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSourceRecord>, CoreError> {
        let idx = self.key("conv_idx");
        block_on(async {
            let ids: Vec<Value> = self
                .client
                .zrevrangebyscore(&idx, "+inf", "-inf", false, Some((0, limit as i64)))
                .await
                .map_err(|e| redis_err("ZREVRANGEBYSCORE conv", e))?;

            let mut results = Vec::new();
            for id_val in &ids {
                let id = val_to_string(id_val);
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("conv:{id}"));
                let vals: Value = self
                    .client
                    .hgetall(&hk)
                    .await
                    .map_err(|e| redis_err("HGETALL conv", e))?;
                if let Some(rec) = parse_conv_hash(&vals) {
                    results.push(rec);
                }
            }
            Ok(results)
        })
    }

    // ── AAAK ──────────────────────────────────────────────────────

    fn upsert_aaak_lesson(&self, lesson: &AaakLesson) -> Result<(), CoreError> {
        let hk = self.key(&format!("aaak:{}", lesson.lesson_id));
        let idx = self.key("aaak_idx");
        let ts = lesson.updated_at_epoch_ms as f64;
        let json = serde_json::to_string(lesson).map_err(|e| redis_err("serialize aaak", e))?;

        block_on(async {
            self.client
                .hset::<(), _, _>(&hk, [("json", json.as_str())])
                .await
                .map_err(|e| redis_err("HSET aaak", e))?;
            self.client
                .zadd::<(), _, _>(
                    &idx,
                    None,
                    None,
                    false,
                    false,
                    (ts, lesson.lesson_id.as_str()),
                )
                .await
                .map_err(|e| redis_err("ZADD aaak", e))?;
            Ok(())
        })
    }

    fn list_aaak_lessons(
        &self,
        _project_id: &str,
        limit: usize,
    ) -> Result<Vec<AaakLesson>, CoreError> {
        let idx = self.key("aaak_idx");
        block_on(async {
            let ids: Vec<Value> = self
                .client
                .zrevrangebyscore(&idx, "+inf", "-inf", false, Some((0, limit as i64)))
                .await
                .map_err(|e| redis_err("ZREVRANGEBYSCORE aaak", e))?;
            let mut results = Vec::new();
            for id_val in &ids {
                let id = val_to_string(id_val);
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("aaak:{id}"));
                let json_val: Value = self
                    .client
                    .hget(&hk, "json")
                    .await
                    .map_err(|e| redis_err("HGET aaak", e))?;
                let j = val_to_string(&json_val);
                if let Ok(l) = serde_json::from_str::<AaakLesson>(&j) {
                    results.push(l);
                }
            }
            Ok(results)
        })
    }

    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError> {
        let hk = self.key(&format!("aaak:{lesson_id}"));
        let idx = self.key("aaak_idx");
        block_on(async {
            let del: i64 = self
                .client
                .del(&hk)
                .await
                .map_err(|e| redis_err("DEL aaak", e))?;
            self.client
                .zrem::<(), _, _>(&idx, lesson_id)
                .await
                .map_err(|e| redis_err("ZREM aaak", e))?;
            Ok(del > 0)
        })
    }

    // ── Diary ─────────────────────────────────────────────────────

    fn upsert_diary_entry(&self, entry: &DiaryEntry) -> Result<(), CoreError> {
        let hk = self.key(&format!("diary:{}", entry.entry_id));
        let idx = self.key("diary_zidx");
        let tags_json = serde_json::to_string(&entry.tags).unwrap_or_else(|_| "[]".to_string());
        let ts = entry.updated_at_epoch_ms as f64;

        block_on(async {
            self.client
                .hset::<(), _, _>(
                    &hk,
                    [
                        ("entry_id", entry.entry_id.as_str()),
                        ("project_id", entry.project_id.as_str()),
                        ("entry_date", entry.entry_date.as_str()),
                        ("mood", entry.mood.as_deref().unwrap_or("")),
                        ("tags_json", &tags_json),
                        ("content", entry.content.as_str()),
                        (
                            "created_at_epoch_ms",
                            &entry.created_at_epoch_ms.to_string(),
                        ),
                        (
                            "updated_at_epoch_ms",
                            &entry.updated_at_epoch_ms.to_string(),
                        ),
                    ],
                )
                .await
                .map_err(|e| redis_err("HSET diary", e))?;
            self.client
                .zadd::<(), _, _>(
                    &idx,
                    None,
                    None,
                    false,
                    false,
                    (ts, entry.entry_id.as_str()),
                )
                .await
                .map_err(|e| redis_err("ZADD diary", e))?;
            Ok(())
        })
    }

    fn list_diary_entries(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let idx = self.key("diary_zidx");
        block_on(async {
            // Fetch more than limit to account for date filtering.
            let fetch = (limit * 3).max(50);
            let ids: Vec<Value> = self
                .client
                .zrevrangebyscore(&idx, "+inf", "-inf", false, Some((0, fetch as i64)))
                .await
                .map_err(|e| redis_err("ZREVRANGEBYSCORE diary", e))?;

            let mut results = Vec::new();
            for id_val in &ids {
                if results.len() >= limit {
                    break;
                }
                let id = val_to_string(id_val);
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("diary:{id}"));
                if let Some(entry) = read_diary_hash(&self.client, &hk).await? {
                    if let Some(sd) = start_date {
                        if entry.entry_date.as_str() < sd {
                            continue;
                        }
                    }
                    if let Some(ed) = end_date {
                        if entry.entry_date.as_str() > ed {
                            continue;
                        }
                    }
                    results.push(entry);
                }
            }
            Ok(results)
        })
    }

    fn search_diary_entries_in_range(
        &self,
        query: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>, CoreError> {
        let index_name = self.key("diary_idx");
        let escaped = query
            .replace('\\', "\\\\")
            .replace('@', "\\@")
            .replace('{', "\\{")
            .replace('}', "\\}")
            .replace('(', "\\(")
            .replace(')', "\\)")
            .replace('[', "\\[")
            .replace(']', "\\]")
            .replace('|', "\\|")
            .replace('-', "\\-")
            .replace('~', "\\~")
            .replace('!', "\\!")
            .replace('"', "\\\"");

        let ft_query = if escaped.trim().is_empty() {
            "*".to_string()
        } else {
            escaped
        };

        block_on(async {
            let results: Value = self
                .client
                .ft_search(
                    &index_name,
                    &ft_query,
                    FtSearchOptions {
                        limit: Some((0, (limit * 3) as i64)),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| redis_err("FT.SEARCH diary", e))?;

            Ok(parse_ft_diary(&results, start_date, end_date, limit))
        })
    }

    // ── Navigation ────────────────────────────────────────────────

    fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError> {
        let hk = self.key(&format!("nav:{}", node.node_id));
        let idx = self.key("nav_idx");
        let ts = node.updated_at_epoch_ms as f64;
        let json = serde_json::to_string(node).map_err(|e| redis_err("serialize nav", e))?;

        block_on(async {
            self.client
                .hset::<(), _, _>(&hk, [("json", json.as_str())])
                .await
                .map_err(|e| redis_err("HSET nav", e))?;
            self.client
                .zadd::<(), _, _>(&idx, None, None, false, false, (ts, node.node_id.as_str()))
                .await
                .map_err(|e| redis_err("ZADD nav", e))?;
            Ok(())
        })
    }

    fn get_navigation_node(
        &self,
        node_id: &str,
    ) -> Result<Option<MemoryNavigationNode>, CoreError> {
        let hk = self.key(&format!("nav:{node_id}"));
        block_on(async {
            let v: Value = self
                .client
                .hget(&hk, "json")
                .await
                .map_err(|e| redis_err("HGET nav", e))?;
            let j = val_to_string(&v);
            if j.is_empty() {
                return Ok(None);
            }
            let node: MemoryNavigationNode =
                serde_json::from_str(&j).map_err(|e| redis_err("deserialize nav", e))?;
            Ok(Some(node))
        })
    }

    fn list_navigation_nodes_paged(
        &self,
        parent_node_id: Option<&str>,
        kind: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationNode>, CoreError> {
        let idx = self.key("nav_idx");
        let offset = req.offset as i64;
        let fetch = req.limit + 1;

        block_on(async {
            // Fetch extra to handle post-fetch filtering.
            let raw_limit = (fetch * 3).max(50);
            let ids: Vec<Value> = self
                .client
                .zrevrangebyscore(
                    &idx,
                    "+inf",
                    "-inf",
                    false,
                    Some((offset, raw_limit as i64)),
                )
                .await
                .map_err(|e| redis_err("ZREVRANGEBYSCORE nav", e))?;

            let mut items = Vec::new();
            for id_val in &ids {
                if items.len() >= fetch {
                    break;
                }
                let id = val_to_string(id_val);
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("nav:{id}"));
                let v: Value = self
                    .client
                    .hget(&hk, "json")
                    .await
                    .map_err(|e| redis_err("HGET nav", e))?;
                let j = val_to_string(&v);
                if j.is_empty() {
                    continue;
                }
                if let Ok(node) = serde_json::from_str::<MemoryNavigationNode>(&j) {
                    if let Some(p) = parent_node_id {
                        if node.parent_node_id.as_deref() != Some(p) {
                            continue;
                        }
                    }
                    if let Some(k) = kind {
                        if node.kind.as_str() != k {
                            continue;
                        }
                    }
                    items.push(node);
                }
            }

            Ok(Page::from_peek(items, req.limit, req.offset, None))
        })
    }

    fn upsert_navigation_tunnel(&self, tunnel: &MemoryNavigationTunnel) -> Result<(), CoreError> {
        let tunnel_key = format!("{}:{}", tunnel.from_node_id, tunnel.to_node_id);
        let hk = self.key(&format!("tunnel:{tunnel_key}"));
        let from_idx = self.key(&format!("tunnelidx:{}", tunnel.from_node_id));
        let to_idx = self.key(&format!("tunnelidx:{}", tunnel.to_node_id));
        let json = serde_json::to_string(tunnel).map_err(|e| redis_err("serialize tunnel", e))?;

        block_on(async {
            self.client
                .hset::<(), _, _>(&hk, [("json", json.as_str())])
                .await
                .map_err(|e| redis_err("HSET tunnel", e))?;
            self.client
                .sadd::<(), _, _>(&from_idx, hk.as_str())
                .await
                .map_err(|e| redis_err("SADD tunnel from", e))?;
            self.client
                .sadd::<(), _, _>(&to_idx, hk.as_str())
                .await
                .map_err(|e| redis_err("SADD tunnel to", e))?;
            Ok(())
        })
    }

    fn list_navigation_tunnels_paged(
        &self,
        node_id: Option<&str>,
        req: &PageRequest,
    ) -> Result<Page<MemoryNavigationTunnel>, CoreError> {
        block_on(async {
            let keys: Vec<String> = if let Some(nid) = node_id {
                let idx = self.key(&format!("tunnelidx:{nid}"));
                let vals: Vec<Value> = self
                    .client
                    .smembers(&idx)
                    .await
                    .map_err(|e| redis_err("SMEMBERS tunnel", e))?;
                vals.iter()
                    .map(val_to_string)
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                Vec::new()
            };

            let offset = req.offset as usize;
            let mut items = Vec::new();
            for (i, tkey) in keys.iter().enumerate() {
                if i < offset {
                    continue;
                }
                if items.len() > req.limit {
                    break;
                }
                let v: Value = self
                    .client
                    .hget(tkey, "json")
                    .await
                    .map_err(|e| redis_err("HGET tunnel", e))?;
                let j = val_to_string(&v);
                if let Ok(t) = serde_json::from_str::<MemoryNavigationTunnel>(&j) {
                    items.push(t);
                }
            }

            Ok(Page::from_peek(items, req.limit, req.offset, None))
        })
    }

    fn list_navigation_tunnels_for_nodes(
        &self,
        node_ids: &[String],
        limit: usize,
    ) -> Result<Vec<MemoryNavigationTunnel>, CoreError> {
        block_on(async {
            let mut seen = HashSet::new();
            let mut results = Vec::new();
            for nid in node_ids {
                let idx = self.key(&format!("tunnelidx:{nid}"));
                let vals: Vec<Value> = self
                    .client
                    .smembers(&idx)
                    .await
                    .map_err(|e| redis_err("SMEMBERS tunnel", e))?;
                for v in &vals {
                    let tkey = val_to_string(v);
                    if tkey.is_empty() || !seen.insert(tkey.clone()) {
                        continue;
                    }
                    if results.len() >= limit {
                        return Ok(results);
                    }
                    let jv: Value = self
                        .client
                        .hget(&tkey, "json")
                        .await
                        .map_err(|e| redis_err("HGET tunnel", e))?;
                    let j = val_to_string(&jv);
                    if let Ok(t) = serde_json::from_str::<MemoryNavigationTunnel>(&j) {
                        results.push(t);
                    }
                }
            }
            Ok(results)
        })
    }
}

// ---------------------------------------------------------------------------
// Stream / FT.SEARCH parsers
// ---------------------------------------------------------------------------

fn parse_audit_stream(entries: &Value) -> Vec<AuditEvent> {
    let arr = match entries {
        Value::Array(a) => a,
        _ => return Vec::new(),
    };

    let mut events = Vec::new();
    for entry in arr {
        let pair = match entry {
            Value::Array(a) if a.len() >= 2 => a,
            _ => continue,
        };
        let fields = match &pair[1] {
            Value::Array(f) => f,
            Value::Map(m) => {
                // fred may return a Map instead of flat array.
                let ek = map_get(m, "error_kind");
                events.push(AuditEvent {
                    id: 0,
                    event_type: map_get(m, "event_type"),
                    payload_json: map_get(m, "payload"),
                    project_id: map_get(m, "project_id"),
                    outcome: map_get(m, "outcome"),
                    error_kind: if ek.is_empty() { None } else { Some(ek) },
                    created_at_epoch_ms: map_get(m, "ts").parse().unwrap_or(0),
                });
                continue;
            }
            _ => continue,
        };

        let mut evt = AuditEvent {
            id: 0,
            event_type: String::new(),
            payload_json: String::new(),
            project_id: String::new(),
            outcome: String::new(),
            error_kind: None,
            created_at_epoch_ms: 0,
        };
        let mut i = 0;
        while i + 1 < fields.len() {
            let key = val_to_string(&fields[i]);
            let val = val_to_string(&fields[i + 1]);
            match key.as_str() {
                "event_type" => evt.event_type = val,
                "payload" => evt.payload_json = val,
                "outcome" => evt.outcome = val,
                "error_kind" => {
                    if !val.is_empty() {
                        evt.error_kind = Some(val);
                    }
                }
                "ts" => evt.created_at_epoch_ms = val.parse().unwrap_or(0),
                "project_id" => evt.project_id = val,
                _ => {}
            }
            i += 2;
        }
        events.push(evt);
    }
    events
}

async fn read_diary_hash(client: &Client, key: &str) -> Result<Option<DiaryEntry>, CoreError> {
    let vals: Value = client
        .hgetall(key)
        .await
        .map_err(|e| redis_err("HGETALL diary", e))?;

    let m = match &vals {
        Value::Map(m) if !m.is_empty() => m,
        _ => return Ok(None),
    };

    let tags: Vec<String> = serde_json::from_str(&map_get(m, "tags_json")).unwrap_or_default();
    let mood_raw = map_get(m, "mood");

    Ok(Some(DiaryEntry {
        entry_id: map_get(m, "entry_id"),
        project_id: map_get(m, "project_id"),
        entry_date: map_get(m, "entry_date"),
        mood: if mood_raw.is_empty() {
            None
        } else {
            Some(mood_raw)
        },
        tags,
        content: map_get(m, "content"),
        created_at_epoch_ms: map_get(m, "created_at_epoch_ms").parse().unwrap_or(0),
        updated_at_epoch_ms: map_get(m, "updated_at_epoch_ms").parse().unwrap_or(0),
    }))
}

fn parse_ft_diary(
    results: &Value,
    start_date: Option<&str>,
    end_date: Option<&str>,
    limit: usize,
) -> Vec<DiaryEntry> {
    let arr = match results {
        Value::Array(a) if a.len() >= 2 => a,
        _ => return Vec::new(),
    };

    let mut entries = Vec::new();
    let mut i = 1;
    while i + 1 < arr.len() && entries.len() < limit {
        // arr[i] = key, arr[i+1] = fields (flat array or map)
        let fields = &arr[i + 1];
        let get = |k: &str| -> String {
            match fields {
                Value::Array(f) => {
                    let mut j = 0;
                    while j + 1 < f.len() {
                        if val_to_string(&f[j]) == k {
                            return val_to_string(&f[j + 1]);
                        }
                        j += 2;
                    }
                    String::new()
                }
                Value::Map(m) => map_get(m, k),
                _ => String::new(),
            }
        };

        let entry_date = get("entry_date");
        let in_range = start_date.is_none_or(|sd| entry_date.as_str() >= sd)
            && end_date.is_none_or(|ed| entry_date.as_str() <= ed);

        let entry_id = get("entry_id");
        if in_range && !entry_id.is_empty() {
            let tags: Vec<String> = serde_json::from_str(&get("tags_json")).unwrap_or_default();
            let mood_raw = get("mood");
            entries.push(DiaryEntry {
                entry_id,
                project_id: get("project_id"),
                entry_date,
                mood: if mood_raw.is_empty() {
                    None
                } else {
                    Some(mood_raw)
                },
                tags,
                content: get("content"),
                created_at_epoch_ms: get("created_at_epoch_ms").parse().unwrap_or(0),
                updated_at_epoch_ms: get("updated_at_epoch_ms").parse().unwrap_or(0),
            });
        }
        i += 2;
    }
    entries
}
