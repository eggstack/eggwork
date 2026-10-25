# Security M005c — Resource-scoped authorization corrective

Status: closing — implementation is in the working tree; the coordinator runs
the full pre-submit gate and sets the final status.

Source implementation plan:
`plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md`

Source corrective addendum (authoritative where it disagrees with the plan):
`plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`
(finding SI-3)

Source roadmap:
`plans/subsystems/security-isolation-resource-roadmap.md` (M005c now
"implemented")

Deciding ADR:
`plans/adrs/ADR-0007-resource-scoped-authorization.md` — **accepted**
(2026-10-25), **Option A: implement resource-scoped grants**. The ADR's
`## Decision` section states eleven numbered constraints; this implementation
follows them as written, and §2 below maps each of them to the code.

Reviewed repository head before implementation: `af5a217`
(`af5a21709573e20f26eca77df83bd5560c46e936`)
Implementation commit: **none** — the working tree was left dirty for the
coordinator to review and commit; no commit was made by the implementing
agent.

Branch: `corrective/interrogation-batch-2026-10`.

> A parallel worker was editing `crates/eggwork-runner/**`,
> `crates/eggwork-core/src/lib.rs`, `architecture/runner-execution.md`, and
> `architecture/core-domain.md` in the same tree during this work. Nothing in
> those paths was touched here. The verification below is therefore scoped to
> `eggwork-server` and its documentation; the workspace-wide gate is the
> coordinator's.

## 1. Executive finding

The boundary is now real, and the node no longer presents a mechanism as
available while it is inert. `GrantAuthorizer` implements the
resource-aware `authorize_request` hook that `dispatch` has always called, so
the typed `ResourceId` that every request carries is consulted instead of
being dropped on the floor.

A grant may now carry an optional, additive, bounded list of **resource
scopes**. A scope names an operation, a resource kind, and a resource
identifier, and it may only narrow an operation the same grant already
holds. The decision is a lookup in a configuration-derived set: it never asks
the store who owns the resource, so authorization keeps the in-memory,
lock-free shape it has always had and acquires no ordering dependency it did
not have before.

Two properties are load-bearing and both are proven below rather than
asserted:

- **A scope can only narrow.** It cannot grant an operation the principal does
  not hold, and it cannot widen one it does. The store's `principal_id`
  fences remain authoritative for ownership, so a scope that names another
  principal's execution, workspace, or artifact lets the request reach the
  store — and the store refuses it there. The end-to-end test shows exactly
  that: the authorizer *allows* the intruder's request (its scope names the
  resource) and the store refuses it.
- **A denial reveals nothing.** A resource-scoped denial is the same
  `403 forbidden` / "operation is not authorized" as an operation-level
  denial, and is byte-identical for a resource that exists and one that does
  not. The caller cannot tell which.

Configuration fails closed at load time. Every malformed scope — unknown
operation name, unknown resource kind, malformed identifier, a scope for an
operation the grant does not hold, a scope for a kind that operation never
names, a duplicate scope, and an over-cap grant count — is a configuration
error, and the node refuses to start rather than degrade to a permissive
decision.

**The blob boundary is not closed.** Blob storage is content-addressed and
shared, so a blob has no owner to fence it. A blob scope is an explicit
digest list an operator must configure, and a grant with no blob scope is
still operation-wide: a broad `BlobRead` still reaches any known digest. §10
states this plainly, and it is stated the same way in the threat model.

## 2. The ADR's decision constraints, and where each one lives

