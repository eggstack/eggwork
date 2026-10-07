# eggwork-client

`eggwork-client` is the caller's entry point into the Eggwork execution fabric: a single Rust
library crate (880 lines, one file, `#![forbid(unsafe_code)]`) that speaks the v1 control-plane
protocol to **one node the caller named explicitly**. `NodeClient` holds exactly two fields — a
normalized `endpoint: String` and an `eggfetch_core::Client` — so the target is fixed at
construction by the type system rather than by policy: there is no node registry, no discovery, no
candidate list, and no code path that substitutes a different host. Every protocol operation
(capabilities, status, execute, observe, cancel, renew, events, blob read/write, workspace
create/derive, artifact read/list) is a method on that object, and every method reports what that
one node said. The crate depends on `eggwork-core` only for domain types; all HTTP, TLS, and
streaming is delegated to `eggfetch-core`, with an optional `eggress-route` feature adding an
outbound TCP route that changes the *path* to the node but never *which* node is addressed.

Architecturally it is a `dev-dependency` of `eggwork-server` (`crates/eggwork-server/Cargo.toml:46`),
used by the server's test module and the installed-release qualification harness. It is **not** a
production dependency of the shipped daemon — `eggworkd` never links it. That inversion is
deliberate: the client is the consumer of the node's protocol, and keeping it out of the daemon's
production graph is what stops client convenience from constraining server design.

## Responsibility boundary

**Owns**

- Construction-time endpoint validation (scheme, host, absence of query/fragment/credentials).
- Local `TlsConfig` assembly from PEM files, and the optional Eggress dial path.
- Wire encoding of every control request: `serde_json` bodies, `content-type` headers, the
  `schema_version` field, blob upload streaming.
- Wire decoding: `ApiError` bodies, `eggwork_core` response types, and the four identity headers
  the server stamps on streaming responses.
- NDJSON framing of the live event stream, a bounded partial-line buffer, and per-event
  `ExecutionEvent::validate()`.
- Pre-submission protocol/capability negotiation for `execute` and `events`.
- Redaction of its own `Debug` output (`NodeClient`, `ClientError`).

**Does not own**

- Scheduling, placement, node selection, fairness, prioritization, or worker selection. The client
  names one node and reports that node's answer. There is no API that means "any node", and no
  operation that takes a node identifier as a runtime argument — the node is a construction-time
  fact.
- Retry policy. No method retries, no method re-resolves, no method falls back. A failed request is
  a returned `Err`. Idempotency primitives exist (`ExecutionHandle` carries a `generation` and a
  `lease_id`), but re-submitting is an explicit caller action.
- Queueing or waiting. If the node is busy, draining, or unauthorized, the client surfaces the
  typed refusal. It does not block, wait-list, or spill to another node.
- Interpreting `NodeStatus` or `NodeCapabilities` into a decision. `capabilities()` and `status()`
  return the facts; the caller decides what they mean. The single exception is
  `ensure_protocol_compatible`, which rejects a node whose protocol range excludes `1.0` or which
  does not advertise `exec.argv.v1` — a compatibility gate, not a placement policy.
- Node-side admission, process lifecycle, workspace/blob storage semantics, or event retention.
- Principal identity. The client presents a certificate and nothing else; it never asserts who it
  is in a payload.
- Local proxy/listener service. The Eggress path is listener-free by construction
  ([M003 §3](../plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md)).

This boundary is the client-side half of
[ADR-0001](../plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md) decision 1 and 2.

## The fixed-target guarantee

The guarantee is structural, not conventional.

1. **One endpoint, validated once.** `normalize_endpoint` (`lib.rs:532`) trims trailing `/`, parses
   with `url::Url::parse`, and requires: `scheme() == "https"`, `host_str().is_some()`, no query, no
   fragment, empty username, no password. It returns the normalized string or `ClientError::InvalidEndpoint`.
   It is called from every constructor — `new`, `with_eggress`, `with_eggress_route` — and never again.
2. **Two fields, no indirection.** `NodeClient { endpoint, http }` (`lib.rs:89`). The `endpoint` is
   a `String`, not a resolver, a list, or an enum of candidates. `Clone` copies it verbatim and
   shares the `HttpClient` (and therefore the connection pool) — cloning a client never changes or
   multiplies its target.
3. **One URL constructor.** `fn url(&self, path) -> String { format!("{}{path}", self.endpoint) }`
   (`lib.rs:527`). Every request in the crate goes through it. There is no second URL builder, no
   redirect target, no server-supplied URL that the client will follow.
4. **No discovery, no "latest", no default.** Nothing in the crate resolves a node name, reads a
   registry, consults an environment variable, or defaults to a well-known host. The endpoint is
   always a caller-supplied argument. There is no code path that can retarget a different node.

**The egress-route invariant.** The `eggress-route` feature adds a *route* without changing *target
identity*. Precisely:

- `with_eggress` (`lib.rs:135`) calls the **same** `normalize_endpoint` and builds the same
  `HttpClient::builder().tls_config(tls)`; it only adds `.dialer(EggressDialer { connector })`.
  `self.endpoint` is byte-identical to what `new` would have produced. The URL, the SNI name, the
  certificate-verification target, and the mTLS peer are all unchanged.
