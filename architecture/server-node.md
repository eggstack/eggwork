# eggwork-server — node service and control plane

`crates/eggwork-server/src/lib.rs` (6,280 lines) is the crate root and the only place where a remote
peer is turned into work. It owns the mTLS-authenticated HTTP surface, the operation taxonomy, the
authorization gate, the per-execution `ExecutionRecord` lifecycle, local admission, lease
enforcement, event fan-out, and the in-process garbage-collection entry point. Everything it calls
into — the journal (`store.rs`), blob/workspace/artifact stores, the CLI (`operations.rs`), the
deployment engine (`deployment.rs`) — is a sibling covered elsewhere; this document covers only the
node-facing control plane defined here.

## Responsibility boundary

**Owns**

- The `NodeServer` lifecycle: build TLS runtime, open stores, recover, bind, serve, shut down.
- The `Operation` taxonomy and the `OperationRequest { operation, resource }` shape used for
  authorization.
- Peer identity resolution from a transport-verified TLS leaf and the `Authorizer` gate.
- The fixed route table (`operation_for`) and uniform response shaping.
- Local admission: drain state, `max_active_executions`, and requirement-vs-capability checks.
- Per-execution state (`ExecutionRecord`), lease state (`LeaseState`), event broadcast, and the
  `publish` / `publish_terminal` transition funnel.
- Runner task supervision (`run_execution`) and `RunnerResult`/`RunnerError` → core outcome mapping.
- Local metrics increments and the live `NodeServer::collect_garbage` pass.

**Does not own**

- **No queue.** There is no pending work list. An execution either starts a task immediately or is
  rejected.
- **No worker selection.** The node is a fixed target; it never picks *where* a workload runs beyond
  "here, if I have a permit".
- **No placement.** Scheduling, node selection, and retry are the controller's problem.
- **No retry.** Every rejection is terminal for that request. A failure is reported once.
- **No scheduling.** `max_active_executions` is a hard semaphore, not a queue depth.
- **Local admission only**, and it is **immediate and bounded**: drain check, three capability
  checks, one `try_acquire_owned`. No awaiting, no waiting for a slot, no backpressure queue.
- No GC scheduling daemon: `collect_garbage` is a bounded, explicitly-invoked maintenance pass
  (`lib.rs:459-460` — "no remote GC route is exposed").
- No TLS material parsing; `operations.rs` builds the config and certs.

## Configuration and startup

`NodeConfig` (`lib.rs:166-179`) is a plain cloneable struct, all fields `pub`:

| Field | Type | Constraint / role |
| --- | --- | --- |
| `node_id` | `NodeId` | Reported in `NodeStatus`. Not validated here. |
| `bind` | `SocketAddr` | Passed to both `RuntimeConfig` and `Server`; `local_addr()` reflects the real bound port. |
| `execution_root` | `PathBuf` | Default working root when no `workspace_id` is supplied (`lib.rs:1632-1634`). |
| `database_path` | `PathBuf` | Opens the execution store, the `.lock` state file, the `.drain` marker, and derives `*.artifacts.sqlite` (`lib.rs:323`). |
| `blob_root` / `blob_quota_bytes` | `PathBuf` / `u64` | `BlobStore::open`; quota failure surfaces as `NodeStartError::EggServe`. |
| `workspace_root` / `workspace_quota_bytes` | `PathBuf` / `u64` | `WorkspaceManager::open`. |
| `max_active_executions` | `u32` | Must be `> 0` and `<= Semaphore::MAX_PERMITS`, else `InvalidLimit` (`lib.rs:302-306`). |
| `lease_ttl` | `Duration` | Must be non-zero, else `InvalidLease` (`lib.rs:312-314`). |
| `tls` | `TlsServerConfig` | Must have `ClientAuthMode::Required` and a default identity, else `TlsPolicy`. |

`NodeStartError` (`lib.rs:140-152`) has five variants: `TlsPolicy`, `EggServe(String)`,
`InvalidLimit`, `InvalidLease`, `AlreadyRunning`.

**Why the hand-written `Debug` re-redacts `EggServe`.** Every other variant is a unit variant, so a
derived `Debug` would be harmless. `EggServe(String)` carries an underlying Eggserve/IO error whose
`Display` may embed filesystem paths (key/certificate/CA paths, database paths, bind errors). The
manual `impl Debug` (`lib.rs:154-164`) therefore prints `NodeStartError::EggServe([REDACTED])` and
discards the payload, while `Display` keeps the real text for direct error reporting. The split
matters because `{:?}` is what ends up in panic messages, `unwrap`/`expect` chains, `#[derive(Debug)]`
structs, and test assertion output — surfaces that are routinely logged verbatim. The
`tls_configuration_diagnostics_redact_certificate_paths` test (`lib.rs:3083`) and
`secret_sentinels_stay_out_of_error_and_debug_surfaces` (`lib.rs:6104`) pin this.

**`AlreadyRunning` and the exclusive state lock.** `acquire_state_lock` (`lib.rs:540-556`) opens
`<database_path>.lock` with mode `0o600` on Unix and takes an exclusive `fs2` lock via
`try_lock_exclusive`. A failure of the open is an `EggServe` error; a failure of the *lock* alone maps
to `AlreadyRunning` ("another node process already owns this state directory"). The `File` handle is
stored in `NodeServer._state_lock` (`lib.rs:229`) so the OS lock is held for the server's whole
lifetime and released only on drop. This is the guard against two processes racing on the same
SQLite journal and the same drain marker.

**`NodeServer::start` order of operations** (`lib.rs:296-422`):

1. Validate `max_active_executions` → `InvalidLimit`.
2. Validate TLS policy → `TlsPolicy`. **Fail-closed**: a node with `ClientAuthMode::Optional` or no
   default identity never starts, rather than starting unauthenticated.
3. Validate `lease_ttl` → `InvalidLease`.
4. `acquire_state_lock` → `AlreadyRunning`.
5. Open `ExecutionStore`, `BlobStore`, `WorkspaceManager`, `ArtifactStore` (artifact DB path is
   `database_path.with_extension("artifacts.sqlite")`).
6. `store.recover()` — any non-terminal row is recovered to a terminal state; each recovery bumps
   `terminal_interrupted` by the recovered count (`lib.rs:326-332`).
7. `recover_retention`, `recover_manifest_retention` with a horizon of *now + DEFAULT_RETENTION_MILLIS*
   (so nothing expires purely because the process was down), then `reconcile_manifests(…, 1024)`.
8. Seed `draining` from the persistent marker via `operations::is_persistently_draining(&drain_path)`
   where `drain_path = database_path.with_extension("drain")`.
9. `runner.execution_capabilities().await` → `server_capability_features(...)`.
10. Hydrate `executions` from `store.load_all()` — every recovered record gets `finished: true`, an
    empty `lease_hash`, and a lease already expired (`expires_at: Instant::now()`).
11. Build `NodeState`; `RuntimeConfig::builder().max_request_body_bytes(blob::MAX_BLOB_BYTES)`,
    `tls_config(config.tls.into_server_config())`, and `tls_expose_peer_chain(true)` — the last is
    what makes `request.context().connection().tls.peer_certificate_chain` available to
    `authenticated_principal`.
12. Build the server, `start_with_service(NodeHttpService)`, then `handle.ready().await` so a
    successful `start` means the listener is live, not merely scheduled.

**Capabilities are derived once, at start.** `static_capability_features()` (`lib.rs:563-578`) is a
fixed list of protocol/transport/workspace/artifact features plus `network.unrestricted.v1`.
`server_capability_features` (`lib.rs:583-592`) merges the runner-reported dynamic features, then
sorts and dedups. Both `Route::Capabilities` and `node_status().capabilities` route through
`node_capabilities_for` (`lib.rs:595-604`) — the doc comment at `lib.rs:580-582` states this MUST
stay true so a node never reports two different answers to the same question.
`node_capabilities_for` also hard-pins the protocol range to `{1.0 … 1.0}`.

## Authentication

```rust
pub trait PeerPrincipalResolver: Send + Sync + 'static {
    fn resolve_verified_leaf(&self, leaf_der: &[u8]) -> Option<NodePrincipal>;
}
```

`FingerprintPrincipalResolver` (`lib.rs:95-119`) holds `HashMap<String, PrincipalId>` and keys it by
`FingerprintPrincipalResolver::fingerprint(leaf_der)`, which is `hex::encode(Sha256::digest(leaf_der))`
— lowercase hex SHA-256 over the leaf certificate's DER. `operations.rs:313` lowercases configured
fingerprints with `to_ascii_lowercase()` before insertion, so lookup is case-normalized on both
sides. `fingerprint_mapping_uses_leaf_der_only` (`lib.rs:2924`) pins that the whole *DER* is
hashed, not the PEM text, not the CN, not a subject-key-id.

