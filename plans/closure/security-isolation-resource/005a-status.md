# Security M005a — Degraded-isolation observability corrective

Status: conditionally closed — implementation commit `6f41957`; the outstanding evidence item is the Windows and macOS exercise of the runner's `OutputChunk` shape change, which the plan requires and this host cannot provide

Source implementation plan:
`plans/implementation/security-isolation-resource/005a-degraded-isolation-observability-corrective.md`

Source corrective addendum (authoritative where it disagrees with the plan):
`plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`
(finding SI-1)

Source roadmap:
`plans/subsystems/security-isolation-resource-roadmap.md` (M005a now
"implemented")

Reviewed repository head before implementation: `778d83c`
Implementation commit: `6f41957`

Branch: `corrective/interrogation-batch-2026-10`. The working tree was left
dirty for the coordinator to review and commit; no commit was made by the
implementing agent.

## 1. Executive finding

A degraded best-effort execution is now visible in the event stream *while it
is running*. The runner classifies the degradation at the moment it decides
not to enforce a request, and it publishes that classification on the output
channel it already owns — before the target process exists, and therefore
before the target can produce any output. The server forwards the chunk in
receipt order, so the diagnostic's event sequence number is strictly below the
first stdout/stderr event's and strictly below the terminal event's.

The mechanism is the runner-side one the plan expected, and it is the only
mechanism that satisfies the ordering requirement in full: the server's
`run_execution` learns the outcome from `RunnerResult`, which arrives after
the execution has finished, so a server-side diagnostic derived from that
result would necessarily arrive after all target output — and, because
`publish` returns early on a terminal base state, would be *silently dropped*
if it were published after the terminal event.

The `Required` path is untouched. A required request still fails closed before
the target starts, keeps its terminal classification, and is never described
as a degradation.

## 2. The emission mechanism, and why

**Chosen: the runner emits on its output channel; the server forwards.**

`crates/eggwork-runner/src/lib.rs`'s public `OutputChunk` gained a
`Diagnostic(String)` variant and became an enum:

```rust
pub enum OutputChunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Diagnostic(String),
}
```

`crates/eggwork-server/src/lib.rs`'s forwarding loop in `run_execution` maps
the chunk to `ExecutionEventKind::Diagnostic(payload)` and publishes it through
the existing `publish` funnel, with `ExecutionState::Running` as the proposed
state.

**Why this and not the server-side alternative.** Three reasons, in order of
weight:

1. **Ordering.** The server's `run_execution` learns the outcome only from
   `RunnerResult`, which arrives after the execution has finished. A
   diagnostic derived from it would land after every stdout/stderr event, and
   the milestone's ordering property would be unsatisfiable. The plan's
   §5.2 permits a server-side publication immediately before
   `publish_terminal` only as a fallback.
2. **The post-terminal drop.** `publish` returns early when the recorded base
   state is already terminal (`crates/eggwork-server/src/lib.rs`, `publish`),
   and `store::ExecutionStore::commit_event` refuses to append to a terminal
   execution. A diagnostic published after the terminal event is therefore
   *silently dropped* — not rejected, not reported. Any mechanism whose
   publication point can drift past the terminal event is unsafe by
   construction.
3. **Knowledge.** The runner is the only party that knows *why* an
   enforcement was not applied, and it knows it at the moment it decides not
   to apply it. The runner does not reconstruct the diagnostic from the
   terminal snapshot after the fact; the payload and the snapshot are derived
   from the same `SetupOutcome`, so the two cannot disagree.

