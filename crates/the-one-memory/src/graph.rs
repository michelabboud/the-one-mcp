//! Lightweight knowledge graph for entity-relation storage.
//!
//! Inspired by LightRAG: instead of only doing vector search on chunks, we also
//! extract entities and relationships during indexing and store them in a graph.
//! At query time, entities mentioned in the query are looked up and their
//! neighbors are retrieved, providing graph-enhanced context alongside vector
//! search results.
//!
//! The graph is stored in SQLite for simplicity (no external graph DB needed).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// An entity extracted from a document chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub entity_type: String,
    pub description: String,
    /// Which chunks reference this entity (chunk IDs).
    pub source_chunks: Vec<String>,
}

/// A relationship between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub description: String,
    pub weight: f32,
    /// Which chunks this relation was extracted from.
    pub source_chunks: Vec<String>,
}

/// Extraction result from a single chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// A graph search result — an entity with its relations and connected entities.
#[derive(Debug, Clone)]
pub struct GraphSearchResult {
    pub entity: Entity,
    /// Related entities (1-hop neighbors).
    pub neighbors: Vec<(Relation, Entity)>,
    /// Chunk IDs reachable through this entity + its neighbors.
    pub related_chunk_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph (SQLite-backed)
// ---------------------------------------------------------------------------

/// In-memory knowledge graph backed by HashMaps.
///
/// Designed to be persisted alongside the project's SQLite DB. Uses simple
/// adjacency-list representation optimized for the entity-lookup + neighbor
/// traversal pattern that LightRAG recommends.
pub struct KnowledgeGraph {
    entities: HashMap<String, Entity>,
    /// Adjacency list: entity_name → Vec<Relation>
    relations: HashMap<String, Vec<Relation>>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            relations: HashMap::new(),
        }
    }

    /// Load graph from a JSON file, or create empty if file doesn't exist.
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read graph: {e}"))?;
        let stored: StoredGraph =
            serde_json::from_str(&content).map_err(|e| format!("failed to parse graph: {e}"))?;

        let mut graph = Self::new();
        for entity in stored.entities {
            graph.entities.insert(entity.name.to_lowercase(), entity);
        }
        for relation in stored.relations {
            graph
                .relations
                .entry(relation.source.to_lowercase())
                .or_default()
                .push(relation.clone());
            graph
                .relations
                .entry(relation.target.to_lowercase())
                .or_default()
                .push(relation);
        }
        Ok(graph)
    }

    /// Save graph to a JSON file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        // Deduplicate relations for storage
        let mut seen = HashSet::new();
        let relations: Vec<Relation> = self
            .relations
            .values()
            .flatten()
            .filter(|r| {
                let key = format!(
                    "{}::{}::{}",
                    r.source.to_lowercase(),
                    r.target.to_lowercase(),
                    r.relation_type.to_lowercase()
                );
                seen.insert(key)
            })
            .cloned()
            .collect();

        let stored = StoredGraph {
            entities: self.entities.values().cloned().collect(),
            relations,
        };
        let json = serde_json::to_string_pretty(&stored)
            .map_err(|e| format!("failed to serialize graph: {e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create graph dir: {e}"))?;
        }
        std::fs::write(path, json).map_err(|e| format!("failed to write graph: {e}"))?;
        Ok(())
    }

    /// Merge extraction results into the graph, deduplicating entities.
    ///
    /// Entity names are normalized via [`normalize_entity_name`] before dedup
    /// so that `"rust"`, `"Rust"`, and `"  Rust  "` all collapse to a single
    /// entity. Relation endpoints are normalized identically so edges don't
    /// reference orphaned names. Shipped in v0.13.1 as part of the LightRAG
    /// parity pass.
    pub fn merge_extraction(&mut self, extraction: &ExtractionResult) {
        for entity in &extraction.entities {
            let canonical = normalize_entity_name(&entity.name);
            if canonical.is_empty() {
                continue;
            }
            let key = canonical.to_lowercase();
            let existing = self.entities.entry(key).or_insert_with(|| Entity {
                name: canonical.clone(),
                entity_type: entity.entity_type.clone(),
                description: String::new(),
                source_chunks: Vec::new(),
            });
            // Prefer the canonical display name even if a previous merge used
            // a less-polished form.
            existing.name = canonical;
            // Merge descriptions (append if new info)
            if !entity.description.is_empty() && !existing.description.contains(&entity.description)
            {
                if !existing.description.is_empty() {
                    existing.description.push_str("; ");
                }
                existing.description.push_str(&entity.description);
            }
            // Merge source chunks
            for chunk_id in &entity.source_chunks {
                if !existing.source_chunks.contains(chunk_id) {
                    existing.source_chunks.push(chunk_id.clone());
                }
            }
        }

        for relation in &extraction.relations {
            let source_canonical = normalize_entity_name(&relation.source);
            let target_canonical = normalize_entity_name(&relation.target);
            if source_canonical.is_empty() || target_canonical.is_empty() {
                continue;
            }
            // Store a canonicalised clone so the edges reference the same
            // entity name format as the nodes table.
            let mut canonical_relation = relation.clone();
            canonical_relation.source = source_canonical.clone();
            canonical_relation.target = target_canonical.clone();

            let source_key = source_canonical.to_lowercase();
            let entry = self.relations.entry(source_key).or_default();
            let exists = entry.iter().any(|r| {
                r.source.eq_ignore_ascii_case(&canonical_relation.source)
                    && r.target.eq_ignore_ascii_case(&canonical_relation.target)
                    && r.relation_type
                        .eq_ignore_ascii_case(&canonical_relation.relation_type)
            });
            if !exists {
                entry.push(canonical_relation.clone());
                // Also add reverse lookup under the target key
                self.relations
                    .entry(target_canonical.to_lowercase())
                    .or_default()
                    .push(canonical_relation);
            }
        }
    }

    /// Search the graph for entities matching any of the given terms.
    /// Returns entities + their 1-hop neighbors + all related chunk IDs.
    pub fn search(&self, terms: &[String], max_results: usize) -> Vec<GraphSearchResult> {
        let mut results = Vec::new();
        let mut seen_entities = HashSet::new();

        for term in terms {
            let term_lower = term.to_lowercase();

            // Exact match
            if let Some(entity) = self.entities.get(&term_lower) {
                if seen_entities.insert(term_lower.clone()) {
                    results.push(self.build_search_result(entity));
                }
            }

            // Partial match (entity name contains the term)
            for (key, entity) in &self.entities {
                if key.contains(&term_lower) && seen_entities.insert(key.clone()) {
                    results.push(self.build_search_result(entity));
                }
            }

            if results.len() >= max_results {
                break;
            }
        }

        results.truncate(max_results);
        results
    }

    fn build_search_result(&self, entity: &Entity) -> GraphSearchResult {
        let key = entity.name.to_lowercase();
        let mut neighbors = Vec::new();
        let mut related_chunk_ids: Vec<String> = entity.source_chunks.clone();

        if let Some(relations) = self.relations.get(&key) {
            for relation in relations {
                let neighbor_key = if relation.source.to_lowercase() == key {
                    relation.target.to_lowercase()
                } else {
                    relation.source.to_lowercase()
                };
                if let Some(neighbor) = self.entities.get(&neighbor_key) {
                    for cid in &neighbor.source_chunks {
                        if !related_chunk_ids.contains(cid) {
                            related_chunk_ids.push(cid.clone());
                        }
                    }
                    for cid in &relation.source_chunks {
                        if !related_chunk_ids.contains(cid) {
                            related_chunk_ids.push(cid.clone());
                        }
                    }
                    neighbors.push((relation.clone(), neighbor.clone()));
                }
            }
        }

        GraphSearchResult {
            entity: entity.clone(),
            neighbors,
            related_chunk_ids,
        }
    }

    /// Get total entity count.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Iterate all entities (read-only). v0.13.1 — used by the broker to
    /// upsert entity vectors into Qdrant after extraction.
    pub fn all_entities(&self) -> Vec<Entity> {
        self.entities.values().cloned().collect()
    }

    /// Get a mutable reference to an entity by name. v0.13.1 — used by the
    /// description-summarization pass in `graph_extractor`.
    pub fn get_entity_mut(&mut self, name: &str) -> Option<&mut Entity> {
        let key = normalize_entity_name(name).to_lowercase();
        self.entities.get_mut(&key)
    }

    /// Iterate all unique relations (read-only). Deduplicates the internal
    /// forward/reverse lookup by keying on (source, target, relation_type).
    /// v0.13.1 — used by the broker to upsert relation vectors into Qdrant.
    pub fn all_relations(&self) -> Vec<Relation> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for rel in self.relations.values().flatten() {
            let key = format!(
                "{}|{}|{}",
                rel.source.to_lowercase(),
                rel.target.to_lowercase(),
                rel.relation_type.to_lowercase()
            );
            if seen.insert(key) {
                out.push(rel.clone());
            }
        }
        out
    }

    /// Get total unique relation count.
    pub fn relation_count(&self) -> usize {
        let mut seen = HashSet::new();
        self.relations
            .values()
            .flatten()
            .filter(|r| {
                let key = format!(
                    "{}::{}::{}",
                    r.source.to_lowercase(),
                    r.target.to_lowercase(),
                    r.relation_type.to_lowercase()
                );
                seen.insert(key)
            })
            .count()
    }

    /// Remove all entities and relations sourced from a given chunk ID.
    pub fn remove_by_chunk(&mut self, chunk_id: &str) {
        // Remove chunk references from entities
        self.entities.retain(|_, entity| {
            entity.source_chunks.retain(|c| c != chunk_id);
            !entity.source_chunks.is_empty()
        });

        // Remove relations referencing this chunk
        for relations in self.relations.values_mut() {
            relations.retain(|r| !r.source_chunks.contains(&chunk_id.to_string()));
        }
        self.relations.retain(|_, v| !v.is_empty());
    }

    /// Clear the entire graph.
    pub fn clear(&mut self) {
        self.entities.clear();
        self.relations.clear();
    }
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct StoredGraph {
    entities: Vec<Entity>,
    relations: Vec<Relation>,
}

