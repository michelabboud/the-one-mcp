# v0.8.0 Design: Watcher Auto-Reindex + Code-Aware Chunker

**Date:** 2026-04-05
**Target release:** v0.8.0
**Status:** Design — proceeding to implementation

## Goal

Two bundled features that together dramatically improve retrieval quality on live codebases:

1. **Auto-reindex on watcher events** — finish the file watcher so it actually re-ingests changed files instead of just logging. Closes the promise made in v0.7.0's `auto-indexing.md` guide.

2. **Code-aware chunker** — extend the markdown-only chunker with language-aware splitting for Rust, Python, TypeScript, JavaScript, and Go. Chunks respect function/class/struct boundaries and attach symbol metadata. Massive quality improvement for code search.

## Feature 1: Watcher Auto-Reindex

### Current state (v0.7.0)

The file watcher works — it spawns a tokio task, watches `.the-one/docs/` and `.the-one/images/`, emits debounced `WatchEvent::Upserted(path)` / `WatchEvent::Removed(path)` events. But the event handler just **logs** the event and tells the user to run `maintain:reindex`. No actual re-ingestion happens.

### Challenge: accessing the broker from a spawned task

The watcher task is spawned inside `maybe_spawn_watcher`, which runs during memory engine creation in the broker. The task needs to call broker methods like `docs_save` on incoming events, but:
- The broker is large and not trivially `Clone` or `Arc`-wrapped
- Spawning with `Arc<McpBroker>` would require refactoring the broker's ownership model
- Per-project state is keyed by `(project_root, project_id)` in HashMaps behind `RwLock`

### Solution: dedicated ingest channel

Add an mpsc channel per project. The watcher task sends `IngestCommand` messages on file events. A dedicated consumer task (or the broker on its next call) drains the channel.

```rust
enum IngestCommand {
    UpsertDoc { path: PathBuf },
    DeleteDoc { path: PathBuf },
    UpsertImage { path: PathBuf },
    DeleteImage { path: PathBuf },
}
```

**Option A (simpler):** Watcher task holds a `tokio::sync::mpsc::Sender<IngestCommand>` and sends commands. The broker spawns a consumer task during memory engine creation that owns the receiver and has `Arc<Self>` — wait, we have the same ownership problem.

**Option B (better):** Broker owns `Arc<Mutex<MemoryEngine>>` per project. The watcher's consumer task takes an `Arc<Mutex<MemoryEngine>>` and calls engine methods directly (bypassing the broker). This requires making `MemoryEngine` support single-file ingest.

**Option C (best):** Split the broker's memory operations into a `MemoryContext` struct that bundles `{memory_engine, docs_manager, project_root, project_id, config}` and is `Arc`-shareable. The watcher task takes an `Arc<MemoryContext>` and calls methods on it.

**Recommendation:** Option B. `MemoryEngine` is already behind a `tokio::sync::RwLock` in the broker's `memory_by_project` HashMap. Extract it to an `Arc<RwLock<MemoryEngine>>` so the watcher task can hold its own reference without touching the broker.

### New MemoryEngine methods

```rust
impl MemoryEngine {
    /// Ingest a single markdown file. Used by the watcher for incremental reindex.
    /// Removes existing chunks for that path first, then re-chunks and re-embeds.
    pub async fn ingest_single_markdown(&mut self, path: &Path) -> Result<usize, String>;

    /// Remove all chunks for a given file path from the index.
    /// Used by the watcher when a file is deleted.
    pub async fn remove_by_path(&mut self, path: &Path) -> Result<usize, String>;
}
```

For images, `image_ingest` already takes a single path — reuse it.

### Deletion handling

Current `docs_delete` moves to trash (user-initiated semantic). Watcher-triggered deletion is different: the user deleted the file directly, so we should just remove from the index without trashing (the file is already gone). Separate method `remove_by_path` handles this.

### Tests

- Integration test: create file → watcher fires → chunks appear in engine
- Integration test: delete file → watcher fires → chunks removed from engine
- Integration test: modify file → chunks re-created with new content
- Manual test: run with `auto_index_enabled: true` and verify logs show actual reindex

## Feature 2: Code-Aware Chunker

### Current state (v0.7.0)

`chunker.rs` has `chunk_markdown(path, content, max_tokens)` which:
- Parses markdown headings (`#`, `##`, etc.)
- Splits at heading boundaries
- Large sections split on paragraph boundaries
- Never splits inside a code block

For non-markdown files, the same function is called with the raw text. Result: code files get split arbitrarily at paragraph boundaries, with no respect for function/class structure.

### New: `chunk_file` dispatcher

```rust
pub fn chunk_file(path: &Path, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("md") | Some("markdown") => chunk_markdown(path.to_str().unwrap_or(""), content, max_tokens),
        Some("rs") => chunk_rust(path.to_str().unwrap_or(""), content, max_tokens),
        Some("py") => chunk_python(path.to_str().unwrap_or(""), content, max_tokens),
        Some("ts") | Some("tsx") => chunk_typescript(path.to_str().unwrap_or(""), content, max_tokens),
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => chunk_javascript(path.to_str().unwrap_or(""), content, max_tokens),
        Some("go") => chunk_go(path.to_str().unwrap_or(""), content, max_tokens),
        _ => chunk_text_fallback(path.to_str().unwrap_or(""), content, max_tokens),
    }
}
```

