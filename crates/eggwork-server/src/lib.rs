#![forbid(unsafe_code)]

//! Authenticated fixed-target Eggwork node service, built on EggServe.

use bytes::Bytes;
use eggserve_core::{
    primitives::{
        canonical::{Response, ResponseBody, ResponseStream, ResponseStreamError, StatusCode},
        request::Request,
        request_body_policy::RequestBodyPolicy,
    },
    server::{RuntimeConfig, Server, ServerHandle, service_fn_with_policy},
    tls::{ClientAuthMode, TlsServerConfig},
};
use eggwork_core::{
    ApiError, EventMetadata, EventSequence, ExecutionEvent, ExecutionEventKind, ExecutionFailure,
    ExecutionGeneration, ExecutionId, ExecutionResult, ExecutionSnapshot, ExecutionSpec,
    ExecutionState, NodeCapabilities, NodeId, NodeStatus, ProtocolVersion, ProtocolVersionRange,
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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const API_SCHEMA_VERSION: u16 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const EVENT_CAPACITY: usize = 32;
const RUNNER_CHANNEL_CAPACITY: usize = 32;
const EXECUTION_GENERATION: u64 = 1;
const MAX_RECENT_EXECUTIONS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Capabilities,
    Status,
    Execute,
    Observe,
    Cancel,
    Events,
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
}

#[derive(Clone)]
pub struct NodeConfig {
    pub node_id: NodeId,
    pub bind: SocketAddr,
    pub execution_root: PathBuf,
    pub max_active_executions: u32,
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
    resolver: Arc<dyn PeerPrincipalResolver>,
    authorizer: Arc<dyn Authorizer>,
    executions: Arc<Mutex<HashMap<ExecutionId, Arc<ExecutionRecord>>>>,
}

struct ExecutionRecord {
    snapshot: RwLock<ExecutionSnapshot>,
    cancellation: CancellationToken,
    events: broadcast::Sender<ExecutionEvent>,
    next_sequence: AtomicU64,
}

pub struct NodeServer {
    handle: ServerHandle,
    state: NodeState,
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
        let state = NodeState {
            node_id: config.node_id,
            execution_root: config.execution_root,
            max_active: config.max_active_executions,
            permits: Arc::new(Semaphore::new(config.max_active_executions as usize)),
            draining: Arc::new(AtomicBool::new(false)),
            runner,
            resolver,
            authorizer,
            executions: Arc::new(Mutex::new(HashMap::new())),
        };
        let runtime = RuntimeConfig::builder()
            .bind(config.bind)
            .max_request_body_bytes(MAX_REQUEST_BYTES as u64)
            .tls_config(config.tls.into_server_config())
            .tls_expose_peer_chain(true)
            .build()
            .map_err(|e| NodeStartError::EggServe(e.to_string()))?;
        let service_state = state.clone();
        let service = service_fn_with_policy(
            move |request| {
                let state = service_state.clone();
                async move { dispatch(state, request).await }
            },
            RequestBodyPolicy::Buffer {
                max_bytes: MAX_REQUEST_BYTES as u64,
            },
        );
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
            if !is_terminal(&record.snapshot.read().await.state) {
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
                if !is_terminal(&record.snapshot.read().await.state) {
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
                    ("GET", "events") => Some((Operation::Events, Route::Events(id))),
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
    Events(ExecutionId),
}

async fn dispatch(
    state: NodeState,
    request: Request,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let method = request.head().method().as_str().to_owned();
    let path = request.head().target().path().to_owned();
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
                ],
                max_active_executions: state.max_active,
            },
        )),
        Route::Status => Ok(json_response(200, &node_status(&state))),
        Route::Execute => execute(state, request).await,
        Route::Observe(id) => {
            let Some(record) = state.executions.lock().await.get(&id).cloned() else {
                return Ok(error_response(
                    404,
                    "execution_not_found",
                    "execution not found",
                ));
            };
            let snapshot = record.snapshot.read().await.clone();
            Ok(json_response(200, &snapshot))
        }
        Route::Cancel(id) => {
            let Some(record) = state.executions.lock().await.get(&id).cloned() else {
                return Ok(error_response(
                    404,
                    "execution_not_found",
                    "execution not found",
                ));
            };
            let snapshot = record.snapshot.read().await.clone();
            if !is_terminal(&snapshot.state) {
                record.cancellation.cancel();
            }
            Ok(json_response(202, &snapshot))
        }
        Route::Events(id) => {
            let Some(record) = state.executions.lock().await.get(&id).cloned() else {
                return Ok(error_response(
                    404,
                    "execution_not_found",
                    "execution not found",
                ));
            };
            Ok(event_stream_response(id, record.events.subscribe()))
        }
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
            ],
            max_active_executions: state.max_active,
        },
    }
}

