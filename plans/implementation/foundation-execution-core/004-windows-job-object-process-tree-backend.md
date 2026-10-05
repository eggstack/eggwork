# Foundation M004 — Windows Job Object finite-process backend

Status: **ready**

Class: capability / invariant

Source roadmap:

- `plans/subsystems/foundation-execution-core-roadmap.md`

Canonical authority:

- `plans/000-long-term-specification.md` — canonical finite-process runner and cross-platform explicit-unsupported requirements;
- `plans/002-long-term-roadmap.md#phase-1--canonical-local-runner`;
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`;
- `architecture/execution-ownership.md`.

Related evidence:

- Operations M004 closure: `plans/closure/operations-distribution/004-status.md`;
- hosted `v0.1.5` operational qualification run `37190674127`;
- diagnosis correction `926e06a`: Windows returns `RunnerError::UnsupportedPlatform` before child creation.

## 1. Objective

Implement the Windows backend of Eggwork's one canonical finite noninteractive process runner without weakening the runner's existing lifecycle invariants.

A Windows execution admitted with no unsupported required controls must be able to start an argv process, stream and capture bounded output, accept bounded stdin, report terminal status, and converge under timeout, cancellation, output-limit termination, leader exit, and descendant cleanup. The runner must retain ownership of the complete process tree until cleanup is complete.

This milestone closes the Windows child-execution gap named by Operations M004. It does not implement Windows filesystem isolation, network isolation, SCM service hosting, or a general Windows resource-enforcement surface.

## 2. Baseline / corrected diagnosis

At the reviewed baseline, `LocalProcessRunner::run` validates the request and then contains an unconditional non-Unix refusal:

```rust
#[cfg(not(unix))]
{
    let _ = (cwd, request, output_tx);
    return Err(RunnerError::UnsupportedPlatform);
}
```

The server maps that runner error to `ExecutionFailure::Internal`. Hosted `v0.1.5` qualification reproduced the same `Failed / Internal / exit_code: null` result even after the Windows environment-shaping and output-monitor fixes were present. That evidence proves no Windows child was created.

The earlier fixes remain valid:

- `baseline_child_environment()` now supplies a Windows-shaped baseline;
- the output-monitor race is fixed and deterministically tested.

Neither is a process-tree backend.

Foundation M002 intentionally closed with non-Unix execution unsupported until a platform process-tree owner existed. M004 is therefore additive platform capability, not a rewrite of the historical M002 closure.

## 3. Ownership invariant

The backend must preserve the Foundation invariant:

> process-tree cleanup completes before the execution owner releases its lifetime permit.

A plain `tokio::process::Child` is insufficient on Windows because it owns only the leader. Spawning normally and assigning a Job Object afterwards is also insufficient: the target may execute and create descendants before Eggwork owns the tree.

The Windows path therefore must place the initial process in a Job Object before it is allowed to run and retain that Job Object until the execution has reached terminal cleanup.

No second production spawn owner may be introduced. The implementation belongs in `eggwork-runner`; `eggwork-server` continues to call only `LocalProcessRunner`.

## 4. Reviewed dependency seam

Reviewed 2026-10-05:

- `process-wrap 10.0.1`;
- license: MIT OR Apache-2.0;
- declared MSRV: Rust 1.87, below Eggwork's Rust 1.89 baseline;
- Tokio frontend available through `tokio1`;
- Windows `JobObject` wrapper uses suspended creation while assigning the process, then resumes it, avoiding a run-before-assignment window;
- the crate also exposes Tokio kill-on-drop/process wrappers.

Preferred dependency shape:

```toml
[target.'cfg(windows)'.dependencies]
process-wrap = { version = "=10.0.1", default-features = false, features = ["tokio1", "job-object", "kill-on-drop", "creation-flags"] }
```

The exact pin is deliberate because the upstream project states that MSRV may increase without a major-version bump. The implementation agent MUST re-check the current 10.0.1 API and Cargo feature graph before editing the lock file. A future dependency bump is a separate reviewed change.

If `process-wrap` cannot provide pre-run Job Object attachment, full-tree wait/termination, the required pipe access, or compatibility with Rust 1.89 without Eggwork-owned unsafe Win32 FFI, stop and re-plan. Do not replace it with a post-spawn `AssignProcessToJobObject` race.

## 5. Scope

In scope:

- Windows argv execution through `LocalProcessRunner`;
- pre-run Job Object assignment;
- complete descendant ownership for the runner lifetime;
- cwd and the existing Windows baseline environment;
- bounded null/byte stdin;
- bounded stdout/stderr draining and live event chunks;
- ordinary zero/nonzero exit;
- timeout;
- explicit cancellation;
- terminate-on-output-overflow;
- leader-exits-before-descendant cleanup;
- cleanup diagnostics;
- hosted Windows runner tests and installed production-path qualification.

Out of scope:

- Windows SCM hosting — Operations M007;
- Windows filesystem sandboxing;
- network isolation;
- Job Object memory/CPU/PID enforcement;
- PTY/interactivity;
- shell-string execution;
- changing CodeGG scheduling/placement semantics;
- changing Eggwork's public transport protocol merely to implement a platform backend.

Required filesystem/resource/network controls that are unsupported on Windows must continue to fail admission before target spawn.

## 6. Required production changes

### 6.1 Isolate platform lifecycle mechanics

Refactor only enough of the Unix-only block in `eggwork-runner` to make process-tree creation/wait/termination platform-specific while sharing request validation, command construction, pipe readers, bounded capture, timeout/cancellation selection, terminal conversion, and provenance.

Do not produce two independent runners.

A small internal platform child/process-tree seam is acceptable. It must not leak scheduler policy or expose raw platform handles through the public runner API.

### 6.2 Windows spawn

The Windows backend must:

1. construct the same sanitized `tokio::process::Command` produced by `direct_command`;
2. attach the Job Object before user code runs;
3. preserve piped stdin/stdout/stderr;
4. return a child/tree owner that remains live until all target descendants are gone;
5. map spawn/attach/resume failure to a runner setup/spawn failure without leaving a target running.

Do not inherit the caller's arbitrary environment. The Windows baseline already supplies system-root/path facts; request environment filtering remains authoritative.

### 6.3 Tree termination semantics

Unix currently uses TERM, a short grace period, then KILL for a process group. Windows has no general POSIX-soft-termination equivalent for arbitrary argv processes.

For timeout, cancellation, and output-limit termination, the Windows backend may terminate the Job Object directly. The invariant is deterministic tree convergence, not emulation of SIGTERM.

Leader exit is not sufficient for runner completion when descendants remain. The backend must wait until the owned Job Object is empty or otherwise prove all owned processes are reaped/terminated before returning.

### 6.4 Drop and failure safety

Any error after process creation must still converge the tree. Dropping the platform owner must not orphan descendants.

The dependency's kill-on-drop behavior may be used as a final guard but must not replace explicit normal-path cleanup evidence.

### 6.5 Preserve unsupported capability truthfulness

M004 does not advertise Windows isolation/resource features. Existing capability projection should remain unchanged except that ordinary `exec.argv.v1` execution now actually works on Windows.

Required filesystem isolation and required hard resource controls must still produce the existing typed capability mismatch before spawn.

### 6.6 Error truthfulness

After this milestone, an admitted ordinary Windows execution must never use `UnsupportedPlatform`.

Keep `RunnerError::UnsupportedPlatform` for genuinely unsupported compile targets if useful. Do not add a broad protocol error solely to hide backend defects. Spawn, setup, timeout, cancellation, output-limit, and cleanup outcomes must remain distinguishable through the existing result model.

## 7. Cancellation, timeout, lease, and restart semantics

- explicit cancellation terminates the complete Job Object, not only the leader;
- timeout does the same and still reports `TimedOut`;
- terminate-on-output-overflow terminates the complete tree and reports `OutputLimit`;
- lease expiry still flows through the server cancellation path and must leave no descendant;
- a server restart never replays an interrupted execution;
- a Windows runner error after process creation must not produce a surviving process whose execution record is already terminal;
- cleanup completes before the node semaphore permit is released.

No semantic retry is added.

## 8. Compatibility

This is additive for Windows and must not change Linux/macOS behavior.

Do not alter:

- Unix process-group semantics;
- Linux Landlock/helper behavior;
- systemd cgroup wrapping;
- wire schemas;
- request digest/idempotency semantics;
- workspace/artifact ownership;
- CodeGG adapter contracts.

Any necessary public `RunnerResult` or `CleanupDiagnostics` change must be compatibility-preserving. If the Windows backend requires breaking the hardened M003 runner API, stop and write a separate API-corrective plan.

## 9. Required tests

Hosted/native Windows tests must prove:

1. zero exit with expected stdout;
2. nonzero exit preserves exit code;
3. cwd is the requested authorized root/subdirectory;
4. bounded stdin reaches the target;
5. stdout and stderr stream and final byte counts agree;
6. output truncation remains bounded;
7. terminate-on-overflow kills the complete tree;
8. timeout kills the complete tree;
9. explicit cancellation kills the complete tree;
10. spawn failure is typed and leaves no job/process;
11. a leader that creates a child/grandchild and exits first does not cause early runner return;
12. cancellation after leader exit still kills remaining descendants;
13. dropping/erroring the runner owner leaves no descendant;
14. required unsupported filesystem/resource requests are refused before the target creates a marker.

The descendant fixture must be deterministic and bounded. Do not use process-name polling as the only proof; use PIDs/markers/handles owned by the fixture where possible.

Existing Unix runner tests, Landlock/resource suites, execution-ownership guard, and installed qualification suites must remain green.

## 10. Hosted qualification

Compilation is not closure evidence.

Closure requires a real `windows-latest` run that exercises:

- the runner-focused tree lifecycle fixtures;
- the existing production server/client path;
- exact installed release bytes through `installed_release_admits_execution_and_refuses_unsupported_isolation`.

The installed receipt must show:

- `state: Succeeded`;
- `exit_code: 0`;
- the expected CRLF stdout;
- no cleanup warning;
- required filesystem isolation still refused before the marker executes.

A release containing M004 is required before Operations M004 may claim Windows child execution.

## 11. Verification

At minimum:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
git diff --check
```

