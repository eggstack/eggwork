# Foundation M005 Closure — Runner Request Validation Parity Corrective

Status: active

Source plan: `plans/implementation/foundation-execution-core/005-runner-request-validation-parity-corrective.md`

Corrective addendum: `plans/subsystems/foundation-execution-core-request-validation-corrective-addendum.md`

Roadmap: `plans/subsystems/foundation-execution-core-roadmap.md`

## Reviewed baseline

- Reviewed head before implementation: `af5a21709573e20f26eca77df83bd5560c46e936` ("docs(plans): accept ADR-0007 — implement resource-scoped grants").
- Implementation commits: **none yet**. The implementation is in the working tree; the coordinator commits.
- Workspace version at the reviewed head: `0.1.5`.

## Executive finding

The runner no longer carries a second copy of the domain validation rules. Before this milestone the runner re-spelled the `eggwork-core` rules in a private helper, and the two copies disagreed: the runner's copy accepted environment names containing `-` or `.`, accepted a name beginning with a digit, and did not detect duplicate environment names at all — while `eggwork-core` rejected all three. The runner's copy was the one gating the actual `Command::env()` call on the run path. Worse, `RunnerRequest::new` and `RunnerRequest::set_environment` reached that weaker gate without passing through core validation at all, which contradicted the runner crate's own documented claim that "Request fields remain private so callers cannot bypass protocol bounds".

After this milestone:

- The argv, environment-entry, timeout, stdin, output-policy and resource rules each have exactly one owner, in `eggwork-core`, and the runner **consumes** them rather than re-deriving them.
- The runner is never weaker than core on any shared rule, and is deliberately stricter on exactly one rule — an empty argv entry — which is documented in the source with its reason and covered by a test.
- Every public construction path converges on the same enforcement point. `RunnerRequest::contract_error()` reports the first violation in a request without running anything, and `RunnerRequest::validate` refuses the request with the same typed error, before setup and before any child exists.
- The runner's local `StdinPolicy` duplicate — the enum, its `Debug` impl, its copy of the stdin bound, and the hand-written conversion in `from_spec` — is gone.
- The failure keeps its attribution: `RunnerError::InvalidRequest` now carries the typed `eggwork_core::ValidationError` instead of a rendered string, so "environment name with invalid syntax" and "duplicate environment name" are two distinguishable outcomes rather than one message.

**The public-API disposition is (b), not (a).** The plan preferred removing `RunnerRequest::new` and `RunnerRequest::set_environment`, and preferred that option "if the repository has no in-tree caller". The in-tree caller set is **not** empty — five test files across two crates — and two of them live in `crates/eggwork-sandbox-helper/tests/`, which is outside this milestone's file ownership and outside the plan's stated scope ("the `eggwork-sandbox-helper` crate … does not construct runner requests"). That premise is true of the helper's *production* crate, which has no workspace-crate dependency, but **not** of its integration tests, which take `eggwork-runner` as a dev-dependency and drive the runner's public API. Removing the constructor would therefore have broken the build rather than hardened it. The builder path is kept and covered instead.

## Requirement-to-evidence matrix

