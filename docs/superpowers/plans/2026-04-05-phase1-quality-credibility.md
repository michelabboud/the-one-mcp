# Phase 1: Quality & Credibility (v0.8.2 + v0.9.0)

**Scope:** Finish what v0.8.0 started (image auto-reindex), upgrade chunker infrastructure to tree-sitter, and publish benchmarks that prove our quality claims.

**Prerequisites:**
- Start at v0.8.1 (docs refresh, 272 tests)
- Read `2026-04-05-SESSION-HANDOFF.md` first
- Verify baseline: `git log --oneline -5 && cargo test --workspace 2>&1 | grep "^test result"`

**Deliverables:**
- v0.8.2 release: image auto-reindex complete, all 6 platforms
- v0.9.0 release: tree-sitter chunker for 10+ languages, benchmark suite, all 6 platforms

---

## Task 1.1: Image Auto-Reindex (v0.8.2)

### Why

v0.8.0 wired markdown auto-reindex but image events only log. The `auto-indexing.md` guide documents this gap as "deferred to v0.8.2". Closing this is a 1-hour task that completes the feature.

### Current state

File: `crates/the-one-mcp/src/broker.rs` — `maybe_spawn_watcher` function spawns a tokio task that matches on `WatchEvent::Upserted(path)` / `WatchEvent::Removed(path)`. For markdown files it calls `ingest_single_markdown` / `remove_by_path` on the MemoryEngine. For image files it currently logs a "manual reindex needed" message.

### What to build

1. Add `image_ingest_single(path)` and `image_remove_by_path(path)` helpers on the broker (not MemoryEngine — image ingest requires config, Qdrant image collection, and optional OCR/thumbnail generation which live at broker level).

2. Wire the watcher task to call these helpers on image events.

### Files to touch

- **`crates/the-one-mcp/src/broker.rs`** — the watcher task body, plus new internal helpers

Look for `maybe_spawn_watcher` or similar function. The tokio task body currently has a match arm like:

```rust
(the_one_memory::watcher::WatchEvent::Upserted(p), _, true) => {
    Ok(format!("image change detected (manual reindex needed): {}", p.display()))
}
```

Replace with actual ingest. You'll need access to the broker for config + image provider + Qdrant. Since `Arc<RwLock<HashMap<String, MemoryEngine>>>` is already shared with the task, the simplest approach is to also share an `Arc<Self>` (Broker) — if not already cloneable, you may need to wrap larger state in Arc.

### Approach options

**Option A — Share Arc<McpBroker> with task:** Clone an `Arc<Self>` into the spawn. Requires McpBroker to be inside an Arc at the call site. Check how `maybe_spawn_watcher` is currently called — if it takes `&self`, you'll need a refactor.

**Option B — Extract image ingest into a standalone helper:** Create a `pub(crate) async fn image_ingest_standalone(project_root, project_id, path, config)` that takes only what it needs (no `self`). Share that function's requirements (config, image provider cache, Qdrant handle) via Arc into the spawn. Cleaner separation.

**Recommendation:** Option B. Extract the image ingest pipeline (decode bytes, embed, OCR if enabled, thumbnail, Qdrant upsert) into a standalone function that takes explicit arguments. The existing `broker.image_ingest()` method becomes a thin wrapper calling the standalone helper.

### Implementation sketch

```rust
// In broker.rs

#[cfg(feature = "image-embeddings")]
pub(crate) async fn image_ingest_standalone(
    project_root: &std::path::Path,
    project_id: &str,
    image_path: &std::path::Path,
    config: &the_one_core::config::AppConfig,
) -> Result<(), String> {
    // Validate file exists, has allowed extension, is under size limit
    // Load FastEmbedImageProvider based on config.image_embedding_model
    // Embed image
    // If config.image_ocr_enabled and #[cfg(feature = "image-ocr")]: extract OCR
    // If config.image_thumbnail_enabled: generate thumbnail
    // Upsert to Qdrant image collection
    // Update SQLite managed_images table
    Ok(())
}

#[cfg(feature = "image-embeddings")]
pub(crate) async fn image_remove_standalone(
    project_root: &std::path::Path,
    project_id: &str,
    image_path: &std::path::Path,
) -> Result<(), String> {
    // Delete from Qdrant image collection by source_path
    // Delete from SQLite managed_images table
    Ok(())
}
```