`authenticated_principal` (`lib.rs:823-833`) gates in order:

1. TLS context must exist.
2. `tls.client_authenticated` must be true **and** `tls.peer_certificates_present` must be true.
3. The peer chain must be non-empty, at most 8 certificates, and no certificate over 64 KiB
   (bounds hashing cost and rejects absurd chains).
4. Only `chain[0]` — the leaf — is resolved.

**Why transport-verified material, not caller assertion.** The principal is derived from bytes the
TLS stack already validated against the configured trust anchor. There is no request field, header,
or body that can name a principal. The `request_payload_cannot_supply_authoritative_principal` test
(`lib.rs:2993`) is the executable statement of this: a payload that tries to assert a principal is
ignored. Trust is anchored in the handshake, so a compromised-but-untrusted client cannot reach the
resolver at all, and a trusted client cannot become a different principal by asking.

**Unknown fingerprint.** `resolve_verified_leaf` returns `None`, `authenticated_principal` returns
`None`, and `dispatch` answers `401 unauthenticated` / "verified client identity required", bumping
`rejected_unauthenticated`. An unknown-but-CA-valid certificate is therefore *indistinguishable from*
no certificate: both take the same 401 path, and no oracle distinguishes "unknown identity" from
"unauthenticated". The tests `mtls_execution_streams_output_and_missing_certificate_is_rejected`
(`lib.rs:3724`) and `cross_principal_execution_fencing_without_lease_oracle` (`lib.rs:5766`) cover
both halves.

## Authorization

`Operation` (`lib.rs:56-69`) is `Copy + Hash`, 11 variants:

| Variant | Permits |
| --- | --- |
| `Capabilities` | `GET /v1/capabilities` — read the advertised feature/limit set. |
| `Status` | `GET /v1/status` — node id, drain flag, active count, capabilities. |
| `Execute` | `POST /v1/executions` — start work; also re-checked per execution id and per workspace id. |
| `Observe` | `GET /v1/executions/{id}` — read the current snapshot. |
| `Cancel` | `POST /v1/executions/{id}/cancel` — request cancellation. |
| `Renew` | `POST /v1/executions/{id}/renew` — extend the lease. |
| `Events` | `GET /v1/executions/{id}/events` — read/stream the event journal. |
| `BlobRead` | `POST /v1/blobs/missing` and `GET /v1/blobs/{digest}`. |
| `BlobWrite` | `POST /v1/blobs/prepare` and `PUT /v1/blobs/{digest}`. |
| `WorkspaceCreate` | `POST /v1/workspaces` and `POST /v1/workspaces/derive`. Note: derive reuses `WorkspaceCreate`; there is no separate `WorkspaceDerive` operation. |
| `ArtifactRead` | `GET /v1/executions/{id}/artifacts` and `GET /v1/artifacts/{artifact_id}`. |

`OperationRequest { operation, resource: Option<ResourceId> }` (`lib.rs:79-83`) pairs the operation
with `ResourceId::Execution | Blob | Workspace | Artifact` (`lib.rs:71-77`).

**`authorize` vs `authorize_request`.** `Authorizer` (`lib.rs:121-129`) has one required method,
`authorize(principal, operation) -> bool`, and one defaulted method,
`authorize_request(principal, request) -> bool`, whose default body is exactly
`self.authorize(principal, request.operation)`. A policy that is operation-wide stays valid by
implementing only `authorize`; a policy that scopes by authenticated ID overrides
`authorize_request`. `dispatch` always calls `authorize_request` (`lib.rs:744-747`), so the
resource-aware path is the only one actually on the request path — the coarse method is reachable
only through the default impl and through direct internal calls. Handlers that learn a resource
*from the body* (which `resource_for_path` cannot see) re-authorize with `authorize_resource`
(`lib.rs:808-821`): `blob_missing` checks every digest in the batch (`lib.rs:928-941`),
`blob_prepare` the prepared digest (`lib.rs:983-994`), `workspace_create`/`workspace_derive` the
workspace id, and `execute` the execution id and, if present, the workspace id
(`lib.rs:1518-1536`).

**Closure blanket impl.** `impl<F> Authorizer for F where F: Fn(&NodePrincipal, Operation) -> bool
+ Send + Sync + 'static` (`lib.rs:131-138`) lets tests and embedders express a policy as a closure
without declaring a type. Because the bound is on the *function* signature, such an authorizer can
only ever see the operation — never the resource. Resource scoping requires a named type, as in
`derived_workspace_denied_and_oversized_bodies_are_typed` (`lib.rs:4499`) and
`resource_aware_authorizer_can_scope_blob_capability` (`lib.rs:3039`).

**How `GrantAuthorizer` realizes per-principal grants.** `GrantAuthorizer(HashMap<PrincipalId,
HashSet<Operation>>)` (`operations.rs:332-340`) implements only `authorize`:
`self.0.get(&principal.id).is_some_and(|allowed| allowed.contains(&operation))`. It is built by
`principals()` (`operations.rs:305-325`), which walks the configured clients, lowercases each
`certificate_sha256` into the `FingerprintPrincipalResolver` table, and maps each configured
operation *name* to an `Operation` through `operation(name)` (`operations.rs:342-357`) — note
`filter_map`, so an unrecognized operation name in configuration is **silently dropped** rather
than rejected, shrinking that client's grant set. Absent principal ⇒ `None` ⇒ `false` (deny by
default). Because it does not override `authorize_request`, grant-based deployments are strictly
operation-wide; the `resource_for_path` enrichment in `dispatch` has no effect for them. The
`GrantAuthorizer` has no cross-principal or cross-execution scoping: per-execution ownership is
enforced later, in the store, by comparing `existing.principal_id` (`lib.rs:1677`, `2044`, `2267`).

**Authorization runs BEFORE resource resolution.** In `dispatch` the order is: route lookup (728) →
authentication (732) → `authorize_request` (744) → handler (755). `resource_for_path` (`lib.rs:783-806`)
parses ids **from the URL path only** and returns `None` on a malformed id, so it never touches the
store. Handlers then resolve existence — `store.load_snapshot_for_principal` in `observe`,
`store.lookup` in `control`/`events_route`, `workspaces.resolve` in `execute`. The implication is
deliberate: a client that is not authorized for an operation learns nothing about whether the target
execution, blob, or artifact exists, because it is rejected with a uniform `403` before any lookup
happens. Where a handler must do a *second* resource check (body-derived ids), an unauthorized
result is still a bare `403` and never leaks existence. The residual disclosure to note: the
operation-wide gate is uniform across resources, so a principal granted `BlobRead` can use
`POST /v1/blobs/missing` to probe digest presence — presence is data the grant explicitly confers.
`Route::BlobInvalidDigest` (`lib.rs:767-771`) is reached only *after* authorization, and returns a
distinct `400 invalid_digest`, so an unauthorized caller cannot use it to distinguish "malformed"
from "well-formed but denied".

## Request dispatch

`dispatch` (`lib.rs:721-777`) is the single entry point; `NodeHttpService::call` (`lib.rs:289-292`)
clones the `NodeState` and delegates to it.

**Route table** (`operation_for`, `lib.rs:640-700`):

| Method | Path | Operation | Route variant |
| --- | --- | --- | --- |
| GET | `/v1/capabilities` | `Capabilities` | `Capabilities` |
| GET | `/v1/status` | `Status` | `Status` |
| POST | `/v1/executions` | `Execute` | `Execute` |
| POST | `/v1/blobs/missing` | `BlobRead` | `BlobMissing` |
| POST | `/v1/blobs/prepare` | `BlobWrite` | `BlobPrepare` |
| POST | `/v1/workspaces` | `WorkspaceCreate` | `WorkspaceCreate` |
| POST | `/v1/workspaces/derive` | `WorkspaceCreate` | `WorkspaceDerive` |
| PUT | `/v1/blobs/{digest}` | `BlobWrite` | `BlobUpload(digest)` |
| PUT | `/v1/blobs/{not-a-digest}` | `BlobWrite` | `BlobInvalidDigest` |
| GET | `/v1/blobs/{digest}` | `BlobRead` | `BlobDownload(digest)` |
| GET | `/v1/blobs/{not-a-digest}` | `BlobRead` | `BlobInvalidDigest` |
| GET | `/v1/executions/{id}` | `Observe` | `Observe(id)` |
| POST | `/v1/executions/{id}/cancel` | `Cancel` | `Cancel(id)` |
| POST | `/v1/executions/{id}/renew` | `Renew` | `Renew(id)` |
| GET | `/v1/executions/{id}/events` | `Events` | `Events(id)` |
| GET | `/v1/executions/{id}/artifacts` | `ArtifactRead` | `ArtifactList(id)` |
| GET | `/v1/artifacts/{artifact_id}` | `ArtifactRead` | `ArtifactDownload(artifact_id)` |

