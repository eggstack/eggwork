# Operations and Distribution — Operator Truthfulness Corrective Addendum

Status: active corrective; M008a and M008b proposed

Predecessor roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Historical predecessor closures:

- `plans/closure/operations-distribution/001-status.md` — the node operations surface
- `plans/closure/operations-distribution/002-status.md` and `plans/closure/operations-distribution/002-resumed-status.md` — Eggup deployment and service integration
- `plans/closure/operations-distribution/002a-status.md` — recovery-required restart suppression and lifecycle composition
- `plans/closure/operations-distribution/003-status.md` — Eggpack producer packaging integration
- `plans/closure/operations-distribution/004-status.md` — operational and release qualification
- `plans/closure/operations-distribution/004a-status.md` — release qualification fixture isolation
- `plans/closure/operations-distribution/004b-status.md` — staged helper publication durability

Canonical authority remains:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `architecture/operations-cli.md`, `architecture/distribution.md`, `architecture/release-qualification.md`
- `architecture/overview.md` — the divergences table

Finding source:

- repository-wide code interrogation pass over the reviewed baseline (`81f34e0`); all six findings below are **new** and are **not** recorded in the `architecture/overview.md` divergences table.

## 1. Findings

This subsystem owns the node's operator surface (`eggworkd`) and the release channel that ships the node. Both are *reporting* surfaces: they restate facts that their owners already know — what the node is doing, what is wrong with the configuration, what version the binary is, what the release contains.

The six findings below share one mechanism. An operator-facing surface keeps a second copy of a fact it could have read from the fact's owner, and the second copy drifts. In five cases the drift is already visible in the tree; in one case the drift is not yet visible but the copy exists and is unwired.

The mechanism is worth naming precisely because it is not a bug in any one place. It is a missing ownership rule: **every operator-visible fact has exactly one owner, and every surface that reports it reads from that owner.** Today the repository has the first half of that sentence (each fact usually *has* an owner) and not the second (surfaces read from their owners).

### 1.1 Five live metrics are never reported by the operator surface (OP-1)

**What the surface should guarantee.** `eggworkd metrics` and `eggworkd status` report the node's counters. A counter the server increments is a fact the node knows; an operator watching the node's workspace-derivation behaviour should be able to see it.

**What the code actually does.** `crates/eggwork-server/src/operations.rs` declares `const METRIC_NAMES: &[&str]`, a hand-maintained list of metric names, and `fn metrics_snapshot` seeds its counter map from that list. When it then reads rows from the node metrics table it copies a value **only if the name is already in the map** — any metric not in `METRIC_NAMES` is silently discarded. `crates/eggwork-server/src/lib.rs` increments five metrics that are not in that list: `workspace_manifest_registrations`, `workspace_derived_requests`, `workspace_derived_hits`, `workspace_patch_entries`, and `workspace_base_missing`. The workspace-derivation path calls `increment_metrics` with three names at once, so a single logical event can move three counters that the operator surface never shows.

Nothing detects the drift: there is no test that compares `METRIC_NAMES` against the set of names the server can increment, and no test that asserts a freshly incremented metric appears in a snapshot.

**Consequence.** An operator watching a node's workspace-derivation behaviour sees counters that never move, with no indication that the counters exist. The node has the facts; the operator surface drops them.

### 1.2 One `active_executions` field name carries two different meanings (OP-2)

**What the surface should guarantee.** One name, one fact. A reader who sees `active_executions` in two places must be able to rely on them meaning the same thing.

**What the code actually does.** Two independent computations produce the same name:

- `crates/eggwork-server/src/lib.rs` → `fn node_status` computes `active_executions` for the `/v1/status` response as *execution permits currently held* (`max_active` minus available permits). The field lives on `eggwork_core::NodeStatus`, which is a serialized wire type.
- `crates/eggwork-server/src/operations.rs` → the execution listing computes `active_executions` as *the number of database rows in a non-terminal state*.

Both surface as `active_executions`: one in the API status response, one in the `eggworkd status` and `eggworkd executions list` output (`crates/eggwork-server/src/bin/eggworkd.rs` writes the key `active_executions` into the status payload).

**Consequence.** The same JSON key reports different facts at different layers. The two disagree whenever the two facts differ — most visibly when an execution has crashed but has not been recovered: the permit has been released, so the API reports it as not active, while the row is still non-terminal and the operator surface still counts it. There is no way for an operator to tell which number is which, and no field name that would let them.

### 1.3 Configuration errors collapse into one unhelpful operator message (OP-3)

**What the surface should guarantee.** When a configuration fault is one wrong file mode or one wrong path, the operator is told which rule failed and what to change.

