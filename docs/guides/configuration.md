# Configuration Reference

> v0.16.0-phase4 — authoritative source: `crates/the-one-core/src/config.rs` and `crates/the-one-core/src/limits.rs`. See § Multi-Backend Selection (v0.16.0+) below for the Phase 2/3/4 pgvector + Postgres state + combined config additions.

## Overview

the-one-mcp resolves its configuration through five ordered layers. Every setting is optional; unset fields fall back to the previous layer until the built-in default is reached. The resolved config is immutable for the lifetime of a server process unless you use the `config update` MCP action, which rewrites the project config file and takes effect on the next `setup` (action: `project`) or server restart.

---

## Config File Locations

| Location | Purpose |
|---|---|
| `~/.the-one/config.json` | Global defaults for all projects on this machine |
| `<project_root>/.the-one/config.json` | Per-project overrides |

Both files are plain JSON. Only fields you want to override need to be present — omitted fields are inherited from lower layers.

The global state directory defaults to `~/.the-one`. Override it with `THE_ONE_HOME=/path/to/dir` (must be an absolute path).

---

## Precedence Layers

Layers are applied in ascending priority order. Higher layers win for any field they set.

```
1. Built-in defaults     (compiled into the binary)
2. Global config file    (~/.the-one/config.json)
3. Project config file   (<project>/.the-one/config.json)
4. Environment variables (THE_ONE_* pattern)
5. Runtime overrides     (set programmatically at server startup, never persisted)
```

A field set in the environment variable layer overrides both config files but not a runtime override. Runtime overrides are only used internally by the server process and cannot be set by the `config update` action.

---

## Complete Field Reference

All fields use their JSON key names (matching the config file format).

### Core Settings

| Field | Type | Default | Description |
|---|---|---|---|
| `provider` | string | `"local"` | Embedding backend. `"local"` uses local ONNX models; `"api"` routes to an external API. |
| `log_level` | string | `"info"` | Logging verbosity. Values: `"error"`, `"warn"`, `"info"`, `"debug"`, `"trace"`. |

**Example:**
```json
{
  "provider": "local",
  "log_level": "warn"
}
```

---

### MemPalace

MemPalace controls transcript memory, hook capture, AAAK lessons, diary flows,
and navigation primitives.

| Field | Type | Default | Description |
|---|---|---|---|
| `memory_palace_enabled` | bool | `true` | Master switch for transcript ingest and wake-up tools. |
| `memory_palace_hooks_enabled` | bool | `false` | Enables `maintain` action `memory.capture_hook`. |
| `memory_palace_aaak_enabled` | bool | `false` | Enables AAAK compression and lesson persistence tools. |
| `memory_palace_diary_enabled` | bool | `false` | Enables diary entry tools (`memory.diary.*`). |
| `memory_palace_navigation_enabled` | bool | `false` | Enables drawers/closets/tunnels navigation tools (`memory.navigation.*`). |

**Example (full profile equivalent):**

```json
{
  "memory_palace_enabled": true,
  "memory_palace_hooks_enabled": true,
  "memory_palace_aaak_enabled": true,
  "memory_palace_diary_enabled": true,
  "memory_palace_navigation_enabled": true
}
```

Environment variable overrides:

- `THE_ONE_MEMORY_PALACE_ENABLED`
- `THE_ONE_MEMORY_PALACE_HOOKS_ENABLED`
- `THE_ONE_MEMORY_PALACE_AAAK_ENABLED`
- `THE_ONE_MEMORY_PALACE_DIARY_ENABLED`
- `THE_ONE_MEMORY_PALACE_NAVIGATION_ENABLED`

Operational note:

- You can set all MemPalace flags consistently through `config` action
  `profile.set` with preset `off`, `core`, or `full` instead of editing each
  flag manually.

---

### Qdrant

Qdrant is the vector database used for semantic search. By default the server connects to a local instance.

| Field | Type | Default | Description |
|---|---|---|---|
| `qdrant_url` | string | `"http://127.0.0.1:6334"` | gRPC endpoint for Qdrant. Use port 6334 (gRPC) not 6333 (HTTP). |
| `qdrant_api_key` | string or null | `null` | API key for Qdrant Cloud or authenticated self-hosted instances. |
| `qdrant_ca_cert_path` | string or null | `null` | Path to a CA certificate file for TLS verification. |
| `qdrant_tls_insecure` | bool | `false` | Skip TLS certificate verification. Use only in development. |
| `qdrant_strict_auth` | bool | `true` | Reject connections if authentication is configured but no key is provided. |

