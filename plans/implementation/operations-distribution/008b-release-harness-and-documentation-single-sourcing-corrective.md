# Operations M008b — Release harness and documentation single-sourcing corrective

Status: proposed

Closure record: `plans/closure/operations-distribution/008b-status.md`

Class: infrastructure (release harness and CI) / polish (documentation drift and citation hygiene)

Source authority:

- `plans/subsystems/operations-distribution-operator-truthfulness-corrective-addendum.md` — findings OP-4, OP-5, OP-6
- `plans/subsystems/operations-distribution-roadmap.md` — the release and qualification milestones and their durable invariants
- `plans/closure/operations-distribution/003-status.md` — Eggpack producer packaging integration
- `plans/closure/operations-distribution/004-status.md`, `plans/closure/operations-distribution/004a-status.md`, `plans/closure/operations-distribution/004b-status.md` — operational and release qualification
- `architecture/distribution.md` — the producer/consumer/release-channel authority split
- `architecture/release-qualification.md` — the qualification harness and its failure semantics
- `architecture/ci-guardrails.md` — the CI jobs, the ownership guard, and the `--locked` discipline
- `architecture/overview.md` — the component table and the divergences table
- `plans/003-planning-process.md` — status vocabulary, milestone sizing, dependency discipline, required plan sections, closure evidence, corrective work, architecture changes, external dependencies

## 1. Objective

Make the release harness, the CI configuration, and the design record each state a fact once, derive it from its owner, and say so where a reader would look.

Three defects are in scope:

1. the release harness re-derives — or simply remembers — facts that already have one owner: the workspace version (three readers), the expected version (two conflicting authorities), the asset inventory (a hardcoded count beside a derived set), the install identities (four declarations), and the release-target classification (an explicit list beside a suffix heuristic);
2. the design record's line-number citations have drifted from the tree, the component size table is wrong in every row, and one divergences entry describes an already-fixed defect as an open question — with nothing in CI that would notice;
3. CI type-checks code it never executes, and one test reports success while asserting nothing.

After M008b, the harness holds no independently remembered constant for any fact it asserts; the design record cites symbols rather than line numbers and a mechanical check keeps it that way; the quickstart is machine-verified or its waiver is recorded where a reader would look; and every test that CI reports as green has asserted something.

**Classification note.** The harness and CI portion is internal machinery required by the release capability and is classified **infrastructure**. The documentation-drift portion is genuinely **polish** — a stale size table and a stale `doctor` check count are documentation defects, not behavioural ones — and must not be represented as capability. The one part of OP-5 that is not polish is the *absence of a gate*: a document that asserts "executed and verified" while nothing verifies it is a truthfulness defect of the same family as OP-1 and OP-3, and it is included here.

## 2. Baseline / problem statement

At the reviewed baseline (`81f34e0`):

**OP-4 — the release harness re-derives facts that already have one owner.**

- *Workspace version, three readers.* `scripts/check_release_tag.py` → `fn workspace_version` uses a regular expression over the whole `Cargo.toml` and takes the **first** `version = ` line in the file, not the one under `[workspace.package]`; it works only by accident of the current manifest layout, in which `[workspace.package]` happens to precede every other `version = ` line. `scripts/release_candidate_probe.py` → `fn workspace_version` reads `workspace.package.version` with `tomllib`. `crates/eggwork-server/tests/release_contract.rs` → `fn the_release_version_is_one_coherent_workspace_identity` reads the same field with the `toml` crate.
- *The tag guard is never executed by an automated job.* `scripts/check_release_tag.py` is referenced by nothing in the repository except its own docstring. It is not invoked by `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/workflows/operational-qualification.yml`, or `.github/workflows/windows-reproducibility.yml`, and it has no test in `tests/release/`. **The script's own docstring explains why the tag-dependent half cannot live in CI** — "the tag-to-version link cannot live in CI: the generated release workflow is producer authority (the `release-drift` job rejects hand edits to it), and CI never knows the future tag. So the check lives here, run by whoever cuts the tag, before the tag exists" — and that reasoning is sound: CI cannot assert a tag it does not know. The defect is therefore narrower than "it is unwired", and it is three things: the script reads the manifest with a **different parser** from the shared reader; `fn lock_versions` parses `Cargo.lock` with regular expressions as well; and a guard whose correctness decides whether a release may be tagged is exercised by **no automated job at all**, not even against a fixture, so its correctness is an untested assumption.
- *Two version authorities.* A manifest-derived authority (`scripts/validate-daemon-version.py`, `scripts/validate-sandbox-helper.py`, `scripts/release_candidate_probe.py` — all derive the expected version from the workspace manifest) and a tag-derived authority (`scripts/qualify_release.py` → `fn stage_install` derives `expected_version` from `args.release_tag` and asserts the installed daemon and helper report it). Nothing states which is the release gate and which is the build assertion.
- *Asset inventory, twice in one function.* `scripts/qualify_release.py` → `fn stage_inventory` derives the expected asset set dynamically from the release manifest and, in the same function, asserts the asset count against the hardcoded `QUALIFIED_ASSET_COUNT`.
- *Install identities, four times.* `crates/eggwork-server/src/deployment.rs` declares them as constants (`SERVICE_ID`, `MEMBER_DAEMON`, `MEMBER_HELPER`); `release/eggpack/distribution.toml` declares the same install identities in its `install` fields and describes itself as "the sole checked-in authority for the published target set, release asset names, install identities, and checksum sidecar names"; `scripts/qualify_release.py` declares `SERVICE_ID` and `HELPER_INSTALL_ID` as its own constants; `crates/eggwork-server/tests/release_contract.rs` pins the Rust side to string literals. The Rust side is cross-checked against the contract by the Rust contract test; the Python side is pinned by nothing.
- *Target classification, twice.* `crates/eggwork-server/tests/release_contract.rs` holds explicit lists of target triples; `scripts/qualify_release.py` → `fn target_daemon_install` and `fn target_ships_helper` classify a target by testing whether the string ends with `-pc-windows-msvc` or `-unknown-linux-gnu`.

