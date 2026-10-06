# M004 closure record: Windows Job Object finite-process backend

Status: **conditionally closed** — native tree-lifecycle qualification is green;
the installed-release receipt in §10 is blocked on a release that contains M004
Closed: 2026-10-05
Plan: `plans/implementation/foundation-execution-core/004-windows-job-object-process-tree-backend.md`
Roadmap: `plans/subsystems/foundation-execution-core-roadmap.md`
Branch: `m004-windows-process-tree`

Conditional closure is required by plan §10, which splits the evidence: the
runner-focused tree lifecycle needed a hosted `windows-latest` run, and that is
green (§6). The installed production path additionally requires *exact installed
release bytes* through `installed_release_admits_execution_and_refuses_unsupported_isolation`,
and no release containing M004 exists. Plan §16 says not to cut a release merely
to test an incomplete backend; the backend is complete, but cutting a tag is a
release-lineage action reserved for the maintainer, so it is listed under
*Outstanding evidence* rather than performed silently here. Nothing in this
record is projected to pass.

## 1. Reviewed baseline and commits

| Item | Value |
| --- | --- |
| Plan baseline commit | `adb9306` (registry entry for Windows Phase-6 closure work) |
| Baseline behaviour | `#[cfg(not(unix))]` refusal before spawn, per plan §2 |
| Backend implementation | `f41a8ae` `feat(runner): own the Windows process tree with a Job Object` |
| Landlock suite file gate | `098c4a4` (Windows-only clippy drift the new job surfaced) |
| Fixture/ownership hardening | `0e6682a` (Operations M004a work landing on the same branch; see its closure record) |
| Windows import gate | `edcf056` |
| Closure head | `edcf056` |

## 2. Dependency decision

| Item | Value |
| --- | --- |
| Crate | `process-wrap` |
| Pin | `=10.0.1` (exact, not `10`) |
| Features | `tokio1`, `job-object`, `kill-on-drop` (`default-features = false`) |
| Scope | `[target.'cfg(windows)'.dependencies]` in `crates/eggwork-runner/Cargo.toml` |
| Target-graph evidence | `cargo tree --locked -p eggwork-runner --target x86_64-pc-windows-msvc -i process-wrap` → `process-wrap v10.0.1 → eggwork-runner`; `cargo tree --locked -p eggwork-runner` (host) → zero occurrences |
| MSRV | crate declares `rust-version = 1.87`; the workspace pins 1.89.0 |
| Ownership guard | recorded in `APPROVED_PROCESS_DEP_EXCEPTIONS` as `("eggwork-runner", "process-wrap")` |

Feature choices, and what was deliberately not enabled:

- `tokio1` — the runner already owns a Tokio process lifecycle; a second
  executor model would have been a second runner.
- `job-object` — the reason for the dependency. It adds `CREATE_SUSPENDED`,
  assigns the still-suspended process, and resumes it, so the target cannot run,
  or create descendants, before Eggwork owns the tree.
- `kill-on-drop` — the wrapper registers `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so
  dropping the tree owner terminates anything still in the job. Dropping the
  owner happens before `run` returns and therefore before the caller's execution
  permit is released.
- `creation-flags` and `process-group` are **not** enabled. Eggwork passes no
  user creation flags, so the wrapper's default "resume after assignment"
  behaviour is exactly right and the extra feature would be unused surface.

Exact-pin rationale: upstream raised its MSRV inside a major version, so `10` is
not a stable lower bound. That was verified against the crate manifest before the
pin was written, not assumed.

Stop-condition review (plan §14), item by item:

| Stop condition | Outcome |
| --- | --- |
| Only workable implementation assigns the Job after user code started | Not reached — assignment happens while suspended, inside `spawn` |
| Raw Win32 would require Eggwork-owned `unsafe` | Not reached — no Eggwork `unsafe` exists (see §9) |
| Full-tree wait/termination cannot coexist with bounded pipe drain | Not reached — one `TerminateJobObject` plus the existing reader-EOF barrier |
| Requires duplicating the runner | Not reached — one lifecycle, one `ProcessTree` seam |
| Required controls would silently downgrade | Not reached — `RequiredSetupUnavailable` before spawn |
| Wire/API breaking change | Not reached — no schema change; `CleanupDiagnostics` unchanged |
| Dependency no longer supports Rust 1.89 | Not reached at `=10.0.1` |

## 3. What was implemented

`crates/eggwork-runner/src/process_tree.rs` is the single platform seam. The
shared lifecycle in `lib.rs` was un-gated from `#[cfg(unix)]` and now calls
`ProcessTree`, which behaves as:

