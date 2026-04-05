# Phase 3: Polish (v0.11.0 + v0.12.0)

**Scope:** Close known gaps from the roadmap — Intel Mac local embeddings, observability deep dive, backup/migration story.

**Prerequisites:**
- Phase 2 complete (v0.10.0 shipped with resources API + catalog expansion, landing page live)
- Read `2026-04-05-SESSION-HANDOFF.md` first
- Verify baseline: `git log --oneline -5 && cargo test --workspace 2>&1 | grep "^test result"`

**Deliverables:**
- v0.11.0: Intel Mac can use local embeddings via ort-tract fallback
- v0.12.0: Observability improvements + backup/migration via `maintain: backup`

---

## Task 3.1: Intel Mac ort-tract Backend (v0.11.0)

### Why

Current state: Intel Mac ships lean (no `local-embeddings` feature). This is because fastembed 5.13 depends on `ort 2.0.0-rc.11` which depends on `ort-sys` which dropped prebuilt ONNX Runtime binaries for `x86_64-apple-darwin` as of early 2026.

Impact: Intel Mac users must use API embeddings (OpenAI, Voyage, Cohere) or build from source with a custom ort backend. Apple Silicon users are unaffected.

Intel Mac market share is dwindling but still significant (~20% of Mac dev market). Restoring local embeddings for this platform closes a real gap.

### Solution space

