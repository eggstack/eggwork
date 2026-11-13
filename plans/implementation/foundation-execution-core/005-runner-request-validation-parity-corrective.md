# Foundation M005 — Runner request validation parity corrective

Status: closed

Closure record: `plans/closure/foundation-execution-core/005-status.md`

Class: invariant

Source authority:

- `plans/subsystems/foundation-execution-core-request-validation-corrective-addendum.md`
- `plans/subsystems/foundation-execution-core-roadmap.md` — durable invariants 4 and 5
- `plans/closure/foundation-execution-core/002-status.md` — the canonical bounded local runner
- `plans/closure/foundation-execution-core/003-status.md` — runner API hardening
- `plans/003-planning-process.md#verification-philosophy`

## 1. Objective

Make the runner's request validation a projection of the `eggwork-core` domain contract instead of an independent restatement of it.

Today the runner re-spells the core validation rules in a private helper. The two copies disagree: the runner is stricter than core on empty argv entries and materially weaker than core on environment-entry syntax and duplicate environment names. The runner's copy is the one that gates the actual `Command::env()` call on the run path, and the runner's public constructor and public environment setter reach that gate without passing through core validation at all.

After M005, every request-construction rule has exactly one owner in `eggwork-core`; the runner consumes those rules rather than re-deriving them; and no public runner API can produce a request that violates the core contract.

## 2. Baseline / problem statement

At the reviewed baseline:

- `crates/eggwork-runner/src/lib.rs` exposes `RunnerRequest::new(argv, root)` and `RunnerRequest::set_environment(Vec<(String, String)>)` as public API. Neither validates.
- `RunnerRequest::from_spec` calls `spec.validate()` before building the request, so the server's protocol path is safe.
- `RunnerRequest::validate` is a private method that re-implements the request rules. It is invoked at the top of the run path, immediately before the child process is constructed, and its result is what gates the environment handed to the child.
- `crates/eggwork-core/src/lib.rs` owns the real rules. `EnvironmentEntry::new` requires an environment name to be non-empty, within `MAX_ENV_NAME_BYTES`, composed only of ASCII alphanumerics and underscore, not to start with a digit, and to be free of NUL bytes. `CommandSpec::validate` additionally rejects duplicate environment names with a typed `ValidationError::InvalidSyntax`.
- The runner's copy checks only emptiness, length, NUL, and `=` in the name. It therefore accepts names containing `-` or `.`, accepts names beginning with a digit, and does not detect duplicate names at all.
- The runner also defines its own `StdinPolicy` enum, `Debug` implementation, and `validate` method, duplicating `eggwork_core::StdinPolicy`, which the crate already imports under the alias `CoreStdinPolicy` and converts to by hand. The runner's `OverflowPolicy` handling shows the intended pattern: the core type is imported and used directly.
- The runner's module documentation states: *"Request fields remain private so callers cannot bypass protocol bounds."* The fields are private, but the public constructor and public environment setter are the bypass that sentence claims cannot exist.

The practical effect is narrow but real: an embedder using the public runner API can submit environment entries that the core domain contract rejects, and will not be told.

## 3. Scope

In scope:

- the request-validation rules enforced by `crates/eggwork-runner/src/lib.rs`;
- the public construction surface of `RunnerRequest`;
- the runner's local `StdinPolicy` duplicate and its conversion from the core type;
- the runner's re-statement of the core output-policy bounds;
- the corresponding documentation in `architecture/runner-execution.md`.

Out of scope:

- any change to the wire representation of `ExecutionSpec`, `CommandSpec`, or any other core type;
- any widening or narrowing of a numeric bound — the core bounds remain the bounds;
- the `eggwork-sandbox-helper` crate, which has no workspace-crate dependencies and does not construct runner requests;
- the `eggwork-client` crate;
- the server's request handling, which already uses the validated path;
- the control-plane shared-contract corrective, the security isolation-truthfulness corrective, and the operations operator-truthfulness corrective — this milestone is independent of all three and may run in parallel with them;
- Operations M007 (Windows service host), which is a separate ready handoff.

## 4. Invariants and non-goals

Invariants that must hold after M005:

1. A request-validation rule has exactly one owner in `eggwork-core`.
2. The runner is never weaker than its owner on any shared rule.
3. A rule the runner adds on top of core is explicit, documented in the code, and covered by a test.
4. No public runner API can produce a request that violates the core contract.
5. The runner accepts argv and never an implicitly interpreted shell string.
6. The runner's bounds are the core bounds.
7. The core crate keeps no internal dependencies and no scheduler or placement semantics.
8. The runner remains the single owner of production finite noninteractive process creation.

Non-goals:

- redesigning the core domain contract — core's rules are correct as they stand and are the authority here;
- introducing a validation framework, a builder crate, or a new error hierarchy;
- changing runner execution, timeout, cancellation, output-capture, or process-tree behaviour;
- making the runner aware of transport, authentication, or scheduling concerns.

