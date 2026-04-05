use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::json;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct QdrantOptions {
    pub api_key: Option<String>,
    pub ca_cert_path: Option<String>,
    pub tls_insecure: bool,
}

#[derive(Debug, Clone)]
pub struct QdrantPoint {
    /// chunk_id — will be hashed to a u64 point ID.
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: QdrantPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantPayload {
    pub chunk_id: String,
    pub source_path: String,
    pub heading: String,
    pub chunk_index: usize,
}

#[derive(Debug, Clone)]
pub struct QdrantSearchResult {
    pub chunk_id: String,
    pub source_path: String,
    pub heading: String,
    pub chunk_index: usize,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Hybrid (dense + sparse) types
// ---------------------------------------------------------------------------

/// A sparse vector in Qdrant's named sparse format (indices + values pairs).
/// Mirrors `sparse_embeddings::SparseVector` — copied here to avoid a
/// cross-module dep inside qdrant.rs (sparse_embeddings is local-embeddings gated).
#[derive(Debug, Clone, Default)]
pub struct QdrantSparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

/// A point that carries both a dense and a sparse vector for hybrid indexing.
#[derive(Debug, Clone)]
pub struct HybridPoint {
    /// chunk_id — hashed to a u64 point ID.
    pub id: String,
    /// Dense embedding vector (e.g., from BGE/MiniLM).
    pub dense: Vec<f32>,
    /// Sparse embedding vector (e.g., from SPLADE++).
    pub sparse: QdrantSparseVector,
    pub payload: QdrantPayload,
}

// ---------------------------------------------------------------------------
// Image types
// ---------------------------------------------------------------------------

/// A single image point to upsert into the image collection.
#[derive(Debug, Clone)]
pub struct ImagePoint {
    /// Unique identifier for this image (typically its SHA-256 hash).
    pub id: String,
    /// The embedding vector produced by the image embedding model.
    pub vector: Vec<f32>,
    /// Absolute path to the source image file.
    pub source_path: String,
    /// File size in bytes.
    pub file_size: u64,
    /// File modification time as Unix epoch seconds.
    pub mtime_epoch: i64,
    /// Optional user-provided or auto-generated caption.
    pub caption: Option<String>,
    /// Optional OCR-extracted text from the image.
    pub ocr_text: Option<String>,
    /// Optional path to the generated thumbnail file.
    pub thumbnail_path: Option<String>,
}

/// A single image search result returned from Qdrant.
#[derive(Debug, Clone)]
pub struct ImageSearchResult {
    pub id: String,
    pub source_path: String,
    pub score: f32,
    pub file_size: u64,
    pub mtime_epoch: i64,
    pub caption: Option<String>,
    pub ocr_text: Option<String>,
    pub thumbnail_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Graph RAG entity & relation vector types (v0.13.1)
//
// LightRAG's killer feature: embed entity descriptions + relation
// descriptions into their own vector stores so `Local` mode (entity-focused)
// and `Global` mode (relation-focused) can use semantic search instead of
// keyword matching. These types mirror the ImagePoint pattern above.
// ---------------------------------------------------------------------------

/// A single entity point to upsert into the entity vector collection.
#[derive(Debug, Clone)]
pub struct EntityPoint {
    /// Unique identifier — we use the lowercase canonical name hash so that
    /// re-extraction of the same entity overwrites rather than duplicates.
    pub id: String,
    /// The embedding vector of `"{name}\n{description}"`.
    pub vector: Vec<f32>,
    /// Canonical (normalized) entity name.
    pub name: String,
    /// Short type label ("person", "technology", etc).
    pub entity_type: String,
    /// Full description as stored in the knowledge graph.
    pub description: String,
    /// Chunk IDs this entity was extracted from.
    pub source_chunks: Vec<String>,
}

/// A single entity search hit returned from `search_entities`.
#[derive(Debug, Clone)]
pub struct EntitySearchResult {
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
    pub score: f32,
}

/// A single relation point for the relation vector collection.
#[derive(Debug, Clone)]
pub struct RelationPoint {
    /// Unique identifier — hash of `(normalized_source, normalized_target, relation_type)`.
    pub id: String,
    /// Embedding of `"{source} {relation_type} {target}\n{description}"`.
    pub vector: Vec<f32>,
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
}

/// A single relation search hit returned from `search_relations`.
#[derive(Debug, Clone)]
pub struct RelationSearchResult {
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub description: String,
    pub source_chunks: Vec<String>,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

pub struct AsyncQdrantBackend {
    client: reqwest::Client,
    base_url: String,
    collection_name: String,
}

impl AsyncQdrantBackend {
    /// Create a new backend. Does **not** create the collection yet.
    pub fn new(base_url: &str, project_id: &str, options: QdrantOptions) -> Result<Self, String> {
        let collection_name = sanitize_collection_name(project_id);

        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref key) = options.api_key {
            headers.insert(
                "api-key",
                reqwest::header::HeaderValue::from_str(key)
                    .map_err(|e| format!("invalid api-key header value: {e}"))?,
            );
        }