- `EggressDialer::dial` (`lib.rs:598`) receives the `DialTarget` that eggfetch derived from the
  endpoint and calls `connect_tcp_timeout_detailed(target.host(), target.port(), 30s)` **exactly
  once**. It returns the Eggress stream as the raw `DialStream`; eggfetch layers its own HTTP/1.1
  framing and destination TLS on top. TLS ownership does not move to Eggress
  ([M003 §2](../plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md),
  [ADR-0002 decision 5/6](../plans/adrs/ADR-0002-eggstack-transport-and-mtls.md)).
- **No fallback exists.** The dialer returns `Err` on failure. There is no second client, no
  default dialer, no retry-direct branch. `ClientError::eggress_error()` (`lib.rs:54`) recovers the
  typed `eggress_outbound::OutboundConnectError` by downcasting the `Transport` source chain, so a
  caller can distinguish timeout / authentication / policy / DNS / connection classes without
  parsing display text — and can see *that* it was a route failure rather than a TLS failure.
- The public failure message is the fixed string `"Eggress route connection failed"`; the typed
  error is attached as a nested source rather than flattened into the message.
- **Route configuration never enters the protocol.** `ExecuteRequest` (`lib.rs:681`) is
  `{ schema_version, handle, workspace_id, spec }`, and `eggwork_core::ExecutionSpec` is
  `{ schema_version, command, metadata }`. No field anywhere in a client-authored body carries a
  route, proxy, or hop. Route selection is a property of the *client object*, and it is not
  visible to, or negotiable by, the node.
- The one place the route is disclosed is `NodeClient`'s `Debug`, which reports only
  `target_origin` — never the route expression, so proxy credentials cannot reach a log through it.

