# Operations M003 — Closure (Eggpack Producer Packaging Integration)

Status: **conditionally closed**

Source implementation plan:
`plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`
(status now `conditionally closed`; see "Named outstanding evidence" below)

Roadmap: `plans/subsystems/operations-distribution-roadmap.md`

Reviewed Eggwork baseline before implementation: `b2bf282`
Implementation commit: `012383b4c19b7c598157381a29a3eec9bfaa5c1b`
("feat(operations): adopt Eggpack producer packaging (M003)")

## Closure status rationale

Every implementation-side acceptance criterion is satisfied and the required
local verification is green. The one criterion this record cannot close is
**acceptance criterion 14**, hosted producer build/qualification over all five
required targets. That evidence requires a `workflow_dispatch` run of the
generated `.github/workflows/release.yml` against a real annotated release tag
in this repository, which this environment cannot perform. The milestone is
therefore **conditionally closed** with one named, non-critical outstanding
evidence item, per `plans/003-planning-process.md` §2. The item is a hosted CI
run, not a code or configuration gap, and the plan explicitly did not require
a real draft mutation at M003.

## Exact dependency disposition

| Dependency | Version / revision | Provenance |
|---|---|---|
| `eggpack` CLI (tool pin) | `8507fbeebc6e6a0f8176965d8b21dfc818a03719` | `cargo install --git https://github.com/eggstack/eggpack --rev <sha> --locked eggpack-cli` |
| `eggup-core` | `0.1.1` | crates.io, unchanged from M002 |
| `eggup-service` | `0.1.1` | crates.io, unchanged from M002 |

### Eggpack pin disposition (deviation from the plan's candidate)

The plan named `398cd43bf1597ba49bfc35b5334611aa04b16600` as the initial
qualified pin candidate and permitted a newer pin when the relevant producer
contracts and hosted qualification remain intact. That is what this
implementation does, deliberately:

- The plan's reviewed Eggpack head was `3f95af43` (Build M006 closure). Between
  that head and `8507fbe`, Eggpack closed CI M003e, M003f, and M003g
  (`b9062d4`, `c190e77`, `5acda73`…`e5c81f2`) plus Ecosystem M001. Those
  correctives fixed thirteen real defects in the *generated* release workflow's
  execution path (tool install invocation, cross-tool PATH, transferred
  candidate exec bits, `_validate-consumer` refusing non-`Passed` evidence,
  `_stage-github-draft` arity, error surfacing, `make_latest` encoding). A pin
  at `398cd43` would check in a workflow known to fail on hosted runners.
- `8507fbe` is the only revision with end-to-end hosted evidence for a
  generated five-target release pipeline (eggsact `v1.2.7`, run `36652731202`).
- The producer *contracts* used here are unchanged in shape between the two
  revisions; only rendering and execution wiring changed.
- `8507fbe` is published at `refs/heads/m003g-live-qualification` in
  `github.com/eggstack/eggpack`, so the pin is fetchable. It is **not** on
  `main` yet. This is recorded as a low-severity finding below: until Eggpack
  merges the CI M003e/f/g correctives into `main`, this pin depends on that
  branch remaining published.

Eggpack's `main` head at implementation time was `404f63e`, a planning-only
commit stacked on the plan's reviewed `3f95af43`. The cross-repo disposition
required by the plan's §12 was already recorded in Eggpack at `b9baa93`,
`3d1cb67`, and `404f63e`; no Eggpack change was made by this milestone.

## Checked-in release configuration inventory

