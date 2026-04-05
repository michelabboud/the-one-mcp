use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
#[cfg(feature = "local-embeddings")]
use the_one_memory::reranker::Reranker;
use the_one_memory::{MemoryEngine, MemorySearchRequest as EngineSearchRequest, RetrievalMode};
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
    memory_by_project: Arc<RwLock<HashMap<String, MemoryEngine>>>,
    docs_by_project: RwLock<HashMap<String, DocsManager>>,
    global_registry_path: Option<PathBuf>,
    policy: PolicyEngine,
    session_approvals: RwLock<HashSet<String>>,
    // v0.12.0: wrapped in Arc so spawned tasks (e.g. the file watcher) can
    // share it lock-free via the atomic counter fields.
    metrics: Arc<BrokerMetrics>,
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
    // v0.12.0: observability deep dive — new counters
    memory_search_latency_ms_total: AtomicU64,
    image_search_calls: AtomicU64,
    image_ingest_calls: AtomicU64,
    resources_list_calls: AtomicU64,
    resources_read_calls: AtomicU64,
    watcher_events_processed: AtomicU64,
    watcher_events_failed: AtomicU64,
    qdrant_errors: AtomicU64,
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
            memory_by_project: Arc::new(RwLock::new(HashMap::new())),
            docs_by_project: RwLock::new(HashMap::new()),
            global_registry_path,
            policy,
            session_approvals: RwLock::new(HashSet::new()),
            metrics: Arc::new(BrokerMetrics::default()),
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
        let mut engine = if config.embedding_provider == "api" {
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
            .map_err(CoreError::Embedding)?
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
                        Ok(engine) => engine,
                        Err(_) => {
                            MemoryEngine::new_local(&config.embedding_model, max_chunk_tokens)
                                .map_err(CoreError::Embedding)?
                        }
                    }
                } else {
                    MemoryEngine::new_local(&config.embedding_model, max_chunk_tokens)
                        .map_err(CoreError::Embedding)?
                }
            }

            // Without local embeddings, fall back to API-only
            #[cfg(not(feature = "local-embeddings"))]
            {
                return Err(CoreError::Embedding(
                    "local embeddings not available (built without local-embeddings feature). \
                     Set embedding_provider to 'api' in config."
                        .to_string(),
                ));
            }
        };

        // Attach cross-encoder reranker if enabled (LightRAG-inspired improvement)
        #[cfg(feature = "local-embeddings")]
        if config.reranker_enabled {
            match the_one_memory::reranker::FastEmbedReranker::new(&config.reranker_model) {
                Ok(reranker) => {
                    tracing::info!(
                        "reranker enabled: {} (model: {})",
                        reranker.name(),
                        config.reranker_model
                    );
                    engine.set_reranker(Box::new(reranker));
                }
                Err(e) => {
                    tracing::warn!("failed to init reranker, continuing without: {e}");
                }
            }
        }

        // Attach sparse provider for hybrid (BM25 + dense) search if enabled
        if config.hybrid_search_enabled {
            #[cfg(feature = "local-embeddings")]
            match the_one_memory::sparse_embeddings::FastEmbedSparseProvider::new(
                &config.sparse_model,
            ) {
                Ok(sparse) => {
                    tracing::info!(
                        "hybrid search enabled: sparse model '{}', dense_weight={}, sparse_weight={}",
                        config.sparse_model,
                        config.hybrid_dense_weight,
                        config.hybrid_sparse_weight,
                    );
                    engine.set_sparse_provider(
                        Box::new(sparse),
                        config.hybrid_dense_weight,
                        config.hybrid_sparse_weight,
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to init sparse provider ({}), continuing without hybrid search: {e}",
                        config.sparse_model
                    );
                }
            }
        }

        // Load knowledge graph if it exists
        let graph_path = config.project_state_dir.join("knowledge_graph.json");
        match the_one_memory::graph::KnowledgeGraph::load_from_file(&graph_path) {
            Ok(graph) => {
                let entity_count = graph.entity_count();
                let relation_count = graph.relation_count();
                if entity_count > 0 {
                    tracing::info!(
                        "loaded knowledge graph: {} entities, {} relations",
                        entity_count,
                        relation_count
                    );
                }
                *engine.graph_mut() = graph;
            }
            Err(e) => {
                tracing::warn!("failed to load knowledge graph: {e}");
            }
        }

        Ok(engine)
    }

    /// Spawn a background file watcher task for the given project if
    /// `auto_index_enabled` is true in the project config.
    ///
    /// When an `.md` file changes the engine calls `ingest_single_markdown` to
    /// re-chunk and re-embed the file in place.  When a file is removed the
    /// engine calls `remove_by_path`.  For image files, the watcher calls the
    /// standalone image ingest/remove helpers so the Qdrant image collection
    /// stays in sync as well (v0.8.2).
    fn maybe_spawn_watcher(&self, project_root: &Path, project_id: &str) {
        let config = match AppConfig::load(project_root, RuntimeOverrides::default()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("watcher: could not load config for {project_id}: {e}");
                return;
            }
        };

        if !config.auto_index_enabled {
            return;
        }

        let docs_path = project_root.join(".the-one").join("docs");
        let images_path = project_root.join(".the-one").join("images");
        let extensions = the_one_memory::watcher::DEFAULT_WATCHED_EXTENSIONS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let debounce_ms = config.auto_index_debounce_ms;
        let project_id = project_id.to_string();
        let project_root_owned = project_root.to_path_buf();

        // Clone the Arc so the spawned task can reach the per-project engines
        let memory_by_project = Arc::clone(&self.memory_by_project);
        let metrics = Arc::clone(&self.metrics);
        let project_key = Self::project_memory_key(project_root, &project_id);

        match the_one_memory::watcher::spawn_watcher(
            vec![docs_path, images_path],
            extensions,
            debounce_ms,
        ) {
            Ok((mut rx, cancel, debouncer)) => {
                tracing::info!(
                    "auto-indexing enabled for project {project_id} (debounce={debounce_ms}ms)"
                );
                tokio::spawn(async move {
                    // Keep debouncer alive for the lifetime of the task
                    let _guard = debouncer;
                    while let Some(event) = rx.recv().await {
                        if cancel.is_cancelled() {
                            break;
                        }

                        let path = match &event {
                            the_one_memory::watcher::WatchEvent::Upserted(p) => p.clone(),
                            the_one_memory::watcher::WatchEvent::Removed(p) => p.clone(),
                        };

                        let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");
                        let is_image = matches!(
                            path.extension().and_then(|e| e.to_str()),
                            Some("png") | Some("jpg") | Some("jpeg") | Some("webp")
                        );

                        // Markdown events go through the MemoryEngine (write lock on
                        // memory_by_project). Image events go through the standalone
                        // helpers and do not touch the text MemoryEngine.
                        if is_md {
                            let mut memories = memory_by_project.write().await;
                            if let Some(engine) = memories.get_mut(&project_key) {
                                let result = match &event {
                                    the_one_memory::watcher::WatchEvent::Upserted(p) => {
                                        engine.ingest_single_markdown(p).await.map(|n| {
                                            format!("reindexed {n} chunks from {}", p.display())
                                        })
                                    }
                                    the_one_memory::watcher::WatchEvent::Removed(p) => engine
                                        .remove_by_path(p)
                                        .await
                                        .map(|n| format!("removed {n} chunks for {}", p.display())),
                                };
                                match result {
                                    Ok(msg) => {
                                        metrics
                                            .watcher_events_processed
                                            .fetch_add(1, Ordering::Relaxed);
                                        tracing::info!("auto-index: {msg}");
                                    }
                                    Err(e) => {
                                        metrics
                                            .watcher_events_failed
                                            .fetch_add(1, Ordering::Relaxed);
                                        tracing::warn!("auto-index failed: {e}");
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    "auto-index: no engine loaded for project {project_id}, skipping"
                                );
                            }
                        } else if is_image {
                            // Re-load config per event so watcher picks up config edits
                            let cfg = match AppConfig::load(
                                &project_root_owned,
                                RuntimeOverrides::default(),
                            ) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!("auto-index (image): config reload failed: {e}");
                                    continue;
                                }
                            };

                            #[cfg(feature = "image-embeddings")]
                            {
                                let result = match &event {
                                    the_one_memory::watcher::WatchEvent::Upserted(p) => {
                                        image_ingest_standalone(&project_id, p, None, &cfg)
                                            .await
                                            .map(|resp| {
                                                format!(
                                                    "reindexed image {} (dims={}, ocr={}, thumb={})",
                                                    resp.path,
                                                    resp.dims,
                                                    resp.ocr_extracted,
                                                    resp.thumbnail_generated
                                                )
                                            })
                                    }
                                    the_one_memory::watcher::WatchEvent::Removed(p) => {
                                        image_remove_standalone(
                                            &project_id,
                                            &p.to_string_lossy(),
                                            &cfg,
                                        )
                                        .await
                                        .map(|()| {
                                            format!("removed image {}", p.display())
                                        })
                                    }
                                };
                                match result {
                                    Ok(msg) => tracing::info!("auto-index: {msg}"),
                                    Err(e) => tracing::warn!("auto-index (image) failed: {e}"),
                                }
                            }
                            #[cfg(not(feature = "image-embeddings"))]
                            {
                                let _ = cfg;
                                tracing::debug!(
                                    "auto-index: image event ignored — image-embeddings feature disabled"
                                );
                            }
                        }
                    }
                });
            }
            Err(e) => {
                tracing::warn!("failed to start file watcher for {project_id}: {e}");
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
        let is_new = !memories.contains_key(&key);
        if is_new {
            let engine = self.build_memory_engine(project_root, project_id)?;
            memories.insert(key.clone(), engine);
        }
        let memory = memories.get_mut(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;

        // Spawn watcher once when the engine is first created
        if is_new {
            self.maybe_spawn_watcher(project_root, project_id);
        }

        match memory.ingest_markdown_tree(docs_root).await {
            Ok(count) => Ok(count),
            Err(e) => {
                // Rebuild as local-only and retry (requires local-embeddings)
                #[cfg(feature = "local-embeddings")]
                {
                    let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
                    let fallback = MemoryEngine::new_local(
                        &config.embedding_model,
                        config.limits.max_chunk_tokens,
                    )
                    .map_err(CoreError::Embedding)?;
                    memories.insert(key.clone(), fallback);
                    let memory = memories.get_mut(&key).ok_or_else(|| {
                        CoreError::InvalidProjectConfig("project memory not indexed".to_string())
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
        let search_start = std::time::Instant::now();
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

        // Load score threshold from config (LightRAG-inspired improvement)
        let score_threshold = AppConfig::load(project_root, RuntimeOverrides::default())
            .map(|c| c.limits.search_score_threshold)
            .unwrap_or(0.0);

        let hits = if route.decision.requires_memory_search {
            let key = Self::project_memory_key(project_root, &project_id);
            let memories = self.memory_by_project.read().await;
            if let Some(memory) = memories.get(&key) {
                memory
                    .search(&EngineSearchRequest {
                        query,
                        top_k,
                        score_threshold,
                        mode: RetrievalMode::Hybrid,
                    })
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

        let search_latency_ms = search_start.elapsed().as_millis() as u64;
        self.metrics
            .memory_search_latency_ms_total
            .fetch_add(search_latency_ms, Ordering::Relaxed);

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
                Err(
                    "local embeddings not available (built without local-embeddings feature)"
                        .into(),
                )
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
        let memory_search_calls = self.metrics.memory_search_calls.load(Ordering::Relaxed);
        let memory_search_latency_ms_total = self
            .metrics
            .memory_search_latency_ms_total
            .load(Ordering::Relaxed);
        let memory_search_latency_avg_ms = if memory_search_calls > 0 {
            memory_search_latency_ms_total / memory_search_calls
        } else {
            0
        };
        MetricsSnapshotResponse {
            project_init_calls: self.metrics.project_init_calls.load(Ordering::Relaxed),
            project_refresh_calls: self.metrics.project_refresh_calls.load(Ordering::Relaxed),
            memory_search_calls,
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
            // v0.12.0: observability deep dive
            memory_search_latency_ms_total,
            memory_search_latency_avg_ms,
            image_search_calls: self.metrics.image_search_calls.load(Ordering::Relaxed),
            image_ingest_calls: self.metrics.image_ingest_calls.load(Ordering::Relaxed),
            resources_list_calls: self.metrics.resources_list_calls.load(Ordering::Relaxed),
            resources_read_calls: self.metrics.resources_read_calls.load(Ordering::Relaxed),
            watcher_events_processed: self
                .metrics
                .watcher_events_processed
                .load(Ordering::Relaxed),
            watcher_events_failed: self.metrics.watcher_events_failed.load(Ordering::Relaxed),
            qdrant_errors: self.metrics.qdrant_errors.load(Ordering::Relaxed),
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

    /// List all available embedding models.
    pub fn models_list(&self, filter: Option<&str>) -> serde_json::Value {
        use the_one_memory::models_registry;

        let mut result = serde_json::json!({});

        let include_local = matches!(
            filter,
            None | Some("local") | Some("installer") | Some("multilingual")
        );
        let include_api = matches!(filter, None | Some("api"));

        if include_local {
            let models = match filter {
                Some("installer") => models_registry::list_installer_models(),
                Some("multilingual") => models_registry::list_local_models()
                    .into_iter()
                    .filter(|m| m.multilingual)
                    .collect(),
                _ => models_registry::list_local_models(),
            };
            result["local_models"] = serde_json::to_value(&models).unwrap_or_default();
            let default = models_registry::default_local_model();
            result["default_local_model"] = serde_json::json!(default.name);
        }

        if include_api {
            let providers = models_registry::list_api_providers();
            result["api_providers"] = serde_json::to_value(&providers).unwrap_or_default();
        }

        result
    }

    /// Check for model registry updates (stub — returns current versions).
    pub fn models_check_updates(&self) -> serde_json::Value {
        use the_one_memory::models_registry;

        serde_json::json!({
            "fastembed_crate_version": models_registry::fastembed_crate_version(),
            "local_model_count": models_registry::list_local_models().len(),
            "api_provider_count": models_registry::list_api_providers().len(),
            "message": "To update models, run: scripts/update-local-models.sh and scripts/update-api-models.sh"
        })
    }

    // -----------------------------------------------------------------------
    // Image search and ingest
    // -----------------------------------------------------------------------

    #[instrument(skip_all)]
    pub async fn image_search(
        &self,
        request: crate::api::ImageSearchRequest,
    ) -> Result<crate::api::ImageSearchResponse, CoreError> {
        self.metrics
            .image_search_calls
            .fetch_add(1, Ordering::Relaxed);
        // Validate mutual exclusion regardless of feature flag.
        match (&request.query, &request.image_base64) {
            (Some(_), Some(_)) => {
                return Err(CoreError::InvalidRequest(
                    "provide exactly one of query or image_base64, not both".to_string(),
                ))
            }
            (None, None) => {
                return Err(CoreError::InvalidRequest(
                    "must provide either query or image_base64".to_string(),
                ))
            }
            _ => {}
        }

        #[cfg(feature = "image-embeddings")]
        {
            self._image_search_impl(request).await
        }
        #[cfg(not(feature = "image-embeddings"))]
        {
            let _ = request;
            Err(CoreError::NotEnabled(
                "image-embeddings feature not compiled in".to_string(),
            ))
        }
    }

    #[cfg(feature = "image-embeddings")]
    async fn _image_search_impl(
        &self,
        request: crate::api::ImageSearchRequest,
    ) -> Result<crate::api::ImageSearchResponse, CoreError> {
        use base64::Engine as _;
        use the_one_memory::image_embeddings::ImageEmbeddingProvider as _;

        // Mutual exclusion already validated in the outer image_search.
        let project_root = PathBuf::from(&request.project_root);
        let config = AppConfig::load(&project_root, RuntimeOverrides::default())?;

        if !config.image_embedding_enabled {
            return Err(CoreError::NotEnabled(
                "image embeddings not enabled".to_string(),
            ));
        }

        let top_k = request
            .top_k
            .min(config.limits.max_image_search_hits)
            .max(1);
        let threshold = config.limits.image_search_score_threshold;

        // Produce a query vector — either from text (dual-encoder text→image space)
        // or from a base64-encoded image (image→image space).
        let query_vector = if let Some(ref text) = request.query {
            // Build text embedding provider to embed the text query into image space.
            // Nomic text embeddings are in the same vector space as Nomic vision embeddings.
            let text_provider =
                Self::try_build_embedding_provider(&config).map_err(CoreError::Embedding)?;
            text_provider
                .embed_single(text)
                .await
                .map_err(CoreError::Embedding)?
        } else {
            // image_base64 path: decode → validate → write to temp file → embed
            let b64 = request.image_base64.as_deref().unwrap();

            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| CoreError::InvalidRequest(format!("invalid base64 encoding: {e}")))?;

            // Validate size
            if bytes.len() > config.limits.max_image_size_bytes {
                return Err(CoreError::InvalidRequest(
                    "image exceeds max_image_size_bytes limit".to_string(),
                ));
            }

            // Validate image format
            let fmt = image::guess_format(&bytes).map_err(|_| {
                CoreError::InvalidRequest(
                    "unsupported image format, expected PNG/JPEG/WebP".to_string(),
                )
            })?;
            match fmt {
                image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP => {}
                _ => {
                    return Err(CoreError::InvalidRequest(
                        "unsupported image format, expected PNG/JPEG/WebP".to_string(),
                    ))
                }
            }

            // Write to temp file so FastEmbedImageProvider can read it from disk
            let mut tmp = tempfile::NamedTempFile::new()
                .map_err(|e| CoreError::Embedding(format!("temp file creation failed: {e}")))?;
            std::io::Write::write_all(&mut tmp, &bytes)
                .map_err(|e| CoreError::Embedding(format!("temp file write failed: {e}")))?;

            let image_provider =
                the_one_memory::image_embeddings::FastEmbedImageProvider::new("default")
                    .map_err(CoreError::Embedding)?;
            image_provider
                .embed_image(tmp.path())
                .await
                .map_err(CoreError::Embedding)?
        };

        // Build Qdrant backend (project-independent — we use the same client)
        let qdrant = the_one_memory::qdrant::AsyncQdrantBackend::new(
            &config.qdrant_url,
            "images", // we need any valid project_id for the client — unused for image methods
            the_one_memory::qdrant::QdrantOptions {
                api_key: config.qdrant_api_key.clone(),
                ca_cert_path: config
                    .qdrant_ca_cert_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                tls_insecure: config.qdrant_tls_insecure,
            },
        )
        .map_err(CoreError::Embedding)?;

        let results = qdrant
            .search_images(&request.project_id, query_vector, top_k, threshold)
            .await
            .map_err(CoreError::Embedding)?;

        let hits = results
            .into_iter()
            .map(|r| crate::api::ImageSearchHit {
                id: r.id,
                source_path: r.source_path,
                thumbnail_path: r.thumbnail_path,
                caption: r.caption,
                ocr_text: r.ocr_text,
                score: r.score,
            })
            .collect();

        Ok(crate::api::ImageSearchResponse { hits })
    }

    #[instrument(skip_all)]
    pub async fn image_ingest(
        &self,
        request: crate::api::ImageIngestRequest,
    ) -> Result<crate::api::ImageIngestResponse, CoreError> {
        self.metrics
            .image_ingest_calls
            .fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "image-embeddings")]
        {
            self._image_ingest_impl(request).await
        }
        #[cfg(not(feature = "image-embeddings"))]
        {
            let _ = request;
            Err(CoreError::NotEnabled(
                "image-embeddings feature not compiled in".to_string(),
            ))
        }
    }

    #[cfg(feature = "image-embeddings")]
    async fn _image_ingest_impl(
        &self,
        request: crate::api::ImageIngestRequest,
    ) -> Result<crate::api::ImageIngestResponse, CoreError> {
        let project_root = PathBuf::from(&request.project_root);
        let config = AppConfig::load(&project_root, RuntimeOverrides::default())?;

        let image_path = if std::path::Path::new(&request.path).is_absolute() {
            PathBuf::from(&request.path)
        } else {
            project_root.join(&request.path)
        };

        image_ingest_standalone(&request.project_id, &image_path, request.caption, &config).await
    }

    // -----------------------------------------------------------------------
    // Image maintenance: rescan, clear, delete
    // -----------------------------------------------------------------------

    #[instrument(skip_all)]
    pub async fn image_rescan(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        #[cfg(feature = "image-embeddings")]
        {
            self._image_rescan_impl(project_root, project_id).await
        }
        #[cfg(not(feature = "image-embeddings"))]
        {
            let _ = (project_root, project_id);
            Err(CoreError::NotEnabled(
                "image-embeddings feature not compiled in".to_string(),
            ))
        }
    }

    #[cfg(feature = "image-embeddings")]
    async fn _image_rescan_impl(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        use the_one_memory::image_ingest::{discover_images, DEFAULT_IMAGE_EXTENSIONS};

        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;

        if !config.image_embedding_enabled {
            return Err(CoreError::NotEnabled(
                "image embeddings not enabled".to_string(),
            ));
        }

        // Walk project root for images
        let images = discover_images(project_root, DEFAULT_IMAGE_EXTENSIONS);
        let total = images.len();
        let mut ingested = 0usize;

        for img in images {
            if img.size_bytes as usize > config.limits.max_image_size_bytes {
                continue;
            }
            let result = self
                .image_ingest(crate::api::ImageIngestRequest {
                    project_root: project_root.to_string_lossy().to_string(),
                    project_id: project_id.to_string(),
                    path: img.path.to_string_lossy().to_string(),
                    caption: None,
                })
                .await;
            if result.is_ok() {
                ingested += 1;
            }
            if ingested >= config.limits.max_images_per_project {
                break;
            }
        }

        Ok(serde_json::json!({
            "discovered": total,
            "ingested": ingested
        }))
    }

    #[instrument(skip_all)]
    pub async fn image_clear(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;

        let qdrant = the_one_memory::qdrant::AsyncQdrantBackend::new(
            &config.qdrant_url,
            "images",
            the_one_memory::qdrant::QdrantOptions {
                api_key: config.qdrant_api_key.clone(),
                ca_cert_path: config
                    .qdrant_ca_cert_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                tls_insecure: config.qdrant_tls_insecure,
            },
        )
        .map_err(CoreError::Embedding)?;

        qdrant
            .delete_image_collection(project_id)
            .await
            .map_err(CoreError::Embedding)?;

        Ok(serde_json::json!({ "cleared": true }))
    }

    #[instrument(skip_all)]
    pub async fn image_delete(
        &self,
        project_root: &Path,
        project_id: &str,
        source_path: &str,
    ) -> Result<serde_json::Value, CoreError> {
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        image_remove_standalone(project_id, source_path, &config).await?;
        Ok(serde_json::json!({ "deleted": true, "path": source_path }))
    }

    // -----------------------------------------------------------------------
    // MCP Resources API (v0.10.0)
    // -----------------------------------------------------------------------

    /// List all resources available for a project. See `crate::resources`
    /// for the `the-one://` URI scheme and what is exposed.
    #[instrument(skip_all)]
    pub async fn resources_list(
        &self,
        project_root: &Path,
        _project_id: &str,
    ) -> Result<crate::resources::ResourcesListResponse, CoreError> {
        self.metrics
            .resources_list_calls
            .fetch_add(1, Ordering::Relaxed);
        let resources = crate::resources::list_resources(project_root)?;
        Ok(crate::resources::ResourcesListResponse { resources })
    }

    /// Read a single resource by URI. Rejects path-traversal on `docs` URIs.
    #[instrument(skip_all)]
    pub async fn resources_read(
        &self,
        project_root: &Path,
        _project_id: &str,
        uri: &str,
    ) -> Result<crate::resources::ResourcesReadResponse, CoreError> {
        self.metrics
            .resources_read_calls
            .fetch_add(1, Ordering::Relaxed);
        crate::resources::read_resource(project_root, uri)
    }

    // -----------------------------------------------------------------------
    // Backup / Restore API (v0.12.0, Task 3.3)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Graph RAG extraction + stats (v0.13.0, Task 12/9)
    // -----------------------------------------------------------------------

    /// Trigger entity/relation extraction on the project's currently-indexed
    /// chunks. Requires `THE_ONE_GRAPH_ENABLED=true` and `THE_ONE_GRAPH_BASE_URL`
    /// pointing at an OpenAI-compatible chat completions endpoint (Ollama,
    /// LM Studio, LiteLLM, etc.).
    ///
    /// See `docs/guides/graph-rag.md` for the full env var set.
    #[instrument(skip_all)]
    pub async fn graph_extract(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<the_one_memory::graph_extractor::GraphExtractResult, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        let memories = self.memory_by_project.read().await;
        let engine = memories.get(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig(
                "project memory not indexed — run setup.refresh first".to_string(),
            )
        })?;
        let chunks = engine.chunks().to_vec();
        drop(memories);

        let result = the_one_memory::graph_extractor::extract_and_persist(project_root, &chunks)
            .await
            .map_err(CoreError::Embedding)?;

        // Reload the graph into the memory engine so `Local`/`Global`/`Hybrid`
        // retrieval modes can immediately use the new entities without a
        // server restart.
        let graph_path = project_root.join(".the-one").join("knowledge_graph.json");
        if graph_path.exists() {
            if let Ok(graph) = the_one_memory::graph::KnowledgeGraph::load_from_file(&graph_path) {
                let mut memories = self.memory_by_project.write().await;
                if let Some(engine) = memories.get_mut(&key) {
                    *engine.graph_mut() = graph;
                }
            }
        }

        Ok(result)
    }

    /// Return summary statistics for the project's knowledge graph.
    /// Reads from `<project>/.the-one/knowledge_graph.json` if present,
    /// otherwise returns zeros. Never fails.
    #[instrument(skip_all)]
    pub async fn graph_stats(&self, project_root: &Path, _project_id: &str) -> serde_json::Value {
        let graph_path = project_root.join(".the-one").join("knowledge_graph.json");
        let Ok(graph) = the_one_memory::graph::KnowledgeGraph::load_from_file(&graph_path) else {
            return serde_json::json!({
                "entity_count": 0,
                "relation_count": 0,
                "graph_enabled": the_one_memory::graph_extractor::is_graph_enabled(),
                "extraction_configured": std::env::var("THE_ONE_GRAPH_BASE_URL").is_ok(),
                "file_exists": false,
            });
        };
        serde_json::json!({
            "entity_count": graph.entity_count(),
            "relation_count": graph.relation_count(),
            "graph_enabled": the_one_memory::graph_extractor::is_graph_enabled(),
            "extraction_configured": std::env::var("THE_ONE_GRAPH_BASE_URL").is_ok(),
            "file_exists": true,
        })
    }

    /// Create a gzipped tar backup of the project's `.the-one/` state plus
    /// shared catalog/registry under `~/.the-one/`. See [`crate::backup`] for
    /// details on exclusions (`.fastembed_cache/`, Qdrant wal/raft state).
    #[instrument(skip_all)]
    pub async fn backup_project(
        &self,
        request: crate::api::BackupRequest,
    ) -> Result<crate::api::BackupResponse, CoreError> {
        // Backup is disk-heavy — run on the blocking pool.
        let req = request;
        tokio::task::spawn_blocking(move || crate::backup::create_backup(&req))
            .await
            .map_err(|e| CoreError::InvalidProjectConfig(format!("join error: {e}")))?
            .map_err(CoreError::InvalidProjectConfig)
    }

    /// Restore a project from a backup tarball previously produced by
    /// [`Self::backup_project`]. Rejects unsafe archive paths and refuses to
    /// overwrite existing state unless `overwrite_existing` is true.
    #[instrument(skip_all)]
    pub async fn restore_project(
        &self,
        request: crate::api::RestoreRequest,
    ) -> Result<crate::api::RestoreResponse, CoreError> {
        let req = request;
        tokio::task::spawn_blocking(move || crate::backup::restore_backup(&req))
            .await
            .map_err(|e| CoreError::InvalidProjectConfig(format!("join error: {e}")))?
            .map_err(CoreError::InvalidProjectConfig)
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

// ---------------------------------------------------------------------------
// Standalone image ingest / remove helpers
//
// These functions are extracted from `McpBroker::_image_ingest_impl` and
// `McpBroker::image_delete` so they can be called from contexts that don't
// have a `&McpBroker` — specifically the file-watcher task spawned by
// `maybe_spawn_watcher`, which only carries owned clones of the data it
// needs (no `self`).
//
// The broker methods now delegate to these helpers so there is a single
// implementation of the image ingest/remove pipeline.
// ---------------------------------------------------------------------------

/// Ingest a single image file — embed, OCR (if enabled), thumbnail (if
/// enabled), and upsert into the project's Qdrant image collection.
///
/// This is a free function so it can be called from a spawned tokio task
/// that doesn't have access to the broker. The caller must provide the
/// loaded `AppConfig` for the project.
#[cfg(feature = "image-embeddings")]
pub(crate) async fn image_ingest_standalone(
    project_id: &str,
    image_path: &Path,
    caption: Option<String>,
    config: &AppConfig,
) -> Result<crate::api::ImageIngestResponse, CoreError> {
    use the_one_memory::image_embeddings::{FastEmbedImageProvider, ImageEmbeddingProvider};
    use the_one_memory::image_ingest::DEFAULT_IMAGE_EXTENSIONS;
    use the_one_memory::qdrant::{AsyncQdrantBackend, ImagePoint, QdrantOptions};

    if !config.image_embedding_enabled {
        return Err(CoreError::NotEnabled(
            "image embeddings not enabled".to_string(),
        ));
    }

    if !image_path.exists() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "image path does not exist: {}",
            image_path.display()
        )));
    }

    // Check extension
    let ext = image_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !DEFAULT_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(CoreError::InvalidProjectConfig(format!(
            "unsupported image extension: .{ext}"
        )));
    }

    // Check file size
    let metadata = std::fs::metadata(image_path)?;
    let file_size = metadata.len();
    if file_size as usize > config.limits.max_image_size_bytes {
        return Err(CoreError::InvalidProjectConfig(format!(
            "image file too large: {} bytes (limit: {})",
            file_size, config.limits.max_image_size_bytes
        )));
    }

    // SHA-256 hash for deduplication
    let file_bytes = std::fs::read(image_path)?;
    let hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&file_bytes);
        let result = hasher.finalize();
        result
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };

    let mtime_epoch = metadata
        .modified()
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs() as i64)
        })
        .unwrap_or(0);

    // Build image embedding provider (ONNX model load happens here)
    let provider = tokio::task::spawn_blocking({
        let model_name = config.image_embedding_model.clone();
        move || FastEmbedImageProvider::new(&model_name)
    })
    .await
    .map_err(|e| CoreError::Embedding(format!("join error: {e}")))?
    .map_err(CoreError::Embedding)?;

    let dims = provider.dimensions();
    let vector = provider
        .embed_image(image_path)
        .await
        .map_err(CoreError::Embedding)?;

    // OCR extraction (feature-gated)
    let ocr_text: Option<String>;
    #[cfg(feature = "image-ocr")]
    {
        ocr_text = if config.image_ocr_enabled {
            match the_one_memory::ocr::extract_text(image_path, &config.image_ocr_language) {
                Ok(text) if !text.trim().is_empty() => Some(text),
                _ => None,
            }
        } else {
            None
        };
    }
    #[cfg(not(feature = "image-ocr"))]
    {
        ocr_text = None;
    }
    let ocr_extracted = ocr_text.is_some();

    // Thumbnail generation
    let thumbnails_dir = config.project_state_dir.join("thumbnails");
    std::fs::create_dir_all(&thumbnails_dir)?;
    let thumb_file = thumbnails_dir.join(format!("{hash}.webp"));

    let (thumbnail_path, thumbnail_generated) = if config.image_thumbnail_enabled {
        match the_one_memory::thumbnail::generate_thumbnail(
            image_path,
            &thumb_file,
            config.image_thumbnail_max_px,
        ) {
            Ok(()) => (Some(thumb_file.to_string_lossy().to_string()), true),
            Err(e) => {
                tracing::warn!("thumbnail generation failed for {:?}: {e}", image_path);
                (None, false)
            }
        }
    } else {
        (None, false)
    };

    // Qdrant upsert
    let qdrant = AsyncQdrantBackend::new(
        &config.qdrant_url,
        "images",
        QdrantOptions {
            api_key: config.qdrant_api_key.clone(),
            ca_cert_path: config
                .qdrant_ca_cert_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            tls_insecure: config.qdrant_tls_insecure,
        },
    )
    .map_err(CoreError::Embedding)?;

    qdrant
        .create_image_collection(project_id, dims)
        .await
        .map_err(CoreError::Embedding)?;

    let point = ImagePoint {
        id: hash,
        vector,
        source_path: image_path.to_string_lossy().to_string(),
        file_size,
        mtime_epoch,
        caption,
        ocr_text,
        thumbnail_path,
    };

    qdrant
        .upsert_image_points(project_id, vec![point])
        .await
        .map_err(CoreError::Embedding)?;

    Ok(crate::api::ImageIngestResponse {
        path: image_path.to_string_lossy().to_string(),
        dims,
        ocr_extracted,
        thumbnail_generated,
    })
}

