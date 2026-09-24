# Security Remote-Admission Corrective C001 Closure — Remote Enforcement Admission and Capability Truthfulness

Status: closed

Implementation plan: `plans/implementation/security-isolation-resource-remote-admission-corrective/001-remote-enforcement-admission-and-capability-truthfulness.md`

Corrective authority: `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md`

Reviewed implementation commit: `a060482` (baseline `d192a7d0dfa456693b278028d1f9288f7bea7a04`).

Downstream evidence that exposed the gap: CodeGG corrective closure `d3d390d56620fb5c6f755a5dfb0987e0a1c01651`.

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| One runtime capability snapshot owned by runner/setup layer | `ExecutionSetup::execution_capabilities()` replaces `resource_capabilities()` in `crates/eggwork-runner/src/lib.rs`; `TrustedLandlockSetup` contributes `isolation.landlock.workspace-rw.v1` plus cgroups memory/cpu/pids facts; `operations::doctor` and sandbox-helper test use the new boundary | Pass |
| Landlock capability proves more than helper-path existence | `probe_landlock_ruleset()` builds an in-process Landlock ABI V4 ruleset with the `workspace_rw` handled rights over `/usr`, `/etc/ld.so.cache`, `/etc/ssl/certs`, `/dev/null`; never calls `restrict_self`; gated on `verify_trusted_helper` path/ownership/mode checks; returns `false` on any failure | Pass |
| Static + dynamic features advertised as one contract | `static_capability_features()` (exec, events, auth, blob, workspace, artifact, `network.unrestricted.v1`) unioned with runtime dynamic features by `server_capability_features()`; `NodeState.capabilities_features` cached at startup | Pass |
| `/v1/capabilities` and `NodeStatus.capabilities` agree | Both go through `node_capabilities_for()`; regression `capabilities_and_status_features_agree_with_a_unified_snapshot` asserts equality plus dynamic-feature presence and `NodeCapabilities::validate` bounds | Pass |
| `None` accepted unchanged | `check_isolation_supported` accepts `None`; mapped through `RunnerRequest::from_spec` as before | Pass |
| `BestEffort` accepted even when Landlock absent; truthful applied/not-applied | Missing/untrusted-helper fixture executes and returns `NotApplied`; server never upgrades best-effort or claims applied pre-runner | Pass |
| `Required` Landlock preflights before spawn; rejects `409 capability_mismatch` when absent | `check_isolation_supported` requires the Landlock feature; rejection increments `rejected_capability`; runner fail-closed setup check preserved | Pass |
| Required resource dimensions preflight truthfully | `check_resources_supported` maps required memory/cpu/pids to `resources.cgroups-v2.*`; missing dimension -> `409 capability_mismatch`; best-effort always reaches runner | Pass |
| `network.unrestricted.v1` explicit; Disabled/AllowListed fail closed | `check_network_supported` accepts only `Unrestricted`; regressions prove disabled and allow-listed never reach `RunnerRequest`/target spawn | Pass |
| Real server qualification: required isolation executes, escape denied, evidence `Applied` | Authenticated `NodeServer` + production client fixture: workspace read succeeds, outside-workspace read/write denied, terminal sandbox evidence `Applied`, no unsandboxed fallback | Pass |
| Controller-style compatibility fixture | `capabilities()` gate on `exec.argv.v1` + Landlock feature, `status()` equality, workspace materialize, required-isolation argv, fenced renew/cancel/observe with no protocol regression | Pass |
| Static guard against blanket-gate reintroduction | Unit tests pin the single canonical static list, forbid unsupported network modes in static features, and forbid dynamic/static overlap duplication | Pass |
| No second scheduler/process/setup authority | Ownership guard passes: approved owners remain `eggwork-runner` (2 spawn sites), `eggwork-sandbox-helper` (1 spawn site) | Pass |

## Capability feature contract

Static (protocol/workspace/artifact/auth + explicit unrestricted network):

