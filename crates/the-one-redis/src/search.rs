//! RediSearch operations: `FT.CREATE`, `FT.SEARCH`, `FT.INFO`, `FT.ALTER`,
//! `FT.DROPINDEX`.
//!
//! ## Context
//!
//! Starting with Redis 8, RediSearch is integral to Redis core (no
//! separate module load). the-one-mcp targets Redis 8.6.2, so all FT.* commands are
//! first-class commands on the wire.
//!
//! `redis-rs` 1.2 doesn't ship typed FT.* wrappers (unlike `fred`'s
//! `i-redisearch` feature or `rustis`'s built-in support), so this
//! module owns the typed surface for the-one-mcp. The shape mirrors `redis-py`'s
//! `redis.commands.search` package — `VectorField` / `TextField` /
//! `NumericField` / `TagField` builders for FT.CREATE, a `Query` struct
//! for FT.SEARCH including KNN syntax, and a `SearchHits` struct for
//! parsing replies.
//!
//! ## What this module deliberately does NOT do
//!
//! - It does NOT try to model every FT.SEARCH option. Hybrid filters,
//!   summarize, highlight, GROUPBY, REDUCE etc. are passed through as
//!   raw strings on `Query::base_query`. We add typed methods only when
//!   3+ call sites want the same shape.
//! - It does NOT abstract the FT.SEARCH response shape into a generic
//!   "result type" that loses information. The reply is returned as a
//!   typed `Vec<SearchHit>` plus the raw [`redis::Value`] for callers
//!   that need to inspect it.

use tracing::{instrument, warn};

use crate::error::{RedisError, RedisResult};
use crate::pool::RedisPool;

/// Handle for RediSearch operations. Returned by [`RedisPool::search`].
pub struct SearchOps<'a> {
    pool: &'a RedisPool,
}

// ── Index schema types (FT.CREATE) ──────────────────────────────────────

/// Vector index algorithm.
#[derive(Debug, Clone, Copy)]
pub enum VectorAlgorithm {
    /// Exhaustive flat search. Best for <1M vectors / perfect recall.
    Flat,
    /// Hierarchical Navigable Small World — approximate, scales further.
    Hnsw,
}

impl VectorAlgorithm {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Flat => "FLAT",
            Self::Hnsw => "HNSW",
        }
    }
}

/// Vector distance metric.
#[derive(Debug, Clone, Copy)]
pub enum DistanceMetric {
    /// Cosine similarity. the-one-mcp's default — see crates/the-one-memory.
    Cosine,
    /// L2 Euclidean.
    L2,
    /// Inner product.
    Ip,
}

impl DistanceMetric {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Cosine => "COSINE",
            Self::L2 => "L2",
            Self::Ip => "IP",
        }
    }
}

/// One field in an FT.CREATE schema. Mirrors `redis.commands.search.field`
/// from redis-py.
#[derive(Debug, Clone)]
pub enum SchemaField {
    /// Full-text searchable field.
    Text {
        /// Field name within the hash/JSON document.
        name: String,
    },
    /// Numeric field — supports range filters.
    Numeric {
        /// Field name.
        name: String,
        /// Whether this field can be filtered/sorted.
        sortable: bool,
    },
    /// Tag field — exact-match filters with delimiter splitting.
    Tag {
        /// Field name.
        name: String,
        /// Optional `SEPARATOR c` to split the stored string into multiple
        /// tag values. Defaults to `,` in RediSearch when omitted; pass
        /// `Some(',')` explicitly only when the behaviour needs to be
        /// pinned in the index schema (e.g. `"foo,bar,baz"` fields).
        separator: Option<char>,
    },
    /// Vector field for KNN search.
    Vector {
        /// Field name.
        name: String,
        /// FLAT or HNSW.
        algorithm: VectorAlgorithm,
        /// Vector dimensionality (e.g. 1536 for OpenAI embeddings).
        dim: usize,
        /// Cosine, L2, or Inner Product.
        distance_metric: DistanceMetric,
        /// Initial capacity hint (FLAT only).
        initial_cap: Option<usize>,
    },
}

