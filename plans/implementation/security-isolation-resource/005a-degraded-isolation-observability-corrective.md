# Security M005a — Degraded isolation observability corrective

Status: conditionally closed

Closure record: `plans/closure/security-isolation-resource/005a-status.md`

Class: invariant

Source authority:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md` — finding SI-1
- `plans/subsystems/security-isolation-resource-roadmap.md` — the subsystem's durable invariants, including "Isolation claims are earned"
- `plans/closure/security-isolation-resource/004-status.md` — security closure
- `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md` — the remote-admission and capability-truthfulness corrective (C001)
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md` — decision 10: "Sandbox helper and resource-controller helper boundaries remain beneath the runner and must fail closed when required"
- `plans/security/threat-model.md` — the standing threat model
- `plans/003-planning-process.md#verification-philosophy`

## 1. Objective

Make a degraded isolation or resource enforcement observable **in the event stream**, not only in the terminal snapshot, and make the terminal state non-misleading for a caller that never polls a snapshot.

Concretely: when a best-effort isolation or resource request could not be enforced, the execution's event stream must carry a diagnostic event that states the class of degradation and a bounded reason. That diagnostic must reach the caller **before the target produces any output** and **before the terminal event**. A `Required` request that cannot be satisfied must continue to fail closed before the target process starts, exactly as it does today.

## 2. Baseline / problem statement

At the reviewed baseline:

- `crates/eggwork-core/src/lib.rs` defines `ExecutionEventKind::Diagnostic(String)` — the only event kind that is neither a state transition nor target output. It is the carrier this milestone uses. Its wire shape does not change.
- `ExecutionEvent::validate` in the same file bounds a diagnostic payload by `MAX_EVENT_CHUNK_BYTES` (64 KiB). `EventMetadata::validate` bounds a metadata field count by `MAX_METADATA_COUNT` (64) and each value by `MAX_METADATA_VALUE_BYTES` (2048).
- `ExecutionEventKind::Diagnostic` is constructed in exactly three places in the workspace, all of them test code: twice inside the `#[cfg(test)] mod tests` block of `crates/eggwork-server/src/lib.rs`, and once inside the `#[cfg(test)] mod tests` block of `crates/eggwork-server/src/store.rs` (a payload-length probe). **There is no production diagnostic event in the repository.**
- The degradation itself *is* recorded. `crates/eggwork-runner/src/lib.rs` produces `SandboxOutcome::NotApplied { reason }` from `TrustedLandlockSetup::prepare` for an unsupported sandbox profile and for an untrusted helper, `ResourceSetupOutcome::NotApplied { reason }` when the cgroup backend probe fails or when the helper cannot be trusted for the resource path, and `RunnerResult::execution_result` maps both onto `eggwork_core::SandboxResult::NotApplied` and `eggwork_core::ResourceDimensionResult::NotApplied`. The server carries them into `ExecutionResult`.
- The runner's degradation paths are, in `LocalProcessRunner::run` in `crates/eggwork-runner/src/lib.rs`:
  - an unsupported sandbox profile — decided in `ExecutionSetup::prepare`, before any process exists;
  - an untrusted helper — decided in `ExecutionSetup::prepare`, before any process exists;
  - no backend configured (`NoExecutionSetup`) — decided in `ExecutionSetup::prepare`, before any process exists;
  - a resource-backend probe failure — decided in `ExecutionSetup::prepare`, before any process exists;
  - a helper handshake failure — decided **after** the helper process was spawned as the tree leader. `SandboxChannel::wait` returning `Ok(None)`, or `Err(SandboxWaitError::BeforeTarget(..))`, causes the runner to kill the helper tree, rewrite `setup.sandbox` and `setup.resources` to `NotApplied`, and **re-spawn the target directly through `direct_command`, bypassing the helper entirely**. The target then runs with no Landlock ruleset and no cgroup verification.
  - a helper-reported unobservable counter (`STATUS_UNOBSERVED` in the helper) — the runner turns this into `NotApplied` for the affected resource dimension.
