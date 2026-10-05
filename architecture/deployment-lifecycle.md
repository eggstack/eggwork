# deployment lifecycle

`crates/eggwork-server/src/deployment.rs` (1,627 lines) is the product's consumer-side
deployment and service-lifecycle layer: it decides *what* a coherent Eggwork node
installation is, *when* a running node may be replaced, *whether* an installed
sandbox helper is compatible with the daemon that will use it, and *what counts as
success* after a replacement. Every filesystem mutation, rollback, receipt, and
platform service-manager interaction is delegated to the pinned Eggup crates
(`eggup-core` / `eggup-service` 0.1.1). This deep dive covers only the policy this
module owns; the producer side of packaging lives in
[distribution.md](distribution.md), the operator surface in
[operations-cli.md](operations-cli.md), the helper binary in
[sandbox-helper.md](sandbox-helper.md), and hosted evidence in
[release-qualification.md](release-qualification.md).

## Ownership boundary

The module doc states the split, and the code follows it without exception. The
boundary is enforced structurally: a unit test greps the module for direct
service-manager invocation and fails if it appears.

| Concern | Owner | Where |
| --- | --- | --- |
| Filesystem transaction, staging, backup, restore, rollback mechanics | Eggup | `eggup_core::InstallPlan` → `prepare` → `verify_integrity` → `validate` → `commit*` |
| Install receipts / transaction evidence | Eggup | `eggup_core::TransactionReceipt`, `eggup_service::LifecycleUpdateReceipt` |
| Platform service-manager adapters (systemd, launchd, SCM) | Eggup | `eggup_service::{SystemdManager, LaunchdManager, WindowsScmManager}` |
| Host fact inspection (what managers exist) | Eggup | `eggup_service::inspect_host` via `platform_support` |
| Which artifacts form one coherent node installation | **Eggwork** | `install_unit_matrix` |
| Remote execution must drain before replacement | **Eggwork** | `UpdateRequest.drain_marker`, `wait_for_quiescence` |
| Helper compatibility / version-coherence policy | **Eggwork** | `HelperCompatibility`, `check_helper_compatibility*` |
| Bounded post-update health validation | **Eggwork** | the `health_check` closure supplied to Eggup |
| Which service is ours | **Eggwork** (identity) / Eggup (comparison) | `service_spec_for_install` supplies the identity; `Eggup` compares registrations against it |
| Platform support claims | **Eggwork** | the CLI refuses unsupported backends; see [Windows](#windows-unsupported-and-fail-closed) |

Two things Eggwork does **not** do, by explicit design and explicit test:

- **Eggwork never implements file backup or restoration itself.** It never copies,
  moves aside, renames, or restores a member. It hands Eggup a validated plan and
  a post-commit policy; Eggup decides what to retain and when.
- **Eggwork never invokes a platform service manager directly.** There is no
  `systemctl`, `launchctl`, `sc.exe`/SCM, or `crontab` call in this module. All
  manager traffic goes through the `eggup_service::ServiceManager` trait. The unit
  test `no_direct_service_manager_invocation_exists` (`deployment.rs:1510`) is the
  enforcement site.

`ServiceManagerAdapter` (`deployment.rs:687`) is a pass-through newtype: it holds
`&mut dyn ServiceManager` and forwards all six verbs. It exists to give Eggup a
mutable manager handle while the plan borrows other state, and it adds no policy.

## The install unit

`install_unit_matrix(helper_required: bool) -> Vec<InstallMember>` is the whole
answer to "what is an Eggwork node installation?" An `InstallMember` is a
`{ member, destination, required }` triple.

| Member identity | Destination (relative to installation root) | Required |
| --- | --- | --- |
| `eggworkd` (`MEMBER_DAEMON`) | `bin/eggworkd` (`DEST_DAEMON`) | always |
| `eggwork-sandbox-helper` (`MEMBER_HELPER`) | `bin/eggwork-sandbox-helper` (`DEST_HELPER`) | `helper_required` |

On Linux, `helper_required` is true, and this is the important part: **the daemon
and the Landlock sandbox helper are one coherent installation, moved by one
Eggup transaction.** They are not two independent updates. The doc comment on
`install_unit_matrix` states the reason directly — the members must move together
"so helper/executable versions cannot drift independently." On macOS and Windows
the helper is not required and not shipped, because the sandbox boundary is
Linux-only (`LANDLOCK_WORKSPACE_RW_CAPABILITY` is advertised only for the closed
Landlock `workspace_rw` profile).

The destinations keep the member identity as their file name, and
`release_contract.rs` asserts that invariant directly:

```
DEST_DAEMON.ends_with(MEMBER_DAEMON) && DEST_HELPER.ends_with(MEMBER_HELPER)
```

Identity constants that must never drift from the install unit:

- `PRODUCT_ID = "eggwork"` — the Eggup `ProductId` in every `InstallPlan`.
- `SERVICE_ID = "eggwork-node"` — the Eggup `ServiceId` in every `ServiceSpec`,
  the launchd `Label`, and the Windows SCM service name.
- `EGGUP_CORE_VERSION` / `EGGUP_SERVICE_VERSION = "0.1.1"` — the pinned
  dependencies this milestone qualified. They are surfaced in
  `DeploymentStatus::collect` as `eggup_core_version` / `eggup_service_version`.

### Keeping the matrix and `release/eggpack/` in agreement

The producer contract lives in `release/eggpack/` and is a *separate* milestone
(see [distribution.md](distribution.md)). The two are held in agreement by
static-parity tests, not by a runtime dependency — in fact the opposite: there is
a test that asserts the *absence* of that dependency.

`release/eggpack/distribution.toml` declares release asset names, install
identities, and checksum sidecars, and it explicitly documents that it mirrors
`eggwork_server::deployment::install_unit_matrix`. The Linux x86_64 and aarch64
targets declare `kind = "bundle"` with exactly two entries — `install = "eggworkd"`
and `install = "eggwork-sandbox-helper"` — because a two-executable bundle means
the daemon and its helper "can never be selected from different releases."
macOS and Windows declare a single `eggworkd` (`eggworkd.exe`).
`install-policy.toml` marks both bundle members `= "executable"` and describes
**first install only**: it "never selects a release, never" registers a service,
and never overwrites. `build-bindings.toml` and `qualification-bindings.toml` bind
bundle entry index 0 to the daemon and index 1 to the helper, which is why entry
order is itself a contract.

The `release/eggpack/` file set: `build-bindings.toml`,
`consumer-validators.json`, `distribution.toml`, `github-policy.json`,
`github-template.json`, `install-policy.toml`, `pack.toml`,
`qualification-bindings.toml`, `workflow-shape.json`.

Parity tests in `crates/eggwork-server/tests/release_contract.rs`:

- `install_identities_match_the_deployment_install_unit` — pins the exact
  `(member, destination, required)` triples, `SERVICE_ID == "eggwork-node"`, the
  `ends_with` file-name rule, and the literal member strings.
- `the_helper_ships_exactly_on_linux_release_targets` — helper presence/absence
  per target triple; `the_linux_helper_and_daemon_share_one_release_identity` —
  one version for the pair.
- `the_linux_helper_validator_agrees_with_deployment_version_policy` — the
  consumer validator's version policy equals the deployment policy.
- `install_policy_marks_every_bundle_member_executable_and_nothing_else` — no
  data, config, database, workspace, blob, or artifact member is bundled.
- `consumer_validation_is_gating_and_selects_the_linux_helper`.
- `the_deployment_module_never_parses_eggpack_configuration` — the mirror-image
  guarantee: `src/deployment.rs`, `src/operations.rs`, and `src/bin/eggworkd.rs`
  must not contain `release/eggpack`, `ReleaseManifest`, `release-manifest`,
  `DistributionContract`, or `eggpack::`.

## Candidate acquisition policy

`CandidateSource { member, source, destination }` is "one candidate artifact
staged outside the installation root." Its `validate()` enforces, in order:

1. `member` is exactly `eggworkd` or `eggwork-sandbox-helper` (kind `candidate`).
2. `source` is an absolute path.
3. `destination` is non-empty, **not** absolute, and contains no `..`.
4. `source` resolves via `symlink_metadata` to a regular file that is not a
   symlink.
5. `0 < len <= MAX_CANDIDATE_BYTES` (256 MiB = `256 * 1024 * 1024`).

`orchestrate_update_with_lifecycle_budgeted` re-validates every source *before*
building the plan and before any service mutation, so a bad candidate can never
reach the stop/replace path. The unit test `candidate_validation_failure_happens_before_any_stop`
(`deployment.rs:1096`) asserts exactly that ordering.

**`deployment apply` accepts already-local candidates only.** The operator hands it
absolute local files via `--daemon` (always) and `--helper` (Linux only). The
command does not fetch a URL, does not resolve or select a release, does not pick
"latest", and does not publish. `--release` is a release *identity label* supplied
by the operator and validated as an Eggup `ReleaseId`; it is not a lookup key.
Release acquisition and selection belong to Eggpack as a separate
runtime-consumer interoperability milestone, which is exactly why
`the_deployment_module_never_parses_eggpack_configuration` exists and why
`DeploymentStatus::collect` reports `producer_packaging_external: true`. The
README's rule is the same: `install.sh`/`install.ps1` are exact-release
**first-install** scripts that "never overwrite, never register a service, never
elevate privileges, and never select a release."

On Linux the CLI *requires* `--helper` (`"Linux release apply requires --helper
<absolute-candidate>"`) and rejects it everywhere else (`"--helper is only valid
for Linux release bundles"`), mirroring the install unit exactly.

## Install plan and transaction

`build_install_plan(installation_root, release, sources) -> eggup_core::InstallPlan`:

- Empty source set → `candidate` error. No empty-plan commit exists.
- `ProductId::new(PRODUCT_ID)`, `ReleaseId::new(release)`.
- Per source: `validate()`, then `eggup_core::hash_file(&source.source)` and
  `.with_integrity(IntegrityRequirement::Sha256(digest))`. **Digests are computed
  from the staged bytes**, so the later `verify_integrity` gate proves the exact
  bytes committed — not the bytes someone intended to commit.
- All members assemble into one `ArtifactSet`, hence one `InstallPlan`.

The commit path is a fixed four-stage pipeline:

```
plan.prepare()?
     .verify_integrity()?
     .validate(&eggup_core::AllValidators::new())?
     .commit_with_post_commit(CommitOwnership::new(verifier, absent), policy, health_check)
```

`commit_with_health_check` and `commit_without_health_check` differ only in the
final call. `commit_without_health_check` is documented as "first install / tests
only" — it is the plain `validated.commit(...)` with no post-commit gate.

`orchestrate_update_with_lifecycle_budgeted` is the real path and its ordering is
the contract:

1. Validate every candidate.
2. Build the plan; `prepare` → `verify_integrity` → `validate(AllValidators)`.
3. If `drain_marker` is `Some`, set the persistent drain marker via
   `crate::operations::set_persistent_drain(marker, true)`; failure →
   `DeploymentError { kind: "draining", .. }`. **Drain is set before replacement**,
   so new execution admission stops while the old generation finishes.
4. `wait_for_quiescence(active_executions, policy)` — see
   [Drain](#drain-before-replacement).
5. `eggup_service::commit_with_lifecycle(...)` with
   `RestoreIntent::Preserve`, `AbsentPolicy::AllowCreate`,
   `post_commit_failure: request.policy.post_commit`.

**One transaction, both binaries.** Because `bin/eggworkd` and
`bin/eggwork-sandbox-helper` are members of a single `ArtifactSet`, there is no
intermediate state in which the daemon is new and the helper is old. Tests
`update_after_drain_replaces_coherent_daemon_helper_pair` (`deployment.rs:1187`)
and `rollback_restores_coherent_daemon_helper_pair` (`deployment.rs:1567`) assert
the pair property on both the commit and the rollback side. The corresponding
exclusion is explicit: the transaction replaces only `bin/` members, so "execution
recovery state (database, workspaces, blobs, artifacts) is never part of the
install unit, so retained terminal records survive update/restart truthfully and a
failed update cannot fabricate execution completion."

### `RollBack` versus `RecoveryRequired`

`UpdatePolicy::post_commit` is an `eggup_core::PostCommitFailurePolicy`. The
default, and the product default, is `PostCommitFailurePolicy::RollBack`: if the
post-commit health check fails, Eggup restores and verifies the previous
generation while backups are still retained. `KeepInstalled` is documented as
"explicit operator policy only." The test
`post_start_health_failure_rolls_back_while_backups_are_retained`
(`deployment.rs:1307`) pins the retained-backup window.

`RecoveryRequired` is a different thing entirely: it is Eggup's
recovery-required disposition, meaning Eggup preserved exact transaction evidence
and **suppressed automatic restoration of the old service generation** — because
restoring would itself be unsafe or unverifiable in that state. The README
("`RecoveryRequired` preserves Eggup's exact transaction evidence and suppresses
automatic restoration of the old service generation") and
[operations-cli.md](operations-cli.md) both treat it as an outcome the JSON
record must name. The key operational consequence: a `RecoveryRequired`
disposition is **not** a successful update, and the CLI must never print it as
one. It is surfaced through the JSON receipt with the recovery named, not as a
clean exit.

### Receipts

`orchestrate_update_with_lifecycle` returns
`eggup_service::LifecycleUpdateReceipt`, which carries the nested
`TransactionReceipt` (returned as `Ok(receipt.transaction)` by the plain
`orchestrate_update`) plus `final_snapshot: Option<LifecycleSnapshot>`. Receipts
are Eggup-owned structures — Eggwork does not synthesize them, which is why
transaction evidence cannot be forged by this layer. The CLI JSON record reports
the release id, the Core artifact disposition, whether manual artifact recovery is
required, the lifecycle restoration status, the final observed ownership/state when
the adapter can prove it, and the bounded failure detail (`phase`, `category`,
`detail`) explaining a rolled-back or retained generation.

## Replacement digest proof

Eggup will replace an *existing* file only when the operator proves what is
currently installed. The proof is an exact SHA-256 digest, supplied per member on
the command line:

- `--previous-daemon-sha256` — required for the daemon member.
- `--previous-helper-sha256` — additionally required on Linux, where the helper
  is part of the install unit.

The CLI enforces an **all-or-nothing pairing rule** before any work:

```
if (previous_daemon.is_some() || previous_helper.is_some())
   && (previous_daemon.is_none()
       || (os == "linux" && previous_helper.is_none()))
  => "replacement requires previous SHA-256 values for every release member"
```

The digests become `eggup_core::MemberId`/`parse_sha256` pairs passed to
`eggup_core::ExactDigestVerifier`, which is then the `OwnershipVerifier` for the
commit. For a file that exists but is not named in the proof, Eggup has no exact
digest to match and **fails closed**.

Why this is the anti-tamper property, stated precisely: the plan's per-member
`IntegrityRequirement::Sha256` proves *what will be written*. The
`ExactDigestVerifier` proves *what is being overwritten*. Without the second proof
an attacker who has replaced the on-disk daemon could have a legitimate release
overwrite a trojanized binary and the transaction would complete cleanly,
laundering the tampering into a "successful update." Exact-digest proof makes
the operator assert the bytes they believe are installed; if the bytes differ, the
update stops and the operator investigates. It also makes an accidental
third-party reinstall visible rather than silently absorbed.

What a legitimate operator must do: record the SHA-256 of the currently installed
`bin/eggworkd` (and `bin/eggwork-sandbox-helper` on Linux) *before* running
`deployment apply`, and pass those values. If a member is absent, the flag for it
is simply omitted — proof is required only for files that exist, and the pairing
rule requires consistency across the install unit so a proof cannot be partially
withheld.

## Drain-before-replacement

Eggwork owns the decision that remote execution must drain before replacement.
The pieces:

`UpdatePolicy` (all fields public, `Copy`):

| Field | Default | Meaning |
| --- | --- | --- |
| `drain_timeout` | `DEFAULT_DRAIN_TIMEOUT` (30s) | how long to wait for active executions before failing closed |
| `health_timeout` | `DEFAULT_HEALTH_TIMEOUT` (30s) | post-update health/version probe budget; the doc comment notes it is "informational" because Eggup's post-commit callback is itself bounded |
| `force` | `false` | proceed even with active executions |
| `post_commit` | `PostCommitFailurePolicy::RollBack` | what Eggup does when the post-commit health check fails |

`wait_for_quiescence(active: impl Fn() -> usize, policy) -> Result<(), DeploymentError>`
polls `active()` every 5 ms until it returns `0`, or fails closed with
`DeploymentError { kind: "draining", detail: "active executions remain after bounded drain wait" }`
once `start.elapsed() >= policy.drain_timeout`. It is fail-closed by default: a
stuck execution cannot be waited out indefinitely, and the update does not proceed
on its own.

The update sequence is therefore: **persistent drain → wait for quiescence →
stop owned service → transactional replace → post-commit health validation →
restore running state.** Two ordering properties matter. Drain is set *before*
replacement, so new executions cannot arrive during the wait. And only an
`Owned` service is stopped or replaced — `Foreign`/`Unknown` registrations are
never touched (tests `foreign_same_name_registration_fails_closed`,
`unknown_malformed_registration_fails_closed`).

### Force

`force: true` makes `wait_for_quiescence` return `Ok(())` immediately. It is not
a speed knob. The doc comment: "proceed even with active executions (explicit
operator choice; active work is still never fabricated as complete)." Work that
was running is cancelled through the normal shutdown path; its records stay
non-terminal and terminal, truthful records are untouched. Force must be explicit
because the alternative — auto-replacing under active execution — is a
silent data-integrity hazard with no way for the operator to have noticed.

The CLI surfaces it as the bare `--force` flag
(`force: args.iter().any(|arg| arg == "--force")`) and exposes it on
`deployment apply` only. The "explicit force policy through the orchestration API"
phrasing in the README refers to the programmatic route: `UpdatePolicy` is a
caller-constructed value, so any embedding application chooses force itself rather
than inheriting it from a flag.

### Drain stays set

`deployment apply` always passes `drain_marker: Some(&drain_marker)`, and nothing
in the orchestrator clears it. The marker is set before replacement and remains
set after, on **every** outcome including success. This is deliberate and is
normative product behavior (README: "The command leaves persistent drain set
after every outcome, including success, for an explicit later operator
`undrain`"). The reasoning: an update is precisely the moment an operator may want
to hold admission closed while they inspect the result. Clearing drain implicitly
as a side effect of a successful update would make it impossible to update and
then examine. Clearing it is an explicit, separate, attributable act —
`eggworkd undrain`. See [operations-cli.md](operations-cli.md) and
[server-node.md](server-node.md) for how the running node observes the marker.

## Post-install health validation

The health check is a caller-supplied `FnOnce() -> Result<(), String>` (or
`FnOnce(Duration) -> Result<(), String>` in the budgeted variant) handed to Eggup
as a `PostInstallCheck`. Eggup runs it **after all members are live and before
backups are discarded** — inside the lifecycle transaction, while the previous
generation is still recoverable. The doc comment on `commit_with_health_check` is
the authoritative statement: "On failure, `policy` selects `RollBack` (restore and
verify the previous generation) or `KeepInstalled` (explicit operator policy
only)."

`OneShotHealthCheck` (`deployment.rs:731`) enforces the shape. It wraps the closure
in a `RefCell<Option<F>>` and fails closed with
`"health check time budget exhausted"` if `remaining` is zero, or
`"health check already consumed"` if it is somehow called twice. It accepts the
Eggup-supplied `remaining` deadline and passes it to the caller's budgeted
closure.

What the CLI's closure actually does, in order: compute
`deadline = now + remaining` and `timeout = min(remaining, 5 s)`, failing with
`"post-install check budget exhausted"` if zero; run
`<installation_root>/bin/eggworkd version` via `eggup_core::run_bounded` with
`.max_output_bytes(MAX_VERSION_PROBE_BYTES)` (bounded, shell-free,
timeout-bounded), failing with `"installed daemon version probe failed"`; require
`output.success()` (`"installed daemon version probe did not succeed"`); parse with
`check_installed_daemon_version(output.stdout(), env!("CARGO_PKG_VERSION"))` and
require `matched()`; then on Linux, with the remaining deadline as budget, require
`check_helper_compatibility_with_timeout(Some(<root>/bin/eggwork-sandbox-helper),
CARGO_PKG_VERSION, helper_budget).compatible()`, else
`"installed sandbox helper is untrusted or version-incoherent"`.

The probe reads the **installed** daemon's own `version` report — not a cached
manifest, not the staged file, not a checksum comparison. It therefore observes the
artifact the node will actually execute, after the transaction.

`check_installed_daemon_version(stdout, expected) -> InstalledVersionCheck` deserves
scrutiny, because the reason it is not a string comparison is subtle:
`MAX_VERSION_PROBE_BYTES = 1024` bounds the output; longer → `Unreadable { "version
probe output exceeded its bound" }`. Non-UTF-8 → `Unreadable { "version probe
output is not UTF-8" }`. The daemon answers `version` with a JSON object
(`{"version": "..."}`), not a bare string — the doc comment says a raw string
comparison "would fail every real update" — so the output is `serde_json`-parsed;
parse failure → `Unreadable { "version probe output is not a JSON object" }`;
missing `version` field → `Unreadable { "version probe output has no version
field" }`. Present and equal → `Matched { version }`; present and different →
`Mismatch { reported }`. Only `Matched` passes.

**The comparison is exact, on purpose.** The doc comment: "a build that reports a
different version is a different release generation, and admitting it would let a
post-install check pass for a binary the operator did not stage." Prefix matching,
"newer than", or "at least this version" would each admit a different binary than
the release being installed, which defeats the point of the check.

**Why a rolled-back disposition is a real post-install failure, not a guess.** The
health check returns `Err` for every observable failure mode: probe spawn failure,
non-zero exit, unparsable report, missing field, version mismatch, and (on Linux)
helper untrust or skew. Any of those makes the post-commit callback fail, and under
the default `post_commit: RollBack` Eggup restores the previous generation and the
command exits non-zero. A rolled-back disposition therefore reports a post-install
failure that was directly observed, not an inference from "something went wrong
somewhere." This is why the CLI maps a `Mismatch` to
`"installed daemon reports version {reported}, not this release"` and an
`Unreadable` to `"installed daemon version report is unusable: {reason}"` — the
bounded failure detail tells the operator which of the two actually happened.

## Helper compatibility policy

`HelperCompatibility` is the full decision surface:

| Variant | Meaning | Fails closed? |
| --- | --- | --- |
| `Compatible { version }` | present, trusted, version-coherent | no |
| `NotConfigured` | no helper path given | yes |
| `Missing` | `symlink_metadata` returned `NotFound` | yes |
| `Untrusted { reason }` | failed location, metadata, ownership, mode, probe, or output checks | yes |
| `VersionSkew { expected, found }` | helper reports a different version | yes |

`helper_satisfies_required_isolation(compatibility) -> bool` is
`compatibility.compatible()` — nothing more. Its doc comment is the policy:
"Only `Compatible` admits required isolation. Every other outcome — including
`NotConfigured` — fails closed and must surface as `capability_mismatch` before
target spawn, never as best-effort downgrade." There is no third state, no
partial trust, and no warning path. Test:
`only_compatible_helper_admits_required_isolation` (`deployment.rs:1542`).

`check_helper_compatibility(path, expected_version)` is
`check_helper_compatibility_with_timeout(..., Duration::from_secs(5))`. The
bounded variant performs, in order:

1. `None` path → `NotConfigured`.
2. `timeout.is_zero()` → `Untrusted { "helper version probe budget is exhausted" }`.
   Zero budget is a refusal, not a shortcut.
3. `symlink_metadata` — `NotFound` → `Missing`; any other error →
   `Untrusted { "helper metadata is unreadable" }`.
4. Must be a regular file and **not** a symlink →
   `Untrusted { "helper must be a regular file" }`.
5. On Unix: `uid == 0 || uid == geteuid()`, and `mode & 0o022 == 0` →
   `Untrusted { "helper ownership or mode check failed" }`; `mode & 0o111 != 0` →
   `Untrusted { "helper is not executable" }`.
6. Probe: `eggup_core::CommandSpec::new(path).arg("--version").timeout(timeout)`
   through `eggup_core::run_bounded`. Err → `Untrusted { "helper version probe
   failed" }`; `!output.success()` → `Untrusted { "helper version probe reported
   failure" }`.
7. `String::from_utf8_lossy(output.stdout()).trim()`; empty, `len() > 64`, or any
   `char::is_control` → `Untrusted { "helper version output is invalid" }`.
8. `found != expected_version` → `VersionSkew { expected, found }`.
9. Otherwise `Compatible { version: found }`.

The probe is bounded on four axes at once: cleared environment and null stdin
(because `run_bounded` is a direct child spawn, not a shell pipeline), a timeout,
bounded output, and kill/reap on expiry. It is not `sh -c`, so there is no
interpolation surface in the helper path.

**Why version skew must fail closed.** A helper is a privilege boundary component.
A daemon expecting isolation semantics from a helper of a different version is
running with an unverified sandbox contract; every admission decision the runner
makes from then on rests on an assumption that was never checked. Failing closed
keeps required isolation *unsupported* rather than degrading it to best-effort.
Test: `helper_version_skew_is_detected` (`deployment.rs:1401`).

### Relationship to `eggwork_runner::verify_trusted_helper`

`crates/eggwork-runner/src/lib.rs` owns the canonical helper trust boundary; the
README states "every caller delegates to it." Its `LANDLOCK_WORKSPACE_RW_CAPABILITY`
constant (`isolation.landlock.workspace-rw.v1`) is the capability name advertised
for the closed Landlock `workspace_rw` profile.

The two implementations are **not** identical, and the difference is worth
stating rather than glossing:

- `verify_trusted_helper` (Linux, `lib.rs:596`) checks the helper file (regular,
  non-symlink, root-or-euid owned, `mode & 0o022 == 0`, `mode & 0o111 != 0`) **and
  then walks the parent and every ancestor directory**, applying the same
  ownership and non-writability rule with a `sticky_root_directory` exception
  (root-owned dir with `mode & 0o1000`). Its own comment says a duplicated stricter
  copy would "deny a user-owned helper that this function would accept, which makes
  an unprivileged user-scope installation permanently unable to advertise required
  filesystem isolation."
- `check_helper_compatibility_with_timeout` checks the file but **not the ancestor
  directories**, then adds the `--version` coherence check.

So the deployment path applies the file-level subset plus version coherence; the
runner applies the full file + ancestor-directory boundary. A real, verifiable
difference in strictness, and a legitimate review focus. The runner's non-Linux
variant returns `Err("trusted Landlock is unsupported on this platform")`
unconditionally. See [sandbox-helper.md](sandbox-helper.md) and
[runner-execution.md](runner-execution.md).

## Service specification and adapters

`service_spec_for_install(executable, config_path) -> ServiceSpec` builds the
identity that every subsequent ownership comparison is made against. It requires
an **absolute** executable (`"daemon executable must be absolute"`) and, if a
config is supplied, an **absolute** config path. The argv always begins with the
literal `"run"` marker, and a config adds `"--config"` plus the path; the config
is *also* recorded in the spec's config field.

The doc comment explains why both participate: "Ownership identity includes the
exact installed executable, the critical `run` argv marker, and the config path
(both argv and config participate so same-name/different-executable or
same-name/different-config can never compare as owned)." The CLI reinforces this:
`--service-config` is **required** for `service` commands — "Fail closed: the
service identity must name the exact registered config path, so it is never guessed
from the operator config."

`render_systemd_unit(executable, config_path) -> Vec<u8>` produces caller-owned
unit bytes; the doc comment is explicit that "Eggwork never writes unit files
itself" — the bytes are passed to `eggup_service::SystemdInstall`. It requires
absolute paths and then applies a **literal-path alphabet** check, permitting only
ASCII alphanumerics and `/_-.+`. The reason is a divergence risk, not tidiness:
"systemd `ExecStart` has quoting, escaping, specifier, and variable expansion
rules. Accept only a literal path alphabet so the rendered command cannot diverge
from `ServiceSpec` argv." The rendered unit is `Type=simple`,
`After=network.target`, `Restart=on-failure`, `RestartSec=2`,
`NoNewPrivileges=true`, `WantedBy=multi-user.target`. Test:
`systemd_unit_rendering_is_bounded_and_absolute_only` (`deployment.rs:1430`).

`render_launchd_plist` is the macOS equivalent and is stricter still: the label
must equal `SERVICE_ID` exactly, paths must be absolute and UTF-8, control
characters are rejected, and all five XML entities are escaped. argv is a literal
XML *array* (`executable`, `run`, `--config`, `config`), never a joined string.
Test: `launchd_policy_uses_literal_argv_and_xml_escapes_paths`
(`deployment.rs:1463`).

`platform_support()` is the read-only observation path: `eggup_service::inspect_host`
through a `SystemExecutor`, returning `HostFacts` with no policy choice and no
mutation. Test `linux_host_reports_manager_facts_without_mutation`
(`deployment.rs:1423`).

### Platform policy is always explicit

| Platform | Required explicit policy | Manager constructor |
| --- | --- | --- |
| Linux | `--unit-path` (absolute), `--scope` `system`\|`user` | `systemd_manager(unit_name, scope, unit_path, exe, config, enable, reload, timeout)` |
| macOS | `--plist-path`, `--launchd-domain` `user`\|`system`, `--launchd-target` | `launchd_manager(domain, target, plist_path, exe, config, bootstrap_on_install, timeout)` |
| Windows | `--windows-start-type` (accepted by the qualification harness, but **no code parses it**; mutating path refused) | `windows_scm_manager(start_type, timeout)` — `#[cfg(windows)]` + `#[allow(dead_code)]`, unreachable |

`parse_systemd_scope` accepts only the literal strings `system` and `user`; the
doc comment says "scope is never guessed from EUID." `systemd_manager` requires an
absolute unit path and derives the unit *name* from the file name. `launchd_manager`
validates the domain/target pairing: `UserAgent` requires a `gui/<uid>` target with
a non-empty all-digit uid; `SystemDaemon` requires the target to be exactly
`system`. Both use a 60 s transition timeout from the CLI. `enable`/`reload` come
from explicit `--enable` and the absence of `--no-reload`.

One honest nuance: the CLI applies `unwrap_or("system")` when `--scope` is
absent. That is a CLI-level default, not a host inference — no EUID or
environment inspection is involved — but it does mean the *scope* value is
defaulted rather than strictly demanded on the command line.

`candidate_managers_for(facts)` is a thin forward to `eggup_service::candidate_managers`;
selection policy there is owned by Eggup, and Eggwork only observes it.

### Adapter availability is never a support claim

A manager type existing for a platform says nothing about whether that platform is
supported. A backend is claimed supported only where the native manager was
actually mutated on a real hosted host. The closure record states this directly:
"Every claimed backend was mutated on a real hosted host. Adapter availability
alone was not treated as a support claim: the Windows adapter successfully
registers the service, and that is precisely why the unsupported disposition had
to be stated explicitly rather than left as 'untested'." See
[release-qualification.md](release-qualification.md) and
[ci-guardrails.md](ci-guardrails.md).

## Windows: unsupported and fail-closed

This is the most carefully hedged part of the module, so the hedging matters.

**What happened.** Hosted M004 qualification created the `eggwork-node` SCM
registration and then failed to start it. The recorded artifact
(`windows-disposition-x86_64-pc-windows-msvc-*` from run `37100047745`):

```
service install exit=0 output={"backend":"windows-scm","completed":true, ...}
service start exit=2 output=eggworkd: service manager failed: start service
  failed in Windows SCM (code 1053)
DISPOSITION: unsupported (registers, then cannot reach Running; no service host)
```

**Why.** Windows error 1053 is "the service did not respond to the start request
in a timely fashion." The cause is structural, confirmed by inspection rather than
inferred: `eggworkd` implements **no service-control dispatcher**. The workspace
grep found no `windows_service::service_dispatcher` or equivalent in `crates/`;
`windows-service` enters the dependency graph only as a transitive dependency of
`eggup-service`'s manager side. The process the SCM launches therefore never
connects back to the manager, and the registration can never reach `Running`. The
adapter registering the service *successfully* is what made this look like
flakiness. It is not: registering an external process would produce a working
installation with **no service behind it**, which is worse than an honest refusal.

**Current behavior.** Every **mutating** `service` verb on Windows is refused
before any mutation. In `product_service_manager` (`eggworkd.rs:278`) the
`#[cfg(windows)]` arm returns an `Err` naming the reason; `service_mutation`
(`eggworkd.rs:269`) calls it *before* `apply_service_operation`, so the refusal
precedes the mutation. `service_backend()` (`eggworkd.rs:378`) still returns
`"windows-scm-unsupported"` for diagnostics, and `deployment status` still reports
host facts.

**What remains supported.** Windows binary installation and daemon runtime are
qualified **independently** of service management. The `deployment status` path,
the install unit, the digest-proof mechanism, the drain decision, and the health
check all remain live; the helper is simply not part of the Windows install unit.
Making the daemon a Windows service host is a **product capability, not a
qualification fix**, tracked as M007
(`plans/implementation/operations-distribution/007-windows-service-host.md`,
recorded ready in `plans/registry.md`). That row adds a standing constraint: the
interim refusal "must stay until a release containing a real host has hosted
Windows evidence."

**What is retained.** `windows_scm_manager` (`deployment.rs:904`) is `#[cfg(windows)]`
and `#[allow(dead_code)]`. It constructs the fixed Eggwork SCM product policy:
`WindowsScmInstall::new(SERVICE_ID, "Eggwork Node Daemon", start_type,
WindowsErrorControl::Normal, None).with_transition_timeout(timeout)`, wrapped in
`WindowsScmManager`. No custom account, dependencies, or elevation. Its doc comment
says it is "the recorded SCM *policy*, retained for review and for a future
service-host milestone. The operator surface does not expose it."

**"Not yet a service host" versus "not supported at all."** The distinction is
deliberate. Windows service *management* is unsupported today — every mutating
verb fails closed by design. The *capability* of running `eggworkd` as a Windows
service host is not supported **yet**: unimplemented, not impossible, with a
milestone for it. The qualification workflow's `windows-platform-disposition` job
fails if either the candidate's registers-and-fails-1053 observation or the
refused `main` changes.

Minor honest note: the refusal message in `eggworkd.rs` contains runs of internal
spaces (a wrapped-literal artifact). Cosmetic, not semantic.

## `DeploymentError` model

`DeploymentError { kind: &'static str, detail: String }`. `Display` renders
`deployment {kind}: {detail}`. `kind()` exposes the machine-readable kind.
`DeploymentError::new` truncates `detail` to 256 characters by `char` count, and
the `From` impls apply `truncate` again for upstream messages — so **every**
detail is bounded regardless of origin. There are no secrets in these strings;
`deployment_surfaces_contain_no_secret_material` (`deployment.rs:1604`) asserts
the module never embeds private key, token, or certificate material.

| `kind` | Raised by | Terminal or recoverable |
| --- | --- | --- |
| `invalid` | `service_spec_for_install` (non-absolute exe/config), `render_systemd_unit` (non-absolute paths, disallowed alphabet), `render_launchd_plist` (bad label/paths/control chars/UTF-8), `systemd_manager` (non-absolute unit path), `launchd_manager` (bad domain/target pairing), `parse_systemd_scope` (not `system`/`user`) | **Terminal** — operator input is wrong; retrying unchanged fails identically |
| `candidate` | `CandidateSource::validate` (unknown member, non-absolute source, bad destination, unreadable/non-regular source, size out of bounds), `build_install_plan` (empty set, invalid product/release/member identity) | **Recoverable** — the operator can stage a corrected candidate and retry; nothing has been mutated |
| `draining` | `wait_for_quiescence` timeout; persistent drain marker could not be set | **Recoverable** — wait longer (`--force` is the explicit alternative) or fix the marker path; no service was stopped |
| `lifecycle` | `From<eggup_service::LifecycleUpdateError>` | **Mixed** — depends on whether Eggup committed; consult the receipt disposition |
| `eggup` | `From<eggup_core::Error>` | **Mixed** — same; a post-commit failure is a real post-install failure |
| `service` | `From<eggup_service::ServiceError>`; `inspect_service` | **Terminal for this attempt** — a service-manager rejection is not retryable without operator change |

The `From` impls are where the `lifecycle` / `eggup` / `service` kinds come from,
and the collapsing is why the receipt, not the error kind, is the authoritative
record of a mixed outcome. A `draining` error is provably pre-mutation (drain
fails before `commit_with_lifecycle` is called). A `candidate` error is provably
pre-mutation by ordering. Anything after `commit_with_lifecycle` is mixed, and the
`RecoveryRequired` disposition is the case where reading the error string alone is
not enough.

### How `RecoveryRequired` differs from a successful update

`RecoveryRequired` is a *disposition reported in the JSON record*, not a
`DeploymentError` kind. It means Eggup preserved exact transaction evidence and
suppressed automatic restoration of the old service generation. The distinction
that matters operationally:

- A successful update prints a clean success record and, because drain stays set,
  still requires an explicit `undrain` before new work is admitted.
- A `RecoveryRequired` disposition **names the manual recovery instead**. The
  command exits non-zero, the record carries the Core artifact disposition,
  "whether manual artifact recovery is required," the lifecycle restoration
  status, and bounded failure detail. The README's rule is absolute: "A
  `RecoveryRequired` disposition never prints a successful update: it names the
  manual recovery instead." The closure record pairs it with the 1053 disposition
  table: `windows-scm | Windows x86_64 | **unsupported** | registers, then start
  fails with Windows error 1053` — the disposition *is* the report.

## Bounded-ness and timeouts

Every bound in and around this module, with what it bounds:

| Bound / timeout | Value | Bounds |
| --- | --- | --- |
| `MAX_CANDIDATE_BYTES` | 256 MiB | max bytes of one staged candidate source; `0 < len <= bound` |
| `MAX_VERSION_PROBE_BYTES` | 1024 bytes | max bytes read from the installed daemon's `version` output |
| `DEFAULT_DRAIN_TIMEOUT` | 30 s | `wait_for_quiescence` wait for active executions before failing closed |
| `DEFAULT_HEALTH_TIMEOUT` | 30 s | `post_commit_timeout` in Eggup's `LifecycleUpdatePolicy` |
| `quiesce_timeout` (hardcoded in orchestration) | 30 s | Eggup's own service-quiesce wait; **not** taken from `policy.drain_timeout` |
| `rollback_restore_timeout` (hardcoded) | 30 s | Eggup's rollback restore-and-verify window |
| `check_helper_compatibility` default probe timeout | 5 s | the `<helper> --version` child process |
| CLI health-probe timeout | `min(remaining, 5 s)` | the `eggworkd version` child process, inside Eggup's budget |
| CLI service transition timeout | 60 s | systemd/launchd start/stop/restart transitions |
| helper version output length | 64 bytes | `--version` stdout, plus a control-character rejection |
| `DeploymentError` detail | 256 chars | every error detail, applied in `new` and again in the `From` impls |
| drain poll interval | 5 ms | quiescence poll granularity |

**Which operations are bounded rather than exhaustive.** Honest accounting:
`wait_for_quiescence` is a *time* bound, not a *count* bound — the execution count
may be any value, but the wait is not. `eggup_core::run_bounded` bounds the child
process by timeout, output, and kill/reap. But several operations in this module
are **not** time-bounded: `hash_file` on a staged candidate reads up to 256 MiB
with no timeout; `CandidateSource::validate`'s `symlink_metadata` and ownership
checks are filesystem operations with no timeout; `platform_support()` /
`inspect_host` executes host inspection with no module-level timeout; and
`render_systemd_unit` / `render_launchd_plist` are unbounded string builds over
operator-supplied paths (bounded in practice by path length). Do not read the
bound table as a claim that every operation in the deployment path is bounded.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
| --- | --- | --- |
| Daemon and helper move in one transaction | `install_unit_matrix` → single `ArtifactSet` | `update_after_drain_replaces_coherent_daemon_helper_pair` |
| Rollback restores a coherent pair | Eggup post-commit policy default `RollBack` | `rollback_restores_coherent_daemon_helper_pair` |
| The install unit is exactly daemon + optional helper | `install_unit_matrix(helper_required)` | `install_unit_moves_daemon_and_helper_together` |
| Matrix agrees with producer packaging | `release_contract.rs` static parity | `install_identities_match_the_deployment_install_unit`, `the_helper_ships_exactly_on_linux_release_targets` |
| The deployment module does not read Eggpack config | module contents (absence check) | `the_deployment_module_never_parses_eggpack_configuration` |
| Every bundle member is `executable` and nothing else | `release/eggpack/install-policy.toml` | `install_policy_marks_every_bundle_member_executable_and_nothing_else` |
| Service identity = exact executable + `run` argv + config | `service_spec_for_install` | `service_spec_distinguishes_executable_and_config` |
| Only `Owned` services are stopped/replaced | Eggup ownership compare via supplied `ServiceSpec` | `owned_install_start_stop_restart_are_idempotent` |
| Foreign same-name registrations never touched | same | `foreign_same_name_registration_fails_closed` |
| Malformed/unknown registrations never touched | same | `unknown_malformed_registration_fails_closed` |
| Candidate validation precedes any service mutation | `orchestrate_update_with_lifecycle_budgeted` step 1 | `candidate_validation_failure_happens_before_any_stop` |
| Active executions block the update without force | `wait_for_quiescence` (force is explicit) | `drain_with_active_execution_blocks_update_without_force` |
| Persistent drain is set by the update path | `set_persistent_drain` before `commit_with_lifecycle` | `update_sets_persistent_drain_and_preserves_recovery_state` |
| Post-install failure rolls back with backups retained | `UpdatePolicy::post_commit` default | `post_start_health_failure_rolls_back_while_backups_are_retained` |
| Replacement failure restores the prior generation | Eggup rollback mechanics | `replacement_failure_rolls_back_to_prior_generation` |
| Installed daemon version is read from its own JSON report | `check_installed_daemon_version` | `installed_daemon_version_is_read_from_its_json_report` |
| Helper version skew is detected | `check_helper_compatibility_with_timeout` | `helper_version_skew_is_detected` |
| Only `Compatible` admits required isolation | `helper_satisfies_required_isolation` | `only_compatible_helper_admits_required_isolation` |
| No direct service-manager invocation in Eggwork | module contents | `no_direct_service_manager_invocation_exists` |
| systemd unit rendering is absolute + literal-alphabet only | `render_systemd_unit` | `systemd_unit_rendering_is_bounded_and_absolute_only` |
| launchd policy uses literal argv + XML escaping | `render_launchd_plist` | `launchd_policy_uses_literal_argv_and_xml_escapes_paths` |
| Host fact reporting mutates nothing | `platform_support` (inspect only) | `linux_host_reports_manager_facts_without_mutation` |
| Deployment surfaces carry no secret material | error/detail construction | `deployment_surfaces_contain_no_secret_material` |
| Windows mutating service verbs fail closed before mutation | `product_service_manager` `#[cfg(windows)]` arm, before `apply_service_operation` | hosted `windows-platform-disposition` job (not a Rust unit test) |

## Test coverage

### `release_contract.rs` (static, no host required)

The producer/consumer parity suite. For this module it asserts: exact install-unit
triples and the `ends_with` file-name rule; the literal member strings and
`SERVICE_ID`; helper presence/absence per target triple; one shared release
identity for the Linux pair; consumer-validator version policy agreeing with the
deployment version policy; the executable-mode install policy; and the *absence*
of any Eggpack configuration dependency in `deployment.rs`, `operations.rs`, and
`eggworkd.rs`. These are what keep `release/eggpack/` and `install_unit_matrix`
from drifting without a runtime coupling.

### `deployment.rs` unit tests (20, `TestDoubleManager` + `TempDir`)

Strong on policy and ordering, silent on real systemd/launchd behavior.
`install_unit_moves_daemon_and_helper_together`,
`service_spec_distinguishes_executable_and_config`,
`owned_install_start_stop_restart_are_idempotent`,
`foreign_same_name_registration_fails_closed`,
`unknown_malformed_registration_fails_closed`,
`candidate_validation_failure_happens_before_any_stop`,
`drain_with_active_execution_blocks_update_without_force`,
`update_after_drain_replaces_coherent_daemon_helper_pair`,
`update_sets_persistent_drain_and_preserves_recovery_state`,
`replacement_failure_rolls_back_to_prior_generation`,
`post_start_health_failure_rolls_back_while_backups_are_retained`,
`installed_daemon_version_is_read_from_its_json_report`,
`helper_version_skew_is_detected`,
`linux_host_reports_manager_facts_without_mutation`,
`systemd_unit_rendering_is_bounded_and_absolute_only`,
`launchd_policy_uses_literal_argv_and_xml_escapes_paths`,
`no_direct_service_manager_invocation_exists`,
`only_compatible_helper_admits_required_isolation`,
`rollback_restores_coherent_daemon_helper_pair`,
`deployment_surfaces_contain_no_secret_material`.

### `tests/installed_qualification.rs`

An **ignored** suite naming the installed binary through the environment. Its two
entry points are `qualification_fixture` and
`installed_release_admits_execution_and_refuses_unsupported_isolation`. Despite
living alongside the deployment surface, it covers **execution admission**,
capability admission, and drain admission — one authenticated loopback execution
through the normal client path. It asserts nothing about `build_install_plan`, the
commit transaction, digest proof, or the health check.

### Hosted closure evidence

`deployment apply` is not covered by a Rust integration test. It is driven by
`scripts/qualify_release.py` (retrieves exact release bytes through the
authenticated GitHub API, verifies them against the release's own
`release-manifest.json` and `.sha256` sidecars, then drives the *installed*
`eggworkd` through the argv list an operator would use, with no shell) and by
`.github/workflows/operational-qualification.yml` on hosted Linux, macOS, and
Windows runners, which uploads the bounded receipts. Recorded subcommands include
`install`, `installer`, `service`, and
`update --release-tag … --prior-release-tag …`.

The Windows 1053 evidence is an **artifact, not a log line**:
`windows-disposition-x86_64-pc-windows-msvc-*` from `37100047745`, reproduced
from `37095687727` / `37096444794`. `plans/closure/operations-distribution/004-status.md`
§9 is the citation of record; the disposition landed in `caa3918`. The same record
notes adversarial ownership coverage: after an owned service is installed, a second
executable with the same name is reported `Foreign` and `install`, `start`,
`restart`, `stop`, `uninstall` are all refused non-zero, with the owned service
verified intact afterwards. "No platform passed by bypassing an ownership check."

### Honest gaps

- No automated test asserts the CLI's Windows refusal message text or that the
  refusal precedes `apply_service_operation`; only the hosted
  `windows-platform-disposition` job observes the end behavior, and it observes
  the *candidate's* old bytes, not the refused `main`.
- `deployment::windows_scm_manager` is `#[allow(dead_code)]` and therefore
  compiles without being exercised by any test.
- `quiesce_timeout` and `rollback_restore_timeout` are hardcoded 30 s literals
  inside `orchestrate_update_with_lifecycle_budgeted`; no test varies or observes
  them, and neither is derived from `UpdatePolicy`.
- The helper trust check in this module does not walk ancestor directories, and no
  test contrasts it against `verify_trusted_helper` (see
  [Helper compatibility policy](#helper-compatibility-policy)).
- `DeploymentStatus::collect` is not directly unit-tested in this module.
- No test covers the `--previous-*-sha256` pairing rule in isolation; the digest
  proof is exercised only through the hosted `update` qualification.

## Review focus

1. **Digest-proof bypass.** Can any path reach a commit that replaces an existing
   file without an `ExactDigestVerifier` entry? Check whether `absent` policy or a
   missing flag combination lets a present file through, and whether the
   all-or-nothing pairing rule can be partially satisfied.
2. **Force-policy abuse.** `force` short-circuits the *only* quiescence wait.
   Confirm nothing else blocks an update on active executions, and that the
   cancelled-work path never marks a non-terminal execution terminal.
3. **Drain state after a failed update.** The marker is set before replacement and
   never cleared by this module. Confirm that every failure path (candidate,
   draining, lifecycle, eggup, service) leaves the node drained rather than
   accidentally admitting work against a half-replaced installation — and that the
   operator-facing message tells the operator a drain is still in force.
4. **Version-check exactness.** `check_installed_daemon_version` must stay an
   exact equality on the JSON `version` field. Any relaxation (prefix, ordering,
   "newer or equal") silently admits a binary the operator did not stage. Also
   confirm `output.success()` is checked, not just parseability.
5. **Helper/daemon pair atomicity.** The pair property rests entirely on both
   members living in one `ArtifactSet` fed to one `InstallPlan`. Verify no caller
   can commit a daemon-only plan on Linux, and that `install_unit_matrix` cannot
   be called with `helper_required = false` on Linux by a future caller.
6. **Receipt interpretation.** The error `kind` collapses three upstream error
   families into coarse buckets, so post-commit outcomes are only distinguishable
   through the receipt disposition. Check that no code path prints a success line
   from the `Result` alone while the receipt says `RecoveryRequired`.
7. **Windows refusal ordering.** The refusal lives in `product_service_manager`,
   which `service_mutation` calls before `apply_service_operation`. Verify the
   ordering survives refactors — a moved `apply_service_operation` call would make
   a "fail-closed" backend mutate.
8. **Rolled-back mistaken for success.** A rollback triggered by the health check
   is a genuine observed post-install failure. Check that the exit status, the
   disposition, and the failure detail all agree, and that `KeepInstalled` is
   never reachable without an explicit operator choice.
9. **`--installation-root` path handling.** The root is operator-supplied and
   `absolute_option`-validated, then `DEST_DAEMON` / `DEST_HELPER` are joined onto
   it and that joined path becomes both the service executable and the health-probe
   target. Confirm there is no way for a relative or symlinked root to redirect the
   probe away from what the transaction wrote.
10. **Hardcoded lifecycle timeouts.** `quiesce_timeout` and
    `rollback_restore_timeout` ignore `UpdatePolicy`. If a caller raises
    `health_timeout`, those two stay at 30 s — is that intended, and should they
    be policy fields?
11. **Trust-check divergence.** The deployment helper check omits the
    ancestor-directory walk that `verify_trusted_helper` performs. Confirm the
    asymmetry is intentional and that no admission decision relies on the weaker
    check.
12. **Bounded vs. exhaustive.** Per
    [Bounded-ness](#bounded-ness-and-timeouts), `hash_file`, metadata checks, and
    host inspection are unbounded. Confirm no claim of "everything is bounded" is
    made in operator-facing text.

## Related

- [overview.md](overview.md) — system topology and where deployment sits.
- [operations-cli.md](operations-cli.md) — the operator surface that drives this
  module: `deployment apply`, `deployment status`, `service …`, `drain`/`undrain`.
- [release-qualification.md](release-qualification.md) — hosted qualification
  evidence and the backend support matrix.
- [distribution.md](distribution.md) — producer/consumer authority split; why the
  producer contract stays outside this module.
- [sandbox-helper.md](sandbox-helper.md) — the Landlock helper itself, the
  `workspace_rw` profile, and its trust boundary.
- [runner-execution.md](runner-execution.md) — capability and drain admission at
  execution time; `verify_trusted_helper`.
- [server-node.md](server-node.md) — the node daemon, persistent drain, and
  recovery state that survives update.
- [ci-guardrails.md](ci-guardrails.md) — what CI enforces about this module.
- [../README.md](../README.md) — normative product behavior for deployment,
  service lifecycle, and Windows disposition.
- `../plans/subsystems/operations-distribution-roadmap.md` — the Operations
  subsystem roadmap, including the M004 Windows disposition and M007.
- `../plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`
  and `../plans/implementation/operations-distribution/002a-eggup-deployment-and-service-integration-corrective.md`
  — the plan and corrective that landed this module.
- `../plans/implementation/operations-distribution/007-windows-service-host.md` —
  the separate milestone that would lift the Windows refusal.
- `../plans/closure/operations-distribution/` — recorded closure status per
  milestone; `004-status.md` §9 is the Windows 1053 citation of record.
- `../plans/registry.md` — M007 registry row stating that the Windows refusal must
  stay until a release with a real service host has hosted Windows evidence.
- `../release/eggpack/` — the producer contract these parity tests keep in
  agreement.