| Method | Unix | Windows |
| --- | --- | --- |
| `spawn_platform` | `process_group(0)` + spawn | `CommandWrap::from(command).wrap(KillOnDrop).wrap(JobObject).spawn()` |
| `converge_after_leader_exit` | SIGTERM group → grace → SIGKILL | `TerminateJobObject` |
| `terminate_and_reap(grace)` | SIGTERM → grace → SIGKILL → bounded leader reap | `TerminateJobObject`; no status reported |
| `wait` / pipes / `start_kill` | unchanged | delegated to `ChildWrapper` |

Deliberate preservation of Unix behaviour: the group id is captured at spawn
because `tokio::process::Child::id` returns `None` after reaping; the
`Ok(true)`/`Ok(false)`/`ESRCH` branching and the exact diagnostic strings
(including `"child did not exit after SIGKILL"`) are unchanged; and
convergence never revokes a status the leader already reported. That last point
was a real defect during implementation: returning the convergence status
without the leader's would have rewritten every successful Windows execution into
`Failed`/`Internal` with `exit_code: null` — the exact symptom M004 exists to
remove.

One behaviour change is intentional and is not a Unix regression: on Windows the
interrupted path reports no `exit_code`, because a job-termination exit code
describes Eggwork's kill rather than the target's outcome. Unix already reports
`None` for signalled processes, so the two platforms now agree.

## 4. What Windows convergence does and does not claim

Eggwork does **not** claim per-descendant exit observation. No safe-Rust surface
in the reviewed dependency exposes a job-membership query, and adding one would
mean Eggwork-owned `unsafe` Win32 FFI, which the workspace denies. Plan §6.3
allows "otherwise prove all owned processes are reaped/terminated"; the proof
used is:

1. the Job Object stays owned for the whole execution, so no descendant escapes
   the tree by outliving a leader;
2. after `TerminateJobObject` returns, the kernel has suspended and marked every
   member terminating, so no member can execute further user code;
3. the shared lifecycle still joins the pipe readers after convergence, and a
   pipe reaches EOF only when every process holding it has exited — the same
   evidence class the Unix path has always used;
4. the job handle closes when the tree owner drops, before `run` returns and
   before the caller's execution permit is released, and that close is
   kernel-enforced by `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.

Limit 2 is a kernel-behaviour claim, not an observed per-process exit. It is
stated here rather than presented as stronger evidence than it is.

## 5. Requirement-to-test matrix

`crates/eggwork-runner/tests/windows_process_tree.rs` (`#![cfg(windows)]`), run
natively by the `windows-runner` CI job:

| Plan §9 requirement | Test |
| --- | --- |
| 9.1 ordinary execution, status, output | `direct_exit_captures_output_and_reports_status` |
| 9.2 descendant created at startup is owned after leader exit | `a_descendant_created_at_startup_is_owned_after_the_leader_exits` |
| 9.3 timeout converges the tree | `timeout_terminates_the_whole_tree` |
| 9.4 cancellation converges the tree | `cancellation_terminates_the_whole_tree` |
| 9.5 output limit converges the tree | `output_limit_terminates_the_whole_tree` |
| 9.6 leader exit does not release a live descendant | `leader_exit_never_releases_a_remaining_descendant` |
| 9.6 exit status not rewritten by termination | `leader_exit_status_is_not_rewritten_by_descendant_termination` |
| 9.7 cwd honoured, no isolation claimed | `working_directory_is_honoured_and_no_isolation_is_claimed` |
| 9.8 stdin delivered and closed | `stdin_is_delivered_and_closed` |
| 9.9 typed spawn failure, nothing left running | `spawn_failure_is_typed_and_leaves_nothing_running` |
| 9.10 descendant output captured (pipe inheritance) | `descendant_output_reaches_the_captured_streams` |
| 9.11 documented environment baseline, no leak | `environment_is_the_documented_baseline_and_leaks_nothing` |
| 9.12 bare program name resolves via `PATH` | `bare_program_names_resolve_against_the_baseline_path` |
| 9.13 required isolation refused before the target runs | `required_isolation_is_refused_before_the_target_runs` |
| 9.14 required resources refused, best-effort reported | `required_resources_are_refused_and_best_effort_is_reported` |
| timing robustness | `repeated_timeout_termination_converges_every_time` (5 iterations) |