See also durable invariant 1 and 10 of the
[control-plane roadmap](../plans/subsystems/control-plane-protocol-roadmap.md) ("One request
targets one explicit node"; "Transport/proxy failure never silently changes route").

## Transport stack

| Crate | Kind | Role in this crate |
|---|---|---|
| `eggfetch-core` 0.2.0 | direct dep | **All** HTTP. Client, `TlsConfig`, request/response, `BoxBytesStream`, `RequestBody`, `Error`. |
| `eggress-outbound` 1.0.8 | optional dep | `OutboundConnector` — listener-free proxy-chain TCP dial path, feature `eggress-route` only. |
| `bytes` | direct dep | `BytesMut` — the NDJSON partial-line buffer in `ExecutionStream::into_events`. |
| `futures-util` | direct dep | `StreamExt` (polling the response body) and `stream::try_unfold` (the decoded event stream). |
| `url` | direct dep | Endpoint parsing/validation and `Url::origin()` for the `Debug` redaction. |
| `tokio` | direct dep | `AsyncRead`/`AsyncWrite` for the `EggressIo` newtype around the Eggress stream; `io::ReadBuf`. |
| `serde` / `serde_json` | direct deps | Request encoding, response decoding. |
| `thiserror` | direct dep | `#[derive(Error)]` on `ClientError`. |
| `eggress-testkit` 1.0.8 | **dev**-dep | SOCKS5 / HTTP CONNECT proxy fixtures and an echo server. |
| `rustls` 0.23 | **dev**-dep | Only to install the ring default crypto provider in tests. |

**The Eggfetch feature profile matters.** The workspace pins
`eggfetch-core = { version = "0.2.0", default-features = false, features = ["standard-http1", "tls-rustls"] }`.
In eggfetch 0.2.0, `standard-http1 = ["transport-http1", "standard-route", "high-level-url"]` — it
does **not** pull `redirects` or `logical-retry` (those come only with the `http1`/`http2`
compatibility aliases) and it does **not** pull `advanced-routing` (only `native-http1` does). Two
consequences the reader should hold onto:

- `Client::send` compiles to the `send_lean` path: **each request is dispatched exactly once** and
  **3xx responses are returned as ordinary responses**, never followed.
- `eggress-route` must therefore add `eggfetch-core/advanced-routing` explicitly, which is exactly
  what `crates/eggwork-client/Cargo.toml:26` does — that is the only reason the explicit
  `ClientBuilder::dialer` seam is available.

`eggress-outbound` is used with `default-features = false, features = ["pproxy-compat"]`, matching
[M003 §5](../plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md)'s
"smallest feature set" rule.

## Public API

### Construction

| Constructor | Signature | Notes |
|---|---|---|
| `from_pem_files` | `(endpoint, ca_certificate, client_certificate, client_private_key)` | `TlsConfig::builder().ca_certificate_path(ca)?.client_cert_path(cert, key)?.build()`. Explicit CA replaces the default trust store. Builder errors become `ClientError::Transport`. |
| `new` | `(endpoint, tls: TlsConfig)` | Caller-configured TLS policy. Verification is whatever the caller's `TlsConfig` says. |
| `with_eggress` | `(endpoint, tls, connector: OutboundConnector)` | `eggress-route` only. Same endpoint normalization; adds the dialer. |
| `with_eggress_route` | `(endpoint, tls, route: &str)` | `eggress-route` only. `OutboundConnector::from_pproxy_uri(route)`; a parse failure maps to `ClientError::InvalidRoute`. |

All four return `Result<Self, ClientError>`, validate the endpoint, and take no node identifier at
call time. `NodeClient` is `Clone` and has a hand-written `Debug` printing only `target_origin`
with `finish_non_exhaustive()`.

### Operations

`handle` is `eggwork_core::ExecutionHandle` (`execution_id`, `generation`, `lease_id`); `spec` is
`eggwork_core::ExecutionSpec`; `manifest` is `eggwork_core::WorkspaceManifest`; `patch` is
`eggwork_core::WorkspaceManifestPatch`.

| Method | HTTP | Route path | Request body | Returns |
|---|---|---|---|---|
| `capabilities()` | GET | `/v1/capabilities` | — | `NodeCapabilities` |
| `status()` | GET | `/v1/status` | — | `NodeStatus` |
| `execute(&spec, &handle)` | POST | `/v1/executions` | `ExecuteRequest` (`workspace_id: None`) | `ExecutionStream` |
| `execute_in_workspace(&spec, &handle, &workspace_id)` | POST | `/v1/executions` | `ExecuteRequest` (`workspace_id: Some(..)`) | `ExecutionStream` |
| `observe(&id)` | GET | `/v1/executions/{id}` | — | `ExecutionSnapshot` |
| `observe_generation(&id, generation)` | GET | `/v1/executions/{id}?generation={generation}` | — | `ExecutionSnapshot` |
| `cancel(&handle)` | POST | `/v1/executions/{handle.execution_id}/cancel` | `ControlRequest { renewal_id: None }` | `ExecutionSnapshot` |
| `renew(&handle, &renewal_id)` | POST | `/v1/executions/{handle.execution_id}/renew` | `ControlRequest { renewal_id: Some(..) }` | `ExecutionSnapshot` |
| `events(&handle, after_sequence)` | GET | `/v1/executions/{id}/events?generation={gen}&after={after}&lease={lease}` | — | `BoxBytesStream` (raw, undecoded) |
| `find_missing_blobs(&[BlobDigest])` | POST | `/v1/blobs/missing` | `{"digests":[…]}` | `Vec<BlobDigest>` |
| `upload_blob(&digest, declared_length, stream)` | POST then PUT | `/v1/blobs/prepare`, then `/v1/blobs/{digest}?size={n}` | `{"digest":…,"size_bytes":n}`; then octet-stream body | `()` |
| `download_blob(&digest)` | GET | `/v1/blobs/{digest}` | — | `BoxBytesStream` |
| `artifacts(&id, generation)` | GET | `/v1/executions/{id}/artifacts?generation={gen}` | — | `Vec<ArtifactRecord>` |
| `download_artifact(&artifact)` | GET | `/v1/artifacts/{artifact.artifact_id}` | — | `BoxBytesStream` |
| `create_workspace(&workspace_id, &handle, &manifest)` | POST | `/v1/workspaces` | `{schema_version, workspace_id, handle, manifest}` | `WorkspaceReady` |
| `create_workspace_derived(&workspace_id, &handle, &base_manifest_digest, &patch)` | POST | `/v1/workspaces/derive` | `{schema_version, workspace_id, handle, patch}` | `WorkspaceReady` |

Prose notes on the non-obvious ones:

- **`execute` is the only method that returns a live stream.** It first calls
  `ensure_protocol_compatible()` (`lib.rs:230`), then POSTs and requires the response to be 2xx,
  then reads `eggwork-execution-id` and requires it to parse as an `ExecutionId` **and** to equal
  `handle.execution_id`; otherwise `InvalidResponse`. The generation header
  `eggwork-execution-generation` is sent by the server but is **not** checked by the client.
- **`upload_blob` is a two-phase negotiation.** `POST /v1/blobs/prepare` with the digest and
  declared length; if the response's `upload_required` is `false` the method returns `Ok(())`
  without a body. Otherwise it `PUT`s `RequestBody::from_stream(stream, Some(length))` with
  `content-type: application/octet-stream`, after converting `declared_length` to `usize`
  (overflow → `InvalidResponse`). The length is therefore declared twice — in the prepare body and
  as the query parameter and stream length.
- **`download_blob` and `download_artifact` are identity-checked.** `download_blob` requires
  `eggwork-blob-digest` to equal the requested digest; `download_artifact` requires both
  `eggwork-artifact-id` and `eggwork-artifact-digest` to equal the requested record's. A missing,
  unparseable, or mismatched header is `InvalidResponse` and the body is not handed back. These are
  the anti-mixup checks that make a content-addressed transfer trustworthy across a stream.
- **`create_workspace_derived` performs a local consistency check first:** if
  `patch.base_manifest_digest != *base_manifest_digest` it returns `InvalidResponse` without
  sending anything. The base digest travels only inside the patch, so this is a caller-argument
  guard. The doc comment records the contract: the server never substitutes a different base; a
  missing or expired base is a typed `base_manifest_missing` API error and the caller may retry with
  a full-manifest create.

### `ExecutionStream`

```rust
pub struct ExecutionStream {
    pub handle: ExecutionHandle,
    pub execution_id: ExecutionId,
    events: BoxBytesStream,   // private
}
```

Constructed only by `execute_with_workspace`. Two consumers, both consuming `self`:

- **`into_events()`** → `impl Stream<Item = Result<ExecutionEvent, ClientError>>`, built with
  `stream::try_unfold` over `(BoxBytesStream, BytesMut)`. Per iteration it scans the buffer for
  `\n`, splits the line off, truncates the newline, skips empty lines, then
  `serde_json::from_slice::<ExecutionEvent>` **and** `event.validate()`; any failure is
  `InvalidResponse`. When the buffer holds no complete line it awaits `events.next()` and appends —
  `InvalidResponse`. If the underlying stream ends with a non-empty partial buffer, the tail is
  parsed as one final event (a trailing newline is not required); if it ends empty, the stream ends.

  The bound is `eggwork_core::MAX_EVENT_BYTES` (512 KiB), **not** a client-chosen number and not
  `MAX_EVENT_CHUNK_BYTES`. `Stdout`/`Stderr` carry `Vec<u8>` with no compact wire representation, so
  a 64 KiB chunk serializes as a JSON number array at up to four characters per byte: a chunk sized at
  the protocol maximum produces a ~256 KiB line. A client cap below the encoded ceiling reports
  `InvalidResponse` for a line a conforming node is entitled to emit, and because the stream is
  `try_unfold` that error discards the whole event history. The node refuses to persist an event
  larger than the same constant (`store.rs` `commit_event`), so the two sides agree by construction.
- **`into_bytes()`** → the raw `BoxBytesStream`, preserving transport chunk boundaries, for callers
  that want to do their own framing.

**Backpressure** is pull-based and comes entirely from not polling ahead: `into_events` calls
`events.next().await` only when it needs more bytes, so a slow consumer stops draining the response
body and the bound is carried by the transport. The client allocates no queue, no per-listener
buffer, and no unbounded collection. The one client-side buffer is the `BytesMut`, hard-capped at
`eggwork_core::MAX_EVENT_BYTES`.

**Termination is explicit, not RAII.** There is no `impl Drop for ExecutionStream`. Dropping the
stream (or the `ExecutionStream`) tears down the response body and nothing else. The method
doc-comment is the contract: *"A dropped stream leaves execution running; cancellation is
explicit."* To stop an execution a caller must call `client.cancel(&handle)`. Dropping an
`ExecutionStream` — or abandoning an `into_events()` stream early — is therefore a normal,
non-cancelling event.

### `WorkspaceReady`

```rust
pub struct WorkspaceReady {
    pub schema_version: u16,
    pub workspace_id: WorkspaceId,
    pub execution_id: ExecutionId,
    pub generation: eggwork_core::ExecutionGeneration,
    pub manifest_digest: BlobDigest,
    pub logical_bytes: u64,
}
```

`#[derive(Debug, Clone, Deserialize)]`, returned by both `create_workspace` and
`create_workspace_derived`. The server's `WorkspaceReadyResponse` types `manifest_digest` as
`String`; `eggwork_core::BlobDigest` is `#[serde(try_from = "String", into = "String")]` and
`parse` enforces 64 lowercase-hex characters, so deserializing this struct **validates the digest
syntax** before the caller sees it. The struct has no `validate()` method, and the client does not
call one.

## Request/response contract

All request bodies except the blob PUT are produced by `serde_json::to_vec` with
`content-type: application/json`; a serialization failure maps to `InvalidResponse`. The blob PUT
uses `content-type: application/octet-stream` and a streamed `RequestBody`. The client sends
`schema_version: API_SCHEMA_VERSION` (`1`, `lib.rs:21`) in the execute, control, workspace-create,
and workspace-derive bodies — matching the server's own `API_SCHEMA_VERSION = 1`
(`crates/eggwork-server/src/lib.rs:48`). It is not sent in the two blob request bodies, whose
server counterparts do not declare one either.

Server-side request structs are all `#[serde(deny_unknown_fields)]` (`ExecuteRequest`,
`ControlRequest`, `WorkspaceCreateRequest`, `DeriveWorkspaceRequest`, `FindMissingRequest`,
`PrepareBlobRequest`), so a client that grows a request field gets a typed `400 invalid_request`
rather than a silently ignored field. `ExecuteRequest.workspace_id` is `#[serde(default)]`, matching
the client's `Option`.

**Routing.** The server resolves with `operation_for(method, path)`
(`crates/eggwork-server/src/lib.rs:640`): seven exact literal matches
(`/v1/capabilities`, `/v1/status`, `/v1/executions`, `/v1/blobs/missing`, `/v1/blobs/prepare`,
`/v1/workspaces`, `/v1/workspaces/derive`), then segment-count matching for
`/v1/executions/{id}`, `/v1/executions/{id}/{cancel|renew|events|artifacts}`,
`/v1/blobs/{digest}` (`PUT`/`GET`), and `/v1/artifacts/{artifact_id}`. Anything unmatched is
`404 not_found`. Every path the client constructs matches one of these.

**Status handling.** Success is `response.status().is_success()` (2xx). Anything else goes to
`api_error(status, &mut response)` (`lib.rs:750`), which reads the body, attempts
`serde_json::from_slice::<ApiError>`, and yields:

- parsed → `ClientError::Api { status, code, message }` from the node's `ApiError { schema_version, code, message }`;
- unreadable or unparseable → `ClientError::Api { status, code: "http_error", message: "node returned an error response" }`.

The body's `schema_version` is discarded via `..`. Response `content-type` is never inspected on
the client side.

**Error codes observed** in `dispatch` and the handlers I read: `400 invalid_request`,
`400 invalid_digest`, `401 unauthenticated`, `403 forbidden`, `404 not_found`,
`404 execution_not_found`, `409 base_manifest_missing`, `410 history_expired`,
`416 cursor_ahead`, `500 internal`. This list is not exhaustive — I read `dispatch`, the route
table, the error renderer, and the blob/workspace/event handlers, not all ~6,300 lines of
`eggwork-server`; treat it as the codes a caller must be able to recognise, not the full set.

**Response headers the client trusts** (and the server emits, per
`crates/eggwork-server/src/lib.rs:1335`, `:1457`, `:1458`, `:2660-2663`):
`content-type: application/x-ndjson` plus `eggwork-execution-id` and
`eggwork-execution-generation` on `POST /v1/executions` (status **202**);
`eggwork-blob-digest` on blob download; `eggwork-artifact-id` and `eggwork-artifact-digest` on
artifact download.

**Size limits are now symmetric.** The server bounds its own request bodies
(`MAX_REQUEST_BYTES` 1 MiB, `MAX_BLOB_FIND_REQUEST_BYTES` 64 KiB,
`MAX_WORKSPACE_REQUEST_BYTES` 4 MiB). The client bounds its event line buffer at
`eggwork_core::MAX_EVENT_BYTES` and its buffered responses at
`MAX_BUFFERED_BODY_BYTES` (16 MiB, set via `max_decoded_body_size`). That ceiling covers **both**
buffered-read paths — `decode_json` and `api_error` — and the second is the hotter of the two,
since it runs on every non-2xx response. 16 MiB is generous against the protocol's real maxima (the
largest single buffered reply is an execution snapshot, which the journal caps at 256 events and
512 KiB of encoded events), so it never truncates a legitimate response.

