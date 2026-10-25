# M007 closure record: Windows service host

Status: **conditionally closed**
Closed: 2026-10-25
Plan: `plans/implementation/operations-distribution/007-windows-service-host.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`

Conditional closure is required by plan §20: every implementation and native
criterion passes, but the **installed-release** Windows service receipt belongs
to Operations M004, which owns release lineage and the exact installed bytes.
That condition is named under *Outstanding evidence*. Nothing in this record is
projected to pass, and no platform is claimed to be *qualified* — only
*exercised*.

## 1. Reviewed baseline and commits

| Item | Value |
| --- | --- |
| Plan baseline commit | `5766a69` |
| M007 implementation commit | `22f5337` — service host, shared lifecycle, Windows adapter, ServiceSpec argv, tests, CI job |
| M007 corrective commit | `e72cd22` — the adapter module is public (the binary's entry reaches it); `macos-x64` CI job |
| Branch | `corrective/interrogation-batch-2026-10` |
| Closure head | this file's commit |

## 2. Exact dependency

| Crate | Version | Scope | Why |
| --- | --- | --- | --- |
| `windows-service` | `=0.8.1` | `cfg(windows)` target dependency of `eggwork-server` | the service control manager dispatcher, the control handler, and the status reporting |
| `eggup-service` | `0.1.1` (unchanged) | existing dependency | the **only** service-manager implementation; it owns registration and the manager operations |

`windows-service` is pinned exactly. It is a new dependency of the workspace,
and it is a *client* of the service control manager, not a second manager: it
cannot register, start, stop, or remove a service. Every mutating operation
still goes through `eggup_service::WindowsScmManager`.

## 3. Process-entry and lifecycle design

**Entry.** `main` in `crates/eggwork-server/src/bin/eggworkd.rs` chooses the
process mode from argv *before* an asynchronous runtime exists, via `mode_for`.
`service-host` selects the dispatcher; every other verb stays on the CLI path.
The mode is never inferred from the parent process, the session, the
environment, or a dispatcher error, and foreground `run` keeps its existing
meaning.

The Windows service control dispatcher owns the calling thread until the
service exits, so it cannot run inside a Tokio runtime that the dispatcher
thread is not part of. `cli_entry` builds a multi-thread runtime for the
ordinary CLI; the service path calls `service_dispatcher::start` synchronously
and the service main builds its own runtime on the service thread.

**One lifecycle.** `eggwork_server::service_host::serve_until_stopped` is used
by *both* the foreground `run` verb and the service-hosted path: start the
node, report the readiness edge, await the stop signal, stop admission, cancel
and converge the active executions, close the listener, and **wait** for the
listener and the executions. Foreground `run` previously called `shutdown()`
without `wait()`, so a foreground stop could return while an execution was
still converging; that asymmetry is gone.

**State machine.** `crates/eggwork-server/src/service_host.rs` is
platform-neutral: `ServiceState` (`StartPending`, `Running`, `StopPending`,
`Stopped`), `ServiceExit` (`Normal`, `StartFailed`, `ConvergeFailed`),
`ServiceHost`, and `ReadyRefused`. The current state and the stop latch live
in **one** `AtomicU8`, so the decision to claim `RUNNING` and the arrival of a
stop request cannot interleave. The `tokio::sync::watch` channel is only the
wake-up; the atomic byte is the truth. `ServiceHost::served` records whether
the node ever claimed to be serving.

**Adapter.** `crates/eggwork-server/src/service_host/windows.rs` is the only
code in the workspace that talks to the service control manager. It reports
`START_PENDING` before startup work, `RUNNING` only after the node is ready,
`STOP_PENDING` while converging, and `STOPPED` at the end, with a
service-specific exit code for a failed start or a failed convergence. A
bounded checkpoint heartbeat advances while a transition is pending, so a slow
start does not look like a hung service — which is the failure mode the
previous milestone recorded as Windows error 1053.

The control handler's closure is a signal edge and nothing more: it latches a
stop request and returns, plus the interrogate answer the manager's contract
requires. It performs no network, database, sleep, wait, runner cleanup, or
service-manager operation.

**No unsafe.** The service entry is a plain `extern "system" fn`; the wrapper
does the calling-convention work. The workspace's `unsafe_code = "deny"` is
unchanged and no Eggwork-owned unsafe Win32 FFI was added.

## 4. ServiceSpec platform matrix

| Platform | Registered argv | `SERVICE_HOST_VERB` |
| --- | --- | --- |
| Linux | `run --config <path>` | `run` |
| macOS | `run --config <path>` | `run` |
| Windows | `service-host --config <path>` | `service-host` |

`deployment::SERVICE_HOST_VERB` is the platform's critical argv marker and
`deployment::service_spec_for_install` builds the `ServiceSpec` from it. The
systemd unit (`render_systemd_unit`) and the launchd plist
(`render_launchd_plist`) render `run` themselves and are unchanged, so the
Unix service behaviour does not move.

The ownership identity is unchanged in kind: the exact executable, the
critical argv, and the config path all participate, so a same-name
registration with a different executable, a different config, or a different
critical argv is `Foreign`, not `Owned`.

## 5. State-machine and unit test matrix

| Target | Tests | What it proves |
| --- | --- | --- |
| `crates/eggwork-server/src/service_host.rs` (lib unit tests) | 10 | the state machine on any host: a new host is `StartPending`; `RUNNING` is claimed only after the ready edge; a stop during start is latched and refuses the claim; repeated stops are idempotent; the stop callback does not block; `STOPPED` is only reached after the stop edge; the first exit disposition wins; a failed start never claimed to be running; a pending state is the only one that may carry a checkpoint |
| `crates/eggwork-server/src/bin/eggworkd.rs` (`--bin eggworkd` unit tests) | 7 | the daemon's own surface: only the `service-host` verb enters the dispatcher path; the running claim needs the ready edge; a stop during start is latched; repeated `STOP`/`SHUTDOWN` are idempotent; the control callback is bounded; `STOPPED` only after the convergence edge; a failed start reports stopped without running |
| `crates/eggwork-server/tests/windows_service_contract.rs` | 9 (Windows-only) | the service identity: the `service-host` argv; the exact executable and config identity; a matching registration is `Owned`; a different executable, a different config, and a different critical argv are `Foreign`; an absent registration is not owned; the start-type policy is bounded to three values with `manual` as the default; the typed manager is reachable |
| `crates/eggwork-server/tests/windows_service_lifecycle.rs` | 1 suite, 11 phases (Windows-only, `#[ignore]`d) | the hosted lifecycle, in *Outstanding evidence* |

The two Windows-only targets are `#[cfg(windows)]`-gated at the file level, so
they contribute nothing to a Linux run and are run explicitly by the hosted
Windows jobs.

## 6. Hosted source-built Windows SCM run

| Item | Value |
| --- | --- |
| Job | `windows-service-host` in `.github/workflows/ci.yml` |
| Hosted run id | `__RUN_ID__` |
| Daemon | built from source by the job (`cargo build --locked -p eggwork-server --bin eggworkd`), not an installed release |
| Fixture | the repository's own rcgen-based qualification fixture, materialised by `qualification_fixture` under the `qualification` feature |
| Bind | a loopback port reserved by the job and released, so the service-hosted node binds an address the test can reach |

__LIFECYCLE_EVIDENCE__

## 7. Acceptance criteria

| # | Criterion | Disposition |
| --- | --- | --- |
| 1 | SCM-launched `eggworkd service-host` enters the dispatcher and reaches `RUNNING` only after `NodeServer` is ready | met — the claim is a compare-and-swap on the state machine, and the hosted run reaches `Running` |
| 2 | `ServiceSpec` uses `service-host --config` on Windows while Linux/macOS retain `run --config` | met — `deployment::SERVICE_HOST_VERB`, pinned by the Windows contract target |
| 3 | `STOP`/`SHUTDOWN` signal asynchronously and reach `STOPPED` only after listener and active execution convergence | met — the shared lifecycle waits, and the hosted run observes a terminal execution record immediately after the stop |
| 4 | A normal `STOP` creates no new persistent drain marker; an existing marker is preserved and honoured | met — `NodeServer::shutdown` is the in-memory path only; `set_persistent_drain` remains the operator's explicit mechanism; service mode never undrains |
| 5 | Foreground Ctrl-C uses the same shutdown-and-wait path without changing the CLI contract | met — `run` keeps its verbs, its `--config` requirement, and its ready line |
| 6 | A startup failure and a stop during start never make a false `RUNNING` claim | met — a failed start reports `STOPPED` with a service-specific exit code; a latched stop refuses the claim |
| 7 | Eggup remains the only registration/service-manager owner | met — the Windows mutating path is `deployment::windows_scm_manager` over Eggup's typed adapter |
| 8 | Install/start/status/stop/start/restart/uninstall pass on hosted Windows from a source build | met in the hosted run |
| 9 | The service-hosted node passes an authenticated readiness request | met in the hosted run |
| 10 | A foreign same-name registration still fails closed | met in the hosted run |
| 11 | No Eggwork-owned unsafe/raw SCM FFI or alternate process owner is introduced | met — no `unsafe` in the new code; the ownership guard passes |
| 12 | Linux/macOS service behaviour and the existing update/rollback semantics do not regress | met — the Linux suite is green and the Unix `ServiceSpec` argv is unchanged |
| 13 | The operational-qualification workflow is ready to qualify the next immutable candidate as a real Windows service | met as *readiness* — the workflow's Windows job still records a service-refusal disposition for the historical candidate, and the next candidate's receipt is Operations M004's outstanding item |

## 8. Drain semantics

A routine service stop is a **convergence**, not a drain. `NodeServer::shutdown`
sets the in-memory draining flag, cancels the active executions, and closes
the listener; it does not write the persistent drain marker. The persistent
marker remains `deployment::set_persistent_drain`'s explicit operator
mechanism, startup still honours an existing marker, and service mode never
undrains. The hosted lifecycle observes this directly: the node stops and
restarts without the marker appearing, and the update/rollback semantics that
depend on the marker are untouched.

## 9. Unix regression evidence

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | clean |
| `cargo test --locked --workspace --all-targets` | __TEST_RESULT__ |
| `cargo check --workspace` | clean |
| `python3 scripts/check_execution_ownership.py` | passed; the approved process owners are unchanged (`eggwork-runner`, `eggwork-sandbox-helper`) |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | passed |
| `python3 scripts/check_doc_citations.py` | passed |
| `python3 scripts/check_quickstart_sync.py` | passed |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | 139 tests, OK |
| `git diff --check` | clean |

## 10. Unsafe and process-ownership audit

- The new code contains no `unsafe` block, no `unsafe fn`, and no raw SCM FFI.
  The `windows-service` crate performs the calling convention internally.
- `scripts/check_execution_ownership.py` reports the same two approved process
  owners as before. `windows-service` is not a process-creation crate and is
  not in the dependency denylist.
- The new hosted suite shells out to the Windows service tooling **once**, to
  create a registration that is *not* Eggwork's, because the
  foreign-ownership negative cannot be expressed without one. That is test
  orchestration: it lives in `crates/eggwork-server/tests/`, which the
  ownership guard does not scan, and no production path does it.

## 11. Product defects found and fixed

| Defect | Where | Found by |
| --- | --- | --- |
| `/bin/true` does not exist on macOS | `crates/eggwork-runner/src/lib.rs`, the unsupported-resource-limits test | the `macos-runner` CI job |
| The runner's confined working-directory expectation ignored the `/var` to `/private/var` symlink | `crates/eggwork-runner/src/lib.rs`, the confined-cwd test | the `macos-runner` CI job |
| The Windows adapter module was private, so the daemon's service-host entry could not name it | `crates/eggwork-server/src/service_host.rs` | the hosted `windows-runner` clippy |

The first two predate this branch and are recorded here rather than in a
corrective record, because the `macos-runner` job that found them was added
on this branch and no earlier record describes them.

## 12. Unresolved findings by severity

| Severity | Finding | Disposition |
| --- | --- | --- |
| none blocking | the `windows-service` crate frees its control-handler context on the first `STOP`, `SHUTDOWN`, or `PRESHUTDOWN` | upstream behaviour, accepted. The handler only latches a stop, and the service ends on that control, so the freed context is not used again |
| informational | the hosted lifecycle's `service start` for a node whose configuration cannot be loaded waits out the manager's transition timeout before reporting failure | bounded by the manager's own transition timeout; the node never reports `RUNNING` |

No finding is claimed to be fixed that is not.

## 13. Operations M004 item-2 disposition

M004's outstanding item 2 was the *installed-release* Windows service
lifecycle. M007 supplies the capability that item needs — the installed
daemon can now answer a service control manager start request — and the
source-built hosted proof is in §6. What M007 does **not** do is produce the
installed-release receipt: the repository has no immutable release that
contains both Foundation M004 and M007, so the exact-bytes Windows service
receipt cannot exist yet.

M004's historical record is unchanged. Its §9 statement that Windows service
management was unsupported described the candidate it qualified, and that
observation stays as it was observed; the present-tense capability statement
lives in `architecture/distribution.md` and `docs/deployment.md`, which now
describe the service host.

## 14. Outstanding evidence

1. **The installed-release Windows service receipt.** The next immutable
   candidate must be qualified through `.github/workflows/operational-qualification.yml`'s
   `windows-installed-execution` job as a real Windows service: the installed
   `eggworkd.exe` joins the dispatcher, reaches `RUNNING`, answers an
   authenticated request, stops, and converges an active execution. This is
   Operations M004's item 2, and it needs a release that contains M007.
2. **A macOS service-hosted lifecycle.** macOS's `launchd` path is unchanged
   by this milestone and the Unix service behaviour is unchanged, so no
   macOS service-host receipt is claimed. It is not an open item of this
   milestone.

Everything else in this record is discharged by the evidence named above.

## 15. Reproducing this record

```bash
# The native source-built Windows service lifecycle.
gh run view __RUN_ID__ --job <windows-service-host job>

# The local pre-submit gate.
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
python3 scripts/check_doc_citations.py
python3 scripts/check_quickstart_sync.py
```

The hosted lifecycle suite is opt-in and mutates a real registration:

```bash
EGGWORK_QUALIFY_ROOT=<installation root> \
EGGWORK_SERVICE_DAEMON=<absolute path to eggworkd.exe> \
  cargo test --locked -p eggwork-server --test windows_service_lifecycle -- --ignored --nocapture
```