| ADR constraint | Implementation | Evidence |
|---|---|---|
| 1. Optional, additive scope list; a grant without scopes keeps today's meaning; the identifier is the string form of the `ResourceId` variant and is parsed back into the typed variant | `ClientGrant.resource_scopes` (`operations.rs`, `#[serde(default)]`), `ResourceScope` (`operations.rs`), `parse_resource_id` (`operations.rs`) | `a_configuration_written_before_resource_scopes_still_loads`, `a_scope_identifier_is_the_string_form_of_the_typed_resource` |
| 2. The store remains authoritative for ownership; a scope may only narrow | `GrantAuthorizer::authorize_request` consults a configuration set only; `store.rs` untouched | `a_resource_scope_cannot_reach_a_foreign_principal_resource` |
| 3. Blob has no owner; a blob scope is an explicit digest list | `ResourceKind::Blob` scopes hold typed `BlobDigest`s; no ownership comparison exists for blobs | `without_a_blob_scope_the_blob_boundary_is_the_operation_grant` |
| 4. The scope applies to the six handler-level call sites *and* to the operations `resource_for_path` resolves | `resource_for_path` unchanged and still called by `dispatch`; the six `authorize_resource` sites unchanged and still called | `a_resource_scope_cannot_reach_a_foreign_principal_resource` (dispatch path: `Observe`), `a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial` (dispatch path: `BlobRead`) |
| 5. The decision consults a configuration-derived set only; no new lock, no ordering dependency | `GrantAuthorizer` holds a `HashMap` built once at startup; the decision path takes no lock and touches no store | Code inspection of `GrantAuthorizer::authorize_request` plus §9's command output; no store call appears on the authorization path |
| 6. Configuration fails closed at load time; every error class is a configuration error; per-grant and per-node caps validated in `OperatorConfig::validate`; `deny_unknown_fields` kept; the new field is `#[serde(default)]` | `OperatorConfig::validate` scope block; `MAX_RESOURCE_SCOPES_PER_GRANT = 64`, `MAX_RESOURCE_SCOPES_TOTAL = 4096`; `principals()` parses strictly as a second gate | `a_malformed_resource_scope_is_refused_by_the_configuration`, `a_duplicate_resource_scope_is_refused_by_the_configuration`, `too_many_resource_scopes_in_one_grant_are_refused`, `too_many_resource_scopes_across_the_node_are_refused`, `the_grant_table_refuses_a_scope_it_cannot_parse` |
| 7. Authorization still precedes resource resolution; a resource-scoped denial is indistinguishable from an operation-level denial and from a missing resource; the store's fences are not weakened | `dispatch` order unchanged; the denial bodies are the same `403 forbidden` | `a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial` |
| 8. No wire change; `API_SCHEMA_VERSION` and capability names unchanged | No wire type touched; `crates/eggwork-core` untouched by this milestone | §9's `cargo check` and the full `eggwork-server` test suite, including `tests/shared_contract_ownership.rs` (`the_schema_version_is_declared_exactly_once_in_the_workspace`) and `tests/release_contract.rs` (36 tests) |
| 9. No policy language, no rule engine, no role system, no IAM | A scope is exactly `(operation, kind, identifier)`; there is no expression, no combinator, no inheritance | Code inspection: `ResourceScope` has three `String` fields and no other type carries policy |
| 10. The `deny_unknown_fields` interaction is deliberate and documented | `ClientGrant` keeps `deny_unknown_fields`; documented in `architecture/operations-cli.md`, `architecture/server-node.md`, and §8 below | `an_older_node_refuses_a_configuration_that_carries_a_resource_scope` |

## 3. The mechanism, and why

**The grant table is configuration, not a query.** `GrantAuthorizer` now holds
`HashMap<PrincipalId, PrincipalGrant>`, where a `PrincipalGrant` is the
principal's `HashSet<Operation>` plus a scope table keyed by
`(Operation, ResourceKind)` holding typed `ResourceId`s. The table is built
once, at startup, by `OperatorConfig::principals()`. The decision is a
lookup. That is the whole reason the ADR's constraint 5 is satisfiable
without a lock or an ordering dependency: nothing on the authorization path
can ask a question that needs the store.

**The scope is `(operation, kind, identifier)`, and the kind is derived from
the typed identifier.** A scope's `resource_id` is parsed through the same
constructors the request path uses — `ExecutionId::new`, `BlobDigest::parse`,
`WorkspaceId::new`, `ArtifactId::new` — so the configuration and the request
path cannot disagree about what an identifier is. The kind is then derived
from the typed variant (`ResourceId::kind`), so a scope written for `blob`
can never be compared against an execution, whatever the configuration says.

**The decision rule is the ADR's rule, in the ADR's order.** Operation grant
present → resource present → scope configured for that `(operation, kind)` →
resource must be inside it → otherwise the operation-level grant stands. The
fallbacks are what make the change safe: a request that carries no resource
(the `dispatch`-level path for `Execute` and `WorkspaceCreate`, where the
resource is in the body) keeps the operation-level answer, and an
`(operation, kind)` pair with no scope keeps the operation-level answer.

**The granularity is per `(operation, kind)`, and that is stated, not hidden.**
A scope for `Execute`/`Execution` does not narrow `Execute`/`Workspace`; a
scope for `BlobRead`/`Blob` does not narrow `BlobWrite`/`Blob`. This is the
ADR's rule 3 read literally. It is a real sharp edge for an operator — see
§11 finding 1 — so it is stated in the threat model, in both deep dives, and
in the `Review focus` section of `architecture/server-node.md`.

**A scope for a kind the operation never names is a configuration error.**
`Operation::resource_kinds()` is the table of the kinds each operation's
requests can actually name, and it is exactly the set the request path
resolves. A scope outside that table could never be consulted, so accepting
it would mean accepting a narrowing the operator believes in and that does
not happen. Refusing it at load time is the same class of decision as
refusing an unknown operation name.

## 4. Requirement-to-evidence matrix

Acceptance criteria are numbered as in the implementation plan's §12.

