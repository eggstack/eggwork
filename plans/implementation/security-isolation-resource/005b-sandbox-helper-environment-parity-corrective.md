# Security M005b — Sandbox-helper environment parity corrective

Status: proposed

Closure record: `plans/closure/security-isolation-resource/005b-status.md`

Class: invariant

Source authority:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md` — finding SI-2
- `plans/subsystems/security-isolation-resource-roadmap.md` — the subsystem's durable invariants, including "Isolation claims are earned"
- `plans/closure/security-isolation-resource/002-status.md` — Landlock runtime and `/dev/null` handling
- `plans/closure/security-isolation-resource/003-status.md` — enforced resource controls
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md` — decision 10: "Sandbox helper and resource-controller helper boundaries remain beneath the runner and must fail closed when required"
- `architecture/sandbox-helper.md` — the standalone-binary rule and the helper's dependency surface
- `AGENTS.md` — hard invariant 3 (crate dependencies stay one-way: core/helper → nothing)
- `plans/003-planning-process.md#verification-philosophy`

## 1. Objective

Give the sandbox environment policy — the denied-key list, the matching rule, the node-owned baseline keys, and the `PATH` value contract — **one owner**, consumed by both the runner and the standalone sandbox helper, without giving the helper a workspace-crate dependency, and make a one-sided edit fail the build instead of silently changing what is enforced.

The milestone also closes the live hole that the duplication makes reachable: at the reviewed baseline, a caller-supplied environment key that collides with the node's own baseline produces a launch specification the helper refuses, which a best-effort execution silently answers by running the target **unconfined**.

## 2. Baseline / problem statement

At the reviewed baseline:

- `crates/eggwork-runner/src/lib.rs` defines `fn denied_environment_key(key: &str) -> bool`. `crates/eggwork-sandbox-helper/src/main.rs` defines its own `fn denied_environment_key(key: &str) -> bool` inside `mod linux`. The two lists are **element-for-element identical**: the same 21 entries in the same order — `LD_`, `DYLD_`, `PATH`, `IFS`, `SHELLOPTS`, `CDPATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `GIT_ASKPASS`, `SSH_ASKPASS`, `GIT_CONFIG`, `GIT_CONFIG_`, `AWS_`, `AZURE_`, `GOOGLE_`, `SSH_AUTH_SOCK` — matched by the same rule: uppercase the key with ASCII rules, then `upper == denied || upper.starts_with(denied)`.
- Two properties of the rule are worth recording, because a parity fix must preserve them: `GIT_CONFIG_` is redundant, because the `GIT_CONFIG` prefix already covers `GIT_CONFIG_GLOBAL` and its relatives; and because the match is prefix-based, `PATH` denies *any* key beginning with `PATH`, not only `PATH` itself.
- **The semantics differ, and they differ in the dangerous direction.**
  - The runner **drops**. `direct_command` in `crates/eggwork-runner/src/lib.rs` calls `command.env_clear()`, installs `baseline_child_environment`, and then installs each caller-supplied entry for which `denied_environment_key` is false. `SandboxChannel::new` does the same for the helper's launch specification.
  - The helper **refuses the whole launch**. `read_spec` in `crates/eggwork-sandbox-helper/src/main.rs` rejects a specification whose environment contains a denied key (`denied_environment_key(key) && key != "PATH"`) or whose `PATH` is not exactly `/usr/bin:/bin` (`key == "PATH" && value != "/usr/bin:/bin"`), returning status code 12. `SandboxChannel::wait` in the runner reads a status byte of 4 or above as `RunnerError::Setup("sandbox helper rejected its launch specification (status {code})")`, delivered as `SandboxWaitError::BeforeTarget`.
- The helper's `PATH` rule has **no runner counterpart**. The runner satisfies it only incidentally, through the non-Windows branch of `baseline_child_environment`, which sets `PATH` to `/usr/bin:/bin`. The Windows branch sets `PATH` to `%SystemRoot%\System32;%SystemRoot%` and adds `SystemRoot` and `windir`. The helper is Linux-only (`#[cfg(target_os = "linux")] mod linux`), so the Windows shape never reaches it — but the rule is a property of the helper's environment contract and it is written in only one of the two places that own the environment.
- **The duplication is architecturally forced.** `crates/eggwork-sandbox-helper/Cargo.toml` declares no workspace-crate production dependency — only `serde`, `serde_json`, and the target-scoped `landlock` and `nix`. `architecture/sandbox-helper.md` states the rule and its reason: the helper is the trusted artifact that is staged, trust-verified, and run under its own rules, and its shipped dependency surface is deliberately kept to serde/serde_json plus one sandbox crate. `AGENTS.md`'s hard invariant 3 says the same thing in one line: "core/helper → nothing". The fix therefore cannot be "share a crate".
- **A live consequence follows from the duplication being invisible.** The runner's launch-specification builder starts from a fixed baseline — `PATH`, `LANG`, `LC_ALL`, `CI`, `NO_COLOR`, `TERM`, `GIT_TERMINAL_PROMPT`, `PAGER` — and then appends every caller-supplied entry that is not denied. None of `LANG`, `LC_ALL`, `CI`, `NO_COLOR`, `TERM`, `GIT_TERMINAL_PROMPT`, `PAGER` is on the deny-list, and `CommandSpec::validate` in `crates/eggwork-core/src/lib.rs` rejects only duplicates *within the caller's own* environment, not a collision with the node's baseline. A caller that sets `LANG` therefore produces a launch specification with two `LANG` entries. `read_spec` in the helper rejects a specification whose environment contains duplicate keys, returning status code 12. The runner reads that as a setup failure before the target, and — for a best-effort request only — kills the helper tree, rewrites `setup.sandbox` to `NotApplied`, and **re-spawns the target directly through `direct_command`, with no Landlock ruleset at all**.
  The observable behaviour today is therefore: a caller that sets `LANG=en_US.UTF-8` gets a successful execution that was never confined, and the only record of the degradation is `ExecutionResult.sandbox`'s `NotApplied { reason }` in the terminal snapshot. M005a is the milestone that makes that degradation visible; M005b is the milestone that stops it happening for a key the caller was never going to be denied.