- The server publishes events in `run_execution` in `crates/eggwork-server/src/lib.rs`: a `State(Running)` event, then one event per output chunk forwarded from the runner's output channel, then the terminal event through `publish_terminal`.
- `publish` in the same file returns early when the recorded base state is already terminal, and `store::ExecutionStore::commit_event` refuses to append to a terminal execution. **A diagnostic published after the terminal event is silently dropped.** The ordering requirement is therefore a correctness requirement, not a style preference.
- The `Required` path is already truthful in the stream and must not change. When a required setup cannot be satisfied, the runner returns an error before the target starts; the server's `failed_result` synthesizes `SandboxResult::Failed { reason: "required sandbox setup failed" }` and a `Failed` terminal event. The gap this milestone closes is the *successful* execution whose isolation was degraded, where the terminal event says only "succeeded".
- `crates/eggwork-client/src/lib.rs`'s `ExecutionStream::into_events` already yields any `ExecutionEvent`, including a diagnostic, so a client can receive the new event without a transport change.

## 3. Scope

In scope:

- the production emission of `ExecutionEventKind::Diagnostic` from the execution path in `crates/eggwork-server/src/lib.rs`;
- the runner-side surfacing of a degradation so that the ordering guarantee can be met — specifically `crates/eggwork-runner/src/lib.rs`'s `LocalProcessRunner::run`, its `ExecutionSetup::prepare` implementations (`TrustedLandlockSetup`, `NoExecutionSetup`), and the helper-handshake recovery path;
- the runner's public `OutputChunk` type, if the ordering guarantee requires it (see §5);
- the payload grammar and bounds for the diagnostic;
- the server's event-publication ordering relative to the terminal event;
- the corresponding documentation in `architecture/runner-execution.md`, `architecture/server-node.md`, and `architecture/sandbox-helper.md`.

Out of scope:

- any change to the wire representation of `ExecutionEventKind`, `ExecutionResult`, `SandboxResult`, or `ResourceResult`. The wire contract belongs to the control-plane shared-contract corrective, and M005a must not collide with it;
- any change to the `Required` path's semantics — a required request still fails closed before the target process starts, and this milestone may not soften that;
- the environment deny-list and the `PATH` rule, which belong to M005b;
- resource-scoped authorization, which belongs to M005c and is blocked on an ADR;
- the `eggwork-client` crate's transport, retries, or egress routing. A client-side convenience accessor is permitted but not required;
- the server's own diagnostics (configuration faults, admission refusals) — this milestone is scoped to the *execution* event stream;
- any change to the isolation, resource, admission, or lease semantics themselves.

## 4. Invariants and non-goals

Invariants that must hold after M005a:

1. A best-effort isolation or resource request that could not be enforced produces a diagnostic event in the execution's event stream.
2. The diagnostic event precedes the terminal event, and precedes any stdout or stderr event of the target.
3. The diagnostic payload is bounded, machine-readable, and carries the degradation class and a bounded reason.
4. A `Required` isolation or resource request that cannot be satisfied still fails closed **before the target process starts**, and its terminal state is unchanged.
5. A `Required` failure is never reported as a degradation. A diagnostic is not a substitute for a refusal.
6. An execution whose isolation and resources were fully applied emits no diagnostic event.
7. The diagnostic is durable: it is committed to the execution journal and is visible to a client that connects after the execution finished and replays the event page.
8. `ExecutionEventKind`'s wire representation is unchanged.
9. The runner remains the single owner of production finite noninteractive process creation.
10. The server does not create a child process on any path added by this milestone.

Non-goals:

- redesigning the event protocol, the event metadata shape, or the client API;
- adding new isolation or resource enforcement mechanisms;
- making the node *fail* on a best-effort degradation — best-effort degradation must remain permitted, and must merely become visible;
- reporting anything about the target's own behaviour, exit status, or output beyond the degradation itself;
- exposing the reason strings verbatim without bounds — they are operator-facing text and must be truncated to a documented limit.

## 5. Required production changes