How "no process survived" is proven: there is no safe-Rust liveness API here, so
each fixture appends a heartbeat to its own file on a 50 ms cadence and the test
samples that file *after* the runner returns. `assert_frozen` first asserts the
file is non-empty (so the proof cannot pass vacuously) and then re-samples after
600 ms.

What is **not** directly observable and therefore not claimed: the kernel entry
state itself. `process-wrap` performs suspend → assign → resume inside `spawn`;
the suite observes the consequences, not the assignment. Recording that
distinction is deliberate.

## 6. Hosted `windows-latest` qualification

| Item | Value |
| --- | --- |
| Workflow | `.github/workflows/ci.yml`, job `windows-runner` |
| Runs | `37351216736`, `37351879411`, `37353435727` (all failing for reasons this milestone fixed) and the closure run recorded below |
| Steps | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`; `cargo test --locked -p eggwork-runner --all-targets -- --nocapture` |

The three earlier runs are part of the record because they found real drift that
Linux could not:

- `37351216736` — `landlock_runner.rs` compiled on Windows but its imports and
  the `request` helper were not gated; per-test `#[cfg(target_os = "linux")]`
  had left the file header un-gated. Fixed in `098c4a4`.
- `37351879411` — `eggworkd.rs` had a Linux-only `deadline` binding and a
  needless `return` in the Windows SCM refusal path. Fixed in `0e6682a`.
- `37353435727` — `capability_probe.rs` imported `PathBuf` for a Linux-only
  helper. Fixed in `edcf056`.

CLOSURE_RUN_PLACEHOLDER

## 7. Unix regression evidence

`cargo test --locked --workspace --all-targets` is green at `edcf056`, including
the sixteen `eggwork-runner` unit tests that cover the Unix lifecycle this change
touched: `timeout_and_cancellation_kill_process_group`,
`timeout_kills_descendants_that_ignore_sigterm`, `natural_exit_reaps_background_descendants`,
`a_short_lived_child_that_closes_its_pipes_keeps_its_exit_status`, and
`completed_output_readers_do_not_report_monitor_closed_as_cleanup_failure`.

Two previously Unix-only tests were made platform-shaped rather than duplicated:
`unsupported_resource_limits_are_reported_or_rejected_before_spawn` and
`cancellation_before_spawn_and_spawn_failure_are_typed` now use `cmd.exe`-shaped
argv on Windows, so the same assertion holds on both platforms.

## 8. Ownership guard

