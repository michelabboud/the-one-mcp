use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::instrument;

use the_one_core::config::{
    update_project_config, AppConfig, NanoProviderKind, ProjectConfigUpdate, RuntimeOverrides,
};
use the_one_core::contracts::{
    AaakCompressionResult, AaakLesson, AaakPattern, AaakTeachOutcome, ApprovalScope, Capability,
    DiaryEntry, DiarySummary, MemoryNavigationNode, MemoryNavigationNodeKind,
    MemoryNavigationTunnel, RiskLevel,
};
use the_one_core::docs_manager::DocsManager;
use the_one_core::error::CoreError;
use the_one_core::manifests::{
    load_overrides_manifest, project_state_paths, save_overrides_manifest, MANIFEST_SCHEMA_VERSION,
};
use the_one_core::policy::PolicyEngine;
use the_one_core::project::{project_init, project_refresh, RefreshMode};
use the_one_core::state_store::StateStore;
use the_one_core::storage::sqlite::ProjectDatabase;
use the_one_memory::conversation::{
    AaakCompressionArtifact, ConversationFormat, ConversationTranscript,
};
use the_one_memory::palace::PalaceMetadata;
use the_one_memory::qdrant::QdrantOptions;
#[cfg(feature = "local-embeddings")]
use the_one_memory::reranker::Reranker;
#[cfg(all(feature = "local-embeddings", feature = "redis-vectors"))]
use the_one_memory::RedisEngineConfig;
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
    DocsUpdateRequest, DocsUpdateResponse, MemoryAaakCompressRequest, MemoryAaakCompressResponse,
    MemoryAaakListLessonsRequest, MemoryAaakListLessonsResponse, MemoryAaakTeachRequest,
    MemoryAaakTeachResponse, MemoryCaptureHookRequest, MemoryCaptureHookResponse,
    MemoryConversationFormat, MemoryDiaryAddRequest, MemoryDiaryAddResponse,
    MemoryDiaryListRequest, MemoryDiaryListResponse, MemoryDiarySearchRequest,
    MemoryDiarySearchResponse, MemoryDiarySummarizeRequest, MemoryDiarySummarizeResponse,
    MemoryFetchChunkRequest, MemoryFetchChunkResponse, MemoryIngestConversationRequest,
    MemoryIngestConversationResponse, MemoryNavigationLinkTunnelRequest,
    MemoryNavigationLinkTunnelResponse, MemoryNavigationListRequest, MemoryNavigationListResponse,
    MemoryNavigationTraverseRequest, MemoryNavigationTraverseResponse,
    MemoryNavigationUpsertNodeRequest, MemoryNavigationUpsertNodeResponse, MemorySearchItem,
    MemorySearchRequest, MemorySearchResponse, MemoryWakeUpRequest, MemoryWakeUpResponse,
    MetricsSnapshotResponse, ProjectInitRequest, ProjectInitResponse, ProjectProfileGetRequest,
    ProjectProfileGetResponse, ProjectRefreshRequest, ProjectRefreshResponse, ToolAddRequest,
    ToolAddResponse, ToolDisableRequest, ToolDisableResponse, ToolEnableRequest,
    ToolEnableResponse, ToolInfoRequest, ToolInstallRequest, ToolInstallResponse, ToolListRequest,
    ToolListResponse, ToolRemoveRequest, ToolRemoveResponse, ToolRunRequest, ToolRunResponse,
    ToolSearchRequest, ToolSearchResponse, ToolSuggestItem, ToolSuggestRequest,
    ToolSuggestResponse, ToolUpdateResponse,
};

const AAAK_MIN_OCCURRENCES: usize = 2;
const AAAK_MIN_CONFIDENCE_PERCENT: u8 = 70;
const AAAK_MAX_LESSONS_PER_BATCH: usize = 8;

/// Concrete type of the per-project state-store cache entry.
///
/// Each entry is an `Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>`:
///
/// - `Arc` so two concurrent handlers targeting the same `(project_root,
///   project_id)` can both clone a reference out of the outer `RwLock`
///   without re-acquiring it for the duration of their work.
/// - `std::sync::Mutex` (not `tokio::sync::Mutex`) is deliberate: it makes
///   the guard `!Send`, which means the compiler refuses any attempt to
///   hold a `StateStore` across an `.await` point. That restriction is
///   load-bearing for Phase 3+ — holding a Postgres/Redis connection-pool
///   checkout across an await is the #1 way to deadlock a backend pool
///   under load. We pay for the correctness with one extra closure-shaped
///   helper (`with_state_store`) and the rewrite of two broker handlers
///   that currently straddle awaits.
/// - `Box<dyn StateStore + Send>` so Phase 2+ can plug in Postgres/Redis
///   without touching the cache type.
type StateStoreCacheEntry = Arc<std::sync::Mutex<Box<dyn StateStore + Send>>>;

pub struct McpBroker {
    router: Router,
    registry: CapabilityRegistry,
    memory_by_project: Arc<RwLock<HashMap<String, MemoryEngine>>>,
    docs_by_project: RwLock<HashMap<String, DocsManager>>,
    /// Per-project `StateStore` cache keyed by `{canonical_root}::{project_id}`.
    /// Added in v0.16.0 Phase 1 — replaces the prior pattern of opening
    /// a fresh `ProjectDatabase` connection on every broker method call.
    /// See [`StateStoreCacheEntry`] for the concurrency rationale.
    state_by_project: RwLock<HashMap<String, StateStoreCacheEntry>>,
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
            state_by_project: RwLock::new(HashMap::new()),
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
            if let Err(err) = self.registry.save_to_path(path) {
                // Capability registry persistence is a side-effect of
                // in-memory mutation — we still want the broker to observe
                // the new capability, but failing silently here was how we
                // lost track of misconfigured state dirs. Emit a warning so
                // ops can see it in the log.
                tracing::warn!(
                    target: "the_one_mcp::registry",
                    path = %path.display(),
                    error = %err,
                    "capability registry save failed"
                );
            }
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

    // ─── StateStore cache (v0.16.0 Phase 1) ────────────────────────────
    //
    // `state_store_factory` / `get_or_init_state_store` / `with_state_store`
    // together replace the pattern of calling `ProjectDatabase::open(...)`
    // directly in broker methods. All three helpers are private; broker
    // methods exclusively use `with_state_store` to run a sync closure
    // against the cached store.