**OP-5 — the design record and the operator documentation have drifted, and there is no gate.**

- *Line-number citations are stale.* `architecture/*.md` carries 718 line-number citations of the form `file.rs:NNN`. Re-measuring against the tree with the nearest backticked identifier as the anchor, 85% of the resolvable citations no longer resolve at the exact cited line, 83% are stale within a ±3-line tolerance, and 72% remain stale even allowing ±20 lines of drift. (The interrogation pass reported roughly 92%; the exact figure depends on the tolerance chosen and the conclusion does not depend on it.) The per-file distribution is concentrated: `data-plane.md` 206, `server-node.md` 198, `core-domain.md` 111, `storage-journal.md` 100, `sandbox-helper.md` 43, `client-transport.md` 39, `deployment-lifecycle.md` 18, `runner-execution.md` 3. Spot-verified examples: `architecture/deployment-lifecycle.md` cites `verify_trusted_helper` as living in `crates/eggwork-server/src/lib.rs`, while the function is defined in `crates/eggwork-runner/src/lib.rs` — the wrong file entirely; `architecture/runner-execution.md` cites `LANDLOCK_WORKSPACE_RW_CAPABILITY` and `probe_landlock_ruleset` at line numbers that sit roughly 140 lines away from the definitions they name in `crates/eggwork-runner/src/lib.rs`. The citation *form* is the defect: a line number is a property of the text, not of the symbol, and it is invalidated by every unrelated edit above it. Citations frequently use a bare basename (`lib.rs`, `artifact.rs`), which is additionally ambiguous across the five crates.
- *The component size table is wrong in every row.* Counting non-test Rust sources at the reviewed baseline: core 2,294 lines against a stated 1,671; runner 2,900 against 2,336; client 1,158 against 880; server 16,980 against 14,104; sandbox-helper 446 against 401.
- *Divergence #20 is stale in the dangerous direction.* The `architecture/overview.md` divergences table states that the client's `decode_json` calls `response.bytes()` with "no size cap", leaving the JSON paths open to memory exhaustion from a hostile node. That is no longer true: `crates/eggwork-client/src/lib.rs` applies `max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)` at the transport layer and a stricter `MAX_TELEMETRY_BODY_BYTES` cap on the telemetry route, and the client's own module comment records the change. The table is read as the list of *open* questions, so a reader would go chasing an issue that is already fixed. This is the most dangerous form of documentation drift.
- *A second drift of the same kind exists between the design record, the operator documentation, and the code.* `architecture/operations-cli.md` describes `doctor` as "Seven checks, in order" and omits `host-telemetry`; `crates/eggwork-server/src/operations.rs` → `fn doctor` emits eight checks including `host-telemetry`; `docs/operations.md` documents eight. `docs/quickstart.md` shows a `doctor` output block with seven checks — so the quickstart's claim that "the output shown is real" is already false for that block.
- *The quickstart is not machine-verified.* No CI job, test, or script executes or even references `docs/quickstart.md`; the only mention outside `docs/` is a source comment. `AGENTS.md`'s assertion that quickstart commands "are executed and verified, not illustrative" is true of their authors' intent and unbacked by any gate.
- *The mechanism.* The repository's prose volume is roughly 85% of its code volume (`architecture/` about 8,500 lines against `crates/` about 28,400; `plans/` about 15,300; `docs/` about 870; `scripts/` about 2,550), and documentation files are touched about 2.7 times as often as Rust files in recent history. Prose moves faster than code and nothing reconciles the two.

**OP-6 — CI does not execute the code it type-checks.**

- *The feature-gated qualification test never runs in CI.* `.github/workflows/ci.yml` runs `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` with `--all-features`, but runs `cargo test --locked --workspace --all-targets` **without** it. `crates/eggwork-server/tests/installed_qualification.rs` is gated by `#![cfg(feature = "qualification")]`, so clippy type-checks it and the CI test step never executes it. It is additionally marked `#[ignore]` on its two tests, and it runs in `.github/workflows/operational-qualification.yml` — which is `workflow_dispatch` only, so on an ordinary push or pull request the suite never executes anywhere.
- *Adding `--all-features` to the test step would not close the gap.* The two `#[ignore]` attributes keep the tests from running regardless of which features are enabled. The gap is closed by **executing** the ignored tests in a job that has the environment they need, not by enabling a feature flag.
- *One test is green while proving nothing.* `crates/eggwork-runner/tests/capability_probe.rs` → `fn built_helper` locates the sandbox helper binary through the runtime environment variables `CARGO_TARGET_DIR` and `OUT_DIR`; when it cannot, the test returns early and **passes with zero assertions**. The file's own comment claims "CI builds it before running this suite, so this skip is a local-only convenience", but `CARGO_TARGET_DIR` is not set by the repository's `.cargo/config.toml` and is not exported to integration test binaries, and `OUT_DIR` is a compile-time variable for build scripts, not a runtime environment variable. The skip is therefore the *normal* path in CI as well as locally. The repository already contains the two deterministic patterns this test should use: `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` → `fn built_helper` resolves the helper through the compile-time `CARGO_BIN_EXE_` variable, and `crates/eggwork-server/src/lib.rs` → `fn locate_helper_binary` walks the current executable's sibling and grandparent before consulting `CARGO_TARGET_DIR`.