- **Two further duplications sit in the same code and are the same class of defect.**
  - The node-owned baseline is written **twice inside the runner**: `baseline_child_environment` produces the direct child's environment, and `SandboxChannel::new` writes a second, separate literal list for the helper's launch specification. The two carry the same eight keys and the same values on Linux and nothing keeps them in step. `SandboxChannel::command` writes a third, shorter literal for the helper *process's own* environment.
  - The protocol limit constants are duplicated from `eggwork-core` into the helper: `MAX_ARG_COUNT`, `MAX_ARG_BYTES`, `MAX_ENV_COUNT`, `MAX_ENV_NAME_BYTES`, and `MAX_ENV_VALUE_BYTES` are re-declared in `crates/eggwork-sandbox-helper/src/main.rs` with the same values that `crates/eggwork-core/src/lib.rs` exports and that `crates/eggwork-runner/src/lib.rs` imports. (`MAX_SPEC_BYTES` is helper-only and has no core counterpart.)
- **The repository already holds the pattern this milestone generalizes.** `crates/eggwork-runner/tests/windows_process_tree.rs` keeps a hand-maintained `BASELINE_KEYS` list beside the runner's own `baseline_child_environment`, with the comment: *"Kept in sync with `baseline_child_environment` in the crate; a mismatch is a contract change, not a test fix."* That is a comment, not a gate. M005b turns it into a gate.

## 3. Scope

In scope:

- the denied-key list and its matching rule, in `crates/eggwork-runner/src/lib.rs` and `crates/eggwork-sandbox-helper/src/main.rs`;
- the `PATH` value rule, which currently exists only in the helper;
- the node-owned baseline environment, which is currently written three times in the runner and once implicitly in the helper;
- the rule that governs a caller-supplied key that collides with a node-owned baseline key — the cause of the live unconfined-execution hole described in §2;
- the helper's exit status for an environment-policy refusal, which is currently indistinguishable from several other specification-validation failures;
- the duplicated protocol limit constants in the helper, insofar as the milestone's mechanism makes them cheap to pin;
- the corresponding documentation in `architecture/sandbox-helper.md`, `architecture/runner-execution.md`, and `architecture/data-plane.md` if the data-plane deep dive describes the environment.

