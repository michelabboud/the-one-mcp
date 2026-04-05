# Troubleshooting

Common problems and their fixes for the-one-mcp v0.8.0.

---

## Installation Issues

### `command not found: the-one-mcp`

The binary is installed but not on your `PATH`.

```bash
# Add to ~/.bashrc or ~/.zshrc:
export PATH="$HOME/.the-one/bin:$PATH"

# Reload immediately:
source ~/.bashrc    # or ~/.zshrc

# Verify:
which the-one-mcp
the-one-mcp --version
```

If the binary is not in `~/.the-one/bin`, check where the installer put it:

```bash
find ~/.the-one -name the-one-mcp 2>/dev/null
```

---

### Download fails in curl installer

The one-liner uses `curl` to download the release binary from GitHub.

**Check network connectivity first:**

```bash
curl -I https://github.com
```

**Proxy/firewall block?** Set the proxy before running the installer:

```bash
export HTTPS_PROXY=http://your-proxy:port
curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash
```

**Manual install (always works):**

1. Go to the [releases page](https://github.com/michelabboud/the-one-mcp/releases/latest)
2. Download the binary for your platform (e.g., `the-one-mcp-x86_64-unknown-linux-gnu`)
3. Install manually:

```bash
mkdir -p ~/.the-one/bin
mv ~/Downloads/the-one-mcp-* ~/.the-one/bin/the-one-mcp
chmod +x ~/.the-one/bin/the-one-mcp
echo 'export PATH="$HOME/.the-one/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

---

### Permission denied on install

```bash
# Option 1: fix ownership of the target directory
sudo chown -R "$USER" ~/.the-one

# Option 2: run the installer with sudo (not recommended for home-dir installs)
sudo bash scripts/install.sh

# Option 3: install to /usr/local/bin (system-wide)
sudo install -m 755 ~/.the-one/bin/the-one-mcp /usr/local/bin/the-one-mcp
```

---

### Binary fails to run on macOS (Gatekeeper)

macOS blocks unsigned binaries downloaded from the internet.

```
"the-one-mcp" cannot be opened because the developer cannot be verified.
```

**Fix — remove the quarantine attribute:**

```bash
xattr -d com.apple.quarantine ~/.the-one/bin/the-one-mcp
```

Or allow it via System Preferences → Privacy & Security → "Allow Anyway".

---

### Windows Git Bash quirks

- Use forward slashes for paths: `~/.the-one/bin/the-one-mcp`
- `source ~/.bashrc` may not work — restart the terminal instead
- If the binary is a `.exe`, the PATH entry needs to include the `.exe` extension in some contexts
- WSL2 is the recommended environment on Windows — install there and use the Linux binary

---

## Connection Issues

### MCP server doesn't show up in Claude Code

Check registration:

```bash
# List registered MCP servers
claude mcp list

# Expected output includes:
# the-one-mcp: ~/.the-one/bin/the-one-mcp serve
```

If it's missing, register manually:

```bash
claude mcp add the-one-mcp -- ~/.the-one/bin/the-one-mcp serve
```

Then restart the Claude Code session. MCP servers are discovered at session start via `initialize` → `tools/list`.

---

### MCP server shows up but `tools/list` returns nothing

This usually means the server started but the `initialize` handshake failed or returned an error.

**Check if the binary runs at all:**

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}' \
  | ~/.the-one/bin/the-one-mcp serve
```

Expected: a JSON response containing `"result"`. If you see an error, check logs:

```bash
RUST_LOG=debug ~/.the-one/bin/the-one-mcp serve 2>&1 | head -50
```

---

### stdio transport hangs

The stdio transport reads line-delimited JSON from stdin and writes to stdout. If either side buffers, the protocol stalls.

- **Do not add extra output** (print statements, banners) to the binary — they corrupt the JSON stream.
- Ensure no other process has stdin/stdout open to the same binary.
- Check that your MCP client sends a newline after each JSON object.

---

### SSE transport 404

The SSE endpoint is `/events`, not `/sse` or `/stream`.

```bash
# Correct:
curl -N http://localhost:3000/events

# Wrong:
curl -N http://localhost:3000/sse   # 404
```

Check the bind address and port via `THE_ONE_UI_BIND` env var (default: `127.0.0.1:3000`).

---

### Stream transport — wrong Content-Type

The stream transport requires `Content-Type: application/x-ndjson` on POST requests.

```bash
curl -X POST http://localhost:3000/stream \
  -H "Content-Type: application/x-ndjson" \
  --data-raw '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

---

## Qdrant Issues

### `qdrant connection refused`

Qdrant is not running or not listening on the configured address.

**Start Qdrant:**

```bash
# Docker (recommended):
docker run -p 6333:6333 -p 6334:6334 qdrant/qdrant

# Or via the installer (if Qdrant was set up with the install script):
~/.the-one/bin/start-qdrant.sh
```

**Or switch to keyword-only mode** (no Qdrant needed):

```json
// ~/.the-one/config.json
{
  "qdrant_url": ""
}
```

Without a Qdrant URL, `memory.search` falls back to keyword search using SQLite FTS5. Quality is lower but it works offline with no dependencies.

---

### `remote qdrant requires api key`

Your Qdrant instance has authentication enabled. Set the key in config:

```json
{
  "qdrant_url": "https://your-qdrant-host:6334",
  "qdrant_api_key": "your-api-key-here"
}
```

Or via environment variable:

```bash
export THE_ONE_QDRANT_API_KEY=your-api-key-here
```

---

### TLS certificate errors with Qdrant

For self-signed certificates, provide the CA cert path:

```json
{
  "qdrant_ca_cert_path": "/path/to/ca.crt"
}
```

Or disable TLS verification (not recommended for production):

```json
{
  "qdrant_tls_insecure": true
}
```

Environment equivalents: `THE_ONE_QDRANT_CA_CERT_PATH`, `THE_ONE_QDRANT_TLS_INSECURE=true`.

---

### Collection not found

Qdrant is running but the `the_one_docs` collection doesn't exist yet.

```bash
# Re-run project init to create the collection and re-index:
# (via MCP tool)
setup { "action": "refresh", "params": { "project_root": "/your/project", "project_id": "myproject" } }

# Or directly via maintain:
maintain { "action": "reindex", "params": { "project_root": "/your/project", "project_id": "myproject" } }
```

---

### Qdrant upsert fails

Two common causes:

1. **Disk space** — Qdrant stores vectors on disk. Check available space:
   ```bash
   df -h ~/.qdrant   # or wherever your Qdrant data dir is
   ```

2. **Dimension mismatch** — You changed the embedding model, but the collection was created with the old dimensions. Drop and recreate:
   ```bash
   # In Qdrant dashboard or API:
   DELETE /collections/the_one_docs

   # Then re-index:
   maintain { "action": "reindex", ... }
   ```

---

## Embedding Issues

### First search is slow (model download)

Expected behavior. On first use, the embedding model downloads from Hugging Face and caches locally.

| Tier | Model | Download size |
|------|-------|---------------|
| fast | all-MiniLM-L6-v2 | ~23MB |
| balanced | nomic-embed-text-v1 | ~75MB |
| quality (default) | BGE-large-en-v1.5 | ~130MB |
| multilingual | paraphrase-multilingual | ~220MB |

Downloads go to `.fastembed_cache/` (gitignored). After the first download, subsequent starts are instant.

---

### Model download fails

**Check network:**

```bash
curl -I https://huggingface.co
```

**Hugging Face blocked?** Set a mirror:

```bash
export HF_ENDPOINT=https://hf-mirror.com
```

**Disk space:** The quality tier requires ~130MB free in `.fastembed_cache/`.

```bash
df -h .fastembed_cache/
```

---

### Wrong dimensions after switching models

If you change `embedding_model` after indexing, the stored vectors have different dimensions than the new model produces. This causes search failures or silent empty results.

**Fix:** Drop the Qdrant collection and re-index.

```bash
# Via maintain tool:
maintain { "action": "reindex", "params": { "project_root": "...", "project_id": "..." } }
```

---

### "Unknown embedding model" warning

The model name you set doesn't match any entry in the model registry.

```bash
# List valid model names and aliases:
config { "action": "models.list" }
```

Use the exact `name` or `alias` from the registry. Tier aliases (`fast`, `balanced`, `quality`, `multilingual`) are always valid.

---

### Out of memory on embedding batch

The embedder processes documents in batches. Reduce the batch size:

```json
// ~/.the-one/config.json — under "limits":
{
  "limits": {
    "max_embedding_batch_size": 16
  }
}
```

Default is 64. Set lower (e.g., 8–16) if you have less than 4GB RAM or are running a large model tier.

---

## Search Issues

### No results returned

Two likely causes:

1. **`search_score_threshold` is too high** — lower it:

   ```json
   {
     "limits": {
       "search_score_threshold": 0.15
     }
   }
   ```

   Default is 0.3. Start at 0.15 for exploratory queries.

2. **Index is stale** — run a re-index:

   ```bash
   maintain { "action": "reindex", "params": { "project_root": "...", "project_id": "..." } }
   ```

---

### Results are irrelevant

Two improvements:

1. **Enable reranking** — a cross-encoder pass improves precision significantly:

   ```json
   { "reranker_enabled": true }
   ```

2. **Tune the threshold** — raising `search_score_threshold` filters out low-confidence results:

   ```json
   {
     "limits": {
       "search_score_threshold": 0.4
     }
   }
   ```

---

### Results are missing newer documents

The RAG index isn't updated automatically when you add files outside of `docs.save`. Run a refresh:

```bash
setup { "action": "refresh", "params": { "project_root": "...", "project_id": "..." } }
```

Or re-index the memory layer directly:

```bash
maintain { "action": "reindex", "params": { "project_root": "...", "project_id": "..." } }
```

---

### Keyword fallback used when Qdrant should work

The broker falls back to SQLite FTS5 when the Qdrant connection fails. Check:

1. Is the Qdrant URL set correctly?

   ```bash
   config { "action": "export", "params": { "project_root": "..." } }
   ```

2. Is Qdrant actually reachable?

   ```bash
   curl http://localhost:6333/healthz
   # Expected: {"title":"qdrant - vector search engine","version":"..."}
   ```

3. Enable debug logging to see the fallback reason:

   ```bash
   RUST_LOG=the_one_memory=debug ~/.the-one/bin/the-one-mcp serve
   ```

---

## Image Search Issues

### `image embeddings not enabled`

Image search is off by default. Enable it:

```json
// ~/.the-one/config.json
{
  "image_embedding_enabled": true
}
```

Or via env var:

```bash
export THE_ONE_IMAGE_EMBEDDING_ENABLED=true
```

---

### `image-embeddings feature not compiled`

You built the binary without the `image-embeddings` feature (lean build).

```bash
# Rebuild with full feature set:
bash scripts/build.sh build

# Or explicitly:
cargo build --release -p the-one-mcp --features image-embeddings
```

The pre-built release binaries always include `image-embeddings`. Only custom lean builds omit it.

---

### Image file too large

Default max is 10MB. Increase it:

```json
{
  "limits": {
    "max_image_size_bytes": 20971520
  }
}
```

Or via env var:

```bash
export THE_ONE_LIMIT_MAX_IMAGE_SIZE_BYTES=20971520
```

---

### OCR returns empty text

1. **Tesseract not installed** — OCR requires `tesseract-ocr`:

   ```bash
   # Ubuntu/Debian:
   sudo apt install tesseract-ocr

   # macOS:
   brew install tesseract

   # Verify:
   tesseract --version
   ```

2. **Wrong language pack** — the default OCR language is `eng`. For other languages:

   ```bash
   sudo apt install tesseract-ocr-fra   # French
   ```

   Then set in config:

   ```json
   { "image_ocr_language": "fra" }
   ```

3. **OCR not enabled** — check your config:

   ```json
   { "image_ocr_enabled": true }
   ```

---

### Thumbnail generation fails

- Check disk space in `.the-one/` — thumbnails are stored alongside indexed images
- Check `RUST_LOG=debug` output for `image` crate errors (corrupt files, unsupported variants)
- Disable thumbnails if they're not needed:

  ```json
  { "image_thumbnail_enabled": false }
  ```

---

### Image search returns no results

1. Check `image_search_score_threshold` (default: 0.25 — lower than text threshold):

   ```json
   {
     "limits": {
       "image_search_score_threshold": 0.1
     }
   }
   ```

2. Check that images were actually indexed. Rescan:

   ```bash
   maintain { "action": "images.rescan", "params": { "project_root": "...", "project_id": "..." } }
   ```

---

## Reranking Issues

### Reranker model download is slow

The default reranker (`jina-reranker-v2-base-multilingual`) is ~180MB. The alternative `bge-reranker-base` is smaller (~280MB uncompressed but faster to download in practice). Both cache in `.fastembed_cache/` after the first download.

If the download is stalled, check:

```bash
ls -lh .fastembed_cache/
```

If the file is partially downloaded, delete it and retry (the embedder will re-download).

---

### Reranking latency is too high

The cross-encoder reruns over a fetch pool of candidates. Reduce how many candidates are fetched before reranking. This is controlled by `top_k` in the `memory.search` call — smaller `top_k` means fewer candidates for the reranker.

Example: use `top_k: 5` instead of `top_k: 20` if latency matters more than exhaustiveness.

---

### Reranker init fails

Check that:

1. The `image-embeddings` or `reranker` feature is compiled in (it is by default in release builds)
2. RAM is sufficient — the reranker model requires ~500MB during inference
3. The model name is valid:

   ```bash
   config { "action": "models.list", "params": { "filter": "reranker" } }
   ```

---

## Tool Catalog Issues

### `tool.find` returns empty

The catalog hasn't been imported yet, or the database is stale.

```bash
maintain { "action": "tool.refresh", "params": { "project_root": "...", "project_id": "..." } }
```

This re-imports from `tools/catalog/*.json` into `~/.the-one/catalog.db`.

---

### Custom tool not showing up

Custom tools live in `~/.the-one/registry/`. Check both the shared and per-CLI files:

```bash
ls ~/.the-one/registry/
# custom.json           — shared across all CLIs
# custom-claude.json    — Claude Code only
# custom-gemini.json    — Gemini CLI only
# recommended.json      — recommended tools
```

Validate the JSON syntax:

```bash
cat ~/.the-one/registry/custom.json | python3 -m json.tool
```

The expected format is an array of tool objects with `id`, `name`, `description`, and `run_command` fields.

---

### Tool install fails

1. **Check the `install_command` in the tool definition** — it runs as a shell command in the user's environment.
2. **Check network access** — most install commands download packages.
3. **View install output:**

   ```bash
   RUST_LOG=the_one_core=debug ~/.the-one/bin/the-one-mcp serve
   ```

---

### Tool run denied

The `tool.run` tool requires approval. This is by design (policy gate).

- Set `"interactive": true` in the call to allow prompting
- Set `"approval_scope": "session"` to approve for the current session
- Set `"approval_scope": "forever"` to permanently approve this action key

---

## Config Issues

### Config change not taking effect

Config resolves through 5 layers in order (later layers win):

1. Defaults (hardcoded)
2. Global file (`~/.the-one/config.json`)
3. Project file (`<project>/.the-one/config.json`)
4. Environment variables (`THE_ONE_*`)
5. Runtime overrides (via `config:update` tool)

**Environment variables override config files.** If you set `THE_ONE_QDRANT_URL` as an env var, the config file value for `qdrant_url` is ignored.

Inspect the resolved config:

```bash
config { "action": "export", "params": { "project_root": "/your/project" } }
```

---

### Invalid JSON in config file

```bash
# Validate syntax:
cat ~/.the-one/config.json | python3 -m json.tool

# Or with jq:
jq . ~/.the-one/config.json
```

A syntax error causes the config file to be ignored entirely, and defaults are used.

---

### Limit clamping warnings in logs

The broker clamps limits to safe ranges at startup. If you see warnings like:

```
WARN: search_score_threshold clamped from 1.5 to 1.0
```

Your configured value is outside the allowed range. Check `the-one-core/src/limits.rs` for min/max bounds, or use `config:export` to see what values were actually applied.

---

## Performance Issues

### High memory usage

- **Embedding batch size** — reduce `max_embedding_batch_size` (default: 64) to lower peak RAM during indexing
- **Model tier** — the `quality` tier (default) uses ~500MB RAM during embedding; switch to `balanced` or `fast` for lighter workloads:

  ```json
  { "embedding_model": "balanced" }
  ```

- **Reranker** — the reranker adds ~500MB when active; disable if RAM is tight:

  ```json
  { "reranker_enabled": false }
  ```

---

### Slow `setup:refresh`

Large projects with many docs take longer to chunk and embed.

- The embedding step is CPU-bound and runs single-threaded for the ONNX model
- Qdrant upserts are batched — if Qdrant is remote, network latency adds up
- For first-time indexing on large projects, expect 1–5 minutes depending on doc count and model tier

To check progress: watch Qdrant collection size grow in the dashboard, or check logs with `RUST_LOG=the_one_memory=info`.

---

### Qdrant slow on first queries

Qdrant builds HNSW index structures as vectors are added. On collections under ~1000 vectors, performance is equivalent to brute-force. Above that, index building may cause initial latency.

The `search_score_threshold` also affects performance — a very low threshold causes more candidates to pass through to reranking, slowing the pipeline.

---

## File/Storage Issues

### Docs not persisting between sessions

Check write permissions on the managed docs directory:

```bash
ls -la ~/.the-one/docs/
```

The directory must be writable by the user running the MCP server. If it's owned by root (e.g., from a `sudo install`), fix it:

```bash
sudo chown -R "$USER" ~/.the-one/
```

---

### Trash fills up disk space

Deleted documents are soft-deleted (moved to trash) and not immediately removed.

```bash
# List trashed documents:
maintain { "action": "trash.list", "params": { "project_root": "...", "project_id": "..." } }

# Permanently delete all trashed documents:
maintain { "action": "trash.empty", "params": { "project_root": "...", "project_id": "..." } }
```

---

### `.the-one/state.db` is locked

SQLite lock errors happen when multiple processes try to write to the same database.

**Check for orphaned processes:**

```bash
lsof ~/.the-one/state.db
```

If you see a stale process holding the lock, kill it:

```bash
kill <pid>
```

If no process is listed but the lock persists (crash without cleanup), delete the WAL files:

```bash
rm ~/.the-one/state.db-wal ~/.the-one/state.db-shm
```

---

## Admin UI Issues

### `/swagger` returns 404

The Swagger UI is compiled in by default with the `embed-swagger` Cargo feature (enabled in full builds).

If you built from source with a lean profile:

```bash
bash scripts/build.sh build   # includes embed-swagger by default
```

---

### Port in use when starting admin UI

Change the bind address:

```bash
export THE_ONE_UI_BIND=127.0.0.1:3001
THE_ONE_PROJECT_ROOT="$(pwd)" THE_ONE_PROJECT_ID="myproject" \
  cargo run -p the-one-ui --bin embedded-ui
```

---

### Config not visible in admin UI

The admin UI reads from the `THE_ONE_PROJECT_ROOT` and `THE_ONE_PROJECT_ID` environment variables. Both must be set:

```bash
export THE_ONE_PROJECT_ROOT="/absolute/path/to/project"
export THE_ONE_PROJECT_ID="myproject"
~/.the-one/bin/the-one-ui
```

---

## Debugging Tips

### Enable verbose logging

```bash
# All debug output:
RUST_LOG=debug ~/.the-one/bin/the-one-mcp serve

# Specific crate only:
RUST_LOG=the_one_memory=debug,the_one_core=info ~/.the-one/bin/the-one-mcp serve
```

Or set `log_level` permanently in config:

```json
{ "log_level": "debug" }
```

---

### Inspect metrics

```bash
observe { "action": "metrics" }
```

Returns broker counters: requests served, errors, tool calls, search hits/misses, fallback counts.

---

### Inspect audit events

```bash
observe {
  "action": "events",
  "params": { "project_root": "...", "project_id": "...", "limit": 50 }
}
```

Shows the last N tool invocations with timestamps, inputs, and outcomes.

---

### Run isolated crate tests

```bash
# Core storage and config:
cargo test -p the-one-core

# Embedding and search:
cargo test -p the-one-memory

# MCP protocol and tools:
cargo test -p the-one-mcp

# Routing and classification:
cargo test -p the-one-router

# Run a single test by name:
cargo test -p the-one-core test_create_and_get
```

---

### Full CI validation

Before reporting a bug, confirm the build is clean:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Or use the release gate script:

```bash
bash scripts/release-gate.sh
```

---

## Hybrid Search Issues

### "Hybrid search requires Qdrant 1.7+"

Your Qdrant instance is too old to support sparse vectors. Options:
- Upgrade Qdrant to 1.7 or later
- Disable hybrid search: `"hybrid_search_enabled": false`

### Search results unchanged after enabling hybrid search

After enabling `hybrid_search_enabled`, you must recreate the Qdrant collection with sparse vector support. Run:

```
maintain (action: reindex)
```

Without reindex, the collection format is dense-only and sparse scoring is silently skipped.

### Sparse scores always near 0.0

This is correct behavior for queries that contain only high-frequency or stop words. SPLADE++ gives high weight to rare tokens. Queries like "what does this do?" have no distinctive tokens, so sparse weight is near zero — dense cosine similarity dominates, which is the right result.

### Config shows hybrid_search_enabled: false but I set true

Check the config layer resolution. A project config file can override the global config. Use `config (action: export)` from an AI session to see the fully resolved config with all layers merged.

---

## File Watcher Issues (v0.8.0)

### Watcher log lines not appearing

1. Verify `auto_index_enabled: true` in resolved config: `config (action: export)`
2. Set `log_level: "info"` — watcher logs at INFO level
3. Ensure the project was initialized: call `setup (action: project)` — the watcher is started per-project at init time

### "Failed to start watcher"

The most common cause is missing directories. Ensure `.the-one/docs/` and `.the-one/images/` exist. They are created on `setup (action: project)`.

On Linux, also check inotify watch limits:

```bash
cat /proc/sys/fs/inotify/max_user_watches
# If this is low (< 8192), raise it:
sudo sysctl fs.inotify.max_user_watches=524288
```

### Markdown file changed but index not updated

Ensure the project was initialized (`setup (action: project)` was called) — the watcher is started per-project at init time, and a fresh server session won't have an active watcher until init runs. Also verify `auto_index_enabled: true` is set. If after init changes still don't appear, run `maintain (action: reindex)` to force a full reindex.

### Image events fire but images are not re-indexed

This is expected in v0.8.0. Image event detection works; automatic image re-ingestion is planned for v0.8.1. Run `maintain (action: images.rescan)` manually to update image search.

---

## Code Chunker Issues (v0.8.0)

### Code file indexed but `language` / `symbol` fields are null in fetch results

The file extension is not in the supported set (`.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.go`). Other extensions fall back to blank-line paragraph splitting — the chunk will have `language: null` and `symbol: null`. Check the file extension and see the [Code Chunking Guide](code-chunking.md) for the full list.

### Symbol name search returns no results

The chunker extracted the function but the search didn't find it. Try a semantic query that describes the function's behavior rather than its name. For exact name matching, enable hybrid search (`hybrid_search_enabled: true`) — the sparse SPLADE++ signal gives high weight to rare tokens like function names.

### After running `setup:refresh`, code chunks still show old boundaries

The chunker runs on the content that was passed at ingest time. If you changed the file on disk but the managed doc wasn't updated via `docs.save`, the index still has the old version. Use `docs.save` to re-ingest the file, or run `maintain (action: reindex)` to force a full re-indexing pass.

### Rust brace-depth tracking produces oversized chunks for large `impl` blocks

Very large `impl` blocks (with many methods) may exceed `max_chunk_tokens` and be split on blank lines within the block. Each sub-chunk retains the same `symbol` and `signature` as the parent item. This is expected behavior — the sub-chunks are tagged consistently, so searches for the symbol still return all relevant sub-chunks.

### TypeScript template literals confuse the chunker

The TypeScript/JavaScript chunker handles backtick template literals by tracking nesting depth and ignoring `{` inside them. If you see unexpected chunk boundaries in a file with complex template literals, this is a known edge case with the regex-based approach. The planned tree-sitter upgrade (v0.9.0) will resolve this.

---

## Admin UI Image Gallery Issues

### `/images` page is blank

The gallery fetches from `/api/images`. If no images are indexed for the active project, the grid will be empty. Ingest images first via `memory.ingest_image` or `maintain (action: images.rescan)`.

### Thumbnail returns 404

Thumbnails are stored in `.the-one/thumbnails/` under the project root. If the file is missing (e.g., project moved or thumbnails deleted), the thumbnail route returns 404. Re-run `maintain (action: images.rescan)` to regenerate thumbnails.

### "Invalid thumbnail hash" error

The `/images/thumbnail/<hash>` route validates the hash against the pattern `^[a-zA-Z0-9-]+$`. If your image IDs contain characters outside alphanumerics and hyphens, they will be rejected. This is a security guard against path traversal — image IDs are generated internally and should always pass this check. If you see this error, it indicates data corruption; run `maintain (action: images.clear)` and re-ingest.

---

## Screenshot Image Search Issues

### "Provide either query or image_base64, not both" error

`memory.search_images` requires exactly one of `query` or `image_base64`. Remove whichever you don't want.

### "Provide either query or image_base64" when both are absent

Both fields are optional in JSON but exactly one must be present. Ensure the LLM is passing the correct field name (`image_base64`, not `image_base_64` or `imageBase64`).

### Image search via base64 returns wrong results

The base64 image is embedded using the same image model as the indexed images. If you indexed with `nomic-vision` but pass a base64 image from a very different domain (e.g., a photo vs. a diagram), the embedding space match may be weak. This is expected — image→image similarity works best within the same visual domain.