## 3. Scope

In scope:

- the version readers in `scripts/check_release_tag.py`, `scripts/release_candidate_probe.py`, `scripts/validate-daemon-version.py`, and `scripts/validate-sandbox-helper.py`, and the version reader in `crates/eggwork-server/tests/release_contract.rs`;
- the expected-version assertion in `scripts/qualify_release.py` → `fn stage_install`;
- the asset inventory and its count in `scripts/qualify_release.py` → `fn stage_inventory`;
- the install-identity and service-identity constants in `scripts/qualify_release.py`, and the target-classification helpers `fn target_daemon_install` and `fn target_ships_helper`;
- the wiring of `scripts/check_release_tag.py` into CI, and the recording of the release-path wiring requirement that cannot be met in this repository alone;
- every line-number citation in `architecture/*.md`, and the design record's component size table and divergences table;
- `docs/quickstart.md`, its machine-verification, and the drift between it and the code;
- the CI test step's feature coverage and the qualification suite's execution;
- `crates/eggwork-runner/tests/capability_probe.rs` → `fn built_helper` and the vacuous skip it enables;
- `architecture/ci-guardrails.md`, `architecture/release-qualification.md`, `architecture/distribution.md`, `architecture/overview.md`, `docs/operations.md`, `docs/quickstart.md`, and `docs/releases.md` as they are affected.

Out of scope:

- `.github/workflows/release.yml`. It is **generated** by `eggpack ci generate` from `release/eggpack/`, and `AGENTS.md` invariant 7 says it must never be hand-edited; `eggpack ci check` fails on drift. Any change to the release workflow's steps must go through the producer configuration, and the plan treats that as an upstream dependency, not as an in-repo edit.
- `release/eggpack/` itself, which Eggpack owns. This plan *reads* the producer configuration; it does not change it. If the only route to a required fact runs through an Eggpack change, that is an upstream prerequisite and must be recorded as such.
- The operator-surface findings OP-1, OP-2, OP-3, which belong to M008a. The one coupling is that the quickstart's troubleshooting table quotes the configuration error text, which M008a changes; the two plans must not both edit that text.
- The service-host lifecycle and the shutdown-and-wait convergence contract, which belong to M007.
- The blob-inspection verb's re-derivation of the blob store's shard path layout, which M008a records as a further instance of the same defect class.
- Any change to a released artifact's bytes, name, target set, install identity, or version. This plan changes *how the harness knows* those facts, never the facts themselves.
- Any change to the wire protocol.

## 4. Invariants and non-goals

Invariants this plan must not weaken:

- The scheduler-free, fixed-target boundary. The release channel ships a node; it does not schedule.
- The release-identity invariant: no release identity in `release/eggpack/` — no tag, source SHA, artifact digest, or size. Deriving facts *from* the producer configuration must not push identity into it.
- `.github/workflows/release.yml` is generated and is never hand-edited; `eggpack ci check` must still pass.
- `--locked` remains load-bearing on `clippy` and `test` in CI. Any CI change this plan makes must keep `--locked` on both.
- The ownership guard's scope and the `--prove-negative-exit` self-test are unchanged.
- No authentication, lease, generation, fencing, or digest bound is weakened. No released artifact format changes and no wire protocol changes.
- The isolation claims remain earned: nothing in the harness may assert a platform or capability that was not probed.

Non-goals:

- Making the harness a general-purpose release tool. It qualifies *this* product against *this* producer contract.
- Rewriting the release qualification workflow's structure or its receipt format.
- Translating the whole design record into a different documentation system. The correction is: cite symbols, and check the citations.
- Editing the divergences table's *substance* beyond re-verifying it. If re-verification reveals a genuine open divergence that is not recorded, that is a finding for the owner of `architecture/overview.md`; this plan records it and does not silently rewrite it.
- Making prose generation automatic. The correction is a *check*, not an author.

## 5. Required production changes

### 5.1 OP-4 — one owner per fact, and the harness's copies removed

The rule this plan establishes: **multiple readers of one owner are fine; multiple owners are not.** A second implementation that *reads* the same owner and is itself checked against it is a useful cross-language check. A second implementation that *remembers* the fact is a defect.

- **Workspace version.** One Python reader, used by every script. The repository already has the shape of the right answer: `scripts/release_candidate_probe.py` → `fn workspace_version` reads `workspace.package.version` with `tomllib`, and `scripts/validate-daemon-version.py` and `scripts/validate-sandbox-helper.py` already import `fn expected_version_line` from it rather than keeping their own copies. What is missing is that `scripts/check_release_tag.py` and `scripts/qualify_release.py` are not on that reader. `scripts/check_release_tag.py` → `fn workspace_version` must stop using a regular expression and must read `workspace.package.version` with a real TOML parser; the reader must be shared, not duplicated, and the plan requires that every release script obtains the version through it. The reader must fail closed on a malformed manifest or a missing `[workspace.package].version`, and must not fall back to a regex.
  - The Rust reader in `crates/eggwork-server/tests/release_contract.rs` stays: it is an independent cross-language check of the *same owner*, not a second owner. The Rust test and the Python reader must both be checked against the manifest.
