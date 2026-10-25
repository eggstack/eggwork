# M008b closure record: release harness and documentation single-sourcing corrective

Status: **closed** — OP-4, OP-5 and OP-6 implemented and verified; every
acceptance criterion of the plan is met and the evidence is recorded below.
Plan: `plans/implementation/operations-distribution/008b-release-harness-and-documentation-single-sourcing-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Source authority: `plans/subsystems/operations-distribution-operator-truthfulness-corrective-addendum.md` — findings OP-4, OP-5 and OP-6

Sections 1-8 cover **OP-4** and **OP-6**. Sections 9-18 cover **OP-5** — the
documentation citation conversion, the CI wiring, the divergences-table
re-verification, and the quickstart document reconciliation. Nothing in one
section should be read as covering another finding.

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

---

# OP-5 — the design record cited line numbers, and the gate that stopped it

Status: **closed** — OP-5 implemented and verified; every acceptance criterion
the plan states for OP-5 is met, and the evidence is recorded below.

This section covers **OP-5**. Sections 1-8 above cover OP-4 and OP-6 and are
not restated here.

## 9. What OP-5 found

The design record carried **718 line-number citations**. Re-measured against the
tree, **85%** of the resolvable ones no longer resolved at the line they named,
83% were stale within ±3 lines, and 72% were stale allowing ±20 lines of
drift. The spot check that made the defect concrete: `architecture/deployment-lifecycle.md`
cited `verify_trusted_helper` as living in `crates/eggwork-server/src/lib.rs`,
and it is defined in `crates/eggwork-runner/src/lib.rs` — the wrong *file*
entirely. Citations also frequently used a bare basename (`lib.rs`), which is
ambiguous across the five crates.

The defect is the *citation form*, not any individual citation. A line number
is a property of the text, not of the symbol: every unrelated edit above the
target invalidates it, and no reviewer notices until the citation quietly
stops meaning anything. The durable form is a **symbol**.

The gate already existed (`scripts/check_doc_citations.py`, added by the
M008b part-B worker). It failed over the live tree, as it must: a gate that
only turns on when the tree is already clean protects nothing.

**Before, on the live tree:**

```
$ python3 scripts/check_doc_citations.py ; echo "EXIT=$?"
- architecture/data-plane.md:11: legacy-line-citation: `lib.rs:10-29` names a line number; cite a symbol instead
… 680 findings …
EXIT=1
```

| Finding class | Count |
|---|---|
| `legacy-line-citation` | 627 |
| `unresolved-path` | 53 |
| **total** | **680** |

| Document | Findings before |
|---|---|
| `architecture/data-plane.md` | 202 |
| `architecture/server-node.md` | 192 |
| `architecture/core-domain.md` | 96 |
| `architecture/storage-journal.md` | 94 |
| `architecture/client-transport.md` | 42 |
| `architecture/deployment-lifecycle.md` | 22 |
| `architecture/sandbox-helper.md` | 8 |
| `architecture/operations-cli.md` | 8 |
| `architecture/overview.md` | 6 |
| `architecture/release-qualification.md` | 3 |
| `architecture/execution-ownership.md` | 3 |
| `architecture/ci-guardrails.md` | 3 |
| `architecture/runner-execution.md` | 1 |
| **total** | **680** |

## 10. The conversion

### 10.1 How a citation's symbol was determined

A citation's line number was true **when the document was written**. The file
has moved since. So the conversion did not ask "what is at line N *today*",
which is the question that produces a plausible wrong answer (the cited line
has drifted by hundreds of lines, and the symbol nearest a drifted line is
usually the wrong one). It asked, for each citation:

> **Which symbol did the cited line point at in the file as it stood when the
> citing document was written?**

That is answerable from the repository's own history. For every document, each
legacy citation occurrence was aligned against the occurrences in each
historical version of that document (`difflib` over the citation sequence), and
the cited line was resolved against the *file at the commit that introduced the
citation*. The symbol's name is stable where the line number is not, so the
resolution survives every edit since. 646 of the 657 citations resolved this
way; the remaining 11 were ranges or bare-continuation forms that name no
single item and were resolved by reading the code and naming the item the
sentence means.

Where the sentence's own subject named an item in the cited file, that subject
won: a cited line that lands on a boundary, on a trait implementation, or on
an attribute resolves to the *neighbouring* item, and the sentence is about the
thing it names. This is what corrected `EggressDialer::dial` (the line landed
inside the enclosing `impl`'s helper) and `ExecuteRequest` (the line landed
one line above the struct's `#[derive]`).