| File | SHA-256 | Purpose |
|---|---|---|
| `release/eggpack/distribution.toml` | `3ef0de1b5df948883640062eecac57af34708a6b78b36d3f816fbc0f1151ef89` | release contract: targets, aliases, asset names, install identities, sidecars |
| `release/eggpack/pack.toml` | `f26580444c5898a124f9f75c6aa94b407fe279abb2fd255f27ddc0f5ce4c9b82` | per-target build/qualification host, toolchain, floor, support tier |
| `release/eggpack/build-bindings.toml` | `1a4864fea89eaabc5504c64450407b8030ab074f28c9489915122500f2169716` | explicit Cargo package/binary per logical slot |
| `release/eggpack/qualification-bindings.toml` | `49a76c5be0b87a8f63a3d7f2370c62fff3bf84b2485e8e344262abcb497e9ea1` | fixed-argument daemon version smoke per target |
| `release/eggpack/consumer-validators.json` | `22549324dad8f23273fbeaedab7d42dc196af87b568c63404a9de9f0feb950b6` | bounded consumer validation of the exact candidate |
| `release/eggpack/install-policy.toml` | `1ae309f700597f51ce816f68c858178b2d69134b9309ba41e655bedbaa7b360e` | first-install mode per bundle member |
| `release/eggpack/github-template.json` | `c354e65ce77f1555af20da9c696a5e1bd7a7d098e4b0a7568faebd3f3d3894b4` | static draft template resolved per run |
| `release/eggpack/github-policy.json` | `829baf4bd62ac569d33a527ab3a329245cf1e67007cc73b708db41ba2214fcfb` | runner labels, action pins, tool pin, input paths, staging policy |
| `release/eggpack/workflow-shape.json` | `a47486b8b7a906bbcefc8f84ae5f49ea6275f61d10cd3ab9379946366fb76a1f` | static shape the checked-in workflow is rendered from |
| `.github/workflows/release.yml` | `f97c7c66b94867fa5f32f3cdf9f53e1c01d62df38e5d988134cff17a8e4d4ba7` | generated, 51529 bytes, never hand-edited |

Supporting product-owned files: `scripts/release_candidate_probe.py`,
`scripts/validate-sandbox-helper.py`, `scripts/validate-daemon-version.py`,
`crates/eggwork-server/tests/release_contract.rs`,
`tests/release/test_release_candidate_probe.py`.

`toml 0.8` was added as an `eggwork-server` **dev**-dependency so the parity
suite can read the checked-in configuration. It is test-only: no runtime crate
depends on it, and `Cargo.lock` gained exactly one edge
(`eggwork-server -> toml`).

## Target and artifact matrix

| Canonical target | Alias | Asset form | Release asset (`{version}` = tag) | Install identity |
|---|---|---|---|---|
| `aarch64-apple-darwin` | `macos-arm64` | direct | `eggwork-{version}-aarch64-apple-darwin` | `eggworkd` |
| `aarch64-unknown-linux-gnu` | `linux-arm64` | bundle | `eggwork-{version}-aarch64-unknown-linux-gnu` | `eggworkd` |
| | | | `eggwork-sandbox-helper-{version}-aarch64-unknown-linux-gnu` | `eggwork-sandbox-helper` |
| `x86_64-apple-darwin` | `macos-x64` | direct | `eggwork-{version}-x86_64-apple-darwin` | `eggworkd` |
| `x86_64-pc-windows-msvc` | `windows-x64` | direct | `eggwork-{version}-x86_64-pc-windows-msvc.exe` | `eggworkd.exe` |
| `x86_64-unknown-linux-gnu` | `linux-x64` | bundle | `eggwork-{version}-x86_64-unknown-linux-gnu` | `eggworkd` |
| | | | `eggwork-sandbox-helper-{version}-x86_64-unknown-linux-gnu` | `eggwork-sandbox-helper` |

Every one of the seven published assets gets an Eggpack-generated
`<asset>.sha256` sidecar. No config, database, workspace, blob, or artifact
state is published; the install unit contains only `bin/` members.

## Build-binding matrix

