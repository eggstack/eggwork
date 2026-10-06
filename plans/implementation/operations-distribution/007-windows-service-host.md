# Operations M007 — Windows service host

Status: **ready**

Class: corrective capability

Source references:

- `plans/subsystems/operations-distribution-roadmap.md`;
- `plans/closure/operations-distribution/004-status.md` outstanding Windows service-management evidence;
- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`;
- `architecture/distribution.md`;
- `architecture/execution-ownership.md`.

Numbering: this is **Eggwork** Operations M007, unrelated to milestones in Eggup.

## 1. Objective

Make `eggworkd` an actual Windows SCM service host while preserving Eggup as the sole service-registration/manager owner.

When SCM launches the registered daemon, the process must join the service dispatcher, report `START_PENDING`, start the real Eggwork node, report `RUNNING` only after the node is serving, accept STOP/SHUTDOWN controls, perform the existing drain/shutdown convergence, and report `STOPPED`.

A registration that succeeds while the launched process never connects to SCM is not service support.

## 2. Baseline

Operations M004 hosted evidence proved that the retained Windows SCM adapter can register `eggwork-node`, but historical candidates then fail service start with Windows error 1053 because `eggworkd` is only a foreground CLI/server process.

Current `main` correctly fails Windows mutating service verbs closed before mutation. That refusal is the safe interim state and must remain until a real service host has native evidence.

This milestone changes service **hosting**, not service-manager ownership:

- Eggup remains responsible for install/start/stop/restart/uninstall and ownership inspection;
- Eggwork supplies the process that SCM can actually host.

## 3. Reviewed service-host dependency

Reviewed 2026-10-05:

- `windows-service 0.8.1`;
- license: MIT OR Apache-2.0;
- declared MSRV: Rust 1.71;
- service dispatcher, service-entry macro, control handler, and service-status APIs;
- transitive `windows-sys 0.61`.

Preferred dependency:

```toml
[target.'cfg(windows)'.dependencies]
windows-service = "=0.8.1"
```

Use the safe Rust-facing crate API. Do not add Eggwork-owned raw `StartServiceCtrlDispatcherW`, `SetServiceStatus`, or service-control FFI.

The implementation agent MUST confirm that using `windows-service` requires no `unsafe` block in Eggwork source and remains compatible with Rust 1.89 before updating the lock file.

## 4. Service entry shape

Use an explicit SCM-only daemon entry, preferably:

```text
eggworkd service-host --config <file>
```

The Eggup-owned Windows service specification should register that argv.

Do not auto-detect "service versus console" from incidental process state. Explicit argv keeps foreground `eggworkd run` deterministic and makes ownership inspection exact.

`service-host` is an internal/operator lifecycle entry, not a second node implementation. Foreground and SCM paths must converge on one shared async node-serving routine.

## 5. Runtime structure

The current binary uses `#[tokio::main]`. A Windows service dispatcher is a blocking process-level entry and the higher-level service callback runs under SCM's service thread model.

Refactor the process entry only as much as needed to keep these responsibilities explicit:

- normal CLI commands use the existing async command implementation;
- `service-host` invokes the Windows service dispatcher synchronously;
- the service callback creates/enters the Tokio runtime needed to call the same `start_server`/NodeServer lifecycle as foreground `run`;
- no second HTTP server or runner stack is introduced.

If preserving `#[tokio::main]` would require global runtime handles, hidden cross-thread state, or blocking the only runtime thread, prefer a thin synchronous `main` that constructs the runtime for the selected entry path.

Unix behavior must remain unchanged.

## 6. Required service state machine

The service host must have a testable state machine:

```text
START_PENDING
   |
   | config valid + node start + EggServe ready
   v
RUNNING
   |
   | STOP or SHUTDOWN
   v
STOP_PENDING
   |
   | persistent drain + cancel active work + server wait converges
   v
STOPPED
```

Rules:

- never report `RUNNING` before `start_server` has returned a ready NodeServer;
- node/config/bind/TLS startup failure reports `STOPPED` with a non-success service exit disposition;
- STOP and SHUTDOWN are idempotent;
- INTERROGATE returns the current state;
- advertise only controls actually handled;
- callbacks signal bounded async work and return promptly; do not perform long blocking cleanup inside the control callback;
- a stop racing startup must not leave `START_PENDING` indefinitely.

## 7. Drain and execution convergence

STOP/SHUTDOWN must reuse Eggwork's existing shutdown semantics:

1. set persistent drain / make admission stop;
2. cancel/converge active executions through NodeServer ownership;
3. shut down the HTTP server;
4. wait until active execution cleanup is complete;
5. report `STOPPED`.

Do not route service stop through a shell command or recursively invoke the CLI.

M002a's `RecoveryRequired` semantics remain untouched. Update/rollback composition remains Eggup-owned.

## 8. Service specification and ownership

Update the Windows service spec so the registered executable/arguments name the real host entry.

Ownership inspection must continue to prove:

- exact executable;
- critical argv including `service-host` and the exact config path;
- expected service identity/start type.

The M004 fail-closed refusal must not be removed before native hosted evidence demonstrates the service reaches `RUNNING`. Stage the implementation so an incomplete host cannot silently re-enable mutation.

## 9. Security boundaries

Service mode must not weaken:

- mTLS/client authorization;
- execution admission;
- runner ownership;
- workspace/artifact roots;
- capability truthfulness;
- helper/isolation policy;
- secret redaction.

Windows required filesystem isolation remains unsupported. Operations M007 does not add an isolation backend.

