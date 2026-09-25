#![forbid(unsafe_code)]

//! Authenticated fixed-target Eggwork node service, built on EggServe.

mod artifact;
mod blob;
pub mod operations;
mod store;
mod workspace;

use bytes::Bytes;
use eggserve_core::{
    primitives::{
        canonical::{Response, ResponseBody, ResponseStream, ResponseStreamError, StatusCode},
        request::Request,
    },
    server::{RuntimeConfig, Server, ServerHandle, Service, ServiceFuture},
    tls::{ClientAuthMode, TlsServerConfig},
};
use eggwork_core::{
    ApiError, ExecutionEvent, ExecutionEventKind, ExecutionFailure, ExecutionFinalizationFailure,
    ExecutionGeneration, ExecutionHandle, ExecutionId, ExecutionResult, ExecutionSnapshot,
    ExecutionSpec, ExecutionState, LeaseId, NodeCapabilities, NodeId, NodeStatus, ProtocolVersion,
    ProtocolVersionRange,
};
use eggwork_runner::{
    ExecutionProvenance, LocalProcessRunner, ResourceSetupRequest, RunnerError, RunnerRequest,
    SandboxRequest,
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

const API_SCHEMA_VERSION: u16 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const EVENT_CAPACITY: usize = 32;
const RUNNER_CHANNEL_CAPACITY: usize = 32;
const MAX_RECENT_EXECUTIONS: usize = 1024;
const MAX_BLOB_FIND_REQUEST_BYTES: usize = 64 * 1024;
const MAX_WORKSPACE_REQUEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Capabilities,
    Status,
    Execute,
    Observe,
    Cancel,
    Renew,
    Events,
    BlobRead,
    BlobWrite,
    WorkspaceCreate,
    ArtifactRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceId {
    Execution(ExecutionId),
    Blob(eggwork_core::BlobDigest),
    Workspace(eggwork_core::WorkspaceId),
    Artifact(eggwork_core::ArtifactId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRequest {
    pub operation: Operation,
    pub resource: Option<ResourceId>,
}

pub trait PeerPrincipalResolver: Send + Sync + 'static {
    fn resolve_verified_leaf(&self, leaf_der: &[u8]) -> Option<NodePrincipal>;
}

#[derive(Debug, Clone)]
pub struct NodePrincipal {
    pub id: eggwork_core::PrincipalId,
}

/// Resolves verified TLS leaf certificates by lowercase SHA-256 fingerprint.
#[derive(Debug, Default)]
pub struct FingerprintPrincipalResolver {
    principals: HashMap<String, eggwork_core::PrincipalId>,
}

impl FingerprintPrincipalResolver {
    pub fn new(entries: impl IntoIterator<Item = (String, eggwork_core::PrincipalId)>) -> Self {
        Self {
            principals: entries.into_iter().collect(),
        }
    }

    pub fn fingerprint(leaf_der: &[u8]) -> String {
        hex::encode(Sha256::digest(leaf_der))
    }
}

impl PeerPrincipalResolver for FingerprintPrincipalResolver {
    fn resolve_verified_leaf(&self, leaf_der: &[u8]) -> Option<NodePrincipal> {
        self.principals
            .get(&Self::fingerprint(leaf_der))
            .cloned()
            .map(|id| NodePrincipal { id })
    }
}

pub trait Authorizer: Send + Sync + 'static {
    fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool;

    /// Resource-aware hook. Existing operation-wide policies remain valid;
    /// scoped policies can override this method to inspect authenticated IDs.
    fn authorize_request(&self, principal: &NodePrincipal, request: &OperationRequest) -> bool {
        self.authorize(principal, request.operation)
    }
}

impl<F> Authorizer for F
where
    F: Fn(&NodePrincipal, Operation) -> bool + Send + Sync + 'static,
{
    fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool {
        self(principal, operation)
    }
}

#[derive(Error)]
pub enum NodeStartError {
    #[error("remote execution requires a valid server identity and required client authentication")]
    TlsPolicy,
    #[error("node service configuration is invalid")]
    EggServe(String),
    #[error("maximum active execution count must be positive")]
    InvalidLimit,
    #[error("execution lease duration must be positive")]
    InvalidLease,
    #[error("another node process already owns this state directory")]
    AlreadyRunning,
}

impl std::fmt::Debug for NodeStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TlsPolicy => f.write_str("NodeStartError::TlsPolicy"),
            Self::EggServe(_) => f.write_str("NodeStartError::EggServe([REDACTED])"),
            Self::InvalidLimit => f.write_str("NodeStartError::InvalidLimit"),
            Self::InvalidLease => f.write_str("NodeStartError::InvalidLease"),
            Self::AlreadyRunning => f.write_str("NodeStartError::AlreadyRunning"),
        }
    }
}

#[derive(Clone)]
pub struct NodeConfig {
    pub node_id: NodeId,
    pub bind: SocketAddr,
    pub execution_root: PathBuf,
    pub database_path: PathBuf,
    pub blob_root: PathBuf,
    pub blob_quota_bytes: u64,
    pub workspace_root: PathBuf,
    pub workspace_quota_bytes: u64,
    pub max_active_executions: u32,
    pub lease_ttl: Duration,
    pub tls: TlsServerConfig,
}

#[derive(Clone)]
struct NodeState {
    node_id: NodeId,
    execution_root: PathBuf,
    max_active: u32,
    permits: Arc<Semaphore>,
    draining: Arc<AtomicBool>,
    drain_path: PathBuf,
    runner: Arc<LocalProcessRunner>,
    store: store::ExecutionStore,
    blobs: blob::BlobStore,
    workspaces: workspace::WorkspaceManager,
    artifacts: artifact::ArtifactStore,
    lease_ttl: Duration,
    capabilities_features: Vec<String>,
    resolver: Arc<dyn PeerPrincipalResolver>,
    authorizer: Arc<dyn Authorizer>,
    executions: Arc<Mutex<HashMap<ExecutionId, Arc<ExecutionRecord>>>>,
}

struct ExecutionRecord {
    snapshot: RwLock<ExecutionSnapshot>,
    cancellation: CancellationToken,
    events: broadcast::Sender<ExecutionEvent>,
    store: store::ExecutionStore,
    draining: Arc<AtomicBool>,
    finished: AtomicBool,
    lease_hash: String,
    lease: tokio::sync::Mutex<LeaseState>,
}

#[derive(Clone)]
struct WorkspaceCapture {
    id: eggwork_core::WorkspaceId,
    root: PathBuf,
    principal: eggwork_core::PrincipalId,
    outputs: Vec<eggwork_core::DeclaredOutput>,
}

struct LeaseState {
    expires_at: tokio::time::Instant,
    notify: Arc<tokio::sync::Notify>,
    expired: Arc<AtomicBool>,
}

pub struct NodeServer {
    handle: ServerHandle,
    state: NodeState,
    _state_lock: std::fs::File,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NodeGcReport {
    pub workspace_candidates: u64,
    pub workspace_logical_bytes: u64,
    pub workspaces_deleted: u64,
    pub manifest_candidates: u64,
    pub manifests_deleted: u64,
    pub artifact_candidates: u64,
    pub artifacts_deleted: u64,
    pub expired_blob_references: u64,
    pub blob_candidates: u64,
    pub blob_candidate_bytes: u64,
    pub blobs_deleted: u64,
    pub blob_bytes_deleted: u64,
    pub dry_run: bool,
}

struct NodeHttpService(NodeState);

impl Service for NodeHttpService {
    fn request_body_policy(
        &self,
        head: &eggserve_core::primitives::request_head::RequestHead,
    ) -> eggserve_core::primitives::request_body_policy::RequestBodyPolicy {
        use eggserve_core::primitives::request_body_policy::RequestBodyPolicy;
        let path = head.target().path();
        match (head.method().as_str(), path) {
            ("POST", "/v1/blobs/missing" | "/v1/blobs/prepare") => RequestBodyPolicy::Buffer {
                max_bytes: MAX_BLOB_FIND_REQUEST_BYTES as u64,
            },
            ("POST", "/v1/workspaces") => RequestBodyPolicy::Buffer {
                max_bytes: MAX_WORKSPACE_REQUEST_BYTES as u64,
            },
            ("POST", "/v1/workspaces/derive") => RequestBodyPolicy::Buffer {
                max_bytes: MAX_WORKSPACE_REQUEST_BYTES as u64,
            },
            ("PUT", path)
                if path.strip_prefix("/v1/blobs/").is_some_and(|digest| {
                    eggwork_core::BlobDigest::parse(digest.to_owned()).is_ok()
                }) =>
            {
                RequestBodyPolicy::Stream {
                    max_bytes: blob::MAX_BLOB_BYTES,
                }
            }
            ("POST", "/v1/executions") => RequestBodyPolicy::Buffer {
                max_bytes: MAX_REQUEST_BYTES as u64,
            },
            ("POST", path) if path.ends_with("/cancel") || path.ends_with("/renew") => {
                RequestBodyPolicy::Buffer {
                    max_bytes: MAX_REQUEST_BYTES as u64,
                }
            }
            _ => RequestBodyPolicy::Reject,
        }
    }

    fn call(&self, request: Request) -> ServiceFuture<'_> {
        let state = self.0.clone();
        Box::pin(async move { dispatch(state, request).await })
    }
}