Anything else ⇒ `404 not_found` + `rejected_route`. Matching is a literal-string match for the
fixed paths, then a segment split requiring exactly 4 or 5 segments with `["", "v1", …]` prefixes
and, for exec paths, a *validating* `ExecutionId::new(parts[3])` / `ArtifactId::new(parts[3])`. A
malformed id in a 4- or 5-segment exec/artifact path therefore returns `404`, not `400` — there is
no `Route::InvalidExecutionId`. `Route` carries `BlobInvalidDigest` but no execution equivalent, so
the digest and id validation asymmetry is intentional: the digest shape is part of the blob
protocol contract (`BlobDigest::parse`), while `ExecutionId`/`ArtifactId` are opaque.
`route_contract_is_fixed_target_and_versioned` (`lib.rs:2934`) pins this table.

**Body policy** is decided earlier, in `Service::request_body_policy` (`lib.rs:252-287`), by
`(method, path)`:

| Routes | Policy | Cap |
| --- | --- | --- |
| `POST /v1/blobs/missing`, `POST /v1/blobs/prepare` | `Buffer` | `MAX_BLOB_FIND_REQUEST_BYTES` = 64 KiB |
| `POST /v1/workspaces`, `POST /v1/workspaces/derive` | `Buffer` | `MAX_WORKSPACE_REQUEST_BYTES` = 4 MiB |
| `PUT /v1/blobs/{parseable digest}` | `Stream` | `blob::MAX_BLOB_BYTES` |
| `POST /v1/executions` | `Buffer` | `MAX_REQUEST_BYTES` = 1 MiB |
| `POST …/cancel`, `POST …/renew` | `Buffer` | `MAX_REQUEST_BYTES` = 1 MiB |
| everything else | `Reject` | — |

Two layers therefore bound each body: the Eggserve runtime's declared policy (also why
`RuntimeConfig` is set to `max_request_body_bytes(blob::MAX_BLOB_BYTES)`), and the handler's own
`read_limited_body` call. `request_body_policy` matches `PUT /v1/blobs/…` by *re-parsing* the
digest and falls through to `Reject` for an unparseable one — so a `PUT` with a bad digest is
rejected at the transport layer before `Route::BlobInvalidDigest` can render its `400`; the route
arm is effectively defensive/dead for `PUT` (it remains reachable for the *policy* difference only
if the runtime does not pre-reject). The streaming arm exists precisely so blob uploads are not
buffered in memory.

`read_limited_body` (`lib.rs:845-858`) pre-allocates
`min(declared_length.unwrap_or(0), limit)` and, per chunk, fails closed with
`saturating_add(chunk.len()) > limit`. Callers map `Err(())` to `413 request_too_large` with a
route-specific message. The runtime cap and the handler cap agree, so the 413 surfaces from one or
the other without a size oracle.

**Uniform error rendering.** `error_response(status, code, message)` (`lib.rs:2689-2698`) is
`json_response` over an `ApiError { schema_version, code, message }`. Almost nothing escapes as an
`Err(ServiceError)`: store failures become `ServiceError::internal("execution store unavailable")`
(uniform, no detail), and handler-level failures become typed status responses. Where a response
builder can fail, the fallback is `error_response(500, "internal", "internal error")`
(`lib.rs:2666`, `2686`, `1300`) rather than a panic.

## Local admission

Admission lives entirely inside `execute` (`lib.rs:1485-1884`) and is a straight-line sequence of
fail-fast checks followed by a non-blocking permit acquisition.

**Order:**

1. Body read (1 MiB) → `413`; JSON parse → `400`; `wire.schema_version != 1` → `426`
   (`lib.rs:1511-1517`).
2. Resource-scoped `Execute` authorization on `execution_id`, and on `workspace_id` if present
   → `403` (`lib.rs:1518-1536`).
3. `ExecutionGeneration::new(...)` invalid → `400` (`lib.rs:1537-1543`).
4. `spec.schema_version != 1` → `426`; `spec.validate()` error → `400`; both bump
   `rejected_invalid` (`lib.rs:1544-1560`).
5. Declared outputs without a workspace → `400 workspace_required` (`lib.rs:1561-1568`).
6. **Drain check** (in-memory `draining` **or** the persistent marker) → `503 draining`, metric
   `rejected_draining` (`lib.rs:1569-1578`).
7. **Capability checks** — `check_isolation_supported`, `check_resources_supported`,
   `check_network_supported` → `409 capability_mismatch`, metric `rejected_capability`
   (`lib.rs:1579-1602`).
8. Request digest via `eggwork_core::request_digest_with_workspace` → `400` on failure
   (`lib.rs:1605-1616`).
9. Workspace resolution (below), `RunnerRequest::from_spec` → `400` on failure.
10. **Idempotency lookup** (see next section) — may return an existing execution instead.
11. **Drain re-check** (`lib.rs:1702-1711`) — a *second* identical drain test, after the store
    lookup, before the permit.
12. **Permit**: `state.permits.clone().try_acquire_owned()` → on `Err`, metric `rejected_busy` and
    `503 busy` / "node execution limit reached" (`lib.rs:1712-1718`).
13. **Map pressure**: if `executions.len() >= MAX_RECENT_EXECUTIONS` (1024), evict terminal records
    and, if still at/over the cap, `503 storage_exhausted` / "recent execution capacity reached"
    (`lib.rs:1719-1740`).
14. `workspaces.mark_active` (if a workspace) → `409 workspace_not_ready`, releasing the permit.
15. `store.reserve(...)` — the durable reservation; `Created` bumps `executions_accepted`.

**Tri-state semantics as applied.** Core's `Requirement<T>` is exactly three states:
`NotRequested | BestEffort(T) | Required(T)` (`eggwork-core/src/lib.rs:353-357`), and
`IsolationRequirement` mirrors it as `None | BestEffort | Required` (`eggwork-core/src/lib.rs:446`).
The node treats them as follows:

- `check_isolation_supported` (`lib.rs:606-618`): `None` and `BestEffort` are *always* accepted;
  only `Required` demands `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY`
  (`isolation.landlock.workspace-rw.v1`). Best-effort is permitted to degrade at run time, which
  `failed_result` reports honestly as `SandboxResult::NotApplied { reason: "best-effort sandbox
  setup was unavailable" }` (`lib.rs:2542-2544`).
- `check_resources_supported` (`lib.rs:620-634`): each of `memory_bytes`/`cpu_millis`/`pids` gates
  on `matches!(requirement, Requirement::Required(_))` only; `Required` requires the corresponding
  `resources.cgroups-v2.{memory,cpu,pids}` feature. `failed_result::requested` (`lib.rs:2521-2528`)
  then reports any non-`NotRequested` dimension as `NotApplied { reason: "requested resource
  enforcement failed before execution" }` — never as "applied".
- `check_network_supported` (`lib.rs:636-638`): accepts `NetworkRequirement::Unrestricted` only.
  The `static_capability_features` doc comment (`lib.rs:558-562`) is explicit that this is a
  deliberate refusal to fake capability: Eggwork has no network-restriction backend, so the node
  advertises `network.unrestricted.v1` and the gate rejects `Disabled`/`AllowListed` with
  `capability_mismatch` rather than silently downgrading. The alternative would be advertising
  support the enforcement path cannot deliver.

`Required` is therefore a *refusal* signal, not a degradation signal: no request whose enforcement
would be skipped is ever admitted. The invariant to hold is: **a node never advertises a capability
its enforcement path would refuse.** It is enforced structurally by deriving the advertised set from
`runner.execution_capabilities()` at start (`lib.rs:351-352`) rather than configuring it by hand,
and it is pinned by `static_capability_features_do_not_advertise_unsupported_network_modes`
(`lib.rs:5704`), `admission_helpers_keep_required_capability_gating_active` (`lib.rs:5719`),
`required_landlock_rejects_with_capability_mismatch_when_helper_is_missing` (`lib.rs:4998`),
`required_resource_admission_rejects_with_capability_mismatch_when_dimension_unavailable`
(`lib.rs:5144`), and `disabled_or_allowlisted_network_requests_are_rejected_with_capability_mismatch`
(`lib.rs:5355`).

**What the caller learns.** Acceptance is `202 Accepted` — an `application/x-ndjson` event stream
from `event_response(state, id, generation, 0, record)` (`lib.rs:1883`) that begins with the
`Accepted` event already published at `lib.rs:1861-1867`. The caller never gets a bare "yes"; it
gets the execution's own event stream starting at sequence 0. Rejection is a typed
`ApiError` with a `rejected_*` metric alongside it, so "busy", "draining", "capability mismatch",
and "storage exhausted" are all distinguishable *by the authorized caller* while remaining opaque to
an unauthorized one.

