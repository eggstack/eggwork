# Eggwork Long-Term Implementation Roadmap

Status: execution roadmap for `plans/000-long-term-specification.md`

Terminology: `plans/001-terminology-and-domain-model.md`

This roadmap is dependency-ordered, not calendar-ordered. Each phase MUST leave the repository coherent and MUST include bounded implementation plans, tests, documentation, and closure evidence before dependent phases are treated as available.

## Cross-phase execution rules

Every phase MUST:

1. preserve the fixed-target scheduler-free boundary;
2. keep one canonical finite-process runner;
3. derive trusted identity from the authenticated transport/session rather than request assertions;
4. bound requests, concurrency, output, events, storage, and artifacts;
5. fail closed when required isolation/resource enforcement is unavailable;
6. preserve explicit execution identity/generation and idempotency semantics;
7. treat transport retry as distinct from semantic retry;
8. avoid silent direct/proxy, sandbox/no-sandbox, or protocol fallbacks;
9. use local deterministic fixtures for routine qualification;
10. record closure evidence before advancing hard dependencies.

## Phase 0 — Repository foundation and execution domain

### Objective

Create the Rust workspace and canonical domain boundaries before process or network capability expands.

### Deliverables

- Rust 1.89 workspace.
- Initial crates: `eggwork-core`, `eggwork-runner`, `eggwork-client`, and `eggwork-server`, with thin `eggwork` / `eggworkd` binaries as appropriate.
- Typed IDs and validated domain types for node, execution, generation, lease, workspace, blob, artifact, events, capability, status, and errors.
- Canonical ExecutionSpec/ExecutionResult model.
- Version fields from the first protocol-capable build.
- Stable error/rejection distinction.
- Baseline CI, formatting, Clippy, test, audit, and dependency-policy hooks.
- Architecture docs defining crate ownership and non-goals.
- No network listener and no child process are required to close this phase.

### Dependencies

None.

### Exit criteria

- Core domain round trips through Serde.
- Bounds reject malformed/oversized input deterministically.
- Crate dependency direction prevents core from depending on server/client adapters.
- No type implies scheduler placement semantics.
- Repository compiles on the declared MSRV.

## Phase 1 — Canonical local runner

### Objective

Implement one bounded finite-process lifecycle owner usable locally and by the future server.

### Deliverables

- argv-first execution;
- explicit cwd;
- sanitized environment policy;
- null/bounded-byte stdin;
- bounded stdout/stderr capture;
- bounded streaming chunks;
- timeout;
- cancellation;
- process-tree/session cleanup;
- typed exit/termination;
- cleanup diagnostics;
- execution provenance;
- local sandbox request/outcome seam;
- no shell fallback;
- cross-platform abstraction with explicit unsupported behavior.

### Dependencies

Phase 0.

### Exit criteria

- Runner tests prove normal exit, nonzero exit, timeout, cancellation, output overflow, descendant cleanup, invalid argv, invalid cwd, and cleanup-error preservation.
- Streaming and final capture agree on authoritative terminal state.
- No second production finite-process spawn path exists within Eggwork.

## Phase 2 — Authenticated fixed-target control plane

### Objective

Execute one request against one explicitly addressed node over the Eggstack networking substrate.

### Deliverables

- EggServe-backed server service;
- Eggfetch-backed client;
- `eggnet-tls`/rustls mTLS support;
- transport-derived principal mapping;
- capability negotiation;
- node status/readiness/drain state;
- bounded request validation before admission;
- protective local concurrency admission;
- fixed-target execute/status/cancel endpoints;
- simple streamed output/events;
- optional Eggress-backed client route adapter if the required public dial seam is available;
- no global queue and no node-selection API.

### Dependencies

Phases 0 and 1.

### Exit criteria

- Two local processes communicate through authenticated TLS with a test CA.
- Unauthenticated/unauthorized requests create no child process.
- Saturation returns typed `busy` instead of queueing.
- A proxy-route failure cannot fall back to direct.
- A caller can execute argv, observe output, cancel, and retrieve a terminal result.

## Phase 3 — Idempotency, leases, resumable events, and restart state

### Objective

Make network retries and disconnect/reconnect safe for long-running execution.

### Deliverables

- canonical request digest;
- duplicate submission handling;
- identity-conflict handling;
- execution generations;
- stale-generation fencing;
- ownership leases and renewal;
- lease-expiry orphan policy;
- monotonically sequenced execution events;
- bounded event retention;
- resume cursor and typed resync;
- durable execution registry;
- conservative restart reconciliation;
- bounded terminal retention/cleanup.

### Dependencies

Phase 2.

### Exit criteria

- repeated submission cannot duplicate one accepted execution;
- same identity with different request digest fails explicitly;
- stale generation cannot cancel/renew newer work;
- disconnect followed by reconnect resumes or explicitly resynchronizes;
- restart never blindly re-executes an interrupted process;
- terminal result is recorded exactly once.

## Phase 4 — Content-addressed workspaces and artifacts

### Objective

Move portable execution inputs and declared outputs without depending on Git or remote absolute paths.

### Deliverables

- SHA-256 blob identity;
- find-missing operation;
- streaming blob upload/download;
- blob integrity verification;
- bounded workspace manifest;
- safe materialization under node-owned roots;
- execution-private workspace lifecycle;
- declared output capture;
- artifact records and streamed retrieval;
- storage quota/GC primitives;
- traversal/symlink/special-file protections.