Also run the Windows-native targeted tests and the operational-qualification Windows installed-execution job.

Verify `cargo tree` shows `process-wrap` only in the Windows target graph and that Eggwork source still contains no new `unsafe` block.

## 12. Documentation

Update on implementation/closure:

- `architecture/execution-ownership.md`;
- Foundation roadmap;
- `plans/registry.md`;
- Operations M004 outstanding-evidence disposition;
- `.github/workflows/operational-qualification.yml` comments that still describe the disproven Unix-environment diagnosis;
- README platform support text.

Historical release rows must remain immutable evidence. Correct current-state statements rather than rewriting why earlier candidates existed.

## 13. Acceptance criteria

1. Windows ordinary argv execution succeeds through the canonical runner.
2. The initial target cannot run before Job Object ownership is established.
3. Timeout, cancellation, and output-limit termination converge the complete process tree.
4. Leader exit never releases the runner while owned descendants remain.
5. No process survives runner return in the tested cancellation/error cases.
6. Existing Unix lifecycle behavior is unchanged.
7. Unsupported required Windows controls still reject before spawn.
8. No second production spawn owner exists.
9. Rust 1.89 and repository unsafe-code policy remain satisfied.
10. Hosted installed Windows execution passes through the production mTLS client/server path.

## 14. Stop conditions

Stop and re-plan if:

- the only workable implementation assigns the Job Object after user code has started;
- a raw Win32 implementation would require Eggwork-owned `unsafe` code;
- full-tree wait/termination cannot coexist with the existing bounded pipe-drain model;
- the change requires duplicating the runner;
- required controls would need to silently downgrade;
- the implementation requires a wire/API breaking change;
- the dependency no longer supports Rust 1.89 at the reviewed version.

## 15. Closure evidence

Create:

- `plans/closure/foundation-execution-core/004-status.md`.

Record:

- implementation commit(s);
- exact `process-wrap` version/features and dependency review;
- requirement-to-test matrix;
- Windows hosted run ids;
- descendant cleanup evidence;
- installed production-path receipt;
- Unix regression evidence;
- ownership-guard result;
- unsafe-code audit;
- remaining Windows unsupported capabilities;
- Operations M004 item-3 disposition.

## 16. Handoff

This plan and Operations M007 may execute in parallel. Their production ownership areas are distinct, but both eventually touch the Windows release/qualification documentation; closure should serialize those shared-file updates.

Do not cut or publish a new release merely to test an incomplete backend. Use native tests first, then cut the next immutable candidate only after the product path is ready for hosted qualification.