`ort` (the Rust ONNX Runtime wrapper) supports multiple backends:
1. **ort-download** — downloads prebuilt C++ ONNX Runtime binaries (default, doesn't support Intel Mac)
2. **ort-compile** — compiles ONNX Runtime from source via CMake (slow, requires C++ toolchain in CI)
3. **ort-tract** — pure-Rust ONNX inference via the [tract](https://github.com/sonos/tract) crate (slower at runtime but no C++ deps)

**Recommended: ort-tract.** Pure-Rust, cross-platform, no C++ toolchain needed, works on Intel Mac out of the box. Tradeoff: ~2-5x slower inference than C++ ort for the same model.

### Investigation steps

1. **Check ort's current tract support:**
   ```bash
   cargo search ort
   # Read ort's docs at https://ort.pyke.io/backends
   ```
   Verify that:
   - ort-tract exists as a feature flag
   - It's compatible with the fastembed version we use (5.13)
   - It supports the model types we use (text embedding, image embedding, reranker)

2. **Check fastembed's tract compatibility:**
   ```bash
   cd ~/.cargo/registry/src
   find . -name "fastembed-5.13*"
   grep -r "tract\|ort" fastembed-5.13*/Cargo.toml
   ```
   fastembed might pin the ort download backend. If so, we may need to:
   - Submit a PR upstream to add a tract feature
   - Fork fastembed temporarily
   - Use a workaround (see below)

3. **Workaround if fastembed doesn't support tract:**
   - Add `ort-tract` as a direct dep in `the-one-memory`
   - Conditionally compile a tract-based provider for `x86_64-apple-darwin` only
   - The tract provider bypasses fastembed entirely — loads ONNX files directly from `~/.fastembed_cache/`
   - Uses the same embedding API as FastEmbedProvider but with tract instead of fastembed

### Implementation path A: fastembed supports tract (preferred)

If fastembed has a tract feature flag:

1. In `crates/the-one-memory/Cargo.toml`:
   ```toml
   [features]
   local-embeddings = ["dep:fastembed"]
   local-embeddings-tract = ["dep:fastembed", "fastembed/tract"]
   ```

2. Update the workspace Cargo.toml or use target-specific features in the release workflow:
   ```yaml
   # .github/workflows/release.yml
   # For x86_64-apple-darwin, use local-embeddings-tract instead of local-embeddings
   - target: x86_64-apple-darwin
     features: "embed-swagger,local-embeddings-tract"
   ```

3. Remove `no_local_embeddings: true` from the Intel Mac matrix entry.

4. Verify build + tests on Intel Mac (locally or via CI).

### Implementation path B: custom tract provider (fallback)

If fastembed doesn't support tract, write a parallel provider:

1. Create `crates/the-one-memory/src/embeddings_tract.rs`:
   ```rust
   #[cfg(feature = "local-embeddings-tract")]
   use tract_onnx::prelude::*;
   
   pub struct TractEmbeddingProvider {
       model: SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>,
       tokenizer: tokenizers::Tokenizer,
       dims: usize,
       model_name: String,
   }
   
   impl TractEmbeddingProvider {
       pub fn new(model_name: &str) -> Result<Self, String> {
           // 1. Find the model file in ~/.fastembed_cache/
           // 2. Load via tract_onnx::onnx().model_for_path(...)
           // 3. Load tokenizer.json
           // 4. Return provider
       }
   }
   ```

2. Add `tract-onnx` as a dep:
   ```toml
   [dependencies]
   tract-onnx = { version = "0.22", optional = true }
   tokenizers = { version = "0.20", optional = true }
   ```

3. Behind `local-embeddings-tract` feature, gated separately from `local-embeddings`.

4. Modify the broker to try `FastEmbedProvider` first, fall back to `TractEmbeddingProvider` if feature is enabled and fastembed init fails.

### Files to modify

- `Cargo.toml` (workspace) — new optional deps
- `crates/the-one-memory/Cargo.toml` — feature flags + optional deps
- `crates/the-one-memory/src/embeddings.rs` or new `embeddings_tract.rs`
- `.github/workflows/release.yml` — Intel Mac matrix entry gets `local-embeddings-tract` feature
- `crates/the-one-ui/Cargo.toml` — if the UI needs a matching feature passthrough
- `crates/the-one-mcp/Cargo.toml` — feature passthrough
- `CHANGELOG.md`, `INSTALL.md` (document the tract fallback)

### Testing

Most challenging aspect: **verify it actually works on Intel Mac.** Options:
1. **CI runners:** `macos-13` GitHub Actions runners are Intel. Add a job that builds and runs tests with `--features local-embeddings-tract` on `macos-13`.
2. **Local test:** if you have access to an Intel Mac, test manually.
3. **QEMU/Rosetta on Apple Silicon:** slow but feasible.

Minimum: add a CI job that **compiles** for x86_64-apple-darwin with `local-embeddings-tract`. Runtime testing is stretch.

```yaml
# .github/workflows/ci.yml or similar
intel-mac-build-check:
  runs-on: macos-13  # Intel
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo build -p the-one-mcp --no-default-features --features embed-swagger,local-embeddings-tract
```

### Performance expectations

Document in CHANGELOG + user guide:

> **Note on Intel Mac embeddings:** The Intel Mac binary uses a pure-Rust ONNX backend (tract) instead of the C++ ort runtime used on other platforms. Expect embedding latency to be 2-5x higher (e.g., 100ms per chunk instead of 30ms). For low-latency needs on Intel Mac, consider API embeddings (OpenAI, Voyage, Cohere).

### Verification

```bash
# Default build (Apple Silicon / Linux / Windows with fastembed)
cargo build --workspace
cargo test --workspace

# Intel Mac simulation (if on Apple Silicon with Rosetta):
cargo build --no-default-features --features embed-swagger,local-embeddings-tract --target x86_64-apple-darwin

# Or on Intel Mac directly:
cargo build --workspace
cargo test --workspace
```

### Alternative: give up on Intel Mac local embeddings

If tract proves too slow, too large, or too unreliable, alternative is to:
1. Keep shipping Intel Mac lean (current behavior)
2. Document clearly that Intel Mac needs API embeddings
3. Add a helpful error message when Intel Mac users try to enable local embeddings

This is a valid v0.11.0 outcome if Task 3.1 turns out infeasible. Document why and move on.

### Release v0.11.0

```bash
echo "v0.11.0" > VERSION
# Update CHANGELOG
git tag -a v0.11.0 -m "v0.11.0: Intel Mac local embeddings via ort-tract"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.11.0
```

Verify all 6/6 platforms including Intel Mac with `local-embeddings-tract` enabled.

---

## Task 3.2: Observability Deep Dive (v0.12.0)

### Why

Current state: The `observe` tool (introduced in v0.5.0) has two actions: `metrics` and `events`. It has never been exercised in real use. The `metrics.snapshot` handler exposes a hardcoded set of counters (`project_init_calls`, `memory_search_calls`, etc.). Nobody has ever looked at what's missing or what would actually be useful for debugging production issues.

This task is about **validating and extending** observability based on real needs.

### Sub-tasks

#### 3.2a: Audit current metrics

Read `crates/the-one-mcp/src/broker.rs` and find the `BrokerMetrics` struct. List every counter currently tracked:

Likely current fields (verify against actual code):
- `project_init_calls: AtomicU64`
- `project_refresh_calls`
- `memory_search_calls`
- `tool_run_calls`
- `router_fallback_calls`
- `router_provider_error_calls`
- `router_decision_latency_ms_total`

Missing (high-value additions):
- **Per-tool call counts** — map of tool name to call count. "Which tools are actually used?"
- **Memory search latency** — p50, p95, p99 (histogram or reservoir sample)
- **Rerank latency** — when hybrid + rerank is enabled
- **Image ingest count** — how many images indexed
- **Image search count** — and latency
- **Watcher events** — how many auto-reindex events, how many succeeded vs failed
- **Qdrant errors** — connection failures, timeouts
- **Embedding cache hits** — how often does fastembed serve from cache vs re-compute
- **Chunk counts** — total chunks per project
- **Tool install count** — which tools users install
- **Resource reads** — if Phase 2 shipped, how often resources/read is called

#### 3.2b: Extend BrokerMetrics

Add the missing counters. For latency histograms, use a simple reservoir approach (keep last N samples + compute percentiles on query) or integrate a proper histogram library like `hdrhistogram` or `tdigest`.

```rust
pub struct BrokerMetrics {
    // ... existing atomic counters ...
    
    // NEW: per-tool call counts
    pub tool_calls: std::sync::RwLock<HashMap<String, u64>>,
    
    // NEW: latency samples (reservoir of last 100 per operation)
    pub memory_search_latency_ms: std::sync::Mutex<Vec<u64>>,
    pub rerank_latency_ms: std::sync::Mutex<Vec<u64>>,
    pub image_search_latency_ms: std::sync::Mutex<Vec<u64>>,
    
    // NEW: more counters
    pub image_ingest_count: AtomicU64,
    pub image_search_count: AtomicU64,
    pub watcher_events_processed: AtomicU64,
    pub watcher_events_failed: AtomicU64,
    pub qdrant_errors: AtomicU64,
}
```

Wire increments into the right places in the broker. Look for existing `self.metrics.memory_search_calls.fetch_add(1, ...)` patterns and mirror them.

#### 3.2c: Extend MetricsSnapshotResponse

In `api.rs`, extend the response struct to expose the new fields. The `observe: action: metrics` call returns this.

```rust
pub struct MetricsSnapshotResponse {
    // ... existing fields ...
    
    pub tool_calls: HashMap<String, u64>,
    pub memory_search_latency_p50_ms: u64,
    pub memory_search_latency_p95_ms: u64,
    pub memory_search_latency_p99_ms: u64,
    pub rerank_latency_p50_ms: u64,
    pub image_search_latency_p50_ms: u64,
    pub image_ingest_count: u64,
    pub image_search_count: u64,
    pub watcher_events_processed: u64,
    pub watcher_events_failed: u64,
    pub qdrant_errors: u64,
}
```

#### 3.2d: Prometheus export endpoint (optional, stretch)

Add a new MCP tool or an HTTP endpoint in the-one-ui that exports metrics in Prometheus format. This lets users scrape them into Grafana / other dashboards.

Route: `GET /metrics` on the admin UI.

```
# HELP the_one_mcp_memory_search_calls_total Number of memory.search calls
# TYPE the_one_mcp_memory_search_calls_total counter
the_one_mcp_memory_search_calls_total 42

# HELP the_one_mcp_memory_search_latency_ms Memory search latency
# TYPE the_one_mcp_memory_search_latency_ms histogram
the_one_mcp_memory_search_latency_ms_bucket{le="50"} 30
the_one_mcp_memory_search_latency_ms_bucket{le="100"} 40
the_one_mcp_memory_search_latency_ms_bucket{le="200"} 42
...
```

This is a stretch goal — add if time permits, defer otherwise.

#### 3.2e: Dogfood the metrics

After extending, run the-one-mcp in a real development context for a few days. Query `observe: action: metrics` regularly. Note:
- Which counters grow (used features)
- Which stay at 0 (dead code or unused features)
- What you wish you could see but can't
- Edge cases where counters don't make sense

Use findings to iterate on the metric set before shipping.

#### 3.2f: Documentation

- Update `docs/guides/troubleshooting.md` with "How to use metrics for debugging" section
- Update `docs/guides/api-reference.md` with the full response schema for `observe: metrics`
- Consider adding `docs/guides/observability.md` if the content grows large enough

### Files to touch

- `crates/the-one-mcp/src/broker.rs` — extend BrokerMetrics, wire increments
- `crates/the-one-mcp/src/api.rs` — extend MetricsSnapshotResponse
- `crates/the-one-ui/src/lib.rs` — optional: add /metrics Prometheus endpoint
- `docs/guides/troubleshooting.md` + possibly new `observability.md`
- `schemas/mcp/v1beta/` — update metrics snapshot response schema

### Tests

Add tests that verify:
- Metrics are incremented correctly on each operation
- `observe: metrics` returns a full snapshot
- Concurrent operations don't cause metric corruption
- Latency percentiles are calculated correctly (use a known sample set)

---

## Task 3.3: Backup / Migration via `maintain: backup` (v0.12.0)

### Why

Current state: A user's entire project state lives in `<project>/.the-one/`:
- `project.json` — manifest
- `state.db` — SQLite with approvals, audit events, managed_images
- `docs/` — managed markdown files
- `images/` — indexed images + thumbnails
- `qdrant/` — local Qdrant fallback data (if used)
- `config.json`

Plus global state in `~/.the-one/`:
- `catalog.db` — tool catalog
- `registry/` — custom tools per CLI
- `.fastembed_cache/` — downloaded models (huge, skip)

If a user wants to move to a new machine, there's no documented process. They have to figure out which directories to copy and hope they got the mix right. Remote Qdrant data isn't included.

### Solution

Add a new action under the existing `maintain` multiplexed tool: `maintain: action: backup`. It creates a tarball of all the necessary state that can be restored on another machine.

**Backup includes:**
- `<project>/.the-one/` (excluding `.fastembed_cache/` and `qdrant/` collections that aren't local)
- `~/.the-one/catalog.db`
- `~/.the-one/registry/`
- A manifest with version info, timestamp, Qdrant collection names

**Backup excludes:**
- `.fastembed_cache/` (models re-download on first use)
- External Qdrant data (user's responsibility to backup their Qdrant server)
- Local Qdrant storage (usually too large; document separately)

**Restore:**
- `maintain: action: restore` takes a tarball path and extracts to the right locations
- Validates manifest version compatibility
- Warns if Qdrant collection names in the manifest don't match current server

### API types

In `crates/the-one-mcp/src/api.rs`:

```rust
pub struct BackupRequest {
    pub project_root: String,
    pub project_id: String,
    pub output_path: String,  // where to write the tarball
    pub include_images: bool,  // default true
    pub include_qdrant_local: bool,  // default false (too large)
}

pub struct BackupResponse {
    pub output_path: String,
    pub size_bytes: u64,
    pub file_count: usize,
    pub manifest_version: String,
    pub qdrant_collections: Vec<String>,
}

pub struct RestoreRequest {
    pub backup_path: String,
    pub target_project_root: String,
    pub target_project_id: String,
    pub overwrite_existing: bool,
}

pub struct RestoreResponse {
    pub restored_files: usize,
    pub warnings: Vec<String>,
}
```

### Manifest format

Embedded in the tarball as `backup-manifest.json`:

```json
{
  "version": "1",
  "the_one_mcp_version": "0.12.0",
  "created_at_epoch": 1712345678,
  "project_id": "my-project",
  "file_count": 142,
  "total_size_bytes": 2345678,
  "qdrant_collections": ["the_one_my-project", "the_one_images_my-project"],
  "includes": ["docs", "images", "config", "catalog", "registry"],
  "excludes": [".fastembed_cache", "qdrant_local"]
}
```

### Implementation

Create `crates/the-one-mcp/src/backup.rs`:

```rust
use std::path::Path;
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::Builder;

pub fn create_backup(
    project_root: &Path,
    project_id: &str,
    output: &Path,
    include_images: bool,
) -> Result<BackupResponse, String> {
    // 1. Open a gzipped tar writer to output path
    // 2. Add <project>/.the-one/ contents (recursive)
    //    - Skip .fastembed_cache
    //    - Skip qdrant_local unless opted in
    //    - Skip images if include_images = false
    // 3. Add ~/.the-one/catalog.db
    // 4. Add ~/.the-one/registry/
    // 5. Write backup-manifest.json
    // 6. Finalize
    // 7. Return size + file count
}

pub fn restore_backup(
    backup_path: &Path,
    target_project_root: &Path,
    target_project_id: &str,
    overwrite: bool,
) -> Result<RestoreResponse, String> {
    // 1. Open tarball
    // 2. Read manifest, validate version
    // 3. Extract to target paths
    //    - <project>/.the-one/ gets extracted to target_project_root/.the-one/
    //    - Catalog + registry extracted to ~/.the-one/
    // 4. Warn if target already has state (unless overwrite)
    // 5. Return count + warnings
}
```

Dependencies: `tar = "0.4"`, `flate2 = "1"`.

### Wire into `maintain` dispatcher

In `crates/the-one-mcp/src/transport/jsonrpc.rs`, find `dispatch_maintain`. Add two new action arms:

```rust
"backup" => {
    let project_root = params["project_root"].as_str().ok_or("missing project_root")?;
    let project_id = params["project_id"].as_str().ok_or("missing project_id")?;
    let output_path = params["output_path"].as_str().ok_or("missing output_path")?;
    let include_images = params["include_images"].as_bool().unwrap_or(true);
    let include_qdrant_local = params["include_qdrant_local"].as_bool().unwrap_or(false);
    let result = broker.backup_project(BackupRequest {
        project_root: project_root.to_string(),
        project_id: project_id.to_string(),
        output_path: output_path.to_string(),
        include_images,
        include_qdrant_local,
    }).await.map_err(|e| e.to_string())?;
    serde_json::to_value(result).map_err(|e| e.to_string())
}
"restore" => {
    // similar pattern
}
```

Update the `maintain` tool schema in `tools.rs` to include `backup` and `restore` in the `action` enum.

### Tests

```rust
#[tokio::test]
async fn test_backup_creates_tarball() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".the-one/docs")).unwrap();
    std::fs::write(project_root.join(".the-one/docs/readme.md"), "# Hello").unwrap();
    
    let output = tmp.path().join("backup.tar.gz");
    let broker = McpBroker::new();
    let response = broker.backup_project(BackupRequest {
        project_root: project_root.display().to_string(),
        project_id: "test".to_string(),
        output_path: output.display().to_string(),
        include_images: false,
        include_qdrant_local: false,
    }).await.unwrap();
    
    assert!(output.exists());
    assert!(response.size_bytes > 0);
    assert!(response.file_count >= 1);
}

#[tokio::test]
async fn test_restore_roundtrip() {
    // 1. Create project with docs
    // 2. Backup
    // 3. Create empty project
    // 4. Restore into it
    // 5. Verify docs exist in the new project
}

#[tokio::test]
async fn test_backup_excludes_fastembed_cache() {
    // Verify that even if .fastembed_cache exists in the project, it's not in the tarball
}

#[tokio::test]
async fn test_restore_warns_on_existing_state_without_overwrite() {
    // ...
}
```

### Documentation

- Update `docs/guides/troubleshooting.md` with "Moving to a new machine" section
- Update `docs/guides/api-reference.md` with the new `maintain` actions
- Maybe add `docs/guides/backup-restore.md` if the content grows large

### User-facing flow

```bash
# On old machine
$ claude
# In chat: "Back up this project to ~/Desktop/my-project-backup.tar.gz"
# LLM calls: maintain action=backup params={project_root, project_id, output_path}

# Copy the tarball to the new machine (scp, cloud drive, etc.)

# On new machine
$ curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
$ claude
# In chat: "Restore the backup from ~/Desktop/my-project-backup.tar.gz into /path/to/project"
# LLM calls: maintain action=restore params={backup_path, target_project_root, target_project_id}
```

### Caveats to document

- Qdrant data for remote servers is NOT backed up — user must backup their Qdrant server separately
- Embedding models are NOT backed up — will re-download on first use after restore
- Tool catalog at ~/.the-one/catalog.db is shared across projects — backing up once is enough

---

## Phase 3 Release Sequence

### v0.11.0 (Task 3.1 only)

Intel Mac tract backend is a substantial change (new dep, new CI path, potential performance impact on that platform). Ship as its own release with clear notes.

```bash
echo "v0.11.0" > VERSION
# Update CHANGELOG, INSTALL.md, troubleshooting.md
git tag -a v0.11.0 -m "v0.11.0: Intel Mac local embeddings via ort-tract"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.11.0
```

### v0.12.0 (Tasks 3.2 + 3.3)

Bundle observability + backup because:
- Both are operational/housekeeping improvements
- Neither is user-facing in a way that changes API surface
- Both touch similar files (broker.rs, api.rs, jsonrpc.rs for maintain/observe)
- One release is cleaner than two small ones

```bash
echo "v0.12.0" > VERSION
# Update CHANGELOG:
# ## [0.12.0] - YYYY-MM-DD
# ### Added
# - Observability: extended BrokerMetrics with per-tool call counts, latency histograms, watcher event counters, Qdrant error counters
# - Optional Prometheus export endpoint at /metrics in the admin UI
# - maintain action: backup — creates a gzipped tarball of project state + global catalog/registry
# - maintain action: restore — restores from a backup tarball
# - docs/guides/observability.md (if created)
# - docs/guides/backup-restore.md (if created)
# ### Changed
# - MetricsSnapshotResponse now includes latency percentiles and per-tool counts

git tag -a v0.12.0 -m "v0.12.0: observability deep dive + backup/restore"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.12.0
```

---

## Phase 3 success criteria

- [ ] v0.11.0 ships with Intel Mac building with `local-embeddings-tract` feature enabled
- [ ] v0.12.0 ships 6/6 platforms
- [ ] `observe: action: metrics` returns per-tool counts and latency percentiles
- [ ] `maintain: action: backup` creates a restorable tarball
- [ ] `maintain: action: restore` successfully restores on a fresh machine
- [ ] Tests still green (target ~370-400 total after Phase 3)
- [ ] Documentation updated for all new features
- [ ] `.fastembed_cache/` untouched (user explicitly wants these preserved)

---

## Cross-phase considerations

### Session budget

Each phase is roughly 1-2 weeks of focused work if done without interruptions. Parallel work across phases is possible because:
- Phase 1 is about code quality and credibility
- Phase 2 is about reach and growth
- Phase 3 is about polish and operations

Realistic sequence: ship Phase 1 entirely (both v0.8.2 and v0.9.0), then Phase 2 (v0.10.0 + landing page), then Phase 3 (v0.11.0 + v0.12.0).

### What NOT to do during these phases

- Don't break existing 17 tools
- Don't remove any Cargo features (only add)
- Don't touch `.fastembed_cache/` directories
- Don't add new multiplexed admin tools (the 4 we have are enough)
- Don't chase experimental grammars (stick with tree-sitter grammars that have active maintenance)
- Don't skip verification — always run the full fmt + clippy (default + lean) + test pipeline before committing

### If you run out of session budget mid-task

All tasks are designed to be checkpointable at phase boundaries. Each sub-task ends with a working `cargo test` state, so you can stop at any sub-task boundary without leaving broken code.

If forced to stop mid-task, commit a WIP tag and document exactly where you left off in the task's markdown plan.

### Dogfood suggestion

After Phase 1 ships, USE the-one-mcp for your own work for a week before starting Phase 2. This is the highest-value activity on the entire roadmap — it will reveal bugs, ergonomic friction, and priority adjustments that these plans cannot anticipate. Keep a notes file of pain points. It becomes the v0.13.0 roadmap.
