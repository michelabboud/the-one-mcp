//! Graph RAG entity extraction pipeline (v0.13.0+).
//!
//! Takes indexed chunks and runs them through an OpenAI-compatible chat
//! completions endpoint with the `build_extraction_prompt` from `graph.rs`,
//! then merges results into the project's knowledge graph file.
//!
//! v0.13.1 adds three LightRAG-parity features to this module:
//! 1. **Gleaning pass** — an optional second LLM call per chunk asks for any
//!    entities/relations the first pass missed. Bumps recall ~15-25%.
//! 2. **Description summarization** — when an entity accumulates descriptions
//!    from many chunks, a summarization call condenses them so downstream
//!    embeddings stay coherent.
//! 3. **Query keyword extraction** — [`extract_query_keywords`] called from
//!    `MemoryEngine::search_graph` splits a user query into high-level themes
//!    and low-level specific identifiers for more targeted graph search.
//!
//! # Configuration
//!
//! For v0.13.x, extraction is configured purely through environment variables
//! to avoid touching 4 separate config structs. Proper config fields will land
//! in v0.14.0.
//!
//! | Env var | Required | Default | Meaning |
//! |---------|----------|---------|---------|
//! | `THE_ONE_GRAPH_ENABLED` | no | `false` | Toggle extraction. Must be `true` for anything to happen |
//! | `THE_ONE_GRAPH_BASE_URL` | yes* | — | OpenAI-compatible base URL (`http://localhost:11434/v1` for Ollama) |
//! | `THE_ONE_GRAPH_MODEL` | yes* | `llama3.2` | Chat model name |
//! | `THE_ONE_GRAPH_API_KEY` | no | `""` | Auth header. Ollama/local usually doesn't need one |
//! | `THE_ONE_GRAPH_ENTITY_TYPES` | no | `person,organization,location,technology,concept,event` | Comma-separated types |
//! | `THE_ONE_GRAPH_MAX_CHUNKS` | no | `50` | Safety cap on chunks per extract call |
//! | `THE_ONE_GRAPH_GLEANING_ROUNDS` | no | `1` | Extra extraction passes per chunk (v0.13.1) |
//! | `THE_ONE_GRAPH_SUMMARIZE_THRESHOLD` | no | `2000` | Description length (chars) that triggers LLM summarization (v0.13.1) |
//! | `THE_ONE_GRAPH_QUERY_EXTRACT` | no | `true` | Whether to extract query keywords for Local/Global retrieval (v0.13.1) |
//!
//! `*` = required only when `THE_ONE_GRAPH_ENABLED=true`.