```text
exec.argv.v1
events.live.v1
auth.mtls.v1
blob.sha256.v1
blob.stream.v1
workspace.manifest.v1
workspace.materialize.v1
artifact.declared.v1
artifact.stream.v1
artifact.retention.v1
network.unrestricted.v1
```

Dynamic (runtime-probed, advertised only when qualified):

```text
isolation.landlock.workspace-rw.v1
resources.cgroups-v2.memory
resources.cgroups-v2.cpu
resources.cgroups-v2.pids
```

`Disabled` and `AllowListed` network modes are never advertised and always rejected with `409 capability_mismatch`.

## Side-effect ordering

Capability checks run at the head of `execute`, in isolation → resources → network order, incrementing `rejected_capability` on mismatch. All three precede `RunnerRequest::from_spec` translation (line ~1474), semaphore permit acquisition (~1545), workspace `mark_active` (~1577), execution `reserve` (~1598), and target spawn. Workspace authorization/resolve may precede the checks where unavoidable, but a rejected capability request leaves no accepted execution record and no child process. Rejection tests assert zero target side effect.

## Verification

- Host: Linux x86_64, kernel `6.8.0-142-generic`; rustc `1.89.0`; systemd `255.4`; cgroup-v2 controllers `cpuset cpu io memory hugetlb pids rdma misc`.
- Landlock: in-process ABI V4 ruleset probe succeeds on this host; `isolation.landlock.workspace-rw.v1` advertised and exercised through required-isolation execution with escape denial.
- Resources: systemd user-manager transient scopes probed per M003 path; memory/cpu/pids features advertised on this host.
- `python3 scripts/check_execution_ownership.py` — passed (2 + 1 approved spawn sites); `--prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — no issues.
- `cargo test --workspace --all-targets` — 98 passed, 1 ignored (8 suites), including 56 `eggwork-server` tests covering capabilities equality, required/best-effort isolation, resource admission, network negatives, and the controller compatibility fixture.
- `cargo check --workspace` — passed.
- `git diff --check` — passed.

## Platform and capability matrix

| Platform/backend | Qualification |
|---|---|
| Linux x86_64, kernel 6.8, Landlock ABI V4, trusted helper installed | Required Landlock remotely reachable; escape denied; evidence `Applied`. |
| Linux without trusted helper or Landlock ABI/rights | Landlock feature not advertised; required isolation rejected `409` pre-spawn; best-effort runs with `NotApplied`. |
| Linux systemd user manager, cgroups v2 | Memory/CPU/PID features advertised and enforced per M003 path; required dimensions preflight truthfully. |
| Windows Job Objects / macOS | No backend; no capability advertised (unchanged). |
| Network Disabled / AllowListed | Intentionally unsupported; `409 capability_mismatch`; no iptables/nftables/namespace logic added. |

## Documentation disposition

- `plans/subsystems/security-isolation-resource-roadmap.md` — C001 marked closed; M004 now waits on Operations M002 only.
- Implementation plan status set to closed with this closure link.
- `architecture/` and `plans/security/threat-model.md` reviewed: no stale claim implies remote Phase 5 enforcement is unreachable (threat model already states an authorized controller may request arbitrary commands within available host isolation), so no edits required.

## Compatibility, security review, and residual findings

Authn/authz/idempotency/lease/workspace/artifact suites remain green. Capability advertisement is a preflight optimization only; the runner's trusted-helper verification and setup probes remain the enforcement authority and fail closed on races. No unresolved high/medium finding remains in this corrective's scope. Residual: deployment/update-surface evidence still belongs to Operations M002 before Security M004 final closure.

## Registry and roadmap disposition

Security remote-admission C001 is closed. Security M004 remains blocked on Operations M002. CodeGG M002a (restricted-spec live requalification) is unblocked on the Eggwork side: it requires CodeGG M002 closure plus this C001 closure, the latter now satisfied.
