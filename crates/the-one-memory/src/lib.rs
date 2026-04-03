pub mod chunker;
pub mod embeddings;
pub mod qdrant;

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use reqwest::blocking::Client;
use reqwest::Certificate;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub trait EmbeddingProvider {
    fn name(&self) -> &'static str;
    fn embed(&self, text: &str) -> Vec<f32>;
}

#[derive(Debug, Default, Clone)]
pub struct LocalEmbeddingProvider;

#[derive(Debug, Default, Clone)]
pub struct HostedEmbeddingProvider;

impl EmbeddingProvider for LocalEmbeddingProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        embed_hash(text, 16, 1.0)
    }
}

impl EmbeddingProvider for HostedEmbeddingProvider {
    fn name(&self) -> &'static str {
        "hosted"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        embed_hash(text, 16, 1.2)
    }
}

pub trait VectorBackend {
    fn reindex(
        &mut self,
        chunks: &[MemoryChunk],
        embedder: &dyn EmbeddingProvider,
    ) -> std::io::Result<()>;
    fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn EmbeddingProvider,
    ) -> Vec<(String, usize)>;
}

#[derive(Debug, Default)]
pub struct KeywordVectorBackend {
    vectors_by_chunk_id: HashMap<String, Vec<f32>>,
}

impl VectorBackend for KeywordVectorBackend {
    fn reindex(
        &mut self,
        chunks: &[MemoryChunk],
        embedder: &dyn EmbeddingProvider,
    ) -> std::io::Result<()> {
        self.vectors_by_chunk_id.clear();
        for chunk in chunks {
            self.vectors_by_chunk_id
                .insert(chunk.id.clone(), embedder.embed(&chunk.content));
        }
        Ok(())
    }

    fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn EmbeddingProvider,
    ) -> Vec<(String, usize)> {
        let query_vector = embedder.embed(query);
        let mut scored = self
            .vectors_by_chunk_id
            .iter()
            .map(|(chunk_id, vector)| {
                (
                    chunk_id.clone(),
                    similarity_score_percent(&query_vector, vector),
                )
            })
            .filter(|(_, score)| *score > 0)
            .collect::<Vec<_>>();
        scored.sort_by_key(|(_, score)| Reverse(*score));
        scored.into_iter().take(top_k).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PersistedVector {
    id: String,
    vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct QdrantLocalBackend {
    index_path: std::path::PathBuf,
    vectors_by_chunk_id: HashMap<String, Vec<f32>>,
}

impl QdrantLocalBackend {
    pub fn new(state_dir: &Path, project_id: &str) -> std::io::Result<Self> {
        let qdrant_dir = state_dir.join("qdrant");
        fs::create_dir_all(&qdrant_dir)?;
        let index_path = qdrant_dir.join(format!("{project_id}.index.json"));

        let mut backend = Self {
            index_path,
            vectors_by_chunk_id: HashMap::new(),
        };
        backend.load_index()?;
        Ok(backend)
    }

    fn load_index(&mut self) -> std::io::Result<()> {
        if !self.index_path.exists() {
            return Ok(());
        }
        let body = fs::read_to_string(&self.index_path)?;
        let items = serde_json::from_str::<Vec<PersistedVector>>(&body)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        self.vectors_by_chunk_id = items
            .into_iter()
            .map(|item| (item.id, item.vector))
            .collect::<HashMap<_, _>>();
        Ok(())
    }

    fn save_index(&self) -> std::io::Result<()> {
        let payload = self
            .vectors_by_chunk_id
            .iter()
            .map(|(id, vector)| PersistedVector {
                id: id.clone(),
                vector: vector.clone(),
            })
            .collect::<Vec<_>>();
        let body = serde_json::to_vec_pretty(&payload)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        fs::write(&self.index_path, body)
    }
}

impl VectorBackend for QdrantLocalBackend {
    fn reindex(
        &mut self,
        chunks: &[MemoryChunk],
        embedder: &dyn EmbeddingProvider,
    ) -> std::io::Result<()> {
        self.vectors_by_chunk_id.clear();
        for chunk in chunks {
            self.vectors_by_chunk_id
                .insert(chunk.id.clone(), embedder.embed(&chunk.content));
        }
        self.save_index()
    }

    fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn EmbeddingProvider,
    ) -> Vec<(String, usize)> {
        let query_vector = embedder.embed(query);
        let mut scored = self
            .vectors_by_chunk_id
            .iter()
            .map(|(chunk_id, vector)| {
                (
                    chunk_id.clone(),
                    similarity_score_percent(&query_vector, vector),
                )
            })
            .filter(|(_, score)| *score > 0)
            .collect::<Vec<_>>();
        scored.sort_by_key(|(_, score)| Reverse(*score));
        scored.into_iter().take(top_k).collect()
    }
}

#[derive(Debug, Clone)]
pub struct QdrantHttpBackend {
    base_url: String,
    collection: String,
    client: Client,
    api_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QdrantHttpOptions {
    pub api_key: Option<String>,
    pub ca_cert_path: Option<PathBuf>,
    pub tls_insecure: bool,
}

impl QdrantHttpBackend {
    pub fn new(
        base_url: &str,
        project_id: &str,
        options: &QdrantHttpOptions,
    ) -> std::io::Result<Self> {
        let mut client_builder = Client::builder()
            .timeout(std::time::Duration::from_millis(700))
            .danger_accept_invalid_certs(options.tls_insecure);
        if let Some(ca_path) = &options.ca_cert_path {
            let pem = fs::read(ca_path)?;
            let cert = Certificate::from_pem(&pem).map_err(io_error)?;
            client_builder = client_builder.add_root_certificate(cert);
        }
        let client = client_builder.build().map_err(io_error)?;
        let collection = sanitize_collection_name(project_id);
        let backend = Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            collection,
            client,
            api_key: options.api_key.clone(),
        };
        backend.ensure_collection()?;
        Ok(backend)
    }

    fn ensure_collection(&self) -> std::io::Result<()> {
        let url = format!("{}/collections/{}", self.base_url, self.collection);
        let body = json!({
            "vectors": {
                "size": 16,
                "distance": "Cosine"
            }
        });
        let mut request = self.client.put(url).json(&body);
        if let Some(api_key) = &self.api_key {
            request = request.header("api-key", api_key);
        }
        request
            .send()
            .and_then(|resp| resp.error_for_status())
            .map_err(io_error)?;
        Ok(())
    }
}

impl VectorBackend for QdrantHttpBackend {
    fn reindex(
        &mut self,
        chunks: &[MemoryChunk],
        embedder: &dyn EmbeddingProvider,
    ) -> std::io::Result<()> {
        self.ensure_collection()?;
        let points = chunks
            .iter()
            .map(|chunk| {
                let point_id = hash_to_u64(&chunk.id);
                json!({
                    "id": point_id,
                    "vector": embedder.embed(&chunk.content),
                    "payload": {
                        "chunk_id": chunk.id,
                        "source_path": chunk.source_path,
                        "heading": chunk.heading,
                    }
                })
            })
            .collect::<Vec<_>>();

        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.base_url, self.collection
        );
        let mut request = self.client.put(url).json(&json!({ "points": points }));
        if let Some(api_key) = &self.api_key {
            request = request.header("api-key", api_key);
        }
        request
            .send()
            .and_then(|resp| resp.error_for_status())
            .map_err(io_error)?;

        Ok(())
    }

    fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn EmbeddingProvider,
    ) -> Vec<(String, usize)> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection
        );
        let request = json!({
            "vector": embedder.embed(query),
            "limit": top_k,
            "with_payload": true,
        });

        let mut call = self.client.post(url).json(&request);
        if let Some(api_key) = &self.api_key {
            call = call.header("api-key", api_key);
        }
        let response = call.send();
        let Ok(response) = response else {
            return Vec::new();
        };
        let Ok(response) = response.error_for_status() else {
            return Vec::new();
        };
        let Ok(body) = response.json::<serde_json::Value>() else {
            return Vec::new();
        };

        body.get("result")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let chunk_id = item
                            .get("payload")
                            .and_then(|payload| payload.get("chunk_id"))
                            .and_then(|id| id.as_str())
                            .map(str::to_string)?;
                        let score = item.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
                        let score_percent = (score.clamp(0.0, 1.0) * 100.0).round() as usize;
                        Some((chunk_id, score_percent))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum VectorBackendKind {
    Keyword(KeywordVectorBackend),
    QdrantLocal(QdrantLocalBackend),
    QdrantHttp(QdrantHttpBackend),
}

