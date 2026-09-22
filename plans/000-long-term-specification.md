# Eggwork Long-Term Architecture and Product Specification

Status: canonical long-term implementation directive

Companion documents:

- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`
- `plans/003-planning-process.md`

This document defines the intended end state for Eggwork. It establishes product scope, ownership boundaries, execution semantics, protocol expectations, security requirements, interoperability, recovery behavior, and acceptance criteria. The keywords MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, and MAY are normative.

## 1. Product definition

Eggwork is a Rust-native generic remote execution fabric.

A caller selects a specific execution node, submits one bounded execution request, observes versioned machine-readable lifecycle events, may cancel or renew ownership, and receives a terminal result plus declared artifacts.

Eggwork is deliberately below schedulers and workflow engines. It provides execution mechanism, not placement or orchestration policy.

The canonical model is:

```text
scheduler / orchestrator / human
        |
        | chooses explicit node
        v
   eggwork client
        |
        | authenticated execution protocol
        v
     eggworkd
        |
        +-- validate request and node capability
        +-- protective local admission
        +-- materialize workspace
        +-- enforce isolation/resource policy
        +-- execute argv
        +-- stream bounded events
        +-- collect declared artifacts
        +-- return terminal result
```

CodeGG is an important downstream consumer, but Eggwork MUST remain usable by unrelated Rust applications, CI/lab tooling, Eggbench, and direct CLI users.

## 2. Primary product goals

Eggwork MUST provide:

1. Fixed-target remote execution: the caller chooses the node.
2. A library-first Rust client and server architecture with thin CLI/daemon adapters.
3. A bounded, versioned execution protocol with capability negotiation.
4. Explicit argv execution as the primary command model.
5. Sanitized environment policy, explicit working directory, stdin policy, timeout, cancellation, bounded output, process-tree cleanup, and typed terminal classification.
6. Authenticated network operation suitable for remote arbitrary-process execution.
7. Execution identity, idempotent submission, ownership generations, leases, and stale-controller fencing.
8. Resumable ordered events with bounded retention and explicit resynchronization.
9. Content-addressed input/workspace and artifact transport without requiring Git semantics in the core.
10. Explicit node capabilities and dynamic status.
11. Honest, enforceable isolation/resource requirements with fail-closed semantics when a required control cannot be provided.
12. Cross-platform operation on Linux, macOS, and Windows where the requested capability exists.
13. Small, auditable boundaries that reuse Eggstack networking, TLS, updater, and proxy components rather than duplicating them.
14. An integration path that lets CodeGG preserve its existing scheduler, AgentRun, worktree, and audit ownership while offloading execution.

## 3. Non-goals

Eggwork is not:

- a global scheduler;
- a job-priority or fairness engine;
- a workflow/DAG system;
- an automatic worker selector;
- a cluster autoscaler;
- a semantic retry engine;
- a CI definition language;
- a Kubernetes replacement;
- a Git hosting or merge system;
- a CodeGG AgentRun store;
- a CodeGG WorkOrder store;
- a general distributed database;
- a remote desktop system;
- a secrets manager or PKI authority.

Eggwork MAY expose facts that a scheduler can consume, but MUST NOT decide placement from those facts.

## 4. Architectural principles

### 4.1 Mechanism below policy

Eggwork owns bounded execution mechanism. The caller owns scheduling, placement, workflow, priority, fairness, semantic retry, and application-specific meaning.

A node MAY reject work because it is draining, saturated, lacks a required capability, exceeds local hard limits, or fails authorization. It MUST NOT silently place the request on another node or retain it in an unbounded scheduler-like queue.

### 4.2 Fixed target is explicit

Every remote execution request is addressed to one concrete node endpoint or already-established node connection.

The public API SHOULD read conceptually as:

```text
execute_on(node, spec)
```

and MUST NOT imply:

```text
submit_to_cluster(spec)
execute_anywhere(spec)
```

### 4.3 One canonical finite-process owner

Finite noninteractive execution MUST converge on one canonical local runner implementation. The daemon, CLI, tests, CodeGG adapter, and future local-only embedding MUST NOT create independent process-supervision implementations.

The implementation SHOULD generalize the mature finite-process semantics already present in CodeGG's `ManagedProcessService`: argv execution, sanitized environment, bounded streaming output, timeout/cancellation, process-tree cleanup, typed termination, and sandbox outcome.

CodeGG-specific provenance and policy MUST remain adapters, not enter Eggwork core types.

### 4.4 Transport identity is authoritative

A request payload MUST NOT be able to self-assert its trusted principal, role, or authority.

Authenticated transport/session context determines the caller identity. Payload metadata may provide attribution labels only after explicit validation and MUST NOT widen authority.

### 4.5 Bounded by construction

Variable-length request fields, output chunks, retained event history, metadata, artifacts, workspace manifests, concurrent executions, connection counts, and storage consumption MUST have explicit limits.

No internal queue or task fan-out may be unbounded.

### 4.6 Truthful capability model

Nodes MUST distinguish:

- supported and enforceable;
- supported best-effort;
- unsupported;
- temporarily unavailable.

A required resource or isolation control MUST fail closed when the node cannot enforce it. Eggwork MUST NOT claim a memory, filesystem, process, or network boundary that is merely advisory.

### 4.7 No silent fallback

A proxy failure MUST NOT silently become a direct connection.
A required sandbox failure MUST NOT silently become unsandboxed execution.
An unsupported transport MUST NOT silently downgrade.
A content digest mismatch MUST NOT be ignored.
A stale ownership generation MUST NOT control a newer run.

### 4.8 Durable identity, bounded state

Execution identity and terminal evidence must survive transport reconnects and, where the roadmap requires, daemon restart. Large logs and artifacts SHOULD remain content- or handle-backed rather than expanding control-plane records without bounds.

## 5. Deployment model

The initial deployment is intentionally simple:

```text
controller process
    |
    | HTTPS/mTLS
    v