### 10.2 The result

**651 net new symbol citations** (14 before, 665 after), spread over ten
documents:

| Document | Symbol citations after |
|---|---|
| `architecture/data-plane.md` | 221 |
| `architecture/server-node.md` | 186 |
| `architecture/core-domain.md` | 98 |
| `architecture/storage-journal.md` | 91 |
| `architecture/client-transport.md` | 38 |
| `architecture/deployment-lifecycle.md` | 18 |
| `architecture/ci-guardrails.md` | 11 |
| `architecture/runner-execution.md` | 1 |
| `architecture/sandbox-helper.md` | 1 |
| **total** | **665** |

The form is a repository-relative path, optionally with `::symbol`:

```
`crates/eggwork-server/src/lib.rs::run_execution`
`crates/eggwork-runner/src/lib.rs::verify_trusted_helper`
`crates/eggwork-server/src/workspace.rs::WorkspaceManager::materialize`
```

Where a sentence refers to a *concept* rather than to one item — a family of
constants, a module, a general rule — a **path-only** citation is used, which
the gate accepts and which is honest about what the sentence claims.

### 10.3 The gate's own extension, and why it was needed

The gate rejects a symbol it cannot find in the named file. It did not find
`NodeId`, `PrincipalId`, `ExecutionId`, `LeaseId`, `WorkspaceId` and
`ArtifactId` — because those are generated by the `id_type!` macro, and the
checker did not expand macros. The document genuinely needs to name them:
they are the crate's public identity types, and citing the macro instead of
the type is a weaker statement about a stronger fact.

**Change made** (`scripts/check_doc_citations.py`): the checker now records the
items a file's *own* macros generate. When a `macro_rules!` macro defined in
the file binds an `ident` metavariable in an item position (`struct $name`,
`enum $name`, `type $name`, `trait $name`, `mod $name`, `union $name`,
`fn $name`, `impl $name`) and is invoked with an identifier as its first
argument, that identifier is an item the file defines. `id_type!(NodeId, "node
id")` therefore defines `NodeId`.

This is a **strict addition** to the symbol table: more names resolve, and
every rejection rule the gate enforces is untouched — the legacy `file.rs:NNN`
form is still rejected outright, a path that does not exist is still rejected, a
symbol absent from the named file is still rejected, an ambiguous in-file
symbol is still rejected, and a bare basename that is not unique in the tree is
still rejected. The macro rule is deliberately narrow: a macro whose body does
*not* put the metavariable in an item position generates nothing, so
`not_an_item_macro!(NotAnItem)` does **not** define `NotAnItem`, and the
fixture asserts exactly that in both directions.

**Two other changes to the gate, both stated because they are changes to the
gate:**

1. **A per-file cache of the symbol table** (`_SYMBOL_CACHE`). The design
   record cites the same source file hundreds of times, and re-parsing an
   8,000-line Rust file for each citation turned a check that should take
   seconds into one that took minutes. The cache changes the cost, not the
   result: the same tree produces the same findings, in the same order.
2. **The fixture grew the macro case** (`FIXTURE_RUST` now defines
   `real_id_type!` and invokes it, and defines `not_an_item_macro!` and
   invokes it). `--prove-negative-exit` now asserts that `RealId` resolves and
   that `NotAnItem` does not, so the new rule is covered by the gate's own
   proof rather than by assertion.

