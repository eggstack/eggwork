# Workspace and Artifact Transport M005a Closure — Declared-artifact capture fail-closed corrective

- Milestone: **M005a — Declared-artifact capture fail-closed corrective**
- Corrective addendum: `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md` (wins over the plan where they disagree)
- Implementation plan: `plans/implementation/workspace-artifact-transport/005a-declared-artifact-capture-fail-closed-corrective.md`
- Roadmap entry: `plans/subsystems/workspace-artifact-transport-roadmap.md` (§ M005a, status **implemented**)
- Implementation commit: `IMPLEMENTATION-COMMIT-PENDING`
- Closed on: 2026-10-23 (Linux x86_64 host)

## Finding

The server's declared-artifact capture refuses on a platform that cannot open a file
beneath the workspace root safely: `artifact::open_file_beneath` has a `#[cfg(not(unix))]`
arm that returns `ErrorKind::Unsupported` rather than silently falling back to a
path-based open. The finalization, however, only recorded
`ExecutionFinalizationFailure::ArtifactCapture` and **left the terminal state
`Succeeded`**. A caller that declared outputs therefore saw a successful execution
with zero artifacts and no error: the declared outputs were silently dropped. The
same finalization set `artifact_count` to `0`, so the dropped outputs were
indistinguishable from an execution that declared none.

The capability advertisement had the mirror-image defect: `static_capability_features`
listed `"artifact.declared.v1"` on **every** platform, so a Windows node advertised a
capability its capture path would refuse. The advertisement is what a caller uses to
decide whether to declare outputs at all, so the two facts had to be reconciled.

The defect was asserted by a test, not merely implied by the code: the existing
integration assertion in `crates/eggwork-server/src/lib.rs`
(`blob_protocol_streams_verifies_deduplicates_and_enforces_quota`) pinned
`state == ExecutionState::Succeeded` together with `artifact_count == 0` and
`finalization_failure == Some(ArtifactCapture)`. That assertion was **revised
deliberately** by this milestone, not incidentally.

## What was inspected by reading, and what was executed

Everything in this record was produced on the Linux x86_64 host this milestone was
implemented on. The distinction is load-bearing, so it is stated per item:

| Kind | Items |
| --- | --- |
| **Executed** | `cargo test --locked --workspace --all-targets` (every suite, including the new portable target and the revised server integration assertion), `cargo test --locked -p eggwork-core --all-targets`, `cargo test --locked -p eggwork-server --all-targets`, `cargo test --locked -p eggwork-server --test artifact_capture_disposition`, `cargo test --locked -p eggwork-server --features qualification --test installed_qualification` (compiles; its 3 tests are `#[ignore]`d), `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --all -- --check`, `cargo check --workspace`, `python3 scripts/check_execution_ownership.py`, `python3 scripts/check_execution_ownership.py --prove-negative-exit`, `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`, `git diff --check`, `cargo check --locked -p eggwork-core --target x86_64-pc-windows-msvc`, `cargo check --locked -p eggwork-core --target x86_64-apple-darwin` |
| **Inspected only (code inspection)** | The Windows and macOS capture paths, the `#[cfg(not(unix))]` arm of `open_file_beneath`, the `windows-runner` CI job's new step, the hosted Windows qualification step, the `artifact.declared.v1` advertisement on a non-Unix node, and the whole of `crates/eggwork-server/tests/installed_qualification.rs` at run time |
| **Not run at all** | The hosted Windows qualification (`.github/workflows/operational-qualification.yml`) — it needs a tagged release, an installed daemon, and a Windows host. No receipt was produced. The `windows-runner` CI job has not run on this branch, because the branch is not pushed. |

**A Linux run is not evidence for a Windows claim.** Nothing in this record should be
read as qualifying Windows or macOS.

## Requirement-to-evidence matrix

The criteria are numbered as in the plan's §12 (the plan's §11 is the documentation
list; the acceptance criteria are its §12).