eggworkd node
    |
    +-- local runner
    +-- local workspace/blob store
    +-- local execution registry
```

A controller may know several nodes, but Eggwork does not create a cluster-wide authority.

Later topologies MAY include a dumb reverse-connect relay:

```text
node --authenticated outbound link--> relay <--authenticated--> controller
```

The relay MUST remain a transport broker. It MUST NOT become the global scheduler or authoritative execution database.

## 6. Canonical identities

Eggwork requires typed stable identities for at least:

- `NodeId`
- `ExecutionId`
- `ExecutionGeneration`
- `LeaseId`
- `WorkspaceId`
- `BlobDigest`
- `ArtifactId`
- `EventSequence`
- `ProtocolVersion`
- transport-derived `PrincipalId` or equivalent authenticated peer identity

Paths, process IDs, sockets, HTTP request IDs, display names, and hostnames are locators or diagnostics, not durable identity.

## 7. Execution request contract

The canonical request model MUST express at least:

```text
ExecutionSpec
|-- execution identity and generation
|-- command
|    |-- executable/argv
|    |-- relative cwd
|    |-- environment policy/overrides
|    `-- stdin
|-- workspace reference
|-- deadline/timeout
|-- declared outputs
|-- resource requirements
|-- isolation requirements
|-- network requirements
`-- bounded caller metadata/provenance
```

### 7.1 Argv first

The primitive command model is executable + argv.

Shell execution MAY be supported as an explicit distinct mode or explicit argv such as `["sh", "-c", "..."]`, but Eggwork MUST NOT silently reinterpret arbitrary command text as a shell program.

### 7.2 Working directory

Remote absolute host paths MUST NOT be the primary portable contract. Execution cwd SHOULD be relative to the materialized workspace or another explicitly authorized node-owned root.

### 7.3 Environment

The default process environment SHOULD be sanitized and deterministic. Inheriting arbitrary controller or daemon environment state MUST require explicit policy.

Secret values MUST NOT enter ordinary debug output, events, stored metadata, or errors.

### 7.4 Stdin

Initial noninteractive execution SHOULD support null stdin and bounded bytes. Streaming stdin and PTY operation are later capabilities and MUST use explicit bounded ownership semantics.

## 8. Execution identity, idempotency, generations, and leases

Network retry MUST NOT accidentally create duplicate mutating execution.

The acceptance key is conceptually:

```text
(execution_id, generation, request_digest)
```

The server MUST provide these semantics:

- same identity/generation + same digest: return the existing accepted execution;
- same identity/generation + different digest: typed identity conflict;
- lower/stale generation attempting control: fenced;
- newer generation: accepted only under explicit transition rules.

An accepted execution obtains an ownership lease. Connection loss alone SHOULD NOT immediately kill an execution. The owner renews the lease; lease expiry applies the configured orphan policy, which defaults to terminating the execution tree unless detached lifetime was explicitly permitted.

Cancellation is explicit and idempotent.

## 9. Execution lifecycle and terminal result

The minimum execution lifecycle is:

```text
Received
  -> Validating
  -> Preparing
  -> Running
  -> CleaningUp
  -> Succeeded | Failed | Cancelled | TimedOut | Rejected | Interrupted