| # | Acceptance criterion | Evidence (command output or test) |
|---|---|---|
| 1 | The decided branch is recorded in ADR-0007 as accepted | `plans/adrs/ADR-0007-resource-scoped-authorization.md` § *Decision* — read, not modified by this milestone (the coordinator's commit `af5a217` already accepted it) |
| 2 | `architecture/overview.md` divergence #4 reflects the decided disposition | **Coordinator-owned.** The exact proposed row text is in §14 below; this milestone did not edit `architecture/overview.md` |
| 3 | The threat model states the boundary as it is after the change | `plans/security/threat-model.md` — invariant 5 and the "Authenticated unauthorized principal" row's remaining-boundary column, both rewritten in this change |
| 4 | The node does not present an inert mechanism | `a_resource_scope_narrows_the_operation_grant_it_names` — the test fails if the scope is ignored: it asserts the operation grant alone would allow a digest the scope then refuses |
| 5 | Authorization still precedes resource resolution, and an unauthorized caller cannot distinguish an existing resource from an absent one | `a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial` — the three denials (out-of-scope existing, out-of-scope absent, operation-level) are asserted equal on `(status, code, message)`; the in-scope read is asserted to succeed, which shows the difference is the scope |
| 6 | The store's owner fences are not weakened, and the existing authorization matrix is green | `crates/eggwork-server/src/store.rs` is **unmodified** (see §7); the full `eggwork-server` suite is green, including `mtls_authorization_and_fixed_target_execution` and `resource_aware_authorizer_can_scope_blob_capability`, unchanged in their assertions |
| 7 | The configuration loader still fails closed on a malformed or unknown field | `an_unknown_field_in_a_resource_scope_is_refused`; the pre-existing `invalid_configuration_is_rejected_without_exposing_secret_material` remains green |
| 8 | No wire protocol change | No wire type touched. `crates/eggwork-core/src/lib.rs` is **not modified by this milestone**; `API_SCHEMA_VERSION` is untouched; `tests/shared_contract_ownership.rs` (6 tests) and `tests/release_contract.rs` (36 tests) are green |
| 9 | The full pre-submit gate is green on the reviewed head | **Not run by this agent** — the coordinator runs the full gate. The scoped verification in §9 was run and is green |
| 10 | A resource scope measurably narrows an operation grant, proven by a test that fails if the scope is ignored | `a_resource_scope_narrows_the_operation_grant_it_names` — asserts `authorize(principal, BlobRead) == true` while `authorize_request` for the unnamed digest is `false`; if the scope were ignored this assertion would fail |
| 11 | The owner fence holds for `Execution`, `Workspace`, and `Artifact`; blobs are governed by an explicit digest list | `a_resource_scope_cannot_reach_a_foreign_principal_resource` — for each of the three kinds the intruder's scope names the owner's resource, the authorizer *allows* the request (asserted), and the store refuses it (404 not found / 409 conflict / empty list) |
| 12 | A principal with no resource scopes receives exactly the operation-level decision | `a_grant_without_resource_scopes_keeps_its_operation_level_meaning` |
| 13 | Every configuration error class is refused at load time | `a_malformed_resource_scope_is_refused_by_the_configuration` (six classes), `a_duplicate_resource_scope_is_refused_by_the_configuration`, `too_many_resource_scopes_in_one_grant_are_refused`, `too_many_resource_scopes_across_the_node_are_refused` |

Plan §9's required tests map onto the same evidence:

| Plan §9 item | Test | Where |
|---|---|---|
| 1 (common) — the divergence table matches the code | `two_principals_with_the_same_operations_and_different_scopes_get_different_answers` | `tests/resource_scoped_authorization.rs` |
| 2 (common) — no regression in the existing boundary | the whole existing `eggwork-server` suite, unchanged assertions | §9 |
| 3 (common) — authorization still precedes resource resolution | `a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial` | `lib.rs` |
| 4 — a resource scope actually narrows | `a_resource_scope_narrows_the_operation_grant_it_names` | `tests/resource_scoped_authorization.rs` |
| 5 — the owner fence holds | `a_resource_scope_cannot_reach_a_foreign_principal_resource` | `lib.rs` |
| 6 — blobs have no owner fence, and the scope says so | `without_a_blob_scope_the_blob_boundary_is_the_operation_grant` | `tests/resource_scoped_authorization.rs` |
| 7 — operation grants are never widened by a scope | `a_scope_never_grants_an_operation_the_principal_does_not_hold` | `tests/resource_scoped_authorization.rs` |
| 8 — no scope configured means today's behaviour | `a_grant_without_resource_scopes_keeps_its_operation_level_meaning` | `tests/resource_scoped_authorization.rs` |
| 9 — configuration failures are refused | five tests, see matrix row 13 | `operations.rs` |
| 10 — redaction | `the_printed_configuration_redacts_resource_scope_identifiers` | `operations.rs` |

## 5. The new tests, and what each one proves

### `crates/eggwork-server/tests/resource_scoped_authorization.rs` (new, 13 tests)

Portable by construction: it drives the grant table through the public API
only, so it needs no filesystem, no process, no TLS handshake, and no host
platform. It compiles and runs on the Windows job.

| Test | Proves |
|---|---|
| `a_grant_without_resource_scopes_keeps_its_operation_level_meaning` | A grant with no scopes answers exactly as before: the operation grant decides, and an ungranted operation is still refused |
| `two_principals_with_the_same_operations_and_different_scopes_get_different_answers` | The scope is reached and consulted: two principals with identical operation grants and different scopes get different answers for the same digest (plan §9 item 1) |
| `a_resource_scope_narrows_the_operation_grant_it_names` | **Non-vacuous.** The operation grant alone allows both digests; the scope refuses the digest it does not name and permits the one it does. If the scope were ignored, the `!authorize_request(...)` assertion fails |
| `a_scope_never_grants_an_operation_the_principal_does_not_hold` | A scope for an operation the grant does not hold grants nothing — *including* when the grant list is misconfigured that way, which is the case `validate` refuses |
| `a_scope_narrows_only_the_operation_and_kind_it_names` | The rule is per `(operation, kind)`: a `blob-read` scope does not narrow `blob-write` |
| `a_request_without_a_resource_falls_back_to_the_operation_grant` | A request with no resource keeps the operation-level answer |
| `an_execute_scope_names_the_execution_and_the_workspace_separately` | `execute` names two kinds; scoping one does not scope the other |
| `a_scope_identifier_is_the_string_form_of_the_typed_resource` | The configuration and the request path agree on what an identifier is: the scope's string is the same typed value the request path resolves |
| `the_authorizer_decides_the_scope_and_leaves_ownership_to_the_store` | The authorizer cannot know ownership and does not pretend to; it lets the request through so the store's fence can refuse it |
| `without_a_blob_scope_the_blob_boundary_is_the_operation_grant` | The residual boundary, stated as a test rather than in prose |
| `a_configuration_written_before_resource_scopes_still_loads` | An older configuration file still loads (`#[serde(default)]`) |
| `an_unknown_field_in_a_resource_scope_is_refused` | `deny_unknown_fields` reaches into the scope |
| `an_older_node_refuses_a_configuration_that_carries_a_resource_scope` | The deliberate older-node refusal (see §8 for what this does and does not prove) |

### `crates/eggwork-server/src/operations.rs` (in-file, 8 new tests)

| Test | Proves |
|---|---|
| `the_printed_configuration_redacts_resource_scope_identifiers` | `config print` prints neither the scope identifier, nor the private key path, nor the certificate fingerprint — while keeping the scope's operation and kind readable |
| `a_resource_scope_that_narrows_a_held_operation_is_accepted` | The valid shape is accepted, so the refusals below are not blanket refusals |
| `a_malformed_resource_scope_is_refused_by_the_configuration` | Six error classes, each refused: unknown operation name, unknown resource kind, malformed identifier, scope for an unheld operation, scope for a kind the operation never names, scope for an operation that names no resource |
| `a_duplicate_resource_scope_is_refused_by_the_configuration` | The same scope twice is a duplicate, not a tolerated redundancy |
| `too_many_resource_scopes_in_one_grant_are_refused` | The per-grant cap (64) is enforced |
| `too_many_resource_scopes_across_the_node_are_refused` | The per-node cap (4096) is enforced across 65 grants of 64 scopes each — every grant individually within its own cap |
| `a_configuration_written_before_resource_scopes_still_validates` | An older configuration file still passes `validate()` |
| `the_grant_table_refuses_a_scope_it_cannot_parse` | `principals()` — the gate the daemon actually calls — is a second, independent fail-closed gate |

### `crates/eggwork-server/src/lib.rs` (in-file, 2 new end-to-end tests)

Both start a real node over mTLS and drive it with the production client.

| Test | Proves |
|---|---|
| `a_resource_scope_cannot_reach_a_foreign_principal_resource` | The owner fence holds for all three owned kinds. The intruder's scopes name the owner's execution, workspace, and artifact; the test asserts the **authorizer allows** each request (so the refusal is the store's, not the authorizer's), and that the store refuses them: 404 not found for the execution, 409 conflict for the workspace, an empty artifact list and 404 for the artifact. The owner's own reads succeed, so the difference is ownership and not a broken path |
| `a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial` | The three denials are equal on `(status, code, message)`, and the in-scope digest is readable — so the difference is the scope and nothing else is disclosed |

## 6. Verification actually run

All commands used a private target directory so they did not fight the
parallel worker for the cargo lock:

```
export CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w1c
```

| Command | Result |
|---|---|
| `cargo fmt -p eggwork-server` | Applied (formatting only to the files this milestone owns) |
| `cargo fmt -p eggwork-server -- --check` | **Clean** — no output |
| `cargo clippy --locked -p eggwork-server --all-targets --all-features -- -D warnings` | **Clean** — `Finished dev profile`, no warnings. One `clippy::cloned_ref_to_slice_refs` finding in the new test was fixed (`std::slice::from_ref`) before the final run |
| `cargo test --locked -p eggwork-server --all-targets` | **243 passed, 0 failed** — 166 in `src/lib.rs`, 13 in the new `tests/resource_scoped_authorization.rs`, 7 in `artifact_capture_disposition`, 3 in `gc_preview_parity`, 12 in `host_telemetry`, 36 in `release_contract`, 6 in `shared_contract_ownership`, 0 in `installed_qualification` (all ignored), 0 in the `eggworkd` binary target |
| `cargo check --locked -p eggwork-server` | **Clean** — `Finished dev profile` |
| `python3 scripts/check_execution_ownership.py` | **Passed** — "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "negative scanner self-test passed; crate dependency direction passed" |
| `cargo run -p eggwork-server --bin eggworkd -- config print --config examples/eggworkd.example.json` | **Succeeded** — the printed document shows `"resource_id": "[REDACTED]"` for every scope, `"certificate_sha256": "[REDACTED]"`, and `"private_key": "[REDACTED]"` |
| `git diff --check` | **Clean** — no whitespace errors |

**Not run by this agent** (the coordinator's full gate; the plan's §10 list):

- `cargo fmt --all -- --check` — deliberately not run: `--all` would reformat
  the parallel worker's in-flight files. `cargo fmt -p eggwork-server
  -- --check` was run instead and is clean.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-targets`
- `cargo check --workspace`
- `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`

The workspace-wide runs were not performed because the parallel worker had
`crates/eggwork-core` and `crates/eggwork-runner` in a temporarily
non-compiling state during part of this work; a workspace run at that moment
would have measured their in-flight edits, not this milestone.

## 7. Compatibility review

### Configuration compatibility

- **An older configuration file still loads.** `ClientGrant.resource_scopes`
  carries `#[serde(default)]`, so a document without the key deserializes
  with an empty scope list. Proven by
  `a_configuration_written_before_resource_scopes_still_loads` and
  `a_configuration_written_before_resource_scopes_still_validates`.
- **An older node refuses a newer configuration that carries a scope.** This
  is the deliberate `deny_unknown_fields` interaction the ADR names. The
  refusal is correct: a node whose boundary is wider than its configuration
  states is the defect this milestone exists to remove. The proof is
  `an_older_node_refuses_a_configuration_that_carries_a_resource_scope`, and
  it is a **simulation**, not an older binary: it deserializes the same
  document with a local struct that has exactly the older `ClientGrant`'s
  three fields and the same `#[serde(deny_unknown_fields)]` attribute, and
  asserts that deserialization fails. No older `eggworkd` binary was built
  or run.
- **A newer node accepts the newer configuration** — asserted in the same
  test.
- **A typo inside a scope is refused**, not ignored
  (`an_unknown_field_in_a_resource_scope_is_refused`), because `ResourceScope`
  is itself `deny_unknown_fields`.

### Public-API position

Honest statement of what changed in the crate's public surface:

| Item | Before | After | Breaking? |
|---|---|---|---|
| `GrantAuthorizer` | `pub struct GrantAuthorizer(HashMap<PrincipalId, HashSet<Operation>>)` — a **private** tuple field | `pub struct GrantAuthorizer { grants: HashMap<PrincipalId, PrincipalGrant> }` — a **private** named field | **Not breaking.** The tuple field was private before and the named field is private now, so no external caller could construct or destructure `GrantAuthorizer` either way. `GrantAuthorizer` has no public constructor, so an out-of-tree caller could only ever obtain one from `OperatorConfig::principals()`, whose signature did not change. The *shape* of the private field did change; its visibility did not |
| `ClientGrant` | `principal_id`, `certificate_sha256`, `operations` | plus `resource_scopes: Vec<ResourceScope>` (`#[serde(default)]`) | **Source-breaking for an out-of-tree struct-literal construction.** `ClientGrant` has public fields, so `ClientGrant { .. }` must now also set `resource_scopes`. This is a compile error, not a silent behaviour change. In-tree there is exactly one such construction, in `operations.rs`'s test helper, updated |
| `ResourceScope` | did not exist | new public struct with three public `String` fields | Additive |
| `ResourceKind` | did not exist | new public enum (`Execution`, `Blob`, `Workspace`, `Artifact`) | Additive |
| `ResourceId` | `Debug, Clone, PartialEq, Eq` | plus `Hash`, plus a `kind()` method | Additive |
| `Operation` | `Debug, Clone, Copy, PartialEq, Eq, Hash` | plus a `resource_kinds()` method | Additive |
| `Authorizer::authorize_request` | defaulted trait method | unchanged; `GrantAuthorizer` now overrides it | Additive behaviour for grant-based deployments that configure scopes; unchanged for those that do not |
| `OperatorConfig::principals`, `OperatorConfig::validate`, `OperatorConfig::redacted_json` | — | signatures unchanged | Not breaking |

**In-tree consumer enumeration.** `GrantAuthorizer` has exactly two in-tree
production consumers: `operations.rs::principals`, which builds it, and
`operations.rs::start_server`, which passes it to `NodeServer::start` as
`Arc<dyn Authorizer>`. A repository-wide grep for `GrantAuthorizer` finds no
other production use; the only other reference is the new in-file test helper
in `src/lib.rs` that builds a grant table for the end-to-end test.

**Out-of-tree position, stated honestly.** `eggwork-server` is not published
to a registry as a library in this repository's release process — the release
artifacts are the `eggworkd` binary and the release channel, not a
consumable crate API. So there is no downstream crate to break in-tree. An
out-of-tree consumer that constructs `ClientGrant` with a struct literal must
add the field; one that only *reads* a `ClientGrant` is unaffected; and one
that constructs a `GrantAuthorizer` directly was already unable to, because
the field was private.

**Release-visibility consequence.** No wire shape, capability name, or
`API_SCHEMA_VERSION` changed, so this is not a protocol-visible release and
needs no control-plane version coordination. The *configuration* shape grew
an optional field: a node running this release accepts both the old and the
new configuration shape, and a node running an older release refuses the new
shape (§8). An operator rolling this out therefore configures the scope
*after* the node is upgraded, not before.

## 8. Negative and adversarial evidence

**A scope refuses a resource outside the scope.** `a_resource_scope_narrows_the_operation_grant_it_names`
asserts the operation grant alone would allow both digests and that the scope
refuses the unnamed one. The end-to-end counterpart
`a_resource_scoped_denial_is_indistinguishable_from_an_operation_level_denial`
drives it over mTLS: the scoped principal reads its in-scope digest
successfully and is refused the out-of-scope one with `403 forbidden`.

**A foreign-owned resource remains refused.**
`a_resource_scope_cannot_reach_a_foreign_principal_resource` is the sharpest
evidence in this milestone, and it is deliberately two-sided:

- the intruder's grant carries scopes naming the *owner's* execution,
  workspace, and artifact;
- the test asserts the **authorizer allows** each of those requests —
  `authorize_request` returns `true` for the intruder — because the scope
  names the resource and the authorizer cannot know who owns it;
- the store then refuses: `404` for the execution, `409 workspace_identity_conflict`
  for the workspace, an empty artifact list plus `404 artifact_not_found` for
  the artifact;
- the owner's own reads succeed, so the difference is ownership and not a
  broken path.

**A misconfigured grant does not widen anything.** `a_scope_never_grants_an_operation_the_principal_does_not_hold`
builds a grant that names `blob-read` in its scope while its operation list
holds only `status` — the shape `validate` refuses — and asserts the
authorizer still refuses the blob read.

**A denial discloses nothing.** The three denial shapes are asserted equal.

**Configuration failures are refused, not tolerated.**
`a_malformed_resource_scope_is_refused_by_the_configuration` covers six
classes; the duplicate and both caps have their own tests; and
`the_grant_table_refuses_a_scope_it_cannot_parse` shows the second gate.

## 9. Documentation status

| Document | Status | Note |
|---|---|---|
| `architecture/overview.md` divergence #4 | **Coordinator-owned; not edited here.** The proposed replacement row is in §14 below | The ADR names this the single most important documentation consequence, and the file is explicitly outside this worker's ownership |
| `plans/security/threat-model.md` | Updated in this change | Invariant 5 and the "Authenticated unauthorized principal" row's remaining-boundary column. The blob boundary is **not** described as closed |
| `architecture/server-node.md` | Updated in this change | The `GrantAuthorizer` paragraph, a new "Resource-scoped grants" section with the decision rule, "The store stays authoritative for ownership", "Configuration fails closed at load time", and the `deny_unknown_fields` note; the residual-disclosure paragraph in "Authorization runs BEFORE resource resolution"; three new invariant rows; two new review-focus bullets; two new test-coverage bullets |
| `architecture/operations-cli.md` | Updated in this change | The grant configuration surface: the scope field, the error-class table, the caps, the redaction change, the rewritten "Authorization model" section, the compatibility note, three new invariant rows, and the test-coverage and honest-gaps paragraphs |
| `examples/eggworkd.example.json` | Updated in this change | A second client demonstrates the shape. The first client is unchanged in meaning, so the example still shows a broad controller |
| `plans/subsystems/security-isolation-resource-roadmap.md` | Updated in this change | The status line, the M005c entry's status, the deciding-ADR link, the closure-evidence pointer, and a "Decided disposition" paragraph |
| `plans/implementation/.../005c-...md` | Updated in this change | The `Status:` line is now `active`; the "blocked on ADR-0007" text in the header and in §15 is replaced with the accepted decision. The coordinator sets the final status |
| `plans/closure/security-isolation-resource/005c-status.md` | This file | — |
| `plans/registry.md` | **Coordinator-owned; not edited here** | §14 below gives the exact proposed edits |
| `AGENTS.md` | Not edited | Plan §11: the hard-invariant list needs no new entry. The existing "Node trust requires a verified mTLS client CA and explicit per-principal grants" line remains true — a resource scope narrows such a grant, it does not replace it. The proposed optional wording is in §14 |
| `docs/**` | Not edited | `docs/` is user- and operator-facing; the configuration surface this milestone adds is an operator concern that the deep dives and the example carry. The coordinator may want a `docs/` line for the new field — see §11 finding 4 |

## 10. The residual boundary that remains

Stated plainly, because the point of this milestone was honesty about
boundaries:

1. **The blob boundary is not closed.** Blob storage is content-addressed and
   shared: a blob has no owner, so there is no `principal_id` fence to hold.
   The only boundary available for a blob is an operator-configured digest
   list. A grant with **no** blob scope is still operation-wide, so a broad
   `BlobRead` still reaches any known digest, and `POST /v1/blobs/missing`
   still lets that principal probe digest presence. Scoping is opt-in and
   the operator has to configure it.
2. **A scope is per `(operation, kind)`.** Scoping `Execute`/`Execution` does
   not narrow `Execute`/`Workspace`. An operator who wants `execute`
   constrained must write a scope for each kind that operation names.
3. **A scope is a static list.** It cannot express "every execution this
   principal creates from now on", "every digest under a prefix", or "every
   resource another principal shares". It is a list of identifiers.
4. **Artifact identifiers are assigned at capture time.** They are random
   UUIDs, so an artifact scope can only name an artifact the operator already
   knows the identifier of. "Learn the identifier, then write the scope" is
   the workflow; there is no scope-by-creation rule.
5. **Ownership remains the store's job.** The authorizer narrows; the store
   fences. Nothing in this milestone moved ownership into configuration.
6. **The scope does not reach the identity model.** A scope is a per-principal
   grant narrowing. It is not a role, not an IAM subject, and it does not
   change the mTLS identity model, the lease rules, or the generation
   fencing.

## 11. Unresolved findings, ranked by severity

1. **Medium — the `(operation, kind)` granularity is a sharp edge.** An
   operator who writes one scope for `execute`/`execution` may believe they
   constrained `execute` generally; the `execute`/`workspace` half is
   unconstrained. This follows the ADR's rule 3 read literally, and it is
   documented in the threat model, in both deep dives, and in
   `architecture/server-node.md`'s `Review focus`. **Not fixed here**: changing
   the granularity would contradict the accepted ADR's decision rule. If the
   coordinator considers the sharp edge unacceptable, it is a follow-up ADR,
   not a code change.
2. **Medium — the blob boundary remains open for unscoped grants.** Inherent
   to content-addressed shared storage; §10 states it. Closing it would mean
   a different storage model, not a different policy.
3. **Low — `operation(name)` still uses `filter_map` for the grant's
   *operation list*,** so an unrecognized operation name in a grant is
   silently dropped by `principals()` rather than refused there. `validate()`
   refuses it, and the daemon calls `validate()` before `start_server()`, so
   the deployed path is fail-closed; but `principals()` on its own would drop
   it. This is **pre-existing** and was deliberately not changed here — it is
   the behaviour the existing deep dives describe, and the milestone's scope
   is the resource-scope surface. The new scope list does **not** share this
   weakness: `principals()` parses scopes strictly and refuses.
4. **Low — the user-facing documentation does not mention the new field.**
   `docs/` is outside this milestone's ownership and the plan's §11 does not
   list it. `architecture/operations-cli.md` and `examples/eggworkd.example.json`
   carry it for operators who read the deep dives; a `docs/operations.md` or
   `docs/quickstart.md` line would be better. Proposed for the coordinator.
5. **Informational — `architecture/server-node.md` says `Operation` has "11
   variants"** while the enum has twelve (the table omits `Telemetry`). That
   inaccuracy is pre-existing and unrelated to this milestone; it was left
   alone rather than folded into this change.

## 12. Platform matrix actually exercised

The authorization decision is host-independent: the grant table is built in
memory at startup and the decision is a lookup. The tests that exercise it
are therefore portable, and the new integration target
`tests/resource_scoped_authorization.rs` is written to compile and run on the
Windows job (no `/bin/sh`, no `std::os::unix`, no Unix-only modes).

The two end-to-end tests live in `src/lib.rs`'s in-file test module, which is
Linux-shaped by construction — the server's own test harness starts
`/bin/sh` and uses Unix-only file modes, so it does not compile for a Windows
host. Those two tests were executed on **this Linux host only**.

**No platform is claimed to be qualified by this milestone.** The change
alters no platform-specific code path, no capability advertisement, and no
wire shape, so no release-matrix exercise is required for the decision logic
itself — the plan says so explicitly. What has *not* been done is any hosted
run on macOS or Windows; the pre-existing limitation of the server's own
test harness still stands.

## 13. Registry and roadmap disposition

- **Roadmap** (`plans/subsystems/security-isolation-resource-roadmap.md`):
  updated in this change — the status line, the M005c entry's status, the
  deciding-ADR link, the closure-evidence pointer, and a "Decided
  disposition" paragraph.
- **Registry** (`plans/registry.md`): **coordinator-owned; not edited here.**
  The exact proposed edits are in §14.
- **Plan** (`plans/implementation/.../005c-...md`): `Status:` is `active`;
  the coordinator sets the final status after the gate.

## 14. Coordinator-owned text

### `architecture/overview.md` — divergence #4

Replace the current row:

```markdown
| 4 | `GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) |
```

with:

```markdown
| 4 | ~~`GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison.~~ **Closed 2026-10-25 by [ADR-0007](../plans/adrs/ADR-0007-resource-scoped-authorization.md) (Option A).** `GrantAuthorizer` now implements `authorize_request`: a grant may carry a bounded, optional list of resource scopes naming an operation, a resource kind, and a typed identifier, and the decision narrows the operation grant against that configuration-derived set. The scope is per `(operation, kind)`, so a scope for `Execute`/`Execution` does not narrow `Execute`/`Workspace`. The store's `principal_id` fences remain authoritative for ownership, so a scope naming another principal's execution, workspace, or artifact is still refused there. **Residual boundary:** a grant with no scope is still operation-wide, so a broad `BlobRead` still reaches any known digest — blob storage is content-addressed and shared, so a blob has no owner to fence it, and a blob scope is an explicit digest list the operator must configure. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) · [threat-model.md](../plans/security/threat-model.md) |
```