| Requirement | Implementation / evidence |
|---|---|
| §5.1 — the environment-entry rules have one owner in core, in a form the runner can consume | `eggwork_core::validate_environment_entries(&[EnvironmentEntry])` is public and owns the list-count bound, the per-entry name/value rules and the duplicate-name rule. `eggwork_core::CommandSpec::validate` calls it, so there is one implementation, not two. `EnvironmentEntry::new` was already public and is consumed unchanged. The runner calls the same function (`crates/eggwork-runner/src/lib.rs`, `RunnerRequest::contract_violation`). |
| §5.2 — the runner consumes the core rules rather than restating them; the empty-argv rule is kept, documented and tested | The runner's private environment check, its argv bounds check, its output-bounds check, its timeout check and its stdin check are all deleted. The runner's own argv rule survives as `RunnerRequest::argv_entry_violation`, with the reason (an empty argument is a child-process construction hazard, not a protocol concern) in the source comment, and is pinned by `the_runner_refuses_an_empty_argv_entry_which_core_permits`. |
| §5.3 — the local `StdinPolicy`, its `Debug` impl, its `validate` and the hand-written conversion are removed | The enum, the `Debug` impl, `StdinPolicy::validate` and the `match stdin { CoreStdinPolicy::… }` conversion in `from_spec` are deleted. The runner now holds `eggwork_core::StdinPolicy` directly. The public path `eggwork_runner::StdinPolicy` survives as `pub use eggwork_core::StdinPolicy;` (see "Public-API compatibility" below). |
| §5.4 — the core output-policy bounds are consumed | The runner's `capture_limit`/`event_chunk_bytes` check is deleted; the runner builds the core `OutputPolicy` (`RunnerRequest::output_policy`) and calls core's `OutputPolicy::validate`. The runner's default `capture_limit`/`event_chunk_bytes` are still core's `MAX_CAPTURE_BYTES`/`MAX_EVENT_CHUNK_BYTES`. |
| §5.5 — the public construction path is guarded | Option (b): the builder path is kept and covered. `RunnerRequest::contract_error()` reports the first violation without running; `RunnerRequest::validate` refuses the same request with the same typed error before setup and before any child exists. The caller enumeration that forced this choice is in "Public-API compatibility" below. |
| §5.6 — error fidelity is preserved | `RunnerError::InvalidRequest` now carries `eggwork_core::ValidationError` instead of `String`. Each rule keeps its own variant and `field` label, so a too-long environment name and a duplicate environment name are distinguishable *by type*, not by parsing a message. |
| §5.7 — the runner's own contract rules survive unchanged | The root canonicalisation, the "root is a directory" check and the "cwd escapes root" check are byte-for-byte unchanged in `RunnerRequest::validate`, and they remain runner-owned: core never sees a filesystem. |
| §4.1 — a request-validation rule has exactly one owner in core | `validate_argv`, `validate_environment_entries` and `validate_timeout` are public in core and are called by both `CommandSpec::validate` and the runner. `StdinPolicy::validate`, `OutputPolicy::validate` and `ResourceRequirements::validate` were already public and are now consumed by the runner rather than restated. |
| §4.2 — the runner is never weaker than core on a shared rule | `every_public_construction_path_reaches_the_same_contract_verdict` asserts, for fifteen single-violation inputs, that `from_spec` rejecting implies the builder path reports the *same typed error*. |
| §4.3 — stricter runner-level rules are documented and tested | Exactly one such rule exists (empty argv entry); documented in `RunnerRequest::argv_entry_violation` and in `architecture/runner-execution.md`; tested by `the_runner_refuses_an_empty_argv_entry_which_core_permits`, which also asserts that core *permits* the same input. |
| §4.4 — no public runner API can produce a request that violates the core contract | Every public entry point is covered by `RunnerRequest::contract_violation`, which the enforcement point uses. `the_runner_request_public_surface_is_pinned` fails if a new public entry point is added to `RunnerRequest`. `the_mutable_resource_door_is_covered_by_the_contract` covers the one door (`resource_setup_request_mut`) that cannot validate on the way in. |
| §4.5 — the argv is an argument vector, not a shell string | Unchanged; no shell is involved and the runner was already stricter than core on the empty-entry case. |
| §4.6 — the runner's bounds are core's bounds | No numeric bound was changed. The runner now takes every bound from core's constants and core's `validate` functions rather than restating them. |
| §4.7 — core has no dependency edges | `crates/eggwork-core/Cargo.toml` was not touched. |
| §4.8 — the runner owns process creation | No process-creation call was added or removed. `scripts/check_execution_ownership.py` still reports two runner-owned and one helper-owned spawn site. |
| §6 — no wire change, no stored-format change, no serde change, no bound change | No type's `Serialize`/`Deserialize` representation was touched. `CommandSpec`, `ExecutionSpec` and every wire type are unchanged. `RunnerError`'s payload type changed (see "Public-API compatibility"), but `RunnerError` is not a wire type and is not serialized. |
| §6 — validation still happens before any child exists, on the same path, at the same point | `LocalProcessRunner::run` still calls `request.validate()?` at the top, unchanged in position. The refusal is now typed. `assert_refused_before_spawn` proves no child was created, using a marker file the target would have written. |
| §6 — `CancelledBeforeSpawn` still takes precedence over validation | Unchanged and verified by inspection at `crates/eggwork-runner/src/lib.rs`: `run` checks `cancellation.is_cancelled()` and returns `CancelledBeforeSpawn` before it reaches `request.validate()`. The existing test `cancellation_before_spawn_and_spawn_failure_are_typed` still passes. |
| §6 — timeout, cancellation, nonzero exit, output-limit termination and cleanup failure remain distinguishable | Untouched: `TerminationReason` still carries `Exited`/`TimedOut`/`Cancelled`/`OutputLimit`, and `RunnerError::Io`/`Wait` are unchanged. `RunnerError::InvalidRequest` stays in the "refused before any side effect" tier. |
| §7 — the compatibility surface is enumerated and the disposition is recorded | See "Public-API compatibility" below. |
| §8.1 — construction-path parity | `every_public_construction_path_reaches_the_same_contract_verdict` (15 inputs × 2 paths). `the_runner_request_public_surface_is_pinned` additionally fails if a new public construction path is added. |
| §8.2 — environment-name syntax, through the runner's public API | `the_runner_rejects_every_environment_name_form_core_rejects` covers nine refused name forms (dash, dot, space, `=`, leading digit, leading zero, empty, NUL, one byte over `MAX_ENV_NAME_BYTES`) through `RunnerRequest::new` + `set_environment` *and* through `from_spec`, asserting the typed `InvalidSyntax { field: "environment name" }` and that no child was created. `a_legal_environment_name_is_accepted_by_the_runner` covers the accepted side. |
| §8.3 — duplicate environment names, attributable to the duplicate rule | `a_duplicate_environment_name_is_refused_by_the_duplicate_rule` asserts `InvalidSyntax { field: "duplicate environment name" }` specifically — not the generic name rule — through the builder path, through `run`, and through `from_spec`. |
| §8.4 — the accepted and rejected sets partition the input space | `crates/eggwork-core/tests/environment_rule_partition.rs`: three tests over the name set, the value set and the list set. The name predicate is re-derived independently of the implementation and asserted to agree on every candidate; the accepted and rejected sets are then asserted to be disjoint and to cover the whole candidate space, and both sides are asserted non-empty. |
| §8.5 — the runner's stricter rules | `the_runner_refuses_an_empty_argv_entry_which_core_permits`. |
| §8.6 — the stdin policy still works and the bound is core's | `the_runner_forwards_stdin_bytes_and_enforces_the_core_stdin_bound` (the compile-time proof that `eggwork_runner::StdinPolicy` *is* `eggwork_core::StdinPolicy`, plus the bound at and over the limit) and `stdin_bytes_are_forwarded_to_the_child`. |
| §8.7 — the output bounds still behave | `the_runner_enforces_the_core_output_bounds`: over-large capture limit, zero event chunk and over-large event chunk are refused by the core output rule; a policy exactly at the bounds is accepted. |
| §8.8 — no behavioural regression | The pre-existing runner, core and sandbox-helper suites are green — see "Verification performed". |
| §10 — documentation | `architecture/runner-execution.md` rewritten for the new ownership, and all of its stale line-number citations replaced with symbol references. `architecture/core-domain.md` records that core now owns the environment-entry rules the runner consumes, and the rule-bearing citations in it were replaced with symbols. `AGENTS.md` needs no change: it already says "Construct validated state behind private fields (`RunnerRequest::from_spec`)" and `from_spec` remains the protocol path. |