**Timeouts.** Both builders (`new`, `with_eggress`) set
`timeout(transport_timeouts())`: pool 30 s, connect 30 s, write 60 s, read 60 s, and **no** total.
The read deadline resets on every chunk, which is what makes it safe for a long-lived event stream:
a stream that is simply quiet between executions is unaffected, while a node that accepts the
connection and then goes silent still terminates. The unary calls — `get_json` (and therefore
`capabilities`, `status`, `observe`, `observe_generation`, `cancel`, `renew`, `artifacts`), the two
workspace calls, `find_missing_blobs`, and `prepare_blob` — additionally set a 120 s **total**
deadline per request, so a node trickling bytes forever still ends. `execute`, `events`, blob upload,
blob download, and artifact download are deliberately excluded from the total: those transfers are
legitimately long-lived. `every_request_path_carries_a_deadline` pins this split.

## Authentication and TLS

**Presenting identity.** The client authenticates with a client certificate and nothing else. No
request body carries a principal, role, capability, or node name: `ExecutionSpec` is
`{schema_version, command, metadata}`, `ControlRequest` is
`{schema_version, handle, renewal_id}`, the workspace bodies carry an `ExecutionHandle` and
manifest data, and the blob bodies carry digests and sizes. The only bearer secret the client
transmits is `handle.lease_id`, and `eggwork_core` documents that as *bearer authority* omitted
from `ExecutionHandle`'s `Debug`.

