# Operations M007 — Windows service host

Status: **ready**

Class: corrective capability

Numbering: this is **Eggwork** Operations M007. It is unrelated to Eggup M007,
which supplied the `commit_with_post_commit` seam and appears in the registry
under the explicit "Eggup M007" name. The two are kept distinct because they
are separate projects with separate milestone sequences.

Source references:

- `plans/subsystems/operations-distribution-roadmap.md` — M004 closure
  disposition and the explicit statement that making the daemon a Windows
  service host is a separate product milestone;
- `plans/closure/operations-distribution/004-status.md` §18 item 2 (outstanding
  evidence) and §17 (Windows service management is `unsupported`);
- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`
  §15 (platform support disposition vocabulary) and §14 (publication boundary);
- `000-long-term-specification.md` — node software is installed and managed as
  service software on every platform it claims to support.

## 1. Objective

Make `eggworkd` a real Windows service: when the Service Control Manager starts
the daemon as a service, the daemon must connect to the SCM dispatcher, report
`SERVICE_RUNNING`, and translate `SERVICE_CONTROL_STOP`/`SHUTDOWN` into the
existing graceful drain. A registered service that cannot reach `Running` is
not service software, and Operations M004 proved that the current registration
produces Windows error `1053` because nothing ever calls the dispatcher.

## 2. Baseline / problem statement

`v0.1.1` and `v0.1.2` register the service through the retained
`deployment::windows_scm_manager` adapter and then time out:

```
service install exit=0 output={"backend":"windows-scm","completed":true,...}
service start  exit=2 output=eggworkd: service manager failed: start service
                     failed in Windows SCM (code 1053)
```

The cause is structural, not a bug in the adapter. `eggup-service` owns
*registration*; nothing in `eggworkd` owns *hosting*. A service process must:

1. call `StartServiceCtrlDispatcherW` within a bounded time of service start;
2. supply a service-type/control callback;
3. call `SetServiceStatus` with `SERVICE_START_PENDING` and then
   `SERVICE_RUNNING`, or SCM reports `1053`;
4. block on its main thread until a stop control arrives.

`eggworkd` today has none of this. `crates/eggwork-server/src/bin/eggworkd.rs`
parses a subcommand, and `run` serves the node until a signal; it has no
service-entry path at all.

`v0.1.2` closes the *safety* half of this properly: the service verbs fail
closed before mutating any state, so a broken service can no longer be
installed as if it worked. That is the correct interim state and it is what
makes this milestone safe to schedule. This plan replaces the refusal with a
real host; the refusal must not be removed before the host is proven.

## 3. Scope

- A `service-host` code path in the daemon that runs the node under the SCM
  dispatcher on Windows.
- SCM service status reporting, including `SERVICE_START_PENDING` with a
  progress hint and a bounded `SERVICE_RUNNING` transition after the node is
  actually serving.
- Translation of `SERVICE_CONTROL_STOP` and `SERVICE_CONTROL_SHUTDOWN` into
  the existing drain-then-exit semantics used by the Unix service paths.
- `SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN` advertised controls.
- A service definition whose binary path and arguments invoke that path.
- Hosted qualification on a real Windows host through the existing
  `operational-qualification.yml` service stage.
- Documentation and support-matrix updates.

## 4. Invariants and non-goals

Invariants that must not change:

- Linux `systemd` and macOS `launchd` lifecycle, drain, and restart composition
  stay byte-identical in behaviour. `M002a` suppression of restart after
  `TransactionDisposition::RecoveryRequired` is untouched.
- The service path may not weaken any security control. In particular a service
  must not run with a different isolation or authorisation posture than a
  foreground daemon, and the sandbox helper must still be the only path to
  required Landlock isolation.
- Update and rollback remain Eggup-owned composition. This milestone adds a
  host, not a new update authority.
- A denied or unsupported capability must still refuse before mutation. The
  fail-closed refusal from `caa3918` is the fallback whenever the host is
  unavailable, and it must remain reachable.

Non-goals:

- Windows required filesystem isolation (no Landlock equivalent; stays
  `unsupported`).
- Network isolation (`Disabled`/`AllowListed` stay `unsupported` everywhere).
- Automatic service recovery configuration, failure actions, or dependency
  ordering between services.
- Replacing the operator CLI, or changing any subcommand's existing meaning.
- `aarch64-unknown-linux-gnu` runtime evidence, which remains `untested`.
- Publication. §14 still applies unchanged.

## 5. Required production changes

1. **A Windows service-entry path.** `eggworkd` gains a way to be started *as a
   service* that is distinct from the interactive `run` path. Two acceptable
   shapes, chosen by what the evidence shows rather than by preference:
   - a `service-host` subcommand that the SCM `ImagePath` invokes; or
   - automatic detection at startup (for example `GetConsoleProcessList`
   reporting no console) with the same code path underneath.
   Detection is the better long-term shape because it keeps the registered
   binary path stable across releases, but it must fail closed if detection is
   ambiguous, and it must never silently convert an interactive session into a
   service session.

2. **The dispatcher.** `StartServiceCtrlDispatcherW` with a
   `SERVICE_TABLE_ENTRY`. `windows-sys` 0.61 is already in the lock file as a
   transitive dependency, so the FFI surface is available without a new crate;
   a direct dependency must be declared target-scoped for Windows.

3. **Status reporting.** `SetServiceStatus` before and after the node is
   serving. `SERVICE_RUNNING` must be reported only after `start_server`
   succeeds, so a node that fails to bind is reported `SERVICE_STOPPED` with a
   non-zero exit rather than a service that claims to be running.

4. **Control handling.** `STOP` and `SHUTDOWN` trigger the same drain the Unix
   paths use, then report `SERVICE_STOPPED`. `INTERROGATE` must be answered
   with the current status; it is what SCM and `sc query` use.

5. **A blocking main thread.** `#[tokio::main]` owns the main thread, and
   `StartServiceCtrlDispatcherW` requires the calling thread to stay alive and
   to not return early. If the dispatcher call has to happen on a dedicated
   thread, the main thread must then block on the control signal rather than
   returning from `main`. This is the most likely place to get the design wrong
   and must be settled by a testable seam, not by inspection.