**Example (Qdrant Cloud):**
```json
{
  "qdrant_url": "https://my-cluster.qdrant.io:6334",
  "qdrant_api_key": "your-api-key-here",
  "qdrant_strict_auth": true
}
```

**Example (self-hosted TLS):**
```json
{
  "qdrant_url": "https://qdrant.internal:6334",
  "qdrant_ca_cert_path": "/etc/ssl/certs/internal-ca.crt"
}
```

---

### Vector Backend Selection

These fields choose where semantic vectors live. Qdrant remains the default.

| Field | Type | Default | Description |
|---|---|---|---|
| `vector_backend` | string | `"qdrant"` | Active vector backend. Supported values today are `"qdrant"` and `"redis"`. |
| `redis_url` | string or null | `null` | Redis connection URL used when `vector_backend` is `"redis"`. |
| `redis_index_name` | string or null | `null` | Stable RediSearch index name to reuse across restarts. |
| `redis_persistence_required` | bool | `false` | Expresses that Redis durability is required for this deployment. |

**Example:**

```json
{
  "vector_backend": "redis",
  "redis_url": "redis://127.0.0.1:6379",
  "redis_index_name": "the_one_memories",
  "redis_persistence_required": true
}
```

Persistence expectations:

- Keep both Redis RDB snapshots and AOF enabled if you expect vector data to survive restarts.
- Keep `redis_index_name` stable for a deployment. Changing it creates a different RediSearch index namespace.
- If Redis persistence is lost, reusing the same `redis_index_name` does not recreate the old vectors by itself; reindexing is still required.

Current runtime note:

- With local embeddings enabled, `vector_backend: "redis"` uses the Redis-backed memory engine path.
- With API embeddings and `vector_backend: "redis"`, broker initialization fails fast with a configuration error because this backend pairing is not supported.

Environment variable overrides:

- `THE_ONE_VECTOR_BACKEND`
- `THE_ONE_REDIS_URL`
- `THE_ONE_REDIS_INDEX_NAME`
- `THE_ONE_REDIS_PERSISTENCE_REQUIRED`

---

### Multi-Backend Selection (v0.16.0+)

v0.16.0 introduces pluggable backends on **both** axes — state store AND
vector backend — via four env vars parsed at broker startup. The legacy
`vector_backend` field above still works (for Qdrant vs Redis), but Phase 2
and Phase 3 add new backends that are selected through the unified env
surface instead. Read this section if you plan to run the-one-mcp on
Postgres (either axis), or if you want to understand the complete
selection surface.

**Four env vars**, two per axis, parallel naming:

```bash
THE_ONE_STATE_TYPE=<sqlite|postgres|redis|postgres-combined|redis-combined>
THE_ONE_STATE_URL=<connection string, may carry credentials>
THE_ONE_VECTOR_TYPE=<qdrant|pgvector|redis-vectors|postgres-combined|redis-combined>
THE_ONE_VECTOR_URL=<connection string, may carry credentials>
```

Valid combinations shipping today:

| `STATE_TYPE` | `VECTOR_TYPE` | Cargo features | Shipped in |
|---|---|---|---|
| unset (= `sqlite`) | unset (= `qdrant`) | (default) | v0.15.x baseline |
| unset | `pgvector` | `pg-vectors` | v0.16.0 Phase 2 |
| `postgres` | unset | `pg-state` | v0.16.0 Phase 3 |
| `postgres` | `pgvector` | `pg-state,pg-vectors` | v0.16.0 Phase 3 (split pools) |
| `postgres-combined` | `postgres-combined` | `pg-state,pg-vectors` | v0.16.0 Phase 4 (combined pool) |
| `redis` | any | (planned) | v0.16.0 Phase 5 |
| `redis-combined` | `redis-combined` | (planned) | v0.16.0 Phase 6 |