## Public-API compatibility

### In-tree caller enumeration

Every in-tree caller of every item whose public shape changed, as of the reviewed head:

`RunnerRequest::new` — **kept**, signature unchanged:

| Caller | Kind |
|---|---|
| `crates/eggwork-runner/tests/environment_policy.rs:35` | test, runner crate |
| `crates/eggwork-runner/tests/diagnostic_degradation.rs:100,156,186,227,258,278,307` | test, runner crate |
| `crates/eggwork-runner/tests/windows_process_tree.rs:143` | test, runner crate, `#![cfg(windows)]` |
| `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:42` | test, sandbox-helper crate (dev-dependency on the runner) |
| `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs:608` | test, sandbox-helper crate (dev-dependency on the runner) |

`RunnerRequest::set_environment` — **kept**, signature unchanged:

| Caller | Kind |
|---|---|
| `crates/eggwork-runner/tests/environment_policy.rs:36` | test, runner crate |
| `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:50` | test, sandbox-helper crate |
| `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs:609` | test, sandbox-helper crate |

`eggwork_runner::StdinPolicy` — the local type is **deleted**; the path is **kept** as a re-export of `eggwork_core::StdinPolicy`:

| Caller | Kind |
|---|---|
| `crates/eggwork-runner/tests/environment_policy.rs:26` (`use eggwork_runner::{… StdinPolicy …}`) | test, runner crate |
| `crates/eggwork-runner/tests/windows_process_tree.rs:38` | test, runner crate, `#![cfg(windows)]` |
| `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:16` | test, sandbox-helper crate |
| `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs:242` | test, sandbox-helper crate |

