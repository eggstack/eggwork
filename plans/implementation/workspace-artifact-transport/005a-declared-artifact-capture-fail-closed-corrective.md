# Workspace and Artifact Transport M005a — Declared-artifact capture fails closed

Status: conditionally closed

Class: invariant

Closure record: `plans/closure/workspace-artifact-transport/005a-status.md`

Source authority:

- `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md`, finding 1.1 (WA-1);
- `plans/subsystems/workspace-artifact-transport-roadmap.md`, durable invariant 6 ("Declared outputs are the default artifact capture boundary");
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`;
- `plans/003-planning-process.md#verification-philosophy` and `#corrective-work`.

## 1. Objective

Make the declared-artifact failure mode honest.

An execution whose declared outputs could not be captured MUST NOT present as a successful
execution with missing declared artifacts. Today, on a platform where safe descriptor-relative
capture is unavailable, the caller receives a successful execution, zero artifacts, and a failure
marker buried inside the terminal snapshot. That is the highest-severity finding in the
workspace/artifact corrective, and this plan is the small, high-severity, always-correct part of
its fix.

The decision this plan must force is stated in section 4. The acceptable resolutions, in order of
preference, are:

(a) implement a safe open on the target platform so that capture actually works; or

(b) if the platform genuinely cannot support descriptor-relative safe capture, the execution must
terminate in a state that truthfully reports the failure instead of `Succeeded` with a buried
`finalization_failure` marker.

## 2. Baseline

**What the code does.** `crates/eggwork-server/src/artifact.rs` has two variants of
`open_file_beneath`. The unix variant opens the root and each path component descriptor-relatively
with `O_NOFOLLOW` (`rustix::fs::{Mode, OFlags, open, openat}`). The `#[cfg(not(unix))]` variant
returns `io::ErrorKind::Unsupported` with the message *"safe artifact capture is unavailable on this
platform"*.

`capture_blob` in the same module calls `open_file_beneath` and propagates the error with `?`.
`capture_declared` calls `capture_blob` inside its capture loop and propagates with `?`, so the
first failure aborts the entire capture: no artifact record is published and the returned count is
never produced.

`crates/eggwork-server/src/lib.rs` (`run_execution`) performs the capture in the
execution-finalization path and matches on the result. `Ok(count)` sets
`execution_result.artifact_count`. `Err(_)` sets
`execution_result.finalization_failure = Some(ExecutionFinalizationFailure::ArtifactCapture)` and
changes nothing else: the terminal `ExecutionState` remains `Succeeded`, the `exit_code` remains
the target's, and `artifact_count` remains `0`. The terminal event published immediately afterwards
is `ExecutionEventKind::State(terminal)`; `eggwork-core`'s `ExecutionEventKind` has variants
`State`, `Stdout`, `Stderr`, and `Diagnostic`, and no artifact-specific variant, so the failure
never appears in the event stream.

**The behaviour is asserted, not merely tolerated.** The integration test in
`crates/eggwork-server/src/lib.rs` (`blob_protocol_streams_verifies_deduplicates_and_enforces_quota`)
asserts the whole combination: `state == ExecutionState::Succeeded`, `exit_code == Some(0)`,
`artifact_count == 0`, and
`finalization_failure == Some(ExecutionFinalizationFailure::ArtifactCapture)`. That assertion must
be changed by this plan; it currently locks the defect in place.

**What the platform means.** `x86_64-pc-windows-msvc` is a published release target (`README.md`,
"Installing a published release"). The `#[cfg(not(unix))]` branch is therefore the live behaviour
of a shipped platform, not a hypothetical path.

**The node advertises the capability it cannot deliver.** `static_capability_features` in
`crates/eggwork-server/src/lib.rs` returns a single static list that includes
`"artifact.declared.v1"`, `"artifact.stream.v1"`, and `"artifact.retention.v1"` on every
platform, with no platform condition. A Windows node therefore advertises declared-artifact
capture and then refuses the capture. That contradicts the repository's own stated capability
rule — "Capability advertisement reflects a runtime probe, not compile target or configuration
intent" (`plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md`) —
and it is the same defect seen from the other side: a static claim where a runtime fact is
required.

