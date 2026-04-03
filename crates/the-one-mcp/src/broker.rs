use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tracing::instrument;

use the_one_core::config::{AppConfig, NanoProviderKind, RuntimeOverrides};
use the_one_core::contracts::{ApprovalScope, Capability, RiskLevel};
use the_one_core::error::CoreError;
use the_one_core::manifests::{
    load_overrides_manifest, project_state_paths, save_overrides_manifest, MANIFEST_SCHEMA_VERSION,
};
use the_one_core::policy::PolicyEngine;
use the_one_core::project::{project_init, project_refresh, RefreshMode};
use the_one_core::storage::sqlite::ProjectDatabase;
use the_one_memory::{MemoryEngine, MemorySearchRequest as EngineSearchRequest, QdrantHttpOptions};
use the_one_registry::CapabilityRegistry;
use the_one_router::providers::{ApiNanoProvider, LmStudioNanoProvider, OllamaNanoProvider};
use the_one_router::{NanoBudget, RouteTelemetry, RoutedDecision, Router};

use crate::api::{
    AuditEventItem, AuditEventsRequest, AuditEventsResponse, ConfigExportResponse, DocsGetRequest,
    DocsGetResponse, DocsGetSectionRequest, DocsGetSectionResponse, DocsListRequest,
    DocsListResponse, MemoryFetchChunkRequest, MemoryFetchChunkResponse, MemorySearchItem,
    MemorySearchRequest, MemorySearchResponse, MetricsSnapshotResponse, ProjectInitRequest,
    ProjectInitResponse, ProjectProfileGetRequest, ProjectProfileGetResponse,
    ProjectRefreshRequest, ProjectRefreshResponse, ToolEnableRequest, ToolEnableResponse,
    ToolRunRequest, ToolRunResponse, ToolSearchRequest, ToolSearchResponse, ToolSuggestItem,
    ToolSuggestRequest, ToolSuggestResponse,
};

#[derive(Debug)]
pub struct McpBroker {
    router: Router,
    registry: CapabilityRegistry,
    memory_by_project: Mutex<HashMap<String, MemoryEngine>>,
    global_registry_path: Option<std::path::PathBuf>,
    policy: PolicyEngine,
    session_approvals: Mutex<std::collections::HashSet<String>>,
    metrics: BrokerMetrics,
}

#[derive(Debug, Default)]
struct BrokerMetrics {
    project_init_calls: AtomicU64,
    project_refresh_calls: AtomicU64,
    memory_search_calls: AtomicU64,
    tool_run_calls: AtomicU64,
    router_fallback_calls: AtomicU64,
    router_decision_latency_ms_total: AtomicU64,
    router_provider_error_calls: AtomicU64,
}

impl Default for McpBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl McpBroker {
    pub fn new() -> Self {
        Self::new_with_policy(PolicyEngine::default())
    }

    pub fn new_with_policy(policy: PolicyEngine) -> Self {
        let (registry, global_registry_path) = match CapabilityRegistry::default_catalog_path() {
            Ok(path) => {
                let registry = CapabilityRegistry::load_from_path(&path).unwrap_or_default();
                (registry, Some(path))
            }
            Err(_) => (CapabilityRegistry::new(), None),
        };

        Self {
            router: Router::new(true),
            registry,
            memory_by_project: Mutex::new(HashMap::new()),
            global_registry_path,
            policy,
            session_approvals: Mutex::new(std::collections::HashSet::new()),
            metrics: BrokerMetrics::default(),
        }
    }

    pub fn register_capability(&mut self, capability: Capability) {
        self.registry.add(capability);
        if let Some(path) = &self.global_registry_path {
            let _ = self.registry.save_to_path(path);
        }
    }

    fn project_memory_key(project_root: &Path, project_id: &str) -> String {
        let root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        format!("{}::{}", root.display(), project_id)
    }

    fn with_project_memory<R>(
        &self,
        project_root: &Path,
        project_id: &str,
        f: impl FnOnce(&MemoryEngine) -> R,
    ) -> Result<R, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        let memories = self
            .memory_by_project
            .lock()
            .map_err(|_| CoreError::PolicyDenied("project memory lock poisoned".to_string()))?;
        let memory = memories.get(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;
        Ok(f(memory))
    }

