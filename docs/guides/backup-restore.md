# Backup & Restore Guide

> v0.12.0 introduced `maintain: action: backup` and `maintain: action: restore`
> for moving project state between machines. This guide covers what's included,
> how to use it, and what you're responsible for backing up separately.
>
> **v0.16.0 multi-backend note:** `maintain: backup` captures SQLite state
> and local Qdrant collection data. It does NOT snapshot Postgres
> (pgvector or `PostgresStateStore`) or Redis. When running against those
> backends, use the native provider tooling (`pg_dump`, `redis-cli BGSAVE`,
> managed-provider PITR snapshots) on a schedule you control. See the
> standalone [pgvector-backend.md](pgvector-backend.md) and
> [postgres-state-backend.md](postgres-state-backend.md) for per-backend
> operational details.

## When to use this

- You're moving to a new laptop and want all your project notes, memory, and
  tool preferences to come along
- You want a before-reorganization snapshot you can roll back to
- You want to share a fully-populated project state with a collaborator
- You want an off-site copy of the knowledge you've accumulated over weeks
  or months of sessions

## What's included in a backup

A v0.12.0 backup is a **gzipped tar archive** (`.tar.gz`) containing:

- **Per-project state** — the entire `<project>/.the-one/` tree, including:
  - `manifests/` — project profile, fingerprint cache
  - `state.db` — SQLite database (approvals, audit events, managed_images)
  - `docs/` — your managed markdown files (the content the LLM has written
    and organized for you)
  - `images/` — indexed images + thumbnails (opt-in; default on)
  - `config.json` — project-level config overrides
- **Global state from `~/.the-one/`**:
  - `catalog.db` — the tool catalog SQLite with your enabled-tool preferences
  - `registry/` — per-CLI custom tool definitions

Every backup has a `backup-manifest.json` at the archive root with:

```json
{
  "version": "1",
  "the_one_mcp_version": "0.12.0",
  "created_at_epoch": 1712345678,
  "project_id": "my-project",
  "file_count": 142,
  "includes": ["docs", "images", "config", "catalog", "registry"],
  "excludes": [".fastembed_cache"]
}
```

## What's deliberately NOT included

These exclusions keep backups small and fast:

- **`.fastembed_cache/`** — embedding model weights (~30MB–2GB per model).
  Re-downloaded automatically on first use after restore. Excluding them
  takes a typical backup from "gigabytes" to "single-digit megabytes".
- **Local Qdrant wal / raft_state** — when the-one-mcp runs with a
  local-file Qdrant, the wal and raft state can grow large. Qdrant can
  rebuild these from the collection data, so the backup skips them.
- **Remote Qdrant server data** — if you use a remote Qdrant (or Qdrant
  Cloud), your vector data lives on that server, not in `.the-one/`.
  **Backing up your Qdrant server is YOUR responsibility** — see the
  [Qdrant docs](https://qdrant.tech/documentation/concepts/snapshots/) for
  snapshot workflows.
- **`.DS_Store`** noise files on macOS

---

## Creating a backup

### Via the `maintain` tool (recommended)

From any AI CLI session connected to the-one-mcp:

```
You: "Back up this project to ~/Desktop/project-backup.tar.gz"
```

The LLM will call:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "backup",
      "params": {
        "project_root": "/path/to/project",
        "project_id": "my-project",
        "output_path": "/Users/you/Desktop/project-backup.tar.gz",
        "include_images": true
      }
    }
  }
}
```

Response:

```json
{
  "output_path": "/Users/you/Desktop/project-backup.tar.gz",
  "size_bytes": 2345678,
  "file_count": 142,
  "manifest_version": "1"
}
```

### Backup parameters

| Param | Required | Default | Meaning |
|-------|----------|---------|---------|
| `project_root` | yes | — | Absolute path to the project directory |
| `project_id` | yes | — | Project identifier (same one you use elsewhere) |
| `output_path` | yes | — | Where to write the tarball. Parent directories are created if missing. |
| `include_images` | no | `true` | Include indexed images + thumbnails |
| `include_qdrant_local` | no | `false` | Include local Qdrant storage (usually large) |

### Performance notes

- Backup runs on the tokio blocking pool so it won't stall the broker's
  async runtime
- Compression is gzip default level (6). A ~50MB raw tree typically
  compresses to ~10MB
- A typical backup finishes in a few seconds; large image collections may
  push it to a minute

---

## Restoring from a backup

Transfer the tarball to the new machine (scp, cloud drive, whatever), then
on the destination:

```
You: "Restore the backup at ~/Downloads/project-backup.tar.gz into /path/to/new-location"
```

Which becomes:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "maintain",
    "arguments": {
      "action": "restore",
      "params": {
        "backup_path": "/Users/you/Downloads/project-backup.tar.gz",
        "target_project_root": "/path/to/new-location",
        "target_project_id": "my-project",
        "overwrite_existing": false
      }
    }
  }
}
```

Response:

```json
{
  "restored_files": 142,
  "warnings": []
}
```

### Restore parameters