// ---------------------------------------------------------------------------
// Entity extraction prompt builder
// ---------------------------------------------------------------------------

/// Build a prompt for LLM-based entity-relation extraction from a text chunk.
///
/// This follows LightRAG's approach: ask the LLM to identify entities and their
/// relationships in a structured format that can be parsed.
pub fn build_extraction_prompt(chunk_text: &str, entity_types: &[&str]) -> String {
    let types_str = entity_types.join(", ");
    format!(
        r#"Extract entities and relationships from the following text.

Entity types to look for: {types_str}

For each entity, provide:
- name: The entity name
- type: One of the entity types above
- description: Brief description

For each relationship, provide:
- source: Source entity name
- target: Target entity name
- type: Relationship type (e.g., "uses", "depends_on", "contains", "implements")
- description: Brief description

Output as JSON:
{{
  "entities": [{{"name": "...", "type": "...", "description": "..."}}],
  "relations": [{{"source": "...", "target": "...", "type": "...", "description": "..."}}]
}}

Text:
{chunk_text}"#
    )
}

/// Parse the LLM response into an ExtractionResult.
/// Tolerant of minor format variations.
pub fn parse_extraction_response(
    response: &str,
    chunk_id: &str,
) -> Result<ExtractionResult, String> {
    // Try to find JSON in the response (LLMs often wrap in markdown code blocks)
    let json_str = extract_json_block(response);

    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("failed to parse extraction JSON: {e}"))?;

    let entities = parsed
        .get("entities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let entity_type = item.get("type")?.as_str()?.to_string();
                    let description = item
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(Entity {
                        name,
                        entity_type,
                        description,
                        source_chunks: vec![chunk_id.to_string()],
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let relations = parsed
        .get("relations")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let source = item.get("source")?.as_str()?.to_string();
                    let target = item.get("target")?.as_str()?.to_string();
                    let relation_type = item.get("type")?.as_str()?.to_string();
                    let description = item
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(Relation {
                        source,
                        target,
                        relation_type,
                        description,
                        weight: 1.0,
                        source_chunks: vec![chunk_id.to_string()],
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ExtractionResult {
        entities,
        relations,
    })
}

/// Extract a JSON block from text that may be wrapped in ```json ... ```
fn extract_json_block(text: &str) -> &str {
    // Try to find ```json ... ``` block
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            return text[json_start..json_start + end].trim();
        }
    }
    // Try to find ``` ... ``` block
    if let Some(start) = text.find("```") {
        let json_start = start + 3;
        // Skip language identifier on same line
        let actual_start = text[json_start..]
            .find('\n')
            .map(|n| json_start + n + 1)
            .unwrap_or(json_start);
        if let Some(end) = text[actual_start..].find("```") {
            return text[actual_start..actual_start + end].trim();
        }
    }
    // Try to find raw JSON object
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return &text[start..=end];
        }
    }
    text.trim()
}