**Compatibility consequence of the public API change.** `OutputChunk` is
public in a published crate (`eggwork-runner` 0.1.5). The struct
`OutputChunk { stderr: bool, bytes: Vec<u8> }` became the enum above, so
this is a **breaking source change** for any downstream code that constructs
or destructures `OutputChunk`. The in-tree blast radius was small: the only
two construction sites were the runner's own `spawn_reader` and one server
test helper, and both are in files this milestone owns. The release version
and release notes still have to carry this change; **the version bump is not
part of this milestone** (no `Cargo.toml` is in this milestone's ownership),
so the coordinator/release owner must fold the `OutputChunk` change into a
version-visible release rather than shipping it inside a
qualification-only candidate. See §9 finding S-03.

**Client behaviour.** The repository's own client tolerates the new traffic:
`ExecutionStream::into_events` in `crates/eggwork-client/src/lib.rs`
deserializes every line into an `ExecutionEvent` and yields every event kind
unchanged, and `ExecutionEvent::validate` bounds the diagnostic payload by
`MAX_EVENT_CHUNK_BYTES` (64 KiB), which the 2048-byte payload is well inside.
This is confirmed by execution, not only by inspection: the server tests in
§4 observe the `Diagnostic` events through that exact client path. The
**external**-client consequence must be flagged: a client that treats an
unknown event kind as fatal would break on the newly-trafficked `Diagnostic`
variant. That is the intended correction — the variant already existed in the
wire contract — but it is a client-visible behaviour change.

## 3. The diagnostic payload grammar as implemented

Defined once in `crates/eggwork-core/src/lib.rs` (`eggwork-core` is the only
crate both the runner and the server depend on, so the grammar cannot drift
between producer and consumer).

```text
eggwork.diagnostic.<class>.v1 key=value key=value …
```

| Element | Constant | Value |
| --- | --- | --- |
| prefix | `DIAGNOSTIC_PREFIX` | `eggwork.diagnostic.` |
| schema suffix | `DIAGNOSTIC_SCHEMA_SUFFIX` | `.v1` |
| class (isolation) | `DIAGNOSTIC_CLASS_ISOLATION_DEGRADED` | `isolation.degraded` |
| class (resources) | `DIAGNOSTIC_CLASS_RESOURCES_DEGRADED` | `resources.degraded` |
| closed class set | `DIAGNOSTIC_CLASSES` | the two classes above |
| bound | `MAX_DIAGNOSTIC_PAYLOAD_BYTES` | `MAX_METADATA_VALUE_BYTES` = 2048 |
| key bound | `MAX_DIAGNOSTIC_KEY_BYTES` | 32 |
| truncation marker | `DIAGNOSTIC_TRUNCATION_MARKER` | `...[truncated]` |

**Key set.** `class` (always; it repeats the class so a consumer that only
reads key/value pairs does not have to parse the prefix), `requirement`
(always `best_effort`), `reason` (the bounded cause), `profile`
(`isolation.degraded` only), `dimension` (`resources.degraded` only:
`memory`, `cpu`, `pids`).

**Value charset.** Values are normalised to printable ASCII: every character
outside `!`..=`~` — a space included — becomes `_`. The grammar separates
fields with spaces, so this keeps the tokenisation unambiguous without
inventing an escaping scheme a reader would have to know about. The reason
text is therefore mangled (spaces become underscores) in the diagnostic while
the terminal snapshot keeps the original sentence; the snapshot stays the
authoritative evidence.

**Bound and truncation rule.** `DiagnosticPayload::build` never returns more
than `MAX_DIAGNOSTIC_PAYLOAD_BYTES`. When the fields do not fit, values are
shortened *in insertion order*: the first value that does not fit is cut on a
UTF-8 character boundary and gains the literal `...[truncated]` marker, and
the fields after it are omitted. The marker is what makes truncation
observable — a bounded payload never silently reads as a complete statement.
The runner inserts the structured keys first and the free-form `reason` last,
so the reason absorbs the overage and the structured keys survive intact.

**Validator.** `DiagnosticPayload::parse` refuses a payload that does not
match the grammar, names a class outside the closed set, exceeds the bound, or
omits `requirement`/`reason`.

No class constant beyond the two named by the coordinator was added.

## 4. Requirement-to-evidence matrix

Acceptance criteria are numbered as in the plan's §11. "Executed" means the
command was run in this working tree and produced the stated result; "inspected"
means the property was established by reading code and is not backed by a
command run here.

| # | Criterion | Implementation site | Evidence | Status |
| --- | --- | --- | --- | --- |
| 1 | A best-effort request that could not be enforced produces a diagnostic event | `report_setup_degradations` in `crates/eggwork-runner/src/lib.rs`; forwarding in `run_execution` in `crates/eggwork-server/src/lib.rs` | Executed: `cargo test -p eggwork-server --lib a_degraded_best_effort_execution_publishes_its_diagnostic_before_output_and_terminal` — passed; `cargo test -p eggwork-runner --test diagnostic_degradation` — 7 passed | Pass |
| 2 | The diagnostic's sequence number is strictly less than the terminal event's | same | Executed: the same server test asserts `diagnostic_sequence < terminal_sequence` | Pass |
| 3 | The diagnostic's sequence number is strictly less than the first stdout/stderr event's | same | Executed: the same server test asserts `diagnostic_sequence < first_output_sequence` against a target that writes to stdout; runner-side `a_degraded_best_effort_sandbox_is_reported_before_the_target_output` asserts the diagnostic is the first chunk on the channel | Pass |
| 4 | The payload matches the grammar, names the class, is at most `MAX_METADATA_VALUE_BYTES` | `eggwork_core::DiagnosticPayload` in `crates/eggwork-core/src/lib.rs` | Executed: `cargo test -p eggwork-core --lib` — 32 passed, including `a_diagnostic_payload_matches_the_documented_grammar`, `an_over_long_diagnostic_reason_is_truncated_observably`, `a_diagnostic_payload_stays_within_its_bound`, `a_malformed_diagnostic_payload_is_refused`; runner-side `a_diagnostic_payload_is_bounded` | Pass |
| 5 | A `Required` request still fails closed before the target starts, proven on the target's side effect | unchanged `Required` branches in `NoExecutionSetup::prepare`, `TrustedLandlockSetup::prepare`, `SandboxChannel::wait` | Executed: runner-side `a_required_sandbox_is_refused_without_a_degradation_diagnostic` asserts the marker file does not exist; server-side `a_required_sandbox_never_reaches_the_target_and_is_not_reported_as_a_degradation` asserts the marker file does not exist | Pass |
| 6 | A `Required` request's terminal classification is unchanged | `failure_for_runner_error` and `failed_result` in `crates/eggwork-server/src/lib.rs` (not modified) | Executed: server-side `a_required_sandbox_never_reaches_the_target_and_is_not_reported_as_a_degradation` asserts `ExecutionState::Failed`, `ExecutionFailure::Internal`, and `SandboxResult::Failed { .. }` — the baseline classification | Pass |
| 7 | An execution with nothing degraded emits no diagnostic | `report_setup_degradations` only reports `BestEffort` requests and only for outcomes it classified | Executed: runner-side `an_execution_with_nothing_degraded_emits_no_diagnostic` and `an_execution_cancelled_before_spawn_emits_no_diagnostic`; server-side `an_execution_with_nothing_degraded_publishes_no_diagnostic` | Pass |
| 8 | The diagnostic is durable and appears in a replayed event page | `publish` → `store.commit_event` in `crates/eggwork-server/src/lib.rs` (the existing path, unchanged) | Executed: server-side `the_diagnostic_is_replayed_from_the_journal_after_the_execution_ended` reads `GET /v1/executions/{id}/events` after the execution finished and finds the payload | Pass |
| 9 | No wire representation changed; `ExecutionEventKind` keeps its shape | `crates/eggwork-core/src/lib.rs` — `ExecutionEventKind` untouched | Executed: `git diff` shows no change to `ExecutionEventKind`, `ExecutionResult`, `SandboxResult` or `ResourceResult`; `cargo check --workspace --all-targets` passes | Pass |
| 10 | The server creates no child process on any path added by this milestone | `crates/eggwork-server/src/lib.rs` — the added path only maps a chunk to an event kind | Executed: `python3 scripts/check_execution_ownership.py` — passed ("approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)") | Pass |
| 11 | The runner's public API change is enumerated and reflected in the release version and notes | §2 above enumerates it | **Partially open.** The change is enumerated here and in the architecture docs. The *version bump* is not done: no `Cargo.toml` is in this milestone's ownership. The coordinator/release owner must make it version-visible. See §9 finding S-03 | Open (coordinator action) |
| 12 | The four deep dives match the code | `architecture/protocol.md`, `architecture/runner-execution.md`, `architecture/server-node.md`, `architecture/sandbox-helper.md` | Inspected: each edit was written against the code as it now stands, using symbol references rather than line numbers | Pass (inspection) |
| 13 | The full pre-submit gate is green on the reviewed head | — | **Not executed here.** The scoped gate was run (§5); the full workspace gate is the coordinator's, and this milestone deliberately did not run `cargo fmt --all` or `cargo clippy --workspace` while other agents work in the same tree | Open (coordinator action) |

## 5. Verification actually executed

Host: Linux x86_64, kernel `6.8.0-147-generic`; rustc `1.89.0` (pinned by
`rust-toolchain.toml`). Everything below was run in this working tree; the
outcomes are quoted, not summarised from memory.

| Command | Outcome |
| --- | --- |
| `cargo test -p eggwork-core --lib` | **32 passed, 0 failed** |
| `cargo test -p eggwork-runner --lib` | **18 passed, 0 failed, 1 ignored** |
| `cargo test -p eggwork-runner --test diagnostic_degradation` | **7 passed, 0 failed** |
| `cargo test -p eggwork-runner --test capability_probe` | **2 passed, 0 failed** |
| `cargo test -p eggwork-server --lib` | **133 passed, 0 failed** (128 pre-existing + 5 new) |
| `cargo fmt -p eggwork-core -p eggwork-runner -p eggwork-server` | applied; `git diff --check` clean afterwards |
| `cargo clippy -p eggwork-core -p eggwork-runner -p eggwork-server --all-targets -- -D warnings` | **no issues** |
| `cargo check --workspace --all-targets` | **passed** (compiles every crate and test target, including `eggwork-client` and the release-contract suites) |
| `cargo check -p eggwork-core -p eggwork-runner --target x86_64-pc-windows-msvc --all-targets` | **passed** |
| `cargo clippy --locked -p eggwork-runner --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings` | **passed** — this is the same command the `windows-runner` CI job runs for the runner's test targets |
| `cargo clippy --locked -p eggwork-runner --target x86_64-apple-darwin --all-targets --all-features -- -D warnings` | **failed on a pre-existing defect, not on this change** — `method 'start_kill' is never used` at `crates/eggwork-runner/src/process_tree.rs:142`. `ProcessTree::start_kill` is called only from the Linux-only helper-handshake paths (the `#[cfg(target_os = "linux")]` block in `LocalProcessRunner::run`), so it is dead code on macOS. `process_tree.rs` is not in this milestone's ownership and was not modified; the same dead-code error is present at the reviewed head `778d83c` (the call sites are inside the same `cfg`-gated block there). See §9 finding S-07. |
| `cargo check -p eggwork-server --lib --bins --target x86_64-pc-windows-msvc` | **blocked by the environment, not by the change** — the `aws-lc-sys` build script needs a Windows C toolchain (`cl.exe`), which is not installed on this Linux host. The failure happens in a third-party dependency's build script, before any Eggwork code is compiled. The server's Windows compilation is covered by the `windows-runner` CI job. |
| `python3 scripts/check_execution_ownership.py` | **passed** — "approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)" |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | **passed** — "synthetic forbidden spawn detected" |
| `git diff --check` | **clean** |

**Not executed in this milestone** (coordinator's full gate, and deliberately
not run here because other agents are working in the same tree):
`cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets
--all-features -- -D warnings`, `cargo test --locked --workspace --all-targets`,
`cargo check --workspace`, and
`python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`.

## 6. Negative and adversarial evidence

### 6.1 The ordering assertions fail against a deliberately reordered implementation

The implementation was temporarily modified so that the degradation diagnostic
was emitted at the **end** of the run instead of at setup time — i.e. after
the target had produced its output, which is the shape a
`RunnerResult`-derived diagnostic would have. The change was reverted
immediately afterwards and the tree re-verified.

Runner suite against the reordered implementation:

```text
test a_degraded_best_effort_sandbox_is_reported_before_the_target_output ... FAILED
---- a_degraded_best_effort_sandbox_is_reported_before_the_target_output stdout ----
panicked at crates/eggwork-runner/tests/diagnostic_degradation.rs:96:5:
assertion `left == right` failed: the diagnostic has to precede the target's first output;
got [OutputChunk { stream: "stdout", length: 14, bytes: "[REDACTED]" },
     OutputChunk { stream: "diagnostic", length: 150, payload: "[REDACTED]" }]
  left: 0
 right: 1
test result: FAILED. 6 passed; 1 failed
```

Server suite against the reordered implementation:

```text
test tests::a_degraded_best_effort_execution_publishes_its_diagnostic_before_output_and_terminal ... FAILED
---- stdout ----
panicked at crates/eggwork-server/src/lib.rs:5467:9:
the diagnostic has to precede the target's first output; got [
  ExecutionEvent { sequence: EventSequence(1), kind: State(Accepted), … },
  ExecutionEvent { sequence: EventSequence(2), kind: State(Running), … },
  ExecutionEvent { sequence: EventSequence(3), kind: Stdout { length: 14, … }, … },
  ExecutionEvent { sequence: EventSequence(4), kind: Diagnostic { length: 150, … }, … },
  ExecutionEvent { sequence: EventSequence(5), kind: State(Succeeded), … } ]
test result: FAILED. 0 passed; 1 failed; 132 filtered out
```

Both assertions pass again after the reorder was reverted (7 passed / 133
passed, §5).

**What this proves, and what it does not.** The reordered run still placed the
diagnostic *before* the terminal event (sequence 4 < sequence 5), so the
"before the terminal event" assertion alone does **not** discriminate. The
discriminating assertion is the one against the *first target output*. That
is the assertion this milestone had to get right, and it is the one the
negative run shows failing when the emission point moves.

### 6.2 The `Required` side-effect assertion

`a_required_sandbox_is_refused_without_a_degradation_diagnostic` (runner) and
`a_required_sandbox_never_reaches_the_target_and_is_not_reported_as_a_degradation`
(server) assert that the target's marker file **does not exist** — the
observable side effect, not merely the terminal state — and that no diagnostic
was published. Both pass (§5).

### 6.3 Non-vacuity controls

- The server tests inject the runner explicitly
  (`LocalProcessRunner::new(NoExecutionSetup)` and, for the required case,
  the `AdvertisesLandlockButCannotEnforce` fixture). `LocalProcessRunner::default()`
  is never used, because on Linux it discovers a sibling helper and the
  behaviour would be environment-dependent.
- The "nothing degraded" and "cancelled before spawn" tests exist precisely so
  that a diagnostic emitted on *every* execution would fail them.
- The core tests pin the truncation marker and the bound, so a payload that
  silently dropped an over-long reason would fail.

### 6.4 Cancellation, restart, contention, and the security review

| Case | Expected (plan §6) | Status |
| --- | --- | --- |
| Cancellation **before** spawn | no diagnostic — a cancellation is not a degradation | Executed: runner-side `an_execution_cancelled_before_spawn_emits_no_diagnostic` — the runner returns `CancelledBeforeSpawn` before `prepare` is reached, so no degradation was ever classified |
| Cancellation **after** spawn | the diagnostic still stands and precedes the terminal `Cancelled`/`Interrupted` event | Inspected: the diagnostic is emitted at setup time, before the spawn, so it is already in the journal when the cancellation is classified. No dedicated test was added for this case; the mechanism is the same one the ordering tests exercise |
| Lease expiry | the diagnostic is not retracted; the server's rewrite to `Interrupted`/`LeaseExpired` is untouched | Inspected: the rewrite happens in `run_execution` on the *result*, after the diagnostic was published, and it changes the snapshot's state only. No dedicated test |
| Timeout / output-limit termination | the diagnostic is emitted at setup time and is unaffected | Inspected: same mechanism as the cancellation-after-spawn case |
| Runner error | no degradation is reported from a result that does not exist; a setup-time diagnostic that was already emitted stands | Inspected: `failure_for_runner_error`/`failed_result` were not modified |
| Event-publication failure | the existing behaviour (drain and cancel) is unchanged | Inspected: the diagnostic goes through the same `publish` → `store.commit_event` path as every other event |
| Restart and recovery | the diagnostic is in the durable journal, so a client that reconnects after a node restart sees it | Executed (same durability path): `the_diagnostic_is_replayed_from_the_journal_after_the_execution_ended` reads the event page after the execution finished |
| Contention | one sequence number and one journal row; no change to the state machine or the trimming | Inspected: the diagnostic is an ordinary event with no special-casing in `commit_event` |

**Security review of the change.**

- *Process ownership*: the server's added path only maps a chunk to an event
  kind; it creates no process. `python3 scripts/check_execution_ownership.py`
  passed, and no `Cargo.toml` was touched, so no dependency edge changed.
- *Wire contract*: `ExecutionEventKind`, `ExecutionResult`, `SandboxResult`
  and `ResourceResult` are byte-for-byte unchanged. The `Diagnostic` variant
  already existed and was already decodable; this milestone makes it appear in
  production traffic.
- *Authentication, authorization, lease, generation, fencing, digest bounds*:
  none of those code paths was touched. The diagnostic is published by the
  existing `publish` funnel, under the same lease and generation as every
  other event of that execution.
- *Secrets*: the diagnostic carries an operator-facing reason, a profile name
  and a dimension name. It carries no credential, no path and no target
  output. `OutputChunk`'s hand-written `Debug` still redacts the payload, and
  the pre-existing `secret_sentinels_stay_out_of_error_and_debug_surfaces`
  test now also covers the `Diagnostic` variant.
- *Fail-closed*: unchanged. A required request is refused before the target
  starts; the refusal is never described as a degradation.

## 7. Platform matrix actually exercised

| Platform | Evidence | Claim |
| --- | --- | --- |
| Linux x86_64 (`6.8.0-147-generic`, rustc 1.89.0) | The suites in §5 were executed on this host. The degradation paths exercised are the *setup* paths (`NoExecutionSetup`: no sandbox backend, no resource backend) and the server's refusal path. | The diagnostic mechanism, its ordering, its bound and its durability are qualified on this host class. The **Landlock and cgroup enforcement paths themselves were not exercised here** — no trusted helper was installed in this environment, so the *handshake-recovery* emission point and the *post-mortem unobservable-counter* emission point are **code-inspected only**. |
| macOS | No hosted runner and no macOS host in this environment. Code inspection only: `verify_trusted_helper` → `Err`, Landlock probe → `false`, cgroup probe → `Err`, so every best-effort request degrades and takes the setup-time emission path, which is platform-neutral. The runner's test targets were *compiled* for `x86_64-apple-darwin` (clippy reached the crate's own code) but the crate does not build warning-free on macOS at the reviewed head — see §5 and §9 finding S-07. | **Not qualified.** No claim is made beyond "the setup-time emission path is platform-neutral code". |
| Windows | The runner (including its test targets) compiles and lints clean for `x86_64-pc-windows-msvc` with `-D warnings`, which is what the `windows-runner` CI job checks. The *server* crate could not be cross-checked here (§5, `aws-lc-sys` needs a Windows C toolchain). The new runner suite is written platform-aware (`cmd.exe` on Windows, `/bin/sh` elsewhere) so the `windows-runner` job's `cargo test -p eggwork-runner --all-targets` exercises it there — but that run has **not** happened in this milestone. | **Not qualified.** Compilation is not qualification. The Windows row stays open until the Windows CI job runs the new suite green — see §9 finding S-02. |

