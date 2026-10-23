# Control Plane M004 — Shared Protocol Contract Ownership

Status: proposed

Class: invariant

Source roadmap:

- `plans/subsystems/control-plane-protocol-roadmap.md`

Corrective authority:

- `plans/subsystems/control-plane-protocol-shared-contract-corrective-addendum.md`

Canonical/ADR references:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `plans/003-planning-process.md`

Historical predecessor closures (immutable; this plan does not rewrite them):

- `plans/closure/control-plane-protocol/001-status.md`
- `plans/closure/control-plane-protocol/002-status.md`
- `plans/closure/control-plane-protocol/002-lease-expiry-race-correction.md`
- `plans/closure/control-plane-protocol/003-status.md`

Closure record: `plans/closure/control-plane-protocol/004-status.md`

## 1. Objective

Make the client, the server, and the runner agree on three protocol contract facts **by construction rather than by convention**, so that a future change to any of them is a compile-time or test-detectable event.

The three facts:

1. **CP-1** — the control-plane API schema version.
2. **CP-2** — the capability feature names and the sandbox profile name.
3. **CP-3** — the terminal/non-terminal classification of `ExecutionState`.

For each, exactly one crate owns the fact; every producer and every consumer refers to it through that owner.

This is a refactor of contract literals into owned constants. It changes no wire value, no wire field, no runtime behavior, and no authorization, lease, generation, fencing, or digest semantics. It removes no capability and adds none.

## 2. Baseline and problem statement

At the reviewed baseline:

**CP-1.** `crates/eggwork-client/src/lib.rs` declares a private `const API_SCHEMA_VERSION: u16 = 1;`; `crates/eggwork-server/src/lib.rs` declares its own private `const API_SCHEMA_VERSION: u16 = 1;`. The client stamps its value into the execute, control, workspace-create, and workspace-derive request bodies. The server compares the received value against its own and rejects a mismatch with HTTP 426 `protocol_version` in the workspace-create, workspace-derive, execute, execute-spec, and cancel/renew paths. Nothing links the two declarations. `crates/eggwork-core/src/lib.rs` already declares `pub const NODE_TELEMETRY_SCHEMA_VERSION: u16 = 1;`, consumed by the client's telemetry reader, the server's telemetry projection, and core's own `NodeTelemetry::validate` — the precedent for the shape CP-1 needs.

**CP-2.** `crates/eggwork-runner/src/lib.rs`, in `TrustedLandlockSetup::execution_capabilities`, pushes the literal `"isolation.landlock.workspace-rw.v1"` — the same file already declares `pub const LANDLOCK_WORKSPACE_RW_CAPABILITY` with that exact value and does not use it there — and carries `"resources.cgroups-v2.memory"`, `"resources.cgroups-v2.cpu"`, and `"resources.cgroups-v2.pids"` as literals in its probe table. `crates/eggwork-server/src/lib.rs`, in `check_resources_supported`, matches the three cgroup names as literals; its `check_isolation_supported` already imports `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY`. The server produces `"exec.argv.v1"` in `static_capability_features`; `crates/eggwork-client/src/lib.rs` hardcodes `"exec.argv.v1"` in `NodeClient::ensure_protocol_compatible`. The `"workspace_rw"` profile name is a literal at three sites in the runner and one production site in `crates/eggwork-sandbox-helper/src/main.rs`.

**CP-3.** `crates/eggwork-core/src/lib.rs` defines `ExecutionState` as a flat nine-variant enum and owns no terminal concept: no `is_terminal` helper, no terminal grouping, no terminal list. `architecture/core-domain.md` records this under **States**. The server enumerates the classification nine times: `fn state_name` and `fn is_terminal_name` in `crates/eggwork-server/src/store.rs`, an inline `matches!` in `ExecutionStore::reserve` that does not call the helper in its own file, `fn is_terminal` in `crates/eggwork-server/src/lib.rs`, an inline `Succeeded | Failed` artifact-capture subset, a wildcard-terminated terminal-to-metric-name match, a non-terminal `matches!` in `crates/eggwork-server/src/operations.rs`, the parallel SQL literal list in `fn count_active_executions`, and the second enum-to-string map `fn state_label`.

**Why this is worth a milestone.** None of these is a live bug: both sides of every pair currently agree, by value. The defect is structural. A future rename or bump on the producing side desynchronizes the consuming side silently, and neither the compiler nor the test suite objects. The corrective converts three classes of silent-drift risk into compile-time or test-detectable events.

**Two review-note corrections the implementer must know.** Two details in the interrogation notes that produced this plan did not survive re-reading against the code, and the plan follows the code:

- The server's `check_isolation_supported` **already** imports `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY`. The Landlock capability's consumer side is not duplicated; only the runner's own push site is. Do not "fix" a duplication that does not exist.
- The `"workspace_rw"` profile name appears **once** in the helper's production code (`crates/eggwork-sandbox-helper/src/main.rs`), not "several places". The runner has seven occurrences including doc comments; the helper's remaining six are in its integration test.

## 3. Scope

**In scope.**

- CP-1: move the API schema version into `eggwork-core`; import it in the client and the server.
- CP-2: export the runner's capability feature names and sandbox profile name as public constants; use them at their producing sites; import them in the server; give `exec.argv.v1` a single owner reachable by both endpoints; pin the helper's `workspace_rw` literal with a test.
- CP-3: give the terminal/non-terminal classification one owner in `eggwork-core`; derive the server's existing helpers, its inline matches, its SQL literal list, and its metric mapping from it.

**Explicitly out of scope.**

- Changing any protocol wire field, field name, or value.
- Changing any capability feature-string **value** or any capability semantics. Those are frozen by `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` §3 and are preserved byte-for-byte. Changing a feature value is a compatibility break requiring an ADR, not this plan.
- Changing authorization, authentication, lease, generation, fencing, digest, or idempotency behavior.
- Any change to the runner's or the server's runtime behavior beyond replacing a literal with the constant it already equals.
- Any new crate dependency edge.
- Adding, removing, or renaming a capability feature.
- Changing the artifact-capture eligibility rule (still exactly `Succeeded` and `Failed`).
- Changing the operator-facing state labels in `eggworkd status`.
- Changing the `state` column values or the SQL that reads them, beyond deriving the literal list from the shared spelling.
- The architecture-record drift noted in §2 (divergence #20's stale text) — recorded, not owned here.

## 4. Invariants and non-goals

**Invariants this plan must preserve.**

1. One request targets one explicitly selected node. No node selection, queue, priority, fairness, DAG, or semantic retry is introduced.
2. Unauthenticated or unauthorized requests cause zero execution side effect.
3. Transport identity stays authoritative; a payload can never supply a principal.
4. Same accepted execution identity and digest cannot spawn twice; stale generations cannot control newer work.
5. Connection loss stays distinct from lease expiry.
6. Event history stays bounded; sequence gaps stay explicit.
7. Restart never blindly repeats uncertain work.
8. `eggwork-core` keeps no internal dependencies; `eggwork-sandbox-helper` keeps no workspace-crate dependency.
9. No authentication, lease, generation, fencing, or digest bound is weakened.
10. No wire representation changes — `ExecutionState` keeps its derived `Serialize`/`Deserialize` with no rename attribute, so it stays externally tagged PascalCase.

**Non-goals.** This plan is not a place to "while we are here" refactor, to rename anything for style, to consolidate the server's two enum-to-string maps into one, or to change a function's signature for tidiness. Every edit must be traceable to CP-1, CP-2, or CP-3.

## 5. Required production changes

### 5.1 CP-1 — one API schema version, owned by `eggwork-core`

1. In `crates/eggwork-core/src/lib.rs`, add `pub const API_SCHEMA_VERSION: u16 = 1;` next to `NODE_TELEMETRY_SCHEMA_VERSION`, with a doc comment stating that it is the control-plane request/response schema version, that every versioned body producer and consumer refers to this constant, and that the value is frozen for schema version 1.
2. In `crates/eggwork-client/src/lib.rs`, delete the private `const API_SCHEMA_VERSION: u16 = 1;` and import `eggwork_core::API_SCHEMA_VERSION`. Every existing use site keeps its behavior.
3. In `crates/eggwork-server/src/lib.rs`, delete the private `const API_SCHEMA_VERSION: u16 = 1;` and import `eggwork_core::API_SCHEMA_VERSION`. Every existing use site keeps its behavior, including the 426 `protocol_version` rejections and the values stamped into responses and into `ExecutionSnapshot.schema_version`.
4. Replace any remaining literal schema-version `1` used to stand for this fact — including the `schema_version: 1` in the `ExecutionSnapshot` fixture in `crates/eggwork-server/src/operations.rs`'s tests — with the shared constant.
5. Do not add a `validate()` to any request type and do not change when the server checks the version. The check sites stay exactly where they are.

### 5.2 CP-2 — capability feature names and the sandbox profile name

**Runner-produced names (owner: `eggwork-runner`).**

6. In `crates/eggwork-runner/src/lib.rs`, export `pub` constants for the three cgroup-v2 resource feature names — for example `CGROUPS_V2_MEMORY_CAPABILITY`, `CGROUPS_V2_CPU_CAPABILITY`, `CGROUPS_V2_PIDS_CAPABILITY` — with doc comments stating that each is the frozen public feature name for that dimension and that the value must not change without an ADR. The exact names are the implementer's choice; the requirement is that they are `pub`, that they are in the runner, and that they are used.
7. In `TrustedLandlockSetup::execution_capabilities`, replace the pushed literal `"isolation.landlock.workspace-rw.v1"` with `LANDLOCK_WORKSPACE_RW_CAPABILITY`, and replace the three literals in the probe table with the new constants.
8. In `crates/eggwork-server/src/lib.rs`, in `check_resources_supported`, replace the three literals with imports of the runner's constants. `check_isolation_supported` needs no change — it already imports `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY`.

**The `exec.argv.v1` name (owner: `eggwork-core`).**

9. In `crates/eggwork-core/src/lib.rs`, add `pub const EXEC_ARGV_CAPABILITY: &str = "exec.argv.v1";` with a doc comment stating that it is the feature name for argv-based execution, that the node advertises it in `NodeCapabilities.features`, and that a client requires it before submitting an execution.
10. In `crates/eggwork-server/src/lib.rs`, `static_capability_features` uses `eggwork_core::EXEC_ARGV_CAPABILITY` in place of the `"exec.argv.v1"` literal.
11. In `crates/eggwork-client/src/lib.rs`, `NodeClient::ensure_protocol_compatible` compares against `eggwork_core::EXEC_ARGV_CAPABILITY` in place of the literal.

**Why `eggwork-core` and not the server.** The server produces the name and the client consumes it, and the client cannot depend on the server. `eggwork-core` is the crate both already depend on, and it already owns the `NodeCapabilities` type whose `features` field carries the name. The owner is the lowest crate every participant depends on.

**The `workspace_rw` profile name (owner: `eggwork-runner`; helper pinned by test).**

12. In `crates/eggwork-runner/src/lib.rs`, export a `pub` constant for the sandbox profile name — for example `WORKSPACE_RW_PROFILE: &str = "workspace_rw"` — with a doc comment stating that it is the only supported `SandboxRequest` profile and that the value must not change without an ADR.
13. Use it at all three runner sites: the profile comparison in `TrustedLandlockSetup::prepare`, the profile construction in `RunnerRequest::from_spec`, and the profile comparison in `SandboxChannel::new`.
14. `crates/eggwork-sandbox-helper/src/main.rs` is **not modified**. The helper's `LaunchSpec` validation keeps its `"workspace_rw"` literal, because the helper has no workspace-crate dependency and adding one would break the standalone-binary rule.
15. The helper's literal is pinned by a test instead: `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` already dev-depends on `eggwork-runner`, so it asserts that the profile the runner produces is accepted by the helper, and that a profile which differs is refused before the target spawns. See §6.4.

**What must not change.**

16. Every capability feature-name value stays byte-for-byte identical. `network.unrestricted.v1`, `events.live.v1`, `auth.mtls.v1`, `blob.sha256.v1`, `blob.stream.v1`, `workspace.manifest.v1`, `workspace.materialize.v1`, `workspace.derive.v1`, `artifact.declared.v1`, `artifact.stream.v1`, and `artifact.retention.v1` are not in scope; they are not duplicated across crates today and must not be moved merely for symmetry.

### 5.3 CP-3 — one terminal-state classification, owned by `eggwork-core`

17. In `crates/eggwork-core/src/lib.rs`, add to `ExecutionState`:
    - a terminal predicate (`is_terminal`) and its complement (`is_non_terminal`);
    - the canonical wire spelling of a state (for example `wire_name`) — the PascalCase string the derived `Serialize` already produces;
    - a way to enumerate every variant (for example a `pub const ALL_EXECUTION_STATES: [ExecutionState; N]` iterated by reference, or a `pub fn all()` accessor), so a property test can range over the whole enum instead of over another hand-written list.

18. `is_terminal` **must** be written as a `match` over all nine variants with **no wildcard arm**, and `is_non_terminal` as its complement. The same exhaustiveness rule applies to the wire-spelling function. This is what makes adding a tenth variant a compile error in core rather than a silent behavior change downstream. A `matches!` with a wildcard, or an `is_non_terminal` written as its own independent list, does not satisfy this requirement.

19. In `crates/eggwork-server/src/store.rs`:
    - remove `fn state_name`; use the core-owned wire spelling at its two call sites (the `state` column write in `ExecutionStore::reserve` and in `ExecutionStore::commit_event`);
    - remove `fn is_terminal_name`; use the core-owned classification at its two call sites;
    - replace the inline `matches!` over the five state-name literals in the generation-reservation path in `ExecutionStore::reserve` with the shared classification, so the path stops duplicating the set that `is_terminal_name` already expressed.

20. In `crates/eggwork-server/src/lib.rs`:
    - remove `fn is_terminal`; use the core-owned predicate at all its call sites;
    - give the artifact-capture eligibility subset (`ExecutionState::Succeeded | ExecutionState::Failed`) a name, and keep it exactly that subset. It is **not** the terminal set and must not be folded into it, nor the terminal set folded into it;
    - replace the wildcard-terminated terminal-to-metric-name match with a derivation from the shared terminal classification, so that a newly added terminal variant cannot silently lose its metric name.

21. In `crates/eggwork-server/src/operations.rs`:
    - replace the non-terminal `matches!` in the active-execution count with the shared classification;
    - replace the hard-coded SQL literal list in `fn count_active_executions` with a list derived from the shared non-terminal spelling, keeping the same four state names and the same query semantics. If the derived list is assembled at runtime, the query must remain parameterized — no string-concatenated SQL;
    - update the comment above `fn count_active_executions` that explains the PascalCase-versus-snake_case distinction, because after this change the two lists are derived from one spelling rather than kept in sync by hand;
    - `fn state_label` stays a display projection. It may remain server-local or move to core, but the decision must be recorded in the closure record, and it must be total over the enum with no wildcard arm, and it must keep its current snake_case output exactly.

22. Do not change the `state` column's stored values, the column's schema, or the JSON stored in the `executions` table. Do not change the store's SQL schema, the journal's event retention, or the recovery classification.

23. Do not add a terminal-state concept to the client as a private copy. The client does not classify states today; if it ever needs to, it must ask `eggwork-core`.

## 6. Required tests

Tests are required. "It compiles" is not acceptance (§8).

### 6.1 CP-1

- **Shared-value pin.** A test in `crates/eggwork-core` asserts `API_SCHEMA_VERSION` equals the frozen value for schema version 1, so that a careless edit to the shared constant is a test failure rather than a silent protocol break.
- **Client-side wire pin.** A test asserts that the client's serialized request bodies carry `eggwork_core::API_SCHEMA_VERSION` in their `schema_version` field. The client currently has no `tests/` directory; either add `crates/eggwork-client/tests/` with a focused serialization test, or place the assertion in the existing client unit-test module. The implementer picks the location and records it.
- **Server-side rejection pin.** The server already has a test that builds an `ExecuteRequest` with `schema_version: API_SCHEMA_VERSION + 1` and asserts a 426 `protocol_version`. Keep it, and make sure it names the shared constant so that the rejection path is proven to use the core-owned value rather than a private copy.
- **Cross-crate agreement.** `crates/eggwork-server/Cargo.toml` already dev-depends on `eggwork-client` (with the `eggress-route` feature). Use that to write a test that observes the actual client-built request body's schema version and asserts it equals the value the server accepts — i.e. that a client request is not rejected as `protocol_version`. This is the test that would have caught CP-1.

### 6.2 CP-2

- **Runner advertisement uses the constants.** A test in `crates/eggwork-runner` asserts `TrustedLandlockSetup::execution_capabilities` output is checked against the exported constants rather than against literals — for example, that when the helper is trusted and the kernel supports the ruleset, the advertised set contains `LANDLOCK_WORKSPACE_RW_CAPABILITY`; and that the probe-table feature names are exactly the exported constants.
- **Value freeze.** The existing `capability_feature_string_is_frozen_for_workspace_rw_profile` test stays and keeps asserting the literal value. Add equivalent freeze assertions for the three cgroup constants and for `EXEC_ARGV_CAPABILITY`, so a value change fails a test even if someone edits the constant and its consumer together.
- **Server consumer imports the producer.** A test in `crates/eggwork-server` asserts `check_resources_supported` recognizes exactly the runner's exported constants — in particular that a required memory/cpu/pids dimension is admitted when the node advertises the runner's constant, and refused with `capability_mismatch` when the advertised name differs by even one character. That refusal case is the CP-2 regression test: it fails if the server and runner drift apart.
- **Client consumer imports the owner.** A test asserts `NodeClient::ensure_protocol_compatible` accepts a node advertising `eggwork_core::EXEC_ARGV_CAPABILITY` and returns the typed unsupported-capability error for a node whose feature list differs.
- **Helper profile pin.** In `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`: the runner-produced profile is accepted by the helper, and a profile that differs is refused before the target spawns. This is the test that pins the helper's unavoidable literal to the runner's constant.

### 6.3 CP-3

- **Totality and disjointness property test.** A test ranges over every `ExecutionState` variant and asserts each one is classified into exactly one of the terminal and non-terminal sets — never both, never neither. It also asserts the terminal set has exactly five members and the non-terminal set exactly four, so a mis-edited classification (`Failed` flipped to non-terminal, say) fails even though the sets still partition.
- **Adding a variant breaks it.** The property test must be written so that adding a variant to `ExecutionState` breaks the build or the test. The primary mechanism is the exhaustive `match` with no wildcard (compile error in core). The test must not be satisfiable by a hand-written list of nine variants that a tenth variant simply joins unnoticed.
- **Wire spelling equals serde output.** A test asserts, for every variant, that the canonical wire spelling equals the enum's serde representation — for example that serializing the variant yields the same string. This is what keeps `state_name` and the `state` column honest against the derived `Serialize`.
- **Store column values unchanged.** A test asserts the `state` column still receives the same PascalCase strings it received before (for example that a `TimedOut` snapshot persists as `TimedOut`), so the storage format did not move.
- **SQL list still selects the non-terminal states.** A test asserts the derived literal list in `fn count_active_executions` selects exactly `Accepted`, `Preparing`, `Running`, and `Cancelling` — no more, no fewer — so the drain/quiescence count keeps its meaning.
- **Capture eligibility preserved.** A test asserts the artifact-capture eligibility predicate accepts exactly `Succeeded` and `Failed`, and rejects `Cancelled`, `TimedOut`, `Interrupted`, and the non-terminal states. It must also assert the subset is a strict subset of the terminal set, so nobody quietly widens it to all five.
- **Metric mapping total.** A test asserts every terminal state maps to a metric name, and that a non-terminal state maps to none.
- **Display labels unchanged.** A test asserts `fn state_label` produces exactly its current snake_case strings for all nine variants.

### 6.4 Test placement constraints

- The helper test in §6.2 lives in `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`, which already dev-depends on `eggwork-runner`. Do not add a workspace-crate dependency to the helper's `[dependencies]`.
- The cross-crate agreement test in §6.1 lives in the server crate, which already dev-depends on `eggwork-client`. Do not make `eggwork-client` a non-dev dependency of the server.
- No test may create a child process outside the repository's existing controlled-fixture patterns, and no test may use a live network.

## 7. Failure, cancellation, restart, and contention semantics

This plan changes no runtime behavior, so it introduces no new failure, cancellation, restart, or contention semantics. The implementing agent must confirm the following rather than assume it:

- **Cancellation.** The cancel/renew path compares a received schema version against the shared constant. Its behavior on a mismatch — the 426 `protocol_version` rejection — is unchanged; only the constant's provenance changes.
- **Restart and recovery.** `ExecutionStore::recover` classifies uncertain work as `Interrupted` using the terminal classification. After this change it uses the shared classification. The recovery outcome for a given journal must be identical before and after. The existing recovery test in `crates/eggwork-server/src/store.rs` (`recovery_marks_uncertain_work_interrupted_and_never_replays_it`) must stay green and must still assert the same outcome.
- **Lease and generation fencing.** `ExecutionStore::reserve`'s generation-reservation path contains the inline `matches!` that this plan replaces. Its behavior must not change: a stale generation is still refused with the same result, and a generation may only advance from a terminal state. The existing reserve tests in `crates/eggwork-server/src/store.rs` must stay green unmodified.
- **Contention.** The `count_active_executions` query is on the drain/quiescence path and is polled on a timer. Changing it from a hard-coded literal list to a derived one must not change its result set, its query plan, or its behavior under concurrent execution. Because the query's `IN` list becomes runtime-assembled, it **must** stay parameterized — string-concatenated SQL is not acceptable and would introduce an injection-shaped defect into an operator path.
- **Failure.** A failure of the new shared-constant mechanism must not be silently tolerated. A compilation error is the intended outcome when the enum and the classification diverge; it is not a defect to work around.

## 8. Compatibility and migration

**Wire compatibility.** Unchanged. The schema version stays `1`. Every capability feature name keeps its exact bytes. `ExecutionState` keeps its derived `Serialize`/`Deserialize` with no rename attribute. The server's `state` column keeps the same PascalCase strings. `eggworkd status` keeps its current snake_case labels.

**Behavioral compatibility.** Unchanged. Every literal this plan replaces is replaced by a constant whose value is identical to the literal it replaces. No execution, admission, authorization, lease, generation, event, artifact, or retention path changes behavior on any platform.

**Rust API compatibility.** The plan adds `pub` constants and `pub` predicates. It removes no `pub` item: the two `API_SCHEMA_VERSION` constants it removes are crate-private. `eggwork-core` gains public items, which is additive.

**Migration.** None. No stored-format migration, no rollout step, no coordination with deployed clients or nodes, because no observable value changes.

**What this plan does not buy.** Sharing a constant prevents *creating* skew inside one build; it cannot remove skew between two already-deployed binaries. A future schema-version bump still needs its own compatibility story. Do not read this plan as pre-authorizing one.

**Recorded, not owned.** `architecture/overview.md` divergence #20 states that the client's `decode_json` reads a response body with no size cap. At the reviewed baseline the client bounds accumulated bodies at the transport builder — `NodeClient::new` and `NodeClient::with_eggress` both set `max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)` on the Eggfetch client builder — so that text no longer matches the code. This plan records the observation and leaves #20's disposition to the owner of the architecture record.

## 9. Documentation updates

Required in the same pass as the code change:

1. **`architecture/core-domain.md`** — the **States** paragraph currently states that there is no `is_terminal()` helper and no terminal-state grouping in core. After M004 that is false. Rewrite it to describe the new ownership, and replace its line-number citations with symbol references. Do not touch its other findings.
2. **`architecture/overview.md`** — divergence #12 says the terminal/non-terminal sets "are spelled out in four places". Reconcile it with the implemented reality and its new count, and note that the classification is now owned by `eggwork-core`. Preserve the divergence's original substance; this is an update to a recorded observation, not a deletion.
3. **`architecture/client-transport.md`** — it documents that the client sends `schema_version: API_SCHEMA_VERSION` and that this matches the server's own constant. After M004 both refer to one core-owned constant; update that sentence and replace its line-number citation with a symbol reference.
4. **`architecture/server-node.md`** — it names `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY` as the constant the isolation check requires. Verify the sentence still matches the code after M004 and update if the wording drifted; replace the surrounding line-number citations with symbol references.
5. **`architecture/storage-journal.md`** — it records that `ExecutionSnapshot.schema_version` is set to `API_SCHEMA_VERSION` at construction and that the `state` column's spellings have a hand-maintained relationship. Update to state that the value and the spelling are now core-owned and derived.
6. **`architecture/runner-execution.md`** — it describes the runner's capability advertisement and the `LANDLOCK_WORKSPACE_RW_CAPABILITY` constant. Update the description of where the feature names now come from, and replace the stale line-number citations with symbol references.
7. **No `docs/` change.** This corrective is invisible to an operator and to a user: no verb, flag, config field, endpoint, wire value, or observable behavior changes. Do not add an operator-facing note about it.

## 10. Required verification commands

The repository's pre-submit gate, exactly:

```text
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

`--locked` is load-bearing on `clippy` and `test`: a version bump that skips `Cargo.lock` passes both and then fails every release build.

Additional focused runs the implementer must perform and record, beyond the whole-workspace commands:

```text
cargo test --locked -p eggwork-core --all-targets
cargo test --locked -p eggwork-client --all-targets
cargo test --locked -p eggwork-runner --all-targets
cargo test --locked -p eggwork-server --all-targets
cargo test --locked -p eggwork-sandbox-helper --all-targets
```

`cargo test --locked -p eggwork-client --features eggress-route --all-targets` is additionally required, because the server's dev-dependency on the client enables that feature and the cross-crate agreement test in §6.1 must compile and run under it.

**Platform and feature matrix actually exercised must be recorded** — not claimed. State plainly which host the run happened on and which platform-specific paths were skipped. The runner's Linux-only Landlock and cgroup paths, the Windows Job Object path, and the macOS fail-closed path each have their own conditions; record which ones the local run actually covered and which it did not. Do not claim a platform is qualified because its code compiles.

## 11. Acceptance criteria

Each criterion is observable and checkable.

1. `API_SCHEMA_VERSION` is declared exactly once in the workspace, publicly, in `crates/eggwork-core/src/lib.rs`; neither `crates/eggwork-client/src/lib.rs` nor `crates/eggwork-server/src/lib.rs` declares a local schema-version constant.
2. The client's request bodies and the server's acceptance and rejection paths both resolve to `eggwork_core::API_SCHEMA_VERSION`; verified by source inspection **and** by a test that would fail if a private copy were reintroduced.
3. A test exists that fails if the client-sent schema version and the server-accepted schema version can diverge, and it exercises the real client-built request body.
4. The runner exports `pub` constants for all three cgroup-v2 resource feature names, and `TrustedLandlockSetup::execution_capabilities` uses them; no capability feature literal remains at the runner's advertisement site.
5. The server's `check_resources_supported` imports the runner's constants; the cgroup feature names are not re-typed in the server.
6. `exec.argv.v1` is owned by `eggwork-core` and imported by both the server's `static_capability_features` and the client's `ensure_protocol_compatible`; the literal appears in neither endpoint crate's production code.
7. The `"workspace_rw"` profile name is a `pub` constant in `eggwork-runner`, used at all three runner sites; the helper's production code is unmodified and its literal is pinned by a test that proves a differing profile is refused before the target spawns.
8. Every capability feature-name value is byte-for-byte identical to its pre-M004 value, proven by freeze assertions.
9. `eggwork-core` owns the terminal/non-terminal classification; the classification is an exhaustive `match` over all nine variants with no wildcard arm, and the wire-spelling function is likewise exhaustive.
10. No terminal-state enumeration remains in `crates/eggwork-server/src/store.rs`, `crates/eggwork-server/src/lib.rs`, or `crates/eggwork-server/src/operations.rs` — including the inline `matches!` in the generation-reservation path, the inline capture-eligibility subset, the metric-name match's wildcard arm, and the SQL literal list.
11. A property test proves the classification is total and disjoint over every variant, and adding a variant to `ExecutionState` breaks the build or the test.
12. The `state` column stores the same values as before, and the SQL that counts active executions selects exactly `Accepted`, `Preparing`, `Running`, and `Cancelling`.
13. The artifact-capture eligibility predicate accepts exactly `Succeeded` and `Failed` and remains distinct from terminality.
14. The terminal-to-metric-name mapping is total over the terminal set, and the operator-facing state labels are byte-identical to their current output.
15. `crates/eggwork-core/src/lib.rs`'s `ExecutionState` wire representation is unchanged — still the derived `Serialize`/`Deserialize` with no rename attribute.
16. No new crate dependency edge exists; `eggwork-core` still has no internal dependency; `eggwork-sandbox-helper` still has no workspace-crate dependency.
17. The seven verification commands in §10 all pass, and the focused per-crate runs plus the `eggress-route` run are recorded.
18. The documentation updates in §9 are applied, and the touched documents cite by symbol name rather than by line number.
19. `plans/closure/control-plane-protocol/004-status.md` exists and satisfies §12.
20. No high or medium finding remains without an explicit owner.

## 12. Stop conditions

Stop, do not improvise, and escalate to the plan's owner if any of these occur.

1. **A feature-string value would have to change.** If sharing a capability constant appears to require changing any feature name's bytes — including `exec.argv.v1`, the three `resources.cgroups-v2.*` names, `isolation.landlock.workspace-rw.v1`, or `network.unrestricted.v1` — stop. A value change is a compatibility break that needs an ADR, and `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` §3 already froze these semantics. Preserve the values; change only the ownership.
2. **A dependency edge the crate rules forbid.** The rules are: `eggwork-core` and `eggwork-sandbox-helper` depend on nothing internal; `eggwork-client` depends on `eggwork-core`; `eggwork-server` depends on `eggwork-core` and `eggwork-runner`; `eggwork-client` is a dev-dependency of `eggwork-server` only. If a shared constant would require violating one of these — in particular, giving `eggwork-sandbox-helper` a workspace dependency, or giving `eggwork-client` a dependency on `eggwork-server` or `eggwork-runner` — stop and re-plan. The correct answer to a blocked owner is a different owner or a test-pinned literal, not a new edge.
3. **The core ownership change needs architecture review.** Giving `eggwork-core` a terminal-state classification and a schema-version constant is an intentional ownership change to a crate that currently owns neither. Per `plans/003-planning-process.md` §8, stop before implementing if that change has not been recorded as an ADR or canonical amendment, and report the blocker.
4. **The artifact-capture eligibility rule would have to widen.** If consolidating the enumerations appears to require capturing artifacts for `Cancelled`, `TimedOut`, or `Interrupted`, stop. That rule is a distinct policy subset and this plan must not touch it.
5. **The store's stored format would have to move.** If deriving the SQL literal list or the `state` spelling would change the stored bytes, the column schema, or the JSON in the `executions` table, stop and re-plan. This plan changes where the spelling comes from, not what it is.
6. **A test would become vacuous.** If any required test in §6 cannot be made non-vacuous — for example if the cross-crate agreement test cannot observe the client's real request body — stop and report. A test that passes for the wrong reason is worse than no test.
7. **A runtime behavior change is discovered to be required.** If any part of this plan appears to need a behavior change on the server or the runner beyond replacing a literal with the constant it already equals, stop. That means the plan's scope was wrong, not that the behavior should change.
8. **The `state_label` disposition cannot be decided.** If it is genuinely unclear whether the display map stays server-local or moves to core, either choice is acceptable — but the decision must be recorded in the closure record. Stop only if the ambiguity would change an operator-visible output.

## 13. Closure evidence required

Create `plans/closure/control-plane-protocol/004-status.md`. Per `plans/003-planning-process.md` §6, it MUST distinguish code inspection from executed evidence, and SHOULD include the following.

**Required content.**

1. **Reviewed baseline and implementation.** The reviewed head, the implementation commit(s), and the exact crates and symbols changed. Cite locations by file path and symbol name, not by line number.
2. **Requirement-to-evidence matrix.** One row per acceptance criterion in §11, each naming the specific implementation site and the specific test that proves it. A criterion with no named test is not closed.
3. **Tests and commands actually run.** The seven pre-submit commands from §10, with their actual outcomes, plus the focused per-crate runs and the `eggress-route` run. Record failures and retests honestly; a rerun is not a fix.
4. **Platform and feature matrix actually exercised.** The host actually used, and for each of the runner's platform paths (Linux Landlock/cgroup, Windows Job Object, macOS fail-closed) whether it was exercised, skipped, or not run. State plainly what was **not** exercised. Do not claim a platform or feature is qualified because its code compiles.
5. **Negative and adversarial evidence.** The cases that must fail: a schema-version mismatch still yields 426 `protocol_version`; a resource capability name differing by one character still yields `capability_mismatch` at admission; a sandbox profile differing from the runner's still fails before target spawn; the artifact-capture predicate still rejects `Cancelled`, `TimedOut`, and `Interrupted`; the SQL list still excludes terminal states.
6. **Compatibility review.** Evidence that no wire value, wire field, feature-string byte, stored format, or operator-visible output changed. If any changed, say so explicitly and treat it as a deviation.
7. **Security review.** Confirmation that authentication, authorization, lease, generation, fencing, and digest semantics are untouched, and that the ownership guard still passes — in particular that the shared constants introduce no new child-process creation path and no new dependency edge.
8. **Documentation status.** Which of the six documents in §9 were updated, and confirmation that the touched documents cite by symbol name.
9. **Unresolved findings by severity.** With an owner for each. A high or medium finding without an owner means the milestone is not closed.
10. **Decision record.** The two dispositions §2 left open and the plan required to be recorded: where `fn state_label` lives, and whether the artifact-capture eligibility predicate stayed server-local or moved to `eggwork-core`.
11. **Registry and roadmap disposition.** Note that `plans/registry.md` and `plans/subsystems/control-plane-protocol-roadmap.md` are owned by other writers and must be updated by them; do not edit them as part of this milestone.
12. **Deviations.** Anything that landed differently from this plan, with the reason.

**Do not** rewrite the historical M001–M003 closure records. This corrective's record is additional.

## 14. Handoff notes

- **One agent, one bounded change.** This is a constant-and-derivation refactor across three crates. It is small per site and wide per crate; keep the edits mechanical and do not wander into the surrounding code.
- **Read the corrective addendum first.** `plans/subsystems/control-plane-protocol-shared-contract-corrective-addendum.md` owns the findings and the ownership decisions. This plan owns the execution. Where they disagree, the addendum wins and the disagreement is a defect in this plan.
- **Start with CP-3's core change**, then CP-1, then CP-2. CP-3 is the one with the ownership question, so it is the one to get reviewed first; §5.3 item 17–18 is the part that needs the ADR named in §12 item 3.
- **The executor's crate dependency rules, verbatim:** `eggwork-core` → nothing; `eggwork-sandbox-helper` → nothing; `eggwork-client` → `eggwork-core`; `eggwork-server` → `eggwork-core` + `eggwork-runner`; `eggwork-client` is dev-only for the server. `eggwork-runner` → `eggwork-core`. No new edge may be introduced by this milestone.
- **The executor's scheduler-free constraint:** this milestone must not introduce node selection, a queue, priority, fairness, a DAG, cross-node placement, or semantic retry. One request targets one explicitly selected node.
- **Do not trust the interrogation notes over the code.** §2 records two places where the notes were wrong and the code was followed. Re-read every site before editing; the notes are a starting point, not an authority.
- **Cite by symbol, not by line.** This repository's architecture deep dives carry a stale-line-number defect at a large scale. New and edited planning documents must not add to it. If a line number is genuinely unavoidable, mark it as being at the reviewed baseline and re-verifiable before editing.
- **Do not claim anything was run.** The closure record is where runs are reported; a plan or an implementation note is not evidence.
- **Handoff is incomplete until the registry names this plan.** `plans/registry.md` is owned by another writer. Do not edit it; report that registration is required.
