# Conversation Memory Benchmark

Use this checklist when you want to reproduce a conversation-memory benchmark run without inventing claims or mixing datasets.

## Scope

This benchmark document is for reproducibility only. Record what you ran, with which dataset, config, and commands. Do not treat it as a source of headline numbers by itself.

## Reproducibility checklist

- Record the git commit SHA for this repository.
- Record whether the binary was built with local embeddings, image embeddings, and redis-related features.
- Record the exact transcript dataset paths used for `memory.ingest_conversation`.
- Record the transcript format for each dataset (`openai_messages`, `claude_transcript`, or `generic_jsonl`).
- Record any palace metadata used during ingest (`wing`, `hall`, `room`).
- Record the resolved `.the-one/config.json` values that affect retrieval:
  `embedding_provider`, `embedding_model`, `vector_backend`, `redis_url`,
  `redis_index_name`, `hybrid_search_enabled`, `reranker_enabled`,
  `limits.search_score_threshold`.
- Record whether Redis persistence was enabled with both RDB and AOF, or
  whether Qdrant was the active backend.
- Record the exact `memory.search` queries and any `wing` / `hall` / `room`
  filters used.
- Record the exact `memory.wake_up` arguments, including `wing` and `max_items`.
- Record whether the run was against a fresh index or an already-populated store.

## Suggested command flow

Ingest the transcript set you plan to evaluate, then run the same search and
wake-up calls you want to compare across configurations.

Example ingest:

```json
{
  "name": "memory.ingest_conversation",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "path": "exports/auth-review.json",
    "format": "openai_messages",
    "wing": "ops",
    "hall": "incidents",
    "room": "auth"
  }
}
```

Example filtered search:

```json
{
  "name": "memory.search",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "query": "refresh token staging incident",
    "top_k": 5,
    "wing": "ops",
    "room": "auth"
  }
}
```

Example wake-up run:

```json
{
  "name": "memory.wake_up",
  "arguments": {
    "project_root": "/home/user/myproject",
    "project_id": "myproject",
    "wing": "ops",
    "max_items": 4
  }
}
```

## Reporting template

Copy this block into your benchmark notes:

```text
commit:
build_features:
dataset_paths:
formats:
palace_metadata:
config_snapshot:
backend:
fresh_or_warm_index:
search_queries:
wake_up_args:
observations:
```
