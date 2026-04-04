# Embedding Model Registry & Interactive Setup

**Date:** 2026-04-04
**Status:** Approved
**Scope:** Model registry, interactive installer selection, maintenance scripts, API model support

---

## 1. Problem

- The default embedding model is `all-MiniLM-L6-v2` (fast tier) — low quality for production use. The user wants `BGE-large-en-v1.5` (quality tier) as the default.
- No interactive model selection during setup. Users get a hardcoded config with no visibility into alternatives.
- `available_models()` is missing 3 supported models: `multilingual-e5-small`, `multilingual-e5-base`, `paraphrase-ml-minilm-l12-v2`.
- No maintenance process for tracking upstream model updates (local or API).
- API embedding models are supported but not presented as first-class options during setup.

## 2. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Default model | `BGE-large-en-v1.5` (quality) | Better retrieval quality; ask user to confirm during setup |
| Setup location | Installer + per-project override | Installer sets global default interactively; `config.update` overrides per-project |
| Local model registry | Hardcoded in TOML, embedded in binary | Tied to `fastembed` crate version; maintenance script checks for updates |
| API model registry | TOML file, embedded + fetchable | Changes independently of binary releases; extensible by users |
| Selection UI | Numbered table with descriptive latency labels | Shows all options at once; `~4x slower` style labels; `[3]` default with Enter-to-accept |
| Maintenance | Two scripts + optional runtime check | `update-local-models.sh` and `update-api-models.sh`; never auto-remove models |

## 3. Model Registry Data Model

### 3.1 Local Models (`models/local-models.toml`)

Embedded in the binary via `include_str!`. Source of truth for `available_models()` and installer table.

```toml
[meta]
fastembed_crate_version = "4.x"
updated = "2026-04-04"

[models.all-minilm-l6-v2]
name = "all-MiniLM-L6-v2"
tier = "fast"
dims = 384
size_mb = 23
latency_vs_fast = "fastest"
multilingual = false
description = "Fast, small. Good for getting started."
fastembed_enum = "AllMiniLML6V2"
default = false

[models.bge-base-en-v1_5]
name = "BGE-base-en-v1.5"
tier = "balanced"
dims = 768
size_mb = 50
latency_vs_fast = "~2x slower"
multilingual = false
description = "Good quality/speed tradeoff."
fastembed_enum = "BGEBaseENV15"
default = false

[models.bge-large-en-v1_5]
name = "BGE-large-en-v1.5"
tier = "quality"
dims = 1024
size_mb = 130
latency_vs_fast = "~4x slower"
multilingual = false
description = "Best local quality. Recommended default."
fastembed_enum = "BGELargeENV15"
default = true

[models.multilingual-e5-large]
name = "multilingual-e5-large"
tier = "multilingual"
dims = 1024
size_mb = 220
latency_vs_fast = "~5x slower"
multilingual = true
description = "Best for non-English or mixed-language projects."
fastembed_enum = "MultilingualE5Large"
default = false

[models.multilingual-e5-base]
name = "multilingual-e5-base"
tier = "multilingual"
dims = 768
size_mb = 90
latency_vs_fast = "~3x slower"
multilingual = true
description = "Multilingual, moderate size."
fastembed_enum = "MultilingualE5Base"
default = false

[models.multilingual-e5-small]
name = "multilingual-e5-small"
tier = "multilingual"
dims = 384
size_mb = 45
latency_vs_fast = "~1.5x slower"
multilingual = true
description = "Lightweight multilingual option."
fastembed_enum = "MultilingualE5Small"
default = false

[models.paraphrase-ml-minilm-l12-v2]
name = "paraphrase-ml-minilm-l12-v2"
tier = "multilingual"
dims = 384
size_mb = 45
latency_vs_fast = "~1.5x slower"
multilingual = true
description = "Paraphrase-tuned multilingual model."
fastembed_enum = "ParaphraseMLMiniLML12V2"
default = false
```

Additional models already in `resolve_model()` (all-MiniLM-L12-v2, BGE-small-en-v1.5, nomic-embed-text-v1, nomic-embed-text-v1.5, mxbai-embed-large-v1, gte-base-en-v1.5, gte-large-en-v1.5, quantized variants) are included in the full registry but shown only in the `models.list` MCP tool, not in the installer table. The installer table shows the 7 primary tiers + API option.