### Implementation strategy: regex-first

Tree-sitter gives robust parsing but adds ~5 language grammar dependencies (each 100-500KB). For v0.8.0, regex-based detection is sufficient for 80-90% of real-world files. Tree-sitter can be added in v0.9.0 if users hit edge cases.

### Rust chunker

Split on top-level items:
```
^(pub\s+)?(async\s+)?(fn|struct|enum|impl|trait|mod|type|const|static|macro_rules!)\s+
```

For each top-level item:
1. Find the item's start line
2. Track brace depth to find its end line
3. Extract the text, build a `ChunkMeta` with:
   - `source_path`
   - `language: "rust"`
   - `symbol: "fn parseConfig"` or `"impl Broker"` etc.
   - `signature: first line`
   - `line_range: (start, end)`
   - `content: full text`
4. If the chunk exceeds `max_tokens`, split inside at blank lines (fallback)

### Python chunker

Split on top-level `def`/`class`:
```
^(async\s+)?def\s+\w+|^class\s+\w+
```

Python uses indentation for scope — track indent level of the `def`/`class` line, include all subsequent lines with greater indent.

### TypeScript/JavaScript chunker

Split on top-level declarations:
```
^(export\s+)?(default\s+)?(async\s+)?(function|class|interface|type|const|let|var)\s+
```

Same brace-tracking approach as Rust. Handle arrow functions at top level (`const foo = () => { ... }`).

### Go chunker

Split on top-level:
```
^(func|type|var|const)\s+
```

Brace-tracking like Rust.

### Fallback (`chunk_text_fallback`)

For unknown extensions, split on blank lines and pack chunks up to `max_tokens`. Same approach as the old markdown fallback.

### Extended ChunkMeta

```rust
pub struct ChunkMeta {
    // existing fields
    pub id: String,
    pub source_path: String,
    pub chunk_index: usize,
    pub content: String,
    pub heading_hierarchy: Vec<String>,  // for markdown

    // NEW fields for code-aware chunking
    pub language: Option<String>,
    pub symbol: Option<String>,
    pub signature: Option<String>,
    pub line_range: Option<(usize, usize)>,
}
```

These new fields are `Option` for backward compat — markdown chunks leave them as `None`.

### Ingest pipeline change

`ingest_markdown_tree` is renamed internally to `ingest_tree` and walks **all** files, not just `.md`. The chunker dispatcher handles per-file-type logic. This is transparent to callers.

Actually — keep backward compat by adding a new method `ingest_tree` and keeping `ingest_markdown_tree` as a thin wrapper that calls it with `.md` filter. Let me verify in the design — no, simpler: just rename and add the new file-type logic. The old method is only called internally.

### Tests

- `chunk_rust` finds all top-level items in a fixture file
- `chunk_python` respects indentation
- `chunk_typescript` handles arrow functions
- `chunk_go` handles `func` and `type`
- Fallback works for unknown extensions
- Symbol metadata is attached correctly
- Long functions get split into sub-chunks

## Files to Create / Modify

### New files
- `crates/the-one-memory/src/chunker_rust.rs`
- `crates/the-one-memory/src/chunker_python.rs`
- `crates/the-one-memory/src/chunker_typescript.rs`
- `crates/the-one-memory/src/chunker_go.rs`
- `crates/the-one-memory/tests/fixtures/code/sample.rs`
- `crates/the-one-memory/tests/fixtures/code/sample.py`
- `crates/the-one-memory/tests/fixtures/code/sample.ts`
- `crates/the-one-memory/tests/fixtures/code/sample.go`
- `docs/guides/code-chunking.md`

### Modified files
- `crates/the-one-memory/src/chunker.rs` — add `chunk_file` dispatcher, `ChunkMeta` new fields, `chunk_text_fallback`
- `crates/the-one-memory/src/lib.rs` — new `ingest_single_markdown`, `remove_by_path`, update `ingest_markdown_tree` to use dispatcher
- `crates/the-one-mcp/src/broker.rs` — update watcher task to actually call ingest methods
- `CHANGELOG.md`, `README.md`, `PROGRESS.md`, `CLAUDE.md`, `VERSION`

## Token Cost Impact

None — no new tools. Extends existing `memory.search` and `setup:refresh` transparently.

## Tests

- Code chunker: ~20 new tests (4 languages × 5 scenarios each)
- Watcher integration: 3 new tests (upsert, remove, modify roundtrip)
- Existing 234 tests remain green

Target: 234 → ~257 tests.

## Success Criteria

- `auto_index_enabled: true` + modify a markdown file → chunks update in < 3 seconds
- `auto_index_enabled: true` + delete a file → chunks removed from index
- Searching for a Rust function name returns the specific chunk containing that function, with signature in metadata
- All 6 platforms still build clean
- `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` pass in both default and lean modes

## Rollout Phases

1. **Phase A** — `ingest_single_markdown` + `remove_by_path` on MemoryEngine (foundation for watcher)
2. **Phase B** — Watcher auto-reindex wiring in broker
3. **Phase C** — Code chunker dispatcher + Rust implementation
4. **Phase D** — Python, TypeScript, Go chunkers
5. **Phase E** — Docs, CHANGELOG, v0.8.0 release