**What the documentation says.** `architecture/data-plane.md` describes the mechanism accurately:
its error-model section states that `capture_declared` has no partial-success result, that any
variant aborts the whole capture, and that the caller records
`ExecutionFinalizationFailure::ArtifactCapture` "while leaving the execution's own exit state
intact"; its "Honest gaps" section states that no test exercises the non-unix branches, naming
`open_file_beneath` returning `ErrorKind::Unsupported`. The design record is honest about the
mechanism and silent about the judgment. The `architecture/overview.md` divergences table records
row 18 (artifact GC does not release blob references) and has **no row** for this defect. The
README's Windows caveat covers isolation, resource enforcement, and service management, and says
nothing about artifacts — so a reader of the README would reasonably expect declared artifacts to
work on a Windows node.

**Correction to the review note.** The review note states that the finalization path "catches the
capture error and sets `finalization_failure` ... while leaving state `Succeeded` and artifact
count zero". That is accurate. One refinement matters for scoping: the failure is not limited to
the platform-unsupported case. The same code path produces `Succeeded` + zero artifacts for a
missing required output, a symlinked output, a non-regular file, and a capture-budget exhaustion,
because `capture_declared` is all-or-nothing and the finalization records the failure without
changing the state. The plan must decide its scope over that whole class explicitly (section 4),
not only over the platform case.

## 3. Scope

In scope:

- the outcome of a declared-artifact capture, from `capture_declared` through the terminal
  execution result, the terminal event, and the client's view of the execution;
- a first-class, platform-independent statement of whether safe descriptor-relative capture is
  available on the running platform, so that "the platform cannot do this" is a runtime fact
  rather than an inference from a `cfg`;
- the terminal state, terminal event, and terminal result when declared outputs were requested and
  the capture did not complete;
- the README's Windows caveat, the `architecture/overview.md` divergences table, and
  `architecture/data-plane.md`;
- the tests and the CI change required to make the corrected behaviour provable on a non-Unix
  target.

Out of scope:

- implementing descriptor-relative safe capture on Windows. That is a substantial, separately
  qualified platform change; see section 4 for the conditions under which it may be deferred.
- the workspace, artifact, blob, and manifest *contents* of a capture. This plan does not change
  what is captured, only what is reported when capture cannot complete.
- the retention and GC behaviour of artifacts and their blob references.
- every other finding in the corrective addendum. WA-2, WA-3, and WA-4 belong to M005b.

## 4. Invariants and non-goals

### 4.1 The invariant this plan establishes

**An execution whose declared outputs could not be captured MUST NOT present as a successful
execution with missing declared artifacts.**

Three consequences follow, and all three are required:

1. **The terminal state is truthful.** If declared outputs were requested and the capture did not
   complete, the terminal `ExecutionState` is not `Succeeded`. The plan must select the truthful
   state from the existing vocabulary in `eggwork-core` (`ExecutionState::Failed` with an
   appropriate `ExecutionFailure`, or another state the plan justifies) and must not invent a
   state outside it without an ADR.
2. **The failure is visible on the terminal state, in the event stream, and to the caller.** Not
   only as a nested marker inside the terminal snapshot. The plan must ensure the event stream
   carries the failure before the terminal state event, so that a caller that consumes the stream
   rather than the final snapshot is not left with a bare `Succeeded`.

   `ExecutionEventKind::Diagnostic` is the existing carrier in the protocol — `eggwork-core`
   defines it and the journal bounds its payload by `MAX_EVENT_CHUNK_BYTES` — but it currently has
   **no production publisher**: every use of the variant in the repository is in test code. The
   plan must therefore either create a production publisher for it, or introduce a dedicated event
   kind. Creating a publisher is the smaller change, but it must be coordinated with the Security
   M005a degraded-isolation observability corrective, which needs the same publisher for a
   different failure. Two correctives must not grow two divergent publishers for the same event
   kind. Adding a new `ExecutionEventKind` variant is a protocol change and needs an ADR.
   Whatever the choice, the published payload must be bounded.