**What happens at the limit.** `try_acquire_owned` never awaits. At the cap the node does not queue,
does not park the request, and does not hold the request body open; it answers `503 busy`
immediately. Fairness is therefore not provided: under saturation a later arrival can win a permit
released by an earlier one. The permit is moved into the spawned `run_execution` task
(`lib.rs:1873-1882`) and held as `_permit` for the task's whole life, so a slot is released exactly
when the execution task ends — including on panic, since the permit's `Drop` runs during unwind.

**Drain-check race.** The drain flag is checked twice (before and after the store lookup) but both
checks read an `AtomicBool` plus a *file* (`operations::is_persistently_draining`). There is no lock
binding the check to the permit acquisition, so a `set_draining(true)` landing between the second
check and `try_acquire_owned` will still admit one more execution. This is bounded to a small number
of in-flight admissions and is why `set_draining` also writes the persistent marker
(`lib.rs:428-432`) — a process restart re-reads the marker at `lib.rs:348-350`.
`concurrent_drain_admission_outcomes_are_closed` (`lib.rs:6180`) and
`drain_rejects_new_admission_while_terminal_records_survive` (`lib.rs:5906`) assert the *outcome
set* is closed to {accepted, rejected}, not that the window is zero. `is_draining` (`lib.rs:434-437`)
and `node_status` (`lib.rs:835-843`) both OR the in-memory flag with the marker so the reported
state is never *more* permissive than the enforced one.

## Idempotency and reservation

The idempotency key is the **canonical request digest**:
`eggwork_core::request_digest_with_workspace(&spec, workspace_id)` (`lib.rs:1605-1607`) returns
`(canonical_version, digest)`. The workspace id is folded in, so the same spec against a different
workspace is a different request. The digest is never taken from the request body; it is recomputed
by the node from the decoded spec.

`execute` first does a **pre-reservation lookup**: `store.lookup(id, generation)` (`lib.rs:1664`).
If a row exists:

- `existing.canonical_version != canonical_version` ⇒ `409 canonicalization_version_mismatch`
  (`lib.rs:1670-1676`). A digest computed under a different canonicalization rule set is refused
  outright rather than compared — comparing across versions would be meaningless.
- `existing.principal_id != principal_id` ⇒ `403 forbidden` / "execution belongs to another
  principal" (`lib.rs:1677-1683`).
- `existing.digest != digest || existing.lease_token_hash != lease_hash` ⇒
  `409 execution_identity_conflict` (`lib.rs:1684-1690`).
- otherwise: an `ExistingExecution` — the in-memory `Arc<ExecutionRecord>` is reused if its
  generation matches, else a `recovered_record` is synthesized from the stored snapshot
  (`lib.rs:1691-1698`), and the handler returns `event_response(state, id, generation, 0, record)`
  (`lib.rs:1700`). A repeat submit *attaches to the existing execution's event stream*; it does not
  start a second one.

The `lease_hash` comparison is what makes the digest safe as a shared key: the same spec submitted
twice under *different* `LeaseId`s is a conflict, not a duplicate. Two callers cannot collapse onto
one execution by guessing its id — the record is bound to both the spec digest and a hash of the
lease token.

The durable path is `store.reserve(snapshot, canonical_version, digest, principal_id, lease_hash,
lease_expires_unix_ms)` (`lib.rs:1763-1773`), which returns `ReserveResult`:

| `ReserveResult` | Response | Metric |
| --- | --- | --- |
| `Created` | `202` event stream; spawn `monitor_lease` + `run_execution` | `executions_accepted` |
| `Existing(snapshot)` | `202` event stream for the existing execution (permit released) | — |
| `Conflict` | `409 execution_identity_conflict` | — |
| `StaleGeneration` | `409 generation_mismatch` / "stale or out of sequence" | — |
| `StorageFull` | `503 storage_exhausted` / "execution identity capacity reached" | `rejected_storage` |

Every non-`Created` arm calls `workspaces.mark_terminal(workspace_id, now + DEFAULT_RETENTION_MILLIS)`
first, so a workspace that was materialized for an execution that will not run is not left active —
otherwise its blobs would stay pinned forever. The same release runs on the `store.reserve` *error*
path (`lib.rs:1776-1787`).

**Generation/lease relationship.** `lease_expires_unix_ms` is computed once as
`store::unix_millis().saturating_add(state.lease_ttl.as_millis().min(i64::MAX as u128) as i64)`
(`lib.rs:1661-1662`) — the `.min(i64::MAX as u128)` clamp prevents a pathological TTL from wrapping
the `i64` conversion. The store persists it alongside the reservation, and the *in-memory* monitor
uses a separate `tokio::time::Instant` deadline (`lib.rs:1903`); the two clocks are deliberately
independent (monotonic for the timer, wall clock for the journal). A reservation is a durable claim
on `(execution_id, generation)` plus a deadline; the journal mechanics are in
[storage-journal.md](storage-journal.md), and the idempotency/fencing rationale is ADR-0003
(`../plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`).

## Lease lifecycle

An `ExecutionHandle` carries `execution_id`, `generation`, and `lease_id`. The raw lease id is never
stored or logged: `store::lease_hash(lease_id.as_str())` (`lib.rs:1659`) is a one-way hash, and every
subsequent comparison is hash-to-hash (`lib.rs:1684`, `2051`, `2086`, `2268`).

`LeaseState` (`lib.rs:220-224`) is `expires_at: tokio::time::Instant`, `notify: Arc<Notify>`, and
`expired: Arc<AtomicBool>`, held behind a `tokio::sync::Mutex` inside the `ExecutionRecord`. A fresh
record gets `expires_at = now + lease_ttl` (`new_record`, `lib.rs:1886-1908`); a `recovered_record`
gets `expires_at = now` (already expired) and `finished: true` (`lib.rs:1910-1930`) — a recovered
execution is history, never a live lease.

`monitor_lease` (`lib.rs:1932-1954`) is one task per accepted execution:

- Returns immediately if the snapshot is already terminal.
- Snapshots `(deadline, notify)` under the lock, then `select!`s on cancellation, `notify.notified()`,
  and `sleep_until(deadline)`.
- On wake, it **re-checks** `now >= lease.expires_at` under the lock before acting. A renewal that
  pushed the deadline out causes the re-check to fail and the loop to continue, so a spurious wake
  can never expire a renewed lease.
- On true expiry: `expired.store(true, Release)`, then `record.cancellation.cancel()`.

**Renewal** (`control` with `renew = true`, `lib.rs:2094-2150`): requires a non-empty `renewal_id`
bounded by `eggwork_core::MAX_ID_BYTES`; refuses if the snapshot is terminal (`409
execution_terminal`); refuses if `expired` is set or `now >= expires_at` (`409 lease_expired`);
computes a new deadline as `now + lease_ttl` (same clamped expression); persists via
`store.update_lease(id, generation, existing.lease_token_hash, renewal_id, requested_expires)`. If
the store returns `None` the wall clock has already judged the lease expired ⇒ `409 lease_expired`.
On success it converts the *persisted wall-clock* deadline into the monotonic one
(`Instant::now() + Duration::from_millis(remaining_ms)`, `lib.rs:2147-2148`), calls
`notify.notify_waiters()` to interrupt the sleeping monitor, and returns the current snapshot.

**Cancellation** (`lib.rs:2152-2176`): if the state is non-terminal and not already `Cancelling`, it
takes the lease lock, refuses when the lease has expired (`409 lease_expired` — *a lease holder
cannot cancel after its lease died*), publishes `Cancelling`, re-checks the state under the lock and
cancels the token if still non-terminal. If the state is already `Cancelling` it re-issues
`cancellation.cancel()` (idempotent). Returns `202` with the fresh snapshot.

**Preconditions before either branch** (`lib.rs:2023-2092`): schema version must be 1 and
`request.handle.execution_id` must equal the path id; the row must exist (`404`); `principal_id` must
match (`403 forbidden`); the lease hash must match the store (`403 invalid_lease`); the *latest*
snapshot's generation must equal the requested generation (`409 generation_mismatch` / "stale"); and
the in-memory `record.lease_hash` must equal `existing.lease_token_hash` (`403 invalid_lease`).

**What a stale lease holder can and cannot do after a new generation.** The `latest.generation !=
request.handle.generation` check (`lib.rs:2072-2078`) is the fence: once a higher generation exists
for that execution id, a request bearing the older generation gets `409 generation_mismatch` and is
rejected *before* any mutation. Combined with the preceding hash comparisons, a holder of an
expired generation cannot renew (renewal would push a deadline the store has already superseded),
cannot cancel another generation's process (`cancellation.cancel()` lives on the *current* record's
token, and reaching it requires passing the generation check), and cannot read events — `events_route`
performs the same `store.lookup(id, generation)` and lease-hash check (`lib.rs:2253-2275`) before
selecting the record. The terminal publish funnel adds a second layer: `publish` returns early if
`is_terminal(&current.state)` (`lib.rs:2576-2578`), so a late-arriving event from a superseded run
cannot move a terminal snapshot backwards. `cross_principal_execution_fencing_without_lease_oracle`
(`lib.rs:5766`) exercises cross-principal fencing and asserts the responses do not form a lease
oracle; `expired_lease_terminates_process_with_a_typed_terminal_result` (`lib.rs:4066`) covers expiry
producing a typed `Interrupted`/`LeaseExpired` result.

