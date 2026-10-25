# M008b closure record: release harness and documentation single-sourcing corrective

Status: **active** — OP-4 and OP-6 implemented and verified; OP-5 is owned by a
separate worker and is not recorded here
Plan: `plans/implementation/operations-distribution/008b-release-harness-and-documentation-single-sourcing-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Source authority: `plans/subsystems/operations-distribution-operator-truthfulness-corrective-addendum.md` — findings OP-4 and OP-6

This record covers **OP-4** and **OP-6** only. OP-5 (the documentation citation
conversion and the quickstart machine-verification) is owned by another worker
and will be appended as its own section; nothing here should be read as
covering it.

## 1. The rule this milestone establishes

*Multiple readers of one owner are fine; multiple owners are not.*

A second implementation that **reads** the same owner and is itself checked
against it is a useful cross-language check — that is what
`crates/eggwork-server/tests/release_contract.rs` is, and it stays. A second
implementation that **remembers** the fact is a defect: it is a second owner,
and it drifts the first time the first owner changes.

Every fact the corrective removed a remembered copy of was removed, not
re-synced. A constant that is updated when it disagrees with the tree is still
a second owner.

## 2. OP-4 — the release harness re-derived facts that already had one owner

### 2.1 What was wrong

| # | Duplication | Owner of the fact |
|---|---|---|
| 1 | `check_release_tag.py` read the version with a regular expression over the whole `Cargo.toml`, taking the **first** `version = ` line rather than the one under `[workspace.package]` | `Cargo.toml` |
| 2 | `release_candidate_probe.py` read the same field with `tomllib` | `Cargo.toml` |
| 3 | `release_contract.rs` read the same field with the `toml` crate | `Cargo.toml` |
| 4 | `check_release_tag.py` parsed `Cargo.lock` with regular expressions | `Cargo.lock` |
| 5 | `qualify_release.py` derived its expected version from `--release-tag` | the tag — a second authority over the version |
| 6 | `qualify_release.py` asserted a hardcoded `QUALIFIED_ASSET_COUNT` in the same function that derived the expected set | the release manifest |
| 7 | `qualify_release.py` held `SERVICE_ID` and `HELPER_INSTALL_ID` constants | `distribution.toml` / `deployment.rs` |
| 8 | `qualify_release.py` classified targets by triple **suffix** | `distribution.toml`'s asset kind and entries |
| 9 | `check_release_tag.py` was executed by **no** automated job and had **no** test | — |

Items 1–3 are not the same defect. The repository already had the right shape:
`scripts/validate-daemon-version.py` and `scripts/validate-sandbox-helper.py`
*import* `expected_version_line` from `scripts/release_candidate_probe.py`
rather than keeping their own copies. Item 1 was the outlier, and it was the
outlier that mattered, because it was the one deciding whether a release could
be tagged.

### 2.2 What changed

**One manifest reader.** `scripts/release_candidate_probe.py` gained
`read_manifest(root)` — the repository's single `Cargo.toml` parser — and
`workspace_version(root)` now reads `[workspace.package].version` through it.
Both fail closed: an unreadable, oversized, or malformed manifest raises
`ValidationFailure`; there is no regex fallback anywhere. `root` is an optional
parameter so the shared reader can be pointed at a fixture; the default is the
repository root.

**One lock reader.** `check_release_tag.py` → `lock_versions()` parses
`Cargo.lock` with `tomllib` and selects the repository's own packages by the
structural property "a lock entry with no `source`", rather than by a
remembered name prefix. `workspace_member_names()` reads the member list from
the workspace manifest and each member's own manifest, so the guard also
asserts that every member is pinned in the lock at all.

**The two version authorities are reconciled and written down.**
`architecture/release-qualification.md` now has a *Version authority: which
number to trust* section stating which number is the build assertion, which is
the release gate, and that they may differ in exactly one respect (the tag's
`v` prefix). The table in that section is the thing a reader of the harness
consults.

**The harness no longer treats the tag as a version.** `stage_install` reads
`probe.workspace_version()` for its expected version and *delegates* the
tag↔version comparison to `tag_guard.tag_failures()`, so the comparison lives
in exactly one module.

**The asset count is deleted.** `expected_asset_names(manifest, names)` is a
named function that derives the expected set from the release manifest; the
staged set is compared against it; the receipt's count is computed from the
derived set. `QUALIFIED_ASSET_COUNT` is gone.

**The install identities are read from the contract.** `contract_targets()` and
`contract_target()` read `release/eggpack/distribution.toml`;
`target_daemon_install()`, `target_helper_install()`, `target_ships_helper()`
and `target_install_identities()` derive from it. No triple suffix is used
anywhere, and the `.exe` suffix is not applied by the harness at all — the
contract states the Windows install identity itself. The harness's `SERVICE_ID`
and `HELPER_INSTALL_ID` constants are gone.

**The service identity is the daemon's to report.** `service_identity()`
runs `eggworkd service spec` against the installed daemon and reads
`service_id`; the identity is threaded through `service_definition_path()`,
`default_systemd_user_unit()`, `service_policy_flags()`, `service_argv()`,
`service_operation()`, `service_state()` and `owned_service()`. The harness
additionally asserts that the identity the daemon reports in its *mutating*
answer is the one it was told to use.

**The Rust side is unchanged, deliberately.** `release_contract.rs` already
pins `MEMBER_DAEMON`, `MEMBER_HELPER` and `SERVICE_ID` to the contract's own
values; that is the cross-language check, and it stays.

### 2.3 The tag guard is now exercised

`tests/release/test_release_tag_guard.py` is new (21 tests). The guard's
functions take an optional `root`, so the tests drive them against **fixtures**
rather than against the repository's own manifest:

| Test | What it proves |
|---|---|
| `test_a_tag_with_the_v_prefix_matches` | the tag rule accepts `v0.1.5` |
| `test_a_tag_without_the_v_prefix_matches` | the tag rule accepts `0.1.5` |
| `test_a_mismatched_tag_is_refused` | a wrong version is refused, and the message names the manifest's version |
| `test_the_v_prefix_is_the_only_permitted_difference` | `version-0.1.5`, `v0.1.5-rc1`, `0.1.5.1` and `v` are all refused |
| `test_the_guard_reads_the_manifest_through_the_shared_reader` | the guard *calls* `probe.workspace_version`, by recording the call rather than by reading the source |
| `test_the_guard_does_not_parse_the_manifest_with_a_regular_expression` | the guard's source has no `import re`, `re.search`, `re.compile` or `re.MULTILINE` |
| `test_the_reader_returns_the_package_version` | the reader's ordinary case |
| `test_a_manifest_whose_package_section_is_not_first_still_reads_right` | the case the old regex reader got wrong — the test carries a copy of that reader as a control and asserts it reads `9.9.9` from the same fixture the TOML reader reads `0.1.5` from |
| `test_a_malformed_manifest_fails_closed` | fail closed |
| `test_a_manifest_without_a_package_version_fails_closed` | fail closed |
| `test_an_unreadable_manifest_fails_closed` | fail closed |
| `test_a_coherent_lock_passes` | the lock half's ordinary case |
| `test_a_lock_pinned_at_another_version_fails` | a stale pin is refused |
| `test_a_workspace_member_missing_from_the_lock_fails` | an unpinned member is refused |
| `test_registry_dependencies_are_not_ours_to_assert` | only the repository's own packages are selected |
| `test_a_malformed_lock_fails_closed` | fail closed |
| `test_the_repository_passes_the_coherence_half` | the command line CI runs, against the real repository |
| `test_the_repository_passes_its_own_tag` | the command line a maintainer runs, against the real repository |
| `test_a_wrong_tag_fails` | the gate refuses |
| `test_neither_a_tag_nor_the_coherence_flag_is_a_usage_error` | usage |
| `test_a_tag_and_the_coherence_flag_together_is_a_usage_error` | usage |

`tests/release/test_release_harness_derivations.py` is new (14 tests) and
covers the harness side: no literal asset count, no literal install or service
identity (the identities to forbid are themselves read from the contract and
from `deployment.rs`, so the check does not become a second owner), no
classification by triple suffix, no derivation of the expected version from the
tag, the install identities and the target classification resolving from the
contract, the Rust deployment constants agreeing with the contract's bundle
order, the helper shipping exactly on the contract's bundle targets and
exactly on the Rust contract test's explicit Linux list, and the expected asset
set equalling the set the contract's own asset templates produce.

### 2.4 Where the tag gate runs today — and where it does not

Stated plainly, because "the guard is in the repository" is not "the guard
ran":

| Half | Runs | Automaton |
|---|---|---|
| Tag-dependent: "does the tag name the manifest's version?" | `python3 scripts/check_release_tag.py vX.Y.Z`, on the checkout about to be tagged, **before** the tag exists | **A person.** No automated job runs it, and none can: CI never knows the future tag. |
| Tag-independent: "do the manifest's version and the lock's pins agree?" | `python3 scripts/check_release_tag.py --coherence` | **CI**, in the `rust` job, on every push and every pull request. |
| The guard's *logic* | `tests/release/test_release_tag_guard.py` | **CI**, in the `rust` job's `release candidate validator tests` step. |

`.github/workflows/release.yml` is **generated** by `eggpack ci generate` from
`release/eggpack/`, hand-editing it is forbidden, and `eggpack ci check` fails
on drift — so the release-path wiring cannot be done in this repository alone.
The upstream prerequisite is recorded in
`architecture/release-qualification.md`: Eggpack must support a
repository-local preflight step in the generated release workflow that runs the
tag guard against the tag being released, configured at the release workflow's
preflight/staging step (`release/eggpack/workflow-shape.json`,
`release/eggpack/github-policy.json`), invoking
`python3 scripts/check_release_tag.py <tag>` from the checked-out repository
root. Until that ships, the tag half is a **process requirement**, stated in
`docs/releases.md`. This repository has not faked it locally.

## 3. OP-6 — CI did not execute the code it type-checked

### 3.1 The two gaps

**The qualification suite never ran in CI.** `.github/workflows/ci.yml` ran
clippy with `--all-features` — so `crates/eggwork-server/tests/installed_qualification.rs`
was *type-checked* — but ran `cargo test --locked --workspace --all-targets`
without it, and the file is `#![cfg(feature = "qualification")]`. Adding
`--all-features` to the test step would **not** have closed the gap: both tests
are `#[ignore]`d, so the feature set is irrelevant to whether they run. The
suite also names its subject through the environment
(`EGGWORK_QUALIFY_DAEMON`, `EGGWORK_QUALIFY_CONFIG`,
`EGGWORK_QUALIFY_TLS_DIR`) and starts the installed daemon as a child
process, so an ordinary test run has nothing to drive.

