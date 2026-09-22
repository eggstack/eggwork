# ADR-0003: Execution Idempotency, Ownership Leases, and Generation Fencing

Status: accepted

Date: 2026-09-22

Decision owners: project maintainers

Related specification:

- `plans/000-long-term-specification.md#8-execution-identity-idempotency-generations-and-leases`
- `plans/000-long-term-specification.md#10-event-stream-and-reconnect`
- `plans/000-long-term-specification.md#18-persistence-and-restart-recovery`

## Context

Remote callers retry network operations. Connections fail while processes continue. Controllers restart. A generic remote runner may execute mutating commands for which duplicate execution is unsafe.

A bare UUID plus "POST execute" is insufficient. Retrying after an ambiguous transport failure could start the same build, migration, formatter, deployment helper, or agent process twice.

The system also needs to prevent an old controller from cancelling or renewing work after ownership has moved forward.

## Decision drivers

- transport retry must be safe;
- mutating execution cannot assume idempotency;
- reconnect must not imply re-execution;
- stale controllers must be fenced;
- lease loss must have deterministic semantics;
- restart recovery must not blindly repeat work.

## Considered options

### Option A — At-least-once execution

Every retry may execute again.

Rejected for generic execution.

### Option B — At-most-once tied to one TCP connection

Connection close kills or invalidates the run.

Rejected because long-running work must survive transient connectivity and laptop sleep/reconnect.

### Option C — Durable identity + request digest + generations + leases

Selected.

## Decision

1. Submission identity is conceptually `(ExecutionId, ExecutionGeneration, RequestDigest)`.
2. The request digest is computed from a canonical normalized execution-defining request.
3. Same ID/generation + same digest returns the existing accepted execution/handle and does not spawn again.
4. Same ID/generation + different digest returns `ExecutionIdentityConflict`.
5. Generations are monotonically ordered ownership epochs.
6. A stale generation cannot cancel, renew, delete, attach mutating input to, or otherwise control a newer generation.
7. Every accepted live execution has an ownership lease unless an explicit detached policy says otherwise.
8. Network connection loss alone does not immediately terminate the process.
9. Lease renewal is explicit and idempotent.
10. Default lease-expiry policy terminates the owned process tree and converges the execution to a typed terminal outcome.
11. Detached lifetime is opt-in, bounded, observable, and separately authorized.
12. Cancellation is explicit and idempotent and records one terminal outcome.
13. The execution registry persists sufficient identity/lifecycle state before restart recovery is considered closed.
14. Restart does not automatically rerun an interrupted execution. Conservative interruption is preferred when process ownership/completion cannot be proven.
15. Event sequencing is generation-scoped so stale streams cannot be confused with a newer ownership epoch.

## Consequences

### Positive

- safe retries after ambiguous HTTP failures;
- clear controller handoff/fencing semantics;
- long executions survive transient network loss;
- restart recovery does not require assuming command idempotency.

### Negative

- requires durable node state;
- lease policy adds clocks/timers and race tests;
- callers must persist/reuse execution identities correctly.

## Canonical races

Implementations must define deterministic behavior for:

- duplicate submit racing initial accept;
- cancel racing natural exit;
- lease expiry racing renewal;
- generation advancement racing stale cancel;
- terminal persistence racing daemon shutdown;
- reconnect cursor against terminal cleanup.

Terminal state is single-assignment. Cleanup diagnostics may be appended without changing the chosen terminal outcome.

## Verification

Tests must prove:

- N concurrent duplicate submissions create one child process;
- digest mismatch creates no second process;
- stale generation control is rejected;
- connection drop alone does not kill before lease policy says so;
- lease expiry kills descendants and records cleanup;
- cancel/natural-exit race records exactly one terminal outcome;
- restart marks uncertain live work interrupted rather than replaying it;
- event sequence cannot cross generation boundaries silently.

## Supersession

None.