**`publish` is also the failure sink.** On a store error it sets `record.draining.store(true)` and
cancels the execution (`lib.rs:2590-2593`). A journal failure therefore degrades the *node* into
draining and kills the affected execution rather than continuing with an unrecordable state. This
shares the node's `draining` `Arc` with the record (`new_record` receives `state.draining.clone()`,
`lib.rs:1855`), which is why `node_status` reports drain and `execute` refuses admission on the same
flag. Caveat: `recovered_record` installs a *fresh* `AtomicBool::new(false)` for `draining`
(`lib.rs:1921`) rather than the node's, so a journal failure on a recovered/synthetic record cannot
flip the node into drain.

## Execution lifecycle

`execute` finishes by publishing `Accepted` (`lib.rs:1861-1867`), spawning `monitor_lease`
(`lib.rs:1869`), spawning `run_execution` with the permit moved in (`lib.rs:1873-1882`), and
returning `event_response(state, id, generation, 0, record)`.

`run_execution` (`lib.rs:2370-2512`):

1. Publishes `Running` before the runner is spawned.
2. Creates `mpsc::channel(RUNNER_CHANNEL_CAPACITY)` — 32 — and spawns
   `runner.run(request, cancellation, output_tx)`.
3. Loops in a **`biased`** `select!`: output is drained first, then the runner task is joined
   (`lib.rs:2396-2414`). Biasing guarantees no output is lost to task-completion ordering. The loop
   ends when the output channel closes *and* a result is known, then a final
   `while let Ok(..) = output_rx.try_recv()` sweep (`lib.rs:2416-2423`) drains anything that raced.
4. Maps the result. If the runner returned a `RunnerResult`, `result.execution_result()` is the base;
   **if the lease expired, it is rewritten in place** to `ExecutionState::Interrupted` /
   `ExecutionFailure::LeaseExpired` with `exit_code: None` (`lib.rs:2425-2432`) — the process may
   have exited successfully, but the node had no authority over it at the end, so success is not
   claimed. Otherwise `failure_for_runner_error` (`lib.rs:2557-2567`) maps: `RunnerError::Spawn(_)` →
   `Spawn`; **lease-expired wins over everything except spawn**; `CancelledBeforeSpawn` →
   `Interrupted`; anything else → `Internal`. The terminal state is `Interrupted` when expired,
   `Cancelled` when the failure is `Interrupted`, else `Failed` (`lib.rs:2436-2441`).
5. Declared-output capture runs only when the runner *completed*, outputs were declared, the state
   is `Succeeded`/`Failed`, and the failure is not `OutputLimit` (`lib.rs:2445-2452`). Capture
   failure becomes `ExecutionFinalizationFailure::ArtifactCapture` rather than changing the outcome;
   `workspaces.mark_terminal` failure becomes `ExecutionFinalizationFailure::Retention`
   (`lib.rs:2478-2485`). The execution outcome is never rewritten by a finalization problem.
6. Emits the terminal metric (`terminal_succeeded` / `terminal_failed` / `terminal_cancelled` /
   `terminal_timed_out` / `terminal_interrupted`), `stdout_bytes`, `stderr_bytes`, and
   `cleanup_failures` if `cleanup_warning` is set.
7. `publish_terminal` then `record.finished.store(true, Release)` — the permit is released by the
   `_permit` argument's `Drop` as the task ends.

**Capacity and what a drop means.** `RUNNER_CHANNEL_CAPACITY = 32` bounds the runner→server output
path. If the server falls behind (e.g. a slow `store.commit_event` per chunk), the runner's
`send` awaits; it does not drop. So the 32-slot bound converts unbounded memory growth into
back-pressure on the runner. `EVENT_CAPACITY = 32` is the `broadcast` channel capacity for live
subscribers and is *lossy by design* — a slow subscriber gets `RecvError::Lagged`, and
`event_stream_response` turns that into a `ResponseStreamError("event consumer exceeded bounded
retained history")` that terminates the stream (`lib.rs:2628-2635`). This is **safe** precisely
because the journal is the source of truth: the client reconnects with its last sequence in `after`
and the page comes from `store.load_page`. No correctness rests on the broadcast. The test
`slow_event_consumer_is_bounded_and_gets_a_stream_error` (`lib.rs:3102`) pins the failure mode
explicitly rather than allowing an unbounded per-subscriber queue.

`publish` is the single write funnel, and it takes **two** locks with different jobs.
`ExecutionRecord::commit_lock` (`lib.rs:205-216`) serialises publishers across the durable commit;
`snapshot` is a plain `RwLock` held only for the cheap read and the short assign. The order is:
commit lock, read lock + terminal short-circuit, clone-and-propose, `store.commit_event` (which
assigns the sequence), write lock to install the new snapshot, `events.send`. `Ok(None)` means the
store declined the event (already recorded) and is silently ignored. `publish_terminal` is just
`publish` with `Some(result)`.

The split matters. `proposed` is derived from a snapshot *read*, so two overlapping publishers would
each rebase onto a stale base and the later would erase the earlier's fields; the write guard that
used to span the whole commit prevented that, at the cost of blocking every snapshot reader
(`event_response`, `control`) for the duration of a `synchronous=FULL` fsync. The separate
`commit_lock` keeps writer serialisation identical while readers only ever wait for a pointer-sized
write. The one visible relaxation is that a reader can now observe the pre-commit snapshot during
the write instead of blocking — the transition is monotonic, so such a reader sees the previous
state and then receives the event on the broadcast channel.

## Observing and control

**`observe`** (`lib.rs:1956-1993`) — `GET /v1/executions/{id}?generation=…`. `generation` is
optional; if present it must parse as `u64` and validate via `ExecutionGeneration::new`, else
`400 invalid_request`. The read is
`store.load_snapshot_for_principal(id, generation, principal.id)`, so **ownership is enforced inside
the store**, not by a separate 403. A missing or non-owned row is a single `404
execution_not_found` — no 403/404 oracle. Query parsing goes through `query_parameter`
(`lib.rs:2351-2368`), which rejects duplicate keys and any key other than `generation`, `after`, or
`lease` (`400 "query is invalid"`).

**`control`** (`lib.rs:1995-2177`) — shared by cancel and renew; the `renew: bool` flag selects the
branch. Covered in detail under *Lease lifecycle*. Failure modes for a stale caller:

| Condition | Response |
| --- | --- |
| Schema ≠ 1, or `handle.execution_id` ≠ path id | `400 invalid_request` |
| No row for `(id, generation)` | `404 execution_not_found` |
| `principal_id` mismatch | `403 forbidden` ("execution belongs to another principal") |
| Lease hash mismatch (store) | `403 invalid_lease` |
| `latest.generation != request.handle.generation` | `409 generation_mismatch` ("stale") |
| No in-memory record (evicted/recovered) | `404 execution_not_found` |
| In-memory `lease_hash` ≠ store hash | `403 invalid_lease` |
| Renew: missing/oversized `renewal_id` | `400 invalid_request` |
| Renew: terminal | `409 execution_terminal` |
| Renew or cancel: lease already expired | `409 lease_expired` |
| Renew: `store.update_lease` returns `None` | `409 lease_expired` |
| Cancel: no such execution | `404 execution_not_found` |

Renew returns `200` with the current snapshot; cancel returns `202` with the post-transition
snapshot. Both are idempotent-ish in the ways described above, and both re-read the snapshot under
the lock so the response reflects state at the time of the reply, not at the time of the check.

## Events and streaming

`GET /v1/executions/{id}/events` → `events_route` (`lib.rs:2179-2301`).

Query parameters — all three effectively required, each with its own typed error:

- `generation` — absent ⇒ `400 "generation is required"`; unparseable ⇒ `400 "generation is
  invalid"`.
- `after` — absent ⇒ `400 "event cursor is required"`; non-`u64` ⇒ `400 "event cursor is invalid"`;
  `> i64::MAX` ⇒ `416 cursor_ahead`.
- `lease` — absent ⇒ `400 "execution lease is required"`; invalid `LeaseId` ⇒ `400 "execution lease
  is invalid"`. This is what makes the events route lease-bearing rather than merely authorized.