Then in `maybe_spawn_watcher`, the task can call these functions since they take owned or cloneable arguments, not `&self`.

### Tests to add

```rust
#[tokio::test]
#[cfg(feature = "image-embeddings")]
async fn test_watcher_auto_reindex_image_upsert() {
    // 1. Create temp project with .the-one/images/
    // 2. Enable auto_index_enabled + image_embedding_enabled in config
    // 3. Start broker, let watcher spawn
    // 4. Write a test PNG to .the-one/images/
    // 5. Wait up to 5 seconds
    // 6. Verify the image appears in Qdrant image collection
}

#[tokio::test]
#[cfg(feature = "image-embeddings")]
async fn test_watcher_auto_reindex_image_delete() {
    // Same setup + ingest, then delete the file, verify removal
}
```

Mark with `#[ignore]` if timing-dependent. The broker has an existing `#[ignore]` watcher integration test — follow the same pattern.

### Verification

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --no-default-features --features the-one-ui/embed-swagger --all-targets -- -D warnings
THE_ONE_EMBEDDING_MODEL=all-MiniLM-L6-v2 cargo test --workspace 2>&1 | grep "^test result"
```

All must pass. Target: 272 + 2-3 new tests = ~274-275.

### Release v0.8.2

```bash
# Update VERSION
echo "v0.8.2" > VERSION

# Update CHANGELOG.md — add [0.8.2] entry at top:
# ## [0.8.2] - YYYY-MM-DD
# ### Added
# - Image auto-reindex: file watcher now actually re-ingests changed images (PNG/JPG/WebP) in addition to markdown. Closes the TODO from v0.8.0.
# - Broker helpers: image_ingest_standalone and image_remove_standalone for watcher-triggered ingest paths
# ### Fixed
# - (nothing else)

# Update PROGRESS.md — bump current version, add v0.8.2 bullet
# Update auto-indexing.md — remove "image auto-reindex deferred" note

git add -A
git commit -m "feat: v0.8.2 — image auto-reindex via watcher

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"