### Dependencies

Phase 3 for durable execution/workspace association. Blob primitives may be prototyped after Phase 0 but MUST NOT be declared integrated earlier.

### Exit criteria

- unchanged files are deduplicated by digest;
- digest mismatch is rejected;
- materialization cannot escape its root;
- execution uses only the materialized workspace;
- only declared outputs are captured by default;
- large artifact bodies stay out of control JSON.

## Phase 5 — Isolation and resource enforcement

### Objective

Make requested execution boundaries truthful and enforceable where the platform supports them.

### Deliverables

- typed required/best-effort isolation requirements;
- generalized trusted sandbox-helper seam;
- Linux Landlock filesystem isolation;
- Linux cgroups v2 enforcement for selected CPU/memory/PID dimensions;
- Unix rlimit integration where useful;
- Windows Job Object process-tree/resource control;
- explicit macOS capability set;
- network-policy capability seam;
- enforcement result in terminal evidence;
- required-capability rejection before spawn.

### Dependencies

Phases 1, 2, and 4.

### Exit criteria

- required unsupported controls fail before spawn;
- sandbox/helper setup failure never becomes unsandboxed execution;
- enforced resource exceedance produces typed terminal evidence;
- process-tree cleanup still converges under enforced limits;
- platform matrices distinguish unsupported from untested.

## Phase 6 — Operations, packaging, and node administration

### Objective

Make a node installable, diagnosable, updatable, and safely drainable.

### Deliverables

- node config;
- status/doctor commands;
- drain/undrain;
- storage inspection/GC;
- service lifecycle integration;
- logs/metrics;
- prebuilt release targets;
- checksum/provenance verification;
- Eggup integration when the required public interface is stable;
- systemd/launchd/Windows SCM support through shared machinery where possible;
- upgrade/restart reconciliation evidence.

### Dependencies

Phases 2–5.

### Exit criteria

- clean supported hosts can install and run without a Rust toolchain;
- update ownership is explicit;
- drain prevents new acceptance while existing work follows documented policy;
- upgrade/restart does not fabricate execution completion;
- operator diagnostics do not leak credentials.

## Phase 7 — CodeGG finite-job integration

### Objective

Let CodeGG use Eggwork as a remote execution backend while preserving the CodeGG scheduler and domain authorities.

### Deliverables

- CodeGG `EggworkExecutor` adapter;
- explicit `ExecutionTarget::Node(NodeId)` or equivalent caller-side choice;
- Job/Attempt to Execution provenance mapping;
- scheduler cancellation to Eggwork cancellation;
- Eggwork events to CodeGG progress;
- workspace snapshot transfer from CodeGG-owned workspace/worktree;
- returned artifacts imported/referenced by RunStore;
- terminal facts mapped to ExecutorCompletion;
- capability mismatch/busy exposed back to scheduler/caller policy;
- no automatic placement in Eggwork.

### Dependencies

Eggwork Phases 0–5 and a stable CodeGG executor integration baseline.

### Exit criteria

- CodeGG can offload build/test/lint/format/managed argv work to one selected node;
- local and remote executors produce equivalent domain-level completion semantics;
- CodeGG scheduler remains the only CodeGG global admission/fairness authority;
- duplicate transport submission does not duplicate CodeGG work;
- worktree/AgentRun ownership remains in CodeGG.

## Phase 8 — Git-aware materialization and remote CodeGG AgentRun worker

### Objective

Reduce transfer overhead and support whole delegated worker execution without moving CodeGG semantic ownership into Eggwork.

### Deliverables

- optional Git-aware workspace source adapter using commit/bundle/object/patch inputs where safe;
- explicit mapping from CodeGG managed worktree state to remote execution workspace;
- bounded CodeGG worker command/protocol;
- whole remote delegated AgentRun execution as an Eggwork process;
- structured result commit/diff/artifact return;
- no implicit parent-branch integration.

### Dependencies

Phase 7 plus stable CodeGG AgentRun/worktree interfaces.

### Exit criteria

- remote AgentRun has one authoritative CodeGG run record;
- Eggwork only owns the remote process/workspace lifecycle;
- returned Git evidence is validated before CodeGG integration;
- cancellation/restart/dirty-result behavior remains inspectable and deterministic.

## Phase 9 — Reverse-connect and interactive extensions

### Objective

Support nodes without inbound reachability and selected interactive workflows without changing the scheduler-free architecture.

### Candidate deliverables

- authenticated reverse-connect relay;
- one-node persistent outbound link;
- controller-to-node stream binding;
- relay backpressure and identity;
- no relay scheduling;
- interactive PTY execution;
- streaming stdin;
- resize/attach/resume semantics;
- bounded scrollback and typed resync.

### Dependencies

Stable execution identity, leases, transport security, and event resume from Phases 2–3.

### Exit criteria

- reverse-connected execution preserves the same ExecutionId/generation semantics;
- relay cannot select workers or queue jobs;
- PTY ownership is transport/principal bound;
- interactive reconnect uses bounded sequence/resync semantics.

## Program completion

Eggwork is substantially ready for a first stable major release when Phases 0–7 are closed with evidence. Phases 8–9 are high-value extensions and MUST NOT block the first useful fixed-target execution release.
