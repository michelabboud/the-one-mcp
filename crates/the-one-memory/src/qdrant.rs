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

        let client = builder.build().map_err(|e| format!("failed to build reqwest client: {e}"))?;

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

        Err(format!(
            "ensure_collection failed (HTTP {status}): {text}"
        ))
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
                Err(format!("collection_exists unexpected status {other}: {text}"))
            }
        }
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

        let backend = AsyncQdrantBackend::new(
            &server.base_url(),
            "test-proj",
            QdrantOptions::default(),
        )
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

        let backend = AsyncQdrantBackend::new(
            &server.base_url(),
            "search-proj",
            QdrantOptions::default(),
        )
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
}