`RunnerError::InvalidRequest` — **payload type changed** from `String` to `eggwork_core::ValidationError`:

| Caller | Kind |
|---|---|
| none in-tree | the server maps `RunnerError` by variant and never names `InvalidRequest`; no test constructs or matches it |

Production callers of the runner's request API: **one**, `crates/eggwork-server/src/lib.rs:1907`, which uses `RunnerRequest::from_spec` only. The server never uses the builder path, so the server's behaviour is unaffected by the builder-path changes.

### Out-of-tree embedder position

**Unknown, and stated as unknown.** The runner crate is part of the released artifact set (`0.1.5`), and the plan's §7 treats it as a published crate with an external embedding surface. This repository contains no evidence of who embeds it outside the tree — no consumer manifest, no downstream workspace, no published-crate download record. Nothing in the reviewed tree establishes that the API is free to change, so it is not assumed free. The compatibility position is stated from the code, not from an assertion that there are no embedders.

### Disposition, item by item

| Item | Disposition | Breaking? |
|---|---|---|
| `RunnerRequest::new` | **kept**, signature unchanged; now covered by the core contract at the enforcement point, and reportable without running via `contract_error()` | No |
| `RunnerRequest::set_environment` | **kept**, signature unchanged; same | No |
| `RunnerRequest::from_spec` | **kept**; its error mapping now carries the typed core error | No (the error *payload* type changed — see the next row) |
| `eggwork_runner::StdinPolicy` | local type **deleted**; the path survives as a re-export of `eggwork_core::StdinPolicy` | No for the common case: the path still resolves, the variants have the same names and shapes, and the new type is a superset (it additionally derives `Serialize`/`Deserialize`). A downstream crate that relied on the runner's local type being *distinct* from core's would now see them unify — that is the intent, and it is a source-compatible change for every use found in-tree. |
| `RunnerError::InvalidRequest` | payload changed from `String` to `eggwork_core::ValidationError` | **Yes** — a published crate's public error type changed shape. A downstream crate that constructs `RunnerError::InvalidRequest("…".into())` or matches it as `InvalidRequest(s)` and uses `s` as a `String` will not compile. No in-tree caller does. This is a **breaking change to a published crate**: the release notes and the version must reflect it. The version bump and the release notes are the coordinator's; they are not part of this milestone. |
| `RunnerRequest::contract_error` | **added** | No (additive) |
| `eggwork_core::validate_argv`, `::validate_environment_entries`, `::validate_timeout` | **added**, public | No (additive) |

### Why option (a) was not taken

Option (a) — removing `RunnerRequest::new` and `RunnerRequest::set_environment` — would have required migrating five in-tree test files, two of them in `crates/eggwork-sandbox-helper/tests/`, which this milestone does not own and the plan places out of scope. It would also have made the remaining `set_*` setters unreachable, since a request could no longer be built without `new`. The plan's stop condition ("Removing the public constructor or setter would break a known in-tree caller that cannot be migrated within this milestone") is therefore met, and the closure record can state the compatibility position honestly: the builder path is kept, covered, and observable.

