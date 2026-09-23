# Operations M001 — Node Operations Surface

Status: ready

Dependencies closed: Control Plane M002, Workspace/Artifact M003, and Security M003.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

## 1. Objective

Provide a bounded operator-facing node lifecycle and diagnostics surface before packaging/update automation is treated as stable.

## 2. Required commands/surfaces

Design thin CLI/admin operations for at least:

- config validation/print;
- status;
- doctor;
- drain;
- undrain;
- execution list/show;
- storage summary;
- artifact/workspace/blob inspection;
- bounded cleanup/GC trigger;
- version.

Human output is rendered from machine-readable status/report types where practical.

## 3. Drain semantics

Drain is a local acceptance state:

- new execute requests receive typed draining rejection;
- existing accepted executions continue, cancel, or terminate according to explicit operator policy;
- drain is not a priority queue;
- restart persistence of drain state is explicit;
- update/service tooling can query drain state.

## 4. Doctor

Doctor should verify:

- config parse/ownership;
- TLS identity/trust material validity without exposing secrets;
- bind/listener viability;
- workspace/blob roots;
- runner/platform capability probes;
- sandbox/resource backend readiness;
- storage headroom;
- service ownership/provenance where known.

Doctor does not execute arbitrary remote commands.

## 5. Observability

Provide bounded structured metrics/status for:

- active/limit;
- accepted/rejected categories;
- terminal outcomes;
- bytes transferred/stored;
- event resync;
- cleanup failures;
- storage use;
- backend capability/health.

Avoid cardinality explosions from raw execution IDs in default metrics.

## 6. Tests

- drain/undrain with live process;
- config invalid;
- TLS material invalid;
- storage full/readonly;
- capability backend unavailable;
- doctor secret-negative;
- list pagination/bounds;
- GC trigger while execution live;
- restart preserving documented drain behavior.

## 7. Acceptance criteria

1. Operators can tell whether a node is safe/ready and why not.
2. Drain prevents new work without creating a scheduler queue.
3. Diagnostics are machine-readable and secret-safe.
4. Cleanup commands cannot remove live referenced state.
5. Version/config/status behavior is deterministic.

## 8. Closure evidence

Create `plans/closure/operations-distribution/001-status.md` with operator command matrix and live drain/storage evidence.
