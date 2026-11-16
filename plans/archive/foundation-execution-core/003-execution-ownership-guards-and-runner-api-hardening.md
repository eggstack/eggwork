# Foundation and Execution Core M003 — Execution Ownership Guards and Runner API Hardening

Status: closed

Source roadmap:

- `plans/subsystems/foundation-execution-core-roadmap.md`

Canonical references:

- `plans/000-long-term-specification.md#4-architectural-principles`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `plans/closure/foundation-execution-core/002-status.md`

Current Eggwork baseline at plan authoring: `86f80d6c0ce5818b11ed272847656a5813f726c2`.

## 1. Objective

Close the remaining execution-ownership hardening work now that the runner, sandbox helper, resource backend, server, and operator surfaces are concrete.

This milestone does not add execution capability. It makes the existing ownership boundary difficult to bypass accidentally and narrows/stabilizes the public runner embedding surface before more downstream integrations rely on it.

## 2. Problem statement

Foundation M002 proved that `LocalProcessRunner` is the intended finite-process owner, but the repository does not yet have a durable source guard proving that future server/client/operator code cannot introduce a second child-process path.

The current runner also contains platform/resource setup details that evolved after M002. M003 should review public visibility and dependency direction now, after Landlock and cgroup integration, rather than preserve early APIs merely because they exist.

## 3. Required ownership guard

Add a repository guard under `scripts/`, with a deterministic test fixture/self-test, that fails when unapproved production code introduces process creation.

At minimum inspect Rust production sources for relevant process-spawn primitives such as:

- `tokio::process::Command`;
- `std::process::Command`;
- direct `spawn()`/`spawn_blocking` patterns when they create OS children;
- platform helper invocation wrappers.

The guard must use an allowlist based on architectural ownership, not filename substring accidents.

Initially approved ownership may include only:

- `eggwork-runner` process lifecycle/resource-wrapper code;
- `eggwork-sandbox-helper` where target/helper mechanics genuinely require process creation;
- narrowly documented build/test code excluded from production scanning.

`eggwork-server`, `eggwork-client`, and `eggwork-core` must not acquire production child-process ownership.

If Operations M002 adds Eggup, calls inside the external Eggup crate are not Eggwork spawn ownership; Eggwork-side direct service-manager process spawning remains prohibited.

## 4. Negative self-test

The guard must have a deterministic negative fixture proving it fails on a synthetic forbidden spawn.

Do not rely on "the current tree happens not to match." A broken regex/parser that always returns success must make CI fail.

Prefer a small parser/token-aware scanner if plain textual matching produces false positives that require broad exclusions.

## 5. Runner API hardening

Review `eggwork-runner` public items and classify each as:

- required stable embedding surface;
- crate-private implementation detail;
- test-only helper;
- future/platform-specific seam.

Narrow visibility where possible without making server integration awkward.

In particular review:

- `RunnerRequest` construction;
- `LocalProcessRunner` constructors;
- `ExecutionSetup` / sandbox-resource setup traits;
- process-tree policy;
- output chunk/capture types;
- sandbox/resource result types;
- platform-specific cgroup/systemd details.

No public runner type may encode scheduling priority, global placement, project fairness, or CodeGG-specific provenance.

## 6. Dependency and feature guards

Add lightweight automated checks that preserve the intended crate direction:

```text
eggwork-core
    ^
    |
eggwork-runner
    ^
    |
eggwork-server

eggwork-client -> eggwork-core
```

The guard should catch at least:

- core depending on runner/client/server;
- client depending on runner/server merely to gain execution internals;
- server introducing a second process library when the runner owns execution.

Do not build a general dependency-policy framework if a small script plus `cargo metadata` is sufficient.

## 7. Output/drain characterization

Add a focused, non-gating characterization for runner output/drain overhead.

Requirements:

- exercise representative small output and large bounded output;
- report throughput/duration in an ignored/manual test or small benchmark utility;
- do not encode unstable wall-clock thresholds in normal CI;
- prove memory retention remains bounded by configured capture/event limits;
- avoid Criterion or a new benchmark dependency unless it materially improves maintainability.

The purpose is regression visibility, not a performance score.

## 8. Documentation

Update:

- `architecture/execution-ownership.md`;
- `architecture/overview.md`;
- runner crate docs as needed;
- `AGENTS.md` or equivalent contributor guidance with the ownership guard command.

Document approved exceptions and how a future platform backend should request a new exception.

## 9. Required tests and verification

At minimum:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
python3 scripts/check_execution_ownership.py
```

If the guard is implemented in another language/tool, use the equivalent command.

Also run the guard's negative self-test.

## 10. Acceptance criteria

1. A deterministic source guard prevents new production process spawning outside approved owners.
2. The guard has a negative self-test.
3. Runner public surface is reviewed and narrowed where safe.
4. Crate dependency direction remains enforceable.
5. No scheduling/placement policy leaks into the runner API.
6. Output/drain characterization exists without brittle performance gates.
7. Existing runner/control-plane/security tests remain green.
8. No unresolved high/medium finding remains.

## 11. Stop conditions

Stop and report rather than weaken the boundary if:

- server code genuinely requires direct child creation rather than calling the runner;
- a broad allowlist would make the guard meaningless;
- hardening would require moving scheduler semantics into the runner;
- the only way to pass is to exclude an entire crate from scanning.

## 12. Closure evidence

Create `plans/closure/foundation-execution-core/003-status.md` containing:

- exact implementation SHA(s);
- approved process-owner list;
- positive and negative guard evidence;
- public API changes;
- dependency-direction evidence;
- output/drain characterization method/results;
- full verification;
- residual findings and disposition.