## §12 stop conditions — assessed, none fired

| Stop condition | Assessment |
|---|---|
| "Consuming the core rules would require the `eggwork-core` crate to acquire a dependency, or would create a dependency edge the workspace's one-way rule forbids" | **Did not fire.** The three new core functions use only `std` types already in the crate. `crates/eggwork-core/Cargo.toml` was not touched, and the ownership guard's dependency-direction check still passes. |
| "Removing the public constructor or setter would break a known in-tree caller that cannot be migrated within this milestone, **and** the closure record cannot state the public-API compatibility position honestly" | **Did not fire, on the second clause.** Removal *would* break five in-tree callers, two of them outside this milestone's ownership — but the compatibility position *is* stated honestly above: every caller is enumerated, the disposition is recorded item by item, and the breaking change is named. The milestone therefore proceeds under §5.5's decision rule, which selects option (b) when in-tree callers exist. |
| "The core rules are found to be *weaker* than the runner's copy on some rule, which would mean the authority itself is wrong" | **Did not fire.** Core is stronger than the runner's old copy on every rule it shares (environment name syntax, leading digit, duplicate names, resource bounds) and equal on the rest. Core is weaker in exactly one place — the empty argv entry, which the addendum names as the runner's permitted stricter rule ("the existing rejection of an empty argv entry, which protects the child-process construction") and which §5.2 and §5.3 sanction explicitly. That is a decided divergence, not a discovered one. |
| "Correcting the validation changes the observable behaviour of the server's execute path in a way that is not a pure rejection tightening" | **Did not fire.** The server builds requests only through `RunnerRequest::from_spec` (`crates/eggwork-server/src/lib.rs:1907`), and `ExecutionSpec::validate` already applied the environment and resource rules before this milestone. Every behavioural difference listed under "Rule dispositions that changed" is a pure rejection tightening, and none of them is reachable from the server's path. |
| "The milestone cannot be completed without weakening an authentication, lease, generation, fencing, or digest bound" | **Did not fire.** No bound of any kind was changed. `RunnerError::InvalidRequest` still refuses before setup and before any child exists; `CancelledBeforeSpawn` still takes precedence; lease, generation and fencing code is not in this milestone's scope and was not touched. |

## Rule dispositions that changed

Named explicitly, because the plan requires it. "Before" is the runner's private check at the reviewed head; "after" is the disposition in this milestone.

