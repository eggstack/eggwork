#![forbid(unsafe_code)]

//! Explicit, single-node Eggwork client using Eggfetch for all HTTP transport.

use bytes::BytesMut;
use eggfetch_core::{
    BoxBytesStream, Client as HttpClient, Error as HttpError, RequestBody, TlsConfig,
};
use eggwork_core::{
    API_SCHEMA_VERSION, ApiError, ArtifactId, ArtifactRecord, BlobDigest, EXEC_ARGV_CAPABILITY,
    ExecutionEvent, ExecutionHandle, ExecutionId, ExecutionSnapshot, ExecutionSpec,
    NodeCapabilities, NodeId, NodeStatus, NodeTelemetry, WorkspaceId, WorkspaceManifest,
    WorkspaceManifestPatch,
};
use futures_util::{StreamExt, stream};
use serde::{Serialize, de::DeserializeOwned};
#[cfg(feature = "eggress-route")]
use std::sync::Arc;
use std::{fmt, path::Path};
use thiserror::Error;

/// Ceiling on a buffered (non-streamed) response body.
///
/// Every non-2xx response goes through `api_error`, and every JSON reply
/// through `decode_json`, and both used to read the body with no bound at all.
/// This is generous relative to the protocol's real maxima — the largest single
/// buffered reply is an execution snapshot, which the journal caps at 256 events
/// and 512 KiB of encoded events — so it never truncates a legitimate response
/// while still refusing to accumulate unbounded node-controlled data.
const MAX_BUFFERED_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Ceiling on a node telemetry response body.
///
/// The telemetry response is a small, bounded document — a schema version, a
/// node id, a state, and at most seven measurement groups. The transport cap
/// (`MAX_BUFFERED_BODY_BYTES`) bounds what the client will *accumulate*; this
/// cap bounds what the client will *accept* for this route, so a node that
/// answers with a megabyte of measurement-shaped JSON is refused rather than
/// parsed.
const MAX_TELEMETRY_BODY_BYTES: usize = 64 * 1024;

/// Time allowed for a pool slot, the connect/TLS handshake, each request-body
/// frame, and each *gap* between response chunks.
///
/// These are the deadlines a long-lived stream needs. Without the read phase a
/// hung or black-holed node left `execute`, `events` or `renew` pending forever;
/// because the read deadline resets on every chunk, an event stream that is
/// simply quiet between executions is unaffected.
fn transport_timeouts() -> eggfetch_core::Timeout {
    eggfetch_core::Timeout {
        pool: Some(std::time::Duration::from_secs(30)),
        connect: Some(std::time::Duration::from_secs(30)),
        write: Some(std::time::Duration::from_secs(60)),
        read: Some(std::time::Duration::from_secs(60)),
        total: None,
    }
}

/// Wall-clock deadline for the unary request/response calls.
///
/// Applied per request rather than on the client, because the streaming calls —
/// `execute`, `events`, blob upload, blob and artifact download — are legitimately
/// long-lived and a total deadline would abort a healthy transfer. They are held
/// to `transport_timeouts` instead.
fn unary_timeout() -> eggfetch_core::Timeout {
    eggfetch_core::Timeout {
        total: Some(std::time::Duration::from_secs(120)),
        ..transport_timeouts()
    }
}

#[derive(Error)]
pub enum ClientError {
    #[error("node endpoint must be an https URL without query, fragment, or credentials")]
    InvalidEndpoint,
    #[error("Eggress route configuration is invalid")]
    InvalidRoute,
    #[error("node transport failed")]
    Transport(HttpError),
    #[error("node returned HTTP {status}: {code}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("node response was malformed")]
    InvalidResponse,
    #[error("node protocol is incompatible")]
    ProtocolIncompatible,
    #[error("node does not support a required protocol capability")]
    UnsupportedCapability,
    #[error("node does not implement the node telemetry contract")]
    TelemetryUnsupported,
}

impl From<HttpError> for ClientError {
    fn from(error: HttpError) -> Self {
        Self::Transport(error)
    }
}

#[cfg(feature = "eggress-route")]
impl ClientError {
    /// Recover Eggress' typed route failure without parsing display text.
    pub fn eggress_error(&self) -> Option<&eggress_outbound::OutboundConnectError> {
        let mut source: &(dyn std::error::Error + 'static) = match self {
            Self::Transport(error) => error,
            _ => return None,
        };
        loop {
            if let Some(route) = source.downcast_ref() {
                return Some(route);
            }
            source = source.source()?;
        }
    }
}

impl fmt::Debug for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => f.write_str("ClientError::InvalidEndpoint"),
            Self::InvalidRoute => f.write_str("ClientError::InvalidRoute"),
            Self::Transport(_) => f.write_str("ClientError::Transport([REDACTED])"),
            Self::Api { status, code, .. } => f
                .debug_struct("ClientError::Api")
                .field("status", status)
                .field("code", code)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::InvalidResponse => f.write_str("ClientError::InvalidResponse"),
            Self::ProtocolIncompatible => f.write_str("ClientError::ProtocolIncompatible"),
            Self::UnsupportedCapability => f.write_str("ClientError::UnsupportedCapability"),
            Self::TelemetryUnsupported => f.write_str("ClientError::TelemetryUnsupported"),
        }
    }
}