    /// Construct a fresh `StateStore` for the given project. Today this
    /// only knows how to open SQLite. Phase 2+ will branch on
    /// [`the_one_core::config::BackendSelection`] parsed from
    /// `THE_ONE_STATE_TYPE` / `THE_ONE_STATE_URL` env vars — at which point
    /// `&self` will be used to read the parsed selection that the broker
    /// computes once at construction. The `&self` parameter is kept in the
    /// signature today purely for that forward compatibility.
    fn state_store_factory(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<Box<dyn StateStore + Send>, CoreError> {
        let db = ProjectDatabase::open(project_root, project_id)?;
        Ok(Box::new(db))
    }

    /// Look up the cached state store for a project, constructing it on
    /// first access.
    ///
    /// Uses a read/write-upgrade pattern so that the fast path (cache hit)
    /// takes only a read lock on the outer `RwLock`, and the cold path
    /// constructs the new store **outside** the write lock — important for
    /// Phase 3+ when the factory becomes async (Postgres pool warm-up,
    /// Redis AOF verification). The double-check under the write lock
    /// handles the rare race where two concurrent callers both miss the
    /// cache; the loser drops its freshly-built store and returns the
    /// winner's entry.
    async fn get_or_init_state_store(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<StateStoreCacheEntry, CoreError> {
        let key = Self::project_memory_key(project_root, project_id);

        // Fast path: read lock, clone the Arc out, release.
        if let Some(existing) = self.state_by_project.read().await.get(&key) {
            return Ok(existing.clone());
        }

        // Cold path: construct outside the write lock so concurrent
        // cache-miss traffic for *different* projects is not serialized
        // through this factory call.
        let new_store = self.state_store_factory(project_root, project_id)?;
        let new_entry: StateStoreCacheEntry = Arc::new(std::sync::Mutex::new(new_store));

        // Double-check under the write lock: another task may have raced
        // us and populated the key while we were building.
        let mut map = self.state_by_project.write().await;
        if let Some(existing) = map.get(&key) {
            return Ok(existing.clone());
        }
        map.insert(key, new_entry.clone());
        Ok(new_entry)
    }

    /// Run a synchronous closure against the cached state store for a
    /// project. The closure receives `&dyn StateStore` — the same
    /// abstraction every future backend (Postgres, Redis, combined)
    /// implements — so this helper is the single chokepoint through which
    /// every broker method reaches persistent state.
    ///
    /// # Why the closure is synchronous
    ///
    /// The inner lock is a [`std::sync::Mutex`], not a tokio mutex. Its
    /// guard is `!Send`, so the compiler refuses any attempt to hold it
    /// across an `.await`. That restriction is deliberate: it prevents
    /// any broker handler from pinning a backend connection (or a
    /// Postgres/Redis pool checkout, in Phase 3+) across asynchronous
    /// work, which is the #1 way to deadlock a backend pool under load.
    ///
    /// Broker handlers that need to interleave state-store calls with
    /// async memory-engine work just invoke `with_state_store` multiple
    /// times around the `.await` boundary, rather than holding one
    /// connection for the whole request.
    async fn with_state_store<R>(
        &self,
        project_root: &Path,
        project_id: &str,
        f: impl FnOnce(&dyn StateStore) -> Result<R, CoreError>,
    ) -> Result<R, CoreError> {
        let entry = self
            .get_or_init_state_store(project_root, project_id)
            .await?;
        let guard = entry.lock().map_err(|_| {
            // Mutex poisoning means a previous closure panicked mid-call.
            // We surface this as InvalidProjectConfig — the project's
            // state store is now in an unknown state, and the operator
            // needs to investigate the earlier panic in the logs.
            CoreError::InvalidProjectConfig(format!(
                "state store mutex poisoned for project '{project_id}'; see prior panic in logs"
            ))
        })?;
        f(&**guard)
    }

    /// Drain the state-store cache, closing every cached backend.
    ///
    /// For SQLite, closing is implicit on `Drop` (the `rusqlite::Connection`
    /// releases its WAL lock). For Phase 3+ Postgres/Redis pools, this is
    /// the explicit teardown point where async `pool.close().await` will
    /// be called — reason the method is `async` today even though the
    /// SQLite-only body is sync.
    ///
    /// Tests and binary shutdown should call this before dropping the
    /// broker to guarantee pool cleanup ordering.
    pub async fn shutdown(&self) {
        let mut map = self.state_by_project.write().await;
        map.clear();
        // Phase 3+: iterate `drain()` and call `store.shutdown().await` on
        // each — requires adding a shutdown method to the `StateStore`
        // trait. Deferred until Postgres/Redis backends exist.
    }

    async fn build_memory_engine(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<MemoryEngine, CoreError> {
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;

        let max_chunk_tokens = config.limits.max_chunk_tokens;
        let vector_backend = config.vector_backend.to_ascii_lowercase();

        let mut engine = if vector_backend == "redis" {
            let redis_url = config.redis_url.as_deref().ok_or_else(|| {
                CoreError::InvalidProjectConfig(
                    "vector_backend 'redis' requires redis_url".to_string(),
                )
            })?;

            let redis_index_name = config
                .redis_index_name
                .clone()
                .unwrap_or_else(|| "the_one_memories".to_string());
            if redis_url.trim().is_empty() {
                return Err(CoreError::InvalidProjectConfig(
                    "vector_backend 'redis' requires a non-empty redis_url".to_string(),
                ));
            }
            if !redis_index_name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-'))
            {
                return Err(CoreError::InvalidProjectConfig(
                    "redis_index_name must use only ASCII letters, digits, ':', '_' or '-'"
                        .to_string(),
                ));
            }

            if config.embedding_provider == "api" {
                return Err(CoreError::InvalidProjectConfig(
                    "vector_backend 'redis' currently supports local embeddings only; set \
                     embedding_provider to 'local'"
                        .to_string(),
                ));
            }

            #[cfg(all(feature = "local-embeddings", feature = "redis-vectors"))]
            {
                let redis_engine_config = RedisEngineConfig {
                    redis_url: redis_url.to_string(),
                    index_name: redis_index_name.clone(),
                    persistence_required: config.redis_persistence_required,
                };
                MemoryEngine::new_with_redis(
                    &config.embedding_model,
                    max_chunk_tokens,
                    redis_engine_config,
                )
                .await
                .map_err(CoreError::Embedding)?
            }

            #[cfg(not(all(feature = "local-embeddings", feature = "redis-vectors")))]
            {
                return Err(CoreError::Embedding(
                    "redis backend selected but redis-vectors + local-embeddings features are \
                     not enabled in this build"
                        .to_string(),
                ));
            }
        } else if config.embedding_provider == "api" {
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

        engine.set_project_id(project_id.to_string());

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

    fn resolve_request_path(project_root: &Path, raw_path: &str) -> PathBuf {
        let path = PathBuf::from(raw_path);
        if path.is_absolute() {
            path
        } else {
            project_root.join(path)
        }
    }

    fn conversation_format(format: &MemoryConversationFormat) -> ConversationFormat {
        match format {
            MemoryConversationFormat::OpenAiMessages => ConversationFormat::OpenAiMessages,
            MemoryConversationFormat::ClaudeTranscript => ConversationFormat::ClaudeTranscript,
            MemoryConversationFormat::GenericJsonl => ConversationFormat::GenericJsonl,
        }
    }

    fn conversation_format_label(format: &MemoryConversationFormat) -> &'static str {
        match format {
            MemoryConversationFormat::OpenAiMessages => "openai_messages",
            MemoryConversationFormat::ClaudeTranscript => "claude_transcript",
            MemoryConversationFormat::GenericJsonl => "generic_jsonl",
        }
    }

    /// Render an absolute path for inclusion in a client-facing response.
    /// If the path is inside `project_root` the result is the repo-relative
    /// form (no leading slash); otherwise it's the absolute path. This
    /// prevents the-one-mcp from leaking host filesystem layout details
    /// for paths the client didn't already know about.
    fn display_source_path(project_root: &Path, path: &Path) -> String {
        match path.strip_prefix(project_root) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }

    fn load_transcript_from_path(
        project_root: &Path,
        raw_path: &str,
        format: &MemoryConversationFormat,
    ) -> Result<(PathBuf, ConversationTranscript), CoreError> {
        let transcript_path = Self::resolve_request_path(project_root, raw_path);
        let transcript_path = transcript_path.canonicalize().map_err(CoreError::Io)?;
        let transcript_json = std::fs::read_to_string(&transcript_path).map_err(CoreError::Io)?;
        let transcript = ConversationTranscript::from_json_str(
            transcript_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("conversation"),
            Self::conversation_format(format),
            &transcript_json,
        )
        .map_err(CoreError::Embedding)?;
        Ok((transcript_path, transcript))
    }

    fn palace_metadata_from_parts(
        project_id: &str,
        wing: Option<String>,
        hall: Option<String>,
        room: Option<String>,
    ) -> Option<PalaceMetadata> {
        if wing.is_none() && hall.is_none() && room.is_none() {
            return None;
        }

        let wing_value = wing.unwrap_or_else(|| project_id.to_string());
        Some(PalaceMetadata::new(&wing_value, hall, room))
    }

    fn palace_metadata_from_record(
        project_id: &str,
        record: &the_one_core::storage::sqlite::ConversationSourceRecord,
    ) -> Option<PalaceMetadata> {
        Self::palace_metadata_from_parts(
            project_id,
            record.wing.clone(),
            record.hall.clone(),
            record.room.clone(),
        )
    }

    fn palace_filters_requested(
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
    ) -> bool {
        wing.is_some() || hall.is_some() || room.is_some()
    }

    fn memory_palace_hall_for_hook_event(event: &crate::api::MemoryHookEvent) -> String {
        format!("hook:{}", event.as_str())
    }

    fn memory_palace_room_for_hook_event(event: &crate::api::MemoryHookEvent) -> String {
        format!("event:{}", event.as_str())
    }

    fn aaak_lessons_from_transcript(
        project_id: &str,
        transcript_path: &Path,
        transcript: &ConversationTranscript,
        max_lessons: usize,
    ) -> Vec<AaakLesson> {
        transcript
            .derive_aaak_patterns(AAAK_MIN_OCCURRENCES)
            .into_iter()
            .filter(|pattern| pattern.confidence_percent >= AAAK_MIN_CONFIDENCE_PERCENT)
            .take(max_lessons.min(AAAK_MAX_LESSONS_PER_BATCH))
            .map(|pattern| {
                let lesson_id = Self::aaak_lesson_id(project_id, &pattern.pattern_key);
                AaakLesson {
                    lesson_id,
                    project_id: project_id.to_string(),
                    pattern_key: pattern.pattern_key,
                    role: Self::conversation_role_label(&pattern.role).to_string(),
                    canonical_text: pattern.canonical_text,
                    occurrence_count: pattern.occurrence_count,
                    confidence_percent: pattern.confidence_percent,
                    source_transcript_path: Some(transcript_path.display().to_string()),
                    updated_at_epoch_ms: current_time_epoch_ms(),
                }
            })
            .collect()
    }

    fn aaak_lesson_id(project_id: &str, pattern_key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{project_id}:{pattern_key}").as_bytes());
        let digest = hasher.finalize();
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("lesson-{}", &hex[..16])
    }

    fn conversation_role_label(
        role: &the_one_memory::conversation::ConversationRole,
    ) -> &'static str {
        match role {
            the_one_memory::conversation::ConversationRole::System => "system",
            the_one_memory::conversation::ConversationRole::User => "user",
            the_one_memory::conversation::ConversationRole::Assistant => "assistant",
            the_one_memory::conversation::ConversationRole::Tool => "tool",
            the_one_memory::conversation::ConversationRole::Unknown => "unknown",
        }
    }

    fn aaak_result_from_artifact(
        transcript: &ConversationTranscript,
        artifact: AaakCompressionArtifact,
    ) -> Result<AaakCompressionResult, CoreError> {
        let compressed_payload_json = artifact
            .envelope
            .to_json_string()
            .map_err(CoreError::Embedding)?;
        Ok(AaakCompressionResult {
            used_verbatim: artifact.used_verbatim,
            confidence_percent: artifact.confidence_percent,
            original_message_count: transcript.messages.len(),
            sequence_item_count: artifact.envelope.sequence.len(),
            compressed_payload_json,
            patterns: artifact
                .patterns
                .into_iter()
                .map(|pattern| AaakPattern {
                    pattern_key: pattern.pattern_key,
                    role: Self::conversation_role_label(&pattern.role).to_string(),
                    canonical_text: pattern.canonical_text,
                    occurrence_count: pattern.occurrence_count,
                    confidence_percent: pattern.confidence_percent,
                })
                .collect(),
        })
    }

    fn aaak_features_enabled(config: &AppConfig) -> bool {
        config.memory_palace_enabled && config.memory_palace_aaak_enabled
    }

    fn diary_features_enabled(config: &AppConfig) -> bool {
        config.memory_palace_enabled && config.memory_palace_diary_enabled
    }

    fn navigation_features_enabled(config: &AppConfig) -> bool {
        config.memory_palace_enabled && config.memory_palace_navigation_enabled
    }

    fn validate_diary_date(value: &str, field_name: &str) -> Result<(), CoreError> {
        if value.len() != 10
            || !matches!(value.as_bytes().get(4), Some(b'-'))
            || !matches!(value.as_bytes().get(7), Some(b'-'))
        {
            return Err(CoreError::InvalidRequest(format!(
                "{field_name} must use YYYY-MM-DD format"
            )));
        }

        let year = value[0..4].parse::<u32>().map_err(|_| {
            CoreError::InvalidRequest(format!("{field_name} must use YYYY-MM-DD format"))
        })?;
        let month = value[5..7].parse::<u32>().map_err(|_| {
            CoreError::InvalidRequest(format!("{field_name} must use YYYY-MM-DD format"))
        })?;
        let day = value[8..10].parse::<u32>().map_err(|_| {
            CoreError::InvalidRequest(format!("{field_name} must use YYYY-MM-DD format"))
        })?;

        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(CoreError::InvalidRequest(format!(
                "{field_name} must be a real calendar-like date"
            )));
        }

        Ok(())
    }

    fn validate_diary_date_range(
        start_date: Option<&str>,
        end_date: Option<&str>,
    ) -> Result<(), CoreError> {
        if let Some(start_date) = start_date {
            Self::validate_diary_date(start_date, "start_date")?;
        }
        if let Some(end_date) = end_date {
            Self::validate_diary_date(end_date, "end_date")?;
        }
        if let (Some(start_date), Some(end_date)) = (start_date, end_date) {
            if start_date > end_date {
                return Err(CoreError::InvalidRequest(
                    "start_date must be less than or equal to end_date".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn normalize_optional_diary_field(value: Option<String>) -> Option<String> {
        value.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }

    fn normalize_diary_tags(tags: Vec<String>) -> Vec<String> {
        let mut normalized = Vec::new();
        let mut seen = HashSet::new();
        for tag in tags {
            let trimmed = tag.trim().to_ascii_lowercase();
            if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
                continue;
            }
            normalized.push(trimmed);
        }
        normalized
    }

    fn diary_entry_id(project_id: &str, entry_date: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(project_id.as_bytes());
        hasher.update(b":");
        hasher.update(entry_date.as_bytes());
        let digest = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("diary:{entry_date}:{}", &digest[..16])
    }

    fn build_diary_summary(
        entries: &[DiaryEntry],
        start_date: Option<&str>,
        end_date: Option<&str>,
        max_summary_items: usize,
    ) -> DiarySummary {
        let mut highlights = Vec::new();
        for entry in entries {
            let remaining = max_summary_items.saturating_sub(highlights.len());
            if remaining == 0 {
                break;
            }
            highlights.extend(Self::extract_facts_from_memory(&entry.content, remaining));
        }

        let summary = if entries.is_empty() {
            "No diary entries matched the requested range.".to_string()
        } else {
            format!(
                "Summarized {} diary entr{} across the requested range.",
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" }
            )
        };

        let date_from = entries
            .last()
            .map(|entry| entry.entry_date.clone())
            .or_else(|| start_date.map(str::to_string));
        let date_to = entries
            .first()
            .map(|entry| entry.entry_date.clone())
            .or_else(|| end_date.map(str::to_string));

        DiarySummary {
            date_from,
            date_to,
            entry_count: entries.len(),
            summary,
            highlights,
        }
    }

    fn navigation_node_kind_from_label(value: &str) -> Result<MemoryNavigationNodeKind, CoreError> {
        match value {
            "drawer" => Ok(MemoryNavigationNodeKind::Drawer),
            "closet" => Ok(MemoryNavigationNodeKind::Closet),
            "room" => Ok(MemoryNavigationNodeKind::Room),
            other => Err(CoreError::InvalidRequest(format!(
                "unsupported navigation node kind: {other}"
            ))),
        }
    }

    fn navigation_slug(value: &str) -> String {
        let mut slug = String::new();
        let mut last_was_separator = false;
        for ch in value.chars().flat_map(char::to_lowercase) {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                slug.push(ch);
                last_was_separator = false;
            } else if !last_was_separator {
                slug.push('_');
                last_was_separator = true;
            }
        }

        let trimmed = slug.trim_matches('_');
        if trimmed.is_empty() {
            "node".to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Compute a collision-resistant digest for a navigation node seed.
    ///
    /// History: v0.14.x used only the first 12 hex chars (48 bits) of a
    /// SHA-256 digest. The birthday bound at 48 bits is 2^24 ≈ 16.7M —
    /// uncomfortably close to the "store everything verbatim" target.
    ///
    /// Production hardening (v0.15.0) widens the output to 32 hex chars
    /// (128 bits, birthday bound 2^64 ≈ 18 quintillion) AND always folds
    /// the project_id into the input so two projects that happen to share
    /// a wing name still produce disjoint node IDs even if the database
    /// layer's composite primary key is ever relaxed.
    ///
    /// Callers that stored node IDs under the v0.14.x scheme keep working
    /// — the 12-char format is still accepted on reads because
    /// `get_navigation_node` looks up by exact `(project_id, node_id)`.
    fn navigation_digest(seed: &str) -> String {
        const DIGEST_HEX_CHARS: usize = 32;
        let mut hasher = Sha256::new();
        hasher.update(seed.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(DIGEST_HEX_CHARS);
        for byte in digest.iter().take(DIGEST_HEX_CHARS / 2) {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn navigation_drawer_node_id(project_id: &str, wing: &str) -> String {
        format!(
            "drawer:{}-{}",
            Self::navigation_slug(wing),
            Self::navigation_digest(&format!("v2:{project_id}:drawer:{wing}"))
        )
    }

    fn navigation_closet_node_id(project_id: &str, wing: &str, hall: &str) -> String {
        format!(
            "closet:{}-{}",
            Self::navigation_slug(hall),
            Self::navigation_digest(&format!("v2:{project_id}:closet:{wing}:{hall}"))
        )
    }

    fn navigation_room_node_id(
        project_id: &str,
        wing: &str,
        hall: Option<&str>,
        room: &str,
    ) -> String {
        let scope = hall
            .map(|hall| format!("v2:{project_id}:room:{wing}:{hall}:{room}"))
            .unwrap_or_else(|| format!("v2:{project_id}:room:{wing}:{room}"));
        format!(
            "room:{}-{}",
            Self::navigation_slug(room),
            Self::navigation_digest(&scope)
        )
    }

    fn navigation_tunnel_id(project_id: &str, from_node_id: &str, to_node_id: &str) -> String {
        format!(
            "tunnel-{}",
            Self::navigation_digest(&format!("v2:{project_id}:{from_node_id}:{to_node_id}"))
        )
    }

    fn normalized_tunnel_endpoints(from_node_id: &str, to_node_id: &str) -> (String, String) {
        if from_node_id <= to_node_id {
            (from_node_id.to_string(), to_node_id.to_string())
        } else {
            (to_node_id.to_string(), from_node_id.to_string())
        }
    }

    fn navigation_missing_node_error(node_id: &str) -> CoreError {
        CoreError::InvalidRequest(format!("navigation node not found: {node_id}"))
    }

    fn validate_navigation_parent(
        node_kind: &MemoryNavigationNodeKind,
        parent_node: Option<&MemoryNavigationNode>,
    ) -> Result<(), CoreError> {
        match (node_kind, parent_node.map(|node| &node.kind)) {
            (MemoryNavigationNodeKind::Drawer, Some(_)) => Err(CoreError::InvalidRequest(
                "drawer nodes cannot declare a parent_node_id".to_string(),
            )),
            (MemoryNavigationNodeKind::Drawer, None) => Ok(()),
            (MemoryNavigationNodeKind::Closet, Some(MemoryNavigationNodeKind::Drawer)) => Ok(()),
            (MemoryNavigationNodeKind::Closet, Some(_)) => Err(CoreError::InvalidRequest(
                "closet nodes must be children of drawer nodes".to_string(),
            )),
            (MemoryNavigationNodeKind::Closet, None) => Err(CoreError::InvalidRequest(
                "closet nodes require a parent drawer".to_string(),
            )),
            (MemoryNavigationNodeKind::Room, Some(MemoryNavigationNodeKind::Drawer)) => Ok(()),
            (MemoryNavigationNodeKind::Room, Some(MemoryNavigationNodeKind::Closet)) => Ok(()),
            (MemoryNavigationNodeKind::Room, Some(_)) => Err(CoreError::InvalidRequest(
                "room nodes must be children of drawer or closet nodes".to_string(),
            )),
            (MemoryNavigationNodeKind::Room, None) => Err(CoreError::InvalidRequest(
                "room nodes require a parent drawer or closet".to_string(),
            )),
        }
    }

    fn sync_navigation_nodes_from_palace_metadata(
        store: &dyn StateStore,
        project_id: &str,
        palace: &PalaceMetadata,
    ) -> Result<(), CoreError> {
        let now = current_time_epoch_ms();
        let drawer_id = Self::navigation_drawer_node_id(project_id, &palace.wing);
        store.upsert_navigation_node(&MemoryNavigationNode {
            node_id: drawer_id.clone(),
            project_id: project_id.to_string(),
            kind: MemoryNavigationNodeKind::Drawer,
            label: palace.wing.clone(),
            parent_node_id: None,
            wing: Some(palace.wing.clone()),
            hall: None,
            room: None,
            updated_at_epoch_ms: now,
        })?;

        let mut parent_node_id = drawer_id;
        if let Some(hall) = palace.hall.as_deref() {
            let closet_id = Self::navigation_closet_node_id(project_id, &palace.wing, hall);
            store.upsert_navigation_node(&MemoryNavigationNode {
                node_id: closet_id.clone(),
                project_id: project_id.to_string(),
                kind: MemoryNavigationNodeKind::Closet,
                label: hall.to_string(),
                parent_node_id: Some(parent_node_id.clone()),
                wing: Some(palace.wing.clone()),
                hall: Some(hall.to_string()),
                room: None,
                updated_at_epoch_ms: now,
            })?;
            parent_node_id = closet_id;
        }

        if let Some(room) = palace.room.as_deref() {
            store.upsert_navigation_node(&MemoryNavigationNode {
                node_id: Self::navigation_room_node_id(
                    project_id,
                    &palace.wing,
                    palace.hall.as_deref(),
                    room,
                ),
                project_id: project_id.to_string(),
                kind: MemoryNavigationNodeKind::Room,
                label: room.to_string(),
                parent_node_id: Some(parent_node_id),
                wing: Some(palace.wing.clone()),
                hall: palace.hall.clone(),
                room: Some(room.to_string()),
                updated_at_epoch_ms: now,
            })?;
        }

        Ok(())
    }

    fn search_candidate_limit(top_k: usize, filters_requested: bool) -> usize {
        if filters_requested {
            top_k.saturating_mul(10).max(25)
        } else {
            top_k
        }
    }

    fn chunk_matches_palace_filters(
        chunk: &the_one_memory::chunker::ChunkMeta,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
    ) -> bool {
        if !Self::palace_filters_requested(wing, hall, room) {
            return true;
        }

        if chunk.language.as_deref() != Some("conversation") {
            return false;
        }

        let palace_path: Vec<&str> = chunk
            .heading_hierarchy
            .iter()
            .skip(1)
            .map(String::as_str)
            .collect();
        let chunk_wing = palace_path.first().copied();
        let chunk_hall = chunk
            .signature
            .as_deref()
            .or_else(|| palace_path.get(1).copied());
        let chunk_room = chunk.symbol.as_deref().or_else(|| {
            if chunk_hall.is_some() {
                palace_path.get(2).copied()
            } else {
                palace_path.get(1).copied()
            }
        });

        wing.is_none_or(|value| chunk_wing == Some(value))
            && hall.is_none_or(|value| chunk_hall == Some(value))
            && room.is_none_or(|value| chunk_room == Some(value))
    }

    fn filter_memory_search_results(
        results: Vec<the_one_memory::MemorySearchResult>,
        wing: Option<&str>,
        hall: Option<&str>,
        room: Option<&str>,
        top_k: usize,
    ) -> Vec<the_one_memory::MemorySearchResult> {
        let mut filtered = results
            .into_iter()
            .filter(|result| Self::chunk_matches_palace_filters(&result.chunk, wing, hall, room))
            .collect::<Vec<_>>();
        filtered.truncate(top_k);
        filtered
    }

    fn conversation_format_from_label(label: &str) -> Result<ConversationFormat, CoreError> {
        match label {
            "openai_messages" => Ok(ConversationFormat::OpenAiMessages),
            "claude_transcript" => Ok(ConversationFormat::ClaudeTranscript),
            "generic_jsonl" => Ok(ConversationFormat::GenericJsonl),
            other => Err(CoreError::InvalidProjectConfig(format!(
                "unsupported stored conversation format: {other}"
            ))),
        }
    }

    async fn hydrate_engine_from_disk(
        engine: &mut MemoryEngine,
        project_root: &Path,
        project_id: &str,
        sources: &[the_one_core::storage::sqlite::ConversationSourceRecord],
        pending_conversation: Option<(&str, &ConversationTranscript, Option<PalaceMetadata>)>,
    ) -> Result<usize, CoreError> {
        let docs_root = project_root.join(".the-one").join("docs");
        if docs_root.exists() {
            engine
                .ingest_markdown_tree(&docs_root)
                .await
                .map_err(|error| CoreError::Embedding(error.to_string()))?;
        }

        for source in sources {
            let transcript_json =
                std::fs::read_to_string(&source.transcript_path).map_err(CoreError::Io)?;
            let transcript = ConversationTranscript::from_json_str(
                Path::new(&source.transcript_path)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("conversation"),
                Self::conversation_format_from_label(&source.format)?,
                &transcript_json,
            )
            .map_err(CoreError::Embedding)?;

            engine
                .ingest_conversation(
                    &source.memory_path,
                    &transcript,
                    Self::palace_metadata_from_record(project_id, source),
                )
                .await
                .map_err(CoreError::Embedding)?;
        }

        if let Some((source_path, transcript, palace)) = pending_conversation {
            let ingested = engine
                .ingest_conversation(source_path, transcript, palace)
                .await
                .map_err(CoreError::Embedding)?;
            return Ok(ingested);
        }

        Ok(0)
    }

    async fn build_rehydrated_local_engine(
        &self,
        project_root: &Path,
        project_id: &str,
        sources: &[the_one_core::storage::sqlite::ConversationSourceRecord],
        pending_conversation: Option<(&str, &ConversationTranscript, Option<PalaceMetadata>)>,
    ) -> Result<(MemoryEngine, usize), CoreError> {
        #[cfg(feature = "local-embeddings")]
        {
            let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
            let mut engine =
                MemoryEngine::new_local(&config.embedding_model, config.limits.max_chunk_tokens)
                    .map_err(CoreError::Embedding)?;
            engine.set_project_id(project_id.to_string());
            let ingested = Self::hydrate_engine_from_disk(
                &mut engine,
                project_root,
                project_id,
                sources,
                pending_conversation,
            )
            .await?;
            Ok((engine, ingested))
        }
        #[cfg(not(feature = "local-embeddings"))]
        {
            let _ = project_root;
            let _ = project_id;
            let _ = sources;
            let _ = pending_conversation;
            Err(CoreError::Embedding(
                "local embeddings not available for non-destructive fallback".to_string(),
            ))
        }
    }

    async fn ensure_project_memory_loaded(
        &self,
        project_root: &Path,
        project_id: &str,
    ) -> Result<(), CoreError> {
        let key = Self::project_memory_key(project_root, project_id);
        if self.memory_by_project.read().await.contains_key(&key) {
            return Ok(());
        }

        let sources = self
            .with_state_store(project_root, project_id, |store| {
                store.list_conversation_sources(None, None, None, usize::MAX)
            })
            .await?;
        let (engine, _) = self
            .build_rehydrated_local_engine(project_root, project_id, &sources, None)
            .await?;

        self.memory_by_project.write().await.insert(key, engine);
        Ok(())
    }

    fn extract_facts_from_memory(content: &str, max_items: usize) -> Vec<String> {
        let mut facts = Vec::new();
        let mut seen = HashSet::new();

        for line in content.lines() {
            let fact = line.trim();
            if fact.is_empty()
                || fact.starts_with('#')
                || fact.starts_with("Source:")
                || fact.starts_with("Format:")
                || fact.starts_with("Wing:")
                || fact.starts_with("Hall:")
                || fact.starts_with("Room:")
                || fact.starts_with("[turn:")
            {
                continue;
            }

            let normalized = fact.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }

            facts.push(normalized);
            if facts.len() >= max_items {
                break;
            }
        }

        facts
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
            let engine = self.build_memory_engine(project_root, project_id).await?;
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
        let MemorySearchRequest {
            project_root,
            project_id,
            query,
            top_k,
            wing,
            hall,
            room,
        } = request;
        let project_root = Path::new(&project_root);
        let route = self.route_query(project_root, &query).await;
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
        let top_k = self.policy.clamp_search_hits(top_k);
        let wing_filter = wing.as_deref();
        let hall_filter = hall.as_deref();
        let room_filter = room.as_deref();
        // v0.15.0: we still degrade gracefully when config is missing (the
        // search path is non-fatal), but we now log the error so an
        // operator can diagnose a misconfigured project. Previously the
        // failure was silently dropped.
        let config = match AppConfig::load(project_root, RuntimeOverrides::default()) {
            Ok(cfg) => Some(cfg),
            Err(err) => {
                tracing::warn!(
                    target: "the_one_mcp::memory_search",
                    project_id = %project_id,
                    error = %err,
                    "failed to load project config — proceeding with defaults"
                );
                None
            }
        };
        let palace_enabled = config
            .as_ref()
            .map(|cfg| cfg.memory_palace_enabled)
            .unwrap_or(true);
        let (wing_filter, hall_filter, room_filter) = if palace_enabled {
            (wing_filter, hall_filter, room_filter)
        } else {
            (None, None, None)
        };
        let filters_requested =
            Self::palace_filters_requested(wing_filter, hall_filter, room_filter);
        let search_top_k = Self::search_candidate_limit(top_k, filters_requested);

        if route.decision.requires_memory_search {
            // Best-effort: if memory load fails the search path falls
            // through to `Vec::new()` below. v0.14.x silently dropped this
            // error; v0.15.0 emits a warning so operators can see why
            // searches return no hits.
            if let Err(err) = self
                .ensure_project_memory_loaded(project_root, &project_id)
                .await
            {
                tracing::warn!(
                    target: "the_one_mcp::memory_search",
                    project_id = %project_id,
                    error = %err,
                    "ensure_project_memory_loaded failed — returning empty hits"
                );
            }
        }

        // Load score threshold from config (LightRAG-inspired improvement)
        let score_threshold = config
            .as_ref()
            .map(|c| c.limits.search_score_threshold)
            .unwrap_or(0.0);

        let hits = if route.decision.requires_memory_search {
            let key = Self::project_memory_key(project_root, &project_id);
            let memories = self.memory_by_project.read().await;
            if let Some(memory) = memories.get(&key) {
                memory
                    .search(&EngineSearchRequest {
                        query,
                        top_k: search_top_k,
                        score_threshold,
                        mode: RetrievalMode::Hybrid,
                    })
                    .await
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let hits =
            Self::filter_memory_search_results(hits, wing_filter, hall_filter, room_filter, top_k)
                .into_iter()
                .map(|item| MemorySearchItem {
                    id: item.chunk.id,
                    source_path: item.chunk.source_path,
                    score: item.score,
                })
                .collect::<Vec<_>>();

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
    pub async fn memory_ingest_conversation(
        &self,
        request: MemoryIngestConversationRequest,
    ) -> Result<MemoryIngestConversationResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        // v0.15.0 production hardening: validate every user-supplied name
        // at the broker entry point. Returns InvalidRequest with a concrete
        // human-readable message the client can show.
        the_one_core::naming::sanitize_project_id(&request.project_id)?;
        let wing = the_one_core::naming::sanitize_optional_name(request.wing.as_deref(), "wing")?;
        let hall = the_one_core::naming::sanitize_optional_name(request.hall.as_deref(), "hall")?;
        let room = the_one_core::naming::sanitize_optional_name(request.room.as_deref(), "room")?;

        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !config.memory_palace_enabled {
            return Err(CoreError::NotEnabled(
                "memory palace features are disabled; enable memory_palace_enabled".to_string(),
            ));
        }

        let (transcript_path, transcript) =
            Self::load_transcript_from_path(project_root, &request.path, &request.format)?;

        // v0.16.0 Phase 1: pull existing sources through the state-store
        // cache. The handler then proceeds to async memory-engine work
        // *without* holding any state-store lock, and reacquires the
        // cache inside a single closure at the end for all DB writes.
        let existing_sources = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.list_conversation_sources(None, None, None, usize::MAX)
            })
            .await?;

        let palace = Self::palace_metadata_from_parts(&request.project_id, wing, hall, room);
        let source_path = transcript_path.display().to_string();
        let pending_conversation = (source_path.as_str(), &transcript, palace.clone());

        let key = Self::project_memory_key(project_root, &request.project_id);
        let mut memories = self.memory_by_project.write().await;
        let is_new = !memories.contains_key(&key);
        if is_new {
            let engine = self
                .build_memory_engine(project_root, &request.project_id)
                .await?;
            memories.insert(key.clone(), engine);
        }
        let memory = memories.get_mut(&key).ok_or_else(|| {
            CoreError::InvalidProjectConfig("project memory not indexed".to_string())
        })?;
        if is_new {
            self.maybe_spawn_watcher(project_root, &request.project_id);
        }

        let direct_ingest = memory
            .ingest_conversation(&source_path, &transcript, palace.clone())
            .await;
        let ingested_chunks = match direct_ingest {
            Ok(count) => count,
            Err(error) => {
                let (fallback, count) = self
                    .build_rehydrated_local_engine(
                        project_root,
                        &request.project_id,
                        &existing_sources,
                        Some(pending_conversation),
                    )
                    .await
                    .map_err(|local_error| {
                        CoreError::Embedding(format!(
                            "conversation ingest failed with configured backend ({error}) \
                             and local fallback ({local_error})"
                        ))
                    })?;
                memories.insert(key.clone(), fallback);
                count
            }
        };
        drop(memories);

        // v0.16.0 Phase 1: all remaining state-store writes run inside a
        // single sync closure. This bundles the conversation source
        // upsert, optional navigation sync, optional AAAK lessons, and
        // the audit entry into one Mutex-guarded span — previously they
        // ran through four separate `db.*` calls on one held handle.
        // The closure returns the audit outcome so a failure to record
        // audit doesn't propagate, matching the v0.14.x non-fatal
        // behavior.
        let conversation_record = the_one_core::storage::sqlite::ConversationSourceRecord {
            project_id: request.project_id.clone(),
            transcript_path: transcript_path.display().to_string(),
            memory_path: source_path,
            format: Self::conversation_format_label(&request.format).to_string(),
            wing: palace.as_ref().map(|value| value.wing.clone()),
            hall: palace.as_ref().and_then(|value| value.hall.clone()),
            room: palace.as_ref().and_then(|value| value.room.clone()),
            message_count: transcript.messages.len(),
        };
        let aaak_lessons = if Self::aaak_features_enabled(&config) {
            Self::aaak_lessons_from_transcript(
                &request.project_id,
                &transcript_path,
                &transcript,
                AAAK_MAX_LESSONS_PER_BATCH,
            )
        } else {
            Vec::new()
        };
        let audit_params = the_one_core::audit::params_json(serde_json::json!({
            "project_id": request.project_id,
            "path": request.path,
            "ingested_chunks": ingested_chunks,
        }));
        let navigation_enabled = Self::navigation_features_enabled(&config);
        let project_id_for_nav = request.project_id.clone();
        let palace_for_nav = palace.clone();

        self.with_state_store(project_root, &request.project_id, |store| {
            store.upsert_conversation_source(&conversation_record)?;

            if navigation_enabled {
                if let Some(palace) = palace_for_nav.as_ref() {
                    Self::sync_navigation_nodes_from_palace_metadata(
                        store,
                        &project_id_for_nav,
                        palace,
                    )?;
                }
            }

            for lesson in &aaak_lessons {
                store.upsert_aaak_lesson(lesson)?;
            }

            if let Err(err) = store.record_audit(&the_one_core::audit::AuditRecord::ok(
                "memory.ingest_conversation",
                audit_params,
            )) {
                tracing::warn!(
                    target: "the_one_mcp::audit",
                    operation = "memory.ingest_conversation",
                    error = %err,
                    "audit record failed (non-fatal)"
                );
            }
            Ok(())
        })
        .await?;

        Ok(MemoryIngestConversationResponse {
            ingested_chunks,
            source_path: Self::display_source_path(project_root, &transcript_path),
        })
    }

    #[instrument(skip_all)]
    pub async fn memory_wake_up(
        &self,
        request: MemoryWakeUpRequest,
    ) -> Result<MemoryWakeUpResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !config.memory_palace_enabled {
            return Err(CoreError::NotEnabled(
                "memory palace features are disabled; enable memory_palace_enabled".to_string(),
            ));
        }

        self.ensure_project_memory_loaded(project_root, &request.project_id)
            .await?;

        let sources = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.list_conversation_sources(
                    request.wing.as_deref(),
                    request.hall.as_deref(),
                    request.room.as_deref(),
                    request.max_items,
                )
            })
            .await?;
        if sources.is_empty() {
            return Ok(MemoryWakeUpResponse {
                summary: "No conversation memory available.".to_string(),
                facts: Vec::new(),
            });
        }

        let mut facts = Vec::new();
        for source in &sources {
            let maybe_content = self
                .with_project_memory(project_root, &request.project_id, |memory| {
                    memory.docs_get(&source.memory_path)
                })
                .await?;

            if let Some(content) = maybe_content {
                let remaining = request.max_items.saturating_sub(facts.len());
                if remaining == 0 {
                    break;
                }

                facts.extend(Self::extract_facts_from_memory(&content, remaining));
            }

            if facts.len() >= request.max_items {
                break;
            }
        }

        let summary = if facts.is_empty() {
            format!(
                "Wake-up pack checked {} conversation source(s) but found no compact facts.",
                sources.len()
            )
        } else {
            format!(
                "Wake-up pack with {} fact(s) from {} conversation source(s).",
                facts.len(),
                sources.len()
            )
        };

        Ok(MemoryWakeUpResponse { summary, facts })
    }

    #[instrument(skip_all)]
    pub async fn memory_capture_hook(
        &self,
        request: MemoryCaptureHookRequest,
    ) -> Result<MemoryCaptureHookResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !config.memory_palace_enabled {
            return Err(CoreError::NotEnabled(
                "memory palace features are disabled; enable memory_palace_enabled".to_string(),
            ));
        }
        if !config.memory_palace_hooks_enabled {
            return Err(CoreError::NotEnabled(
                "memory palace hook capture is disabled; enable memory_palace_hooks_enabled"
                    .to_string(),
            ));
        }

        let wing = request.wing.unwrap_or_else(|| request.project_id.clone());
        let hall = request
            .hall
            .unwrap_or_else(|| Self::memory_palace_hall_for_hook_event(&request.event));
        let room = request
            .room
            .unwrap_or_else(|| Self::memory_palace_room_for_hook_event(&request.event));

        let ingest = self
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: request.project_root.clone(),
                project_id: request.project_id,
                path: request.path,
                format: request.format,
                wing: Some(wing.clone()),
                hall: Some(hall.clone()),
                room: Some(room.clone()),
            })
            .await?;

        Ok(MemoryCaptureHookResponse {
            event: request.event.as_str().to_string(),
            ingested_chunks: ingest.ingested_chunks,
            source_path: ingest.source_path,
            wing,
            hall,
            room,
        })
    }

    #[instrument(skip_all)]
    pub async fn memory_aaak_compress(
        &self,
        request: MemoryAaakCompressRequest,
    ) -> Result<MemoryAaakCompressResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::aaak_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "AAAK features are disabled; enable memory_palace_enabled and memory_palace_aaak_enabled"
                    .to_string(),
            ));
        }