impl NodeServer {
    pub async fn start(
        config: NodeConfig,
        runner: Arc<LocalProcessRunner>,
        resolver: Arc<dyn PeerPrincipalResolver>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Result<Self, NodeStartError> {
        if config.max_active_executions == 0
            || config.max_active_executions as usize > Semaphore::MAX_PERMITS
        {
            return Err(NodeStartError::InvalidLimit);
        }
        if config.tls.client_auth() != ClientAuthMode::Required
            || !config.tls.has_default_identity()
        {
            return Err(NodeStartError::TlsPolicy);
        }
        if config.lease_ttl.is_zero() {
            return Err(NodeStartError::InvalidLease);
        }
        let state_lock = acquire_state_lock(&config.database_path)?;
        let store = store::ExecutionStore::open(&config.database_path)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let blobs = blob::BlobStore::open(&config.blob_root, config.blob_quota_bytes)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let workspaces =
            workspace::WorkspaceManager::open(&config.workspace_root, config.workspace_quota_bytes)
                .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let artifact_path = config.database_path.with_extension("artifacts.sqlite");
        let artifacts = artifact::ArtifactStore::open(&artifact_path)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let recovered = store
            .recover()
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        if !recovered.is_empty() {
            increment_metric(&store, "terminal_interrupted", recovered.len() as u64).await;
        }
        workspaces
            .recover_retention(
                artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS),
                &blobs,
            )
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        workspaces
            .recover_manifest_retention(
                artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS),
            )
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        workspaces
            .reconcile_manifests(&blobs, artifact::now_unix_ms(), 1024)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let drain_path = config.database_path.with_extension("drain");
        let draining = Arc::new(AtomicBool::new(operations::is_persistently_draining(
            &drain_path,
        )));
        let resource_capabilities = runner.execution_capabilities().await;
        let capabilities_features = server_capability_features(resource_capabilities);
        let mut executions = HashMap::new();
        for snapshot in store
            .load_all()
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?
        {
            let (events, _) = broadcast::channel(EVENT_CAPACITY);
            executions.insert(
                snapshot.execution_id.clone(),
                Arc::new(ExecutionRecord {
                    snapshot: RwLock::new(snapshot),
                    cancellation: CancellationToken::new(),
                    events,
                    store: store.clone(),
                    draining: draining.clone(),
                    finished: AtomicBool::new(true),
                    lease_hash: String::new(),
                    lease: tokio::sync::Mutex::new(LeaseState {
                        expires_at: tokio::time::Instant::now(),
                        notify: Arc::new(tokio::sync::Notify::new()),
                        expired: Arc::new(AtomicBool::new(false)),
                    }),
                }),
            );
        }
        let state = NodeState {
            node_id: config.node_id,
            execution_root: config.execution_root,
            max_active: config.max_active_executions,
            permits: Arc::new(Semaphore::new(config.max_active_executions as usize)),
            draining,
            drain_path,
            runner,
            store,
            blobs,
            workspaces,
            artifacts,
            lease_ttl: config.lease_ttl,
            capabilities_features,
            resolver,
            authorizer,
            executions: Arc::new(Mutex::new(executions)),
        };
        let runtime = RuntimeConfig::builder()
            .bind(config.bind)
            .max_request_body_bytes(blob::MAX_BLOB_BYTES)
            .tls_config(config.tls.into_server_config())
            .tls_expose_peer_chain(true)
            .build()
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let service = NodeHttpService(state.clone());
        let server = Server::builder()
            .runtime(runtime)
            .bind(config.bind)
            .build()
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let handle = server
            .start_with_service(service)
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        handle
            .ready()
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        Ok(Self {
            handle,
            state,
            _state_lock: state_lock,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    pub fn set_draining(&self, draining: bool) {
        if operations::set_persistent_drain(&self.state.drain_path, draining).is_ok() {
            self.state.draining.store(draining, Ordering::Release);
        }
    }

    pub fn is_draining(&self) -> bool {
        self.state.draining.load(Ordering::Acquire)
            || operations::is_persistently_draining(&self.state.drain_path)
    }

    pub async fn shutdown(&self) {
        self.state.draining.store(true, Ordering::Release);
        let records: Vec<_> = self
            .state
            .executions
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for record in records {
            if !record.finished.load(Ordering::Acquire)
                && !is_terminal(&record.snapshot.read().await.state)
            {
                record.cancellation.cancel();
            }
        }
        self.handle.shutdown();
    }

    /// Run a bounded local maintenance pass; no remote GC route is exposed.
    pub async fn collect_garbage(
        &self,
        dry_run: bool,
        requested_limit: usize,
    ) -> Result<NodeGcReport, String> {
        let limit = requested_limit.clamp(1, 1024);
        let now = artifact::now_unix_ms();
        let (workspace_candidates, workspace_logical_bytes, workspaces_deleted) = self
            .state
            .workspaces
            .garbage_collect(now, limit, dry_run)
            .map_err(|error| error.to_string())?;
        let (manifest_candidates, manifests_deleted) = self
            .state
            .workspaces
            .manifest_garbage_collect(now, limit, dry_run, &self.state.blobs)
            .map_err(|error| error.to_string())?;
        let artifact_report = self
            .state
            .artifacts
            .garbage_collect(now, limit, dry_run)
            .map_err(|error| error.to_string())?;
        let blob_report = self
            .state
            .blobs
            .garbage_collect(now, limit, dry_run)
            .await
            .map_err(|error| error.to_string())?;
        Ok(NodeGcReport {
            workspace_candidates,
            workspace_logical_bytes,
            workspaces_deleted,
            manifest_candidates,
            manifests_deleted,
            artifact_candidates: artifact_report.candidate_artifacts,
            artifacts_deleted: artifact_report.deleted_artifacts,
            expired_blob_references: if dry_run {
                blob_report.expired_references
            } else {
                blob_report.expired_references_removed
            },
            blob_candidates: blob_report.candidate_blobs,
            blob_candidate_bytes: blob_report.candidate_bytes,
            blobs_deleted: blob_report.removed_blobs,
            blob_bytes_deleted: blob_report.removed_bytes,
            dry_run,
        })
    }

    pub async fn wait(self) -> Result<(), NodeStartError> {
        self.handle
            .wait()
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        loop {
            let records: Vec<_> = self
                .state
                .executions
                .lock()
                .await
                .values()
                .cloned()
                .collect();
            let mut active = false;
            for record in records {
                if !record.finished.load(Ordering::Acquire)
                    && !is_terminal(&record.snapshot.read().await.state)
                {
                    active = true;
                    break;
                }
            }
            if !active {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

fn acquire_state_lock(database_path: &std::path::Path) -> Result<std::fs::File, NodeStartError> {
    use fs2::FileExt;
    let lock_path = database_path.with_extension("lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(lock_path)
        .map_err(|error| NodeStartError::EggServe(error.to_string()))?;
    file.try_lock_exclusive()
        .map_err(|_| NodeStartError::AlreadyRunning)?;
    Ok(file)
}

/// Static protocol/transport/workspace/artifact feature list. Network is
/// always advertised as unrestricted because Eggwork does not yet implement a
/// network-restriction backend; the admission gate therefore rejects
/// `Disabled` and `AllowListed` requests with `capability_mismatch` instead of
/// silently downgrading them.
fn static_capability_features() -> &'static [&'static str] {
    &[
        "exec.argv.v1",
        "events.live.v1",
        "auth.mtls.v1",
        "blob.sha256.v1",
        "blob.stream.v1",
        "workspace.manifest.v1",
        "workspace.materialize.v1",
        "workspace.derive.v1",
        "artifact.declared.v1",
        "artifact.stream.v1",
        "artifact.retention.v1",
        "network.unrestricted.v1",
    ]
}

/// One capability feature construction. Both `Route::Capabilities` and
/// `NodeStatus.capabilities` MUST go through this function so a single node
/// always reports the same execution-capability feature set.
fn server_capability_features(dynamic: Vec<String>) -> Vec<String> {
    let mut features: Vec<String> = static_capability_features()
        .iter()
        .map(|feature| (*feature).to_owned())
        .collect();
    features.extend(dynamic);
    features.sort();
    features.dedup();
    features
}

/// Build a wire `NodeCapabilities` from the cached static+dynamic feature set.
fn node_capabilities_for(state: &NodeState) -> NodeCapabilities {
    NodeCapabilities {
        protocol: ProtocolVersionRange {
            min: ProtocolVersion { major: 1, minor: 0 },
            max: ProtocolVersion { major: 1, minor: 0 },
        },
        features: state.capabilities_features.clone(),
        max_active_executions: state.max_active,
    }
}

fn check_isolation_supported(
    state: &NodeState,
    isolation: &eggwork_core::IsolationRequirement,
) -> bool {
    match isolation {
        eggwork_core::IsolationRequirement::None
        | eggwork_core::IsolationRequirement::BestEffort => true,
        eggwork_core::IsolationRequirement::Required => state
            .capabilities_features
            .iter()
            .any(|feature| feature == eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY),
    }
}

fn check_resources_supported(
    state: &NodeState,
    resources: &eggwork_core::ResourceRequirements,
) -> bool {
    let has = |feature: &str| state.capabilities_features.iter().any(|f| f == feature);
    let memory_ok = !matches!(
        resources.memory_bytes,
        eggwork_core::Requirement::Required(_)
    ) || has("resources.cgroups-v2.memory");
    let cpu_ok = !matches!(resources.cpu_millis, eggwork_core::Requirement::Required(_))
        || has("resources.cgroups-v2.cpu");
    let pids_ok = !matches!(resources.pids, eggwork_core::Requirement::Required(_))
        || has("resources.cgroups-v2.pids");
    memory_ok && cpu_ok && pids_ok
}

fn check_network_supported(network: &eggwork_core::NetworkRequirement) -> bool {
    matches!(network, eggwork_core::NetworkRequirement::Unrestricted)
}

fn operation_for(method: &str, path: &str) -> Option<(Operation, Route)> {
    match (method, path) {
        ("GET", "/v1/capabilities") => Some((Operation::Capabilities, Route::Capabilities)),
        ("GET", "/v1/status") => Some((Operation::Status, Route::Status)),
        ("POST", "/v1/executions") => Some((Operation::Execute, Route::Execute)),
        ("POST", "/v1/blobs/missing") => Some((Operation::BlobRead, Route::BlobMissing)),
        ("POST", "/v1/blobs/prepare") => Some((Operation::BlobWrite, Route::BlobPrepare)),
        ("POST", "/v1/workspaces") => Some((Operation::WorkspaceCreate, Route::WorkspaceCreate)),
        ("POST", "/v1/workspaces/derive") => {
            Some((Operation::WorkspaceCreate, Route::WorkspaceDerive))
        }
        _ => {
            let parts: Vec<_> = path.split('/').collect();
            if parts.len() == 4 && parts[..2] == ["", "v1"] && parts[2] == "artifacts" {
                let artifact = eggwork_core::ArtifactId::new(parts[3].to_owned()).ok()?;
                return (method == "GET")
                    .then_some((Operation::ArtifactRead, Route::ArtifactDownload(artifact)));
            }
            if parts.len() == 4 && parts[..3] == ["", "v1", "executions"] {
                let id = ExecutionId::new(parts[3]).ok()?;
                return match method {
                    "GET" => Some((Operation::Observe, Route::Observe(id))),
                    _ => None,
                };
            }
            if parts.len() == 5 && parts[..3] == ["", "v1", "executions"] {
                let id = ExecutionId::new(parts[3]).ok()?;
                return match (method, parts[4]) {
                    ("POST", "cancel") => Some((Operation::Cancel, Route::Cancel(id))),
                    ("POST", "renew") => Some((Operation::Renew, Route::Renew(id))),
                    ("GET", "events") => Some((Operation::Events, Route::Events(id))),
                    ("GET", "artifacts") => {
                        Some((Operation::ArtifactRead, Route::ArtifactList(id)))
                    }
                    _ => None,
                };
            }
            if parts.len() == 4 && parts[..2] == ["", "v1"] && parts[2] == "blobs" {
                if !matches!(method, "PUT" | "GET") {
                    return None;
                }
                let Ok(digest) = eggwork_core::BlobDigest::parse(parts[3].to_owned()) else {
                    return Some((
                        if method == "PUT" {
                            Operation::BlobWrite
                        } else {
                            Operation::BlobRead
                        },
                        Route::BlobInvalidDigest,
                    ));
                };
                return match method {
                    "PUT" => Some((Operation::BlobWrite, Route::BlobUpload(digest))),
                    "GET" => Some((Operation::BlobRead, Route::BlobDownload(digest))),
                    _ => None,
                };
            }
            None
        }
    }
}

enum Route {
    Capabilities,
    Status,
    Execute,
    Observe(ExecutionId),
    Cancel(ExecutionId),
    Renew(ExecutionId),
    Events(ExecutionId),
    BlobMissing,
    BlobPrepare,
    BlobUpload(eggwork_core::BlobDigest),
    BlobDownload(eggwork_core::BlobDigest),
    BlobInvalidDigest,
    WorkspaceCreate,
    WorkspaceDerive,
    ArtifactList(ExecutionId),
    ArtifactDownload(eggwork_core::ArtifactId),
}

async fn dispatch(
    state: NodeState,
    request: Request,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let method = request.head().method().as_str().to_owned();
    let path = request.head().target().path().to_owned();
    let query = request.head().target().query().map(str::to_owned);
    let Some((operation, route)) = operation_for(&method, &path) else {
        increment_metric(&state.store, "rejected_route", 1).await;
        return Ok(error_response(404, "not_found", "operation not found"));
    };
    let Some(principal) = authenticated_principal(&state, &request) else {
        increment_metric(&state.store, "rejected_unauthenticated", 1).await;
        return Ok(error_response(
            401,
            "unauthenticated",
            "verified client identity required",
        ));
    };
    let authorization = OperationRequest {
        operation,
        resource: resource_for_path(operation, &path),
    };
    if !state
        .authorizer
        .authorize_request(&principal, &authorization)
    {
        increment_metric(&state.store, "rejected_unauthorized", 1).await;
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    match route {
        Route::Capabilities => Ok(json_response(200, &node_capabilities_for(&state))),
        Route::Status => Ok(json_response(200, &node_status(&state))),
        Route::Execute => execute(state, request, principal).await,
        Route::Observe(id) => observe(state, id, query.as_deref(), principal).await,
        Route::Cancel(id) => control(state, id, request, principal, false).await,
        Route::Renew(id) => control(state, id, request, principal, true).await,
        Route::Events(id) => events_route(state, id, query.as_deref(), principal).await,
        Route::BlobMissing => blob_missing(state, request, principal).await,
        Route::BlobPrepare => blob_prepare(state, request, principal).await,
        Route::BlobUpload(digest) => blob_upload(state, request, digest).await,
        Route::BlobDownload(digest) => blob_download(state, digest).await,
        Route::BlobInvalidDigest => Ok(error_response(
            400,
            "invalid_digest",
            "blob digest is invalid",
        )),
        Route::WorkspaceCreate => workspace_create(state, request, principal).await,
        Route::WorkspaceDerive => workspace_derive(state, request, principal).await,
        Route::ArtifactList(id) => artifact_list(state, id, query.as_deref(), principal).await,
        Route::ArtifactDownload(id) => artifact_download(state, id, principal).await,
    }
}

async fn increment_metric(store: &store::ExecutionStore, name: &'static str, amount: u64) {
    let _ = store.increment_metric(name, amount).await;
}

fn resource_for_path(operation: Operation, path: &str) -> Option<ResourceId> {
    let parts: Vec<_> = path.split('/').collect();
    match operation {
        Operation::Observe | Operation::Cancel | Operation::Renew | Operation::Events => {
            ExecutionId::new(parts.get(3)?.to_string())
                .ok()
                .map(ResourceId::Execution)
        }
        Operation::BlobRead | Operation::BlobWrite => {
            eggwork_core::BlobDigest::parse(parts.get(3)?.to_string())
                .ok()
                .map(ResourceId::Blob)
        }
        Operation::ArtifactRead if parts.get(2) == Some(&"artifacts") => {
            eggwork_core::ArtifactId::new(parts.get(3)?.to_string())
                .ok()
                .map(ResourceId::Artifact)
        }
        Operation::ArtifactRead => ExecutionId::new(parts.get(3)?.to_string())
            .ok()
            .map(ResourceId::Execution),
        _ => None,
    }
}

fn authorize_resource(
    state: &NodeState,
    principal: &NodePrincipal,
    operation: Operation,
    resource: ResourceId,
) -> bool {
    state.authorizer.authorize_request(
        principal,
        &OperationRequest {
            operation,
            resource: Some(resource),
        },
    )
}

fn authenticated_principal(state: &NodeState, request: &Request) -> Option<NodePrincipal> {
    let tls = request.context().connection().tls.as_ref()?;
    if !tls.client_authenticated || !tls.peer_certificates_present {
        return None;
    }
    let chain = tls.peer_certificate_chain.as_ref()?;
    if chain.is_empty() || chain.len() > 8 || chain.iter().any(|cert| cert.len() > 64 * 1024) {
        return None;
    }
    state.resolver.resolve_verified_leaf(&chain[0])
}

fn node_status(state: &NodeState) -> NodeStatus {
    NodeStatus {
        node_id: state.node_id.clone(),
        draining: state.draining.load(Ordering::Acquire)
            || operations::is_persistently_draining(&state.drain_path),
        active_executions: state.max_active - state.permits.available_permits() as u32,
        capabilities: node_capabilities_for(state),
    }
}

async fn read_limited_body(
    mut body: eggserve_core::primitives::RequestBody,
    limit: usize,
) -> Result<Vec<u8>, ()> {
    let mut bytes =
        Vec::with_capacity(body.declared_length().unwrap_or(0).min(limit as u64) as usize);
    while let Some(chunk) = body.next_chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindMissingRequest {
    digests: Vec<eggwork_core::BlobDigest>,
}

#[derive(Serialize)]
struct FindMissingResponse {
    missing: Vec<eggwork_core::BlobDigest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareBlobRequest {
    digest: eggwork_core::BlobDigest,
    size_bytes: u64,
}

#[derive(Serialize)]
struct PrepareBlobResponse {
    upload_required: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCreateRequest {
    schema_version: u16,
    workspace_id: eggwork_core::WorkspaceId,
    handle: ExecutionHandle,
    manifest: eggwork_core::WorkspaceManifest,
}

#[derive(Serialize)]
struct WorkspaceReadyResponse {
    schema_version: u16,
    workspace_id: eggwork_core::WorkspaceId,
    execution_id: ExecutionId,
    generation: ExecutionGeneration,
    manifest_digest: String,
    logical_bytes: u64,
}

async fn blob_missing(
    state: NodeState,
    request: Request,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let bytes = match read_limited_body(body, MAX_BLOB_FIND_REQUEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "request body exceeds limit",
            ));
        }
    };
    let query: FindMissingRequest = match serde_json::from_slice(&bytes) {
        Ok(query) => query,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "request body is invalid",
            ));
        }
    };
    for digest in &query.digests {
        if !authorize_resource(
            &state,
            &principal,
            Operation::BlobRead,
            ResourceId::Blob(digest.clone()),
        ) {
            return Ok(error_response(
                403,
                "forbidden",
                "operation is not authorized",
            ));
        }
    }
    match state.blobs.find_missing(&query.digests) {
        Ok(missing) => Ok(json_response(200, &FindMissingResponse { missing })),
        Err(blob::BlobError::TooManyDigests) => Ok(error_response(
            413,
            "too_many_digests",
            "digest batch exceeds limit",
        )),
        Err(_) => Ok(error_response(
            500,
            "storage_error",
            "blob metadata is unavailable",
        )),
    }
}

async fn blob_prepare(
    state: NodeState,
    request: Request,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let bytes = match read_limited_body(body, MAX_BLOB_FIND_REQUEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "request body exceeds limit",
            ));
        }
    };
    let prepare: PrepareBlobRequest = match serde_json::from_slice(&bytes) {
        Ok(prepare) => prepare,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "request body is invalid",
            ));
        }
    };
    if !authorize_resource(
        &state,
        &principal,
        Operation::BlobWrite,
        ResourceId::Blob(prepare.digest.clone()),
    ) {
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    match state
        .blobs
        .prepare_upload(&prepare.digest, prepare.size_bytes)
        .await
    {
        Ok(upload_required) => Ok(json_response(200, &PrepareBlobResponse { upload_required })),
        Err(error) => Ok(blob_error_response(error)),
    }
}

async fn workspace_create(
    state: NodeState,
    request: Request,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let bytes = match read_limited_body(body, MAX_WORKSPACE_REQUEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "workspace manifest exceeds limit",
            ));
        }
    };
    let wire: WorkspaceCreateRequest = match serde_json::from_slice(&bytes) {
        Ok(wire) => wire,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "workspace request is invalid",
            ));
        }
    };
    if wire.schema_version != API_SCHEMA_VERSION {
        return Ok(error_response(
            426,
            "protocol_version",
            "unsupported schema version",
        ));
    }
    if !authorize_resource(
        &state,
        &principal,
        Operation::WorkspaceCreate,
        ResourceId::Workspace(wire.workspace_id.clone()),
    ) {
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    if ExecutionGeneration::new(wire.handle.generation.get()).is_err() {
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            400,
            "invalid_request",
            "workspace owner is invalid",
        ));
    }
    match state
        .workspaces
        .materialize(
            wire.workspace_id.clone(),
            &wire.handle,
            &principal.id,
            wire.manifest,
            &state.blobs,
        )
        .await
    {
        Ok(ready) => {
            increment_metric(&state.store, "workspace_manifest_registrations", 1).await;
            Ok(json_response(
                201,
                &WorkspaceReadyResponse {
                    schema_version: API_SCHEMA_VERSION,
                    workspace_id: wire.workspace_id,
                    execution_id: wire.handle.execution_id,
                    generation: wire.handle.generation,
                    manifest_digest: ready.manifest_digest,
                    logical_bytes: ready.logical_bytes,
                },
            ))
        }
        Err(workspace::WorkspaceError::InvalidManifest) => Ok(error_response(
            400,
            "invalid_manifest",
            "workspace manifest is invalid",
        )),
        Err(workspace::WorkspaceError::InvalidBlob) => Ok(error_response(
            409,
            "missing_blob",
            "workspace references a missing or corrupt blob",
        )),
        Err(workspace::WorkspaceError::BlobSizeMismatch) => Ok(error_response(
            409,
            "blob_size_mismatch",
            "workspace blob size does not match",
        )),
        Err(workspace::WorkspaceError::QuotaExceeded) => Ok(error_response(
            507,
            "quota_exceeded",
            "workspace quota exceeded",
        )),
        Err(workspace::WorkspaceError::Conflict) => Ok(error_response(
            409,
            "workspace_identity_conflict",
            "workspace identity conflicts",
        )),
        Err(_) => Ok(error_response(
            500,
            "workspace_error",
            "workspace materialization failed",
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeriveWorkspaceRequest {
    schema_version: u16,
    workspace_id: eggwork_core::WorkspaceId,
    handle: ExecutionHandle,
    patch: eggwork_core::WorkspaceManifestPatch,
}

async fn workspace_derive(
    state: NodeState,
    request: Request,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    increment_metric(&state.store, "workspace_derived_requests", 1).await;
    let (_head, body, _context) = request.into_parts_with_context();
    let bytes = match read_limited_body(body, MAX_WORKSPACE_REQUEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "workspace patch exceeds limit",
            ));
        }
    };
    let wire: DeriveWorkspaceRequest = match serde_json::from_slice(&bytes) {
        Ok(wire) => wire,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "workspace derive request is invalid",
            ));
        }
    };
    if wire.schema_version != API_SCHEMA_VERSION {
        return Ok(error_response(
            426,
            "protocol_version",
            "unsupported schema version",
        ));
    }
    if !authorize_resource(
        &state,
        &principal,
        Operation::WorkspaceCreate,
        ResourceId::Workspace(wire.workspace_id.clone()),
    ) {
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    if ExecutionGeneration::new(wire.handle.generation.get()).is_err() {
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            400,
            "invalid_request",
            "workspace owner is invalid",
        ));
    }
    let patch_entries = wire.patch.entries.len() as u64;
    match state
        .workspaces
        .materialize_derived(
            wire.workspace_id.clone(),
            &wire.handle,
            &principal.id,
            wire.patch,
            &state.blobs,
        )
        .await
    {
        Ok(ready) => {
            increment_metric(&state.store, "workspace_derived_hits", 1).await;
            increment_metric(&state.store, "workspace_manifest_registrations", 1).await;
            increment_metric(&state.store, "workspace_patch_entries", patch_entries).await;
            Ok(json_response(
                201,
                &WorkspaceReadyResponse {
                    schema_version: API_SCHEMA_VERSION,
                    workspace_id: wire.workspace_id,
                    execution_id: wire.handle.execution_id,
                    generation: wire.handle.generation,
                    manifest_digest: ready.manifest_digest,
                    logical_bytes: ready.logical_bytes,
                },
            ))
        }
        Err(workspace::WorkspaceError::BaseMissing) => {
            increment_metric(&state.store, "workspace_base_missing", 1).await;
            Ok(error_response(
                409,
                "base_manifest_missing",
                "base manifest is missing or expired; retry with a full manifest",
            ))
        }
        Err(workspace::WorkspaceError::InvalidPatch) => Ok(error_response(
            400,
            "invalid_patch",
            "workspace patch is invalid",
        )),
        Err(workspace::WorkspaceError::InvalidManifest) => Ok(error_response(
            400,
            "invalid_manifest",
            "workspace manifest is invalid",
        )),
        Err(workspace::WorkspaceError::InvalidBlob) => Ok(error_response(
            409,
            "missing_blob",
            "workspace references a missing or corrupt blob",
        )),
        Err(workspace::WorkspaceError::BlobSizeMismatch) => Ok(error_response(
            409,
            "blob_size_mismatch",
            "workspace blob size does not match",
        )),
        Err(workspace::WorkspaceError::QuotaExceeded) => Ok(error_response(
            507,
            "quota_exceeded",
            "workspace quota exceeded",
        )),
        Err(workspace::WorkspaceError::Conflict) => Ok(error_response(
            409,
            "workspace_identity_conflict",
            "workspace identity conflicts",
        )),
        Err(_) => Ok(error_response(
            500,
            "workspace_error",
            "workspace materialization failed",
        )),
    }
}