/// A connection to exactly one caller-selected Eggwork node.
#[derive(Clone)]
pub struct NodeClient {
    endpoint: String,
    http: HttpClient,
}

impl fmt::Debug for NodeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = safe_target_origin(&self.endpoint);
        f.debug_struct("NodeClient")
            .field("target_origin", &origin)
            .finish_non_exhaustive()
    }
}

fn safe_target_origin(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .map(|endpoint| endpoint.origin().ascii_serialization())
        .unwrap_or_else(|_| "[INVALID]".into())
}

impl NodeClient {
    /// Build a fixed-target client using an explicit CA and mTLS identity.
    pub fn from_pem_files(
        endpoint: impl AsRef<str>,
        ca_certificate: impl AsRef<Path>,
        client_certificate: impl AsRef<Path>,
        client_private_key: impl AsRef<Path>,
    ) -> Result<Self, ClientError> {
        let tls = TlsConfig::builder()
            .ca_certificate_path(ca_certificate)?
            .client_cert_path(client_certificate, client_private_key)?
            .build();
        Self::new(endpoint, tls)
    }

    /// Build a fixed-target client with a caller-configured Eggfetch TLS policy.
    pub fn new(endpoint: impl AsRef<str>, tls: TlsConfig) -> Result<Self, ClientError> {
        let endpoint = normalize_endpoint(endpoint.as_ref())?;
        let http = HttpClient::builder()
            .tls_config(tls)
            .timeout(transport_timeouts())
            .max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)
            .build();
        Ok(Self { endpoint, http })
    }

    /// Build a fixed-target client over an explicit Eggress connector.
    /// Eggfetch retains ownership of HTTP and destination TLS. A route error
    /// is propagated without retrying or constructing a direct client.
    #[cfg(feature = "eggress-route")]
    pub fn with_eggress(
        endpoint: impl AsRef<str>,
        tls: TlsConfig,
        connector: eggress_outbound::OutboundConnector,
    ) -> Result<Self, ClientError> {
        let endpoint = normalize_endpoint(endpoint.as_ref())?;
        let http = HttpClient::builder()
            .tls_config(tls)
            .timeout(transport_timeouts())
            .max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)
            .dialer(EggressDialer {
                connector: Arc::new(connector),
            })
            .build();
        Ok(Self { endpoint, http })
    }

    /// Construct an opt-in Eggress route from a pproxy-compatible expression.
    #[cfg(feature = "eggress-route")]
    pub fn with_eggress_route(
        endpoint: impl AsRef<str>,
        tls: TlsConfig,
        route: &str,
    ) -> Result<Self, ClientError> {
        let connector = eggress_outbound::OutboundConnector::from_pproxy_uri(route)
            .map_err(|_| ClientError::InvalidRoute)?;
        Self::with_eggress(endpoint, tls, connector)
    }

    pub async fn capabilities(&self) -> Result<NodeCapabilities, ClientError> {
        self.get_json("/v1/capabilities").await
    }

    pub async fn status(&self) -> Result<NodeStatus, ClientError> {
        self.get_json("/v1/status").await
    }

    /// Read the node's host-wide telemetry observation.
    ///
    /// The node serves this from its own cache. A remote read never contacts
    /// the telemetry provider, so a burst of readers cannot become a burst of
    /// provider requests, and a provider that is down does not turn a read into
    /// a timeout.
    ///
    /// The three states a caller most often needs to tell apart are
    /// distinguishable without parsing prose:
    ///
    /// * the node has no provider configured — the response's `state` is
    ///   `disabled`;
    /// * the node's provider is failing — the response's `state` is
    ///   `unavailable` and `last_error` carries the bounded category;
    /// * the node does not implement this contract at all — the call fails
    ///   with [`ClientError::TelemetryUnsupported`].
    ///
    /// A node that reports `unavailable` is not reporting zero utilization.
    /// The per-measurement groups keep the difference explicit: a measurement
    /// the host platform cannot produce is `unsupported`, one that was
    /// supported but not reported is `unknown`, and only a real measurement is
    /// a value.
    pub async fn telemetry(&self) -> Result<NodeTelemetry, ClientError> {
        let mut response = self
            .http
            .get(&self.url("/v1/node/telemetry"))?
            .timeout(unary_timeout())
            .send()
            .await?;
        let status = response.status().as_u16();
        if status == 404 {
            // The node does not have the route at all. That is a node older
            // than this contract, and it is not the same answer as a node
            // that has the route and refuses the caller.
            return Err(ClientError::TelemetryUnsupported);
        }
        if !response.status().is_success() {
            return Err(api_error(status, &mut response).await);
        }
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_TELEMETRY_BODY_BYTES {
            return Err(ClientError::InvalidResponse);
        }
        let telemetry: NodeTelemetry =
            serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)?;
        if telemetry.schema_version != eggwork_core::NODE_TELEMETRY_SCHEMA_VERSION {
            return Err(ClientError::ProtocolIncompatible);
        }
        telemetry
            .validate()
            .map_err(|_| ClientError::InvalidResponse)?;
        Ok(telemetry)
    }

    /// Read the node's host-wide telemetry observation and require it to name
    /// `expected`.
    ///
    /// A node is a fixed target, so a response naming a different node is a
    /// misdirected or substituted answer. This refuses it rather than handing
    /// the caller a measurement taken somewhere else.
    pub async fn telemetry_for(&self, expected: &NodeId) -> Result<NodeTelemetry, ClientError> {
        let telemetry = self.telemetry().await?;
        if &telemetry.node_id != expected {
            return Err(ClientError::InvalidResponse);
        }
        Ok(telemetry)
    }

    /// Submit to this node and return its execution identifier and live event stream.
    /// A dropped stream leaves execution running; cancellation is explicit.
    pub async fn execute(
        &self,
        spec: &ExecutionSpec,
        handle: &ExecutionHandle,
    ) -> Result<ExecutionStream, ClientError> {
        self.execute_with_workspace(spec, handle, None).await
    }

    pub async fn execute_in_workspace(
        &self,
        spec: &ExecutionSpec,
        handle: &ExecutionHandle,
        workspace_id: &WorkspaceId,
    ) -> Result<ExecutionStream, ClientError> {
        self.execute_with_workspace(spec, handle, Some(workspace_id.clone()))
            .await
    }

    async fn execute_with_workspace(
        &self,
        spec: &ExecutionSpec,
        handle: &ExecutionHandle,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<ExecutionStream, ClientError> {
        self.ensure_protocol_compatible().await?;
        let payload = ExecuteRequest {
            schema_version: API_SCHEMA_VERSION,
            handle: handle.clone(),
            workspace_id,
            spec,
        };
        let mut response = self
            .http
            .post(&self.url("/v1/executions"))?
            .header("content-type", "application/json")
            .bytes(serde_json::to_vec(&payload).map_err(|_| ClientError::InvalidResponse)?)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        let id = response
            .headers()
            .get("eggwork-execution-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| ExecutionId::new(value.to_owned()).ok())
            .ok_or(ClientError::InvalidResponse)?;
        let events = response.bytes_stream()?;
        if id != handle.execution_id {
            return Err(ClientError::InvalidResponse);
        }
        Ok(ExecutionStream {
            handle: handle.clone(),
            execution_id: id,
            events,
        })
    }

    async fn ensure_protocol_compatible(&self) -> Result<(), ClientError> {
        let capabilities = self.capabilities().await?;
        protocol_compatibility(&capabilities)
    }

    pub async fn create_workspace(
        &self,
        workspace_id: &WorkspaceId,
        handle: &ExecutionHandle,
        manifest: &WorkspaceManifest,
    ) -> Result<WorkspaceReady, ClientError> {
        let mut response = self
            .http
            .post(&self.url("/v1/workspaces"))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&CreateWorkspaceRequest {
                    schema_version: API_SCHEMA_VERSION,
                    workspace_id,
                    handle,
                    manifest,
                })
                .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    /// Create a fresh execution-private workspace from a retained canonical
    /// base manifest plus a bounded deterministic patch, without resending
    /// the full manifest. The server never falls back to a different base:
    /// a missing/expired base surfaces as a typed `base_manifest_missing`
    /// API error, and the caller may retry with a full-manifest create.
    pub async fn create_workspace_derived(
        &self,
        workspace_id: &WorkspaceId,
        handle: &ExecutionHandle,
        base_manifest_digest: &BlobDigest,
        patch: &WorkspaceManifestPatch,
    ) -> Result<WorkspaceReady, ClientError> {
        if patch.base_manifest_digest != *base_manifest_digest {
            return Err(ClientError::InvalidResponse);
        }
        let mut response = self
            .http
            .post(&self.url("/v1/workspaces/derive"))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&CreateDerivedWorkspaceRequest {
                    schema_version: API_SCHEMA_VERSION,
                    workspace_id,
                    handle,
                    patch,
                })
                .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    pub async fn observe(&self, id: &ExecutionId) -> Result<ExecutionSnapshot, ClientError> {
        self.get_json(&format!("/v1/executions/{id}")).await
    }

    pub async fn cancel(&self, handle: &ExecutionHandle) -> Result<ExecutionSnapshot, ClientError> {
        let mut response = self
            .http
            .post(&self.url(&format!("/v1/executions/{}/cancel", handle.execution_id)))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&ControlRequest {
                    schema_version: API_SCHEMA_VERSION,
                    handle: handle.clone(),
                    renewal_id: None,
                })
                .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    pub async fn renew(
        &self,
        handle: &ExecutionHandle,
        renewal_id: &str,
    ) -> Result<ExecutionSnapshot, ClientError> {
        let mut response = self
            .http
            .post(&self.url(&format!("/v1/executions/{}/renew", handle.execution_id)))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&ControlRequest {
                    schema_version: API_SCHEMA_VERSION,
                    handle: handle.clone(),
                    renewal_id: Some(renewal_id.to_owned()),
                })
                .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    pub async fn observe_generation(
        &self,
        id: &ExecutionId,
        generation: u64,
    ) -> Result<ExecutionSnapshot, ClientError> {
        self.get_json(&format!("/v1/executions/{id}?generation={generation}"))
            .await
    }

    pub async fn events(
        &self,
        handle: &ExecutionHandle,
        after_sequence: u64,
    ) -> Result<BoxBytesStream, ClientError> {
        self.ensure_protocol_compatible().await?;
        let mut response = self
            .http
            .get(&self.url(&format!(
                "/v1/executions/{}/events?generation={}&after={after_sequence}&lease={}",
                handle.execution_id,
                handle.generation.get(),
                handle.lease_id.as_str(),
            )))?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        Ok(response.bytes_stream()?)
    }

    pub async fn find_missing_blobs(
        &self,
        digests: &[BlobDigest],
    ) -> Result<Vec<BlobDigest>, ClientError> {
        let mut response = self
            .http
            .post(&self.url("/v1/blobs/missing"))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&FindMissingRequest { digests })
                    .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        let missing: FindMissingResponse = decode_json(&mut response).await?;
        Ok(missing.missing)
    }

    /// Upload one content-addressed blob as a bounded HTTP byte stream.
    pub async fn upload_blob(
        &self,
        digest: &BlobDigest,
        declared_length: u64,
        stream: BoxBytesStream,
    ) -> Result<(), ClientError> {
        let mut preparation = self
            .http
            .post(&self.url("/v1/blobs/prepare"))?
            .timeout(unary_timeout())
            .header("content-type", "application/json")
            .bytes(
                serde_json::to_vec(&PrepareBlobRequest {
                    digest,
                    size_bytes: declared_length,
                })
                .map_err(|_| ClientError::InvalidResponse)?,
            )
            .send()
            .await?;
        if !preparation.status().is_success() {
            return Err(api_error(preparation.status().as_u16(), &mut preparation).await);
        }
        let preparation: PrepareBlobResponse = decode_json(&mut preparation).await?;
        if !preparation.upload_required {
            return Ok(());
        }
        let length = usize::try_from(declared_length).map_err(|_| ClientError::InvalidResponse)?;
        let mut response = self
            .http
            .put(&self.url(&format!(
                "/v1/blobs/{}?size={declared_length}",
                digest.as_str()
            )))?
            .header("content-type", "application/octet-stream")
            .body(RequestBody::from_stream(stream, Some(length)))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        Ok(())
    }

    /// Open a streamed download for a digest from this fixed node.
    pub async fn download_blob(&self, digest: &BlobDigest) -> Result<BoxBytesStream, ClientError> {
        let mut response = self
            .http
            .get(&self.url(&format!("/v1/blobs/{}", digest.as_str())))?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        let returned_digest = response
            .headers()
            .get("eggwork-blob-digest")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| BlobDigest::parse(value.to_owned()).ok());
        if returned_digest.as_ref() != Some(digest) {
            return Err(ClientError::InvalidResponse);
        }
        Ok(response.bytes_stream()?)
    }

    pub async fn artifacts(
        &self,
        execution_id: &ExecutionId,
        generation: u64,
    ) -> Result<Vec<ArtifactRecord>, ClientError> {
        self.get_json(&format!(
            "/v1/executions/{execution_id}/artifacts?generation={generation}"
        ))
        .await
    }

    pub async fn download_artifact(
        &self,
        artifact: &ArtifactRecord,
    ) -> Result<BoxBytesStream, ClientError> {
        let mut response = self
            .http
            .get(&self.url(&format!("/v1/artifacts/{}", artifact.artifact_id.as_str())))?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        let returned_id = response
            .headers()
            .get("eggwork-artifact-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| ArtifactId::new(value.to_owned()).ok());
        let returned_digest = response
            .headers()
            .get("eggwork-artifact-digest")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| BlobDigest::parse(value.to_owned()).ok());
        if returned_id.as_ref() != Some(&artifact.artifact_id)
            || returned_digest.as_ref() != Some(&artifact.digest)
        {
            return Err(ClientError::InvalidResponse);
        }
        Ok(response.bytes_stream()?)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let mut response = self
            .http
            .get(&self.url(path))?
            .timeout(unary_timeout())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint)
    }
}

