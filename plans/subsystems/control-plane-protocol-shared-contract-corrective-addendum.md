# Control-Plane Protocol Post-Closure Corrective — Shared Protocol Contract Ownership

Status: active corrective; M004 proposed

Predecessor roadmap:

- `plans/subsystems/control-plane-protocol-roadmap.md`

Historical predecessor closures:

- `plans/closure/control-plane-protocol/001-status.md` — authenticated fixed-target execution
- `plans/closure/control-plane-protocol/002-status.md` — idempotency, leases, resumable events, recovery
- `plans/closure/control-plane-protocol/002-lease-expiry-race-correction.md` — lease-expiry race correction
- `plans/closure/control-plane-protocol/003-status.md` — Eggress route adapter and protocol compatibility hardening

Canonical authority remains:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `plans/003-planning-process.md`

Finding source:

- repository-wide code interrogation pass over the reviewed baseline, covering `crates/eggwork-core/src/lib.rs`, `crates/eggwork-client/src/lib.rs`, `crates/eggwork-server/src/lib.rs`, `crates/eggwork-server/src/store.rs`, `crates/eggwork-server/src/operations.rs`, `crates/eggwork-runner/src/lib.rs`, and `crates/eggwork-sandbox-helper/src/main.rs`, cross-read against `architecture/overview.md`, `architecture/core-domain.md`, `architecture/client-transport.md`, `architecture/server-node.md`, `architecture/storage-journal.md`, `architecture/sandbox-helper.md`, and `architecture/runner-execution.md`.

Relationship to the recorded divergences table:

- **CP-2 is new.** It is not recorded in `architecture/overview.md`.
- **CP-3 is a widening of divergence #12**, not a wholly new defect. Divergence #12 says the terminal/non-terminal sets "are spelled out in four places with nothing keeping them in sync". The actual count of enumeration sites in the server is nine, and the root cause #12 does not name — that `eggwork-core` owns no terminal concept at all — is documented separately in `architecture/core-domain.md` under **States**. This corrective owns the delta: the count, the root cause, and the ownership rule.
- **CP-1 is new.** It is not recorded in `architecture/overview.md`. Divergence #20 is adjacent in subject area (the client's wire framing) but describes a different fact, and its text is now stale — see §7.3. This corrective records that observation; it does not own #20's disposition.

All three findings share one root cause: **a protocol contract fact is stated by string literal in more than one crate, with no shared owner and no test pinning the two sides together.** The facts are individually small. The failure mode they share is not: a rename or a bump on the producing side silently desynchronizes the consuming side, and no compiler or test in the workspace objects.

## 1. Findings

### 1.1 CP-1 — `API_SCHEMA_VERSION` is defined twice, in the two crates that must agree

**What the contract is supposed to be.** One control-plane schema version, owned by one crate, checked by every side that sends or receives it.

**What the code actually does.**

- `crates/eggwork-client/src/lib.rs` declares a private `const API_SCHEMA_VERSION: u16 = 1;` and stamps it into every request body it builds — the execute body, the control body, the workspace-create body, and the workspace-derive body.
- `crates/eggwork-server/src/lib.rs` declares its own private `const API_SCHEMA_VERSION: u16 = 1;`. The server compares the received value against it and hard-rejects any mismatch with HTTP 426 `protocol_version` — in the workspace-create handler, the workspace-derive handler, the execute handler, the execute spec's own embedded version, the cancel/renew path, and again when it stamps its own responses.
- Nothing links the two declarations. `crates/eggwork-client/Cargo.toml` already depends on `eggwork-core`, so a shared owner is available without a new dependency edge.

The correct precedent already exists in the same codebase. `crates/eggwork-core/src/lib.rs` declares `pub const NODE_TELEMETRY_SCHEMA_VERSION: u16 = 1;`, and that one constant is consumed by the client's telemetry reader, by the server's telemetry projection, and by core's own `NodeTelemetry::validate`. That is exactly the shape `API_SCHEMA_VERSION` should take.

**Consequence.** A server-side bump silently breaks every deployed client with a runtime HTTP 426 at request time. Both sides agree with themselves, so no test in the workspace can catch it: there is no artifact in the build that fails when the two constants diverge.