    fn build_memory_engine(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<MemoryEngine, CoreError> {
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        let state_dir = project_root.join(".the-one");
        let hosted_embeddings = config.provider == "hosted";

        if config.qdrant_url.starts_with("http://") || config.qdrant_url.starts_with("https://") {
            let remote = !(config.qdrant_url.contains("localhost")
                || config.qdrant_url.starts_with("http://127.0.0.1"));
            if remote && config.qdrant_strict_auth && config.qdrant_api_key.is_none() {
                return Err(CoreError::InvalidProjectConfig(
                    "remote qdrant requires api key when strict auth is enabled".to_string(),
                ));
            }
            if let Ok(engine) = MemoryEngine::with_qdrant_http(
                &config.qdrant_url,
                project_id,
                hosted_embeddings,
                QdrantHttpOptions {
                    api_key: config.qdrant_api_key.clone(),
                    ca_cert_path: config.qdrant_ca_cert_path.clone(),
                    tls_insecure: config.qdrant_tls_insecure,
                },
            ) {
                return Ok(engine);
            }
        }

        if config.qdrant_url.starts_with("local://")
            || config.qdrant_url.contains("localhost")
            || config.qdrant_url.starts_with("http://127.0.0.1")
        {
            if let Ok(engine) =
                MemoryEngine::with_qdrant_local(&state_dir, project_id, hosted_embeddings)
            {
                return Ok(engine);
            }
        }

        Ok(MemoryEngine::new())
    }

    fn route_query(&self, project_root: &Path, query: &str) -> RoutedDecision {
        let config = match AppConfig::load(project_root, RuntimeOverrides::default()) {
            Ok(config) => config,
            Err(_) => {
                return RoutedDecision {
                    decision: self.router.route_rules_only(query),
                    telemetry: RouteTelemetry {
                        provider_path: "rules-config-fallback".to_string(),
                        confidence_percent: 100,
                        latency_ms: 0,
                        used_fallback: true,
                        attempts: 0,
                        timeout_ms_bound: 0,
                        retries_bound: 0,
                        last_error: Some("config-load-failed".to_string()),
                    },
                }
            }
        };

        let budget = NanoBudget {
            timeout_ms: 300,
            retries: 1,
        };
        match config.nano_provider {
            NanoProviderKind::RulesOnly => RoutedDecision {
                decision: self.router.route_rules_only(query),
                telemetry: RouteTelemetry {
                    provider_path: "rules-only".to_string(),
                    confidence_percent: 100,
                    latency_ms: 0,
                    used_fallback: false,
                    attempts: 0,
                    timeout_ms_bound: 0,
                    retries_bound: 0,
                    last_error: None,
                },
            },
            NanoProviderKind::Api => {
                let provider = ApiNanoProvider::new(&config.nano_model);
                self.router
                    .route_with_provider_budget(query, Some(&provider), budget)
            }
            NanoProviderKind::Ollama => {
                let provider = OllamaNanoProvider;
                self.router
                    .route_with_provider_budget(query, Some(&provider), budget)
            }
            NanoProviderKind::LmStudio => {
                let provider = LmStudioNanoProvider;
                self.router
                    .route_with_provider_budget(query, Some(&provider), budget)
            }
        }
    }

    fn route_tool_action(&self, project_root: &Path, action_key: &str) -> RoutedDecision {
        let request = format!("run {action_key}");
        let config = match AppConfig::load(project_root, RuntimeOverrides::default()) {
            Ok(config) => config,
            Err(_) => {
                return RoutedDecision {
                    decision: self.router.route_rules_only(&request),
                    telemetry: RouteTelemetry {
                        provider_path: "rules-config-fallback".to_string(),
                        confidence_percent: 100,
                        latency_ms: 0,
                        used_fallback: true,
                        attempts: 0,
                        timeout_ms_bound: 0,
                        retries_bound: 0,
                        last_error: Some("config-load-failed".to_string()),
                    },
                }
            }
        };

        let budget = NanoBudget {
            timeout_ms: 300,
            retries: 1,
        };
        match config.nano_provider {
            NanoProviderKind::RulesOnly => RoutedDecision {
                decision: self.router.route_rules_only(&request),
                telemetry: RouteTelemetry {
                    provider_path: "rules-only".to_string(),
                    confidence_percent: 100,
                    latency_ms: 0,
                    used_fallback: false,
                    attempts: 0,
                    timeout_ms_bound: 0,
                    retries_bound: 0,
                    last_error: None,
                },
            },
            NanoProviderKind::Api => {
                let provider = ApiNanoProvider::new(&config.nano_model);
                self.router
                    .route_with_provider_budget(&request, Some(&provider), budget)
            }
            NanoProviderKind::Ollama => {
                let provider = OllamaNanoProvider;
                self.router
                    .route_with_provider_budget(&request, Some(&provider), budget)
            }
            NanoProviderKind::LmStudio => {
                let provider = LmStudioNanoProvider;
                self.router
                    .route_with_provider_budget(&request, Some(&provider), budget)
            }
        }
    }

    pub fn ingest_docs(
        &self,
        project_root: &Path,
        project_id: &str,
        docs_root: &Path,
    ) -> Result<usize, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        let mut memories = self
            .memory_by_project
            .lock()
            .map_err(|_| CoreError::PolicyDenied("project memory lock poisoned".to_string()))?;
        if !memories.contains_key(&key) {
            let engine = self.build_memory_engine(project_root, project_id)?;
            memories.insert(key.clone(), engine);
        }
        let memory = memories.get_mut(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;
        Ok(memory.ingest_markdown_tree(docs_root)?)
    }

    #[instrument(skip_all)]
    pub fn project_init(
        &self,
        request: ProjectInitRequest,
    ) -> Result<ProjectInitResponse, CoreError> {
        self.metrics
            .project_init_calls
            .fetch_add(1, Ordering::Relaxed);
        let result = project_init(Path::new(&request.project_root), &request.project_id)?;
        Ok(ProjectInitResponse {
            project_id: result.project_id,
            profile_version: crate::schema_version().to_string(),
            fingerprint: result.fingerprint,
        })
    }

    #[instrument(skip_all)]
    pub fn project_refresh(
        &self,
        request: ProjectRefreshRequest,
    ) -> Result<ProjectRefreshResponse, CoreError> {
        self.metrics
            .project_refresh_calls
            .fetch_add(1, Ordering::Relaxed);
        let result = project_refresh(Path::new(&request.project_root), &request.project_id)?;
        let mode = match result.mode {
            RefreshMode::ReusedCachedProfile => "cached",
            RefreshMode::RecomputedProfile => "recomputed",
        };
        Ok(ProjectRefreshResponse {
            project_id: result.project_id,
            mode: mode.to_string(),
            fingerprint: result.fingerprint,
        })
    }

    #[instrument(skip_all)]
    pub fn memory_search(&self, request: MemorySearchRequest) -> MemorySearchResponse {
        self.metrics
            .memory_search_calls
            .fetch_add(1, Ordering::Relaxed);
        let route = self.route_query(Path::new(&request.project_root), &request.query);
        self.metrics
            .router_decision_latency_ms_total
            .fetch_add(route.telemetry.latency_ms, Ordering::Relaxed);
        if route.telemetry.used_fallback {
            self.metrics
                .router_fallback_calls
                .fetch_add(1, Ordering::Relaxed);
        }
        if route.telemetry.last_error.is_some() {
            self.metrics
                .router_provider_error_calls
                .fetch_add(1, Ordering::Relaxed);
        }
        let top_k = self.policy.clamp_search_hits(request.top_k);

        let project_root = Path::new(&request.project_root);
        let project_id = request.project_id.clone();
        let query = request.query;

        let hits = if route.decision.requires_memory_search {
            self.with_project_memory(project_root, &project_id, |memory| {
                memory
                    .search(&EngineSearchRequest { query, top_k })
                    .into_iter()
                    .map(|item| MemorySearchItem {
                        id: item.chunk.id,
                        source_path: item.chunk.source_path,
                        score: item.score,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        } else {
            Vec::new()
        };

        MemorySearchResponse {
            hits,
            route: format!("{:?}", route.decision.route),
            rationale: route.decision.rationale,
            provider_path: route.telemetry.provider_path,
            confidence_percent: route.telemetry.confidence_percent,
            fallback_used: route.telemetry.used_fallback,
            timeout_ms_bound: route.telemetry.timeout_ms_bound,
            retries_bound: route.telemetry.retries_bound,
            last_error: route.telemetry.last_error,
        }
    }

    #[instrument(skip_all)]
    pub fn memory_fetch_chunk(
        &self,
        request: MemoryFetchChunkRequest,
    ) -> Option<MemoryFetchChunkResponse> {
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.fetch_chunk(&request.id),
        )
        .ok()
        .flatten()
        .map(|chunk| MemoryFetchChunkResponse {
            id: chunk.id,
            source_path: chunk.source_path,
            content: chunk.content,
        })
    }

    #[instrument(skip_all)]
    pub fn docs_list(&self, request: DocsListRequest) -> DocsListResponse {
        let docs = self
            .with_project_memory(
                Path::new(&request.project_root),
                &request.project_id,
                MemoryEngine::docs_list,
            )
            .unwrap_or_default();
        DocsListResponse { docs }
    }

    #[instrument(skip_all)]
    pub fn docs_get(&self, request: DocsGetRequest) -> Option<DocsGetResponse> {
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.docs_get(&request.path),
        )
        .ok()
        .flatten()
        .map(|content| DocsGetResponse {
            path: request.path,
            content,
        })
    }

    #[instrument(skip_all)]
    pub fn docs_get_section(
        &self,
        request: DocsGetSectionRequest,
    ) -> Option<DocsGetSectionResponse> {
        let max_bytes = self.policy.clamp_doc_bytes(request.max_bytes);
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.docs_get_section(&request.path, &request.heading, max_bytes),
        )
        .ok()
        .flatten()
        .map(|content| DocsGetSectionResponse {
            path: request.path,
            heading: request.heading,
            content,
        })
    }

    #[instrument(skip_all)]
    pub fn tool_suggest(&self, request: ToolSuggestRequest) -> ToolSuggestResponse {
        let suggestions = self
            .registry
            .suggest(
                &request.query,
                RiskLevel::Medium,
                self.policy.clamp_suggestions(request.max),
            )
            .into_iter()
            .map(|s| ToolSuggestItem {
                id: s.id,
                title: s.title,
                reason: s.reason,
            })
            .collect();

        ToolSuggestResponse { suggestions }
    }

    #[instrument(skip_all)]
    pub fn tool_search(&self, request: ToolSearchRequest) -> ToolSearchResponse {
        let response = self.tool_suggest(ToolSuggestRequest {
            query: request.query,
            max: request.max,
        });
        ToolSearchResponse {
            matches: response.suggestions,
        }
    }

    #[instrument(skip_all)]
    pub fn tool_run(
        &self,
        project_root: &Path,
        project_id: &str,
        request: ToolRunRequest,
    ) -> Result<ToolRunResponse, CoreError> {
        self.metrics.tool_run_calls.fetch_add(1, Ordering::Relaxed);
        let routed = self.route_tool_action(project_root, &request.action_key);
        self.metrics
            .router_decision_latency_ms_total
            .fetch_add(routed.telemetry.latency_ms, Ordering::Relaxed);
        if routed.telemetry.used_fallback {
            self.metrics
                .router_fallback_calls
                .fetch_add(1, Ordering::Relaxed);
        }
        if routed.telemetry.last_error.is_some() {
            self.metrics
                .router_provider_error_calls
                .fetch_add(1, Ordering::Relaxed);
        }

        if !routed.decision.requires_approval {
            return Ok(ToolRunResponse {
                allowed: true,
                reason: "no approval required".to_string(),
            });
        }

        let db = ProjectDatabase::open(project_root, project_id)?;

        if !request.interactive {
            if self
                .session_approvals
                .lock()
                .map_err(|_| CoreError::PolicyDenied("session lock poisoned".to_string()))?
                .contains(&request.action_key)
            {
                db.record_audit_event(
                    "tool_run",
                    "{\"mode\":\"headless\",\"result\":\"approved_session\"}",
                )?;
                return Ok(ToolRunResponse {
                    allowed: true,
                    reason: "approved by session policy".to_string(),
                });
            }
            let approved = db.is_approved(&request.action_key, ApprovalScope::Forever)?;
            if approved {
                db.record_audit_event(
                    "tool_run",
                    "{\"mode\":\"headless\",\"result\":\"approved\"}",
                )?;
                return Ok(ToolRunResponse {
                    allowed: true,
                    reason: "approved by persisted policy".to_string(),
                });
            }
            db.record_audit_event("tool_run", "{\"mode\":\"headless\",\"result\":\"denied\"}")?;
            return Ok(ToolRunResponse {
                allowed: false,
                reason: "headless mode denies unapproved high-risk action".to_string(),
            });
        }

        let scope = match request.approval_scope.as_deref() {
            Some("once") => ApprovalScope::Once,
            Some("session") => ApprovalScope::Session,
            Some("forever") => ApprovalScope::Forever,
            _ => ApprovalScope::Once,
        };
        match scope {
            ApprovalScope::Once => {}
            ApprovalScope::Session => {
                self.session_approvals
                    .lock()
                    .map_err(|_| CoreError::PolicyDenied("session lock poisoned".to_string()))?
                    .insert(request.action_key.clone());
            }
            ApprovalScope::Forever => {
                db.set_approval(&request.action_key, ApprovalScope::Forever, true)?;
            }
        }
        db.record_audit_event(
            "tool_run",
            "{\"mode\":\"interactive\",\"result\":\"approved\"}",
        )?;

        Ok(ToolRunResponse {
            allowed: true,
            reason: "approved interactively".to_string(),
        })
    }

    #[instrument(skip_all)]
    pub fn config_export(&self, project_root: &Path) -> Result<ConfigExportResponse, CoreError> {
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        Ok(ConfigExportResponse {
            schema_version: crate::schema_version().to_string(),
            provider: config.provider,
            log_level: config.log_level,
            qdrant_url: config.qdrant_url,
            qdrant_auth_configured: config.qdrant_api_key.is_some(),
            qdrant_ca_cert_path: config
                .qdrant_ca_cert_path
                .as_ref()
                .map(|path| path.display().to_string()),
            qdrant_tls_insecure: config.qdrant_tls_insecure,
            qdrant_strict_auth: config.qdrant_strict_auth,
            nano_provider: config.nano_provider.as_str().to_string(),
            nano_model: config.nano_model,
        })
    }

    #[instrument(skip_all)]
    pub fn project_profile_get(
        &self,
        request: ProjectProfileGetRequest,
    ) -> Result<Option<ProjectProfileGetResponse>, CoreError> {
        let db = ProjectDatabase::open(Path::new(&request.project_root), &request.project_id)?;
        let profile_json = db.latest_project_profile()?;
        Ok(profile_json.map(|profile_json| ProjectProfileGetResponse {
            project_id: request.project_id,
            profile_json,
        }))
    }

    #[instrument(skip_all)]
    pub fn tool_enable(&self, request: ToolEnableRequest) -> Result<ToolEnableResponse, CoreError> {
        let paths = project_state_paths(Path::new(&request.project_root));
        let mut overrides = load_overrides_manifest(&paths.overrides_json)?;
        if !overrides.enabled_families.contains(&request.family) {
            overrides.enabled_families.push(request.family);
            overrides.enabled_families.sort();
            overrides.enabled_families.dedup();
        }
        self.policy
            .validate_enabled_families_count(overrides.enabled_families.len())?;
        overrides.schema_version = MANIFEST_SCHEMA_VERSION.to_string();
        save_overrides_manifest(&paths.overrides_json, &overrides)?;

        Ok(ToolEnableResponse {
            enabled_families: overrides.enabled_families,
        })
    }

    pub fn metrics_snapshot(&self) -> MetricsSnapshotResponse {
        MetricsSnapshotResponse {
            project_init_calls: self.metrics.project_init_calls.load(Ordering::Relaxed),
            project_refresh_calls: self.metrics.project_refresh_calls.load(Ordering::Relaxed),
            memory_search_calls: self.metrics.memory_search_calls.load(Ordering::Relaxed),
            tool_run_calls: self.metrics.tool_run_calls.load(Ordering::Relaxed),
            router_fallback_calls: self.metrics.router_fallback_calls.load(Ordering::Relaxed),
            router_decision_latency_ms_total: self
                .metrics
                .router_decision_latency_ms_total
                .load(Ordering::Relaxed),
            router_provider_error_calls: self
                .metrics
                .router_provider_error_calls
                .load(Ordering::Relaxed),
        }
    }

    #[instrument(skip_all)]
    pub fn audit_events(
        &self,
        request: AuditEventsRequest,
    ) -> Result<AuditEventsResponse, CoreError> {
        let db = ProjectDatabase::open(Path::new(&request.project_root), &request.project_id)?;
        let events = db.list_audit_events(request.limit).map(|items| {
            items
                .into_iter()
                .map(|item| AuditEventItem {
                    id: item.id,
                    project_id: item.project_id,
                    event_type: item.event_type,
                    payload_json: item.payload_json,
                    created_at_epoch_ms: item.created_at_epoch_ms,
                })
                .collect::<Vec<_>>()
        })?;

        Ok(AuditEventsResponse { events })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use the_one_core::contracts::{Capability, CapabilityType, RiskLevel, VisibilityMode};
    use the_one_core::limits::ConfigurableLimits;
    use the_one_core::policy::PolicyEngine;

    use super::McpBroker;
    use crate::api::{
        AuditEventsRequest, DocsGetSectionRequest, DocsListRequest, MemorySearchRequest,
        ProjectInitRequest, ProjectProfileGetRequest, ProjectRefreshRequest, ToolEnableRequest,
        ToolRunRequest, ToolSuggestRequest,
    };

    #[test]
    fn test_project_init_and_refresh_flow() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");
        fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let broker = McpBroker::new();
        let init = broker
            .project_init(ProjectInitRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("init should succeed");
        assert_eq!(init.project_id, "project-1");

        let refresh = broker
            .project_refresh(ProjectRefreshRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("refresh should succeed");
        assert_eq!(refresh.mode, "cached");
    }

    #[test]
    fn test_memory_search_and_tool_suggest() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let global = temp.path().join("global-state");
        fs::create_dir_all(&global).expect("global dir should exist");
        std::env::set_var("THE_ONE_HOME", global.display().to_string());
        fs::create_dir_all(&root).expect("project root should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nsearch and retrieval")
            .expect("doc write should succeed");

        let mut broker = McpBroker::new();
        broker.register_capability(Capability {
            id: "docs.search".to_string(),
            title: "Docs Search".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "docs".to_string(),
            visibility_mode: VisibilityMode::Core,
            risk_level: RiskLevel::Low,
            description: "search docs".to_string(),
        });
        broker
            .ingest_docs(&root, "project-1", &docs)
            .expect("ingest should succeed");

        let search = broker.memory_search(MemorySearchRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
            query: "search docs".to_string(),
            top_k: 5,
        });
        assert_eq!(search.hits.len(), 1);
        assert!(!search.provider_path.is_empty());

        let docs = broker.docs_list(DocsListRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
        });
        assert_eq!(docs.docs.len(), 1);
        let section = broker.docs_get_section(DocsGetSectionRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
            path: docs.docs[0].clone(),
            heading: "Intro".to_string(),
            max_bytes: 64,
        });
        assert!(section.is_some());

        let suggest = broker.tool_suggest(ToolSuggestRequest {
            query: "docs".to_string(),
            max: 5,
        });
        assert!(suggest
            .suggestions
            .iter()
            .any(|item| item.id == "docs.search"));

        std::env::remove_var("THE_ONE_HOME");
    }