**One test was green while proving nothing.**
`crates/eggwork-runner/tests/capability_probe.rs` → `built_helper` looked for
the helper through `CARGO_TARGET_DIR` and `OUT_DIR` and **returned early** when
it could not find it — passing with zero assertions. `CARGO_TARGET_DIR` is not
set by `.cargo/config.toml` and is not exported to integration test binaries;
`OUT_DIR` is a compile-time build-script variable, not a runtime one. The skip
was therefore the *normal* path in CI as well as locally.

### 3.2 What changed

**The probe now locates the helper deterministically and fails loudly.**
`built_helper()` walks the test executable's own directory and its parent — the
`deps` directory and the profile directory above it — which is the same walk
`eggwork_server`'s `locate_helper_binary` performs, and the same shape as the
`CARGO_BIN_EXE_` pattern the sandbox helper's own suite uses. The runner crate
cannot use `CARGO_BIN_EXE_eggwork-sandbox-helper` at all: Cargo only sets that
variable for the package that owns the binary. When the helper is absent the
test **panics** with the paths it looked at and the command that builds it.
The plan prefers the loud failure over `#[ignore]`, because an ignored test is
a hole rather than a check.

**CI builds the helper before the test step.** The `rust` job gained
`cargo build --locked -p eggwork-sandbox-helper` immediately before
`cargo test --locked --workspace --all-targets`, so the precondition holds
deterministically rather than incidentally.

