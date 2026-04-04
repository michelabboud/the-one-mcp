use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::RwLock;
use tracing::instrument;

use the_one_core::config::{
    update_project_config, AppConfig, NanoProviderKind, ProjectConfigUpdate, RuntimeOverrides,
};
use the_one_core::contracts::{ApprovalScope, Capability, RiskLevel};
use the_one_core::docs_manager::DocsManager;
use the_one_core::error::CoreError;
use the_one_core::manifests::{
    load_overrides_manifest, project_state_paths, save_overrides_manifest, MANIFEST_SCHEMA_VERSION,
};
use the_one_core::policy::PolicyEngine;
use the_one_core::project::{project_init, project_refresh, RefreshMode};
use the_one_core::storage::sqlite::ProjectDatabase;
use the_one_memory::qdrant::QdrantOptions;
use the_one_memory::{MemoryEngine, MemorySearchRequest as EngineSearchRequest};
use the_one_registry::CapabilityRegistry;
use the_one_router::providers::{ApiNanoProvider, LmStudioNanoProvider, OllamaNanoProvider};
use the_one_router::{NanoBudget, RouteTelemetry, RoutedDecision, Router};

use crate::api::{
    AuditEventItem, AuditEventsRequest, AuditEventsResponse, ConfigExportResponse,
    ConfigUpdateRequest, ConfigUpdateResponse, DocsCreateRequest, DocsCreateResponse,
    DocsDeleteRequest, DocsDeleteResponse, DocsGetRequest, DocsGetResponse, DocsGetSectionRequest,
    DocsGetSectionResponse, DocsListRequest, DocsListResponse, DocsMoveRequest, DocsMoveResponse,
    DocsReindexRequest, DocsReindexResponse, DocsTrashEmptyRequest, DocsTrashEmptyResponse,
    DocsTrashListRequest, DocsTrashListResponse, DocsTrashRestoreRequest, DocsTrashRestoreResponse,
    DocsUpdateRequest, DocsUpdateResponse, MemoryFetchChunkRequest, MemoryFetchChunkResponse,
    MemorySearchItem, MemorySearchRequest, MemorySearchResponse, MetricsSnapshotResponse,
    ProjectInitRequest, ProjectInitResponse, ProjectProfileGetRequest, ProjectProfileGetResponse,
    ProjectRefreshRequest, ProjectRefreshResponse, ToolAddRequest, ToolAddResponse,
    ToolDisableRequest, ToolDisableResponse, ToolEnableRequest, ToolEnableResponse,
    ToolInfoRequest, ToolInstallRequest, ToolInstallResponse, ToolListRequest, ToolListResponse,
    ToolRemoveRequest, ToolRemoveResponse, ToolRunRequest, ToolRunResponse, ToolSearchRequest,
    ToolSearchResponse, ToolSuggestItem, ToolSuggestRequest, ToolSuggestResponse,
    ToolUpdateResponse,
};

pub struct McpBroker {
    router: Router,
    registry: CapabilityRegistry,
    memory_by_project: RwLock<HashMap<String, MemoryEngine>>,
    docs_by_project: RwLock<HashMap<String, DocsManager>>,
    global_registry_path: Option<PathBuf>,
    policy: PolicyEngine,
    session_approvals: RwLock<HashSet<String>>,
    metrics: BrokerMetrics,
    catalog: std::sync::Mutex<Option<the_one_core::tool_catalog::ToolCatalog>>,
    tools_embedded: std::sync::atomic::AtomicBool,
}