impl SchemaField {
    fn append_args(&self, cmd: &mut redis::Cmd) {
        match self {
            Self::Text { name } => {
                cmd.arg(name).arg("TEXT");
            }
            Self::Numeric { name, sortable } => {
                cmd.arg(name).arg("NUMERIC");
                if *sortable {
                    cmd.arg("SORTABLE");
                }
            }
            Self::Tag { name, separator } => {
                cmd.arg(name).arg("TAG");
                if let Some(sep) = separator {
                    cmd.arg("SEPARATOR").arg(sep.to_string());
                }
            }
            Self::Vector {
                name,
                algorithm,
                dim,
                distance_metric,
                initial_cap,
            } => {
                let mut attr_count = 6_usize; // TYPE FLOAT32 DIM N DISTANCE_METRIC X
                if initial_cap.is_some() {
                    attr_count += 2;
                }
                cmd.arg(name)
                    .arg("VECTOR")
                    .arg(algorithm.as_str())
                    .arg(attr_count)
                    .arg("TYPE")
                    .arg("FLOAT32")
                    .arg("DIM")
                    .arg(*dim)
                    .arg("DISTANCE_METRIC")
                    .arg(distance_metric.as_str());
                if let Some(cap) = initial_cap {
                    cmd.arg("INITIAL_CAP").arg(*cap);
                }
            }
        }
    }
}

/// FT.CREATE options.
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// Storage type — `HASH` (default) or `JSON`.
    pub on_json: bool,
    /// Key prefixes to index. Empty = index all keys (rarely what you want).
    pub prefixes: Vec<String>,
}

// ── Query types (FT.SEARCH) ─────────────────────────────────────────────

/// A FT.SEARCH query. Build with [`Query::new`] then chain modifiers.
///
/// `base_query` is the raw query string — typically `*` or `*=>[KNN k @field $vec AS score]`
/// for vector search. Hybrid filters go in the base_query (e.g.
/// `@title:Scottish=>[KNN ...]`). The `params` map is the `PARAMS` block
/// — typically holds the binary vector for KNN.
#[derive(Debug, Clone)]
pub struct Query {
    /// The raw FT.SEARCH query string.
    pub base_query: String,
    /// Fields to RETURN. Empty = return everything.
    pub return_fields: Vec<String>,
    /// Sort by field (typically `vector_score` for KNN).
    pub sort_by: Option<String>,
    /// LIMIT offset count.
    pub limit: Option<(usize, usize)>,
    /// PARAMS map — values are bytes (typical for vector queries).
    pub params: Vec<(String, Vec<u8>)>,
    /// DIALECT n — required >= 2 for vector search.
    pub dialect: Option<u8>,
}

impl Query {
    /// New query with the given base query string.
    pub fn new(base_query: impl Into<String>) -> Self {
        Self {
            base_query: base_query.into(),
            return_fields: Vec::new(),
            sort_by: None,
            limit: None,
            params: Vec::new(),
            dialect: Some(2),
        }
    }

    /// Add `RETURN n field1 field2 ...`.
    pub fn return_fields<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.return_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Sort the result set by the named (returned) field.
    pub fn sort_by(mut self, field: impl Into<String>) -> Self {
        self.sort_by = Some(field.into());
        self
    }

    /// LIMIT offset count.
    pub fn limit(mut self, offset: usize, count: usize) -> Self {
        self.limit = Some((offset, count));
        self
    }

    /// Add a PARAMS entry. Pass binary vectors here for KNN.
    pub fn param(mut self, name: impl Into<String>, value: Vec<u8>) -> Self {
        self.params.push((name.into(), value));
        self
    }

    /// Set DIALECT (>= 2 required for vector search; default is 2).
    pub fn dialect(mut self, n: u8) -> Self {
        self.dialect = Some(n);
        self
    }
}

// ── Reply types ─────────────────────────────────────────────────────────

/// One hit returned by FT.SEARCH.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// The Redis key of the matching document.
    pub key: String,
    /// Field-value pairs from RETURN. Empty if no RETURN was specified
    /// (the caller asked for keys only).
    pub fields: Vec<(String, String)>,
}

/// FT.SEARCH reply: total count + hit list. The raw `redis::Value` is
/// preserved for callers that need to inspect the protocol-level shape
/// (e.g. for hybrid scoring).
#[derive(Debug, Clone)]
pub struct SearchReply {
    /// Total number of matching documents BEFORE LIMIT.
    pub total: u64,
    /// Hit list, length capped by LIMIT.
    pub hits: Vec<SearchHit>,
    /// The raw `redis::Value` — exposed for callers that need to do
    /// custom parsing of vector_score etc.
    pub raw: redis::Value,
}