**Verifying the node.** `from_pem_files` requires an explicit CA (`ca_certificate_path` replaces
the trust store) and a client cert/key pair; `TlsConfig::builder()` in eggfetch 0.2.0 starts from
`verify_certificate: true`, `verify_hostname: true`, `sni_enabled: true`. The client itself does not
inspect or weaken that: `new` accepts any caller-supplied `TlsConfig` verbatim, including one built
with `danger_accept_invalid_certs(true)`. The explicit-CA trust model is therefore a convention
enforced by `from_pem_files` and by ADR-0002, not a type-level invariant. Because
`ClientBuilder::dialer` supplies only the raw TCP stream, destination TLS, SNI, and certificate
verification stay inside eggfetch on both the direct and routed paths.

**What the node derives, and what the client learns.** The node never trusts a payload. In
`dispatch`, `authenticated_principal`
(`crates/eggwork-server/src/lib.rs:823`) requires `tls.client_authenticated` **and**
`tls.peer_certificates_present`, a non-empty chain of at most 8 certificates each at most 64 KiB,
and then calls `resolver.resolve_verified_leaf(&chain[0])`. Failure is `401 unauthenticated`
("verified client identity required"). Authorization runs next, on the transport-derived principal,
before any handler and therefore before any admission or process side effect
([ADR-0002 decision 4](../plans/adrs/ADR-0002-eggstack-transport-and-mtls.md)).

**Failures surfaced as typed errors:** a missing/invalid CA or key file, a TLS handshake failure, an
untrusted or hostname-mismatched node certificate, and a failed peer authentication all arrive as
`ClientError::Transport(HttpError)` — with the underlying `eggfetch_core::Error` intact as the
source. Missing/invalid peer identity and refused authorization are **not** client errors: they
arrive as `ClientError::Api` with `status: 401` / `code: "unauthenticated"` and
`status: 403` / `code: "forbidden"`. A `403 forbidden` is specifically how a lease belonging to a
different principal, or a cross-principal workspace base, is reported — the server deliberately
makes a foreign base look *missing* (`409 base_manifest_missing`) rather than *forbidden*.

## Error model

| Variant | `Display` | Represents | Retryable in principle | Preserved for diagnosis |
|---|---|---|---|---|
| `InvalidEndpoint` | `node endpoint must be an https URL without query, fragment, or credentials` | Construction-time endpoint rejection. | No — the client cannot be built. | Nothing; the reason is in the message but the offending URL is deliberately not echoed. |
| `InvalidRoute` | `Eggress route configuration is invalid` | `eggress-route` only: `from_pproxy_uri` rejected the route expression. | No — construction-time. | Nothing; the route string is not echoed (it may carry proxy credentials). |
| `Transport(HttpError)` | `node transport failed` | DNS/TCP/TLS/timeout/protocol/body-stream failure; also local TLS-config load errors from `From<HttpError>`. | Sometimes (connection-class), **but this client never retries**. | The full `eggfetch_core::Error` as the variant payload and error source. Feature-gated `eggress_error()` can downcast a nested `OutboundConnectError`. |
| `Api { status, code, message }` | `node returned HTTP {status}: {code}` | Any non-2xx response, typed by the node. | Depends entirely on `code` — the client takes no position. | `status`, node `code`, and node `message` as public fields. |
| `InvalidResponse` | `node response was malformed` | Response did not match the expected contract (see below). | Caller's judgement; usually a hard stop for that response. | Nothing beyond the variant. |
| `ProtocolIncompatible` | `node protocol is incompatible` | `ensure_protocol_compatible`: the node's `[min, max]` does not contain `1.0`. | No — retrying the same node cannot help. | Nothing; the node's actual range is not carried. |
| `UnsupportedCapability` | `node does not support a required protocol capability` | Node does not advertise `exec.argv.v1`. | No. | Nothing; the advertised feature list is not carried. |