Out of scope:

- any change to the **content** of the denied-key list. M005b is an ownership and enforcement milestone, not a policy-tightening milestone. Adding or removing an entry is a separate decision with its own security review; the closure record must state that the entry set was carried over unchanged, and must list the entries as they stand.
- any change to the `Required` path's semantics. A required request still fails closed before the target starts.
- any change to the wire representation of `ExecutionEventKind`, `ExecutionResult`, `SandboxResult`, or `ResourceResult`;
- the diagnostic event itself, which belongs to M005a;
- resource-scoped authorization, which belongs to M005c and is blocked on an ADR;
- the Landlock ruleset, the cgroup verification, and the helper's status protocol, except for the environment-policy exit status named above;
- any new dependency for the helper, including a dev-dependency on a workspace crate for production code;
- the foundation runner-request-validation corrective, which changes request *validation* and is independent of this milestone.

## 4. Invariants and non-goals

Invariants that must hold after M005b:

1. The denied-key list and the matching rule have exactly one owner.
2. The sandbox helper has no workspace-crate production dependency, and none is introduced.
3. Neither the runner's nor the helper's build reads a file from outside its own crate directory.
4. The runner's and the helper's effective environment policies agree, element-wise and rule-wise, and a test proves it.
5. A one-sided edit to either copy fails the build.
6. The runner drops a denied key; the helper refuses a launch that contains one. That difference is a deliberate two-layer contract, documented on both sides.
7. The helper refuses a launch whose `PATH` is not exactly `/usr/bin:/bin`, and the runner guarantees that value on every path that reaches the helper.
8. A caller-supplied key that collides with a node-owned baseline key does not produce a launch specification with duplicate keys, and does not cause a best-effort execution to lose its confinement.
9. The node-owned baseline environment is written once, not three times.
10. The `Required` path still fails closed before the target process starts.
11. The helper's environment-policy refusal is distinguishable, by exit status, from the other specification-validation failures.
12. The duplicated protocol limit constants agree with their `eggwork-core` owners.

Non-goals:

- tightening the environment policy — M005b makes the existing policy single-sourced and enforced, not stricter;
- changing what the child process may do, beyond what the shared policy already specifies;
- redesigning the helper's launch protocol or its status bytes beyond the one named exit status;
- introducing a shared crate, a shared workspace library, or a code-generation framework;
- changing the node's capability advertisement.

## 5. Required production changes

### 5.1 Choose the mechanism, and justify it against the standalone-binary rule

The plan MUST evaluate and settle a mechanism. Three candidates were considered at the reviewed baseline:

| Candidate | How the helper consumes it | Verdict |
|---|---|---|
| **(A) Share a crate** — a new workspace crate owning the policy, depended on by both the runner and the helper | The helper gains a workspace-crate dependency | **Rejected.** It violates `AGENTS.md` hard invariant 3 and `architecture/sandbox-helper.md`'s stated rule that the helper ships with no production dependency on any workspace crate. The helper is the trusted artifact; its dependency surface is part of its trust story. |
| **(B) Build-time generation** — a `build.rs` in the helper reads the policy from elsewhere in the workspace and generates a Rust module into `OUT_DIR` | The helper's build reads a path outside its own package directory | **Rejected.** It makes the helper's build depend on the workspace layout rather than on its own crate, which is exactly the standalone property the helper has. It also adds a `build.rs`, and `build.rs` is outside the ownership guard's scan (`crates/*/src/**/*.rs`), so the guard would stop seeing part of the helper. |
| **(C) A single source-of-truth file, with generated copies checked into each crate** — the policy is authored once; a script regenerates both copies; both copies live inside their own crate's directory | The helper compiles its own in-crate copy; no cross-crate path, no `build.rs`, no new dependency | **Selected.** See below. |