3. **The artifact-count / declared-output-count relationship is explicit.** The caller must be able
   to tell "no declared outputs" from "declared outputs that vanished" without guessing. The plan
   must introduce a first-class, typed artifact-capture disposition on the terminal result that
   distinguishes at least: not requested, completed, and failed. The caller must not have to
   infer the outcome from `artifact_count == 0` plus knowledge of an optional field.

### 4.2 Scope of the invariant over capture failures

The plan must decide, and record in its closure record, whether the invariant in 4.1 applies to
**every** capture failure or only to the platform-unsupported case.

The recommended decision is to apply it to **every** capture failure — missing required output,
symlinked or non-regular output, capture-budget exhaustion, I/O failure, and platform
unsupported — because:

- the invariant as written is about the *class* "declared outputs could not be captured", and a
  platform-only carve-out would leave the same silent-success shape reachable on unix hosts
  (for example a required output that disappeared between discovery and capture);
- one rule is smaller and cheaper to implement and to test than a rule plus an exception;
- the existing all-or-nothing semantics of `capture_declared` mean there is no partial artifact
  set to be truthful about in any of these cases.

The plan may narrow this to the platform case **only** if it records the remainder as an open
finding with a named owner and a follow-on milestone in the same commit. It must not narrow it
silently.

### 4.3 Non-goals

This plan does not:

- change the capture boundary, the declared-output vocabulary, `MAX_OUTPUTS`,
  `MAX_ARTIFACT_FILES`, `MAX_ARTIFACT_TOTAL_BYTES`, `MAX_BLOB_BYTES`, or any other bound;
- change what is captured, how bytes are hashed, or the double hash in
  `crates/eggwork-server/src/artifact.rs` (the capture hash plus the blob store's re-hash on
  write). That double hash is documented and accepted; it is not to be removed here;
- change the symlink policy, the `NOFOLLOW` chain on unix, or the refusal to substitute a
  path-based open for a descriptor-relative one;
- weaken authentication, authorization, lease renewal, generation fencing, or the digest bounds;
- introduce a queue, a worker selection policy, a priority or fairness rule, a DAG, or a semantic
  retry. Eggwork remains a fixed-target, scheduler-free execution fabric;
- change the `eggworkd` command surface, the route set, or the `NodeCapabilities` feature set
  beyond the conditional advertisement that section 5.1 requires. Removing or renaming a
  capability string is out of scope; making `"artifact.declared.v1"` conditional on the runtime
  capability fact is in scope;
- remove or weaken `ExecutionFinalizationFailure::ArtifactCapture` as a field. The existing
  marker may remain, and the `Retention` variant's current behaviour is out of this plan's scope;
  what must change is that the `ArtifactCapture` variant can no longer be the *only* signal, and
  can no longer accompany a `Succeeded` state.

## 5. Required production changes

The plan must implement, at minimum, the following. Exact symbol names and file placement are the
implementing agent's choice within these constraints.

### 5.1 A runtime capture-capability fact

The server must be able to state, at runtime, whether safe descriptor-relative capture is
available on the running platform. Today that fact exists only implicitly, as the presence of a
`#[cfg(not(unix))]` branch.

- The fact must be **runtime-probed or compile-time-exact and publicly reachable**, not inferred
  by a caller from a `cfg` of its own.
- It must be reachable from outside `eggwork-server`'s crate-private modules, because the
  crate's data-plane modules have no `pub use` re-export and an external test cannot name them.
  The natural home is a small public function or type in `eggwork-server`, or a type in
  `eggwork-core` (which is already the home of the wire types).
- The plan must justify the placement and record the compatibility consequence. `eggwork-server`
  and `eggwork-core` are published workspace crates, so a public API addition is release-visible;
  an addition is compatible, a removal is not.
- **The advertised capability must follow the fact.** `static_capability_features` in
  `crates/eggwork-server/src/lib.rs` currently advertises `"artifact.declared.v1"` on every
  platform. A node that cannot capture must not advertise that it can. The plan must decide
  whether the declared-artifact capability string becomes conditional on the runtime fact, and
  record the decision and its consequence: a caller that reads the capability list must be able to
  tell that a node will refuse declared outputs before it submits one. The plan must also state
  what a caller sees when the capability is absent — whether the admission gate rejects a request
  that declares outputs, or accepts it and then fails the execution. Both are defensible; the
  plan must choose one and justify it, because "the node advertised a capability it does not
  have" and "the node advertised nothing and the caller learned only at execution time" are
  different products.