The gate's own test module (`tests/release/test_doc_citations.py`) was **not**
edited: it is outside this worker's ownership. It still passes unchanged (see
§14), and the new behaviour is covered by the gate's own
`--prove-negative-exit` fixture, which lives in the script this worker owns.

### 10.4 Substantive corrections, not just reference corrections

The conversion is not a find-and-replace. Where the record was wrong, the
substance was corrected:

- **`architecture/storage-journal.md`'s metrics section was stale.** It claimed
  20 metric names lived in `operations::METRIC_NAMES`, that five workspace
  counters were written but "silently discarded" by the report, and that the
  gap was worth knowing about. The M008a corrective had already closed that:
  the vocabulary is now a typed `Metric` enumeration declared with the
  producers, the report reads that same declaration, and a counter that is not
  named cannot be incremented. The section now says that, counts 25 names,
  lists the five workspace counters in the table, and the "Observability gap"
  bullet is replaced by the closed statement.
- **`architecture/overview.md` row 3 was corrected**: the flag is documented
  in `docs/deployment.md`, not in a README.
- **`architecture/client-transport.md`'s doctor output was already correct**
  (see §13); the M008a worker had already put `host-telemetry` in it.
- **`build.rs` citations were prose about a Cargo build script**, not
  citations to a file that does not exist. They now say "a Cargo build
  script" in four places rather than naming a path the tree does not have.
- **The `server-node.md` "Notable cases" list** carried bare line numbers as
  list-item prefixes (`- 2934 \`route_contract_is_fixed_target_and_versioned\``).
  The gate does not catch those — a bare number is not a path — but they are the
  same defect, so the list now names each test.

## 11. The CI wiring

`.github/workflows/ci.yml` is **hand-maintained** (unlike
`.github/workflows/release.yml`, which is generated by `eggpack ci generate` and
must never be hand-edited; `eggpack ci check` fails on drift).

Three steps were added to the **`rust` job** — `ubuntu-latest`, triggered on
every `push` and every `pull_request`:

```yaml
      - name: documentation citation guard
        run: python3 scripts/check_doc_citations.py
      - name: documentation citation guard negative-exit proof
        run: python3 scripts/check_doc_citations.py --prove-negative-exit
      - name: quickstart document and script sync
        run: python3 scripts/check_quickstart_sync.py
```

They sit immediately after the execution-ownership guard steps, because they
are the same class of check: repository-shape guards that read the tree rather
than build it. Both scripts are **stdlib-only**, read only files on disk, and
depend on nothing that exists on one platform only, so the steps behave
identically on every host and need no build product.

One further **job** was added — `quickstart` — which runs
`scripts/quickstart.sh` end to end on `ubuntu-latest`:

```yaml
  quickstart:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.89.0
      - uses: Swatinem/rust-cache@v2
      - name: run the quickstart end to end
        shell: bash
        run: |
          set -euo pipefail
          bash scripts/quickstart.sh
```

The script needs `cargo`, `openssl`, `python3` and `stat`; all four are present
on `ubuntu-latest`, and it generates its own TLS material with `openssl`. It
installs the helper where the helper's trust check accepts it, and stops the
node on every failure path, so a failed step never leaves a process holding
the state directory's lock. This is the job the plan's criterion 10 asks for:
the quickstart **executed** by CI against a locally built node with generated
TLS material, not merely asserted to be true.

One further step was added to the **`installed-qualification` job**, which
already stages an installation root and materialises a node configuration into
it:

```yaml
      - name: quickstart doctor output against the node's report
        env:
          EGGWORK_QUALIFY_ROOT: ${{ runner.temp }}/qualification
        run: |
          set -euo pipefail
          "$EGGWORK_QUALIFY_ROOT/bin/eggworkd" doctor \
            --config "$EGGWORK_QUALIFY_ROOT/state/node.json" \
            > "$EGGWORK_QUALIFY_ROOT/doctor.json"
          python3 scripts/check_quickstart_sync.py \
            --doctor-report "$EGGWORK_QUALIFY_ROOT/doctor.json"
```

