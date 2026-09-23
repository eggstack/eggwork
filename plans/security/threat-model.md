# Eggwork Node Threat Model

Status: current implementation baseline after Security M001

Scope: the authenticated fixed-target node API, local execution admission,
workspace and artifact storage, event/result exposure, and secret-bearing
request types. Operating-system process isolation and resource enforcement are
tracked separately in Security M002 and M003.

## Assets

- Node identity and TLS private-key material.
- Controller identity mapped from a TLS-verified client certificate.
- Execution specifications, including argv, environment, stdin, and metadata.
- Execution handles and lease tokens.
- Workspace files, blob bytes, artifact bytes, and their digests.
- Execution state, result, and output events.
- Node availability, storage quota, and process capacity.

## Trust boundaries and invariants

1. The TLS transport verifies peer certificates and requires client
   authentication. The node maps only the verified leaf DER certificate to a
   configured principal; request JSON never supplies that identity.
2. Every recognized network route maps to an `Operation`. An authorizer sees
   the authenticated principal and operation, and may additionally inspect a
   typed resource identifier. Resource ownership checks remain mandatory for
   executions, workspaces, and artifacts.
3. Authentication and authorization precede request body parsing and all
   mutation. Body-carried resource identifiers are authorized after decoding
   and before mutation. Denied execution requests cannot acquire admission or
   spawn a child.
4. Execution observe results are principal-fenced and conceal foreign
   executions as not found. Control and event routes additionally verify
   principal, generation, and lease as applicable. Workspace and artifact
   routes enforce their stored principal owner.
5. Blob digests are content identifiers, not credentials. Access to the raw
   Blob API is controlled by its `BlobRead`/`BlobWrite` authorization
   capabilities. Deployments that require per-digest policy should implement
   the resource-aware authorizer hook; principals granted broad BlobRead can
   read any known digest.
6. Environment values, stdin bytes, argv, execution metadata values, client
   transport internals, and API error messages are redacted from ordinary
   `Debug` output. The API returns stable generic errors for malformed or
   unauthorized requests. This is structural redaction at formatting
   boundaries; applications must still avoid logging raw request bodies or
   process output.
7. The node does not log request bodies, certificate chains, environment
   values, workspace contents, blob/artifact contents, or authorization
   headers. Output events are intentionally available to an authorized
   execution owner and may contain secrets emitted by the child.

## Threat actors and mitigations

| Actor | Relevant attack | Mitigation in current baseline | Remaining boundary |
|---|---|---|---|
| Unauthenticated peer | Invoke API or enumerate execution state | Required mTLS; absent/unmapped verified identity rejected before route body handling | Certificate issuance and revocation are deployment responsibilities |
| Authenticated unauthorized principal | Execute, cancel, read, or mutate another principal's resources | Per-operation authorizer; principal fences on execution controls/events, workspace, artifact; observe returns not found for foreign ownership | Broad BlobRead capability permits any known digest unless resource-scoped policy is configured |
| Malicious/compromised controller | Forge principal fields, reuse identity, submit oversized or malformed values | Identity derives from certificate; wire DTOs reject unknown fields; bounded bodies, core validation, generation/lease/digest fencing | A controller authorized for Execute can request arbitrary commands within currently available host isolation |
| Malicious executed process | Exfiltrate inherited secrets, flood output, leave descendants | Environment is cleared and rebuilt from a narrow baseline; sensitive runtime variables are denied; output and process-tree controls are bounded; user output is not logged by node | No sandbox or OS resource backend is active in this baseline; M002/M003 must enforce required controls |
| Hostile workspace input | Traversal, symlink races, malformed names, special files | Portable manifest validation, digest verification, node-owned roots, no-follow descriptor-relative materialization/capture, bounded entries and bytes | Platform coverage is recorded in the workspace closure; unsupported capture fails closed |
| Malicious proxy/intermediary | Redirect fixed-target client or inject credential-bearing URLs | Client requires HTTPS, rejects URL userinfo/query/fragment, and uses explicit caller TLS configuration; no proxy route is accepted by the node protocol | Caller-selected resolver/network path remains a deployment trust decision |
| Stale/replayed controller | Reuse an old generation or expired execution lease | Canonical request digest, principal-bound identity reservation, hashed lease token, generation checks, expiry fencing, idempotent renewal | Distributed controller key compromise is outside the node's ability to distinguish |
| Local unprivileged user | Read node files or replace managed storage entries | Restrictive creation modes, node-owned roots, descriptor-relative no-follow access for artifact collection, private TLS key handling delegated to TLS configuration | Service account, parent directory ownership, mount and backup policy are operational requirements |
| Dependency/supply-chain compromise | Change transport/authentication or unsafe process behavior | Dependency versions are explicit in Cargo.lock; unsafe code is forbidden at crate/workspace level; TLS verification is not disabled | No independent dependency audit or reproducible-build attestation is claimed by M001 |