```

Implementations MAY collapse internal stages in public projection but MUST retain enough evidence to distinguish rejection before spawn, execution failure, timeout, cancellation, cleanup failure, and restart interruption.

The terminal result MUST include:

- execution identity/generation;
- final state;
- process exit code/signal where meaningful;
- duration;
- bounded stdout/stderr summary or handles;
- output truncation facts;
- sandbox/isolation outcome;
- cleanup diagnostics;
- declared artifacts;
- warnings;
- retry-neutral failure classification.

Eggwork's failure taxonomy provides facts. It MUST NOT make application-specific retry decisions.

## 10. Event stream and reconnect

Execution events MUST have monotonically increasing sequence numbers scoped to one execution generation.

Events SHOULD include:

- accepted;
- preparing;
- started;
- stdout chunk;
- stderr chunk;
- resource sample when enabled;
- artifact-ready;
- cleanup;
- terminal state.

Retained live event history MUST be bounded.

A reconnecting client supplies a sequence cursor. If history is retained, streaming resumes from that point. If the cursor predates retention, the node returns a typed resynchronization response with the oldest retained sequence, current sequence, and current execution snapshot.

No client may be told that a discontinuous stream is continuous.

## 11. Workspace and artifact model

The portable initial workspace representation is content-addressed, not Git-specific.

A workspace manifest maps bounded relative paths to typed entries such as files, directories, and supported link forms. File content is addressed by cryptographic digest.

The initial content service SHOULD support:

- find missing digests;
- upload blob;
- fetch blob where authorized;
- validate manifest;
- materialize immutable or execution-private workspace;
- collect only declared outputs;
- publish result artifacts by digest/handle.

SHA-256 is the default initial digest unless a later ADR changes the required set.

Materialization MUST defend against traversal, symlink escape, device nodes, special-file confusion, case/canonicalization hazards, and digest mismatch.

Git-aware materializers MAY be added later but MUST remain adapters above the neutral workspace model. Eggwork core MUST NOT own branch integration, merge policy, or CodeGG worktree leases.

## 12. Local admission

Each node owns protective local admission.

Initial controls SHOULD include:

- maximum active executions;
- maximum preparing executions;
- total retained event bytes;
- per-execution output cap;
- workspace/blob storage quotas;
- maximum request/manifest/artifact size;
- drain/readiness state.

A saturated node returns a typed refusal, optionally with bounded retry timing advice. It MUST NOT establish a policy queue whose ordering competes with a caller scheduler.

## 13. Security model

Eggwork executes arbitrary programs and therefore treats every network-facing operation as security-sensitive.

Required properties:

- authenticated network transport;
- encrypted transport for remote operation;
- strong peer identity;
- authorization before side effects;
- bounded payloads before allocation/admission;
- request identity cannot self-grant authority;
- safe path normalization and confinement;
- deterministic secret redaction;
- no unsandboxed fallback when sandbox is required;
- no direct-route fallback when a required Eggress route fails;
- explicit transport and workspace ownership;
- rate/concurrency/storage limits;
- secure temporary-file handling;
- dependency and supply-chain review;
- audit-ready structured provenance.

The initial recommended network authentication is mutual TLS using the reusable Eggstack TLS substrate. Token/bootstrap enrollment MAY be added later but is not a substitute for the normal peer identity model.

## 14. Isolation and resource enforcement

Isolation and resource controls are separate dimensions.

### 14.1 Filesystem/process isolation

Linux SHOULD initially reuse/generalize the Landlock-based helper design proven in CodeGG where applicable.

Required sandbox semantics must report whether enforcement actually occurred.

Future backends MAY include additional Linux namespaces/seccomp/container mechanisms, macOS sandbox/VM approaches, Windows restricted tokens/AppContainer, or external container/microVM integrations. Such backends are capabilities, not universal guarantees.

### 14.2 Resource enforcement

Resource requirements SHOULD distinguish advisory from required enforcement.

Potential enforcement backends include:

- Linux cgroups v2 for CPU, memory, PIDs, and selected IO controls;
- Unix rlimits for narrow process/file limits;
- Windows Job Objects;
- platform-specific best-effort controls when explicitly labeled as such.

If `enforcement = required` and the node cannot enforce the requested dimension, execution is rejected before spawn.

### 14.3 Network policy

Network access MUST be explicit in the execution policy. A future isolated network backend may provide none/allowlist/full modes. Until such enforcement exists on a platform, a required restrictive policy MUST be reported unsupported.

## 15. Transport and Eggstack ownership

The preferred initial control plane is HTTP/1.1 streaming over TLS/mTLS:

```text
eggwork-client
    -> eggfetch-core
        -> optional Eggress-backed dial path
            -> direct / HTTP CONNECT / SOCKS / SSH / multi-hop
    -> HTTPS/mTLS
    -> eggserve-core + eggnet-tls
    -> eggwork-server