git tag -a v0.8.2 -m "v0.8.2: image auto-reindex"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.8.2
```

Monitor build with `gh run list --workflow release.yml --limit 1` — should take ~12 minutes for all 6 platforms.

---

## Task 1.2: Tree-Sitter Chunker (v0.9.0)

### Why

v0.8.0 shipped regex-based code chunkers for 5 languages (Rust, Python, TS, JS, Go). Regex handles 80-90% of real-world code but has known edge cases:
- Rust `impl<T>` with complex where clauses
- Python decorators with multi-line arguments
- TypeScript generics with nested template literals
- Go method receivers with pointer types

Tree-sitter is the industry standard AST parser. It:
- Handles edge cases robustly
- Has prebuilt grammars for 100+ languages
- Each grammar is 100-500KB
- Adds ~5-10MB binary size for the languages we want

### Languages to add via tree-sitter

Keep existing 5 regex chunkers as fallback when a tree-sitter grammar is unavailable or fails. Add these new languages:
- **Swift** — growing language, modern features
- **Ruby** — large Rails ecosystem
- **C** — foundational
- **C++** — common in systems code
- **Java** — enterprise
- **Kotlin** — Android, modern JVM
- **Zig** — emerging systems language
- **PHP** — WordPress + Laravel ecosystems

That's 8 new languages + 5 existing = 13 total.

### Strategy

**Phase 1.2a:** Add tree-sitter infrastructure
- Add `tree-sitter` crate (core parser) + individual grammar crates
- Create `crates/the-one-memory/src/chunker_ts_impl.rs` (tree-sitter impl) — note: "ts" here means "tree-sitter", not TypeScript
- Add `chunk_with_tree_sitter(language, source_path, content, max_tokens)` that takes any supported language and uses tree-sitter to walk the AST, extracting top-level declarations

**Phase 1.2b:** Migrate existing 5 languages
- Rust, Python, TS, JS, Go each get a tree-sitter-based replacement
- Keep old regex versions as `chunker_rust_regex.rs` etc. for fallback
- `chunk_file` dispatcher tries tree-sitter first, falls back to regex on failure

**Phase 1.2c:** Add 8 new languages
- Swift, Ruby, C, C++, Java, Kotlin, Zig, PHP
- Each uses the shared tree-sitter pipeline
- No regex fallback needed for these — tree-sitter is the only implementation

### Files to create

- `crates/the-one-memory/src/chunker_ts_impl.rs` — shared tree-sitter walker
- `crates/the-one-memory/src/chunker_swift.rs`
- `crates/the-one-memory/src/chunker_ruby.rs`
- `crates/the-one-memory/src/chunker_c.rs`
- `crates/the-one-memory/src/chunker_cpp.rs`
- `crates/the-one-memory/src/chunker_java.rs`
- `crates/the-one-memory/src/chunker_kotlin.rs`
- `crates/the-one-memory/src/chunker_zig.rs`
- `crates/the-one-memory/src/chunker_php.rs`
- Test fixtures in `crates/the-one-memory/tests/fixtures/code/sample.{swift,rb,c,cpp,java,kt,zig,php}`

### Files to modify

- `Cargo.toml` (workspace) — add tree-sitter deps
- `crates/the-one-memory/Cargo.toml` — add tree-sitter + grammar crates
- `crates/the-one-memory/src/chunker.rs` — extend `chunk_file` dispatcher with 8 new extensions
- `crates/the-one-memory/src/lib.rs` — export new chunker modules

### Dependency additions

Check crates.io for the current versions of these. Likely:
```toml
tree-sitter = "0.25"
tree-sitter-rust = "0.23"
tree-sitter-python = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-go = "0.23"
tree-sitter-swift = "0.6"  # check — community crate
tree-sitter-ruby = "0.23"
tree-sitter-c = "0.23"
tree-sitter-cpp = "0.23"
tree-sitter-java = "0.23"
tree-sitter-kotlin = "1.1"  # community
tree-sitter-zig = "1.1"     # community
tree-sitter-php = "0.23"
```

**Feature flag:** Gate behind `tree-sitter-chunker` feature (default on). Users who want the leanest binary can disable.

### Implementation pattern

```rust
// chunker_ts_impl.rs

use crate::chunker::ChunkMeta;
use tree_sitter::{Node, Parser, Tree};

pub fn chunk_with_tree_sitter(
    language: tree_sitter::Language,
    language_name: &str,
    source_path: &str,
    content: &str,
    max_tokens: usize,
    top_level_node_kinds: &[&str],
    name_field: &str,
) -> Vec<ChunkMeta> {
    let mut parser = Parser::new();
    parser.set_language(&language).expect("set language");
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    
    let mut chunks = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    
    // Walk top-level children
    for child in root.children(&mut cursor) {
        if top_level_node_kinds.contains(&child.kind()) {
            let start_line = child.start_position().row + 1;
            let end_line = child.end_position().row + 1;
            let content_slice = &content[child.byte_range()];
            
            let symbol = extract_symbol(&child, content, name_field);
            let signature = content_slice.lines().next().unwrap_or("").to_string();
            
            chunks.push(ChunkMeta {
                id: format!("{source_path}:{}", chunks.len()),
                source_path: source_path.to_string(),
                chunk_index: chunks.len(),
                content: content_slice.to_string(),
                heading_hierarchy: vec![],
                byte_offset: child.start_byte(),
                byte_length: child.end_byte() - child.start_byte(),
                content_hash: 0, // fill in properly
                language: Some(language_name.to_string()),
                symbol: Some(symbol),
                signature: Some(signature),
                line_range: Some((start_line, end_line)),
            });
        }
    }
    
    chunks
}