> **Combined deployments reuse both config sections.** When
> `postgres-combined` is set on both axes, the broker reads
> `[state_postgres]` AND `[vector_pgvector]` from the same
> `config.json` — there is no `[combined_postgres]` section.
> Pool-sizing fields (`max_connections`, `min_connections`, the
> timeout fields, `statement_timeout_ms`) come from
> `state_postgres` only — `vector_pgvector`'s corresponding pool
> fields are ignored on the combined path. HNSW tuning still
> comes from `vector_pgvector` because those are migration-time
> and query-time settings. Full rationale in the
> [combined Postgres backend guide](combined-postgres-backend.md).

**Fail-loud startup validation** — the broker parses these env vars once at
`McpBroker::try_new_with_policy` and enforces every rule below. Any
violation produces `CoreError::InvalidProjectConfig` with a targeted error
message:

| Input state | Broker behaviour |
|---|---|
| All four unset | `(Sqlite, Qdrant)` default. Zero behaviour change from v0.15.x. |
| One `TYPE` set, other `TYPE` unset | **FAIL LOUD** — `"THE_ONE_STATE_TYPE=postgres set but THE_ONE_VECTOR_TYPE is unset; both axes must be explicit when either is overridden."` |
| `TYPE` set but matching `URL` unset | **FAIL LOUD** — `"THE_ONE_STATE_TYPE=postgres requires THE_ONE_STATE_URL to be set."` |
| Unknown `TYPE` value | **FAIL LOUD with the enum list** — `"Unknown THE_ONE_STATE_TYPE=pgsql; expected one of: sqlite, postgres, redis, postgres-combined, redis-combined"` |
| Combined `TYPE`s with mismatched values | **FAIL LOUD** — `"Combined backends must match: THE_ONE_STATE_TYPE=postgres-combined requires THE_ONE_VECTOR_TYPE=postgres-combined"` |
| Both `*-combined`, URL mismatch | **FAIL LOUD with both URLs echoed** — `"Combined postgres-combined: THE_ONE_STATE_URL and THE_ONE_VECTOR_URL must be byte-identical; got state_url=X vs vector_url=Y"` |
| Both non-combined, same URL | **Allowed, silent.** Operator opts into split pools sharing one host. |

**First-match failure** — validation runs top-to-bottom and fails on the
first violation. Collecting all errors would obscure cascading root causes
(unknown `TYPE` makes the URL check meaningless); first-match keeps the
v0.15.0 "one `corr=<id>` per error" envelope invariant.

**Test isolation pattern** — the parser is covered by 12 tests in
`the_one_core::config::backend_selection::tests` (8 negative + 4 positive
controls), all wrapped in `temp_env::with_vars` so parallel `cargo test`
runs don't poison each other. Never set `THE_ONE_*` env vars directly in a
test — use `temp_env::with_vars` instead.

---

### Vector Backend — pgvector (v0.16.0 Phase 2)

Tuning knobs for the `PgVectorBackend` live in a nested `vector_pgvector`
block on `config.json`. Every field has a serde `default`, so the block is
optional — operators who just want the defaults can omit it entirely.

```json
{
  "vector_pgvector": {
    "schema": "the_one",
    "hnsw_m": 16,
    "hnsw_ef_construction": 100,
    "hnsw_ef_search": 40,
    "max_connections": 10,
    "min_connections": 2,
    "acquire_timeout_ms": 30000,
    "idle_timeout_ms": 600000,
    "max_lifetime_ms": 1800000
  }
}
```

| Field | Default | Notes |
|---|---|---|
| `schema` | `"the_one"` | Postgres schema the migrations write into. Changing only affects **new installs**. |
| `hnsw_m` | `16` | HNSW graph connectivity. Migration-time only — drops + recreates the index to retune. |
| `hnsw_ef_construction` | `100` | HNSW build-time quality. Migration-time only. |
| `hnsw_ef_search` | `40` | HNSW query-time recall. **Runtime-tunable** — applied per search via `SET LOCAL hnsw.ef_search`. |
| `max_connections` | `10` | sqlx pool max. |
| `min_connections` | `2` | Non-zero to avoid cold-start TCP+TLS+auth latency on the first query after a restart. |
| `acquire_timeout_ms` | `30000` | How long a broker handler waits for a free connection. |
| `idle_timeout_ms` | `600000` (10 min) | Idle connection TTL. |
| `max_lifetime_ms` | `1800000` (30 min) | Forces periodic reconnect to pick up IAM credential rotation, PGBouncer reshards, etc. |