| # | Criterion | Implementation site | Test | Status |
| --- | --- | --- | --- | --- |
| 1 | No execution can reach terminal state `Succeeded` while a declared output it requested was not captured | `eggwork_server::decide_artifact_capture` (crate root, `crates/eggwork-server/src/lib.rs`), applied by `run_execution` | `a_declared_output_that_was_not_captured_never_reports_success` (server unit test and `tests/artifact_capture_disposition.rs`); `blob_protocol_streams_verifies_deduplicates_and_enforces_quota` (the missing-required-output path) | Met, executed on Linux |
| 2 | The capture failure is visible on the terminal state, in the event stream, and through the client's view — not only as `finalization_failure` | `publish_capture_diagnostic` (server crate root) on the existing `publish` path; `ExecutionResult.state` / `.failure` / `.artifact_capture` | The revised `blob_protocol_streams_verifies_deduplicates_and_enforces_quota` asserts the diagnostic event is present and that its position is **before** the `State(Failed)` event; the portable target asserts the terminal facts | Met, executed on Linux |
| 3 | The terminal result distinguishes "no declared outputs" from "declared outputs that vanished" through a first-class typed disposition, with no arithmetic required of the caller | `eggwork_core::ArtifactCaptureDisposition`; `ExecutionResult.artifact_capture` (`#[serde(default)]`) | `the_capture_disposition_round_trips` and `a_snapshot_without_the_capture_disposition_decodes_as_unknown` (core); `an_execution_without_declared_outputs_reports_not_requested` (server unit test and portable target) | Met, executed on Linux |
| 4 | The runtime capture-capability fact reports the correct value on every platform, and is reachable from outside the crate-private data-plane modules | `artifact::capture_capability` / `artifact::ArtifactCaptureCapability`, re-exported at the crate root as `eggwork_server::artifact_capture_capability` | `the_capture_capability_fact_matches_the_platform` (portable target) — asserts `Available` on `cfg!(unix)` and `Unavailable` otherwise; the *non-Unix* branch is **not executed here** | Met on Linux; the non-Unix value is code inspection plus the portable assertion, and is executed by the `windows-runner` job once it runs |
| 5 | The advertised capability set agrees with the runtime fact, and the plan records what a caller sees when the capability is absent | `server_capability_features` filters `ARTIFACT_DECLARED_CAPABILITY` on the fact; the capability *name* is unchanged | `the_declared_artifact_capability_is_advertised_only_when_the_platform_can_capture` (server unit test) | Met, executed on Linux (where the fact is `Available`, so the test pins the present case; the absent case is pinned by the same test's `Unavailable` arm and by code inspection) |
| 6 | The existing integration test that asserted `Succeeded` + `artifact_count == 0` + `finalization_failure == Some(ArtifactCapture)` has been replaced by an assertion of the corrected disposition, and the replacement is proven non-vacuous | The revised assertion in `blob_protocol_streams_verifies_deduplicates_and_enforces_quota` now asserts `Failed`, `failure == Some(Internal)`, `artifact_capture == Failed`, and the diagnostic-before-terminal ordering | The negative-evidence experiment below | Met, executed on Linux |
| 7 | The new portable test target compiles and passes on a non-unix host, and is wired into the `windows-runner` CI job | `crates/eggwork-server/tests/artifact_capture_disposition.rs`; `.github/workflows/ci.yml` step *declared-artifact capture disposition suite* | The 7 tests of the portable target pass on Linux; **the Windows compile and run is not observed here** — the cross-check is blocked by the environment (below) and the CI job is the evidence | Partially met: wired and green on Linux; the Windows run is outstanding |
| 8 | The Windows hosted qualification declares an output and asserts the truthful terminal disposition against the installed daemon, and the receipt is recorded | `installed_release_reports_a_truthful_declared_artifact_disposition` in `crates/eggwork-server/tests/installed_qualification.rs` | The test compiles with `--features qualification` and is `#[ignore]`d like its sibling; **it was not executed** — it needs an installed daemon, a Windows host, and a hosted run | Not met here: the test exists, the *run* does not, and the workflow step that would run it is coordinator-owned (finding F-01) |
| 9 | The README's Windows caveat and the `architecture/overview.md` divergences table state the artifact disposition, and the statement matches the code | `architecture/overview.md` row 22 (new *Platform limitations* subsection) | Inspection; the row is checked against `artifact::capture_capability` and `server_capability_features` | Partially met: the overview row is written; **the README is not edited** (coordinator-owned, finding F-02) |
| 10 | No bound is widened | No constant was added, removed, or changed in `eggwork-core`, `eggwork-server`, or `eggwork-runner` | `git diff` over the three crates: `MAX_OUTPUTS`, `MAX_ARTIFACT_FILES`, `MAX_ARTIFACT_TOTAL_BYTES`, `MAX_BLOB_BYTES`, and every digest bound are untouched | Met |
| 11 | The unix capture tests keep their original meaning and stay green, and the happy-path integration assertion is still green | The capture path (`artifact.rs`) is unchanged apart from the added capability fact | `descriptor_relative_capture_rejects_parent_swapped_to_symlink`, `missing_required_and_symlink_outputs_fail_without_records`, `disappearing_output_and_storage_quota_fail_without_publishing_records` — executed, green; the happy path now also asserts `artifact_capture == Completed` | Met, executed on Linux |
| 12 | The double hash in `crates/eggwork-server/src/artifact.rs` is still present and documented as accepted | `capture_blob` hashes while reading and `blobs.put_stream` verifies again | Inspection; the function is unchanged in this milestone | Met |
| 13 | No authentication, lease, generation, fencing, or digest-bound semantics weakened; no queue, worker selection, priority, fairness, DAG, or semantic retry introduced | The change is confined to the capture finalization, the capability advertisement, and the diagnostic class set | `python3 scripts/check_execution_ownership.py` (passed); the lease/admission code is untouched; no new crate edge | Met |
| 14 | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and `cargo test --locked --workspace --all-targets` are green on the same head | — | Executed here: both green (see *Verification actually run*) | Met |

## Decisions taken where the plan left the choice open

### The §5.4 resolution: **(b)**

The platform cannot support safe capture — `open_file_beneath` is a descriptor-relative
`NOFOLLOW` chain with no non-Unix implementation — so **resolution (b)** is the truth
of the platform:

- the platform's capture-capability fact reports capture as **unavailable** where safe
  capture is not possible (`artifact::capture_capability` is `Unavailable` on
  `not(unix)`);
- an execution that declares outputs there terminates in a **truthful non-success
  state** (`ExecutionState::Failed`, `ExecutionFailure::Internal`, disposition
  `Failed`);
- the platform claim states that declared outputs are **refused** on that platform
  rather than silently dropped.

The Windows *implementation* of a safe open remains outstanding. Per the plan's §5.5 it
is deferred to a follow-on, and the deferral is only legitimate because the fail-closed
contract is implemented in this same milestone. The follow-on is named in the
unresolved findings (F-04).

On the Linux host used here, safe capture **does** work, so the fact reports
`Available` and the truthful-failure path is exercised by the portable test rather
than by the platform itself. Both branches of the decision exist and are reachable:
`decide_artifact_capture` is parameterised by the capability, and the portable target
drives the `Unavailable` branch directly.

### The public-reachability shape: a targeted `pub use` re-export plus a public decision function

`crates/eggwork-server/src/lib.rs` declares `mod artifact;` privately, alongside
`mod blob;`, `mod store;`, and `mod workspace;`. The least-privilege shape that makes
the contract reachable from outside the crate is:

- `pub use artifact::ArtifactCaptureCapability;` and
  `pub use artifact::capture_capability as artifact_capture_capability;` — the data
  plane keeps ownership of the fact, and the crate root re-exports it rather than
  reimplementing it. A caller asks the *node's* platform; it does not infer the
  answer from its own `cfg`.
- `pub fn decide_artifact_capture(facts: ArtifactCaptureFacts) -> ArtifactCaptureDecision`
  at the crate root — the single, testable decision.

The alternative shapes were rejected: making `mod artifact` public would expose the
whole data-plane module (blob store internals, artifact store internals) rather than
the two facts the contract needs, and a wrapper function at the crate root would add
a second call path for the same fact.

**Compatibility consequence.** `eggwork-server` is a published crate, so this is a
release-visible public API addition. It is **additive**: nothing is removed and
nothing is renamed, so it is compatible. The added surface is
`ArtifactCaptureCapability`, `artifact_capture_capability`, `ArtifactCaptureAttempt`,
`ArtifactCaptureFacts`, `ArtifactCaptureDecision`, and `decide_artifact_capture`.

### The `ExecutionFailure` choice: the terminal state **and** the failure both change

A capture that did not complete moves the terminal state to `ExecutionState::Failed`
**and** sets `ExecutionFailure::Internal`. The reasoning:

- The plan requires that a caller can distinguish "the target failed" from "the target
  succeeded and its declared outputs could not be collected". The terminal *state*
  alone cannot carry that distinction, because both cases end in `Failed`.
- The *target's own* failure is never overwritten. A target that failed for its own
  reason keeps that failure (`Some(ExecutionFailure::Spawn)` stays `Spawn`), so the
  node's `Internal` failure appears only where the node itself failed.
- The new `ExecutionFailure` variant that would say "the capture failed" was **not**
  added. The plan's stop condition (§13) makes a vocabulary change that the existing
  vocabulary cannot express an architecture question. The existing vocabulary *can*
  express it — `Failed` + `Internal`, plus the first-class disposition, plus the
  diagnostic — so no ADR is needed.

The distinction is therefore: `failure == Some(Internal)` **and**
`artifact_capture == Failed` means "the target succeeded and the node could not
collect its declared outputs"; `failure == Some(<the target's own>)` means the target
failed for its own reason.

### The disposition has a fourth state: `Unknown`

The plan named three states (`not requested`, `completed`, `failed`). The
implementation has four: `NotRequested`, `Completed`, `Failed`, and `Unknown`.

The fourth state exists because of the `#[serde(default)]` requirement. The field's
default is what a peer decodes when it reads a snapshot written *before* the field
existed, and the plan's own framing of the requirement is that a peer must not be
forced to read an absence as a fact. `NotRequested` as the default would be a claim —
"the caller asked for nothing" — that such a snapshot cannot support, and it would
reproduce exactly the silent-success misreading this milestone exists to kill. So the
default is `Unknown`: "the node reported no disposition", and a caller falls back to
the legacy `finalization_failure` marker and the artifact count.

`Unknown` is also what the crash-recovery path writes, because
`ExecutionStore::recover` cannot know what the interrupted execution declared. That is
the same statement: the node reports no disposition.

### The diagnostic class: `artifact.capture.failed`

`eggwork_core::DIAGNOSTIC_CLASS_ARTIFACT_CAPTURE_FAILED = "artifact.capture.failed"`,
added to the closed `DIAGNOSTIC_CLASSES` set (which becomes `[&str; 3]`). The payload
carries `requirement=declared` (the caller declared outputs and the node owns
delivering them — this is not a degradation of a best-effort request),
`outcome=platform_unavailable | capture_failed`, and `reason`.

The diagnostic rides the server's **existing** event-publishing path — the same
`publish` call every stdout/stderr/diagnostic chunk uses — so there is one publisher
for `ExecutionEventKind::Diagnostic`, not two. The Security M005a milestone's
publisher was extended, not duplicated.

The payload is bounded by `MAX_DIAGNOSTIC_PAYLOAD_BYTES` (2048, which is
`MAX_METADATA_VALUE_BYTES`), and `ExecutionEvent::validate` bounds a `Diagnostic`
payload by `MAX_EVENT_CHUNK_BYTES` (65536). **2048 < 65536**, so the diagnostic can
never overflow the event it rides in; the relationship is asserted as a compile-time
fact in `eggwork-core` and documented in `architecture/protocol.md`.

### The `finalization_failure` marker stays, as a secondary marker

`ExecutionFinalizationFailure::ArtifactCapture` is still set on a failed capture, and
it is no longer the only signal. It remains a *secondary* marker with two known
properties, recorded as findings below: it can still be overwritten by the retention
marker when both the capture and the workspace retention fail (F-05), and a legacy
node's snapshot still carries it alone (F-06).

### The §9 coverage combination: **Option B + Option C + Option D**

The plan's recommended combination, taken in full.

- **Option B** — a new portable integration test target,
  `crates/eggwork-server/tests/artifact_capture_disposition.rs`, and a new
  `windows-runner` step in `.github/workflows/ci.yml` that runs **only that target**
  (`cargo test --locked -p eggwork-server --test artifact_capture_disposition`). The
  job's scope comment was updated in the same commit to say what is now covered, and
  the claim was not widened: the rest of the server's test surface is still
  Linux-shaped and is still explicitly not claimed.
- **Option C** — `crates/eggwork-server/tests/installed_qualification.rs` gained
  `installed_release_reports_a_truthful_declared_artifact_disposition`, which
  declares an output, drives the installed daemon, and asserts the truthful terminal
  disposition: `Succeeded` + `Completed` + one published artifact where the node
  advertises the capability, and **not** `Succeeded` + `Failed` + zero artifacts
  where it does not.
- **Option D** — the platform fact is reachable from the portable test
  (`eggwork_server::artifact_capture_capability`), so the non-Unix stub's behaviour is
  covered indirectly: the portable test asserts the fact's value for the host it runs
  on and drives the fail-closed decision from it.

**The trade-off accepted.** Option D's coverage of "`open_file_beneath` refuses" is
host-dependent: the stub's own error path is exercised only on a non-Unix host, so
the *decision* is covered everywhere and the *stub* only where the stub runs. Option B
costs a small additive public API change to a published crate and one more step in the
Windows job. Option C is workflow-level, tag-gated, and slow, so a regression could
merge green in `ci.yml` and be caught only at qualification time — which is exactly
why B is not optional. The combination buys PR-gate coverage on every platform, plus
the real-Windows evidence against installed bytes, at the cost of a public API
addition and a duplicated test surface (the same contract is asserted in the unit
harness and in the portable target).

## Non-vacuity evidence

The corrective is not proven by a green run alone: the new tests were shown to **fail
against the silent-success shape**.

The production decision was temporarily patched to reproduce the old shape — a declared
output that was not captured leaves the terminal state `Succeeded` — and the suites
were re-run against the final code:

```text
$ cargo test --locked -p eggwork-server --test artifact_capture_disposition
---- a_declared_output_that_was_not_captured_never_reports_success stdout ----
thread 'a_declared_output_that_was_not_captured_never_reports_success' panicked at
  crates/eggwork-server/tests/artifact_capture_disposition.rs:104:9:
assertion `left != right` failed: an execution whose declared outputs were not captured must not report success
  left: Succeeded
 right: Succeeded
---- a_platform_that_cannot_capture_refuses_a_declared_output stdout ----
thread 'a_platform_that_cannot_capture_refuses_a_declared_output' panicked at
  crates/eggwork-server/tests/artifact_capture_disposition.rs:140:5:
assertion `left != right` failed
  left: Succeeded
 right: Succeeded
test result: FAILED. 5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

```text
$ cargo test --locked -p eggwork-server --lib
test tests::a_declared_output_that_was_not_captured_never_reports_success ... FAILED
test tests::blob_protocol_streams_verifies_deduplicates_and_enforces_quota ... FAILED
---- tests::blob_protocol_streams_verifies_deduplicates_and_enforces_quota stdout ----
thread 'tests::blob_protocol_streams_verifies_deduplicates_and_enforces_quota' panicked at
  crates/eggwork-server/src/lib.rs:4055:9:
the terminal state event must report the failure: [ExecutionEvent { sequence: EventSequence(1), kind: State(Accepted), … },
ExecutionEvent { sequence: EventSequence(2), kind: State(Running), … },
ExecutionEvent { sequence: EventSequence(3), kind: Diagnostic { length: 166, value: "[REDACTED]" }, … },
ExecutionEvent { sequence: EventSequence(4), kind: State(Succeeded), … }]
test result: FAILED. 154 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

The second failure is the important one. The dumped event stream is the defect in
one place: the capture-failure **diagnostic** is published (sequence 3) and the
terminal event still says `State(Succeeded)` (sequence 4) — a successful execution
with a capture failure recorded and no error. The pre-existing integration assertion
— the one that had pinned the silent-success shape — fails against the corrected
behaviour, and the failure message names the missing terminal state event rather than
a symptom of it.

The patch was reverted immediately afterwards, and both suites were re-run green
(156 passed; 7 passed).

The other non-vacuity arguments:

- The revised happy-path assertion (`artifact_count == 1`,
  `finalization_failure == None`) gained `artifact_capture == Completed`, so a
  corrective that turned a successful capture into a failure would fail the test.
- The portable target asserts the *wire spellings* of the disposition, so a
  disposition that is renamed or re-spelled breaks it.
- The advertisement test asserts the capability is present **iff** the fact says the
  platform can capture, so restoring the unconditional advertisement fails it on a
  platform that cannot capture.

## Verification actually run

All commands were run on this Linux x86_64 host against the working tree as it stands
(commit `IMPLEMENTATION-COMMIT-PENDING`).

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | **passed** — no diff |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | **passed** — no warnings. One clippy finding was fixed during the work: `clippy::assertions_on_constants` on the payload-bound assertion, replaced by a compile-time `const _: () = assert!(…)` |
| `cargo check --workspace` | **passed** |
| `cargo test --locked -p eggwork-core --all-targets` | **passed** — 35 unit + 7 contract tests, 0 failed |
| `cargo test --locked -p eggwork-server --all-targets` | **passed** — 156 unit tests, plus `gc_preview_parity` (3), `host_telemetry` (12), `release_contract` (36), `shared_contract_ownership` (6), `artifact_capture_disposition` (7), 0 failed |
| `cargo test --locked -p eggwork-server --test artifact_capture_disposition` | **passed** — 7 passed, 0 failed |
| `cargo test --locked -p eggwork-server --features qualification --test installed_qualification` | **compiles**; 3 tests present, all `#[ignore]`d, 0 run. The qualification needs an installed daemon and `EGGWORK_QUALIFY_*`, so it **was not executed** |
| `cargo test --locked --workspace --all-targets` | **passed** — every suite green: client 13, core 35, `shared_protocol_contract` 7, runner 28 (+1 ignored), `capability_probe` 2, `diagnostic_degradation` 7, `environment_policy` 5, `environment_policy_parity` 12, `landlock_runner` 18, server 156, `artifact_capture_disposition` 7, `gc_preview_parity` 3, `host_telemetry` 12, `release_contract` 36, `shared_contract_ownership` 6. The `windows_process_tree` suite is `#![cfg(windows)]` and reported 0 tests on this host |
| `python3 scripts/check_execution_ownership.py` | **passed** — approved process owners unchanged: `eggwork-runner` (2 spawn sites), `eggwork-sandbox-helper` (1 spawn site) |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | **passed** — the synthetic forbidden spawn is detected |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | **passed** — 57 tests, OK |
| `git diff --check` | **passed** — no whitespace error |
| `cargo check --locked -p eggwork-core --target x86_64-pc-windows-msvc` | **passed** — the core crate, which carries the new disposition and the new diagnostic class, compiles for the Windows target |
| `cargo check --locked -p eggwork-core --target x86_64-apple-darwin` | **passed** — same, for the macOS target |
| `cargo check --locked -p eggwork-server --test artifact_capture_disposition --target x86_64-pc-windows-msvc` | **blocked by the environment, not by the change** — a transitive C dependency (`ring` / `aws-lc-sys`) needs MSVC's `lib.exe`, which is not installed on this Linux host. The failure happens in the dependency's build script, before any Eggwork code is compiled. This is the constraint `architecture/ci-guardrails.md` already records for a full `--workspace --target x86_64-pc-windows-msvc` check ("`cc-rs` needs MSVC's `lib.exe` for a transitive C dependency"). The Windows compile of the server crate and of the new target is the `windows-runner` CI job's evidence, and it has not run yet |

**Workspace-wide gate.** The coordinator runs `cargo test --locked --workspace --all-targets`
and the `tests/release` unittest suite once at the end of the batch. This milestone ran
both itself (results above) and records them; that is a second observation of the same
command, not a substitute for the coordinator's.

## Platform and feature matrix actually exercised

| Platform | What was exercised | Status |
| --- | --- | --- |
| **Linux x86_64** | The full unit and integration suites, including the capture path, the revised missing-required-output integration assertion, the diagnostic ordering, the advertisement, and the portable target. The platform fact is `Available` here, so the *capable* branch of the decision is the one the platform itself drives. | **Executed and green.** This is the only platform with executed evidence. |
| **Windows x86_64 (`x86_64-pc-windows-msvc`)** | `eggwork-core` compiles for the target. The server crate and the new portable target **could not be cross-checked** (the environment lacks the MSVC C toolchain). The `#[cfg(not(unix))]` arm of `open_file_beneath`, the `Unavailable` capability branch, the Windows advertisement, and the hosted qualification are **code inspection only**. | **Not qualified.** Compilation is not qualification. The Windows rows stay open until the `windows-runner` job and the hosted qualification run green. |
| **macOS (`x86_64-apple-darwin`)** | `eggwork-core` compiles for the target. The capture path is the Unix path, so the platform behaves like Linux for this feature, but **nothing was executed** for macOS. | **Not qualified.** No macOS host and no execution. |

## Compatibility review

**Wire.** `ExecutionResult` gained one `#[serde(default)]` field,
`artifact_capture`. This is wire-additive:

- a peer that predates the field **ignores** it, because `ExecutionResult` does not
  deny unknown fields — so an older client still decodes a terminal snapshot produced
  by a newer node;
- a peer that has the field decodes a snapshot written before it existed as
  `Unknown`, because the field is `#[serde(default)]` and the default is `Unknown`.

**The terminal snapshot is NOT byte-identical across this change.** Two things moved:
the result gained a field, and a capture that did not complete no longer reports
`Succeeded`. A consumer that diffs terminal snapshots byte-for-byte across the
upgrade will see a difference, and a consumer that reads `state` will see `Failed`
where it used to see `Succeeded` for a failed capture. This is the intended behaviour
change, not an accident.

**No stored format changes.** No blob, workspace, artifact, manifest, or journal row
format changed. `ExecutionSnapshot` still serializes as it did, with one more field
inside its result.

**No capability-string change.** The name `artifact.declared.v1` is unchanged. Only
its *advertisement* became conditional, which is the point of the milestone. No new
capability string was added, so `crates/eggwork-server/tests/release_contract.rs`
needed no change.

**No migration.** Nothing is rewritten on disk. A node upgraded with this change
reports the corrected disposition for new executions only; historical terminal
snapshots keep their existing shape and their `finalization_failure` marker, and
decode as `Unknown` in the new field.

**Client.** `crates/eggwork-client` returns `ExecutionSnapshot` from
`observe` / `observe_generation`, and `ExecutionSnapshot.result` is an
`eggwork_core::ExecutionResult`. The disposition is therefore already visible to a
client caller through the existing API: **no client API addition is required**, and
none was made.

**Source compatibility (out-of-tree).** `ExecutionResult` is a public struct with
public fields, so adding a field breaks an out-of-tree *constructor* of it (it is
compatible for readers). Inside the workspace there are exactly two construction
sites outside the milestone's declared file ownership, and both were updated with a
one-line placeholder (see *Deviations from the plan*).

## Documentation status

| Document | Change |
| --- | --- |
| `architecture/protocol.md` | The diagnostic class set now names `artifact.capture.failed`; the required-keys table gained the `requirement=declared` value for that class and the `outcome` key; the bound section states the `MAX_DIAGNOSTIC_PAYLOAD_BYTES` (2048) / `MAX_EVENT_CHUNK_BYTES` (65536) relationship. The truncation rule is unchanged and still documented. |
| `architecture/data-plane.md` | A new section, *The capture-failure contract and the platform claim*, documents the capability fact, the conditional advertisement, the four disposition states, the decision, the diagnostic, and the unchanged eligibility rule. The *Honest gaps* entry about the untested non-Unix branches was revised to say what is now covered and what is still not. Three rows were added to the *Invariants and enforcement* table. |
| `architecture/overview.md` | The divergences section gained **row 22** in a new *Platform limitations* subsection, stating the platform limitation truthfully and marking the silent-success defect closed. Row 12 (the Control-plane M004 divergence) was **not** touched. |
| `docs/operations.md` | The *Inspection and history* section now documents the `artifact_capture` field an operator sees on `executions show`, with the four states and what each means. |
| `docs/quickstart.md` | **Not changed.** The plan requires both documents to be checked and says which changed. The quickstart is a Linux walkthrough, and on Linux the operator-facing behaviour of a declared output is unchanged (it is captured and published); the quickstart never mentions artifacts, so there was nothing to correct there. The failing-capture behaviour is documented in `docs/operations.md` instead. |
| `README.md` | **Not changed — coordinator-owned.** The Windows caveat is silent on artifacts, and the plan requires it to state the disposition. See finding F-02 for the exact text. |
| `architecture/ci-guardrails.md` | **Not changed — coordinator-owned.** It reproduces the `windows-runner` job's YAML verbatim, so the reproduced block is now one step short. See finding F-03 for the exact text. |
| `plans/subsystems/workspace-artifact-transport-roadmap.md` | The M005a entry is now **implemented**, with a closure-record link and an *Implemented* list. The status line was updated. The M005b section was **not** touched. |

## Deviations from the plan

1. **Two files outside the milestone's declared ownership were edited.** Adding a field
   to `eggwork_core::ExecutionResult` — a public struct with public fields — breaks
   every struct-literal construction of it. There are exactly two such sites outside
   the milestone's ownership, and both had to be updated for the workspace to compile:

   - `crates/eggwork-runner/src/lib.rs` — `RunnerResult::execution_result()` now sets
     `artifact_capture: eggwork_core::ArtifactCaptureDisposition::Unknown`. The runner
     owns the process, not the capture; the server's finalization replaces this
     placeholder. The placeholder claims nothing, which is why it is `Unknown` and
     not `NotRequested`.
   - `crates/eggwork-server/src/store.rs` — `ExecutionStore::recover` now sets
     `artifact_capture: eggwork_core::ArtifactCaptureDisposition::Unknown`, because
     the recovery path cannot know what the interrupted execution declared.

   Each is a one-line addition with a comment. No other line of either file changed,
   and no behaviour of either file changed. The alternative — leaving the field out of
   `ExecutionResult` — would have failed acceptance criterion 3 outright.

2. **The disposition has a fourth state**, `Unknown`, beyond the three the plan named.
   The reasoning is in *Decisions taken where the plan left the choice open*: the
   `#[serde(default)]` default must not fabricate a claim, and the plan's own
   requirement is that a peer must not read an absence as a fact.

3. **The overview's new row is in a new *Platform limitations* subsection.** Rows 1-21
   are cited by number elsewhere in the repository, so inserting row 22 into an earlier
   table would renumber them. The new subsection keeps the numbering monotonic and the
   theme honest: the row is about a platform, not about a module.

4. **The qualification receipt path is derived, not taken verbatim.** The new
   qualification suite writes to `<receipt-stem>-declared-artifact.<ext>` rather than
   to the subject's receipt path, because the execution qualification writes to that
   path and two suites writing one file would leave whichever ran last as the only
   evidence.

## Unresolved findings by severity

| # | Severity | Finding | Owner |
| --- | --- | --- | --- |
| F-01 | **High** | The hosted Windows qualification step that would run `installed_release_reports_a_truthful_declared_artifact_disposition` does not exist. `.github/workflows/operational-qualification.yml` runs the qualification suites by `--exact` name, so the new suite is never invoked. The exact step text is in the report to the coordinator. Until it is added, acceptance criterion 8 is unmet. | Coordinator (workflow is not in this milestone's ownership) |
| F-02 | **High** | The README's Windows caveat is still silent on artifacts. Acceptance criterion 9 requires the README and the divergences table to say the same thing; the table says it, the README does not. Exact text in the report to the coordinator. | Coordinator (README is not in this milestone's ownership) |
| F-03 | Medium | `architecture/ci-guardrails.md` reproduces the `windows-runner` job's YAML verbatim; the reproduced block is now one step short of the checked-in workflow. Exact text in the report to the coordinator. | Coordinator (file is not in this milestone's ownership) |
| F-04 | Medium | The Windows *implementation* of a safe descriptor-relative open beneath the workspace root is still outstanding. This milestone makes the refusal truthful; it does not make the capture work on Windows. The plan's §5.5 permits the deferral only because the fail-closed contract landed in the same milestone — which it did. | Product/roadmap (a named follow-on milestone) |
| F-05 | Low | The legacy `finalization_failure` marker can still be overwritten by the retention marker when both the capture and the workspace retention fail. The first-class disposition is unaffected, but a consumer that reads *only* the legacy marker can miss the capture failure in that combination. | Product/roadmap |
| F-06 | Low | A legacy node's silent-success shape cannot be repaired by a new peer: an old snapshot decodes with `artifact_capture == Unknown`, and the caller must fall back to the legacy `finalization_failure` marker and the artifact count. The new disposition says "not reported", not "fine". | Product/roadmap |
| F-07 | Low | The crash-recovery path reports `Unknown` rather than a disposition, because it cannot know what the interrupted execution declared. The execution's terminal state is `Interrupted`, so no caller is told a success it did not get. | Product/roadmap |
| F-08 | Low | `ExecutionResult` is a public struct with public fields, so the new field is source-breaking for an out-of-tree *constructor* of it (compatible for readers). The two in-workspace constructors were updated in this milestone. | Release management (semver note for the next release) |
| F-09 | Info | The `windows-runner` CI job's new step and the hosted qualification step have not run on this branch, because the branch is not pushed. The Windows rows of the platform matrix stay open until they do. | Coordinator (push and read the job) |