**Mechanism (C), in detail.**

- The **source of truth** is a single checked-in file that states the policy: the denied-key entries, the matching rule, the node-owned baseline keys and values, and the `PATH` value contract. It is authored in one place and is not duplicated by hand.
- A **generator script** (new, under `scripts/`, alongside the other Python guardrails) reads the source of truth and writes a generated copy into each crate, inside that crate's own source directory:
  - `crates/eggwork-runner/src/` — the runner's copy, which the runner compiles directly.
  - `crates/eggwork-sandbox-helper/src/` — the helper's copy, which the helper compiles directly.
- Both crates compile only their **own** copy. No crate reads a file from another crate's directory, no `build.rs` is added, and no dependency edge is created. `scripts/check_execution_ownership.py`'s `check_dependencies` operates on Cargo dependency edges, so a generated file inside a crate creates no dependency edge and does not trip the guard.
- The generated copies are **checked in**. A build does not need the generator, so the release build does not depend on the script, and a reader of either crate sees the policy in that crate.
- The generated files carry a header saying they are generated, naming the source of truth and the generator, and saying that a hand edit is a contract change rather than a local fix.

**Direction of ownership — the decision.** The **runner owns the policy**; the helper's copy is the generated conformance copy. The justification is the ownership hierarchy the repository already states: `AGENTS.md` makes `eggwork-runner` "the single owner of OS process lifecycle + isolation setup seam", and `architecture/sandbox-helper.md` places the helper *beneath* the runner. The environment policy is part of the isolation setup contract the runner owns; the helper is the enforcement implementation that must conform to it. The reverse direction — the helper owning the policy — was rejected because it would invert that hierarchy and would make the runner, the crate with the wider consumer set, depend on the helper's directory.

**The generator's output must be deterministic and reviewable.** Re-running the generator over an unchanged source of truth must produce byte-identical output, and the generated file must be a plain, diff-friendly Rust module rather than a generated blob. The generator must not depend on network access and must fail loudly rather than emitting a partial file.

**The generator is not the gate; the test is.** The generator exists to make the intended edit a one-step operation. The *enforcement* of parity is the test in §8, which is what makes a one-sided edit fail the build even when the generator is not run.

### 5.2 Reconcile the drop-versus-reject difference explicitly

The two behaviours are **not** a contradiction to be eliminated. The milestone's decision is that they are a deliberate two-layer contract, and the fix is to remove the duplicated *list*, not to unify the duplicated *behaviour*.

- **The runner drops a denied key.** Rationale: the runner is the request-normalisation boundary. A caller may legitimately send `LD_PRELOAD` or `PATH`; the node's contract is that such a key never reaches the child. Dropping is silent, it is the current behaviour, and it is what makes a hardened environment a *property of the node* rather than a request the caller has to get right. Changing this to a refusal would turn currently-accepted requests into failures, which is a compatibility break with no security benefit.
- **The helper refuses a launch that contains a denied key.** Rationale: the helper is the enforcement point, and its job is to refuse a launch it cannot confine. If a denied key reaches the helper's launch specification, the runner's filtering did not happen — which is a defect in the node, not a caller error, and must surface as a failure rather than be silently tolerated. This is the repository's fail-closed rule applied to the enforcement point.
- The consequence of the two behaviours is that **the helper's refusal is unreachable while the lists agree**, and reachable the moment they do not. That is precisely the failure mode M005b exists to remove, and it is why the parity test in §8 is behavioural rather than structural.
- Both sides must carry a comment stating this contract in the same terms, and `architecture/sandbox-helper.md` must state it.

### 5.3 Settle the `PATH` rule

The `PATH == /usr/bin:/bin` rule exists only in the helper today. The milestone's decision is that it becomes an **enforced contract on both sides**, not a removed rule.