6. **Service definition.** The installed definition must point at the real host
   path. Today `--windows-start-type manual` is honoured; the binary path and
   arguments must now be correct enough to reach the dispatcher.

7. **Bounded timeouts everywhere.** SCM's start timeout is external; the daemon
   must not be able to hang inside a control callback.

## 6. Failure, cancellation, restart, and contention semantics

- If the dispatcher returns `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` while not
  started by SCM, the daemon is being run interactively; it must say so and
  exit non-zero rather than serving.
- If the node fails to start under the service, the service reports
  `SERVICE_STOPPED` and the process exits non-zero. A service that reports
  `RUNNING` without a serving node is the exact failure M004 must not produce.
- Stop must be idempotent: a second stop control after a completed stop is
  acknowledged, not an error.
- Stop racing an in-flight execution must produce the same outcome as the Unix
  paths, including `RecoveryRequired` handling owned by Eggup.
- A stop control arriving during start must not leave the status handle in
  `START_PENDING` forever.

## 7. Compatibility and migration

- Additive: no protocol, schema, wire, or storage change. No capability
  negotiation change.
- Windows service management moves from `unsupported` to `service-qualified`
  only when hosted evidence exists. Until then it stays `unsupported`, and the
  fail-closed refusal stays in the code as the no-SCM fallback.
- `v0.1.0`, `v0.1.1`, and `v0.1.2` remain immutable. A new patch release
  carries this host; none of the earlier releases is re-tagged.
- An installation that already refused service mutation on `v0.1.2` needs no
  repair step: nothing was mutated.

## 8. Required tests

- A unit-level test of the status-state machine: `START_PENDING` before serving,
  `RUNNING` only after, `STOPPED` on start failure, and no transition that skips
  a state. This is the piece that is cheap to get wrong and impossible to see
  from a passing build.
- A test that `SERVICE_RUNNING` is not reported when `start_server` fails.
- A test that the control callback is `Send`/`Sync`-safe and cannot be called
  re-entrantly into the runtime in a way that deadlocks.
- A test that the interactive `run` path is unchanged and still serves without
  SCM present.
- The existing ownership guards must continue to pass, and the service path must
  not acquire an execution-authority dependency.

## 9. Required verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-targets`
- `python3 scripts/check_execution_ownership.py`
- `python3 -m unittest discover -s tests/release`
- A release that contains the host, cut through the normal immutable process.
- Hosted `operational-qualification.yml` on a real Windows host, with the
  Windows service stage asserting: install succeeds, start reaches `Running`,
  the node serves, stop reaches `Stopped`, and an update while the service is
  running completes with the documented lifecycle.

## 10. Documentation updates

- `README.md` platform support table: Windows service management moves from
  `unsupported` to `service-qualified`, citing the run.
- `architecture/distribution.md`: the service host's ownership boundary, and the
  retained refusal as the no-SCM fallback.
- Support matrix in the closure record, with the exact run and receipts.

## 11. Acceptance criteria

1. `eggworkd` started by SCM reaches `SERVICE_RUNNING` and serves.
2. `SERVICE_CONTROL_STOP` and `SHUTDOWN` drain and reach `SERVICE_STOPPED`.
3. `SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN` are advertised; `INTERROGATE`
   is answered.
4. A node that cannot start is reported `SERVICE_STOPPED` with a non-zero exit,
   never `RUNNING`.
5. No regression in the systemd or launchd lifecycle, update, or rollback
   evidence.
6. Windows service management is claimed as `service-qualified` only with hosted
   evidence, and required filesystem isolation remains `unsupported`.
7. The fail-closed refusal remains reachable and is exercised when no SCM is
   available.

## 12. Stop conditions

Stop and re-plan rather than continuing if:

- Implementing the host requires a new global queue, worker selection, or any
  other mechanism listed in `plans/003-planning-process.md` §8.
- The host cannot be made to report `RUNNING` only after the node is serving
  without a race that cannot be tested.
- A new crate is required whose maintenance or licence posture has not been
  reviewed, rather than the already-locked `windows-sys`.
- The dispatcher turns out to require changing the `start_server` signature in
  a way that affects the Unix paths; that is an architecture change, not an
  implementation detail.
- The hosted Windows service stage cannot be made to observe `Running` and
  `Stopped` from outside the process, in which case there is no evidence and
  the milestone must not close.

## 13. Closure evidence required

- Implementation commit(s) and reviewed head.
- Requirement-to-evidence matrix over §11.
- Hosted run id with the Windows service stage green, and the receipts.
- Negative evidence: the refusal path, and the start-failure path that must not
  report `Running`.
- Confirmation that `v0.1.0`/`v0.1.1`/`v0.1.2` were not moved or clobbered.
- Security and compatibility review, including the unchanged isolation posture.
- Registry and roadmap disposition.

## 14. Handoff notes

- This plan closes Operations M004 closure-record §18 item 2. It does not close
  item 1 (publication is a maintainer action) or item 3 (Windows child execution,
  which `v0.1.2` addresses separately).
- The interim fail-closed behaviour is correct and should stay until §11.1
  through §11.4 have hosted evidence. Do not remove the refusal in the same
  change that adds the host.
- Do not widen this milestone to macOS or Linux service work. Both are already
  `service-qualified`.