## Authorization route matrix

| HTTP operation | Capability | Resource/owner checks |
|---|---|---|
| `GET /v1/capabilities` | Capabilities | None |
| `GET /v1/status` | Status | None |
| `POST /v1/executions` | Execute | Execution ID and optional workspace ID are passed to resource-aware policy; execution ID is reserved to the verified principal |
| `GET /v1/executions/{id}` | Observe | Snapshot lookup is fenced by principal and optional generation |
| `POST /v1/executions/{id}/cancel` | Cancel | Principal, generation, and lease token |
| `POST /v1/executions/{id}/renew` | Renew | Principal, generation, lease token, and renewal idempotency key |
| `GET /v1/executions/{id}/events` | Events | Principal, generation, and lease token |
| `POST /v1/blobs/missing`, `GET /v1/blobs/{digest}` | BlobRead | Operation/resource authorization; raw digest access is capability-based |
| `POST /v1/blobs/prepare`, `PUT /v1/blobs/{digest}` | BlobWrite | Operation/resource authorization; digest and content integrity checked |
| `POST /v1/workspaces` | WorkspaceCreate | Workspace ID resource policy, then principal-bound workspace identity |
| `GET /v1/executions/{id}/artifacts`, `GET /v1/artifacts/{id}` | ArtifactRead | Execution/artifact resource policy and stored principal ownership |

Unknown routes are not dispatched. There are no network routes for drain,
workspace deletion, artifact deletion, relay, or PTY in this API version.

## Secret classification and handling

| Data | Classification | Handling |
|---|---|---|
| TLS private key bytes | Secret | Loaded by TLS configuration; never serialized or included in node diagnostics |
| Bearer/bootstrap tokens | Secret | Not accepted by this protocol; mTLS is the identity mechanism |
| Environment values and stdin | Secret-capable | Accepted only as request content; redacted in `Debug`; child receives explicitly supplied values |
| Proxy credentials / authorization headers | Secret | Not accepted as route configuration; URL userinfo rejected by client; no header logging |
| Workspace and artifact bytes | Sensitive user content | Stored as content-addressed bytes; never emitted in ordinary diagnostics; streamed only through authorized endpoints |
| Peer certificate chain | Identity material | Used only during transport-derived principal mapping; not logged or returned |
| Request metadata and argv | Potentially secret | Redacted in `ExecutionSpec` and runner request debug formats; not logged by node |
| Process output | Potentially secret | Intentionally returned as execution events/results to the authorized owner; caller controls emitted content |

## Verification and limits

The implementation tests cover mapped and unmapped mTLS identities, operation
denials before process spawn and storage mutation, unknown payload principal
fields, principal-fenced observe behavior, URL userinfo rejection, resource
descriptors, and secret-negative debug formatting. M001 does not claim a
complete host isolation boundary. Landlock/helper trust is M002; enforced
resource controls are M003; adversarial qualification across those controls is
the later M004 roadmap item.