    #[test]
    fn test_memory_search_uses_nano_provider_and_reports_telemetry() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nsearch and retrieval")
            .expect("doc write should succeed");
        fs::write(
            state_dir.join("config.json"),
            r#"{"nano_provider":"api","nano_model":"gpt-nano"}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .ingest_docs(&root, "project-1", &docs)
            .expect("ingest should succeed");

        let search = broker.memory_search(MemorySearchRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
            query: "search docs".to_string(),
            top_k: 5,
        });

        assert_eq!(search.route, "RuleWithNano");
        assert_eq!(search.provider_path, "api-nano");
        assert!(!search.fallback_used);
    }

    #[test]
    fn test_ingest_docs_creates_qdrant_local_index_file() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nqdrant test").expect("doc write should work");
        fs::write(
            state_dir.join("config.json"),
            r#"{"provider":"local","qdrant_url":"http://127.0.0.1:6334"}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .ingest_docs(&root, "project-1", &docs)
            .expect("ingest should succeed");

        let index = state_dir.join("qdrant").join("project-1.index.json");
        assert!(index.exists());
    }

    #[test]
    fn test_remote_qdrant_strict_auth_rejects_missing_api_key() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nqdrant strict auth").expect("doc write should work");
        fs::write(
            state_dir.join("config.json"),
            r#"{"qdrant_url":"https://qdrant.example.com:6333","qdrant_strict_auth":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        let err = broker
            .ingest_docs(&root, "project-1", &docs)
            .expect_err("ingest should fail without api key");
        assert!(err.to_string().contains("remote qdrant requires api key"));
    }

    #[test]
    fn test_memory_search_reports_fallback_when_nano_fails() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nsearch and retrieval")
            .expect("doc write should succeed");
        fs::write(
            state_dir.join("config.json"),
            r#"{"nano_provider":"api","nano_model":"gpt-nano"}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .ingest_docs(&root, "project-1", &docs)
            .expect("ingest should succeed");

        let search = broker.memory_search(MemorySearchRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
            query: "search docs nano-fail".to_string(),
            top_k: 5,
        });
        assert!(search.fallback_used);
        assert!(search.last_error.is_some());
        assert!(search.retries_bound <= 3);

        let metrics = broker.metrics_snapshot();
        assert!(metrics.router_fallback_calls >= 1);
    }

    #[test]
    fn test_tool_run_headless_requires_prior_approval() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let denied = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .expect("tool run should complete");
        assert!(!denied.allowed);

        let approved = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: true,
                    approval_scope: Some("forever".to_string()),
                },
            )
            .expect("tool run should complete");
        assert!(approved.allowed);

        let headless_after = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .expect("tool run should complete");
        assert!(headless_after.allowed);
    }

    #[test]
    fn test_config_export_returns_defaults() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let config = broker
            .config_export(&root)
            .expect("config export should work");
        assert_eq!(config.schema_version, "v1beta");
        assert_eq!(config.provider, "local");
        assert!(!config.qdrant_auth_configured);
        assert!(!config.qdrant_tls_insecure);
        assert!(config.qdrant_strict_auth);
        assert_eq!(config.nano_provider, "rules");
        assert_eq!(config.nano_model, "none");
    }

    #[test]
    fn test_tool_route_uses_project_nano_provider_config() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"nano_provider":"ollama","nano_model":"tiny"}"#,
        )
        .expect("project config should write");

        let broker = McpBroker::new();
        let decision = broker.route_tool_action(&root, "tool.run:danger");

        assert!(decision.decision.rationale.contains("ollama"));
    }

    #[test]
    fn test_session_approval_allows_headless_in_same_broker_session() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let interactive = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: true,
                    approval_scope: Some("session".to_string()),
                },
            )
            .expect("interactive approval should work");
        assert!(interactive.allowed);

        let headless = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .expect("headless run should work");
        assert!(headless.allowed);
    }

    #[test]
    fn test_tool_enable_and_profile_get_and_metrics() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");
        fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let broker = McpBroker::new();
        broker
            .project_init(ProjectInitRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("init should succeed");

        let enabled = broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "docs".to_string(),
            })
            .expect("tool enable should succeed");
        assert!(enabled.enabled_families.contains(&"docs".to_string()));

        let profile = broker
            .project_profile_get(ProjectProfileGetRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("profile get should succeed");
        assert!(profile.is_some());

        let metrics = broker.metrics_snapshot();
        assert!(metrics.project_init_calls >= 1);
    }

    #[test]
    fn test_policy_limits_bound_suggestions_and_search_hits() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        fs::create_dir_all(&root).expect("project root should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nsearch docs now").expect("doc write should succeed");

        let policy = PolicyEngine::new(ConfigurableLimits {
            max_tool_suggestions: 1,
            max_search_hits: 1,
            max_raw_section_bytes: 1024,
            max_enabled_families: 2,
            ..ConfigurableLimits::default()
        });
        let mut broker = McpBroker::new_with_policy(policy);
        broker.register_capability(Capability {
            id: "a".to_string(),
            title: "A".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "docs".to_string(),
            visibility_mode: VisibilityMode::Core,
            risk_level: RiskLevel::Low,
            description: "docs".to_string(),
        });
        broker.register_capability(Capability {
            id: "b".to_string(),
            title: "B".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "docs".to_string(),
            visibility_mode: VisibilityMode::Core,
            risk_level: RiskLevel::Low,
            description: "docs".to_string(),
        });
        broker
            .ingest_docs(&root, "project-1", &docs)
            .expect("ingest should work");

        let suggestions = broker.tool_suggest(ToolSuggestRequest {
            query: "docs".to_string(),
            max: 10,
        });
        assert_eq!(suggestions.suggestions.len(), 1);

        let hits = broker.memory_search(MemorySearchRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
            query: "search".to_string(),
            top_k: 10,
        });
        assert_eq!(hits.hits.len(), 1);

        let docs_list = broker.docs_list(DocsListRequest {
            project_root: root.display().to_string(),
            project_id: "project-1".to_string(),
        });

        let section = broker
            .docs_get_section(DocsGetSectionRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: docs_list.docs[0].clone(),
                heading: "Intro".to_string(),
                max_bytes: 100,
            })
            .expect("section should exist");
        assert!(section.content.len() <= 1024);
    }

    #[test]
    fn test_tool_enable_respects_max_enabled_families() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");
        fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let policy = PolicyEngine::new(ConfigurableLimits {
            max_tool_suggestions: 5,
            max_search_hits: 5,
            max_raw_section_bytes: 1024,
            max_enabled_families: 2,
            ..ConfigurableLimits::default()
        });
        let broker = McpBroker::new_with_policy(policy);
        broker
            .project_init(ProjectInitRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("init should succeed");

        broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "a".to_string(),
            })
            .expect("enable a");
        broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "b".to_string(),
            })
            .expect("enable b");

        let err = broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "c".to_string(),
            })
            .expect_err("enable c should fail");
        assert!(err
            .to_string()
            .contains("enabled families exceed policy limit"));
    }

    #[test]
    fn test_audit_events_endpoint_returns_recent_entries() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let _ = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:danger".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .expect("tool run should complete");

        let events = broker
            .audit_events(AuditEventsRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                limit: 10,
            })
            .expect("audit events should load");
        assert!(!events.events.is_empty());
        assert_eq!(events.events[0].event_type, "tool_run");
    }

    #[test]
    fn test_router_fallback_metrics_increment_on_provider_failure() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state = root.join(".the-one");
        fs::create_dir_all(&state).expect("state dir should exist");
        fs::write(
            state.join("config.json"),
            r#"{"nano_provider":"api","nano_model":"gpt-nano"}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        let _ = broker
            .tool_run(
                &root,
                "project-1",
                ToolRunRequest {
                    action_key: "tool.run:nano-fail".to_string(),
                    interactive: false,
                    approval_scope: None,
                },
            )
            .expect("tool run should complete");

        let metrics = broker.metrics_snapshot();
        assert_eq!(metrics.router_fallback_calls, 1);
        assert_eq!(metrics.tool_run_calls, 1);
        assert_eq!(metrics.router_provider_error_calls, 1);
    }

    #[test]
    fn test_project_refresh_soak_keeps_cached_mode_when_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");
        fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");

        let broker = McpBroker::new();
        broker
            .project_init(ProjectInitRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .expect("init should succeed");

        for _ in 0..50 {
            let refresh = broker
                .project_refresh(ProjectRefreshRequest {
                    project_root: root.display().to_string(),
                    project_id: "project-1".to_string(),
                })
                .expect("refresh should succeed");
            assert_eq!(refresh.mode, "cached");
        }

        let metrics = broker.metrics_snapshot();
        assert!(metrics.project_refresh_calls >= 50);
    }
}