| Rule | Before | After | Direction |
|---|---|---|---|
| argv empty | refused (bundled with other argv failures) | refused by `eggwork_core::validate_argv` → `Empty { field: "argv" }` | same verdict, per-rule attribution |
| argv entry count > 256 | refused | refused by `validate_argv` → `TooMany { field: "argv" }` | same |
| argv entry longer than 32 KiB | refused | refused by `validate_argv` → `OutOfRange { field: "argv entry" }` | same |
| argv entry containing NUL | refused | refused by `validate_argv` | same |
| **argv entry that is empty** | refused (bundled) | refused by the runner's own `argv_entry_violation` → `InvalidSyntax { field: "argv entry" }` | same verdict, now attributable to the runner's own rule |
| **environment name containing a character outside ASCII alphanumerics and `_`** | **accepted** by the runner | **refused** by `eggwork_core::validate_environment_entries` → `InvalidSyntax { field: "environment name" }` | **runner was weaker; now aligned** |
| **environment name beginning with an ASCII digit** | **accepted** by the runner | **refused** by `validate_environment_entries` | **runner was weaker; now aligned** |
| **duplicate environment names** | **accepted** by the runner | **refused** by `validate_environment_entries` → `InvalidSyntax { field: "duplicate environment name" }` | **runner was weaker; now aligned** |
| environment name empty | refused | refused by `validate_environment_entries` | same verdict, per-rule attribution |
| environment name longer than 256 bytes | refused | refused by `validate_environment_entries` | same |
| environment name containing NUL | refused | refused by `validate_environment_entries` | same |
| environment name containing `=` | refused | refused by `validate_environment_entries` (the charset rule covers it) | same |
| environment value longer than 16 KiB | refused | refused by `validate_environment_entries` → `TooLong { field: "environment value" }` | same verdict, per-rule attribution |
| environment value containing NUL | refused | refused by `validate_environment_entries` → `Forbidden { field: "environment value NUL" }` | same verdict, per-rule attribution |
| environment entry count > 256 | refused | refused by `validate_environment_entries` → `TooMany { field: "environment" }` | same verdict, per-rule attribution |
| **resource requirement out of core's range** (e.g. memory `Required(u64::MAX)`) | **not checked by the runner at all** | **refused** by `eggwork_core::ResourceRequirements::validate` → `OutOfRange { field: "resource requirements" }` | **new rule at the runner; a pure rejection tightening.** The server is unaffected: it builds requests only through `from_spec`, and `ExecutionSpec::validate` already applied this rule. The rule existed in core and was reachable through the runner's mutable resource door; the runner now consumes it. |
| capture limit above 16 MiB | refused | refused by `eggwork_core::OutputPolicy::validate` → `OutOfRange { field: "output policy" }` | same verdict, per-rule attribution |
| event chunk of 0 | refused | refused by `OutputPolicy::validate` | same |
| event chunk above 64 KiB | refused | refused by `OutputPolicy::validate` | same |
| timeout of zero | refused | refused by `eggwork_core::validate_timeout` → `OutOfRange { field: "timeout" }` | same verdict, per-rule attribution |
| timeout above 7 days | refused | refused by `validate_timeout` | same |
| stdin above 4 MiB | refused by the runner's local copy of the bound | refused by `eggwork_core::StdinPolicy::validate` → `TooLong { field: "stdin", max: MAX_STDIN_BYTES }` | same verdict, bound now owned by core |
| root canonicalisation / "root is a directory" | runner-owned | runner-owned, unchanged | unchanged |
| cwd canonicalisation / "cwd escapes root" | runner-owned | runner-owned, unchanged | unchanged |

Net effect on a request that was previously accepted: three classes of request that core refused and the runner accepted are now refused by the runner (environment names with a non-alphanumeric character, names beginning with a digit, and duplicate names), plus one class the runner never checked (out-of-range resource requirements). All four are pure rejection tightenings and every one of them is a request core already considered invalid.

## Verification performed

All commands were run with `CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w1f`, on Linux (`6.8.0-147-generic x64`), with the pinned toolchain from `rust-toolchain.toml` (1.89.0). Counts are the real output.

| Command | Result |
|---|---|
| `cargo fmt -p eggwork-runner -- --check` | passed (after `cargo fmt -p eggwork-runner`) |
| `cargo fmt -p eggwork-core -- --check` | passed (after `cargo fmt -p eggwork-core`) |
| `cargo clippy --locked -p eggwork-runner -p eggwork-core --all-targets --all-features -- -D warnings` | passed, no warnings |
| `cargo test --locked -p eggwork-runner --all-targets` | passed — unit: 28 passed, 0 failed, 1 ignored (`characterize_output_drain_overhead`, the pre-existing manual characterization); `capability_probe`: 2 passed; `diagnostic_degradation`: 7 passed; `environment_policy`: 5 passed; `request_validation_parity` (new): 10 passed; `windows_process_tree`: 0 tests (`#![cfg(windows)]`) |
| `cargo test --locked -p eggwork-core --all-targets` | passed — unit: 35 passed; `environment_rule_partition` (new): 3 passed; `shared_protocol_contract`: 7 passed |
| `cargo test --locked -p eggwork-sandbox-helper --all-targets` | passed — `environment_policy_parity`: 12 passed; `landlock_runner`: 18 passed. Run because that crate's tests are in-tree callers of the changed runner API. |
| `cargo check --locked --workspace --all-targets` | passed — the whole workspace, including the server, compiles against the changed runner API |
| `cargo check --locked -p eggwork-runner -p eggwork-core` | passed |
| `python3 scripts/check_execution_ownership.py` | passed — "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "negative scanner self-test passed; crate dependency direction passed" |

