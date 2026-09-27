# Operations M002 — Resumed Closure (Eggup Deployment and Service Integration)

Status: closed

Source implementation plan:
`plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`
(status now `closed`; see disposition below)

Roadmap: `plans/subsystems/operations-distribution-roadmap.md` (M002 now closed)

Historical record: `plans/closure/operations-distribution/002-status.md` is the
immutable blocker record for the pre-M007 Eggup interface. It is preserved
unchanged. This resumed record is the current success disposition for the
re-opened milestone.

Reviewed Eggwork head before implementation: `2ffe071`
Implementation commit: `d5722d9`
("feat(operations): integrate Eggup deployment and service lifecycle (M002)")

## Unblock verification

The historical blocker required Eggup to retain backups through post-start
health validation and expose rollback at that boundary. The implementation
qualifies against published `eggup-core 0.1.1` /
`eggup-service 0.1.1`, which provide:

- `ValidatedTransaction::commit_with_post_commit` with
  `PostCommitFailurePolicy::{KeepInstalled, RollBack}` — the callback runs
  while the mutation lock and backup set remain owned;
- Unix manager mechanics, launchd adapter, native Windows SCM adapter, and
  cron fallback through the neutral `ServiceManager` contract;
- `RegistrationSnapshot::ownership` distinguishing
  `Absent | Owned | Foreign | Unknown`, with destructive operations denied
  unless `Owned`.

No Eggwork-side file backup, restoration, or deletion was added. No platform
service manager is invoked from Eggwork code (static guard test).

## Exact dependency disposition

| Dependency | Version | Provenance | Checksum (Cargo.lock) |
|---|---|---|---|
| `eggup-core` | `0.1.1` | crates.io (published) | `c1b45b44ce742ff059ac4ff34e083168b8c37bd2c91968843b5761076c84c2d6` |
| `eggup-service` | `0.1.1` | crates.io (published) | `c631e33dbde6e435a990a4a6a3782f95205d7c28123ff46dbc1820a96f29f162` |

Publication/pinning disposition: both dependencies are published crates.io
releases pinned by exact version in `crates/eggwork-server/Cargo.toml` and
captured with checksums in `Cargo.lock`. No floating branch or Git revision
is used. The plan's Git-revision fallback was not needed. Release publication
of Eggwork itself remains a separate Operations M003/M004 concern.

## Requirement-to-evidence matrix

| Requirement (plan §) | Evidence | Result |
|---|---|---|
| §3 install unit; no independent helper/executable drift | `deployment::install_unit_matrix` declares `bin/eggworkd` (required) + `bin/eggwork-sandbox-helper` (required iff helper configured); `update_after_drain_replaces_coherent_daemon_helper_pair` commits both members in one Eggup multi-artifact transaction | Pass |
| §4 service spec from validated config; exact executable + critical argv/config; same-name/different-executable never owned | `deployment::service_spec_for_install` builds `eggwork-node` spec with `run --config <path>` argv plus config field; `service_spec_distinguishes_executable_and_config` proves a different executable classifies `Foreign` and relative paths are rejected | Pass |
| §5 manager adapters via Eggup; no direct manager invocation | `deployment::systemd_manager` constructs `SystemdManager` from caller-owned rendered unit bytes; `no_direct_service_manager_invocation_exists` scans `deployment.rs`, `operations.rs`, `eggworkd.rs` for manager command literals; ownership guard script passes | Pass |
| §6.1 candidate validation before stop | `CandidateSource::validate` runs before any service mutation in `orchestrate_update`; `candidate_validation_failure_happens_before_any_stop` proves a bad member leaves the running service untouched | Pass |
| §6.2 ownership inspection | `orchestrate_update` inspects first and proceeds only on `Owned`; `Absent/Foreign/Unknown` fail closed | Pass |
| §6.3 persistent drain before replacement | `UpdateRequest::drain_marker`; `orchestrate_update` sets the persistent marker via `operations::set_persistent_drain` before replacement; `update_sets_persistent_drain_and_preserves_recovery_state` asserts the marker exists post-update | Pass |
| §6.4 bounded drain wait or explicit force | `wait_for_quiescence`; `drain_with_active_execution_blocks_update_without_force` proves active executions block at 20 ms bound and `force` proceeds explicitly | Pass |
| §6.5 stop only owned service | `manager.stop` is reached only after the `Owned` check; `foreign_same_name_registration_fails_closed` proves stop/start/restart/uninstall/install all denied for `Foreign`; `unknown_malformed_registration_fails_closed` proves denial for `Unknown` | Pass |
| §6.6 Eggup transactional replacement | `build_install_plan` (SHA-256 per member) + `commit_with_health_check`; Eggup owns all filesystem mechanics | Pass |
| §6.7 start/restart owned service | `orchestrate_update` restores `was_running` via `manager.start`; `owned_install_start_stop_restart_are_idempotent` proves idempotent transitions | Pass |
| §6.8–6.9 bounded post-update health/version check through the post-commit boundary; `RollBack` default | `commit_with_post_commit(..., policy, health_check)`; `post_start_health_failure_rolls_back_while_backups_are_retained` proves `RollBack` restores the prior daemon with `rollback_performed` + `rollback_verified`, and `KeepInstalled` keeps the new set explicitly | Pass |
| §6.10 recovery state preserved | Install unit contains only `bin/` members; database/workspaces/blobs/artifacts are never staged; `update_sets_persistent_drain_and_preserves_recovery_state` proves adjacent recovery bytes are identical after update; `replacement_failure_rolls_back_to_prior_generation` proves a failed commit leaves the prior daemon bytes intact | Pass |
| §7 helper/version compatibility | `check_helper_compatibility`: `NotConfigured | Missing | Untrusted | VersionSkew | Compatible`; helper gained `--version` (`eggwork-sandbox-helper --version` prints `0.1.0`); `helper_version_skew_is_detected` covers missing/untrusted paths; version equality is enforced by comparing probe output to `CARGO_PKG_VERSION` | Pass |
| §8 operator commands, machine-readable | `eggworkd deployment status`, `service spec|status|install|uninstall|start|stop|restart` with JSON output; `--service-config` is required and never guessed; non-Linux lifecycle verbs fail with a structured diagnostic | Pass |
| §10 producer packaging boundary | No manifest/CI/installer/release-publishing code added; `DeploymentStatus.producer_packaging_external` is `true`; `install_unit_matrix` covers consumer binaries only | Pass |
| Acceptance 2: no direct manager invocation | Static guard + `check_execution_ownership.py` (2 + 1 approved spawn sites) | Pass |
| Acceptance 7: hosted manager claims have evidence | `platform_support` against live host (see matrix below); `linux_host_reports_manager_facts_without_mutation` | Pass |
| Acceptance 9: no unresolved high/medium finding | No findings; see residual section | Pass |