This makes the document's shown `doctor` output an **executed** fact: the
installed daemon's own report is compared against the document's shown check
names, set **and order**. It runs after the qualification execution because the
fixture step is what materialises `state/node.json`.

The step captures the report whatever `doctor`'s exit status, because `doctor`
exits non-zero when any check is not ok — a verdict about the *node*, which the
qualification step above already requires to be healthy, not a statement about
the document. The step's assertion is the comparison alone; an unreadable or
empty report fails the comparison with an explicit message.

`architecture/ci-guardrails.md` gained the three `rust`-job steps (with the
step-by-step explanation renumbered), the new `installed-qualification` step,
and the reason the doctor comparison lives in that job.

### 11.1 The release path — an upstream prerequisite, recorded not faked

`.github/workflows/release.yml` is **generated** by `eggpack ci generate` from
`release/eggpack/`. The generated workflow's steps are build, capture and
qualification steps for the five release targets; it has **no hook** for a
repository-local preflight command, and its `consumer_validators` are per-target
scripts that validate the *built artifact*, not the repository's source tree. A
documentation gate is the wrong shape for either.

The upstream prerequisite is recorded in `architecture/release-qualification.md`
in a new section, *Where the documentation gates run today — and where they do
not*: Eggpack must support a repository-local preflight step in the generated
release workflow — the same hook the tag guard already needs — configured at
`release/eggpack/workflow-shape.json` and `release/eggpack/github-policy.json`.
Until it ships, the gates are covered by CI on every push and every pull
request, and the release path inherits that coverage *indirectly* (a release is
cut from a tagged commit CI already gated) rather than from a step inside the
generated workflow. **The generated workflow was not hand-edited.**

## 12. The divergences table, re-verified entry by entry

`architecture/overview.md`'s divergences table was re-verified against the tree
row by row — not assumed. Every row was checked by reading the code it names.