**The Windows job's reason is recorded, not assumed.** The `windows-runner` job
runs the same test target. The helper-dependent half of the probe is
`#[cfg(target_os = "linux")]`, so it **does not exist** in a Windows build at
all and only the platform-neutral negative case — a missing helper must not
advertise a Landlock capability — runs there. The workflow says so in the
step's comment and `architecture/ci-guardrails.md` states it in the coverage
section.

**The qualification suite is executed.** A new `installed-qualification` job in
`.github/workflows/ci.yml` builds the server and helper binaries, stages an
installation root under the runner's temporary directory, checks that root's
trust chain, runs the `rcgen`-based fixture to materialise the TLS material
and the node configuration into it, and then drives the installed daemon
through the production client path with `cargo test --locked -p eggwork-server
--features qualification --test installed_qualification -- --ignored`. The
workflow comment states *why* the default test step does not enable
`qualification` and where the suite is executed instead, so a reader of the
workflow does not have to infer it.

Three conditions in that job are enforced by the job itself rather than by
prose, each one found by running it (§4.2): the installation root's ancestor
chain is checked for group- and world-writable bits; the two execution tests
run with `--test-threads=1`, because they share one installation root, one
state directory, and one execution-state lock; and the fixture's client grant
names `workspace-create` and `artifact-read`, which the declared-artifact test
needs and without which the node refuses the step with a bare `403 forbidden`.
That last one is this work's only functional change to
`crates/eggwork-server/tests/installed_qualification.rs`, and it repairs a
fixture that could not exercise the product: the hosted
`operational-qualification` workflow has a step that runs the declared-artifact
test, and with the old grant that step could not pass on any platform.