### 1.2 CP-2 — Capability feature-name strings are duplicated across the runner/server/client boundary

**What the contract is supposed to be.** A capability feature name is a wire token. The node advertises it; a consumer matches on it. Producer and consumer must refer to the same token.

**What the code actually does.**

- The runner produces the names. `crates/eggwork-runner/src/lib.rs`, in `TrustedLandlockSetup::execution_capabilities`, pushes the literal `"isolation.landlock.workspace-rw.v1"` and carries the literals `"resources.cgroups-v2.memory"`, `"resources.cgroups-v2.cpu"`, and `"resources.cgroups-v2.pids"` in its probe table.
- The same file already declares `pub const LANDLOCK_WORKSPACE_RW_CAPABILITY: &str = "isolation.landlock.workspace-rw.v1";` with the identical value — but the push site does not use it. The constant is used by its own freezing test and by the server, while the producer of the value types it out again.
- The server consumes the names. `crates/eggwork-server/src/lib.rs`, in `check_resources_supported`, matches `"resources.cgroups-v2.memory"`, `"resources.cgroups-v2.cpu"`, and `"resources.cgroups-v2.pids"` as literals. The server's `check_isolation_supported` is the one exception in this group: it already imports `eggwork_runner::LANDLOCK_WORKSPACE_RW_CAPABILITY`.
- The server also produces `"exec.argv.v1"` in `static_capability_features`, and the client independently hardcodes `"exec.argv.v1"` in `NodeClient::ensure_protocol_compatible`.
- The `"workspace_rw"` sandbox profile name is written as a literal in the runner — in `TrustedLandlockSetup::prepare`'s profile check, in `RunnerRequest::from_spec`'s profile construction, and in `SandboxChannel::new`'s profile check — and once in `crates/eggwork-sandbox-helper/src/main.rs`, in the `LaunchSpec` validation that rejects any other profile. The runner's seven occurrences include doc comments; the helper's production occurrence is a single site, with six more in the helper's integration test.

**Consequence.** A rename on the producer side makes the consumer silently stop recognizing the capability, producing a runtime `capability_mismatch` at admission with no compile-time signal and no test pinning the two sides. The Landlock capability is the sharpest illustration: the constant exists, the value is identical, and the site that emits the value still types it out.

The `"workspace_rw"` profile name is a special case, because `crates/eggwork-sandbox-helper/Cargo.toml` declares no workspace-crate dependencies at all — it is a standalone binary whose only workspace relationships are dev-dependencies. The helper therefore cannot import a shared constant from the runner or from core. That constraint is a rule, not an obstacle to route around; §3.3.4 states how the helper's literal is pinned instead.

### 1.3 CP-3 — The terminal-state set is enumerated many times, and `eggwork-core` owns no terminal concept

**What the contract is supposed to be.** One classification of `ExecutionState` into terminal and non-terminal, owned by the crate that defines the enum.

**What the code actually does.**

`crates/eggwork-core/src/lib.rs` defines `ExecutionState` as a flat nine-variant enum — `Accepted`, `Preparing`, `Running`, `Cancelling`, `Succeeded`, `Failed`, `Cancelled`, `TimedOut`, `Interrupted` — and owns no terminal concept: there is no `is_terminal` helper, no terminal grouping, and no terminal-state list anywhere in the crate. `architecture/core-domain.md` states this explicitly: "There is no `is_terminal()` helper and no terminal-state grouping in this crate — the five terminal variants are a convention that the server applies."

The convention is applied nine times across the server:

| Site | What it enumerates |
|---|---|
| `crates/eggwork-server/src/store.rs` — `fn state_name` | hand-written enum-to-string map over all nine variants, duplicating the serde derive's PascalCase naming |
| `crates/eggwork-server/src/store.rs` — `fn is_terminal_name` | the terminal set as five string literals |
| `crates/eggwork-server/src/store.rs` — the generation-reservation path in `ExecutionStore::reserve` | an inline `matches!` over the same five string literals, which does not call `is_terminal_name` even though that helper exists in the same file |
| `crates/eggwork-server/src/lib.rs` — `fn is_terminal` | the terminal set as five enum variants |
| `crates/eggwork-server/src/lib.rs` — the artifact-capture eligibility check | an inline two-element subset, `ExecutionState::Succeeded \| ExecutionState::Failed` |
| `crates/eggwork-server/src/lib.rs` — the terminal-to-metric-name match | a wildcard-terminated match over the five terminal states |
| `crates/eggwork-server/src/operations.rs` — the active-execution count | the non-terminal set as four enum variants |
| `crates/eggwork-server/src/operations.rs` — `fn count_active_executions` | a parallel SQL literal list of the same four state names |
| `crates/eggwork-server/src/operations.rs` — `fn state_label` | a second full enum-to-string map, using snake_case where `state_name` uses PascalCase |

**Consequence.** Adding a state variant requires edits at roughly nine sites, and nothing mechanically prevents the sets from drifting. Two of the sites are already visibly adrift in shape: the artifact-capture subset is not the terminal set and would be wrong to conflate with it, and `state_label` uses a different casing convention from `state_name` for the same enum.

The client does not currently duplicate the terminal set — `crates/eggwork-client/src/lib.rs` never classifies a state. That is precisely why putting the classification in `eggwork-core` matters: core is the one crate both `eggwork-server` and `eggwork-client` depend on, so the client becomes able to ask the same question the server asks, without either side owning a private copy of the answer.

## 2. Corrective milestone