async fn blob_upload(
    state: NodeState,
    request: Request,
    digest: eggwork_core::BlobDigest,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let query = request.head().target().query();
    let declared = query
        .and_then(|query| query.strip_prefix("size="))
        .and_then(|size| size.parse::<u64>().ok());
    let Some(declared) = declared else {
        return Ok(error_response(
            400,
            "invalid_request",
            "blob size is required",
        ));
    };
    if declared > blob::MAX_BLOB_BYTES {
        return Ok(error_response(
            413,
            "blob_too_large",
            "blob exceeds size limit",
        ));
    }
    let (_head, body, _context) = request.into_parts_with_context();
    if body
        .declared_length()
        .is_some_and(|length| length != declared)
    {
        return Ok(error_response(
            400,
            "length_mismatch",
            "declared blob length does not match request",
        ));
    }
    let input = stream::unfold(body, |mut body| async move {
        match body.next_chunk().await {
            Ok(Some(chunk)) => Some((Ok::<_, blob::BlobError>(chunk), body)),
            Ok(None) => None,
            Err(_) => Some((Err(blob::BlobError::Body), body)),
        }
    });
    match state.blobs.put_stream(digest, declared, input).await {
        Ok(()) => {
            increment_metric(&state.store, "blob_upload_bytes", declared).await;
            Ok(Response::builder()
                .status(StatusCode::new(201).expect("valid HTTP status"))
                .body(ResponseBody::Empty)
                .unwrap_or_else(|_| error_response(500, "internal", "internal error")))
        }
        Err(error) => Ok(blob_error_response(error)),
    }
}

async fn blob_download(
    state: NodeState,
    digest: eggwork_core::BlobDigest,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let file = match state.blobs.open_verified(&digest).await {
        Ok(file) => file,
        Err(error) => return Ok(blob_error_response(error)),
    };
    let metric_store = state.store.clone();
    let stream = stream::try_unfold(
        (file, metric_store),
        |(mut file, metric_store)| async move {
            let mut chunk = vec![0u8; 64 * 1024];
            match tokio::io::AsyncReadExt::read(&mut file, &mut chunk).await {
                Ok(0) => Ok(None),
                Ok(size) => {
                    chunk.truncate(size);
                    let _ = metric_store
                        .increment_metric("blob_download_bytes", size as u64)
                        .await;
                    Ok(Some((Bytes::from(chunk), (file, metric_store))))
                }
                Err(_) => Err(ResponseStreamError::new("blob read failed")),
            }
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .and_then(|builder| builder.header("eggwork-blob-digest", digest.as_str()))
        .and_then(|builder| builder.body(ResponseBody::Stream(ResponseStream::new(stream))))
        .map_err(|e| eggserve_core::server::ServiceError::internal(e.to_string()))
}

async fn artifact_list(
    state: NodeState,
    execution_id: ExecutionId,
    query: Option<&str>,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let generation = match query_parameter(query, "generation") {
        Ok(Some(value)) => value
            .parse::<u64>()
            .ok()
            .and_then(|value| ExecutionGeneration::new(value).ok()),
        _ => None,
    };
    let Some(generation) = generation else {
        return Ok(error_response(
            400,
            "invalid_request",
            "generation is invalid",
        ));
    };
    match state
        .artifacts
        .list(&execution_id, generation, &principal.id)
    {
        Ok(records) => Ok(json_response(200, &records)),
        Err(_) => Ok(error_response(
            500,
            "storage_error",
            "artifact metadata unavailable",
        )),
    }
}

struct BlobReferenceLease {
    blobs: blob::BlobStore,
    owner_id: String,
}

impl BlobReferenceLease {
    fn renew(&self) -> Result<(), blob::BlobError> {
        self.blobs.set_reference_expiry(
            "reader",
            &self.owner_id,
            artifact::now_unix_ms().saturating_add(30 * 60 * 1000),
        )
    }
}

impl Drop for BlobReferenceLease {
    fn drop(&mut self) {
        let _ = self.blobs.release_references("reader", &self.owner_id);
    }
}

async fn artifact_download(
    state: NodeState,
    artifact_id: eggwork_core::ArtifactId,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let record = match state.artifacts.get(&artifact_id, &principal.id) {
        Ok(record) => record,
        Err(_) => {
            return Ok(error_response(
                404,
                "artifact_not_found",
                "artifact not found",
            ));
        }
    };
    let owner_id = uuid::Uuid::new_v4().to_string();
    if state
        .blobs
        .retain(
            "reader",
            &owner_id,
            std::slice::from_ref(&record.digest),
            Some(artifact::now_unix_ms().saturating_add(30 * 60 * 1000)),
        )
        .is_err()
    {
        return Ok(error_response(
            404,
            "artifact_not_found",
            "artifact not found",
        ));
    }
    let lease = BlobReferenceLease {
        blobs: state.blobs.clone(),
        owner_id,
    };
    let file = match state.blobs.open_verified(&record.digest).await {
        Ok(file) => file,
        Err(_) => {
            return Ok(error_response(
                500,
                "artifact_unavailable",
                "artifact bytes unavailable",
            ));
        }
    };
    let stream = stream::try_unfold((file, lease), |(mut file, lease)| async move {
        let mut chunk = vec![0u8; 64 * 1024];
        match tokio::io::AsyncReadExt::read(&mut file, &mut chunk).await {
            Ok(0) => Ok(None),
            Ok(size) => {
                lease
                    .renew()
                    .map_err(|_| ResponseStreamError::new("artifact lease refresh failed"))?;
                chunk.truncate(size);
                Ok(Some((Bytes::from(chunk), (file, lease))))
            }
            Err(_) => Err(ResponseStreamError::new("artifact read failed")),
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .and_then(|builder| builder.header("eggwork-artifact-id", artifact_id.as_str()))
        .and_then(|builder| builder.header("eggwork-artifact-digest", record.digest.as_str()))
        .and_then(|builder| builder.body(ResponseBody::Stream(ResponseStream::new(stream))))
        .map_err(|e| eggserve_core::server::ServiceError::internal(e.to_string()))
}

fn blob_error_response(error: blob::BlobError) -> Response {
    match error {
        blob::BlobError::TooLarge => {
            error_response(413, "blob_too_large", "blob exceeds size limit")
        }
        blob::BlobError::QuotaExceeded => {
            error_response(507, "quota_exceeded", "blob storage quota exceeded")
        }
        blob::BlobError::LengthMismatch => {
            error_response(422, "length_mismatch", "blob length does not match")
        }
        blob::BlobError::DigestMismatch => {
            error_response(422, "digest_mismatch", "blob digest does not match")
        }
        blob::BlobError::NotFound => error_response(404, "blob_not_found", "blob not found"),
        blob::BlobError::CorruptExisting => {
            error_response(500, "corrupt_blob", "stored blob was quarantined")
        }
        _ => error_response(500, "storage_error", "blob storage operation failed"),
    }
}

async fn execute(
    state: NodeState,
    request: Request,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let body = match read_limited_body(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "request body exceeds limit",
            ));
        }
    };
    let wire: ExecuteRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "request body is invalid",
            ));
        }
    };
    if wire.schema_version != API_SCHEMA_VERSION {
        return Ok(error_response(
            426,
            "protocol_version",
            "unsupported schema version",
        ));
    }
    if !authorize_resource(
        &state,
        &principal,
        Operation::Execute,
        ResourceId::Execution(wire.handle.execution_id.clone()),
    ) || wire.workspace_id.as_ref().is_some_and(|workspace_id| {
        !authorize_resource(
            &state,
            &principal,
            Operation::Execute,
            ResourceId::Workspace(workspace_id.clone()),
        )
    }) {
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    if ExecutionGeneration::new(wire.handle.generation.get()).is_err() {
        return Ok(error_response(
            400,
            "invalid_request",
            "execution generation is invalid",
        ));
    }
    if wire.spec.schema_version != API_SCHEMA_VERSION {
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            426,
            "protocol_version",
            "unsupported execution schema version",
        ));
    }
    if let Err(error) = wire.spec.validate() {
        let _ = error;
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            400,
            "invalid_request",
            "execution specification is invalid",
        ));
    }
    if !wire.spec.command.declared_outputs.is_empty() && wire.workspace_id.is_none() {
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            400,
            "workspace_required",
            "declared outputs require an execution workspace",
        ));
    }
    if state.draining.load(Ordering::Acquire)
        || operations::is_persistently_draining(&state.drain_path)
    {
        increment_metric(&state.store, "rejected_draining", 1).await;
        return Ok(error_response(
            503,
            "draining",
            "node is not accepting executions",
        ));
    }
    if !check_isolation_supported(&state, &wire.spec.command.isolation) {
        increment_metric(&state.store, "rejected_capability", 1).await;
        return Ok(error_response(
            409,
            "capability_mismatch",
            "requested filesystem isolation capability is unavailable",
        ));
    }
    if !check_resources_supported(&state, &wire.spec.command.resources) {
        increment_metric(&state.store, "rejected_capability", 1).await;
        return Ok(error_response(
            409,
            "capability_mismatch",
            "requested resource control capability is unavailable",
        ));
    }
    if !check_network_supported(&wire.spec.command.network) {
        increment_metric(&state.store, "rejected_capability", 1).await;
        return Ok(error_response(
            409,
            "capability_mismatch",
            "requested network capability is unavailable",
        ));
    }
    let id = wire.handle.execution_id.clone();
    let generation = wire.handle.generation;
    let (canonical_version, digest) =
        match eggwork_core::request_digest_with_workspace(&wire.spec, wire.workspace_id.as_ref()) {
            Ok((version, digest)) => (version, digest.as_str().to_owned()),
            Err(_) => {
                increment_metric(&state.store, "rejected_invalid", 1).await;
                return Ok(error_response(
                    400,
                    "invalid_request",
                    "execution specification is invalid",
                ));
            }
        };
    let execution_root = if let Some(workspace_id) = &wire.workspace_id {
        match state
            .workspaces
            .resolve(workspace_id, &id, generation, &principal.id)
        {
            Ok(workspace) => workspace.root,
            Err(_) => {
                increment_metric(&state.store, "rejected_invalid", 1).await;
                return Ok(error_response(
                    409,
                    "workspace_not_ready",
                    "workspace is unavailable for this execution",
                ));
            }
        }
    } else {
        state.execution_root.clone()
    };
    let workspace_capture = wire.workspace_id.clone().map(|id| WorkspaceCapture {
        id,
        root: execution_root.clone(),
        principal: principal.id.clone(),
        outputs: wire.spec.command.declared_outputs.clone(),
    });
    let runner_request = match RunnerRequest::from_spec(
        &wire.spec,
        execution_root,
        ExecutionProvenance {
            execution_id: Some(id.clone()),
            generation: Some(generation),
        },
    ) {
        Ok(request) => request,
        Err(_) => {
            increment_metric(&state.store, "rejected_invalid", 1).await;
            return Ok(error_response(
                400,
                "invalid_request",
                "execution specification is invalid",
            ));
        }
    };
    let lease_hash = store::lease_hash(wire.handle.lease_id.as_str());
    let principal_id = principal.id.as_str().to_owned();
    let lease_expires_unix_ms = store::unix_millis()
        .saturating_add(state.lease_ttl.as_millis().min(i64::MAX as u128) as i64);
    let mut executions = state.executions.lock().await;
    if let Some(existing) = state
        .store
        .lookup(id.clone(), generation)
        .await
        .map_err(|_| eggserve_core::server::ServiceError::internal("execution store unavailable"))?
    {
        if existing.canonical_version != canonical_version {
            return Ok(error_response(
                409,
                "canonicalization_version_mismatch",
                "execution identity uses an unsupported request canonicalization version",
            ));
        }
        if existing.principal_id != principal_id {
            return Ok(error_response(
                403,
                "forbidden",
                "execution belongs to another principal",
            ));
        }
        if existing.digest != digest || existing.lease_token_hash != lease_hash {
            return Ok(error_response(
                409,
                "execution_identity_conflict",
                "execution identity conflicts with an existing request",
            ));
        }
        let record = match executions.get(&id) {
            Some(record) if record.snapshot.read().await.generation == generation => record.clone(),
            _ => recovered_record(
                existing.snapshot.clone(),
                state.store.clone(),
                existing.lease_token_hash,
            ),
        };
        drop(executions);
        return event_response(state, id, generation, 0, record).await;
    }
    if state.draining.load(Ordering::Acquire)
        || operations::is_persistently_draining(&state.drain_path)
    {
        increment_metric(&state.store, "rejected_draining", 1).await;
        return Ok(error_response(
            503,
            "draining",
            "node is not accepting executions",
        ));
    }
    let permit = match state.permits.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            increment_metric(&state.store, "rejected_busy", 1).await;
            return Ok(error_response(503, "busy", "node execution limit reached"));
        }
    };
    if executions.len() >= MAX_RECENT_EXECUTIONS {
        let mut terminal = Vec::new();
        for (old_id, old_record) in executions.iter() {
            if is_terminal(&old_record.snapshot.read().await.state) {
                terminal.push(old_id.clone());
            }
            if executions.len() - terminal.len() < MAX_RECENT_EXECUTIONS {
                break;
            }
        }
        for old_id in terminal {
            executions.remove(&old_id);
        }
    }
    if executions.len() >= MAX_RECENT_EXECUTIONS {
        increment_metric(&state.store, "rejected_storage", 1).await;
        return Ok(error_response(
            503,
            "storage_exhausted",
            "recent execution capacity reached",
        ));
    }
    if let Some(workspace_id) = &wire.workspace_id
        && state
            .workspaces
            .mark_active(workspace_id, &state.blobs)
            .is_err()
    {
        drop(permit);
        drop(executions);
        increment_metric(&state.store, "rejected_invalid", 1).await;
        return Ok(error_response(
            409,
            "workspace_not_ready",
            "workspace is unavailable for this execution",
        ));
    }
    let snapshot = ExecutionSnapshot {
        schema_version: API_SCHEMA_VERSION,
        execution_id: id.clone(),
        generation,
        state: ExecutionState::Accepted,
        result: None,
    };
    let reservation = match state
        .store
        .reserve(
            snapshot.clone(),
            canonical_version,
            digest,
            principal_id,
            lease_hash.clone(),
            lease_expires_unix_ms,
        )
        .await
    {
        Ok(reservation) => reservation,
        Err(_) => {
            if let Some(workspace_id) = &wire.workspace_id {
                let expiry =
                    artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
                let _ = state
                    .workspaces
                    .mark_terminal(workspace_id, expiry, &state.blobs);
            }
            return Err(eggserve_core::server::ServiceError::internal(
                "execution store unavailable",
            ));
        }
    };
    match reservation {
        store::ReserveResult::Created => {
            increment_metric(&state.store, "executions_accepted", 1).await;
        }
        store::ReserveResult::Existing(snapshot) => {
            drop(permit);
            if is_terminal(&snapshot.state)
                && let Some(workspace_id) = &wire.workspace_id
            {
                let expiry =
                    artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
                let _ = state
                    .workspaces
                    .mark_terminal(workspace_id, expiry, &state.blobs);
            }
            let record = recovered_record(*snapshot, state.store.clone(), lease_hash);
            drop(executions);
            return event_response(state, id, generation, 0, record).await;
        }
        store::ReserveResult::Conflict => {
            if let Some(workspace_id) = &wire.workspace_id {
                let expiry =
                    artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
                let _ = state
                    .workspaces
                    .mark_terminal(workspace_id, expiry, &state.blobs);
            }
            return Ok(error_response(
                409,
                "execution_identity_conflict",
                "execution identity conflicts with an existing request",
            ));
        }
        store::ReserveResult::StaleGeneration => {
            if let Some(workspace_id) = &wire.workspace_id {
                let expiry =
                    artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
                let _ = state
                    .workspaces
                    .mark_terminal(workspace_id, expiry, &state.blobs);
            }
            return Ok(error_response(
                409,
                "generation_mismatch",
                "execution generation is stale or out of sequence",
            ));
        }
        store::ReserveResult::StorageFull => {
            increment_metric(&state.store, "rejected_storage", 1).await;
            if let Some(workspace_id) = &wire.workspace_id {
                let expiry =
                    artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
                let _ = state
                    .workspaces
                    .mark_terminal(workspace_id, expiry, &state.blobs);
            }
            return Ok(error_response(
                503,
                "storage_exhausted",
                "execution identity capacity reached",
            ));
        }
    }
    let record = new_record(
        snapshot,
        state.store.clone(),
        state.draining.clone(),
        lease_hash,
        state.lease_ttl,
    );
    executions.insert(id.clone(), record.clone());
    drop(executions);
    publish(
        &record,
        ExecutionState::Accepted,
        None,
        ExecutionEventKind::State(ExecutionState::Accepted),
    )
    .await;
    let lease_record = record.clone();
    tokio::spawn(async move { monitor_lease(lease_record).await });
    let service_state = state.clone();
    let execution_record = record.clone();
    let workspace_capture = workspace_capture.clone();
    tokio::spawn(async move {
        run_execution(
            service_state,
            execution_record,
            runner_request,
            workspace_capture,
            permit,
        )
        .await;
    });
    event_response(state, id, generation, 0, record).await
}

fn new_record(
    snapshot: ExecutionSnapshot,
    store: store::ExecutionStore,
    draining: Arc<AtomicBool>,
    lease_hash: String,
    lease_ttl: Duration,
) -> Arc<ExecutionRecord> {
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    Arc::new(ExecutionRecord {
        snapshot: RwLock::new(snapshot),
        cancellation: CancellationToken::new(),
        events,
        store,
        draining,
        finished: AtomicBool::new(false),
        lease_hash,
        lease: tokio::sync::Mutex::new(LeaseState {
            expires_at: tokio::time::Instant::now() + lease_ttl,
            notify: Arc::new(tokio::sync::Notify::new()),
            expired: Arc::new(AtomicBool::new(false)),
        }),
    })
}

fn recovered_record(
    snapshot: ExecutionSnapshot,
    store: store::ExecutionStore,
    lease_hash: String,
) -> Arc<ExecutionRecord> {
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    Arc::new(ExecutionRecord {
        snapshot: RwLock::new(snapshot),
        cancellation: CancellationToken::new(),
        events,
        store,
        draining: Arc::new(AtomicBool::new(false)),
        finished: AtomicBool::new(true),
        lease_hash,
        lease: tokio::sync::Mutex::new(LeaseState {
            expires_at: tokio::time::Instant::now(),
            notify: Arc::new(tokio::sync::Notify::new()),
            expired: Arc::new(AtomicBool::new(false)),
        }),
    })
}