// Manual Debug impl since MemoryEngine, DocsManager, and ToolCatalog don't implement Debug
impl std::fmt::Debug for McpBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBroker")
            .field("router", &self.router)
            .field("registry", &self.registry)
            .field("global_registry_path", &self.global_registry_path)
            .field("policy", &self.policy)
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
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
            memory_by_project: RwLock::new(HashMap::new()),
            docs_by_project: RwLock::new(HashMap::new()),
            global_registry_path,
            policy,
            session_approvals: RwLock::new(HashSet::new()),
            metrics: BrokerMetrics::default(),
            catalog: std::sync::Mutex::new(None),
            tools_embedded: std::sync::atomic::AtomicBool::new(false),
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

    async fn with_project_memory<R>(
        &self,
        project_root: &Path,
        project_id: &str,
        f: impl FnOnce(&MemoryEngine) -> R,
    ) -> Result<R, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        let memories = self.memory_by_project.read().await;
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

        let max_chunk_tokens = config.limits.max_chunk_tokens;

        // Determine embedding + qdrant setup from config
        if config.embedding_provider == "api" {
            // API embeddings + Qdrant
            MemoryEngine::new_api(the_one_memory::ApiEngineConfig {
                embedding_base_url: config.embedding_api_base_url.as_deref().unwrap_or(""),
                embedding_api_key: config.embedding_api_key.as_deref(),
                embedding_model: &config.embedding_model,
                embedding_dims: config.embedding_dimensions,
                qdrant_url: &config.qdrant_url,
                project_id,
                qdrant_options: QdrantOptions {
                    api_key: config.qdrant_api_key.clone(),
                    ca_cert_path: config
                        .qdrant_ca_cert_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
                    tls_insecure: config.qdrant_tls_insecure,
                },
                max_chunk_tokens,
            })
            .map_err(CoreError::Embedding)
        } else {
            // Local fastembed (requires "local-embeddings" feature)
            #[cfg(feature = "local-embeddings")]
            {
                let qdrant_url = &config.qdrant_url;
                if qdrant_url.starts_with("http") {
                    let remote = !(qdrant_url.contains("localhost")
                        || qdrant_url.starts_with("http://127.0.0.1"));
                    if remote && config.qdrant_strict_auth && config.qdrant_api_key.is_none() {
                        return Err(CoreError::InvalidProjectConfig(
                            "remote qdrant requires api key when strict auth is enabled"
                                .to_string(),
                        ));
                    }

                    match MemoryEngine::new_with_qdrant(
                        &config.embedding_model,
                        qdrant_url,
                        project_id,
                        QdrantOptions {
                            api_key: config.qdrant_api_key.clone(),
                            ca_cert_path: config
                                .qdrant_ca_cert_path
                                .as_ref()
                                .map(|p| p.to_string_lossy().to_string()),
                            tls_insecure: config.qdrant_tls_insecure,
                        },
                        max_chunk_tokens,
                    ) {
                        Ok(engine) => Ok(engine),
                        Err(_) => MemoryEngine::new_local(
                            &config.embedding_model,
                            max_chunk_tokens,
                        )
                        .map_err(CoreError::Embedding),
                    }
                } else {
                    MemoryEngine::new_local(&config.embedding_model, max_chunk_tokens)
                        .map_err(CoreError::Embedding)
                }
            }

            // Without local embeddings, fall back to API-only
            #[cfg(not(feature = "local-embeddings"))]
            {
                Err(CoreError::Embedding(
                    "local embeddings not available (built without local-embeddings feature). \
                     Set embedding_provider to 'api' in config."
                        .to_string(),
                ))
            }
        }
    }

    async fn route_query(&self, project_root: &Path, query: &str) -> RoutedDecision {
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

    async fn route_tool_action(&self, project_root: &Path, action_key: &str) -> RoutedDecision {
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

    pub async fn ingest_docs(
        &self,
        project_root: &Path,
        project_id: &str,
        docs_root: &Path,
    ) -> Result<usize, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        let mut memories = self.memory_by_project.write().await;
        if !memories.contains_key(&key) {
            let engine = self.build_memory_engine(project_root, project_id)?;
            memories.insert(key.clone(), engine);
        }
        let memory = memories.get_mut(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;

        match memory.ingest_markdown_tree(docs_root).await {
            Ok(count) => Ok(count),
            Err(e) => {
                // Rebuild as local-only and retry (requires local-embeddings)
                #[cfg(feature = "local-embeddings")]
                {
                    let config =
                        AppConfig::load(project_root, RuntimeOverrides::default())?;
                    let fallback = MemoryEngine::new_local(
                        &config.embedding_model,
                        config.limits.max_chunk_tokens,
                    )
                    .map_err(CoreError::Embedding)?;
                    memories.insert(key.clone(), fallback);
                    let memory = memories.get_mut(&key).ok_or_else(|| {
                        CoreError::InvalidProjectConfig(
                            "project memory not indexed".to_string(),
                        )
                    })?;
                    memory.ingest_markdown_tree(docs_root).await.map_err(|e2| {
                        CoreError::Embedding(format!(
                            "ingest failed with qdrant ({e}) and local ({e2})"
                        ))
                    })
                }
                #[cfg(not(feature = "local-embeddings"))]
                Err(CoreError::Embedding(format!("ingest failed: {e}")))
            }
        }
    }

    #[instrument(skip_all)]
    pub async fn project_init(
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
    pub async fn project_refresh(
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
    pub async fn memory_search(&self, request: MemorySearchRequest) -> MemorySearchResponse {
        self.metrics
            .memory_search_calls
            .fetch_add(1, Ordering::Relaxed);
        let route = self
            .route_query(Path::new(&request.project_root), &request.query)
            .await;
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
            let key = Self::project_memory_key(project_root, &project_id);
            let memories = self.memory_by_project.read().await;
            if let Some(memory) = memories.get(&key) {
                memory
                    .search(&EngineSearchRequest { query, top_k })
                    .await
                    .into_iter()
                    .map(|item| MemorySearchItem {
                        id: item.chunk.id,
                        source_path: item.chunk.source_path,
                        score: item.score,
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
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
    pub async fn memory_fetch_chunk(
        &self,
        request: MemoryFetchChunkRequest,
    ) -> Option<MemoryFetchChunkResponse> {
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.fetch_chunk(&request.id),
        )
        .await
        .ok()
        .flatten()
        .map(|chunk| MemoryFetchChunkResponse {
            id: chunk.id,
            source_path: chunk.source_path,
            content: chunk.content,
        })
    }

    #[instrument(skip_all)]
    pub async fn docs_list(&self, request: DocsListRequest) -> DocsListResponse {
        let docs = self
            .with_project_memory(
                Path::new(&request.project_root),
                &request.project_id,
                MemoryEngine::docs_list,
            )
            .await
            .unwrap_or_default();
        DocsListResponse { docs }
    }

    #[instrument(skip_all)]
    pub async fn docs_get(&self, request: DocsGetRequest) -> Option<DocsGetResponse> {
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.docs_get(&request.path),
        )
        .await
        .ok()
        .flatten()
        .map(|content| DocsGetResponse {
            path: request.path,
            content,
        })
    }

    #[instrument(skip_all)]
    pub async fn docs_get_section(
        &self,
        request: DocsGetSectionRequest,
    ) -> Option<DocsGetSectionResponse> {
        let max_bytes = self.policy.clamp_doc_bytes(request.max_bytes);
        self.with_project_memory(
            Path::new(&request.project_root),
            &request.project_id,
            |memory| memory.docs_get_section(&request.path, &request.heading, max_bytes),
        )
        .await
        .ok()
        .flatten()
        .map(|content| DocsGetSectionResponse {
            path: request.path,
            heading: request.heading,
            content,
        })
    }

    // -----------------------------------------------------------------------
    // Catalog helpers
    // -----------------------------------------------------------------------

    fn ensure_catalog(&self) -> Result<(), CoreError> {
        {
            let guard = self
                .catalog
                .lock()
                .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
            if guard.is_some() {
                return Ok(());
            }
        }
        let global_dir = the_one_core::config::global_state_dir_or_default();
        let catalog = the_one_core::tool_catalog::ToolCatalog::open(&global_dir)?;

        // Import catalog files if catalog is empty
        if catalog.tool_count()? == 0 {
            let catalog_dir = Self::find_catalog_data_dir();
            if let Some(dir) = catalog_dir {
                let _ = catalog.import_catalog_dir(&dir);
            }
            let _ = catalog.scan_system_inventory();
        }

        let mut guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        *guard = Some(catalog);
        Ok(())
    }

    fn find_catalog_data_dir() -> Option<PathBuf> {
        let global = the_one_core::config::global_state_dir_or_default();
        let candidates = [PathBuf::from("tools/catalog"), global.join("catalog")];
        candidates.into_iter().find(|p| p.exists() && p.is_dir())
    }

    // -----------------------------------------------------------------------
    // Tool lifecycle methods (catalog-backed)
    // -----------------------------------------------------------------------

    #[instrument(skip_all)]
    pub async fn tool_add(&self, request: ToolAddRequest) -> Result<ToolAddResponse, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;

        let entry = the_one_core::tool_catalog::CatalogToolEntry {
            id: request.id.clone(),
            name: request.name,
            tool_type: request.tool_type,
            category: request.category,
            languages: request.languages,
            description: request.description,
            when_to_use: String::new(),
            what_it_finds: String::new(),
            install: Some(the_one_core::tool_catalog::CatalogInstall {
                command: request.install_command,
                binary_name: String::new(),
            }),
            run: Some(the_one_core::tool_catalog::CatalogRun {
                command: request.run_command,
            }),
            risk_level: request.risk_level.unwrap_or_else(|| "low".to_string()),
            tags: request.tags,
            github: request.github.unwrap_or_default(),
            trust_level: "user".to_string(),
        };
        cat.add_user_tool(&entry)?;
        Ok(ToolAddResponse {
            added: true,
            id: request.id,
        })
    }

    #[instrument(skip_all)]
    pub async fn tool_remove(
        &self,
        request: ToolRemoveRequest,
    ) -> Result<ToolRemoveResponse, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;
        let removed = cat.remove_user_tool(&request.tool_id)?;
        Ok(ToolRemoveResponse { removed })
    }

    #[instrument(skip_all)]
    pub async fn tool_disable(
        &self,
        request: ToolDisableRequest,
    ) -> Result<ToolDisableResponse, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;
        cat.disable_tool(&request.tool_id, "default", &request.project_root)?;
        Ok(ToolDisableResponse { disabled: true })
    }

    #[instrument(skip_all)]
    pub async fn tool_install(
        &self,
        request: ToolInstallRequest,
    ) -> Result<ToolInstallResponse, CoreError> {
        self.ensure_catalog()?;

        // Extract the install command while holding the lock, then drop it before await
        let install_command = {
            let guard = self
                .catalog
                .lock()
                .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
            let cat = guard
                .as_ref()
                .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;
            let tool = cat.get_tool(&request.tool_id)?.ok_or_else(|| {
                CoreError::Catalog(format!("tool not found: {}", request.tool_id))
            })?;
            tool.install_command.clone()
        };

        // Execute install command without holding the lock
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&install_command)
            .output()
            .await
            .map_err(|e| CoreError::Catalog(format!("install exec: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = format!("{stdout}{stderr}");

        if output.status.success() {
            // Re-acquire lock for post-install operations
            let guard = self
                .catalog
                .lock()
                .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
            let cat = guard
                .as_ref()
                .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;

            // Re-scan inventory to pick up the new binary
            let _ = cat.scan_system_inventory();
            // Auto-enable
            let _ = cat.enable_tool(&request.tool_id, "default", &request.project_root);

            let info = cat.get_tool(&request.tool_id)?;
            Ok(ToolInstallResponse {
                installed: true,
                binary_path: info.as_ref().and_then(|t| t.installed_path.clone()),
                version: info.as_ref().and_then(|t| t.installed_version.clone()),
                auto_enabled: true,
                output: combined,
            })
        } else {
            Ok(ToolInstallResponse {
                installed: false,
                binary_path: None,
                version: None,
                auto_enabled: false,
                output: combined,
            })
        }
    }

    #[instrument(skip_all)]
    pub async fn tool_info(
        &self,
        request: ToolInfoRequest,
    ) -> Result<Option<the_one_core::tool_catalog::ToolFullInfo>, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;
        cat.get_tool(&request.tool_id)
    }

    #[instrument(skip_all)]
    pub async fn tool_catalog_update(&self) -> Result<ToolUpdateResponse, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;

        let version_before = cat.catalog_version()?;

        // Re-import from catalog dir
        let catalog_dir = Self::find_catalog_data_dir();
        let mut tools_added = 0;
        if let Some(dir) = catalog_dir {
            tools_added = cat.import_catalog_dir(&dir)?;
        }

        // Re-scan system
        let system_found = cat.scan_system_inventory()?;

        let version_after = cat.catalog_version()?;

        Ok(ToolUpdateResponse {
            catalog_version_before: version_before,
            catalog_version_after: version_after,
            tools_added,
            tools_updated: 0,
            system_tools_found: system_found,
        })
    }

    #[instrument(skip_all)]
    pub async fn tool_list(&self, request: ToolListRequest) -> Result<ToolListResponse, CoreError> {
        self.ensure_catalog()?;
        let guard = self
            .catalog
            .lock()
            .map_err(|e| CoreError::Catalog(format!("catalog lock poisoned: {e}")))?;
        let cat = guard
            .as_ref()
            .ok_or_else(|| CoreError::Catalog("catalog not initialized".into()))?;

        let result = cat.suggest(&[], None, None, "default", &request.project_root, 1000)?;

        let all: Vec<the_one_core::tool_catalog::ToolSummary> = result
            .enabled
            .into_iter()
            .chain(result.available)
            .chain(result.recommended)
            .collect();

        // Filter by state if requested
        let tools = if let Some(ref state_filter) = request.state {
            all.into_iter()
                .filter(|t| t.state == *state_filter)
                .collect()
        } else {
            all
        };

        Ok(ToolListResponse { tools })
    }

    #[instrument(skip_all)]
    pub async fn tool_suggest(&self, request: ToolSuggestRequest) -> ToolSuggestResponse {
        let limit = self.policy.clamp_suggestions(request.max);

        // Try catalog-based search first
        if self.ensure_catalog().is_ok() {
            if let Ok(guard) = self.catalog.lock() {
                if let Some(cat) = guard.as_ref() {
                    if let Ok(results) = cat.search_fts(&request.query, limit as u32) {
                        if !results.is_empty() {
                            let suggestions = results
                                .into_iter()
                                .map(|r| ToolSuggestItem {
                                    id: r.id,
                                    title: r.name,
                                    reason: format!("[{}] {}", r.source, r.description),
                                })
                                .take(limit)
                                .collect();
                            return ToolSuggestResponse { suggestions };
                        }
                    }
                }
            }
        }

        // Fallback to old registry-based suggest
        let suggestions = self
            .registry
            .suggest(&request.query, RiskLevel::Medium, limit)
            .into_iter()
            .map(|s| ToolSuggestItem {
                id: s.id,
                title: s.title,
                reason: s.reason,
            })
            .collect();

        ToolSuggestResponse { suggestions }
    }

    fn try_build_embedding_provider(
        config: &AppConfig,
    ) -> Result<Box<dyn the_one_memory::embeddings::EmbeddingProvider>, String> {
        if config.embedding_provider == "api" {
            Ok(Box::new(
                the_one_memory::embeddings::ApiEmbeddingProvider::new(
                    config.embedding_api_base_url.as_deref().unwrap_or(""),
                    config.embedding_api_key.as_deref(),
                    &config.embedding_model,
                    config.embedding_dimensions,
                ),
            ))
        } else {
            #[cfg(feature = "local-embeddings")]
            {
                Ok(Box::new(
                    the_one_memory::embeddings::FastEmbedProvider::new(&config.embedding_model)
                        .map_err(|e| format!("fastembed: {e}"))?,
                ))
            }
            #[cfg(not(feature = "local-embeddings"))]
            {
                Err("local embeddings not available (built without local-embeddings feature)".into())
            }
        }
    }

    async fn ensure_tools_embedded(&self) {
        if self
            .tools_embedded
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }

        // Try to embed tools into Qdrant — failures are silently ignored (FTS fallback)
        let result: Result<(), String> = async {
            let config = AppConfig::load(
                &std::env::current_dir().unwrap_or_default(),
                RuntimeOverrides::default(),
            )
            .map_err(|e| e.to_string())?;

            // Get tool descriptions from catalog
            let descriptions = {
                let guard = self.catalog.lock().map_err(|e| format!("lock: {e}"))?;
                match guard.as_ref() {
                    Some(cat) => cat.all_tool_descriptions().map_err(|e| e.to_string())?,
                    None => return Ok(()),
                }
            };

            if descriptions.is_empty() {
                return Ok(());
            }

            // Build embedding provider
            let provider = Self::try_build_embedding_provider(&config)?;

            // Build Qdrant backend for "tools" collection (global, not per-project)
            let qdrant = the_one_memory::qdrant::AsyncQdrantBackend::new(
                &config.qdrant_url,
                "tools",
                the_one_memory::qdrant::QdrantOptions {
                    api_key: config.qdrant_api_key.clone(),
                    ca_cert_path: config
                        .qdrant_ca_cert_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
                    tls_insecure: config.qdrant_tls_insecure,
                },
            )
            .map_err(|e| format!("qdrant: {e}"))?;

            qdrant
                .ensure_collection(provider.dimensions())
                .await
                .map_err(|e| format!("collection: {e}"))?;

            // Embed in batches
            let texts: Vec<String> = descriptions.iter().map(|(_, text)| text.clone()).collect();
            let batch_size = 64;
            for batch_start in (0..texts.len()).step_by(batch_size) {
                let batch_end = (batch_start + batch_size).min(texts.len());
                let batch = texts[batch_start..batch_end].to_vec();
                let vectors = provider
                    .embed_batch(&batch)
                    .await
                    .map_err(|e| format!("embed: {e}"))?;

                let points: Vec<the_one_memory::qdrant::QdrantPoint> = descriptions
                    [batch_start..batch_end]
                    .iter()
                    .zip(vectors)
                    .map(|((id, _), vector)| the_one_memory::qdrant::QdrantPoint {
                        id: id.clone(),
                        vector,
                        payload: the_one_memory::qdrant::QdrantPayload {
                            chunk_id: id.clone(),
                            source_path: String::new(),
                            heading: String::new(),
                            chunk_index: 0,
                        },
                    })
                    .collect();

                qdrant
                    .upsert_points(points)
                    .await
                    .map_err(|e| format!("upsert: {e}"))?;
            }

            Ok(())
        }
        .await;

        if result.is_ok() {
            self.tools_embedded
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("Tool catalog embedded into Qdrant");
        } else {
            tracing::debug!(
                "Tool embedding skipped (Qdrant unavailable): {:?}",
                result.err()
            );
        }
    }

    async fn search_tools_semantic(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<the_one_core::tool_catalog::SearchResult>, String> {
        let config = AppConfig::load(
            &std::env::current_dir().unwrap_or_default(),
            RuntimeOverrides::default(),
        )
        .map_err(|e| e.to_string())?;

        let provider = Self::try_build_embedding_provider(&config)?;
        let query_vector = provider.embed_single(query).await?;

        let qdrant = the_one_memory::qdrant::AsyncQdrantBackend::new(
            &config.qdrant_url,
            "tools",
            the_one_memory::qdrant::QdrantOptions {
                api_key: config.qdrant_api_key.clone(),
                ca_cert_path: config
                    .qdrant_ca_cert_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                tls_insecure: config.qdrant_tls_insecure,
            },
        )
        .map_err(|e| format!("qdrant: {e}"))?;

        let qdrant_results = qdrant.search(query_vector, limit, 0.3).await?;

        // Map Qdrant results back to SearchResult using catalog DB
        let guard = self.catalog.lock().map_err(|e| format!("lock: {e}"))?;
        let cat = guard.as_ref().ok_or("catalog not initialized")?;

        let mut results = Vec::new();
        for qr in qdrant_results {
            if let Ok(Some(tool)) = cat.get_tool(&qr.chunk_id) {
                results.push(the_one_core::tool_catalog::SearchResult {
                    id: tool.id,
                    name: tool.name,
                    description: tool.description,
                    score: qr.score as f64,
                    state: tool.state,
                    source: tool.source,
                });
            }
        }

        Ok(results)
    }

    #[instrument(skip_all)]
    pub async fn tool_search(&self, request: ToolSearchRequest) -> ToolSearchResponse {
        let clamped = self.policy.clamp_search_hits(request.max);

        // Try semantic search via Qdrant
        self.ensure_tools_embedded().await;
        if let Ok(results) = self.search_tools_semantic(&request.query, clamped).await {
            if !results.is_empty() {
                let matches: Vec<ToolSuggestItem> = results
                    .iter()
                    .map(|r| ToolSuggestItem {
                        id: r.id.clone(),
                        title: r.name.clone(),
                        reason: format!("[{:.0}% match] {}", r.score * 100.0, r.description),
                    })
                    .collect();
                return ToolSearchResponse { matches };
            }
        }

        // Try catalog FTS
        self.ensure_catalog().ok();
        if let Ok(guard) = self.catalog.lock() {
            if let Some(cat) = guard.as_ref() {
                if let Ok(results) = cat.search_fts(&request.query, clamped as u32) {
                    let matches: Vec<ToolSuggestItem> = results
                        .iter()
                        .map(|r| ToolSuggestItem {
                            id: r.id.clone(),
                            title: r.name.clone(),
                            reason: format!("[text match] {}", r.description),
                        })
                        .collect();
                    if !matches.is_empty() {
                        return ToolSearchResponse { matches };
                    }
                }
            }
        }

        // Final fallback: old registry search
        let suggestions = self
            .registry
            .suggest(&request.query, RiskLevel::Medium, clamped);
        ToolSearchResponse {
            matches: suggestions
                .into_iter()
                .map(|s| ToolSuggestItem {
                    id: s.id,
                    title: s.title,
                    reason: s.reason,
                })
                .collect(),
        }
    }

    #[instrument(skip_all)]
    pub async fn tool_run(
        &self,
        project_root: &Path,
        project_id: &str,
        request: ToolRunRequest,
    ) -> Result<ToolRunResponse, CoreError> {
        self.metrics.tool_run_calls.fetch_add(1, Ordering::Relaxed);
        let routed = self
            .route_tool_action(project_root, &request.action_key)
            .await;
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
                .read()
                .await
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
                    .write()
                    .await
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
    pub async fn config_export(
        &self,
        project_root: &Path,
    ) -> Result<ConfigExportResponse, CoreError> {
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
    pub async fn project_profile_get(
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
    pub async fn tool_enable(
        &self,
        request: ToolEnableRequest,
    ) -> Result<ToolEnableResponse, CoreError> {
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
    pub async fn audit_events(
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

    // -----------------------------------------------------------------------
    // Docs CRUD — delegated to DocsManager per-project
    // -----------------------------------------------------------------------

    async fn with_docs_manager<R>(
        &self,
        project_root: &Path,
        f: impl FnOnce(&DocsManager) -> Result<R, CoreError>,
    ) -> Result<R, CoreError> {
        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf())
            .to_string_lossy()
            .to_string();
        {
            let managers = self.docs_by_project.read().await;
            if let Some(dm) = managers.get(&key) {
                return f(dm);
            }
        }
        let dm = DocsManager::new(project_root)?;
        let result = f(&dm);
        let mut managers = self.docs_by_project.write().await;
        managers.entry(key).or_insert(dm);
        result
    }

    #[instrument(skip_all)]
    pub async fn docs_create(
        &self,
        request: DocsCreateRequest,
    ) -> Result<DocsCreateResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let config = AppConfig::load(&project_root, RuntimeOverrides::default())?;
        let max_doc_bytes = config.limits.max_doc_size_bytes;
        let max_docs = config.limits.max_managed_docs;
        let path = request.path.clone();
        let content = request.content.clone();
        self.with_docs_manager(&project_root, move |dm| {
            dm.create(&path, &content, max_doc_bytes, max_docs)?;
            Ok(DocsCreateResponse { path })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_update(
        &self,
        request: DocsUpdateRequest,
    ) -> Result<DocsUpdateResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let config = AppConfig::load(&project_root, RuntimeOverrides::default())?;
        let max_doc_bytes = config.limits.max_doc_size_bytes;
        let path = request.path.clone();
        let content = request.content.clone();
        self.with_docs_manager(&project_root, move |dm| {
            dm.update(&path, &content, max_doc_bytes)?;
            Ok(DocsUpdateResponse { path })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_delete(
        &self,
        request: DocsDeleteRequest,
    ) -> Result<DocsDeleteResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let path = request.path.clone();
        self.with_docs_manager(&project_root, move |dm| {
            dm.delete(&path)?;
            Ok(DocsDeleteResponse { deleted: true })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_move(&self, request: DocsMoveRequest) -> Result<DocsMoveResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let from = request.from.clone();
        let to = request.to.clone();
        self.with_docs_manager(&project_root, move |dm| {
            dm.move_doc(&from, &to)?;
            Ok(DocsMoveResponse { from, to })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_trash_list(
        &self,
        request: DocsTrashListRequest,
    ) -> Result<DocsTrashListResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        self.with_docs_manager(&project_root, |dm| {
            let entries = dm.trash_list()?;
            Ok(DocsTrashListResponse { entries })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_trash_restore(
        &self,
        request: DocsTrashRestoreRequest,
    ) -> Result<DocsTrashRestoreResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let path = request.path.clone();
        self.with_docs_manager(&project_root, move |dm| {
            dm.trash_restore(&path)?;
            Ok(DocsTrashRestoreResponse { restored: true })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_trash_empty(
        &self,
        request: DocsTrashEmptyRequest,
    ) -> Result<DocsTrashEmptyResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        self.with_docs_manager(&project_root, |dm| {
            dm.trash_empty()?;
            Ok(DocsTrashEmptyResponse { emptied: true })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn docs_reindex(
        &self,
        request: DocsReindexRequest,
    ) -> Result<DocsReindexResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let project_id = request.project_id.clone();
        let config = AppConfig::load(&project_root, RuntimeOverrides::default())?;

        // Ensure DocsManager exists for the project (creates managed docs dir)
        let managed_root = {
            let key = project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.clone())
                .to_string_lossy()
                .to_string();
            {
                let managers = self.docs_by_project.read().await;
                if let Some(dm) = managers.get(&key) {
                    dm.managed_root().to_path_buf()
                } else {
                    drop(managers);
                    let dm = DocsManager::new(&project_root)?;
                    let root = dm.managed_root().to_path_buf();
                    let mut managers = self.docs_by_project.write().await;
                    managers.entry(key).or_insert(dm);
                    root
                }
            }
        };

        // Ingest managed docs via the fallback-aware ingest_docs
        let managed_count = self
            .ingest_docs(&project_root, &project_id, &managed_root)
            .await?;

        // Ingest external docs root if configured
        let mut external_count = 0;
        if let Some(ext_root) = &config.external_docs_root {
            external_count = self
                .ingest_docs(&project_root, &project_id, ext_root)
                .await?;
        }

        let total_new = managed_count + external_count;

        Ok(DocsReindexResponse {
            new: total_new,
            updated: 0,
            removed: 0,
            unchanged: 0,
        })
    }

    #[instrument(skip_all)]
    pub async fn config_update(
        &self,
        request: ConfigUpdateRequest,
    ) -> Result<ConfigUpdateResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let obj = request.update.as_object().ok_or_else(|| {
            CoreError::InvalidProjectConfig("config update must be a JSON object".to_string())
        })?;

        let mut update = ProjectConfigUpdate::default();
        if let Some(v) = obj.get("provider").and_then(|v| v.as_str()) {
            update.provider = Some(v.to_string());
        }
        if let Some(v) = obj.get("log_level").and_then(|v| v.as_str()) {
            update.log_level = Some(v.to_string());
        }
        if let Some(v) = obj.get("qdrant_url").and_then(|v| v.as_str()) {
            update.qdrant_url = Some(v.to_string());
        }
        if let Some(v) = obj.get("nano_provider").and_then(|v| v.as_str()) {
            update.nano_provider = Some(v.to_string());
        }
        if let Some(v) = obj.get("nano_model").and_then(|v| v.as_str()) {
            update.nano_model = Some(v.to_string());
        }
        if let Some(v) = obj.get("qdrant_api_key").and_then(|v| v.as_str()) {
            update.qdrant_api_key = Some(v.to_string());
        }
        if let Some(v) = obj.get("qdrant_ca_cert_path").and_then(|v| v.as_str()) {
            update.qdrant_ca_cert_path = Some(v.to_string());
        }
        if let Some(v) = obj.get("qdrant_tls_insecure").and_then(|v| v.as_bool()) {
            update.qdrant_tls_insecure = Some(v);
        }
        if let Some(v) = obj.get("qdrant_strict_auth").and_then(|v| v.as_bool()) {
            update.qdrant_strict_auth = Some(v);
        }
        if let Some(v) = obj.get("embedding_provider").and_then(|v| v.as_str()) {
            update.embedding_provider = Some(v.to_string());
        }
        if let Some(v) = obj.get("embedding_model").and_then(|v| v.as_str()) {
            update.embedding_model = Some(v.to_string());
        }
        if let Some(v) = obj.get("embedding_api_base_url").and_then(|v| v.as_str()) {
            update.embedding_api_base_url = Some(v.to_string());
        }
        if let Some(v) = obj.get("embedding_api_key").and_then(|v| v.as_str()) {
            update.embedding_api_key = Some(v.to_string());
        }
        if let Some(v) = obj.get("embedding_dimensions").and_then(|v| v.as_u64()) {
            update.embedding_dimensions = Some(v as usize);
        }
        if let Some(v) = obj.get("external_docs_root").and_then(|v| v.as_str()) {
            update.external_docs_root = Some(v.to_string());
        }

        let path = update_project_config(&project_root, update)?;
        Ok(ConfigUpdateResponse {
            path: path.display().to_string(),
        })
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
        AuditEventsRequest, DocsCreateRequest, DocsDeleteRequest, DocsGetSectionRequest,
        DocsListRequest, DocsMoveRequest, DocsReindexRequest, DocsTrashEmptyRequest,
        DocsTrashListRequest, DocsTrashRestoreRequest, DocsUpdateRequest, MemorySearchRequest,
        ProjectInitRequest, ProjectProfileGetRequest, ProjectRefreshRequest, ToolEnableRequest,
        ToolRunRequest, ToolSuggestRequest,
    };

    #[tokio::test]
    async fn test_project_init_and_refresh_flow() {
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
            .await
            .expect("init should succeed");
        assert_eq!(init.project_id, "project-1");

        let refresh = broker
            .project_refresh(ProjectRefreshRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .await
            .expect("refresh should succeed");
        assert_eq!(refresh.mode, "cached");
    }

    #[tokio::test]
    async fn test_memory_search_and_tool_suggest() {
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
            .await
            .expect("ingest should succeed");

        let search = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search docs".to_string(),
                top_k: 5,
            })
            .await;
        assert_eq!(search.hits.len(), 1);
        assert!(!search.provider_path.is_empty());

        let docs_list = broker
            .docs_list(DocsListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .await;
        assert_eq!(docs_list.docs.len(), 1);
        let section = broker
            .docs_get_section(DocsGetSectionRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: docs_list.docs[0].clone(),
                heading: "Intro".to_string(),
                max_bytes: 64,
            })
            .await;
        assert!(section.is_some());

        let suggest = broker
            .tool_suggest(ToolSuggestRequest {
                query: "docs".to_string(),
                max: 5,
            })
            .await;
        assert!(suggest
            .suggestions
            .iter()
            .any(|item| item.id == "docs.search"));

        std::env::remove_var("THE_ONE_HOME");
    }

    #[tokio::test]
    async fn test_memory_search_uses_nano_provider_and_reports_telemetry() {
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
            .await
            .expect("ingest should succeed");

        let search = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search docs".to_string(),
                top_k: 5,
            })
            .await;

        assert_eq!(search.route, "RuleWithNano");
        assert_eq!(search.provider_path, "api-nano");
        assert!(!search.fallback_used);
    }

    #[tokio::test]
    async fn test_ingest_docs_builds_memory_engine_from_config() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let docs = temp.path().join("docs");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::create_dir_all(&docs).expect("docs dir should exist");
        fs::write(docs.join("x.md"), "# Intro\nqdrant test").expect("doc write should work");

        let broker = McpBroker::new();
        let count = broker
            .ingest_docs(&root, "project-1", &docs)
            .await
            .expect("ingest should succeed");
        assert!(count >= 1);
    }

    #[tokio::test]
    async fn test_remote_qdrant_strict_auth_rejects_missing_api_key() {
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
            .await
            .expect_err("ingest should fail without api key");
        assert!(err.to_string().contains("remote qdrant requires api key"));
    }

    #[tokio::test]
    async fn test_memory_search_reports_fallback_when_nano_fails() {
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
            .await
            .expect("ingest should succeed");

        let search = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search docs nano-fail".to_string(),
                top_k: 5,
            })
            .await;
        assert!(search.fallback_used);
        assert!(search.last_error.is_some());
        assert!(search.retries_bound <= 3);

        let metrics = broker.metrics_snapshot();
        assert!(metrics.router_fallback_calls >= 1);
    }

    #[tokio::test]
    async fn test_tool_run_headless_requires_prior_approval() {
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
            .await
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
            .await
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
            .await
            .expect("tool run should complete");
        assert!(headless_after.allowed);
    }

    #[tokio::test]
    async fn test_config_export_returns_defaults() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let config = broker
            .config_export(&root)
            .await
            .expect("config export should work");
        assert_eq!(config.schema_version, "v1beta");
        assert_eq!(config.provider, "local");
        assert!(!config.qdrant_auth_configured);
        assert!(!config.qdrant_tls_insecure);
        assert!(config.qdrant_strict_auth);
        assert_eq!(config.nano_provider, "rules");
        assert_eq!(config.nano_model, "none");
    }

    #[tokio::test]
    async fn test_tool_route_uses_project_nano_provider_config() {
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
        let decision = broker.route_tool_action(&root, "tool.run:danger").await;

        assert!(decision.decision.rationale.contains("ollama"));
    }

    #[tokio::test]
    async fn test_session_approval_allows_headless_in_same_broker_session() {
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
            .await
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
            .await
            .expect("headless run should work");
        assert!(headless.allowed);
    }

    #[tokio::test]
    async fn test_tool_enable_and_profile_get_and_metrics() {
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
            .await
            .expect("init should succeed");

        let enabled = broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "docs".to_string(),
            })
            .await
            .expect("tool enable should succeed");
        assert!(enabled.enabled_families.contains(&"docs".to_string()));

        let profile = broker
            .project_profile_get(ProjectProfileGetRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .await
            .expect("profile get should succeed");
        assert!(profile.is_some());

        let metrics = broker.metrics_snapshot();
        assert!(metrics.project_init_calls >= 1);
    }

    #[tokio::test]
    async fn test_policy_limits_bound_suggestions_and_search_hits() {
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
            .await
            .expect("ingest should work");

        let suggestions = broker
            .tool_suggest(ToolSuggestRequest {
                query: "docs".to_string(),
                max: 10,
            })
            .await;
        assert_eq!(suggestions.suggestions.len(), 1);

        let hits = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search".to_string(),
                top_k: 10,
            })
            .await;
        assert_eq!(hits.hits.len(), 1);

        let docs_list = broker
            .docs_list(DocsListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
            })
            .await;

        let section = broker
            .docs_get_section(DocsGetSectionRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: docs_list.docs[0].clone(),
                heading: "Intro".to_string(),
                max_bytes: 100,
            })
            .await
            .expect("section should exist");
        assert!(section.content.len() <= 1024);
    }

    #[tokio::test]
    async fn test_tool_enable_respects_max_enabled_families() {
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
            .await
            .expect("init should succeed");

        broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "a".to_string(),
            })
            .await
            .expect("enable a");
        broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "b".to_string(),
            })
            .await
            .expect("enable b");

        let err = broker
            .tool_enable(ToolEnableRequest {
                project_root: root.display().to_string(),
                family: "c".to_string(),
            })
            .await
            .expect_err("enable c should fail");
        assert!(err
            .to_string()
            .contains("enabled families exceed policy limit"));
    }

    #[tokio::test]
    async fn test_audit_events_endpoint_returns_recent_entries() {
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
            .await
            .expect("tool run should complete");

        let events = broker
            .audit_events(AuditEventsRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                limit: 10,
            })
            .await
            .expect("audit events should load");
        assert!(!events.events.is_empty());
        assert_eq!(events.events[0].event_type, "tool_run");
    }

    #[tokio::test]
    async fn test_router_fallback_metrics_increment_on_provider_failure() {
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
            .await
            .expect("tool run should complete");

        let metrics = broker.metrics_snapshot();
        assert_eq!(metrics.router_fallback_calls, 1);
        assert_eq!(metrics.tool_run_calls, 1);
        assert_eq!(metrics.router_provider_error_calls, 1);
    }

    #[tokio::test]
    async fn test_project_refresh_soak_keeps_cached_mode_when_unchanged() {
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
            .await
            .expect("init should succeed");

        for _ in 0..50 {
            let refresh = broker
                .project_refresh(ProjectRefreshRequest {
                    project_root: root.display().to_string(),
                    project_id: "project-1".to_string(),
                })
                .await
                .expect("refresh should succeed");
            assert_eq!(refresh.mode, "cached");
        }

        let metrics = broker.metrics_snapshot();
        assert!(metrics.project_refresh_calls >= 50);
    }

    #[tokio::test]
    async fn test_docs_crud_create_update_delete_move() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();

        // Create
        let created = broker
            .docs_create(DocsCreateRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "hello.md".to_string(),
                content: "# Hello\nWorld".to_string(),
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.path, "hello.md");

        // Update
        let updated = broker
            .docs_update(DocsUpdateRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "hello.md".to_string(),
                content: "# Hello\nUpdated".to_string(),
            })
            .await
            .expect("update should succeed");
        assert_eq!(updated.path, "hello.md");

        // Move
        let moved = broker
            .docs_move(DocsMoveRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                from: "hello.md".to_string(),
                to: "renamed.md".to_string(),
            })
            .await
            .expect("move should succeed");
        assert_eq!(moved.from, "hello.md");
        assert_eq!(moved.to, "renamed.md");

        // Delete (soft)
        let deleted = broker
            .docs_delete(DocsDeleteRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "renamed.md".to_string(),
            })
            .await
            .expect("delete should succeed");
        assert!(deleted.deleted);

        // Trash list
        let trash = broker
            .docs_trash_list(DocsTrashListRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
            })
            .await
            .expect("trash list should succeed");
        assert_eq!(trash.entries.len(), 1);

        // Trash restore
        let restored = broker
            .docs_trash_restore(DocsTrashRestoreRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "renamed.md".to_string(),
            })
            .await
            .expect("restore should succeed");
        assert!(restored.restored);

        // Delete again + empty trash
        broker
            .docs_delete(DocsDeleteRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "renamed.md".to_string(),
            })
            .await
            .expect("delete again should succeed");
        let emptied = broker
            .docs_trash_empty(DocsTrashEmptyRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
            })
            .await
            .expect("empty should succeed");
        assert!(emptied.emptied);
    }

    #[tokio::test]
    async fn test_docs_reindex() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();

        // Create a doc first
        broker
            .docs_create(DocsCreateRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
                path: "test.md".to_string(),
                content: "# Test\nReindex content".to_string(),
            })
            .await
            .expect("create should succeed");

        let result = broker
            .docs_reindex(DocsReindexRequest {
                project_root: root.display().to_string(),
                project_id: "p1".to_string(),
            })
            .await
            .expect("reindex should succeed");
        assert!(result.new >= 1);
    }

    #[tokio::test]
    async fn test_config_update() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let broker = McpBroker::new();
        let result = broker
            .config_update(crate::api::ConfigUpdateRequest {
                project_root: root.display().to_string(),
                update: serde_json::json!({
                    "nano_provider": "ollama",
                    "nano_model": "tiny"
                }),
            })
            .await
            .expect("config update should succeed");
        assert!(result.path.contains("config.json"));

        // Verify the config was actually updated
        let config = broker
            .config_export(&root)
            .await
            .expect("export should work");
        assert_eq!(config.nano_provider, "ollama");
        assert_eq!(config.nano_model, "tiny");
    }
}
