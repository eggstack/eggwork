# Security M002 Closure — Trusted Landlock Sandbox Path

Source plan: `plans/implementation/security-isolation-resource/002-trusted-landlock-sandbox-path.md`  
Subsystem roadmap: `plans/subsystems/security-isolation-resource-roadmap.md`  
Threat model: `plans/security/threat-model.md`  
Reviewed implementation commit: `98d13f1`  
Planning/closure commit: `dfd9c70`

## Finding

M002 is closed for Linux hosts where the installed sibling helper passes the
ownership checks and the kernel fully enforces the required Landlock ruleset.
The runner launches the helper from a path derived from its own executable,
validates helper ownership/mode and each ancestor directory, and sends a
bounded JSON launch specification through a mode-0600 file in a mode-0700
temporary directory. A dedicated Unix socket returns the helper's enforcement
and target-start status. The target cannot forge this status through stdout or
stderr. Required setup never falls back to direct execution.

The `workspace_rw` profile grants full filesystem access only beneath the
canonical prepared workspace root. Runtime reads are limited to `/usr`,
`/etc/ld.so.cache`, and `/etc/ssl/certs`, with rights appropriate to whether a
path is a directory or file. Landlock must report full enforcement and
`no_new_privs` before the target command starts. The helper validates the
private spec independently and rejects unsupported profiles, oversized or
invalid argv/environment, loader-injection variables, and paths outside the
canonical workspace boundary. The target receives a fixed `/usr/bin:/bin`
PATH. Landlock filesystem rules do not provide network isolation.

The sandbox outcome is serialized in `ExecutionResult` as not requested, not
applied, applied, or failed. `BestEffort` continues only with an explicit
not-applied result when setup cannot be established. On non-Linux platforms,
the default runner does not claim Landlock support; required isolation remains
unavailable.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Helper comes from a trusted installation location | `TrustedLandlockSetup::discover_sibling` resolves beside the runner executable. `verify_trusted_helper` rejects symlink/non-file helpers, untrusted ownership, writable executable/parent/ancestor directories, and non-executable files. `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` exercises missing, wrong, and symlinked helpers. |
| Launch specification and enforcement status use bounded private channels | `SandboxChannel` applies directory mode 0700, spec mode 0600, a 1 MiB spec bound, and a dedicated Unix socket. The helper checks spec owner, mode, size, file identity, schema, profile, argv, environment, root, and cwd. `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` verifies malformed/tampered input and an unavailable status socket cannot start the target. |
| Helper status cannot be forged by target output | The runner accepts setup only from the private socket handshake before target startup; stdout/stderr are only output pipes. Required workspace execution verifies the successful handshake and typed `Applied` result. |
| Required enforcement fails closed | Missing/untrusted helper, unsupported profile, ruleset setup failure, partial enforcement, absent `no_new_privs`, and invalid pre-enforcement helper status prevent required target execution. Tests check required setup failure does not create a target marker. |
| Workspace and runtime access remain narrow | `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` verifies workspace read/write success, outside read/write denial, malicious request PATH filtering, and applied result. `workspace_symlink_cannot_read_outside_workspace` checks a symlink escape. `cwd_symlink_outside_workspace_is_rejected_before_launch` checks cwd escape rejection. |
| Best-effort state is explicit | `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` verifies a missing helper permits execution only under BestEffort and records `NotApplied`. Channel creation/status failures also map to explicit fallback state before any target may have started. |
| Cancellation, timeout, and process cleanup remain correct | `sandbox_timeout_and_cancellation_reap_the_helper_process_group` exercises both timeout and cancellation with the sandbox helper and child process. The helper and target share the process group managed by the canonical runner. |
| Machine-readable result records sandbox evidence | `ExecutionResult.sandbox` is serde-defaulted for stored-result compatibility. The runtime integration test checks both `RunnerResult` and serialized core result report `Applied`. Server failure/recovery constructors set explicit failed/not-applied/unknown values. |

## Verification actually run

Host: Linux x86_64, kernel `6.8.0-139-generic`; Rust `1.89.0`. A temporary
capability probe observed Landlock ABI 4 (`effective_abi: V4`) and full
enforcement for ABIs V1 through V4. The production helper requires the ABI V4
rights set and checks the actual restriction status at every launch.

- `cargo fmt --all` — passed.
- `cargo test --workspace --all-targets` — passed, 61 tests across 6 suites.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed with no warnings.
- `git diff --check` — passed.
- Dedicated Landlock integration suite — passed, 6 tests, including actual
  workspace allow/deny, path attacks, helper identity, spec/status failures,
  timeout, and cancellation.

Only the Linux x86_64 runtime was exercised. No macOS or Windows runtime or
cross-compilation claim is made. The Landlock integration requires an enabled
kernel with ABI V4 and is not evidence for older or differently configured
Linux hosts.

## Compatibility, security review, and residual risk

- `ExecutionResult.sandbox` is an optional serde-defaulted field, so older
  persisted results decode without it. New results expose sandbox status.
- Requests that previously named the placeholder `default` profile now map to
  the implemented `workspace_rw` profile. Unsupported profiles fail required
  requests or produce an explicit BestEffort outcome.
- The runtime allowlist intentionally permits reads beneath `/usr`, plus the
  loader cache and system certificate directory. This is broader than a
  per-executable runtime closure and should be revisited if deployments need a
  narrower image-specific policy.
- The owner-private channel protects against other local users, not a hostile
  process with the same effective UID. Installation directories and the
  service account remain part of the trusted computing base.
- The profile does not isolate networking, constrain CPU/memory/PIDs, create a
  separate user namespace, or claim host-wide containment. Those resource
  controls remain Security M003; broader hostile-workload qualification is
  Security M004.
- No high/medium issue in the M002 scope blocks closure. No independent
  penetration test or non-Linux runtime qualification was performed.

## Registry/roadmap disposition

Security M002 is closed. Security M003 is promoted to active because its only
plan dependency is satisfied. The current host exposes cgroups v2 controllers,
but the session cgroup is root-owned and not writable by the current user, so
M003 must probe delegation and report unsupported rather than imply cgroup
enforcement. Operations M001 and CodeGG M001 remain blocked on M003 closure;
Operations M002 also requires a stable Eggup consumer interface.