- **The two version authorities, reconciled explicitly.**
  - **Owner of the version the binaries report:** `Cargo.toml` `[workspace.package].version`, inherited by every crate through `version.workspace = true` and surfaced by the daemon as `env!("CARGO_PKG_VERSION")` in `eggworkd version`. This is the *build assertion*: the binary must report the manifest version, always.
  - **Release gate:** the release tag must equal `v` + the manifest version. That is `scripts/check_release_tag.py`'s job, and it is the only place a tag and a version are compared.
  - **Why they are permitted to differ:** they are permitted to differ in exactly one respect — the tag may carry a `v` prefix and the manifest version may not. Beyond that prefix, the two must be equal, and the gate is what proves it. A binary built from a working tree reports the manifest version but is not a release candidate; a release candidate is a binary built from a tag that the gate has accepted. The qualification harness's job is therefore to assert the *binary* against the *manifest-derived* version, and to record the tag it was qualified under — not to re-derive the expected version from the tag. `scripts/qualify_release.py` → `fn stage_install` must stop deriving `expected_version` from `args.release_tag` and must instead use the shared manifest-derived version, while still asserting that the tag it was invoked with matches that version (or delegating that assertion to `check_release_tag.py`, which the release job must then run).
  - The plan requires this reconciliation to be written down in `architecture/release-qualification.md`, in a form that a reader of the harness can consult to learn which number to trust.
- **Asset inventory.** `QUALIFIED_ASSET_COUNT` is **deleted**, not re-synced. `scripts/qualify_release.py` → `fn stage_inventory` already derives the expected asset set dynamically from the release manifest; the assertion must compare the staged set against the derived set. If a count is wanted for the receipt, it is computed from the derived list, never remembered.
- **Install identities and the service identity.** The harness must contain **no literal** install or service identity.
  - Install identities (`eggworkd`, `eggwork-sandbox-helper`) are read from `release/eggpack/distribution.toml`'s `install` fields. `scripts/qualify_release.py`'s `SERVICE_ID` and `HELPER_INSTALL_ID` constants are deleted and their uses derive from the contract. The executable-name suffix convention (`.exe` on Windows) is applied in exactly one place and is not an identity.
  - The service identity (`eggwork-node` — the systemd unit name and the launchd label) is owned by `crates/eggwork-server/src/deployment.rs` → `SERVICE_ID`, and the harness must obtain it from the daemon rather than remember it: `eggworkd service spec` already reports `service_id` (it requires `--executable` and `--service-config`, both of which the harness has when it stages an installation), and `eggworkd deployment status` reports it too. The harness reads it from the candidate daemon.
  - The Rust contract test keeps pinning the Rust constants to the contract's values — that is the check that keeps the two languages honest — and the Python harness must have an equivalent check against the same owner.
- **Target classification.** `fn target_daemon_install` and `fn target_ships_helper` must stop classifying by string suffix and derive from the contract's `[[targets]]` entries: which assets a target ships, and therefore whether it ships the helper, is stated by the contract's asset kind and entries. The Rust test's explicit triple lists stay as the independent check against the same owner.

### 5.2 OP-4 — the tag guard uses the shared reader and is exercised

- **The reader.** `scripts/check_release_tag.py` → `fn workspace_version` must stop using a regular expression and must obtain the version through the shared reader (§5.1). `fn lock_versions` must likewise stop parsing `Cargo.lock` with regular expressions and must use a real TOML reader for the lock file. A guard that decides whether a release may be tagged must not depend on the layout of the files it reads.
- **The tests.** `scripts/check_release_tag.py` must gain tests in `tests/release/`, which it does not currently have: the tag↔version rule, the `v`-prefix case, the lock-version rule, the malformed-manifest case, the missing-`[workspace.package].version` case, and a manifest whose `[workspace.package]` section is not first.
- **The CI wiring — what CI *can* assert.** The tag-dependent half of the guard cannot run in CI, and the plan does not pretend otherwise: CI does not know the future tag, and the script's own docstring says so. What CI must do is:
  - **exercise the guard's logic** against fixtures, so that the code path that decides whether a release may be tagged is executed by an automated job rather than trusted;
  - **assert the version-coherence half on every push and pull request** — that the workspace manifest's version and the lock's pinned versions agree with each other, which needs no tag and is the condition that most often breaks (a version bump without a lock regeneration is exactly what failed the `v0.1.3` release attempt); and
  - **state in the workflow itself** that the tag-dependent assertion lives in `scripts/check_release_tag.py` and is run by whoever cuts the tag, so a reader of `.github/workflows/ci.yml` can see where the gate is.
- **The wiring belongs in `.github/workflows/ci.yml`**, which is hand-maintained and is **not** checked by `eggpack ci check` (that check covers `.github/workflows/release.yml` only).
- **The release-path wiring cannot be done in this repository alone.** `.github/workflows/release.yml` is generated by `eggpack ci generate` from `release/eggpack/`, and hand-editing it is forbidden; `eggpack ci check` fails on drift. The plan therefore requires:
  - the upstream prerequisite to be recorded explicitly, naming what Eggpack must support (a repository-local preflight step that runs the tag guard against the tag being released, or an equivalent hook) and where it would be configured;
  - the in-repo CI wiring to land now, so the guard's logic is exercised even before the upstream hook exists;
  - the *process* requirement to be documented — the script's own docstring already states that whoever cuts the tag runs it before the tag exists, and `docs/releases.md` or `architecture/release-qualification.md` must say so plainly rather than leaving it in a script docstring; and
  - a statement, in `architecture/release-qualification.md` and in the closure record, of exactly where the gate runs today and where it does not, so that nobody reads "the tag guard is in the repository" as "the tag guard ran".

### 5.3 OP-5 — citations become symbols, and a check keeps them that way

