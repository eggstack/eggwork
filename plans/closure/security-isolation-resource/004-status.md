# Security M004 — Adversarial and Cross-Platform Closure Qualification

Status: closed

Source implementation plan:
`plans/implementation/security-isolation-resource/004-security-closure-and-cross-platform-adversarial-qualification.md`
(status now `closed`; see disposition below)

Source roadmap: `plans/subsystems/security-isolation-resource-roadmap.md`
(M004 now closed)

Hard dependency satisfied: Operations M002 Eggup deployment/service
integration, closed in
`plans/closure/operations-distribution/002-resumed-status.md`
(implementation `d5722d9`, published `eggup-core`/`eggup-service` 0.1.1).
Final helper/install ownership and update/restart behavior were adversarially
qualified as part of this milestone.

Reviewed Eggwork head before implementation: `870f2fe`
Implementation commit: `283f3ea`
("feat(security): adversarial closure qualification and null-device write fix (M004)")

Exact Eggup revisions used: unchanged from M002 — `eggup-core 0.1.1`
(`c1b45b44ce742ff059ac4ff34e083168b8c37bd2c91968843b5761076c84c2d6`),
`eggup-service 0.1.1`
(`c631e33dbde6e435a990a4a6a3782f95205d7c28123ff46dbc1820a96f29f162`).

## 1. Bounded hardening fix discovered by the fixtures

**Finding (M004-F01, severity high before fix):** the trusted `workspace_rw`
Landlock profile granted `/dev/null` read-only access. Under that rule,
sandboxed targets cannot open `/dev/null` for writing, so ordinary
`>/dev/null` redirection fails — and, worse, every escape-trap fixture that
redirects through `/dev/null` fails at redirection setup *before* the escape
attempt runs, falling through to success. The historical outside-read/write
escape evidence (M002, M002a, remote-admission C001, controller fixture) was
therefore vacuous: it passed with or without confinement. Live proof: a
required-isolation trap failed with
`/bin/sh: 1: cannot create /dev/null: Permission denied` before reaching any
escape probe.

**Fix (bounded, no new sandbox):** grant `/dev/null` `ReadFile | WriteFile`
in `crates/eggwork-sandbox-helper/src/main.rs` `restrict()`, and mirror the
exact rule in the runner capability probe
(`probe_landlock_ruleset`) so advertisement stays truthful. The null device
discards writes and yields EOF on reads, so the grant is
information-neutral; no other `/dev` path is allowed. Historical closure
records are preserved immutably; this record documents the re-proof.

**Re-proof:** after the fix, all Landlock escape fixtures genuinely execute
their probes (see §3–§4). Non-vacuity controls: the same trap script run
without the sandbox exits `71` (write arm), and isolated arm runs exit `72`
(child read) and `73` (grandchild read, non-empty marker); under the fixed
sandbox all arms are denied and the execution exits `0` with `Applied`
evidence.

## 2. Adversarial requirement matrix

### Identity and authorization

| Requirement | Evidence | Result |
|---|---|---|
| Payload principal forgery rejected | Pre-existing `request_payload_cannot_supply_authoritative_principal` | Pass (re-run) |
| Wrong mTLS principal cannot observe another principal's execution (no existence oracle) | New `cross_principal_execution_fencing_without_lease_oracle`: stranger observe → `404`, identical to unknown-id observe → `404` | Pass |
| Wrong principal cannot cancel/renew/read events of another principal; no lease-validity leak | Same test: stranger cancel/renew → `403 forbidden`, events → `403`; principal is checked before lease hash in `control`/`events_route` | Pass |
| Resource-aware authorization on execution/workspace/blob/artifact ops | Pre-existing `resource_aware_authorizer_can_scope_blob_capability`, `mtls_authorization_and_fixed_target_execution` (deny-before-spawn, deny-before-mutation) | Pass (re-run) |
| Certificate-chain/pathological-input bounds; anonymous requests have zero side effect | Pre-existing `mtls_execution_streams_output_and_missing_certificate_is_rejected` | Pass (re-run) |
| Secrets not in ordinary diagnostics/errors/debug | New `secret_sentinels_stay_out_of_error_and_debug_surfaces` (409 evidence echoes no sentinel; `OutputChunk`/`EnvironmentEntry` Debug redacted) + new deployment sentinel tests; pre-existing TLS redaction tests | Pass |

### Lease/generation/idempotency