Not run here, and therefore not claimed: the full-workspace `cargo test`, the full-workspace `cargo clippy`, `cargo fmt --all -- --check` (the coordinator's gate owns these; `cargo fmt --all` was deliberately not run because it would reformat the concurrent worker's in-flight files), and any non-Linux target. **No non-Linux platform is qualified by this record.** The Windows-only suite (`crates/eggwork-runner/tests/windows_process_tree.rs`) compiles only under `cfg(windows)` and was not executed here; its two in-tree uses of the changed API (`RunnerRequest::new` at line 143 and `eggwork_runner::StdinPolicy` at line 38) are covered by the re-export, but that is code inspection, not executed evidence.

### Pre-existing condition, not caused by this work

`crates/eggwork-runner/src/process_tree.rs` has a known macOS clippy failure (`method 'start_kill' is never used`, with `#[cfg(target_os = "linux")]`-gated call sites). It was **not observed** in this run, because this run was on Linux and the lint is macOS-specific. It is reported here as an inspection-level statement about a platform that was not exercised, not as an observed failure.

## Negative and adversarial evidence

The new suites are non-vacuous. Both were checked by deliberately breaking the code and observing the failure:

1. **The runner's weaker environment rules restored.** The runner's `contract_violation` was temporarily replaced with the pre-corrective environment check (empty name, over-long name/value, NUL, `=` in the name — but no charset rule, no leading-digit rule and no duplicate detection). `cargo test --locked -p eggwork-runner --test request_validation_parity` then failed **3 of 10**:
   - `the_runner_rejects_every_environment_name_form_core_rejects` — "a dash: the builder path must refuse \"WITH-DASH\" by the name rule; got None";
   - `a_duplicate_environment_name_is_refused_by_the_duplicate_rule` — "a repeated name must be refused by the duplicate rule; got None";
   - `every_public_construction_path_reaches_the_same_contract_verdict` — "an environment name with a dash: the two construction paths must report the same typed error; left: None, right: Some(InvalidSyntax { field: \"environment name\" })".

   The runner was then restored and the suite returned to 10/10.

2. **Core's environment-name rule widened.** `eggwork_core`'s name rule was temporarily widened to accept `-`. `cargo test --locked -p eggwork-core --test environment_rule_partition` then failed: "the implementation and the documented rule disagree about \"A-B\": implemented accepted = true, documented accepted = false". Core was then restored and the suite returned to 3/3.

3. **Refusal happens before the spawn.** Every refusal assertion in the runner suite runs a real request whose target would create a marker file, and asserts the marker does not exist afterwards. This is the adversarial form of "the request was refused", not merely "the request returned an error".

4. **The stricter direction is checked, not just the stricter-rule behaviour.** `the_runner_refuses_an_empty_argv_entry_which_core_permits` asserts that `from_spec` *accepts* the same spec, so the test cannot pass by core having become stricter.

## Documentation status

