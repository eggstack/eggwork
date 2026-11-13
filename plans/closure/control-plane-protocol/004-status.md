# Control Plane M004 Closure — Shared Protocol Contract Corrective

Status: closed

Source plan: `plans/implementation/control-plane-protocol/004-shared-protocol-contract-corrective.md`

Corrective addendum: `plans/subsystems/control-plane-protocol-shared-contract-corrective-addendum.md`

Decision record: `plans/adrs/ADR-0008-core-protocol-contract-ownership.md`

Roadmap: `plans/subsystems/control-plane-protocol-roadmap.md`

## Reviewed baseline and implementation

- Reviewed parent head: `63699df` (branch `corrective/interrogation-batch-2026-10`).
- Implementation commit: `1da739b`.
- Crates changed: `eggwork-core`, `eggwork-client`, `eggwork-runner`, `eggwork-server`. The
  sandbox helper's **production** code is unchanged; only its test file changed.
- `eggwork-core` (`crates/eggwork-core/src/lib.rs`) — new public surface:
  `API_SCHEMA_VERSION`, `EXEC_ARGV_CAPABILITY`, `ALL_EXECUTION_STATES`, and on `ExecutionState`
  the methods `is_terminal`, `is_non_terminal`, `wire_name`. The enum's derives are untouched.
- `eggwork-client` (`crates/eggwork-client/src/lib.rs`) — the private
  `const API_SCHEMA_VERSION` was deleted and the core constant imported; the literal
  `"exec.argv.v1"` in `ensure_protocol_compatible` was replaced by
  `eggwork_core::EXEC_ARGV_CAPABILITY`; the protocol decision was split into a pure
  `protocol_compatibility` helper so it can be tested without a live node.
- `eggwork-runner` (`crates/eggwork-runner/src/lib.rs`) — new public surface:
  `CGROUPS_V2_MEMORY_CAPABILITY`, `CGROUPS_V2_CPU_CAPABILITY`, `CGROUPS_V2_PIDS_CAPABILITY`,
  `WORKSPACE_RW_PROFILE`. The advertisement site (`TrustedLandlockSetup::execution_capabilities`)
  now names constants; the probe table was extracted to the module-level
  `CGROUP_PROBE_TABLE` so a test can read the names it would advertise.
- `eggwork-server` (`crates/eggwork-server/src/lib.rs`, `store.rs`, `operations.rs`) — the private
  `const API_SCHEMA_VERSION` was deleted and the core constant imported; `static_capability_features`
  names `eggwork_core::EXEC_ARGV_CAPABILITY`; `check_resources_supported` names the runner's
  constants; the private `is_terminal` was removed in favour of `ExecutionState::is_terminal`;
  the artifact-capture subset became `artifact_capture_eligible`; the metric map became the
  exhaustive `terminal_metric_name`; `store::state_name` and `store::is_terminal_name` were removed
  in favour of `ExecutionState::wire_name`, `store::stored_state_is_terminal`, and
  `store::non_terminal_state_names`.

**Inspection only (no executed evidence):** the facts above were established by reading the
source at the reviewed head. Every behavioural claim in this record is backed by a test that was
actually run; see the matrix and the command log below.

## Requirement-to-evidence matrix

§11's twenty acceptance criteria, one row each. Every row names an implementation site and a test.