- The rule is a confinement-relevant invariant: a `PATH` that points outside `/usr/bin:/bin` widens what a confined target can execute, and the helper's Landlock ruleset does not restrict `execve` by path.
- The helper keeps the check — it is the enforcement point — but the expected value comes from the shared policy rather than from a literal in `read_spec`.
- The runner guarantees the value: the value it puts in the child environment and the value it puts in the launch specification are the same, and both come from the shared policy. The runner does **not** merely satisfy the rule incidentally through the baseline; the shared policy states the value, and both sides reference it.
- The Windows shape is explicitly **outside** this contract. The helper is Linux-only; the runner's Windows baseline is a different, platform-shaped environment that the helper never sees. The shared policy must say so, so that a future reader does not "fix" the Windows `PATH` to `/usr/bin:/bin`.
- The `PATH` rule is a Linux/Unix contract. The non-Windows runner baseline is the one it constrains.

### 5.4 Give the node-owned baseline one owner

`baseline_child_environment`, the launch-specification baseline in `SandboxChannel::new`, and the helper-process environment in `SandboxChannel::command` are three separate literals. The milestone requires the node-owned baseline to be written once in the shared policy, with the platform-specific branch (Windows) kept runner-local and stated as such.

The rule that the baseline is *node-owned* — a caller-supplied value for a baseline key does not replace it — is part of the shared policy and is what §5.5 depends on.

### 5.5 Close the baseline-key collision hole

This is the live defect the milestone must close, and it is the reason the milestone is not merely cosmetic.

The shared policy must distinguish three classes of key:

1. **Denied** — the caller may not set them. The runner drops them; the helper refuses a launch that contains them.
2. **Node-owned (reserved)** — the node sets them and the caller's value never reaches the child. `PATH`, `LANG`, `LC_ALL`, `CI`, `NO_COLOR`, `TERM`, `GIT_TERMINAL_PROMPT`, `PAGER` on the Unix baseline. A caller-supplied value for a reserved key is discarded in favour of the node's value, and the launch specification therefore never contains a duplicate key.
3. **Caller-controlled** — everything else.

The rule must be applied in **both** places the runner builds an environment: `direct_command` and `SandboxChannel::new`. A caller-supplied `LANG` must therefore produce a launch specification with exactly one `LANG`, not two, and must not cause the helper to refuse the specification and the execution to lose its confinement.

The milestone must state, and the closure record must record, the chosen behaviour for a caller-supplied reserved key: **discard the caller's value and use the node's** (the same shape as the denied-key drop), rather than refusing the request. Discarding is chosen because it is the current effective behaviour for the direct path — the direct child today receives the node's baseline `LANG` *and* the caller's `LANG`, with the caller's applied last, so the two paths currently disagree — and because refusing a request for `LANG` would be a surprising compatibility break for an ordinary key. The milestone must make the direct path and the helper path agree on this, and must say which way it resolved the disagreement.

### 5.6 Give the environment-policy refusal a distinguishable exit status

At the reviewed baseline, the helper's status 12 covers "the specification contains a denied key", "the `PATH` value is wrong", "the specification contains a duplicate key", "the argv is empty or oversized", "the environment exceeds its bounds", and "the schema version or profile is wrong". An operator reading "status 12" cannot tell which happened.

The milestone must give the **environment-policy refusal** its own status byte, distinct from the general specification-validation status, and must document the status table in `architecture/sandbox-helper.md`. The runner's handling of the new status must remain the fail-closed behaviour it has today: a required request fails before the target; a best-effort request degrades to `NotApplied` — and, after M005a, emits a diagnostic.

### 5.7 Pin the duplicated protocol limit constants

`MAX_ARG_COUNT`, `MAX_ARG_BYTES`, `MAX_ENV_COUNT`, `MAX_ENV_NAME_BYTES`, and `MAX_ENV_VALUE_BYTES` are re-declared in the helper with the same values that `eggwork-core` exports. The milestone's mechanism makes it cheap to generate the helper's copy from the core constants, and the closure record must state whether that was done. If it was not done, the parity test must still cover these five constants, and the closure record must state that they are duplicated rather than generated.

`MAX_SPEC_BYTES` is helper-only and has no core counterpart; it stays where it is.

## 6. Failure, cancellation, restart, and contention semantics

