# Operations M007 — Windows service host

Status: **ready**

Class: corrective capability / platform lifecycle

Reviewed baseline: `d4db4e72` (2026-10-06)

Source references:

- `plans/subsystems/operations-distribution-roadmap.md`;
- `plans/closure/operations-distribution/004-status.md` outstanding Windows service-management evidence;
- `plans/closure/foundation-execution-core/004-status.md`;
- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`;
- `architecture/distribution.md`;
- `architecture/execution-ownership.md`;
- `architecture/ci-guardrails.md`.

Numbering: this is **Eggwork** Operations M007, unrelated to milestones in Eggup.

## 1. Objective

Make `eggworkd` an actual Windows SCM service host while preserving Eggup as the sole service-registration and service-manager owner.

When SCM launches the registered daemon, the process must join the Windows service dispatcher, report truthful lifecycle state, run the same Eggwork node implementation as foreground `eggworkd run`, respond to STOP/SHUTDOWN without blocking the control callback, converge active executions, and report `STOPPED`.

The milestone closes the remaining Windows **implementation** gap from Operations M004. It does not publish a release. Exact-release installed qualification remains owned by Operations M004.

## 2. Current baseline

The repository already has all adjacent pieces except the host:

- Eggup `eggup-service 0.1.1` owns the typed Windows SCM manager and registration policy.
- `deployment::windows_scm_manager` preserves the intended SCM policy but is intentionally unreachable from the operator mutation path.
- `product_service_manager` currently returns a fail-closed "Windows service management is unsupported" error before mutation.
- `service_spec_for_install` currently hard-codes `run --config <path>` for every platform.
- foreground `eggworkd run` calls `start_server`, waits for Ctrl-C, then calls `NodeServer::shutdown()`; it does **not** call `NodeServer::wait()`.
- `NodeServer::start` returns only after EggServe's handle reports ready.
- `NodeServer::shutdown` stops admission in memory, cancels active executions, and requests server shutdown.
- `NodeServer::wait(self)` waits for the listener plus active execution convergence.
- Foundation M004 is conditionally closed: Windows finite-process execution and Job Object tree ownership are implemented and hosted-native-qualified.
- `.github/workflows/ci.yml` now has a native `windows-runner` surface, but the general server/client unit-test graph remains Linux-shaped and must not be mislabeled as Windows coverage.

Operations M004's historical error 1053 is therefore still expected for every immutable candidate through `v0.1.5`: those binaries never call `StartServiceCtrlDispatcherW`.

## 3. Durable ownership boundaries

M007 must preserve these boundaries:

1. **Eggup owns SCM registration and manager operations.** Eggwork may host the service process; it must not implement install/delete/start/stop by calling SCM APIs directly.
2. **Eggwork-server owns node serving.** Service mode must call the existing `start_server`/NodeServer path, not a second server.
3. **eggwork-runner remains the only finite target-process owner.** Service hosting creates no alternate execution path.
4. **The service control callback is a signal edge, not an execution context.** It records STOP/SHUTDOWN intent and returns promptly.
5. **Required isolation/resource claims remain unchanged.** Windows execution works after Foundation M004, but required filesystem/resource controls still reject before spawn.
6. **No Eggwork-owned unsafe Win32 FFI.** The workspace's `unsafe_code = "deny"` remains unchanged.

An implementation that needs to violate one of these boundaries requires re-planning before code changes continue.

## 4. Reviewed dependency seam

Reviewed 2026-10-05 and re-used by this handoff:

- `windows-service 0.8.1`;
- MIT OR Apache-2.0;
- declared MSRV Rust 1.71, below Eggwork's 1.89 baseline;
- dispatcher, service-entry, control-handler, and service-status wrappers;
- transitive `windows-sys 0.61`.

Add it as a direct **Windows-target-only** dependency of `eggwork-server`:

```toml
[target.'cfg(windows)'.dependencies]
windows-service = "=0.8.1"
```

The exact pin makes the reviewed surface explicit. Re-check the crate API, license metadata, and Rust 1.89 build at implementation time.

Do not add direct Eggwork calls to `StartServiceCtrlDispatcherW`, `RegisterServiceCtrlHandlerExW`, `SetServiceStatus`, or other raw SCM FFI.

## 5. Process-entry architecture

### 5.1 Explicit service-host argv

Windows SCM registration must use an explicit host entry:

```text
eggworkd service-host --config <absolute-path>
```

Do not infer service mode from parent process, session id, environment, or dispatcher errors. Foreground `eggworkd run` must retain its existing meaning.

On non-Windows targets, `service-host` is unsupported.

### 5.2 Synchronous outer process entry

The current `#[tokio::main]` process entry is a poor fit for a dispatcher that owns the process thread until service exit.