1. **Define the diagnostic payload grammar.** The diagnostic is carried by the existing `ExecutionEventKind::Diagnostic(String)` variant. Its payload MUST be a single string with a documented, stable grammar of the form:

   ```text
   eggwork.diagnostic.<class>.v1 key=value key=value …
   ```

   `<class>` is drawn from a closed set owned by this milestone:

   | Class | Meaning |
   |---|---|
   | `isolation.degraded` | a requested best-effort filesystem isolation was not applied |
   | `resources.degraded` | a requested best-effort resource enforcement was not applied |

   The minimum key set is `class`, `requirement` (always `best_effort` for a degradation), and `reason`. `isolation.degraded` additionally carries `profile` (the requested profile) when one was requested. `resources.degraded` carries `dimension` (the affected `ResourceDimension`) when a single dimension is affected, and one event per affected dimension when more than one is.

   The payload MUST be at most `MAX_METADATA_VALUE_BYTES` (2048) bytes so it stays inside the same bound as any event-metadata value, and therefore trivially inside `MAX_EVENT_CHUNK_BYTES`. The reason text MUST be truncated to fit that bound, and the truncation MUST be observable in the payload rather than silent.

2. **Decide where the diagnostic is produced and state it in the closure record.** The expected mechanism is that the runner surfaces the degradation through its output channel, before it spawns the target, and the server's forwarding loop in `run_execution` maps that chunk to `ExecutionEventKind::Diagnostic`. This is the only mechanism that can satisfy invariant 2 in full, because the server's `run_execution` learns the outcome from the runner's `RunnerResult` only **after** the execution has already finished — a server-side diagnostic derived from `RunnerResult` would necessarily arrive after all target output. A server-side mechanism that publishes the diagnostic from `RunnerResult` immediately before `publish_terminal` is permitted only as a fallback, and the closure record MUST state which mechanism was chosen and why.

   If the runner-side mechanism is chosen, `crates/eggwork-runner/src/lib.rs`'s public `OutputChunk` type gains a diagnostic variant. That is a public-API change to a published crate; see §7.

3. **Emit the diagnostic exactly once per degradation.** A single degraded dimension produces exactly one diagnostic event. A degraded isolation and a degraded resource enforcement produce two events, each naming its own class. The diagnostic MUST NOT be duplicated by both the runner and the server.

4. **Emit the diagnostic before the target starts, in every degradation path.** For the degradations decided in `ExecutionSetup::prepare` — unsupported profile, untrusted helper, no backend, resource probe failure — the diagnostic is emitted before the target process exists. For the helper-handshake recovery path, the helper process was already spawned as the tree leader, but the helper refused to start the target; the runner MUST emit the diagnostic after the handshake failure is classified and **before** it re-spawns the target directly. The target therefore never starts without the caller having already been told.

5. **Publish the diagnostic before the terminal event.** The publication point in `run_execution` MUST be before `publish_terminal`, because `publish` returns early on a terminal base state and the event would be silently dropped. The implementer MUST verify this by asserting on event sequence numbers, not by inspection.

6. **Leave the `Required` path's behaviour unchanged.** The runner's `Required` branches — the `Err(RunnerError::Setup(..))` returns in `TrustedLandlockSetup::prepare`, the `RunnerError::RequiredSetupUnavailable` returns in `LocalProcessRunner::run`, and the `Ok(None)` arm that kills the helper tree — MUST keep their current semantics and their current terminal classification. No diagnostic may be used to convert a required refusal into a degraded execution. A diagnostic MAY additionally be emitted on the `Required` path to explain the refusal, provided it does not change the terminal state and provided the closure record states that the path was otherwise untouched; this milestone does not require it.

7. **Keep the terminal snapshot's evidence.** `ExecutionResult.sandbox` and `ExecutionResult.resources` continue to carry `NotApplied { reason }` exactly as they do today. The diagnostic is additive; the snapshot is not replaced by it.

8. **Keep the journal complete.** The diagnostic is committed through the existing `commit_event` path so it is durable and replayable, and it must carry a sequence number consistent with the rest of the execution's event sequence.

9. **Keep the client able to receive it.** Confirm that `ExecutionStream::into_events` in `crates/eggwork-client/src/lib.rs` surfaces the diagnostic unchanged. If a typed convenience accessor is added, it must not change the transport.