async fn execute(
    state: NodeState,
    request: Request,
) -> Result<Response, eggserve_core::server::ServiceError> {
    let (_head, body, _context) = request.into_parts_with_context();
    let body = match body.read_all().await {
        Ok(body) if body.len() <= MAX_REQUEST_BYTES => body,
        _ => {
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
    let id = ExecutionId::new(Uuid::new_v4().to_string()).expect("UUID is a valid execution id");
    let generation = ExecutionGeneration::new(EXECUTION_GENERATION).expect("generation is nonzero");
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
    let permit = match state.permits.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Ok(error_response(503, "busy", "node execution limit reached")),
    };
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    let record = Arc::new(ExecutionRecord {
        snapshot: RwLock::new(ExecutionSnapshot {
            schema_version: API_SCHEMA_VERSION,
            execution_id: id.clone(),
            generation,
            state: ExecutionState::Accepted,
            result: None,
        }),
        cancellation: CancellationToken::new(),
        events,
        next_sequence: AtomicU64::new(1),
    });
    {
        let mut executions = state.executions.lock().await;
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
        executions.insert(id.clone(), record.clone());
    }
    let receiver = record.events.subscribe();
    publish(
        &record,
        ExecutionState::Accepted,
        None,
        ExecutionEventKind::State(ExecutionState::Accepted),
    )
    .await;
    let service_state = state.clone();
    let execution_record = record.clone();
    tokio::spawn(async move {
        run_execution(service_state, execution_record, runner_request, permit).await;
    });
    Ok(event_stream_response(id, receiver))
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
        let execution_result = result.execution_result();
        let state_value = execution_result.state.clone();
        publish_terminal(
            &record,
            state_value.clone(),
            execution_result,
            ExecutionEventKind::State(state_value),
        )
        .await;
    } else {
        let failure = match runner_error {
            Some(RunnerError::Spawn(_)) => ExecutionFailure::Spawn,
            Some(RunnerError::CancelledBeforeSpawn) => ExecutionFailure::Interrupted,
            _ => ExecutionFailure::Internal,
        };
        let terminal = if matches!(failure, ExecutionFailure::Interrupted) {
            ExecutionState::Cancelled
        } else {
            ExecutionState::Failed
        };
        let result = failed_result(failure);
        publish_terminal(
            &record,
            terminal.clone(),
            result,
            ExecutionEventKind::State(terminal),
        )
        .await;
    }
}

fn failed_result(failure: ExecutionFailure) -> ExecutionResult {
    ExecutionResult {
        state: ExecutionState::Failed,
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
    current.state = state;
    if let Some(result) = result {
        current.result = Some(result);
    }
    drop(current);
    emit(record, kind);
}

async fn publish_terminal(
    record: &ExecutionRecord,
    state: ExecutionState,
    result: ExecutionResult,
    kind: ExecutionEventKind,
) {
    let mut current = record.snapshot.write().await;
    current.state = state;
    current.result = Some(result);
    drop(current);
    emit(record, kind);
}

fn emit(record: &ExecutionRecord, kind: ExecutionEventKind) {
    let sequence = EventSequence::new(record.next_sequence.fetch_add(1, Ordering::Relaxed));
    let event = ExecutionEvent {
        sequence,
        kind,
        metadata: EventMetadata { fields: vec![] },
    };
    let _ = record.events.send(event);
}

fn event_stream_response(
    id: ExecutionId,
    receiver: broadcast::Receiver<ExecutionEvent>,
) -> Response {
    let stream = stream::unfold((receiver, false), |(mut receiver, ended)| async move {
        if ended {
            return None;
        }
        match receiver.recv().await {
            Ok(event) => {
                let terminal =
                    matches!(&event.kind, ExecutionEventKind::State(state) if is_terminal(state));
                let bytes = serde_json::to_vec(&event).unwrap_or_default();
                let mut line = bytes;
                line.push(b'\n');
                Some((Ok(Bytes::from(line)), (receiver, terminal)))
            }
            Err(broadcast::error::RecvError::Lagged(_)) => Some((
                Err(ResponseStreamError::new(
                    "event consumer exceeded bounded live history",
                )),
                (receiver, true),
            )),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    let stream = ResponseStream::new(stream);
    Response::builder()
        .status(StatusCode::new(202).expect("valid HTTP status"))
        .header("content-type", "application/x-ndjson")
        .and_then(|builder| builder.header("eggwork-execution-id", id.to_string()))
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
    spec: ExecutionSpec,
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggfetch_core::TlsConfig;
    use eggwork_client::NodeClient;
    use eggwork_core::{
        CommandSpec, EnvironmentEntry, IsolationRequirement, NetworkRequirement, OutputPolicy,
        ResourceRequirements, StdinPolicy,
    };
    use futures_util::StreamExt;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::{fs, sync::Arc};
    use tempfile::TempDir;

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
                max_active_executions: 2,
                tls: tls_server_config(root.clone(), &server_identity),
            },
            Arc::new(LocalProcessRunner::default()),
            resolver,
            Arc::new(|_: &NodePrincipal, operation| operation != Operation::Execute),
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
        let denied = unauthorized
            .execute(&execution_spec(vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("touch {}", marker.display()),
            ]))
            .await;
        assert!(matches!(
            denied,
            Err(eggwork_client::ClientError::Api { status: 403, .. })
        ));
        assert!(
            !marker.exists(),
            "authorization denial must precede admission and spawn"
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
                max_active_executions: 1,
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
        let execution = client
            .execute(&execution_spec(vec![
                "/bin/echo".into(),
                "hello eggwork".into(),
            ]))
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

        let running = client
            .execute(&execution_spec(vec![
                "/bin/sh".into(),
                "-c".into(),
                "sleep 1".into(),
            ]))
            .await
            .unwrap();
        let running_id = running.execution_id.clone();
        assert!(matches!(
            client
                .execute(&execution_spec(vec!["/bin/true".into()]))
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        server.set_draining(true);
        assert!(matches!(
            client
                .execute(&execution_spec(vec!["/bin/true".into()]))
                .await,
            Err(eggwork_client::ClientError::Api { status: 503, .. })
        ));
        server.set_draining(false);
        drop(running); // A disconnected live stream does not cancel execution.
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
            .execute(&execution_spec(vec![
                "/bin/sh".into(),
                "-c".into(),
                "sleep 5".into(),
            ]))
            .await
            .unwrap();
        let cancellable_id = cancellable.execution_id.clone();
        drop(cancellable);
        client.cancel(&cancellable_id).await.unwrap();
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
            .execute(&execution_spec(vec![
                "/bin/sh".into(),
                "-c".into(),
                "sleep 5".into(),
            ]))
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
}
