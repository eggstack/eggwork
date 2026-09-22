# Control Plane M001 — Authenticated Fixed-Target Execution

Status: active

Source roadmap:

- `plans/subsystems/control-plane-protocol-roadmap.md`

Canonical/ADR references:

- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Objective

Expose one caller-selected Eggwork node through an authenticated EggServe/Eggfetch control plane supporting capabilities, status, immediate local admission, execution, bounded streamed events/output, cancellation, and terminal result retrieval.

Idempotent retry generations/leases/restart persistence are M002 and must not be faked here.

## 2. Upstream re-check

Before implementation, verify current public APIs for:

- eggserve-core 0.2 line and eggnet-tls;
- eggfetch-core 0.2 line;
- Eggress optional listener-free route seam.

If a required server/client TLS or streaming surface is missing, record the narrow blocker rather than replacing Eggstack networking.

## 3. Protocol operations

Implement versioned semantics for at least:

- capabilities;
- status;
- execute;
- execution status/result;
- cancel;
- execution event stream.

Exact URI paths may follow the canonical candidate surface but remain pre-v1 compatibility until stabilization.

## 4. Authentication and principal

- server starts network execution listener only with valid configured identity/trust policy;
- mTLS required for the qualified remote profile;
- verified peer identity maps to a server-owned principal context;
- request JSON has no authoritative principal/role/capability field;
- authorization hook runs before admission;
- diagnostics expose bounded certificate/principal metadata, never private keys/raw unbounded chains.

A loopback development profile may be added only if explicit and unable to be mistaken for remote secure mode.

## 5. Local admission

Add immediate bounded admission:

- drain state;
- max active executions;
- request hard bounds;
- runner capability compatibility.

No persisted priority queue. No wait-list. No automatic other-node route.

Hold the owned permit through runner cleanup.

## 6. Event streaming

M001 may use a simple sequence counter and bounded live channel for current connection, but it must already:

- bound channel capacity;
- bound chunk bytes;
- watch disconnect/cancellation;
- avoid per-listener unbounded queues;
- make final result authoritative.

Full retained sequence/resume/resync belongs to M002.

## 7. Client

`eggwork-client` should expose an explicit endpoint/node connection object and methods such as:

- capabilities/status;
- execute on this client/node;
- stream/observe;
- cancel;
- fetch result.

No API named/behaving like execute-anywhere.

## 8. Failure semantics

Keep separate:

- transport/TLS;
- authentication/authorization;
- validation;
- busy/draining/capability mismatch rejection;
- runner spawn/internal failure;
- process terminal failure;
- client stream disconnect.

No transport error automatically triggers semantic resubmission in this milestone.

## 9. Tests

- local TLS server with test CA;
- valid client cert;
- missing/wrong/untrusted cert;
- authorization deny zero-side-effect;
- malformed/oversized request;
- active-limit busy;
- drain reject;
- success/nonzero output stream;
- cancellation;
- slow event consumer/backpressure;
- disconnect while process continues/cancel policy as explicitly chosen for M001;
- server shutdown cleanup;
- protocol/version mismatch;
- secret-negative logs/errors.

## 10. Acceptance criteria

1. Authenticated client can run one argv request on one selected node.
2. Unauthenticated/denied request starts no process.
3. Busy/drain returns typed rejection without queueing.
4. Runner remains only process owner.
5. Output/event transport is bounded.
6. No scheduler/worker selection API exists.
7. Remote profile fails closed without authentication.

## 11. Stop conditions

Stop if:

- EggServe cannot expose verified peer identity through a supported public API;
- Eggfetch cannot perform required mTLS without unsafe custom verification;
- implementation needs a second HTTP stack;
- local admission begins requiring priority/fairness scheduling.

## 12. Closure evidence

Create `plans/closure/control-plane-protocol/001-status.md` with TLS/auth matrix, endpoint contract, zero-side-effect denials, admission evidence, streaming/backpressure results, and upstream versions used.