### 5.2 A typed artifact-capture disposition on the terminal result

- `ExecutionResult` in `crates/eggwork-core/src/lib.rs` must carry a first-class artifact-capture
  disposition with at least the three states named in 4.1(3): not requested, completed, failed.
- The field must be `#[serde(default)]` so that a peer running an older build still decodes a
  terminal snapshot produced by a newer node.
- The plan must record that the terminal snapshot is **not byte-identical** across this change.
  The existing `finalization_failure: Option<ExecutionFinalizationFailure>` field stays; it becomes
  a secondary marker rather than the only signal.
- The disposition must distinguish "no declared outputs were requested" from "declared outputs
  were requested and the capture did not complete". A caller must never have to compare
  `artifact_count` against a declared-output count it does not have.

### 5.3 A truthful terminal state and a visible event

- `crates/eggwork-server/src/lib.rs` (`run_execution`) must map a failed capture to a terminal
  state that is not `Succeeded`, and must publish the failure in the event stream before the
  terminal state event.
- The mapping must be a single, testable decision. The plan should put it in one place rather
  than inline it in the finalization match arms, so that a portable test can reach it.
- The plan must decide whether the capture failure changes the terminal `ExecutionState` alone or
  also the reported `ExecutionFailure`, and must record the choice. A caller must be able to
  distinguish "the target failed" from "the target succeeded and its declared outputs could not be
  collected".
- The event-stream half of this requirement needs a **production publisher** for the chosen event
  kind. `ExecutionEventKind::Diagnostic` has none today — every use of the variant in the
  repository is in test code. The plan must either add the publisher here, or record an explicit
  dependency on the Security M005a degraded-isolation observability corrective
  (`plans/implementation/security-isolation-resource/005a-degraded-isolation-observability-corrective.md`),
  which needs the same publisher. Two correctives must not grow two divergent publishers for the
  same event kind; the server's event-publishing path must be the single place either one of them
  extends. The published payload must be bounded by `MAX_EVENT_CHUNK_BYTES`, which
  `ExecutionEvent::validate` enforces for `Diagnostic`.

### 5.4 The platform claim matches the code

Whichever resolution the plan chooses, exactly one of the following must be true and documented:

- **Resolution (a)** — safe capture works on the target platform. Then the platform's capability
  fact reports capture as available, and the platform claim states that declared artifacts are
  captured and published there.
- **Resolution (b)** — the platform cannot support safe capture. Then the platform's capability
  fact reports capture as unavailable, an execution that declares outputs terminates in the
  truthful non-success state, and the platform claim states that declared outputs are refused on
  that platform rather than silently dropped.

In both cases the README's Windows caveat and the `architecture/overview.md` divergences table must
say the same thing, and the divergences table must gain a row for this defect so that a reader of
`architecture/overview.md` sees it before reading the code.

### 5.5 Deferral condition

The full Windows safe-open implementation may be deferred to a follow-on milestone **only if the
fail-closed contract of 4.1 and 5.2 and 5.3 is implemented in this same milestone**. A plan that
defers the Windows implementation *and* leaves the silent-success shape in place is not
acceptable and is not a smaller version of this plan — it is the same defect.

The plan must record, in its closure record, which resolution it took and, if (b), that the
Windows implementation is outstanding with a named follow-on.

## 6. Failure, cancellation, restart, and contention semantics

- **Cancellation and timeout.** The capture eligibility rule today is: the runner completed, the
  declared-output list is non-empty, the terminal state is `Succeeded` or `Failed`, and the
  failure is not `OutputLimit`. A cancelled, timed-out, or interrupted execution captures nothing
  today, and it must continue to capture nothing. The plan must confirm this rule is unchanged and
  must not widen it.
- **`OutputLimit`.** An execution killed for output limit is deliberately excluded from capture.
  The plan must not change that.