`InvalidResponse` is produced in every one of these situations: `serde_json::from_slice` failure on
a response body; `serde_json::to_vec` failure on a request body; a missing, unparseable, or
mismatched `eggwork-execution-id` (including the explicit `id != handle.execution_id` check);
a missing or mismatched `eggwork-blob-digest`; a missing or mismatched
`eggwork-artifact-id`/`eggwork-artifact-digest`; a `usize::try_from(declared_length)` overflow on
blob upload; an NDJSON partial line exceeding `eggwork_core::MAX_EVENT_BYTES`; a line that is not a valid `ExecutionEvent`;
and an `ExecutionEvent` that fails `validate()`. Note the last-but-one category is **local**: the
`create_workspace_derived` base-digest mismatch is detected before any request is sent.

**Redaction.** `impl fmt::Debug for ClientError` is hand-written: `Transport` prints
`ClientError::Transport([REDACTED])`, and `Api` prints `status` and `code` but renders `message` as
`"[REDACTED]"`. The `#[error("node returned HTTP {status}: {code}")]` display string omits
`message` entirely, so neither `Debug` nor `Display` can leak node-controlled text. `message`
remains readable through the public field, so this is a presentation-layer control, not
encapsulation. `NodeClient`'s `Debug` reports only the URL *origin* — path, query, fragment, and
userinfo are all dropped by `safe_target_origin`, and an unparseable endpoint renders as
`[INVALID]`.

**The client implements no retry policy.** Even where a variant is retryable in principle
(`Transport` connection-class failures, `409 base_manifest_missing` where the doc-comment explicitly
invites a full-manifest retry), nothing here loops, backs off, re-resolves, or changes target. A
`Transport` error is returned to the caller once.

## Feature flags

| Feature | Default | Turns on | Security / behavioral consequence |
|---|---|---|---|
| *(none)* | yes | Direct `eggfetch-core` transport. `NodeClient::new` / `from_pem_files`. | No Eggress dependency, no `advanced-routing`, no `eggress_error()`. Every connection is a direct TCP+TLS connection to the named endpoint. |
| `eggress-route` | no | `dep:eggress-outbound`, `eggfetch-core/advanced-routing`; enables `with_eggress`, `with_eggress_route`, `ClientError::eggress_error()`, and the private `EggressDialer`/`EggressIo` types. | Changes the **path** to the node (proxy chain, optional 30 s connect timeout, typed route-failure classification) and adds a dependency. Changes **not**: endpoint validation, target identity, SNI, certificate verification, mTLS, or any protocol byte. `advanced-routing` is required because the workspace's `standard-http1` profile does not include it. Failure is fail-closed with no direct fallback. |

Cargo feature unification applies: `eggwork-server`'s dev-dependency on `eggwork-client` requests
`features = ["eggress-route"]`, so test and qualification builds always compile the routed path
regardless of any local `--features` flag. `rustls` is a dev-dependency used only to install the
ring default provider in `eggress_route_tests::init_crypto_provider`; the `rustls` crypto provider
in a real deployment is Eggfetch's concern, not this crate's.

## Test coverage

### `mod security_tests` (`lib.rs:770`, 2 tests, both plain `#[test]`)

- `endpoint_rejects_credentials_and_non_tls_schemes` — asserts `https://controller.example/` is
  accepted; `https://user:password@controller.example` is rejected **and** neither the `Debug` nor
  the `Display` of the resulting error contains `password`; `http://controller.example` is rejected;
  `https://controller.example/?token=secret` is rejected; and
  `safe_target_origin("https://controller.example/prefix/path-token") == "https://controller.example"`,
  proving a path prefix cannot leak through `Debug`.
- `client_error_debug_redacts_remote_message` — builds `ClientError::Api { message: "private-key-material" }`
  and asserts the string appears in neither `Debug` nor `Display`.

### `mod eggress_route_tests` (`lib.rs:801`, gated `#[cfg(all(test, feature = "eggress-route"))]`, 3 `#[tokio::test]`s)

- `socks5_route_round_trips` — `Socks5Upstream` + `eggress_testkit::start_echo_server`, dials via
  `EggressDialer`, writes 13 bytes, reads them back.
- `http_connect_route_round_trips` — same through `HttpConnectUpstream` and an `http://` route.
- `route_failure_does_not_connect_direct_and_preserves_typed_error` — binds a real `TcpListener`,
  configures a deliberately dead route `socks5://127.0.0.1:1`, then asserts the dial returned an
  error with a non-`None` `source()` **and** that the listener accepted zero connections. This is
  the direct no-fallback proof: the failure is terminal, typed, and no direct connection happened.

All three drive `EggressDialer` directly; none constructs a `NodeClient` or performs TLS.

### Honest gaps in this file