fn normalize_endpoint(endpoint: &str) -> Result<String, ClientError> {
    let endpoint = endpoint.trim_end_matches('/').to_owned();
    let parsed = url::Url::parse(&endpoint).map_err(|_| ClientError::InvalidEndpoint)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ClientError::InvalidEndpoint);
    }
    Ok(endpoint)
}

pub struct ExecutionStream {
    pub handle: ExecutionHandle,
    pub execution_id: ExecutionId,
    events: BoxBytesStream,
}

#[cfg(feature = "eggress-route")]
struct EggressDialer {
    connector: Arc<eggress_outbound::OutboundConnector>,
}

#[cfg(feature = "eggress-route")]
struct EggressIo<S>(S);

#[cfg(feature = "eggress-route")]
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for EggressIo<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[cfg(feature = "eggress-route")]
impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for EggressIo<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(feature = "eggress-route")]
impl eggfetch_core::Dialer for EggressDialer {
    fn dial(&self, target: eggfetch_core::DialTarget) -> eggfetch_core::DialFuture<'_> {
        Box::pin(async move {
            let (stream, _) = self
                .connector
                .connect_tcp_timeout_detailed(
                    target.host(),
                    target.port(),
                    std::time::Duration::from_secs(30),
                )
                .await
                .map_err(|error| {
                    use eggfetch_core::DialErrorKind;
                    use eggress_outbound::OutboundConnectErrorKind as RouteKind;
                    let kind = match error.kind() {
                        RouteKind::Timeout => DialErrorKind::Timeout,
                        RouteKind::Authentication => DialErrorKind::Authentication,
                        RouteKind::Policy => DialErrorKind::Rejected,
                        RouteKind::Dns
                        | RouteKind::ConnectionRefused
                        | RouteKind::NetworkUnreachable
                        | RouteKind::HostUnreachable => DialErrorKind::Connection,
                        _ => DialErrorKind::Other,
                    };
                    eggfetch_core::DialError::with_source(
                        kind,
                        "Eggress route connection failed",
                        error,
                    )
                })?;
            let routed: eggfetch_core::DialStream = Box::new(EggressIo(stream));
            Ok(routed)
        })
    }
}

