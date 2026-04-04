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
    pub fn merge_extraction(&mut self, extraction: &ExtractionResult) {
        for entity in &extraction.entities {
            let key = entity.name.to_lowercase();
            let existing = self.entities.entry(key).or_insert_with(|| Entity {
                name: entity.name.clone(),
                entity_type: entity.entity_type.clone(),
                description: String::new(),
                source_chunks: Vec::new(),
            });
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
            let source_key = relation.source.to_lowercase();
            let entry = self.relations.entry(source_key).or_default();
            // Check for duplicate relation
            let exists = entry.iter().any(|r| {
                r.source.to_lowercase() == relation.source.to_lowercase()
                    && r.target.to_lowercase() == relation.target.to_lowercase()
                    && r.relation_type.to_lowercase() == relation.relation_type.to_lowercase()
            });
            if !exists {
                entry.push(relation.clone());
                // Also add reverse lookup
                self.relations
                    .entry(relation.target.to_lowercase())
                    .or_default()
                    .push(relation.clone());
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