**What the code actually does.** `crates/eggwork-server/src/operations.rs` returns `OperationsError::Config` at 25 sites, covering every distinct configuration fault: unreadable file, bad permissions, wrong owner, a sandbox helper outside a safe ancestor, a malformed field, a failed parse of the bind address, and more. `crates/eggwork-server/src/bin/eggworkd.rs` then maps the load failure to the single string `configuration is invalid or unreadable`, discarding the error kind entirely.

The repository's own `AGENTS.md` calls this out: the config loader's strictness (owner-only state directories, private-key mode, nothing group/world writable, a sandbox helper outside world-writable ancestors) "is undocumented anywhere else and produces a single unhelpful `configuration is invalid or unreadable`".

**Consequence.** An operator who has a single wrong file mode gets no indication of which file or which rule failed. This is a documented, operator-visible pain point with a mechanical fix, and it is the one finding in this addendum whose remedy is called out in the repository's own entry instructions.

The fix is not free of tension. `architecture/operations-cli.md` records that the binary "never interpolates an error's source text for library errors, no filesystem path, key detail, or child-process output can reach stderr through that path". That is a deliberate redaction property, and adding detail must not become a leak. The remedy therefore has to name the *rule* and the *file or path* that failed — and only the *role* of an item whose path is itself secret-bearing — rather than interpolate arbitrary source text.

### 1.4 The release harness re-derives facts that already have one owner (OP-4)

**What the surface should guarantee.** The release harness reports the same version, the same asset inventory, the same install identities, and the same target classification that the shipped artifacts actually have.

**What the code actually does.** Four separate duplications:

- **Workspace version, three readers.** `scripts/check_release_tag.py` → `fn workspace_version` uses a regular expression over the whole `Cargo.toml` and takes the *first* `version = ` line in the file, rather than the one under `[workspace.package]`; it works only by accident of the current manifest layout, in which `[workspace.package]` happens to precede every other `version = ` line. `scripts/release_candidate_probe.py` → `fn workspace_version` reads `workspace.package.version` with `tomllib`, and `scripts/validate-daemon-version.py` and `scripts/validate-sandbox-helper.py` already import that reader from it rather than keeping their own. `crates/eggwork-server/tests/release_contract.rs` → `fn the_release_version_is_one_coherent_workspace_identity` reads the same field with the `toml` crate. `scripts/check_release_tag.py` is **not wired into any CI workflow**, so its divergence from the other readers is currently latent rather than observable.
- **The tag guard is never executed by an automated job.** `scripts/check_release_tag.py` is referenced by nothing in the repository except its own docstring. It is not invoked by `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/workflows/operational-qualification.yml`, or `.github/workflows/windows-reproducibility.yml`, and it has no test in `tests/release/`. The script's own docstring gives a deliberate reason for part of this — "the tag-to-version link cannot live in CI: the generated release workflow is producer authority … and CI never knows the future tag. So the check lives here, run by whoever cuts the tag" — and that reason is sound: a tag-dependent assertion cannot run in CI. The defect is therefore narrower than "it is unwired", and it is three things: the script reads the manifest with a **different parser** from the shared reader; `fn lock_versions` parses `Cargo.lock` with regular expressions as well; and a guard whose correctness decides whether a release is tagged is exercised by **no automated job at all** — not even against a fixture — so its correctness is an untested assumption.
- **Two version authorities.** A manifest-derived authority (`scripts/validate-daemon-version.py`, `scripts/validate-sandbox-helper.py`, `scripts/release_candidate_probe.py` — all derive the expected version from the workspace manifest) and a tag-derived authority (`scripts/qualify_release.py` → `fn expected_version` derives the expected version from the release tag and asserts the binary reports it). Both are legitimate; nothing states which one is the release gate and which one is the build assertion.
- **Asset inventory, twice.** `scripts/qualify_release.py` → `fn stage_inventory` derives the expected asset set dynamically from the release manifest, and in the same function asserts the asset count against the hardcoded `QUALIFIED_ASSET_COUNT`. A constant that duplicates a value already computed one line away.
- **Install identities, four times.** `crates/eggwork-server/src/deployment.rs` declares them as constants (`SERVICE_ID`, `MEMBER_DAEMON`, `MEMBER_HELPER`); `release/eggpack/distribution.toml` declares the same identities in its `install` fields and is described in its own header as "the sole checked-in authority for the published target set, release asset names, install identities, and checksum sidecar names"; `scripts/qualify_release.py` declares `SERVICE_ID` and `HELPER_INSTALL_ID` as its own constants; and `crates/eggwork-server/tests/release_contract.rs` pins the Rust side to string literals. Nothing pins the Python side.
- **Target classification, twice.** `crates/eggwork-server/tests/release_contract.rs` holds explicit lists of target triples; `scripts/qualify_release.py` → `fn target_daemon_install` and `fn target_ships_helper` classify a target by testing whether the string ends with `-pc-windows-msvc` or `-unknown-linux-gnu`.