Only the Linux toolchain is installed here, so no cross-compilation was
attempted. **No platform is claimed as qualified merely because its code
compiles.**

## 8. Documentation status

| Document | Change |
| --- | --- |
| `architecture/protocol.md` | New section "The execution diagnostic payload (schema version 1)": the grammar, the closed class set, the key set, the bound, the truncation rule, and what the payload deliberately does not carry. |
| `architecture/runner-execution.md` | New section "Reporting a degradation": where the runner emits and when, the emission-site table, and the rules (required is never degraded, one event per degraded dimension, silence when nothing degraded). The `OutputChunk` description was replaced with the enum, the `spawn_reader` step and the process-lifecycle steps 4/4a/6/9 were updated, and two invariant rows were added. |
| `architecture/server-node.md` | New paragraph "Where a degradation diagnostic is published": the publication point, the ordering guarantee, and the fact that a post-terminal publication would be dropped (because `publish` short-circuits on a terminal base state and `commit_event` refuses a terminal execution). The test-coverage list names the new cases. |
| `architecture/sandbox-helper.md` | The handshake-recovery paragraph now states that the runner reports the degradation before it re-spawns the target directly, and a new paragraph states that the unobservable counter is a post-mortem fact whose diagnostic necessarily follows the target's output. One invariant row was added. |
| `architecture/overview.md` | **Not edited — the coordinator owns it.** See §9 finding S-01 for the recommended disposition. |
| `AGENTS.md` | Not edited. The hard-invariant list needed no new entry; the "Isolation claims are earned" line is unchanged and remains true. |
| `plans/subsystems/security-isolation-resource-roadmap.md` | M005a entry updated (status, closure-evidence pointer, exit conditions rewritten to the implemented mechanism) and the roadmap's status line updated. |
| `plans/registry.md` | **Not edited — the coordinator owns it.** See §9 finding S-01. |

