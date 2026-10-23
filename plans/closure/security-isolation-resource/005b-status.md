# Security M005b — Sandbox-helper environment parity corrective

Status: implemented — closure record written, implementation commit pending

Source implementation plan:
`plans/implementation/security-isolation-resource/005b-sandbox-helper-environment-parity-corrective.md`

Source corrective addendum (authoritative where it disagrees with the plan):
`plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`
(finding SI-2)

Source roadmap:
`plans/subsystems/security-isolation-resource-roadmap.md` (M005b now
"implemented")

Reviewed repository head before implementation: `6f41957`
Implementation commit: `63699df`

Branch: `corrective/interrogation-batch-2026-10`. The working tree was left
dirty for the coordinator to review and commit; no commit was made by the
implementing agent.

## 1. Executive finding

The hole is closed. A caller that sets `LANG` — or any other node-owned key —
no longer produces a launch specification with two entries for that key, so the
helper no longer refuses the specification, so a best-effort execution no
longer answers that pre-target refusal by re-spawning the target with **no
Landlock ruleset at all**. The reserved-key rule was applied in both places the
runner builds an environment (`direct_command` and `SandboxChannel::new`), and
both places now call **one** builder, so the direct path and the helper path
cannot disagree about what a child sees.

The environment policy — the denied-key list and its matching rule, the
node-owned (reserved) keys, the node-owned baseline, and the `PATH` value
contract — now has exactly one owner:
`crates/eggwork-runner/policy/environment-policy.toml`. The runner compiles a
generated copy of it; the standalone sandbox helper, which may not depend on
any workspace crate, compiles a generated copy of the *same* source. The two
copies are pinned together by a parity test that is **behavioural**: it drives
the real helper executable with a launch specification for every probe key and
compares its accept/refuse decision against what the runner's copy of the
policy says. A one-sided edit to either copy fails that suite — demonstrated
below, in both directions.

The `Required` path is untouched. A required request still fails closed before
the target starts.

## 2. The mechanism, and why

**Chosen: one checked-in source of truth, generated copies in each crate, no
new dependency edge.**

`crates/eggwork-runner/policy/environment-policy.toml` is the source of truth.
`scripts/generate_environment_policy.py` (Python 3, standard library only, no
network) renders `crates/eggwork-runner/src/environment_policy.rs` and
`crates/eggwork-sandbox-helper/src/environment_policy.rs` from it. Both copies
are checked in. The runner's crate compiles its own copy; the helper's crate
compiles its own copy. No `build.rs` was added, no crate reads a file from
outside its own directory at build time, and `eggwork-sandbox-helper`'s
`[dependencies]` still contains only `serde` and `serde_json` — the helper
remains the standalone binary an operator can read the dependency surface of.

The alternative — a shared crate — was rejected because it would give the
helper a workspace-crate production dependency, which is exactly the
constraint this milestone has to respect. The alternative of reading the
policy file at runtime or at build time from a relative path was rejected
because it would make the helper's enforcement depend on a file's location and
would put a second copy of the policy's *content* in the tree.

**The generator is not the gate; the test is.** A hand edit of a generated copy
is a contract change, and the parity test — not the generator — is what makes
that visible. `scripts/generate_environment_policy.py --check` is a
convenience, and `tests/release/test_environment_policy.py` asserts that the
checked-in copies are byte-identical to the generator's output.

## 3. The generated-file contract

