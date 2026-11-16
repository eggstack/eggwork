# Foundation and Execution Core M001 — Repository Bootstrap and Domain Contract

Status: closed

Source roadmap:

- `plans/subsystems/foundation-execution-core-roadmap.md`

Canonical references:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Objective

Create the initial Rust 1.89 Eggwork workspace and protocol-neutral domain contract with strict crate ownership, bounded validated types, baseline quality gates, and architecture documentation.

This milestone deliberately does not execute a process and does not open a network listener.

## 2. Baseline

The repository contains planning documents only. There is no Cargo workspace, source tree, CI, architecture directory, or release configuration.

Current external research baselines are recorded in `plans/registry.md` and must be re-checked before adding dependencies.

## 3. Required workspace shape

Create a small workspace approximately:

```text
Cargo.toml
crates/
  eggwork-core/
  eggwork-runner/
  eggwork-client/
  eggwork-server/
src/bin/                 # only if root package is chosen for thin binaries
architecture/
tests/ or crate-local tests
```

Exact packaging may vary if a cleaner Cargo layout preserves these ownership boundaries.

Rules:

- `eggwork-core` MUST NOT depend on client/server/runner adapters.
- `eggwork-runner` may depend on core.
- client/server may depend on core.
- server may depend on runner.
- avoid a maximal shared dependency union that widens lean crates unnecessarily.
- no Python binding is required.
- default code should deny unsafe except narrowly justified future platform modules.

## 4. Canonical domain types

Define typed validated representations for at least:

- NodeId
- PrincipalId or transport-neutral principal identifier
- ExecutionId
- ExecutionGeneration
- LeaseId
- WorkspaceId
- ArtifactId
- BlobDigest
- EventSequence
- ProtocolVersion / version range
- ExecutionSpec
- CommandSpec
- environment entry/policy representation
- stdin policy
- output policy/bounds
- declared output specification
- ResourceRequirements
- IsolationRequirement
- NetworkRequirement
- ExecutionState
- ExecutionRejection
- ExecutionFailure
- ExecutionResult
- ExecutionEvent and bounded event metadata
- NodeCapabilities
- NodeStatus

Use opaque/newtype IDs rather than plain strings at internal APIs where practical.

## 5. Bounds and validation

Centralize explicit constants/configurable limits for:

- ID length/token syntax;
- argv count and per-entry bytes;
- environment count/name/value bytes;
- cwd/path bytes;
- metadata keys/values/count;
- declared outputs count/pattern/path size;
- stdin bytes;
- event chunk bytes;
- output capture defaults/hard maximums;
- duration/timeout ranges;
- capability-list sizes.

Validation must happen without side effects and return typed errors.

Do not accept absolute remote cwd as the portable default model. The initial type may represent a relative workspace cwd even before workspace materialization exists.

## 6. Serialization and compatibility

- Add Serde where required for future wire/storage use.
- Every future wire-facing aggregate carries an explicit schema/protocol version or is wrapped by a versioned envelope.
- Unknown additive fields should remain forward-compatible where appropriate.
- Enums expected to evolve should avoid exposing brittle exhaustive external construction.
- Define deterministic canonical serialization rules needed later for request digest, but do not prematurely freeze a public v1 wire schema.
- Add golden/round-trip fixtures for representative values.

## 7. Error taxonomy

Keep these concepts distinct from the first commit:

- validation error;
- rejection before acceptance;
- accepted execution failure;
- transport error;
- internal error.

Do not add retry recommendations to low-level error variants.

## 8. Scheduler-free static design

There must be no types or methods named/semantically equivalent to:

- submit-anywhere;
- worker selection;
- priority queue;
- fairness lane;
- cluster scheduling.

`NodeStatus` may expose active count/limit and dynamic resource facts only.

Add a short architecture document explaining local admission versus scheduling.

## 9. Repository quality foundation

Add:

- rust-toolchain or MSRV policy consistent with Rust 1.89;
- rustfmt;
- Clippy with warnings denied in CI after allowing only reviewed noise;
- cargo test;
- dependency/security audit workflow or documented command;
- minimal GitHub Actions for supported host baseline;
- license and contribution/security placeholders if repository policy expects them;
- .gitignore.

Do not create a broad release matrix yet.

## 10. Architecture documentation

Create at least:

- `architecture/overview.md` — crate/ownership map;
- `architecture/domain.md` — identities and execution model;
- `architecture/execution-ownership.md` — runner/server/scheduler-free boundary;
- `architecture/protocol.md` — future protocol layers and versioning intent, clearly marked pre-wire.

Docs must reference canonical plans instead of copying them wholesale.

## 11. Required tests

- typed ID valid/invalid cases;
- generation ordering/bounds;
- BlobDigest parsing/format validation;
- ExecutionSpec minimal valid case;
- empty/oversized argv;
- cwd absolute/traversal rejection according to chosen model;
- environment limits/NUL behavior;
- metadata limits;
- declared-output limits;
- timeout/resource/isolation enum round trips;
- unknown additive JSON behavior where intentionally supported;
- secret-safe Debug/Display for any secret-capable placeholder type.

Use property tests only where they buy meaningful parser/path/state coverage.

## 12. Required verification

At minimum:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
```

Add an exact Rust 1.89 check if the CI/toolchain environment supports it.

## 13. Acceptance criteria

1. Repository is a coherent Rust workspace.
2. Domain types match canonical terminology.
3. All wire/storage-facing variable fields are bounded or have an explicit owner for future bounds.
4. Core has no server/client/runner dependency.
5. No network listener or process execution is hidden in this milestone.
6. Scheduler-free ownership is documented and reflected in types.
7. Quality commands pass.
8. No unresolved high/medium review finding remains.

## 14. Stop conditions

Stop and report rather than improvise if:

- a required upstream crate forces Rust newer than 1.89 and cannot be avoided cleanly;
- domain types would need to embed CodeGG Job/AgentRun/worktree semantics;
- the implementation wants to add network endpoints or process spawning to demonstrate progress;
- a scheduler/queue abstraction appears necessary.

## 15. Closure evidence required

Create `plans/closure/foundation-execution-core/001-status.md` with:

- exact reviewed head/commits;
- final crate graph;
- domain-type/validation matrix;
- verification outputs;
- MSRV evidence;
- architecture doc list;
- dependency review;
- severity-ranked findings;
- registry/roadmap update.

## 16. Handoff note

This is the only dependency-ready implementation plan at project bootstrap. Do not start M002 in the same change unless M001 is first closed and the registry is advanced.