- **A failed target.** An execution whose target failed is still eligible for capture today.
  Making a *capture* failure change the terminal state does not change the *target's* failure:
  the plan must decide and record how a failed target that also failed to capture reports, so
  that the capture failure does not overwrite the target's own failure.
- **Restart.** The capture runs after the runner has terminated and before the terminal event is
  published. A crash before the terminal event leaves the execution non-terminal, as today. The
  plan must not introduce a durable side effect that a restart could observe as a partial capture.
  `capture_declared` already publishes records only after every blob reference has been retained,
  so a crash retains extra bytes until expiry rather than losing live data; the plan must not
  disturb that ordering.
- **Contention.** The capture path is not concurrent with another capture for the same execution
  under the current single-finalizer model. The plan must not introduce a second capture path.
- **No new failure surface.** The plan must not make the artifact path a place where a caller can
  provoke a new error class. A capture failure caused by the caller's own declared outputs (a
  missing required output, a symlink) must remain a bounded, typed outcome.

## 7. Compatibility and migration

- **Wire.** Adding a `#[serde(default)]` field to `ExecutionResult` in `eggwork-core` is a
  wire-additive change. The plan must state that the terminal snapshot is not byte-identical
  across the change, and that an older client ignores the new field. The client
  (`crates/eggwork-client`) should surface the disposition to its own caller; the plan must
  confirm whether that requires a client API addition and, if so, record it.
- **No stored format changes.** No blob, workspace, artifact, manifest, or journal row format
  changes.
- **No capability-string change**, unless the plan adds a capture-capability feature string, in
  which case it must follow the repository's rule that a capability string reflects a runtime
  probe and not a compile target, and it must be added to
  `crates/eggwork-server/tests/release_contract.rs` if that file enumerates capability strings.
- **No migration.** Nothing is rewritten on disk. A node upgraded with this change reports the
  corrected disposition for new executions only; historical terminal snapshots keep their existing
  shape, and `finalization_failure` continues to decode.

## 8. Required tests

The plan must add the following coverage. Every test must be non-vacuous: it must fail if the
corrective is reverted, and the closure record must say how non-vacuity was established.

### 8.1 A platform-independent contract test (runs in the ordinary PR gate)

A new **portable** integration test target in `crates/eggwork-server/tests/` (name to be chosen)
that asserts:

- the artifact-capture disposition has the required states and serializes and deserializes
  correctly;
- a capture outcome of "failed" never maps to terminal state `Succeeded`;
- "not requested" and "failed" are distinguishable from each other, and both are distinguishable
  from "completed with zero artifacts";
- the terminal result and the terminal event agree about the failure.

This test must not spawn `/bin/sh` and must not use `std::os::unix`, so that it compiles for
every target. It must reach the decision through the public surface established in 5.1-5.3.

### 8.2 The platform-capability test

A test that asserts the runtime capture-capability fact reports the *correct* value for the
platform it is running on, and that the fail-closed disposition is produced when capture is
requested but the platform reports it unavailable. On a non-unix target this test is the direct
proof that the platform is handled honestly rather than silently.

### 8.3 The corrected integration test

The existing assertion in `crates/eggwork-server/src/lib.rs`
(`blob_protocol_streams_verifies_deduplicates_and_enforces_quota`) that
`state == ExecutionState::Succeeded` with `artifact_count == 0` and
`finalization_failure == Some(ExecutionFinalizationFailure::ArtifactCapture)` must be replaced
with an assertion of the corrected disposition. The replacement must distinguish:

- an execution with **no** declared outputs (terminal success, no capture attempted, disposition
  not-requested); and
- an execution **with** declared outputs whose capture did not complete (terminal state not
  `Succeeded`, disposition failed, the failure visible in the event stream).

The second case must not be produced by making a unix-only test pretend to be non-unix. It must
be driven through whatever seam the plan establishes (an injected capture outcome, or the real
platform stub when the plan runs on a non-unix host), and the closure record must say which.

### 8.4 The platform-level evidence

A test that proves the disposition on a **real** non-Unix host, driving the installed daemon, not
a simulated one. See section 9 for how this is run and what it costs.