impl ExecutionStream {
    /// Consume newline-delimited JSON events from the live response.
    pub fn into_events(
        self,
    ) -> impl futures_util::Stream<Item = Result<ExecutionEvent, ClientError>> {
        stream::try_unfold(
            (self.events, BytesMut::new()),
            |(mut events, mut pending)| async move {
                loop {
                    if let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                        let mut line = pending.split_to(newline + 1);
                        line.truncate(newline);
                        if line.is_empty() {
                            continue;
                        }
                        let event: ExecutionEvent = serde_json::from_slice(&line)
                            .map_err(|_| ClientError::InvalidResponse)?;
                        event.validate().map_err(|_| ClientError::InvalidResponse)?;
                        return Ok(Some((event, (events, pending))));
                    }
                    match events.next().await {
                        Some(Ok(bytes)) => {
                            if pending.len().saturating_add(bytes.len())
                                > eggwork_core::MAX_EVENT_BYTES
                            {
                                return Err(ClientError::InvalidResponse);
                            }
                            pending.extend_from_slice(&bytes)
                        }
                        Some(Err(error)) => return Err(ClientError::Transport(error)),
                        None if pending.is_empty() => return Ok(None),
                        None => {
                            let event: ExecutionEvent = serde_json::from_slice(&pending)
                                .map_err(|_| ClientError::InvalidResponse)?;
                            event.validate().map_err(|_| ClientError::InvalidResponse)?;
                            return Ok(Some((event, (events, BytesMut::new()))));
                        }
                    }
                }
            },
        )
    }

    /// Access the raw Eggfetch response stream, preserving transport chunk boundaries.
    pub fn into_bytes(self) -> BoxBytesStream {
        self.events
    }
}