10. **Document the grammar.** The closed class set, the key set, the bound, and the truncation rule must be written down where an operator or a client author will find them — `architecture/protocol.md` for the grammar, `docs/` only if an operator-facing behaviour changes (it does not: the operator surface is unchanged).

## 6. Failure, cancellation, restart, and contention semantics

- **Cancellation before spawn.** `RunnerError::CancelledBeforeSpawn` must not be reported as a degradation. A cancelled execution must not emit an `isolation.degraded` or `resources.degraded` diagnostic.
- **Cancellation after spawn.** A degraded execution that is then cancelled still emits its diagnostic — the degradation happened and is still true. The diagnostic precedes the terminal `Cancelled`/`Interrupted` event.
- **Lease expiry.** The server rewrites a successful runner result in place to `Interrupted`/`LeaseExpired`. A diagnostic emitted before that rewrite stays in the stream; it is not retracted, because the degradation really occurred.
- **Timeout and output-limit termination.** The diagnostic is emitted at setup time and is unaffected by a later timeout or output-limit termination.
- **Runner error.** When the runner returns an error, there is no `RunnerResult` and therefore no degradation to report from the result. If the degradation was already observed at setup time and the diagnostic was emitted, it stands. The server's `failed_result` path is unchanged.
- **Event-publication failure.** If `commit_event` fails, the existing behaviour applies — the server marks the execution draining and cancels. The diagnostic must not change that.
- **Restart and recovery.** The diagnostic is part of the durable journal, so a client that reconnects after a node restart sees it in the replayed event page.
- **Contention.** The diagnostic occupies one event sequence number and one journal row. It must not change the ordering guarantees of the state machine or the existing per-execution event trimming.
- **Ordering under the `biased` select.** `run_execution`'s output-forwarding loop is `biased` and drains the runner's output channel before observing the runner task's completion. A diagnostic delivered through that channel is therefore forwarded before the terminal publication, deterministically.

## 7. Compatibility and migration

**Wire protocol.** No change. `ExecutionEventKind::Diagnostic(String)` already exists in the wire contract and is already decodable by `eggwork-client`. This milestone makes a previously-unused variant appear in production traffic.

**Client behaviour.** A client that today assumes the event stream contains only `State`, `Stdout`, and `Stderr` kinds must be updated to tolerate `Diagnostic`. That is the intended correction and is a client-side behaviour change. The change is additive at the transport level: an older client decoding the stream still receives a well-formed event, but a client that treats an unknown kind as fatal would break. The implementer MUST state, in the closure record, whether the repository's own client tolerates it (it does — `into_events` yields every kind) and MUST flag the external-client consequence explicitly.

**Public Rust API.** If the runner-side mechanism is chosen, `crates/eggwork-runner/src/lib.rs`'s public `OutputChunk` struct gains a variant. `OutputChunk` currently has exactly two construction sites in the workspace — the runner's own producer and one server test helper — so the in-tree blast radius is small, but the type is public in a published crate and the change is release-visible. It MUST be handled as a version-visible change and MUST NOT be bundled silently into a release that claims to be a qualification-only candidate.

**Operator surface.** Unchanged. `eggworkd` verbs, the node's capability advertisement, and the deployment trust checks are not touched. The capability advertisement already reflects the *node's* ability to enforce isolation (`verify_trusted_helper` and `probe_landlock_ruleset`), not any individual execution's degradation, and must not be changed to carry per-execution state.

## 8. Required tests

Required, and each must be non-vacuous — a test that would still pass if the feature were removed is not acceptable.

1. **A degraded best-effort execution emits the diagnostic.** A server-level integration test that submits an execution with a best-effort sandbox request against a node whose runner is constructed with an explicit, deterministic setup that cannot enforce isolation — `LocalProcessRunner::new(NoExecutionSetup)` in `crates/eggwork-runner/src/lib.rs`, whose `ExecutionSetup::prepare` returns `SandboxOutcome::NotApplied { reason: "no sandbox backend is configured" }` for a best-effort request — and then asserts that the execution's events contain an `ExecutionEventKind::Diagnostic` whose payload carries the `isolation.degraded` class.

   The test MUST use an explicitly constructed setup, not `LocalProcessRunner::default()`: on Linux the default discovers a sibling helper next to the test binary and on other platforms it uses `NoExecutionSetup`, so the default's behaviour is environment-dependent and the test would not be deterministic.