### 8.5 Regression guards

- The existing unix capture tests
  (`descriptor_relative_capture_rejects_parent_swapped_to_symlink`,
  `missing_required_and_symlink_outputs_fail_without_records`,
  `disappearing_output_and_storage_quota_fail_without_publishing_records`) must remain green and
  must keep their original meaning.
- The existing happy-path integration assertion (`artifact_count == 1` with
  `finalization_failure == None`) must remain green: the corrective must not turn a successful
  capture into a failure.

## 9. How the non-Unix test is actually run

This is the part of the plan that most needs to be honest, because the repository's Windows CI job
does not currently build the crate under test.

**The constraint.** `.github/workflows/ci.yml` job `windows-runner` runs
`cargo clippy --locked --workspace --lib --bins --all-features -- -D warnings` and
`cargo test --locked -p eggwork-runner --all-targets -- --nocapture`. Its comment states the
scope explicitly: *"Test targets only for the crate this milestone owns. `eggwork-server` and
`eggwork-client` unit tests are Linux-shaped (they spawn `/bin/sh` and set `0700` modes through
`std::os::unix`), so they do not compile for Windows and are not claimed as coverage. Making them
portable is its own milestone; until then this job's scope is stated, not widened to hide the
gap."*

Two consequences follow. First, the crate's **library** does compile for Windows — the existing
`--lib --bins` clippy step proves it. Second, the crate's **`#[cfg(test)]` unit-test harness** does
not, so `cargo test -p eggwork-server --lib` cannot run on Windows today.

**The options, and their trade-offs.**

- **Option A — extend the Windows job to `cargo test --locked -p eggwork-server --lib`.**
  *Trade-off:* this requires the crate's entire unit-test harness to become portable, which the
  CI comment explicitly calls a separate milestone. M005a must not absorb that work, and must not
  block on it. **Rejected as a precondition for M005a.**
- **Option B — add a portable integration test target and run only that target on Windows.**
  `cargo test --locked -p eggwork-server --test <name>` compiles the library (which already
  compiles for Windows) and that one integration test target; it does **not** compile the crate's
  unit-test harness, so the Linux-shaped unit tests are not in the way. The cost is that an
  integration test can only reach the crate's **public** surface, and the capture code is in
  crate-private modules with no `pub use` re-export. Option B therefore depends on section 5.1-5.3
  making the platform fact and the disposition decision publicly reachable — which the plan
  already requires for the contract to be testable at all.
  *Trade-off:* a small additive public API change to a published crate, and the Windows CI job
  grows one step. In exchange, the corrected behaviour is covered in the ordinary PR gate on
  every platform, not only at release-qualification time.
- **Option C — hosted qualification on `windows-latest`.**
  `.github/workflows/operational-qualification.yml` already runs
  `crates/eggwork-server/tests/installed_qualification.rs` against the **installed** daemon on
  `windows-latest`. That test file is already portable (`#[cfg(unix)]` guards) and its execution
  currently declares `declared_outputs: vec![]`, so it never exercises capture. The plan must
  extend it to declare an output and assert the truthful terminal disposition on the real Windows
  host.
  *Trade-off:* this evidence is workflow-level, tag-gated, and slow. A regression could merge
  green in `ci.yml` and be caught only at qualification time. **Option C alone is insufficient** —
  it is exactly the gap the CI job's comment refuses to paper over.
- **Option D — make the platform stub reachable from the portable test.** Instead of exposing the
  private capture module, expose the *platform fact* (section 5.1) and drive the fail-closed
  decision from it. The non-unix stub's refusal is then covered indirectly by a test that runs on
  the non-unix host and asserts that the platform reports capture unavailable and that the
  disposition follows.
  *Trade-off:* the stub's own error path is exercised only on a non-unix host, so the coverage of
  "`open_file_beneath` refuses" is host-dependent rather than universal.

**Required decision.** The plan must choose and record a combination. The recommended combination
is **Option B plus Option C plus Option D**: Option B gives the PR-gate coverage on every
platform, Option D makes the platform behaviour reachable from a portable test, and Option C gives
the real-Windows evidence against installed bytes. The plan must state the trade-off it accepts
and must not claim Windows coverage from a Linux run.