/// Decide whether a node's advertised capabilities are usable by this client.
///
/// The decision is separated from the fetch so the rule can be tested without
/// a live node, and so the rule names the *owned* constants rather than
/// literals: the protocol range the client speaks, and the argv feature the
/// client cannot work without. A node that advertises a different argv
/// feature name is refused with the typed unsupported-capability error rather
/// than being talked to in a dialect this client does not implement.
fn protocol_compatibility(capabilities: &NodeCapabilities) -> Result<(), ClientError> {
    let required = eggwork_core::ProtocolVersion { major: 1, minor: 0 };
    let supported = capabilities.protocol.min <= required && capabilities.protocol.max >= required;
    if !supported {
        return Err(ClientError::ProtocolIncompatible);
    }
    if !capabilities
        .features
        .iter()
        .any(|feature| feature == EXEC_ARGV_CAPABILITY)
    {
        return Err(ClientError::UnsupportedCapability);
    }
    Ok(())
}

#[derive(Serialize)]
struct ExecuteRequest<'a> {
    schema_version: u16,
    handle: ExecutionHandle,
    workspace_id: Option<WorkspaceId>,
    spec: &'a ExecutionSpec,
}

#[derive(Serialize)]
struct CreateWorkspaceRequest<'a> {
    schema_version: u16,
    workspace_id: &'a WorkspaceId,
    handle: &'a ExecutionHandle,
    manifest: &'a WorkspaceManifest,
}

#[derive(Serialize)]
struct CreateDerivedWorkspaceRequest<'a> {
    schema_version: u16,
    workspace_id: &'a WorkspaceId,
    handle: &'a ExecutionHandle,
    patch: &'a WorkspaceManifestPatch,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkspaceReady {
    pub schema_version: u16,
    pub workspace_id: WorkspaceId,
    pub execution_id: ExecutionId,
    pub generation: eggwork_core::ExecutionGeneration,
    pub manifest_digest: BlobDigest,
    pub logical_bytes: u64,
}

#[derive(Serialize)]
struct ControlRequest {
    schema_version: u16,
    handle: ExecutionHandle,
    renewal_id: Option<String>,
}

#[derive(Serialize)]
struct FindMissingRequest<'a> {
    digests: &'a [BlobDigest],
}

#[derive(Serialize)]
struct PrepareBlobRequest<'a> {
    digest: &'a BlobDigest,
    size_bytes: u64,
}

#[derive(serde::Deserialize)]
struct PrepareBlobResponse {
    upload_required: bool,
}

#[derive(serde::Deserialize)]
struct FindMissingResponse {
    missing: Vec<BlobDigest>,
}

async fn decode_json<T: DeserializeOwned>(
    response: &mut eggfetch_core::Response,
) -> Result<T, ClientError> {
    let bytes = response.bytes().await?;
    serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
}