### `plans/registry.md`

**This milestone did not edit `plans/registry.md`.** A *concurrent* uncommitted
edit in the shared working tree — not this milestone's — already updated three
of the five places that mention M005c (the addendum row at line 95, the
security row at line 114, and the batch paragraph at line 122) to say that
M005c is unblocked by the accepted ADR-0007. The coordinator should reconcile
that edit rather than apply the proposals below over it.

Two places still read "blocked" and need the change:

- **Line 183** (the milestone table row):

  ```markdown
  | Security M005c resource-scoped authorization corrective | **blocked** | `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md`. Blocked on ADR-0007 (`plans/adrs/ADR-0007-resource-scoped-authorization.md`, **proposed, not accepted**), which must decide whether resource-scoped grants are implemented or the inert hook and its `ResourceId` plumbing are removed. The gap itself is already recorded as divergence #4; this is a decision about a known gap, not a new defect. |
  ```

  becomes (with `Status:` set to the final status from
  `plans/003-planning-process.md` §2 after the gate):

  ```markdown
  | Security M005c resource-scoped authorization corrective | **active** | `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md`. ADR-0007 is **accepted** — Option A: implement resource-scoped grants. The hook is real: `GrantAuthorizer` implements `authorize_request` and a grant may carry a bounded, optional resource-scope list. The store's `principal_id` fences remain authoritative for ownership, and blobs are governed by an explicit digest list. No wire value, wire field, capability byte, or `API_SCHEMA_VERSION` changes. Closure evidence: `plans/closure/security-isolation-resource/005c-status.md`. |
  ```