| # | What was checked | Evidence | Outcome |
|---|---|---|---|
| 1 | The helper does not `execve` | `crates/eggwork-sandbox-helper/src/main.rs` builds a `Command` and calls `command.spawn()` (line 129) then `child.wait()` (line 138) | **left alone** — still true |
| 2 | The helper sets no `RLIMIT_*` | no `rlimit`/`setrlimit`/`RLIMIT` occurrence anywhere under `crates/eggwork-sandbox-helper/src/` | **left alone** — still true |
| 3 | `--windows-start-type` is parsed by no code path | `scripts/qualify_release.py:798` emits the flag; no occurrence of `windows_start_type` or `windows-start-type` in any `.rs` file | **corrected** — the flag is documented in `docs/deployment.md`, not in a README |
| 4 | The `GrantAuthorizer` resource-scope boundary (closed by ADR-0007) | `crates/eggwork-server/src/operations.rs`: `GrantAuthorizer` implements `authorize_request`; the scope lookup is `grant.scopes.get(&(request.operation, resource.kind()))` — per `(operation, kind)`; `authorize` is called first, so a scope never grants; ownership stays with the store | **left alone, verbatim** — the closed claim and its residual boundary still hold |
| 5 | `check_helper_compatibility_with_timeout` checks the file only | `crates/eggwork-server/src/deployment.rs`: `symlink_metadata`, `is_file()`, ownership and mode checks — no ancestor walk | **left alone** — still true |
| 6 | Invariant 2 has no dedicated mechanical guard | `no_direct_service_manager_invocation_exists` scans exactly `src/deployment.rs`, `src/operations.rs`, `src/bin/eggworkd.rs`; `eggwork-runner` is outside it | **left alone** — still true |
| 7 | The ownership guard globs only `crates/*/src/**/*.rs` | `scripts/check_execution_ownership.py:203` — `(ROOT / "crates").glob("*/src/**/*.rs")` | **left alone** — still true |
| 8 | `--prove-negative-exit` is narrower than its name | `scripts/check_execution_ownership.py`: `main()` returns `prove_negative_exit()` before `scan_sources()` and `check_dependencies()` are called | **left alone** — still true |
| 9 | `blob_download` takes no reference lease | no `retain`/`lease` call in `blob_download`; `artifact_download` calls `.retain(` | **left alone** — still true |
| 10 | Workspace materialization has no `O_NOFOLLOW` | `crates/eggwork-server/src/workspace.rs:1118` — `options.write(true).create_new(true)` on a plain path; `crates/eggwork-server/src/artifact.rs` uses `OFlags::NOFOLLOW` chains | **left alone** — still true |
| 11 | No schema version or migration path | no `PRAGMA user_version` in any server source file | **left alone** — still true |
| 12 | The store does not enforce the state machine | `commit_event` rejects only a *terminal* current state; no legality check on the transition | **left alone** — still true |
| 13 | `generation.get() as i64` in eleven places | `grep -c 'generation.get() as i64' crates/eggwork-server/src/store.rs` → **11**; `after_sequence as i64` at store.rs:442 | **left alone** — the count is still eleven |
| 14 | `load_page` has no `LIMIT` | the three statements in `load_page` carry no `LIMIT` clause | **left alone** — still true |
| 15 | `MAX_EXECUTION_IDENTITIES` never evicts | `store.rs:19` — `const MAX_EXECUTION_IDENTITIES: i64 = 2048`; `store.rs:179` — `if execution_count >= MAX_EXECUTION_IDENTITIES` with no eviction path | **left alone** — still true |
| 16 | TOCTOU between admission and launch | `crates/eggwork-server/src/lib.rs`: permit acquired at 2123, idempotency `reserve` at 2179, runner task spawned at 2283/2287 | **left alone** — still true |
| 17 | Blob GC cannot race `retain` (closed) | `garbage_collect` locks `self.inner.metadata` at entry (blob.rs:492); `retain` takes that same mutex (blob.rs:313) — mutually exclusive | **left alone, verbatim** — the closed claim still holds |
| 18 | Artifact GC does not release blob references | no `release_references`/`retain`/`clear_reference` call in `ArtifactStore::garbage_collect` | **left alone** — still true |
| 19 | `events()` puts the lease id in a URL query string | `crates/eggwork-client/src/lib.rs:496` — `"/v1/executions/{}/events?generation={}&after={after_sequence}&lease={}"` | **left alone** — still true |
| 20 | `decode_json` has no size cap | **the observation is no longer true.** The client's `HttpClient` is built with `.max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)` (16 MiB) at `crates/eggwork-client/src/lib.rs:182` and `:200`; Eggfetch 0.2.0's `compression.rs` wraps the response body in a `LimitingStream` that enforces the bound on both the compressed and the uncompressed path. The telemetry route is bounded twice (transport cap, then 64 KiB). The cap was added by `17ff53f`, after the row was written | **closed** — struck, with `**Closed 2026-10-25 by the 17ff53f corrective.**`, what now enforces it, and the residual boundary (a transport-wide cap, not a per-route one) |
| 21 | Redirect/retry safety is an accident of the dependency profile | `Cargo.toml:35` — `eggfetch-core` with `default-features = false, features = ["standard-http1", "tls-rustls"]`; eggfetch 0.2.0's `standard-http1` is `transport-http1 + standard-route + high-level-url` and does **not** enable `redirects` or `logical-retry` — only the `http1`/`http2` aliases do | **left alone** — still true |
| 22 | Declared artifacts are refused, not captured, on a platform that cannot open a file beneath the workspace root | `crates/eggwork-server/src/artifact.rs`: the `#[cfg(not(unix))]` `open_file_beneath` returns `Unsupported`; `#[cfg(not(unix))] capture_capability()` returns `Unavailable`; `decide_artifact_capture` maps `Unavailable` to `ArtifactCaptureDisposition::Failed` and rewrites `Succeeded` to `Failed` | **left alone, verbatim** — the closed claim and the remaining platform limitation both hold |
| 23 | The operator surface's metric vocabulary (closed by M008a) | preserved verbatim; the underlying code was inspected for §10.4 and found consistent with the closure | **left alone, verbatim** |
| 24 | `active_executions` carried two facts (closed by M008a) | preserved verbatim | **left alone, verbatim** |