**Consequence.** Every one of these is a place where a release can be qualified against a fact that the shipped artifact does not have, and where the harness cannot tell the difference. The `QUALIFIED_ASSET_COUNT` case is the sharpest: the function that *derives* the expected set also *asserts a constant* about it, so adding a release target requires editing two things in one function that disagree with each other in spirit — one derived, one remembered.

### 1.5 The design record and the operator documentation have drifted, and there is no gate (OP-5)

**What the surface should guarantee.** `architecture/overview.md` is the first document an agent is told to read and every other document cites it. `AGENTS.md` asserts that the quickstart's commands "are executed and verified, not illustrative". A reader following a citation should arrive at the symbol it names.

**What the code actually does.**

- **Line-number citations are stale.** `architecture/*.md` carries 718 line-number citations of the form `file.rs:NNN`. Re-measuring against the tree with the nearest backticked identifier as the anchor, 85% of the resolvable citations no longer resolve at the exact cited line, 83% are stale within a ±3-line tolerance, and 72% remain stale even allowing ±20 lines of drift. (The interrogation pass reported roughly 92%; the exact figure depends on the tolerance chosen, and the conclusion does not depend on it.) Spot-verified examples: `architecture/deployment-lifecycle.md` cites `verify_trusted_helper` as living in `crates/eggwork-server/src/lib.rs`, while the function is defined in `crates/eggwork-runner/src/lib.rs` — the wrong file entirely; `architecture/runner-execution.md` cites `LANDLOCK_WORKSPACE_RW_CAPABILITY` and `probe_landlock_ruleset` at line numbers that sit roughly 140 lines away from the definitions they name in `crates/eggwork-runner/src/lib.rs`. The citation form itself is the defect: a line number is a property of the text, not of the symbol, and it is invalidated by every unrelated edit above it.
- **The component size table is stale.** Every crate size figure in the `architecture/overview.md` component table understates the tree. Counting non-test Rust sources at the reviewed baseline: core 2,294 lines against a stated 1,671; runner 2,900 against 2,336; client 1,158 against 880; server 16,980 against 14,104; sandbox-helper 446 against 401. A table of numbers that are all wrong is worse than no table, because it looks authoritative.
- **Divergence #20 is stale in the dangerous direction.** The `architecture/overview.md` divergences table states that the client decodes JSON bodies "with no size cap". That is no longer true: `crates/eggwork-client/src/lib.rs` applies `max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)` at the transport layer and a stricter `MAX_TELEMETRY_BODY_BYTES` cap on the telemetry route. The table is read as the list of *open* questions, so a reader would go chasing an issue that is already fixed. This is the most dangerous form of documentation drift, because it actively misdirects.
- **A second divergence of the same kind exists between documents.** `architecture/operations-cli.md` describes `doctor` as "Seven checks, in order" and does not list `host-telemetry`; `crates/eggwork-server/src/operations.rs` → `fn doctor` emits eight checks, including `host-telemetry`, and `docs/operations.md` already documents eight. The design record lags the operator documentation and the code.
- **`docs/quickstart.md` is not machine-verified.** No CI job, test, or script executes or even references it; the only mention outside `docs/` is a source comment. `AGENTS.md`'s claim that quickstart commands "are executed and verified, not illustrative" is true of their authors' intent and unbacked by any gate.
- **The mechanism.** The repository's prose volume is roughly 85% of its code volume (`architecture/` about 8,500 lines against `crates/` about 28,400; `plans/` about 15,300; `docs/` about 870; `scripts/` about 2,550), and documentation files are touched about 2.7 times as often as Rust files in recent history. Prose moves faster than code and nothing reconciles the two.

**Consequence.** The design record's authority is asserted, not earned. An agent that follows a citation arrives at the wrong symbol; an agent that reads the divergences table as open questions investigates a fixed bug; an operator that follows the quickstart may find a command that no longer works.

**Class note.** Part of this finding is genuinely **polish** — a stale size table and a stale `doctor` check count are documentation defects, not behavioural ones. It must not be represented as a capability. The part that is not polish is the *absence of a gate*: a document that asserts "executed and verified" while nothing verifies it is a truthfulness defect of the same family as OP-1 and OP-3.