Refactor the outer entry so it chooses the mode before constructing the asynchronous runtime:

- ordinary CLI commands construct/use the Tokio runtime and execute the existing async CLI path;
- Windows `service-host` calls the service dispatcher synchronously;
- the service-main callback constructs/enters the Tokio runtime required for the shared node lifecycle;
- the dispatcher thread is never parked inside an unrelated pre-existing async runtime.

A small immutable bootstrap handoff is acceptable if the service-entry ABI requires it. Do not store runtime handles, NodeServer instances, or mutable lifecycle state in global statics.

### 5.3 One shared server lifecycle

Factor foreground and service execution around one bounded async lifecycle equivalent to:

```text
start node -> report/observe readiness -> await stop signal
          -> stop admission/cancel active executions
          -> request server shutdown
          -> wait for listener + execution convergence
          -> return
```

The exact API name is implementation-owned, but both `run` and `service-host` must use it.

This corrects a current asymmetry: foreground `run` calls `shutdown()` but not `wait()`. M007 should make foreground Ctrl-C and SCM STOP share the same shutdown-and-wait convergence contract.

Do not duplicate NodeServer startup or cleanup logic in the binary.

## 6. Drain semantics

Normal service STOP/SHUTDOWN must **not create a new persistent drain marker**.

Reason: a routine `service stop` followed by `service start` should not silently resurrect the node permanently drained. The existing persistent drain file is an explicit operator/update safety mechanism.

Required behavior:

- during shutdown, admission stops immediately through NodeServer's in-memory drain/cancellation path;
- active executions converge before the service process exits;
- if a persistent drain marker already exists (for example because an update workflow set it), M007 must preserve it;
- service startup continues to honor an already-present persistent drain marker;
- service mode must never automatically `undrain`.

Operations M002a/M004 update policy remains unchanged: update orchestration sets persistent drain before service replacement when required and clears it only through explicit policy.

## 7. Windows service state machine

Use a small, testable state machine with truthful SCM reports:

```text
START_PENDING
      |
      | configuration valid + NodeServer::start returned ready
      v
RUNNING
      |
      | STOP or SHUTDOWN received
      v
STOP_PENDING
      |
      | shared shutdown + wait converged
      v
STOPPED
```

Requirements:

- report `START_PENDING` before potentially blocking startup work;
- use bounded wait hints/checkpoints where the crate API expects them;
- never report `RUNNING` before NodeServer is actually ready;
- startup/config/TLS/bind failure goes directly to `STOPPED` with a non-success service-specific exit disposition;
- accepted controls advertise only what is implemented;
- STOP and SHUTDOWN are idempotent;
- INTERROGATE returns/reports current state according to the wrapper's contract;
- a STOP received during startup is latched and honored as soon as startup can safely converge; it must not produce a transient false `RUNNING` claim;
- the control-handler closure performs no network, database, sleep, wait, runner cleanup, or service-manager operation.

A bounded synchronous channel, atomic flag, or Tokio watch/oneshot edge may carry control intent into the service lifecycle. The channel must have a closed/duplicate-control policy rather than unbounded accumulation.