fn extract_symbol(node: &Node, content: &str, name_field: &str) -> String {
    if let Some(name_node) = node.child_by_field_name(name_field) {
        content[name_node.byte_range()].to_string()
    } else {
        node.kind().to_string()
    }
}
```

Each language-specific file calls this with its node kinds:

```rust
// chunker_swift.rs
pub fn chunk_swift(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    crate::chunker_ts_impl::chunk_with_tree_sitter(
        tree_sitter_swift::language(),
        "swift",
        source_path,
        content,
        max_tokens,
        &["function_declaration", "class_declaration", "struct_declaration", "protocol_declaration", "extension_declaration"],
        "name",
    )
}
```

### Tests per language

For each language, create a fixture file and test that:
1. Top-level items are detected
2. Symbol names are extracted correctly
3. Line ranges are accurate
4. Language metadata is "swift"/"ruby"/etc

Target: ~3 tests per language × 13 languages = ~40 new tests.

### Verification

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --no-default-features --features the-one-ui/embed-swagger --all-targets -- -D warnings
THE_ONE_EMBEDDING_MODEL=all-MiniLM-L6-v2 cargo test --workspace 2>&1 | grep "^test result"
```

Target: 272 + ~40 new tests + Task 1.1's ~3 = ~315 total.

Binary size check:
```bash
cargo build --release -p the-one-mcp --bin the-one-mcp
ls -lh target/release/the-one-mcp
```

Expect growth from ~50MB (v0.8.1) to ~60-70MB with all grammars included.

### Risks

- **Grammar crate churn:** Community crates (swift, kotlin, zig) may have breaking changes. Pin specific versions.
- **Build time:** Each grammar compiles a C file. Cold cargo build may go from 30s to 60-90s. Incremental builds unaffected.
- **CI build time:** 6 platforms × longer compile = total release time from 10min → 20min. Acceptable but notable.
- **License compatibility:** Check each tree-sitter grammar license. Most are MIT/Apache, but verify.

---

## Task 1.3: Benchmark Suite (v0.9.0)

### Why

The project's README claims:
- "Hybrid search improves exact-match retrieval"
- "Cross-encoder reranking typically improves quality 15-30%"
- "Semantic search finds relevant chunks"

None of these are measured. A simple benchmark would:
- Validate or refute these claims
- Find regressions over time
- Give us numbers to put in the README (credibility)
- Expose performance bottlenecks

### Benchmark design

**Corpus:** Use the-one-mcp's own source tree + docs. ~21,000 Rust LOC + 15 guides (~7,500 lines). This is realistic because it's what developers index.

**Query sets:** Three tiers

1. **Exact-match queries (50)** — function names, type names, error strings, config fields
   - e.g., "parse_config", "McpBroker", "image_embedding_enabled", "ort-sys prebuilts dropped"
   - Expected top result: the chunk containing that exact string
   - Measures: does sparse/BM25/SPLADE actually help?

2. **Semantic queries (50)** — natural-language descriptions of code behavior
   - e.g., "how does auto-reindex work", "which chunker handles TypeScript", "what is the Qdrant collection naming pattern"
   - Expected top result: the most relevant chunk (hand-labeled)
   - Measures: does dense embedding quality?

3. **Mixed queries (25)** — combinations of exact + semantic
   - e.g., "bm25_normalize function implementation details"
   - Measures: does hybrid search actually outperform either alone?

### Metrics

- **Recall@1** — top result is the expected chunk (or synonym)
- **Recall@5** — expected chunk in top 5
- **MRR** (mean reciprocal rank)
- **Latency p50, p95** per query
- **Index build time** for the corpus

### Configurations to benchmark

1. Dense only (baseline) — current default
2. Dense + rerank
3. Dense + sparse hybrid
4. Dense + sparse hybrid + rerank (full pipeline)

Run each config against all 3 query sets. Report a 4×3 matrix of metrics.

### Implementation

**Create `crates/the-one-memory/benches/retrieval_bench.rs`:**