- **No end-to-end `NodeClient` test lives here.** mTLS, execution, cancellation, resume, and
  workspace behavior are exercised from `eggwork-server`'s test module, which depends on this crate
  as a dev-dependency.
- **Destination TLS above a routed stream is not asserted here.** `round_trip` is a raw TCP echo.
  The central M003 claim — that Eggfetch still performs destination TLS/mTLS after Eggress
  establishes the route — is not proven by this file, and the "wrong node certificate through a
  successful proxy route" case from [M003 §9](../plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md)
  is not present in it.
- `DialErrorKind` mapping correctness is untested; only "some source is retained" is asserted.
- No test for `into_events` framing: line splitting, empty-line skipping, the 128 KiB cap,
  `event.validate()` rejection, or the trailing-partial-line tail.
- No test for the response identity headers (execution-id mismatch, blob digest mismatch, artifact
  id/digest mismatch) in this file.
- No test for `ensure_protocol_compatible` rejecting an incompatible range or a missing
  `exec.argv.v1`.
- `normalize_endpoint` is not tested against a fragment, a host-less URL, or a multi-slash
  trailing form.
- No test that dropping an `ExecutionStream` leaves the execution running.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
|---|---|---|
| Endpoint must be `https`, have a host, and carry no query, fragment, or credentials | `normalize_endpoint` — `lib.rs:532` | `security_tests::endpoint_rejects_credentials_and_non_tls_schemes` |
| Client `Debug` cannot leak endpoint path, query, or credentials | `safe_target_origin` + `impl Debug for NodeClient` — `lib.rs:94` | same test (origin-equality assertion) |
| Node-supplied error text cannot reach `Debug` or `Display` | `impl Debug for ClientError` — `lib.rs:68`; `#[error(...)]` string — `lib.rs:31` | `security_tests::client_error_debug_redacts_remote_message` |
| Target identity is fixed at construction and cannot broaden | `NodeClient { endpoint, http }`; single `url()` — `lib.rs:89`, `lib.rs:527` | none directly — structural |
| Route changes the path, never the target | `with_eggress` reuses `normalize_endpoint`; `EggressDialer::dial` uses eggfetch's `DialTarget` — `lib.rs:135`, `lib.rs:598` | `eggress_route_tests::*` (path-level, via the dialer) |
| Route failure never falls back direct | one `connect_tcp_timeout_detailed` call; no second client path — `lib.rs:598` | `route_failure_does_not_connect_direct_and_preserves_typed_error` |
| Typed route failure is recoverable without string parsing | `ClientError::eggress_error()` downcast loop — `lib.rs:54` | `route_failure_does_not_connect_direct_and_preserves_typed_error` (asserts a `source` exists) |
| Route credentials never reach route bodies or `Debug` | no route field in `ExecuteRequest`; `Debug` prints only origin | none in this file |
| SOCKS5 and HTTP CONNECT routes work | `EggressDialer` + `EggressIo` — `lib.rs:553` | `socks5_route_round_trips`, `http_connect_route_round_trips` |
| Streamed responses are bound to the request that asked for them | `eggwork-execution-id` == `handle.execution_id`; blob/artifact header equality — `lib.rs:220`, `lib.rs:472`, `lib.rs:511` | none in this file |
| Client buffering is bounded | `eggwork_core::MAX_EVENT_BYTES` partial-line cap; `MAX_BUFFERED_BODY_BYTES` on buffered reads — `lib.rs:656`, `lib.rs:127`, `lib.rs:141-143` | `an_oversized_partial_line_is_still_refused` |
| Every request path has a deadline | `transport_timeouts()` on both builders; `unary_timeout()` total on the 7 unary calls | `every_request_path_carries_a_deadline` |
| Every delivered event is re-validated | `event.validate()` at `lib.rs:651` and `lib.rs:666` | none in this file |
| Framing handles empty lines, split boundaries, and a final partial line | `into_events` newline scan; `ndjson` fixture delivers in 7-byte pieces | `framing_handles_empty_lines_split_boundaries_and_a_final_partial_line` |
| A protocol-maximum chunk is delivered, not rejected | 64 KiB payload encodes to ~256 KiB, past the old 128 KiB cap | `a_maximum_size_chunk_is_delivered_not_rejected` |
| Incompatible nodes are refused before submission | `ensure_protocol_compatible` — `lib.rs:230`, called from `execute` (`lib.rs:196`) and `events` (`lib.rs:375`) | none in this file |
| No `unsafe` | `#![forbid(unsafe_code)]` — `lib.rs:1` | compile-time |

## Review focus

1. **`handle.lease_id` travels in a URL query string.** Core documents it as *bearer authority*
   and redacts it from `ExecutionHandle`'s `Debug`, but `events()` places it in
   `?generation=..&after=..&lease=..` while `cancel`/`renew` put it in a JSON body. Query strings
   are routinely captured by proxy logs, Eggfetch trace output, and any intermediary. The lease
   also has a 30 s dial timeout and no retry, so the exposure window is per-request — but it is a
   bearer secret in a location that is designed to be logged.
2. **No response-size limit on the JSON paths.** `decode_json` calls `response.bytes()`
   uncapped, so `capabilities`, `status`, `observe`, `observe_generation`, `cancel`, `renew`,
   `artifacts`, and both workspace calls are unbounded allocations of node-controlled data. The
   server bounds its own request bodies; the client does not bound its responses. A hostile or
   compromised node can exhaust caller memory on any of those eight paths.