| Target | Slot | Package | Binary |
|---|---|---|---|
| `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | `bundle_entry 0` | `eggwork-server` | `eggworkd` |
| | `bundle_entry 1` | `eggwork-sandbox-helper` | `eggwork-sandbox-helper` |
| `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` | `direct` | `eggwork-server` | `eggworkd` |

No workspace-default binary inference: every slot names its exact package and
binary, and `build_binding_packages_and_binaries_exist_in_this_workspace`
proves each name is a real `[[bin]]` target in this workspace.

## Native runner and qualification matrix

| Target | Build strategy | Build host | Qualification | Support | Runner label |
|---|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | `native_cargo` | linux / x86_64 | `native` | required | `ubuntu-latest` |
| `aarch64-unknown-linux-gnu` | `native_cargo` | linux / aarch64 | `native` | required | `ubuntu-24.04-arm` |
| `x86_64-apple-darwin` | `native_cargo` | macos / x86_64 | `native` | required | `macos-15-intel` |
| `aarch64-apple-darwin` | `native_cargo` | macos / aarch64 | `native` | required | `macos-14` |
| `x86_64-pc-windows-msvc` | `native_cargo` | windows / x86_64 | `native` | required | `windows-latest` |

All five are `SupportTier::Required`; none was silently downgraded to
structural. Toolchain is Rust `1.89.0`, matching `rust-toolchain.toml`. No
compatibility floor and no CargoZigbuild/Zig policy is declared, because
Eggwork documents no deployment floor that would justify one.

Core smoke on every target selects the daemon candidate and runs the fixed,
configuration-free argv `["version"]` (10 s, 8 KiB output bounds). Linux
selects `bundle_entry 0`; the others select `direct`.

## Requirement-to-evidence matrix

| Requirement (plan §) | Evidence | Result |
|---|---|---|
| §4 five canonical targets, finite conventional aliases | `contract_publishes_exactly_the_five_canonical_targets`, `every_contract_alias_resolves_to_its_own_triple`; runtime resolution of all five aliases | Pass |
| §4 Linux two-member bundle, one release identity | `linux_targets_publish_one_two_member_bundle`, `the_linux_helper_and_daemon_share_one_release_identity` | Pass |
| §4 macOS/Windows daemon only | `non_linux_targets_publish_the_daemon_directly`, `windows_publishes_an_exe_suffixed_executable` | Pass |
| §4 helper present exactly on Linux release targets | `the_helper_ships_exactly_on_linux_release_targets` | Pass |
| §4 daemon on every release target | `the_daemon_ships_on_every_release_target` | Pass |
| §4 no node state in release artifacts | `no_release_asset_carries_node_state` | Pass |
| §4 exact sidecars | `every_release_file_has_an_exact_sha256_sidecar` (7 assets, 7 sidecars) | Pass |
| §14 case-insensitive collisions rejected | `expanded_release_names_are_unique_under_case_insensitive_comparison` | Pass |
| §5 `native_cargo` for all five; no floor | `pack_policy_builds_and_qualifies_every_target_natively`, `toolchain_policy_matches_the_checked_in_rust_toolchain` | Pass |
| §5 exact package/bin bindings, no default inference | `build_bindings_name_exact_packages_and_binaries`, `build_binding_packages_and_binaries_exist_in_this_workspace` | Pass |
| §6 native qualification on a matching host | `pack_policy_builds_and_qualifies_every_target_natively`; locally executed on `x86_64-unknown-linux-gnu` (see below) | Pass (configuration); hosted on 4 targets outstanding |
| §6 core smoke `eggworkd version` per target | `core_qualification_smoke_selects_the_daemon_with_a_fixed_argv`, `the_qualification_selector_always_names_a_bound_build_output`; executed qualification evidence below | Pass |
| §6 Linux helper exact-candidate validation is gating | `consumer_validation_is_gating_and_selects_the_linux_helper`; executed `ConsumerValidationEvidenceV1` below | Pass |
| §6 validator is bounded, offline, shell-free, single-argument | `test_probe_never_shells_out`, `test_probe_never_reaches_the_network`, `test_candidate_environment_is_an_allowlist`, `test_validators_take_only_the_candidate_path`, `the_linux_helper_validator_agrees_with_deployment_version_policy` | Pass |
| §6 no candidate output in Eggpack evidence | `test_entrypoint_reports_a_fixed_reason_without_candidate_output`; consumer evidence carries only identity, size, digest, outcome | Pass |
| §6 wrong helper output/version fails | `test_helper_rejects_a_different_version`, `test_helper_rejects_an_empty_version_line`, `test_helper_rejects_more_than_one_line`, `test_helper_rejects_the_daemon_as_a_substitute` | Pass |
| §6 wrong selector/candidate identity fails | `the_qualification_selector_always_names_a_bound_build_output`, `test_validators_reject_a_symlinked_candidate`, `test_validators_reject_an_unusable_candidate`; substituted candidate refused with `CandidateMismatch` | Pass |
| §6 missing required target suppresses final release | executed `_evaluate-gate` with only one of five required targets present: exit 1, no gate outcome written | Pass |
| §7 static config carries no future identity | `static_release_configuration_embeds_no_future_release_identity`; zero occurrences of `0.1.0` in `release.yml` | Pass |
| §7 exact reviewed tool pin | `github_policy_pins_every_runner_action_and_the_tool_immutably`; `8507fbeebc6e6a0f8176965d8b21dfc818a03719` in `github-policy.json` and in the rendered `cargo install --rev` line | Pass |
| §7 no `installer-presentation.json` (GeneratedDefault) | `the_github_draft_policy_is_draft_only_and_needs_no_wrapper` | Pass |
| §8 generated workflow, not a hand-written matrix | `no_second_hand_maintained_release_matrix_exists`; `eggpack ci generate` output is byte-identical to the checked-in bytes | Pass |
| §8 `workflow_dispatch` + exact `release_tag`, no latest/branch fallback | `the_generated_workflow_never_publishes_or_mutates_tags` | Pass |
| §8 identity resolved once and verified by every job | `the_generated_workflow_propagates_one_runtime_identity_to_every_job` (18 source-verifying jobs, 1 resolve, 1 preflight upload + 18 downloads) | Pass |
| §8 only the staging job may write | `the_generated_workflow_grants_contents_write_to_only_the_staging_job` | Pass |
| §8 immutable action pins, no `--rev main` | `the_generated_workflow_pins_actions_and_installs_the_pinned_tool` | Pass |
| §8 no publication, tag mutation, or clobber | `the_generated_workflow_never_publishes_or_mutates_tags` (no `--clobber`, `gh release`, `make_latest`, `git tag`, `git push origin`, `actions/github-script`, `curl`) | Pass |
| §8 `eggpack ci check` zero drift | `ci check: match (51529 bytes)` against the pinned tool | Pass |
| §8 no arbitrary shell-command extension interface | the generated workflow has no `workflow_dispatch` free-form input beyond `release_tag`; no `actions/github-script`; the `ValidatorInterpreterV1` enum is exactly `Python3` | Pass |
| §9 draft-only staging, token via environment only | `the_generated_workflow_never_publishes_or_mutates_tags` (exactly two `GITHUB_TOKEN` occurrences, both inside the `stage` job), `the_github_draft_policy_is_draft_only_and_needs_no_wrapper` | Pass |
| §9 exact asset-set reconciliation, no `--clobber` | Eggpack's `stage` job; absence of `--clobber` asserted | Pass |
| §10 `GeneratedDefault` presentation | no `installer_presentation` in `release_inputs` or staging `inputs` | Pass |
| §10 daemon and Linux helper executable mode, no data member | `install_policy_marks_every_bundle_member_executable_and_nothing_else`; rendered `chmod 755` on both bundle payloads | Pass |
| §10 bootstrap is first-install only | generated installers contain 21 `destination already exists` guards (`install.sh`) and 14 (`install.ps1`); zero matches for `latest/download`, `systemctl`, `launchctl`, `sc.exe`, `sudo`, `curl -s`, `chown` | Pass |
| §11 static release/deployment parity | the whole `release_contract` suite reconciles `deployment::{PRODUCT_ID, MEMBER_DAEMON, MEMBER_HELPER, DEST_DAEMON, DEST_HELPER, SERVICE_ID, install_unit_matrix}` with the checked-in configuration | Pass |
| §11 no runtime Eggpack dependency | `the_deployment_module_never_parses_eggpack_configuration`; `toml` is dev-only; `eggwork-server` has no `eggpack` dependency in `Cargo.toml` | Pass |
| §12 no Eggpack Eggup-interoperability claim | this record's "Non-claim" section; no manifest-driven acquisition, discovery, or update path was added | Pass |
| §13 documentation | `README.md` release/install section, `architecture/distribution.md`, `architecture/overview.md`, `CONTRIBUTING.md` regeneration and cut-a-release instructions | Pass |
| §14 existing Operations M002 / Security M004 regression | `cargo test --workspace --all-targets` — 164 passed, 1 ignored across 9 suites; ownership guard passes; `no_direct_service_manager_invocation_exists` green | Pass |
| Acceptance 1 Eggpack is the sole producer authority | only `release/eggpack/` + the generated workflow describe the release matrix | Pass |
| Acceptance 5 Linux helper validation is gating | required validator on both Linux targets; a failing required validator suppresses the release before the gate | Pass |
| Acceptance 6 manifest and sidecars from finalized bytes | Eggpack's `_aggregate` finalization; the executed local finalization probe below reproduced the sidecar/manifest relationship byte-for-byte | Pass |
| Acceptance 8 deterministic workflow guarded by `eggpack ci check` | two independent generations produced identical SHA-256 `f97c7c66…`; CI drift job added | Pass |
| Acceptance 9 no future identity in static config | see §7 | Pass |
| Acceptance 10 no automatic publication or tag mutation | see §9 | Pass |
| Acceptance 11 service/drain/update/rollback stays Eggwork/Eggup-owned | no `eggworkd` command, deployment type, or service path changed | Pass |
| Acceptance 12 no runtime Eggpack dependency | see §11 | Pass |
| Acceptance 13 Operations M002 / Security M004 green | see §14 | Pass |
| Acceptance 14 hosted five-target producer run | **not obtained**; named outstanding evidence | **Outstanding** |
| Acceptance 15 no unresolved high/medium finding | none; two low-severity findings recorded below | Pass |

## Executed verification (actually run)

Host: Linux x86_64, kernel `6.8.0-142-generic`; `cargo 1.89.0` / `rustc 1.89.0`
for the release probe; `python3 3.12.3`; `bash`; `pwsh` present.

Eggwork:

- `python3 scripts/check_execution_ownership.py` — passed (2 + 1 approved spawn sites).
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — no issues.
- `cargo test --workspace --all-targets` — 164 passed, 1 ignored, 9 suites
  (includes the new 32-test `release_contract` suite).
- `cargo check --workspace` — passed.
- `git diff --check` — passed.
- `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`
  — 26 passed.

Eggpack-driven release configuration, at the exact pinned revision
(`eggpack 0.1.0` built from `8507fbe`):

- `eggpack ci generate --workflow-shape … --contract … --github-policy …`
  → 51529 bytes; run twice, both times SHA-256
  `f97c7c66b94867fa5f32f3cdf9f53e1c01d62df38e5d988134cff17a8e4d4ba7`.
- `eggpack ci check --workflow-shape … --contract … --github-policy … --workflow .github/workflows/release.yml`
  → `ci check: match (51529 bytes)`.
- `eggpack ci _resolve-release` against this repository's `HEAD`
  (`b2bf282f6ef80a329e552c4f021bedec7b792218`) with all five selected aliases →
  `resolved runtime release identity`. The resolved plan carries all five
  targets with the intended artifact forms (`direct` ×3, `bundle` ×2), all
  `native_cargo` / `native` / `required`.
- `cargo build --release --locked --target x86_64-unknown-linux-gnu --package eggwork-server --bin eggworkd`
  and `… --package eggwork-sandbox-helper --bin eggwork-sandbox-helper`
  → 10350648 and 704504 bytes.
- `eggpack ci _capture-build --target x86_64-unknown-linux-gnu` → handoff with
  exactly two outputs, `bundle_entry 0` = `eggwork-server`/`eggworkd` and
  `bundle_entry 1` = `eggwork-sandbox-helper`/`eggwork-sandbox-helper`.
- `eggpack ci _qualify-target --target x86_64-unknown-linux-gnu` →
  `qualified x86_64-unknown-linux-gnu: Passed`; evidence records
  `planned_classification: native`, `method: native`, `actual_host:
  {linux, x86_64}`, `support: required`, `smoke_selector: bundle_entry 0`,
  both candidates as `elf`/`x86_64` with SHA-256
  `4e492570dcc5180f4206d7b4805a8dc829a2edd2286bf9f1cfdf408d3ba8db1b` (daemon)
  and `b332ef36df8c32d52781ef26cd4eca6bcc874b8bf546e113089eafbd4ac3a0b8` (helper).
- `eggpack ci _validate-consumer --target x86_64-unknown-linux-gnu` →
  `consumer validation passed`; evidence is
  `{"selector":{"kind":"bundle_entry","index":1},"interpreter":"python3","outcome":"passed","candidate_size":704504,"candidate_sha256":"b332ef36…"}`
  — identity and outcome only, no candidate output.
- Negative: substituting a different payload for `bundle_entry 1` is refused
  with `consumer validation failed for x86_64-unknown-linux-gnu: CandidateMismatch`
  before the validator runs.
- Negative: `eggpack ci _evaluate-gate` with only one of five required targets
  present → exit 1, no gate outcome. A missing required target suppresses the
  release.
- Bootstrap projection probe: with a manifest covering all five contract
  targets and a matching finalized root, `eggpack ci _prepare-stage` produced
  the deterministic payload (17 assets) and both generated installers. The
  POSIX installer has one branch per runtime pair (`linux:aarch64`,
  `linux:x86_64`, `macos:aarch64`, `macos:x86_64`, `windows:x86_64`); both
  Linux branches place `eggworkd` and `eggwork-sandbox-helper` with
  `chmod 755`, and each macOS/Windows branch places the daemon only
  (`eggworkd`, `eggworkd.exe` on Windows).
  `bash -n install.sh` passed and
  `[System.Management.Automation.Language.Parser]::ParseFile` on
  `install.ps1` reported zero errors.
  This probe used the real Linux x86-64 binaries for the Linux slots and
  repeated copies of them for the non-Linux slots purely to exercise the
  *shape* of the projection. It is **not** release evidence and makes no claim
  about macOS or Windows binaries.

## Non-claim: Eggpack's separate Eggup Interoperability M003

Eggwork does **not** close, and does not claim to close, Eggpack's Eggup
Interoperability M003. That milestone is about a real runtime consumer
currently duplicating ReleaseManifest-to-update mapping. Eggwork accepts local
`CandidateSource` files, has no release discovery or manifest acquisition
policy, and does not translate a remote manifest into an update plan. Adding
any of that here would move release selection into Eggwork and add a runtime
Eggpack dependency, which this milestone forbids. The cross-repo disposition
was already recorded in Eggpack planning at `b9baa93` / `3d1cb67` / `404f63e`,
which distinguishes this producer-only adoption from that runtime-consumer
milestone. The two names must not be conflated.

## Documentation disposition

- `README.md` — new "Releases and installation" section stating the four
  authorities, the Linux bundle rule, the first-install-only bootstrap, the
  draft-only staging boundary, and that SHA-256 is integrity rather than
  authenticity. The previous sentence claiming producer packaging was out of
  scope was removed.
- `architecture/distribution.md` — new owner document: per-file producer
  authority, invariants, and the explicit non-goals.
- `architecture/overview.md` — links the new document.
- `CONTRIBUTING.md` — full pre-submit command list, the exact regeneration and
  drift commands with the pinned revision, the identity-free static-config
  rule, and the cut-a-release procedure.
- `crates/eggwork-server/src/deployment.rs` deliberately unchanged: the plan
  forbids the runtime deployment module from referencing producer
  configuration, and `the_deployment_module_never_parses_eggpack_configuration`
  enforces that.

## Compatibility and security review

- No `OperatorConfig` schema change, no command-surface change, no new `eggworkd`
  verb, and no `eggup` version change. The install unit, destinations, service
  identity, drain marker, and post-commit rollback policy are byte-identical.
- Release artifacts gain no new permissions or capabilities; the Linux helper
  keeps its existing `--version` protocol.
- Least privilege: the generated workflow grants `contents: read` at workflow
  level and `contents: write` to the `stage` job alone; no `id-token: write`,
  no `packages: write`, no `deployments: write`. The staging token is bound
  only through the job environment.
- The consumer validators run with an allowlisted minimal environment, no
  shell, no network, null stdin, a 30 s internal bound on top of Eggpack's, and
  bounded output. They print fixed reason phrases only, so a failing run cannot
  leak candidate bytes into workflow logs or evidence.
- No credential, token, or digest is committed anywhere in `release/eggpack/`.
- The one new dev-dependency edge (`toml`) cannot affect any runtime behavior.

## Named outstanding evidence

1. **Hosted five-target producer run (acceptance 14).** Dispatch
   `.github/workflows/release.yml` with an exact existing annotated release tag
   and record the run identifier plus the five build, qualification, and
   consumer-validation job results. Until then, only
   `x86_64-unknown-linux-gnu` has executed producer evidence from this
   repository, and the macOS/Windows/ARM targets are **not** qualified. This is
   the first obligation of Operations M004.

## Unresolved findings

| Severity | Finding | Disposition |
|---|---|---|
| None high/medium | No unresolved high or medium finding in this milestone's scope. | Acceptance 15 satisfied. |
| Low | The Eggpack tool pin `8507fbe` lives on `refs/heads/m003g-live-qualification`, not `main`. If that branch is deleted the pin becomes unfetchable and the release workflow cannot install its tool. | Owned upstream. Re-pin to the merged `main` head once Eggpack merges CI M003e/f/g; the change is one field in `release/eggpack/github-policy.json` followed by regeneration. No Eggwork code change. |
| Low | `sandbox_timeout_and_cancellation_reap_the_helper_process_group` (`crates/eggwork-sandbox-helper/tests/landlock_runner.rs`) is timing-sensitive and failed once in six full-workspace runs with `CancelledBeforeSpawn` after its child-pid wait had already succeeded. It passed on three consecutive subsequent full runs and on three isolated runs. | Pre-existing flake outside M003's diff (no M003 change touches `eggwork-runner`, `eggwork-sandbox-helper`, Landlock, or cancellation). Owned by Foundation/Security regression work. Does not affect any M003 claim. |

## Registry and roadmap disposition

- `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`:
  ready for handoff → conditionally closed.
- `plans/subsystems/operations-distribution-roadmap.md` M003: ready for handoff
  → conditionally closed, with the named outstanding hosted evidence.
- `plans/registry.md`: Operations M003 moves to the closed table with the
  conditional qualifier; the Operations workstream row is updated.
- **Operations M004 becomes ready.** Its M002-side dependency was already
  satisfied and this closure satisfies the M003 dependency. M004 owns live
  release/draft/rerun evidence, installed first-install smoke, service
  lifecycle/update/rollback on qualified hosts, the final macOS/Windows hosted
  support disposition, and — as its first act — the named outstanding hosted
  five-target producer run above.
- No other plan is unblocked by this closure. Operations M005 and M006 remain
  deferred for their own reasons, and CodeGG M004 remains deferred on the
  stable AgentRun worker-entry contract.