```rust
//! Retrieval quality benchmark for the-one-mcp.
//!
//! Usage:
//!     cargo bench -p the-one-memory --bench retrieval_bench
//!
//! Or for quick iteration:
//!     cargo test -p the-one-memory --test retrieval_bench --release

use std::path::PathBuf;
use the_one_memory::{MemoryEngine, /* ... */};

#[derive(Debug)]
struct QueryCase {
    query: String,
    expected_chunks: Vec<String>,  // substrings that identify correct chunks
    category: &'static str,
}

fn load_exact_match_queries() -> Vec<QueryCase> {
    vec![
        QueryCase {
            query: "parse_config".to_string(),
            expected_chunks: vec!["fn parse_config".to_string()],
            category: "exact",
        },
        // ... 50 more
    ]
}

fn load_semantic_queries() -> Vec<QueryCase> { /* ... */ }
fn load_mixed_queries() -> Vec<QueryCase> { /* ... */ }

async fn run_benchmark(
    engine: &MemoryEngine,
    queries: &[QueryCase],
    config_name: &str,
) -> BenchmarkResult {
    let mut recall_at_1 = 0;
    let mut recall_at_5 = 0;
    let mut reciprocal_ranks = Vec::new();
    let mut latencies_ms = Vec::new();

    for q in queries {
        let start = std::time::Instant::now();
        let results = engine.search(&q.query, 5, 0.0).await;
        let latency = start.elapsed().as_millis() as u64;
        latencies_ms.push(latency);

        let rank = results.iter().position(|r| 
            q.expected_chunks.iter().any(|expected| r.content.contains(expected))
        );

        if let Some(r) = rank {
            if r == 0 { recall_at_1 += 1; }
            if r < 5 { recall_at_5 += 1; }
            reciprocal_ranks.push(1.0 / (r as f32 + 1.0));
        }
    }

    let n = queries.len() as f32;
    BenchmarkResult {
        config_name: config_name.to_string(),
        recall_at_1: recall_at_1 as f32 / n,
        recall_at_5: recall_at_5 as f32 / n,
        mrr: reciprocal_ranks.iter().sum::<f32>() / n,
        latency_p50: percentile(&latencies_ms, 0.5),
        latency_p95: percentile(&latencies_ms, 0.95),
    }
}

fn percentile(values: &[u64], p: f32) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort();
    let idx = (sorted.len() as f32 * p) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[tokio::main]
async fn main() {
    // Index the-one-mcp's source + docs
    let corpus_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf();
    
    // Build engines for each config
    let dense_only = build_engine(&corpus_root, /* ... */).await;
    let dense_rerank = build_engine_with_rerank(&corpus_root, /* ... */).await;
    let hybrid = build_engine_with_sparse(&corpus_root, /* ... */).await;
    let full = build_engine_full(&corpus_root, /* ... */).await;
    
    let exact = load_exact_match_queries();
    let semantic = load_semantic_queries();
    let mixed = load_mixed_queries();
    
    // 4 configs × 3 query sets = 12 runs
    let results = vec![
        run_benchmark(&dense_only, &exact, "dense_only").await,
        // ... all 12 combinations
    ];
    
    print_markdown_table(&results);
}
```

### Output format

Produce a `benchmarks/results.md` file committed to the repo with a markdown table that gets copied into the README:

```markdown
| Config | Query Set | Recall@1 | Recall@5 | MRR | p50 latency | p95 latency |
|--------|-----------|----------|----------|-----|-------------|-------------|
| Dense only | Exact | 62% | 80% | 0.68 | 45ms | 120ms |
| Dense + rerank | Exact | 68% | 88% | 0.75 | 85ms | 180ms |
| Hybrid | Exact | 94% | 98% | 0.95 | 52ms | 135ms |
| Full pipeline | Exact | 96% | 99% | 0.97 | 95ms | 210ms |
| Dense only | Semantic | 72% | 90% | 0.78 | 45ms | 120ms |
| ... | ... | ... | ... | ... | ... | ... |
```