## 8. ServiceSpec and ownership identity

`service_spec_for_install` currently always identifies the service as:

```text
<eggworkd> run --config <path>
```

M007 must make the product service identity truthful by platform:

- Linux: preserve `run --config <path>`;
- macOS: preserve `run --config <path>`;
- Windows: `service-host --config <path>`.

The exact executable and config path remain part of Eggup's ownership proof.

Do not change systemd unit or launchd plist rendering to `service-host`; those platforms do not use the Windows dispatcher.

Add a native Windows contract test proving the `ServiceSpec` argv and ownership comparison use `service-host`, while existing Unix tests pin `run`.

## 9. Re-enable the Eggup Windows manager only with the host

The current Windows branch in `product_service_manager` refuses before mutation. Replace that refusal with the existing `deployment::windows_scm_manager` only in the same implementation line that adds:

- the dispatcher/host entry;
- platform-correct ServiceSpec argv;
- state-machine tests;
- native hosted SCM qualification.

Do not merge an intermediate state where registration is re-enabled but the daemon is not proven to enter the dispatcher.

Parse `--windows-start-type` through the existing Eggup enum/policy. Do not add account, dependency, elevation, recovery-action, or delayed-auto-start features in this milestone unless Eggup's already-published API requires them for the existing product policy.

## 10. Test architecture

The repository-wide server/client unit graph is Linux-shaped. Do not broaden M007 into a cross-platform rewrite of unrelated tests.

Use three bounded layers.

### 10.1 Pure/bin lifecycle tests

Place the service-host lifecycle/state logic where `cargo test -p eggwork-server --bin eggworkd` can exercise it on Windows without compiling every Linux-shaped server unit test.

Test:

- startup success cannot report RUNNING before the ready edge;
- startup failure reports STOPPED without RUNNING;
- STOP during START_PENDING is latched;
- repeated STOP/SHUTDOWN is idempotent;
- callback signaling is non-blocking/bounded;
- shutdown reaches STOPPED only after the mocked/shared convergence edge;
- ordinary foreground command dispatch does not enter the SCM path.

Keep Win32 wrapper calls behind a narrow adapter so state transitions can be tested without actually installing a service.

### 10.2 Native Windows ServiceSpec contract test

Add a dedicated Windows-only integration target, for example:

```text
crates/eggwork-server/tests/windows_service_contract.rs
```

Run it explicitly on `windows-latest`, not through `--workspace --all-targets`.

It must prove:

- Windows ServiceSpec argv is `service-host --config ...`;
- executable/config identity is exact;
- different executable/config/critical argv is not considered owned;
- start-type parsing/policy is bounded;
- the Windows manager path is reachable only on Windows.

### 10.3 Hosted SCM lifecycle qualification from source

Add a dedicated Windows CI job (or a clearly separate stage in the existing Windows job) that actually mutates SCM on the hosted runner.

Use a source-built `eggworkd.exe`, bounded temporary config/TLS/state paths, and the **product's own service CLI/Eggup adapter**. Test orchestration may invoke the binary; it must not become a second production manager implementation.

Required lifecycle:

1. materialize a valid node configuration/TLS fixture;
2. `service install`;
3. `service start`;
4. wait boundedly until Eggup reports `Owned + Running`;
5. prove the authenticated node actually serves while SCM says Running;
6. `service stop` and prove Stopped;
7. `service start` again and prove Running;
8. `service restart` and prove Running;
9. `service uninstall` and prove registration absent/not owned.

The test must clean up in a finally/drop path so a failed assertion does not leave a service registration on the runner.

Also prove:

- startup with an invalid/unavailable node config never reports Running;
- STOP while a bounded execution is active converges the execution before process exit;
- a foreign same-name registration is not replaced/stopped/uninstalled by Eggwork.

Existing M002/M004 foreign-ownership tests may be reused where they already operate through Eggup; do not duplicate their manager implementation.