### 1.6 CI does not execute the code it type-checks (OP-6)

**What the surface should guarantee.** A green CI run is evidence that the code ran.

**What the code actually does.**

- **The feature-gated qualification test never runs.** `.github/workflows/ci.yml` runs `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` with `--all-features`, but runs `cargo test --locked --workspace --all-targets` **without** it. `crates/eggwork-server/tests/installed_qualification.rs` is gated by `#![cfg(feature = "qualification")]`, so clippy type-checks it and the test step never executes it. It is additionally marked `#[ignore]`, and it runs in the operational-qualification workflow instead.
- **One test is green while proving nothing.** `crates/eggwork-runner/tests/capability_probe.rs` → `fn built_helper` locates the sandbox helper binary via runtime environment variables (`CARGO_TARGET_DIR`, `OUT_DIR`); when it cannot, the test returns early and **passes with zero assertions**. The file's own comment claims "CI builds it before running this suite, so this skip is a local-only convenience" — but `CARGO_TARGET_DIR` is not set by the repository's `.cargo/config.toml` and is not exported to integration test binaries, and `OUT_DIR` is a compile-time variable for build scripts, not a runtime environment variable. The skip is therefore the *normal* path in CI, not an exceptional one. The repository already contains the deterministic pattern this test should use: `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` → `fn built_helper` resolves the helper through the compile-time `CARGO_BIN_EXE_` variable, and `crates/eggwork-server/src/lib.rs` → `fn locate_helper_binary` walks the current executable's sibling and grandparent before consulting `CARGO_TARGET_DIR`.

**Consequence.** A green CI run is weaker evidence than it appears. One suite is type-checked but never executed, and one test in an executed suite reports success without asserting anything.

## 2. Corrective milestones

### M008a — Operator surface truthfulness

Class: capability

Status: proposed

Implementation plan:

- `plans/implementation/operations-distribution/008a-operator-surface-truthfulness-corrective.md`

Closure record:

- `plans/closure/operations-distribution/008a-status.md`

Covers OP-1, OP-2, OP-3. Operator-visible behaviour changes: live metrics become visible, the two `active_executions` facts become separately named, and configuration failures become actionable.

### M008b — Release harness and documentation single-sourcing

Class: infrastructure (release harness and CI) / polish (documentation drift and citation hygiene)

Status: proposed

Implementation plan:

- `plans/implementation/operations-distribution/008b-release-harness-and-documentation-single-sourcing-corrective.md`

Closure record:

- `plans/closure/operations-distribution/008b-status.md`

Covers OP-4, OP-5, OP-6. The harness and CI portion is internal machinery that supports the release capability and is classified as infrastructure; the documentation-drift portion is explicitly polish and must not be represented as capability. The plan changes no released artifact format and no wire protocol.

## 3. The ownership rule these correctives establish

**One owner per fact. Every surface reads from the owner.**

Concretely, after M008a and M008b:

| Fact | Owner | Surfaces that read it |
|---|---|---|
| Metric names and their values | the increment sites in `crates/eggwork-server/src/lib.rs` | `eggworkd metrics`, `eggworkd status` |
| Execution permits held | the permit accounting in `crates/eggwork-server/src/lib.rs` | `/v1/status` (`NodeStatus`) |
| Non-terminal execution rows | the execution store | `eggworkd status`, `eggworkd executions list` |
| Configuration fault kind and rule | the config loader in `crates/eggwork-server/src/operations.rs` | `eggworkd` config verbs, `doctor`, daemon startup |
| Workspace version | `Cargo.toml` `[workspace.package].version` | release scripts, release contract test |
| Published asset inventory | `release/eggpack/distribution.toml` | qualification harness, release contract test |
| Release install identities | `release/eggpack/distribution.toml` and `crates/eggwork-server/src/deployment.rs` | deployment, qualification harness, release contract test |
| Release target classification | `release/eggpack/distribution.toml` | qualification harness, release contract test |
| Citation targets in the design record | the symbols in the tree | `architecture/*.md`, `docs/*.md` |

A second copy of a fact is not permitted. Where a second copy is unavoidable across a language boundary (Rust and Python), the copy must be *derived* from the owner at run time or checked by a gate that fails when the two disagree — never remembered independently.

## 4. Durable invariants