## Registry and roadmap disposition

- **Roadmap** — `plans/subsystems/workspace-artifact-transport-roadmap.md` § M005a is
  now **implemented**, with a closure-record link and an *Implemented* list; the
  status line reads *"M001-M004 closed; M005a data-plane corrective implemented; M005b
  data-plane corrective implemented"*. The M005b section was not touched.
- **Registry** — `plans/registry.md` is coordinator-owned and was **not** edited. The
  exact text for it is in the report to the coordinator.
- **Closure record** — this file.

## Change set

| File | Change |
| --- | --- |
| `crates/eggwork-core/src/lib.rs` | `DIAGNOSTIC_CLASS_ARTIFACT_CAPTURE_FAILED`; `DIAGNOSTIC_CLASSES` becomes `[&str; 3]`; `ArtifactCaptureDisposition` (+ `Default`); `ExecutionResult.artifact_capture`; three new unit tests |
| `crates/eggwork-server/src/artifact.rs` | `ArtifactCaptureCapability` and `capture_capability`, stated next to the platform-specific `open_file_beneath` |
| `crates/eggwork-server/src/lib.rs` | The crate-root public surface (`ArtifactCaptureAttempt`, `ArtifactCaptureFacts`, `ArtifactCaptureDecision`, `decide_artifact_capture`, the two re-exports); the conditional `artifact.declared.v1` advertisement; the finalization's fail-closed capture handling; `publish_capture_diagnostic`; the `failed_result` placeholder; the revised integration assertion; six new unit tests |
| `crates/eggwork-server/src/store.rs` | The crash-recovery placeholder (one line) — outside the declared ownership, see *Deviations* |
| `crates/eggwork-runner/src/lib.rs` | The runner's placeholder (one line) — outside the declared ownership, see *Deviations* |
| `crates/eggwork-server/tests/artifact_capture_disposition.rs` | **New.** The portable integration target: 7 tests, no `/bin/sh`, no `std::os::unix` |
| `crates/eggwork-server/tests/installed_qualification.rs` | The declared-artifact qualification suite and its receipt-path helper |
| `architecture/protocol.md` | The diagnostic class set, the `requirement`/`outcome` keys, the payload-bound relationship |
| `architecture/data-plane.md` | The capture-failure contract and platform claim; the revised *Honest gaps* entry; three invariant rows |
| `architecture/overview.md` | Divergences row 22 in a new *Platform limitations* subsection |
| `docs/operations.md` | The `artifact_capture` field in *Inspection and history* |
| `.github/workflows/ci.yml` | The `windows-runner` step *declared-artifact capture disposition suite*, and the job's scope comment updated to say what is now covered |
| `plans/subsystems/workspace-artifact-transport-roadmap.md` | The M005a entry (status, closure link, *Implemented* list) and the status line |
| `plans/closure/workspace-artifact-transport/005a-status.md` | **New.** This record |
