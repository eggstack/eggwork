# Foundation and Execution Core Post-Closure CI Corrective Addendum

Status: active corrective; C001 ready

Predecessor:

- `plans/subsystems/foundation-execution-core-roadmap.md`
- Foundation M003 closure: `plans/closure/foundation-execution-core/003-status.md`

Canonical authority remains:

- `plans/000-long-term-specification.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Finding

Foundation M003 implemented `scripts/check_execution_ownership.py` and its negative self-test, but the invariant is not executed by `.github/workflows/ci.yml`.

The guard also invokes `rtk cargo metadata` internally. `rtk` is a local wrapper and is not part of the repository's declared CI/toolchain contract. This prevents the guard from being safely wired into a clean GitHub Actions runner without an unrelated local-tool dependency.

The original M003 closure remains immutable historical evidence. This corrective owns the newly discovered enforcement gap.

## 2. Invariant at risk

Eggwork requires one canonical finite-process owner. A future change must not be able to introduce a new production process-spawn path in `eggwork-server`, `eggwork-client`, `eggwork-core`, or another unapproved crate while ordinary CI remains green.

A source guard that only works through an undeclared developer wrapper is not a durable CI invariant.

## 3. C001 — Portable ownership guard and CI enforcement

Class: invariant/corrective

Status: ready

Implementation plan:

- `plans/implementation/foundation-execution-core-ci-corrective/001-portable-ownership-guard-ci-enforcement.md`

Objective:

Make the existing ownership/dependency guard runnable with the repository's declared Rust/Python toolchain and make it a required ordinary CI step.

Exit conditions:

- guard uses ordinary `cargo metadata` or another repository-declared portable command;
- no `rtk` dependency remains in the guard;
- positive and synthetic negative self-tests still pass;
- GitHub Actions invokes the guard;
- a CI fixture/proof shows a forbidden spawn would fail the step;
- existing workspace fmt/clippy/test/check remain green.

## 4. Non-goals

This corrective does not:

- change process ownership;
- add a new process backend;
- broaden the M003 allowlist;
- change runner behavior;
- redesign CI generally;
- alter scheduling, routing, or platform resource semantics.

## 5. Downstream relationship

Security M004 should treat C001 closure as part of the current execution-ownership evidence. Operations M002 and CodeGG integration do not need to wait for C001 unless their implementation directly alters process ownership files; merged-head closure verification must still include the guard once C001 lands.