2. **The diagnostic precedes the terminal event.** The same test MUST assert on sequence numbers: the diagnostic's sequence number is strictly less than the terminal event's sequence number.

3. **The diagnostic precedes any target output.** The same test MUST run a target that writes to stdout, and assert that the diagnostic's sequence number is strictly less than the first `Stdout` event's sequence number. This is the assertion that fails if the diagnostic is produced from the runner's `RunnerResult` instead of from the setup path, so it must be present and must not be weakened to "the diagnostic exists somewhere in the stream".

4. **A `Required` execution never reaches the target.** A test that submits an execution with a required sandbox request against the same deterministic setup, and asserts that the target's observable side effect never happened — the child must not have created its marker file, and the terminal state must be a failure, not a success. The assertion must be on the side effect, not merely on the terminal state.

5. **The `Required` path's terminal classification is unchanged.** A test asserting that a required setup failure still yields `ExecutionFailure`/`ExecutionState` exactly as it does at the reviewed baseline, so that no future change can quietly convert a required refusal into a best-effort degradation.

6. **No diagnostic when nothing degraded.** A test proving that an execution with no isolation and no resource request, or with both fully applied, emits no diagnostic event. Without this test, a diagnostic emitted on every execution would pass tests 1–5.

7. **Payload bounds and grammar.** A test proving that the diagnostic payload is at most `MAX_METADATA_VALUE_BYTES` bytes, that it begins with the documented `eggwork.diagnostic.<class>.v1` prefix, that `<class>` is one of the closed set, and that an over-long reason is truncated in a documented way rather than silently.

8. **The diagnostic is durable and replayable.** A test that reads the execution's event page after the execution has finished and finds the diagnostic, proving it was committed to the journal and not merely broadcast to a live subscriber.

9. **Resource degradation.** A test that induces a best-effort resource degradation — the cgroup backend probe failing, or the helper reporting an unobservable counter — and asserts a `resources.degraded` diagnostic naming the affected dimension.

10. **Cancellation does not produce a false degradation.** A test proving that an execution cancelled before spawn emits no diagnostic.

11. **No behavioural regression.** The existing server and runner suites remain green and unchanged, including the runner's timeout, cancellation, output-limit, and process-tree tests, and the server's existing event-ordering tests.

## 9. Required verification

At minimum:

```text
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

`--locked` is load-bearing on both `clippy` and `test`: a version bump that skips `Cargo.lock` passes both and then fails every release build.

The verification must prove the *ordering* property, not merely the presence of a diagnostic. A test that asserts "the stream contains a diagnostic" without asserting sequence numbers does not satisfy this milestone.

If the runner's public `OutputChunk` type changes, the change must also be exercised on the Windows and macOS targets the release matrix builds, because the runner has platform-specific spawn paths and the diagnostic sits in front of all of them. Record the platform matrix actually exercised, honestly: do not claim a platform was qualified merely because it compiles.

## 10. Documentation updates

- `architecture/protocol.md` — document the diagnostic payload grammar, the closed class set, the key set, the bound, and the truncation rule.
- `architecture/runner-execution.md` — describe where the runner emits a degradation and when, replacing the current prose that implies the degradation is only visible in the terminal result.
- `architecture/server-node.md` — describe the publication point and the ordering guarantee relative to the terminal event, and the fact that a post-terminal publication would be dropped.
- `architecture/sandbox-helper.md` — note the helper-handshake recovery path's degradation and where it is now reported.
- `architecture/overview.md` — add a divergence row only if a residual divergence remains after M005a; if the truthfulness gap is closed, do not add a row. Divergence #5 (the weaker deployment-path helper-trust check) is a separate, still-open item and must not be closed by this milestone.
- `AGENTS.md` — the hard-invariant list does not need a new entry; if the "Isolation claims are earned" line is extended, extend it in the same wording style and keep it true.
- `plans/subsystems/security-isolation-resource-roadmap.md` — add the M005a milestone entry and set the roadmap's status line to reflect it.
- `plans/registry.md` — updated by the registry owner when the milestone is registered; this plan does not edit it.

## 11. Acceptance criteria

1. A best-effort isolation or resource request that could not be enforced produces a diagnostic event in the execution's event stream.
2. The diagnostic's sequence number is strictly less than the terminal event's sequence number.
3. The diagnostic's sequence number is strictly less than the first stdout/stderr event's sequence number.
4. The diagnostic payload matches the documented grammar, names the degradation class, and is at most `MAX_METADATA_VALUE_BYTES` bytes.
5. A `Required` request still fails closed before the target process starts, proven by an assertion on the target's side effect.
6. A `Required` request's terminal classification is unchanged from the reviewed baseline.
7. An execution with nothing degraded emits no diagnostic.
8. The diagnostic is durable and appears in a replayed event page.
9. No wire representation changed; `ExecutionEventKind` keeps its current shape.
10. The server creates no child process on any path added by this milestone.
11. The runner's public API change, if any, is enumerated and reflected in the release version and notes.
12. `architecture/protocol.md`, `architecture/runner-execution.md`, `architecture/server-node.md`, and `architecture/sandbox-helper.md` match the code.
13. The full pre-submit gate is green on the reviewed head.

## 12. Stop conditions

Stop and re-plan, recording the blocker, if any of the following occurs:

- meeting the ordering requirement would require changing the wire representation of `ExecutionEventKind`, which belongs to the control-plane shared-contract corrective;
- meeting the ordering requirement would require the server to spawn or inspect a child process, which would break the process-ownership invariant;
- the diagnostic would have to carry data that is not already available at the point of emission — in particular, the diagnostic must not be reconstructed from the terminal snapshot after the fact, because that reintroduces the gap;
- the runner-side mechanism would require the sandbox helper to gain a workspace-crate dependency, which M005b owns and the helper's standalone-binary rule forbids;
- the implementation would change the `Required` path's behaviour in any way, including only its diagnostic;
- a degraded execution would begin emitting a diagnostic that could be mistaken for a terminal failure by an existing client, and the distinction cannot be made unambiguous in the payload;
- the milestone cannot be completed without weakening an authentication, lease, generation, fencing, or digest bound;
- the capability advertisement would have to change to carry per-execution degradation state, which would make a node-level advertisement depend on an execution's outcome.

## 13. Closure evidence required

Create:

- `plans/closure/security-isolation-resource/005a-status.md`

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output;
- the platform matrix actually exercised, stated honestly — do not claim a platform was qualified merely because its code compiles;
- negative and adversarial evidence: the ordering assertions shown to fail against the pre-corrective behaviour (or against a deliberately reordered implementation) and pass after it, and the `Required` side-effect assertion shown to pass;
- the chosen emission mechanism and the reason for choosing it, including the compatibility consequence of any public runner API change;
- the diagnostic payload grammar as implemented, with the bound and the truncation rule;
- documentation status, including the four deep dives;
- unresolved findings ranked by severity;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- This milestone is independent of M005b and M005c and may be executed in parallel with them, with the foundation runner-request-validation corrective, with the control-plane shared-contract corrective, and with Operations M007.
- The hard part is not emitting an event; it is emitting it **in the right order**. Read §5.4 and §5.5 before choosing a mechanism, because the obvious implementation — deriving the diagnostic from the runner's result inside the server — satisfies "the stream contains a diagnostic" and still fails the milestone.
- The repository's own experience is that a claim written in one module and re-implemented in another drifts, and that the drift was found by inspection rather than by a failing test. The ordering assertions in §8 are the part that must not be skipped.
- The `Required` path is the safety property this milestone must not damage. If a change to it ever looks necessary, that is a stop condition, not a task.
- The wire contract is not this milestone's to change. If a structured diagnostic payload appears to need a new event variant, stop and hand the question to the control-plane shared-contract corrective rather than widening the protocol here.
