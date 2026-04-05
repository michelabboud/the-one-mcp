# Observability Guide

> v0.12.0 extended the broker's metrics surface with 8 new counters and
> per-operation latency tracking. This guide covers what's measured, how
> to access it, and how to use the data for debugging.

## The `observe` tool

the-one-mcp exposes a multiplexed admin tool called `observe` with two
actions:

| Action | Purpose |
|--------|---------|
| `metrics` | Dump the current in-memory counter snapshot |
| `events` | Read the SQLite audit event log (last N events) |

Call it via any AI CLI or direct JSON-RPC:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "observe",
    "arguments": {
      "action": "metrics"
    }
  }
}
```

---

## v0.12.0 metrics snapshot

The response structure (`MetricsSnapshotResponse`) contains all these fields:

### Core counters (since v0.5.0)

| Field | Meaning |
|-------|---------|
| `project_init_calls` | How many times `setup (action: init)` ran |
| `project_refresh_calls` | How many times `setup (action: refresh)` ran |
| `memory_search_calls` | Total `memory.search` invocations |
| `tool_run_calls` | Total `tool.run` invocations |
| `router_fallback_calls` | How often the nano router fell back to rules-only |
| `router_decision_latency_ms_total` | Running sum of router decision latencies |
| `router_provider_error_calls` | How many times a nano provider errored |

### v0.12.0 additions

| Field | Meaning |
|-------|---------|
| `memory_search_latency_ms_total` | Running sum of memory.search wall-clock latencies (milliseconds) |
| `memory_search_latency_avg_ms` | Derived: `memory_search_latency_ms_total / memory_search_calls` (0 if no calls yet) |
| `image_search_calls` | Total `memory.search_images` invocations |
| `image_ingest_calls` | Total `memory.ingest_image` invocations |
| `resources_list_calls` | Total MCP `resources/list` invocations |
| `resources_read_calls` | Total MCP `resources/read` invocations |
| `watcher_events_processed` | Markdown files successfully re-ingested by the watcher |
| `watcher_events_failed` | Markdown files the watcher tried to re-ingest but errored on |
| `qdrant_errors` | Total Qdrant HTTP / gRPC errors surfaced to the broker |

All counters are `u64` atomics incremented lock-free. The JSON response
is generated on-demand when you call `observe: metrics` — there's no
background aggregation thread to worry about.

---

## Using metrics for debugging

### "Why is search slow?"

Look at `memory_search_latency_avg_ms`:

- **Under 50ms** — the embedding model and Qdrant are both healthy
- **50–200ms** — likely the expected p50 for a hybrid or rerank-enabled
  search. If this is new, check `hybrid_search_enabled` or
  `rerank_enabled` in config
- **200–1000ms** — Qdrant is probably network-attached (remote server). Or
  the reranker is running on every result. Or both
- **Over 1s** — the embedding model is likely re-loading on every call. Or
  Qdrant is unreachable and searches are hitting the keyword fallback
  with a huge corpus. Check `qdrant_errors` at the same time

Average latency hides p95/p99 — a handful of very slow queries can
dominate the average. Future versions will add percentile reservoirs;
for v0.12.0 you have the running sum and total count.

### "Is the watcher actually working?"

Compare `watcher_events_processed` against `watcher_events_failed`:

- Both **zero** — either you haven't enabled `auto_index_enabled: true`
  in config, or no files have changed since the server started, or the
  watcher failed to start (check logs for a `[WARN] failed to start
  file watcher`)
- **Processed > 0, failed = 0** — watcher healthy
- **Both growing, failed non-zero** — some files are failing to index.
  Check the server logs (WARN level) for the per-file error messages.
  Common causes: malformed markdown frontmatter, file path with non-UTF-8
  bytes, a file changing mid-read

### "Is Qdrant healthy?"

`qdrant_errors` is the headline metric. It increments on every HTTP/gRPC
error the broker sees from Qdrant. Zero is the happy path.

If it's growing, also look for:

- Log lines containing `qdrant`
- Whether `memory_search_calls` is growing at the same rate (Qdrant down
  means every search errors)

### "Are MCP resources being used?"

`resources_list_calls` and `resources_read_calls` tell you whether any
connected MCP client is actually exercising the v0.10.0 resources API.
Stale at zero means your clients only use the tools path.

### "Which features are dead code?"

Run the broker for a day or two against your normal workflow, then look
at the snapshot. Counters that stay at 0 are good candidates for:

- Features you could disable to reduce binary size
- Features that need better discoverability in their docs
- Features that are genuinely not useful and should be cut

---

## Shell helper for reading metrics

You can pipe the broker directly over stdio without going through an AI
CLI:

```bash
# One-shot metrics dump via stdio transport
the-one-mcp serve <<'EOF' 2>/dev/null | tail -1 | jq '.result.content[0].text | fromjson'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"observe","arguments":{"action":"metrics"}}}
EOF
```

Save that snippet as `scripts/print-metrics.sh` in your project for quick
health checks.

---

## Audit events (`observe: events`)

Complementary to counters, the audit event log records every high-risk
tool invocation and policy decision in the project's `state.db`:

```json
{
  "name": "observe",
  "arguments": {
    "action": "events",
    "project_root": "/abs/path",
    "project_id": "my-project",
    "limit": 20
  }
}
```

Returns the last 20 events as an array of:

```json
{
  "id": 42,
  "project_id": "my-project",
  "event_type": "tool.run",
  "payload_json": "{\"family\":\"cargo-audit\",\"action\":\"run\",\"risk\":\"medium\"}",
  "created_at_epoch_ms": 1712345678123
}
```

Event types include:
- `project.init`, `project.refresh`
- `tool.enable`, `tool.disable`, `tool.run`
- `docs.save`, `docs.delete`, `docs.move`
- `approval.granted`, `approval.denied`
- Provider health events (circuit-breaker trip, recovery)

Events are append-only and survive server restarts. Counters reset on
every restart.

---

## Metrics vs audit events — which to use

| Question | Use |
|----------|-----|
| "How many times did X happen since the server started?" | Counters (`observe: metrics`) |
| "What happened at 14:23 last Tuesday?" | Audit events (`observe: events`) |
| "Is the watcher healthy right now?" | Counters |
| "Why did that tool run get denied?" | Audit events |
| "What's my slowest feature?" | Counters (latency totals) |
| "Who approved that risky operation?" | Audit events |

---

## Prometheus export (not in v0.12.0)

The roadmap mentions a `GET /metrics` endpoint on the embedded admin UI
that would serve the counters in Prometheus format for Grafana scraping.
That's a stretch goal and did not land in v0.12.0. The existing
`observe: metrics` JSON response is the authoritative interface until
that lands.

If you want Grafana dashboards today, you can write a small bridge
script:

```bash
# Every 60s, fetch metrics and write to node_exporter textfile collector
while true; do
  the-one-mcp serve < metrics-probe.jsonl \
    | jq -r '.result.content[0].text | fromjson | to_entries[]
             | "the_one_mcp_\(.key) \(.value)"' \
    > /var/lib/node_exporter/the-one-mcp.prom
  sleep 60
done
```

---

## See also

- [Troubleshooting](troubleshooting.md) — symptom-based debugging flows
- [Auto-Indexing Guide](auto-indexing.md) — what the watcher does
- [API Reference](api-reference.md) — full schema for the `observe` tool
