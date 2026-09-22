# Control-Plane Protocol Roadmap

Status: active roadmap

Canonical authority:

- `plans/000-long-term-specification.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Ownership boundary

This subsystem owns:

- eggwork-server authenticated request handling;
- eggwork-client fixed-target protocol;
- capability/version negotiation;
- node status and drain projection;
- transport identity to principal context;
- execution acceptance/local admission;
- execution registry;
- cancel/status/event APIs;
- request digest/idempotency/generation/lease semantics;
- event retention/resume/resync;
- durable node execution state and restart reconciliation.

It does not own:

- node selection;
- global queueing/fairness;
- process spawn internals;
- workspace blob semantics;
- application retry policy.

## 2. Durable invariants

1. One request targets one explicit node.
2. Unauthenticated/unauthorized requests cause zero execution side effect.
3. Local saturation returns typed refusal, not queued scheduling.
4. Transport identity is authoritative.
5. Same accepted execution identity/digest cannot spawn twice.
6. Stale generations cannot control newer generations.
7. Connection loss is distinct from lease expiry.
8. Event history is bounded; sequence gaps are explicit.
9. Restart never blindly repeats uncertain work.
10. Transport/proxy failure never silently changes route.

## 3. Milestones

### M001 — Authenticated fixed-target execution

Class: capability/infrastructure/invariant

Status: blocked on Foundation M002 closure

Implementation plan:

- `plans/implementation/control-plane-protocol/001-authenticated-fixed-target-execution.md`

Objective:

Use EggServe/Eggfetch/eggnet-tls to expose capabilities, status, one bounded execute request, output/event streaming, result/status, and cancellation for a caller-selected node.

Exit conditions:

- mTLS fixture authenticates both sides;
- principal derives from verified transport context;
- invalid auth/input creates no runner side effect;
- local active-execution cap returns busy;
- client executes/cancels/observes one process;
- no cluster selection/queue API exists.

### M002 — Idempotency, leases, resumable events, and recovery

Class: invariant/capability

Status: blocked on M001 closure

Implementation plan:

- `plans/implementation/control-plane-protocol/002-idempotency-leases-events-and-recovery.md`

Objective:

Make long-running execution safe across retries, disconnect/reconnect, stale controllers, and daemon restart.

Exit conditions:

- canonical request digest stable and tested;
- duplicate submission returns existing execution;
- identity collision is explicit;
- generations fence stale controllers;
- lease expiry policy is deterministic;
- event resume/resync is bounded;
- durable registry reconciles restart conservatively.

### M003 — Eggress route adapter and protocol compatibility hardening

Class: infrastructure/polish

Status: blocked on M002 closure and current public Eggfetch/Eggress dial compatibility

Objective:

Support optional Eggress-routed client connections without starting a proxy listener, preserve typed route failures, define protocol compatibility/generation behavior, and harden slow-consumer/backpressure limits.

Exit conditions:

- direct and Eggress-routed clients exercise identical execution semantics;
- SOCKS/HTTP CONNECT/SSH route support is limited to proven public interfaces/features;
- route failure never falls back direct;
- protocol-version mismatch is typed;
- slow event consumers cannot produce unbounded node memory.

## 4. Local admission model

Local admission is immediate bounded protection. Initial decisions may account for:

- node draining;
- active execution slots;
- preparation/materialization slots;
- hard request/output bounds;
- storage headroom;
- required node capability.

The admission result is accept or typed refusal. There is no priority queue.

## 5. Persistence model

Before M002 closes, durable records must minimally preserve:

- node generation/version as needed;
- execution ID/generation;
- canonical request digest;
- accepted/terminal state;
- lease identity/expiry;
- last event sequence/base retention sequence;
- timestamps;
- terminal result summary;
- workspace/artifact references when those subsystems exist.

Large output does not belong in the main execution row.

## 6. Verification strategy

- test CA and mTLS matrix;
- authorization zero-side-effect assertions;
- duplicate concurrent POST race;
- busy/drain refusal;
- cancel/natural-exit race;
- disconnect/lease expiry;
- stale-generation operations;
- event-retention gap/resync;
- server restart at accepted/preparing/running/cleanup/terminal boundaries;
- bounded slow reader;
- proxy-route failure behavior.