### 3.2 API Models (`models/api-models.toml`)

Embedded in binary + fetchable from GitHub raw URL. Users extend with `~/.the-one/api-models.toml`.

```toml
[meta]
updated = "2026-04-04"

# ── OpenAI ──────────────────────────────────────────────────────────

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
auth_env = "OPENAI_API_KEY"
docs_url = "https://platform.openai.com/docs/guides/embeddings"

[providers.openai.models.text-embedding-3-small]
name = "text-embedding-3-small"
dims = 1536
multilingual = true
description = "Fast, cheap. Good default for API users."
default = true

[providers.openai.models.text-embedding-3-large]
name = "text-embedding-3-large"
dims = 3072
multilingual = true
description = "Best quality from OpenAI."

# ── Voyage AI ───────────────────────────────────────────────────────

[providers.voyage]
name = "Voyage AI"
base_url = "https://api.voyageai.com/v1"
auth_env = "VOYAGE_API_KEY"
docs_url = "https://docs.voyageai.com/docs/embeddings"

[providers.voyage.models.voyage-3]
name = "voyage-3"
dims = 1024
multilingual = true
description = "Best quality from Voyage. Strong code understanding."
default = true

[providers.voyage.models.voyage-3-lite]
name = "voyage-3-lite"
dims = 512
multilingual = true
description = "Lighter, faster Voyage model."

# ── Cohere ──────────────────────────────────────────────────────────

[providers.cohere]
name = "Cohere"
base_url = "https://api.cohere.com/v2"
auth_env = "COHERE_API_KEY"
docs_url = "https://docs.cohere.com/reference/embed"

[providers.cohere.models.embed-v4_0]
name = "embed-v4.0"
dims = 1024
multilingual = true
description = "Latest Cohere embedding model."
default = true

[providers.cohere.models.embed-multilingual-v3_0]
name = "embed-multilingual-v3.0"
dims = 1024
multilingual = true
description = "Optimized for 100+ languages."
```

## 4. Interactive Setup Flow

### 4.1 Installer Table (`scripts/install.sh`)

New function `select_embedding_model()` runs after binary install, before CLI registration.

```
╔══════════════════════════════════════════════════════════════════════════╗
║  Embedding Model Selection                                              ║
╠══════════════════════════════════════════════════════════════════════════╣
║                                                                          ║
║  #   Model                     Dims   Size    Latency        Multilingual║
║  ─── ───────────────────────── ────── ─────── ──────────────  ──────────║
║  1   all-MiniLM-L6-v2          384    23MB    fastest              No    ║
║  2   BGE-base-en-v1.5          768    50MB    ~2x slower           No    ║
║ [3]  BGE-large-en-v1.5 ★      1024   130MB   ~4x slower           No    ║
║  4   multilingual-e5-large    1024   220MB   ~5x slower          Yes    ║
║  5   multilingual-e5-base      768    90MB    ~3x slower          Yes    ║
║  6   multilingual-e5-small     384    45MB    ~1.5x slower        Yes    ║
║  7   paraphrase-ml-minilm      384    45MB    ~1.5x slower        Yes    ║
║  8   API (OpenAI/Voyage/Cohere)  —      —        —              Depends  ║
║                                                                          ║
║  ★ = recommended                                                         ║
╚══════════════════════════════════════════════════════════════════════════╝

Select model [3]:
```

Behaviors:
- **Enter** accepts default `3` (quality)
- **1-7** writes the chosen local model to `config.json`
- **8** triggers API provider sub-menu (provider selection, API key, model selection)
- **`--yes` flag** skips prompt, uses default
- **Non-interactive stdin** (piped) uses default silently
- Table data is generated from `local-models.toml` (downloaded from GitHub raw URL, with embedded fallback)

### 4.2 API Sub-Menu

When user selects `8`:

```
API Provider:
  1  OpenAI
  2  Voyage AI
  3  Cohere
  4  Custom (enter base URL)

Select provider [1]:

API Key (or env var name) [OPENAI_API_KEY]:

Model [text-embedding-3-small]:
```

Writes `embedding_provider`, `embedding_model`, `embedding_api_base_url`, `embedding_api_key`, and `embedding_dimensions` to `config.json`.

### 4.3 Per-Project Override

No new mechanism needed. The existing `config.update` MCP tool already supports all embedding fields. Users override per-project via:

```json
{"embedding_model": "multilingual-e5-large", "embedding_provider": "local"}
```

## 5. Code Changes

### 5.1 Files to Create

| File | Purpose |
|------|---------|
| `models/local-models.toml` | Local model registry (embedded in binary) |
| `models/api-models.toml` | API model registry (embedded + fetchable) |
| `scripts/update-local-models.sh` | Check fastembed crate for new/changed models |
| `scripts/update-api-models.sh` | Check API providers for new models |

### 5.2 Files to Modify

| File | Changes |
|------|---------|
| `crates/the-one-core/src/config.rs` | Change `DEFAULT_EMBEDDING_MODEL` to `"BGE-large-en-v1.5"`, `DEFAULT_EMBEDDING_DIMENSIONS` to `1024` |
| `crates/the-one-memory/src/embeddings.rs` | Add missing models to `available_models()` with full metadata (latency, size, multilingual). Change `"default"` alias to quality tier. Load model list from embedded TOML. |
| `crates/the-one-memory/src/lib.rs` | Add `models_registry` module: parse TOML, provide `list_local_models()` and `list_api_models()` |
| `crates/the-one-mcp/src/broker.rs` | Add `models.list` and `models.check_updates` MCP tools |
| `scripts/install.sh` | Add `select_embedding_model()` with table display, API follow-up, `--yes` support |

### 5.3 Files NOT Changed

- `EmbeddingProvider` trait — untouched
- `MemoryEngine` construction — still takes a model name string
- Per-project config override — already works
- Qdrant integration — unaffected
- Reranker — separate concern

## 6. Maintenance Process

### 6.1 Local Models (`scripts/update-local-models.sh`)

1. Read current `fastembed` crate version from `Cargo.toml`
2. Check crates.io API for latest `fastembed` version
3. If newer: pull crate source, diff the `EmbeddingModel` enum against `local-models.toml`
4. Report: new models found, deprecated models, version delta
5. With `--apply`: add new entries (marked `status = "new"` for review), bump crate version in `Cargo.toml`, update `embeddings.rs` match arms
6. With `--pr`: open a PR via `gh pr create`

Safety:
- **Never removes models automatically** — only flags as `deprecated = true`
- Removal requires manual confirmation after validating the replacement
- New models are added with `status = "new"` and excluded from installer table until reviewed

### 6.2 API Models (`scripts/update-api-models.sh`)

1. For each curated provider: fetch models endpoint / docs
2. Diff against `api-models.toml`
3. Report new/deprecated models
4. With `--apply`: add new entries (marked `status = "new"`)
5. With `--pr`: open a PR

### 6.3 Runtime Update Check

- `models.check_updates` MCP tool: fetches latest registry files from GitHub raw URL, reports diff
- Config flag `auto_update_models` (default `false`): if true, `project.init` fetches and merges new models into `~/.the-one/`
- Never removes models at runtime — only adds or marks deprecations
- If a user's configured model is deprecated: log warning `"Model X is deprecated, consider switching to Y"`, continue working

### 6.4 Safety Guarantees

- **Never auto-remove**: deprecated models remain available for at least one release cycle
- **Never auto-switch**: user's configured model is always respected, even if deprecated
- **Pinned to fastembed version**: `local-models.toml` entries have `fastembed_crate_version` field; models unsupported by the installed binary are hidden from selection
- **User extensions preserved**: `~/.the-one/api-models.toml` is user-owned, never overwritten

## 7. Tests

| Test | Crate | Description |
|------|-------|-------------|
| Registry TOML parsing | `the-one-memory` | All entries in both TOML files have required fields, valid types |
| `resolve_model("default")` returns quality | `the-one-memory` | Verify default alias points to `BGE-large-en-v1.5` |
| `available_models()` completeness | `the-one-memory` | All models from `local-models.toml` appear in `available_models()` |
| Default config values | `the-one-core` | `DEFAULT_EMBEDDING_MODEL` is `BGE-large-en-v1.5`, dims is 1024 |
| API registry extensibility | `the-one-memory` | User TOML merges correctly with embedded TOML |
| Deprecation warning | `the-one-mcp` | Deprecated model logs warning but doesn't fail |
| Installer non-interactive | shell | `--yes` flag writes quality model to config |
| `models.list` MCP tool | `the-one-mcp` | Returns all local + API models with metadata |