- **Canonical citation form.** Every code reference in `architecture/*.md` and `docs/*.md` names a **repository-relative path** and a **symbol**, never a line number and never a bare basename. A path is required because `lib.rs` alone is ambiguous across five crates.
- **Conversion.** All 718 line-number citations in `architecture/*.md` are converted, in this order (largest first): `data-plane.md`, `server-node.md`, `core-domain.md`, `storage-journal.md`, `sandbox-helper.md`, `client-transport.md`, `deployment-lifecycle.md`, `runner-execution.md`. `docs/*.md` currently carries no line-number citations; the conversion must leave it that way.
- **The durable guard.** A script — the plan names `scripts/check_doc_citations.py` — verifies every code citation in `architecture/*.md` and `docs/*.md`:
  - a citation in the legacy `file.rs:NNN` form is a failure, not a warning;
  - a citation whose named path does not exist is a failure;
  - a citation whose named symbol does not exist in the named file is a failure. The check must recognise Rust items (`fn`, `pub fn`, `const`, `static`, `struct`, `enum`, `trait`, `impl`, `mod`, `type`, `macro_rules!`) and Python items (`def`, `class`, and module-level assignments), and must resolve a bare basename only when it is unique in the tree — otherwise the citation must be rejected;
  - the script must be wired into CI and must fail the build, not warn.
- **Why a symbol check and not a line check.** A line-number check is *strictly weaker* than it looks: it verifies that a line exists, not that it says what the citation claims, and it breaks on every unrelated edit above the target. A symbol check verifies that the named thing is still there and still named that, which is the property a reader actually relies on. It is not complete — a symbol can exist in the file without being the one the sentence means — so the plan requires that where a symbol name is ambiguous within its file, the citation names the qualified path (for example the module path as well as the item name).
- **The script must be proven to fail.** The plan requires a self-test of the checker — a fixture containing a deliberately wrong citation that the checker must reject — analogous to the ownership guard's `--prove-negative-exit`. A checker that has never been shown to fail is not a gate.
- **The component size table.** The plan requires the size column of the `architecture/overview.md` component table to be **removed and replaced by something that cannot silently go stale**: a checked-in script that prints the current per-crate figures on demand, with the table pointing at that script instead of carrying numbers. Rationale: the figures were never load-bearing — they are an at-a-glance claim about the shape of the tree — and they were wrong in every row, which is worse than absent. The alternative (regenerating the numbers into the table) is rejected because it would put a generated table inside a hand-written document and re-create the same drift loop with a new trigger.
- **The divergences table.** Every entry is re-verified against the tree, entry by entry, and each entry is either corrected, closed, or confirmed. Specifically:
  - **Divergence #20 must be corrected or closed.** It states that the client's `decode_json` reads the body with no size cap; the client now applies a transport-level maximum decoded body size and a stricter telemetry cap. The entry must be rewritten to describe the cap that exists and to state the residual question precisely, or closed with the reason it no longer applies.
  - The entry that describes the `doctor` check count must be reconciled with the code (`fn doctor` emits eight checks including `host-telemetry`), and the affected documents — `architecture/operations-cli.md` and the `doctor` block in `docs/quickstart.md` — must agree with it.
  - The entry about `--windows-start-type` being passed by `scripts/qualify_release.py` to a code path that parses nothing is directly relevant to this plan's harness work and must be re-verified rather than assumed still true.
  - Entries that remain open must stay open, with the open question stated.

### 5.4 OP-5 — the quickstart becomes machine-verified

The options considered:

- **(a) A CI job that executes the quickstart against a locally built node with generated TLS material.** Real cost: the quickstart needs `openssl`, a built daemon and helper, the helper installed at a *trusted* path (outside any world-writable ancestor), a config with correct `0700`/`0600` modes, a running node that must be stopped before the fixture step (the fixture takes an exclusive lock on the state directory), and the `qualification`-feature tests, which are `#[ignore]`d and need their environment variables set. The job also has to keep the document honest about its *shown output*, not only its commands.
- **(b) A test that extracts and runs the quickstart's commands.** Rejected: it parses arbitrary shell out of Markdown, which is brittle (the quickstart uses process substitution and a `pkill`), it inherits the prose's ordering as an implicit dependency, and a failure in such a test is hard to read. It also cannot check the *shown output*, which is where the quickstart's existing falsehood is.
- **(c) A documented waiver.** Rejected: it leaves `AGENTS.md`'s claim that the quickstart's commands "are executed and verified, not illustrative" unbacked — which is the defect this plan exists to remove. A waiver is acceptable only as an interim state with an expiry and a named owner, and it must live where a reader of `AGENTS.md` would look.

**Decision: (a), implemented through a checked-in script that the document embeds.**

- The quickstart's executable steps live in a checked-in script (the plan names `scripts/quickstart.sh`, or an equivalent under `scripts/`), which the CI job runs end to end on Linux: build the daemon and the helper, generate the TLS material with `openssl`, install the helper under a trusted path, write the config, apply the documented modes, run `doctor`, start the node, run `status`, `executions list` and `storage summary`, stop the node, and then run the two `installed_qualification` tests with the `qualification` feature and the documented environment variables.
- `docs/quickstart.md` embeds the script's steps, and a check compares the document's command blocks with the script, so the document cannot drift from what CI runs. The document stays prose-first for a human reader; the script is the machine-checked form.
- The job must also check the document's *shown output*: at minimum, the set of `doctor` check names shown in the quickstart must equal the set the node actually reports. That check alone would have caught the present seven-versus-eight mismatch.
- The job's failure output must name the failing step, so a failing quickstart is diagnosable from CI alone.

### 5.5 OP-6 — CI executes what it type-checks