- **Line 230** (the dependency diagram):

  ```markdown
                                                        +--> Security M005c resource-scoped authorization [BLOCKED on ADR-0007]
  ```

  becomes:

  ```markdown
                                                        +--> Security M005c resource-scoped authorization [ADR-0007 ACCEPTED — IMPLEMENTING]
  ```

- **Line 183's status cell and the roadmap's M005c status** should agree. The
  roadmap entry in this change says "implemented"; the coordinator may prefer
  "closing" until the gate is green. `plans/003-planning-process.md` §2's
  vocabulary is the authority.

### `AGENTS.md`

Plan §11 says the hard-invariant list needs no new entry, and it does not need
one: the change adds a *narrowing* to an existing per-principal grant, and it
adds no new process owner, dependency edge, wire shape, or release-identity
rule.

The only candidate is the Security section's line:

> Node trust requires a verified mTLS client CA and explicit per-principal grants;

which remains true. If the coordinator wants the narrowing recorded there,
the wording that keeps it true and in the same style is:

> Node trust requires a verified mTLS client CA and explicit per-principal grants; a grant may narrow an operation to named resources, and the store's ownership fences remain authoritative.

### `plans/subsystems/operations-distribution-roadmap.md`

No change is required. This milestone touches no operations, distribution, or
release surface: no CLI verb, no configuration file placement, no release
artifact, and no wire value changed. `examples/eggworkd.example.json` grew an
optional field and a second client entry, which is a configuration example,
not a distribution change.

## 15. What this milestone did not do

- It did not change the store's ownership model, the artifact store's
  retention, blob content-addressing, or the workspace derivation rules.
  `crates/eggwork-server/src/store.rs` is **unmodified**.
- It did not change the execution-ownership boundary, the mTLS identity
  model, the lease rules, or the generation fencing.
- It did not change `API_SCHEMA_VERSION`, any capability name, or any wire
  shape, and it did not touch `crates/eggwork-core/**`,
  `crates/eggwork-runner/**`, `crates/eggwork-client/**`, or
  `crates/eggwork-sandbox-helper/**`.
- It did not add a policy language, a rule engine, a role system, or an IAM.
  A scope is `(operation, kind, identifier)` and nothing else.
- It did not close the blob boundary. §10 states what remains.
- It did not edit `architecture/overview.md`, `plans/registry.md`, `AGENTS.md`,
  `docs/**`, `scripts/**`, or any other plan or closure file. The exact
  proposed text for the coordinator-owned files is in §14.
- It did not commit, branch, or stage anything.