impl VectorBackend for VectorBackendKind {
    fn reindex(
        &mut self,
        chunks: &[MemoryChunk],
        embedder: &dyn EmbeddingProvider,
    ) -> std::io::Result<()> {
        match self {
            Self::Keyword(backend) => backend.reindex(chunks, embedder),
            Self::QdrantLocal(backend) => backend.reindex(chunks, embedder),
            Self::QdrantHttp(backend) => backend.reindex(chunks, embedder),
        }
    }

    fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn EmbeddingProvider,
    ) -> Vec<(String, usize)> {
        match self {
            Self::Keyword(backend) => backend.search(query, top_k, embedder),
            Self::QdrantLocal(backend) => backend.search(query, top_k, embedder),
            Self::QdrantHttp(backend) => backend.search(query, top_k, embedder),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchRequest {
    pub query: String,
    pub top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryChunk {
    pub id: String,
    pub source_path: String,
    pub heading: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchResult {
    pub chunk: MemoryChunk,
    pub score: usize,
}

#[derive(Debug)]
pub struct MemoryEngine {
    chunks: Vec<MemoryChunk>,
    by_id: HashMap<String, usize>,
    backend: VectorBackendKind,
    embedding_provider: EmbeddingProviderKind,
}

#[derive(Debug, Clone)]
pub enum EmbeddingProviderKind {
    Local(LocalEmbeddingProvider),
    Hosted(HostedEmbeddingProvider),
}

impl Default for EmbeddingProviderKind {
    fn default() -> Self {
        Self::Local(LocalEmbeddingProvider)
    }
}

impl EmbeddingProviderKind {
    fn as_provider(&self) -> &dyn EmbeddingProvider {
        match self {
            Self::Local(provider) => provider,
            Self::Hosted(provider) => provider,
        }
    }
}

impl Default for MemorySearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            top_k: 5,
        }
    }
}

impl MemoryEngine {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            backend: VectorBackendKind::Keyword(KeywordVectorBackend::default()),
            embedding_provider: EmbeddingProviderKind::default(),
        }
    }

    pub fn with_qdrant_local(
        state_dir: &Path,
        project_id: &str,
        hosted: bool,
    ) -> std::io::Result<Self> {
        let backend = QdrantLocalBackend::new(state_dir, project_id)?;
        let embedding_provider = if hosted {
            EmbeddingProviderKind::Hosted(HostedEmbeddingProvider)
        } else {
            EmbeddingProviderKind::Local(LocalEmbeddingProvider)
        };

        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            backend: VectorBackendKind::QdrantLocal(backend),
            embedding_provider,
        })
    }

    pub fn with_qdrant_http(
        base_url: &str,
        project_id: &str,
        hosted: bool,
        options: QdrantHttpOptions,
    ) -> std::io::Result<Self> {
        let backend = QdrantHttpBackend::new(base_url, project_id, &options)?;
        let embedding_provider = if hosted {
            EmbeddingProviderKind::Hosted(HostedEmbeddingProvider)
        } else {
            EmbeddingProviderKind::Local(LocalEmbeddingProvider)
        };

        Ok(Self {
            chunks: Vec::new(),
            by_id: HashMap::new(),
            backend: VectorBackendKind::QdrantHttp(backend),
            embedding_provider,
        })
    }

    pub fn ingest_markdown_tree(&mut self, docs_root: &Path) -> std::io::Result<usize> {
        if !docs_root.exists() {
            return Ok(0);
        }

        let docs_root_prefix = docs_root.display().to_string();
        let mut ingested = 0usize;
        let mut new_chunks = Vec::new();
        let mut stack = vec![docs_root.to_path_buf()];

        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(path)? {
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
                let source_path = path.display().to_string();
                let chunks = chunk_markdown(&source_path, &content);
                ingested += chunks.len();
                new_chunks.extend(chunks);
            }
        }

        self.chunks
            .retain(|chunk| !chunk.source_path.starts_with(&docs_root_prefix));
        self.chunks.extend(new_chunks);

        self.by_id.clear();
        for (index, chunk) in self.chunks.iter().enumerate() {
            self.by_id.insert(chunk.id.clone(), index);
        }
        self.backend
            .reindex(&self.chunks, self.embedding_provider.as_provider())?;

        Ok(ingested)
    }

    pub fn search(&self, request: &MemorySearchRequest) -> Vec<MemorySearchResult> {
        self.backend
            .search(
                &request.query,
                request.top_k,
                self.embedding_provider.as_provider(),
            )
            .into_iter()
            .filter_map(|(chunk_id, score)| {
                self.by_id
                    .get(&chunk_id)
                    .and_then(|index| self.chunks.get(*index))
                    .cloned()
                    .map(|chunk| MemorySearchResult { chunk, score })
            })
            .collect()
    }

    pub fn fetch_chunk(&self, id: &str) -> Option<MemoryChunk> {
        self.by_id
            .get(id)
            .and_then(|index| self.chunks.get(*index))
            .cloned()
    }

    pub fn docs_list(&self) -> Vec<String> {
        let mut paths = self
            .chunks
            .iter()
            .map(|chunk| chunk.source_path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn docs_get(&self, path: &str) -> Option<String> {
        let mut chunks = self
            .chunks
            .iter()
            .filter(|chunk| chunk.source_path == path)
            .collect::<Vec<_>>();
        if chunks.is_empty() {
            return None;
        }
        chunks.sort_by_key(|chunk| chunk.id.clone());
        let combined = chunks
            .into_iter()
            .map(|chunk| chunk.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        Some(combined)
    }

    pub fn docs_get_section(&self, path: &str, heading: &str, max_bytes: usize) -> Option<String> {
        let full = self.docs_get(path)?;
        let section = extract_markdown_section(&full, heading)?;
        if section.len() <= max_bytes {
            return Some(section);
        }
        Some(section[..max_bytes].to_string())
    }
}

impl Default for MemoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn io_error(err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

fn sanitize_collection_name(project_id: &str) -> String {
    project_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn hash_to_u64(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn embed_hash(text: &str, dims: usize, scale: f32) -> Vec<f32> {
    let mut vector = vec![0f32; dims];
    for (idx, byte) in text.bytes().enumerate() {
        let slot = idx % dims;
        vector[slot] += (byte as f32 / 255.0) * scale;
    }
    normalize(vector)
}

fn normalize(vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return vector;
    }
    vector.into_iter().map(|v| v / norm).collect()
}

fn similarity_score_percent(a: &[f32], b: &[f32]) -> usize {
    let dot = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>();
    let clamped = dot.clamp(0.0, 1.0);
    (clamped * 100.0).round() as usize
}

fn extract_markdown_section(content: &str, heading: &str) -> Option<String> {
    let heading_marker = format!("# {}", heading);
    let lines = content.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.trim() == heading_marker)?;

    let mut end = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        if line.starts_with('#') {
            end = idx;
            break;
        }
    }

    Some(lines[start..end].join("\n"))
}

fn chunk_markdown(source_path: &str, content: &str) -> Vec<MemoryChunk> {
    let mut chunks = Vec::new();
    let mut current_heading = None;
    let mut current_lines = Vec::new();
    let mut chunk_index = 0usize;

    for line in content.lines() {
        if line.starts_with('#') {
            if !current_lines.is_empty() {
                chunks.push(MemoryChunk {
                    id: format!("{source_path}:{chunk_index}"),
                    source_path: source_path.to_string(),
                    heading: current_heading.clone(),
                    content: current_lines.join("\n"),
                });
                chunk_index += 1;
                current_lines.clear();
            }

            current_heading = Some(line.trim_start_matches('#').trim().to_string());
        }

        current_lines.push(line.to_string());
    }

    if !current_lines.is_empty() {
        chunks.push(MemoryChunk {
            id: format!("{source_path}:{chunk_index}"),
            source_path: source_path.to_string(),
            heading: current_heading,
            content: current_lines.join("\n"),
        });
    }

    if chunks.is_empty() {
        chunks.push(MemoryChunk {
            id: format!("{source_path}:0"),
            source_path: source_path.to_string(),
            heading: None,
            content: content.to_string(),
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{MemoryEngine, MemorySearchRequest, QdrantHttpOptions};

    #[test]
    fn test_ingest_and_search_markdown() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let docs_dir = temp.path().join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir should be created");
        fs::write(
            docs_dir.join("readme.md"),
            "# Intro\nThis broker manages routing and memory search.",
        )
        .expect("file write should succeed");

        let mut engine = MemoryEngine::new();
        let ingested = engine
            .ingest_markdown_tree(&docs_dir)
            .expect("ingest should succeed");
        assert!(ingested >= 1);

        let results = engine.search(&MemorySearchRequest {
            query: "routing memory".to_string(),
            top_k: 5,
        });
        assert_eq!(results.len(), 1);
        assert!(results[0].score >= 2);
    }

    #[test]
    fn test_reingest_replaces_existing_chunks_for_same_root() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let docs_dir = temp.path().join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir should be created");
        let file = docs_dir.join("readme.md");
        fs::write(&file, "# Intro\none").expect("file write should succeed");

        let mut engine = MemoryEngine::new();
        engine
            .ingest_markdown_tree(&docs_dir)
            .expect("first ingest should succeed");

        fs::write(&file, "# Intro\none\n# Usage\ntwo").expect("file write should succeed");
        engine
            .ingest_markdown_tree(&docs_dir)
            .expect("second ingest should succeed");

        let listed = engine.docs_list();
        assert_eq!(listed.len(), 1);
        let usage = engine
            .docs_get_section(&listed[0], "Usage", 200)
            .expect("usage section should exist");
        assert!(usage.contains("two"));
    }

    #[test]
    fn test_docs_get_section_returns_bounded_output() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let docs_dir = temp.path().join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir should be created");
        let file = docs_dir.join("guide.md");
        fs::write(
            &file,
            "# Intro\nhello\n# Usage\nthis is a long section with details",
        )
        .expect("file write should succeed");

        let mut engine = MemoryEngine::new();
        engine
            .ingest_markdown_tree(&docs_dir)
            .expect("ingest should succeed");

        let section = engine
            .docs_get_section(&file.display().to_string(), "Usage", 10)
            .expect("section should be found");
        assert!(section.len() <= 10);
    }

    #[test]
    fn test_qdrant_local_backend_persists_index_file() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let state = temp.path().join(".the-one");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("docs dir should be created");
        fs::write(docs.join("readme.md"), "# Intro\nqdrant local index")
            .expect("doc write should succeed");

        let mut engine = MemoryEngine::with_qdrant_local(&state, "project-1", false)
            .expect("engine should initialize");
        let ingested = engine
            .ingest_markdown_tree(&docs)
            .expect("ingest should succeed");
        assert!(ingested >= 1);

        let index_path = state.join("qdrant").join("project-1.index.json");
        assert!(index_path.exists());
    }

    #[test]
    fn test_qdrant_http_backend_uses_api_key_header() {
        let server = httpmock::MockServer::start();
        let create = server.mock(|when, then| {
            when.method(httpmock::Method::PUT)
                .path("/collections/project_1")
                .header("api-key", "secret");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":true}"#);
        });
        let upsert = server.mock(|when, then| {
            when.method(httpmock::Method::PUT)
                .path("/collections/project_1/points")
                .query_param("wait", "true")
                .header("api-key", "secret");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":true}"#);
        });

        let temp = tempfile::tempdir().expect("tempdir should be created");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).expect("docs dir should be created");
        fs::write(docs.join("readme.md"), "# Intro\nqdrant over http")
            .expect("doc write should succeed");

        let mut engine = MemoryEngine::with_qdrant_http(
            &server.base_url(),
            "project/1",
            false,
            QdrantHttpOptions {
                api_key: Some("secret".to_string()),
                ca_cert_path: None,
                tls_insecure: true,
            },
        )
        .expect("http engine should initialize");
        engine
            .ingest_markdown_tree(&docs)
            .expect("ingest should succeed");

        create.assert_hits(2);
        upsert.assert_hits(1);
    }
}
