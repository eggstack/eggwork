# Foundation M003 Closure — Execution Ownership Guards and Runner API Hardening

Status: closed

Source plan: `plans/implementation/foundation-execution-core/003-execution-ownership-guards-and-runner-api-hardening.md`

Roadmap: `plans/subsystems/foundation-execution-core-roadmap.md`

## Reviewed baseline and implementation

- Reviewed head before implementation: `7fe07c0b75c2ab022f2a17b5999ee589e3d94e62` (Control Plane M003 closure).
- Implementation commit: `34ef0811ef80acb9fc5584c5a6db643e106f426c`.
- Process ownership exceptions are limited to `eggwork-runner` for finite-process lifecycle/resource wrappers and `eggwork-sandbox-helper` for target launch under helper-enforced isolation/resource setup.
- `eggwork-server`, `eggwork-client`, and `eggwork-core` are scanned as non-owners. Tokio's `process` feature is declared by the runner rather than enabled in the workspace-wide Tokio feature list.

## Requirement-to-evidence matrix

| Requirement | Implementation/evidence |
|---|---|
| Production spawn guard | `scripts/check_execution_ownership.py` scans Rust production source for process `Command` imports/construction outside the explicit owner allowlist. `spawn_blocking` remains allowed for database/filesystem work. |
| Deterministic negative self-test | The guard self-test detects a synthetic forbidden `std::process::Command::new(...).spawn()` and rejects spawn text hidden only in comments/strings. The self-test runs on every guard invocation. |
| Dependency-direction guard | The same script runs `cargo metadata --no-deps`; core/client cannot depend on runner/server, server must depend on runner, server cannot enable Tokio process or add a known process-owner crate. |
| Runner public surface | `RunnerRequest` fields are private and can be created through `from_spec` or `new`, then adjusted through named setters; run-time validation remains before setup/spawn. Server now reads setup requests through accessors. `LocalProcessRunner::{default,new,run}`, `ExecutionSetup`, setup/result/output types, and custom `StdinPolicy` remain public embedding surfaces. `NoExecutionSetup` remains public because the operator surface uses it. Unused `ProcessTreePolicy` was removed. |
| Scheduler policy exclusion | No scheduling priority, placement, fairness, or CodeGG provenance was added to runner types. |
| Output/drain characterization | An ignored/manual test covers 4 KiB and 8 MiB output, reports duration/throughput, and asserts retention remains no greater than the 64 KiB capture limit. No timing threshold or benchmark dependency was added. |
| Documentation | Updated execution ownership, architecture overview, runner crate docs, and contributor guard command. |

## Verification performed

- `python3 scripts/check_execution_ownership.py` — passed; positive scan found two runner-owned spawn sites and one helper-owned site; synthetic negative test and crate dependency check passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed: 5 client, 12 core, 11 runner, 10 sandbox-helper integration, and 44 server tests; one manual characterization test is intentionally ignored by the normal suite.
- `cargo check --workspace` — passed.
- `cargo test -p eggwork-runner characterize_output_drain_overhead -- --ignored --nocapture` — passed. Observed 4 KiB in 1 ms (2.97 MiB/s reported) and 8 MiB in 168 ms (47.59 MiB/s reported); retained capture was 4 KiB and 64 KiB respectively. These are characterization observations, not performance guarantees.
- Linux runtime only. No cross-platform runtime claim is made.

## Residual findings and disposition

- No unresolved high/medium findings were identified in the scoped review.
- A full-workspace test attempt during host contention had several pre-existing systemd resource probes exceed their one-second test deadline; the focused sandbox-helper integration suite and subsequent full-workspace run both passed. This does not qualify non-Linux platforms.
- Foundation M003 is closed. Security M004 remains blocked until Operations M002 closes; Control Plane M003 and Foundation M003 now satisfy two of its three dependencies. Operations M003 remains blocked on Eggpack's producer contracts. No other directly dependent plan became ready from this closure alone.
