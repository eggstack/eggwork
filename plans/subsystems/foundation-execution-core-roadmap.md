# Foundation and Execution Core Roadmap

Status: active roadmap

Canonical authority:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Ownership boundary

This subsystem owns:

- Rust workspace/crate dependency structure;
- protocol-neutral domain identities and validation;
- execution request/result/event core types;
- bounded finite-process runner;
- process-tree lifecycle and cleanup;
- local streaming/capture primitives;
- source-level execution ownership guards.

It does not own:

- HTTP/TLS transport;
- authentication policy;
- global/node placement;
- content-addressed workspace transfer;
- platform isolation/resource policy beyond runner hooks;
- CodeGG semantics.

## 2. Durable invariants

1. Eggwork core has no scheduling/placement semantics.
2. One canonical runner owns production finite noninteractive process creation.
3. Core domain types are transport-neutral.
4. The runner accepts argv, not an implicitly interpreted shell string.
5. Output and variable-length fields are bounded.
6. Timeout, explicit cancellation, nonzero exit, output-limit termination, and cleanup failure remain distinguishable.
7. A sandbox/resource helper failure cannot be mistaken for normal process exit.
8. Process-tree cleanup completes before the execution owner releases its lifetime permit.

## 3. Milestones

### M001 — Repository bootstrap and canonical domain contract

Class: infrastructure/invariant

Status: closed

Implementation plan:

- `plans/implementation/foundation-execution-core/001-repository-bootstrap-and-domain-contract.md`

Objective:

Create the Rust workspace, crate boundaries, typed IDs, validated request/result/capability/status/event models, errors, serialization, baseline CI, and architecture documentation without implementing networking or process execution.

Exit conditions:

- Rust 1.89 workspace builds;
- core types round-trip through Serde;
- explicit size/count validation exists;
- no crate dependency cycle;
- scheduler placement concepts are absent;
- CI/format/clippy/test baseline exists.

### M002 — Canonical bounded local runner

Class: capability/invariant

Status: closed

Implementation plan:

- `plans/implementation/foundation-execution-core/002-canonical-local-runner.md`

Objective:

Generalize the useful CodeGG ManagedProcessService semantics into Eggwork's canonical local runner without importing CodeGG domain ownership.

Exit conditions:

- argv process execution works locally;
- environment/cwd/stdin/output policies are bounded;
- timeout/cancellation/descendant cleanup are verified;
- typed terminal and cleanup results are preserved;
- no second production spawn owner exists.

### M003 — Execution ownership guards and runner API hardening

Class: invariant/polish

Status: closed

Implementation plan:

- `plans/implementation/foundation-execution-core/003-execution-ownership-guards-and-runner-api-hardening.md`

Objective:

Stabilize the runner embedding surface, add source-level guards against direct production spawn bypass, document approved platform helper exceptions, and close API/footprint findings before network service work treats the runner as stable.

Exit conditions:

- static guard has negative self-test;
- server/client crates cannot bypass runner ownership by dependency direction or approved guard;
- public runner types expose no scheduler policy;
- focused benchmarks characterize output/drain overhead without creating CI performance gates.

## 4. CodeGG reuse strategy

The implementation agent MUST inspect the current CodeGG `src/managed_process.rs` baseline before M002.

Reusable semantics include:

- environment sanitization;
- explicit provenance;
- bounded head/tail capture;
- streaming output chunks;
- timeout/cancellation distinction;
- Unix process-session cleanup;
- sandbox request/outcome plumbing;
- cleanup diagnostics.

Do not copy CodeGG JobId, AttemptId, scheduler, AgentRun, or permission types into Eggwork. Use neutral provenance attributes/adapters.

## 5. Verification strategy

Subsystem closure requires:

- unit tests for every validation bound;
- property tests where path/size/state-machine input benefits;
- controlled process fixtures;
- descendant cleanup tests on supported platforms;
- cancellation/timeout/output-overflow races;
- no leaked process after runner return;
- source guard against bypass;
- MSRV build.

## 6. Deferred work

PTY/interactivity is not part of this subsystem's initial closure. It belongs to later operations/distribution work and may reuse the same lifecycle principles.


## 7. Post-closure corrective

Foundation M003 remains historically closed, but a later review found that its execution-ownership guard is not run by ordinary GitHub Actions and currently invokes undeclared local wrapper `rtk` for `cargo metadata`.

Current corrective authority:

- `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md`
- C001: `plans/implementation/foundation-execution-core-ci-corrective/001-portable-ownership-guard-ci-enforcement.md` — ready.

Until C001 closes, treat CI enforcement of the ownership invariant as corrective-required rather than rewriting the M003 closure.