New text in the deep dives uses symbol references rather than line numbers,
so it does not add to the repository's stale-line-number defect.

## 9. Unresolved findings, ranked by severity

**Critical: none. High: none.**

- **S-01 — Medium — owner: the coordinator (registry/overview owner).** The
  registry (`plans/registry.md`) and `architecture/overview.md` are outside this
  milestone's file ownership and were not edited. The recommendation is in
  §10.
- **S-02 — Medium — owner: the release owner / CI owner.** The runner's
  public `OutputChunk` type changed shape (struct → enum). The plan's §8
  requires the change to be "exercised on the Windows and macOS targets the
  release matrix builds", and this milestone could only execute Linux. The
  new suite `crates/eggwork-runner/tests/diagnostic_degradation.rs` uses
  `/bin/sh` and therefore does not run on Windows as written; the Windows
  tree suite (`crates/eggwork-runner/tests/windows_process_tree.rs`) already
  proves that best-effort is reported `NotApplied` there, but it does not yet
  assert the diagnostic. Until a Windows run covers it, the Windows row of
  §7 stays "not qualified".
- **S-03 — Medium — owner: the coordinator / release owner.** The
  `OutputChunk` change is release-visible and must be folded into a
  version-visible release with release notes, not into a qualification-only
  candidate. No `Cargo.toml` is in this milestone's ownership, so the version
  bump was not made.