## 5. Required production changes

1. **Move the environment-entry rules behind the core owner.** Ensure `eggwork-core` exposes the environment-name syntax and duplicate-name rules in a form the runner can consume, rather than the runner re-deriving them. If core already exposes enough, the change is to consume it; if not, the change is to widen an existing core helper's visibility rather than to add a second one.
2. **Consume, do not restate.** Replace the runner's environment checks in `RunnerRequest::validate` with calls into the core rules. The runner may retain an explicit check for an empty argv entry, because that rule protects the runner's child-process construction and is stricter than core; it must be documented as a runner-level rule in a comment that says why, and it must be tested.
3. **Remove the local `StdinPolicy`.** Delete the runner's duplicate enum, its `Debug` implementation, and its `validate` method; import `eggwork_core::StdinPolicy` directly, as the runner already does for `OverflowPolicy`. Remove the hand-written conversion.
4. **Consume the core output-policy bounds.** Replace the runner's re-stated capture-limit and event-chunk checks with the core `OutputPolicy` validation.
5. **Guard the public construction path.** Decide, and record the decision in the closure record, between:
   - (a) removing `RunnerRequest::new` and `RunnerRequest::set_environment` from the public API so that `from_spec` is the only construction path; or
   - (b) keeping them and making them validate, returning a typed error.
   Option (a) is preferred if the repository has no in-tree caller, because it makes the bypass structurally impossible rather than merely checked. Verify the in-tree caller set before choosing.
6. **Preserve error fidelity.** A validation failure raised by the runner must remain attributable to the rule that produced it. Do not collapse distinct rules into a single undifferentiated message if the core types already distinguish them.
7. **Keep the runner's own contract rules intact.** The root-directory and working-directory resolution in `RunnerRequest::validate` — canonicalisation, the "root is a directory" check, and the "cwd escapes root" check — are runner-level rules that core does not own. They must survive M005 unchanged.

## 6. Failure, cancellation, restart, and contention semantics

M005 changes no runtime semantics.

- Validation still occurs before any child process exists, on the same path, at the same point in the run sequence.
- A request that fails validation must still produce the same `RunnerError` family, and must still fail before spawn.
- Timeout, explicit cancellation, nonzero exit, output-limit termination, and cleanup failure must remain distinguishable, exactly as they are today.
- The `CancelledBeforeSpawn` check must continue to take precedence over validation, so a cancelled run does not report a validation error.
- Process-tree cleanup must complete before the execution owner releases its lifetime permit.

The only behavioural difference M005 may introduce is that a request which previously passed the runner's weaker rules and was rejected by core, or vice versa, may now be rejected at a different point. That is the intended correction, and the closure record must name the specific rule dispositions that changed.

## 7. Compatibility and migration

No wire protocol change. No stored-format change. No change to any core type's serde representation. No change to the `ExecutionSpec` or `CommandSpec` wire shape.

The compatibility surface is the runner's **public Rust API**. The runner crate is part of the published release artifact set, so any removal of a public item is release-visible and must be handled as a version-visible change, not a silent one.

The plan must, before changing the API:

1. enumerate every in-tree caller of `RunnerRequest::new` and `RunnerRequest::set_environment`;
2. state whether any out-of-tree embedder is known to exist, and if the answer is unknown, say so explicitly rather than assuming the API is free to change;
3. decide the disposition — removal, or validation with a typed error — and record which;
4. if removal is chosen, note that it is a breaking change to a published crate and that the release notes and the version must reflect it.

The removal of the runner's local `StdinPolicy` is also public-API-visible, and must be included in the same compatibility statement.

## 8. Required tests

Required, and each must be non-vacuous:

1. **Construction-path parity.** A test proving that a request built through every public construction path satisfies the same rules as a request built through `from_spec`. If the public constructor is removed, the test asserts that no other public construction path exists — that is, a test that fails if a new public constructor is added.
2. **Environment-name syntax.** Tests proving that the runner rejects every name form core rejects: a name containing a character outside ASCII alphanumerics and underscore, a name beginning with a digit, a name exceeding the core bound, and an empty name. These must fail through the runner's public API, not only through core's own tests.
3. **Duplicate environment names.** A test proving that a request carrying two entries with the same name is rejected, and that the rejection is attributable to the duplicate-name rule.
4. **Total and disjoint rule coverage.** A test over the core environment-entry rules proving the accepted and rejected sets partition the input space, so that a future rule change cannot silently open a gap.
5. **Stricter runner-level rules.** A test proving the runner still rejects an empty argv entry, and a comment at that check stating that it is a runner-level rule stricter than core.
6. **Stdin policy.** Tests proving the runner forwards stdin bytes and enforces the core stdin bound, after the duplicate type is removed.
7. **Output bounds.** Tests proving the capture-limit and event-chunk bounds behave as before the corrective.
8. **No behavioural regression.** The existing runner suite — timeout, cancellation, output-limit, process-tree, and cleanup diagnostics — must remain green unchanged.

