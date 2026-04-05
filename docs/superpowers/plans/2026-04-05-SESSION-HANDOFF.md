# Session Handoff — v0.8.1 → v0.12.0 Roadmap

**Date:** 2026-04-05
**Current shipped version:** v0.8.1 (6/6 platforms, full binaries)
**Purpose:** Self-contained handoff for a fresh Claude Code session to continue work on the-one-mcp without needing conversation history.

---

## Project state at end of previous session

### Stats
- **17 MCP tools** (11 work + 4 multiplexed admin + 2 image) — consolidated from 33 in v0.5.0
- **272 tests** passing (was 174 at session start, +98)
- **35 JSON schemas** in `schemas/mcp/v1beta/`
- **~21,000 Rust LOC** across 8 workspace crates
- **6 platforms** building clean (Linux x86_64 + aarch64, macOS x86_64 lean + aarch64, Windows x86_64 + aarch64)
- **15 user guides** in `docs/guides/` + 8 root docs, all audited for v0.8.0/v0.8.1 accuracy
- **CI green** on both default and lean feature matrices with `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings`

### Architecture (quick reference)

Workspace crates:
- `the-one-core` — config, storage (SQLite), policy, profiler, docs manager, tool catalog
- `the-one-memory` — RAG: chunker, embeddings, sparse, reranker, image, Qdrant, graph, watcher
- `the-one-mcp` — async broker, API types, JSON-RPC transport, CLI binary
- `the-one-router` — request routing, provider pool, health tracking
- `the-one-registry` — capability registry
- `the-one-claude` — Claude Code adapter
- `the-one-codex` — Codex adapter
- `the-one-ui` — embedded admin UI (dashboard, config, audit, images, swagger)

Key patterns:
- **`memory_by_project`** is `Arc<RwLock<HashMap<String, MemoryEngine>>>` (v0.8.0) so the watcher's spawned tokio task can hold its own reference
- **Feature flags:** `local-embeddings` (default), `image-embeddings` (default), `image-ocr` (opt-in), `embed-swagger` (default)
- **Intel Mac** ships lean (no `local-embeddings`) because `ort-sys 2.0.0-rc.11` dropped prebuilt binaries
- **Watcher auto-reindex** works for markdown (live), image events log-only (Phase 1 task)

### What shipped in this session

| Version | Headline |
|---------|----------|
| v0.5.0 | Tool consolidation 33→15 |
| v0.6.0 | Multimodal images + OCR + reranking + fastembed 5.13 bump |
| v0.7.0 | Hybrid search (BM25+dense) + file watcher + admin UI gallery + screenshot search |
| v0.7.1 | Intel Mac embedded-ui lean build fix |
| v0.8.0 | Watcher auto-reindex (markdown) + code-aware chunker (Rust/Python/TS/JS/Go) |
| v0.8.1 | Docs refresh — 11/15 guides + 7/8 root docs updated |

### Deferred items (from v0.8.0 spec)

1. **Image auto-reindex** — watcher detects image changes but only logs. Phase 1 task.
2. **Tree-sitter chunker** — regex-based chunkers work for 80-90%, tree-sitter would handle the rest + more languages. Phase 1 task.
3. **Benchmarks** — claims of 15-30% quality improvements from hybrid/rerank have no measurements. Phase 1 task.

---

## The 9-item roadmap

Grouped into 3 phases. Each phase has its own detailed plan file with full deliverables, file paths, and verification steps.

### Phase 1: Quality & Credibility (v0.8.2 + v0.9.0)
**File:** `2026-04-05-phase1-quality-credibility.md`

1. **Image auto-reindex** (v0.8.2) — finish what v0.8.0 started
2. **Tree-sitter chunker** (v0.9.0) — upgrade regex → AST parsing, add Swift/Ruby/C/Java/Kotlin/Zig
3. **Benchmark suite** (v0.9.0) — measure retrieval quality, publish numbers