| Requirement | Evidence | Result |
|---|---|---|
| Wrong lease token → `invalid_lease` | New fencing test: owner cancel with random lease → `403 invalid_lease` | Pass |
| Wrong principal → `forbidden` without leaking lease validity | Same test (`forbidden`, distinct from `invalid_lease`, principal checked first) | Pass |
| Stale generation cannot control newer generation | Pre-existing generation-mismatch handling + `restart_recovers_uncertain_execution_as_interrupted_without_replay` | Pass (re-run) |
| Idempotency/conflict, reconnect creates no second child | Pre-existing control-plane suites (M002 closure) | Pass (re-run) |
| Lease expiry terminates process tree with truthful terminal evidence | Pre-existing `expired_lease_terminates_process_with_a_typed_terminal_result`, `lease_expiry_takes_precedence_over_cancel_before_spawn` | Pass (re-run) |

### Hostile workspace/input

| Requirement | Evidence | Result |
|---|---|---|
| Traversal, absolute, backslash, dot-segment, empty, NUL, oversized paths rejected | New `hostile_workspace_manifests_are_rejected_without_side_effect`: raw wire JSON with 10 hostile paths → `400` each; valid create afterwards succeeds (no half-created state) | Pass |
| Symlink/special-file, missing parents, corrupt blob, digest mismatch, quota | Pre-existing `missing_blobs_and_invalid_symlinks_never_create_ready_workspaces`, `workspace_symlink_cannot_read_outside_workspace`, `cwd_symlink_outside_workspace_is_rejected_before_launch`, blob invalid-upload/quota tests | Pass (re-run) |
| Derived-manifest patch attacks | Workspace M004 is closed; pre-existing `derived_workspace_denied_and_oversized_bodies_are_typed`, concurrent derived idempotency/conflict fencing | Pass (re-run) |

### Executed-process sandbox attacks (qualified Linux)

| Requirement | Evidence | Result |
|---|---|---|
| Read outside workspace denied | Pre-existing required-Landlock remote test (now genuinely probative after §1 fix) | Pass (re-run) |
| Write outside workspace denied | New `required_landlock_denies_outside_writes_and_confines_descendants` (exit-71 arm) | Pass |
| Descendants inherit confinement (child + delayed background grandchild) | Same test (exit-72/73 arms, deterministic via `wait` + non-empty marker check) | Pass |
| Required helper/setup failure produces no unsandboxed target | Pre-existing `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers`, `required_landlock_rejects_with_capability_mismatch_when_helper_is_missing` | Pass (re-run) |
| Helper trust failure fail-closed for required isolation | New `only_compatible_helper_admits_required_isolation` (all non-`Compatible` outcomes deny, never downgrade) | Pass |

### Resource attacks (qualified Linux)

| Requirement | Evidence | Result |
|---|---|---|
| Memory/PID pressure enforced and classified; CPU quota verified | Pre-existing `required_memory_limit_is_enforced_and_classified`, `required_pid_limit_is_enforced_and_classified`, `required_cpu_quota_is_verified_before_target_start`, `concurrent_resource_scopes_keep_pid_limits_isolated` | Pass (re-run on this host) |
| Required unavailable controller rejects before spawn; best-effort reports not-applied | Pre-existing `unsupported_resource_limits_are_reported_or_rejected_before_spawn`, resource admission tests | Pass (re-run) |
| Limit-exceeded distinguishable from exit/timeout/cancel | `ExecutionFailure::ResourceLimit` assertions in the above | Pass (re-run) |

### Network policy

| Requirement | Evidence | Result |
|---|---|---|
| `Unrestricted` explicit; `Disabled`/`AllowListed` → `capability_mismatch`; Landlock never implied as network sandbox | Pre-existing `disabled_or_allowlisted_network_requests_are_rejected_with_capability_mismatch`, static network-feature tests | Pass (re-run) |

## 3. Deployment/update adversarial qualification (post-M002 installed unit)