- **The qualification suite.** The `qualification`-feature tests must **execute** in CI. Adding `--all-features` to the CI test step is explicitly *not* the fix: the two `#[ignore]` attributes keep the tests from running regardless of the feature set, so a feature flag alone leaves the gap open. The plan requires one of:
  - the quickstart CI job (§5.4) runs the two ignored qualification tests with `--ignored` and the documented environment — which it does by construction, since the quickstart's step 7 is exactly those tests; and
  - the CI configuration states, in the workflow itself and in `architecture/ci-guardrails.md`, why the default `cargo test` step does not enable `qualification` and where the suite is executed instead. A reader of the workflow must not have to infer it.
- **The vacuous capability probe.** `crates/eggwork-runner/tests/capability_probe.rs` → `fn built_helper` must stop silently passing when it cannot find the helper. The plan requires the deterministic location the repository already uses elsewhere: `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` → `fn built_helper` resolves the helper through the compile-time `CARGO_BIN_EXE_` variable, and `crates/eggwork-server/src/lib.rs` → `fn locate_helper_binary` walks the current executable's sibling and grandparent. The runner crate cannot use `CARGO_BIN_EXE_eggwork-sandbox-helper` directly, because that variable is only provided to the package that owns the binary; the runner's test must therefore locate the helper deterministically relative to its own test executable, which lives under the target directory's `deps` directory.
  - When the helper cannot be found, the test must **fail loudly** with a message naming the expected path and what to build — not return early and pass. Alternatively the test may be marked `#[ignore]` with a stated reason; the plan prefers the loud failure, because an `#[ignore]`d test is a hole rather than a check.
  - The CI job that runs the runner's test targets must build the helper first (`cargo build --locked -p eggwork-sandbox-helper`), so the precondition holds. The `windows-runner` job currently runs `cargo test --locked -p eggwork-runner --all-targets` alone, which does not build the helper — so the plan requires that job to build it, or that the test's loud failure be explicitly expected there with the reason recorded.
  - The test must be shown to be non-vacuous: the plan requires evidence that it fails when the helper is absent and passes when it is present.

## 6. Failure, cancellation, restart, and contention semantics

- **The harness must fail closed.** Every derivation in §5.1 is a check, not a hint: a manifest that cannot be parsed, a contract that cannot be read, an identity that cannot be resolved, a tag that does not match the manifest — each must stop the qualification, not produce a report that quietly omits the fact. The harness's existing failure semantics (a `QualificationFailure` with a fixed reason, no child-process output interpolated) are preserved.
- **The citation checker must fail closed.** An unreadable document, an unparsable citation, or a citation it cannot resolve must be a failure. It must never treat "cannot check" as "checked".
- **The CI jobs must fail loudly.** The quickstart job's failure must name the failing step; the runner capability probe must fail when its precondition is absent; the qualification suite must not be reported as green when it did not run.
- **Contention.** The qualification fixture takes an exclusive lock on the state directory, and the quickstart itself warns that a running node holds it. The quickstart CI job must use a state root that no other job or workflow uses, and must stop the node before running the fixture. Two CI jobs sharing a state root is a contention failure that must be designed out, not retried.
- **Restart.** The harness has no restart semantics of its own; it qualifies an installed tree and exits. The quickstart job's node start/stop must be bounded and must not leave a process behind, or the job's own step ordering fails on the lock.
- **No change** to the release workflow's own failure semantics, to the qualification receipt format, or to the operational-qualification workflow's structure.

## 7. Compatibility and migration

- **No released artifact format changes.** Asset names, install identities, target set, checksum sidecar names, and the binary version are unchanged. This plan changes only the harness's route to those facts.
- **No wire protocol changes.** Nothing in `eggwork-core`, `eggwork-client`, or the server's HTTP surface changes.
- **The release workflow file is unchanged.** `.github/workflows/release.yml` is generated by `eggpack ci generate`; it is never hand-edited, and `eggpack ci check` must continue to pass. The plan's CI changes are in `.github/workflows/ci.yml`, which is hand-maintained.
- **The producer configuration is unchanged.** `release/eggpack/` is Eggpack's; this plan reads it.
- **The harness's behaviour changes in one observable way:** if a fact in the tree and the harness's remembered copy of it disagree, the harness now follows the tree and the qualification stops with a specific failure, where before it might have passed. This is the intended behaviour change and it is why the harness's remembered copies must be removed rather than reconciled.
- **Operator-facing:** the quickstart's troubleshooting table and its `doctor` output block are corrected to match the node's actual behaviour. The quickstart's error-message quotes must not be edited by both this plan and M008a; the coupling is recorded in both plans and resolved by whichever lands second.
- **Migration:** none for operators. The release process gains a guard that was previously absent.

## 8. Required tests

