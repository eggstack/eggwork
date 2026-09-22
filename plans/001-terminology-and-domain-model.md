# Eggwork Terminology and Domain Model

Status: canonical terminology directive

Companion documents:

- `plans/000-long-term-specification.md`
- `plans/002-long-term-roadmap.md`

This document defines the canonical language used by Eggwork plans, APIs, protocol types, documentation, and integrations. When older notes or downstream code use overlapping terms, this document controls Eggwork's meaning.

## 1. Node

A **Node** is one concrete Eggwork execution endpoint identified by `NodeId`.

A node owns local process state, local workspace paths, local blob/artifact storage, local protective admission, local isolation/resource enforcement, and local execution recovery.

A node is not a scheduler and does not represent a cluster.

Hostname, IP address, URL, or socket path is a locator for a node, not its durable identity.

## 2. Controller

A **Controller** is a client-side process or application that chooses a node and requests execution.

Examples include CodeGG, Eggbench, an Eggwork CLI, CI glue, and custom Rust applications.

The controller owns placement and higher-level retry policy.

## 3. Principal

A **Principal** is the authenticated identity associated with a transport/session after verification.

Principal identity is transport-derived or server-resolved. It MUST NOT be accepted merely because a request payload says `principal = ...`.

A principal may be mapped to local policy that permits or denies execution, workspace access, artifact retrieval, or administrative operations.

## 4. Execution

An **Execution** is one accepted finite remote execution identity.

It is identified by:

- `ExecutionId`;
- `ExecutionGeneration`;
- a canonical request digest.

An execution may move through preparation, running, cleanup, and exactly one terminal state.

An execution is not a scheduler job. A downstream scheduler job may reference an Eggwork execution.

## 5. Execution generation

An **ExecutionGeneration** is a monotonically ordered ownership epoch for one `ExecutionId`.

A stale generation cannot renew, cancel, mutate, or otherwise control a newer generation.

Generation is a fencing primitive, not a retry count.

## 6. Request digest

The **request digest** is a canonical hash of the execution-defining request after normalization.

It distinguishes a safe duplicate submission of the same execution from an identity collision where the same execution ID is reused for different work.

Secrets SHOULD be represented through stable references or excluded according to the canonicalization contract rather than persisted in digest-debug material.

## 7. Lease

A **Lease** is time-bounded controller ownership of a live execution.

A lease contains a `LeaseId`, execution identity/generation, expiry, and policy-relevant state.

Transport disconnect does not itself transfer ownership. Lease expiry applies the configured orphan policy.

A lease is not a scheduler reservation.

## 8. Execution specification

An **ExecutionSpec** is the validated description of what the node is asked to execute.

It contains:

- command;
- workspace reference;
- cwd;
- environment policy/overrides;
- stdin policy;
- timeout/deadline;
- declared outputs;
- resource requirements;
- isolation requirements;
- network requirements;
- bounded provenance metadata.

An ExecutionSpec does not contain placement policy.

## 9. Command

A **Command** is the executable + argv representation used by the local runner.

The canonical command is not an opaque shell string.

An explicit shell mode is a separate requested capability.

## 10. Workspace

A **Workspace** is a node-local materialized filesystem tree made available to an execution under a stable `WorkspaceId`.

A workspace is built from a portable source description such as a content-addressed manifest.

A workspace path is a node-local locator, not workspace identity.

A CodeGG Workspace or Git worktree may be transformed into an Eggwork workspace input, but the concepts are not identical.

## 11. Workspace manifest

A **WorkspaceManifest** is a bounded portable description of workspace filesystem entries and referenced blob digests.

It describes content and metadata sufficient for safe materialization without requiring Git.

## 12. Blob

A **Blob** is immutable byte content identified by `BlobDigest`.

Initial digest authority is SHA-256 unless superseded by ADR.

The presence of a digest is an integrity claim only. Authenticity and authorization remain separate questions.

## 13. Artifact

An **Artifact** is a declared execution output that Eggwork captures and exposes through an `ArtifactId` and content metadata/digests.

Artifacts are not arbitrary recursive dumps of the entire workspace unless that behavior is explicitly requested and bounded.

Large artifact bodies do not belong inline in terminal result JSON.

## 14. Event

An **ExecutionEvent** is one ordered lifecycle/output observation with an `EventSequence`.

Events are projections of execution state, not the authoritative process itself.