async fn monitor_lease(record: Arc<ExecutionRecord>) {
    loop {
        if is_terminal(&record.snapshot.read().await.state) {
            return;
        }
        let (deadline, notify) = {
            let lease = record.lease.lock().await;
            (lease.expires_at, lease.notify.clone())
        };
        tokio::select! {
            _ = record.cancellation.cancelled() => return,
            _ = notify.notified() => continue,
            _ = tokio::time::sleep_until(deadline) => {
                let lease = record.lease.lock().await;
                if tokio::time::Instant::now() >= lease.expires_at {
                    lease.expired.store(true, Ordering::Release);
                    record.cancellation.cancel();
                    return;
                }
            }
        }
    }
}

async fn observe(
    state: NodeState,
    id: ExecutionId,
    query: Option<&str>,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let generation = match query_parameter(query, "generation") {
        Ok(Some(value)) => match value
            .parse::<u64>()
            .ok()
            .and_then(|v| ExecutionGeneration::new(v).ok())
        {
            Some(generation) => Some(generation),
            None => {
                return Ok(error_response(
                    400,
                    "invalid_request",
                    "generation is invalid",
                ));
            }
        },
        Ok(None) => None,
        Err(()) => return Ok(error_response(400, "invalid_request", "query is invalid")),
    };
    match state
        .store
        .load_snapshot_for_principal(id, generation, principal.id.as_str().to_owned())
        .await
        .map_err(|_| eggserve_core::server::ServiceError::internal("execution store unavailable"))?
    {
        Some(snapshot) => Ok(json_response(200, &snapshot)),
        None => Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        )),
    }
}

async fn control(
    state: NodeState,
    id: ExecutionId,
    request: Request,
    principal: NodePrincipal,
    renew: bool,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let bytes = match read_limited_body(body, MAX_REQUEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(()) => {
            return Ok(error_response(
                413,
                "request_too_large",
                "request body exceeds limit",
            ));
        }
    };
    let request: ControlRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "request body is invalid",
            ));
        }
    };
    if request.schema_version != API_SCHEMA_VERSION || request.handle.execution_id != id {
        return Ok(error_response(
            400,
            "invalid_request",
            "control request is invalid",
        ));
    }
    let Some(existing) = state
        .store
        .lookup(id.clone(), request.handle.generation)
        .await
        .map_err(|_| {
            eggserve_core::server::ServiceError::internal("execution store unavailable")
        })?
    else {
        return Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        ));
    };
    if existing.principal_id != principal.id.as_str() {
        return Ok(error_response(
            403,
            "forbidden",
            "execution belongs to another principal",
        ));
    }
    if existing.lease_token_hash != store::lease_hash(request.handle.lease_id.as_str()) {
        return Ok(error_response(
            403,
            "invalid_lease",
            "execution lease is invalid",
        ));
    }
    let Some(latest) = state
        .store
        .load_snapshot(id.clone(), None)
        .await
        .map_err(|_| {
            eggserve_core::server::ServiceError::internal("execution store unavailable")
        })?
    else {
        return Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        ));
    };
    if latest.generation != request.handle.generation {
        return Ok(error_response(
            409,
            "generation_mismatch",
            "execution generation is stale",
        ));
    }
    let Some(record) = state.executions.lock().await.get(&id).cloned() else {
        return Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        ));
    };
    if record.lease_hash != existing.lease_token_hash {
        return Ok(error_response(
            403,
            "invalid_lease",
            "execution lease is invalid",
        ));
    }
    let snapshot = record.snapshot.read().await.clone();
    if renew {
        let Some(renewal_id) = request.renewal_id.as_deref() else {
            return Ok(error_response(
                400,
                "invalid_request",
                "renewal identifier is required",
            ));
        };
        if renewal_id.is_empty() || renewal_id.len() > eggwork_core::MAX_ID_BYTES {
            return Ok(error_response(
                400,
                "invalid_request",
                "renewal identifier is invalid",
            ));
        }
        if is_terminal(&snapshot.state) {
            return Ok(error_response(
                409,
                "execution_terminal",
                "terminal execution cannot be renewed",
            ));
        }
        let mut lease = record.lease.lock().await;
        if lease.expired.load(Ordering::Acquire) || tokio::time::Instant::now() >= lease.expires_at
        {
            return Ok(error_response(
                409,
                "lease_expired",
                "execution lease has expired",
            ));
        }
        let requested_expires_unix_ms = store::unix_millis()
            .saturating_add(state.lease_ttl.as_millis().min(i64::MAX as u128) as i64);
        let expires_unix_ms = state
            .store
            .update_lease(
                id,
                request.handle.generation,
                existing.lease_token_hash,
                renewal_id.to_owned(),
                requested_expires_unix_ms,
            )
            .await
            .map_err(|_| {
                eggserve_core::server::ServiceError::internal("execution store unavailable")
            })?;
        let Some(expires_unix_ms) = expires_unix_ms else {
            return Ok(error_response(
                409,
                "lease_expired",
                "execution lease has expired",
            ));
        };
        let remaining_ms = expires_unix_ms.saturating_sub(store::unix_millis()).max(0) as u64;
        lease.expires_at = tokio::time::Instant::now() + Duration::from_millis(remaining_ms);
        lease.notify.notify_waiters();
        return Ok(json_response(200, &snapshot));
    }
    if !is_terminal(&snapshot.state) && snapshot.state != ExecutionState::Cancelling {
        let lease = record.lease.lock().await;
        if lease.expired.load(Ordering::Acquire) || tokio::time::Instant::now() >= lease.expires_at
        {
            return Ok(error_response(
                409,
                "lease_expired",
                "execution lease has expired",
            ));
        }
        drop(lease);
        publish(
            &record,
            ExecutionState::Cancelling,
            None,
            ExecutionEventKind::State(ExecutionState::Cancelling),
        )
        .await;
        if !is_terminal(&record.snapshot.read().await.state) {
            record.cancellation.cancel();
        }
    } else if snapshot.state == ExecutionState::Cancelling {
        record.cancellation.cancel();
    }
    Ok(json_response(202, &record.snapshot.read().await.clone()))
}

async fn events_route(
    state: NodeState,
    id: ExecutionId,
    query: Option<&str>,
    principal: NodePrincipal,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let generation = match query_parameter(query, "generation") {
        Ok(Some(value)) => match value
            .parse::<u64>()
            .ok()
            .and_then(|v| ExecutionGeneration::new(v).ok())
        {
            Some(generation) => generation,
            None => {
                return Ok(error_response(
                    400,
                    "invalid_request",
                    "generation is invalid",
                ));
            }
        },
        _ => {
            return Ok(error_response(
                400,
                "invalid_request",
                "generation is required",
            ));
        }
    };
    let after = match query_parameter(query, "after") {
        Ok(Some(value)) => match value.parse::<u64>() {
            Ok(after) => after,
            Err(_) => {
                return Ok(error_response(
                    400,
                    "invalid_request",
                    "event cursor is invalid",
                ));
            }
        },
        _ => {
            return Ok(error_response(
                400,
                "invalid_request",
                "event cursor is required",
            ));
        }
    };
    if after > i64::MAX as u64 {
        return Ok(error_response(
            416,
            "cursor_ahead",
            "event cursor is ahead of the journal",
        ));
    }
    let lease_id = match query_parameter(query, "lease") {
        Ok(Some(value)) => match LeaseId::new(value.to_owned()) {
            Ok(lease_id) => lease_id,
            Err(_) => {
                return Ok(error_response(
                    400,
                    "invalid_request",
                    "execution lease is invalid",
                ));
            }
        },
        _ => {
            return Ok(error_response(
                400,
                "invalid_request",
                "execution lease is required",
            ));
        }
    };
    let Some(existing) = state
        .store
        .lookup(id.clone(), generation)
        .await
        .map_err(|_| {
            eggserve_core::server::ServiceError::internal("execution store unavailable")
        })?
    else {
        return Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        ));
    };
    if existing.principal_id != principal.id.as_str()
        || existing.lease_token_hash != store::lease_hash(lease_id.as_str())
    {
        return Ok(error_response(
            403,
            "forbidden",
            "execution event access is not authorized",
        ));
    }
    let record = state.executions.lock().await.get(&id).cloned();
    let record = match record {
        Some(record) if record.snapshot.read().await.generation == generation => record,
        _ => recovered_record(
            match state
                .store
                .load_snapshot(id.clone(), Some(generation))
                .await
                .map_err(|_| {
                    eggserve_core::server::ServiceError::internal("execution store unavailable")
                })? {
                Some(snapshot) => snapshot,
                None => {
                    return Ok(error_response(
                        404,
                        "execution_not_found",
                        "execution not found",
                    ));
                }
            },
            state.store.clone(),
            String::new(),
        ),
    };
    event_response(state, id, generation, after, record).await
}

async fn event_response(
    state: NodeState,
    id: ExecutionId,
    generation: ExecutionGeneration,
    after: u64,
    record: Arc<ExecutionRecord>,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let receiver = record.events.subscribe();
    let Some(page) = state
        .store
        .load_page(id.clone(), generation, after)
        .await
        .map_err(|_| {
            eggserve_core::server::ServiceError::internal("execution store unavailable")
        })?
    else {
        return Ok(error_response(
            404,
            "execution_not_found",
            "execution not found",
        ));
    };
    if after.saturating_add(1) < page.base_sequence {
        increment_metric(&state.store, "event_history_resync", 1).await;
        return Ok(error_response(
            410,
            "history_expired",
            "event history has expired",
        ));
    }
    if after >= page.next_sequence {
        return Ok(error_response(
            416,
            "cursor_ahead",
            "event cursor is ahead of the journal",
        ));
    }
    let terminal = is_terminal(&record.snapshot.read().await.state);
    Ok(event_stream_response(
        id,
        generation,
        after,
        page.events,
        receiver,
        terminal,
    ))
}

fn query_parameter<'a>(query: Option<&'a str>, name: &str) -> Result<Option<&'a str>, ()> {
    let Some(query) = query else { return Ok(None) };
    let mut found = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(());
        };
        if key == name {
            if found.is_some() {
                return Err(());
            }
            found = Some(value);
        } else if key != "generation" && key != "after" && key != "lease" {
            return Err(());
        }
    }
    Ok(found)
}

async fn run_execution(
    state: NodeState,
    record: Arc<ExecutionRecord>,
    request: RunnerRequest,
    workspace: Option<WorkspaceCapture>,
    _permit: OwnedSemaphorePermit,
) {
    publish(
        &record,
        ExecutionState::Running,
        None,
        ExecutionEventKind::State(ExecutionState::Running),
    )
    .await;
    let (output_tx, mut output_rx) = mpsc::channel(RUNNER_CHANNEL_CAPACITY);
    let runner = state.runner.clone();
    let cancellation = record.cancellation.clone();
    let sandbox_request = request.sandbox_request().clone();
    let resource_request = request.resource_setup_request().clone();
    let mut runner_task =
        tokio::spawn(async move { runner.run(request, cancellation, output_tx).await });
    let mut result = None;
    let mut runner_error = None;
    let mut output_closed = false;
    loop {
        tokio::select! {
            biased;
            output = output_rx.recv(), if !output_closed => {
                if let Some(output) = output {
                    let kind = if output.stderr { ExecutionEventKind::Stderr(output.bytes) } else { ExecutionEventKind::Stdout(output.bytes) };
                    publish(&record, ExecutionState::Running, None, kind).await;
                } else {
                    output_closed = true;
                    if result.is_some() { break; }
                }
            }
            joined = &mut runner_task, if result.is_none() && runner_error.is_none() => {
                match joined {
                    Ok(Ok(value)) => result = Some(value),
                    Ok(Err(error)) => runner_error = Some(error),
                    Err(_) => runner_error = Some(RunnerError::Wait("runner task failed".into())),
                }
                if output_rx.is_closed() && output_rx.is_empty() { break; }
            }
        }
    }
    while let Ok(output) = output_rx.try_recv() {
        let kind = if output.stderr {
            ExecutionEventKind::Stderr(output.bytes)
        } else {
            ExecutionEventKind::Stdout(output.bytes)
        };
        publish(&record, ExecutionState::Running, None, kind).await;
    }
    let runner_completed = result.is_some();
    let mut execution_result = if let Some(result) = result {
        let mut execution_result = result.execution_result();
        if record.lease.lock().await.expired.load(Ordering::Acquire) {
            execution_result.state = ExecutionState::Interrupted;
            execution_result.failure = Some(ExecutionFailure::LeaseExpired);
            execution_result.exit_code = None;
        }
        execution_result
    } else {
        let expired = record.lease.lock().await.expired.load(Ordering::Acquire);
        let failure = failure_for_runner_error(runner_error.as_ref(), expired);
        let terminal = if expired {
            ExecutionState::Interrupted
        } else if matches!(failure, ExecutionFailure::Interrupted) {
            ExecutionState::Cancelled
        } else {
            ExecutionState::Failed
        };
        failed_result(terminal, failure, sandbox_request, resource_request)
    };
    if let Some(workspace) = workspace {
        let should_capture = runner_completed
            && !workspace.outputs.is_empty()
            && matches!(
                execution_result.state,
                ExecutionState::Succeeded | ExecutionState::Failed
            )
            && execution_result.failure != Some(ExecutionFailure::OutputLimit);
        if should_capture {
            let expiry = artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
            let (execution_id, generation) = {
                let snapshot = record.snapshot.read().await;
                (snapshot.execution_id.clone(), snapshot.generation)
            };
            match artifact::capture_declared(
                &state.artifacts,
                &state.blobs,
                &workspace.root,
                &workspace.outputs,
                &execution_id,
                generation,
                &workspace.principal,
                expiry,
            )
            .await
            {
                Ok(count) => execution_result.artifact_count = count,
                Err(_) => {
                    execution_result.finalization_failure =
                        Some(ExecutionFinalizationFailure::ArtifactCapture);
                }
            }
        }
        let expiry = artifact::now_unix_ms().saturating_add(artifact::DEFAULT_RETENTION_MILLIS);
        if state
            .workspaces
            .mark_terminal(&workspace.id, expiry, &state.blobs)
            .is_err()
        {
            execution_result.finalization_failure = Some(ExecutionFinalizationFailure::Retention);
        }
    }
    let terminal = execution_result.state.clone();
    let terminal_metric = match terminal {
        ExecutionState::Succeeded => Some("terminal_succeeded"),
        ExecutionState::Failed => Some("terminal_failed"),
        ExecutionState::Cancelled => Some("terminal_cancelled"),
        ExecutionState::TimedOut => Some("terminal_timed_out"),
        ExecutionState::Interrupted => Some("terminal_interrupted"),
        _ => None,
    };
    if let Some(name) = terminal_metric {
        increment_metric(&record.store, name, 1).await;
    }
    increment_metric(&record.store, "stdout_bytes", execution_result.stdout_bytes).await;
    increment_metric(&record.store, "stderr_bytes", execution_result.stderr_bytes).await;
    if execution_result.cleanup_warning.is_some() {
        increment_metric(&record.store, "cleanup_failures", 1).await;
    }
    publish_terminal(
        &record,
        terminal.clone(),
        execution_result,
        ExecutionEventKind::State(terminal),
    )
    .await;
    record.finished.store(true, Ordering::Release);
}

fn failed_result(
    state: ExecutionState,
    failure: ExecutionFailure,
    sandbox_request: SandboxRequest,
    resource_request: ResourceSetupRequest,
) -> ExecutionResult {
    use eggwork_core::ResourceDimensionResult as Dimension;
    fn requested<T>(requirement: eggwork_core::Requirement<T>) -> Dimension {
        match requirement {
            eggwork_core::Requirement::NotRequested => Dimension::NotRequested,
            _ => Dimension::NotApplied {
                reason: "requested resource enforcement failed before execution".into(),
            },
        }
    }
    ExecutionResult {
        state,
        exit_code: None,
        failure: Some(failure),
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_omitted: 0,
        stderr_omitted: 0,
        cleanup_warning: None,
        finalization_failure: None,
        artifact_count: 0,
        sandbox: Some(match sandbox_request {
            SandboxRequest::None => eggwork_core::SandboxResult::NotRequested,
            SandboxRequest::BestEffort { .. } => eggwork_core::SandboxResult::NotApplied {
                reason: "best-effort sandbox setup was unavailable".into(),
            },
            SandboxRequest::Required { .. } => eggwork_core::SandboxResult::Failed {
                reason: "required sandbox setup failed".into(),
            },
        }),
        resources: Some(eggwork_core::ResourceResult {
            memory_bytes: requested(resource_request.memory_bytes),
            cpu_millis: requested(resource_request.cpu_millis),
            pids: requested(resource_request.pids),
        }),
    }
}

fn failure_for_runner_error(
    runner_error: Option<&RunnerError>,
    lease_expired: bool,
) -> ExecutionFailure {
    match runner_error {
        Some(RunnerError::Spawn(_)) => ExecutionFailure::Spawn,
        _ if lease_expired => ExecutionFailure::LeaseExpired,
        Some(RunnerError::CancelledBeforeSpawn) => ExecutionFailure::Interrupted,
        _ => ExecutionFailure::Internal,
    }
}

async fn publish(
    record: &ExecutionRecord,
    state: ExecutionState,
    result: Option<ExecutionResult>,
    kind: ExecutionEventKind,
) {
    let mut current = record.snapshot.write().await;
    if is_terminal(&current.state) {
        return;
    }
    let mut proposed = current.clone();
    proposed.state = state;
    if let Some(result) = result {
        proposed.result = Some(result);
    }
    match record.store.commit_event(proposed.clone(), kind).await {
        Ok(Some(event)) => {
            *current = proposed;
            let _ = record.events.send(event);
        }
        Ok(None) => {}
        Err(_) => {
            record.draining.store(true, Ordering::Release);
            record.cancellation.cancel();
        }
    }
}

async fn publish_terminal(
    record: &ExecutionRecord,
    state: ExecutionState,
    result: ExecutionResult,
    kind: ExecutionEventKind,
) {
    publish(record, state, Some(result), kind).await;
}