The verification must prove the rules are enforced where the runner claims them to be, not merely that the core crate's own tests pass.

## 9. Required verification

At minimum:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 scripts/check_execution_ownership.py
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

`--locked` is load-bearing on both `clippy` and `test`: a version bump that skips `Cargo.lock` passes both and then fails every release build.

If the runner's public API changes, the change must also be exercised on the Windows and macOS targets the release matrix builds, because the runner has platform-specific spawn paths and the request-validation change sits in front of all of them. Record the platform matrix actually exercised.

## 10. Documentation updates

- `architecture/runner-execution.md` — update the description of the runner's request validation so it matches the code, and in the same pass replace this deep dive's stale line-number citations with symbol references. This deep dive's citations are among the most drifted in the repository: the cited positions of `verify_trusted_helper`, `LANDLOCK_WORKSPACE_RW_CAPABILITY`, and `probe_landlock_ruleset` no longer point at those symbols.
- `architecture/overview.md` — record the new invariant in the divergences table only if a residual divergence remains after M005; if none remains, do not add a row.
- `architecture/core-domain.md` — note that core now owns the environment-entry rules the runner consumes, if the current text still describes them as unimplemented or ambiguous.
- `AGENTS.md` — update the "Construct validated state behind private fields (`RunnerRequest::from_spec`)" line if the construction path changes.
- `plans/subsystems/foundation-execution-core-roadmap.md` — add the M005 milestone entry and set the roadmap's status line to reflect it.
- `plans/registry.md` — updated by the registry owner when the milestone is registered; this plan does not edit it.

## 11. Acceptance criteria

1. Every environment-entry rule enforced by the runner is owned by `eggwork-core` and consumed by the runner.
2. The runner rejects every environment-name form core rejects, through the runner's public API.
3. Duplicate environment names are rejected by the runner.
4. The runner's local `StdinPolicy` duplicate, its `Debug` implementation, and its hand-written conversion are gone.
5. The runner's re-stated output-policy bounds are gone.
6. No public runner API can produce a request that violates the core contract, and a test proves it.
7. The runner remains at least as strict as core on every shared rule, and every intentional runner-level addition is documented in the code and covered by a test.
8. No numeric bound changed.
9. No wire representation changed; no core type's serde behaviour changed.
10. The existing runner suite is green, including the timeout, cancellation, output-limit, and process-tree tests.
11. `architecture/runner-execution.md` matches the code and no longer carries stale line-number citations.
12. The full pre-submit gate is green on the reviewed head.

## 12. Stop conditions

Stop and re-plan, recording the blocker, if any of the following occurs:

- Consuming the core rules would require the `eggwork-core` crate to acquire a dependency, or would create a dependency edge the workspace's one-way rule forbids (core and the sandbox helper depend on nothing; client depends on core; server depends on core and runner; `eggwork-client` is dev-only for the server).
- Removing the public constructor or setter would break a known in-tree caller that cannot be migrated within this milestone, and the closure record cannot state the public-API compatibility position honestly.
- The core rules are found to be *weaker* than the runner's copy on some rule, which would mean the authority itself is wrong and the decision of which copy is correct must be escalated rather than assumed.
- Correcting the validation changes the observable behaviour of the server's execute path in a way that is not a pure rejection tightening — for example if a currently accepted request becomes rejected for a reason other than a validation rule.
- The milestone cannot be completed without weakening an authentication, lease, generation, fencing, or digest bound.

## 13. Closure evidence required

Create:

- `plans/closure/foundation-execution-core/005-status.md`

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output;
- the platform matrix actually exercised, stated honestly — do not claim a platform was qualified merely because it compiles;
- negative and adversarial evidence: the environment-name and duplicate-name rejection tests, run and shown to fail the *pre-corrective* behaviour and pass the post-corrective behaviour;
- compatibility review: the public-API disposition, the in-tree caller enumeration, and the release-visibility consequence;
- documentation status, including the `architecture/runner-execution.md` update;
- unresolved findings ranked by severity;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- This milestone is independent and may be executed in parallel with the control-plane shared-contract corrective, the workspace data-plane correctives, the security isolation-truthfulness correctives, and the operations operator-truthfulness correctives. It may also be executed in parallel with Operations M007, which does not touch the runner's request API.
- The work is small and mechanical in the main, but the public-API decision in §5.5 is the part that needs a human's judgement. Resolve that decision first; the rest follows from it.
- The repository's own experience is that a rule written in one module and re-implemented in another has drifted before, and that the drift was found by inspection rather than by a failing test. Prefer the test that fails when a rule is weakened, over a comment that describes the rule.
- The runner crate is published. If the public API changes, the change must be reflected in the release version and notes, and must not be bundled silently into a release that claims to be a qualification-only candidate.
