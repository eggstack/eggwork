# Security M003 Closure — Enforced Resource Controls

Status: closed

Reviewed implementation commit: `26f42bb84533fd9d77812377e4603d591810a12c`

Implementation plan: `plans/implementation/security-isolation-resource/003-enforced-resource-controls.md`

Dependencies: Security M002 closure `plans/closure/security-isolation-resource/002-status.md` and corrective closure `plans/closure/security-isolation-resource/002a-status.md`.

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| Required controls fail before target start when unavailable | Runner test for `NoExecutionSetup` required resources; trusted setup probes the requested systemd properties before launching the helper | Pass |
| Best-effort unsupported controls are explicit | `ResourceResult` serializes `NotApplied` with a reason; runner unit coverage exercises unsupported setup | Pass |
| Enforce memory, process count, and CPU quota with truthful capability reporting | Linux backend probes transient systemd scopes per controller. Helper checks `memory.max`, `memory.swap.max`, `cpu.max`, and `pids.max` in its actual cgroup before target start. Integration tests prove memory and PID limit events and verify CPU quota in the target cgroup | Pass on the host below |
| Distinguish limit exceedance from normal exit | Memory and PID integration fixtures return typed `ExecutionFailure::ResourceLimit` and per-dimension `LimitExceeded` results | Pass |
| Avoid cross-execution limit contamination | Concurrent executions with separate PID limits verify the low-limit scope exceeds while the higher-limit scope succeeds | Pass |
| Clean up descendants and resource scope | Cancellation/timeout fixture runs under enforced CPU quota and verifies descendant reaping; runner stops the transient systemd scope after collecting final evidence | Pass |
| Capability report reflects runtime availability | Local runner probes memory, CPU, and PID systemd properties separately and only adds successful dimensions to node feature capabilities; integration fixture asserts all three on this host | Pass on this host |

## Verification

- Host: Linux x86_64, kernel `6.8.0-139-generic`; Cargo `1.89.0`.
- Runtime: systemd user manager successfully established transient cgroup-v2 scopes for memory, CPU, and PID properties. Direct writes to this login session's cgroup were unavailable; implementation uses manager-created scopes and verifies actual cgroup files before target start.
- `rtk cargo test -p eggwork-sandbox-helper --test landlock_runner` — 10 passed, including required memory exceedance, required PID exceedance, CPU quota verification, concurrent PID scopes, and cancellation/timeout cleanup.
- `rtk cargo test --workspace --all-targets` — 67 passed.
- `rtk cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `rtk git diff --check` — passed.

## Platform and capability matrix

| Platform/backend | Qualification |
|---|---|
| Linux x86_64, systemd user manager, cgroups v2 | Memory, CPU quota, and PID controls were runtime-probed and exercised on the host above. Actual availability is re-probed at runner startup and per requested dimension. |
| Linux without usable systemd manager/controller/helper | Not qualified as enforcing these dimensions. Required requests fail before target start; best-effort results report `NotApplied`. |
| Windows Job Objects | Not implemented or tested in this milestone; no capability is advertised. |
| macOS | No hard resource backend implemented or tested; no capability is advertised. |

## Compatibility, security review, and residual findings

Legacy optional-number resource JSON remains accepted and round-trips with its existing shape; required values use the explicit `required` representation. Resource ranges are validated before setup. The backend resolves `systemd-run` from trusted system locations, checks ownership and write permissions, avoids caller PATH lookup, uses unique transient unit names, verifies the resulting cgroup settings inside the trusted helper, and scrubs target environment through the helper spec. Cleanup diagnostics retain errors if the system manager cannot stop the scope.

The Linux result qualifies the systemd user-manager path only. It does not qualify system-manager/service deployment, other Linux distributions, Windows, or macOS. CPU quota was verified as installed in the target cgroup; this run did not claim a measured workload throughput ratio. No unresolved findings remain that prevent closure of the implemented Linux-first milestone; unimplemented platform backends remain explicitly unsupported.

## Registry and roadmap disposition

Security M003 is closed for the Linux systemd user-manager path exercised above. The corrective `/dev/null` closure is recorded separately; historical Security M002 evidence remains unchanged. Operations M001 and CodeGG M001 directly depend on Security M003 and are unblocked. Operations M002 remains blocked on Operations M001 and a stable Eggup consumer interface.