// ---------------------------------------------------------------------------
// Entity name normalization (v0.13.1)
// ---------------------------------------------------------------------------

/// Normalize a raw entity name from an LLM extraction into a canonical form
/// that can be used as a dedup key + display name.
///
/// Rules (mirrored from LightRAG's `clean_str` logic):
/// 1. Unicode-normalize via `.trim()` and collapse whitespace runs
/// 2. Strip leading/trailing punctuation (quotes, parentheses, dots, commas)
/// 3. Preserve pure acronyms (all-uppercase alphabetic like `RUST`, `API`)
/// 4. Title-case word-by-word, skipping 1-2 char words to keep things like
///    `of`, `to`, `in` intact
///
/// This is a pure function and cheap to call — it runs inside merge loops.
pub fn normalize_entity_name(raw: &str) -> String {
    // 1. trim + collapse whitespace
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    // 2. strip surrounding punctuation
    let stripped: String = collapsed
        .trim_matches(|c: char| {
            c == '"'
                || c == '\''
                || c == '.'
                || c == ','
                || c == '('
                || c == ')'
                || c == '['
                || c == ']'
        })
        .to_string();
    if stripped.is_empty() {
        return String::new();
    }
    // 3. preserve all-uppercase acronyms (must have at least one letter)
    let has_letter = stripped.chars().any(|c| c.is_alphabetic());
    let all_upper = has_letter
        && stripped
            .chars()
            .all(|c| !c.is_alphabetic() || c.is_uppercase());
    if all_upper {
        return stripped;
    }
    // 4. title-case
    stripped
        .split(' ')
        .map(|word| {
            if word.is_empty() {
                String::new()
            } else if word.len() <= 2 {
                word.to_string()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>()
                            + chars.as_str().to_lowercase().as_str()
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// ExtractionResult::merge — used by the gleaning loop (v0.13.1) to combine
// multiple passes over the same chunk while deduplicating entities/relations.
// ---------------------------------------------------------------------------

impl ExtractionResult {
    /// Merge another extraction's entities and relations into this one,
    /// deduplicating by normalized entity name + relation triple.
    ///
    /// Used by [`crate::graph_extractor::extract_with_gleaning`] to
    /// accumulate results across the initial extraction pass and any
    /// follow-up "gleaning" passes without counting the same entity twice.
    pub fn merge(&mut self, other: ExtractionResult) {
        for incoming in other.entities {
            let canonical = normalize_entity_name(&incoming.name);
            if canonical.is_empty() {
                continue;
            }
            let key = canonical.to_lowercase();
            if let Some(existing) = self
                .entities
                .iter_mut()
                .find(|e| normalize_entity_name(&e.name).to_lowercase() == key)
            {
                if !incoming.description.is_empty()
                    && !existing.description.contains(&incoming.description)
                {
                    if !existing.description.is_empty() {
                        existing.description.push_str("; ");
                    }
                    existing.description.push_str(&incoming.description);
                }
                for chunk_id in incoming.source_chunks {
                    if !existing.source_chunks.contains(&chunk_id) {
                        existing.source_chunks.push(chunk_id);
                    }
                }
            } else {
                let mut normalized = incoming;
                normalized.name = canonical;
                self.entities.push(normalized);
            }
        }

        for incoming in other.relations {
            let src = normalize_entity_name(&incoming.source);
            let tgt = normalize_entity_name(&incoming.target);
            if src.is_empty() || tgt.is_empty() {
                continue;
            }
            let exists = self.relations.iter().any(|r| {
                normalize_entity_name(&r.source).eq_ignore_ascii_case(&src)
                    && normalize_entity_name(&r.target).eq_ignore_ascii_case(&tgt)
                    && r.relation_type
                        .eq_ignore_ascii_case(&incoming.relation_type)
            });
            if !exists {
                let mut normalized = incoming;
                normalized.source = src;
                normalized.target = tgt;
                self.relations.push(normalized);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_entity_name_title_case() {
        assert_eq!(normalize_entity_name("rust"), "Rust");
        assert_eq!(normalize_entity_name("  rust  "), "Rust");
        assert_eq!(normalize_entity_name("Rust"), "Rust");
    }

    #[test]
    fn test_normalize_entity_name_preserves_acronyms() {
        assert_eq!(normalize_entity_name("RUST"), "RUST");
        assert_eq!(normalize_entity_name("API"), "API");
        assert_eq!(normalize_entity_name("HTTP"), "HTTP");
        assert_eq!(normalize_entity_name("gRPC"), "Grpc"); // mixed-case not an acronym
    }

    #[test]
    fn test_normalize_entity_name_multi_word() {
        assert_eq!(
            normalize_entity_name("model context protocol"),
            "Model Context Protocol"
        );
        // Short words stay as-is so "of", "to", "in" don't get title-cased
        assert_eq!(
            normalize_entity_name("bureau of investigation"),
            "Bureau of Investigation"
        );
    }

    #[test]
    fn test_normalize_entity_name_strips_punctuation() {
        assert_eq!(normalize_entity_name("\"rust\""), "Rust");
        assert_eq!(normalize_entity_name("(Rust)"), "Rust");
        assert_eq!(normalize_entity_name("rust."), "Rust");
    }

    #[test]
    fn test_normalize_entity_name_empty() {
        assert_eq!(normalize_entity_name(""), "");
        assert_eq!(normalize_entity_name("   "), "");
        assert_eq!(normalize_entity_name("\"\""), "");
    }

    #[test]
    fn test_merge_extraction_dedups_across_cases() {
        let mut graph = KnowledgeGraph::new();
        let extraction1 = ExtractionResult {
            entities: vec![Entity {
                name: "rust".to_string(),
                entity_type: "technology".to_string(),
                description: "A systems programming language".to_string(),
                source_chunks: vec!["chunk-1".to_string()],
            }],
            relations: vec![],
        };
        let extraction2 = ExtractionResult {
            entities: vec![Entity {
                name: "Rust".to_string(),
                entity_type: "technology".to_string(),
                description: "Memory safe without GC".to_string(),
                source_chunks: vec!["chunk-2".to_string()],
            }],
            relations: vec![],
        };
        let extraction3 = ExtractionResult {
            entities: vec![Entity {
                name: "  RUST  ".to_string(),
                entity_type: "technology".to_string(),
                description: "".to_string(),
                source_chunks: vec!["chunk-3".to_string()],
            }],
            relations: vec![],
        };
        graph.merge_extraction(&extraction1);
        graph.merge_extraction(&extraction2);
        graph.merge_extraction(&extraction3);
        // All three should collapse into a single entity
        assert_eq!(graph.entity_count(), 1);
        // The canonical name should be "RUST" (last write wins from the acronym form)
        // and source_chunks should contain all three chunks
        let (_, entity) = graph.entities.iter().next().unwrap();
        assert_eq!(entity.source_chunks.len(), 3);
    }

    #[test]
    fn test_graph_merge_and_search() {
        let mut graph = KnowledgeGraph::new();

        let extraction = ExtractionResult {
            entities: vec![
                Entity {
                    name: "Rust".to_string(),
                    entity_type: "language".to_string(),
                    description: "A systems programming language".to_string(),
                    source_chunks: vec!["doc.md:0".to_string()],
                },
                Entity {
                    name: "Tokio".to_string(),
                    entity_type: "library".to_string(),
                    description: "Async runtime for Rust".to_string(),
                    source_chunks: vec!["doc.md:0".to_string()],
                },
            ],
            relations: vec![Relation {
                source: "Tokio".to_string(),
                target: "Rust".to_string(),
                relation_type: "implemented_in".to_string(),
                description: "Tokio is implemented in Rust".to_string(),
                weight: 1.0,
                source_chunks: vec!["doc.md:0".to_string()],
            }],
        };

        graph.merge_extraction(&extraction);
        assert_eq!(graph.entity_count(), 2);
        assert_eq!(graph.relation_count(), 1);

        let results = graph.search(&["rust".to_string()], 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity.name, "Rust");
        assert!(!results[0].neighbors.is_empty());
        assert!(results[0]
            .related_chunk_ids
            .contains(&"doc.md:0".to_string()));
    }

    #[test]
    fn test_graph_entity_deduplication() {
        let mut graph = KnowledgeGraph::new();

        let ext1 = ExtractionResult {
            entities: vec![Entity {
                name: "Qdrant".to_string(),
                entity_type: "database".to_string(),
                description: "Vector database".to_string(),
                source_chunks: vec!["a.md:0".to_string()],
            }],
            relations: vec![],
        };

        let ext2 = ExtractionResult {
            entities: vec![Entity {
                name: "Qdrant".to_string(),
                entity_type: "database".to_string(),
                description: "Supports cosine similarity".to_string(),
                source_chunks: vec!["b.md:0".to_string()],
            }],
            relations: vec![],
        };

        graph.merge_extraction(&ext1);
        graph.merge_extraction(&ext2);

        assert_eq!(graph.entity_count(), 1);
        let results = graph.search(&["qdrant".to_string()], 5);
        let entity = &results[0].entity;
        assert!(entity.description.contains("Vector database"));
        assert!(entity.description.contains("Supports cosine similarity"));
        assert_eq!(entity.source_chunks.len(), 2);
    }

    #[test]
    fn test_graph_remove_by_chunk() {
        let mut graph = KnowledgeGraph::new();

        let extraction = ExtractionResult {
            entities: vec![
                Entity {
                    name: "A".to_string(),
                    entity_type: "test".to_string(),
                    description: String::new(),
                    source_chunks: vec!["chunk:0".to_string()],
                },
                Entity {
                    name: "B".to_string(),
                    entity_type: "test".to_string(),
                    description: String::new(),
                    source_chunks: vec!["chunk:0".to_string(), "chunk:1".to_string()],
                },
            ],
            relations: vec![],
        };

        graph.merge_extraction(&extraction);
        assert_eq!(graph.entity_count(), 2);

        graph.remove_by_chunk("chunk:0");
        // A was only in chunk:0, so it should be removed
        assert_eq!(graph.entity_count(), 1);
        // B still has chunk:1
        let results = graph.search(&["b".to_string()], 5);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_graph_save_and_load() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("graph.json");

        let mut graph = KnowledgeGraph::new();
        graph.merge_extraction(&ExtractionResult {
            entities: vec![Entity {
                name: "Test".to_string(),
                entity_type: "concept".to_string(),
                description: "A test entity".to_string(),
                source_chunks: vec!["test.md:0".to_string()],
            }],
            relations: vec![],
        });

        graph.save_to_file(&path).expect("save");
        let loaded = KnowledgeGraph::load_from_file(&path).expect("load");
        assert_eq!(loaded.entity_count(), 1);
    }

    #[test]
    fn test_parse_extraction_response_json() {
        let response = r#"```json
{
  "entities": [
    {"name": "Rust", "type": "language", "description": "Systems language"}
  ],
  "relations": [
    {"source": "Tokio", "target": "Rust", "type": "uses", "description": "Built with Rust"}
  ]
}
```"#;

        let result = parse_extraction_response(response, "chunk:0").expect("parse");
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "Rust");
        assert_eq!(result.relations.len(), 1);
        assert_eq!(result.relations[0].source, "Tokio");
    }

    #[test]
    fn test_parse_extraction_response_raw_json() {
        let response =
            r#"{"entities": [{"name": "X", "type": "t", "description": "d"}], "relations": []}"#;
        let result = parse_extraction_response(response, "c:0").expect("parse");
        assert_eq!(result.entities.len(), 1);
    }

    #[test]
    fn test_build_extraction_prompt() {
        let prompt =
            build_extraction_prompt("Some text about Rust", &["language", "library", "concept"]);
        assert!(prompt.contains("language, library, concept"));
        assert!(prompt.contains("Some text about Rust"));
    }

    #[test]
    fn test_graph_load_nonexistent_returns_empty() {
        let graph =
            KnowledgeGraph::load_from_file(Path::new("/nonexistent/path.json")).expect("load");
        assert_eq!(graph.entity_count(), 0);
    }
}