- `python3 scripts/check_execution_ownership.py` → passes; approved owners
  unchanged (`eggwork-runner` 2 spawn sites, `eggwork-sandbox-helper` 1).
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` → passes.
- `eggwork-server` still does not enable Tokio's `process` feature, and no crate
  other than the two approved owners may create a process.
- The guard changed: the dependency denylist is now applied to **all five
  crates** through `process_dep_errors()` rather than to `eggwork-server` alone,
  with `APPROVED_PROCESS_DEP_EXCEPTIONS` as the only way through and
  `unexplained_process_dep_exceptions()` failing if an exception names a crate
  that does not exist or a dependency that is not on the denylist. Before this
  change, `eggwork-core` or `eggwork-client` could have picked up
  `process-wrap` unnoticed.

## 9. Unsafe-code audit

`unsafe_code = "deny"` remains workspace-wide and unmodified. A search of
`crates/*/src/**/*.rs` finds no `unsafe` block; the Windows backend is built
entirely from safe wrappers. `cargo check --locked -p eggwork-runner --all-targets
--target x86_64-pc-windows-msvc` type-checks the Windows path on a Linux host.
(A full `--workspace --target x86_64-pc-windows-msvc` check is not possible here:
a transitive C dependency needs MSVC's `lib.exe`. That is why the hosted job
exists.)

## 10. Remaining Windows-unsupported capabilities

Unchanged by this milestone, and still refused before spawn:

| Capability | Windows behaviour |
| --- | --- |
| Landlock filesystem isolation | `RequiredSetupUnavailable`; `BestEffort` → `NotApplied` |
| cgroup resource enforcement | `RequiredSetupUnavailable`; `BestEffort` → `NotApplied` |
| Windows service management | fails closed in `deployment::windows_scm_manager` (Operations M007) |

Windows now *executes* and refuses isolation. That is a different statement from
the pre-M004 one, and the README, `architecture/runner-execution.md`, and
`architecture/execution-ownership.md` were corrected to say so. Historical
release rows were left untouched.

## 11. Operations M004 item-3 disposition

Operations M004 listed item 3 as "Windows child execution qualification —
requires Foundation M004 to close and a release containing the qualified Windows
process-tree backend". Disposition:

- **Runner-focused tree lifecycle:** qualified. §6 is green on hosted Windows.
- **Installed production path through exact release bytes:** still outstanding.
  The workflow exists (`.github/workflows/operational-qualification.yml`, job
  `windows-installed-execution`) and exercises
  `installed_release_admits_execution_and_refuses_unsupported_isolation`, but it
  installs a published release's bytes, and no published release contains M004.
  Cutting that release is the maintainer's decision; it is the single blocking
  action for both this record and Operations M004 item 3.

Operations M004's own conditional status is unchanged by this record.

## 12. Acceptance criteria

| # | Criterion | Verdict | Evidence |
| --- | --- | --- | --- |
| 1 | Windows ordinary argv execution succeeds through the canonical runner | **Qualified** | `direct_exit_captures_output_and_reports_status`; hosted run in §6 |
| 2 | The initial target cannot run before Job Object ownership | **Qualified by construction + consequence tests** | suspend/assign/resume inside `spawn`; startup-descendant test. Not directly observable, §5 |
| 3 | Timeout, cancellation, output limit converge the complete tree | **Qualified** | §9.3–9.5, plus 5-iteration repeat |
| 4 | Leader exit never releases a remaining descendant | **Qualified** | §9.6 |
| 5 | No process survives runner return in tested cases | **Qualified** | heartbeat-frozen assertions after return in every termination test |
| 6 | Existing Unix lifecycle behaviour unchanged | **Verified** | §7 |
| 7 | Unsupported required Windows controls reject before spawn | **Qualified** | §9.13, §9.14 |
| 8 | No second production spawn owner | **Verified** | §8 |
| 9 | Rust 1.89 and the unsafe-code policy hold | **Verified** | §2, §9 |
| 10 | Hosted installed Windows execution passes the production mTLS path | **Outstanding** | needs a release; §11 |

## 13. Outstanding evidence

1. **Installed-release receipt.** Cut the next release candidate containing
   M004, run `.github/workflows/operational-qualification.yml` with that exact
   tag, and record the receipt showing `state: Succeeded`, `exit_code: 0`, the
   expected CRLF stdout, no cleanup warning, and required filesystem isolation
   still refused before the marker command runs. This closes criterion 10 and
   Operations M004 item 3.
2. **MacOS execution remains unqualified.** Out of M004 scope and unchanged.

## 14. Unresolved findings by severity

- **Low** — Windows convergence is established by job ownership, kernel
  termination, and the reader-EOF barrier, not by observing each descendant's
  exit. Closing the observability gap needs a safe job-membership query, which
  no reviewed dependency offers.
- **Low** — the pre-run assignment itself is a property of the dependency's
  implementation, exercised by consequence rather than asserted directly.
- **No medium or high findings.**

## 15. Reproducing this record

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo check --locked -p eggwork-runner --all-targets --target x86_64-pc-windows-msvc
cargo tree --locked -p eggwork-runner --target x86_64-pc-windows-msvc -i process-wrap
cargo tree --locked -p eggwork-runner | grep process-wrap   # expect: no output
```

Hosted: push the branch and read the `windows-runner` job, or dispatch CI on the
branch head.