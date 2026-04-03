pub mod chunker;
pub mod embeddings;
pub mod qdrant;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chunker::{chunk_markdown, ChunkMeta};
use crate::embeddings::EmbeddingProvider;
use crate::qdrant::{AsyncQdrantBackend, QdrantOptions, QdrantPayload, QdrantPoint};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for creating a MemoryEngine with API embeddings + Qdrant.
pub struct ApiEngineConfig<'a> {
    pub embedding_base_url: &'a str,
    pub embedding_api_key: Option<&'a str>,
    pub embedding_model: &'a str,
    pub embedding_dims: usize,
    pub qdrant_url: &'a str,
    pub project_id: &'a str,
    pub qdrant_options: QdrantOptions,
    pub max_chunk_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct MemorySearchRequest {
    pub query: String,
    pub top_k: usize,
}

impl Default for MemorySearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            top_k: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub chunk: ChunkMeta,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// MemoryEngine
// ---------------------------------------------------------------------------

pub struct MemoryEngine {
    chunks: Vec<ChunkMeta>,
    by_id: HashMap<String, usize>,
    embedding_provider: Box<dyn EmbeddingProvider>,
    qdrant: Option<AsyncQdrantBackend>,
    max_chunk_tokens: usize,
}

impl MemoryEngine {
    /// Create with fastembed local embeddings, no Qdrant (in-memory keyword fallback).
    pub fn new_local(model_name: &str, max_chunk_tokens: usize) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: None,
            max_chunk_tokens,
        })
    }

    /// Create with fastembed local embeddings + Qdrant HTTP backend.
    pub fn new_with_qdrant(
        model_name: &str,
        qdrant_url: &str,
        project_id: &str,
        qdrant_options: QdrantOptions,
        max_chunk_tokens: usize,
    ) -> Result<Self, String> {
        let provider = crate::embeddings::FastEmbedProvider::new(model_name)?;
        let qdrant = AsyncQdrantBackend::new(qdrant_url, project_id, qdrant_options)?;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: Some(qdrant),
            max_chunk_tokens,
        })
    }

    /// Create with API embeddings + Qdrant HTTP backend.
    pub fn new_api(config: ApiEngineConfig<'_>) -> Result<Self, String> {
        let provider = crate::embeddings::ApiEmbeddingProvider::new(
            config.embedding_base_url,
            config.embedding_api_key,
            config.embedding_model,
            config.embedding_dims,
        );
        let qdrant =
            AsyncQdrantBackend::new(config.qdrant_url, config.project_id, config.qdrant_options)?;
        let max_chunk_tokens = config.max_chunk_tokens;
        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            embedding_provider: Box::new(provider),
            qdrant: Some(qdrant),
            max_chunk_tokens,
        })
    }

    /// Ingest all `.md` files from a directory tree.
    /// Returns the number of chunks indexed.
    pub async fn ingest_markdown_tree(
        &mut self,
        docs_root: &Path,
    ) -> Result<usize, std::io::Error> {
        if !docs_root.exists() {
            return Ok(0);
        }

        let docs_root_prefix = docs_root.display().to_string();

        // Walk directory tree for .md files
        let mut new_chunks: Vec<ChunkMeta> = Vec::new();
        let mut stack = vec![docs_root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }

                let content = fs::read_to_string(&path)?;
                let relative_path = path.display().to_string();
                let chunks = chunk_markdown(&relative_path, &content, self.max_chunk_tokens);
                new_chunks.extend(chunks);
            }
        }

        // Remove old chunks from this docs_root, then add new ones
        self.chunks
            .retain(|c| !c.source_path.starts_with(&docs_root_prefix));
        let count = new_chunks.len();
        self.chunks.extend(new_chunks);

        // Rebuild the by_id index
        self.by_id.clear();
        for (idx, chunk) in self.chunks.iter().enumerate() {
            self.by_id.insert(chunk.id.clone(), idx);
        }

        // If qdrant is available, ensure collection + embed + upsert
        if let Some(qdrant) = &self.qdrant {
            let dims = self.embedding_provider.dimensions();
            qdrant
                .ensure_collection(dims)
                .await
                .map_err(std::io::Error::other)?;

            // Embed in batches of 64
            let batch_size = 64;
            for batch_start in (0..self.chunks.len()).step_by(batch_size) {
                let batch_end = (batch_start + batch_size).min(self.chunks.len());
                let texts: Vec<String> = self.chunks[batch_start..batch_end]
                    .iter()
                    .map(|c| c.content.clone())
                    .collect();

                let vectors = self
                    .embedding_provider
                    .embed_batch(&texts)
                    .await
                    .map_err(std::io::Error::other)?;

                let points: Vec<QdrantPoint> = self.chunks[batch_start..batch_end]
                    .iter()
                    .zip(vectors.into_iter())
                    .map(|(chunk, vector)| QdrantPoint {
                        id: chunk.id.clone(),
                        vector,
                        payload: QdrantPayload {
                            chunk_id: chunk.id.clone(),
                            source_path: chunk.source_path.clone(),
                            heading: chunk.heading_hierarchy.last().cloned().unwrap_or_default(),
                            chunk_index: chunk.chunk_index,
                        },
                    })
                    .collect();

                qdrant
                    .upsert_points(points)
                    .await
                    .map_err(std::io::Error::other)?;
            }
        }

        Ok(count)
    }

    /// Search for relevant chunks.
    pub async fn search(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        if let Some(qdrant) = &self.qdrant {
            // Embed the query
            let query_vector = match self.embedding_provider.embed_single(&request.query).await {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            };

            // Search qdrant
            let results = match qdrant.search(query_vector, request.top_k, 0.0).await {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };

            // Map results back to ChunkMeta
            results
                .into_iter()
                .filter_map(|r| {
                    self.by_id
                        .get(&r.chunk_id)
                        .and_then(|&idx| self.chunks.get(idx))
                        .map(|chunk| MemorySearchResult {
                            chunk: chunk.clone(),
                            score: r.score,
                        })
                })
                .collect()
        } else {
            // Fallback: in-memory keyword search
            let query_words: Vec<String> = request
                .query
                .split_whitespace()
                .map(|w| w.to_lowercase())
                .collect();

            if query_words.is_empty() {
                return Vec::new();
            }

            let total_words = query_words.len() as f32;

            let mut scored: Vec<MemorySearchResult> = self
                .chunks
                .iter()
                .filter_map(|chunk| {
                    let content_lower = chunk.content.to_lowercase();
                    let matches = query_words
                        .iter()
                        .filter(|w| content_lower.contains(w.as_str()))
                        .count() as f32;
                    if matches > 0.0 {
                        Some(MemorySearchResult {
                            chunk: chunk.clone(),
                            score: matches / total_words,
                        })
                    } else {
                        None
                    }
                })
                .collect();

            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            scored.truncate(request.top_k);
            scored
        }
    }

    /// Fetch a specific chunk by ID.
    pub fn fetch_chunk(&self, id: &str) -> Option<ChunkMeta> {
        self.by_id.get(id).map(|&idx| self.chunks[idx].clone())
    }

    /// List all indexed document paths.
    pub fn docs_list(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .chunks
            .iter()
            .map(|c| c.source_path.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        paths.sort();
        paths
    }

    /// Get full content of a document by reconstructing from chunks.
    pub fn docs_get(&self, path: &str) -> Option<String> {
        let mut file_chunks: Vec<&ChunkMeta> = self
            .chunks
            .iter()
            .filter(|c| c.source_path == path)
            .collect();
        if file_chunks.is_empty() {
            return None;
        }
        file_chunks.sort_by_key(|c| c.chunk_index);
        Some(
            file_chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Get a section of a document by heading.
    pub fn docs_get_section(&self, path: &str, heading: &str, max_bytes: usize) -> Option<String> {
        let matching: Vec<&ChunkMeta> = self
            .chunks
            .iter()
            .filter(|c| c.source_path == path && c.heading_hierarchy.iter().any(|h| h == heading))
            .collect();
        if matching.is_empty() {
            return None;
        }
        let content: String = matching
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if content.len() > max_bytes {
            Some(content[..max_bytes].to_string())
        } else {
            Some(content)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    /// Shared FastEmbedProvider to avoid concurrent model downloads.
    static PROVIDER: LazyLock<crate::embeddings::FastEmbedProvider> = LazyLock::new(|| {
        crate::embeddings::FastEmbedProvider::new("all-MiniLM-L6-v2").expect("should init")
    });

    /// Helper: build a local-only engine reusing the shared provider is tricky
    /// because MemoryEngine owns the provider. For tests we just create engines
    /// with `new_local` which will init fastembed (the model is already cached).
    fn make_local_engine() -> MemoryEngine {
        MemoryEngine::new_local("all-MiniLM-L6-v2", 500).expect("engine should init")
    }

    #[tokio::test]
    async fn test_ingest_and_search_keyword_fallback() {
        // Force the shared provider to initialize (caches the model)
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(
            docs.join("readme.md"),
            "# Intro\nThis broker manages routing and memory search.\n# Details\nSome other content about configuration.",
        )
        .expect("write");
        fs::write(
            docs.join("guide.md"),
            "# Setup\nHow to set up the system for production use.",
        )
        .expect("write");

        let mut engine = make_local_engine();
        let count = engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");
        assert!(count >= 3, "expected at least 3 chunks, got {count}");

        // Keyword fallback search (no qdrant)
        let results = engine
            .search(&MemorySearchRequest {
                query: "routing memory".to_string(),
                top_k: 5,
            })
            .await;
        assert!(
            !results.is_empty(),
            "should find at least one result for 'routing memory'"
        );
        assert!(results[0].score > 0.0);
    }

    #[tokio::test]
    async fn test_docs_list_and_get() {
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(
            docs.join("readme.md"),
            "# Intro\nHello world\n# Usage\nUse it well",
        )
        .expect("write");

        let mut engine = make_local_engine();
        engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");

        let listed = engine.docs_list();
        assert_eq!(listed.len(), 1);

        let content = engine.docs_get(&listed[0]).expect("should have doc");
        assert!(content.contains("Hello world"));
        assert!(content.contains("Use it well"));
    }

    #[tokio::test]
    async fn test_fetch_chunk_by_id() {
        let _ = &*PROVIDER;

        let temp = tempfile::tempdir().expect("tempdir");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("mkdir");
        fs::write(docs.join("readme.md"), "# Intro\nSome content here").expect("write");

        let mut engine = make_local_engine();
        engine
            .ingest_markdown_tree(&docs)
            .await
            .expect("ingest should succeed");

        // The chunk ID format from the chunker is "{source_path}:{chunk_index}"
        let listed = engine.docs_list();
        let chunk_id = format!("{}:0", listed[0]);
        let chunk = engine.fetch_chunk(&chunk_id).expect("chunk should exist");
        assert!(chunk.content.contains("Intro"));
        assert!(chunk.content.contains("Some content"));
    }
}