        let mut builder = reqwest::Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(options.tls_insecure);

        if let Some(ref ca_path) = options.ca_cert_path {
            let pem = std::fs::read(ca_path)
                .map_err(|e| format!("failed to read CA cert at {ca_path}: {e}"))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| format!("invalid PEM certificate: {e}"))?;
            builder = builder.add_root_certificate(cert);
        }

        let client = builder
            .build()
            .map_err(|e| format!("failed to build reqwest client: {e}"))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            collection_name,
        })
    }

    /// Ensure the collection exists with the right vector config.
    /// Creates it if missing; ignores "already exists" errors.
    pub async fn ensure_collection(&self, dims: usize) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.base_url, self.collection_name);
        let body = json!({
            "vectors": {
                "size": dims,
                "distance": "Cosine"
            },
            "hnsw_config": {
                "m": 16,
                "ef_construct": 100
            }
        });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("ensure_collection request failed: {e}"))?;

        let status = resp.status().as_u16();
        // 200/201 = created, 409 = already exists — both are fine.
        if status == 409 || (200..300).contains(&status) {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        // Qdrant may return 400 with "already exists" in the body.
        if text.contains("already exists") {
            return Ok(());
        }

        Err(format!("ensure_collection failed (HTTP {status}): {text}"))
    }

    /// Upsert points (vectors with payloads).
    pub async fn upsert_points(&self, points: Vec<QdrantPoint>) -> Result<(), String> {
        let url = format!(
            "{}/collections/{}/points",
            self.base_url, self.collection_name
        );

        let json_points: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                json!({
                    "id": hash_to_point_id(&p.id),
                    "vector": p.vector,
                    "payload": {
                        "chunk_id": p.payload.chunk_id,
                        "source_path": p.payload.source_path,
                        "heading": p.payload.heading,
                        "chunk_index": p.payload.chunk_index,
                    }
                })
            })
            .collect();

        let body = json!({ "points": json_points });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upsert_points request failed: {e}"))?;

        if resp.status().is_success() {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!("upsert_points failed: {text}"))
    }

    /// Search for similar vectors.
    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        score_threshold: f32,
    ) -> Result<Vec<QdrantSearchResult>, String> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection_name
        );

        let body = json!({
            "vector": query_vector,
            "limit": top_k,
            "score_threshold": score_threshold,
            "with_payload": true,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("search request failed: {e}"))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("search failed: {text}"));
        }

        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("search response parse error: {e}"))?;

        let results = value
            .get("result")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let payload = item.get("payload")?;
                        let chunk_id = payload.get("chunk_id")?.as_str()?.to_string();
                        let source_path = payload.get("source_path")?.as_str()?.to_string();
                        let heading = payload
                            .get("heading")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let chunk_index = payload
                            .get("chunk_index")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;
                        let score = item.get("score")?.as_f64()? as f32;

                        Some(QdrantSearchResult {
                            chunk_id,
                            source_path,
                            heading,
                            chunk_index,
                            score,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Delete all points matching a source_path.
    pub async fn delete_by_source_path(&self, source_path: &str) -> Result<(), String> {
        let url = format!(
            "{}/collections/{}/points/delete",
            self.base_url, self.collection_name
        );

        let body = json!({
            "filter": {
                "must": [{
                    "key": "source_path",
                    "match": { "value": source_path }
                }]
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("delete_by_source_path request failed: {e}"))?;

        if resp.status().is_success() {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!("delete_by_source_path failed: {text}"))
    }

    /// Check whether the collection exists (health check).
    pub async fn collection_exists(&self) -> Result<bool, String> {
        let url = format!("{}/collections/{}", self.base_url, self.collection_name);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("collection_exists request failed: {e}"))?;

        match resp.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            other => {
                let text = resp.text().await.unwrap_or_default();
                Err(format!(
                    "collection_exists unexpected status {other}: {text}"
                ))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Hybrid collection helpers (dense + sparse named vectors)
    // -----------------------------------------------------------------------

    /// Ensure a hybrid collection exists with both a named dense and sparse vector config.
    ///
    /// Qdrant collection config:
    /// - Named dense vector `"dense"` — cosine distance, HNSW indexed.
    /// - Named sparse vector `"sparse"` — IDF modifier for BM25-style weighting.
    ///
    /// Creates the collection if missing; ignores "already exists" errors.
    pub async fn ensure_hybrid_collection(&self, dense_dims: usize) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.base_url, self.collection_name);
        let body = json!({
            "vectors": {
                "dense": {
                    "size": dense_dims,
                    "distance": "Cosine"
                }
            },
            "sparse_vectors": {
                "sparse": {
                    "index": {
                        "on_disk": false
                    },
                    "modifier": "Idf"
                }
            },
            "hnsw_config": {
                "m": 16,
                "ef_construct": 100
            }
        });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("ensure_hybrid_collection request failed: {e}"))?;

        let status = resp.status().as_u16();
        if status == 409 || (200..300).contains(&status) {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        if text.contains("already exists") {
            return Ok(());
        }

        Err(format!(
            "ensure_hybrid_collection failed (HTTP {status}): {text}"
        ))
    }

    /// Upsert hybrid points (dense + sparse vectors with payloads).
    pub async fn upsert_hybrid_points(&self, points: Vec<HybridPoint>) -> Result<(), String> {
        let url = format!(
            "{}/collections/{}/points",
            self.base_url, self.collection_name
        );

        let json_points: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                json!({
                    "id": hash_to_point_id(&p.id),
                    "vector": {
                        "dense": p.dense,
                        "sparse": {
                            "indices": p.sparse.indices,
                            "values": p.sparse.values
                        }
                    },
                    "payload": {
                        "chunk_id": p.payload.chunk_id,
                        "source_path": p.payload.source_path,
                        "heading": p.payload.heading,
                        "chunk_index": p.payload.chunk_index,
                    }
                })
            })
            .collect();

        let body = json!({ "points": json_points });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upsert_hybrid_points request failed: {e}"))?;

        if resp.status().is_success() {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!("upsert_hybrid_points failed: {text}"))
    }

    /// Issue two parallel Qdrant queries — one dense cosine, one sparse — and
    /// return both result sets. The caller is responsible for score fusion.
    ///
    /// Named vector search uses the `"/points/search"` endpoint with `"using": "dense"`
    /// or `"using": "sparse"` to select the target vector space.
    pub async fn search_hybrid(
        &self,
        dense: Vec<f32>,
        sparse: QdrantSparseVector,
        top_k: usize,
        threshold: f32,
    ) -> Result<(Vec<QdrantSearchResult>, Vec<QdrantSearchResult>), String> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection_name
        );

        let dense_body = json!({
            "vector": {
                "name": "dense",
                "vector": dense
            },
            "limit": top_k,
            "score_threshold": threshold,
            "with_payload": true,
        });

        let sparse_body = json!({
            "vector": {
                "name": "sparse",
                "vector": {
                    "indices": sparse.indices,
                    "values": sparse.values
                }
            },
            "limit": top_k,
            "with_payload": true,
        });

        let (dense_resp, sparse_resp) = tokio::join!(
            self.client.post(&url).json(&dense_body).send(),
            self.client.post(&url).json(&sparse_body).send(),
        );

        let dense_results = Self::parse_search_response(
            dense_resp.map_err(|e| format!("dense search failed: {e}"))?,
        )
        .await?;
        let sparse_results = Self::parse_search_response(
            sparse_resp.map_err(|e| format!("sparse search failed: {e}"))?,
        )
        .await?;

        Ok((dense_results, sparse_results))
    }

    /// Parse a Qdrant `/points/search` HTTP response into `Vec<QdrantSearchResult>`.
    async fn parse_search_response(
        resp: reqwest::Response,
    ) -> Result<Vec<QdrantSearchResult>, String> {
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("search response error: {text}"));
        }

        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("search response parse error: {e}"))?;

        Ok(value
            .get("result")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let payload = item.get("payload")?;
                        let chunk_id = payload.get("chunk_id")?.as_str()?.to_string();
                        let source_path = payload.get("source_path")?.as_str()?.to_string();
                        let heading = payload
                            .get("heading")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let chunk_index = payload
                            .get("chunk_index")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;
                        let score = item.get("score")?.as_f64()? as f32;

                        Some(QdrantSearchResult {
                            chunk_id,
                            source_path,
                            heading,
                            chunk_index,
                            score,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    // -----------------------------------------------------------------------
    // Image collection helpers
    // -----------------------------------------------------------------------

    /// Build the image collection name for a given project ID.
    /// The collection name is `the_one_images_<sanitized_project_id>`.
    pub fn image_collection_name(project_id: &str) -> String {
        let sanitized: String = project_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("the_one_images_{sanitized}")
    }

    /// Ensure the image collection exists with the given vector dimensions.
    /// Creates it if missing; ignores "already exists" errors.
    pub async fn create_image_collection(
        &self,
        project_id: &str,
        dims: usize,
    ) -> Result<(), String> {
        let collection = Self::image_collection_name(project_id);
        let url = format!("{}/collections/{}", self.base_url, collection);
        let body = json!({
            "vectors": {
                "size": dims,
                "distance": "Cosine"
            },
            "hnsw_config": {
                "m": 16,
                "ef_construct": 100
            }
        });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("create_image_collection request failed: {e}"))?;

        let status = resp.status().as_u16();
        if status == 409 || (200..300).contains(&status) {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        if text.contains("already exists") {
            return Ok(());
        }

        Err(format!(
            "create_image_collection failed (HTTP {status}): {text}"
        ))
    }

    /// Upsert image points into the image collection for `project_id`.
    pub async fn upsert_image_points(
        &self,
        project_id: &str,
        points: Vec<ImagePoint>,
    ) -> Result<(), String> {
        let collection = Self::image_collection_name(project_id);
        let url = format!("{}/collections/{}/points", self.base_url, collection);

        let json_points: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                let mut payload = serde_json::json!({
                    "source_path": p.source_path,
                    "file_size": p.file_size,
                    "mtime_epoch": p.mtime_epoch,
                });
                if let Some(caption) = p.caption {
                    payload["caption"] = serde_json::Value::String(caption);
                }
                if let Some(ocr_text) = p.ocr_text {
                    payload["ocr_text"] = serde_json::Value::String(ocr_text);
                }
                if let Some(thumbnail_path) = p.thumbnail_path {
                    payload["thumbnail_path"] = serde_json::Value::String(thumbnail_path);
                }
                json!({
                    "id": hash_to_point_id(&p.id),
                    "vector": p.vector,
                    "payload": payload
                })
            })
            .collect();

        let body = json!({ "points": json_points });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upsert_image_points request failed: {e}"))?;

        if resp.status().is_success() {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!("upsert_image_points failed: {text}"))
    }

    /// Semantic search over the image collection for `project_id`.
    pub async fn search_images(
        &self,
        project_id: &str,
        query_vector: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<ImageSearchResult>, String> {
        let collection = Self::image_collection_name(project_id);
        let url = format!("{}/collections/{}/points/search", self.base_url, collection);

        let body = json!({
            "vector": query_vector,
            "limit": top_k,
            "score_threshold": threshold,
            "with_payload": true,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("search_images request failed: {e}"))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("search_images failed: {text}"));
        }

        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("search_images response parse error: {e}"))?;

        let results = value
            .get("result")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let payload = item.get("payload")?;
                        let source_path = payload.get("source_path")?.as_str()?.to_string();
                        let score = item.get("score")?.as_f64()? as f32;
                        let file_size = payload
                            .get("file_size")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let mtime_epoch = payload
                            .get("mtime_epoch")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let caption = payload
                            .get("caption")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let ocr_text = payload
                            .get("ocr_text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let thumbnail_path = payload
                            .get("thumbnail_path")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);

                        // Recover the original id from the payload source path (use
                        // source_path as surrogate id — callers can re-hash if needed)
                        let id = source_path.clone();

                        Some(ImageSearchResult {
                            id,
                            source_path,
                            score,
                            file_size,
                            mtime_epoch,
                            caption,
                            ocr_text,
                            thumbnail_path,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Delete the entire image collection for `project_id`.
    pub async fn delete_image_collection(&self, project_id: &str) -> Result<(), String> {
        let collection = Self::image_collection_name(project_id);
        let url = format!("{}/collections/{}", self.base_url, collection);

        let resp = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| format!("delete_image_collection request failed: {e}"))?;

        let status = resp.status().as_u16();
        // 200/404 are both fine (already deleted)
        if (200..300).contains(&status) || status == 404 {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!(
            "delete_image_collection failed (HTTP {status}): {text}"
        ))
    }

    /// Delete a single image point identified by its source path.
    pub async fn delete_image_by_source_path(
        &self,
        project_id: &str,
        source_path: &str,
    ) -> Result<(), String> {
        let collection = Self::image_collection_name(project_id);
        let url = format!("{}/collections/{}/points/delete", self.base_url, collection);

        let body = json!({
            "filter": {
                "must": [{
                    "key": "source_path",
                    "match": { "value": source_path }
                }]
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("delete_image_by_source_path request failed: {e}"))?;

        if resp.status().is_success() {
            return Ok(());
        }

        let text = resp.text().await.unwrap_or_default();
        Err(format!("delete_image_by_source_path failed: {text}"))
    }

    // -----------------------------------------------------------------------
    // Graph RAG — entity collection (v0.13.1)
    // -----------------------------------------------------------------------

    pub fn entity_collection_name(project_id: &str) -> String {
        let sanitized: String = project_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("the_one_entities_{sanitized}")
    }

    pub fn relation_collection_name(project_id: &str) -> String {
        let sanitized: String = project_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("the_one_relations_{sanitized}")
    }

    async fn create_graph_collection(&self, name: &str, dims: usize) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.base_url, name);
        let body = json!({
            "vectors": { "size": dims, "distance": "Cosine" },
            "hnsw_config": { "m": 16, "ef_construct": 100 }
        });
        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("create {name} request failed: {e}"))?;
        let status = resp.status().as_u16();
        if status == 409 || (200..300).contains(&status) {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        if text.contains("already exists") {
            return Ok(());
        }
        Err(format!("create {name} failed (HTTP {status}): {text}"))
    }

    pub async fn create_entity_collection(
        &self,
        project_id: &str,
        dims: usize,
    ) -> Result<(), String> {
        let name = Self::entity_collection_name(project_id);
        self.create_graph_collection(&name, dims).await
    }

    pub async fn create_relation_collection(
        &self,
        project_id: &str,
        dims: usize,
    ) -> Result<(), String> {
        let name = Self::relation_collection_name(project_id);
        self.create_graph_collection(&name, dims).await
    }

    pub async fn upsert_entity_points(
        &self,
        project_id: &str,
        points: Vec<EntityPoint>,
    ) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        let collection = Self::entity_collection_name(project_id);
        let url = format!("{}/collections/{}/points", self.base_url, collection);
        let json_points: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                json!({
                    "id": hash_to_point_id(&p.id),
                    "vector": p.vector,
                    "payload": {
                        "name": p.name,
                        "entity_type": p.entity_type,
                        "description": p.description,
                        "source_chunks": p.source_chunks,
                    }
                })
            })
            .collect();
        let body = json!({ "points": json_points });
        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upsert_entity_points request failed: {e}"))?;
        if resp.status().is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(format!("upsert_entity_points failed: {text}"))
    }

    pub async fn upsert_relation_points(
        &self,
        project_id: &str,
        points: Vec<RelationPoint>,
    ) -> Result<(), String> {
        if points.is_empty() {
            return Ok(());
        }
        let collection = Self::relation_collection_name(project_id);
        let url = format!("{}/collections/{}/points", self.base_url, collection);
        let json_points: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                json!({
                    "id": hash_to_point_id(&p.id),
                    "vector": p.vector,
                    "payload": {
                        "source": p.source,
                        "target": p.target,
                        "relation_type": p.relation_type,
                        "description": p.description,
                        "source_chunks": p.source_chunks,
                    }
                })
            })
            .collect();
        let body = json!({ "points": json_points });
        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upsert_relation_points request failed: {e}"))?;
        if resp.status().is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(format!("upsert_relation_points failed: {text}"))
    }

    pub async fn search_entities(
        &self,
        project_id: &str,
        query_vector: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<EntitySearchResult>, String> {
        let collection = Self::entity_collection_name(project_id);
        let url = format!("{}/collections/{}/points/search", self.base_url, collection);
        let body = json!({
            "vector": query_vector,
            "limit": top_k,
            "score_threshold": threshold,
            "with_payload": true,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("search_entities request failed: {e}"))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("search_entities failed: {text}"));
        }
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("search_entities parse error: {e}"))?;
        let results = value
            .get("result")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let payload = item.get("payload")?;
                        let score = item.get("score")?.as_f64()? as f32;
                        let name = payload.get("name")?.as_str()?.to_string();
                        let entity_type = payload
                            .get("entity_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let description = payload
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let source_chunks = payload
                            .get("source_chunks")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|c| c.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(EntitySearchResult {
                            name,
                            entity_type,
                            description,
                            source_chunks,
                            score,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(results)
    }

    pub async fn search_relations(
        &self,
        project_id: &str,
        query_vector: Vec<f32>,
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<RelationSearchResult>, String> {
        let collection = Self::relation_collection_name(project_id);
        let url = format!("{}/collections/{}/points/search", self.base_url, collection);
        let body = json!({
            "vector": query_vector,
            "limit": top_k,
            "score_threshold": threshold,
            "with_payload": true,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("search_relations request failed: {e}"))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("search_relations failed: {text}"));
        }
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("search_relations parse error: {e}"))?;
        let results = value
            .get("result")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let payload = item.get("payload")?;
                        let score = item.get("score")?.as_f64()? as f32;
                        let source = payload.get("source")?.as_str()?.to_string();
                        let target = payload.get("target")?.as_str()?.to_string();
                        let relation_type = payload
                            .get("relation_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let description = payload
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let source_chunks = payload
                            .get("source_chunks")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|c| c.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(RelationSearchResult {
                            source,
                            target,
                            relation_type,
                            description,
                            source_chunks,
                            score,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(results)
    }

    pub async fn delete_graph_collections(&self, project_id: &str) -> Result<(), String> {
        for collection in [
            Self::entity_collection_name(project_id),
            Self::relation_collection_name(project_id),
        ] {
            let url = format!("{}/collections/{}", self.base_url, collection);
            let _ = self.client.delete(&url).send().await;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sanitize_collection_name(project_id: &str) -> String {
    let sanitized: String = project_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("the_one_{sanitized}")
}

fn hash_to_point_id(chunk_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    chunk_id.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn test_sanitize_collection_name() {
        assert_eq!(sanitize_collection_name("my-project"), "the_one_my-project");
        assert_eq!(
            sanitize_collection_name("foo/bar baz"),
            "the_one_foo_bar_baz"
        );
        assert_eq!(
            sanitize_collection_name("hello_world"),
            "the_one_hello_world"
        );
        assert_eq!(sanitize_collection_name("a@b#c!d"), "the_one_a_b_c_d");
        assert_eq!(sanitize_collection_name(""), "the_one_");
    }

    #[tokio::test]
    async fn test_ensure_collection_sends_correct_request() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/collections/the_one_test-proj")
                .header("content-type", "application/json")
                .json_body_partial(
                    r#"{"vectors":{"size":384,"distance":"Cosine"},"hnsw_config":{"m":16,"ef_construct":100}}"#,
                );
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":true}"#);
        });

        let backend =
            AsyncQdrantBackend::new(&server.base_url(), "test-proj", QdrantOptions::default())
                .expect("backend should be created");

        backend
            .ensure_collection(384)
            .await
            .expect("ensure_collection should succeed");

        mock.assert();
    }

    #[tokio::test]
    async fn test_search_returns_parsed_results() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/collections/the_one_search-proj/points/search");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    serde_json::to_string(&json!({
                        "result": [
                            {
                                "id": 12345,
                                "score": 0.95,
                                "payload": {
                                    "chunk_id": "readme.md:0",
                                    "source_path": "docs/readme.md",
                                    "heading": "Introduction",
                                    "chunk_index": 0
                                }
                            },
                            {
                                "id": 67890,
                                "score": 0.82,
                                "payload": {
                                    "chunk_id": "guide.md:1",
                                    "source_path": "docs/guide.md",
                                    "heading": "Getting Started",
                                    "chunk_index": 1
                                }
                            }
                        ],
                        "status": "ok",
                        "time": 0.001
                    }))
                    .unwrap(),
                );
        });

        let backend =
            AsyncQdrantBackend::new(&server.base_url(), "search-proj", QdrantOptions::default())
                .expect("backend should be created");

        let results = backend
            .search(vec![0.1, 0.2, 0.3], 5, 0.5)
            .await
            .expect("search should succeed");

        mock.assert();

        assert_eq!(results.len(), 2);

        assert_eq!(results[0].chunk_id, "readme.md:0");
        assert_eq!(results[0].source_path, "docs/readme.md");
        assert_eq!(results[0].heading, "Introduction");
        assert_eq!(results[0].chunk_index, 0);
        assert!((results[0].score - 0.95).abs() < 0.001);

        assert_eq!(results[1].chunk_id, "guide.md:1");
        assert_eq!(results[1].source_path, "docs/guide.md");
        assert_eq!(results[1].heading, "Getting Started");
        assert_eq!(results[1].chunk_index, 1);
        assert!((results[1].score - 0.82).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_api_key_header_is_included() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/collections/the_one_auth-proj")
                .header("api-key", "my-secret-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":{}}"#);
        });

        let backend = AsyncQdrantBackend::new(
            &server.base_url(),
            "auth-proj",
            QdrantOptions {
                api_key: Some("my-secret-key".to_string()),
                ca_cert_path: None,
                tls_insecure: false,
            },
        )
        .expect("backend should be created");

        let exists = backend
            .collection_exists()
            .await
            .expect("collection_exists should succeed");

        mock.assert();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_ensure_hybrid_collection_sends_correct_request() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/collections/the_one_hybrid-proj")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"vectors":{"dense":{"size":384,"distance":"Cosine"}}}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":true}"#);
        });

        let backend =
            AsyncQdrantBackend::new(&server.base_url(), "hybrid-proj", QdrantOptions::default())
                .expect("backend should be created");

        backend
            .ensure_hybrid_collection(384)
            .await
            .expect("ensure_hybrid_collection should succeed");

        mock.assert();
    }

    #[tokio::test]
    async fn test_upsert_hybrid_points_sends_correct_request() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/collections/the_one_hybrid-upsert/points");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"status":"ok","result":{"operation_id":0,"status":"completed"}}"#);
        });

        let backend = AsyncQdrantBackend::new(
            &server.base_url(),
            "hybrid-upsert",
            QdrantOptions::default(),
        )
        .expect("backend should be created");

        let point = HybridPoint {
            id: "chunk:0".to_string(),
            dense: vec![0.1, 0.2, 0.3],
            sparse: QdrantSparseVector {
                indices: vec![10, 42, 100],
                values: vec![0.5, 0.8, 0.3],
            },
            payload: QdrantPayload {
                chunk_id: "chunk:0".to_string(),
                source_path: "docs/readme.md".to_string(),
                heading: "Intro".to_string(),
                chunk_index: 0,
            },
        };

        backend
            .upsert_hybrid_points(vec![point])
            .await
            .expect("upsert_hybrid_points should succeed");

        mock.assert();
    }

    #[tokio::test]
    async fn test_search_hybrid_issues_two_parallel_queries() {
        let server = MockServer::start();

        let search_response = serde_json::to_string(&json!({
            "result": [
                {
                    "id": 12345,
                    "score": 0.9,
                    "payload": {
                        "chunk_id": "doc.md:0",
                        "source_path": "docs/doc.md",
                        "heading": "Title",
                        "chunk_index": 0
                    }
                }
            ],
            "status": "ok",
            "time": 0.001
        }))
        .unwrap();

        // Both dense and sparse queries hit the same endpoint — use a call count mock
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/collections/the_one_hybrid-search/points/search");
            then.status(200)
                .header("content-type", "application/json")
                .body(search_response);
        });

        let backend = AsyncQdrantBackend::new(
            &server.base_url(),
            "hybrid-search",
            QdrantOptions::default(),
        )
        .expect("backend should be created");

        let sparse = QdrantSparseVector {
            indices: vec![5, 10],
            values: vec![0.7, 0.3],
        };

        let (dense_results, sparse_results) = backend
            .search_hybrid(vec![0.1, 0.2, 0.3], sparse, 5, 0.0)
            .await
            .expect("search_hybrid should succeed");

        // Two requests must have been issued (tokio::join!)
        mock.assert_hits(2);

        assert_eq!(dense_results.len(), 1);
        assert_eq!(sparse_results.len(), 1);
        assert_eq!(dense_results[0].chunk_id, "doc.md:0");
        assert!((dense_results[0].score - 0.9).abs() < 0.001);
    }
}
