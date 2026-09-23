#![forbid(unsafe_code)]

//! Explicit, single-node Eggwork client using Eggfetch for all HTTP transport.

use bytes::BytesMut;
use eggfetch_core::{
    BoxBytesStream, Client as HttpClient, Error as HttpError, RequestBody, TlsConfig,
};
use eggwork_core::{
    ApiError, BlobDigest, ExecutionEvent, ExecutionHandle, ExecutionId, ExecutionSnapshot,
    ExecutionSpec, NodeCapabilities, NodeStatus,
};
use futures_util::{StreamExt, stream};
use serde::{Serialize, de::DeserializeOwned};
use std::{fmt, path::Path};
use thiserror::Error;

const API_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("node endpoint must be an https URL without query, fragment, or credentials")]
    InvalidEndpoint,
    #[error("Eggfetch request failed: {0}")]
    Transport(#[from] HttpError),
    #[error("node returned HTTP {status}: {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("node response was malformed")]
    InvalidResponse,
}

/// A connection to exactly one caller-selected Eggwork node.
#[derive(Clone)]
pub struct NodeClient {
    endpoint: String,
    http: HttpClient,
}

impl fmt::Debug for NodeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
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
        let endpoint = endpoint.as_ref().trim_end_matches('/').to_owned();
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
        let http = HttpClient::builder().tls_config(tls).build();
        Ok(Self { endpoint, http })
    }

    pub async fn capabilities(&self) -> Result<NodeCapabilities, ClientError> {
        self.get_json("/v1/capabilities").await
    }

    pub async fn status(&self) -> Result<NodeStatus, ClientError> {
        self.get_json("/v1/status").await
    }

    /// Submit to this node and return its execution identifier and live event stream.
    /// A dropped stream leaves execution running; cancellation is explicit.
    pub async fn execute(
        &self,
        spec: &ExecutionSpec,
        handle: &ExecutionHandle,
    ) -> Result<ExecutionStream, ClientError> {
        let payload = ExecuteRequest {
            schema_version: API_SCHEMA_VERSION,
            handle: handle.clone(),
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

    pub async fn observe(&self, id: &ExecutionId) -> Result<ExecutionSnapshot, ClientError> {
        self.get_json(&format!("/v1/executions/{id}")).await
    }

    pub async fn cancel(&self, handle: &ExecutionHandle) -> Result<ExecutionSnapshot, ClientError> {
        let mut response = self
            .http
            .post(&self.url(&format!("/v1/executions/{}/cancel", handle.execution_id)))?
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

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let mut response = self.http.get(&self.url(path))?.send().await?;
        if !response.status().is_success() {
            return Err(api_error(response.status().as_u16(), &mut response).await);
        }
        decode_json(&mut response).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint)
    }
}

pub struct ExecutionStream {
    pub handle: ExecutionHandle,
    pub execution_id: ExecutionId,
    events: BoxBytesStream,
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
                        let event = serde_json::from_slice(&line)
                            .map_err(|_| ClientError::InvalidResponse)?;
                        return Ok(Some((event, (events, pending))));
                    }
                    match events.next().await {
                        Some(Ok(bytes)) => pending.extend_from_slice(&bytes),
                        Some(Err(error)) => return Err(ClientError::Transport(error)),
                        None if pending.is_empty() => return Ok(None),
                        None => {
                            let event = serde_json::from_slice(&pending)
                                .map_err(|_| ClientError::InvalidResponse)?;
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

#[derive(Serialize)]
struct ExecuteRequest<'a> {
    schema_version: u16,
    handle: ExecutionHandle,
    spec: &'a ExecutionSpec,
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