## 11. Authenticated service readiness

SCM state alone is insufficient. `RUNNING` means the service-hosted NodeServer is ready, not merely that the process entered its dispatcher.

Hosted qualification must perform at least one authenticated control-plane operation against the service-hosted node using the fixture client identity.

Prefer a bounded status/capabilities request. If an execution is used, it must request only Windows-supported controls and be fully cleaned before service stop.

This evidence is separate from Foundation M004's runner-focused tree suite.

## 12. Release qualification and closure ownership

M007 must not cut or publish a tag merely to prove source implementation.

Closure has two evidence levels:

### M007 implementation/native closure

M007 may become **conditionally closed** when:

- all implementation acceptance criteria are met;
- source-built hosted Windows SCM lifecycle is green;
- authenticated node readiness is proven under SCM;
- ordinary Linux/macOS CI remains green;
- no unsafe/direct-manager ownership regression exists.

### Operations M004 installed-release closure

The remaining condition is an immutable release containing both:

- Foundation M004 Windows execution;
- Operations M007 Windows service host.

Operations M004 owns:

- candidate creation/lineage;
- exact installed bytes;
- Windows installed execution receipt;
- native Windows service lifecycle/update receipt;
- same-tag/release integrity checks;
- publication boundary and anonymous bootstrap evidence.

After M007 lands, update `.github/workflows/operational-qualification.yml` so Windows no longer merely records an expected service refusal. It must execute the real service lifecycle for the next candidate.

Do not publish `v0.1.5`; it is immutable pre-M004/M007 evidence.

## 13. Failure and restart semantics

- STOP/SHUTDOWN: refuse new admission in memory, cancel/converge active executions, shutdown listener, wait, then STOPPED.
- process crash: SCM observes failure according to existing Eggup policy; M007 does not invent restart/recovery policy.
- node startup failure: STOPPED/non-success; never RUNNING.
- duplicate controls: idempotent.
- service restart: new process starts through dispatcher and honors any pre-existing persistent drain marker.
- update rollback: remains Eggup-owned; `RecoveryRequired` behavior from M002a is unchanged.
- retained execution records remain durable; service hosting must not fabricate completion.

## 14. Security and maintenance review

Verify:

- no `unsafe` blocks under Eggwork source;
- no `sc.exe`, PowerShell manager, registry mutation, or raw SCM FFI in production;
- `windows-service` is direct only under `cfg(windows)`;
- Eggup remains the only service-manager dependency;
- private key/config/TLS errors remain bounded/redacted;
- control-handler diagnostics do not emit secret paths/content beyond existing operator policy;
- service-host does not change mTLS/authz capability semantics;
- direct process-spawn ownership remains unchanged.

## 15. Expected production files

Expected touch surface:

- `crates/eggwork-server/Cargo.toml` — target-scoped direct `windows-service`;
- `Cargo.lock`;
- `crates/eggwork-server/src/bin/eggworkd.rs` and/or a narrowly scoped sibling Windows service-host module;
- `crates/eggwork-server/src/deployment.rs` — platform-correct ServiceSpec and Windows manager enablement;
- `crates/eggwork-server/tests/windows_service_contract.rs` or equivalent dedicated target;
- `.github/workflows/ci.yml` — explicit Windows service-host tests/native SCM job;
- `.github/workflows/operational-qualification.yml` — next-release Windows service claim instead of refusal;
- focused documentation/planning files required at closure.

Do not move the service-host implementation into `eggwork-runner`.

## 16. Required verification

Common:

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

Windows-target/source evidence:

```text
cargo clippy --locked --workspace --lib --bins --all-features -- -D warnings
cargo test --locked -p eggwork-server --bin eggworkd
cargo test --locked -p eggwork-server --test windows_service_contract
cargo test --locked -p eggwork-runner --all-targets
```

Plus the hosted SCM lifecycle qualification from §10.3.