// ── SearchOps impl ──────────────────────────────────────────────────────

impl<'a> SearchOps<'a> {
    pub(crate) fn new(pool: &'a RedisPool) -> Self {
        Self { pool }
    }

    /// `FT.CREATE index ON {HASH|JSON} PREFIX n p1 p2 ... SCHEMA ...`.
    #[instrument(skip(self, fields), level = "debug", fields(field_count = fields.len()))]
    pub async fn ft_create(
        &self,
        index_name: &str,
        opts: &CreateOptions,
        fields: &[SchemaField],
    ) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("FT.CREATE");
        cmd.arg(index_name);
        cmd.arg("ON")
            .arg(if opts.on_json { "JSON" } else { "HASH" });
        if !opts.prefixes.is_empty() {
            cmd.arg("PREFIX").arg(opts.prefixes.len());
            for p in &opts.prefixes {
                cmd.arg(p);
            }
        }
        cmd.arg("SCHEMA");
        for f in fields {
            f.append_args(&mut cmd);
        }
        let _: () = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `FT.INFO index`. Returns the raw [`redis::Value`] — the FT.INFO
    /// reply shape varies by Redis Stack version, so we don't pre-parse.
    /// Callers can extract `num_docs`, `index_definition`, etc. via
    /// shape-matching on the reply.
    #[instrument(skip(self), level = "trace")]
    pub async fn ft_info(&self, index_name: &str) -> RedisResult<redis::Value> {
        let mut conn = self.pool.conn();
        redis::cmd("FT.INFO")
            .arg(index_name)
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)
    }

    /// `FT.DROPINDEX index [DD]` — drops the index. With `delete_docs =
    /// true`, also deletes the indexed documents.
    #[instrument(skip(self), level = "debug")]
    pub async fn ft_dropindex(&self, index_name: &str, delete_docs: bool) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("FT.DROPINDEX");
        cmd.arg(index_name);
        if delete_docs {
            cmd.arg("DD");
        }
        let _: () = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `FT.ALTER index SCHEMA ADD field args...` — append a new field to
    /// an existing index.
    #[instrument(skip(self), level = "debug")]
    pub async fn ft_alter_add(&self, index_name: &str, new_field: &SchemaField) -> RedisResult<()> {
        let mut conn = self.pool.conn();
        let mut cmd = redis::cmd("FT.ALTER");
        cmd.arg(index_name).arg("SCHEMA").arg("ADD");
        new_field.append_args(&mut cmd);
        let _: () = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;
        Ok(())
    }

    /// `FT.SEARCH index query [PARAMS ...] [DIALECT n] [LIMIT offset count]`.
    /// Parses the reply into [`SearchReply`].
    #[instrument(skip(self, query), level = "debug", fields(index = %index_name))]
    pub async fn ft_search(&self, index_name: &str, query: &Query) -> RedisResult<SearchReply> {
        let mut conn = self.pool.conn();
        let cmd = build_ft_search_cmd(index_name, query, false);

        let raw: redis::Value = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;

        parse_ft_search_reply(raw)
    }

    /// `FT.SEARCH index query NOCONTENT ...` — key-only search. Useful for
    /// sweep / backfill / migration paths where the caller only needs the
    /// matching hash keys, not the stored fields. Returns
    /// `(total, keys)`.
    #[instrument(skip(self, query), level = "debug", fields(index = %index_name))]
    pub async fn ft_search_keys(
        &self,
        index_name: &str,
        query: &Query,
    ) -> RedisResult<(u64, Vec<String>)> {
        let mut conn = self.pool.conn();
        let cmd = build_ft_search_cmd(index_name, query, true);

        let raw: redis::Value = cmd
            .query_async(&mut conn)
            .await
            .map_err(RedisError::Command)?;

        parse_ft_search_keys_reply(raw)
    }

    /// Borrow a raw connection for callers that need to issue commands
    /// the typed wrappers don't yet cover. Use sparingly — every direct
    /// use here is a hint that the wrapper layer is incomplete.
    pub fn raw(&self) -> redis::aio::MultiplexedConnection {
        self.pool.conn()
    }
}