### Files to create

- `crates/the-one-memory/benches/retrieval_bench.rs` (main benchmark)
- `crates/the-one-memory/benches/queries_exact.json` (query corpus)
- `crates/the-one-memory/benches/queries_semantic.json`
- `crates/the-one-memory/benches/queries_mixed.json`
- `benchmarks/results.md` (generated output, committed)
- `benchmarks/README.md` (how to run, methodology)
- `scripts/run-benchmark.sh` (wrapper script)

### CI integration (optional)

Add a `benchmark` GitHub Action that runs on PRs labeled `benchmark`. Output a comment with the results table. Skip in regular CI (too slow).

### Verification

```bash
cargo bench -p the-one-memory --bench retrieval_bench 2>&1 | tail -50
# Or for faster iteration during dev:
cargo test --release -p the-one-memory --test retrieval_bench 2>&1 | tail -50
```

Expected: all configs produce non-zero recall, hybrid beats dense-only on exact queries, rerank beats baseline on semantic queries.

### README update

Add a "Benchmarks" section to `README.md` with the results table and a link to `benchmarks/results.md` for details.

---

## Phase 1 Release Sequence

### v0.8.2 (Task 1.1 only)

Ship image auto-reindex as a patch release. Small, focused, completes v0.8.0's promise.

```bash
# After Task 1.1 complete and verified:
git tag -a v0.8.2 -m "v0.8.2: image auto-reindex"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.8.2
```

### v0.9.0 (Tasks 1.2 + 1.3)

Minor version bump because tree-sitter is a significant change (new deps, binary size increase, broader language support).

Bundle tree-sitter + benchmarks in one release because:
- The benchmarks depend on a stable chunker for consistent results
- Tree-sitter is a retrieval-quality change, so it's natural to benchmark it against the regex baseline
- One release is cleaner than two back-to-back minor bumps

```bash
# After both Tasks 1.2 and 1.3 complete:
echo "v0.9.0" > VERSION
# Update CHANGELOG, PROGRESS, README (with benchmarks)
git tag -a v0.9.0 -m "v0.9.0: tree-sitter chunker (13 languages) + benchmark suite"
git push origin main --tags
echo "y" | bash scripts/build.sh release v0.9.0
```

### Test counts at end of phase

- v0.8.1 baseline: 272
- After v0.8.2 (Task 1.1): ~275
- After v0.9.0 (Task 1.2): ~315
- After v0.9.0 (Task 1.3): ~320 (benchmark adds a few tests, but most are #[bench] not #[test])

---

## Phase 1 risks

1. **Tree-sitter grammar instability:** Community crates (swift, kotlin, zig) may be abandoned or have breaking changes. Mitigation: pin exact versions, fall back to regex chunkers for these.

2. **Binary size regression:** Tree-sitter adds ~5-10MB per language. 13 languages could add ~80MB. Mitigation: feature-gate per language, allow opt-out via `--no-default-features --features tree-sitter-core --features tree-sitter-rust`.

3. **Benchmark results may be unflattering:** Hybrid search might NOT beat dense-only on our corpus. Mitigation: publish anyway, honesty builds trust. Use the results to guide v0.10.0 work.

4. **CI time growth:** Tree-sitter compilation slows the release pipeline. Monitor and potentially move non-Rust grammar compilation to a separate build step.

5. **Image ingest concurrency:** If multiple images change in rapid succession, the watcher's debounce batches them. Verify the standalone ingest functions are safe to call concurrently (they share a FastEmbedImageProvider which is `Arc<Mutex<...>>` — should be fine).

---

## Phase 1 success criteria

- [ ] v0.8.2 ships 6/6 platforms
- [ ] v0.9.0 ships 6/6 platforms
- [ ] 13 languages supported in chunk_file dispatcher
- [ ] Benchmark results published in README with real numbers
- [ ] CI green, tests 315+
- [ ] Existing features unchanged (no regressions)
- [ ] `.fastembed_cache/` untouched