async fn api_error(status: u16, response: &mut eggfetch_core::Response) -> ClientError {
    let parsed = response
        .bytes()
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<ApiError>(&body).ok());
    match parsed {
        Some(ApiError { code, message, .. }) => ClientError::Api {
            status,
            code,
            message,
        },
        None => ClientError::Api {
            status,
            code: "http_error".into(),
            message: "node returned an error response".into(),
        },
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn endpoint_rejects_credentials_and_non_tls_schemes() {
        assert!(normalize_endpoint("https://controller.example/").is_ok());
        let credential_url =
            normalize_endpoint("https://user:password@controller.example").unwrap_err();
        assert!(!format!("{credential_url:?}").contains("password"));
        assert!(!credential_url.to_string().contains("password"));
        assert!(normalize_endpoint("http://controller.example").is_err());
        assert!(normalize_endpoint("https://controller.example/?token=secret").is_err());
        assert_eq!(
            safe_target_origin("https://controller.example/prefix/path-token"),
            "https://controller.example"
        );
    }

    #[test]
    fn every_request_path_carries_a_deadline() {
        // Regression: neither builder set a timeout, and Eggfetch derives every
        // phase from `Default` as `None`, so a hung or black-holed node left
        // `execute`, `events` and `renew` pending forever — with `renew` the
        // sharpest case, because the lease expires silently while the caller is
        // still blocked on it.
        let transport = transport_timeouts();
        assert!(transport.pool.is_some());
        assert!(transport.connect.is_some());
        assert!(transport.write.is_some());
        assert!(
            transport.read.is_some(),
            "the streaming paths depend on the read deadline to bound idle"
        );
        // A total deadline would abort a healthy long-lived transfer, so the
        // shared client must never carry one.
        assert!(transport.total.is_none());
        // The unary paths additionally get a wall-clock cap, so a node that
        // trickles bytes forever still terminates.
        let unary = unary_timeout();
        assert!(unary.total.is_some());
        assert!(unary.read.is_some());
    }

    #[test]
    fn client_error_debug_redacts_remote_message() {
        let error = ClientError::Api {
            status: 500,
            code: "internal".into(),
            message: "private-key-material".into(),
        };
        assert!(!format!("{error:?}").contains("private-key-material"));
        assert!(!error.to_string().contains("private-key-material"));
    }
}

/// The shared protocol contract facts the client consumes.
///
/// These live in the client's own crate rather than in `tests/` because the
/// request bodies the client authors are private: the assertion has to reach
/// the actual structs the transport serializes, not a re-implementation of
/// them. The server crate dev-depends on this one and runs the other half of
/// the agreement — that a body this client builds is accepted by the node.
#[cfg(test)]
mod shared_contract_tests {
    use super::*;
    use eggwork_core::{
        BlobDigest, CommandSpec, ExecutionGeneration, ExecutionId, ExecutionSpec,
        IsolationRequirement, LeaseId, NetworkRequirement, NodeCapabilities, OutputPolicy,
        ProtocolVersion, ProtocolVersionRange, Requirement, ResourceRequirements, StdinPolicy,
        WorkspaceManifest, WorkspaceManifestPatch,
    };

    fn handle() -> ExecutionHandle {
        ExecutionHandle {
            execution_id: ExecutionId::new("execution-a").unwrap(),
            generation: ExecutionGeneration::new(1).unwrap(),
            lease_id: LeaseId::new("lease-a").unwrap(),
        }
    }

    fn spec() -> ExecutionSpec {
        ExecutionSpec {
            schema_version: API_SCHEMA_VERSION,
            command: CommandSpec {
                argv: vec!["/bin/true".into()],
                cwd: None,
                environment: Vec::new(),
                stdin: StdinPolicy::Null,
                timeout_millis: 5_000,
                output: OutputPolicy::default(),
                declared_outputs: Vec::new(),
                resources: ResourceRequirements {
                    memory_bytes: Requirement::NotRequested,
                    cpu_millis: Requirement::NotRequested,
                    pids: Requirement::NotRequested,
                },
                isolation: IsolationRequirement::None,
                network: NetworkRequirement::Unrestricted,
            },
            metadata: Vec::new(),
        }
    }

    fn manifest() -> WorkspaceManifest {
        WorkspaceManifest {
            schema_version: 1,
            entries: Vec::new(),
        }
    }