/// Build an `FT.SEARCH` redis::Cmd from a [`Query`]. When `no_content` is
/// true emits `NOCONTENT`; the caller is responsible for parsing the
/// resulting flat key list via [`parse_ft_search_keys_reply`].
fn build_ft_search_cmd(index_name: &str, query: &Query, no_content: bool) -> redis::Cmd {
    let mut cmd = redis::cmd("FT.SEARCH");
    cmd.arg(index_name).arg(&query.base_query);
    if no_content {
        cmd.arg("NOCONTENT");
    }

    if !query.return_fields.is_empty() {
        cmd.arg("RETURN").arg(query.return_fields.len());
        for f in &query.return_fields {
            cmd.arg(f);
        }
    }
    if let Some(field) = &query.sort_by {
        cmd.arg("SORTBY").arg(field);
    }
    if let Some((offset, count)) = query.limit {
        cmd.arg("LIMIT").arg(offset).arg(count);
    }
    if !query.params.is_empty() {
        cmd.arg("PARAMS").arg(query.params.len() * 2);
        for (name, value) in &query.params {
            cmd.arg(name).arg(value.as_slice());
        }
    }
    if let Some(d) = query.dialect {
        cmd.arg("DIALECT").arg(d);
    }
    cmd
}

/// Parse an `FT.SEARCH ... NOCONTENT` reply shape:
///   [total_count, key1, key2, key3, ...]
fn parse_ft_search_keys_reply(raw: redis::Value) -> RedisResult<(u64, Vec<String>)> {
    let arr = match raw {
        redis::Value::Array(a) => a,
        other => {
            return Err(RedisError::ReplyParse(format!(
                "ft_search_keys: expected Array, got {other:?}"
            )));
        }
    };
    if arr.is_empty() {
        return Ok((0, Vec::new()));
    }

    let total = match &arr[0] {
        redis::Value::Int(n) => *n as u64,
        other => {
            return Err(RedisError::ReplyParse(format!(
                "ft_search_keys: expected Int as first element, got {other:?}"
            )));
        }
    };

    let mut keys = Vec::with_capacity(arr.len().saturating_sub(1));
    for item in arr.into_iter().skip(1) {
        match item {
            redis::Value::BulkString(b) => keys.push(String::from_utf8_lossy(&b).into_owned()),
            redis::Value::SimpleString(s) => keys.push(s),
            other => {
                warn!(?other, "ft_search_keys: unexpected key shape, skipping");
            }
        }
    }
    Ok((total, keys))
}

/// Parse FT.SEARCH reply shape:
///   [total_count, key1, [field1, value1, field2, value2, ...], key2, [...], ...]
fn parse_ft_search_reply(raw: redis::Value) -> RedisResult<SearchReply> {
    let arr = match &raw {
        redis::Value::Array(a) => a.clone(),
        _ => {
            return Err(RedisError::ReplyParse(format!(
                "expected Array, got {raw:?}"
            )));
        }
    };
    if arr.is_empty() {
        return Ok(SearchReply {
            total: 0,
            hits: Vec::new(),
            raw,
        });
    }

    let total = match &arr[0] {
        redis::Value::Int(n) => *n as u64,
        other => {
            return Err(RedisError::ReplyParse(format!(
                "expected Int as first element, got {other:?}"
            )));
        }
    };

    let mut hits = Vec::new();
    let mut i = 1;
    while i < arr.len() {
        let key = match &arr[i] {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
            redis::Value::SimpleString(s) => s.clone(),
            other => {
                warn!(?other, "ft_search: unexpected key shape, skipping");
                i += 1;
                continue;
            }
        };
        let fields = if i + 1 < arr.len() {
            match &arr[i + 1] {
                redis::Value::Array(pairs) => parse_field_pairs(pairs),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        hits.push(SearchHit { key, fields });
        i += 2;
    }

    Ok(SearchReply { total, hits, raw })
}

fn parse_field_pairs(pairs: &[redis::Value]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < pairs.len() {
        let name = match &pairs[i] {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
            redis::Value::SimpleString(s) => s.clone(),
            _ => {
                i += 2;
                continue;
            }
        };
        let value = match &pairs[i + 1] {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
            redis::Value::SimpleString(s) => s.clone(),
            redis::Value::Int(n) => n.to_string(),
            redis::Value::Double(f) => f.to_string(),
            _ => String::new(),
        };
        out.push((name, value));
        i += 2;
    }
    out
}
