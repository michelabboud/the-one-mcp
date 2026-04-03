use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;

use the_one_core::config::{NanoProviderEntry, NanoRoutingPolicy};

use crate::health::ProviderHealth;
use crate::providers::OpenAiCompatibleProvider;
use crate::RequestIntent;

pub struct PoolClassifyResult {
    pub intent: Option<RequestIntent>,
    pub provider_used: Option<String>,
    pub latency_ms: u64,
    pub fallback_to_rules: bool,
    pub attempts: u8,
    pub last_error: Option<String>,
}

pub struct ProviderPool {
    providers: Vec<(OpenAiCompatibleProvider, Mutex<ProviderHealth>)>,
    policy: NanoRoutingPolicy,
    round_robin_index: AtomicUsize,
}

impl std::fmt::Debug for ProviderPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderPool")
            .field("provider_count", &self.providers.len())
            .field("policy", &self.policy)
            .finish()
    }
}

impl ProviderPool {
    pub fn new(entries: Vec<NanoProviderEntry>, policy: NanoRoutingPolicy) -> Self {
        let providers = entries
            .into_iter()
            .filter(|e| e.enabled)
            .map(|e| {
                let provider = OpenAiCompatibleProvider::new(
                    &e.name,
                    &e.base_url,
                    &e.model,
                    e.api_key.as_deref(),
                    e.timeout_ms,
                );
                (provider, Mutex::new(ProviderHealth::new()))
            })
            .collect();
        Self {
            providers,
            policy,
            round_robin_index: AtomicUsize::new(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Classify a request using the provider pool with routing policy.
    pub async fn classify(&self, request: &str) -> PoolClassifyResult {
        if self.providers.is_empty() {
            return PoolClassifyResult {
                intent: None,
                provider_used: None,
                latency_ms: 0,
                fallback_to_rules: true,
                attempts: 0,
                last_error: Some("no providers configured".to_string()),
            };
        }

        let indices = self.select_provider_order().await;
        let mut attempts: u8 = 0;
        let mut last_error: Option<String> = None;

        for idx in indices {
            let (provider, health_mutex) = &self.providers[idx];
            attempts += 1;

            // TCP connect check
            if !provider.tcp_check().await {
                let mut health = health_mutex.lock().await;
                health.record_failure();
                last_error = Some(format!("provider {} TCP check failed", provider.name));
                continue;
            }

            // Classify
            let start = Instant::now();
            match provider.classify(request).await {
                Ok(intent) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let mut health = health_mutex.lock().await;
                    health.record_success(latency);
                    return PoolClassifyResult {
                        intent: Some(intent),
                        provider_used: Some(provider.name.clone()),
                        latency_ms: latency,
                        fallback_to_rules: false,
                        attempts,
                        last_error: None,
                    };
                }
                Err(e) => {
                    let mut health = health_mutex.lock().await;
                    health.record_failure();
                    last_error = Some(e);
                }
            }
        }

        // All providers failed
        PoolClassifyResult {
            intent: None,
            provider_used: None,
            latency_ms: 0,
            fallback_to_rules: true,
            attempts,
            last_error,
        }
    }

    async fn select_provider_order(&self) -> Vec<usize> {
        match self.policy {
            NanoRoutingPolicy::Priority => self.select_priority().await,
            NanoRoutingPolicy::RoundRobin => self.select_round_robin().await,
            NanoRoutingPolicy::Latency => self.select_latency().await,
        }
    }

    async fn select_priority(&self) -> Vec<usize> {
        let mut indices = Vec::new();
        for (i, (_, health_mutex)) in self.providers.iter().enumerate() {
            let health = health_mutex.lock().await;
            if health.is_available() {
                indices.push(i);
            }
        }
        indices
    }

    async fn select_round_robin(&self) -> Vec<usize> {
        let start = self.round_robin_index.fetch_add(1, Ordering::Relaxed) % self.providers.len();
        let mut indices = Vec::new();
        for offset in 0..self.providers.len() {
            let i = (start + offset) % self.providers.len();
            let health = self.providers[i].1.lock().await;
            if health.is_available() {
                indices.push(i);
            }
        }
        indices
    }

    async fn select_latency(&self) -> Vec<usize> {
        let mut with_latency: Vec<(usize, u64)> = Vec::new();
        for (i, (_, health_mutex)) in self.providers.iter().enumerate() {
            let health = health_mutex.lock().await;
            if health.is_available() {
                with_latency.push((i, health.p50_latency_ms()));
            }
        }
        with_latency.sort_by_key(|(_, lat)| *lat);
        with_latency.into_iter().map(|(i, _)| i).collect()
    }

    /// Get health snapshots for all providers (for metrics).
    pub async fn health_snapshots(&self) -> Vec<(String, ProviderHealth)> {
        let mut snapshots = Vec::new();
        for (provider, health_mutex) in &self.providers {
            let health = health_mutex.lock().await;
            snapshots.push((provider.name.clone(), health.clone()));
        }
        snapshots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use the_one_core::config::{NanoProviderEntry, NanoRoutingPolicy};

    fn make_entry(name: &str, base_url: &str, enabled: bool) -> NanoProviderEntry {
        NanoProviderEntry {
            name: name.to_string(),
            base_url: base_url.to_string(),
            model: "test-model".to_string(),
            api_key: None,
            timeout_ms: 2000,
            enabled,
        }
    }

    #[tokio::test]
    async fn test_empty_pool_returns_fallback() {
        let pool = ProviderPool::new(vec![], NanoRoutingPolicy::Priority);
        assert!(pool.is_empty());
        let result = pool.classify("search docs").await;
        assert!(result.fallback_to_rules);
        assert!(result.intent.is_none());
        assert_eq!(result.attempts, 0);
        assert!(result.last_error.is_some());
    }

    #[tokio::test]
    async fn test_disabled_entries_are_filtered() {
        let pool = ProviderPool::new(
            vec![make_entry("disabled", "http://127.0.0.1:1", false)],
            NanoRoutingPolicy::Priority,
        );
        assert!(pool.is_empty());
    }

    #[tokio::test]
    async fn test_pool_with_unreachable_providers_falls_back() {
        let pool = ProviderPool::new(
            vec![
                make_entry("bad1", "http://192.0.2.1:19999", true),
                make_entry("bad2", "http://192.0.2.2:19998", true),
            ],
            NanoRoutingPolicy::Priority,
        );
        assert!(!pool.is_empty());
        let result = pool.classify("search docs").await;
        assert!(result.fallback_to_rules);
        assert!(result.intent.is_none());
        assert!(result.attempts >= 1);
        assert!(result.last_error.is_some());
    }

    #[tokio::test]
    async fn test_pool_classify_with_mock_provider() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/chat/completions");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": "search_docs"
                        }
                    }]
                }));
        });

        let pool = ProviderPool::new(
            vec![NanoProviderEntry {
                name: "mock-provider".to_string(),
                base_url: server.base_url(),
                model: "test-model".to_string(),
                api_key: None,
                timeout_ms: 5000,
                enabled: true,
            }],
            NanoRoutingPolicy::Priority,
        );

        let result = pool.classify("find documentation about routing").await;
        assert!(!result.fallback_to_rules);
        assert_eq!(result.intent, Some(RequestIntent::SearchDocs));
        assert_eq!(result.provider_used, Some("mock-provider".to_string()));
        assert_eq!(result.attempts, 1);
        mock.assert();
    }

    #[tokio::test]
    async fn test_priority_selects_first_available() {
        let server1 = httpmock::MockServer::start();
        let mock1 = server1.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/chat/completions");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": "run_tool"
                        }
                    }]
                }));
        });

        let server2 = httpmock::MockServer::start();
        let _mock2 = server2.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/chat/completions");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": "search_docs"
                        }
                    }]
                }));
        });

        let pool = ProviderPool::new(
            vec![
                NanoProviderEntry {
                    name: "first-provider".to_string(),
                    base_url: server1.base_url(),
                    model: "test-model".to_string(),
                    api_key: None,
                    timeout_ms: 5000,
                    enabled: true,
                },
                NanoProviderEntry {
                    name: "second-provider".to_string(),
                    base_url: server2.base_url(),
                    model: "test-model".to_string(),
                    api_key: None,
                    timeout_ms: 5000,
                    enabled: true,
                },
            ],
            NanoRoutingPolicy::Priority,
        );

        let result = pool.classify("execute migration").await;
        assert!(!result.fallback_to_rules);
        assert_eq!(result.intent, Some(RequestIntent::RunTool));
        assert_eq!(result.provider_used, Some("first-provider".to_string()));
        assert_eq!(result.attempts, 1);
        mock1.assert();
    }
}
