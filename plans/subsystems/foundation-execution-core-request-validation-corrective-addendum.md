# Foundation and Execution Core Post-Closure Corrective — Runner Request Validation Parity

Status: active corrective; M005 proposed

Predecessor roadmap:

- `plans/subsystems/foundation-execution-core-roadmap.md`

Historical predecessor closures:

- `plans/closure/foundation-execution-core/002-status.md` — canonical bounded local runner
- `plans/closure/foundation-execution-core/003-status.md` — execution ownership guards and runner API hardening

Canonical authority remains:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `plans/003-planning-process.md#verification-philosophy`

Finding source:

- repository-wide code interrogation pass over the reviewed baseline; the finding is new and is not recorded in the `architecture/overview.md` divergences table.

## 1. Findings

This subsystem owns "protocol-neutral domain identities and validation" and the "bounded finite-process runner". The two are supposed to be one contract: the runner's request type is a validated projection of a core `ExecutionSpec`, and the runner is the only place a process is created. The gap below is that the runner re-states the validation rules instead of deferring to their owner, and the two copies do not agree.

### 1.1 The runner's public construction path bypasses core validation

`crates/eggwork-runner/src/lib.rs` exposes `RunnerRequest::new(argv, root)` and `RunnerRequest::set_environment(Vec<(String, String)>)` as public API. Neither performs validation. `RunnerRequest::from_spec` does call `spec.validate()` first, so the server's request path is safe. The embedding path is not.

The runner's own module documentation states the intent directly:

> *"construct a validated `RunnerRequest` from an `eggwork_core::ExecutionSpec` with `RunnerRequest::from_spec` … Request fields remain private so callers cannot bypass protocol bounds."*

The fields are private, but the public constructor and the public environment setter are the bypass the sentence claims cannot exist. The invariant the subsystem states — "Request fields remain private so callers cannot bypass protocol bounds" — is true of the field declarations and false of the API surface.

### 1.2 The runner's validation copy is materially weaker than core's for environment entries

`crates/eggwork-core/src/lib.rs` routes every environment entry through `EnvironmentEntry::new`, which requires the name to be non-empty, within `MAX_ENV_NAME_BYTES`, composed only of ASCII alphanumerics and underscore, not starting with a digit, and free of NUL bytes. `CommandSpec::validate` additionally rejects duplicate environment names with a typed `ValidationError::InvalidSyntax`.

`RunnerRequest::validate` in `crates/eggwork-runner/src/lib.rs` checks only that the name is non-empty, within `MAX_ENV_NAME_BYTES`, free of NUL, and free of `=`, and that the value is within `MAX_ENV_VALUE_BYTES` and free of NUL. It therefore accepts names containing `-` or `.`, accepts names starting with a digit, and performs no duplicate-name check at all.

`RunnerRequest::validate` is called at the top of the run path, immediately before spawn, and its result gates the `Command::env()` call. So the weaker rules are the effective gate for any embedder that uses the public constructor.

The two copies are not equivalent in the other direction either. The runner additionally rejects an empty argv entry, which core permits. The divergence is therefore asymmetric: the runner is stricter on argv and weaker on the environment. Neither direction is deliberate; both are drift.

### 1.3 The runner redefines a core domain type it already imports

`crates/eggwork-runner/src/lib.rs` defines its own `StdinPolicy` enum with a `Null` and a `Bytes` variant, its own `Debug` implementation, and its own `validate` method, duplicating `eggwork_core::StdinPolicy`. The crate already imports core's type — aliased as `CoreStdinPolicy` — and converts between the two by hand. The runner's copy is the one that decides whether stdin bytes are forwarded to the child, and its `validate` is a separate copy of core's `MAX_STDIN_BYTES` rule.

The correct pattern is already present in the same file: `OverflowPolicy` is imported from core and used directly, with no local copy. `StdinPolicy` should follow it.

### 1.4 The runner re-states the output-policy bounds instead of delegating

`RunnerRequest::validate` re-spells the capture-limit and event-chunk bounds that `eggwork_core::OutputPolicy::validate` already owns, in a second copy with its own error message.

## 2. Corrective milestone