**CI change.** `.github/workflows/ci.yml` job `windows-runner` must gain a step that runs only the
new portable test target (for example
`cargo test --locked -p eggwork-server --test <name>`). The job's scope comment must be updated in
the same commit to say what is now covered, because the comment currently states the gap that
this step closes. The plan must keep the job's existing honesty: it must not widen the job's
claim beyond the targets it actually runs.

## 10. Required verification commands

The repository's pre-submit gate, in the repository's own spelling:

```text
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Additional, for this milestone specifically:

- the new portable test target, run on its own:
  `cargo test --locked -p eggwork-server --test <name>` — and, on a Windows host, the same
  command executed by the extended `windows-runner` job;
- the hosted Windows qualification, through
  `.github/workflows/operational-qualification.yml`, with the receipt recorded. A Linux run is not
  evidence for a Windows claim and must not be recorded as one.

`--locked` is load-bearing on `clippy` and `test`: a version bump that skips `Cargo.lock` passes
both and then fails every release build.

## 11. Documentation updates

- `README.md` — the Windows caveat must state the artifact disposition for a Windows node:
  either that declared artifacts are captured and published there, or that they are refused with a
  truthful terminal state. The current caveat covers isolation, resource enforcement, and service
  management and is silent on artifacts.
- `architecture/overview.md` — the divergences table must gain a row for this defect, with a link
  to the deep dive that establishes it. The table currently has row 18 (artifact GC does not
  release blob references) and no row for this.
- `architecture/data-plane.md` — its error-model section and its "Honest gaps" section must be
  updated to match the corrected code. The "Honest gaps" entry that names the untested non-unix
  branches must be revised to reflect the new evidence, and its stale line-number citations must
  be replaced with symbol references in the same pass.
- `docs/quickstart.md` and `docs/operations.md` — only if the operator-facing behaviour changes.
  The plan must check both and say which, if either, changed.
- `plans/closure/workspace-artifact-transport/005a-status.md` — the closure record itself.

## 12. Acceptance criteria

1. No execution can reach terminal state `Succeeded` while a declared output it requested was not
   captured.
2. The capture failure is visible on the terminal state, in the event stream, and through the
   client's view of the execution — not only as `finalization_failure`.
3. The terminal result distinguishes "no declared outputs" from "declared outputs that vanished"
   through a first-class typed disposition, with no arithmetic required of the caller.
4. The runtime capture-capability fact reports the correct value on every platform, and is
   reachable from outside the crate-private data-plane modules.
5. The advertised capability set agrees with the runtime fact: a node that cannot capture does not
   advertise `"artifact.declared.v1"`, and the plan records what a caller sees when the capability
   is absent.
6. The existing integration test that asserted `Succeeded` + `artifact_count == 0` +
   `finalization_failure == Some(ArtifactCapture)` has been replaced by an assertion of the
   corrected disposition, and the replacement is proven non-vacuous.
7. The new portable test target compiles and passes on a non-unix host, and is wired into the
   `windows-runner` CI job.
8. The Windows hosted qualification declares an output and asserts the truthful terminal
   disposition against the installed daemon, and the receipt is recorded.
9. The README's Windows caveat and the `architecture/overview.md` divergences table state the
   artifact disposition, and the statement matches the code.
10. No bound is widened: `MAX_OUTPUTS`, `MAX_ARTIFACT_FILES`, `MAX_ARTIFACT_TOTAL_BYTES`,
    `MAX_BLOB_BYTES`, and every digest bound are unchanged.
11. The unix capture tests keep their original meaning and stay green, and the happy-path
    integration assertion (`artifact_count == 1`, `finalization_failure == None`) is still green.
12. The double hash in `crates/eggwork-server/src/artifact.rs` is still present and documented as
    accepted.
13. No authentication, lease, generation, fencing, or digest-bound semantics weakened; no queue,
    worker selection, priority, fairness, DAG, or semantic retry introduced.
14. `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and
    `cargo test --locked --workspace --all-targets` are green on the same head.

## 13. Stop conditions