- **I-1.** Every metric the server can increment is reported by the operator surface. A metric that is incremented but not reported is a defect, and a test must fail when it occurs.
- **I-2.** No two operator-visible fields in the same system carry the same name and different meanings. Where a name is protocol-visible, the protocol wins and the other fact takes a different name.
- **I-3.** A configuration failure names the rule that was violated, the file or path that failed (the item's role, where the path is secret-bearing), and the corrective action — and never discloses a private-key path, a certificate fingerprint, or arbitrary underlying error text.
- **I-4.** The release harness derives every fact it asserts from its owner; it holds no independently remembered constants for version, inventory, identity, or target classification.
- **I-5.** The design record cites symbols, not line numbers. A citation that cannot be checked mechanically is not a citation.
- **I-6.** A test that asserts nothing is not evidence. A suite that is type-checked but not executed is not evidence. Both states must be visible to a reader of the CI configuration.
- **I-7.** These correctives change no released artifact format and no wire protocol, and they do not weaken any authentication, lease, generation, fencing, or digest bound. They do not move the fixed-target, scheduler-free boundary in either direction.

## 5. Compatibility and migration

- **OP-2 is the only finding with a protocol-visible consequence.** The `active_executions` field of `eggwork_core::NodeStatus` is a serialized wire type consumed by `eggwork-client` → `NodeClient::status`. Renaming a field of a wire type that deployed clients already deserialize breaks them, because a missing field is a deserialization error, not a default. The M008a plan therefore renames the *operator-side* field and leaves the wire field alone, and states the consequence explicitly.
- The operator-side field rename is a change to `eggworkd`'s JSON output. `eggworkd status` and `eggworkd executions list` output is operator-facing and is not a published protocol, but operators and any scripting around it may parse it. The plan must record the new names and keep the old names as accepted aliases for one release, or state why an alias is not offered.
- **OP-1 is additive.** Metrics that were invisible become visible. No counter disappears and no value changes meaning.
- **OP-3 changes only failure text.** A successful configuration load is unaffected. The redaction property in `architecture/operations-cli.md` is preserved, and a test proves that a failure message contains neither a private-key path nor a certificate fingerprint.
- **OP-4 changes no released artifact.** The asset names, install identities, target set, and binary versions are unchanged; only the harness's route to those facts changes. If the harness's derived expectations change at all, that is a signal that a *fact* changed, not that the harness was wrong to remember it — and it must be investigated as such.
- **OP-5 changes no behaviour.** The design record and operator documentation are corrected to match the tree.
- **OP-6 changes no released behaviour.** The CI gap is closed by executing code that already exists, or by recording in the place a reader would look why it cannot be executed.

## 6. Downstream relationships

- **M007 (Windows service host) is `ready` and is not disturbed.** M007 owns the foreground `run` path's shutdown asymmetry — the foreground path calls shutdown without waiting for convergence, and M007 commits to a shared shutdown-and-wait convergence contract. **That finding is already owned by M007 and is deliberately not duplicated in this addendum.** Nothing in M008a or M008b touches the service-host lifecycle, the shutdown contract, or the Windows service path.
- **M004, M004a, M004b are closed.** Their closure records stand and are not rewritten. OP-4's findings concern the *harness's* duplication, not the fixture isolation or staged-helper durability that M004a and M004b closed.
- **M005 and M006 are deferred** in `plans/subsystems/operations-distribution-roadmap.md`. Nothing here depends on them and nothing here pre-empts them.
- **M008a and M008b are independent of each other** and of M007. They may be executed in parallel with the other corrective addenda in this planning batch.
- **The registry is owned by another writer.** `plans/registry.md` is not modified by these documents; registering M008a and M008b as current authority is the registry owner's action, per `plans/README.md`.

## 7. Exit conditions

The operations and distribution subsystem may be treated as truthfully reported when:

1. every metric the server can increment is visible to `eggworkd metrics`, with a test that fails when a producer increments an unreported metric;
2. the two `active_executions` facts are separately named, separately documented, and separately tested, with the wire field's disposition stated;
3. a configuration fault produces a message naming the rule and the offending file or path — the item's role where the path is secret-bearing — and no configuration fault message discloses a private-key path or a certificate fingerprint;
4. the release harness derives its version, inventory, identities, and target classification from their owners, holds no independently remembered copies, and `scripts/check_release_tag.py` uses the shared reader and is exercised by an automated job;
5. the design record cites symbols rather than line numbers, a mechanical check enforces it, the component size table is corrected or replaced by something that cannot silently go stale, and divergence #20 is corrected or closed;
6. the quickstart is machine-verified, or the waiver is recorded where a reader would look for it;
7. the feature-gated qualification test executes in CI or the reason it cannot is recorded in the same place; and the capability probe either asserts something or fails loudly when its precondition is absent.

Until then, the operations and distribution subsystem carries an active corrective, and its closure records must not be read as evidence that these properties hold.