### 3.3 Non-vacuity evidence for the capability probe

Executed, not inspected. Both directions were run against the same test
binary:

| Direction | Command | Result |
|---|---|---|
| Helper **absent** | `cargo test --locked -p eggwork-runner --test capability_probe` (helper not built) | **FAILED** — `trusted_helper_advertises_landlock_workspace_rw_capability` panicked with `eggwork-sandbox-helper was not found next to this test executable. expected one of: …/target-w4b/debug/deps/eggwork-sandbox-helper, …/target-w4b/debug/eggwork-sandbox-helper / build it first: cargo build --locked -p eggwork-sandbox-helper`. `test result: FAILED. 1 passed; 1 failed` |
| Helper **present** | `cargo build --locked -p eggwork-sandbox-helper` then the same test command | **OK** — `test result: ok. 2 passed; 0 failed; 0 ignored` |

Before this change the first row would have reported `2 passed`, because
`built_helper` returned `None` and the test returned early. The `expected one
of:` paths in the failure message are the two directories the probe walks,
which is the proof that the lookup is relative to the test executable and not
to an environment variable.

## 4. Verification actually run

Every command below was executed in this working tree. Commands that were
**not** run are listed in §6 with the reason.

| Command | Result |
|---|---|
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | **OK** — 135 tests, 2 skipped (the 2 skips pre-date this work: they are the opt-in Windows-reproducibility checks). 21 of the 135 are the new tag-guard tests and 19 are the new harness-derivation tests; the remainder are the pre-existing suite plus another worker's citation and quickstart modules. |
| `python3 scripts/check_release_tag.py --coherence` | exit 0 — `workspace version and lock agree (0.1.5)` |
| `python3 scripts/check_release_tag.py v0.1.5` | exit 0 — `tag 'v0.1.5' matches workspace version and lock (0.1.5)` |
| `python3 scripts/check_release_tag.py v0.1.9` | exit 1 — `workspace version is '0.1.5', tag 'v0.1.9' expects '0.1.9'` |
| `cargo build --locked -p eggwork-sandbox-helper` | OK |
| `cargo test --locked -p eggwork-runner --test capability_probe` (helper absent) | **FAILED** as designed — see §3.3 |
| `cargo test --locked -p eggwork-runner --test capability_probe` (helper present) | **OK** — 2 passed, 0 failed |
| `cargo fmt -p eggwork-server -p eggwork-runner -- --check` | **OK** — no diff |
| `cargo clippy --locked -p eggwork-runner --all-targets --all-features -- -D warnings` | **OK** — no warnings |
| `python3 scripts/check_execution_ownership.py` | **OK** — `execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)` / `negative scanner self-test passed; crate dependency direction passed` |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | exit 0 |
| `cargo clippy --locked -p eggwork-server -p eggwork-runner --all-targets --all-features -- -D warnings` | **OK in the isolated export** (§4.2) — clean, no warnings. **NOT RUN in this tree** — the crate did not compile while this work was in flight; see §6 |
| `cargo test --locked -p eggwork-server --test release_contract` | **OK in the isolated export** (§4.2) — **36 passed**, 0 failed. **NOT RUN in this tree** — same cause |
| the new `installed-qualification` CI job's steps, end to end | **OK in the isolated export** (§4.2) |

### 4.2 The new CI job's steps, executed end to end