    fn patch() -> WorkspaceManifestPatch {
        WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: BlobDigest::from_bytes(b"base"),
            entries: Vec::new(),
        }
    }

    /// Every request body the client authors carries the shared schema version.
    ///
    /// The four body shapes are the execute, control, workspace-create and
    /// workspace-derive requests. Each one is serialized here and its
    /// `schema_version` field is read back, so this asserts the bytes that
    /// actually go on the wire rather than the field in isolation. A private
    /// copy of the constant in this crate would still satisfy the assertion
    /// only if it happened to equal the shared value — which is exactly why
    /// the server crate additionally checks that this client's request is not
    /// rejected as `protocol_version`.
    #[test]
    fn every_authored_request_body_carries_the_shared_schema_version() {
        let handle = handle();
        let workspace_id = WorkspaceId::new("workspace-a").unwrap();
        let spec = spec();
        let manifest = manifest();
        let patch = patch();

        let bodies = [
            serde_json::to_value(&ExecuteRequest {
                schema_version: API_SCHEMA_VERSION,
                handle: handle.clone(),
                workspace_id: None,
                spec: &spec,
            })
            .unwrap(),
            serde_json::to_value(&ControlRequest {
                schema_version: API_SCHEMA_VERSION,
                handle: handle.clone(),
                renewal_id: None,
            })
            .unwrap(),
            serde_json::to_value(&ControlRequest {
                schema_version: API_SCHEMA_VERSION,
                handle: handle.clone(),
                renewal_id: Some("renewal-a".into()),
            })
            .unwrap(),
            serde_json::to_value(&CreateWorkspaceRequest {
                schema_version: API_SCHEMA_VERSION,
                workspace_id: &workspace_id,
                handle: &handle,
                manifest: &manifest,
            })
            .unwrap(),
            serde_json::to_value(&CreateDerivedWorkspaceRequest {
                schema_version: API_SCHEMA_VERSION,
                workspace_id: &workspace_id,
                handle: &handle,
                patch: &patch,
            })
            .unwrap(),
        ];

        for body in bodies {
            assert_eq!(
                body.get("schema_version").and_then(|value| value.as_u64()),
                Some(u64::from(API_SCHEMA_VERSION)),
                "an authored request body carries the wrong schema version: {body}"
            );
        }
    }

    fn capabilities(features: Vec<&str>) -> NodeCapabilities {
        NodeCapabilities {
            protocol: ProtocolVersionRange {
                min: ProtocolVersion { major: 1, minor: 0 },
                max: ProtocolVersion { major: 1, minor: 0 },
            },
            features: features.into_iter().map(str::to_owned).collect(),
            max_active_executions: 1,
        }
    }

    /// The client accepts a node that advertises the owned argv feature.
    #[test]
    fn a_node_advertising_the_owned_argv_feature_is_accepted() {
        assert!(
            protocol_compatibility(&capabilities(vec![EXEC_ARGV_CAPABILITY])).is_ok(),
            "a node advertising the owned argv feature must be accepted"
        );
    }

    /// The client refuses a node whose argv feature name differs.
    ///
    /// The differing name is one character away from the owned one, so this is
    /// the regression test for the drift this milestone removes: a server that
    /// advertised `exec.argv.v2` while the client demanded `exec.argv.v1`
    /// would have been refused with a typed error rather than a silent
    /// mismatch — and, before the shared constant existed, nothing in either
    /// crate would have said the two names had to agree.
    #[test]
    fn a_node_advertising_a_differing_argv_feature_is_refused() {
        let differing = format!("{EXEC_ARGV_CAPABILITY}v2");
        let error = protocol_compatibility(&capabilities(vec![differing.as_str()]))
            .expect_err("a node with a different argv feature must be refused");
        assert!(
            matches!(error, ClientError::UnsupportedCapability),
            "expected the typed unsupported-capability error, got {error:?}"
        );
    }

    /// A node that speaks a different protocol version is refused first.
    #[test]
    fn a_node_speaking_a_different_protocol_version_is_refused() {
        let mut node = capabilities(vec![EXEC_ARGV_CAPABILITY]);
        node.protocol = ProtocolVersionRange {
            min: ProtocolVersion { major: 2, minor: 0 },
            max: ProtocolVersion { major: 2, minor: 0 },
        };
        let error = protocol_compatibility(&node)
            .expect_err("a node on another protocol version must be refused");
        assert!(
            matches!(error, ClientError::ProtocolIncompatible),
            "expected the typed protocol-incompatible error, got {error:?}"
        );
    }
}

#[cfg(test)]
mod event_framing_tests {
    use super::*;
    use eggwork_core::{EventSequence, ExecutionEventKind};

    /// Frame `events` as the node writes them: one JSON object per `\n`-terminated
    /// line, delivered in `chunk` sized transport pieces so the line-splitting
    /// path is exercised rather than short-circuited by whole-line delivery.
    fn ndjson(events: &[&ExecutionEvent], chunk: usize) -> Vec<Result<bytes::Bytes, HttpError>> {
        let mut body = Vec::new();
        for event in events {
            let line = serde_json::to_vec(event).unwrap();
            body.extend_from_slice(&line);
            body.push(b'\n');
        }
        body.chunks(chunk)
            .map(|piece| Ok(bytes::Bytes::copy_from_slice(piece)))
            .collect()
    }

    fn stream_over(chunks: Vec<Result<bytes::Bytes, HttpError>>) -> ExecutionStream {
        ExecutionStream {
            handle: ExecutionHandle {
                execution_id: ExecutionId::new("exec-a").unwrap(),
                generation: eggwork_core::ExecutionGeneration::new(1).unwrap(),
                lease_id: eggwork_core::LeaseId::new("lease-a").unwrap(),
            },
            execution_id: ExecutionId::new("exec-a").unwrap(),
            events: Box::pin(stream::iter(chunks)),
        }
    }

    fn stdout_event(sequence: u64, bytes: &[u8]) -> ExecutionEvent {
        ExecutionEvent {
            sequence: EventSequence::new(sequence),
            kind: ExecutionEventKind::Stdout(bytes.to_vec()),
            metadata: eggwork_core::EventMetadata { fields: vec![] },
        }
    }