See the full [pgvector backend guide](pgvector-backend.md) for installation
per managed-Postgres provider (Supabase, RDS, Cloud SQL, Azure,
self-hosted), migration ownership model, `dim=1024` Decision C rationale,
HNSW vs IVFFlat trade-offs, and monitoring queries.

---

### State Store — Postgres (v0.16.0 Phase 3)

Tuning knobs for the `PostgresStateStore` live in a nested `state_postgres`
block, structurally parallel to `vector_pgvector`. Same defaults strategy —
omit the block for production-sane defaults.

```json
{
  "state_postgres": {
    "schema": "the_one",
    "statement_timeout_ms": 30000,
    "max_connections": 10,
    "min_connections": 2,
    "acquire_timeout_ms": 30000,
    "idle_timeout_ms": 600000,
    "max_lifetime_ms": 1800000
  }
}
```

| Field | Default | Notes |
|---|---|---|
| `schema` | `"the_one"` | Same schema as pgvector by default, so combined Phase 4 deployments share one namespace. |
| `statement_timeout_ms` | `30000` (30s) | Applied per-connection via `SET statement_timeout` in sqlx's `after_connect` hook. `0` disables entirely (Postgres default). On the **combined** path (Phase 4), this value also applies to vector queries — the shared pool's `after_connect` hook uses the state-side value for every checked-out connection. |
| `max_connections` ... `max_lifetime_ms` | (same defaults as `vector_pgvector`) | Field shapes are deliberately parallel. On the **combined** path (Phase 4), these state-side fields win — `vector_pgvector`'s corresponding fields are ignored and this one block controls the shared pool. |

See the full [Postgres state backend guide](postgres-state-backend.md) for
the sync-over-async bridge rationale, FTS5 → tsvector translation details,
`'simple'` vs `'english'` tokenizer choice, schema v7 parity, and the
migration path from SQLite. For the combined-pool variant shipped in
Phase 4 (one `sqlx::PgPool` serving both trait roles), see the
[combined Postgres backend guide](combined-postgres-backend.md).

---

### Embeddings (text)

| Field | Type | Default | Description |
|---|---|---|---|
| `embedding_provider` | string | `"local"` | `"local"` for on-device ONNX models via fastembed; `"api"` for OpenAI-compatible HTTP endpoints. |
| `embedding_model` | string | `"BGE-large-en-v1.5"` | Model name. For local provider use the `name` from `models.list`. For API provider use the model ID string from the API (e.g. `"text-embedding-3-small"`). Tier aliases (`"fast"`, `"balanced"`, `"quality"`, `"multilingual"`) are also accepted for the local provider. |
| `embedding_api_base_url` | string or null | `null` | Base URL for an OpenAI-compatible embeddings API. Required when `embedding_provider` is `"api"`. |
| `embedding_api_key` | string or null | `null` | Bearer token sent as `Authorization: Bearer <key>`. Required for most API providers. |
| `embedding_dimensions` | integer | `1024` | Output vector dimensions. Must match what the selected model actually produces. Only set this when the model supports Matryoshka truncation (e.g. OpenAI `text-embedding-3-*`). |

**Changing the embedding model requires a full reindex.** Vectors produced by different models are not comparable. Run `maintain reindex` after any change to `embedding_model`.

---

### Reranking

Cross-encoder reranking re-scores search results with a more precise model after the initial vector recall step. Disabled by default because it adds latency.