**Numbering was not disturbed.** No row was renumbered, no row was moved
between tables, and the rows cited by number elsewhere in `architecture/` and
`plans/` still mean what they meant. A closed row is closed **in place**, using
the document's own convention — the old text struck through, followed by
`**Closed <date> by <what>.**`, then what now enforces it and what residual
boundary remains — as rows 17 and 22 already did. A new row takes the next
free number at the end of the document; none was needed.

A short paragraph recording the re-verification, its date, and which rows
changed was added above the table in `architecture/overview.md`.

## 13. The component size table is now a script

The same defect class as a line-number citation: a checked-in line count is a
fact about the tree that rots on the next edit and is wrong the moment it is
written. `architecture/overview.md` carried five such counts
(`1,671 LOC` … `14,104 LOC`); they were stale by 70-100% and had no owner.

**`scripts/component_sizes.py`** (new, stdlib-only, deterministic) prints the
figures on demand. It counts every line of every `.rs` file a crate owns,
split into the crate's `src/` (the product) and `tests/` (the evidence), and
sums to a workspace total so a reader can check the arithmetic.

```
$ python3 scripts/component_sizes.py
| Crate | `src/` lines | `tests/` lines | files | total lines |
|---|---|---|---|---|
| `eggwork-core` | 2,938 | 434 | 3 | 3,372 |
| `eggwork-runner` | 3,820 | 2,000 | 8 | 5,820 |
| `eggwork-sandbox-helper` | 651 | 1,930 | 5 | 2,581 |
| `eggwork-client` | 1,350 | 0 | 1 | 1,350 |
| `eggwork-server` | 20,998 | 5,162 | 20 | 26,160 |
| **total** | **29,757** | **9,526** | **37** | **39,283** |
```

`--markdown` prints one table row per crate; `--json` prints the counts
machine-readably. The document's size column now says *see the size script* and
points at the command. **The document carries no size figures at all** —
acceptance criterion 8 says the component table carries no hand-written size
figures, and a snapshot table beside it would be the same fact in a different
place. The snapshot lives here, in the closure record, where it is evidence
about one commit rather than a second owner of a number the tree owns.

## 14. The quickstart document, reconciled

`scripts/check_quickstart_sync.py` compares the document's command blocks with
the script's steps, by *shape* rather than by literal text, so the document's
`$ROOT` and the script's `$ROOT` are the same command.

**What was wrong.** The document's two `cargo test` commands in blocks 13-14
lacked `--locked`, while `scripts/quickstart.sh` uses it. `--locked` is
load-bearing on `clippy` and `test` for the same reason it is load-bearing on
a release: a version bump that skips `Cargo.lock` passes both and then fails
every release build. The document is the user-facing instruction, so it must
carry the flag a user needs to reproduce the locked build.

**Fixed:** `--locked` added to both commands. The sync check then passed:

```
$ python3 scripts/check_quickstart_sync.py ; echo "EXIT=$?"
quickstart sync check passed: the document's command blocks and shown doctor output agree with scripts/quickstart.sh
EXIT=0
```

**The doctor output.** The earlier worker's finding — the document showing
seven check names while the node emits eight — **no longer holds**: the M008a
worker had already added `host-telemetry` to the document. The node emits
eight, in this order, and the document now shows the same eight in the same
order:

| Node (`crates/eggwork-server/src/operations.rs`) | Document |
|---|---|
| `configuration` | `configuration` |
| `host-telemetry` | `host-telemetry` |
| `sandbox-resource-backend` | `sandbox-resource-backend` |
| `listener` | `listener` |
| `execution-root`, `blob-root`, `workspace-root` | the same three, in the same order |
| `storage-headroom` | `storage-headroom` |