    #[tokio::test]
    async fn a_maximum_size_chunk_is_delivered_not_rejected() {
        // Regression: the framing cap used to be 128 KiB, but a caller may legally
        // ask for `MAX_EVENT_CHUNK_BYTES` of output, which encodes to ~256 KiB
        // because `Vec<u8>` serializes as a JSON number array. The client rejected
        // the line, and because the stream is `try_unfold` that discarded every
        // event already delivered.
        let payload = vec![0xFFu8; eggwork_core::MAX_EVENT_CHUNK_BYTES];
        let event = stdout_event(1, &payload);
        let line = serde_json::to_vec(&event).unwrap();
        assert!(
            line.len() > 128 * 1024,
            "fixture must exceed the old cap, got {}",
            line.len()
        );

        let trailing = stdout_event(2, b"done");
        let chunks = ndjson(&[&event, &trailing], 8192);

        let mut events = Box::pin(stream_over(chunks).into_events());
        let first = events.next().await.unwrap().unwrap();
        assert_eq!(first.kind, ExecutionEventKind::Stdout(payload));
        // The oversized line must not terminate the stream: `try_unfold` turns a
        // framing error into the loss of everything already buffered.
        let second = events.next().await.unwrap().unwrap();
        assert_eq!(second.kind, ExecutionEventKind::Stdout(b"done".to_vec()));
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn framing_handles_empty_lines_split_boundaries_and_a_final_partial_line() {
        let first = stdout_event(1, b"alpha");
        let second = stdout_event(2, b"beta");
        let mut body = Vec::new();
        body.extend_from_slice(b"\n");
        body.extend_from_slice(&serde_json::to_vec(&first).unwrap());
        body.extend_from_slice(b"\n\n");
        body.extend_from_slice(&serde_json::to_vec(&second).unwrap());

        let pieces: Vec<Result<bytes::Bytes, HttpError>> = body
            .chunks(7)
            .map(|piece| Ok(bytes::Bytes::copy_from_slice(piece)))
            .collect();
        let mut events = Box::pin(stream_over(pieces).into_events());
        assert_eq!(
            events.next().await.unwrap().unwrap().kind,
            ExecutionEventKind::Stdout(b"alpha".to_vec())
        );
        // The last line carries no trailing newline and must still be parsed.
        assert_eq!(
            events.next().await.unwrap().unwrap().kind,
            ExecutionEventKind::Stdout(b"beta".to_vec())
        );
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn an_oversized_partial_line_is_still_refused() {
        // The bound protects memory, so it must not simply be removed: a line
        // past `MAX_EVENT_BYTES` with no newline is still `InvalidResponse`.
        let pieces: Vec<Result<bytes::Bytes, HttpError>> = vec![Ok(bytes::Bytes::from(vec![
                b'x';
                eggwork_core::MAX_EVENT_BYTES
                    + 1
            ]))];
        let mut events = Box::pin(stream_over(pieces).into_events());
        assert!(matches!(
            events.next().await,
            Some(Err(ClientError::InvalidResponse))
        ));
    }
}

#[cfg(all(test, feature = "eggress-route"))]
mod eggress_route_tests {
    use super::*;
    use eggfetch_core::Dialer;
    use eggress_testkit::fixtures::{HttpConnectUpstream, Socks5Upstream};
    use std::error::Error;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn init_crypto_provider() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    async fn round_trip(route: String) {
        init_crypto_provider();
        let (target, server) = eggress_testkit::start_echo_server().await;
        let connector = eggress_outbound::OutboundConnector::from_pproxy_uri(&route).unwrap();
        let dialer = EggressDialer {
            connector: Arc::new(connector),
        };
        let mut stream = dialer
            .dial(eggfetch_core::DialTarget::new("127.0.0.1", target.port()))
            .await
            .unwrap();
        stream.write_all(b"eggwork route").await.unwrap();
        let mut response = [0u8; 13];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"eggwork route");
        server.abort();
    }

    #[tokio::test]
    async fn socks5_route_round_trips() {
        let proxy = Socks5Upstream::start().await;
        round_trip(format!("socks5://{}", proxy.addr())).await;
    }

    #[tokio::test]
    async fn http_connect_route_round_trips() {
        let proxy = HttpConnectUpstream::start().await;
        round_trip(format!("http://{}", proxy.addr())).await;
    }

    #[tokio::test]
    async fn route_failure_does_not_connect_direct_and_preserves_typed_error() {
        init_crypto_provider();
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::net::TcpListener;

        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_task = accepted.clone();
        let server = tokio::spawn(async move {
            if target.accept().await.is_ok() {
                accepted_task.fetch_add(1, Ordering::SeqCst);
            }
        });
        let connector =
            eggress_outbound::OutboundConnector::from_pproxy_uri("socks5://127.0.0.1:1").unwrap();
        let dialer = EggressDialer {
            connector: Arc::new(connector),
        };
        let result = dialer
            .dial(eggfetch_core::DialTarget::new(
                target_addr.ip().to_string(),
                target_addr.port(),
            ))
            .await;
        let error = match result {
            Ok(_) => panic!("configured proxy unexpectedly connected"),
            Err(error) => error,
        };
        assert!(error.source().is_some());
        assert_eq!(accepted.load(Ordering::SeqCst), 0);
        server.abort();
    }
}