- **S-04 — Low — owner: the security-isolation subsystem owner.** The
  post-mortem resource observation (the helper's `STATUS_UNOBSERVED` counter)
  is a fact that only exists *after* the target exited, so its diagnostic
  necessarily follows the target's output. It still precedes the terminal
  event, and the terminal snapshot remains the authoritative evidence. This
  is the one documented exception to "the diagnostic precedes any target
  output"; it is a property of the fact, not of the mechanism. The terminal
  snapshot is also coarse here: the whole resource outcome is rewritten to
  `NotApplied` when *one* counter was unobservable, so the snapshot reads
  `NotApplied` for dimensions whose limits were in fact applied and observed.
  The diagnostic is more precise than the snapshot in that case (it names only
  the dimension the helper actually reported). The snapshot's behaviour was
  **not** changed by this milestone, per the plan's §5.7.
- **S-05 — Low — owner: the security-isolation subsystem owner.** The
  diagnostic's `reason` value is normalised to printable ASCII, so a space
  becomes `_`. The reason text in the diagnostic is therefore less readable
  than the same sentence in the terminal snapshot. This is the price of a
  space-separated grammar without an escaping scheme; the snapshot keeps the
  original sentence and remains the authoritative evidence.
- **S-06 — Low — owner: the control-plane owner.** An external client that
  treats an unknown `ExecutionEventKind` as fatal would break on the newly
  trafficked `Diagnostic` variant. The repository's own client tolerates it
  (executed evidence in §4, criterion 8's path). The wire contract itself is
  unchanged — the variant already existed — so this is a client-side
  behaviour note for the release notes rather than a protocol change.
- **S-07 — Low — owner: the foundation/runner owner (NOT this milestone).**
  `crates/eggwork-runner/src/process_tree.rs`'s `ProcessTree::start_kill` is
  called only from the Linux-only helper-handshake paths, so on macOS it is
  dead code and `cargo clippy --target x86_64-apple-darwin -- -D warnings`
  fails with "method `start_kill` is never used". This is **pre-existing at
  the reviewed head `778d83c`** (the same call sites are inside the same
  `cfg`-gated block there) and `process_tree.rs` is not in this milestone's
  ownership, so it was not touched. It is recorded here because it blocks the
  macOS cross-target clippy check that §8's platform matrix would otherwise
  use. The one-line fix is a `#[cfg(target_os = "linux")]` on the method.

## 10. `architecture/overview.md` recommendation

**Conclusion: no divergence row should be added for M005a.**

The truthfulness gap the milestone set out to close is closed: a degraded
best-effort execution now produces a diagnostic event in the stream, bounded,
grammar-conforming, published before the target's output and before the
terminal event, and durable in the journal. The plan's §10 says to add a row
"only if a residual divergence remains after M005a; if the truthfulness gap is
closed, do not add a row", and it is closed.

The one documented exception (S-04, the post-mortem unobservable counter) is a
property of the *fact* being reported, not a divergence between the code and
the documentation: the code and the docs both say that this particular
diagnostic follows the target's output, because the helper can only report an
unobservable counter after the target exited. There is nothing for a
divergence row to point at.

**If the coordinator nonetheless wants a row** — for instance to make the
post-mortem exception visible from the bird's-eye view — the exact text would
be:

```markdown
| 6 | The helper-reported unobservable-counter degradation is published after the target's output | `crates/eggwork-runner/src/lib.rs` (post-mortem resource observation block) | `architecture/runner-execution.md` § "Reporting a degradation"; the fact only exists after the target exited |
```

It should be added **only** with that wording, and it must not touch
divergence #5 (the weaker deployment-path helper-trust check), which is a
separate, still-open item that this milestone neither closes nor worsens.

## 11. Registry and roadmap disposition

- `plans/subsystems/security-isolation-resource-roadmap.md`: M005a —
  `proposed` → **implemented** (closure record written, implementation commit
  pending). The exit condition "the diagnostic is emitted on the same path as
  the terminal result, so the two cannot disagree" was **rewritten** to the
  implemented mechanism, because the original wording describes the
  server-side fallback that the plan explicitly demotes. The intent is
  preserved and made stronger: the payload and the terminal snapshot are
  derived from the same `SetupOutcome`, and the diagnostic is published
  *before* the terminal event rather than next to it.
- The roadmap's status line was updated to match.
- `plans/registry.md`: **not edited** — the registry is the coordinator's
  file. The coordinator's action is to register M005a as implemented (closure
  record `plans/closure/security-isolation-resource/005a-status.md`) and to
  fill the real commit SHA in this record and in the roadmap entry. **Done:**
  the implementation commit is `6f41957`.
- `plans/implementation/security-isolation-resource/005a-degraded-isolation-observability-corrective.md`:
  **not edited** — the plan is not in this milestone's ownership and is left
  as the authoritative statement of what was asked.

## Outstanding evidence item (why this is conditionally closed)

The implementation is complete and every acceptance criterion is met on the
Linux host, but one criterion requires evidence this host cannot produce:

- the plan requires that a change to the runner's public `OutputChunk` type
  "must also be exercised on the Windows and macOS targets the release matrix
  builds, because the runner has platform-specific spawn paths and the
  diagnostic sits in front of all of them."

`OutputChunk` did change shape (struct to enum, carrying a `Diagnostic`
variant), so that requirement applies. Only Linux x86_64 was exercised here.
Nothing about the implementation is unfinished; what is missing is a run on
two platforms, and the milestone stays **conditionally closed** until the
`windows-runner` CI job and the macOS release job have run. The claim that is
*not* made is that any platform has been qualified.