No direct `sc.exe`, PowerShell service-manager implementation, registry mutation, or duplicate Windows manager is allowed in Eggwork.

## 10. Required tests

### Platform-independent state-machine seam

Test without SCM where possible:

- START_PENDING -> RUNNING only after node-ready signal;
- start failure -> STOPPED, never RUNNING;
- STOP during START_PENDING converges;
- repeated STOP is idempotent;
- SHUTDOWN uses the same convergence path;
- INTERROGATE reports current state;
- no callback performs unbounded/blocking node cleanup.

### Windows-native

Hosted Windows evidence must prove:

1. Eggup service install succeeds;
2. SCM start reaches `Running`;
3. the mTLS node actually serves while SCM reports Running;
4. service status/ownership reports `Owned` and the expected argv;
5. STOP reaches `Stopped`;
6. START after STOP works;
7. RESTART works;
8. uninstall removes the registration;
9. a foreign same-name registration still causes every mutating verb to refuse;
10. node startup failure never reports Running;
11. stop while a bounded execution is active drains/cancels and leaves truthful terminal evidence;
12. running-service update/rollback preserves the M002a lifecycle contract.

## 11. Release qualification

A source-tree unit test is not sufficient to change the support matrix.

Closure requires a new immutable release candidate containing the host and an `operational-qualification.yml` Windows service stage that observes service state from outside the process.

Only after that evidence may Windows move from `unsupported` to `service-qualified`.

Foundation M004 Windows child execution is independent. M007 may close service hosting even if ordinary child execution is still separately tracked, provided the service can start and serve the node and the support matrix remains explicit.

## 12. Verification

At minimum:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
python3 -m unittest discover -s tests/release
git diff --check
```

Also record native Windows service qualification and `cargo tree` evidence showing `windows-service` is Windows-target-scoped.

The `windows-runner` CI job in `.github/workflows/ci.yml` now exists and is the hosted place to run Windows-target lint and `eggwork-runner` tests. It became available with Foundation M004's closure, so M007 gets hosted Windows evidence it did not previously have. Note its scope honestly: the job lints production code on the Windows target (`--lib --bins`) plus `eggwork-runner` test targets, because the server's and client's unit tests are Linux-shaped and do not compile for Windows. A service-host state-machine test placed in `eggwork-server` unit tests would therefore need either a Linux-compatible design or a new Windows-target test target before this job would run it — decide that when writing §10 tests, and record the choice.

No Eggwork source file may add an `unsafe` block.

## 13. Documentation

Update on implementation/closure:

- README Windows service support;
- `architecture/distribution.md`;
- `architecture/execution-ownership.md` if process-entry ownership text needs clarification;
- Operations roadmap;
- registry;
- operational-qualification workflow comments/stages;
- Operations M004 outstanding-evidence matrix.

Earlier releases remain immutable failure evidence.

## 14. Compatibility

Additive service capability only.

Do not change:

- protocol schemas;
- `eggworkd run` meaning;
- systemd or launchd product policy;
- Eggup service-manager ownership;
- installation/update transaction ownership;
- CodeGG integration.

An existing Windows installation that previously refused service mutation requires no repair; no service was installed by the fail-closed path.

## 15. Acceptance criteria

1. SCM-launched `eggworkd service-host` reaches `RUNNING` only after the node is ready.
2. The node serves the normal authenticated control plane while Running.
3. STOP and SHUTDOWN drain/converge and reach `STOPPED`.
4. Startup failure never claims Running.
5. INTERROGATE and advertised accepted controls are correct.
6. Service registration/manager operations still flow only through Eggup.
7. Foreground CLI/run behavior and Unix service qualification do not regress.
8. The fail-closed Windows refusal remains until hosted native evidence exists.
9. No Eggwork-owned unsafe Win32 FFI is introduced.
10. A release containing the host passes the native Windows service lifecycle/update qualification.

## 16. Stop conditions

Stop and re-plan if:

- service hosting requires duplicating Eggup's manager/registration logic;
- `windows-service` cannot support the required dispatcher/status/control path without Eggwork-owned unsafe code;
- the service callback cannot share the existing NodeServer lifecycle cleanly;
- the implementation requires a second daemon/server stack;
- RUNNING cannot be ordered after actual node readiness;
- stop cannot converge active executions without changing core execution ownership;
- Unix service behavior would require a semantic change rather than a mechanical process-entry refactor.

## 17. Closure evidence

Create:

- `plans/closure/operations-distribution/007-status.md`.

Record:

- dependency/version review;
- process-entry/state-machine design;
- implementation commit(s);
- state-machine tests;
- hosted Windows run id and receipts;
- install/start/serve/stop/restart/uninstall evidence;
- foreign-registration negative evidence;
- start-failure negative evidence;
- update/rollback evidence;
- unsafe/source ownership audit;
- unchanged Unix service evidence;
- Operations M004 item-2 disposition.

## 18. Handoff

M007 remains **ready** and independent: Foundation M004 closed conditionally without introducing any service-host interface, and M004's closure record says so explicitly. The one new fact is that hosted Windows evidence is now cheap to produce — the `windows-runner` CI job exists — where before M007 had no Windows-target CI surface at all.

The next immutable release candidate should be cut only after the product capabilities intended for that candidate are complete; do not publish `v0.1.5` as the Phase-6 closure release. As of the Foundation M004 closure, the release candidate is *blocked on that decision* rather than on missing implementation: M004's Windows process-tree backend is complete and hosted-qualified, and its only outstanding evidence is an installed-release receipt that requires a tag.