| Requirement (plan §5) | Evidence | Result |
|---|---|---|
| Same-name foreign service registration cannot be stopped/replaced/uninstalled | M002 `foreign_same_name_registration_fails_closed` + `unknown_malformed_registration_fails_closed` | Pass (re-run) |
| Helper path/version mismatch blocks required isolation | New trust-gate test (§2) wired to `check_helper_compatibility` version coherence (`--version` vs daemon release) | Pass |
| Update enters persistent drain before replacement; admission stops while draining | M002 `update_sets_persistent_drain_and_preserves_recovery_state` + new live `drain_rejects_new_admission_while_terminal_records_survive` (`503 draining`) | Pass |
| Retained terminal records survive update/restart truthfully | M002 preservation test + live drain test (pre-drain terminal observable, exit 0 intact) + pre-existing restart-recovery test | Pass |
| Post-start health failure rolls back through Eggup with backups retained | M002 `post_start_health_failure_rolls_back_while_backups_are_retained` (`rollback_performed` + `rollback_verified`) | Pass (re-run) |
| Rollback restores a coherent daemon/helper pair | New `rollback_restores_coherent_daemon_helper_pair` (both members verified, no split generation) | Pass |
| Successful update reconciles drain per documented policy | Drain marker remains set after update (fail-closed; explicit `undrain` required); documented in README operator guidance | Pass |
| Failed update cannot fabricate execution completion | Install unit contains only `bin/` members; M002 `replacement_failure_rolls_back_to_prior_generation`; execution records live outside the transaction | Pass (re-run) |

## 4. Race and TOCTOU review

| Race | Disposition |
|---|---|
| Workspace/materialization vs GC | Deterministic fixtures pre-existing (`concurrent_materialization_is_idempotent_and_recovery_cleans_unready_state`, `terminal_retention_gc_preserves_live_workspace_then_reclaims_inputs`, manifest reaping, crash-reopen reconciliation) — re-run green |
| Terminalization vs artifact/blob GC | Pre-existing bounded GC suites (artifact retention/GC, blob quota/corruption/cleanup) — re-run green |
| Cancellation vs spawn/setup handshake | Pre-existing `cancellation_before_spawn_and_spawn_failure_are_typed`, `timeout_and_cancellation_kill_process_group` — re-run green |
| Lease expiry vs explicit cancel | Pre-existing `lease_expiry_takes_precedence_over_cancel_before_spawn` — re-run green |
| Drain vs execute admission | New `concurrent_drain_admission_outcomes_are_closed`: 8 concurrent admissions across a drain transition; every outcome is admitted-with-truthful-terminal-evidence or typed `503`; `admitted + rejected == 8` (no lost/fabricated admission). Exact split is timing-dependent by design; the outcome set is closed |
| Helper replacement/trust around launch | Trust is verified in `prepare()` immediately before spawn within one setup call; helper ancestry/directory must be root/euid-owned and group/other-unwritable, so replacement requires privilege. Not claimed as exhaustive proof; documented invariant, no finding |
| Update/restart vs retained recovery | M002 preservation + live drain survival evidence above |

## 5. Static ownership and dependency audit

- `python3 scripts/check_execution_ownership.py` — passed (approved
  process owners: `eggwork-runner` 2 spawn sites, `eggwork-sandbox-helper`
  1 spawn site; crate dependency direction clean).
- `--prove-negative-exit` — passed (synthetic forbidden spawn detected).
- `unsafe_code` is `forbid`-denied in every crate (`core`, `runner`,
  `client`, `server`, `sandbox-helper`); grep confirms zero `unsafe`
  blocks — only the `forbid` attributes themselves.
- Service-manager ownership: new `no_direct_service_manager_invocation_exists`
  scans `deployment.rs`/`operations.rs`/`eggworkd.rs` for manager command
  literals; all lifecycle flows through `eggup-service` adapters.
- Secret-bearing Debug/Serialize: `CommandSpec` (argv redacted),
  `ExecutionSpec` (metadata count only), `EnvironmentEntry` (value
  redacted), `OutputChunk`/`BoundedCapture` (content redacted),
  `ExecutionResult` carries byte counts only; operator `redacted_json`
  hides key paths and fingerprints. New sentinel tests pin the wire and
  deployment surfaces.
- One bounded exception is documented in
  `architecture/execution-ownership.md`: the helper `--version` coherence
  probe runs through Eggup's bounded candidate primitive (cleared env,
  null stdin, bounded output/timeout, kill/reap); it performs no admission
  and owns no lifecycle.

## 6. Verification (actually executed)

- Host: Linux x86_64, kernel `6.8.0-142-generic`; rustc `1.89.0`;
  systemd `255.4`; cgroup-v2 controllers available; Landlock ABI V4
  probed live (`isolation.landlock.workspace-rw.v1` advertised and
  exercised, not skipped).
- `python3 scripts/check_execution_ownership.py` — passed.
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — no issues.
- `cargo test --workspace --all-targets` — **132 passed, 1 ignored
  (8 suites)**, including 6 new live adversarial server tests, 4 new
  deployment adversarial tests, and all re-run M001–M003/C001 suites.
- `cargo check --workspace` — passed.
- `git diff --check` — passed.
- Non-vacuity controls for the confinement traps executed manually:
  unsandboxed trap exits `71`/`72`/`73` per arm; sandboxed exits `0`.