- **The `Required` path is unchanged.** A required isolation request that cannot be satisfied still fails closed before the target process starts. M005b must not convert a helper's environment-policy refusal into a degraded execution for a required request.
- **A best-effort request that the helper refuses** still degrades to `NotApplied` and, after M005a, still emits a diagnostic. M005b's effect is that this path becomes unreachable for a caller-supplied key that the policy reserves, because the specification the runner builds no longer contains the collision.
- **Cancellation** is unaffected. A cancelled run must not be reported as an environment-policy refusal, and the `CancelledBeforeSpawn` check must keep its precedence over the setup path.
- **Helper exit statuses.** Introducing a new status byte changes the runner's interpretation of the helper's status. The runner's status interpretation is a small match on the byte read from the status channel in `SandboxChannel::wait`; the new byte must be classified as a setup failure before the target, exactly like the status it replaces, and must not be mistaken for a success or a post-target status.
- **Restart and recovery.** The policy is compile-time. Nothing about restart behaviour changes.
- **Contention.** None. The policy is a static list; the only runtime effect is a lookup per environment entry at spawn time, which is bounded by `MAX_ENV_COUNT`.

## 7. Compatibility and migration

**Wire protocol.** No change.

**Public Rust API.** The parity test needs to observe the runner's policy from outside the runner crate, because the helper's crate cannot be a library dependency of anything (it is a binary-only crate) and the helper's own test suite already depends on the runner as a dev-dependency. The runner's environment policy must therefore be reachable from the runner's public API — a `pub` predicate and a `pub` list, or equivalent — or the parity check must be implemented by comparing the generated files as data rather than by calling into the runner.

This is an **additive** public API change to a published crate, which is materially smaller than a removal, but it is still release-visible and must be handled as a version-visible change: the new public items need doc comments saying that they are the node's environment policy and that the sandbox helper's copy is generated from them, so that a consumer does not treat the runner's copy as the only one.

**Behaviour change — this is the point of the milestone.** At the reviewed baseline, a caller that supplies `LANG` (or `LC_ALL`, `CI`, `NO_COLOR`, `TERM`, `GIT_TERMINAL_PROMPT`, `PAGER`) receives, on the helper path, an execution that was never confined; on the direct path, the child's `LANG` is the caller's value rather than the node's. After M005b, the node's baseline wins on both paths and the execution stays confined. The closure record MUST state this behaviour change explicitly, in the "before" and "after" form, and MUST list the affected keys. It is a security improvement, and it is still a behaviour change a caller can observe.

**Behaviour change — the helper's exit status.** The new status byte changes what the runner reports for an environment-policy refusal, and therefore what appears in `ExecutionResult.sandbox`'s `NotApplied { reason }` and, after M005a, in the diagnostic payload. The reason text is operator-facing; the closure record must show the before and after text.

**Operator surface.** `eggworkd` verbs and the node's capability advertisement are unchanged.

## 8. Required tests

Required, and each must be non-vacuous.

1. **Element-wise parity between the runner's policy and the helper's policy.** A test that loads both lists and asserts they have the same length, the same entries in the same order, and the same matching rule. This is the test that makes a one-sided edit fail the build. It must fail with a message that names the first differing entry and states that a mismatch is a contract change rather than a test to be adjusted — the wording the repository already uses in `crates/eggwork-runner/tests/windows_process_tree.rs`.
2. **Behavioural parity against the real helper binary.** A test that drives the actual helper executable — the helper's own test suite already does this through `env!("CARGO_BIN_EXE_eggwork-sandbox-helper")` — with a launch specification for each probe key, and asserts that the helper's accept/refuse decision matches the runner's policy for every probe key. This is what proves the two *rules* agree, not merely that the two *lists* have the same text. The probe corpus must include, for each entry: the entry itself, a lowercase form, a prefixed form (for prefix entries), and a near-miss that must be allowed.
3. **The generated copies are up to date.** A test that re-runs the generator over the unchanged source of truth and asserts the output is byte-identical to the checked-in copies. This catches a hand edit of a generated file with a message that points at the generator.
4. **A one-sided edit fails the build — demonstrated, not asserted.** The closure record must show, as negative evidence, that editing only the runner's copy (or only the helper's copy) makes the parity test fail, and that the failure message is the intended one. This is the evidence that the gate works.
5. **The baseline-key collision hole is closed.** A test that submits a request carrying `LANG` (and each other reserved key) and proves that:
   - the launch specification the runner builds contains exactly one entry for that key — proven either by observing the real helper's accept/refuse decision, or by asserting on the specification the runner writes;
   - the helper accepts the specification; and
   - the execution remains confined — that is, the helper path is taken and the target is not re-spawned directly.
   This test must be shown to fail against the pre-corrective behaviour.