3. **Redirect and retry behavior is an implicit Cargo-level property, not a crate-level one.**
   The workspace's `standard-http1` profile leaves eggfetch's `redirects` and `logical-retry`
   features off, so 3xx is returned as an ordinary response and every request is dispatched once —
   today. Nothing in `eggwork-client` asserts or documents this. Bumping the workspace to
   eggfetch's `http1` alias feature would silently enable both redirect following and logical
   retry, and a redirect hop would carry the client certificate to whatever origin the node names.
   There is no test that would fail.
4. **`ensure_protocol_compatible` is not universal.** It guards `execute` and `events` only.
   `cancel`, `renew`, `observe`, blob, artifact, and workspace calls are sent to a node whose
   protocol range has already been shown to exclude `1.0`. That is defensible for read-only calls
   and questionable for fenced control operations.
5. **Verification is a convention, not an invariant.** `new` accepts any `TlsConfig`, including
   one built with `danger_accept_invalid_certs(true)`, and `TlsConfig::builder()`'s default trust
   store is `NativeWithWebPkiFallback` — so a caller who uses `new` with an otherwise-default
   builder gets public CA roots rather than the private CA the design intends. Nothing in the
   crate detects or reports this.
6. **Local construction failures are misclassified as transport failures.** `from_pem_files`
   surfaces an unreadable CA file, a malformed PEM, or a bad key through
   `From<HttpError> for ClientError` as `Transport`, indistinguishable by variant from a network
   failure. A `ClientError` variant for TLS-material loading would separate "I cannot start" from
   "the node is unreachable".
7. **`InvalidResponse` is overloaded for a purely local argument error.** The
   `create_workspace_derived` base-digest mismatch is detected before any bytes leave the process,
   yet it reports "node response was malformed" — naming a node that was never contacted.
8. **`observe()` sends no generation.** `observe(&id)` hits `/v1/executions/{id}` with no
   `generation` query, so it reads whatever the node currently considers for that id, while
   `observe_generation` is explicit. Confirm the implicit form is intended for a client whose
   defining property is explicitness, and that it cannot silently read a newer generation.
9. **`ExecutionStream` termination is caller-discipline, not RAII.** There is no `Drop` impl, no
   cancellation token, and no guard type. Dropping the stream, or breaking out of an `into_events()`
   loop after the first event, leaves the execution running and unreferenced from the client's
   perspective. A caller that loses track of a stream has no compile-time or runtime signal, and
   the only recovery is a remembered `handle`.
10. **`url()` is string concatenation.** Every interpolated segment is a validated `eggwork_core`
    ID (`ExecutionId::new`, `ArtifactId::new`, `BlobDigest::parse`) and every other path is a
    compile-time literal, so no injection is reachable — but that safety is a property of how
    callers construct IDs, not of `url()`. `Url::join` or explicit percent-encoding would make it
    structural and would also survive a future path template.
11. **`ApiError.schema_version` is discarded.** `api_error` destructures with `..`, so a node
    replying with an incompatible error schema is indistinguishable from a conforming one, and the
    client will surface its `code` as authoritative.
12. **The `DialErrorKind` mapping is coarse and untested.** `Dns`, `ConnectionRefused`,
    `NetworkUnreachable`, and `HostUnreachable` all collapse to `DialErrorKind::Connection`, and
    everything unmatched becomes `Other`. A caller that only reads the Eggfetch-level kind gets a
    materially less precise diagnosis than `eggress_error()` would give.

## Related

- [overview.md](overview.md) — crate map, one-way dependency graph, and why `eggwork-client` is a dev-dependency of the server.
- [core-domain.md](core-domain.md) — `eggwork_core` types, ID validation, `ExecutionHandle`, `BlobDigest`, `ExecutionEvent::validate`.
- [server-node.md](server-node.md) — the node side of the same contract: `dispatch`, `operation_for`, admission, and error rendering.
- [protocol.md](protocol.md) — protocol layering intent and the pre-wire status of the schema.
- [domain.md](domain.md) · [execution-ownership.md](execution-ownership.md) — normative vocabulary and the process-ownership rule this client also obeys (it spawns nothing).
- [ADR-0001: Fixed-Target, Scheduler-Free Remote Execution](../plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md) — why the client holds no placement policy.
- [ADR-0002: Eggstack HTTP Transport with Transport-Derived mTLS Identity](../plans/adrs/ADR-0002-eggstack-transport-and-mtls.md) — HTTP/1.1 + mTLS + optional Eggress, and the no-silent-retry-direct rule.
- [ADR-0003: Execution Idempotency, Leases, and Fencing](../plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md) — what `ExecutionHandle`'s `generation` and `lease_id` fence.
- [Control-plane roadmap](../plans/subsystems/control-plane-protocol-roadmap.md) — durable invariants 1 and 10, and the M001/M002/M003 milestones.
- [M001 — Authenticated Fixed-Target Execution](../plans/implementation/control-plane-protocol/001-authenticated-fixed-target-execution.md) — the original client surface, and the "no execute-anywhere" rule.
- [M003 — Eggress Route Adapter and Protocol Compatibility Hardening](../plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md) — the route adapter, feature shape, and typed-failure requirements implemented here.
- [M003 closure evidence](../plans/closure/control-plane-protocol/003-status.md) — feature graph, TLS-ownership evidence, and the typed failure mapping matrix.
- [`plans/000-long-term-specification.md`](../plans/000-long-term-specification.md) — §15 transport/Eggstack ownership and the CodeGG → `eggwork-client` → caller-selected `NodeId` chain.
