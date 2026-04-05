//! Graph RAG entity extraction pipeline (v0.13.0).
//!
//! Takes indexed chunks and runs them through an OpenAI-compatible chat
//! completions endpoint with the `build_extraction_prompt` from `graph.rs`,
//! then merges results into the project's knowledge graph file.
//!
//! # Configuration
//!
//! For v0.13.0, extraction is configured purely through environment variables
//! to avoid touching 4 separate config structs. Proper config fields will land
//! in v0.13.1 alongside the config page model selector dropdown.
//!
//! | Env var | Required | Default | Meaning |
//! |---------|----------|---------|---------|
//! | `THE_ONE_GRAPH_ENABLED` | no | `false` | Toggle extraction. Must be `true` for anything to happen |
//! | `THE_ONE_GRAPH_BASE_URL` | yes* | — | OpenAI-compatible base URL (`http://localhost:11434/v1` for Ollama) |
//! | `THE_ONE_GRAPH_MODEL` | yes* | `llama3.2` | Chat model name |
//! | `THE_ONE_GRAPH_API_KEY` | no | `""` | Auth header. Ollama/local usually doesn't need one |
//! | `THE_ONE_GRAPH_ENTITY_TYPES` | no | `person,organization,location,technology,concept,event` | Comma-separated types |
//! | `THE_ONE_GRAPH_MAX_CHUNKS` | no | `50` | Safety cap on chunks per extract call |
//!
//! `*` = required only when `THE_ONE_GRAPH_ENABLED=true`.

use crate::chunker::ChunkMeta;
use crate::graph::{build_extraction_prompt, parse_extraction_response, KnowledgeGraph};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of a graph extraction run, surfaced back to the caller so the UI
/// can show "X entities and Y relations extracted from Z chunks".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphExtractResult {
    pub chunks_processed: usize,
    pub chunks_skipped: usize,
    pub entities_added: usize,
    pub relations_added: usize,
    pub total_entities: usize,
    pub total_relations: usize,
    pub errors: Vec<String>,
    pub disabled_reason: Option<String>,
}

/// Check whether graph extraction is enabled via environment variables.
pub fn is_graph_enabled() -> bool {
    std::env::var("THE_ONE_GRAPH_ENABLED")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Run extraction over a set of chunks and merge results into the project's
/// knowledge graph file. Returns a summary of what was added.
///
/// If `THE_ONE_GRAPH_ENABLED` is false, returns an empty result with a
/// `disabled_reason` explaining why — the caller can surface this to the UI
/// without erroring.
pub async fn extract_and_persist(
    project_root: &Path,
    chunks: &[ChunkMeta],
) -> Result<GraphExtractResult, String> {
    let mut result = GraphExtractResult::default();

    if !is_graph_enabled() {
        result.disabled_reason = Some(
            "THE_ONE_GRAPH_ENABLED is not set to true. See docs/guides/graph-rag.md to enable."
                .to_string(),
        );
        return Ok(result);
    }

    let base_url = std::env::var("THE_ONE_GRAPH_BASE_URL").map_err(|_| {
        "THE_ONE_GRAPH_BASE_URL must be set when THE_ONE_GRAPH_ENABLED=true (e.g. http://localhost:11434/v1 for Ollama)".to_string()
    })?;
    let model = std::env::var("THE_ONE_GRAPH_MODEL").unwrap_or_else(|_| "llama3.2".to_string());
    let api_key = std::env::var("THE_ONE_GRAPH_API_KEY").unwrap_or_default();
    let entity_types_str = std::env::var("THE_ONE_GRAPH_ENTITY_TYPES")
        .unwrap_or_else(|_| "person,organization,location,technology,concept,event".to_string());
    let entity_types: Vec<&str> = entity_types_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let max_chunks: usize = std::env::var("THE_ONE_GRAPH_MAX_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    // Load existing graph (if any) so we accumulate rather than replace
    let graph_path = project_root.join(".the-one").join("knowledge_graph.json");
    let mut graph = KnowledgeGraph::load_from_file(&graph_path).unwrap_or_default();
    let start_entities = graph.entity_count();
    let start_relations = graph.relation_count();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let to_process = chunks.iter().take(max_chunks);
    for chunk in to_process {
        let prompt = build_extraction_prompt(&chunk.content, &entity_types);
        match extract_one(&client, &base_url, &model, &api_key, &prompt).await {
            Ok(text) => match parse_extraction_response(&text, &chunk.id) {
                Ok(extraction) => {
                    graph.merge_extraction(&extraction);
                    result.chunks_processed += 1;
                }
                Err(e) => {
                    result.errors.push(format!("parse {}: {e}", chunk.id));
                    result.chunks_skipped += 1;
                }
            },
            Err(e) => {
                result.errors.push(format!("http {}: {e}", chunk.id));
                result.chunks_skipped += 1;
            }
        }
    }

    // Persist back to disk (best-effort — extraction succeeded even if save fails)
    if let Some(parent) = graph_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = graph.save_to_file(&graph_path) {
        result
            .errors
            .push(format!("save knowledge_graph.json: {e}"));
    }

    result.entities_added = graph.entity_count().saturating_sub(start_entities);
    result.relations_added = graph.relation_count().saturating_sub(start_relations);
    result.total_entities = graph.entity_count();
    result.total_relations = graph.relation_count();

    Ok(result)
}

/// Single chat completions call. Uses the OpenAI Chat Completions schema,
/// which Ollama, LM Studio, LiteLLM, LocalAI, and vLLM all speak.
async fn extract_one(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "You extract entities and relationships from text. Respond ONLY with valid JSON in the format requested. No commentary."},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.0,
        "stream": false,
    });

    let mut req = client.post(&url).json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, text));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {e}"))?;
    let content = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| "response missing choices[0].message.content".to_string())?;
    Ok(content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extract_disabled_by_default() {
        // Ensure THE_ONE_GRAPH_ENABLED is not set
        std::env::remove_var("THE_ONE_GRAPH_ENABLED");
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = extract_and_persist(tmp.path(), &[]).await.expect("ok");
        assert!(result.disabled_reason.is_some());
        assert_eq!(result.chunks_processed, 0);
    }

    #[tokio::test]
    async fn test_extract_enabled_without_base_url_errors() {
        std::env::set_var("THE_ONE_GRAPH_ENABLED", "true");
        std::env::remove_var("THE_ONE_GRAPH_BASE_URL");
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = extract_and_persist(tmp.path(), &[]).await;
        assert!(result.is_err());
        std::env::remove_var("THE_ONE_GRAPH_ENABLED");
    }
}