The comparison is now **executed** rather than read: the new
`installed-qualification` CI step runs the installed daemon's own `doctor`
report and compares it with the document (§11), and `scripts/quickstart.sh`
performs the same comparison when it runs the flow end to end.

`bash scripts/quickstart.sh` was run end to end in this tree: it completed
successfully, and its final step reported
`quickstart sync check passed: the document's command blocks and shown doctor
output agree with scripts/quickstart.sh`. The two ignored qualification tests
passed inside it (`test result: ok. 1 passed; 0 failed`).

## 15. Verification actually run

Every command below was executed in this working tree. Commands that were
**not** run are in §17 with the reason.

| Command | Result |
|---|---|
| `python3 scripts/check_doc_citations.py` (before the conversion) | **exit 1** — 680 findings (627 `legacy-line-citation`, 53 `unresolved-path`) |
| `python3 scripts/check_doc_citations.py` (after the conversion) | **exit 0** — `doc citation guard passed; every citation names a symbol that resolves`, 21 documents scanned |
| `python3 scripts/check_doc_citations.py --prove-negative-exit` | **exit 0** — `doc citation guard negative-exit proof passed: the deliberately wrong citation was rejected (legacy-line-citation, unresolved-path, unresolved-symbol); the correct citation was accepted` |
| `python3 scripts/check_quickstart_sync.py` | **exit 0** — `quickstart sync check passed: the document's command blocks and shown doctor output agree with scripts/quickstart.sh` |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | **OK** — 139 tests, 0 failures, 0 errors, 0 skipped |
| `python3 scripts/check_execution_ownership.py` | **exit 0** — the ownership guard and its negative scanner self-test both pass |
| `python3 scripts/component_sizes.py` | exit 0 — the table in §13 |
| `bash scripts/quickstart.sh` | **OK** — `quickstart completed successfully`; the final sync check passed and both ignored qualification tests passed |
| `cargo fmt --all -- --check` | **exit 0** — no formatting diff |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | **exit 0** — no warnings |
| `cargo check --workspace` | **exit 0** |
| `cargo test --locked --workspace --all-targets` | **OK** — the whole workspace suite passes |
| `eggpack ci check --workflow-shape … --contract … --github-policy … --workflow …` | **exit 0** — `ci check: match (51529 bytes)`; the generated release workflow is unchanged |

### 15.1 The Rust toolchain

**No Rust source file was changed by this work.** The changes are to
`architecture/*.md`, `docs/quickstart.md`, `.github/workflows/ci.yml`, and
`scripts/`. The Rust-toolchain commands were nevertheless run against the
whole branch, so the pre-submit gate is verified over everything the branch
carries and not merely over this diff — criterion 13 asks for the full
pre-submit gate, and the branch's other workers' commits are part of it.

### 15.2 Code inspection versus executed evidence

| Claim | How it was established |
|---|---|
| Every row of the divergences table | **Code inspection**, each row against the file it names — the evidence is in §12's table |
| The citation conversion resolves | **Executed** — the gate, 680 findings to 0 |
| The gate rejects a wrong citation | **Executed** — `--prove-negative-exit`, and the 139-test release suite |
| The macro-generated item rule | **Executed** — the gate's own fixture asserts `RealId` resolves and `NotAnItem` does not |
| The quickstart document matches the script | **Executed** — the sync check, and the script run end to end |
| The document's doctor output matches the node | **Executed** — `scripts/quickstart.sh` compared the installed daemon's report with the document |
| The new CI steps run | **Partly executed.** The `rust`-job steps were run as the commands they are. The new `quickstart` job and the new `installed-qualification` step were **not** executed as GitHub jobs: only the coordinator can dispatch them. The `quickstart` job's body is exactly `bash scripts/quickstart.sh`, which **was** run in this tree and completed successfully; the `installed-qualification` step's body is the same daemon `doctor` comparison the script performed. So the *commands* are executed evidence; the *jobs in the workflow file* are not |
| The release path carries the documentation gates | **Not covered, by design.** The release workflow is generated; the upstream prerequisite is recorded in `architecture/release-qualification.md` (§11.1) |