Then `store.lookup(id, generation)` (`404` if absent), and a single combined check
(`lib.rs:2267-2275`): if the principal differs **or** the lease hash differs, both yield
`403 forbidden` / "execution event access is not authorized" — deliberately one status for two
causes, so the route is not an existence oracle.

**Poll vs stream.** There is no separate poll route. `event_response` (`lib.rs:2303-2349`) always
returns a stream; a client that wants only the current batch reads the stream and closes it. It
subscribes to the broadcast *before* loading the page (`lib.rs:2310-2311`) — subscribe-then-read is
the correct order, since anything published after the subscribe is buffered and anything before it
is in the page.

**Gap-free resume.** `store.load_page(id, generation, after)` supplies
`{ base_sequence, next_sequence, events }`:

- `after + 1 < page.base_sequence` ⇒ the requested history has been trimmed: metric
  `event_history_resync` and `410 history_expired` / "event history has expired" (`lib.rs:2325-2332`).
  This is the honest "you are too far behind, resynchronize" answer rather than silently skipping.
- `after >= page.next_sequence` ⇒ `416 cursor_ahead`.
- Otherwise the page plus the live receiver is handed to `event_stream_response`.

`event_stream_response` (`lib.rs:2606-2667`) emits `202` with `content-type: application/x-ndjson`
and headers `eggwork-execution-id` / `eggwork-execution-generation`. It drains the replay
`VecDeque` first, then the broadcast, and enforces
`if event.sequence.get() <= last_sequence { continue }` (`lib.rs:2639-2641`) — the dedup that makes
replay-then-live overlap harmless and is what guarantees a client sees **no gap and no duplicate**.
The stream ends when it emits a terminal `State` event, when the receiver is `Closed`, or on lag.

**What a reconnecting client sees.** Exactly the events after its cursor, in sequence order, each as
one JSON line. If it was disconnected long enough for the journal page to be trimmed, it gets `410`
and must re-`observe` for the snapshot; if it was merely lagging the broadcast, it gets a mid-stream
error and reconnects with `after = ` its last sequence, which is still inside the journal and
therefore resumes cleanly. If the execution is already terminal, `initial_terminal && replay.is_empty()`
makes the stream end immediately (`lib.rs:2615`).

## Data-operation handlers

Dispatch-level only; internals are in [data-plane.md](data-plane.md).

| Handler | Operation | Body cap | Reference / lease mechanism |
| --- | --- | --- | --- |
| `blob_missing` (`lib.rs:902`) | `BlobRead` | `read_limited_body(64 KiB)` | None. Per-digest `authorize_resource` for the whole batch before `find_missing`. `413 too_many_digests` on batch overflow. |
| `blob_prepare` (`lib.rs:957`) | `BlobWrite` | `read_limited_body(64 KiB)` | None; `prepare_upload` creates the reference. Per-digest authorization on the *body's* digest. |
| `blob_upload` (`lib.rs:1253`) | `BlobWrite` | streamed, `blob::MAX_BLOB_BYTES` | None in `lib.rs`; the digest is from the path. Requires `?size=`; declared size must not exceed the max (`413 blob_too_large`) and must equal `Content-Length` when present (`400 length_mismatch`). A body-read error becomes `BlobError::Body`. Metric `blob_upload_bytes`. |
| `blob_download` (`lib.rs:1306`) | `BlobRead` | streamed 64 KiB chunks | `blobs.open_verified(&digest)` — verified, not merely opened. Per-chunk `blob_download_bytes`. Response carries `eggwork-blob-digest`. **No** reference lease: blob reads are content-addressed and idempotent, so GC has nothing to protect. |
| `workspace_create` (`lib.rs:1005`) | `WorkspaceCreate` | `read_limited_body(4 MiB)` | `handle` is the owner reference; `materialize` records the workspace against `(execution_id, generation, principal)`. Metric `workspace_manifest_registrations`. |
| `workspace_derive` (`lib.rs:1125`) | `WorkspaceCreate` | `read_limited_body(4 MiB)` | Same handle binding, `materialize_derived` against a base manifest. Metrics `workspace_derived_requests` / `_hits` / `_patch_entries` / `workspace_base_missing`. |
| `artifact_list` (`lib.rs:1340`) | `ArtifactRead` | none (query only) | `generation` is **required** and must validate, else `400`. `artifacts.list(&id, generation, &principal.id)` filters by principal. |
| `artifact_download` (`lib.rs:1394`) | `ArtifactRead` | streamed 64 KiB chunks | `BlobReferenceLease` + `Drop` (below). |

**`BlobReferenceLease` and why a download must hold one.** Artifacts are *declared outputs* whose
backing blobs are otherwise garbage-collectable. Without a live reference, a concurrent
`collect_garbage` could unlink the blob between `artifacts.get` and the first chunk, and the client
would get a truncated stream with no error it can distinguish from completion. The pattern
(`lib.rs:1373-1392`, used at `lib.rs:1409-1453`):

1. Generate a `uuid::Uuid::new_v4()` `owner_id` — unique per download, so two concurrent readers of
   the same artifact do not collide on the reference row.
2. `blobs.retain("reader", &owner_id, [digest], now + 30 min)` — creates the reference *before* the
   file is opened. Failure returns `404 artifact_not_found` rather than a 500, so a reference-store
   failure does not distinguish "artifact absent" from "reference denied".
3. Construct `BlobReferenceLease { blobs, owner_id }` and **move it into the response stream state**
   (`lib.rs:1440`). The lease is owned by the stream, so it lives exactly as long as the bytes are
   being produced.
4. `renew()` (`lib.rs:1379-1385`) pushes the expiry to `now + 30 min` on *every chunk*, so any
   download slower than 30 minutes per chunk still holds. A renewal failure aborts the stream
   (`ResponseStreamError::new("artifact lease refresh failed")`) — fail-closed: better a visible
   error than a silently unbacked read.
5. `Drop` (`lib.rs:1388-1392`) calls `release_references("reader", &owner_id)`, ignoring the result.
   Release happens on normal completion, on client disconnect, on stream error, and on task panic
   alike — it is `Drop`, not a happy-path call, so it cannot be skipped. Its only weakness is that
   the residual reference is *not* removed by `Drop` alone if the process dies; the 30-minute
   expiry (or a future GC of expired references — `NodeGcReport.expired_blob_references`) is the
   backstop.

## Metrics and live GC

`increment_metric(store, name, amount)` (`lib.rs:779-781`) is a thin `async` wrapper that
**discards its result** (`let _ = …`). Metrics are therefore best-effort and can never fail a
request or abort a transition. Names are `&'static str`, so the metric key space is closed at compile
time. Instrumented sites: `terminal_interrupted` (recovered at start, `lib.rs:331`),
`rejected_route` (728), `rejected_unauthenticated` (733), `rejected_unauthorized` (748),
`rejected_draining` (1572, 1705), `rejected_capability` (1580, 1588, 1596),
`rejected_invalid` (1554, 1562, 1624, 1651, 1609, 1051, 1172, 1749),
`rejected_busy` (1715), `rejected_storage` (1734, 1837), `executions_accepted` (1791),
`event_history_resync` (2326), `workspace_manifest_registrations` / `workspace_derived_requests` /
`workspace_derived_hits` / `workspace_patch_entries` / `workspace_base_missing`,
`blob_upload_bytes` / `blob_download_bytes`, `terminal_*` (2488-2495), `stdout_bytes`,
`stderr_bytes`, `cleanup_failures`.

**`NodeGcReport`** (`lib.rs:232-247`) is a `Serialize` struct of 13 counters plus `dry_run`. It
flattens four sub-reports — workspaces, manifests, artifacts, blobs — into one shape, and
normalizes `expired_blob_references` to `expired_references` in dry-run mode vs
`expired_references_removed` in real mode (`lib.rs:496-500`), so a dry run reports what *would* be
reclaimed.

**`NodeServer::collect_garbage(dry_run, requested_limit)`** (`lib.rs:460-507`) clamps
`requested_limit` to `1..=1024`, then runs, in order: `workspaces.garbage_collect`,
`workspaces.manifest_garbage_collect(now, limit, dry_run, &blobs)`,
`artifacts.garbage_collect`, `blobs.garbage_collect`. Every one is **bounded** by the same clamped
limit, so a single pass is a fixed amount of work. Errors are stringified into
`Result<NodeGcReport, String>`.

The critical property is that GC runs **against the live, open stores** — the same
`BlobStore`, `WorkspaceManager`, and `ArtifactStore` instances the serving path uses, sharing state
with running executions. A workspace currently in use is not terminal, so it is not a candidate;
blob references held by in-flight artifact downloads (§ *Data-operation handlers*) are exactly the
mechanism that keeps a blob a non-candidate while its bytes are being read. Contrast with the CLI
GC path (`operations.rs`), which takes the **exclusive state lock** — the same `fs2` lock that makes
a second `NodeServer::start` fail with `AlreadyRunning`. While a node is serving, the CLI GC cannot
acquire that lock and therefore fails rather than running concurrently. The two designs are
deliberately asymmetric: the live node gets bounded, reference-aware GC with no lock; the
out-of-band CLI gets an exclusive lock and refuses to run. Neither path can run two GCs over the same
stores at once.