fn event_stream_response(
    id: ExecutionId,
    generation: ExecutionGeneration,
    after_sequence: u64,
    replay: Vec<ExecutionEvent>,
    receiver: broadcast::Receiver<ExecutionEvent>,
    initial_terminal: bool,
) -> Response {
    let replay: std::collections::VecDeque<_> = replay.into();
    let ended = initial_terminal && replay.is_empty();
    let stream = stream::unfold(
        (receiver, replay, after_sequence, ended),
        |(mut receiver, mut replay, mut last_sequence, ended)| async move {
            if ended {
                return None;
            }
            loop {
                let event = if let Some(event) = replay.pop_front() {
                    event
                } else {
                    match receiver.recv().await {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            return Some((
                                Err(ResponseStreamError::new(
                                    "event consumer exceeded bounded retained history",
                                )),
                                (receiver, replay, last_sequence, true),
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                };
                if event.sequence.get() <= last_sequence {
                    continue;
                }
                last_sequence = event.sequence.get();
                let terminal = matches!(
                    &event.kind,
                    ExecutionEventKind::State(state) if is_terminal(state)
                );
                let bytes = serde_json::to_vec(&event).unwrap_or_default();
                let mut line = bytes;
                line.push(b'\n');
                return Some((
                    Ok(Bytes::from(line)),
                    (receiver, replay, last_sequence, terminal),
                ));
            }
        },
    );
    let stream = ResponseStream::new(stream);
    Response::builder()
        .status(StatusCode::new(202).expect("valid HTTP status"))
        .header("content-type", "application/x-ndjson")
        .and_then(|builder| builder.header("eggwork-execution-id", id.to_string()))
        .and_then(|builder| {
            builder.header("eggwork-execution-generation", generation.get().to_string())
        })
        .and_then(|builder| builder.body(ResponseBody::Stream(stream)))
        .unwrap_or_else(|_| error_response(500, "internal", "internal error"))
}

fn is_terminal(state: &ExecutionState) -> bool {
    matches!(
        state,
        ExecutionState::Succeeded
            | ExecutionState::Failed
            | ExecutionState::Cancelled
            | ExecutionState::TimedOut
            | ExecutionState::Interrupted
    )
}

fn json_response<T: Serialize>(status: u16, value: &T) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::builder()
        .status(StatusCode::new(status).expect("valid HTTP status"))
        .header("content-type", "application/json")
        .and_then(|builder| builder.body(ResponseBody::Bytes(body)))
        .unwrap_or_else(|_| error_response(500, "internal", "internal error"))
}

fn error_response(status: u16, code: &str, message: &str) -> Response {
    json_response(
        status,
        &ApiError {
            schema_version: API_SCHEMA_VERSION,
            code: code.into(),
            message: message.into(),
        },
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteRequest {
    schema_version: u16,
    handle: ExecutionHandle,
    #[serde(default)]
    workspace_id: Option<eggwork_core::WorkspaceId>,
    spec: ExecutionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    schema_version: u16,
    handle: ExecutionHandle,
    renewal_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggfetch_core::TlsConfig;
    use eggwork_client::NodeClient;
    use eggwork_core::{
        CommandSpec, EnvironmentEntry, EventMetadata, EventSequence, ExecutionHandle,
        IsolationRequirement, NetworkRequirement, OutputPolicy, Requirement, ResourceRequirements,
        StdinPolicy,
    };
    use futures_util::{StreamExt, TryStreamExt, future::join_all};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::{fs, sync::Arc};
    use tempfile::TempDir;
    use uuid::Uuid;

    fn init_tls() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    struct IssuedIdentity {
        cert: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
        cert_pem: String,
        key_pem: String,
    }

    fn issue_identity(
        issuer: &rcgen::Certificate,
        issuer_key: &KeyPair,
        sans: Vec<String>,
    ) -> IssuedIdentity {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(sans).unwrap();
        let cert = params.signed_by(&key, issuer, issuer_key).unwrap();
        IssuedIdentity {
            cert: cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        }
    }

    fn tls_material() -> (
        CertificateDer<'static>,
        IssuedIdentity,
        IssuedIdentity,
        IssuedIdentity,
    ) {
        init_tls();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["eggwork test CA".into()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let root = ca.der().clone();
        let server = issue_identity(&ca, &ca_key, vec!["localhost".into()]);
        let client = issue_identity(&ca, &ca_key, vec!["eggwork-controller".into()]);
        let unknown_client = issue_identity(&ca, &ca_key, vec!["unknown-controller".into()]);
        (root, server, client, unknown_client)
    }

    fn untrusted_client_identity() -> IssuedIdentity {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["untrusted CA".into()]).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = params.self_signed(&key).unwrap();
        issue_identity(&ca, &key, vec!["rogue-controller".into()])
    }

    fn write_client_config(dir: &TempDir, root: &[u8], identity: &IssuedIdentity) -> TlsConfig {
        let ca_path = dir.path().join("ca.pem");
        let cert_path = dir.path().join("client.pem");
        let key_path = dir.path().join("client-key.pem");
        fs::write(&ca_path, pem("CERTIFICATE", root)).unwrap();
        fs::write(&cert_path, &identity.cert_pem).unwrap();
        fs::write(&key_path, &identity.key_pem).unwrap();
        TlsConfig::builder()
            .ca_certificate_path(ca_path)
            .unwrap()
            .client_cert_path(cert_path, key_path)
            .unwrap()
            .build()
    }

    async fn post_raw(endpoint: &str, tls: TlsConfig, body: Vec<u8>) -> u16 {
        let http = eggfetch_core::Client::builder().tls_config(tls).build();
        http.post(endpoint)
            .unwrap()
            .header("content-type", "application/json")
            .bytes(body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    fn pem(label: &str, der: &[u8]) -> String {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(der);
        let lines = encoded
            .as_bytes()
            .chunks(64)
            .map(std::str::from_utf8)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    }

    fn tls_server_config(
        root: CertificateDer<'static>,
        identity: &IssuedIdentity,
    ) -> TlsServerConfig {
        TlsServerConfig::builder()
            .single_identity(vec![identity.cert.clone()], identity.key.clone_key())
            .unwrap()
            .client_auth_required(vec![root])
            .unwrap()
            .build()
            .unwrap()
    }

    fn execution_spec(argv: Vec<String>) -> ExecutionSpec {
        ExecutionSpec {
            schema_version: 1,
            command: CommandSpec {
                argv,
                cwd: None,
                environment: vec![EnvironmentEntry::new("PATH", "/usr/bin:/bin").unwrap()],
                stdin: StdinPolicy::Null,
                timeout_millis: 5_000,
                output: OutputPolicy::default(),
                declared_outputs: vec![],
                resources: ResourceRequirements {
                    memory_bytes: Requirement::NotRequested,
                    cpu_millis: Requirement::NotRequested,
                    pids: Requirement::NotRequested,
                },
                isolation: IsolationRequirement::None,
                network: NetworkRequirement::Unrestricted,
            },
            metadata: vec![],
        }
    }

    fn execution_handle(id: &str) -> ExecutionHandle {
        ExecutionHandle {
            execution_id: ExecutionId::new(id).unwrap(),
            generation: ExecutionGeneration::new(1).unwrap(),
            lease_id: LeaseId::new(Uuid::new_v4().to_string()).unwrap(),
        }
    }

    /// Locate or stage a trusted `eggwork-sandbox-helper` binary for the test
    /// process. The integration runner currently lives in `target/debug/`
    /// while cargo test binaries sit under `target/debug/deps/`, so the helper
    /// cannot rely on `CARGO_BIN_EXE_eggwork-sandbox-helper`. We probe the
    /// current executable's sibling, then its grandparent, then `CARGO_TARGET_DIR`.
    fn place_trusted_helper() -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let source = locate_helper_binary()?;
        let helper_dir = tempfile::tempdir().ok()?;
        let helper = helper_dir.path().join("eggwork-sandbox-helper");
        fs::copy(&source, &helper).ok()?;
        fs::set_permissions(helper_dir.path(), fs::Permissions::from_mode(0o700)).ok()?;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).ok()?;
        // Leak the directory so the helper keeps living for the duration of the
        // test; trust checks happen once at capability probe time.
        let leaked: &'static TempDir = Box::leak(Box::new(helper_dir));
        Some(leaked.path().join("eggwork-sandbox-helper"))
    }

    fn locate_helper_binary() -> Option<PathBuf> {
        let candidates = |base: &std::path::Path| {
            let here = base.join("eggwork-sandbox-helper");
            here.exists().then_some(here)
        };
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            if let Some(candidate) = candidates(parent) {
                return Some(candidate);
            }
            if let Some(grand) = parent.parent()
                && let Some(candidate) = candidates(grand)
            {
                return Some(candidate);
            }
        }
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            let p = std::path::PathBuf::from(target_dir).join("debug/eggwork-sandbox-helper");
            if candidates(p.parent()?).is_some() {
                return Some(p);
            }
        }
        None
    }

    #[test]
    fn fingerprint_mapping_uses_leaf_der_only() {
        let cert = b"verified certificate DER";
        let fingerprint = FingerprintPrincipalResolver::fingerprint(cert);
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = FingerprintPrincipalResolver::new([(fingerprint, principal.clone())]);
        assert_eq!(resolver.resolve_verified_leaf(cert).unwrap().id, principal);
        assert!(resolver.resolve_verified_leaf(b"other cert").is_none());
    }

    #[test]
    fn route_contract_is_fixed_target_and_versioned() {
        assert!(matches!(
            operation_for("POST", "/v1/executions"),
            Some((Operation::Execute, Route::Execute))
        ));
        assert!(matches!(
            operation_for("GET", "/v1/executions/exec-1"),
            Some((Operation::Observe, _))
        ));
        assert!(matches!(
            operation_for("GET", "/v1/executions/exec-1/artifacts"),
            Some((Operation::ArtifactRead, Route::ArtifactList(_)))
        ));
        assert!(matches!(
            operation_for("GET", "/v1/artifacts/00000000-0000-4000-8000-000000000000"),
            Some((Operation::ArtifactRead, Route::ArtifactDownload(_)))
        ));
        assert!(operation_for("POST", "/v1/execute-anywhere").is_none());
        for (method, path, expected) in [
            ("GET", "/v1/capabilities", Operation::Capabilities),
            ("GET", "/v1/status", Operation::Status),
            ("POST", "/v1/executions", Operation::Execute),
            ("GET", "/v1/executions/exec-1", Operation::Observe),
            ("POST", "/v1/executions/exec-1/cancel", Operation::Cancel),
            ("POST", "/v1/executions/exec-1/renew", Operation::Renew),
            ("GET", "/v1/executions/exec-1/events", Operation::Events),
            ("POST", "/v1/blobs/missing", Operation::BlobRead),
            ("POST", "/v1/blobs/prepare", Operation::BlobWrite),
            (
                "PUT",
                "/v1/blobs/0000000000000000000000000000000000000000000000000000000000000000",
                Operation::BlobWrite,
            ),
            (
                "GET",
                "/v1/blobs/0000000000000000000000000000000000000000000000000000000000000000",
                Operation::BlobRead,
            ),
            ("POST", "/v1/workspaces", Operation::WorkspaceCreate),
            ("POST", "/v1/workspaces/derive", Operation::WorkspaceCreate),
            (
                "GET",
                "/v1/executions/exec-1/artifacts",
                Operation::ArtifactRead,
            ),
            ("GET", "/v1/artifacts/artifact-1", Operation::ArtifactRead),
        ] {
            assert_eq!(
                operation_for(method, path).map(|entry| entry.0),
                Some(expected)
            );
        }
        assert_eq!(
            resource_for_path(Operation::Observe, "/v1/executions/exec-1"),
            Some(ResourceId::Execution(ExecutionId::new("exec-1").unwrap()))
        );
    }

    #[test]
    fn request_payload_cannot_supply_authoritative_principal() {
        let mut execute = serde_json::to_value(ExecuteRequest {
            schema_version: API_SCHEMA_VERSION,
            handle: execution_handle("forged-principal"),
            workspace_id: None,
            spec: execution_spec(vec!["/bin/true".into()]),
        })
        .unwrap();
        execute["principal_id"] = serde_json::json!("victim");
        assert!(serde_json::from_value::<ExecuteRequest>(execute).is_err());

        let mut workspace = serde_json::json!({
            "schema_version": API_SCHEMA_VERSION,
            "workspace_id": "forged-workspace",
            "handle": execution_handle("forged-workspace-execution"),
            "manifest": {"schema_version": 1, "entries": []},
            "principal_id": "victim"
        });
        workspace["principal_id"] = serde_json::json!("victim");
        assert!(serde_json::from_value::<WorkspaceCreateRequest>(workspace).is_err());

        let mut derived = serde_json::json!({
            "schema_version": API_SCHEMA_VERSION,
            "workspace_id": "forged-derived-workspace",
            "handle": execution_handle("forged-derived-execution"),
            "patch": {
                "schema_version": 1,
                "base_manifest_digest": eggwork_core::BlobDigest::from_bytes(b"base").as_str(),
                "entries": []
            },
            "principal_id": "victim"
        });
        derived["principal_id"] = serde_json::json!("victim");
        assert!(serde_json::from_value::<DeriveWorkspaceRequest>(derived).is_err());

        let mut control = serde_json::to_value(ControlRequest {
            schema_version: API_SCHEMA_VERSION,
            handle: execution_handle("forged-control-principal"),
            renewal_id: None,
        })
        .unwrap();
        control["principal_id"] = serde_json::json!("victim");
        assert!(serde_json::from_value::<ControlRequest>(control).is_err());
    }

    #[test]
    fn resource_aware_authorizer_can_scope_blob_capability() {
        struct OneBlob;
        impl Authorizer for OneBlob {
            fn authorize(&self, _principal: &NodePrincipal, _operation: Operation) -> bool {
                true
            }

            fn authorize_request(
                &self,
                _principal: &NodePrincipal,
                request: &OperationRequest,
            ) -> bool {
                request.operation != Operation::BlobRead
                    || request.resource
                        == Some(ResourceId::Blob(eggwork_core::BlobDigest::from_bytes(
                            b"allowed",
                        )))
            }
        }
        let principal = NodePrincipal {
            id: eggwork_core::PrincipalId::new("controller-a").unwrap(),
        };
        let authorizer = OneBlob;
        assert!(authorizer.authorize_request(
            &principal,
            &OperationRequest {
                operation: Operation::BlobRead,
                resource: Some(ResourceId::Blob(eggwork_core::BlobDigest::from_bytes(
                    b"allowed"
                ))),
            }
        ));
        assert!(!authorizer.authorize_request(
            &principal,
            &OperationRequest {
                operation: Operation::BlobRead,
                resource: Some(ResourceId::Blob(eggwork_core::BlobDigest::from_bytes(
                    b"denied"
                ))),
            }
        ));
    }

    #[test]
    fn tls_configuration_diagnostics_redact_certificate_paths() {
        let error = NodeStartError::EggServe("/private/client-key.pem".into());
        assert!(!format!("{error:?}").contains("client-key.pem"));
        assert!(!error.to_string().contains("client-key.pem"));
    }

    #[test]
    fn lease_expiry_takes_precedence_over_cancel_before_spawn() {
        assert_eq!(
            failure_for_runner_error(Some(&RunnerError::CancelledBeforeSpawn), true),
            ExecutionFailure::LeaseExpired
        );
        assert_eq!(
            failure_for_runner_error(Some(&RunnerError::CancelledBeforeSpawn), false),
            ExecutionFailure::Interrupted
        );
    }

    #[tokio::test]
    async fn slow_event_consumer_is_bounded_and_gets_a_stream_error() {
        let id = ExecutionId::new("slow-reader").unwrap();
        let (sender, receiver) = broadcast::channel(EVENT_CAPACITY);
        let mut response = event_stream_response(
            id,
            ExecutionGeneration::new(1).unwrap(),
            0,
            vec![],
            receiver,
            false,
        );
        let ResponseBody::Stream(mut stream) = response.take_body().unwrap() else {
            panic!("event response must stream");
        };
        for index in 1..=(EVENT_CAPACITY + 1) {
            let _ = sender.send(ExecutionEvent {
                sequence: EventSequence::new(index as u64),
                kind: ExecutionEventKind::Diagnostic("bounded diagnostic".into()),
                metadata: EventMetadata { fields: vec![] },
            });
        }
        assert!(stream.next().await.unwrap().is_err());
        // A disconnected slow listener does not block event producers.
        assert!(
            sender
                .send(ExecutionEvent {
                    sequence: EventSequence::new((EVENT_CAPACITY + 2) as u64),
                    kind: ExecutionEventKind::Diagnostic("execution continues".into()),
                    metadata: EventMetadata { fields: vec![] },
                })
                .is_ok()
        );
    }

    #[tokio::test]
    async fn mtls_authorization_and_fixed_target_execution() {
        init_tls();
        let (root, server_identity, client_identity, unknown_identity) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-a").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 2,
                lease_ttl: std::time::Duration::from_secs(3),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, operation| {
                !matches!(
                    operation,
                    Operation::Execute
                        | Operation::Observe
                        | Operation::Cancel
                        | Operation::Renew
                        | Operation::Events
                        | Operation::BlobRead
                        | Operation::BlobWrite
                        | Operation::WorkspaceCreate
                        | Operation::ArtifactRead
                )
            }),
        )
        .await
        .unwrap();
        let authorized_temp = TempDir::new().unwrap();
        let unauthorized = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&authorized_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let marker = temp.path().join("must-not-exist");
        let denied_handle = execution_handle("denied-execution");
        let denied = unauthorized
            .execute(
                &execution_spec(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("touch {}", marker.display()),
                ]),
                &denied_handle,
            )
            .await;
        assert!(matches!(
            denied,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(
            !marker.exists(),
            "authorization denial must precede admission and spawn"
        );
        let denied_digest = eggwork_core::BlobDigest::from_bytes(b"denied");
        assert!(matches!(
            unauthorized
                .find_missing_blobs(std::slice::from_ref(&denied_digest))
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized
                .create_workspace(
                    &eggwork_core::WorkspaceId::new("denied-workspace").unwrap(),
                    &execution_handle("denied-workspace-execution"),
                    &eggwork_core::WorkspaceManifest {
                        schema_version: 1,
                        entries: vec![],
                    },
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized
                .artifacts(&ExecutionId::new("denied-artifact-read").unwrap(), 1)
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized
                .upload_blob(&denied_digest, 0, Box::pin(stream::empty()),)
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized.observe(&denied_handle.execution_id).await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized.cancel(&denied_handle).await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized.renew(&denied_handle, "renew-1").await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized.events(&denied_handle, 0).await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(matches!(
            unauthorized.download_blob(&denied_digest).await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        let capabilities = unauthorized.capabilities().await.unwrap();
        // Capability advertisement is now a runtime-probed unified feature
        // snapshot shared with /v1/status. The presence (or absence) of
        // `resources.cgroups-v2.*` therefore reflects the host's runtime
        // probes — not a fixed deny list. The blanket-gate regression suite
        // in this module pins the protocol-level invariant that static and
        // dynamic features stay merged.
        let resource_capabilities_present = capabilities
            .features
            .iter()
            .any(|feature| feature.starts_with("resources.cgroups-v2."));
        if resource_capabilities_present {
            // Verify the same feature appears in /v1/status.
            let status = unauthorized.status().await.unwrap();
            assert!(
                status
                    .capabilities
                    .features
                    .iter()
                    .any(|feature| feature.starts_with("resources.cgroups-v2.")),
                "resources capability must appear in both /v1/capabilities and /v1/status when probed"
            );
        }
        let has_landlock = capabilities
            .features
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1");
        if has_landlock {
            let status = unauthorized.status().await.unwrap();
            assert!(
                status
                    .capabilities
                    .features
                    .iter()
                    .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
                "Landlock capability must appear in both /v1/capabilities and /v1/status when probed"
            );
        }
        assert!(
            capabilities
                .features
                .iter()
                .any(|feature| feature == "network.unrestricted.v1"),
            "node must advertise network.unrestricted.v1"
        );
        assert!(unauthorized.status().await.is_ok());
        let unknown_temp = TempDir::new().unwrap();
        let unknown_client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&unknown_temp, root.as_ref(), &unknown_identity),
        )
        .unwrap();
        assert!(matches!(
            unknown_client.status().await,
            Err(eggwork_client::ClientError::Api { status: 401, .. })
        ));
        let rogue_temp = TempDir::new().unwrap();
        let rogue_client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&rogue_temp, root.as_ref(), &untrusted_client_identity()),
        )
        .unwrap();
        assert!(rogue_client.status().await.is_err());
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    async fn routed_mtls_execution(route: &str, execution_id: &str) {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("routed-work");
        fs::create_dir_all(&work_root).unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            eggwork_core::PrincipalId::new("routed-controller").unwrap(),
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("routed-node").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 2,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::with_eggress_route(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
            route,
        )
        .unwrap();
        let handle = execution_handle(execution_id);
        let stream = client
            .execute(
                &execution_spec(vec!["/bin/sh".into(), "-c".into(), "printf routed".into()]),
                &handle,
            )
            .await
            .unwrap();
        let events = stream.into_events().try_collect::<Vec<_>>().await.unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            ExecutionEventKind::Stdout(bytes) if bytes == b"routed"
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            ExecutionEventKind::State(ExecutionState::Succeeded)
        )));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn socks5_route_preserves_mtls_execution_semantics() {
        let proxy = eggress_testkit::fixtures::Socks5Upstream::start().await;
        routed_mtls_execution(&format!("socks5://{}", proxy.addr()), "socks-routed").await;
        assert!(proxy.connection_count().load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn http_connect_route_preserves_mtls_execution_semantics() {
        let proxy = eggress_testkit::fixtures::HttpConnectUpstream::start().await;
        routed_mtls_execution(&format!("http://{}", proxy.addr()), "connect-routed").await;
        assert!(proxy.connection_count().load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn blob_protocol_streams_verifies_deduplicates_and_enforces_quota() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("blob-controller").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("blob-node").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blobs"),
                blob_quota_bytes: 3 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 3 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();

        assert_eq!(
            eggwork_core::BlobDigest::from_bytes(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let payload = vec![0x5a; 2 * 1024 * 1024];
        let digest = eggwork_core::BlobDigest::from_bytes(&payload);
        assert_eq!(
            client
                .find_missing_blobs(std::slice::from_ref(&digest))
                .await
                .unwrap(),
            vec![digest.clone()]
        );
        let chunks = payload
            .chunks(32 * 1024)
            .map(|chunk| Ok::<_, eggfetch_core::Error>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        client
            .upload_blob(
                &digest,
                payload.len() as u64,
                Box::pin(stream::iter(chunks)),
            )
            .await
            .unwrap();
        assert!(
            client
                .find_missing_blobs(std::slice::from_ref(&digest))
                .await
                .unwrap()
                .is_empty()
        );
        // Same-digest upload is a verified no-op even when the quota is full.
        let chunks = payload
            .chunks(64 * 1024)
            .map(|chunk| Ok::<_, eggfetch_core::Error>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        client
            .upload_blob(
                &digest,
                payload.len() as u64,
                Box::pin(stream::iter(chunks)),
            )
            .await
            .unwrap();
        let downloaded = client
            .download_blob(&digest)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let downloaded: Vec<u8> = downloaded.into_iter().flatten().collect();
        assert_eq!(downloaded, payload);

        let other = vec![0x33; 2 * 1024 * 1024];
        let other_digest = eggwork_core::BlobDigest::from_bytes(&other);
        let chunks = other
            .chunks(32 * 1024)
            .map(|chunk| Ok::<_, eggfetch_core::Error>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        assert!(matches!(
            client
                .upload_blob(
                    &other_digest,
                    other.len() as u64,
                    Box::pin(stream::iter(chunks))
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 507, .. })
        ));
        let wrong_digest = eggwork_core::BlobDigest::from_bytes(b"wrong");
        let chunks = vec![Ok::<_, eggfetch_core::Error>(Bytes::from_static(b"abc"))];
        assert!(matches!(
            client
                .upload_blob(&wrong_digest, 3, Box::pin(stream::iter(chunks)))
                .await,
            Err(eggwork_client::ClientError::Api { status: 422, .. })
        ));

        let workspace_bytes = b"workspace says hi\n";
        let workspace_digest = eggwork_core::BlobDigest::from_bytes(workspace_bytes);
        let workspace_chunks = vec![Ok::<_, eggfetch_core::Error>(Bytes::copy_from_slice(
            workspace_bytes,
        ))];
        client
            .upload_blob(
                &workspace_digest,
                workspace_bytes.len() as u64,
                Box::pin(stream::iter(workspace_chunks)),
            )
            .await
            .unwrap();
        let workspace_id = eggwork_core::WorkspaceId::new("demo-workspace").unwrap();
        let workspace_handle = execution_handle("workspace-execution");
        let manifest = eggwork_core::WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                eggwork_core::WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/message.txt").unwrap(),
                    digest: workspace_digest,
                    size_bytes: workspace_bytes.len() as u64,
                    executable: false,
                },
            ],
        };
        let ready = client
            .create_workspace(&workspace_id, &workspace_handle, &manifest)
            .await
            .unwrap();
        assert_eq!(ready.workspace_id, workspace_id);
        let mut workspace_spec = execution_spec(vec![
            "/bin/sh".into(),
            "-c".into(),
            "cat message.txt | tee result.txt".into(),
        ]);
        workspace_spec.command.cwd = Some(eggwork_core::RelativePath::new("src").unwrap());
        workspace_spec.command.declared_outputs = vec![eggwork_core::DeclaredOutput {
            path: eggwork_core::RelativePath::new("src/result.txt").unwrap(),
            required: true,
        }];
        let stream = client
            .execute_in_workspace(&workspace_spec, &workspace_handle, &workspace_id)
            .await
            .unwrap();
        let workspace_events = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            stream.into_events().try_collect::<Vec<_>>(),
        )
        .await
        .expect("workspace execution reaches terminal state")
        .unwrap();
        assert!(workspace_events.iter().any(|event| matches!(
            &event.kind,
            ExecutionEventKind::Stdout(bytes) if bytes == workspace_bytes
        )));
        let final_snapshot = client
            .observe_generation(
                &workspace_handle.execution_id,
                workspace_handle.generation.get(),
            )
            .await
            .unwrap();
        let result = final_snapshot.result.unwrap();
        assert_eq!(result.artifact_count, 1);
        assert_eq!(result.finalization_failure, None);
        let artifacts = client
            .artifacts(
                &workspace_handle.execution_id,
                workspace_handle.generation.get(),
            )
            .await
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].path.as_str(), "src/result.txt");
        let downloaded = client
            .download_artifact(&artifacts[0])
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(downloaded, workspace_bytes);
        let missing_handle = execution_handle("artifact-finalization-failure");
        let missing_workspace =
            eggwork_core::WorkspaceId::new("artifact-missing-workspace").unwrap();
        client
            .create_workspace(
                &missing_workspace,
                &missing_handle,
                &eggwork_core::WorkspaceManifest {
                    schema_version: 1,
                    entries: vec![],
                },
            )
            .await
            .unwrap();
        let mut missing_output_spec = execution_spec(vec!["/bin/true".into()]);
        missing_output_spec.command.declared_outputs = vec![eggwork_core::DeclaredOutput {
            path: eggwork_core::RelativePath::new("required-missing.txt").unwrap(),
            required: true,
        }];
        client
            .execute_in_workspace(&missing_output_spec, &missing_handle, &missing_workspace)
            .await
            .unwrap()
            .into_events()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let failed_finalization = client
            .observe_generation(
                &missing_handle.execution_id,
                missing_handle.generation.get(),
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(failed_finalization.state, ExecutionState::Succeeded);
        assert_eq!(failed_finalization.exit_code, Some(0));
        assert_eq!(failed_finalization.artifact_count, 0);
        assert_eq!(
            failed_finalization.finalization_failure,
            Some(ExecutionFinalizationFailure::ArtifactCapture)
        );
        let timeout_handle = execution_handle("artifact-timeout");
        let timeout_workspace =
            eggwork_core::WorkspaceId::new("artifact-timeout-workspace").unwrap();
        client
            .create_workspace(
                &timeout_workspace,
                &timeout_handle,
                &eggwork_core::WorkspaceManifest {
                    schema_version: 1,
                    entries: vec![],
                },
            )
            .await
            .unwrap();
        let mut timeout_spec = execution_spec(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf partial > partial.txt; sleep 1".into(),
        ]);
        timeout_spec.command.timeout_millis = 100;
        timeout_spec.command.declared_outputs = vec![eggwork_core::DeclaredOutput {
            path: eggwork_core::RelativePath::new("partial.txt").unwrap(),
            required: true,
        }];
        client
            .execute_in_workspace(&timeout_spec, &timeout_handle, &timeout_workspace)
            .await
            .unwrap()
            .into_events()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let timed_out = client
            .observe_generation(
                &timeout_handle.execution_id,
                timeout_handle.generation.get(),
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(timed_out.state, ExecutionState::TimedOut);
        assert_eq!(timed_out.artifact_count, 0);
        assert!(
            client
                .artifacts(
                    &timeout_handle.execution_id,
                    timeout_handle.generation.get()
                )
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            client
                .execute_in_workspace(
                    &workspace_spec,
                    &execution_handle("foreign-workspace-execution"),
                    &workspace_id,
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));

        fs::write(server.state.blobs.path_for(&digest), b"corruption").unwrap();
        assert!(matches!(
            client.download_blob(&digest).await,
            Err(eggwork_client::ClientError::Api { status: 500, .. })
        ));
        assert!(!server.state.blobs.path_for(&digest).exists());
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn mtls_execution_streams_output_and_missing_certificate_is_rejected() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-a").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();

        let address = format!("https://localhost:{}", server.local_addr().port());
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            &address,
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        assert_eq!(
            post_raw(
                &format!("{address}/v1/executions"),
                write_client_config(&client_temp, root.as_ref(), &client_identity),
                b"{".to_vec(),
            )
            .await,
            400
        );
        assert_eq!(
            post_raw(
                &format!("{address}/v1/executions"),
                write_client_config(&client_temp, root.as_ref(), &client_identity),
                vec![b'x'; MAX_REQUEST_BYTES + 1],
            )
            .await,
            413
        );
        let unsupported_schema = serde_json::to_vec(&ExecuteRequest {
            schema_version: API_SCHEMA_VERSION + 1,
            handle: execution_handle("unsupported-schema"),
            workspace_id: None,
            spec: execution_spec(vec!["/bin/true".into()]),
        })
        .unwrap();
        assert_eq!(
            post_raw(
                &format!("{address}/v1/executions"),
                write_client_config(&client_temp, root.as_ref(), &client_identity),
                unsupported_schema,
            )
            .await,
            426
        );
        let execution = client
            .execute(
                &execution_spec(vec!["/bin/echo".into(), "hello eggwork".into()]),
                &execution_handle("echo-output"),
            )
            .await
            .unwrap();
        let id = execution.execution_id.clone();
        let event_read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execution.into_events().collect::<Vec<_>>(),
        )
        .await;
        assert!(
            event_read.is_ok(),
            "live event stream must end at terminal state"
        );
        let events: Vec<_> = event_read
            .unwrap()
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind, ExecutionEventKind::Stdout(bytes) if bytes == b"hello eggwork\n")));
        let snapshot = client.observe(&id).await.unwrap();
        assert_eq!(snapshot.state, ExecutionState::Succeeded);
        assert!(snapshot.result.is_some());

        let idempotent_handle = execution_handle("concurrent-same-request");
        let marker = temp.path().join("spawn-count");
        let idempotent_spec = execution_spec(vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("printf x >> '{}'; sleep 0.25", marker.display()),
        ]);
        let submissions = join_all((0..24).map(|_| {
            let client = client.clone();
            let handle = idempotent_handle.clone();
            let spec = idempotent_spec.clone();
            async move { client.execute(&spec, &handle).await }
        }))
        .await;
        let streams: Vec<_> = submissions.into_iter().map(Result::unwrap).collect();
        assert!(
            streams
                .iter()
                .all(|stream| stream.execution_id == idempotent_handle.execution_id)
        );
        drop(streams);
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if client
                    .observe(&idempotent_handle.execution_id)
                    .await
                    .unwrap()
                    .state
                    == ExecutionState::Succeeded
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the single idempotent execution must finish");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "x");
        assert!(matches!(
            client
                .execute(
                    &execution_spec(vec!["/bin/true".into()]),
                    &idempotent_handle
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));

        let next_generation = ExecutionHandle {
            generation: ExecutionGeneration::new(2).unwrap(),
            lease_id: LeaseId::new(Uuid::new_v4().to_string()).unwrap(),
            ..idempotent_handle.clone()
        };
        let generation_two = client
            .execute(&execution_spec(vec!["/bin/true".into()]), &next_generation)
            .await
            .unwrap();
        assert!(matches!(
            client.cancel(&idempotent_handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        drop(generation_two);
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if client
                    .observe_generation(&next_generation.execution_id, 2)
                    .await
                    .unwrap()
                    .state
                    == ExecutionState::Succeeded
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("next generation must become terminal");

        let running = client
            .execute(
                &execution_spec(vec!["/bin/sh".into(), "-c".into(), "sleep 2".into()]),
                &execution_handle("disconnected-run"),
            )
            .await
            .unwrap();
        let running_id = running.execution_id.clone();
        let running_handle = running.handle.clone();
        let events_client = client.clone();
        let events_task = tokio::spawn(async move {
            let stream = events_client.events(&running_handle, 0).await?;
            Ok::<_, eggwork_client::ClientError>(stream.try_collect::<Vec<_>>().await?)
        });
        let live_gc = server.collect_garbage(false, 1).await.unwrap();
        assert!(!live_gc.dry_run);
        assert_eq!(client.status().await.unwrap().active_executions, 1);
        assert!(matches!(
            client
                .execute(
                    &execution_spec(vec!["/bin/true".into()]),
                    &execution_handle("busy-one")
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        operations::set_persistent_drain(&server.state.drain_path, true).unwrap();
        assert!(server.is_draining());
        assert!(matches!(
            client
                .execute(
                    &execution_spec(vec!["/bin/true".into()]),
                    &execution_handle("drain-one")
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        operations::set_persistent_drain(&server.state.drain_path, false).unwrap();
        assert!(!server.is_draining());
        drop(running); // A disconnected live stream does not cancel execution.
        let attached_events = tokio::time::timeout(std::time::Duration::from_secs(4), events_task)
            .await
            .expect("GET events stream should end at terminal state")
            .unwrap()
            .unwrap();
        assert!(!attached_events.is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if client.observe(&running_id).await.unwrap().state == ExecutionState::Succeeded {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("disconnect must not cancel execution");

        let cancellable = client
            .execute(
                &execution_spec(vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()]),
                &execution_handle("cancel-run"),
            )
            .await
            .unwrap();
        let cancellable_id = cancellable.execution_id.clone();
        let cancellable_handle = cancellable.handle.clone();
        drop(cancellable);
        client
            .renew(&cancellable_handle, "renewal-one")
            .await
            .unwrap();
        client
            .renew(&cancellable_handle, "renewal-one")
            .await
            .unwrap();
        client.cancel(&cancellable_handle).await.unwrap();
        client.cancel(&cancellable_handle).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if client.observe(&cancellable_id).await.unwrap().state == ExecutionState::Cancelled
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("explicit cancellation reaches a terminal state");
        let cancelled_snapshot = client.observe(&cancellable_id).await.unwrap();
        assert_eq!(
            cancelled_snapshot.result.unwrap().state,
            ExecutionState::Cancelled
        );
        let cancelled_events = client.events(&cancellable_handle, 0).await.unwrap();
        let event_chunks = cancelled_events.try_collect::<Vec<_>>().await.unwrap();
        let joined: Vec<u8> = event_chunks.into_iter().flatten().collect();
        let cancelled_events: Vec<ExecutionEvent> = joined
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(
            cancelled_events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    ExecutionEventKind::State(ExecutionState::Cancelled)
                ))
                .count(),
            1
        );
        assert_eq!(
            cancelled_events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    ExecutionEventKind::State(ExecutionState::Cancelling)
                ))
                .count(),
            1
        );
        let last_sequence = cancelled_events.last().unwrap().sequence.get();
        let resumed = client
            .events(&cancellable_handle, last_sequence)
            .await
            .unwrap();
        assert!(resumed.try_collect::<Vec<_>>().await.unwrap().is_empty());

        let no_cert = eggfetch_core::TlsConfig::builder()
            .ca_certificate_path({
                let path = client_temp.path().join("server-ca.pem");
                fs::write(&path, pem("CERTIFICATE", root.as_ref())).unwrap();
                path
            })
            .unwrap()
            .build();
        let unauthenticated = NodeClient::new(&address, no_cert).unwrap();
        assert!(unauthenticated.status().await.is_err());
        let shutdown_execution = client
            .execute(
                &execution_spec(vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()]),
                &execution_handle("shutdown-run"),
            )
            .await
            .unwrap();
        let shutdown_record = server
            .state
            .executions
            .lock()
            .await
            .get(&shutdown_execution.execution_id)
            .unwrap()
            .clone();
        drop(shutdown_execution);
        server.shutdown().await;
        server.wait().await.unwrap();
        assert_eq!(
            shutdown_record.snapshot.read().await.state,
            ExecutionState::Cancelled
        );
    }

    #[tokio::test]
    async fn expired_lease_terminates_process_with_a_typed_terminal_result() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-lease").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("lease.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_millis(150),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let handle = execution_handle("lease-expiry");
        let stream = client
            .execute(
                &execution_spec(vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()]),
                &handle,
            )
            .await
            .unwrap();
        drop(stream);
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let snapshot = client.observe(&handle.execution_id).await.unwrap();
                if is_terminal(&snapshot.state) {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("lease expiry must terminate the process");
        assert_eq!(snapshot.state, ExecutionState::Interrupted);
        assert_eq!(
            snapshot.result.unwrap().failure,
            Some(ExecutionFailure::LeaseExpired)
        );
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn restart_recovers_uncertain_execution_as_interrupted_without_replay() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("recovery.sqlite");
        let store = store::ExecutionStore::open(&database_path).unwrap();
        let handle = execution_handle("restart-recovery");
        let running = ExecutionSnapshot {
            schema_version: API_SCHEMA_VERSION,
            execution_id: handle.execution_id.clone(),
            generation: handle.generation,
            state: ExecutionState::Running,
            result: None,
        };
        assert!(matches!(
            store
                .reserve(
                    ExecutionSnapshot {
                        state: ExecutionState::Accepted,
                        ..running.clone()
                    },
                    eggwork_core::CANONICAL_REQUEST_VERSION,
                    "a".repeat(64),
                    "controller-a".into(),
                    store::lease_hash(handle.lease_id.as_str()),
                    store::unix_millis() + 60_000,
                )
                .await
                .unwrap(),
            store::ReserveResult::Created
        ));
        store
            .commit_event(
                ExecutionSnapshot {
                    state: ExecutionState::Accepted,
                    ..running.clone()
                },
                ExecutionEventKind::State(ExecutionState::Accepted),
            )
            .await
            .unwrap();
        store
            .commit_event(
                running.clone(),
                ExecutionEventKind::State(ExecutionState::Running),
            )
            .await
            .unwrap();
        drop(store);

        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-recovery").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path,
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(30),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let recovered = client.observe(&handle.execution_id).await.unwrap();
        assert_eq!(recovered.state, ExecutionState::Interrupted);
        assert_eq!(client.status().await.unwrap().active_executions, 0);
        let event_stream = client.events(&handle, 0).await.unwrap();
        let event_bytes = event_stream.try_collect::<Vec<_>>().await.unwrap();
        let joined: Vec<u8> = event_bytes.into_iter().flatten().collect();
        let lines: Vec<ExecutionEvent> = joined
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert!(lines.iter().any(|event| matches!(
            event.kind,
            ExecutionEventKind::State(ExecutionState::Interrupted)
        )));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    async fn upload_test_blob(
        client: &NodeClient,
        bytes: &'static [u8],
    ) -> eggwork_core::BlobDigest {
        let digest = eggwork_core::BlobDigest::from_bytes(bytes);
        let chunks = vec![Ok::<_, eggfetch_core::Error>(Bytes::copy_from_slice(bytes))];
        client
            .upload_blob(&digest, bytes.len() as u64, Box::pin(stream::iter(chunks)))
            .await
            .unwrap();
        digest
    }

    #[tokio::test]
    async fn derived_workspace_materialization_loopback() {
        init_tls();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["eggwork derive test CA".into()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let root = ca.der().clone();
        let server_identity = issue_identity(&ca, &ca_key, vec!["localhost".into()]);
        let client_a_identity = issue_identity(&ca, &ca_key, vec!["controller-a".into()]);
        let client_b_identity = issue_identity(&ca, &ca_key, vec!["controller-b".into()]);
        let principal_a = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let principal_b = eggwork_core::PrincipalId::new("controller-b").unwrap();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([
            (
                FingerprintPrincipalResolver::fingerprint(client_a_identity.cert.as_ref()),
                principal_a.clone(),
            ),
            (
                FingerprintPrincipalResolver::fingerprint(client_b_identity.cert.as_ref()),
                principal_b.clone(),
            ),
        ]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-derive").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 64 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 64 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let endpoint = format!("https://localhost:{}", server.local_addr().port());
        let temp_a = TempDir::new().unwrap();
        let temp_b = TempDir::new().unwrap();
        let client_a = NodeClient::new(
            &endpoint,
            write_client_config(&temp_a, root.as_ref(), &client_a_identity),
        )
        .unwrap();
        let client_b = NodeClient::new(
            &endpoint,
            write_client_config(&temp_b, root.as_ref(), &client_b_identity),
        )
        .unwrap();

        // The versioned derive capability is advertised alongside the
        // existing workspace capabilities; no existing feature is removed.
        let capabilities = client_a.capabilities().await.unwrap();
        assert!(
            capabilities
                .features
                .iter()
                .any(|feature| feature == "workspace.derive.v1"),
            "node must advertise workspace.derive.v1"
        );
        assert!(
            capabilities
                .features
                .iter()
                .any(|feature| feature == "workspace.materialize.v1")
        );

        let base_bytes = b"derive base v1";
        let updated_bytes = b"derive base v2 with more content";
        let base_digest = upload_test_blob(&client_a, base_bytes).await;
        let updated_digest = upload_test_blob(&client_a, updated_bytes).await;
        let base_manifest = eggwork_core::WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                eggwork_core::WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/app.txt").unwrap(),
                    digest: base_digest.clone(),
                    size_bytes: base_bytes.len() as u64,
                    executable: false,
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("stale.txt").unwrap(),
                    digest: base_digest.clone(),
                    size_bytes: base_bytes.len() as u64,
                    executable: false,
                },
            ],
        };
        let base_canonical = base_manifest.digest().unwrap();
        let base_handle = execution_handle("derive-base-execution");
        client_a
            .create_workspace(
                &eggwork_core::WorkspaceId::new("derive-base").unwrap(),
                &base_handle,
                &base_manifest,
            )
            .await
            .unwrap();

        // Authenticated full -> derived flow with remove + upsert semantics.
        let patch = eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: base_canonical.clone(),
            entries: vec![
                eggwork_core::WorkspacePatchEntry::Remove {
                    path: eggwork_core::RelativePath::new("stale.txt").unwrap(),
                },
                eggwork_core::WorkspacePatchEntry::File {
                    path: eggwork_core::RelativePath::new("src/app.txt").unwrap(),
                    digest: updated_digest.clone(),
                    size_bytes: updated_bytes.len() as u64,
                    executable: true,
                },
            ],
        };
        let derived_handle = execution_handle("derive-execution");
        let derived_id = eggwork_core::WorkspaceId::new("derive-target").unwrap();
        let derived = client_a
            .create_workspace_derived(&derived_id, &derived_handle, &base_canonical, &patch)
            .await
            .unwrap();
        // The derived response digest equals the locally reconstructed full
        // manifest digest: the optimization changes no content facts.
        let expected = eggwork_core::WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                eggwork_core::WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/app.txt").unwrap(),
                    digest: updated_digest,
                    size_bytes: updated_bytes.len() as u64,
                    executable: true,
                },
            ],
        };
        assert_eq!(derived.manifest_digest, expected.digest().unwrap());
        assert_eq!(derived.logical_bytes, expected.logical_bytes());
        let resolved = server
            .state
            .workspaces
            .resolve(
                &derived_id,
                &derived_handle.execution_id,
                derived_handle.generation,
                &principal_a,
            )
            .unwrap();
        assert_eq!(
            fs::read(resolved.root.join("src/app.txt")).unwrap(),
            updated_bytes
        );
        assert!(!resolved.root.join("stale.txt").exists());

        // A full create of the equivalent manifest converges on the same
        // digest without sharing execution ownership.
        let converged = client_a
            .create_workspace(
                &eggwork_core::WorkspaceId::new("derive-converged").unwrap(),
                &execution_handle("derive-converged-execution"),
                &expected,
            )
            .await
            .unwrap();
        assert_eq!(converged.manifest_digest, derived.manifest_digest);

        // Transport retry: repeating the same derived request is idempotent.
        let retried = client_a
            .create_workspace_derived(&derived_id, &derived_handle, &base_canonical, &patch)
            .await
            .unwrap();
        assert_eq!(retried.manifest_digest, derived.manifest_digest);

        // Unknown bases are typed non-destructive misses.
        let unknown = eggwork_core::BlobDigest::from_bytes(b"never retained");
        let unknown_patch = eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: unknown,
            entries: vec![],
        };
        let missing = client_a
            .create_workspace_derived(
                &eggwork_core::WorkspaceId::new("derive-unknown-base").unwrap(),
                &execution_handle("derive-unknown-execution"),
                &eggwork_core::BlobDigest::from_bytes(b"never retained"),
                &unknown_patch,
            )
            .await;
        assert!(
            matches!(missing, Err(eggwork_client::ClientError::Api { status: 409, ref code, .. }) if code == "base_manifest_missing"),
            "unknown base must be a typed miss, got {missing:?}"
        );

        // Another principal's retained manifest is unusable and reports the
        // identical miss: existence is not revealed across principals.
        let foreign = client_b
            .create_workspace_derived(
                &eggwork_core::WorkspaceId::new("derive-foreign").unwrap(),
                &execution_handle("derive-foreign-execution"),
                &base_canonical,
                &patch,
            )
            .await;
        assert!(
            matches!(foreign, Err(eggwork_client::ClientError::Api { status: 409, ref code, .. }) if code == "base_manifest_missing"),
            "cross-principal base must look missing, got {foreign:?}"
        );

        // Malformed patches are rejected without creating a workspace.
        let duplicate_patch = eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: base_canonical.clone(),
            entries: vec![
                eggwork_core::WorkspacePatchEntry::Remove {
                    path: eggwork_core::RelativePath::new("stale.txt").unwrap(),
                },
                eggwork_core::WorkspacePatchEntry::Remove {
                    path: eggwork_core::RelativePath::new("stale.txt").unwrap(),
                },
            ],
        };
        assert!(matches!(
            client_a
                .create_workspace_derived(
                    &eggwork_core::WorkspaceId::new("derive-bad-patch").unwrap(),
                    &execution_handle("derive-bad-patch-execution"),
                    &base_canonical,
                    &duplicate_patch,
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 400, .. })
        ));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn derived_workspace_denied_and_oversized_bodies_are_typed() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        struct DenyDerive;
        impl Authorizer for DenyDerive {
            fn authorize(&self, _: &NodePrincipal, operation: Operation) -> bool {
                !matches!(operation, Operation::WorkspaceCreate)
            }
        }
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-derive-deny").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(DenyDerive),
        )
        .await
        .unwrap();
        let endpoint = format!("https://localhost:{}", server.local_addr().port());
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            &endpoint,
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let digest = eggwork_core::BlobDigest::from_bytes(b"denied base");
        let patch = eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: digest.clone(),
            entries: vec![],
        };
        assert!(matches!(
            client
                .create_workspace_derived(
                    &eggwork_core::WorkspaceId::new("denied-derive").unwrap(),
                    &execution_handle("denied-derive-execution"),
                    &digest,
                    &patch,
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert_eq!(
            post_raw(
                &format!("{endpoint}/v1/workspaces/derive"),
                write_client_config(&client_temp, root.as_ref(), &client_identity),
                vec![b'x'; MAX_WORKSPACE_REQUEST_BYTES + 1],
            )
            .await,
            413
        );
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn operator_drain_persists_across_node_restart() {
        init_tls();
        let (root, server_identity, _, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let config = NodeConfig {
            node_id: NodeId::new("node-drain-restart").unwrap(),
            bind: "127.0.0.1:0".parse().unwrap(),
            execution_root: work_root,
            database_path: temp.path().join("drain.sqlite"),
            blob_root: temp.path().join("blob-store"),
            blob_quota_bytes: 1024 * 1024,
            workspace_root: temp.path().join("workspace-store"),
            workspace_quota_bytes: 1024 * 1024,
            max_active_executions: 1,
            lease_ttl: std::time::Duration::from_secs(30),
            tls: tls_server_config(root, &server_identity),
        };
        let start = || {
            NodeServer::start(
                config.clone(),
                Arc::new(LocalProcessRunner::default()),
                Arc::new(FingerprintPrincipalResolver::default()),
                Arc::new(|_: &NodePrincipal, _| true),
            )
        };
        let first = start().await.unwrap();
        assert!(matches!(start().await, Err(NodeStartError::AlreadyRunning)));
        first.set_draining(true);
        assert!(first.is_draining());
        first.shutdown().await;
        first.wait().await.unwrap();

        let second = start().await.unwrap();
        assert!(second.is_draining());
        second.set_draining(false);
        assert!(!second.is_draining());
        second.shutdown().await;
        second.wait().await.unwrap();
    }

    #[tokio::test]
    async fn capabilities_and_status_features_agree_with_a_unified_snapshot() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let helper = place_trusted_helper();
        let helper_present = helper.is_some();
        let runner = match helper {
            Some(path) => Arc::new(LocalProcessRunner::new(
                eggwork_runner::TrustedLandlockSetup::new(path),
            )),
            None => Arc::new(LocalProcessRunner::default()),
        };
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-cap-consistency").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let capabilities = client.capabilities().await.unwrap();
        let status = client.status().await.unwrap();
        let capability_features = capabilities.features.clone();
        let status_features = status.capabilities.features.clone();
        assert_eq!(
            capability_features, status_features,
            "/v1/capabilities and /v1/status MUST agree on the feature list"
        );
        assert!(
            capability_features.contains(&"network.unrestricted.v1".to_owned()),
            "node must advertise network.unrestricted.v1; got {capability_features:?}"
        );
        assert!(
            capability_features.contains(&"exec.argv.v1".to_owned()),
            "node must advertise exec.argv.v1; got {capability_features:?}"
        );
        if helper_present {
            assert!(
                capability_features
                    .iter()
                    .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
                "trusted helper + Landlock-capable host must advertise Landlock capability; got {capability_features:?}"
            );
        } else {
            assert!(
                !capability_features
                    .iter()
                    .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
                "host without sandbox helper must not advertise Landlock capability; got {capability_features:?}"
            );
        }
        capabilities.validate().unwrap();
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn required_landlock_is_admitted_remotely_and_denies_outside_workspace_access() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let helper = match place_trusted_helper() {
            Some(helper) => helper,
            None => {
                eprintln!(
                    "skipping required_landlock_is_admitted_remotely: sandbox-helper binary not located"
                );
                return;
            }
        };
        let runner = Arc::new(LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(helper),
        ));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-required-landlock").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let capabilities = client.capabilities().await.unwrap();
        if !capabilities
            .features
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1")
        {
            eprintln!(
                "skipping required_landlock_is_admitted_remotely: kernel does not advertise Landlock capability"
            );
            server.shutdown().await;
            server.wait().await.unwrap();
            return;
        }

        // Stage workspace content with a host-side escape target the target must NOT read.
        let inside = b"inside-content\n";
        let inside_digest = eggwork_core::BlobDigest::from_bytes(inside);
        let chunks = vec![Ok::<_, eggfetch_core::Error>(Bytes::from_static(inside))];
        client
            .upload_blob(
                &inside_digest,
                inside.len() as u64,
                Box::pin(stream::iter(chunks)),
            )
            .await
            .unwrap();
        // Plant an escape target outside the workspace root (so the path is
        // not covered by any Landlock allow rule).
        let escape_target = temp.path().join("landlock-escape-target.txt");
        fs::write(&escape_target, b"outside-content\n").unwrap();
        let workspace_id = eggwork_core::WorkspaceId::new("remote-landlock-ws").unwrap();
        let workspace_handle = execution_handle("remote-landlock-ws-exec");
        let manifest = eggwork_core::WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                eggwork_core::WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/hello.txt").unwrap(),
                    digest: inside_digest,
                    size_bytes: inside.len() as u64,
                    executable: false,
                },
            ],
        };
        client
            .create_workspace(&workspace_id, &workspace_handle, &manifest)
            .await
            .unwrap();
        let escape_target_path = escape_target.display().to_string();
        let shell_command = format!(
            "set -e; cat hello.txt; if cat '{escape_target_path}' >/dev/null 2>&1; then exit 77; fi; exit 0"
        );
        let mut spec = execution_spec(vec!["/bin/sh".into(), "-c".into(), shell_command]);
        spec.command.isolation = IsolationRequirement::Required;
        spec.command.cwd = Some(eggwork_core::RelativePath::new("src").unwrap());
        let stream = client
            .execute_in_workspace(&spec, &workspace_handle, &workspace_id)
            .await
            .unwrap();
        let events = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.into_events().try_collect::<Vec<_>>(),
        )
        .await
        .expect("required-isolation execution must terminate")
        .unwrap();
        let snapshot = client
            .observe_generation(
                &workspace_handle.execution_id,
                workspace_handle.generation.get(),
            )
            .await
            .unwrap();
        let result = snapshot.result.unwrap();
        assert_eq!(snapshot.state, ExecutionState::Succeeded, "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            ExecutionEventKind::Stdout(bytes) if bytes == inside
        )));
        assert!(matches!(
            result.sandbox,
            Some(eggwork_core::SandboxResult::Applied { .. })
        ));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn required_landlock_rejects_with_capability_mismatch_when_helper_is_missing() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let runner = Arc::new(LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(std::path::PathBuf::from(
                "/nonexistent/eggwork-sandbox-helper",
            )),
        ));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-no-helper").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let capabilities = client.capabilities().await.unwrap();
        assert!(
            !capabilities
                .features
                .iter()
                .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
            "node without trusted helper must not advertise Landlock capability; got {:?}",
            capabilities.features
        );

        let mut spec = execution_spec(vec!["/bin/true".into()]);
        spec.command.isolation = IsolationRequirement::Required;
        let handle = execution_handle("required-without-helper");
        assert!(matches!(
            client.execute(&spec, &handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        // No reservation/target spawned: the generation must not have entered the runner.
        let snapshot = client.observe(&handle.execution_id).await;
        assert!(
            matches!(
                snapshot,
                Err(eggwork_client::ClientError::Api { status: 404, .. })
            ),
            "rejected capability check must leave no accepted execution record; got {snapshot:?}"
        );
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn best_effort_landlock_reports_not_applied_when_helper_is_missing() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let runner = Arc::new(LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(std::path::PathBuf::from(
                "/nonexistent/eggwork-sandbox-helper",
            )),
        ));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-best-effort-no-helper").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let mut spec = execution_spec(vec!["/bin/true".into()]);
        spec.command.isolation = IsolationRequirement::BestEffort;
        let handle = execution_handle("best-effort-without-helper");
        let stream = client.execute(&spec, &handle).await.unwrap();
        let events = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.into_events().try_collect::<Vec<_>>(),
        )
        .await
        .expect("best-effort execution must terminate")
        .unwrap();
        let snapshot = client
            .observe_generation(&handle.execution_id, handle.generation.get())
            .await
            .unwrap();
        let result = snapshot.result.unwrap();
        assert_eq!(snapshot.state, ExecutionState::Succeeded);
        assert_eq!(result.exit_code, Some(0));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ExecutionEventKind::State(ExecutionState::Succeeded)
        )));
        assert!(matches!(
            result.sandbox,
            Some(eggwork_core::SandboxResult::NotApplied { .. })
        ));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn required_resource_admission_rejects_with_capability_mismatch_when_dimension_unavailable()
     {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let runner = Arc::new(LocalProcessRunner::new(eggwork_runner::NoExecutionSetup));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-resource-rejection").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let capabilities = client.capabilities().await.unwrap();
        assert!(
            !capabilities
                .features
                .iter()
                .any(|feature| feature.starts_with("resources.cgroups-v2.")),
            "NoExecutionSetup must advertise no resource capabilities; got {:?}",
            capabilities.features
        );

        let mut spec = execution_spec(vec!["/bin/true".into()]);
        spec.command.resources.memory_bytes = Requirement::Required(64 * 1024 * 1024);
        let handle = execution_handle("required-memory-no-backend");
        assert!(matches!(
            client.execute(&spec, &handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        let snapshot = client.observe(&handle.execution_id).await;
        assert!(
            matches!(
                snapshot,
                Err(eggwork_client::ClientError::Api { status: 404, .. })
            ),
            "rejected capability check must leave no accepted execution record; got {snapshot:?}"
        );

        let mut cpu_spec = execution_spec(vec!["/bin/true".into()]);
        cpu_spec.command.resources.cpu_millis = Requirement::Required(500);
        let cpu_handle = execution_handle("required-cpu-no-backend");
        assert!(matches!(
            client.execute(&cpu_spec, &cpu_handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));

        let mut pids_spec = execution_spec(vec!["/bin/true".into()]);
        pids_spec.command.resources.pids = Requirement::Required(8);
        let pids_handle = execution_handle("required-pids-no-backend");
        assert!(matches!(
            client.execute(&pids_spec, &pids_handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn required_resource_admission_passes_when_runtime_dimension_is_available() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let helper = match place_trusted_helper() {
            Some(helper) => helper,
            None => {
                eprintln!(
                    "skipping required_resource_admission_passes_when_runtime_dimension_is_available: helper binary not located"
                );
                return;
            }
        };
        let runner = Arc::new(LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(helper),
        ));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-resource-accept").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        let capabilities = client.capabilities().await.unwrap();
        let available: Vec<&str> = capabilities
            .features
            .iter()
            .filter_map(|feature| match feature.as_str() {
                "resources.cgroups-v2.memory" => Some("memory_bytes"),
                "resources.cgroups-v2.cpu" => Some("cpu_millis"),
                "resources.cgroups-v2.pids" => Some("pids"),
                _ => None,
            })
            .collect();
        assert!(
            !available.is_empty(),
            "trusted helper host must advertise at least one resource capability; got {:?}",
            capabilities.features
        );

        let mut spec = execution_spec(vec!["/bin/true".into()]);
        if available.contains(&"memory_bytes") {
            spec.command.resources.memory_bytes = Requirement::Required(64 * 1024 * 1024);
        }
        if available.contains(&"cpu_millis") {
            spec.command.resources.cpu_millis = Requirement::Required(500);
        }
        if available.contains(&"pids") {
            spec.command.resources.pids = Requirement::Required(16);
        }
        let handle = execution_handle("required-resource-available");
        let stream = client.execute(&spec, &handle).await.unwrap();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.into_events().try_collect::<Vec<_>>(),
        )
        .await
        .expect("execution with available resource backend must terminate");
        let snapshot = client
            .observe_generation(&handle.execution_id, handle.generation.get())
            .await
            .unwrap();
        assert_eq!(snapshot.state, ExecutionState::Succeeded);
        let result = snapshot.result.unwrap();
        if let Some(resources) = &result.resources {
            let resources = resources.clone();
            for (dim, capability_name, dim_available) in [
                (
                    resources.memory_bytes,
                    "resources.cgroups-v2.memory",
                    available.contains(&"memory_bytes"),
                ),
                (
                    resources.cpu_millis,
                    "resources.cgroups-v2.cpu",
                    available.contains(&"cpu_millis"),
                ),
                (
                    resources.pids,
                    "resources.cgroups-v2.pids",
                    available.contains(&"pids"),
                ),
            ] {
                if dim_available {
                    assert!(
                        matches!(
                            dim,
                            eggwork_core::ResourceDimensionResult::Applied { .. }
                                | eggwork_core::ResourceDimensionResult::LimitExceeded { .. }
                        ),
                        "expected {capability_name} to be enforced when advertised; got {dim:?}"
                    );
                }
            }
        }
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn disabled_or_allowlisted_network_requests_are_rejected_with_capability_mismatch() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-a").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-network-reject").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024,
                max_active_executions: 1,
                lease_ttl: std::time::Duration::from_secs(5),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();

        let mut disabled_spec = execution_spec(vec!["/bin/true".into()]);
        disabled_spec.command.network = NetworkRequirement::Disabled;
        let disabled_handle = execution_handle("network-disabled");
        assert!(matches!(
            client.execute(&disabled_spec, &disabled_handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        let disabled_snapshot = client.observe(&disabled_handle.execution_id).await;
        assert!(
            matches!(
                disabled_snapshot,
                Err(eggwork_client::ClientError::Api { status: 404, .. })
            ),
            "disabled network must not produce an execution record; got {disabled_snapshot:?}"
        );

        let mut allowlisted_spec = execution_spec(vec!["/bin/true".into()]);
        allowlisted_spec.command.network =
            NetworkRequirement::AllowListed(vec!["example.com".into()]);
        let allowlisted_handle = execution_handle("network-allowlist");
        assert!(matches!(
            client.execute(&allowlisted_spec, &allowlisted_handle).await,
            Err(eggwork_client::ClientError::Api { status: 409, .. })
        ));
        let allowlisted_snapshot = client.observe(&allowlisted_handle.execution_id).await;
        assert!(
            matches!(
                allowlisted_snapshot,
                Err(eggwork_client::ClientError::Api { status: 404, .. })
            ),
            "allow-listed network must not produce an execution record; got {allowlisted_snapshot:?}"
        );

        // Unrestricted network is the only accepted mode and must execute.
        let unrestricted_handle = execution_handle("network-unrestricted");
        let stream = client
            .execute(
                &execution_spec(vec!["/bin/true".into()]),
                &unrestricted_handle,
            )
            .await
            .unwrap();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.into_events().try_collect::<Vec<_>>(),
        )
        .await
        .expect("unrestricted network execution must terminate");
        let snapshot = client
            .observe_generation(
                &unrestricted_handle.execution_id,
                unrestricted_handle.generation.get(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.state, ExecutionState::Succeeded);

        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn controller_compatibility_fixture_requires_landlock_then_executes_required_isolation() {
        init_tls();
        let (root, server_identity, client_identity, _) = tls_material();
        let temp = TempDir::new().unwrap();
        let work_root = temp.path().join("work");
        fs::create_dir_all(&work_root).unwrap();
        let principal = eggwork_core::PrincipalId::new("controller-codegg").unwrap();
        let resolver = Arc::new(FingerprintPrincipalResolver::new([(
            FingerprintPrincipalResolver::fingerprint(client_identity.cert.as_ref()),
            principal,
        )]));
        let helper = match place_trusted_helper() {
            Some(helper) => helper,
            None => {
                eprintln!("skipping controller_compatibility_fixture: helper binary not located");
                return;
            }
        };
        let runner = Arc::new(LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(helper),
        ));
        let server = NodeServer::start(
            NodeConfig {
                node_id: NodeId::new("node-codegg-compat").unwrap(),
                bind: "127.0.0.1:0".parse().unwrap(),
                execution_root: work_root,
                database_path: temp.path().join("node.sqlite"),
                blob_root: temp.path().join("blob-store"),
                blob_quota_bytes: 1024 * 1024 * 1024,
                workspace_root: temp.path().join("workspace-store"),
                workspace_quota_bytes: 1024 * 1024 * 1024,
                max_active_executions: 2,
                lease_ttl: std::time::Duration::from_secs(10),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            runner,
            resolver,
            Arc::new(|_: &NodePrincipal, _| true),
        )
        .await
        .unwrap();
        let client_temp = TempDir::new().unwrap();
        let client = NodeClient::new(
            format!("https://localhost:{}", server.local_addr().port()),
            write_client_config(&client_temp, root.as_ref(), &client_identity),
        )
        .unwrap();
        // 1. Pre-flight against /v1/capabilities.
        let capabilities = client.capabilities().await.unwrap();
        assert!(
            capabilities
                .features
                .iter()
                .any(|feature| feature == "exec.argv.v1"),
            "controller fixture requires exec.argv.v1; got {:?}",
            capabilities.features
        );
        let landlock_advertised = capabilities
            .features
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1");
        if !landlock_advertised {
            eprintln!(
                "skipping controller_compatibility_fixture: kernel does not advertise Landlock"
            );
            server.shutdown().await;
            server.wait().await.unwrap();
            return;
        }
        // 2. /v1/status must agree on the same feature set.
        let status = client.status().await.unwrap();
        assert_eq!(
            status.capabilities.features, capabilities.features,
            "controller fixture requires /v1/capabilities and /v1/status to agree"
        );
        // 3. Stage workspace content with one read-only file.
        let body = b"controller-fixture\n";
        let body_digest = eggwork_core::BlobDigest::from_bytes(body);
        let chunks = vec![Ok::<_, eggfetch_core::Error>(Bytes::from_static(body))];
        client
            .upload_blob(
                &body_digest,
                body.len() as u64,
                Box::pin(stream::iter(chunks)),
            )
            .await
            .unwrap();
        let workspace_id = eggwork_core::WorkspaceId::new("controller-ws").unwrap();
        let workspace_handle = execution_handle("controller-ws-exec");
        let manifest = eggwork_core::WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                eggwork_core::WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                eggwork_core::WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/payload.txt").unwrap(),
                    digest: body_digest,
                    size_bytes: body.len() as u64,
                    executable: false,
                },
            ],
        };
        client
            .create_workspace(&workspace_id, &workspace_handle, &manifest)
            .await
            .unwrap();
        // 4. Required-isolation execution must complete and report applied sandbox evidence.
        let mut spec = execution_spec(vec![
            "/bin/sh".into(),
            "-c".into(),
            "cat payload.txt; sleep 4".into(),
        ]);
        spec.command.isolation = IsolationRequirement::Required;
        spec.command.cwd = Some(eggwork_core::RelativePath::new("src").unwrap());
        spec.command.timeout_millis = 30_000;
        let stream = client
            .execute_in_workspace(&spec, &workspace_handle, &workspace_id)
            .await
            .unwrap();
        // Capture stdout/stderr by replaying the journal through `client.events`
        // once the execution reaches Running; this proves the sandboxed read
        // succeeded and lets us drop the live stream so renew/cancel operate
        // against a still-running execution.
        let handle = workspace_handle.clone();
        let event_client = client.clone();
        let events_task = tokio::spawn(async move {
            let stream = event_client.events(&handle, 0).await?;
            stream
                .map(|chunk| {
                    chunk
                        .map_err(eggwork_client::ClientError::Transport)
                        .and_then(|bytes| {
                            serde_json::from_slice::<ExecutionEvent>(&bytes)
                                .map_err(|_| eggwork_client::ClientError::InvalidResponse)
                        })
                })
                .try_collect::<Vec<_>>()
                .await
        });
        // Wait until Running before exercising renew/cancel.
        let mut sandbox_read_observed = false;
        for _ in 0..200 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let snap = client
                .observe_generation(
                    &workspace_handle.execution_id,
                    workspace_handle.generation.get(),
                )
                .await
                .unwrap();
            if matches!(snap.state, ExecutionState::Running) {
                sandbox_read_observed = true;
                break;
            }
        }
        assert!(
            sandbox_read_observed,
            "controller-fixture execution must reach Running state"
        );
        drop(stream);
        // The execution is still in Running state because the command sleeps;
        // renew + cancel must succeed against a fenced handle.
        client.renew(&workspace_handle, "renewal-id").await.unwrap();
        client.cancel(&workspace_handle).await.unwrap();
        let cancelled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let snap = client
                    .observe_generation(
                        &workspace_handle.execution_id,
                        workspace_handle.generation.get(),
                    )
                    .await
                    .unwrap();
                if matches!(
                    snap.state,
                    ExecutionState::Cancelled
                        | ExecutionState::Succeeded
                        | ExecutionState::Failed
                        | ExecutionState::TimedOut
                        | ExecutionState::Interrupted
                ) {
                    break snap;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("controller-fixture cancellation must reach a terminal state");
        let result = cancelled.result.unwrap();
        assert_eq!(cancelled.state, ExecutionState::Cancelled, "{result:?}");
        assert!(matches!(
            result.sandbox,
            Some(eggwork_core::SandboxResult::Applied { .. })
        ));
        // Replay the journal to confirm the sandboxed read produced the
        // expected stdout (workspace allow path) and cancel terminated the
        // sleep.
        let events = tokio::time::timeout(std::time::Duration::from_secs(5), events_task)
            .await
            .expect("events task must terminate")
            .unwrap()
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            ExecutionEventKind::Stdout(bytes) if bytes == body
        )));
        server.shutdown().await;
        server.wait().await.unwrap();
    }

    #[test]
    fn static_capability_features_are_a_single_canonical_list() {
        let features = static_capability_features();
        assert!(features.contains(&"exec.argv.v1"));
        assert!(features.contains(&"network.unrestricted.v1"));
        let mut sorted = features.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            features.len(),
            "static capability list must not contain duplicates"
        );
    }

    #[test]
    fn server_capability_features_do_not_duplicate_static_dynamic_overlap() {
        let dynamic = vec!["isolation.landlock.workspace-rw.v1".to_owned()];
        let features = server_capability_features(dynamic.clone());
        assert_eq!(
            features
                .iter()
                .filter(|feature| *feature == "exec.argv.v1")
                .count(),
            1,
            "static features must not be duplicated"
        );
        assert_eq!(
            features
                .iter()
                .filter(|feature| feature.as_str() == "isolation.landlock.workspace-rw.v1")
                .count(),
            1,
            "dynamic features must be preserved"
        );
    }

    #[test]
    fn static_capability_features_do_not_advertise_unsupported_network_modes() {
        let features = static_capability_features();
        assert!(
            !features.contains(&"network.disabled.v1"),
            "network.disabled.v1 is unsupported and must not be advertised"
        );
        assert!(
            !features
                .iter()
                .any(|feature| feature.starts_with("network.allow")),
            "network allow-list features are unsupported and must not be advertised"
        );
    }

    #[test]
    fn admission_helpers_keep_required_capability_gating_active() {
        // This test does not exercise live sockets; it pins the admission
        // contract so a regression that re-introduces a blanket
        // `IsolationRequirement::None && NetworkRequirement::Unrestricted`
        // gate is caught here even if the integration tests are skipped on
        // hosts without Landlock.
        let features = vec!["isolation.landlock.workspace-rw.v1".to_owned()];
        struct Probe {
            features: Vec<String>,
        }
        impl Probe {
            fn isolation_supported(&self, requirement: &IsolationRequirement) -> bool {
                match requirement {
                    IsolationRequirement::None | IsolationRequirement::BestEffort => true,
                    IsolationRequirement::Required => self
                        .features
                        .iter()
                        .any(|f| f == "isolation.landlock.workspace-rw.v1"),
                }
            }
            fn network_supported(&self, network: &NetworkRequirement) -> bool {
                matches!(network, NetworkRequirement::Unrestricted)
            }
        }
        let probe = Probe { features };
        assert!(probe.isolation_supported(&IsolationRequirement::None));
        assert!(probe.isolation_supported(&IsolationRequirement::BestEffort));
        assert!(probe.isolation_supported(&IsolationRequirement::Required));
        let empty = Probe { features: vec![] };
        assert!(empty.isolation_supported(&IsolationRequirement::None));
        assert!(empty.isolation_supported(&IsolationRequirement::BestEffort));
        assert!(!empty.isolation_supported(&IsolationRequirement::Required));
        assert!(probe.network_supported(&NetworkRequirement::Unrestricted));
        assert!(!probe.network_supported(&NetworkRequirement::Disabled));
        assert!(
            !probe.network_supported(&NetworkRequirement::AllowListed(vec!["example.com".into()]))
        );
    }
}
