#![forbid(unsafe_code)]

//! Authenticated fixed-target Eggwork node service, built on EggServe.

mod blob;
mod store;

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
    ApiError, ExecutionEvent, ExecutionEventKind, ExecutionFailure, ExecutionGeneration,
    ExecutionHandle, ExecutionId, ExecutionResult, ExecutionSnapshot, ExecutionSpec,
    ExecutionState, LeaseId, NodeCapabilities, NodeId, NodeStatus, ProtocolVersion,
    ProtocolVersionRange,
};
use eggwork_runner::{ExecutionProvenance, LocalProcessRunner, RunnerError, RunnerRequest};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl<F> Authorizer for F
where
    F: Fn(&NodePrincipal, Operation) -> bool + Send + Sync + 'static,
{
    fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool {
        self(principal, operation)
    }
}

#[derive(Debug, Error)]
pub enum NodeStartError {
    #[error("remote execution requires a valid server identity and required client authentication")]
    TlsPolicy,
    #[error("invalid EggServe configuration: {0}")]
    EggServe(String),
    #[error("maximum active execution count must be positive")]
    InvalidLimit,
    #[error("execution lease duration must be positive")]
    InvalidLease,
}

#[derive(Clone)]
pub struct NodeConfig {
    pub node_id: NodeId,
    pub bind: SocketAddr,
    pub execution_root: PathBuf,
    pub database_path: PathBuf,
    pub blob_root: PathBuf,
    pub blob_quota_bytes: u64,
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
    runner: Arc<LocalProcessRunner>,
    store: store::ExecutionStore,
    blobs: blob::BlobStore,
    lease_ttl: Duration,
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

struct LeaseState {
    expires_at: tokio::time::Instant,
    notify: Arc<tokio::sync::Notify>,
    expired: Arc<AtomicBool>,
}

pub struct NodeServer {
    handle: ServerHandle,
    state: NodeState,
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
            ("POST", "/v1/blobs/missing") => RequestBodyPolicy::Buffer {
                max_bytes: MAX_BLOB_FIND_REQUEST_BYTES as u64,
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
        let store = store::ExecutionStore::open(&config.database_path)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let blobs = blob::BlobStore::open(&config.blob_root, config.blob_quota_bytes)
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        store
            .recover()
            .await
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let draining = Arc::new(AtomicBool::new(false));
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
            runner,
            store,
            blobs,
            lease_ttl: config.lease_ttl,
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
        Ok(Self { handle, state })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    pub fn set_draining(&self, draining: bool) {
        self.state.draining.store(draining, Ordering::Release);
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

fn operation_for(method: &str, path: &str) -> Option<(Operation, Route)> {
    match (method, path) {
        ("GET", "/v1/capabilities") => Some((Operation::Capabilities, Route::Capabilities)),
        ("GET", "/v1/status") => Some((Operation::Status, Route::Status)),
        ("POST", "/v1/executions") => Some((Operation::Execute, Route::Execute)),
        ("POST", "/v1/blobs/missing") => Some((Operation::BlobRead, Route::BlobMissing)),
        _ => {
            let parts: Vec<_> = path.split('/').collect();
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
    BlobUpload(eggwork_core::BlobDigest),
    BlobDownload(eggwork_core::BlobDigest),
    BlobInvalidDigest,
}

async fn dispatch(
    state: NodeState,
    request: Request,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let method = request.head().method().as_str().to_owned();
    let path = request.head().target().path().to_owned();
    let query = request.head().target().query().map(str::to_owned);
    let Some((operation, route)) = operation_for(&method, &path) else {
        return Ok(error_response(404, "not_found", "operation not found"));
    };
    let Some(principal) = authenticated_principal(&state, &request) else {
        return Ok(error_response(
            401,
            "unauthenticated",
            "verified client identity required",
        ));
    };
    if !state.authorizer.authorize(&principal, operation) {
        return Ok(error_response(
            403,
            "forbidden",
            "operation is not authorized",
        ));
    }
    match route {
        Route::Capabilities => Ok(json_response(
            200,
            &NodeCapabilities {
                protocol: ProtocolVersionRange {
                    min: ProtocolVersion { major: 1, minor: 0 },
                    max: ProtocolVersion { major: 1, minor: 0 },
                },
                features: vec![
                    "exec.argv.v1".into(),
                    "events.live.v1".into(),
                    "auth.mtls.v1".into(),
                    "blob.sha256.v1".into(),
                    "blob.stream.v1".into(),
                ],
                max_active_executions: state.max_active,
            },
        )),
        Route::Status => Ok(json_response(200, &node_status(&state))),
        Route::Execute => execute(state, request, principal).await,
        Route::Observe(id) => observe(state, id, query.as_deref()).await,
        Route::Cancel(id) => control(state, id, request, principal, false).await,
        Route::Renew(id) => control(state, id, request, principal, true).await,
        Route::Events(id) => events_route(state, id, query.as_deref(), principal).await,
        Route::BlobMissing => blob_missing(state, request).await,
        Route::BlobUpload(digest) => blob_upload(state, request, digest).await,
        Route::BlobDownload(digest) => blob_download(state, digest).await,
        Route::BlobInvalidDigest => Ok(error_response(
            400,
            "invalid_digest",
            "blob digest is invalid",
        )),
    }
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
        draining: state.draining.load(Ordering::Acquire),
        active_executions: state.max_active - state.permits.available_permits() as u32,
        capabilities: NodeCapabilities {
            protocol: ProtocolVersionRange {
                min: ProtocolVersion { major: 1, minor: 0 },
                max: ProtocolVersion { major: 1, minor: 0 },
            },
            features: vec![
                "exec.argv.v1".into(),
                "events.live.v1".into(),
                "auth.mtls.v1".into(),
                "blob.sha256.v1".into(),
                "blob.stream.v1".into(),
            ],
            max_active_executions: state.max_active,
        },
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
struct FindMissingRequest {
    digests: Vec<eggwork_core::BlobDigest>,
}

#[derive(Serialize)]
struct FindMissingResponse {
    missing: Vec<eggwork_core::BlobDigest>,
}

async fn blob_missing(
    state: NodeState,
    request: Request,
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
        Ok(()) => Ok(Response::builder()
            .status(StatusCode::new(201).expect("valid HTTP status"))
            .body(ResponseBody::Empty)
            .unwrap_or_else(|_| error_response(500, "internal", "internal error"))),
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
    let stream = stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0u8; 64 * 1024];
        match tokio::io::AsyncReadExt::read(&mut file, &mut chunk).await {
            Ok(0) => Ok(None),
            Ok(size) => {
                chunk.truncate(size);
                Ok(Some((Bytes::from(chunk), file)))
            }
            Err(_) => Err(ResponseStreamError::new("blob read failed")),
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .and_then(|builder| builder.header("eggwork-blob-digest", digest.as_str()))
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
    if wire.spec.schema_version != API_SCHEMA_VERSION {
        return Ok(error_response(
            426,
            "protocol_version",
            "unsupported execution schema version",
        ));
    }
    if let Err(error) = wire.spec.validate() {
        let _ = error;
        return Ok(error_response(
            400,
            "invalid_request",
            "execution specification is invalid",
        ));
    }
    if state.draining.load(Ordering::Acquire) {
        return Ok(error_response(
            503,
            "draining",
            "node is not accepting executions",
        ));
    }
    if !matches!(
        wire.spec.command.isolation,
        eggwork_core::IsolationRequirement::None
    ) || !matches!(
        wire.spec.command.network,
        eggwork_core::NetworkRequirement::Unrestricted
    ) {
        return Ok(error_response(
            409,
            "capability_mismatch",
            "requested execution capability is unavailable",
        ));
    }
    let id = wire.handle.execution_id.clone();
    let generation = wire.handle.generation;
    let runner_request = match RunnerRequest::from_spec(
        &wire.spec,
        state.execution_root.clone(),
        ExecutionProvenance {
            execution_id: Some(id.clone()),
            generation: Some(generation),
        },
    ) {
        Ok(request) => request,
        Err(_) => {
            return Ok(error_response(
                400,
                "invalid_request",
                "execution specification is invalid",
            ));
        }
    };
    let digest = match eggwork_core::request_digest(&wire.spec) {
        Ok(digest) => digest.as_str().to_owned(),
        Err(_) => {
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
        if existing.canonical_version != eggwork_core::CANONICAL_REQUEST_VERSION {
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
    if state.draining.load(Ordering::Acquire) {
        return Ok(error_response(
            503,
            "draining",
            "node is not accepting executions",
        ));
    }
    let permit = match state.permits.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Ok(error_response(503, "busy", "node execution limit reached")),
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
        return Ok(error_response(
            503,
            "storage_exhausted",
            "recent execution capacity reached",
        ));
    }
    let snapshot = ExecutionSnapshot {
        schema_version: API_SCHEMA_VERSION,
        execution_id: id.clone(),
        generation,
        state: ExecutionState::Accepted,
        result: None,
    };
    match state
        .store
        .reserve(
            snapshot.clone(),
            eggwork_core::CANONICAL_REQUEST_VERSION,
            digest,
            principal_id,
            lease_hash.clone(),
            lease_expires_unix_ms,
        )
        .await
        .map_err(|_| eggserve_core::server::ServiceError::internal("execution store unavailable"))?
    {
        store::ReserveResult::Created => {}
        store::ReserveResult::Existing(snapshot) => {
            drop(permit);
            let record = recovered_record(snapshot, state.store.clone(), lease_hash);
            drop(executions);
            return event_response(state, id, generation, 0, record).await;
        }
        store::ReserveResult::Conflict => {
            return Ok(error_response(
                409,
                "execution_identity_conflict",
                "execution identity conflicts with an existing request",
            ));
        }
        store::ReserveResult::StaleGeneration => {
            return Ok(error_response(
                409,
                "generation_mismatch",
                "execution generation is stale or out of sequence",
            ));
        }
        store::ReserveResult::StorageFull => {
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
    tokio::spawn(async move {
        run_execution(service_state, execution_record, runner_request, permit).await;
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
        .load_snapshot(id, generation)
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
    if let Some(result) = result {
        let mut execution_result = result.execution_result();
        if record.lease.lock().await.expired.load(Ordering::Acquire) {
            execution_result.state = ExecutionState::Interrupted;
            execution_result.failure = Some(ExecutionFailure::LeaseExpired);
            execution_result.exit_code = None;
        }
        let state_value = execution_result.state.clone();
        publish_terminal(
            &record,
            state_value.clone(),
            execution_result,
            ExecutionEventKind::State(state_value),
        )
        .await;
    } else {
        let expired = record.lease.lock().await.expired.load(Ordering::Acquire);
        let failure = match runner_error {
            Some(RunnerError::Spawn(_)) => ExecutionFailure::Spawn,
            Some(RunnerError::CancelledBeforeSpawn) => ExecutionFailure::Interrupted,
            _ if expired => ExecutionFailure::LeaseExpired,
            _ => ExecutionFailure::Internal,
        };
        let terminal = if expired {
            ExecutionState::Interrupted
        } else if matches!(failure, ExecutionFailure::Interrupted) {
            ExecutionState::Cancelled
        } else {
            ExecutionState::Failed
        };
        let result = failed_result(terminal.clone(), failure);
        publish_terminal(
            &record,
            terminal.clone(),
            result,
            ExecutionEventKind::State(terminal),
        )
        .await;
    }
    record.finished.store(true, Ordering::Release);
}

fn failed_result(state: ExecutionState, failure: ExecutionFailure) -> ExecutionResult {
    ExecutionResult {
        state,
        exit_code: None,
        failure: Some(failure),
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_omitted: 0,
        stderr_omitted: 0,
        cleanup_warning: None,
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
struct ExecuteRequest {
    schema_version: u16,
    handle: ExecutionHandle,
    spec: ExecutionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        IsolationRequirement, NetworkRequirement, OutputPolicy, ResourceRequirements, StdinPolicy,
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
                    memory_bytes: None,
                    cpu_millis: None,
                    pids: None,
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
        assert!(operation_for("POST", "/v1/execute-anywhere").is_none());
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
                max_active_executions: 2,
                lease_ttl: std::time::Duration::from_secs(3),
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, operation| {
                !matches!(
                    operation,
                    Operation::Execute | Operation::BlobRead | Operation::BlobWrite
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
                .upload_blob(&denied_digest, 0, Box::pin(stream::empty()),)
                .await,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
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
        assert!(matches!(
            client
                .execute(
                    &execution_spec(vec!["/bin/true".into()]),
                    &execution_handle("busy-one")
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        server.set_draining(true);
        assert!(matches!(
            client
                .execute(
                    &execution_spec(vec!["/bin/true".into()]),
                    &execution_handle("drain-one")
                )
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        server.set_draining(false);
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
}
