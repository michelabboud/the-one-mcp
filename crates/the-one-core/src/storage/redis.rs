//! Redis StateStore backend (v0.16.0 Phase 5, migrated to the-one-redis
//! facade in v0.17.0).
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
//!
//! ## Substrate
//!
//! Built on [`the_one_redis::RedisPool`] (redis-rs 1.2), not `fred`.
//! See `docs/plans/2026-05-16-fred-removal-and-bug-fixes.md` for the
//! migration history; the load-bearing fix is in
//! `the_one_redis::pool::connection_config` which sets
//! `response_timeout = None` so blocking commands aren't capped at
//! 500ms.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use the_one_redis::search::{CreateOptions, Query, SchemaField};
use the_one_redis::{PoolConfig, RedisPool};

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

/// Escape a user-supplied query string for RediSearch FT.SEARCH.
///
/// Covers the full RediSearch special-character set including `*`
/// (wildcard) and `,` (term delimiter in tag/numeric filters) — both
/// previously missed, which let unescaped user input flip a diary
/// search into a wildcard or split a single term into a multi-term
/// query.
fn redis_query_escape(input: &str) -> String {
    input
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
        .replace('*', "\\*")
        .replace(',', "\\,")
        .replace('"', "\\\"")
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

fn parse_conv_hash(m: &HashMap<String, String>) -> Option<ConversationSourceRecord> {
    if m.is_empty() {
        return None;
    }
    let wing = m.get("wing").cloned().filter(|s| !s.is_empty());
    let hall = m.get("hall").cloned().filter(|s| !s.is_empty());
    let room = m.get("room").cloned().filter(|s| !s.is_empty());

    Some(ConversationSourceRecord {
        project_id: m.get("project_id").cloned().unwrap_or_default(),
        transcript_path: m.get("transcript_path").cloned().unwrap_or_default(),
        memory_path: m.get("memory_path").cloned().unwrap_or_default(),
        format: m.get("format").cloned().unwrap_or_default(),
        wing,
        hall,
        room,
        message_count: m
            .get("message_count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// RedisStateStore
// ---------------------------------------------------------------------------

pub struct RedisStateStore {
    pool: RedisPool,
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
        let pool = RedisPool::new(PoolConfig::from_url(url))
            .await
            .map_err(|e| redis_err("pool init", e))?;

        let prefix = format!("{}:{}", config.prefix, project_id);

        if config.require_aof {
            verify_aof(&pool).await?;
        }

        let store = Self {
            pool,
            project_id: project_id.to_string(),
            prefix,
            config: config.clone(),
        };

        store.ensure_diary_index().await?;
        Ok(store)
    }

    /// Phase 6 — build from an already-connected pool.
    pub async fn from_pool(
        pool: RedisPool,
        config: &RedisStateConfig,
        project_id: &str,
    ) -> Result<Self, CoreError> {
        let prefix = format!("{}:{}", config.prefix, project_id);
        let store = Self {
            pool,
            project_id: project_id.to_string(),
            prefix,
            config: config.clone(),
        };
        store.ensure_diary_index().await?;
        Ok(store)
    }

    pub fn pool(&self) -> &RedisPool {
        &self.pool
    }

    pub async fn close(&self) {
        // redis-rs's multiplexed connection cleans up on Drop; no explicit
        // close needed. Kept for API compatibility with the previous
        // fred-based version.
    }

    /// Build a fully-qualified Redis key as `{prefix}:{project_id}:{suffix}`.
    ///
    /// **Invariant:** `self.prefix` was constructed at `new()` from a
    /// sanitised `project_id` (see `the_one_core::naming::sanitize_project_id`,
    /// which forbids `:`). Callers MUST route every key through this method
    /// so the namespace separator stays unambiguous.
    fn key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.prefix)
    }

    async fn ensure_diary_index(&self) -> Result<(), CoreError> {
        let index_name = self.key("diary_idx");
        let prefix = format!("{}:diary:", self.prefix);

        // If FT.INFO succeeds, the index exists.
        if self.pool.search().ft_info(&index_name).await.is_ok() {
            return Ok(());
        }

        let opts = CreateOptions {
            on_json: false,
            prefixes: vec![prefix],
        };

        let schema = vec![
            SchemaField::Text {
                name: "content".into(),
            },
            SchemaField::Tag {
                name: "entry_date".into(),
                separator: None,
            },
        ];

        self.pool
            .search()
            .ft_create(&index_name, &opts, &schema)
            .await
            .map_err(|e| redis_err("FT.CREATE diary_idx", e))?;
        Ok(())
    }
}

/// Verify that the Redis server has AOF persistence enabled.
///
/// Single source of truth for the AOF check — also called by
/// `the-one-mcp`'s combined-Redis pool cache so both code paths
/// agree on what counts as "AOF on".
///
/// **Parse failure is treated as an error**, not as `aof_enabled=false`.
/// A malformed `INFO persistence` response should never silently
/// downgrade a `require_aof=true` deployment to cache mode.
pub async fn verify_aof(pool: &RedisPool) -> Result<(), CoreError> {
    let info: String = pool
        .raw_cmd(&["INFO", "persistence"])
        .await
        .map_err(|e| redis_err("INFO persistence", e))?;

    let aof_enabled = info
        .lines()
        .find(|l| l.starts_with("aof_enabled:"))
        .and_then(|l| l.strip_prefix("aof_enabled:"))
        .map(|v| v.trim() == "1")
        .ok_or_else(|| {
            CoreError::Redis(
                "INFO persistence response missing aof_enabled line — \
                 cannot verify durability"
                    .to_string(),
            )
        })?;

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
        block_on(async {
            self.pool
                .keys()
                .set(&key, profile_json)
                .await
                .map_err(|e| redis_err("SET profile", e))
        })
    }

    fn latest_project_profile(&self) -> Result<Option<String>, CoreError> {
        let key = self.key("profile");
        block_on(async {
            self.pool
                .keys()
                .get::<String>(&key)
                .await
                .map_err(|e| redis_err("GET profile", e))
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
                self.pool
                    .keys()
                    .set(&key, "1")
                    .await
                    .map_err(|e| redis_err("SET approval", e))
            } else {
                self.pool
                    .keys()
                    .del(&key)
                    .await
                    .map_err(|e| redis_err("DEL approval", e))?;
                Ok(())
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
            self.pool
                .keys()
                .exists(&key)
                .await
                .map_err(|e| redis_err("EXISTS approval", e))
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
            self.pool
                .streams()
                .xadd(&key, &fields, None)
                .await
                .map(|_| ())
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
            self.pool
                .streams()
                .xadd(&key, &fields, None)
                .await
                .map(|_| ())
                .map_err(|e| redis_err("XADD audit", e))
        })
    }

    fn audit_event_count_for_project(&self) -> Result<u64, CoreError> {
        let key = self.key("audit");
        block_on(async {
            self.pool
                .streams()
                .xlen(&key)
                .await
                .map_err(|e| redis_err("XLEN audit", e))
        })
    }

    fn list_audit_events_paged(&self, req: &PageRequest) -> Result<Page<AuditEvent>, CoreError> {
        let key = self.key("audit");
        let limit = req.limit;
        // Use offset-based pagination via XREVRANGE with COUNT.
        // For simplicity, fetch all and skip to offset.
        block_on(async {
            let count = (req.offset as usize) + limit + 1;
            let entries = self
                .pool
                .streams()
                .xrevrange(&key, "+", "-", Some(count))
                .await
                .map_err(|e| redis_err("XREVRANGE audit", e))?;

            let all = entries_to_audit(&entries);
            let offset = req.offset as usize;
            let slice: Vec<AuditEvent> = all.into_iter().skip(offset).take(limit + 1).collect();

            let page = Page::from_peek(slice, limit, req.offset, None);
            Ok(page)
        })
    }

    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, CoreError> {
        let key = self.key("audit");
        block_on(async {
            let entries = self
                .pool
                .streams()
                .xrevrange(&key, "+", "-", Some(limit))
                .await
                .map_err(|e| redis_err("XREVRANGE audit", e))?;
            Ok(entries_to_audit(&entries))
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

        let mc_str = record.message_count.to_string();
        let fields: [(&str, &[u8]); 8] = [
            ("project_id", record.project_id.as_bytes()),
            ("transcript_path", record.transcript_path.as_bytes()),
            ("memory_path", record.memory_path.as_bytes()),
            ("format", record.format.as_bytes()),
            ("wing", record.wing.as_deref().unwrap_or("").as_bytes()),
            ("hall", record.hall.as_deref().unwrap_or("").as_bytes()),
            ("room", record.room.as_deref().unwrap_or("").as_bytes()),
            ("message_count", mc_str.as_bytes()),
        ];

        block_on(async {
            self.pool
                .hashes()
                .hset_multi(&hk, &fields)
                .await
                .map_err(|e| redis_err("HSET conv", e))?;
            self.pool
                .sorted_sets()
                .zadd(&idx, ts, conv_id.as_str())
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
            // zrevrange with rank-based pagination is equivalent to
            // zrevrangebyscore "+inf" "-inf" LIMIT 0 N since we never
            // filter by score.
            let ids: Vec<String> = self
                .pool
                .sorted_sets()
                .zrevrange(&idx, 0, (limit as isize) - 1)
                .await
                .map_err(|e| redis_err("ZREVRANGE conv", e))?;

            let mut results = Vec::new();
            for id in &ids {
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("conv:{id}"));
                let vals = self
                    .pool
                    .hashes()
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
            self.pool
                .hashes()
                .hset(&hk, "json", json.as_str())
                .await
                .map_err(|e| redis_err("HSET aaak", e))?;
            self.pool
                .sorted_sets()
                .zadd(&idx, ts, lesson.lesson_id.as_str())
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
            let ids: Vec<String> = self
                .pool
                .sorted_sets()
                .zrevrange(&idx, 0, (limit as isize) - 1)
                .await
                .map_err(|e| redis_err("ZREVRANGE aaak", e))?;
            let mut results = Vec::new();
            for id in &ids {
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("aaak:{id}"));
                let j: Option<String> = self
                    .pool
                    .hashes()
                    .hget(&hk, "json")
                    .await
                    .map_err(|e| redis_err("HGET aaak", e))?;
                if let Some(j) = j {
                    if let Ok(l) = serde_json::from_str::<AaakLesson>(&j) {
                        results.push(l);
                    }
                }
            }
            Ok(results)
        })
    }

    fn delete_aaak_lesson(&self, lesson_id: &str) -> Result<bool, CoreError> {
        let hk = self.key(&format!("aaak:{lesson_id}"));
        let idx = self.key("aaak_idx");
        block_on(async {
            let del = self
                .pool
                .keys()
                .del(&hk)
                .await
                .map_err(|e| redis_err("DEL aaak", e))?;
            self.pool
                .sorted_sets()
                .zrem(&idx, lesson_id)
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

        let created = entry.created_at_epoch_ms.to_string();
        let updated = entry.updated_at_epoch_ms.to_string();
        let fields: [(&str, &[u8]); 8] = [
            ("entry_id", entry.entry_id.as_bytes()),
            ("project_id", entry.project_id.as_bytes()),
            ("entry_date", entry.entry_date.as_bytes()),
            ("mood", entry.mood.as_deref().unwrap_or("").as_bytes()),
            ("tags_json", tags_json.as_bytes()),
            ("content", entry.content.as_bytes()),
            ("created_at_epoch_ms", created.as_bytes()),
            ("updated_at_epoch_ms", updated.as_bytes()),
        ];

        block_on(async {
            self.pool
                .hashes()
                .hset_multi(&hk, &fields)
                .await
                .map_err(|e| redis_err("HSET diary", e))?;
            self.pool
                .sorted_sets()
                .zadd(&idx, ts, entry.entry_id.as_str())
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
            let ids: Vec<String> = self
                .pool
                .sorted_sets()
                .zrevrange(&idx, 0, (fetch as isize) - 1)
                .await
                .map_err(|e| redis_err("ZREVRANGE diary", e))?;

            let mut results = Vec::new();
            for id in &ids {
                if results.len() >= limit {
                    break;
                }
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("diary:{id}"));
                if let Some(entry) = read_diary_hash(&self.pool, &hk).await? {
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
        let escaped = redis_query_escape(query);

        let ft_query = if escaped.trim().is_empty() {
            "*".to_string()
        } else {
            escaped
        };

        block_on(async {
            let q = Query::new(ft_query).limit(0, limit * 3);
            let reply = self
                .pool
                .search()
                .ft_search(&index_name, &q)
                .await
                .map_err(|e| redis_err("FT.SEARCH diary", e))?;

            Ok(reply_to_diary(&reply, start_date, end_date, limit))
        })
    }

    // ── Navigation ────────────────────────────────────────────────

    fn upsert_navigation_node(&self, node: &MemoryNavigationNode) -> Result<(), CoreError> {
        let hk = self.key(&format!("nav:{}", node.node_id));
        let idx = self.key("nav_idx");
        let ts = node.updated_at_epoch_ms as f64;
        let json = serde_json::to_string(node).map_err(|e| redis_err("serialize nav", e))?;

        block_on(async {
            self.pool
                .hashes()
                .hset(&hk, "json", json.as_str())
                .await
                .map_err(|e| redis_err("HSET nav", e))?;
            self.pool
                .sorted_sets()
                .zadd(&idx, ts, node.node_id.as_str())
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
            let j: Option<String> = self
                .pool
                .hashes()
                .hget(&hk, "json")
                .await
                .map_err(|e| redis_err("HGET nav", e))?;
            let Some(j) = j else { return Ok(None) };
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
        let offset = req.offset as isize;
        let fetch = req.limit + 1;

        block_on(async {
            // Fetch extra to handle post-fetch filtering.
            let raw_limit = (fetch * 3).max(50);
            let ids: Vec<String> = self
                .pool
                .sorted_sets()
                .zrevrange(&idx, offset, offset + (raw_limit as isize) - 1)
                .await
                .map_err(|e| redis_err("ZREVRANGE nav", e))?;

            let mut items = Vec::new();
            for id in &ids {
                if items.len() >= fetch {
                    break;
                }
                if id.is_empty() {
                    continue;
                }
                let hk = self.key(&format!("nav:{id}"));
                let j: Option<String> = self
                    .pool
                    .hashes()
                    .hget(&hk, "json")
                    .await
                    .map_err(|e| redis_err("HGET nav", e))?;
                let Some(j) = j else { continue };
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
            self.pool
                .hashes()
                .hset(&hk, "json", json.as_str())
                .await
                .map_err(|e| redis_err("HSET tunnel", e))?;
            self.pool
                .sets()
                .sadd(&from_idx, hk.as_str())
                .await
                .map_err(|e| redis_err("SADD tunnel from", e))?;
            self.pool
                .sets()
                .sadd(&to_idx, hk.as_str())
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
                self.pool
                    .sets()
                    .smembers::<String>(&idx)
                    .await
                    .map_err(|e| redis_err("SMEMBERS tunnel", e))?
                    .into_iter()
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
                let j: Option<String> = self
                    .pool
                    .hashes()
                    .hget(tkey, "json")
                    .await
                    .map_err(|e| redis_err("HGET tunnel", e))?;
                if let Some(j) = j {
                    if let Ok(t) = serde_json::from_str::<MemoryNavigationTunnel>(&j) {
                        items.push(t);
                    }
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
                let vals: Vec<String> = self
                    .pool
                    .sets()
                    .smembers(&idx)
                    .await
                    .map_err(|e| redis_err("SMEMBERS tunnel", e))?;
                for tkey in vals {
                    if tkey.is_empty() || !seen.insert(tkey.clone()) {
                        continue;
                    }
                    if results.len() >= limit {
                        return Ok(results);
                    }
                    let j: Option<String> = self
                        .pool
                        .hashes()
                        .hget(&tkey, "json")
                        .await
                        .map_err(|e| redis_err("HGET tunnel", e))?;
                    if let Some(j) = j {
                        if let Ok(t) = serde_json::from_str::<MemoryNavigationTunnel>(&j) {
                            results.push(t);
                        }
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

/// Build [`AuditEvent`] values from a Vec<StreamEntry>. Each StreamEntry has
/// a typed `HashMap<String, String>` of fields, so we just look up by name.
fn entries_to_audit(entries: &[the_one_redis::streams::StreamEntry]) -> Vec<AuditEvent> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let m = &entry.fields;
        let ek = m.get("error_kind").cloned().unwrap_or_default();
        out.push(AuditEvent {
            id: 0,
            event_type: m.get("event_type").cloned().unwrap_or_default(),
            payload_json: m.get("payload").cloned().unwrap_or_default(),
            project_id: m.get("project_id").cloned().unwrap_or_default(),
            outcome: m.get("outcome").cloned().unwrap_or_default(),
            error_kind: if ek.is_empty() { None } else { Some(ek) },
            created_at_epoch_ms: m.get("ts").and_then(|v| v.parse().ok()).unwrap_or(0),
        });
    }
    out
}

async fn read_diary_hash(pool: &RedisPool, key: &str) -> Result<Option<DiaryEntry>, CoreError> {
    let m = pool
        .hashes()
        .hgetall(key)
        .await
        .map_err(|e| redis_err("HGETALL diary", e))?;

    if m.is_empty() {
        return Ok(None);
    }

    let tags: Vec<String> = m
        .get("tags_json")
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default();
    let mood = m.get("mood").cloned().filter(|s| !s.is_empty());

    Ok(Some(DiaryEntry {
        entry_id: m.get("entry_id").cloned().unwrap_or_default(),
        project_id: m.get("project_id").cloned().unwrap_or_default(),
        entry_date: m.get("entry_date").cloned().unwrap_or_default(),
        mood,
        tags,
        content: m.get("content").cloned().unwrap_or_default(),
        created_at_epoch_ms: m
            .get("created_at_epoch_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        updated_at_epoch_ms: m
            .get("updated_at_epoch_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    }))
}

/// Parse FT.SEARCH reply into DiaryEntry values, applying date-range filter
/// after-the-fact and stopping at `limit` matches.
fn reply_to_diary(
    reply: &the_one_redis::search::SearchReply,
    start_date: Option<&str>,
    end_date: Option<&str>,
    limit: usize,
) -> Vec<DiaryEntry> {
    let mut entries = Vec::new();
    for hit in &reply.hits {
        if entries.len() >= limit {
            break;
        }
        let get = |k: &str| -> String {
            hit.fields
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
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
    }
    entries
}