Stop and re-plan, rather than absorbing the work, if any of the following holds:

- making the terminal state truthful requires a change to `ExecutionState`, `ExecutionFailure`, or
  `ExecutionEventKind` that the existing vocabulary cannot express. That is an architecture
  question (`plans/003-planning-process.md#architecture-changes`) and needs an ADR or a canonical
  amendment first.
- the truthful disposition would require weakening the unix `NOFOLLOW` capture chain, the
  all-or-nothing record publication, or any bound.
- the platform capability fact cannot be made publicly reachable without a breaking change to a
  published crate's API. Additive is acceptable; removal is not, and the plan must escalate.
- the portable test target cannot be made to compile for a non-unix target without making the
  crate's whole unit-test harness portable. That portability work is a separate milestone; the
  plan must escalate rather than absorb it.
- the Windows hosted qualification cannot be run, or shows a disposition different from the one
  the code asserts on other platforms.
- the implementation discovers that the correct resolution is to *remove* the `#[cfg(not(unix))]`
  capture stub and refuse declared outputs at admission instead of after the target has run. That
  is a materially different design — it changes admission rather than finalization — and it crosses
  into the control-plane subsystem's ownership of admission. Escalate it.

## 14. Closure evidence required

Create:

- `plans/closure/workspace-artifact-transport/005a-status.md`.

The record must distinguish code inspection from executed evidence and must include:

- the reviewed head and the implementation commits;
- a requirement-to-evidence matrix, with one row per acceptance criterion in section 12;
- the exact commands run, from the list in section 10, with their results;
- the **platform matrix actually exercised**: which platforms ran the portable test target, which
  ran the hosted qualification, and which did not. "The code compiles for Windows" is not a
  platform claim, and a Linux run must not be recorded as Windows evidence;
- the resolution taken — (a) safe capture implemented, or (b) fail-closed contract implemented
  with the Windows implementation deferred — and, for (b), the named follow-on milestone;
- the non-vacuity evidence for each new test: how each test was shown to fail when the
  corrective is reverted;
- the negative evidence: the case where declared outputs are requested and the capture fails, and
  the case where no declared outputs are requested, shown to be distinguishable;
- the documentation status: the README caveat, the divergences-table row, and the
  `architecture/data-plane.md` updates, with the exact wording that was adopted;
- the compatibility review: the wire delta, the `#[serde(default)]` behaviour against an older
  peer, and confirmation that no stored format changed;
- unresolved findings by severity, with owners;
- the registry and roadmap disposition.

Do not claim a platform or feature is qualified merely because its code compiles.

## 15. Handoff notes

- **The decision this plan exists to force.** Section 5.4. The implementing agent must not treat
  "record the failure more visibly" as an acceptable substitute for choosing a resolution.
- **Start with the assertion.** The single most important edit is the existing integration test
  that asserts `Succeeded` + `artifact_count == 0`. Change it first, watch it fail, and let the
  corrected disposition be driven by that failure.
- **Do not implement the Windows safe open.** It is explicitly deferrable, and attempting it will
  consume the milestone. The deferral is legitimate only because the fail-closed contract lands
  in the same milestone.
- **The public-reachability requirement is not optional.** Sections 5.1 and 5.3 exist so that the
  decision can be tested from outside the crate's private modules. A decision that can only be
  reached from inside `artifact.rs` cannot be covered by a portable test target and therefore
  cannot be covered on a non-unix host in the PR gate.
- **Boundary discipline.** The data plane owns capture and the meaning of "capture completed".
  The server's finalization owns the terminal state. Do not move the terminal-state decision into
  `artifact.rs`, and do not let `artifact.rs` decide that an execution succeeded.
- **Do not touch the sibling findings.** WA-2, WA-3, and WA-4 are M005b. This plan must not
  change the GC candidate selection, the blob path layout, or the manifest owner-id composite.
- **Do not edit `plans/registry.md`** in this milestone; the registry is owned by the planning
  process, not by the implementing agent.
- **Do not edit `plans/000-` through `plans/003-`.** If the corrective appears to require a change
  to a canonical document, stop and escalate under
  `plans/003-planning-process.md#architecture-changes`.