The working tree's `eggwork-server` crate was mid-edit by another worker for
the whole of this work (§6), so the job's steps were executed against an
**isolated export of `HEAD`** (`git archive HEAD` into a scratch directory)
with this work's `crates/eggwork-server/tests/installed_qualification.rs`
copied over it. That is the committed server crate plus this work's only
server-side change.

The job's steps, in order, with their real results:

| Step | Result |
|---|---|
| `cargo build --locked -p eggwork-server -p eggwork-sandbox-helper --bins` | **OK** |
| `cargo clippy --locked -p eggwork-server -p eggwork-runner --all-targets --all-features -- -D warnings` | **OK** — no warnings |
| `cargo test --locked -p eggwork-server --test release_contract` | **OK** — 36 passed, 0 failed |
| `cargo test --locked -p eggwork-runner --test capability_probe` | **OK** — 2 passed, 0 failed |
| stage the installation root (`install -m 0755`, `chmod 0755`) | **OK** |
| check the installation root's trust chain | **OK** — `trust chain ok`. The same script was run against a deliberately group-writable ancestor and **failed** with `… is group/world writable (mode 775); the installed helper's trust check walks every ancestor and would reject it` |
| materialise the fixture (`--ignored --exact qualification_fixture`) | **OK** — 1 passed |
| qualify installed execution (`--ignored --skip qualification_fixture --test-threads=1`) | **OK** — **2 passed**, 0 failed: `installed_release_admits_execution_and_refuses_unsupported_isolation` and `installed_release_reports_a_truthful_declared_artifact_disposition` |

The three findings that came out of running it, rather than reading it, are
recorded in `architecture/ci-guardrails.md`: the ancestor chain must be free of
group- and world-writable bits; the two execution tests must run serially; and
the fixture's client grant must name `workspace-create` and `artifact-read` or
the node refuses the step with a bare `403 forbidden`. The grant change is
this work's only edit to `crates/eggwork-server/tests/installed_qualification.rs`
beyond a documentation paragraph, and the serial-execution and trust-chain
conditions are enforced in the workflow itself.

Running the two execution tests **concurrently** was also executed, and it
**fails**: `installed_release_admits_execution_and_refuses_unsupported_isolation`
fails with `the installed daemon exited before reporting ready`, because the
second daemon cannot take the shared execution-state lock. The job therefore
passes `--test-threads=1`.

### 4.1 Platform and feature matrix actually exercised

| Surface | Exercised | Not exercised |
|---|---|---|
| `scripts/check_release_tag.py` | Linux, Python 3.12, against fixtures and against this repository | Windows and macOS hosts (the guard is host-independent by construction; only the file paths differ) |
| `scripts/qualify_release.py` derivations | Linux, against this repository's `release/eggpack/distribution.toml` | **The harness's network stages (`install`, `installer`, `service`, `update`) were NOT executed** — they need a published release and a token. The derivation functions were exercised directly and by unit test; the end-to-end harness against a real release was not. |
| `crates/eggwork-runner/tests/capability_probe.rs` | Linux, Landlock ABI V4 present, helper built and absent | Windows and macOS (the helper is a Linux-only binary); a kernel without Landlock ABI V4 |
| `crates/eggwork-server/tests/installed_qualification.rs` | Linux, against the build tree's own bytes, in a staged installation root: the fixture and **both** execution tests pass (§4.2) | The hosted macOS and Windows runs, and the published-release bytes |

## 5. Security, compatibility, documentation

**Security review.** No new process creation was added anywhere; the ownership
guard still passes with its negative-exit self-test. The harness now *reads* a
service identity from the daemon instead of remembering one, which removes a
string the harness could disagree with the product about. The tag guard's
inputs are the repository's own files, bounded by size (`MAX_SOURCE_BYTES`,
`MAX_LOCK_BYTES`) and parsed with TOML readers, so a malformed or oversized
input fails closed rather than being partially parsed. The qualification CI job
installs into the runner's temporary directory for the same reason the hosted
workflow does: nothing on the installed helper's path may be group- or
world-writable, and the helper's trust check refuses a tree that is.

**Compatibility review.** No wire format, no capability name, no published
asset, no target set, and no install identity changed. `release/eggpack/` was
**not** edited: it is read-only, and it still carries no release identity. The
published release asset set, the target set and the install identities are
unchanged — only *where they are read from* changed. `scripts/check_release_tag.py`'s
command line gained an optional `--coherence` mode and its tag argument became
optional **only** when that flag is present; `python3 scripts/check_release_tag.py v0.1.5`
behaves as before. Crate dependency edges are unchanged; no dependency was
added.

**Documentation status.** `architecture/release-qualification.md` gained the
version-authority reconciliation, the "what the harness derives, and from whom"
table, and the "where the tag gate runs today — and where it does not"
section. `architecture/ci-guardrails.md` gained the `installed-qualification`
job, the tag-guard and coherence steps, the Windows-job reason, and the two
new test modules. `docs/releases.md` states the process requirement plainly:
whoever cuts the tag runs `python3 scripts/check_release_tag.py vX.Y.Z` before
the tag exists, and CI runs the `--coherence` half on every push and pull
request.

## 6. Unresolved findings, by severity

**Medium — the tag-dependent half of the release gate is still a manual step.**
No automated job compares a tag against the manifest version, and nothing
prevents a maintainer from tagging without running the guard. The upstream
Eggpack prerequisite is recorded in `architecture/release-qualification.md` and
`docs/releases.md`. *This is the milestone's stated boundary, not an
oversight:* CI cannot know the future tag.

**Medium — `CONTRIBUTING.md`'s "Cutting a release" does not mention the guard.**
That file is outside this worker's ownership; the exact proposed change is in
the handoff report. `docs/releases.md` carries the requirement in the meantime.

**Low — the harness's contract *ordering* convention is asserted, not read.**
The harness treats the first install identity of a bundle target as the
daemon's. The contract's own order is pinned independently by
`crates/eggwork-server/tests/release_contract.rs` (which asserts
`install_identities(triple) == vec![MEMBER_DAEMON, MEMBER_HELPER]`) and by
`tests/release/test_release_harness_derivations.py`. A producer that reordered
a bundle would fail those tests rather than silently mis-qualify, and the
harness would additionally fail loudly at run time because the mis-placed
binary does not answer `version`.

**Low — the qualification CI job could not be dispatched from this tree.** Its
steps were executed end to end against an isolated export of `HEAD` (§4.2), but
the GitHub-hosted job itself has not run: only the coordinator can dispatch it.

**Low — the `eggwork-server` crate did not compile while this work was in
flight.** A concurrent worker was editing `crates/eggwork-server/src/lib.rs`
and `crates/eggwork-server/src/operations.rs`; the crate failed to compile with
four errors, all of them inside `src/lib.rs` (`operations::publish_permit_count`
not found, and two `increment_metric`/`increment_metrics` argument mismatches).
Those are the other worker's files and were not touched here. The consequence
for this record is that `cargo clippy -p eggwork-server --all-targets
--all-features` and `cargo test -p eggwork-server --test release_contract` were
not executed **in this tree**; both are the coordinator's to re-run once the
concurrent edit has landed. The `installed-qualification` job's steps *were*
executed, against an isolated export of `HEAD` (§4.2), and this work's only
server-side change — the fixture's client grant — is in that export.
Neither `crates/eggwork-server/tests/release_contract.rs` nor
`release/eggpack/distribution.toml` was changed by this work, so the contract
test's subject is unaffected by it.

**Informational — the tag guard's tag half was not executed against a real
tag here.** There is no tag in this tree to check. The *rule* was executed
against fixtures in both directions, and the command line was executed against
this repository's own version and a deliberately wrong tag.

## 7. Registry and roadmap disposition

The registry and roadmap files are coordinator-owned and were not edited by
this worker. The proposed rows are in the handoff report. This milestone adds
one durable invariant to
`architecture/release-qualification.md`'s enforcement table — *the harness
holds no independently remembered constant for any fact it asserts* — and one
CI fact to `architecture/ci-guardrails.md`'s map: the tag-independent half of
the release gate is enforced on every push and pull request.

## 8. Reproducing this record

```bash
export CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w4b
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
python3 scripts/check_release_tag.py --coherence
python3 scripts/check_release_tag.py v0.1.5
cargo build --locked -p eggwork-sandbox-helper
cargo test --locked -p eggwork-runner --test capability_probe
python3 scripts/check_execution_ownership.py
```

To reproduce the *absent-helper* direction of §3.3, delete
`$CARGO_TARGET_DIR/debug/eggwork-sandbox-helper` (or use a fresh target
directory) before running the capability probe.