If test target names differ, closure must record the exact commands actually executed.

## 17. Documentation reconciliation

At implementation/closure update:

- README Windows service support;
- `docs/deployment.md` / service operator guidance;
- `architecture/distribution.md`;
- `architecture/execution-ownership.md` if process-entry ownership needs clarification;
- `architecture/ci-guardrails.md` for the dedicated Windows service test surface;
- Operations roadmap;
- `plans/registry.md`;
- Operations M004 outstanding evidence;
- operational-qualification workflow comments.

Historical release evidence must remain intact. Correct present-tense claims rather than rewriting old candidate observations.

## 18. Acceptance criteria

1. SCM-launched `eggworkd service-host` enters the dispatcher and reaches RUNNING only after NodeServer is ready.
2. ServiceSpec uses `service-host --config` on Windows while Linux/macOS retain `run --config`.
3. STOP/SHUTDOWN signal asynchronously and reach STOPPED only after listener and active execution convergence.
4. Normal STOP does not create a new persistent drain marker; an existing marker is preserved and honored on restart.
5. Foreground Ctrl-C now uses the same shutdown-and-wait convergence path without changing its CLI contract.
6. Startup failure and STOP-during-start never make a false RUNNING claim.
7. Eggup remains the only registration/service-manager owner.
8. Install/start/status/stop/start/restart/uninstall pass on hosted Windows from a source build.
9. The service-hosted node passes an authenticated readiness request.
10. Foreign same-name ownership still fails closed.
11. No Eggwork-owned unsafe/raw SCM FFI or alternate process owner is introduced.
12. Linux/macOS service behavior and existing update/rollback semantics do not regress.
13. The operational-qualification workflow is ready to qualify the next immutable candidate as a real Windows service, not an expected refusal.

## 19. Stop conditions

Stop and re-plan if:

- the wrapper cannot implement the dispatcher/status/control path without Eggwork-owned unsafe code;
- Windows service hosting requires duplicating Eggup manager/registration behavior;
- ServiceSpec cannot represent the service-host argv while preserving ownership identity;
- the only workable design requires a second NodeServer or runner implementation;
- STOP cannot converge active executions through the existing NodeServer ownership path;
- a routine service stop would require changing persistent drain/update semantics;
- a source-built native SCM test cannot be made deterministic on the existing hosted Windows environment that previously reproduced error 1053;
- the implementation requires a durable public protocol/schema change.

## 20. Closure evidence

Create:

- `plans/closure/operations-distribution/007-status.md`.

Record:

- reviewed baseline and implementation commits;
- exact `windows-service` dependency/version;
- process-entry and lifecycle design;
- ServiceSpec platform matrix;
- state-machine/unit test matrix;
- hosted source-built Windows SCM run id;
- authenticated service readiness evidence;
- install/start/stop/restart/uninstall receipts;
- STOP-during-start and startup-failure negatives;
- active-execution stop convergence evidence;
- foreign-registration refusal;
- persistent-drain preservation/non-creation evidence;
- Unix regression evidence;
- unsafe/source ownership audit;
- unresolved findings by severity;
- Operations M004 item-2 disposition.

If every implementation/native criterion passes but only immutable-release evidence remains, close M007 **conditionally** and name that exact downstream M004 condition rather than cutting a release inside the implementation handoff.

## 21. Handoff

M007 is independently ready. Foundation M004 has already supplied the Windows process-tree backend and native Windows CI surface; M004a is closed.

No additional prerequisite implementation plan is required.

The intended sequence is:

```text
M007 implementation + source-built hosted SCM proof
        |
        v
M007 conditional/full implementation closure
        |
        v
next immutable candidate containing Foundation M004 + M007
        |
        v
Operations M004 exact-release qualification
        |
        v
maintainer-controlled publication + anonymous bootstrap proof
```

Keep M005 reverse-connect and M006 PTY deferred; neither belongs in this line.