6. **The direct path and the helper path agree on a reserved key.** A test proving that the child's environment on the direct path and the environment in the launch specification carry the same value for each reserved key.
7. **The `PATH` contract holds on both sides.** A test proving that the runner's child environment and the runner's launch specification both carry `PATH` equal to the shared policy's value, and that the helper refuses a specification whose `PATH` differs from it.
8. **The denied-key drop is unchanged.** A test proving that a caller-supplied denied key still does not reach the child, on both the direct path and the helper path, and that the execution is not refused by the helper for it.
9. **The helper's environment-policy refusal is distinguishable.** A test proving that the new status byte is used for an environment-policy refusal and is distinct from the general specification-validation status, and that the runner classifies it as a setup failure before the target.
10. **The `Required` path is unchanged.** A test proving that a required request whose helper refuses the specification still fails before the target process starts.
11. **The protocol limit constants agree.** A test comparing the helper's five duplicated constants against the `eggwork-core` values, whether by generation or by an explicit comparison.
12. **The generator is deterministic.** A test running the generator twice and asserting identical output, and asserting that it fails loudly on a malformed source of truth.
13. **No behavioural regression.** The existing runner, helper, and server suites remain green — in particular the helper's `tests/landlock_runner.rs` suite, which drives the real binary through the real runner, and the runner's Windows process-tree suite, whose `BASELINE_KEYS` list must be reconciled with the new single owner rather than left as a third hand-maintained copy.

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

The environment-policy and Landlock machinery is Linux-only. The helper's test suite is `#[cfg(target_os = "linux")]` and runs zero tests elsewhere. The closure record must state the platform matrix **actually exercised**, honestly, and must not claim the helper path was qualified on a platform where the suite did not run. The parity gate itself must run everywhere, because the *lists* are platform-independent; only the behavioural helper tests are Linux-gated.

If the new generator script is added, it must also be covered by the release-harness test suite under `tests/release/`, following the convention the other `scripts/` guardrails follow.

## 10. Documentation updates

- `architecture/sandbox-helper.md` — state the single owner of the environment policy, the drop-versus-reject contract, the `PATH` contract and its Linux scope, the node-owned baseline, the status table including the new environment-policy status, and the generated-file convention. Replace any stale line-number citations with symbol references in the same pass.
- `architecture/runner-execution.md` — describe the runner's environment construction: the denied-key drop, the reserved-key rule, the baseline, and the launch-specification environment.
- `architecture/overview.md` — add a divergence row only if a residual divergence remains after M005b. If the duplication is closed, do not add a row.
- `AGENTS.md` — the hard-invariant list does not need a new entry. If the "Isolation claims are earned" line is extended to mention the environment policy's single owner, keep the wording in the same style and keep it true.
- `plans/subsystems/security-isolation-resource-roadmap.md` — add the M005b milestone entry and set the roadmap's status line to reflect it.
- `plans/registry.md` — updated by the registry owner when the milestone is registered; this plan does not edit it.

## 11. Acceptance criteria