```

Ownership rules:

- EggServe owns inbound HTTP service mechanics, cancellation-aware streaming, backpressure, and TLS termination surfaces.
- Eggfetch owns outbound HTTP/TLS client semantics.
- Eggress owns optional outbound proxy-chain transport and route failure classification.
- Eggwork owns execution protocol, authentication-to-principal mapping, lifecycle, leases, events, workspaces, artifacts, and execution policy.
- Eggup SHOULD own shared verified install/update/service-management machinery once its public interface is suitable.
- Gregg/gregg-protocol MAY supply or inspire telemetry/capability representation but `greggd` is not the execution control plane.

Eggwork MUST NOT fork those stacks merely to gain control it can obtain through supported public interfaces.

## 16. Protocol surface

The initial HTTP resource model SHOULD remain small and versioned. A candidate v1 surface is:

```text
GET    /v1/capabilities
GET    /v1/status

POST   /v1/executions
GET    /v1/executions/{id}
POST   /v1/executions/{id}/cancel
POST   /v1/executions/{id}/lease
GET    /v1/executions/{id}/events?after=<seq>

POST   /v1/blobs/find-missing
PUT    /v1/blobs/{sha256}
GET    /v1/blobs/{sha256}

POST   /v1/workspaces
DELETE /v1/workspaces/{id}
```

Exact routes are implementation detail until stabilized, but the semantic operations and ownership must remain.

JSON is appropriate for bounded control objects. Blob bodies SHOULD stream as bytes. Execution events MAY use NDJSON or another simple framed stream. A later binary protocol is allowed only if it preserves the same domain semantics.

## 17. Capability and status model

Static/semi-static node capabilities MUST be distinct from dynamic status.

Capabilities SHOULD describe:

- protocol version range;
- OS and architecture;
- supported execution modes;
- isolation backends;
- resource-enforcement backends;
- supported digest algorithms;
- workspace/artifact features;
- event resume;
- PTY support when implemented;
- hard request/output/artifact bounds.

Dynamic status SHOULD describe:

- ready/draining/degraded state;
- active executions;
- admission limit;
- local storage availability;
- optional CPU/memory/disk observations;
- observation timestamp.

A scheduler may consume these facts. Eggwork does not interpret them into placement policy.

## 18. Persistence and restart recovery

The node requires a small durable state store before restart recovery is considered complete.

Durable state SHOULD include:

- execution identity/generation/request digest;
- accepted request summary or validated spec reference;
- lifecycle/terminal state;
- ownership lease state;
- event sequence metadata;
- workspace/artifact handles;
- cleanup/recovery classification.

Restart MUST NOT blindly re-run an interrupted mutating process.

Initial recovery may conservatively mark in-flight executions interrupted after proving no surviving process ownership. Later platform-specific process adoption is optional and requires an explicit design.

Terminal executions MUST remain queryable for a bounded retention period or until policy cleanup.

## 19. Observability

Every execution SHOULD be attributable through stable execution/node/principal/provenance identifiers without exposing secrets.

Metrics SHOULD distinguish:

- accepted/rejected/busy/capability-mismatch counts;
- active executions;
- execution duration;
- output/artifact bytes;
- cancellation/timeout/interruption;
- cleanup failures;
- workspace/blob storage;
- event-resync frequency.

Logs MUST remain supplementary. Protocol clients MUST not scrape logs for execution state.

## 20. CodeGG integration

CodeGG remains the owner of:

- Job and Attempt records;
- scheduler fairness and global resource admission;
- AgentTask/AgentRun state;
- WorkOrder state;
- worktree lifecycle and Git integration;
- project/session authorization and audit;
- semantic retry policy.

Eggwork provides a remote executor boundary.

The target mapping is:

```text
CodeGG JobScheduler
    -> JobExecutionContext
        -> EggworkExecutor
            -> eggwork-client
                -> caller-selected NodeId
                    -> eggworkd
                        -> eggwork-runner