        let (_, transcript) =
            Self::load_transcript_from_path(project_root, &request.path, &request.format)?;
        let artifact = transcript.compress_aaak(AAAK_MIN_OCCURRENCES, AAAK_MIN_CONFIDENCE_PERCENT);
        Ok(MemoryAaakCompressResponse {
            result: Self::aaak_result_from_artifact(&transcript, artifact)?,
        })
    }

    #[instrument(skip_all)]
    pub async fn memory_aaak_teach(
        &self,
        request: MemoryAaakTeachRequest,
    ) -> Result<MemoryAaakTeachResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::aaak_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "AAAK features are disabled; enable memory_palace_enabled and memory_palace_aaak_enabled"
                    .to_string(),
            ));
        }

        let (transcript_path, transcript) =
            Self::load_transcript_from_path(project_root, &request.path, &request.format)?;
        let lessons = Self::aaak_lessons_from_transcript(
            &request.project_id,
            &transcript_path,
            &transcript,
            AAAK_MAX_LESSONS_PER_BATCH,
        );
        self.with_state_store(project_root, &request.project_id, |store| {
            for lesson in &lessons {
                store.upsert_aaak_lesson(lesson)?;
            }
            Ok(())
        })
        .await?;

        Ok(MemoryAaakTeachResponse {
            outcome: AaakTeachOutcome {
                lessons_written: lessons.len(),
                skipped_reason: if lessons.is_empty() {
                    Some("no repeated motifs met AAAK confidence threshold".to_string())
                } else {
                    None
                },
                lessons,
            },
        })
    }

    #[instrument(skip_all)]
    pub async fn memory_aaak_list_lessons(
        &self,
        request: MemoryAaakListLessonsRequest,
    ) -> Result<MemoryAaakListLessonsResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::aaak_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "AAAK features are disabled; enable memory_palace_enabled and memory_palace_aaak_enabled"
                    .to_string(),
            ));
        }

        let lessons = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.list_aaak_lessons(&request.project_id, request.limit)
            })
            .await?;
        Ok(MemoryAaakListLessonsResponse { lessons })
    }

    #[instrument(skip_all)]
    pub async fn memory_diary_add(
        &self,
        request: MemoryDiaryAddRequest,
    ) -> Result<MemoryDiaryAddResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        // v0.15.0: validate project_id and each optional tag up front.
        the_one_core::naming::sanitize_project_id(&request.project_id)?;

        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::diary_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "diary features are disabled; enable memory_palace_enabled and memory_palace_diary_enabled"
                    .to_string(),
            ));
        }

        Self::validate_diary_date(&request.entry_date, "entry_date")?;
        let content = request.content.trim().to_string();
        if content.is_empty() {
            return Err(CoreError::InvalidRequest(
                "diary entry content cannot be empty".to_string(),
            ));
        }

        let mood = Self::normalize_optional_diary_field(request.mood);
        let tags = Self::normalize_diary_tags(request.tags);
        // Validate each tag — tags flow into FTS5 and JSON payloads.
        for tag in &tags {
            the_one_core::naming::sanitize_name(tag, "diary tag")?;
        }
        let now = current_time_epoch_ms();
        let entry_date = request.entry_date.clone();
        let project_id = request.project_id.clone();
        let entry = self
            .with_state_store(project_root, &request.project_id, |store| {
                let existing_entry = store
                    .list_diary_entries(Some(&entry_date), Some(&entry_date), 1)?
                    .into_iter()
                    .next();
                let entry = DiaryEntry {
                    entry_id: existing_entry
                        .as_ref()
                        .map(|entry| entry.entry_id.clone())
                        .unwrap_or_else(|| Self::diary_entry_id(&project_id, &entry_date)),
                    project_id: project_id.clone(),
                    entry_date,
                    mood,
                    tags,
                    content,
                    created_at_epoch_ms: existing_entry
                        .as_ref()
                        .map(|entry| entry.created_at_epoch_ms)
                        .unwrap_or(now),
                    updated_at_epoch_ms: now,
                };

                store.upsert_diary_entry(&entry)?;

                let audit_params = the_one_core::audit::params_json(serde_json::json!({
                    "project_id": entry.project_id,
                    "entry_id": entry.entry_id,
                    "entry_date": entry.entry_date,
                }));
                if let Err(err) = store.record_audit(&the_one_core::audit::AuditRecord::ok(
                    "memory.diary.add",
                    audit_params,
                )) {
                    tracing::warn!(
                        target: "the_one_mcp::audit",
                        operation = "memory.diary.add",
                        error = %err,
                        "audit record failed (non-fatal)"
                    );
                }

                Ok(entry)
            })
            .await?;

        Ok(MemoryDiaryAddResponse { entry })
    }

    #[instrument(skip_all)]
    pub async fn memory_diary_list(
        &self,
        request: MemoryDiaryListRequest,
    ) -> Result<MemoryDiaryListResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::diary_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "diary features are disabled; enable memory_palace_enabled and memory_palace_diary_enabled"
                    .to_string(),
            ));
        }

        Self::validate_diary_date_range(
            request.start_date.as_deref(),
            request.end_date.as_deref(),
        )?;
        // v0.15.0: route through pagination validation — returns an
        // InvalidRequest error on over-limit, never silently truncates.
        let entries = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.list_diary_entries(
                    request.start_date.as_deref(),
                    request.end_date.as_deref(),
                    request.max_results,
                )
            })
            .await?;
        Ok(MemoryDiaryListResponse { entries })
    }

    #[instrument(skip_all)]
    pub async fn memory_diary_search(
        &self,
        request: MemoryDiarySearchRequest,
    ) -> Result<MemoryDiarySearchResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::diary_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "diary features are disabled; enable memory_palace_enabled and memory_palace_diary_enabled"
                    .to_string(),
            ));
        }

        let query = request.query.trim();
        if query.is_empty() {
            return Err(CoreError::InvalidRequest(
                "diary search query cannot be empty".to_string(),
            ));
        }
        Self::validate_diary_date_range(
            request.start_date.as_deref(),
            request.end_date.as_deref(),
        )?;

        let entries = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.search_diary_entries_in_range(
                    query,
                    request.start_date.as_deref(),
                    request.end_date.as_deref(),
                    request.max_results,
                )
            })
            .await?;
        Ok(MemoryDiarySearchResponse { entries })
    }

    #[instrument(skip_all)]
    pub async fn memory_diary_summarize(
        &self,
        request: MemoryDiarySummarizeRequest,
    ) -> Result<MemoryDiarySummarizeResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::diary_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "diary features are disabled; enable memory_palace_enabled and memory_palace_diary_enabled"
                    .to_string(),
            ));
        }

        Self::validate_diary_date_range(
            request.start_date.as_deref(),
            request.end_date.as_deref(),
        )?;

        let max_summary_items = self.policy.clamp_search_hits(request.max_summary_items);
        let entries = self
            .with_state_store(project_root, &request.project_id, |store| {
                store.list_diary_entries(
                    request.start_date.as_deref(),
                    request.end_date.as_deref(),
                    max_summary_items,
                )
            })
            .await?;

        Ok(MemoryDiarySummarizeResponse {
            summary: Self::build_diary_summary(
                &entries,
                request.start_date.as_deref(),
                request.end_date.as_deref(),
                max_summary_items,
            ),
        })
    }

    #[instrument(skip_all)]
    pub async fn memory_navigation_upsert_node(
        &self,
        request: MemoryNavigationUpsertNodeRequest,
    ) -> Result<MemoryNavigationUpsertNodeResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        // v0.15.0: validate every name at the entry point. label is a
        // user-facing string so we use sanitize_name; node_id uses the
        // action_key charset since it can carry `:` separators.
        the_one_core::naming::sanitize_project_id(&request.project_id)?;
        the_one_core::naming::sanitize_action_key(&request.node_id)?;
        the_one_core::naming::sanitize_name(&request.label, "label")?;
        let wing = the_one_core::naming::sanitize_optional_name(request.wing.as_deref(), "wing")?;
        let hall = the_one_core::naming::sanitize_optional_name(request.hall.as_deref(), "hall")?;
        let room = the_one_core::naming::sanitize_optional_name(request.room.as_deref(), "room")?;

        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::navigation_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "navigation features are disabled; enable memory_palace_enabled and memory_palace_navigation_enabled"
                    .to_string(),
            ));
        }

        let kind = Self::navigation_node_kind_from_label(&request.kind)?;
        // Clone project_id for the outer call so the closure can move
        // `request` freely (it moves `request.project_id` / `request.node_id`
        // / `request.label` / `request.parent_node_id` into the node).
        let project_id_key = request.project_id.clone();
        let node = self
            .with_state_store(project_root, &project_id_key, |store| {
                let parent_node = request
                    .parent_node_id
                    .as_deref()
                    .map(|parent_node_id| {
                        the_one_core::naming::sanitize_action_key(parent_node_id)?;
                        store
                            .get_navigation_node(parent_node_id)?
                            .ok_or_else(|| Self::navigation_missing_node_error(parent_node_id))
                    })
                    .transpose()?;
                Self::validate_navigation_parent(&kind, parent_node.as_ref())?;

                let node = MemoryNavigationNode {
                    node_id: request.node_id,
                    project_id: request.project_id,
                    kind,
                    label: request.label,
                    parent_node_id: request.parent_node_id,
                    wing,
                    hall,
                    room,
                    updated_at_epoch_ms: current_time_epoch_ms(),
                };
                store.upsert_navigation_node(&node)?;

                let audit_params = the_one_core::audit::params_json(serde_json::json!({
                    "project_id": node.project_id,
                    "node_id": node.node_id,
                    "kind": node.kind.as_str(),
                }));
                if let Err(err) = store.record_audit(&the_one_core::audit::AuditRecord::ok(
                    "memory.navigation.upsert_node",
                    audit_params,
                )) {
                    tracing::warn!(
                        target: "the_one_mcp::audit",
                        operation = "memory.navigation.upsert_node",
                        error = %err,
                        "audit record failed (non-fatal)"
                    );
                }

                Ok(node)
            })
            .await?;

        Ok(MemoryNavigationUpsertNodeResponse { node })
    }

    #[instrument(skip_all)]
    pub async fn memory_navigation_link_tunnel(
        &self,
        request: MemoryNavigationLinkTunnelRequest,
    ) -> Result<MemoryNavigationLinkTunnelResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        the_one_core::naming::sanitize_project_id(&request.project_id)?;
        the_one_core::naming::sanitize_action_key(&request.from_node_id)?;
        the_one_core::naming::sanitize_action_key(&request.to_node_id)?;

        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::navigation_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "navigation features are disabled; enable memory_palace_enabled and memory_palace_navigation_enabled"
                    .to_string(),
            ));
        }

        // Compute the normalized endpoints first so the closure can move
        // `request` into the tunnel record.
        let (from_node_id, to_node_id) =
            Self::normalized_tunnel_endpoints(&request.from_node_id, &request.to_node_id);
        if from_node_id == to_node_id {
            return Err(CoreError::InvalidRequest(
                "navigation tunnel endpoints must differ".to_string(),
            ));
        }

        // Clone project_id for the outer call so the closure can move
        // `request` freely (it needs to move `request.project_id` into
        // the tunnel record).
        let project_id_key = request.project_id.clone();
        let tunnel = self
            .with_state_store(project_root, &project_id_key, |store| {
                store
                    .get_navigation_node(&request.from_node_id)?
                    .ok_or_else(|| Self::navigation_missing_node_error(&request.from_node_id))?;
                store
                    .get_navigation_node(&request.to_node_id)?
                    .ok_or_else(|| Self::navigation_missing_node_error(&request.to_node_id))?;

                let tunnel = MemoryNavigationTunnel {
                    tunnel_id: Self::navigation_tunnel_id(
                        &request.project_id,
                        &from_node_id,
                        &to_node_id,
                    ),
                    project_id: request.project_id,
                    from_node_id,
                    to_node_id,
                    updated_at_epoch_ms: current_time_epoch_ms(),
                };
                store.upsert_navigation_tunnel(&tunnel)?;
                Ok(tunnel)
            })
            .await?;

        Ok(MemoryNavigationLinkTunnelResponse { tunnel })
    }

    #[instrument(skip_all)]
    pub async fn memory_navigation_list(
        &self,
        request: MemoryNavigationListRequest,
    ) -> Result<MemoryNavigationListResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::navigation_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "navigation features are disabled; enable memory_palace_enabled and memory_palace_navigation_enabled"
                    .to_string(),
            ));
        }

        let kind_filter = request
            .kind
            .as_deref()
            .map(Self::navigation_node_kind_from_label)
            .transpose()?
            .map(|kind| kind.as_str().to_string());
        // v0.15.0: pagination — reject over-limit requests instead of silently
        // truncating to 2 000 like v0.14.x did.
        let nodes_req = the_one_core::pagination::PageRequest::decode(
            request.limit,
            request.cursor.as_deref(),
            the_one_core::storage::sqlite::page_limits::NAVIGATION_NODES_DEFAULT,
            the_one_core::storage::sqlite::page_limits::NAVIGATION_NODES_MAX,
        )?;

        self.with_state_store(project_root, &request.project_id, |store| {
            if let Some(parent_node_id) = request.parent_node_id.as_deref() {
                store
                    .get_navigation_node(parent_node_id)?
                    .ok_or_else(|| Self::navigation_missing_node_error(parent_node_id))?;
            }

            let nodes_page = store.list_navigation_nodes_paged(
                request.parent_node_id.as_deref(),
                kind_filter.as_deref(),
                &nodes_req,
            )?;
            let nodes = nodes_page.items;

            // v0.15.0: tunnel filter pushed to SQL — previously we fetched every
            // tunnel in the project into Rust and filtered client-side. Now we
            // either fetch the full page of tunnels (when no filter is active)
            // or call the indexed `list_navigation_tunnels_for_nodes` helper.
            let tunnels = if request.parent_node_id.is_none() && request.kind.is_none() {
                let tunnels_req = the_one_core::pagination::PageRequest::decode(
                    0,
                    None,
                    the_one_core::storage::sqlite::page_limits::NAVIGATION_TUNNELS_MAX,
                    the_one_core::storage::sqlite::page_limits::NAVIGATION_TUNNELS_MAX,
                )?;
                store
                    .list_navigation_tunnels_paged(None, &tunnels_req)?
                    .items
            } else {
                let node_id_list: Vec<String> =
                    nodes.iter().map(|node| node.node_id.clone()).collect();
                store.list_navigation_tunnels_for_nodes(
                    &node_id_list,
                    the_one_core::storage::sqlite::page_limits::NAVIGATION_TUNNELS_MAX,
                )?
            };

            Ok(MemoryNavigationListResponse {
                nodes,
                tunnels,
                next_cursor: nodes_page.next_cursor.map(|c| c.as_str().to_string()),
                total_nodes: nodes_page.total_count,
            })
        })
        .await
    }

    #[instrument(skip_all)]
    pub async fn memory_navigation_traverse(
        &self,
        request: MemoryNavigationTraverseRequest,
    ) -> Result<MemoryNavigationTraverseResponse, CoreError> {
        let project_root = Path::new(&request.project_root);
        let config = AppConfig::load(project_root, RuntimeOverrides::default())?;
        if !Self::navigation_features_enabled(&config) {
            return Err(CoreError::NotEnabled(
                "navigation features are disabled; enable memory_palace_enabled and memory_palace_navigation_enabled"
                    .to_string(),
            ));
        }

        // v0.15.0 production-hardening: BFS traverses the node graph using
        // SQL-side neighbor queries. Prior to v0.15.0 this function loaded
        // every node (capped at 2 000) and every tunnel in the project into
        // memory on every call — O(total_nodes) per traverse even if the
        // user only visited 10 reachable neighbors.
        //
        // Traversal strategy:
        // 1. BFS with an explicit visited set.
        // 2. For each frontier node, fetch its direct child nodes via
        //    `list_navigation_nodes_paged(parent_node_id = current)` (indexed).
        // 3. Fetch tunnels touching the current frontier via
        //    `list_navigation_tunnels_for_nodes` (indexed).
        // 4. Cap total visited nodes at `MAX_TRAVERSE_NODES` so a pathological
        //    request cannot OOM the server; return truncated results + a
        //    `truncated` flag.
        //
        // v0.16.0 Phase 1: the BFS runs entirely inside `with_state_store`
        // — this is the ideal closure case because a single traversal
        // makes dozens of sequential DB calls that all want the same
        // backend connection.
        const MAX_TRAVERSE_NODES: usize = 2_000;

        self.with_state_store(project_root, &request.project_id, |store| {
            store
                .get_navigation_node(&request.start_node_id)?
                .ok_or_else(|| Self::navigation_missing_node_error(&request.start_node_id))?;

            let start_node = store
                .get_navigation_node(&request.start_node_id)?
                .expect("start node existence was checked above");

            let mut queue: VecDeque<(MemoryNavigationNode, usize)> =
                VecDeque::from([(start_node.clone(), 0usize)]);
            let mut visited: HashMap<String, MemoryNavigationNode> = HashMap::new();
            visited.insert(start_node.node_id.clone(), start_node);
            let mut ordered_nodes: Vec<MemoryNavigationNode> = Vec::new();
            let mut truncated = false;

            while let Some((node, depth)) = queue.pop_front() {
                ordered_nodes.push(node.clone());
                if ordered_nodes.len() >= MAX_TRAVERSE_NODES {
                    truncated = !queue.is_empty();
                    break;
                }
                if depth >= request.max_depth {
                    continue;
                }

                // Parent edge — walk up one level.
                if let Some(parent_id) = node.parent_node_id.as_deref() {
                    if !visited.contains_key(parent_id) {
                        if let Some(parent) = store.get_navigation_node(parent_id)? {
                            visited.insert(parent.node_id.clone(), parent.clone());
                            queue.push_back((parent, depth + 1));
                        }
                    }
                }

                // Child nodes — indexed lookup by parent_node_id.
                let mut child_offset: u64 = 0;
                loop {
                    let req = the_one_core::pagination::PageRequest::decode(
                        the_one_core::storage::sqlite::page_limits::NAVIGATION_NODES_DEFAULT,
                        Some(&the_one_core::pagination::Cursor::from_offset(child_offset).0),
                        the_one_core::storage::sqlite::page_limits::NAVIGATION_NODES_DEFAULT,
                        the_one_core::storage::sqlite::page_limits::NAVIGATION_NODES_MAX,
                    )?;
                    let child_page =
                        store.list_navigation_nodes_paged(Some(&node.node_id), None, &req)?;
                    for child in child_page.items {
                        if visited.contains_key(&child.node_id) {
                            continue;
                        }
                        visited.insert(child.node_id.clone(), child.clone());
                        queue.push_back((child, depth + 1));
                    }
                    match child_page.next_cursor {
                        Some(cursor) => {
                            let (off, _) =
                                the_one_core::pagination::Cursor::decode(cursor.as_str())?;
                            child_offset = off;
                        }
                        None => break,
                    }
                }
            }

            // Collect tunnels touching any visited node.
            let visited_ids: Vec<String> = visited.keys().cloned().collect();
            let mut tunnels = store.list_navigation_tunnels_for_nodes(
                &visited_ids,
                the_one_core::storage::sqlite::page_limits::NAVIGATION_TUNNELS_MAX,
            )?;
            // Only retain tunnels where BOTH endpoints were visited — mirrors
            // the v0.14.x semantics.
            let visited_set: HashSet<&str> = visited_ids.iter().map(String::as_str).collect();
            tunnels.retain(|tunnel| {
                visited_set.contains(tunnel.from_node_id.as_str())
                    && visited_set.contains(tunnel.to_node_id.as_str())
            });
            tunnels.sort_by(|left, right| {
                left.from_node_id
                    .cmp(&right.from_node_id)
                    .then(left.to_node_id.cmp(&right.to_node_id))
                    .then(left.tunnel_id.cmp(&right.tunnel_id))
            });

            if truncated {
                tracing::warn!(
                    target: "the_one_mcp::navigation",
                    project_id = %request.project_id,
                    visited = ordered_nodes.len(),
                    "navigation traverse truncated at MAX_TRAVERSE_NODES"
                );
            }

            Ok(MemoryNavigationTraverseResponse {
                nodes: ordered_nodes,
                tunnels,
                truncated,
            })
        })
        .await
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

        // Import catalog files if catalog is empty. Failures are logged but
        // don't block the caller — the catalog is a best-effort index that
        // the broker can rebuild on the next call.
        if catalog.tool_count()? == 0 {
            if let Some(dir) = Self::find_catalog_data_dir() {
                if let Err(err) = catalog.import_catalog_dir(&dir) {
                    tracing::warn!(
                        target: "the_one_mcp::catalog",
                        dir = %dir.display(),
                        error = %err,
                        "initial catalog import failed"
                    );
                }
            }
            if let Err(err) = catalog.scan_system_inventory() {
                tracing::warn!(
                    target: "the_one_mcp::catalog",
                    error = %err,
                    "initial system inventory scan failed"
                );
            }
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

            // Re-scan inventory to pick up the new binary. Log on failure
            // but don't fail the install — the binary is on disk either way.
            if let Err(err) = cat.scan_system_inventory() {
                tracing::warn!(
                    target: "the_one_mcp::catalog",
                    tool_id = %request.tool_id,
                    error = %err,
                    "post-install inventory scan failed"
                );
            }
            // Auto-enable — track the real outcome so we don't lie about
            // `auto_enabled: true` in the response when the enable call
            // actually failed (mempalace audit H1).
            let auto_enabled =
                match cat.enable_tool(&request.tool_id, "default", &request.project_root) {
                    Ok(_) => true,
                    Err(err) => {
                        tracing::warn!(
                            target: "the_one_mcp::catalog",
                            tool_id = %request.tool_id,
                            error = %err,
                            "post-install auto-enable failed"
                        );
                        false
                    }
                };

            let info = cat.get_tool(&request.tool_id)?;
            Ok(ToolInstallResponse {
                installed: true,
                binary_path: info.as_ref().and_then(|t| t.installed_path.clone()),
                version: info.as_ref().and_then(|t| t.installed_version.clone()),
                auto_enabled,
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

        // Try catalog FTS — this path is a fallback, so we log but continue
        // on failure. Prior to v0.15.0 the `self.ensure_catalog().ok()` call
        // silently swallowed catalog initialization failures, meaning the
        // subsequent search always hit the registry fallback.
        if let Err(err) = self.ensure_catalog() {
            tracing::debug!(
                target: "the_one_mcp::tool_search",
                error = %err,
                "catalog init unavailable, falling through to registry"
            );
        }
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

        if !request.interactive {
            // v0.16.0 Phase 1: session-approvals check must happen BEFORE
            // entering the state-store closure. The session_approvals
            // RwLock is tokio-async while the state store uses a sync
            // Mutex — interleaving them inside one closure would hold
            // the state-store lock across an `.await`, which the compiler
            // rejects (by design — see `with_state_store` docs).
            let session_approved = self
                .session_approvals
                .read()
                .await
                .contains(&request.action_key);

            return self
                .with_state_store(project_root, project_id, |store| {
                    if session_approved {
                        store.record_audit_event(
                            "tool_run",
                            "{\"mode\":\"headless\",\"result\":\"approved_session\"}",
                        )?;
                        return Ok(ToolRunResponse {
                            allowed: true,
                            reason: "approved by session policy".to_string(),
                        });
                    }
                    let approved =
                        store.is_approved(&request.action_key, ApprovalScope::Forever)?;
                    if approved {
                        store.record_audit_event(
                            "tool_run",
                            "{\"mode\":\"headless\",\"result\":\"approved\"}",
                        )?;
                        return Ok(ToolRunResponse {
                            allowed: true,
                            reason: "approved by persisted policy".to_string(),
                        });
                    }
                    store.record_audit_event(
                        "tool_run",
                        "{\"mode\":\"headless\",\"result\":\"denied\"}",
                    )?;
                    Ok(ToolRunResponse {
                        allowed: false,
                        reason: "headless mode denies unapproved high-risk action".to_string(),
                    })
                })
                .await;
        }

        // Interactive path. Session-scope approval mutates
        // `session_approvals` (async RwLock), which must happen outside
        // the state-store closure. Forever-scope approval is a sync DB
        // write inside the closure.
        let scope = match request.approval_scope.as_deref() {
            Some("once") => ApprovalScope::Once,
            Some("session") => ApprovalScope::Session,
            Some("forever") => ApprovalScope::Forever,
            _ => ApprovalScope::Once,
        };
        if matches!(scope, ApprovalScope::Session) {
            self.session_approvals
                .write()
                .await
                .insert(request.action_key.clone());
        }
        self.with_state_store(project_root, project_id, |store| {
            if matches!(scope, ApprovalScope::Forever) {
                store.set_approval(&request.action_key, ApprovalScope::Forever, true)?;
            }
            store.record_audit_event(
                "tool_run",
                "{\"mode\":\"interactive\",\"result\":\"approved\"}",
            )
        })
        .await?;

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
        let profile_json = self
            .with_state_store(
                Path::new(&request.project_root),
                &request.project_id,
                |store| store.latest_project_profile(),
            )
            .await?;
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
        let events = self
            .with_state_store(
                Path::new(&request.project_root),
                &request.project_id,
                |store| {
                    store.list_audit_events(request.limit).map(|items| {
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
                    })
                },
            )
            .await?;

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

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn parse_labeled_value(output: &str, label: &str) -> Option<String> {
        output.lines().find_map(|line| {
            let trimmed = line.trim();
            let prefix = format!("{label}:");
            trimmed
                .strip_prefix(&prefix)
                .map(|value| value.trim().to_string())
        })
    }

    fn parse_update_available(current: &str, latest: &str) -> Option<bool> {
        fn parse_semver(input: &str) -> Option<Vec<u64>> {
            let core = input
                .split(['-', '+'])
                .next()
                .map(str::trim)
                .unwrap_or_default();
            if core.is_empty() {
                return None;
            }
            let mut parts = Vec::new();
            for piece in core.split('.') {
                parts.push(piece.parse::<u64>().ok()?);
            }
            Some(parts)
        }

        let current_parts = parse_semver(current)?;
        let latest_parts = parse_semver(latest)?;
        let max_len = current_parts.len().max(latest_parts.len());
        for idx in 0..max_len {
            let c = current_parts.get(idx).copied().unwrap_or(0);
            let l = latest_parts.get(idx).copied().unwrap_or(0);
            if l > c {
                return Some(true);
            }
            if l < c {
                return Some(false);
            }
        }
        Some(false)
    }

    fn run_model_check_script(script_name: &str) -> serde_json::Value {
        let repo_root = Self::repo_root();
        let script_path = repo_root.join("scripts").join(script_name);
        if !script_path.exists() {
            return serde_json::json!({
                "name": script_name,
                "status": "error",
                "error": format!("script not found: {}", script_path.display())
            });
        }

        let output = std::process::Command::new("bash")
            .arg(&script_path)
            .current_dir(&repo_root)
            .output();

        match output {
            Ok(cmd) => {
                let stdout = String::from_utf8_lossy(&cmd.stdout).to_string();
                let stderr = String::from_utf8_lossy(&cmd.stderr).to_string();
                let mut json = serde_json::json!({
                    "name": script_name,
                    "status": if cmd.status.success() { "ok" } else { "error" },
                    "exit_code": cmd.status.code(),
                });

                if script_name == "update-local-models.sh" {
                    let current = Self::parse_labeled_value(&stdout, "Current fastembed version");
                    let latest = Self::parse_labeled_value(&stdout, "Latest fastembed version");
                    json["current_fastembed_version"] = serde_json::json!(current);
                    json["latest_fastembed_version"] = serde_json::json!(latest);
                    if let (Some(current), Some(latest)) = (
                        json["current_fastembed_version"].as_str(),
                        json["latest_fastembed_version"].as_str(),
                    ) {
                        json["update_available"] =
                            serde_json::json!(Self::parse_update_available(current, latest));
                    }
                }

                // Keep payload bounded and useful for operators.
                let stdout_excerpt: String = stdout.lines().take(40).collect::<Vec<_>>().join("\n");
                let stderr_excerpt: String = stderr.lines().take(20).collect::<Vec<_>>().join("\n");
                json["stdout_excerpt"] = serde_json::json!(stdout_excerpt);
                if !stderr_excerpt.trim().is_empty() {
                    json["stderr_excerpt"] = serde_json::json!(stderr_excerpt);
                }
                json
            }
            Err(err) => serde_json::json!({
                "name": script_name,
                "status": "error",
                "error": format!("failed to execute {}: {}", script_name, err),
            }),
        }
    }

    pub fn models_check_updates(&self) -> serde_json::Value {
        let local = Self::run_model_check_script("update-local-models.sh");
        let api = Self::run_model_check_script("update-api-models.sh");

        let any_error = local["status"] == "error" || api["status"] == "error";
        let update_available = local
            .get("update_available")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = if any_error {
            "degraded"
        } else if update_available {
            "updates_available"
        } else {
            "up_to_date"
        };

        serde_json::json!({
            "status": status,
            "checks": {
                "local_models": local,
                "api_models": api
            },
            "next_actions": [
                "Review check output excerpts for provider-specific changes.",
                "Run scripts/update-local-models.sh --apply and scripts/update-api-models.sh --apply after validating model additions/deprecations.",
                "Re-run cargo test -p the-one-memory models_registry before shipping registry updates."
            ]
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

        let mut result =
            the_one_memory::graph_extractor::extract_and_persist(project_root, &chunks)
                .await
                .map_err(CoreError::Embedding)?;

        // v0.13.1: reload the graph into the memory engine AND upsert
        // entity/relation descriptions into Qdrant vector collections so
        // Local/Global/Hybrid retrieval modes can do semantic search.
        let graph_path = project_root.join(".the-one").join("knowledge_graph.json");
        if graph_path.exists() {
            if let Ok(graph) = the_one_memory::graph::KnowledgeGraph::load_from_file(&graph_path) {
                let entities = graph.all_entities();
                let relations = graph.all_relations();

                let mut memories = self.memory_by_project.write().await;
                if let Some(engine) = memories.get_mut(&key) {
                    *engine.graph_mut() = graph;
                    match engine.upsert_entity_vectors(project_id, &entities).await {
                        Ok(n) if n > 0 => tracing::info!(
                            "upserted {n} entity vectors into qdrant for project {project_id}"
                        ),
                        Ok(_) => {}
                        Err(e) => result.errors.push(format!("upsert_entity_vectors: {e}")),
                    }
                    match engine.upsert_relation_vectors(project_id, &relations).await {
                        Ok(n) if n > 0 => tracing::info!(
                            "upserted {n} relation vectors into qdrant for project {project_id}"
                        ),
                        Ok(_) => {}
                        Err(e) => result.errors.push(format!("upsert_relation_vectors: {e}")),
                    }
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
        if let Some(v) = obj.get("memory_palace_enabled").and_then(|v| v.as_bool()) {
            update.memory_palace_enabled = Some(v);
        }
        if let Some(v) = obj
            .get("memory_palace_hooks_enabled")
            .and_then(|v| v.as_bool())
        {
            update.memory_palace_hooks_enabled = Some(v);
        }
        if let Some(v) = obj.get("memory_palace_profile").and_then(|v| v.as_str()) {
            update.memory_palace_profile = Some(v.to_string());
        }
        if let Some(v) = obj
            .get("memory_palace_aaak_enabled")
            .and_then(|v| v.as_bool())
        {
            update.memory_palace_aaak_enabled = Some(v);
        }
        if let Some(v) = obj
            .get("memory_palace_diary_enabled")
            .and_then(|v| v.as_bool())
        {
            update.memory_palace_diary_enabled = Some(v);
        }
        if let Some(v) = obj
            .get("memory_palace_navigation_enabled")
            .and_then(|v| v.as_bool())
        {
            update.memory_palace_navigation_enabled = Some(v);
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

fn current_time_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use the_one_core::contracts::{
        Capability, CapabilityType, MemoryNavigationNodeKind, RiskLevel, VisibilityMode,
    };
    use the_one_core::limits::ConfigurableLimits;
    use the_one_core::policy::PolicyEngine;

    use super::McpBroker;
    use crate::api::{
        AuditEventsRequest, DocsCreateRequest, DocsDeleteRequest, DocsGetSectionRequest,
        DocsListRequest, DocsMoveRequest, DocsReindexRequest, DocsTrashEmptyRequest,
        DocsTrashListRequest, DocsTrashRestoreRequest, DocsUpdateRequest, ImageSearchRequest,
        MemoryAaakCompressRequest, MemoryAaakListLessonsRequest, MemoryAaakTeachRequest,
        MemoryCaptureHookRequest, MemoryDiaryAddRequest, MemoryDiaryListRequest,
        MemoryDiarySearchRequest, MemoryDiarySummarizeRequest, MemoryIngestConversationRequest,
        MemoryNavigationLinkTunnelRequest, MemoryNavigationListRequest,
        MemoryNavigationTraverseRequest, MemoryNavigationUpsertNodeRequest, MemorySearchRequest,
        MemoryWakeUpRequest, ProjectInitRequest, ProjectProfileGetRequest, ProjectRefreshRequest,
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

    /// Phase 1 (v0.16.0): back-to-back `with_state_store` calls for the
    /// same `(project_root, project_id)` must reuse a single cached
    /// entry — we verify via `Arc::ptr_eq` on the internal cache slot so
    /// the check is independent of any backend-observable side effect.
    ///
    /// Also verifies:
    /// - A second project gets a *different* cache entry.
    /// - `McpBroker::shutdown()` drains every entry.
    #[tokio::test]
    async fn broker_state_store_cache_reuses_connections() {
        // Two distinct project roots so each can hold its own
        // `project_id` in its manifest — a single root is bound to a
        // single project id by `project_init`.
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root_a = temp.path().join("repo-a");
        let root_b = temp.path().join("repo-b");
        for root in [&root_a, &root_b] {
            fs::create_dir_all(root).expect("project dir should exist");
            fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
                .expect("cargo write should succeed");
        }

        let broker = McpBroker::new();
        broker
            .project_init(ProjectInitRequest {
                project_root: root_a.display().to_string(),
                project_id: "cache-test-a".to_string(),
            })
            .await
            .expect("init A should succeed");
        broker
            .project_init(ProjectInitRequest {
                project_root: root_b.display().to_string(),
                project_id: "cache-test-b".to_string(),
            })
            .await
            .expect("init B should succeed");

        // First access populates the cache.
        let entry_a_first = broker
            .get_or_init_state_store(&root_a, "cache-test-a")
            .await
            .expect("first store lookup should succeed");
        // Second access for the same project MUST return the same Arc.
        let entry_a_second = broker
            .get_or_init_state_store(&root_a, "cache-test-a")
            .await
            .expect("second store lookup should succeed");
        assert!(
            std::sync::Arc::ptr_eq(&entry_a_first, &entry_a_second),
            "repeated get_or_init for the same project must return the cached entry"
        );

        // A *different* project must get a *different* entry.
        let entry_b = broker
            .get_or_init_state_store(&root_b, "cache-test-b")
            .await
            .expect("project B lookup should succeed");
        assert!(
            !std::sync::Arc::ptr_eq(&entry_a_first, &entry_b),
            "distinct projects must get distinct state store entries"
        );

        // Cache should hold exactly two entries at this point.
        assert_eq!(
            broker.state_by_project.read().await.len(),
            2,
            "cache should hold one entry per project"
        );

        // `with_state_store` must route through the same cached entry —
        // a sanity check that it actually uses the cache.
        let project_id_from_store = broker
            .with_state_store(&root_a, "cache-test-a", |store| {
                Ok(store.project_id().to_string())
            })
            .await
            .expect("with_state_store should succeed");
        assert_eq!(project_id_from_store, "cache-test-a");

        // Drop all outstanding Arcs so `shutdown` can fully drain.
        drop(entry_a_first);
        drop(entry_a_second);
        drop(entry_b);

        broker.shutdown().await;
        assert!(
            broker.state_by_project.read().await.is_empty(),
            "shutdown should drain every cache entry"
        );
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
                wing: None,
                hall: None,
                room: None,
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
                wing: None,
                hall: None,
                room: None,
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
    async fn test_memory_ingest_conversation_indexes_transcript_content() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let transcript_path = root.join("transcript.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"system","content":"You are helpful"},
              {"role":"user","content":"Why did we switch auth vendors?"},
              {"role":"assistant","content":"Refresh tokens were failing in staging"}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let response = broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
            })
            .await
            .expect("conversation ingest should succeed");

        assert!(response.ingested_chunks >= 1);
        // v0.15.0: the response source_path is project-relative (or the
        // file stem) so we do not leak absolute host paths to the client.
        // The ingest target still stores the canonical absolute path in
        // SQLite so internal lookups work — we verify that via the search
        // below.
        assert!(
            response.source_path.ends_with("transcript.json"),
            "response source_path should end with the transcript filename, got {}",
            response.source_path
        );
        assert!(
            !response.source_path.starts_with('/'),
            "response source_path should be project-relative, got {}",
            response.source_path
        );

        let search = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search refresh tokens staging".to_string(),
                top_k: 5,
                wing: None,
                hall: None,
                room: None,
            })
            .await;
        assert!(
            !search.hits.is_empty(),
            "conversation text should be retrievable after ingest"
        );
        assert!(
            search.hits.iter().any(|hit| hit
                .source_path
                .contains(transcript_path.file_name().unwrap().to_str().unwrap())),
            "conversation memory should keep the transcript filename, got {:?}",
            search
                .hits
                .iter()
                .map(|h| &h.source_path)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn test_memory_aaak_compress_returns_lossless_payload() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_aaak_enabled":true}"#,
        )
        .expect("config write should succeed");

        let transcript_path = root.join("aaak-compress.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."},
              {"role":"user","content":"What was the mitigation?"},
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let response = broker
            .memory_aaak_compress(MemoryAaakCompressRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
            })
            .await
            .expect("aaak compression should succeed");

        assert!(!response.result.used_verbatim);
        assert_eq!(response.result.patterns.len(), 1);
        assert!(response
            .result
            .compressed_payload_json
            .contains("\"motif_ref\""));
    }

    #[tokio::test]
    async fn test_memory_aaak_teach_persists_lessons() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_aaak_enabled":true}"#,
        )
        .expect("config write should succeed");

        let transcript_path = root.join("aaak-teach.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."},
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let taught = broker
            .memory_aaak_teach(MemoryAaakTeachRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
            })
            .await
            .expect("aaak teach should succeed");

        assert_eq!(taught.outcome.lessons_written, 1);

        let listed = broker
            .memory_aaak_list_lessons(MemoryAaakListLessonsRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                limit: 10,
            })
            .await
            .expect("aaak list should succeed");
        assert_eq!(listed.lessons.len(), 1);
        assert_eq!(listed.lessons[0].occurrence_count, 2);
    }

    #[tokio::test]
    async fn test_memory_ingest_conversation_auto_teach_persists_aaak_lessons() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_aaak_enabled":true}"#,
        )
        .expect("config write should succeed");

        let transcript_path = root.join("aaak-auto-teach.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."},
              {"role":"assistant","content":"Refresh tokens were failing in staging due to issuer drift."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: None,
                hall: None,
                room: None,
            })
            .await
            .expect("conversation ingest should succeed");

        let listed = broker
            .memory_aaak_list_lessons(MemoryAaakListLessonsRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                limit: 10,
            })
            .await
            .expect("aaak list should succeed");
        assert_eq!(listed.lessons.len(), 1);
        assert!(listed.lessons[0]
            .canonical_text
            .contains("Refresh tokens were failing"));
    }

    #[tokio::test]
    async fn test_memory_diary_add_and_list_by_date_range() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .memory_diary_add(MemoryDiaryAddRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                entry_date: "2026-04-08".to_string(),
                mood: Some("tired".to_string()),
                tags: vec!["ops".to_string()],
                content: "Closed the incident follow-up tasks.".to_string(),
            })
            .await
            .expect("first diary add should succeed");
        broker
            .memory_diary_add(MemoryDiaryAddRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                entry_date: "2026-04-10".to_string(),
                mood: Some("focused".to_string()),
                tags: vec!["release".to_string(), "auth".to_string()],
                content: "Validated the release and auth migration checklist.".to_string(),
            })
            .await
            .expect("second diary add should succeed");

        let listed = broker
            .memory_diary_list(MemoryDiaryListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                start_date: Some("2026-04-09".to_string()),
                end_date: Some("2026-04-10".to_string()),
                max_results: 10,
            })
            .await
            .expect("diary list should succeed");

        assert_eq!(listed.entries.len(), 1);
        assert_eq!(listed.entries[0].entry_date, "2026-04-10");
        assert_eq!(listed.entries[0].mood.as_deref(), Some("focused"));
    }

    #[tokio::test]
    async fn test_memory_diary_search_returns_matches() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .memory_diary_add(MemoryDiaryAddRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                entry_date: "2026-04-10".to_string(),
                mood: Some("relieved".to_string()),
                tags: vec!["release".to_string(), "auth".to_string()],
                content: "Finished the release after fixing token refresh.".to_string(),
            })
            .await
            .expect("diary add should succeed");

        let found = broker
            .memory_diary_search(MemoryDiarySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "release".to_string(),
                start_date: None,
                end_date: None,
                max_results: 10,
            })
            .await
            .expect("diary search should succeed");

        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].tags, vec!["release", "auth"]);
    }

    #[tokio::test]
    async fn test_memory_diary_summarize_recent_entries() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        for (entry_date, content) in [
            (
                "2026-04-09",
                "Tracked the auth rollout blockers and documented the mitigation plan.",
            ),
            (
                "2026-04-10",
                "Validated the release checklist and confirmed refresh token recovery.",
            ),
        ] {
            broker
                .memory_diary_add(MemoryDiaryAddRequest {
                    project_root: root.display().to_string(),
                    project_id: "project-1".to_string(),
                    entry_date: entry_date.to_string(),
                    mood: Some("focused".to_string()),
                    tags: vec!["release".to_string()],
                    content: content.to_string(),
                })
                .await
                .expect("diary add should succeed");
        }

        let summary = broker
            .memory_diary_summarize(MemoryDiarySummarizeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                start_date: Some("2026-04-09".to_string()),
                end_date: Some("2026-04-10".to_string()),
                max_summary_items: 6,
            })
            .await
            .expect("diary summarize should succeed");

        assert_eq!(summary.summary.entry_count, 2);
        assert_eq!(summary.summary.date_from, Some("2026-04-09".to_string()));
        assert_eq!(summary.summary.date_to, Some("2026-04-10".to_string()));
        assert!(
            summary.summary.summary.contains("2 diar"),
            "unexpected summary text: {}",
            summary.summary.summary
        );
        assert!(
            summary
                .summary
                .highlights
                .iter()
                .any(|item| item.contains("release checklist") || item.contains("rollout blockers")),
            "expected highlights to capture recent diary facts: {:?}",
            summary.summary.highlights
        );
    }

    #[tokio::test]
    async fn test_memory_diary_add_refreshes_existing_entry_instead_of_duplicating() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_diary_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        let first = broker
            .memory_diary_add(MemoryDiaryAddRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                entry_date: "2026-04-10".to_string(),
                mood: Some("focused".to_string()),
                tags: vec!["release".to_string()],
                content: "Initial release checklist".to_string(),
            })
            .await
            .expect("first diary add should succeed");
        let second = broker
            .memory_diary_add(MemoryDiaryAddRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                entry_date: "2026-04-10".to_string(),
                mood: Some("relieved".to_string()),
                tags: vec!["release".to_string(), "auth".to_string()],
                content: "Updated release checklist after auth validation".to_string(),
            })
            .await
            .expect("refresh diary add should succeed");

        let listed = broker
            .memory_diary_list(MemoryDiaryListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                start_date: Some("2026-04-10".to_string()),
                end_date: Some("2026-04-10".to_string()),
                max_results: 10,
            })
            .await
            .expect("diary list should succeed");

        assert_eq!(listed.entries.len(), 1);
        assert_eq!(first.entry.entry_id, second.entry.entry_id);
        assert_eq!(
            first.entry.created_at_epoch_ms,
            second.entry.created_at_epoch_ms
        );
        assert_eq!(
            listed.entries[0].content,
            "Updated release checklist after auth validation"
        );
        assert_eq!(listed.entries[0].tags, vec!["release", "auth"]);
    }

    #[tokio::test]
    async fn test_navigation_upsert_and_list_drawer() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "drawer:ops".to_string(),
                kind: "drawer".to_string(),
                label: "Operations".to_string(),
                parent_node_id: None,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("drawer upsert should succeed");

        let listed = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                parent_node_id: None,
                kind: Some("drawer".to_string()),
                limit: 10,
                cursor: None,
            })
            .await
            .expect("navigation list should succeed");

        assert_eq!(listed.nodes.len(), 1);
        assert_eq!(listed.nodes[0].node_id, "drawer:ops");
        assert_eq!(listed.nodes[0].kind, MemoryNavigationNodeKind::Drawer);
    }

    #[tokio::test]
    async fn test_navigation_create_and_list_closet_under_drawer() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "drawer:ops".to_string(),
                kind: "drawer".to_string(),
                label: "Operations".to_string(),
                parent_node_id: None,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("drawer upsert should succeed");
        broker
            .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "closet:ops:incidents".to_string(),
                kind: "closet".to_string(),
                label: "Incidents".to_string(),
                parent_node_id: Some("drawer:ops".to_string()),
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: None,
            })
            .await
            .expect("closet upsert should succeed");

        let listed = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                parent_node_id: Some("drawer:ops".to_string()),
                kind: Some("closet".to_string()),
                limit: 10,
                cursor: None,
            })
            .await
            .expect("closet list should succeed");

        assert_eq!(listed.nodes.len(), 1);
        assert_eq!(
            listed.nodes[0].parent_node_id.as_deref(),
            Some("drawer:ops")
        );
    }

    #[tokio::test]
    async fn test_navigation_link_tunnel_between_two_nodes() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        for (node_id, label, wing) in [
            ("drawer:ops", "Operations", "ops"),
            ("drawer:platform", "Platform", "platform"),
        ] {
            broker
                .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                    project_root: root.display().to_string(),
                    project_id: "project-1".to_string(),
                    node_id: node_id.to_string(),
                    kind: "drawer".to_string(),
                    label: label.to_string(),
                    parent_node_id: None,
                    wing: Some(wing.to_string()),
                    hall: None,
                    room: None,
                })
                .await
                .expect("node upsert should succeed");
        }

        let linked = broker
            .memory_navigation_link_tunnel(MemoryNavigationLinkTunnelRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                from_node_id: "drawer:platform".to_string(),
                to_node_id: "drawer:ops".to_string(),
            })
            .await
            .expect("tunnel link should succeed");

        assert_eq!(linked.tunnel.from_node_id, "drawer:ops");
        assert_eq!(linked.tunnel.to_node_id, "drawer:platform");

        let listed = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                parent_node_id: None,
                kind: None,
                limit: 10,
                cursor: None,
            })
            .await
            .expect("navigation list should succeed");
        assert_eq!(listed.tunnels.len(), 1);
    }

    #[tokio::test]
    async fn test_navigation_traverse_returns_deterministic_path_ordering() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        for request in [
            MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "drawer:ops".to_string(),
                kind: "drawer".to_string(),
                label: "Operations".to_string(),
                parent_node_id: None,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            },
            MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "closet:ops:alpha".to_string(),
                kind: "closet".to_string(),
                label: "Alpha".to_string(),
                parent_node_id: Some("drawer:ops".to_string()),
                wing: Some("ops".to_string()),
                hall: Some("alpha".to_string()),
                room: None,
            },
            MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "closet:ops:beta".to_string(),
                kind: "closet".to_string(),
                label: "Beta".to_string(),
                parent_node_id: Some("drawer:ops".to_string()),
                wing: Some("ops".to_string()),
                hall: Some("beta".to_string()),
                room: None,
            },
            MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                node_id: "room:ops:delta".to_string(),
                kind: "room".to_string(),
                label: "Delta".to_string(),
                parent_node_id: Some("closet:ops:beta".to_string()),
                wing: Some("ops".to_string()),
                hall: Some("beta".to_string()),
                room: Some("delta".to_string()),
            },
        ] {
            broker
                .memory_navigation_upsert_node(request)
                .await
                .expect("navigation upsert should succeed");
        }

        broker
            .memory_navigation_link_tunnel(MemoryNavigationLinkTunnelRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                from_node_id: "closet:ops:alpha".to_string(),
                to_node_id: "room:ops:delta".to_string(),
            })
            .await
            .expect("tunnel link should succeed");

        let traversed = broker
            .memory_navigation_traverse(MemoryNavigationTraverseRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                start_node_id: "drawer:ops".to_string(),
                max_depth: 3,
            })
            .await
            .expect("navigation traverse should succeed");

        let ordered_ids = traversed
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ordered_ids,
            vec![
                "drawer:ops",
                "closet:ops:alpha",
                "closet:ops:beta",
                "room:ops:delta",
            ]
        );
        assert_eq!(traversed.tunnels.len(), 1);
    }

    #[tokio::test]
    async fn test_navigation_ingest_conversation_maps_wing_hall_room_into_nodes() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let transcript_path = root.join("nav-map.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Document the auth incident remediation path."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
            })
            .await
            .expect("conversation ingest should succeed");

        let listed = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                parent_node_id: None,
                kind: None,
                limit: 10,
                cursor: None,
            })
            .await
            .expect("navigation list should succeed");

        assert_eq!(listed.nodes.len(), 3);
        assert!(listed
            .nodes
            .iter()
            .any(|node| node.kind == MemoryNavigationNodeKind::Drawer
                && node.wing.as_deref() == Some("ops")));
        assert!(listed
            .nodes
            .iter()
            .any(|node| node.kind == MemoryNavigationNodeKind::Closet
                && node.hall.as_deref() == Some("incidents")));
        assert!(listed
            .nodes
            .iter()
            .any(|node| node.kind == MemoryNavigationNodeKind::Room
                && node.room.as_deref() == Some("auth")));
    }

    #[tokio::test]
    async fn test_navigation_same_node_id_is_isolated_per_project_in_same_root() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_navigation_enabled":true}"#,
        )
        .expect("config write should succeed");

        let broker = McpBroker::new();
        broker
            .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-a".to_string(),
                node_id: "drawer:shared".to_string(),
                kind: "drawer".to_string(),
                label: "Project A Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-a".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("project-a node upsert should succeed");
        broker
            .memory_navigation_upsert_node(MemoryNavigationUpsertNodeRequest {
                project_root: root.display().to_string(),
                project_id: "project-b".to_string(),
                node_id: "drawer:shared".to_string(),
                kind: "drawer".to_string(),
                label: "Project B Drawer".to_string(),
                parent_node_id: None,
                wing: Some("ops-b".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("project-b node upsert should succeed");

        let listed_a = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-a".to_string(),
                parent_node_id: None,
                kind: Some("drawer".to_string()),
                limit: 10,
                cursor: None,
            })
            .await
            .expect("project-a list should succeed");
        let listed_b = broker
            .memory_navigation_list(MemoryNavigationListRequest {
                project_root: root.display().to_string(),
                project_id: "project-b".to_string(),
                parent_node_id: None,
                kind: Some("drawer".to_string()),
                limit: 10,
                cursor: None,
            })
            .await
            .expect("project-b list should succeed");

        assert_eq!(listed_a.nodes.len(), 1);
        assert_eq!(listed_b.nodes.len(), 1);
        assert_eq!(listed_a.nodes[0].label, "Project A Drawer");
        assert_eq!(listed_b.nodes[0].label, "Project B Drawer");
    }

    #[tokio::test]
    async fn memory_search_can_filter_by_wing_and_room() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let auth_transcript_path = root.join("ops-auth-transcript.json");
        fs::write(
            &auth_transcript_path,
            r#"[
              {"role":"assistant","content":"palace-filter-smoke-test shared incident thread for room auth."}
            ]"#,
        )
        .expect("auth transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: auth_transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
            })
            .await
            .expect("auth conversation ingest should succeed");

        let unfiltered = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search palace-filter-smoke-test shared incident thread".to_string(),
                top_k: 10,
                wing: None,
                hall: None,
                room: None,
            })
            .await;
        assert!(
            unfiltered
                .hits
                .iter()
                .any(|hit| hit.source_path == auth_transcript_path.display().to_string()),
            "unfiltered search should include the ingested conversation"
        );

        let filtered = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search palace-filter-smoke-test shared incident thread".to_string(),
                top_k: 10,
                wing: Some("ops".to_string()),
                hall: None,
                room: Some("auth".to_string()),
            })
            .await;

        assert!(
            !filtered.hits.is_empty(),
            "filtered search should still return the matching conversation room"
        );
        assert!(filtered
            .hits
            .iter()
            .all(|hit| hit.source_path == auth_transcript_path.display().to_string()));

        let wrong_room = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search palace-filter-smoke-test shared incident thread".to_string(),
                top_k: 10,
                wing: Some("ops".to_string()),
                hall: None,
                room: Some("pager".to_string()),
            })
            .await;
        assert!(
            wrong_room.hits.is_empty(),
            "room filtering should exclude conversations stored in a different room"
        );

        let wrong_wing = broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search palace-filter-smoke-test shared incident thread".to_string(),
                top_k: 10,
                wing: Some("support".to_string()),
                hall: None,
                room: Some("auth".to_string()),
            })
            .await;
        assert!(
            wrong_wing.hits.is_empty(),
            "wing filtering should exclude conversations stored in a different wing"
        );
    }

    #[tokio::test]
    async fn test_memory_wake_up_returns_facts_for_ingested_conversation() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let transcript_path = root.join("transcript.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"user","content":"We switched auth vendors after refresh-token failures."},
              {"role":"assistant","content":"The staging outage was fixed by rotating the issuer config."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("conversation ingest should succeed");

        let wake_up = broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
                max_items: 4,
            })
            .await
            .expect("wake-up pack should succeed");

        assert!(!wake_up.summary.is_empty());
        assert!(
            !wake_up.facts.is_empty(),
            "wake-up facts should include conversation-derived memory"
        );
    }

    #[tokio::test]
    async fn test_memory_wake_up_reloads_persisted_conversations_after_broker_restart() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let transcript_path = root.join("restart-transcript.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"user","content":"Why did we replace the auth vendor?"},
              {"role":"assistant","content":"Because refresh token rotation kept failing in staging."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: None,
            })
            .await
            .expect("conversation ingest should succeed");

        let restarted_broker = McpBroker::new();
        let wake_up = restarted_broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: None,
                max_items: 4,
            })
            .await
            .expect("wake-up pack should succeed after restart");

        assert!(
            wake_up
                .facts
                .iter()
                .any(|fact| fact.contains("refresh token rotation kept failing in staging")),
            "wake-up should reload facts from persisted conversation metadata after restart"
        );
    }

    #[tokio::test]
    async fn test_memory_wake_up_filters_by_hall_and_room() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let auth_transcript_path = root.join("auth-transcript.json");
        fs::write(
            &auth_transcript_path,
            r#"[
              {"role":"assistant","content":"Auth room incident: refresh token issuer mismatch."}
            ]"#,
        )
        .expect("auth transcript should be written");

        let pager_transcript_path = root.join("pager-transcript.json");
        fs::write(
            &pager_transcript_path,
            r#"[
              {"role":"assistant","content":"Pager room incident: alert fan-out saturation."}
            ]"#,
        )
        .expect("pager transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: auth_transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
            })
            .await
            .expect("auth conversation ingest should succeed");

        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: pager_transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("pager".to_string()),
            })
            .await
            .expect("pager conversation ingest should succeed");

        let wake_up = broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
                max_items: 8,
            })
            .await
            .expect("wake-up pack should succeed");

        assert!(
            wake_up
                .facts
                .iter()
                .any(|fact| fact.contains("refresh token issuer mismatch")),
            "wake-up should include facts from matching hall+room"
        );
        assert!(
            wake_up
                .facts
                .iter()
                .all(|fact| !fact.contains("alert fan-out saturation")),
            "wake-up should exclude facts from non-matching room"
        );
    }

    #[tokio::test]
    async fn test_memory_palace_feature_toggle_blocks_conversation_features() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":false}"#,
        )
        .expect("config should be written");

        let transcript_path = root.join("disabled-palace.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"This should not ingest when palace is disabled."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let ingest_error = broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: None,
                hall: None,
                room: None,
            })
            .await
            .expect_err("ingest should be disabled");
        assert!(ingest_error
            .to_string()
            .contains("memory palace features are disabled"));

        let wake_error = broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: None,
                hall: None,
                room: None,
                max_items: 4,
            })
            .await
            .expect_err("wake-up should be disabled");
        assert!(wake_error
            .to_string()
            .contains("memory palace features are disabled"));
    }

    #[tokio::test]
    async fn test_memory_capture_hook_defaults_and_ingests_with_hook_metadata() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_hooks_enabled":true}"#,
        )
        .expect("config should be written");

        let transcript_path = root.join("hook-precompact.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Hook capture should persist this memory."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let captured = broker
            .memory_capture_hook(MemoryCaptureHookRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                event: crate::api::MemoryHookEvent::PreCompact,
                wing: None,
                hall: None,
                room: None,
            })
            .await
            .expect("hook capture should succeed");
        assert_eq!(captured.event, "precompact");
        assert_eq!(captured.wing, "project-1");
        assert_eq!(captured.hall, "hook:precompact");
        assert_eq!(captured.room, "event:precompact");
        assert!(captured.ingested_chunks >= 1);

        let wake_up = broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: Some("project-1".to_string()),
                hall: Some("hook:precompact".to_string()),
                room: Some("event:precompact".to_string()),
                max_items: 4,
            })
            .await
            .expect("wake-up should include hook-ingested facts");

        assert!(wake_up
            .facts
            .iter()
            .any(|fact| fact.contains("Hook capture should persist this memory")));
    }

    #[tokio::test]
    async fn test_memory_capture_hook_requires_hook_flag() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        let state_dir = root.join(".the-one");
        fs::create_dir_all(&state_dir).expect("state dir should exist");
        fs::write(
            state_dir.join("config.json"),
            r#"{"memory_palace_enabled":true,"memory_palace_hooks_enabled":false}"#,
        )
        .expect("config should be written");

        let transcript_path = root.join("hook-stop.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Hook capture should fail when disabled."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        let error = broker
            .memory_capture_hook(MemoryCaptureHookRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                event: crate::api::MemoryHookEvent::Stop,
                wing: None,
                hall: None,
                room: None,
            })
            .await
            .expect_err("hook capture should be disabled");
        assert!(error
            .to_string()
            .contains("memory palace hook capture is disabled"));
    }

    #[tokio::test]
    async fn test_memory_search_reloads_persisted_conversations_after_broker_restart() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let transcript_path = root.join("restart-search-transcript.json");
        fs::write(
            &transcript_path,
            r#"[
              {"role":"assistant","content":"Search reload should find persisted conversation memory after restart."}
            ]"#,
        )
        .expect("transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: Some("incidents".to_string()),
                room: Some("auth".to_string()),
            })
            .await
            .expect("conversation ingest should succeed");

        let restarted_broker = McpBroker::new();
        let search = restarted_broker
            .memory_search(MemorySearchRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                query: "search persisted conversation memory after restart".to_string(),
                top_k: 5,
                wing: Some("ops".to_string()),
                hall: None,
                room: Some("auth".to_string()),
            })
            .await;

        assert!(
            search
                .hits
                .iter()
                .any(|hit| hit.source_path == transcript_path.display().to_string()),
            "memory search should reload persisted conversation metadata after restart"
        );
    }

    #[tokio::test]
    async fn test_memory_ingest_conversation_fallback_preserves_prior_conversation_state() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("project dir should exist");

        let first_transcript_path = root.join("first-transcript.json");
        fs::write(
            &first_transcript_path,
            r#"[
              {"role":"assistant","content":"The first incident was caused by stale issuer config."}
            ]"#,
        )
        .expect("first transcript should be written");

        let second_transcript_path = root.join("second-transcript.json");
        fs::write(
            &second_transcript_path,
            r#"[
              {"role":"assistant","content":"The second incident was caused by refresh token rotation failures."}
            ]"#,
        )
        .expect("second transcript should be written");

        let broker = McpBroker::new();
        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: first_transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("first conversation ingest should succeed");

        let replacement_engine = broker
            .build_memory_engine(&root, "project-1")
            .await
            .expect("replacement engine should build");
        let key = McpBroker::project_memory_key(&root, "project-1");
        broker
            .memory_by_project
            .write()
            .await
            .insert(key, replacement_engine);

        broker
            .memory_ingest_conversation(MemoryIngestConversationRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                path: second_transcript_path.display().to_string(),
                format: crate::api::MemoryConversationFormat::OpenAiMessages,
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
            })
            .await
            .expect("second conversation ingest should succeed");

        let wake_up = broker
            .memory_wake_up(MemoryWakeUpRequest {
                project_root: root.display().to_string(),
                project_id: "project-1".to_string(),
                wing: Some("ops".to_string()),
                hall: None,
                room: None,
                max_items: 8,
            })
            .await
            .expect("wake-up pack should succeed");

        assert!(
            wake_up
                .facts
                .iter()
                .any(|fact| fact.contains("stale issuer config")),
            "fallback ingest should preserve previously persisted conversation state"
        );
        assert!(
            wake_up
                .facts
                .iter()
                .any(|fact| fact.contains("refresh token rotation failures")),
            "fallback ingest should include the newly ingested conversation"
        );
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
                wing: None,
                hall: None,
                room: None,
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
                wing: None,
                hall: None,
                room: None,
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
        assert!(result["status"].is_string());
        assert!(result["checks"]["local_models"]["name"].is_string());
        assert!(result["checks"]["api_models"]["name"].is_string());
        assert!(result["next_actions"].is_array());
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