1. The denied-key list and the matching rule have exactly one owner; the helper's copy is generated and checked in.
2. `crates/eggwork-sandbox-helper/Cargo.toml` still declares no workspace-crate production dependency, and neither crate's build reads a file from outside its own crate directory.
3. A test compares the runner's policy and the helper's policy element-wise and rule-wise, and fails on a one-sided edit.
4. The negative evidence shows a one-sided edit failing the build with the intended message.
5. The runner drops denied keys and the helper refuses a launch containing one; both behaviours are documented on both sides, and the difference is stated as deliberate.
6. The `PATH == /usr/bin:/bin` rule is an enforced contract on both sides, sourced from the shared policy, with the Windows shape explicitly out of scope.
7. The node-owned baseline is written once, and `crates/eggwork-runner/tests/windows_process_tree.rs`'s `BASELINE_KEYS` is reconciled with the new owner rather than left as a hand-maintained third copy.
8. A caller-supplied reserved key no longer produces a duplicate launch-specification key, and no longer causes a best-effort execution to lose its confinement.
9. The direct path and the helper path agree on every reserved key's value.
10. The helper's environment-policy refusal has a status byte distinct from the general specification-validation status, and the runner classifies it as a setup failure before the target.
11. A required request still fails closed before the target process starts.
12. The helper's five duplicated protocol limit constants agree with `eggwork-core`, whether by generation or by an explicit comparison test.
13. The denied-key entry set is unchanged from the reviewed baseline, and the closure record lists it.
14. No wire representation changed.
15. `architecture/sandbox-helper.md` and `architecture/runner-execution.md` match the code.
16. The full pre-submit gate is green on the reviewed head.

## 12. Stop conditions

Stop and re-plan, recording the blocker, if any of the following occurs:

- the selected mechanism would require the helper to gain a workspace-crate dependency, or would require either crate's build to read a file outside its own directory;
- parity cannot be established without changing the *content* of the denied-key list — that is a policy decision, not an ownership decision, and it must be escalated separately;
- the resolution of the direct-path versus helper-path disagreement over a reserved key would require refusing a request that is accepted today, without an explicit decision to make that compatibility break;
- the helper's environment-policy refusal cannot be given a distinct status byte without breaking the runner's status interpretation in a way that would weaken a fail-closed path;
- the fix would change the `Required` path's behaviour in any way;
- the milestone cannot be completed without weakening an authentication, lease, generation, fencing, or digest bound;
- the parity test turns out to be impossible to express against the real helper binary — in that case, escalate rather than weakening the test to a structural comparison of text.

## 13. Closure evidence required

Create:

- `plans/closure/security-isolation-resource/005b-status.md`

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output;
- the platform matrix actually exercised, stated honestly — the helper suite is Linux-gated and must not be claimed as run elsewhere;
- negative and adversarial evidence: the one-sided-edit demonstration, the baseline-key collision test shown to fail against the pre-corrective behaviour and pass after it, and the behavioural parity sweep over the probe corpus;
- the denied-key entry set as it stood before and after, with the explicit statement that it was carried over unchanged;
- the reserved-key disposition, including the direct-path versus helper-path disagreement and how it was resolved;
- the before-and-after reason text for the helper's environment-policy refusal;
- the public-API change and its release-visibility consequence;
- documentation status, including the two deep dives;
- unresolved findings ranked by severity;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- This milestone is independent of M005a and M005c and may be executed in parallel with them, with the foundation runner-request-validation corrective, with the control-plane shared-contract corrective, and with Operations M007. It does touch `crates/eggwork-runner/src/lib.rs` in the same file as the foundation corrective, but in a different region.
- The mechanism choice in §5.1 is the part that needs judgement; the rest follows from it. Read the three candidates and their verdicts before writing code.
- The live hole in §2 is the reason this milestone is not cosmetic. A caller who sets `LANG` today gets an execution that was never confined, and only the terminal snapshot says so. If the implementation lands the single-source-of-truth change without closing the baseline-key collision, the milestone will have produced a tidier tree and the same hole.
- The repository already holds the pattern this milestone generalizes: `crates/eggwork-runner/tests/windows_process_tree.rs` keeps a hand-maintained `BASELINE_KEYS` list with the comment that a mismatch is a contract change, not a test fix. That comment is right and it is not enough. Prefer the test that fails when the two sides drift.
- The helper is the trusted artifact. Every choice in this milestone should be defensible as "the helper still ships alone, with a dependency surface an operator can read".
