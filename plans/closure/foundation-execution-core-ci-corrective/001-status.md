# Foundation Execution Core CI Corrective C001 Closure — Portable Ownership Guard and CI Enforcement

Status: closed

Source plan: `plans/implementation/foundation-execution-core-ci-corrective/001-portable-ownership-guard-ci-enforcement.md`

Corrective authority: `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md`

Predecessor closure: `plans/closure/foundation-execution-core/003-status.md` (Foundation M003, implementation `34ef0811ef80acb9fc5584c5a6db643e106f426c`)

Implementation commit: `f962b3c4098c44b7fe3165d72f76c168de5d6d86`

Reviewed baseline before implementation: `e7d9a8e5a9b66f7c68ff4236c9a074ba81e035d7` (the plan-authoring Eggwork baseline recorded at the top of the C001 implementation plan; matches the registry's "Current Eggwork implementation baseline").

## Finding

The Foundation M003 execution-ownership guard was no longer a durable CI invariant. `scripts/check_execution_ownership.py` invoked `rtk cargo metadata` internally — `rtk` is an undeclared developer wrapper, not part of the repository's Rust/Python toolchain contract — and `.github/workflows/ci.yml` did not run the guard at all. The invariant could therefore drift silently in a clean CI environment.

C001 makes the existing source/dependency-direction guard portable to a plain Rust/Python CI image, wires it into ordinary GitHub Actions on push/PR, and adds a deterministic `--prove-negative-exit` mode that exercises the failure path on every run so the scanner regression class is closed.

## Scope of changes

- `scripts/check_execution_ownership.py`
  - Dependency-direction check now invokes `cargo metadata --no-deps --format-version 1` directly. No `rtk` prefix; no fallback; no shell through a user profile.
  - Per-file scan extracted into `scan_file_for_spawns(path)` so the production scanner and the new negative-exit proof share one detection path.
  - New `--prove-negative-exit` mode writes a synthetic forbidden `std::process::Command::new(...).spawn()` into a `tempfile.TemporaryDirectory` outside `crates/`, scans it with `scan_file_for_spawns`, and exits `1` if the scanner fails to detect the spawn. The synthetic file is never inside `crates/`, so the production scan does not see it; production source is not mutated.
  - Argparse-based CLI so the new mode is a documented, distinct invocation rather than a hidden flag.
- `.github/workflows/ci.yml`
  - Two new required steps after checkout/toolchain setup, before broad Rust verification:
    - `python3 scripts/check_execution_ownership.py`
    - `python3 scripts/check_execution_ownership.py --prove-negative-exit`
  - No CI framework change, no step reordering beyond inserting these steps.

The approved process-owner allowlist (`eggwork-runner`, `eggwork-sandbox-helper`), the comment/string stripping, the imported `Command::new` detection, the forbidden dependency list, the server dependency-direction checks, and the synthetic forbidden-spawn self-test are unchanged.

## Before / after guard command ownership

| Aspect | Before | After |
|---|---|---|
| Dependency-direction invocation | `rtk cargo metadata --no-deps --format-version 1` | `cargo metadata --no-deps --format-version 1` |
| Required tooling in CI image | Local `rtk` wrapper on PATH | Standard Cargo binary from the pinned Rust toolchain |
| GitHub Actions invocation | None | Required step on push/PR (normal + `--prove-negative-exit`) |
| Negative-failure evidence | Synthetic self-test only (in-process) | Same self-test plus deterministic per-file probe via shared `scan_file_for_spawns` |

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Drop undeclared `rtk` dependency | `scripts/check_execution_ownership.py:172` invokes `["cargo", "metadata", "--no-deps", "--format-version", "1"]`. Repository-wide `rtk` search across non-plan source returns no matches. |
| Preserve scanner behaviour | `process_spawn_patterns`, `strip_comments_and_literals`, `self_test`, `APPROVED_PROCESS_OWNERS`, `FORBIDDEN_PROCESS_DEPS`, and the server dependency-direction checks in `check_dependencies` are unchanged from the M003 baseline; only the `rtk` prefix was removed. |
| Wire guard into CI | `.github/workflows/ci.yml` runs `python3 scripts/check_execution_ownership.py` and `python3 scripts/check_execution_ownership.py --prove-negative-exit` as required steps before `cargo fmt`/`clippy`/`test`/`check`. |
| No third-party Python package | Only the Python 3 standard library is imported (`argparse`, `json`, `re`, `subprocess`, `sys`, `tempfile`, `pathlib`). |
| Negative-exit proof | `--prove-negative-exit` writes a synthetic `std::process::Command::new("tool").spawn()` to a `tempfile.TemporaryDirectory`, scans it via the shared `scan_file_for_spawns` helper, and exits `1` if findings are empty. Verified locally with the actual scanner and with a forced scanner regression (returns 1 with "scanner regression" message). |
| Approved process owners unchanged | `APPROVED_PROCESS_OWNERS` still lists only `eggwork-runner` and `eggwork-sandbox-helper`. Allowlist was not broadened. |
| Existing dependency-direction enforcement intact | `check_dependencies` retains: `eggwork-core` cannot depend on `eggwork-runner`, `eggwork-server`, or `eggwork-client`; `eggwork-client` cannot depend on `eggwork-runner` or `eggwork-server`; `eggwork-server` cannot enable Tokio `process` or add a `FORBIDDEN_PROCESS_DEPS` crate; `eggwork-server` must depend on `eggwork-runner`. |
| No unrelated CI framework/refactor | Workflow file only gained two required steps; no caching changes, no matrix expansion, no extra jobs. |

## Verification actually run

Pinned toolchain: `rustc 1.89.0 (29483883e 2025-08-04)` (`cargo 1.89.0 (c24e10642 2025-06-23)` on this host).

- `python3 scripts/check_execution_ownership.py` — passed. Output: `execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)` followed by `negative scanner self-test passed; crate dependency direction passed`. Two runner-owned spawn sites and one helper-owned site match the M003 baseline.
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed. Output: `execution ownership guard negative-exit proof passed: synthetic forbidden spawn detected (\b(?:std|tokio)\s*::\s*process\s*::\s*Command\b, \bprocess\s*::\s*Command\b, imported process Command::new)`. Exit code 0.
- Forced scanner-regression simulation (monkey-patched `process_spawn_patterns` to return `[]`, then called `prove_negative_exit()`) — returned exit code 1 with the "scanner regression — synthetic forbidden spawn was not detected" message, confirming the proof path exits nonzero on regression.
- Forced production-spawn simulation (`eggwork-server/src/lib.rs` containing `std::process::Command::new("x").spawn()`, run against the real `scan_sources` against a temp root) — `scan_sources` returned `errors=['crates/eggwork-server/src/lib.rs: process creation outside approved owners (...)']`; `main()` would therefore return 1.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed (`cargo clippy: No issues found`).
- `cargo test --workspace --all-targets` — passed (`cargo test: 82 passed, 1 ignored (7 suites, 8.36s)`).
- `cargo check --workspace` — passed.
- `git diff --check` — passed (whitespace clean).
- `python3 -m py_compile scripts/check_execution_ownership.py` — passed.
- `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/ci.yml"))'` — passed.
- Hosted CI run: not exercised locally; the new steps will execute on the next hosted push/PR. The `ubuntu-latest` runner image plus `dtolnay/rust-toolchain@1.89.0` (with `rustfmt, clippy` components) supplies `python3` and `cargo`, which is the only tooling either step requires.

## Acceptance criteria check

1. Guard has no undeclared `rtk` dependency — met. `scripts/check_execution_ownership.py:172` invokes `cargo` directly.
2. A clean Rust/Python CI environment can execute it — met. Only standard-library Python and the pinned Cargo binary are required; both are present in the workflow image.
3. Ordinary CI runs the guard on push/PR — met. Two required steps added to `.github/workflows/ci.yml`; the workflow `on:` already covers push and pull_request.
4. Negative self-test still proves forbidden spawn detection — met. The existing in-process `self_test` plus the new `--prove-negative-exit` probe exercise both the detection function and the per-file scanner used in production; the regression-simulation above proves the failure path returns nonzero.
5. Approved process owners remain unchanged — met. `APPROVED_PROCESS_OWNERS` is identical to the M003 baseline.
6. Existing dependency-direction enforcement remains intact — met. `check_dependencies` retains every M003 rule; the only change is removing `rtk` from the command vector.
7. No unrelated CI framework/refactor is introduced — met. Only two required steps were added; no caching, matrix, job, or toolchain changes.

## Residual findings and disposition

- Hosted CI run for the new step has not been executed from this host. The step is wired to run on push/PR; the first hosted execution is part of the natural CI loop and will be recorded against the implementation commit on its first run. No hosted CI evidence is required to close the corrective because all behaviour is reproducible locally with the same pinned toolchain and the same standard-library Python.
- The synthetic negative-exit probe writes to a `tempfile.TemporaryDirectory` and is cleaned up on context exit. It does not touch the working tree and cannot leak state between CI runs.
- Cross-platform qualification remains Linux-only by the existing M003 scope. No platform claim is altered here.
- Disposition: **closed**.

## Registry, roadmap, and next-plan disposition

The Foundation / execution core workstream moves from `corrective required` to `closed`. The dependency-ready implementation plan table is cleared of this entry. The corrective addendum's exit conditions are all met.

Security M004 still waits on Operations M002; its blocker text is updated to remove the C001 dependency, since this closure now contributes enforced ownership-CI evidence into the Security evidence surface. Operations M002 remains independently ready and is the remaining gate for Security M004.

No other plan's blocker is affected.