## Error model

Three layers, deliberately ordered from most specific to least revealing:

1. **Typed `ApiError` bodies** (`lib.rs:2689-2698`): `{ schema_version, code, message }` with
   `content-type: application/json`. `code` is the stable machine key; `message` is a fixed,
   human-readable constant. Neither is derived from internal state.
2. **`ServiceError::internal(fixed string)`** for store unavailability. Every call site uses one of a
   very small set of literals — `"execution store unavailable"`, and `e.to_string()` only in the two
   places where a builder genuinely fails (stream/response construction, `lib.rs:1337`, `1460`).
   Internal `ServiceError` never reaches the client as a body.
3. **`catch-all` arms** that discard the underlying error: `Err(_) => error_response(500,
   "storage_error", …)` in `blob_missing` (`lib.rs:949`), `artifact_list` (`lib.rs:1365`),
   `workspace_create` (`lib.rs:1108`); `let _ = error;` in `execute`'s `spec.validate()` branch
   (`lib.rs:1552-1553`); `let _ = record.events.send(event)` (`lib.rs:2587`).

**Status mapping summary:**

| Status | Codes |
| --- | --- |
| 200 | `capabilities`, `status`, observe snapshot, `find_missing`, `blob_prepare`, renew snapshot, artifact list |
| 201 | `workspace_create` / `workspace_derive` ready; blob upload |
| 202 | cancel accepted; event stream (including the `execute` acceptance stream) |
| 400 | `invalid_request`, `invalid_manifest`, `invalid_patch`, `invalid_digest`, `length_mismatch`, `workspace_required` |
| 401 | `unauthenticated` |
| 403 | `forbidden`, `invalid_lease` |
| 404 | `not_found`, `execution_not_found`, `blob_not_found`, `artifact_not_found` |
| 409 | `capability_mismatch`, `workspace_not_ready`, `missing_blob`, `blob_size_mismatch`, `workspace_identity_conflict`, `base_manifest_missing`, `execution_identity_conflict`, `canonicalization_version_mismatch`, `generation_mismatch`, `execution_terminal`, `lease_expired` |
| 410 | `history_expired` |
| 413 | `request_too_large`, `too_many_digests`, `blob_too_large` |
| 416 | `cursor_ahead` |
| 422 | `length_mismatch` (blob), `digest_mismatch` |
| 426 | `protocol_version` |
| 500 | `internal`, `storage_error`, `workspace_error`, `corrupt_blob`, `artifact_unavailable` |
| 503 | `busy`, `draining`, `storage_exhausted` |
| 507 | `quota_exceeded` |

**Detail deliberately withheld.** Filesystem paths (only in `NodeStartError::Display`, redacted in
its `Debug`), lease ids (stored and compared only as hashes), peer certificate bytes (never
echoed), internal error chains (collapsed into a fixed `code`/`message`), and the *existence* of
resources behind a 403 (authorization precedes resolution, and observe/control/events return a
single 404 for both "absent" and "not yours"). `secret_sentinels_stay_out_of_error_and_debug_surfaces`
(`lib.rs:6104`) is the executable guard for the first two.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
| --- | --- | --- |
| Only a transport-verified, known peer is admitted | `authenticated_principal` (`lib.rs:823-833`) | `request_payload_cannot_supply_authoritative_principal` (2993), `mtls_execution_streams_output_and_missing_certificate_is_rejected` (3724) |
| Identity is the SHA-256 of the leaf DER, nothing else | `FingerprintPrincipalResolver::fingerprint` (`lib.rs:107-109`) | `fingerprint_mapping_uses_leaf_der_only` (2924) |
| TLS must require client auth and a server identity | `NodeServer::start` (`lib.rs:307-311`) | `mtls_authorization_and_fixed_target_execution` (3137) |
| A node never advertises an unenforceable capability | `server_capability_features` / `check_*_supported` (`lib.rs:583`, `606`, `620`, `636`) | `static_capability_features_do_not_advertise_unsupported_network_modes` (5704), `admission_helpers_keep_required_capability_gating_active` (5719) |
| Capabilities and status never disagree | single `node_capabilities_for` (`lib.rs:595`, `580-582`) | `capabilities_and_status_features_agree_with_a_unified_snapshot` (4617) |
| The feature list is single-canonical and deduped | `static_capability_features` + sort/dedup (`lib.rs:563`, `583`) | `static_capability_features_are_a_single_canonical_list` (5667), `server_capability_features_do_not_duplicate_static_dynamic_overlap` (5682) |
| Only the fixed, versioned route set exists | `operation_for` (`lib.rs:640-700`) | `route_contract_is_fixed_target_and_versioned` (2934) |
| An authorized op on a different resource is still denied | `authorize_resource` re-checks (`lib.rs:808`, `928`, `983`, `1038`, `1159`, `1518`) | `resource_aware_authorizer_can_scope_blob_capability` (3039), `derived_workspace_denied_and_oversized_bodies_are_typed` (4499) |
| A repeat submit never starts a second execution | `store.lookup` pre-check + `ReserveResult::Existing` (`lib.rs:1664`, `1793`) | `blob_protocol_streams_verifies_deduplicates_and_enforces_quota` (3399) |
| Identity conflicts are refused, not merged | `lib.rs:1670-1690`, `1808-1835` | `mtls_authorization_and_fixed_target_execution` (3137) |
| Stale generations cannot mutate state | generation preconditions in `control` (`lib.rs:2072`) and `events_route` | `cross_principal_execution_fencing_without_lease_oracle` (5766) |
| Only the lease holder controls an execution | lease-hash comparisons (`lib.rs:1684`, `2051`, `2086`, `2268`) | `cross_principal_execution_fencing_without_lease_oracle` (5766) |
| An expired lease cannot be renewed or used to cancel | `lib.rs:2117`, `2154` | `expired_lease_terminates_process_with_a_typed_terminal_result` (4066) |
| Lease expiry beats a spurious wake | re-check under lock (`lib.rs:1946`), renewal `notify_waiters` (`lib.rs:2149`) | `expired_lease_terminates_process_with_a_typed_terminal_result` (4066) |
| Lease expiry is not reported as success | in-place rewrite to `Interrupted`/`LeaseExpired` (`lib.rs:2427-2431`) | `expired_lease_terminates_process_with_a_typed_terminal_result` (4066) |
| A terminal execution is never moved backwards | `publish` terminal short-circuit (`lib.rs:2576`) | `restart_recovers_uncertain_execution_as_interrupted_without_replay` (4133) |
| Restart recovers, never replays, uncertain executions | `store.recover()` + `recovered_record` `finished: true` (`lib.rs:326`, `1922`) | `restart_recovers_uncertain_execution_as_interrupted_without_replay` (4133) |
| Concurrent admission is bounded and immediate | `try_acquire_owned` (`lib.rs:1712`) | `drain_rejects_new_admission_while_terminal_records_survive` (5906) |
| Drain outcomes are closed under concurrency | double drain check + persistent marker | `concurrent_drain_admission_outcomes_are_closed` (6180) |
| Drain survives restart | marker re-read at start (`lib.rs:348`) | `operator_drain_persists_across_node_restart` (4574) |
| Slow event consumers are bounded, not buffered without limit | `EVENT_CAPACITY` + lag→error (`lib.rs:2628`) | `slow_event_consumer_is_bounded_and_gets_a_stream_error` (3102) |
| Event resume has no gap and no duplicate | subscribe-then-page + sequence filter (`lib.rs:2310`, `2639`) | `mtls_execution_streams_output_and_missing_certificate_is_rejected` (3724) |
| A download's bytes are protected from concurrent GC | `BlobReferenceLease` + `Drop` (`lib.rs:1373-1392`) | covered indirectly by artifact/blob protocol tests (3399) |
| Terminal record memory is bounded | `MAX_RECENT_EXECUTIONS` eviction (`lib.rs:1719-1740`) | `drain_rejects_new_admission_while_terminal_records_survive` (5906) |
| One process per state directory | `acquire_state_lock` (`lib.rs:540-556`) | startup `AlreadyRunning` path (no dedicated test observed) |
| Secrets never reach error/debug surfaces | redacting `Debug`, hash-only leases (`lib.rs:154`, `1659`) | `tls_configuration_diagnostics_redact_certificate_paths` (3083), `secret_sentinels_stay_out_of_error_and_debug_surfaces` (6104) |
| Required isolation is enforced, not merely requested | landlock admission + E2E denial (`lib.rs:613`, `606`) | `required_landlock_is_admitted_remotely_and_denies_outside_workspace_access` (4699), `required_landlock_denies_outside_writes_and_confines_descendants` (4846) |
| Hostile workspace manifests cause no side effect | `WorkspaceError` mapping (`lib.rs:1083-1107`) | `hostile_workspace_manifests_are_rejected_without_side_effect` (6008) |