- **Version reader.** A test in `tests/release/` proving the shared Python reader returns `[workspace.package].version` from a manifest whose `[workspace.package]` section is *not* first, and fails closed on a malformed manifest and on a manifest with no `[workspace.package].version`. This is the test that distinguishes a real TOML reader from the regex the current `check_release_tag.py` uses, and it must be shown to fail against the pre-change reader.
- **Tag guard.** Tests for `scripts/check_release_tag.py`: the tag matches the manifest version with and without the `v` prefix; a mismatched tag fails; the lock's pinned workspace versions are checked against the same value; a malformed manifest fails closed; a manifest with no `[workspace.package].version` fails closed; and a manifest whose `[workspace.package]` section is not first still yields the right version — the case the regex reader gets wrong.
- **CI exercise of the guard.** Evidence that an automated job executes the tag guard's logic against fixtures, and that the version-coherence half — the manifest against the lock — is asserted on every push and pull request. The point of the finding was that the guard was executed by no job at all, so "the script exists" is not evidence.
- **Version authority reconciliation.** A test proving the qualification harness's expected version equals the manifest version, and that a tag which disagrees with the manifest is refused by the gate rather than being used to derive the expected version.
- **Asset inventory.** A test proving the harness's expected asset set equals the set derived from the contract, and that no hardcoded count remains in the harness. The test should assert that the harness module contains no literal asset count — the kind of check that fails when someone re-adds one.
- **Install and service identities.** A test proving the harness resolves the install identities from the contract and the service identity from the daemon, and that the harness source contains no literal install or service identity string.
- **Target classification.** A test proving the classification of each target in the contract matches the contract's own asset entries, including the Linux targets that ship the helper and the macOS and Windows targets that do not.
- **Citation checker.** Tests for `scripts/check_doc_citations.py`: a correct citation passes; a citation naming a non-existent symbol fails; a citation naming a non-existent file fails; a legacy line-number citation fails; an ambiguous bare basename fails; a citation into a Python file resolves Python symbols. Plus the self-test fixture proving the checker rejects a deliberately wrong citation — the negative-exit proof.
- **Documentation gate.** A test or CI assertion that the citation checker runs over every document in `architecture/` and `docs/`, and that the design record contains no line-number citation at all.
- **Quickstart.** The CI job that executes the quickstart script is the test. Additionally, a check that the `doctor` check names shown in `docs/quickstart.md` match the node's reported check names, and that the quickstart's command blocks match the checked-in script.
- **Qualification execution.** Evidence that the `qualification`-feature tests execute in CI, and that the CI configuration records where they run and why the default test step does not run them.
- **Capability probe non-vacuity.** Evidence that `crates/eggwork-runner/tests/capability_probe.rs` fails when the helper binary is absent and passes when it is present, with the helper present in the CI job that runs it.

## 9. Required verification

The repository's pre-submit gate, run in full:

```
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Plus the focused evidence:

- `python3 scripts/check_doc_citations.py` over `architecture/` and `docs/`, run against the pre-change tree to show it fails, and after the conversion to show it passes;
- `python3 scripts/check_release_tag.py v0.1.5` (or the repository's current tag), which passes on the pre-change tree and must still pass after the reader changes — proving the new reader yields the same value the old one did by accident — together with the tag-guard tests, run against the pre-change script to show the regex reader's failure mode;
- `eggpack ci check --workflow-shape release/eggpack/workflow-shape.json --contract release/eggpack/distribution.toml --github-policy release/eggpack/github-policy.json --workflow .github/workflows/release.yml`, proving the generated workflow has not drifted;
- the release-harness tests in `tests/release/`, run before and after;
- the capability probe, run with the helper absent and with the helper present;
- the quickstart CI job, run end to end.

`--locked` remains load-bearing on the clippy and test steps, and `git diff --check` remains part of the gate because the plan's own subject is citation hygiene and because `CONTRIBUTING.md`'s pre-submit list includes it.

## 10. Documentation updates

- `architecture/overview.md` — the component table's size column is removed and replaced by a pointer to the script that prints the current figures; the divergences table is re-verified entry by entry, with divergence #20 corrected or closed and the `doctor`-check-count entry reconciled; every line-number citation in the file is converted.
- `architecture/ci-guardrails.md` — the new CI job is documented: what it runs, what it proves, what it does not prove, and why the default test step does not enable `qualification`. The citation checker is documented alongside the ownership guard, with its scope and its limits stated honestly.
- `architecture/release-qualification.md` — the version-authority reconciliation is written down: which is the gate, which is the build assertion, and why they are permitted to differ. It must also state **where the tag-dependent gate runs today** — `scripts/check_release_tag.py`, run by whoever cuts the tag, before the tag exists — so that no reader mistakes "the guard is in the repository" for "the guard ran". The harness's derivation of the asset inventory, the install identities, and the target classification is documented as derivations from their owners, not as lists.
- `architecture/distribution.md` — updated only where it describes how the harness learns the release facts, so that it agrees with the code.
- `architecture/operations-cli.md`, `architecture/core-domain.md`, `architecture/server-node.md`, `architecture/runner-execution.md`, `architecture/data-plane.md`, `architecture/storage-journal.md`, `architecture/sandbox-helper.md`, `architecture/client-transport.md`, `architecture/deployment-lifecycle.md` — line-number citations converted to symbol citations; no other content changes except where a citation's surrounding sentence was itself wrong (the `doctor` check count in `operations-cli.md`).
- `docs/quickstart.md` — the executed steps are aligned with the checked-in script; the `doctor` output block is corrected to the eight checks the node actually reports; the document states how it is machine-verified and where.
- `docs/operations.md` — updated only if the quickstart or the harness changes anything it documents.
- `docs/releases.md` — updated only if the release-consumption story changes; this plan is expected not to change it.
- `AGENTS.md` — the claim that the quickstart's commands "are executed and verified" must be either made true by §5.4 or qualified. Leaving an unqualified claim that is still unbacked is not an acceptable outcome of this plan.
- No document in this repository gains a line-number citation as a result of this plan. This is the plan's own subject, and it must model the fix.

## 11. Acceptance criteria

1. The workspace version is read by exactly one Python implementation, using a real TOML parser scoped to `[workspace.package].version`, and every release script obtains it through that implementation.
2. `scripts/check_release_tag.py` reads the manifest and the lock through real TOML readers, has tests in `tests/release/`, and its logic is exercised by an automated CI job; the tag-dependent half is documented as running by whoever cuts the tag, since CI does not know the future tag.
3. The version authorities are reconciled in writing and in code: the manifest owns the version the binaries report; the tag↔version comparison is the release gate; the qualification harness no longer derives its expected version from the tag.
4. `QUALIFIED_ASSET_COUNT` and the harness's install- and service-identity constants are deleted, not re-synced. The harness contains no literal for any fact it asserts.
5. The harness's target classification is derived from `release/eggpack/distribution.toml`, not from string suffixes.
6. Every line-number citation in `architecture/*.md` is converted to a symbol citation; `docs/*.md` carries none.
7. A citation checker runs in CI, fails on a legacy line-number citation, fails on a missing symbol or a missing file, and is proven to fail on a deliberately wrong citation.
8. The `architecture/overview.md` component table carries no hand-written size figures; the current figures are obtainable from a checked-in script.
9. Every entry of the divergences table is re-verified; divergence #20 is corrected or closed; the `doctor`-check-count entry and the affected documents agree with the code.
10. The quickstart is executed by CI against a locally built node with generated TLS material, and the document's shown output is checked against the node's actual output — or, if the quickstart CI job cannot be landed, a waiver with an expiry and a named owner is recorded where a reader of `AGENTS.md` would look.
11. The `qualification`-feature tests execute in CI, and the CI configuration and `architecture/ci-guardrails.md` record where they run and why the default test step does not run them.
12. `crates/eggwork-runner/tests/capability_probe.rs` fails loudly when the helper is absent, passes when it is present, and is proven non-vacuous.
13. The full pre-submit gate passes; `eggpack ci check` passes against the unchanged generated workflow; the focused tests are demonstrated failing before the fix.
14. No released artifact format changes, no wire protocol changes, and no authentication, lease, generation, fencing, or digest bound is weakened.

## 12. Stop conditions

Stop and escalate rather than proceeding if:

- wiring the tag guard into the *release* workflow would require hand-editing `.github/workflows/release.yml` — the file is generated and `eggpack ci check` must keep passing; the change goes through the producer configuration or it does not happen, and the upstream prerequisite is recorded instead;
- the producer configuration `release/eggpack/` would have to change to carry a fact this plan needs — that is Eggpack's file; record the prerequisite and escalate rather than editing it;
- the citation checker's rule set would have to be weakened (for example by allowing a bare basename, or by exempting a file) to make the conversion pass — a check with an exemption is a check that will be bypassed; resolve the ambiguity in the citations instead;
- the quickstart CI job cannot be made deterministic in the CI environment (for example because the helper's trusted-path requirement or the fixture's state-directory lock cannot be satisfied) — then the waiver route applies, and it must be recorded with an expiry and a named owner, not silently;
- executing the qualification suite in CI would require weakening an isolation, trust, or resource-enforcement bound, or would require a platform claim that was not probed;
- the harness's derivations would change any released fact — asset name, install identity, target set, or version. If a derivation disagrees with the tree, the *tree* is the owner and the disagreement is a finding to investigate, not a constant to update;
- the work would weaken the ownership guard, the `--locked` discipline, or the `--prove-negative-exit` self-test.

## 13. Closure evidence required

Create `plans/closure/operations-distribution/008b-status.md`.

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output and the `eggpack ci check` output;
- the platform matrix actually exercised, stated honestly — do not claim a platform was qualified merely because it compiles, and do not claim the qualification suite ran on a platform where it did not;
- negative and adversarial evidence: the citation checker shown failing on the pre-change tree and on a deliberately wrong fixture; the version reader shown failing on a manifest whose `[workspace.package]` is not first; the capability probe shown failing with the helper absent; the quickstart job shown failing when a step is broken;
- the release-visibility consequence: confirmation that no asset name, install identity, target set, or binary version changed, and that the generated workflow did not drift;
- documentation status, listing each document changed and each citation converted, with the before-and-after counts;
- the upstream prerequisite status for the release-path tag gate;
- unresolved findings ranked by severity;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- **M008b is `proposed`, not `ready`.** It has no unmet in-repo dependency, but it carries two decisions that need a human's judgement before implementation: the canonical citation form and the checker's rule set (§5.3), and the reconciliation of the two version authorities (§5.1), which is an ownership-boundary statement and therefore an architecture review item under `plans/003-planning-process.md` §8. Resolve those first.
- **M008b is independent of M007 and of M008a.** It may execute in parallel with M007, with M008a, and with the other corrective addenda in this planning batch. It does not touch the service-host lifecycle or the shutdown-and-wait convergence contract.
- **One coupling with M008a:** the quickstart's troubleshooting table quotes the configuration error text, and M008a changes that text. Whichever plan lands second must reconcile it; neither may edit it blind. This is recorded in both plans.
- The citation conversion is the bulk of the effort and the least interesting part. Land the checker *with* the conversion, not after it: a checker that is added before the citations are converted will fail the build, and that is the correct outcome — a gate that only turns on when the tree is already clean protects nothing.
- The release-harness work is the part with the highest blast radius, because it decides what a release is allowed to contain. The discipline that matters is that the harness's remembered copies are **deleted** rather than reconciled: a constant that is updated when it disagrees is still a second owner, and it will drift again.
- The repository's own experience is that a rule written in one module and re-implemented in another has drifted before, and the drift was found by inspection rather than by a failing test. Prefer the test that fails when the rule is weakened, over a comment that describes the rule — that is the whole shape of OP-4 and OP-6.
- The quickstart job is the most expensive item here and the one that most directly discharges an unbacked claim in `AGENTS.md`. If it turns out to be too expensive to land as part of this milestone, take the waiver route and record it honestly rather than letting the claim stand unbacked.