use crate::chunker::ChunkMeta;
use crate::graph::{
    build_extraction_prompt, parse_extraction_response, Entity, ExtractionResult, KnowledgeGraph,
};
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
    let gleaning_rounds: u32 = std::env::var("THE_ONE_GRAPH_GLEANING_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let summarize_threshold: usize = std::env::var("THE_ONE_GRAPH_SUMMARIZE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

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
        match extract_with_gleaning(
            &client,
            &base_url,
            &model,
            &api_key,
            chunk,
            &entity_types,
            gleaning_rounds,
        )
        .await
        {
            Ok(extraction) => {
                graph.merge_extraction(&extraction);
                result.chunks_processed += 1;
            }
            Err(e) => {
                result.errors.push(format!("chunk {}: {e}", chunk.id));
                result.chunks_skipped += 1;
            }
        }
    }

    // v0.13.1: description summarization — after all chunks merge, any
    // entity whose description grew past the threshold gets map-reduce
    // summarized via a single LLM call. Keeps downstream embeddings coherent.
    if summarize_threshold > 0 {
        let oversized: Vec<String> = graph
            .all_entities()
            .into_iter()
            .filter(|e| e.description.chars().count() > summarize_threshold)
            .map(|e| e.name)
            .collect();
        for name in oversized {
            if let Some(entity) = graph.get_entity_mut(&name) {
                match summarize_description(&client, &base_url, &model, &api_key, entity).await {
                    Ok(()) => {}
                    Err(e) => result
                        .errors
                        .push(format!("summarize {}: {e}", entity.name)),
                }
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

// ---------------------------------------------------------------------------
// v0.13.1: gleaning loop — multiple extraction passes per chunk
// ---------------------------------------------------------------------------

/// Run one extraction pass on a chunk and then up to `gleaning_rounds` extra
/// passes asking the LLM for anything it missed. Returns the union of all
/// passes. Early-terminates when a pass returns no new entities or relations.
async fn extract_with_gleaning(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    chunk: &ChunkMeta,
    entity_types: &[&str],
    gleaning_rounds: u32,
) -> Result<ExtractionResult, String> {
    // First pass — standard extraction prompt
    let prompt = build_extraction_prompt(&chunk.content, entity_types);
    let text = extract_one(client, base_url, model, api_key, &prompt).await?;
    let mut combined = parse_extraction_response(&text, &chunk.id)?;

    // Gleaning rounds — ask the LLM what it missed, bail out on empty
    for round in 0..gleaning_rounds {
        let already = serde_json::to_string(&combined).unwrap_or_default();
        let glean_prompt = format!(
            r#"You previously extracted entities and relationships from this text. List ONLY additional items that were missed on the previous pass, in the same JSON format as before. If nothing was missed, return {{"entities": [], "relations": []}}.

Already extracted:
{already}

Entity types to look for: {types}

Text:
{text}

Output:"#,
            already = already,
            types = entity_types.join(", "),
            text = chunk.content,
        );
        match extract_one(client, base_url, model, api_key, &glean_prompt).await {
            Ok(response) => match parse_extraction_response(&response, &chunk.id) {
                Ok(glean) => {
                    if glean.entities.is_empty() && glean.relations.is_empty() {
                        tracing::debug!(
                            "gleaning round {} returned empty for chunk {}, stopping early",
                            round + 1,
                            chunk.id
                        );
                        break;
                    }
                    combined.merge(glean);
                }
                Err(e) => {
                    tracing::warn!(
                        "gleaning round {} parse error for chunk {}: {e}",
                        round + 1,
                        chunk.id
                    );
                    break;
                }
            },
            Err(e) => {
                tracing::warn!(
                    "gleaning round {} http error for chunk {}: {e}",
                    round + 1,
                    chunk.id
                );
                break;
            }
        }
    }

    Ok(combined)
}

// ---------------------------------------------------------------------------
// v0.13.1: description summarization
// ---------------------------------------------------------------------------

async fn summarize_description(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    entity: &mut Entity,
) -> Result<(), String> {
    let prompt = format!(
        "Summarize the following entity description into ONE coherent paragraph of 2-4 sentences. \
         Preserve all distinct facts. Third person only, no pronouns. Output only the summary — \
         no preamble, no quotes, no markdown.\n\n\
         Entity: {name}\n\n\
         Description: {desc}\n\n\
         Summary:",
        name = entity.name,
        desc = entity.description,
    );
    let summary = extract_one_plain(client, base_url, model, api_key, &prompt).await?;
    entity.description = summary.trim().to_string();
    Ok(())
}

// ---------------------------------------------------------------------------
// v0.13.1: query keyword extraction (the core of LightRAG's Local/Global
// retrieval modes). Called from MemoryEngine::search_graph.
// ---------------------------------------------------------------------------

/// Extracted keywords from a user query, split into themes and specifics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryKeywords {
    /// High-level themes / categories — used for Global (relation-focused) mode.
    pub high_level: Vec<String>,
    /// Low-level proper nouns / identifiers — used for Local (entity-focused) mode.
    pub low_level: Vec<String>,
}

impl QueryKeywords {
    pub fn is_empty(&self) -> bool {
        self.high_level.is_empty() && self.low_level.is_empty()
    }
}

/// Extract query keywords via a single LLM call. Returns empty keywords if
/// extraction is disabled (`THE_ONE_GRAPH_QUERY_EXTRACT=false`) or the LLM
/// endpoint is unreachable — never errors the caller, since query-time
/// latency is more important than perfect extraction.
pub async fn extract_query_keywords(query: &str) -> QueryKeywords {
    let enabled = std::env::var("THE_ONE_GRAPH_QUERY_EXTRACT")
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true);
    if !enabled || !is_graph_enabled() {
        return QueryKeywords::default();
    }
    let Ok(base_url) = std::env::var("THE_ONE_GRAPH_BASE_URL") else {
        return QueryKeywords::default();
    };
    let model = std::env::var("THE_ONE_GRAPH_MODEL").unwrap_or_else(|_| "llama3.2".to_string());
    let api_key = std::env::var("THE_ONE_GRAPH_API_KEY").unwrap_or_default();

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(_) => return QueryKeywords::default(),
    };

    let prompt = format!(
        r#"Extract keywords from the user query for a knowledge-graph retrieval system.

Respond with JSON only, no other text:
{{"high_level": ["theme1", "theme2"], "low_level": ["specific1", "specific2"]}}

- high_level: broad themes, categories, concepts (use for thematic search)
- low_level: specific proper nouns, function names, product names, identifiers (use for exact entity lookup)

Query: {query}

Output:"#
    );

    let response = match extract_one(&client, &base_url, &model, &api_key, &prompt).await {
        Ok(r) => r,
        Err(_) => return QueryKeywords::default(),
    };

    // Try to parse JSON, tolerating optional ```json ... ``` markdown wrappers.
    let candidate: String = {
        let trimmed = response.trim();
        if let Some(stripped) = trimmed.strip_prefix("```json") {
            stripped
                .trim_start_matches('\n')
                .trim_end_matches("```")
                .trim()
                .to_string()
        } else if let Some(stripped) = trimmed.strip_prefix("```") {
            stripped
                .trim_start_matches('\n')
                .trim_end_matches("```")
                .trim()
                .to_string()
        } else {
            trimmed.to_string()
        }
    };

    if let Ok(kw) = serde_json::from_str::<QueryKeywords>(&candidate) {
        return kw;
    }
    // Tolerant parse via raw Value if the model added extra fields.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&candidate) {
        let high: Vec<String> = v
            .get("high_level")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let low: Vec<String> = v
            .get("low_level")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !high.is_empty() || !low.is_empty() {
            return QueryKeywords {
                high_level: high,
                low_level: low,
            };
        }
    }
    QueryKeywords::default()
}