| # | Criterion | Implementation site | Test |
|---|---|---|---|
| 1 | `API_SCHEMA_VERSION` declared exactly once, publicly, in `eggwork-core` | `crates/eggwork-core/src/lib.rs` (`pub const API_SCHEMA_VERSION`); the private constants in `crates/eggwork-client/src/lib.rs` and `crates/eggwork-server/src/lib.rs` were deleted | `crates/eggwork-server/tests/shared_contract_ownership.rs::the_schema_version_is_declared_exactly_once_in_the_workspace` |
| 2 | Client bodies and server accept/reject paths both resolve to the core constant; a private copy would fail a test | `crates/eggwork-client/src/lib.rs` (`API_SCHEMA_VERSION` import); `crates/eggwork-server/src/lib.rs` (`execute`, `workspace_create`, `workspace_derive` compare against the imported constant) | `shared_contract_ownership.rs::the_schema_version_is_declared_exactly_once_in_the_workspace`; `crates/eggwork-client/src/lib.rs::shared_contract_tests::every_authored_request_body_carries_the_shared_schema_version`; `crates/eggwork-server/src/lib.rs::tests::the_client_and_the_node_agree_on_the_control_plane_schema_version` |
| 3 | A test fails if the client-sent and server-accepted versions can diverge, exercising the real client-built body | `crates/eggwork-server/src/lib.rs::tests::the_client_and_the_node_agree_on_the_control_plane_schema_version` (real `NodeClient::execute` against a live node) | same |
| 4 | The runner exports the three cgroup constants and the advertisement site uses them; no feature literal remains at the site | `crates/eggwork-runner/src/lib.rs` (`CGROUPS_V2_*_CAPABILITY`, `CGROUP_PROBE_TABLE`, `TrustedLandlockSetup::execution_capabilities`) | `crates/eggwork-runner/src/lib.rs::tests::the_probe_table_advertises_exactly_the_exported_constants`; `crates/eggwork-server/tests/shared_contract_ownership.rs::the_runner_declares_each_capability_literal_in_the_constant_that_owns_it` |
| 5 | The server imports the runner's constants; the cgroup names are not re-typed there | `crates/eggwork-server/src/lib.rs::check_resources_supported` | `crates/eggwork-server/src/lib.rs::tests::the_resource_admission_rule_reads_the_runner_constants`; `shared_contract_ownership.rs::the_cgroup_feature_names_are_not_retyped_in_the_server` |
| 6 | `exec.argv.v1` is core-owned and imported by both endpoint crates; the literal appears in neither production file | `crates/eggwork-core/src/lib.rs` (`EXEC_ARGV_CAPABILITY`); `crates/eggwork-server/src/lib.rs::static_capability_features`; `crates/eggwork-client/src/lib.rs::protocol_compatibility` | `shared_contract_ownership.rs::the_argv_capability_name_is_not_retyped_in_either_endpoint_crate`; `crates/eggwork-server/src/lib.rs::tests::the_advertised_argv_feature_is_the_one_the_owner_declares`; `crates/eggwork-client/src/lib.rs::shared_contract_tests::a_node_advertising_a_differing_argv_feature_is_refused` |
| 7 | `workspace_rw` is a `pub` constant used at all three runner sites; the helper's production code is unmodified and its literal is pinned | `crates/eggwork-runner/src/lib.rs` (`WORKSPACE_RW_PROFILE`; used in `TrustedLandlockSetup::prepare`, `RunnerRequest::from_spec`, `SandboxChannel::new`); `crates/eggwork-sandbox-helper/src/main.rs` unchanged | `crates/eggwork-sandbox-helper/tests/landlock_runner.rs::the_runner_profile_is_granted_and_a_differing_one_is_refused_before_the_target_spawns`; `crates/eggwork-runner/src/lib.rs::tests::the_workspace_rw_profile_name_is_frozen` |
| 8 | Every capability feature-name value is byte-for-byte identical to its pre-M004 value | `crates/eggwork-core/src/lib.rs`; `crates/eggwork-runner/src/lib.rs` | `crates/eggwork-core/tests/shared_protocol_contract.rs::the_exec_argv_capability_name_is_frozen`; `crates/eggwork-runner/src/lib.rs::tests::capability_feature_string_is_frozen_for_workspace_rw_profile`, `::cgroup_capability_feature_strings_are_frozen`, `::the_workspace_rw_profile_name_is_frozen` |
| 9 | `eggwork-core` owns the classification; both the predicate and the wire spelling are exhaustive matches with no wildcard arm | `crates/eggwork-core/src/lib.rs` (`ExecutionState::is_terminal`, `::is_non_terminal`, `::wire_name`) | `crates/eggwork-core/tests/shared_protocol_contract.rs::every_state_is_classified_into_exactly_one_set`, `::the_terminal_set_has_five_members_and_the_non_terminal_set_four`; the exhaustive-match property is additionally shown by the adversarial tenth-variant build break below |
| 10 | No terminal-state enumeration remains in the server's `store.rs`, `lib.rs`, or `operations.rs` | `crates/eggwork-server/src/store.rs` (`stored_state_is_terminal`, `non_terminal_state_names`; the recovery `UPDATE` binds the state); `crates/eggwork-server/src/lib.rs` (`terminal_metric_name`, `artifact_capture_eligible`); `crates/eggwork-server/src/operations.rs` (`execution_page` uses `is_non_terminal`; `count_active_executions` binds the derived list) | `shared_contract_ownership.rs::the_server_names_no_execution_state_as_a_string_literal` |
| 11 | A property test proves the classification is total and disjoint, and adding a variant breaks the build or the test | `crates/eggwork-core/src/lib.rs` (`ALL_EXECUTION_STATES`) | `crates/eggwork-core/tests/shared_protocol_contract.rs::every_state_is_classified_into_exactly_one_set`, `::the_terminal_set_has_five_members_and_the_non_terminal_set_four`, `::the_execution_state_enum_has_exactly_nine_variants` — plus the adversarial tenth-variant build break recorded below |
| 12 | The `state` column stores the same values as before, and the active-execution SQL selects exactly `Accepted`, `Preparing`, `Running`, `Cancelling` | `crates/eggwork-server/src/store.rs` (`ExecutionState::wire_name` at the column write); `crates/eggwork-server/src/operations.rs::count_active_executions` | `crates/eggwork-server/src/store.rs::tests::the_state_column_stores_the_pascal_case_wire_spelling`; `crates/eggwork-server/src/operations.rs::tests::the_active_execution_count_selects_exactly_the_non_terminal_states` |
| 13 | The capture-eligibility predicate accepts exactly `Succeeded` and `Failed` and stays distinct from terminality | `crates/eggwork-server/src/lib.rs::artifact_capture_eligible` | `crates/eggwork-server/src/lib.rs::tests::artifact_capture_eligibility_is_exactly_succeeded_and_failed` |
| 14 | The metric mapping is total over the terminal set and the operator-facing labels are byte-identical | `crates/eggwork-server/src/lib.rs::terminal_metric_name`; `crates/eggwork-server/src/operations.rs::state_label` | `crates/eggwork-server/src/lib.rs::tests::the_terminal_metric_mapping_is_total_over_the_terminal_set`; `crates/eggwork-server/src/operations.rs::tests::the_operator_state_labels_are_unchanged` |
| 15 | `ExecutionState`'s wire representation is unchanged — derived `Serialize`/`Deserialize`, no rename attribute | `crates/eggwork-core/src/lib.rs` (derives untouched) | `crates/eggwork-core/tests/shared_protocol_contract.rs::the_state_wire_form_is_still_a_bare_pascal_case_string`, `::the_wire_spelling_matches_the_derived_serde_representation`; `shared_contract_ownership.rs::the_core_state_enum_keeps_its_derived_wire_representation` |
| 16 | No new crate dependency edge; `eggwork-core` still has no internal dependency; the helper still has no workspace-crate dependency | no `Cargo.toml` was touched | `python3 scripts/check_execution_ownership.py` (crate dependency direction passed); `git diff --stat -- '*Cargo.toml'` is empty |
| 17 | The §10 commands pass and the focused per-crate runs are recorded | — | the command log below |
| 18 | The §9 documentation updates are applied and the touched documents cite by symbol name | `architecture/core-domain.md`, `architecture/overview.md` (divergence #12 only), `architecture/client-transport.md`, `architecture/server-node.md`, `architecture/storage-journal.md`, `architecture/runner-execution.md` | review of the diff; `git diff --check` is clean |
| 19 | This closure record exists and satisfies §13 | `plans/closure/control-plane-protocol/004-status.md` | this document |
| 20 | No high or medium finding remains without an explicit owner | — | the findings section below |

## Tests and commands actually run

### Pre-submit gate

| Command | Outcome |
|---|---|
| `python3 scripts/check_execution_ownership.py` | **passed** — "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "crate dependency direction passed" |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | **passed** — "negative scanner self-test passed"; the synthetic forbidden spawn was detected |
| `cargo fmt --all -- --check` | **passed** (run after `cargo fmt --all`; only files inside this milestone's ownership differed, and the re-check was clean) |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | **passed** after one fix — the first run failed with `clippy::redundant_static_lifetimes` on `CGROUP_PROBE_TABLE`'s `&'static str` element type; the lifetime was removed and the re-run passed. No rerun was reported as a fix without the underlying change. |
| `cargo check --locked --workspace --all-targets` | **passed** |
| `cargo test --locked --workspace --all-targets` | **not run by this milestone** — the coordinator runs the whole-workspace gate once at the end of the batch. The per-crate runs below cover every crate this milestone touched. |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | **not run by this milestone** — the coordinator runs the release-harness suite once at the end of the batch. This milestone changes no release script, artifact form, or release target. |
| `git diff --check` | **passed** (no whitespace errors) |

### Focused per-crate runs

| Command | Outcome |
|---|---|
| `cargo test --locked -p eggwork-core --all-targets` | **passed** — 32 lib tests + 7 in `tests/shared_protocol_contract.rs` |
| `cargo test --locked -p eggwork-client --all-targets` | **passed** — 10 tests |
| `cargo test --locked -p eggwork-client --features eggress-route --all-targets` | **passed** — 13 tests (the server's dev-dependency on the client enables this feature, so the cross-crate agreement test compiles and runs under it) |
| `cargo test --locked -p eggwork-runner --all-targets` | **passed** — 28 lib + 2 `capability_probe` + 7 `diagnostic_degradation` + 5 `environment_policy` + 0 `windows_process_tree`. **Note:** in this invocation `capability_probe::trusted_helper_advertises_landlock_workspace_rw_capability` printed "eggwork-sandbox-helper is not built; skipping" and returned early — the probe locates the helper binary through `CARGO_TARGET_DIR`/`OUT_DIR`, and a `-p eggwork-runner` test run builds neither. The same test was re-run with `CARGO_TARGET_DIR` pointed at the workspace target directory, where it **did** execute and passed (see the platform matrix). |
| `cargo test --locked -p eggwork-server --all-targets` | **passed** — 150 lib + 0 `eggworkd` + 3 `gc_preview_parity` + 12 `host_telemetry` + 0 `installed_qualification` (ignored, opt-in) + 36 `release_contract` + 6 `shared_contract_ownership` |
| `cargo test --locked -p eggwork-sandbox-helper --all-targets` | **passed** — 12 `environment_policy_parity` + 18 `landlock_runner` |
| `CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target cargo test --locked -p eggwork-runner --test capability_probe` | **passed** — 2 tests, and this time the Landlock advertisement assertion **executed** rather than skipping (the probe locates the helper binary through `CARGO_TARGET_DIR`/`OUT_DIR`, which the plain `-p eggwork-runner` invocation does not provide). Run to establish that the runner's advertisement path really advertises `LANDLOCK_WORKSPACE_RW_CAPABILITY` on this host. |

### Failures observed and how they were resolved

Recorded honestly; a rerun is not a fix.

1. **`cargo clippy` — `clippy::redundant_static_lifetimes`** on
   `crates/eggwork-runner/src/lib.rs` (`CGROUP_PROBE_TABLE: [(&'static str, …)]`). *Fix:* the
   element type was changed to `&str`; the lint is gone because the constant is `'static` by
   default. Re-run passed.
2. **`the_state_column_stores_the_pascal_case_wire_spelling` failed** with
   `left: "TimedOut", right: "Interrupted"`. The test committed three states onto one row, and a
   stored terminal state refuses further events, so the third commit was rolled back. *Fix:* the
   test now uses one execution per state. No production code was changed for this.
3. **`the_client_and_the_node_agree_on_the_control_plane_schema_version` failed** with
   `ClientError::Api { status: 503, code: "busy" }`. The test's first raw POST had started an
   execution that occupied the node's single permit. *Fix:* the accepted body now carries an
   invalid specification on purpose, so it passes the version gate and is refused later without
   starting an execution. No production code was changed for this.

## Platform and feature matrix actually exercised

Host: **Linux x86_64**, kernel `6.8.0-147-generic`, Rust **1.89.0** as pinned by
`rust-toolchain.toml`.

| Path | Exercised? | Evidence / what was *not* covered |
|---|---|---|
| Linux Landlock (helper enforcement, `workspace_rw`) | **yes** | `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` ran 18 tests green, including `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` and the new `the_runner_profile_is_granted_and_a_differing_one_is_refused_before_the_target_spawns` — the latter drives the real helper binary with the runner's own profile and observes a confined target run. The runner's *advertisement* probe was additionally executed by re-running `cargo test --locked -p eggwork-runner --test capability_probe` with `CARGO_TARGET_DIR` pointed at the workspace target directory (in the plain `-p eggwork-runner` invocation it skips, because that crate's test run does not build the helper binary); with the helper located, `trusted_helper_advertises_landlock_workspace_rw_capability` ran and passed, i.e. this host advertises `LANDLOCK_WORKSPACE_RW_CAPABILITY`. |
| Linux cgroup v2 (memory / CPU / pids) | **yes** | `crates/eggwork-sandbox-helper/tests/landlock_runner.rs::required_memory_limit_is_enforced_and_classified` ran a **required** 16 MiB memory dimension and observed `ResourceLimit` / `ResourceDimensionResult::LimitExceeded`; `required_pid_limit_is_enforced_and_classified` and `required_cpu_quota_is_verified_before_target_start` did the same for pids and CPU. A *required* dimension errors out when the systemd transient-cgroup backend is unavailable, so the cgroup backend was available and enforcing on this host. `concurrent_resource_scopes_keep_pid_limits_isolated` also passed. |
| Windows Job Object process tree | **not exercised** | `crates/eggwork-runner/tests/windows_process_tree.rs` reported `0 passed` — the file is `#[cfg(windows)]`-gated. No Windows host was available. **Windows is not claimed.** |
| macOS fail-closed path | **not exercised** | No macOS host was available. The non-Linux stubs were not run. **macOS is not claimed.** |
| `eggwork-client/eggress-route` feature | **yes** | `cargo test --locked -p eggwork-client --features eggress-route --all-targets` — 13 tests green, including `route_failure_does_not_connect_direct_and_preserves_typed_error`. |
| Server mTLS control plane | **yes** | The cross-crate agreement test starts a real `NodeServer` with generated mTLS material and drives the real `NodeClient` over loopback TLS. |
| `installed_qualification` (hosted qualification) | **not exercised** | Ignored and opt-in; it needs `EGGWORK_QUALIFY_DAEMON`/`EGGWORK_QUALIFY_CONFIG`/`EGGWORK_QUALIFY_TLS_DIR` and an installed node. Not required by this milestone. |

Nothing above is claimed on the strength of compilation. The Windows and macOS paths are
**not qualified** by this run.

## Negative and adversarial evidence

All of the following were executed; the quoted output is what the run produced.

1. **A private schema-version copy in the client does not compile.** Reintroducing
   `const API_SCHEMA_VERSION: u16 = 1;` beside the imported name produced
   `error[E0255]: API_SCHEMA_VERSION redefined here`. The compiler prevents the copy outright.
2. **A private schema-version copy that *does* compile is caught by the ownership test.**
   With the import renamed to `CoreApiSchemaVersion` and a private
   `const API_SCHEMA_VERSION: u16 = 1;` added,
   `the_schema_version_is_declared_exactly_once_in_the_workspace` failed with
   `left: [("crates/eggwork-core/src/lib.rs", 1), ("crates/eggwork-client/src/lib.rs", 1)]`.
3. **A private copy with a divergent value breaks the cross-crate agreement test.** With the
   client's private constant set to `2`,
   `the_client_and_the_node_agree_on_the_control_plane_schema_version` failed with
   `ClientError::Api { status: 426, code: "protocol_version" }` — the real divergence the
   milestone exists to prevent.
4. **The argv capability literal retyped in the client is caught.**
   `the_argv_capability_name_is_not_retyped_in_either_endpoint_crate` failed with
   "crates/eggwork-client/src/lib.rs must import the owned capability name, not re-type it".
5. **A mis-edited classification is caught.** Moving `Failed` from the terminal arm to the
   non-terminal arm of `ExecutionState::is_terminal` failed two core tests:
   `every_state_is_classified_into_exactly_one_set` ("Failed is classified as both terminal and
   non-terminal, or as neither") and `the_terminal_set_has_five_members_and_the_non_terminal_set_four`
   (`left: ["Succeeded", "Cancelled", "TimedOut", "Interrupted"]`).
6. **Adding a tenth variant to `ExecutionState` breaks the build.** Adding `Quarantined` produced
   three `error[E0004]: non-exhaustive patterns: &ExecutionState::Quarantined not covered` errors
   in `eggwork-core`, one for each exhaustive match (`is_terminal`, `is_non_terminal`,
   `wire_name`). This is the primary mechanism behind criterion 11; the
   `the_execution_state_enum_has_exactly_nine_variants` assertion is the secondary one.
7. **A schema-version mismatch still yields 426 `protocol_version`.**
   `the_client_and_the_node_agree_on_the_control_plane_schema_version` posts a body with
   `API_SCHEMA_VERSION + 1` and asserts `426`.
8. **A resource capability name differing by one character still yields a refusal.**
   `the_resource_admission_rule_reads_the_runner_constants` rewrites each of the three runner
   constants with a trailing `x` and asserts that the required dimension is refused.
9. **A sandbox profile differing from the runner's still fails before the target spawns.**
   `the_runner_profile_is_granted_and_a_differing_one_is_refused_before_the_target_spawns`
   asserts status byte `12` for a profile one character off and that the target marker was never
   created.
10. **The artifact-capture predicate still rejects `Cancelled`, `TimedOut`, and `Interrupted`.**
    `artifact_capture_eligibility_is_exactly_succeeded_and_failed` asserts the eligible set is
    exactly `["Succeeded", "Failed"]`, that every other variant is ineligible, and that the
    eligible count is strictly smaller than the terminal count.
11. **The active-execution SQL still excludes terminal states.**
    `the_active_execution_count_selects_exactly_the_non_terminal_states` seeds one row per
    variant and asserts the count is `4`, and asserts the derived list is exactly
    `["Accepted", "Preparing", "Running", "Cancelling"]`.

All adversarial edits were reverted; the tree was re-verified with `cargo fmt --all -- --check`
and the affected suites after the reverts.

## Compatibility review

No wire value, wire field, feature-string byte, stored format, or operator-visible output changed.

| Fact | How it was proven |
|---|---|
| The `ExecutionState` wire form | The enum's derives are untouched; `the_state_wire_form_is_still_a_bare_pascal_case_string` asserts `TimedOut` encodes as `"TimedOut"` and decodes back, `the_wire_spelling_matches_the_derived_serde_representation` asserts `wire_name()` equals the serde output for every variant, and `the_core_state_enum_keeps_its_derived_wire_representation` asserts the declaration carries no `rename` or `serde(` attribute. |
| The `state` column's stored values | `the_state_column_stores_the_pascal_case_wire_spelling` reserves an execution per state and reads the raw column back, asserting `Running`, `TimedOut`, and `Interrupted` — the same bytes the column held before. |
| The journal's stored format | No `CREATE TABLE` statement and no column definition was touched. The only SQL change is the recovery `UPDATE`, which now binds the interrupted state as a parameter instead of writing the literal `'Interrupted'`; the bound value is `ExecutionState::Interrupted.wire_name()`, so the stored bytes are identical. `recovery_marks_uncertain_work_interrupted_and_never_replays_it` still passes unchanged and asserts the same outcome. |
| The capability feature names | `the_exec_argv_capability_name_is_frozen`, `capability_feature_string_is_frozen_for_workspace_rw_profile`, `cgroup_capability_feature_strings_are_frozen`, and `the_workspace_rw_profile_name_is_frozen` assert the literal byte values. The values were also compared against the pre-M004 source before editing; none moved. |
| The advertised feature list | No entry was added, removed, or reordered. `static_capability_features_are_a_single_canonical_list` and `server_capability_features_do_not_duplicate_static_dynamic_overlap` still pass. The freeze assertions deliberately pin the *values* and *membership*, not the whole advertised list, because Workspace M005a makes `artifact.declared.v1` conditional on the runtime capture fact. |
| The operator-facing state labels | `the_operator_state_labels_are_unchanged` pins all nine snake_case labels. |
| The active-execution query's semantics | `the_active_execution_count_selects_exactly_the_non_terminal_states` asserts the same four names and the same count. The list is assembled at runtime and every state name is bound as a SQL parameter; only the `?` placeholders are assembled into the statement text, never a value. |

## Security review

- **Authentication, authorization, lease, generation, fencing, digest, and idempotency semantics are
  untouched.** The diff in `crates/eggwork-server/src/lib.rs` is confined to the capability and
  classification call sites; no authorization, lease, generation, or digest code path was modified.
  The `reserve` generation-reservation path keeps its result — a stale generation is still refused
  with `ReserveResult::StaleGeneration`, and a generation may only advance from a terminal state —
  because the shared classification answers the same question the inline `matches!` did.
- **No new child-process creation path.** `eggwork-core`, `eggwork-client`, and `eggwork-server`
  still create no processes; `scripts/check_execution_ownership.py` passes and reports the same two
  approved owners and the same spawn-site counts as before.
- **No new dependency edge.** No `Cargo.toml` was modified. `eggwork-core` still has no internal
  dependency, `eggwork-sandbox-helper` still has no workspace-crate dependency in `[dependencies]`,
  and `eggwork-client` remains a dev-dependency of `eggwork-server` only.
- **No new injection surface.** The runtime-assembled SQL binds every state name as a parameter.
- **No new panic path.** The new code is const arrays, exhaustive matches, and helper functions with
  no `unwrap`/`expect` on untrusted input.

## Documentation status

All six §9 documents were updated, and no `docs/` file was touched (this milestone is invisible to
an operator and to a user).

| Document | What changed |
|---|---|
| `architecture/core-domain.md` | The **States** paragraph no longer says there is no `is_terminal()` helper; it now describes the core-owned classification, the wire spelling, and `ALL_EXECUTION_STATES`, and adds the core-owned schema-version and argv-capability constants. Its line-number citations were replaced with symbol references. |
| `architecture/overview.md` | Divergence #12 only. The count changed from "four places" to the classification being spelled out in one place — `eggwork-core`'s `ExecutionState::is_terminal`/`is_non_terminal` — with the server deriving from it, while the original substance (the store does **not** enforce the state machine; legality lives in `publish`) is preserved. No other row was touched. |
| `architecture/client-transport.md` | The schema-version sentence now names `eggwork_core::API_SCHEMA_VERSION` as the single number the client sends and the server compares against; the `ensure_protocol_compatible` description, the `UnsupportedCapability` error row, and the stale "no test for `ensure_protocol_compatible`" limitation were updated. |
| `architecture/server-node.md` | The capability-advertisement paragraph, the tri-state admission bullets, the capability-refusal paragraph, the invariants table, and the source anchors now name `eggwork_core::EXEC_ARGV_CAPABILITY` and the runner's `CGROUPS_V2_*_CAPABILITY` constants by symbol. |
| `architecture/storage-journal.md` | The schema-version paragraph, the state-machine-persistence section, the enforced-predicates list, the `Preparing` note, the terminal-single-assignment paragraph, the `commit_event` step list, the invariants row, and the known-limitation bullet now describe the core-owned spelling and classification and the derived derivations. |
| `architecture/runner-execution.md` | The profile mapping, the profile resolution, the capability-advertisement description, the helper-argv composition, and the `LANDLOCK_WORKSPACE_RW_CAPABILITY` explanation now name the exported constants and drop the stale line-number citations. |

Every symbol introduced or edited in these documents is cited by **symbol name**; no new line-number
citation was added.

## Decisions recorded

1. **`fn state_label` stays server-local**, in `crates/eggwork-server/src/operations.rs`. It is a
   *display* projection, not a protocol fact: the wire spelling is
   `eggwork_core::ExecutionState::wire_name` and the terminal classification is
   `ExecutionState::is_terminal`, while the label's snake_case is an operator-facing convention that
   the protocol-neutral core should not carry. The map stays total over the enum with no wildcard
   arm (it already was) and its output is unchanged — `the_operator_state_labels_are_unchanged`
   pins all nine labels. It is deliberately **not** derived from `wire_name()`, because a generic
   PascalCase→snake_case conversion would be a new mechanism replacing a pinned, readable map, and
   the plan's §4 explicitly excludes consolidating the server's enum-to-string maps.
2. **The artifact-capture eligibility predicate stayed server-local**, as
   `crates/eggwork-server/src/lib.rs::artifact_capture_eligible`. It is a *policy* subset
   (`Succeeded | Failed`), not a protocol classification, and it is deliberately **not** the terminal
   set: `Cancelled`, `TimedOut`, and `Interrupted` are terminal but not capture-eligible, and
   `artifact_capture_eligibility_is_exactly_succeeded_and_failed` asserts the subset is a strict
   subset of the terminal set.

## Unresolved findings by severity

No high or medium findings.

| Severity | Finding | Owner |
|---|---|---|
| Low | The `state_label` map is still a second, hand-maintained naming scheme (snake_case) beside the core-owned PascalCase `wire_name`. It is pinned by a test, but a state added to the enum requires a new label here as well as a new arm in the core matches. | Control-plane roadmap owner (future milestone), if the display projection is ever unified. |
| Low | The store resolves a stored `state` string back to its enum value by scanning `ALL_EXECUTION_STATES` rather than by a name→state lookup. This keeps the core's public surface to the names the plan fixed, at the cost of a nine-element scan on a non-hot path. | Control-plane roadmap owner, if the journal ever becomes hot enough for it to matter. |
| Low | The source-shape checks in `crates/eggwork-server/tests/shared_contract_ownership.rs` are textual. A copy introduced in a way the scan does not model (for example a `const` assembled at runtime) would not be caught by them, though it would be caught by the behavioural freeze assertions. | Control-plane roadmap owner. |

## Registry and roadmap disposition

- `plans/subsystems/control-plane-protocol-roadmap.md` — **updated by this milestone**: M004's
  entry now reads `Status: closed` and carries the decision-record and closure-record links; the
  roadmap's status line reads `Status: active roadmap; M001-M004 closed; no milestone in flight`.
- `plans/registry.md` — **not edited**: the registry is owned by the coordinator. Registration of
  this milestone is required and is listed in the handoff report.
- `AGENTS.md` — **not edited**: owned by the coordinator.

## Deviations

Anything that landed differently from the plan, with the reason.

1. **`check_isolation_supported` and `check_resources_supported` now take `&[String]` (the
   advertised feature list) instead of `&NodeState`.** The plan requires a test that exercises
   `check_resources_supported` directly, including the one-character-drift refusal. Constructing a
   `NodeState` in a test is impractical, and the pre-existing test in that area worked around this
   by keeping a *copy* of the rule in the test — exactly the duplication this milestone removes.
   The functions are private; the behaviour is unchanged.
2. **The client's protocol decision was split into a pure `protocol_compatibility` helper.**
   `ensure_protocol_compatible` now fetches the capabilities and delegates. The plan requires a
   test for the argv-feature refusal, and the client crate cannot host a stub node without a new
   dev-dependency (which §12.2 forbids). The helper is private; no public API changed.
3. **The store's recovery `UPDATE` binds the interrupted state as a parameter** instead of writing
   the SQL literal `'Interrupted'`. The plan did not require this, but it makes the "no state name
   as a literal in the server" check in criterion 10 crisp, and the stored bytes are unchanged.
4. **`ExecutionStore::recover`'s SQL `IN` list was derived** as well as
   `count_active_executions`'s. §5.3 item 19 does not name `recover()`'s list, but §7 says recovery
   "uses the shared classification" after the change, and leaving one hard-coded copy of the
   non-terminal set in `store.rs` while the other became derived would have preserved exactly the
   drift the milestone removes. The four names and the query semantics are unchanged, and
   `recovery_marks_uncertain_work_interrupted_and_never_replays_it` passes unchanged.
5. **The runner's probe table was extracted to a module-level `const CGROUP_PROBE_TABLE`.** §6.2
   asks for a test that the "probe-table feature names are exactly the exported constants"; reading
   a local table from a test is not possible, so the table became a named constant.
6. **The client's request-body serialization test lives in the client's in-crate test module**
   (`crates/eggwork-client/src/lib.rs::shared_contract_tests`), not in a new
   `crates/eggwork-client/tests/` file. The four request structs are private to the client crate,
   so an integration test could not reach the bodies the transport actually serializes. §6.1
   explicitly allowed either location and required the choice to be recorded — this is that record.
7. **The `ExecutionSpec` and `ExecutionSnapshot` schema-version fixtures were changed to the shared
   constant.** §5.1 item 4 requires "any remaining literal schema-version `1` used to stand for this
   fact". The server compares `wire.spec.schema_version` against `API_SCHEMA_VERSION`, so the spec's
   field is the same fact. Fixtures for *other* versioned documents (the workspace manifest, the
   manifest patch, the telemetry payload, the doctor report, the operator config, and the helper's
   launch spec) were deliberately left alone: those are separate versioned documents with their own
   validation.
8. **Two `architecture/` documents received edits beyond the §9 items**, all of them inside the
   documents §9 already lists: `architecture/server-node.md`'s invariants table and source-anchor
   list, and `architecture/client-transport.md`'s stale limitation bullet and error table row. They
   were stale statements about the code this milestone changed.
