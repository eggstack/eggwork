# Foundation M002 Closure — Canonical Local Runner

Source plan: `plans/implementation/foundation-execution-core/002-canonical-local-runner.md`  
Subsystem roadmap: `plans/subsystems/foundation-execution-core-roadmap.md`  
Reviewed baseline: `7d263c7fa030aca3f33f62117983368836f64942`  
Implementation commits: `5dd4274eca71b952311c95a431851ddae96ee8f3`, `6dba4c6b895c9893dc206ec621245797725713b2`

## Finding

M002 is closed for Linux-hosted execution. `eggwork-runner::LocalProcessRunner` is the only production process-spawn path in the workspace. It accepts argv, validates a caller-authorized root and relative cwd, rebuilds a conservative environment, writes bounded stdin, drains both output streams concurrently, retains bounded head/tail output, and exposes bounded best-effort streamed chunks. Cancellation, timeout, output overflow, and leader exit with lingering descendants clean the Unix process group before return. Required sandbox/resource setup fails closed when no backend is configured; best-effort absence is reported as not applied.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Canonical async runner and argv-only invocation | `LocalProcessRunner::run`; source inspection finds `Command::new` and `.spawn()` only in `crates/eggwork-runner/src/lib.rs`. No shell fallback exists. Shell use in fixtures is an explicit argv executable. |
| Core request translation | `RunnerRequest::from_spec` validates `ExecutionSpec` and translates command, cwd, env, stdin, timeout, output and isolation fields. |
| Environment sanitization/noninteractive defaults | Child environment is cleared, host PATH and fixed locale/noninteractive values are rebuilt, then caller values are filtered against loader, shell, executable search, GIT and credential-related names. Tests prove `LD_PRELOAD` cannot be reintroduced. |
| Bounded capture and streaming | Concurrent reader tasks maintain per-stream head/tail capture and total/omitted counts; bounded `try_send` drops event chunks under backpressure while continuing pipe drain. Tests emit 200KB through a one-slot channel and verify bounded capture/drop accounting. |
| Timeout, cancellation, overflow and process descendants | Linux tests cover timeout, cancellation, terminate-on-overflow, descendants ignoring SIGTERM, and a naturally exiting leader leaving a background process. Group SIGKILL follows a grace period. |
| Typed terminal and cleanup evidence | `TerminationReason`, `RunnerError`, `CleanupDiagnostics`, `RunnerResult`; a unit test proves cleanup warning does not change a known nonzero exit. |
| Sandbox/resource hooks | Typed request/outcome and `ExecutionSetup`; `NoExecutionSetup` reports best-effort as not applied and rejects required setup before spawn. No enforcement is claimed. |
| Ownership/scope boundaries | Runner depends on core only. Client/server do not spawn processes. No HTTP/TLS, admission, queueing, workspace transfer, PTY, or scheduler API was added. |

## Verification actually run

Pinned toolchain: `rustc 1.89.0 (29483883e 2025-08-04)`.

- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (13 tests; RTK reported 4 suites).
- `cargo check --workspace` — passed.

Focused runner fixtures cover success/stdout/stderr, nonzero exit, stdin bytes and null stdin, environment deny, cwd, output overflow, slow consumer, cancellation before spawn and while running, timeout, spawn failure, cleanup outcome preservation, and Linux descendant cleanup.

## Compatibility, security, and platform review

- Reuse review: inspected current clean CodeGG head `28b4695661d463dd1675d045ac6299c5fbc9ea31` and `src/managed_process.rs`. Lifecycle ideas were generalized; no CodeGG source or domain type was imported or copied.
- Dependency review: Tokio, tokio-util, async-trait, thiserror, and Unix-only nix added. No HTTP/TLS stack or sandbox enforcement library added here.
- Linux qualification: process-group behavior and descendant cleanup exercised on this Linux host.
- Other Unix platforms: implementation uses Unix process groups but was not run there; not qualified by this closure.
- Windows: explicitly returns `UnsupportedPlatform` before spawn. No Job Object support or Windows descendant-cleanup claim is made.
- Security review: environment is reconstructed and key families are denied; cwd canonicalization confines resolved paths beneath root. The root must already be authorized by its caller. Host PATH remains inherited by the node process and is never caller-overridden.
- Residual findings: no unresolved high or medium findings. Cross-platform process-tree support is deferred and explicit.
- Disposition: **closed for the qualified Linux host**.

## Registry/roadmap and next-plan disposition

The foundation roadmap records M001 and M002 closed; the registry promotes Control Plane M001 to ready.

Before handoff, the clean current EggStack checkouts were re-inspected:

- EggServe / `eggserve-core` / `eggnet-tls`: `7e52ecec0b336ef1755b3e1654ddb1438b38bcdb`. Required client-auth TLS configuration is supported by Eggnet TLS; verified `TlsInfo` is populated from the rustls peer chain and reaches `Request::context().connection().tls`. The canonical request lifecycle and streaming body APIs are available.
- Eggfetch: `8959ca890ee34f4cf456aed648315322f1e83ef7`. `TlsConfigBuilder::client_cert_path`, caller-provided CA roots, `Client::send_detailed`, request body streaming, and response streaming are available.
- Eggress: `93fafff8ec8f601b509bb347bdfa136d51998c7d`. Listener-free `OutboundConnector::connect_tcp` exists. Eggfetch raw-stream dial composition is explicitly outside Control Plane M001 and remains a later compatibility question.

No upstream interface blocker prevents authenticated direct fixed-target execution. Control Plane M001 is unblocked. The mTLS integration still needs local certificate-matrix execution as its acceptance evidence.