| Param | Required | Default | Meaning |
|-------|----------|---------|---------|
| `backup_path` | yes | — | Absolute path to the `.tar.gz` file |
| `target_project_root` | yes | — | Where the restored project should live |
| `target_project_id` | yes | — | Project ID for the restored project (can differ from backup's) |
| `overwrite_existing` | no | `false` | If false and the target already has a `.the-one/` directory, restore fails rather than clobbering |

### Safety properties

v0.12.0's restore enforces several safety properties:

1. **Manifest version check** — restore aborts if `backup-manifest.json`
   declares a `version` the current binary doesn't understand
2. **Unsafe path rejection** — any archive entry containing `..`, an
   absolute path, or non-Normal components is rejected before extraction
3. **Refuses to overwrite by default** — if the target already has a
   `.the-one/` directory, restore returns an error unless
   `overwrite_existing: true`. This protects against accidentally clobbering
   a populated project
4. **Unknown top-level entries** in the archive become warnings, not
   errors — this gives forward-compatibility room for future backup formats

Warnings are surfaced in `RestoreResponse.warnings` so you can see what
the restore did or didn't do.

### What happens after restore

1. The `<target_project_root>/.the-one/` directory is populated
2. The global `~/.the-one/catalog.db` and `~/.the-one/registry/` are
   populated (or merged if they already exist)
3. **Embedding models re-download on first use** — the `.fastembed_cache/`
   was excluded from the backup, so the first `memory.search` after
   restore triggers a model download (~30MB–2GB depending on tier)
4. **Qdrant collections must be reachable** — if your backup uses a
   remote Qdrant, make sure the new machine can reach the server. If
   Qdrant is unreachable, searches return an error and you'll need to
   run `maintain: action: reindex` once the server is back

---

## Moving to a new machine: recommended flow

```bash
# --- On the old machine ---

# 1. Open an AI CLI in your project
$ cd ~/projects/my-project
$ claude

# 2. Ask for a backup
You: "Back up this project to ~/Desktop/my-project.tar.gz"

# 3. Verify the tarball was created
$ ls -lh ~/Desktop/my-project.tar.gz
-rw-r--r--  1 you  staff   12M Apr  5 14:23 my-project.tar.gz


# --- Transfer to the new machine ---

$ scp ~/Desktop/my-project.tar.gz you@newmac:~/Downloads/


# --- On the new machine ---

# 1. Install the-one-mcp
$ curl -fsSL https://raw.githubusercontent.com/michelabboud/the-one-mcp/main/scripts/install.sh | bash

# 2. Clone your code repo
$ git clone git@github.com:you/my-project.git ~/projects/my-project
$ cd ~/projects/my-project

# 3. Ask Claude (or any other MCP CLI) to restore
$ claude
You: "Restore ~/Downloads/my-project.tar.gz into the current directory, project_id my-project"

# 4. Verify — your memory, docs, and tool preferences are back
You: "List all my indexed docs"
Claude: [calls docs.list, shows everything from the old machine]
```

---

## Troubleshooting

### `project state directory does not exist`

Backup was pointed at a project that hasn't been initialized. Run
`setup (action: init)` first to create `.the-one/`.

### `target already has .the-one/ state; pass overwrite_existing=true to replace`

Restore refused because the target directory already has a populated
project state. Either:

- Move the existing `.the-one/` aside: `mv .the-one .the-one.backup` and retry
- Pass `overwrite_existing: true` if you're sure you want to clobber

### `backup manifest version X is not supported by this binary`

You're trying to restore a newer backup format on an older binary. Upgrade
the-one-mcp to the version that created the backup (or newer).

### `unsafe path in backup archive`

The tarball has entries with `..` or absolute paths. This should never
happen for backups produced by the-one-mcp itself. If it does, someone
produced a malicious or corrupted archive — don't restore it.

### First search after restore is very slow

Expected: the embedding model is downloading for the first time after
restore. A ~130MB download is typical for the `quality` tier. Cached for
all subsequent queries.

### `maintain: reindex` needed after restore

If the project was using a local Qdrant fallback and you excluded
`include_qdrant_local`, the Qdrant collection on the new machine is
empty. Run `maintain (action: reindex)` to rebuild the vector index from
the restored `docs/` directory.

---

## Caveats and limitations

- **The catalog is shared across projects** — restoring a project merges
  (or overwrites, if using `overwrite_existing`) the global
  `~/.the-one/catalog.db`. If you have multiple projects on the new
  machine, consider restoring the catalog separately or using per-project
  enabled-tool profiles
- **No incremental backups yet** — every backup is a full snapshot. Future
  versions may add rsync-style or timestamp-based incremental support
- **No encryption at rest** — the tarball is plain gzip. If the contents
  are sensitive, handle the file accordingly (age, gpg, encrypted cloud
  storage, etc.)
- **Backups are not automatically scheduled** — nothing runs them on a
  timer. Ask your AI to back up regularly, or run `maintain: backup`
  from a cron job

---

## See also

- [API Reference](api-reference.md) — full schema for the `maintain` tool
- [Troubleshooting](troubleshooting.md) — general troubleshooting
- [Architecture](architecture.md) — where project state lives on disk