Time: ~1-2 weeks. Ships 2 releases.

### Phase 2: Reach & Growth (v0.10.0)
**File:** `2026-04-05-phase2-reach-growth.md`

4. **MCP resources API** — expose indexed docs as `resources/list` / `resources/read` native MCP resources
5. **Catalog expansion** — Python, JavaScript, Go, Java tools (~200 additions)
6. **Landing page via GitHub Pages** — hero, 30-sec demo, benchmarks, catalog browser

Time: ~1-2 weeks. Ships 1 release + GitHub Pages site.

### Phase 3: Polish (v0.11.0 + v0.12.0)
**File:** `2026-04-05-phase3-polish.md`

7. **Intel Mac ort-tract backend** — restore local embeddings for Intel Mac via pure-Rust ONNX runtime
8. **Observability deep dive** — add missing counters, exercise `observe` tool, dashboard
9. **Backup / migration story** — `maintain: backup` action, restore tarball, export Qdrant + SQLite

Time: ~1 week. Ships 1-2 releases.

---

## How to continue in a fresh session

1. **Start fresh** with: `claude --dangerously-skip-permissions`
2. **Open the project**: `cd /home/michel/projects/the-one-mcp`
3. **Read the handoff** (this file) + the phase plan for whatever you're starting with
4. **Run initial state check**:
   ```bash
   git log --oneline -5
   cargo test --workspace 2>&1 | grep "^test result" | tail -5
   cat VERSION
   ```
   Expected: v0.8.1, 272 tests, recent commits from 2026-04-05 docs audit
5. **Pick a phase** and follow its tasks in order
6. **Each task** has detailed deliverables, file paths, and verification steps — no additional context needed

## Critical conventions to remember (for any session)

### Verification before claiming done
Agents have lied about build state in this project. ALWAYS run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --no-default-features --features the-one-ui/embed-swagger --all-targets -- -D warnings
THE_ONE_EMBEDDING_MODEL=all-MiniLM-L6-v2 cargo test --workspace 2>&1 | grep "^test result"
```
Never claim "clean" without actually running these. rust-analyzer diagnostics are unreliable due to stale ABI cache — trust `cargo` output only.

### Rustfmt version matters
CI uses `stable` which pulls latest. Local rustfmt must match. If CI fails on fmt, run `rustup update stable` and re-run `cargo fmt`.

### CI environment variable
The release workflow sets `THE_ONE_EMBEDDING_MODEL=all-MiniLM-L6-v2` in the test step to avoid parallel download contention on BGE-large (130MB). Tests depending on FastEmbed will fail in parallel without this.

### Release process
```bash
git tag -a vX.Y.Z -m "message"
git push origin main --tags
echo "y" | bash scripts/build.sh release vX.Y.Z
```
Monitor: `gh run list --workflow release.yml --limit 1`

### Local cache
`.fastembed_cache/` directories in 4 crates contain ~33GB of downloaded ONNX models. **DO NOT TOUCH** — user explicitly wants these preserved.

### Orphan folder
`/home/michel/projects/the-one-mcp/the-one-mcp-models-cache/` is an untracked empty git repo from an earlier session. User said to leave it. Ignore in git operations.

### Subagent dispatch wisdom
- Previous agents have lied about "success" while leaving broken code
- Always verify yourself after an agent returns
- Prefer serial dispatch when tasks touch the same files
- rust-analyzer diagnostics are often stale during/after agent edits — trust `cargo` not the IDE
- If an agent hits a "linter reverts my changes" loop, have it use Write instead of Edit to write the full file at once

## Reference files to read first

For any new session working on this project:

1. **This handoff** (you're reading it)
2. **`CLAUDE.md`** — build commands, architecture, code conventions
3. **`docs/superpowers/specs/2026-04-05-v080-auto-reindex-code-chunker-design.md`** — latest design spec, establishes patterns for subsequent work
4. **The phase plan** you're executing

That's sufficient context. No need to read conversation history.