## 7. Hosted platform matrix (actual)

| Platform/backend | Evidence | Claim |
|---|---|---|
| Linux x86_64, Landlock ABI V4, trusted helper, systemd/cgroup-v2 | Full suite green on the host above; outside read/write/grandchild denial physically demonstrated; memory/PID/CPU enforcement demonstrated; systemd manager facts probed (`systemd_available=true`) | Required Landlock + resource controls qualified on this host class |
| macOS | No hosted runner in this environment. Code inspection: `verify_trusted_helper` → `Err`, Landlock probe → `false`, cgroup probe → `Err`, service lifecycle verbs refuse non-Linux with a structured diagnostic, capability advertisement excludes enforcement features; required requests fail closed with `capability_mismatch` | Fail-closed behavior by construction; launchd path NOT production-qualified |
| Windows | Same as macOS (SCM adapter exists in Eggup but is unexercised here) | Fail-closed behavior by construction; SCM path NOT production-qualified |
| Network Disabled/AllowListed | Rejected `409 capability_mismatch` on Linux run | Intentionally unsupported, no backend implied |

Only Linux toolchain installed here, so cross-compilation was not
attempted. Compilation alone is not claimed as qualification anywhere.

## 8. Documentation reconciliation

- `plans/security/threat-model.md`: status/scope updated to M004;
  executed-process row updated (closed controls, remaining network-explicit
  boundary); new rows for foreign service registration, helper
  trust/skew, and failed-update behavior; verification section rewritten
  to the M004 evidence set.
- `README.md`: operator section lists `deployment status` and the
  `service` verbs with required flags; Eggup ownership/update/drain
  policy and the Eggpack boundary documented.
- `architecture/execution-ownership.md`: bounded `--version` probe
  exception documented with rationale; direct manager invocation
  restated as forbidden.
- Capability/platform matrix: §7 above; static capability lists pinned
  by existing tests; runtime features probed live.
- Roadmap/registry: updated on closure (see §10).

## 9. Acceptance criteria disposition

1. Every remotely reachable operation has adversarial authn/authz
   evidence — Pass (§2 identity matrix + re-run suites).
2. Lease/generation/idempotency fencing survives negative and restart
   cases — Pass.
3. Workspace/blob/artifact confinement survives hostile inputs and GC
   races — Pass.
4. Required Linux Landlock confinement physically demonstrated against
   outside reads/writes and descendants — Pass (after §1 fix; traps
   proven non-vacuous).
5. Required qualified resource controls demonstrated; unsupported
   platforms fail closed — Pass.
6. Network restriction explicitly unsupported — Pass.
7. Installed helper/service/update ownership adversarially qualified
   after Operations M002 — Pass (§3).
8. Post-update health failure rolls back through Eggup without
   Eggwork-owned filesystem transaction logic — Pass (re-run).
9. Secret sentinels do not leak through ordinary diagnostics/errors/
   debug — Pass.
10. Hosted platform claims match the actual evidence matrix — Pass (§7).
11. No unresolved critical/high finding remains — Pass (see §10;
    F01 found and fixed in-milestone).
12. No medium finding relabeled as polish — Pass (no mediums; minors
    in §10).

## 10. Unresolved findings by severity and workstream disposition

- Critical: none. High: none open (F01 fixed and re-proven in-milestone).
- Medium: none. Low/notes: (a) helper trust TOCTOU between check and
  launch is bounded by installation-ownership preconditions, not proven
  exhaustive; (b) macOS/Windows lifecycle paths await hosted evidence;
  (c) live systemd unit install/start/stop was intentionally not mutated
  on shared hosts — belongs to Operations M004.
- Security workstream: M001–M003, remote-admission C001, and M004 are
  now closed. The Security workstream is **closed**; remaining platform
  expansion (macOS/Windows hosted qualification) is future work under
  Operations M004 scope, not a Security defect.
- Downstream: no new CodeGG dependency created by this milestone.

## 11. Registry and roadmap disposition

- Security M004 is closed by this record.
- `plans/subsystems/security-isolation-resource-roadmap.md` M004:
  ready → closed.
- `plans/implementation/security-isolation-resource/004-security-closure-and-cross-platform-adversarial-qualification.md`:
  ready → closed.
- `plans/registry.md`: Security workstream marked closed; M004 moved to
  the closed-plans table; remaining interface gates unchanged
  (Operations M003 still blocked on Eggpack; M004 operational
  qualification still blocked on M003).