| Field | Type | Default | Description |
|---|---|---|---|
| `reranker_enabled` | bool | `false` | Enable cross-encoder reranking for search results. |
| `reranker_model` | string | `"bge-reranker-base"` | Reranker model name. See [Reranker Models](embedding-models.md#reranker-models) for the full list. |

**Example:**
```json
{
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual"
}
```

---

### Image Embeddings

Image indexing, OCR, and thumbnail generation are all disabled by default. Enabling them requires the `image-embeddings` feature flag to have been compiled into the binary (see [Feature Flags](#feature-flags)).

| Field | Type | Default | Description |
|---|---|---|---|
| `image_embedding_enabled` | bool | `false` | Enable image indexing and visual semantic search. |
| `image_embedding_model` | string | `"nomic-embed-vision-v1.5"` | Image embedding model. Must be a value from `image-models.toml`. See [Image Embedding Models](embedding-models.md#image-embedding-models). |
| `image_ocr_enabled` | bool | `false` | Enable OCR text extraction from indexed images using Tesseract. |
| `image_ocr_language` | string | `"eng"` | Tesseract language code. Examples: `"eng"`, `"fra"`, `"deu"`, `"jpn"`. Multiple languages: `"eng+fra"`. |
| `image_thumbnail_enabled` | bool | `true` | Generate thumbnails for indexed images (used by the admin UI). |
| `image_thumbnail_max_px` | integer | `256` | Maximum thumbnail dimension in pixels (applied to both width and height). |

**Example:**
```json
{
  "image_embedding_enabled": true,
  "image_embedding_model": "nomic-embed-vision-v1.5",
  "image_ocr_enabled": true,
  "image_ocr_language": "eng"
}
```

---

### Hybrid Search

Hybrid search combines dense cosine similarity with sparse lexical matching (SPLADE++) for stronger exact-match retrieval. Disabled by default. See [Hybrid Search Guide](hybrid-search.md) for full details.

| Field | Type | Default | Description |
|---|---|---|---|
| `hybrid_search_enabled` | bool | `false` | Enable hybrid dense+sparse search. Requires a full reindex after enabling. |
| `hybrid_dense_weight` | float | `0.7` | Weight applied to the dense cosine score in the final fusion. Range [0.0, 1.0]. |
| `hybrid_sparse_weight` | float | `0.3` | Weight applied to the normalized sparse score in the final fusion. Range [0.0, 1.0]. |
| `sparse_model` | string | `"bm25"` | Sparse model alias. Currently `"bm25"` maps to SPLADE++Ensemble Distil in fastembed 5.13. |

**Example:**
```json
{
  "hybrid_search_enabled": true,
  "hybrid_dense_weight": 0.7,
  "hybrid_sparse_weight": 0.3
}
```

**Note:** enabling hybrid search requires Qdrant 1.7+ for sparse vector support. Run `maintain (action: reindex)` after changing this setting.

---

### Auto-Indexing (File Watcher)

An optional background file watcher that monitors `.the-one/docs/` and `.the-one/images/` for changes. Markdown and image file changes are automatically re-ingested when their corresponding feature paths are enabled. See [Auto-Indexing Guide](auto-indexing.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_index_enabled` | bool | `false` | Start the background file watcher at server startup. |
| `auto_index_debounce_ms` | integer | `2000` | Milliseconds to wait after the last file event before processing. Prevents redundant triggers from editor burst saves. |

**Example:**
```json
{
  "auto_index_enabled": true,
  "auto_index_debounce_ms": 2000
}
```

---

### Nano LLM Providers

The nano LLM layer is an optional lightweight classifier that routes tool suggestions semantically. When disabled (`"rules"`), the server uses keyword-based routing only.

| Field | Type | Default | Description |
|---|---|---|---|
| `nano_provider` | string | `"rules"` | Provider kind. Values: `"rules"` (disabled), `"api"`, `"ollama"`, `"lmstudio"`. |
| `nano_model` | string | `"none"` | Model name sent to the nano provider when `nano_provider` is not `"rules"`. |
| `nano_routing_policy` | string | `"priority"` | How to select from the `nano_providers` pool. Values: `"priority"`, `"round_robin"`, `"latency"`. |
| `nano_providers` | array | `[]` | Pool of nano provider entries. Each entry configures one endpoint. |

**`nano_providers` entry fields:**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Display name for this provider entry. |
| `base_url` | string | yes | HTTP base URL for the API endpoint. |
| `model` | string | yes | Model identifier to request. |
| `api_key` | string or null | no | Bearer token for authentication. |
| `timeout_ms` | integer | yes | Per-request timeout in milliseconds. |
| `enabled` | bool | yes | Whether this entry is active in the pool. |

**Example (Ollama):**
```json
{
  "nano_provider": "ollama",
  "nano_model": "qwen2.5:0.5b",
  "nano_providers": [
    {
      "name": "local-ollama",
      "base_url": "http://127.0.0.1:11434",
      "model": "qwen2.5:0.5b",
      "api_key": null,
      "timeout_ms": 2000,
      "enabled": true
    }
  ]
}
```

---

### External Docs

| Field | Type | Default | Description |
|---|---|---|---|
| `external_docs_root` | string or null | `null` | Absolute path to a directory of Markdown/text files that the server indexes and makes searchable alongside project docs. Useful for indexing framework or library documentation. |

---

## Configurable Limits

Limits live under the `limits` key in config files and are validated (clamped) on load. Out-of-range values are silently clamped with a log warning.

| Field | Type | Default | Min | Max | Description |
|---|---|---|---|---|---|
| `max_tool_suggestions` | integer | `5` | `1` | `50` | Maximum tool suggestions returned per request. |
| `max_search_hits` | integer | `5` | `1` | `100` | Maximum documents returned per semantic search query. |
| `max_raw_section_bytes` | integer | `24576` (24 KB) | `1024` | `1048576` (1 MB) | Maximum size of a raw doc section returned in search results. |
| `max_enabled_families` | integer | `12` | `1` | `100` | Maximum number of tool families that can be enabled simultaneously. |
| `max_doc_size_bytes` | integer | `102400` (100 KB) | `1024` | `10485760` (10 MB) | Maximum size of a single document that can be indexed. |
| `max_managed_docs` | integer | `500` | `10` | `10000` | Maximum number of documents managed per project. |
| `max_embedding_batch_size` | integer | `64` | `1` | `256` | How many text chunks are embedded in a single batch call. Increase for throughput on fast hardware, decrease for memory pressure. |
| `max_chunk_tokens` | integer | `512` | `64` | `2048` | Token budget per text chunk during document splitting. |
| `max_nano_timeout_ms` | integer | `2000` | `100` | `10000` | Per-request timeout for nano LLM provider calls in milliseconds. |
| `max_nano_retries` | integer | `3` | `0` | `10` | Retry count for failed nano LLM calls before cooldown. |
| `max_nano_providers` | integer | `5` | `1` | `10` | Maximum entries in the nano provider pool. |
| `search_score_threshold` | float | `0.3` | `0.0` | `1.0` | Minimum cosine similarity score to include a text result. |
| `max_image_size_bytes` | integer | `10485760` (10 MB) | `102400` (100 KB) | `104857600` (100 MB) | Maximum image file size accepted for indexing. |
| `max_images_per_project` | integer | `500` | `10` | `10000` | Maximum images indexed per project. |
| `max_image_search_hits` | integer | `5` | `1` | `50` | Maximum images returned per visual search query. |
| `image_search_score_threshold` | float | `0.25` | `0.0` | `1.0` | Minimum score to include an image result. |

**Example:**
```json
{
  "limits": {
    "max_search_hits": 10,
    "max_chunk_tokens": 256,
    "search_score_threshold": 0.25
  }
}
```

---

## Environment Variables

All environment variables follow the `THE_ONE_*` prefix. They take precedence over both config files but not runtime overrides. Boolean variables accept `1`, `true`, `yes`, `on` (true) and `0`, `false`, `no`, `off` (false).

| Variable | Config field equivalent |
|---|---|
| `THE_ONE_HOME` | Global state directory (default: `~/.the-one`) |
| `THE_ONE_PROVIDER` | `provider` |
| `THE_ONE_LOG_LEVEL` | `log_level` |
| `THE_ONE_QDRANT_URL` | `qdrant_url` |
| `THE_ONE_QDRANT_API_KEY` | `qdrant_api_key` |
| `THE_ONE_QDRANT_CA_CERT_PATH` | `qdrant_ca_cert_path` |
| `THE_ONE_QDRANT_TLS_INSECURE` | `qdrant_tls_insecure` |
| `THE_ONE_QDRANT_STRICT_AUTH` | `qdrant_strict_auth` |
| `THE_ONE_NANO_PROVIDER` | `nano_provider` |
| `THE_ONE_NANO_MODEL` | `nano_model` |
| `THE_ONE_EMBEDDING_PROVIDER` | `embedding_provider` |
| `THE_ONE_EMBEDDING_MODEL` | `embedding_model` |
| `THE_ONE_EMBEDDING_API_BASE_URL` | `embedding_api_base_url` |
| `THE_ONE_EMBEDDING_API_KEY` | `embedding_api_key` |
| `THE_ONE_EMBEDDING_DIMENSIONS` | `embedding_dimensions` |
| `THE_ONE_EXTERNAL_DOCS_ROOT` | `external_docs_root` |
| `THE_ONE_RERANKER_ENABLED` | `reranker_enabled` |
| `THE_ONE_RERANKER_MODEL` | `reranker_model` |
| `THE_ONE_IMAGE_EMBEDDING_ENABLED` | `image_embedding_enabled` |
| `THE_ONE_IMAGE_EMBEDDING_MODEL` | `image_embedding_model` |
| `THE_ONE_IMAGE_OCR_ENABLED` | `image_ocr_enabled` |
| `THE_ONE_IMAGE_OCR_LANGUAGE` | `image_ocr_language` |
| `THE_ONE_IMAGE_THUMBNAIL_ENABLED` | `image_thumbnail_enabled` |
| `THE_ONE_IMAGE_THUMBNAIL_MAX_PX` | `image_thumbnail_max_px` |

**Limit variables:**

| Variable | Limit field |
|---|---|
| `THE_ONE_LIMIT_MAX_TOOL_SUGGESTIONS` | `limits.max_tool_suggestions` |
| `THE_ONE_LIMIT_MAX_SEARCH_HITS` | `limits.max_search_hits` |
| `THE_ONE_LIMIT_MAX_RAW_SECTION_BYTES` | `limits.max_raw_section_bytes` |
| `THE_ONE_LIMIT_MAX_ENABLED_FAMILIES` | `limits.max_enabled_families` |
| `THE_ONE_LIMIT_MAX_DOC_SIZE_BYTES` | `limits.max_doc_size_bytes` |
| `THE_ONE_LIMIT_MAX_MANAGED_DOCS` | `limits.max_managed_docs` |
| `THE_ONE_LIMIT_MAX_EMBEDDING_BATCH_SIZE` | `limits.max_embedding_batch_size` |
| `THE_ONE_LIMIT_MAX_CHUNK_TOKENS` | `limits.max_chunk_tokens` |
| `THE_ONE_LIMIT_MAX_NANO_TIMEOUT_MS` | `limits.max_nano_timeout_ms` |
| `THE_ONE_LIMIT_MAX_NANO_RETRIES` | `limits.max_nano_retries` |
| `THE_ONE_LIMIT_MAX_NANO_PROVIDERS` | `limits.max_nano_providers` |
| `THE_ONE_LIMIT_SEARCH_SCORE_THRESHOLD` | `limits.search_score_threshold` |
| `THE_ONE_LIMIT_MAX_IMAGE_SIZE_BYTES` | `limits.max_image_size_bytes` |
| `THE_ONE_LIMIT_MAX_IMAGES_PER_PROJECT` | `limits.max_images_per_project` |
| `THE_ONE_LIMIT_MAX_IMAGE_SEARCH_HITS` | `limits.max_image_search_hits` |
| `THE_ONE_LIMIT_IMAGE_SEARCH_SCORE_THRESHOLD` | `limits.image_search_score_threshold` |

---

## Feature Flags

Feature flags are compile-time switches baked into the binary at build time. They cannot be changed at runtime.

| Flag | What it enables |
|---|---|
| `local-embeddings` | On-device ONNX embedding via fastembed. Required for `embedding_provider = "local"`. Included in the default build. |
| `image-embeddings` | Image indexing and visual search via fastembed's image models. Required for `image_embedding_enabled = true`. Not included in the `--lean` build. |
| `image-ocr` | OCR text extraction from images via Tesseract. Required for `image_ocr_enabled = true`. Requires Tesseract and language data to be installed on the system. |

To check which features are compiled in, run:
```bash
the-one-mcp --version
```

The lean build (`scripts/build.sh build --lean`) omits `image-embeddings` and `image-ocr` to produce a smaller binary.

---

## Example Configurations

### Minimal (zero config)

No config file needed. Defaults give you a working local setup:

- Local BGE-large-en-v1.5 embedding model (downloads ~130 MB on first use)
- Qdrant at `http://127.0.0.1:6334`
- Rules-only tool routing (no nano LLM)
- No image indexing, no reranking

---

### Production (remote Qdrant + API embeddings)

`~/.the-one/config.json`:
```json
{
  "provider": "api",
  "log_level": "warn",
  "qdrant_url": "https://my-cluster.qdrant.io:6334",
  "qdrant_api_key": "qd-key-abc123",
  "embedding_provider": "api",
  "embedding_model": "text-embedding-3-small",
  "embedding_api_base_url": "https://api.openai.com/v1",
  "embedding_api_key": "sk-...",
  "embedding_dimensions": 1536,
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual",
  "limits": {
    "max_search_hits": 15,
    "search_score_threshold": 0.25
  }
}
```

Use environment variables for secrets to avoid storing them in the file:
```bash
export THE_ONE_QDRANT_API_KEY="qd-key-abc123"
export THE_ONE_EMBEDDING_API_KEY="sk-..."
```

---

### Multilingual project

`<project>/.the-one/config.json`:
```json
{
  "embedding_provider": "local",
  "embedding_model": "multilingual-e5-large",
  "embedding_dimensions": 1024,
  "reranker_enabled": true,
  "reranker_model": "jina-reranker-v2-base-multilingual"
}
```

Note: switching from the default BGE model requires a reindex (`maintain reindex`).

---

### Image search enabled

`<project>/.the-one/config.json`:
```json
{
  "image_embedding_enabled": true,
  "image_embedding_model": "nomic-embed-vision-v1.5",
  "image_ocr_enabled": true,
  "image_ocr_language": "eng",
  "image_thumbnail_enabled": true,
  "image_thumbnail_max_px": 256,
  "limits": {
    "max_images_per_project": 1000,
    "max_image_search_hits": 10
  }
}
```

The binary must have been built with the `image-embeddings` and `image-ocr` feature flags. Tesseract must be installed for OCR.

---

### Development (local everything)

`~/.the-one/config.json`:
```json
{
  "log_level": "debug",
  "qdrant_url": "http://127.0.0.1:6334",
  "embedding_provider": "local",
  "embedding_model": "fast",
  "nano_provider": "ollama",
  "nano_model": "qwen2.5:0.5b",
  "nano_providers": [
    {
      "name": "local-ollama",
      "base_url": "http://127.0.0.1:11434",
      "model": "qwen2.5:0.5b",
      "api_key": null,
      "timeout_ms": 3000,
      "enabled": true
    }
  ],
  "limits": {
    "max_chunk_tokens": 256,
    "max_embedding_batch_size": 16
  }
}
```

Using `embedding_model: "fast"` selects the all-MiniLM-L6-v2 tier alias (23 MB, fastest).

---

## Updating Config at Runtime

The `config` MCP tool exposes an `update` action that writes directly to the project config file. Changes take effect on the next `setup` (action: `project`) or server restart.

**Export current config:**
```json
{
  "action": "export",
  "params": { "project_root": "/path/to/project" }
}
```

**Update one or more fields:**
```json
{
  "action": "update",
  "params": {
    "project_root": "/path/to/project",
    "update": {
      "reranker_enabled": true,
      "reranker_model": "bge-reranker-v2-m3",
      "limits": {
        "max_search_hits": 20
      }
    }
  }
}
```

The `update` object accepts any subset of config fields. Only fields present in the object are written; others are left unchanged. The file is written atomically (write to `.tmp`, then rename).

Other `config` actions: `tool.add`, `tool.remove`, `models.list`, `models.check`.

---

## Troubleshooting Config Issues

**Server logs a warning about a clamped limit value**

A limit was set outside its allowed range. Check the [Configurable Limits](#configurable-limits) table for min/max bounds. The value has been clamped automatically.

**`THE_ONE_HOME must be absolute`**

The `THE_ONE_HOME` environment variable was set to a relative path. It must be an absolute path (starting with `/`).

**`project root does not exist`**

The `project_root` path passed to `AppConfig::load` does not exist or is not a directory. Verify the path and that it has been created.

**Embedding model not found after changing `embedding_model`**

For local models the name must exactly match the `name` field in `models/local-models.toml` (or a tier alias). Run `config models.list` to see valid names.

**Config file is ignored**

JSON parse errors cause the file to be skipped with an error log entry. Validate the JSON with `jq . ~/.the-one/config.json` or `jq . .the-one/config.json`.

**Image indexing has no effect**

Check that the binary was built with the `image-embeddings` feature flag and that `image_embedding_enabled` is `true` in the resolved config. Run `config export` to see the resolved values.