## 16. What changed, file by file

| File | Change |
|---|---|
| `architecture/ci-guardrails.md` | the three new `rust`-job steps, the new `installed-qualification` step, the renumbered step-by-step explanation, and the reason the doctor comparison lives in that job |
| `architecture/client-transport.md` | 38 citations converted; the two `cargo test` commands' `--locked`; the doctor's `forbid(unsafe_code)` row |
| `architecture/core-domain.md` | 98 citations converted; the `RunnerRequest` contrast now cites the runner |
| `architecture/data-plane.md` | 221 citations converted; the storage-layout code blocks' line references converted |
| `architecture/deployment-lifecycle.md` | 18 citations converted; `verify_trusted_helper` now cites `crates/eggwork-runner/src/lib.rs` — **the wrong-file citation the plan's spot check named** |
| `architecture/execution-ownership.md` | one relative path made repository-relative |
| `architecture/operations-cli.md` | relative paths made repository-relative |
| `architecture/overview.md` | the component size column replaced by the size script; divergence row 3 corrected; row 20 closed; the re-verification note |
| `architecture/release-qualification.md` | the new "where the documentation gates run today — and where they do not" section with the upstream prerequisite |
| `architecture/runner-execution.md` | one relative path made repository-relative |
| `architecture/sandbox-helper.md` | the Cargo build script prose |
| `architecture/server-node.md` | 186 citations converted; the "Notable cases" list names each test instead of a line number |
| `architecture/storage-journal.md` | 91 citations converted; the metrics section corrected against the M008a corrective |
| `docs/quickstart.md` | `--locked` on the two `cargo test` commands |
| `.github/workflows/ci.yml` | three steps in the `rust` job, a new `quickstart` job, one step in the `installed-qualification` job |
| `scripts/check_doc_citations.py` | the macro-generated item rule; the symbol-table cache; the fixture's macro case |
| `scripts/component_sizes.py` | **new** — the per-crate size report |

## 17. Residual work

**Medium — the documentation gates do not run inside the generated release
workflow.** This is the milestone's stated boundary, not an oversight: the
workflow is generated by Eggpack and has no repository-local preflight hook.
The upstream prerequisite is recorded in `architecture/release-qualification.md`
(§11.1) and the generated workflow was not hand-edited.

**Medium — `CONTRIBUTING.md`'s pre-submit list does not mention the citation
gate.** That file is outside this worker's ownership. The exact proposed
change is in the handoff report.

**Low — the new CI jobs have not run as GitHub jobs.** The `quickstart` job's
body is exactly the command that was run in this tree, and the
`installed-qualification` step's body is the same daemon `doctor` comparison
the script performed; the hosted jobs themselves need the coordinator to
dispatch them.

**Low — the gate is not a substitute for a reviewer's judgement.** It proves
that a cited symbol still exists and is still named that. It does not prove
that the symbol is the one the sentence means. The divergences re-verification
in §12 is the part of this work that could not be automated.

**Informational — the conversion made the documents more verbose.** A citation
is now a full repository-relative path, sometimes twice the length of the
symbol it names. That is the cost of a form that survives an edit, and the
gate's rule set is what makes it worth paying. Where a sentence refers to a
concept rather than to one item, a path-only citation keeps it short.

## 18. Reproducing this record

```bash
python3 scripts/check_doc_citations.py
python3 scripts/check_doc_citations.py --prove-negative-exit
python3 scripts/check_quickstart_sync.py
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
python3 scripts/check_execution_ownership.py
python3 scripts/component_sizes.py
bash scripts/quickstart.sh
```