## Verification (actually executed)

- Host: Linux x86_64, kernel `6.8.0-142-generic`; rustc `1.89.0`; systemd `255.4`.
- `python3 scripts/check_execution_ownership.py` — passed (2 + 1 approved spawn sites).
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — no issues.
- `cargo test --workspace --all-targets` — 123 passed, 1 ignored (8 suites),
  including 15 new `deployment::tests` covering every required case class.
- `cargo check --workspace` — passed.
- `git diff --check` — passed.
- Live CLI: `eggworkd deployment status`, `service spec`, `service status`
  return machine-readable JSON against a valid operator config; missing
  `--service-config` fails closed with a structured error.
- `eggwork-sandbox-helper --version` prints `0.1.0`.

## Hosted manager matrix (actual)

| Platform/manager | Evidence | Claim |
|---|---|---|
| Linux systemd | `platform_support()` on this host: `systemd_available=true`, candidates `[Systemd(System), Cron]`; `SystemdManager` constructed only from explicit `--unit-path/--scope`; no unit installed or mutated by tests (TestDouble covers transitions) | systemd path implemented through Eggup adapter; lifecycle transitions contract-tested, not live-mutated on this host |
| Linux cron fallback | `crontab_available=true` reported; no cron content written by Eggwork | Detection only; cron lifecycle not claimed |
| macOS launchd | Not available on this host; `service` lifecycle verbs refuse non-Linux with a structured diagnostic | Not claimed; requires hosted macOS evidence (M004 scope) |
| Windows SCM | Not available on this host; same refusal | Not claimed; requires hosted Windows evidence (M004 scope) |

Cross-compilation is not claimed as qualification anywhere.

## Incidental bounded fix (not an architecture change)

`crates/eggwork-server/src/bin/eggworkd.rs` now installs the process-wide
rustls CryptoProvider (`ring::default_provider`) at startup. Without it, every
operator command that validates TLS config panics because both `ring` and
`aws-lc-rs` provider features are enabled transitively and rustls cannot
auto-select. This restores the already-designed M001 operator behavior
(`doctor`/`config validate`/`status`/`run` plus the new
`deployment`/`service` arms). No invariant or ownership boundary changed.

## Documentation disposition

- `crates/eggwork-server/src/deployment.rs` documents the Eggup/Eggwork
  ownership split at the module head.
- `eggworkd` usage string lists the new `deployment`/`service` verbs.
- Architecture/threat-model docs need no change: no new transport, identity,
  scheduler, or isolation authority was introduced.

## Compatibility review

- `OperatorConfig` schema unchanged; existing `drain`/`undrain` semantics
  unchanged (`orchestrate_update` reuses `set_persistent_drain`).
- Helper `--version` is additive; the `--spec/--status` protocol is untouched.
- No migration required.

## Residual findings and next-blocker state

- No unresolved high/medium/low finding in this milestone's scope.
- Operations M003 remains blocked on Eggpack build/qualification and
  release-orchestration interfaces (independent of this closure).
- Operations M004 remains blocked on M003 (M002 side now satisfied).
- Security M004 is unblocked by this closure: final helper/install ownership
  and update/restart behavior now exist for adversarial qualification.
- Live systemd unit install/start/stop was intentionally not mutated on this
  host; hosted lifecycle mutation evidence belongs to Operations M004.

## Registry and roadmap disposition

- Operations M002 is closed (resumed). The historical `002-status.md`
  blocker record is preserved immutably.
- `plans/subsystems/operations-distribution-roadmap.md` M002: ready → closed.
- `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`:
  ready for handoff → closed.
- Security M004: blocked on Operations M002 → ready (dependency satisfied by
  this closure).