/// Remove a single image from the project's Qdrant image collection by
/// its source path. Used by the file watcher on `Removed` events and by
/// the `image_delete` API.
pub(crate) async fn image_remove_standalone(
    project_id: &str,
    source_path: &str,
    config: &AppConfig,
) -> Result<(), CoreError> {
    let qdrant = the_one_memory::qdrant::AsyncQdrantBackend::new(
        &config.qdrant_url,
        "images",
        the_one_memory::qdrant::QdrantOptions {
            api_key: config.qdrant_api_key.clone(),
            ca_cert_path: config
                .qdrant_ca_cert_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            tls_insecure: config.qdrant_tls_insecure,
        },
    )
    .map_err(CoreError::Embedding)?;

    qdrant
        .delete_image_by_source_path(project_id, source_path)
        .await
        .map_err(CoreError::Embedding)?;

    Ok(())
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
        DocsTrashListRequest, DocsTrashRestoreRequest, DocsUpdateRequest, ImageSearchRequest,
        MemorySearchRequest, ProjectInitRequest, ProjectProfileGetRequest, ProjectRefreshRequest,
        ToolEnableRequest, ToolRunRequest, ToolSuggestRequest,
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
    async fn test_models_list_returns_local_and_api() {
        let broker = McpBroker::new();
        let result = broker.models_list(None);
        assert!(result["local_models"].is_array());
        assert!(result["api_providers"].is_array());
        assert!(result["default_local_model"].is_string());
        assert_eq!(result["default_local_model"], "BGE-large-en-v1.5");
    }

    #[tokio::test]
    async fn test_models_list_filter_installer() {
        let broker = McpBroker::new();
        let result = broker.models_list(Some("installer"));
        let models = result["local_models"].as_array().unwrap();
        assert_eq!(models.len(), 7);
    }

    #[tokio::test]
    async fn test_models_list_filter_multilingual() {
        let broker = McpBroker::new();
        let result = broker.models_list(Some("multilingual"));
        let models = result["local_models"].as_array().unwrap();
        for model in models {
            assert_eq!(model["multilingual"], true);
        }
    }

    #[tokio::test]
    async fn test_models_check_updates() {
        let broker = McpBroker::new();
        let result = broker.models_check_updates();
        assert!(result["fastembed_crate_version"].is_string());
        assert!(result["local_model_count"].as_u64().unwrap() >= 10);
        assert_eq!(result["api_provider_count"].as_u64().unwrap(), 3);
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

    #[tokio::test]
    async fn test_image_search_rejects_both_query_and_base64() {
        let broker = McpBroker::new();
        let req = ImageSearchRequest {
            project_root: "/tmp/nonexistent".to_string(),
            project_id: "test".to_string(),
            query: Some("hello".to_string()),
            image_base64: Some("aGVsbG8=".to_string()),
            top_k: 5,
        };
        let result = broker.image_search(req).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("exactly one") || err.contains("mutually exclusive"),
            "expected exclusive error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_image_search_rejects_neither_query_nor_base64() {
        let broker = McpBroker::new();
        let req = ImageSearchRequest {
            project_root: "/tmp/nonexistent".to_string(),
            project_id: "test".to_string(),
            query: None,
            image_base64: None,
            top_k: 5,
        };
        let result = broker.image_search(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_image_search_rejects_invalid_base64() {
        let broker = McpBroker::new();
        let req = ImageSearchRequest {
            project_root: "/tmp/nonexistent".to_string(),
            project_id: "test".to_string(),
            query: None,
            image_base64: Some("!!!invalid!!!".to_string()),
            top_k: 5,
        };
        let result = broker.image_search(req).await;
        assert!(result.is_err());
    }

    /// Integration test: watcher detects a file write and auto-reindexes it.
    ///
    /// The watcher runs in a background task with a debounce window so this
    /// test polls the engine state.  Marked `#[ignore]` because it is
    /// timing-sensitive (inotify debounce) and can be flaky in heavily-loaded
    /// CI containers.  Run manually with:
    ///   `cargo test -p the-one-mcp watcher_auto_reindex -- --ignored --nocapture`
    #[ignore]
    #[tokio::test]
    async fn test_watcher_auto_reindex_on_file_change() {
        // Uses the default embedding model (all-MiniLM-L6-v2 via env or fallback)
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let docs_dir = root.join(".the-one").join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir");

        // Write config.json enabling auto-indexing with a short debounce
        let config_path = root.join(".the-one").join("config.json");
        fs::write(
            &config_path,
            r#"{"auto_index_enabled":true,"auto_index_debounce_ms":200}"#,
        )
        .expect("config write");

        // Ingest an initial file — this also spawns the watcher
        let initial_doc = docs_dir.join("notes.md");
        fs::write(&initial_doc, "# Initial\nOriginal content.").expect("initial doc");

        let broker = McpBroker::new();
        broker
            .ingest_docs(&root, "watcher-test", &docs_dir)
            .await
            .expect("initial ingest");

        // Confirm initial content is indexed
        {
            let memories = broker.memory_by_project.read().await;
            let key = McpBroker::project_memory_key(&root, "watcher-test");
            let engine = memories.get(&key).expect("engine should exist");
            let all: String = engine.docs_list().join("");
            assert!(all.contains("notes.md"), "notes.md should be indexed");
        }

        // Overwrite the file with new content — the watcher should pick it up
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        fs::write(&initial_doc, "# Updated\nCompletely new content.").expect("updated doc");

        // Poll for up to 5 seconds until the new content appears
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let memories = broker.memory_by_project.read().await;
            let key = McpBroker::project_memory_key(&root, "watcher-test");
            if let Some(engine) = memories.get(&key) {
                let all_content: String = engine
                    .docs_list()
                    .iter()
                    .flat_map(|p| engine.docs_get(p))
                    .collect();
                if all_content.contains("Completely new content") {
                    // Success — also verify old content is gone
                    assert!(
                        !all_content.contains("Original content"),
                        "old content should be replaced"
                    );
                    return;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("watcher did not re-index the changed file within 5 seconds");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Task 1.1 (v0.8.2): Image auto-reindex helpers
    // -----------------------------------------------------------------------

    /// Unit test: the standalone ingest helper fast-fails with `NotEnabled`
    /// when `image_embedding_enabled = false` (the default). Exercises the
    /// extraction without requiring Qdrant or an ONNX model.
    #[tokio::test]
    #[cfg(feature = "image-embeddings")]
    async fn test_image_ingest_standalone_rejects_when_disabled() {
        use the_one_core::config::{AppConfig, RuntimeOverrides};

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        fs::create_dir_all(root.join(".the-one")).expect("dotdir");
        // image_embedding_enabled defaults to false — do not override
        fs::write(
            root.join(".the-one").join("config.json"),
            r#"{"auto_index_enabled":false}"#,
        )
        .expect("config write");

        let config = AppConfig::load(&root, RuntimeOverrides::default()).expect("config load");
        assert!(!config.image_embedding_enabled);

        let fake_image = temp.path().join("nope.png");
        let result = super::image_ingest_standalone("proj", &fake_image, None, &config).await;
        assert!(result.is_err(), "expected error when disabled");
        let err = result.unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("not enabled")
                || err.to_lowercase().contains("image embeddings"),
            "expected NotEnabled error, got: {err}"
        );
    }

    /// Unit test: the standalone ingest helper rejects a path that does not
    /// exist on disk, even when image embeddings are enabled in config.
    ///
    /// Uses a config with `image_embedding_enabled = true` so we get past
    /// the NotEnabled guard and into the existence check.
    #[tokio::test]
    #[cfg(feature = "image-embeddings")]
    async fn test_image_ingest_standalone_rejects_missing_path() {
        use the_one_core::config::{AppConfig, RuntimeOverrides};

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        fs::create_dir_all(root.join(".the-one")).expect("dotdir");
        fs::write(
            root.join(".the-one").join("config.json"),
            r#"{"image_embedding_enabled":true}"#,
        )
        .expect("config write");

        let config = AppConfig::load(&root, RuntimeOverrides::default()).expect("config load");
        assert!(config.image_embedding_enabled);

        let missing = temp.path().join("definitely_does_not_exist.png");
        let result = super::image_ingest_standalone("proj", &missing, None, &config).await;
        assert!(result.is_err(), "expected error for missing path");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("does not exist") || err.contains("image path"),
            "expected missing-path error, got: {err}"
        );
    }

    /// Integration test: watcher auto-reindexes image file changes.
    ///
    /// Like `test_watcher_auto_reindex_on_file_change`, this is marked
    /// `#[ignore]` because it requires:
    ///   - A running Qdrant instance at the configured URL
    ///   - Downloading the image embedding ONNX model (~200MB, first run)
    ///   - Timing tolerance for inotify debounce
    ///
    /// Run manually with:
    ///   `cargo test -p the-one-mcp watcher_auto_reindex_image -- --ignored --nocapture`
    #[ignore]
    #[tokio::test]
    #[cfg(feature = "image-embeddings")]
    async fn test_watcher_auto_reindex_image_upsert() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let images_dir = root.join(".the-one").join("images");
        fs::create_dir_all(&images_dir).expect("images dir");

        let config_path = root.join(".the-one").join("config.json");
        fs::write(
            &config_path,
            r#"{"auto_index_enabled":true,"auto_index_debounce_ms":200,"image_embedding_enabled":true,"image_thumbnail_enabled":false}"#,
        )
        .expect("config write");

        // Create a minimal valid PNG (1x1 red pixel) so the embedder accepts it.
        // This is a real 67-byte PNG file header + IHDR + IDAT + IEND.
        let png_1x1_red: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + tag
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // bit depth / color / CRC
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT length + tag
            0x08, 0x99, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x5C, 0xCD,
            0xFF, 0x69, // CRC
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82, // IEND
        ];

        // Ingest an initial markdown file to spawn the watcher via ingest_docs.
        let docs_dir = root.join(".the-one").join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir");
        fs::write(docs_dir.join("seed.md"), "# Seed\n").expect("seed md");

        let broker = McpBroker::new();
        broker
            .ingest_docs(&root, "watcher-img-test", &docs_dir)
            .await
            .expect("initial ingest");

        // Give the watcher a moment to spin up, then drop an image into the watched dir.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let img_path = images_dir.join("pixel.png");
        fs::write(&img_path, png_1x1_red).expect("png write");

        // Poll the Qdrant image collection for up to 30s (first run downloads ONNX).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // Search the image collection for our path — if present, the
            // watcher completed the ingest.
            let search = broker
                .image_search(crate::api::ImageSearchRequest {
                    project_root: root.display().to_string(),
                    project_id: "watcher-img-test".to_string(),
                    query: Some("red pixel".to_string()),
                    image_base64: None,
                    top_k: 5,
                })
                .await;
            if let Ok(resp) = search {
                if resp
                    .hits
                    .iter()
                    .any(|h| h.source_path.contains("pixel.png"))
                {
                    return;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("watcher did not re-index the image within 30 seconds");
            }
        }
    }
}