- `architecture/runner-execution.md` — updated. The request-validation narrative now states that the rules have one owner in core and that the runner consumes them; the `RunnerRequest` field table now names the rule owner and the checker per field; the "Two `StdinPolicy` types" section became "One `StdinPolicy`"; the `RunnerError` table now describes the typed payload; the "What a caller cannot do" section states the coverage property; the runner-level empty-argv rule is documented with its reason; and the test-coverage section describes the two new suites. **All 42 stale line-number citations in this file were replaced with symbol references**, as the plan required — `grep -nE "lines? [0-9]+" architecture/runner-execution.md` now returns nothing.
- `architecture/core-domain.md` — updated. The `CommandSpec::validate` section now records that the argv, environment-entry and timeout rules are three public core functions rather than inline code, and that core is the single owner of the environment-entry rules; the rule-bearing citations were replaced with symbols. The remaining `lib.rs:NNN` citations in this file were **already stale before this milestone** (spot-checked against the reviewed head: `lib.rs:229-268`, cited for `EnvironmentEntry`, actually pointed inside `DiagnosticError`; `lib.rs:1264`, cited for a test, pointed at the `Spawn` enum variant), and the plan scopes the citation cleanup to the runner deep dive, so they were left alone. That is a recorded residual, not an oversight.
- `architecture/overview.md` — **not changed by this worker** (coordinator-owned). No residual divergence is known to remain: the runner is now at least as strict as core on every shared rule and the one stricter rule is explicit. If the coordinator prefers a row in the divergences table, the proposed text is in the handoff report.
- `AGENTS.md` — **not changed**: the line "Construct validated state behind private fields (`RunnerRequest::from_spec`)" remains accurate. `from_spec` is still the protocol path and the construction path is still private-fields-based.
- `plans/subsystems/foundation-execution-core-roadmap.md` — updated: the status line and the M005 entry now read `active`, the M005 entry carries the implementation record and the public-API disposition, and the entry points at this closure record.
- `plans/implementation/foundation-execution-core/005-runner-request-validation-parity-corrective.md` — `Status:` changed from `proposed` to `active`. The coordinator sets the final status after the gate.
- `docs/` — no change. The loader strictness and the operator verbs are unaffected; the runner's request contract is an embedding-surface concern, not an operator-facing one.

## Unresolved findings, by severity

**No high or medium findings.**

- **Low — the `RunnerError::InvalidRequest` payload type change is breaking for unknown out-of-tree embedders.** The out-of-tree position is unknown (§ "Out-of-tree embedder position"), so the change is recorded as breaking and must be reflected in the release notes and the version. Mitigation: no in-tree caller; the error family, the variant name and the `Display` prefix are unchanged; the new payload is strictly more informative.
- **Low — the builder path is checked at the enforcement point rather than at the setter.** The plan's option (b) said the public setters would "validate with a typed error" *at the call*. Every reading of (b) that changes a signature is blocked by the same boundary as (a): `RunnerRequest::new` and `RunnerRequest::set_environment` have five in-tree callers across two crates, two of them in `crates/eggwork-sandbox-helper/tests/` which this milestone does not own; changing either to return `Result<_, ValidationError>` would not compile those callers, and would additionally introduce `unused_must_use` warnings that fail `clippy -D warnings` in files this milestone cannot edit. The typed error is therefore returned by the enforcement point (`RunnerRequest::validate`, before setup and before any child exists) and is observable without running via the new `RunnerRequest::contract_error()`. The property the milestone protects — no request built through any public path is *executed* in violation of the core contract — holds, and is proved by test. What does not hold is the stronger literal reading of §4.4: a request *object* built through the builder can transiently hold a core-invalid value between the setter call and the run. If the coordinator prefers the stricter reading, the follow-up is a separate API-breaking milestone that migrates the five callers to `from_spec` and returns `Result` from the setters.
- **Low — `architecture/core-domain.md` still carries stale `lib.rs:NNN` citations** outside the rule-bearing text this milestone touched. They pre-date this work; the plan scopes the citation cleanup to the runner deep dive.
- **Low — the root/cwd rules remain untested at the runner level.** Unchanged by this milestone and already recorded as a thin area in `architecture/runner-execution.md`. The "cwd escapes root" case is covered only in the helper crate's test, and only in its symlink form.
- **Informational — the runner now refuses out-of-range resource requirements** that it previously accepted. Pure rejection tightening; the server is unaffected because it only uses `from_spec`.

## Registry and roadmap disposition

- Roadmap `plans/subsystems/foundation-execution-core-roadmap.md`: M005 status set to `active`; the roadmap status line updated to read "M005 request-validation parity corrective active"; the M005 entry carries the public-API disposition and points at this record. The coordinator sets the final status after the gate.
- Registry `plans/registry.md`: **coordinator-owned; not edited by this worker.** The proposed entry text is in the handoff report.
- No other roadmap or closure file was edited.

## Scope note

`crates/eggwork-sandbox-helper` was **not** modified. Its production crate has no workspace-crate dependency, as the plan states, and it does not construct runner requests. Its *integration tests* do, and they were the reason option (a) was not taken; they were left untouched and their suite was run to confirm they still pass against the changed runner API.