## Test coverage

`mod tests` starts at `lib.rs:2719` and runs to EOF (3,562 lines — by far the largest part of the
file). Fixtures first: `init_tls`, `issue_identity`, `tls_material`, `untrusted_client_identity`,
`write_client_config`, `post_raw`, `pem`, `tls_server_config`, `execution_spec`,
`execution_handle`, `place_trusted_helper`, `locate_helper_binary`. Then roughly 30 cases.

**Notable cases and what they pin** (line numbers in `lib.rs`):

- 2934 `route_contract_is_fixed_target_and_versioned` — the route table is exhaustive; known routes
  are refused unauthenticated.
- 2993 `request_payload_cannot_supply_authoritative_principal` — a body-supplied principal is
  ignored; the TLS identity wins.
- 3039 `resource_aware_authorizer_can_scope_blob_capability` — an authorizer overriding **only**
  `authorize_request` denies one digest, exercising the resource-scoped path independently of
  `GrantAuthorizer`.
- 3090 `lease_expiry_takes_precedence_over_cancel_before_spawn` — the match order in
  `failure_for_runner_error` (`Spawn` > expired > `CancelledBeforeSpawn`) is load-bearing.
- 3137 `mtls_authorization_and_fixed_target_execution`, with 3326 `routed_mtls_execution` and the
  3385/3392 SOCKS5 and HTTP-CONNECT variants — execution semantics must be identical regardless of
  which network route carried the request, so no route is a weaker security boundary.
- 3399 `blob_protocol_streams_verifies_deduplicates_and_enforces_quota` — the largest test: streamed
  upload, digest verification, dedup, quota.
- 4066 `expired_lease_terminates_process_with_a_typed_terminal_result` and 4133
  `restart_recovers_uncertain_execution_as_interrupted_without_replay` — the two
  durability-critical cases.
- 4249 `derived_workspace_materialization_loopback` and 4499
  `derived_workspace_denied_and_oversized_bodies_are_typed` — the derive path end to end,
  including the 413 branch.
- 4699, 4846, 4998, 5071, 5144, 5229, 5355 — the landlock/resource/network admission set: every
  `Required` gate plus both degradations, driven through a real sandbox helper. `place_trusted_
  helper` / `locate_helper_binary` returning `Option` indicates these are **skipped when the helper
  binary is absent**, so their coverage is environment-dependent.
- 5455 `controller_compatibility_fixture_requires_landlock_then_executes_required_isolation` — the
  controller-facing contract, as a fixture rather than an assertion pair.
- 5766 `cross_principal_execution_fencing_without_lease_oracle` — the security centerpiece.
- 5906 / 6180 — terminal records survive drain, and concurrent drain outcomes are closed.
- 6104 `secret_sentinels_stay_out_of_error_and_debug_surfaces` — sentinels searched across error and
  `Debug` renderings.

**Honest gaps**, identified from the test names without reading every body:

- No named test for `AlreadyRunning` / the exclusive state lock.
- No named test for the `MAX_RECENT_EXECUTIONS` eviction loop in isolation; terminal-record memory
  pressure is exercised only indirectly.
- No named test for `monitor_lease`'s spurious-wake re-check under concurrent renewal. Expiry is
  covered; the wake-vs-renew interleaving is not visibly named.
- No named test for `NodeServer::collect_garbage` / `NodeGcReport`, so the
  live-GC-preserves-active-references property is not obviously asserted *at this layer*. It may
  be asserted in the `blob`/`artifact` module tests, which are out of my scope — I did not verify.
- Gap-freeness after a broadcast lag rests on 3102 (lag becomes a stream error); I did not verify a
  test that reconnects with `after` after a lag to prove resume end to end.
- `Route::BlobInvalidDigest`'s reachability through `PUT` is not obviously covered, consistent with
  `request_body_policy` rejecting that shape first (§ *Request dispatch*).

## Review focus

- **Authorization precedes resolution — is the disclosure boundary right?** `dispatch` rejects at 744
  before any handler. Confirm every body-derived resource id is covered by an `authorize_resource`
  re-check (`execute`'s `workspace_id`, `blob_prepare`'s digest, `blob_missing`'s whole batch).
- **Lease fencing under concurrency.** `control` checks the store row, the latest generation, and the
  in-memory record across three separate awaits. Can a newer generation land between `lib.rs:2072`
  and `lib.rs:2163`'s `publish`? `publish` re-checks terminal while holding `commit_lock` and the
  snapshot read guard, but does **not** re-check generation.
- **Digest reuse across callers.** `store.lookup` compares `principal_id` *before* the digest
  (`1677` vs `1684`), so a foreign principal learns an execution id exists from the 403 while a
  matching principal learns identity conflicts. Intended?
- **`GrantAuthorizer` ignores resources.** It implements only `authorize`, so `resource` in
  `OperationRequest` is inert for the real deployment path. Is store-level per-execution ownership
  genuinely sufficient, or are resource-scoped grants required?
- **Silent config-drop in `operation(name)`** (`operations.rs:342-357`): `filter_map` means a typo in
  a configured operation name quietly narrows a client's grants with no startup error. Should it be
  `OperationsError::Config`?
- **Capacity-drop correctness.** The only hard bound on buffered stdout/stderr is
  `RUNNER_CHANNEL_CAPACITY = 32`. Confirm the runner *awaits* on send rather than dropping, and that
  `event_history_resync` covers broadcast-lag resyncs, not just journal-trim ones.
- **Drain-check race.** Between the second drain check (`1702`) and `try_acquire_owned` (`1712`) a
  `set_draining(true)` can land — bounded, but is the bound documented? `set_draining` also mutates a
  file the request path reads; check for torn reads on the marker.
- **Unbounded collections.** The eviction loop (`1719-1732`) iterates a `HashMap` in arbitrary order
  and breaks on a non-terminal entry, so a pathological mix can reject with `storage_exhausted`
  while space is reclaimable. `resource_for_path` and `operation_for` also each `split('/')` into a
  fresh `Vec` per request — driven by unauthenticated callers before the 401.
- **Integer conversions.** `lease_ttl.as_millis().min(i64::MAX as u128) as i64` (1661, 2125),
  `expires.saturating_sub(now).max(0) as u64` (2147), `recovered.len() as u64` (331),
  `body.declared_length().min(limit as u64) as usize` (850), `after > i64::MAX as u64` (2227). Check
  truncation-vs-saturation, and whether the duplicated `Duration`→`i64` clamp can drift.
- **TOCTOU between admission and runner launch.** The permit is taken at 1712 but the task is not
  spawned until 1873, after the `reserve` await. A crash between `Created` and the spawn leaves a
  durable reservation for an execution that never starts, recovered only as `Interrupted` on the
  *next* start. Intended crash window?
- **Redaction completeness.** The `NodeStartError` `Debug` redaction covers one variant. What about
  `collect_garbage`'s `Result<_, String>` (stringified filesystem errors) and
  `ServiceError::internal(e.to_string())` at `1337`/`1460`? Both are `Debug`-reachable.
- **`_permit` is unused but load-bearing.** Named `_permit` (`2375`) because it is held purely for its
  `Drop`. Confirm no refactor replaces it with `let _ = permit;` or a conditionally-dropped field.

## Related

Architecture deep dives:

- [overview.md](overview.md)
- [domain.md](domain.md)
- [core-domain.md](core-domain.md)
- [client-transport.md](client-transport.md)
- [storage-journal.md](storage-journal.md)
- [data-plane.md](data-plane.md)
- [operations-cli.md](operations-cli.md)
- [runner-execution.md](runner-execution.md)
- [sandbox-helper.md](sandbox-helper.md)
- [execution-ownership.md](execution-ownership.md)
- [distribution.md](distribution.md)
- [protocol.md](protocol.md)

Decision record consulted for idempotency, leases, and fencing:

- [ADR-0003-execution-idempotency-leases-and-fencing.md](../plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md)

Source anchors in this file: `crates/eggwork-server/src/lib.rs`; sibling
`crates/eggwork-server/src/operations.rs` (`GrantAuthorizer`, `principals`, `operation`);
`crates/eggwork-core/src/lib.rs` (`Requirement<T>`, `IsolationRequirement`, `NetworkRequirement`,
`request_digest_with_workspace`, `MAX_ID_BYTES`); `crates/eggwork-runner/src/lib.rs`
(`LANDLOCK_WORKSPACE_RW_CAPABILITY`).
