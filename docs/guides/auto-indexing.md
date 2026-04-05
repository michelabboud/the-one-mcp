# Auto-Indexing Guide

> v0.7.0 — background file watcher for `.the-one/docs/` and `.the-one/images/`.

## What the File Watcher Does

When `auto_index_enabled` is true, the-one-mcp starts a background tokio task at server startup that watches two directories under your project's `.the-one/` folder:

- `.the-one/docs/` — Markdown files (`*.md`)
- `.the-one/images/` — Image files (`*.png`, `*.jpg`, `*.jpeg`, `*.webp`)

Whenever a file is created, modified, or deleted in these directories, the watcher detects the event and logs it. A configurable debounce timer (default 2000ms) prevents rapid bursts of events from triggering redundant processing — editor save operations often generate multiple filesystem events in quick succession, and debouncing waits until the activity settles.

### Current Scope (v0.7.0)

In v0.7.0, the watcher **detects and logs** file change events. It does not yet automatically re-ingest changed files into Qdrant. Full auto-reingestion is planned for v0.7.1.

**What this means in practice:** you can see in server logs when files change, but to update the search index you still need to run `maintain (action: reindex)` manually or from an AI session.

This staged approach lets you verify the watcher is running and receiving events correctly before the more complex auto-ingest behavior is added.

---

## Enabling the Watcher

The watcher is **opt-in**. Add to your config:

```json
{
  "auto_index_enabled": true
}
```

Or via environment variable:

```bash
export THE_ONE_AUTO_INDEX_ENABLED=true
```

The server must be restarted for this setting to take effect. It reads config at startup and spawns (or skips) the watcher task during broker initialization.

---

## Configuration Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_index_enabled` | bool | `false` | Start the background file watcher when the server initializes. |
| `auto_index_debounce_ms` | integer | `2000` | Milliseconds to wait after the last file event before processing the batch. Prevents triggering on intermediate editor saves. |

Both fields live at the top level of your config file.

### Choosing a Debounce Value

The debounce timer controls how quickly the watcher responds to bursts of filesystem activity.

| Value | Behavior |
|-------|----------|
| `500` | Reacts quickly; may fire multiple times during a slow save |
| `2000` (default) | Waits 2 seconds after the last event — suitable for most editors |
| `5000` | More conservative; useful on network filesystems with delayed metadata |

If you use an editor that writes multiple temp files before the final save (common with vim, emacs, and some JetBrains IDEs), the default 2000ms is appropriate. For editors that write atomically (VS Code, Zed), 500ms is fine.

---

## Example Config

### Minimal (default debounce)

```json
{
  "auto_index_enabled": true
}
```

### Custom debounce

```json
{
  "auto_index_enabled": true,
  "auto_index_debounce_ms": 500
}
```

### With other features

```json
{
  "auto_index_enabled": true,
  "auto_index_debounce_ms": 2000,
  "hybrid_search_enabled": true,
  "image_embedding_enabled": true
}
```

---

## What Is Watched

The watcher monitors exactly two paths per project:

| Path | File types | Triggered by |
|------|-----------|--------------|
| `<project_root>/.the-one/docs/` | `*.md` | `docs.save`, manual file edits |
| `<project_root>/.the-one/images/` | `*.png`, `*.jpg`, `*.jpeg`, `*.webp` | `memory.ingest_image`, manual file copies |

Subdirectories are **not** recursively watched in v0.7.0. Only the top-level files in each directory are monitored.

Files created outside these directories — including your project source files — are not watched. The watcher does not monitor `src/`, your workspace root, or any other directory. Its scope is intentionally narrow.

---

## How to Verify It Is Running

When the watcher starts successfully you will see a log line at `INFO` level:

```
[INFO the_one_mcp::watcher] Auto-index watcher started for project <project_id>
  docs: /path/to/project/.the-one/docs
  images: /path/to/project/.the-one/images
```

When a file event fires after the debounce settles:

```
[INFO the_one_mcp::watcher] File changed: /path/to/project/.the-one/docs/architecture.md (event: Modify)
```

To see these logs, set `log_level` to `"info"` or `"debug"` in your config:

```json
{ "log_level": "info" }
```

Logs go to stderr. If you are running via stdio transport (the default for Claude Code / Gemini), stderr output does not interfere with the JSON-RPC stream on stdout.

---

## CPU and Battery Considerations

The file watcher uses `notify` (inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows) — the OS kernel's own filesystem event mechanism. It does **not** poll. When no files are changing, the watcher task sleeps and uses zero CPU.

The debounce timer uses a single tokio interval and does not busywait.

In practice, the watcher has negligible CPU and battery impact during normal development. The only compute cost is the event processing, which in v0.7.0 is a log write — extremely cheap.

---

## Current Limitations

- **No auto-reingestion yet** — events are logged but not ingested into Qdrant automatically. This is planned for v0.7.1.
- **No recursive directory watching** — only the top-level docs/ and images/ directories are watched. Subdirectory support is planned.
- **No watch on project source files** — if you want to index your `src/` or other directories, you still need to use `docs.save` or `maintain reindex` manually.
- **One watcher per active project** — the watcher is started when a project is initialized via `setup (action: init)`. Projects that have not been initialized in the current server session do not have an active watcher.

---

## Workflow Until v0.7.1

Until auto-reingestion lands, the recommended workflow is:

1. Enable `auto_index_enabled: true` to get event logging (useful for debugging and confirming file writes).
2. After editing or adding docs, manually trigger reindex in your AI session:

   ```
   maintain (action: reindex)
   ```

3. In v0.7.1, step 2 will happen automatically after the debounce fires.

---

## Troubleshooting

### Watcher logs not appearing

1. Check that `auto_index_enabled` is `true` in the resolved config: use `config (action: export)` from an AI session.
2. Check `log_level` — the watcher logs at `INFO`. If `log_level` is `"warn"` or `"error"`, watcher messages are suppressed.
3. Verify the project was initialized (`setup (action: init)` was called). The watcher is started per-project at init time.

### "Failed to start watcher" in logs

The most common cause is a missing directory. Ensure `.the-one/docs/` and `.the-one/images/` exist under your project root. They are created automatically on `setup (action: init)`.

On Linux, check that inotify limits are not exhausted:

```bash
# Check current limit
cat /proc/sys/fs/inotify/max_user_watches

# Increase if needed (temporary)
sudo sysctl fs.inotify.max_user_watches=524288

# Permanent (add to /etc/sysctl.conf or /etc/sysctl.d/)
echo "fs.inotify.max_user_watches=524288" | sudo tee /etc/sysctl.d/99-inotify.conf
```

### Events fire but nothing is indexed

This is expected in v0.7.0. Event detection works; automatic re-ingestion is not implemented yet. Run `maintain (action: reindex)` manually to update the search index.

### High debounce latency during saves

If your editor triggers many events and the debounce keeps resetting, increase `auto_index_debounce_ms`:

```json
{ "auto_index_debounce_ms": 5000 }
```

### Watcher stops after some time

If the watcher background task panics or is dropped, the server continues running normally — the watcher is non-critical. Check server logs for a `[WARN]` or `[ERROR]` message from `the_one_mcp::watcher`. File a bug if you see unexpected panics.

---

## Dependencies Added in v0.7.0

- `notify 6.1` — cross-platform filesystem event API
- `notify-debouncer-mini 0.4` — lightweight debounce wrapper over notify