### M004 — Shared protocol contract ownership

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/control-plane-protocol/004-shared-protocol-contract-corrective.md`

Objective:

Give each of the three duplicated contract facts a single owner, so that client, server, and runner agree by construction rather than by convention, and a future change to any of them is a compile-time or test-detectable event.

Status is **proposed**, not **ready**, for three reasons recorded here rather than left to the implementing agent:

1. M004 requires an intentional, reviewed ownership change to `eggwork-core`: the crate must take ownership of a terminal-state classification it does not currently have. `plans/003-planning-process.md` §8 requires that an ownership-boundary change be recorded as an ADR or canonical amendment before the implementation lands. That record does not exist yet.
2. Two dispositions are still open and are not implementation detail: where the operator-facing `state_label` map lives, and whether the artifact-capture eligibility predicate stays server-local or moves to core. §3.4.4 states the constraint on each; the plan must record the decision it makes.
3. This corrective is not registered in `plans/registry.md`. Registry ownership is not this document's, so a handoff before registration would present a plan as ready when the control surface does not say so.

## 3. Shared-contract design

### 3.1 One owner per contract fact

The corrective establishes one rule and applies it three times:

> A protocol contract fact — a schema version, a capability feature name, a state classification — has exactly one owner. Every producer and every consumer refers to it through that owner. The owner is the lowest crate that every participant already depends on, or the producing crate when every consumer already depends on the producer.

The second clause matters, because "the producer owns it" is not always available. `crates/eggwork-client/Cargo.toml` depends on `eggwork-core` only; the client cannot depend on the server, and the server cannot be depended upon by the client without inverting the workspace's one-way dependency rule. Where producer-ownership is therefore impossible, the owner is the crate both sides already share.

The workspace dependency rules this corrective operates under are fixed: `eggwork-core` and `eggwork-sandbox-helper` depend on nothing internal; `eggwork-client` depends on `eggwork-core`; `eggwork-server` depends on `eggwork-core` and `eggwork-runner`; `eggwork-client` is a dev-dependency of `eggwork-server` only.

### 3.2 CP-1 — the API schema version

Owner: `eggwork-core`.

`crates/eggwork-core/src/lib.rs` declares `pub const API_SCHEMA_VERSION: u16 = 1;` alongside `NODE_TELEMETRY_SCHEMA_VERSION`, with a doc comment stating that it is the control-plane request/response schema version and that every producer and consumer of a versioned body refers to this constant.

`crates/eggwork-client/src/lib.rs` and `crates/eggwork-server/src/lib.rs` delete their private constants and import the core one.

The precedent is `NODE_TELEMETRY_SCHEMA_VERSION`, which already has exactly this shape: one `pub const` in core, consumed by the client, the server, and core's own validator.

### 3.3 CP-2 — capability feature names and the sandbox profile name

#### 3.3.1 Runner-produced resource capabilities

Owner: `eggwork-runner`, because the server already depends on the runner and the runner is where the probe produces the fact.

`crates/eggwork-runner/src/lib.rs` exports public constants for the three cgroup-v2 resource feature names alongside `LANDLOCK_WORKSPACE_RW_CAPABILITY`. `TrustedLandlockSetup::execution_capabilities` uses those constants in the probe table instead of literals.

#### 3.3.2 The Landlock capability's unused constant

Owner: `eggwork-runner`, already correct.

The only change is that `TrustedLandlockSetup::execution_capabilities` pushes `LANDLOCK_WORKSPACE_RW_CAPABILITY` instead of the literal with the identical value. The value itself does not change. `crates/eggwork-server/src/lib.rs` needs no change for this name — `check_isolation_supported` already imports the constant.

#### 3.3.3 `exec.argv.v1`

Owner: `eggwork-core`.

This is the case where producer-ownership is impossible. The server produces the name, in `static_capability_features`; the client consumes it, in `NodeClient::ensure_protocol_compatible`; and the client cannot depend on the server. `eggwork-core` is the crate both already depend on, and it already models the type the name appears in — `NodeCapabilities.features` — so the constant belongs there beside the capability contract rather than in either endpoint crate.

#### 3.3.4 The `workspace_rw` profile name

Owner: `eggwork-runner` for the producing side.

`crates/eggwork-runner/src/lib.rs` exports a public profile-name constant and uses it at all three of its sites: the profile check in `TrustedLandlockSetup::prepare`, the profile construction in `RunnerRequest::from_spec`, and the profile check in `SandboxChannel::new`.

`crates/eggwork-sandbox-helper/src/main.rs` keeps its literal. The helper has no workspace-crate dependency, and adding one would violate the standalone-binary rule that keeps the helper out of the workspace build graph. The helper's literal is therefore pinned by a test rather than by an import: the helper's integration test `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` already dev-depends on `eggwork-runner`, so it can assert that the profile the runner produces is the profile the helper accepts, and that a profile that differs is refused before the target spawns.

This is the one case where the corrective does not achieve "by construction" on both sides, and it says so plainly rather than implying the helper was also fixed.

#### 3.3.5 What this corrective does not touch

The capability feature-name **values** are frozen by `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` §3, which defines the semantics of `exec.argv.v1`, `isolation.landlock.workspace-rw.v1`, `network.unrestricted.v1`, and the three `resources.cgroups-v2.*` names. This corrective preserves those values byte-for-byte and re-specifies none of their semantics. A change to any of those values is a compatibility break that needs an ADR, not this corrective.

### 3.4 CP-3 — the terminal-state classification

Owner: `eggwork-core`.

#### 3.4.1 One classification, in the crate that defines the enum

`crates/eggwork-core/src/lib.rs` gains, for `ExecutionState`:

- a terminal predicate and its complement;
- the canonical wire spelling of a state — the PascalCase name that the enum's derived `Serialize` already produces, and that `crates/eggwork-server/src/store.rs` writes into the `state` column;
- a way to enumerate every variant, so a property test can range over the whole enum rather than over a hand-written list that would itself be another copy.

The wire representation of `ExecutionState` does not change. The enum keeps its derived `Serialize`/`Deserialize` with no rename attribute, so it stays externally tagged PascalCase. The canonical spelling is a function of the enum, not a parallel table.

#### 3.4.2 Exhaustive by construction

The classification must be written as a `match` over all nine variants with no wildcard arm, so that adding a tenth variant is a compile error in core rather than a silent behavioural change downstream. The same applies to the wire-spelling function and to any server-side map derived from it.

This is the mechanism that makes the corrective's central claim true: adding a state variant becomes a compile-time event.

#### 3.4.3 Derived server helpers

- `crates/eggwork-server/src/store.rs`: `fn state_name` and `fn is_terminal_name` are removed in favour of the core-owned spelling and classification; the inline `matches!` in the generation-reservation path calls the shared classification instead of repeating the five string literals; the `state` column continues to hold exactly the same PascalCase strings it holds today.
- `crates/eggwork-server/src/lib.rs`: `fn is_terminal` is removed in favour of the core-owned predicate.
- `crates/eggwork-server/src/operations.rs`: the non-terminal `matches!` and the SQL literal list in `fn count_active_executions` are derived from the shared classification rather than hand-typed. The `state` column values the SQL matches against remain the same strings.

#### 3.4.4 Two things that are *not* the terminal set

The corrective must not conflate either of these with terminality:

- **Artifact-capture eligibility.** The server captures workspace outputs only for `Succeeded` and `Failed`, not for all five terminal states. That is a narrower policy subset, not the terminal set. It must keep its own name and its own test.
- **`state_label`'s casing.** `crates/eggwork-server/src/operations.rs` renders operator-facing state labels in snake_case, while the wire/column spelling is PascalCase. The operator-visible output must not change, so the label casing is a display projection and is not a protocol fact. It may stay server-local or move to core, but whichever it does, it must be total over the enum and pinned by a test, and it must not become a third independent naming scheme.

### 3.5 What the corrective does not change

- No wire field, field name, or value.
- No capability feature-string value and no capability semantics.
- No authorization, lease, generation, fencing, or digest behavior.
- No runtime behavior of the runner or the server beyond replacing a literal with the constant it already equals.
- No new crate dependency edge.

## 4. Durable invariants

These extend the control-plane subsystem's existing invariants; none of them is weakened.

1. A protocol contract fact has exactly one owner.
2. A consumer never re-types a producer's contract token; it imports it.
3. The owner of a contract fact is a crate every participant already depends on, or the producing crate when every consumer already depends on the producer.
4. A change to an owned contract fact is a compile-time or test-detectable event, not a runtime desynchronization.
5. The terminal/non-terminal classification is total over `ExecutionState` and disjoint between the two sets, and is owned by `eggwork-core`.
6. The canonical wire spelling of a state is a function of the enum, and equals the enum's serde representation.
7. `eggwork-core` continues to have no internal dependencies.
8. `eggwork-sandbox-helper` continues to have no workspace-crate dependency.
9. No capability feature-name value changes under this corrective.
10. Authentication, authorization, lease, generation, fencing, and digest bounds are unchanged.
11. Eggwork remains scheduler-free: one explicit target, typed local refusal, caller-owned retry.
12. The historical M001–M003 closure records remain immutable; this corrective owns the newly discovered shared-contract defect.

## 5. Compatibility and migration

**Wire compatibility.** Unchanged. The schema version value stays `1`; the capability feature names keep their exact byte sequences; `ExecutionState` keeps its serde representation; the server's `state` column keeps the same PascalCase strings; `eggworkd status` keeps its current snake_case labels.

**Behavioral compatibility.** Unchanged. Every literal this corrective replaces is replaced by a constant whose value is identical to the literal it replaces. There is no runtime path whose behavior differs before and after, on any platform.

**Rust API compatibility.** The corrective adds public constants and public predicates. It does not remove any public item. The two private `API_SCHEMA_VERSION` constants are crate-private and are removed, which is not a public API change.

**Migration.** None. There is no stored-format migration, no rollout step, and no coordination with deployed clients or nodes, because no observable value changes.

**The one asymmetry worth recording.** Sharing a constant makes divergence *harder*, not impossible, in one direction: the server and client now move together at build time, but an already-deployed client and an already-deployed server were compiled separately and can still differ. The corrective removes the possibility of *creating* that skew inside one build; it cannot remove skew between two existing builds. Any future schema-version bump still needs a compatibility story of its own, and this corrective does not provide one.

## 6. Downstream relationships

- `eggwork-client` and `eggwork-server` are the two CP-1 participants and the two CP-2 parties on opposite sides of the `exec.argv.v1` boundary. Both are in scope.
- `eggwork-runner` is the producer for the Landlock and cgroup capability names and for the `workspace_rw` profile name. It is in scope as a producer, not as a consumer.
- `eggwork-sandbox-helper` is **not** modified. Its `workspace_rw` literal is pinned by a test that lives in the helper's existing integration test, which already dev-depends on the runner.
- `eggwork-core` gains ownership of the API schema version, the `exec.argv.v1` feature name, and the terminal-state classification. This is an ownership change to the crate that currently owns nothing about protocol negotiation, and it is the part of this corrective that needs an architecture record rather than only an implementation.
- The security/isolation/resource remote-admission corrective C001 already closed and its capability contract remains authoritative for feature-name semantics. This corrective must not reopen it. `plans/subsystems/foundation-execution-core-request-validation-corrective-addendum.md` §6 already anticipates a "control-plane shared-contract corrective" and states that the two do not interact; that statement holds.
- Operations M007 (Windows service host) is independent. It may proceed in parallel.
- CodeGG consumes Eggwork's capability features from outside the workspace. Since this corrective changes no feature value, no CodeGG change is implied and no CodeGG re-qualification is triggered.

## 7. Exit conditions

M004 closes only when:

### 7.1 CP-1

- Exactly one `API_SCHEMA_VERSION` declaration exists in the workspace, and it is public in `eggwork-core`.
- Neither `crates/eggwork-client/src/lib.rs` nor `crates/eggwork-server/src/lib.rs` declares a local schema-version constant.
- The client's request bodies and the server's acceptance check refer to the same constant, provably so from the source.
- A test fails if the client-sent version and the server-accepted version can diverge, and a test asserts the server's 426 rejection path uses the shared constant.

### 7.2 CP-2

- The runner's capability-name constants are `pub` in `eggwork-runner`, and the runner's advertisement site uses them rather than literals.
- The server's `check_resources_supported` imports the runner's constants rather than re-typing them.
- `exec.argv.v1` has one owner reachable by both the server and the client, and neither endpoint crate re-types it.
- The helper's `workspace_rw` literal is pinned by a test that fails if the runner's profile name and the helper's accepted profile diverge, and the negative case proves the helper still refuses a different profile before the target spawns.
- Every capability feature-name value is byte-for-byte what it was before the corrective.

### 7.3 CP-3

- `eggwork-core` owns the terminal/non-terminal classification, and the classification is exhaustive over the enum with no wildcard arm.
- No terminal-state enumeration remains in `crates/eggwork-server/src/store.rs`, `crates/eggwork-server/src/lib.rs`, or `crates/eggwork-server/src/operations.rs`.
- The store's `state` column values and the operations SQL literal list are derived from the shared spelling, and a test asserts the derived SQL list still selects exactly the non-terminal states.
- A property test proves the classification is total and disjoint over every `ExecutionState` variant, and that adding a variant to the enum breaks the build or the test.
- The artifact-capture eligibility subset remains exactly `Succeeded` and `Failed`, and remains distinct from terminality.
- The operator-facing state labels are unchanged from their current snake_case spelling.
- A test asserts the canonical wire spelling equals the enum's serde output for every variant.

### 7.4 General

- `python3 scripts/check_execution_ownership.py` passes.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- `cargo test --locked --workspace --all-targets` passes.
- `cargo check --workspace` passes.
- `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` passes.
- `git diff --check` passes.
- No new crate dependency edge exists.
- The `architecture/core-domain.md` **States** paragraph and the `architecture/overview.md` divergence #12 row are reconciled with the code in the same pass, including replacement of their stale line-number citations with symbol references.
- No high or medium finding remains without an explicit owner.

### 7.5 Observation recorded, not owned

`architecture/overview.md` divergence #20 states that the client's `decode_json` reads the response body with "no size cap". At the reviewed baseline the client bounds accumulated response bodies at the transport builder — `NodeClient::new` and `NodeClient::with_eggress` both set `max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)` on the Eggfetch client builder — so the divergence's text no longer matches the code. This corrective records the observation and leaves #20's disposition to the owner of the architecture record; it neither reopens nor closes that item.

## 8. Note on citation style

This repository's architecture deep dives carry a known citation defect: a large fraction of their `file.rs:line` citations no longer point at the symbol they claim. This corrective, and the implementation plan it authorises, cite locations by file path and symbol name only. A line number appears in these documents only where it is genuinely unavoidable, and is marked as being at the reviewed baseline and re-verifiable before editing.