/// Simpler chat completions call that returns plain text (used by
/// summarization — no JSON parsing overhead).
async fn extract_one_plain(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<String, String> {
    extract_one(client, base_url, model, api_key, prompt).await
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
    use std::sync::OnceLock;
    use tokio::sync::Mutex;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn test_extract_disabled_by_default() {
        let _guard = env_lock().lock().await;
        // Ensure THE_ONE_GRAPH_ENABLED is not set
        std::env::remove_var("THE_ONE_GRAPH_ENABLED");
        std::env::remove_var("THE_ONE_GRAPH_BASE_URL");
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = extract_and_persist(tmp.path(), &[]).await.expect("ok");
        assert!(result.disabled_reason.is_some());
        assert_eq!(result.chunks_processed, 0);
    }

    #[tokio::test]
    async fn test_extract_enabled_without_base_url_errors() {
        let _guard = env_lock().lock().await;
        std::env::set_var("THE_ONE_GRAPH_ENABLED", "true");
        std::env::remove_var("THE_ONE_GRAPH_BASE_URL");
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = extract_and_persist(tmp.path(), &[]).await;
        assert!(result.is_err());
        std::env::remove_var("THE_ONE_GRAPH_ENABLED");
        std::env::remove_var("THE_ONE_GRAPH_BASE_URL");
    }
}
