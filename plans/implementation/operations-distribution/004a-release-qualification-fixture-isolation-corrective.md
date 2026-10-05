# Operations M004a — Qualification helper fixture isolation corrective

Status: **ready**

Class: corrective / test-infrastructure invariant

Source authority:

- `plans/closure/operations-distribution/004-status.md` unresolved low finding for intermittent Linux `ETXTBSY`;
- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`;
- `plans/003-planning-process.md#verification-philosophy`.

## 1. Objective

Remove the release/security qualification race in which concurrent tests stage or replace the same `eggwork-sandbox-helper` executable and one execution intermittently fails with Linux `ETXTBSY` ("Text file busy").

Qualification evidence must be deterministic. A rerun is evidence that the race is intermittent, not a fix.

## 2. Baseline

Operations M004 recorded one same-head CI failure in the required-Landlock qualification path with:

```text
Spawn("Text file busy (os error 26)")
```

The immediate rerun passed.

The closure finding attributes the failure to test staging: multiple fixtures use a shared helper location derived from the build output/staging path. A test may therefore try to replace or rewrite an executable while another test is executing it.

There is no evidence of a product runtime defect: installed releases use immutable installation-owned helper bytes. The defect is that the test harness does not model that installation ownership independently per fixture.

## 3. Invariant

A test or qualification fixture must never mutate, truncate, overwrite, rename over, or otherwise restage an executable path that another concurrently runnable test may execute.

Build output is input to fixtures, not a mutable installation directory.

Each test that needs an installed/trusted helper must receive its own fixture-owned path, with the ownership/mode semantics the test intends to prove.

## 4. Required changes

1. Identify the shared helper-staging utility/path used by the Landlock/security/release tests implicated by the closure finding.
2. Keep the Cargo-produced helper binary read-only as source input.
3. Copy/materialize it into a unique per-test temporary installation directory before trust/execution checks.
4. Apply required executable permissions to the copy without mutating the shared build artifact.
5. Pass the unique path into `TrustedLandlockSetup` / operator fixture configuration.
6. Ensure cleanup happens only after that test's target process and descendants have converged.
7. Remove any retry/sleep workaround whose only purpose would be to mask `ETXTBSY`.

Prefer one reusable test-fixture helper so security, runner capability, and release qualification tests cannot reintroduce divergent staging behavior.

## 5. Concurrency semantics

Tests must remain safe under the repository's normal parallel test execution.

Do not globally serialize the entire test suite unless a genuinely global resource exists. The helper executable is not such a resource: independent fixture copies can execute concurrently.

If a test needs to validate trust rejection for writable/unowned/symlinked helpers, mutate only that test's private copy/path.

## 6. Required tests

Add focused coverage proving:

- two concurrently created helper fixtures have distinct executable paths;
- both may execute/probe concurrently;
- changing permissions/replacing one fixture cannot affect the other;
- trust/version negative tests remain meaningful;
- required Landlock qualification remains non-vacuous.

Then run the affected Linux qualification test repeatedly with parallel test execution. The closure evidence should include a bounded stress repetition on the same head; do not convert the repetition into a permanent high-cost CI loop unless evidence shows it is necessary.

## 7. Verification

At minimum:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
python3 scripts/check_execution_ownership.py
python3 -m unittest discover -s tests/release
git diff --check
```

On Linux, repeat the previously flaky focused suite enough times to exercise parallel fixture creation and record the count in closure evidence.

## 8. Compatibility and non-goals

No production API, protocol, release format, sandbox policy, or helper trust rule changes are authorized.

Do not:

- loosen helper ownership checks;
- retry process spawn on `ETXTBSY`;
- change Landlock policy;
- serialize unrelated tests;
- move production execution to a test-only path.

## 9. Acceptance criteria

1. No concurrently runnable tests share a mutable helper installation path.
2. The Cargo-built helper is never overwritten by a test fixture.
3. The formerly flaky qualification path passes bounded repeated parallel execution on the same head.
4. Landlock positive and negative trust tests retain their original semantics.
5. Production code behavior is unchanged.
6. Operations M004's open `ETXTBSY` finding can be closed with direct evidence rather than a rerun.

## 10. Stop conditions

Stop and re-plan if the failure reproduces after fixture path isolation and evidence points to production helper launch/trust behavior rather than test staging.

## 11. Closure evidence

Create:

- `plans/closure/operations-distribution/004a-status.md`.

Record the original failure, exact fixture correction, focused parallel/stress evidence, ordinary CI evidence, and the updated M004 unresolved-findings disposition.