### M005 — Runner request validation parity with the core domain contract

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/foundation-execution-core/005-runner-request-validation-parity-corrective.md`

Objective:

Make the runner's request validation a projection of the core domain contract rather than an independent restatement of it, so that a rule exists once, is enforced at the point where the request enters the runner, and cannot be weaker on the embedding path than on the protocol path.

## 3. Validation contract after M005

M005 freezes the following dispositions.

### 3.1 One owner per rule

Environment-entry name syntax, environment-entry value bounds, duplicate environment names, argv bounds, and stdin bounds each have exactly one owner: `eggwork-core`. The runner consumes them rather than re-deriving them.

The runner is permitted to be **stricter** than core where the stricter rule protects the runner's own execution contract — for example the existing rejection of an empty argv entry, which protects the child-process construction. The runner is NOT permitted to be **weaker**. A rule the runner adds must be expressed as an explicit, documented runner-level rule, not as an accidental gap in a re-derived copy.

### 3.2 The public construction path is guarded

Every public path that produces a `RunnerRequest` must leave the request in a state that satisfies the core contract, or must fail. `RunnerRequest::from_spec` satisfies this by construction. The public constructor and the public environment setter must either:

- validate on the way in and return a typed error, or
- be removed, so that `from_spec` is the only construction path.

The plan must decide which, and the decision must be recorded. Removal is preferred if the repository has no in-tree caller of the public constructor, because it makes the bypass structurally impossible rather than merely checked. If a caller exists, the constructor must validate.

### 3.3 One `StdinPolicy`

The runner uses `eggwork_core::StdinPolicy`. The local duplicate and the hand-written conversion are removed. The `OverflowPolicy` pattern is the precedent.

### 3.4 Error fidelity

A validation failure raised by the runner must remain attributable to the rule that produced it. The runner's existing `RunnerError::InvalidRequest` carries a message string; M005 must decide whether that is sufficient or whether the typed `eggwork_core::ValidationError` should be preserved, and must not discard the distinction between, for example, a too-long environment name and a duplicate environment name.

## 4. Durable invariants

These extend the subsystem's existing invariants 4 ("The runner accepts argv, not an implicitly interpreted shell string") and 5 ("Output and variable-length fields are bounded"):

1. A validation rule for a runner request has exactly one owner in `eggwork-core`.
2. The runner is never weaker than its owner on any shared rule.
3. A stricter runner-level rule is explicit, documented, and tested.
4. No public runner API can produce a request that violates the core contract.
5. `RunnerRequest` construction validates; the private-field discipline is not sufficient on its own.
6. The runner continues to accept argv and never an implicitly interpreted shell string.
7. The runner's bounds remain the core bounds; no bound is widened by this corrective.
8. The core crate continues to have no internal dependencies and no scheduler or placement semantics.
9. No change to the wire representation of any core type.
10. The historical M002 and M003 closure records remain immutable; this corrective owns the newly discovered validation-parity defect.

## 5. Compatibility and migration

No wire protocol changes. No stored format changes. No change to the `ExecutionSpec` or `CommandSpec` wire representation.

The compatibility question this corrective does raise is the runner's **public Rust API**: if the plan removes `RunnerRequest::new` or `set_environment`, that is a breaking change for any out-of-tree embedder. The plan must state who is affected. Within this repository, the server crate constructs runner requests through `from_spec`; the plan must confirm that before recommending removal.

The runner crate is published as a workspace crate and is part of the release artifact set, so an API change is a release-visible decision. It must be handled as a version-visible change, not silently.

## 6. Downstream relationship

- `eggwork-server` is the in-tree consumer of the runner's request API. It uses the validated `from_spec` path, so it is not affected by the defect, and it must remain unaffected by the fix.
- The `eggwork-sandbox-helper` crate is unaffected: it has no workspace-crate dependencies and does not construct runner requests.
- The `eggwork-client` crate is unaffected.
- This corrective does not interact with Operations M007 (Windows service host), with the control-plane shared-contract corrective, or with the security isolation-truthfulness corrective. It may be executed in parallel with all of them.

## 7. Exit conditions

M005 closes only when:

- every environment-entry rule the runner enforces is enforced by `eggwork-core` and consumed by the runner, with no weaker runner copy remaining;
- the duplicate environment-name rule is enforced on the runner's public construction path;
- the runner's local `StdinPolicy` duplicate and its hand-written conversion are removed;
- no public runner API can produce a request that violates the core contract, and that property is proven by a test rather than by inspection;
- the runner remains at least as strict as core on every shared rule, and each intentional runner-level addition is documented and tested;
- the runner's existing execution, timeout, cancellation, output-limit, and process-tree behaviours are unchanged and still green;
- no new crate dependency edge is introduced;
- the `architecture/runner-execution.md` deep dive is updated so that its description of the runner's validation matches the code, and its stale line-number citations are replaced with symbol references in the same pass;
- no high or medium finding remains without an explicit owner.