Both crates get a module named `environment_policy`, declared at the crate
root (`pub mod environment_policy;` in the runner's `lib.rs`, `mod
environment_policy;` in the helper's `main.rs`). The two modules expose the
same API, so the parity test can compare them element by element:

| Item | Signature | Meaning |
| --- | --- | --- |
| `DENIED_KEYS` | `pub const DENIED_KEYS: &[&str]` | the 21 denied keys, in policy order |
| `MATCH_RULE` | `pub const MATCH_RULE: &str` | `"ascii-uppercase-exact-or-prefix"` |
| `RESERVED_KEYS` | `pub const RESERVED_KEYS: &[&str]` | the 8 node-owned keys on the Unix baseline |
| `BASELINE_COMMON` | `pub const BASELINE_COMMON: &[(&str, &str)]` | the platform-independent part of the baseline |
| `BASELINE` | `pub const BASELINE: &[(&str, &str)]` | the node-owned Unix baseline, key → value |
| `PATH_VALUE` | `pub const PATH_VALUE: &str` | `"/usr/bin:/bin"` — the Linux/Unix `PATH` contract |
| `denied_environment_key` | `pub fn denied_environment_key(key: &str) -> bool` | case-insensitive exact-or-prefix deny match |
| `is_reserved_key` | `pub fn is_reserved_key(key: &str) -> bool` | case-insensitive node-owned match |
| `baseline_entry` | `pub fn baseline_entry(key: &str) -> Option<(&'static str, &'static str)>` | the node's own spelling and value for a key |
| `baseline_value` | `pub fn baseline_value(key: &str) -> Option<&'static str>` | the node's value for a key |

The helper's copy additionally carries the five protocol limit constants,
because `eggwork-core` owns the wire limits and the helper cannot import
them:

| Item | Value | Source |
| --- | --- | --- |
| `MAX_ARG_COUNT` | 256 | generated from `eggwork-core/src/lib.rs` |
| `MAX_ARG_BYTES` | 32768 | generated from `eggwork-core/src/lib.rs` |
| `MAX_ENV_COUNT` | 256 | generated from `eggwork-core/src/lib.rs` |
| `MAX_ENV_NAME_BYTES` | 256 | generated from `eggwork-core/src/lib.rs` |
| `MAX_ENV_VALUE_BYTES` | 16384 | generated from `eggwork-core/src/lib.rs` |

`MAX_SPEC_BYTES` is helper-only, has no `eggwork-core` counterpart, and stays
in the helper's own source.

**Are the five limit constants generated or duplicated? Generated.** The
generator reads them out of `crates/eggwork-core/src/lib.rs` — the owner of the
wire limits — and emits them into the helper's copy only; the runner uses
`eggwork_core::MAX_*` directly and never sees a second copy. The generator
evaluates the owner's arithmetic expressions (`32 * 1024`, `16 * 1024`) with a
small integer-only parser rather than `eval`. A parity test compares the
helper's copy against `eggwork-core` at run time, so a drift fails the build.

Each generated file carries a header naming the source of truth, naming the
generator, and stating that a hand edit is a contract change rather than a
local fix.

## 4. The reserved-key disposition, and the direct/helper disagreement

**The rule chosen: discard the caller's value and use the node's.** A
node-owned key is one the node's baseline sets; the caller's value for it is
discarded, and the node's value is what reaches the child and what appears in
the launch specification.

**How the direct-path versus helper-path disagreement was resolved.** Before
this milestone the two paths disagreed, and they disagreed in *opposite*
directions:

- the **direct path** installed the node's baseline and then applied the
  caller's environment on top, so the child saw the **caller's** `LANG`;
- the **launch-specification path** started from the baseline and then
  *appended* every non-denied caller key, so the specification carried
  **both** `LANG` entries — and the helper refuses a specification with
  duplicate keys.

So the direct path gave the caller the last word, and the helper path gave
the caller no word at all and refused the launch. Neither matched the
"node-owned" intent, and the second one is what cost a best-effort execution
its confinement.

Both paths now call one builder, `build_child_environment`, which applies the
three rules in a fixed order: the node's baseline first, then the caller's
entries with denied keys dropped and node-owned keys replaced by the node's
values and a repeated key resolved to its last value, then the provenance
entries. The two paths therefore cannot disagree, and the proof is
behavioural: `the_direct_path_and_the_helper_path_agree_about_the_environment`
runs the same request through `NoExecutionSetup` and through the real helper
and compares the child's actual environment.

**The node-owned class is platform-shaped.** It is the node's own baseline,
and the baseline differs by platform: on Unix it is the policy's
`RESERVED_KEYS`; on Windows the node owns `SystemRoot`, `windir` and `PATH`
instead of the Unix locale keys, and that branch is runner-local because its
values are read from the parent process at spawn time. The shared policy says
so in the same terms, so the statement cannot drift from the code. The
coordinator's fixed reserved set for the Unix baseline — `PATH`, `LANG`,
`LC_ALL`, `CI`, `NO_COLOR`, `TERM`, `GIT_TERMINAL_PROMPT`, `PAGER` — is
exactly what the policy carries.

**A repeated caller key.** `RunnerRequest::validate` does not reject a
repeated environment key, and `CommandSpec::validate` only rejects duplicates
within the caller's own environment, so a caller using the public
`set_environment` could produce one. The builder collapses a repeated key to
its last value, which is what `Command::env` already did on the direct path.
This is a behaviour change on the specification path: such a specification used
to be refused (status 12) and is now accepted with one entry. It is a change
in the safe direction — a refusal there is what triggered the unconfined
re-spawn — and it makes the two paths agree. Rejecting duplicate keys outright
is a request-validation decision and remains with the foundation
runner-request-validation corrective; see §9 finding S-04.

**Denied-key behaviour is unchanged and is deliberately divergent.** The runner
*drops* a denied key silently; the helper *refuses the whole launch*. Only the
**list** is shared; the **behaviour** is not. Both sides now state the
contract in the same terms, and both documents say the difference is
deliberate.

## 5. The helper's status byte, and the before-and-after reason text

The environment-policy refusal gets its own status byte, **13**, distinct from
the general specification-validation status **12**.

Status 13 was previously the root-canonicalization failure. It could not be
reused for two meanings without an operator being unable to tell them apart,
and the coordinator fixed the environment-policy refusal at 13, so the
root-canonicalization failure moved to **17**, which was unused. This is a
status-byte renumbering and it is recorded here as such; the runner's
interpretation of the bytes is unchanged (every byte `>= 4` is a pre-target
refusal), so both `13` and `17` fail closed before the target exists.

| Situation | Byte before | Byte after |
| --- | --- | --- |
| specification malformed in a way unrelated to the environment | 12 | 12 (unchanged) |
| environment denied key present | 12 | **13** |
| `PATH` is not the contract value | 12 | **13** |
| a node-owned key carries a non-node value or a different spelling | 12 (only duplicates were caught) | **13** |
| duplicate environment key | 12 | **13** |
| root could not be canonicalized | 13 | **17** |

The reason text the runner produces, from `classify_helper_status`:

- **before**: `sandbox helper rejected its launch specification (status 12)`
- **after, environment-policy refusal**: `sandbox helper rejected its launch specification (status 13)`
- **after, other pre-target refusal**: `sandbox helper rejected its launch specification (status 12)`

The runner classifies 13 as `SandboxWaitError::BeforeTarget(RunnerError::Setup(...))`
— a setup failure *before the target process exists* — exactly like 12.

## 6. The denied-key entry set, before and after

**Carried over unchanged.** The entry set, its order, and its matching rule
are the reviewed baseline's:

```text
LD_  DYLD_  PATH  IFS  SHELLOPTS  CDPATH  BASH_ENV  ENV
PYTHONPATH  PYTHONHOME  NODE_OPTIONS  RUBYOPT  PERL5OPT
GIT_ASKPASS  SSH_ASKPASS  GIT_CONFIG  GIT_CONFIG_
AWS_  AZURE_  GOOGLE_  SSH_AUTH_SOCK
```

21 entries, matched case-insensitively by exact name or prefix
(`ascii-uppercase-exact-or-prefix`). `GIT_CONFIG_` remains in the set even
though the `GIT_CONFIG` prefix already covers `GIT_CONFIG_GLOBAL` and its
relatives; it was carried over unchanged and removing it would be a policy
decision, not an ownership decision.

## 7. Requirement-to-evidence matrix

Acceptance criteria are numbered as in the plan's §11. "Executed" means the
command was run in this working tree and produced the stated result;
"inspected" means the property was established by reading code and is not
backed by a command run here.

| # | Criterion | Implementation site | Evidence | Status |
| --- | --- | --- | --- | --- |
| 1 | One owner for the denied-key list and the matching rule; the helper's copy is generated and checked in | `crates/eggwork-runner/policy/environment-policy.toml`; `scripts/generate_environment_policy.py`; `crates/eggwork-runner/src/environment_policy.rs`; `crates/eggwork-sandbox-helper/src/environment_policy.rs` | Executed: `python3 scripts/generate_environment_policy.py --check` → "the environment-policy copies are up to date"; `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release -p 'test_environment_policy.py'` → 5 passed, including `test_the_checked_in_copies_are_what_the_generator_produces` | Pass |
| 2 | The helper still declares no workspace-crate production dependency, and neither crate's build reads a file from outside its own crate directory | `crates/eggwork-sandbox-helper/Cargo.toml` (unchanged); no `build.rs` in either crate | Executed: `python3 scripts/check_execution_ownership.py` → "crate dependency direction passed"; `cargo build -p eggwork-sandbox-helper` → success. Inspected: the helper's `[dependencies]` is `serde` + `serde_json` only | Pass |
| 3 | A test compares the runner's policy and the helper's policy element-wise and rule-wise, and fails on a one-sided edit | `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs` | Executed: `cargo test -p eggwork-sandbox-helper --test environment_policy_parity` → 12 passed; the one-sided-edit demonstration in §8 | Pass |
| 4 | Negative evidence: a one-sided edit fails the build with the intended message | same | Executed: §8 — both directions, with the failure messages quoted | Pass |
| 5 | The runner drops denied keys and the helper refuses a launch containing one; both documented on both sides; the difference stated as deliberate | `build_child_environment` in `crates/eggwork-runner/src/lib.rs`; `linux::environment_policy_refuses` in `crates/eggwork-sandbox-helper/src/main.rs`; `architecture/sandbox-helper.md`; `architecture/runner-execution.md` | Executed: runner-side `a_caller_supplied_denied_key_does_not_reach_the_child` (5-test suite passed); helper-side `the_helper_refuses_exactly_what_the_runner_drops` (12-test suite passed) | Pass |
| 6 | `PATH == /usr/bin:/bin` is an enforced contract on both sides, sourced from the shared policy, with the Windows shape explicitly out of scope | `environment_policy::PATH_VALUE`; the runner installs it; `linux::environment_policy_refuses` checks it; `crates/eggwork-runner/policy/environment-policy.toml` `[path_contract] scope = "linux-unix"` | Executed: `the_matching_rule_and_the_path_contract_are_the_same_in_both_copies` and `the_path_contract_states_that_windows_is_outside_it` (both passed); the probe corpus carries `("PATH", "/usr/bin:/bin", false)` and `("PATH", "/tmp/bin", true)` | Pass |
| 7 | The node-owned baseline is written once, and `windows_process_tree.rs`'s `BASELINE_KEYS` is reconciled with the new owner | `crates/eggwork-runner/policy/environment-policy.toml` `[baseline.unix]` / `[baseline.windows_common]`; `crates/eggwork-runner/src/lib.rs` (`baseline_child_environment`, `platform_baseline`, `baseline_environment_keys`); `crates/eggwork-runner/tests/windows_process_tree.rs` (`baseline_keys()`) | Executed: `cargo test -p eggwork-runner --test environment_policy` → `the_installed_baseline_is_the_policy_baseline` passed. **The Windows half is code-inspected only** — see §9 finding S-02 | Pass (Linux) / partially inspected (Windows) |
| 8 | A caller-supplied reserved key no longer produces a duplicate launch-specification key, and no longer causes a best-effort execution to lose its confinement | `build_child_environment`; `SandboxChannel::new` uses the same builder | Executed: runner-side `the_launch_specification_carries_one_entry_per_node_owned_key` and `the_direct_path_and_the_launch_specification_agree` (25-test lib suite passed); helper-side `a_caller_supplied_node_owned_key_keeps_the_execution_confined` (12-test suite passed) asserts `SandboxOutcome::Applied` and exactly one `LANG=C.UTF-8` in the child's environment | Pass |
| 9 | The direct path and the helper path agree on every reserved key's value | `build_child_environment` is the single builder | Executed: `the_direct_path_and_the_helper_path_agree_about_the_environment` (12-test suite passed) compares the two children's actual environments, key by key, including every `RESERVED_KEYS` entry | Pass |
| 10 | The environment-policy refusal has a status byte distinct from the general specification-validation status, and the runner classifies it as a setup failure before the target | `crates/eggwork-sandbox-helper/src/main.rs` (`read_spec` → 13); `classify_helper_status` in `crates/eggwork-runner/src/lib.rs` | Executed: helper-side `a_duplicate_environment_key_is_refused_with_the_environment_policy_status` (13) and `a_malformed_specification_keeps_the_general_validation_status` (12) both passed; runner-side `the_helper_status_bytes_classify_a_refusal_before_the_target` passed | Pass |
| 11 | A required request still fails closed before the target process starts | unchanged; `Required` still fails closed | Executed: `cargo test -p eggwork-sandbox-helper --test landlock_runner` → 17 passed, including `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` (required request → error, marker never created) and `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` | Pass |
| 12 | The helper's five duplicated protocol limit constants agree with `eggwork-core`, by generation or by an explicit comparison test | generated into `crates/eggwork-sandbox-helper/src/environment_policy.rs` from `crates/eggwork-core/src/lib.rs` | Executed: `the_helper_limit_constants_match_eggwork_core` (12-test suite passed) | Pass |
| 13 | The denied-key entry set is unchanged from the reviewed baseline, and the closure record lists it | `crates/eggwork-runner/policy/environment-policy.toml` `[denied]` | Executed: `python3 scripts/generate_environment_policy.py --check` passes against the checked-in copies; the entry set is listed in §6. Inspected: the 21 entries are byte-identical to the pre-corrective list in both crates | Pass |
| 14 | No wire representation changed | the launch specification's *shape* is unchanged; only its `environment` array's content is now policy-conformant | Inspected: `SandboxLaunchSpec`'s fields and `LaunchSpec`'s fields are unchanged; `ExecutionEventKind`, `ExecutionResult`, `SandboxResult`, `ResourceResult` untouched. **The helper's status byte 13 is a process-level protocol change** — see §5 and §9 finding S-03 | Pass (with the status-byte note) |
| 15 | `architecture/sandbox-helper.md` and `architecture/runner-execution.md` match the code | both edited | Inspected: the status table lists every byte the helper writes, the environment-policy section states the one owner, the drop-versus-reject contract, the `PATH` contract and its Linux scope, the node-owned baseline and the generated-file convention; the runner document describes the denied-key drop, the reserved-key rule, the baseline and the launch-specification environment | Pass |
| 16 | The full pre-submit gate is green on the reviewed head | — | **Not claimed.** The scoped gate was run (§9); the full workspace gate is the coordinator's to run once at the end, as the milestone instructions require | Open (coordinator) |

## 8. Negative and adversarial evidence

### 8.1 A one-sided edit to the **runner's** copy fails the build

Editing `crates/eggwork-runner/src/environment_policy.rs` alone — the first
denied entry `LD_` → `LD_ONE_SIDED` — and running
`cargo test -p eggwork-sandbox-helper --test environment_policy_parity`:

```text
---- the_denied_key_list_is_the_same_list_in_the_same_order stdout ----
assertion `left == right` failed: denied-key entry 0 differs: the runner's copy
has "LD_ONE_SIDED" and the helper's has "LD_". A mismatch is a contract change,
not a test to be adjusted

---- linux::the_helper_refuses_exactly_what_the_runner_drops stdout ----
assertion `left == right` failed: the runner's policy and the probe table
disagree about LD_PRELOAD="/tmp/evil.so"; one of them is wrong about the contract

---- linux::a_caller_supplied_node_owned_key_keeps_the_execution_confined stdout ----
the execution runs: Setup("sandbox helper rejected its launch specification (status 13)")

---- linux::the_direct_path_and_the_helper_path_agree_about_the_environment stdout ----
the confined execution runs: Setup("sandbox helper rejected its launch specification (status 13)")

test result: FAILED. 8 passed; 4 failed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Four tests failed. The element-wise test named the first differing entry; the
behavioural test caught the *rule* divergence independently — and note what the
third and fourth failures show: with the runner's copy no longer denying
`LD_PRELOAD`, the runner's specification carries it and the helper refuses
the launch. That is the divergence doing real damage, which is why the
behavioural half of the gate matters.

### 8.2 A one-sided edit to the **helper's** copy fails the build

Editing `crates/eggwork-sandbox-helper/src/environment_policy.rs` alone — the
same change — and running the same command:

```text
---- the_denied_key_list_is_the_same_list_in_the_same_order stdout ----
assertion `left == right` failed: denied-key entry 0 differs: the runner's copy
has "LD_" and the helper's has "LD_ONE_SIDED". A mismatch is a contract change,
not a test to be adjusted

---- linux::the_helper_refuses_exactly_what_the_runner_drops stdout ----
assertion `left == right` failed: the helper must refuse LD_PRELOAD="/tmp/evil.so"
with the environment-policy status
  left: 1
 right: 13

test result: FAILED. 10 passed; 2 failed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Both edits were reverted, and `python3 scripts/generate_environment_policy.py
--check` then reported "the environment-policy copies are up to date" and the
suite was green again (12 passed).

### 8.3 The behavioural parity sweep over the probe corpus

`the_helper_refuses_exactly_what_the_runner_drops` drives the **real** helper
executable with a launch specification for each of 37 probe entries and
compares three things: the probe table's stated expectation, what the
runner's copy of the policy says, and what the helper actually does. The
corpus has 40 entries and covers every denied entry (exact names, a prefix,
and a lower-case spelling), every node-owned key with the caller's value,
every node-owned key with the node's value, `PATH` with and without the contract
value, a lower-case spelling of a node-owned key, and two caller-controlled
keys.

This is the behavioural half of the gate. It is not a structural comparison of
two files: it measures what the helper *enforces*.

### 8.4 The baseline-key collision

The regression for the live defect is
`a_caller_supplied_node_owned_key_keeps_the_execution_confined`. It drives the
real runner with the real helper and a request that sets `LANG`,
`LC_ALL`, `CI`, `TERM`, `PAGER`, `NO_COLOR`, `GIT_TERMINAL_PROMPT`, `PATH`
and `LD_PRELOAD` to caller values, and asserts:

- the sandbox outcome is `SandboxOutcome::Applied` — **not** `NotApplied`;
- the child saw exactly one `LANG`, and it was `LANG=C.UTF-8`;
- the caller's `en_US.UTF-8` appears nowhere in the child's environment.

Before this milestone the same request produced a specification with two
`LANG` entries, the helper refused it with status 12, and a best-effort
request answered that refusal by re-spawning the target with no Landlock
ruleset — a successful, entirely unconfined execution. The `Required` variant
of the same request fails closed instead, which is the unchanged `Required`
behaviour.

The specification-side half of the same proof is the runner-side unit test
`the_launch_specification_carries_one_entry_per_node_owned_key`, which reads
the specification the runner actually wrote and asserts one entry per
node-owned key, carrying the node's value.

### 8.5 The generator refuses a malformed source of truth

`test_a_malformed_source_of_truth_fails_without_writing_anything` corrupts
the policy in a temporary tree (malformed TOML; an unknown schema version) and
asserts the generator exits non-zero **and** writes neither copy. A generator
that wrote half a policy and exited zero would be worse than one that
refuses. `test_a_policy_whose_class_and_baseline_disagree_fails` corrupts the
node-owned class and asserts the generator refuses it with the message about
the node-owned class.

### 8.6 The ownership guard

`python3 scripts/check_execution_ownership.py` → "execution ownership guard
passed; approved process owners: eggwork-runner (2 spawn sites),
eggwork-sandbox-helper (1 spawn sites)" and "negative scanner self-test
passed; crate dependency direction passed".
`python3 scripts/check_execution_ownership.py --prove-negative-exit` → passed.
The two generated Rust files live under `crates/*/src/`, so they are scanned by
the guard; they contain no process-creation API.

## 9. Verification actually run

Executed in this working tree, with
`CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w2a`:

| Command | Result |
| --- | --- |
| `cargo test -p eggwork-runner --lib` | **passed** — 25 passed, 0 failed, 1 ignored |
| `cargo test -p eggwork-runner --test environment_policy` | **passed** — 5 passed, 0 failed |
| `cargo test -p eggwork-runner --test capability_probe` (pre-existing, unmodified) | **passed** — 2 passed, 0 failed |
| `cargo test -p eggwork-sandbox-helper --lib` | **error** — "no library targets found in package `eggwork-sandbox-helper`". The crate has a `[[bin]]` target only and no `[lib]`; this is pre-existing and not changed by this milestone. The helper has no library to unit-test. |
| `cargo test -p eggwork-sandbox-helper --test landlock_runner` | **passed** — 17 passed, 0 failed |
| `cargo test -p eggwork-sandbox-helper --test environment_policy_parity` | **passed** — 12 passed, 0 failed |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release -p 'test_environment_policy.py'` | **passed** — 5 passed, 0 failed |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` (whole release-harness suite) | **passed** — 57 tests, OK (2 skipped) |
| `cargo fmt -p eggwork-runner -p eggwork-sandbox-helper -- --check` | **passed** — no diff |
| `cargo clippy -p eggwork-runner -p eggwork-sandbox-helper --all-targets -- -D warnings` | **passed** — no warnings |
| `python3 scripts/check_execution_ownership.py` | **passed** |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | **passed** |
| `python3 scripts/generate_environment_policy.py --check` | **passed** — "the environment-policy copies are up to date" |
| `cargo build -p eggwork-sandbox-helper` | **passed** |
| `cargo check -p eggwork-server` | **passed** — the server crate still compiles against the changed runner |
| `git diff --check` | **passed** — no whitespace errors |

**Not run here, and therefore not claimed:** `cargo test --workspace`,
`cargo fmt --all`, `cargo clippy --workspace`, and the full pre-submit gate.
The milestone instructions assign the full gate to the coordinator, who runs it
once at the end; acceptance criterion 16 stays open until then.

**Cross-target checks were not attempted.** Only the Linux toolchain is
installed here.

## 10. Platform matrix actually exercised

| Platform | Evidence | Claim |
| --- | --- | --- |
| Linux x86_64 (`6.8.0-147-generic`, rustc 1.89.0) | The suites in §9 were executed on this host. The behavioural parity sweep drove the **real** helper binary with Landlock enforcement available, and the end-to-end regression drove the real runner with a trusted helper staged from the Cargo build output. | The environment policy, the reserved-key rule, the `PATH` contract, the two generated copies and the status byte are qualified on this host class. |
| macOS | No macOS host and no cross-compilation in this environment. Code inspection only: the runner's Unix baseline is the policy's `BASELINE` on every non-Windows target, and the helper has no macOS path at all (`#[cfg(target_os = "linux")] mod linux`), so the helper's behaviour cannot differ on macOS. | **Not qualified.** No claim is made beyond "the runner's Unix baseline is a single generated constant". The pre-existing macOS clippy failure in `crates/eggwork-runner/src/process_tree.rs` (recorded as S-06, and *not* this milestone's to fix) also means the macOS cross-target clippy check could not have been used as evidence here. |
| Windows | No Windows host and no cross-compilation in this environment. The Windows branch (`platform_baseline`) is code-inspected only: it reads `BASELINE_COMMON` from the shared policy and adds `SystemRoot`, `windir` and the Windows `PATH` from the parent process. The Windows tree suite's `BASELINE_KEYS` was replaced by a call to the runner's own `baseline_environment_keys()`, but the file is `#![cfg(windows)]` and **was not compiled or run here**. | **Not qualified.** Compilation is not qualification. The Windows row stays open until the `windows-runner` CI job runs the suite green — see §9 finding S-02. |

**No platform is claimed as qualified merely because its code compiles.**

## 11. Public-API change and its release-visibility consequence

Two additions to `eggwork-runner`'s public API, both additive:

| Item | Signature |
| --- | --- |
| `eggwork_runner::environment_policy` | a new public module, exposing the ten items in §3 |
| `eggwork_runner::baseline_environment_keys` | `pub fn baseline_environment_keys() -> Vec<&'static str>` |

Nothing was removed and no existing public item changed shape or semantics.
The runner's private `denied_environment_key` function was **replaced** by
`environment_policy::denied_environment_key`; it was private, so this is not a
public change.

**Release-visibility consequence.** The new public module and the new public
function are additive API in a published crate (`eggwork-runner` 0.1.5). They
do not break a downstream consumer, but they are new public surface and should
be mentioned in the release notes. The milestone's ownership does not include
any `Cargo.toml`, so **the version bump was not made** — see §9 finding S-03.

**A process-level protocol change.** The helper's status byte 13 is new and
the root-canonicalization byte moved from 13 to 17. The runner and the helper
ship in the same release bundle ("Linux releases ship the daemon and its
Landlock sandbox helper as one bundle from one source revision, so the pair
can never drift"), so a runner that understands 13 and 17 must never meet a
helper that writes the old 13. The bundle rule already guarantees this, but
the change is worth a line in the release notes: see §9 finding S-03.

## 12. Documentation status

| Document | Change |
| --- | --- |
| `architecture/sandbox-helper.md` | The "Environment handling" section was rewritten: one owner and two generated copies, the drop-versus-reject contract table, the node-owned baseline and the collision hole it closes, the `PATH` contract and its Linux scope, and the surviving environment. A full status-byte table was added, including the new byte 13 and the moved byte 17. The "Platform support", "Test coverage", "Invariants and enforcement" and "Review focus" sections were updated. **Every stale line-number citation in this file was replaced with a symbol reference in the same pass** — the file now contains no `file.rs:NNN` citation. |
| `architecture/runner-execution.md` | "Environment clearing and the conservative baseline" was rewritten: the single builder, the three classes, the reserved-key rule and the collision hole, the `PATH` contract and its Linux scope, and the provenance entries. The `TrustedLandlockSetup` section, the responsibility boundary, the invariants table and the test-coverage section were updated, and the "Env baseline duplication" review-focus bullet was rewritten to say what the fix does and does not do. |
| `architecture/overview.md` | **Not edited — the coordinator owns it.** See §13 for the recommendation. |
| `architecture/data-plane.md` | **Not edited — the coordinator owns it.** See §13. |
| `AGENTS.md` | **Not edited — the coordinator owns it.** See §13 for the recommended wording of the "Isolation claims are earned" line. |
| `plans/subsystems/security-isolation-resource-roadmap.md` | The M005b entry's status was set to "implemented — closure record written, implementation commit pending" and the closure-evidence pointer was added, matching the M005a entry's shape. The roadmap's status line was updated to "closed through M004 (remote-admission C001 and Operations M002 included); M005a and M005b implemented (closure records written, implementation commits pending); M005c blocked on ADR-0007". |
| `plans/registry.md` | **Not edited — the coordinator owns it.** See §14. |
| `plans/implementation/security-isolation-resource/005b-sandbox-helper-environment-parity-corrective.md` | **Not edited** — the plan is not in this milestone's ownership and is left as the authoritative statement of what was asked. |

New text in the deep dives uses symbol references rather than line numbers, so
it does not add to the repository's stale-line-number defect.

## 13. Unresolved findings, ranked by severity

**Critical: none. High: none.**

- **S-01 — Medium — owner: the coordinator (`architecture/overview.md`,
  `architecture/data-plane.md`, `AGENTS.md`, `plans/registry.md`).** Those four
  files are outside this milestone's ownership and were not edited. The
  recommended text is in §14.
- **S-02 — Medium — owner: the release owner / CI owner.** The Windows half of
  this milestone is **code-inspected only**. `crates/eggwork-runner/src/lib.rs`'s
  Windows branch (`platform_baseline`) and the rewritten
  `crates/eggwork-runner/tests/windows_process_tree.rs` (`baseline_keys()`)
  were neither compiled nor run here — the suite is `#![cfg(windows)]` and
  there is no Windows toolchain in this environment. The `windows-runner` CI
  job's `cargo test -p eggwork-runner --all-targets` is what will exercise it.
  Until that runs, the Windows row of §10 stays "not qualified".
- **S-03 — Medium — owner: the coordinator / release owner.** Two
  release-visible items: the new public `eggwork_runner::environment_policy`
  module and `baseline_environment_keys`, and the helper's status-byte change
  (new byte 13; root-canonicalization moved from 13 to 17). No `Cargo.toml` is
  in this milestone's ownership, so the version bump was not made, and the
  status-byte change needs a release-note line. The daemon and the helper ship
  as one bundle, so a mixed pair cannot reach an operator — but the note is
  still owed.
- **S-04 — Low — owner: the foundation/runner owner (NOT this milestone).**
  A repeated environment key is now collapsed to its last value by the
  environment builder rather than refused. Rejecting duplicate keys outright
  is a *request-validation* decision and belongs to the foundation
  runner-request-validation corrective, which the plan lists as out of scope.
  Until then, the builder's collapsing is what keeps the specification free of
  duplicates. The builder's collapsing also matches what `Command::env`
  already did on the direct path, so the two paths agree.
- **S-05 — Low — owner: the security-isolation subsystem owner.** The
  node-owned class is platform-shaped, and the shared policy defines the Unix
  class only. On the Windows baseline the node owns `SystemRoot`, `windir` and
  `PATH` instead of the Unix locale keys, and the Windows branch is
  runner-local. That is stated in the policy file and in both deep dives, but
  it means a reader who takes `RESERVED_KEYS` as platform-neutral would be
  wrong on Windows. The policy file says so explicitly.
- **S-06 — Low — owner: the foundation/runner owner (NOT this milestone).**
  `crates/eggwork-runner/src/process_tree.rs`'s `ProcessTree::start_kill` is
  called only from the Linux-only helper-handshake paths, so on macOS it is
  dead code and `cargo clippy --target x86_64-apple-darwin -- -D warnings`
  fails. This is **pre-existing at the reviewed head `6f41957`** and
  `process_tree.rs` is not in this milestone's ownership, so it was not
  touched. It is recorded because it blocks the macOS cross-target clippy
  check that §10's platform matrix would otherwise use.
- **S-07 — Low — owner: the security-isolation subsystem owner.** The
  deny-list is still a *closed* list. The duplication is gone, so the list is
  now written once — but a new interpreter or loader knob (`PERL5LIB`,
  `RUBYLIB`, `NODE_PATH`, `GLIBC_TUNABLES`, `PROMPT_COMMAND`) is not covered,
  and adding one is a policy decision with its own security review. Removing
  the duplication did not make the list exhaustive.

## 14. Coordinator-owned text

**`architecture/overview.md` — recommendation: no divergence row.** The
duplication this milestone set out to close is closed: the policy has one
owner, the two copies are generated and pinned by a behavioural parity test,
the reserved-key collision is closed, and the two launch paths agree. The
plan's §10 says to add a row "only if a residual divergence remains after
M005b. If the duplication is closed, do not add a row", and it is closed. If
the coordinator wants the status-byte change visible from the bird's-eye view
anyway, the exact text would be:

```markdown
| 7 | The helper's environment-policy refusal has its own status byte | `crates/eggwork-sandbox-helper/src/main.rs` (`linux::read_spec`, `linux::environment_policy_refuses`) | `architecture/sandbox-helper.md` § "The status-byte table"; byte 13 is the environment-policy refusal and byte 17 is the root-canonicalization failure |
```

It must not touch the existing rows.

**`architecture/data-plane.md` — recommendation: no change.** Nothing in the
data plane — blobs, workspaces, artifacts, GC — is touched by this milestone,
and no wire representation changed. The one adjacent fact, that the launch
specification's `environment` array is now policy-conformant by construction,
belongs in `architecture/sandbox-helper.md` and `architecture/runner-execution.md`,
where it is stated. No row or paragraph is warranted.

**`AGENTS.md` — recommendation: extend one hard-invariant line.** The
"Isoolation claims are earned" line does not need a new entry, but it can name
the environment policy's single owner, in the same style and keeping it true:

```markdown
4. **Isolation claims are earned.** Advertise filesystem isolation only when `verify_trusted_helper` accepts the installed helper; the same function backs capability advertisement, `doctor`, and admission. The node's environment policy has one owner — `crates/eggwork-runner/policy/environment-policy.toml` — generated into each crate and pinned by a parity test, so the runner and the sandbox helper cannot enforce different rules.
```

The alternative — leaving the line untouched — is also defensible; the
invariant is not weakened by this milestone either way.

**`plans/registry.md` — recommendation: register M005b as implemented.** The
registry is the coordinator's file and was not edited. The coordinator's action
is to point M005b at
`plans/closure/security-isolation-resource/005b-status.md`, mark it
implemented, and fill the real commit SHA in this record and in
the roadmap entry. **Done:** the implementation commit is `63699df`.

## 15. Registry and roadmap disposition

- `plans/subsystems/security-isolation-resource-roadmap.md`: M005b —
  `proposed` → **implemented** (closure record written, implementation commit
  pending). The status line was updated to match. The milestone's exit
  conditions were **not** rewritten: each one is now satisfied by a named
  implementation site and a named test, as §7 maps them. The objective text
  was left as written, because the defect it describes is what the milestone
  set out to close and the closure record says it is closed.
- `plans/registry.md`: **not edited** — the registry is the coordinator's
  file. See §14.
- `plans/implementation/security-isolation-resource/005b-sandbox-helper-environment-parity-corrective.md`:
  **not edited** — the plan is not in this milestone's ownership and is left
  as the authoritative statement of what was asked.

## 16. What this milestone did not do

- It did not change the **content** of the denied-key list. The 21 entries
  were carried over unchanged, including the redundant `GIT_CONFIG_`.
- It did not add a workspace-crate dependency to the helper, add a `build.rs`,
  or make either crate read a file outside its own directory at build time.
- It did not unify the drop-versus-reject behaviour. The runner still drops a
  denied key silently and the helper still refuses the launch; only the list
  is shared.
- It did not change the `Required` path, the wire representation, the
  authentication, authorization, lease, generation, fencing or digest bounds,
  the release version, or any `Cargo.toml`.
- It did not add node selection, a queue, priority, fairness, a DAG, or
  semantic retry.
- It did not run the full workspace gate, cross-compile, or touch the
  coordinator-owned files listed in §14.