Output events may be discarded from bounded retention after their sequence range ages out. A client then receives explicit resynchronization state.

## 15. Snapshot

An **ExecutionSnapshot** is a bounded point-in-time representation of the current execution state.

A snapshot contains enough information to recover client presentation after event-history expiry without claiming that expired output can be reconstructed.

## 16. Capability

A **NodeCapability** is a fact about what a node implementation can support or enforce.

Examples include protocol versions, OS/architecture, isolation backend, cgroup/Job Object support, event resume, workspace materialization, and PTY support.

Capability is not current capacity.

## 17. Status

**NodeStatus** is dynamic operational state, including readiness/drain state and current resource/admission observations.

Status is not a scheduling decision.

## 18. Local admission

**Local admission** is the node's protective check before accepting execution.

It answers whether this node can safely accept the specific request under its configured hard bounds at that moment.

It is deliberately not a queued fairness/priority scheduler.

## 19. Runner

The **Runner** is the process-execution component that owns finite child-process lifecycle:

- spawn;
- environment application;
- stdin;
- stdout/stderr drain;
- timeout;
- cancellation;
- process-tree termination;
- sandbox handoff;
- terminal classification;
- cleanup diagnostics.

The runner has no network-facing scheduling semantics.

## 20. Server / eggworkd

The **Server** or **eggworkd** is the authenticated node service.

It owns protocol handling, principal resolution/authorization, request validation, local admission, execution registry, leases, event publication, workspace/artifact services, runner invocation, and restart reconciliation.

It does not own global placement.

## 21. Client

The **Eggwork client** is the Rust library used to communicate with one explicit node.

It owns protocol negotiation, request submission, lease renewal, event consumption/resume, blob/workspace transfer, cancellation, and result retrieval.

It may use Eggress to establish its route. Route selection policy remains the embedding application/caller's responsibility.

## 22. Route

A **Route** describes how the client reaches the node.

Examples include direct, HTTP CONNECT, SOCKS, SSH, or multi-hop Eggress route.

Route does not identify the node and must be credential-redacted in ordinary diagnostics.

## 23. Rejection

A **Rejection** is a pre-execution refusal with no child-process side effect.

Examples include unauthorized, invalid request, busy, draining, capability mismatch, impossible resource requirement, invalid workspace, or digest mismatch.

Rejection is distinct from a process failure.

## 24. Failure

A **Failure** is a typed fact about an accepted execution, transport, materialization, or cleanup.

Failure classifications are retry-neutral unless explicitly documented. The caller decides retry policy.

## 25. Cancellation

**Cancellation** is an explicit idempotent request to stop an accepted execution generation.

Cancellation attempts process-tree cleanup and converges to a terminal state.

A stale controller cannot cancel a newer generation.

## 26. Timeout

A **Timeout** occurs when the node-owned execution deadline expires.

Timeout is distinct from controller disconnect and explicit cancellation.

## 27. Interrupted

**Interrupted** is the conservative terminal/recovery classification used when an execution cannot be proven to have completed normally, commonly after daemon restart.

Interrupted MUST NOT imply safe automatic retry.

## 28. Detached lifetime

A **Detached** execution is one explicitly permitted to outlive normal controller lease expiry under a separately defined policy.

Detached is opt-in. It is not the default consequence of lost connectivity.

## 29. CodeGG mapping

The following names MUST remain distinct:

| CodeGG | Eggwork |
|---|---|
| Job | Execution |
| Attempt | Execution/generation reference according to adapter semantics |
| AgentRun | no equivalent; may own one or more Eggwork executions |
| Worktree | workspace source/input, not Eggwork worktree authority |
| ResourcePermitGuard | caller-side scheduler admission; not Eggwork local admission |
| RunStore artifact | may reference/import an Eggwork Artifact |
| WorkspaceId | CodeGG identity; may map to Eggwork workspace transfer/materialization |

The adapter must preserve both domains rather than aliasing them into one type.

## 30. Prohibited terminology shortcuts

Avoid these ambiguous phrases in normative Eggwork docs:

- "job scheduler" for eggworkd;
- "cluster queue";
- "worker selection";
- "retry automatically elsewhere";
- "workspace path" when durable workspace identity is meant;
- "authenticated because the request says user X";
- "sandboxed" when only best-effort process limits were applied;
- "memory limit" when it is only an admission hint;
- "resumed output" after an event-history gap without explicit resync.