```

CodeGG cancellation maps to Eggwork cancellation. Eggwork events map to CodeGG progress. Eggwork artifacts map to RunStore/artifact handles. Eggwork terminal facts map to `ExecutorCompletion`.

CodeGG MUST NOT delegate placement or scheduling policy into Eggwork.

A later whole-AgentRun mode may execute a bounded CodeGG worker process remotely, but the authoritative AgentRun remains in CodeGG.

## 21. Platform and packaging targets

The initial intended targets are:

- Linux x86_64;
- Linux aarch64, including Raspberry Pi / Le Potato class systems;
- macOS x86_64;
- macOS arm64;
- Windows x86_64.

Additional targets MAY be qualified when dependencies support them.

The workspace MSRV target is Rust 1.89 unless an explicitly reviewed dependency requires a change.

Prebuilt binaries SHOULD be installable without a Rust toolchain. Update/service-management integration SHOULD use Eggup rather than duplicate shared lifecycle machinery once the required Eggup interfaces are stable.

## 22. Deferred capabilities

Later phases MAY add:

- Git-native/bundle-based workspace materializers;
- remote CodeGG AgentRun workers;
- reverse-connect relay;
- interactive PTY execution;
- streaming stdin;
- H2/H3 transport;
- chunked/delta blob transfer;
- shared CAS servers;
- container/microVM isolation backends;
- richer node telemetry;
- short-lived enrollment tokens and automatic certificate issuance.

These MUST NOT block a useful first release based on fixed-target noninteractive execution.

## 23. System invariants

The following MUST remain true:

1. Eggwork never becomes the caller's scheduler.
2. One canonical local runner owns finite noninteractive process lifecycle.
3. Payload identity cannot self-grant transport authority.
4. Duplicate network submission cannot silently duplicate an accepted execution.
5. Stale generations cannot control newer execution ownership.
6. Required isolation/resource controls fail closed.
7. Output/events/storage are bounded.
8. Reconnect either resumes from a valid sequence or explicitly resynchronizes.
9. Workspace materialization cannot escape its authorized root.
10. Content is accepted only under verified digest.
11. Proxy failure never silently becomes direct routing.
12. Restart never blindly repeats an interrupted non-idempotent execution.
13. CodeGG integration preserves CodeGG scheduler and domain ownership.
14. Large artifacts remain handle/content-backed rather than unbounded control-plane payloads.
15. Unsupported platform capability is explicit, not emulated by false claims.

## 24. Program completion criteria

Eggwork is ready for an initial stable release when:

- fixed-target authenticated execution works end to end;
- finite process lifecycle is bounded and platform-qualified;
- duplicate submission, cancellation, lease expiry, reconnect, and restart behavior are deterministic;
- workspace/blob transfer and declared artifacts are integrity-checked and confined;
- required isolation/resource controls are truthfully negotiated and fail closed;
- node status/capabilities are machine-readable;
- cross-platform binaries have reproducible qualification evidence;
- CodeGG can use Eggwork as a remote executor without creating a second scheduler;
- release/update/service ownership is documented;
- no unresolved high- or medium-severity correctness/security finding remains.
