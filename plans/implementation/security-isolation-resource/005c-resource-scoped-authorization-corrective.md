# Security M005c — Resource-scoped authorization disposition corrective

Status: blocked

Concrete blocker: `plans/adrs/ADR-0007-resource-scoped-authorization.md` — **proposed, not yet accepted**. The ADR must decide whether resource-scoped grants are implemented or the inert hook and its `ResourceId` plumbing are removed. Per `plans/003-planning-process.md` §4, this plan MUST NOT be handed off as ready until that ADR is accepted, and it MUST NOT be handed off as ready merely because adjacent implementation exists.

Closure record: `plans/closure/security-isolation-resource/005c-status.md`

Class: invariant

Source authority:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md` — finding SI-3
- `plans/adrs/ADR-0007-resource-scoped-authorization.md` — the decision this milestone is blocked on (**proposed**)
- `architecture/overview.md` — divergence #4, which already records this gap
- `plans/security/threat-model.md` — the "Authenticated unauthorized principal" row, which already states this boundary
- `plans/subsystems/security-isolation-resource-roadmap.md` — the subsystem's durable invariants
- `plans/adrs/README.md` — the ADR rules this milestone must obey
- `plans/003-planning-process.md#dependency-discipline` and `#architecture-changes`

## 1. Objective

Resolve the already-recorded divergence #4 to a **decided disposition**, so that the node stops presenting a mechanism as available when it is inert.

This milestone is a decision about an existing gap, not a new defect report. The gap is recorded in `architecture/overview.md`'s divergences table and stated as an open boundary in `plans/security/threat-model.md`. The objective is to close it in one of the two coherent ways the ADR weighs, and to leave the code, the public API, the design record, and the threat model all saying the same thing.

## 2. Baseline / problem statement

At the reviewed baseline:

- `crates/eggwork-server/src/lib.rs` defines `pub enum ResourceId { Execution(ExecutionId), Blob(BlobDigest), Workspace(WorkspaceId), Artifact(ArtifactId) }` — a typed resource identifier.
- The same file defines `pub struct OperationRequest { pub operation: Operation, pub resource: Option<ResourceId> }`.
- The same file defines `fn resource_for_path(operation: Operation, path: &str) -> Option<ResourceId>`, mapping a request path segment to a typed resource for `Observe`, `Cancel`, `Renew`, `Events`, `BlobRead`, `BlobWrite`, and `ArtifactRead`. `ArtifactRead` distinguishes the artifact route from the execution route by path shape.
- The same file defines the public trait `Authorizer`, whose `fn authorize_request(&self, principal: &NodePrincipal, request: &OperationRequest) -> bool` has a default implementation that delegates to `fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool`.
- `crates/eggwork-server/src/operations.rs` defines `pub struct GrantAuthorizer(HashMap<PrincipalId, HashSet<Operation>>)` and implements **only** `authorize`. The trait's default `authorize_request` therefore applies on the production path, and the typed `ResourceId` is **never consulted**.
- `fn authorize_resource(state, principal, operation, resource)` in `crates/eggwork-server/src/lib.rs` is the resource-scoped call helper. It is invoked at six handler sites:
  1. the blob-find-missing handler — `Operation::BlobRead`, `ResourceId::Blob(digest)`;
  2. the blob-prepare handler — `Operation::BlobWrite`, `ResourceId::Blob(digest)`;
  3. the workspace-create handler — `Operation::WorkspaceCreate`, `ResourceId::Workspace(workspace_id)`;
  4. the workspace-derive handler — `Operation::WorkspaceCreate`, `ResourceId::Workspace(workspace_id)`;
  5. the execute handler — `Operation::Execute`, `ResourceId::Execution(execution_id)`;
  6. the execute handler, conditionally on the request naming one — `Operation::Execute`, `ResourceId::Workspace(workspace_id)`.
- The `dispatch` function in the same file additionally calls `authorize_request` with a `resource_for_path`-derived resource for every routed request.
- Every one of those call sites resolves to the same operation-level decision. The typed resource is constructed, passed, and discarded.
- **The owner fence that does exist is a store fence, not an authorizer fence.** `ExecutionStore::load_snapshot_for_principal` in `crates/eggwork-server/src/store.rs` selects by `(execution_id, principal_id)`. The artifact store's `load` selects by `(artifact_id, principal_id)` and its list selects by `(execution_id, generation, principal_id)`. The workspace store's `lookup_manifest` selects by `(principal_id, manifest_digest)`, and `WorkspaceStore::materialize` rejects a conflicting materialization whose `principal_id` differs. **Blob storage has no owner fence at all** — it is content-addressed and shared, which is why the threat model's remaining-boundary column reads: *"Broad BlobRead capability permits any known digest unless resource-scoped policy is configured."*
- The only exerciser of the resource-scoped path is a unit test in `crates/eggwork-server/src/lib.rs`'s test module, which asserts that `resource_for_path` maps a path segment to a `ResourceId`. No production authorization decision depends on the answer.
- The configuration surface is `ClientGrant` in `crates/eggwork-server/src/operations.rs` — `{ principal_id, certificate_sha256, operations: Vec<String> }`, with `#[serde(deny_unknown_fields)]`, held in `OperatorConfig`'s `clients` list and validated in `OperatorConfig::validate`, which rejects an unknown operation name, an empty operations list, a malformed principal id, and a duplicate certificate fingerprint. `GrantAuthorizer` is built from that configuration in `OperatorConfig::principals`, which returns the `FingerprintPrincipalResolver` and the `GrantAuthorizer` pair.

## 3. Scope

In scope:

- the disposition of the `Authorizer::authorize_request` hook and its `ResourceId` plumbing;
- the grant configuration shape, if the ADR selects implementation;
- the authorizer's decision logic, if the ADR selects implementation;
- the six resource-scoped call sites and the `dispatch`-level `authorize_request` call, under whichever disposition is decided;
- `architecture/overview.md`'s divergence #4 row, which must end up reflecting the decided disposition;
- `plans/security/threat-model.md`'s remaining-boundary column for the "Authenticated unauthorized principal" row;
- the corresponding deep dives — `architecture/server-node.md` and `architecture/operations-cli.md`, both of which are cited by divergence #4.

Out of scope:

- any change to the wire representation of a request or response type, unless the decided disposition requires one and the ADR says so;
- any change to the store's ownership model, the artifact store's retention, the blob store's content-addressing, or the workspace store's derivation rules;
- the execution-ownership boundary, the mTLS identity model, and the lease and fencing rules;
- M005a (degraded-isolation observability) and M005b (sandbox-helper environment parity), which are independent and may run in parallel;
- the control-plane shared-contract corrective, except that this milestone must not change `API_SCHEMA_VERSION` on its own authority;
- any change to `eggwork-client`'s request shapes unless the decided disposition requires one.

## 4. Invariants and non-goals

Invariants that hold in **both** branches:

1. **The node does not present a mechanism as available when it is inert.** If the hook is kept, it decides. If it is removed, the public API, the documentation, and the operator surface no longer imply that resource-scoped authorization exists.
2. **The `architecture/overview.md` divergences table is updated to the decided disposition.** Divergence #4 is either closed with a statement of what now enforces the boundary, or rewritten to describe the mechanism that is actually in force. It is not left describing an inert hook as an open gap once the gap has been decided.
3. **Authorization still strictly precedes resource resolution** in `dispatch`, and a denied caller still cannot distinguish an existing resource from an absent one.
4. **No new information is disclosed to an unauthorized caller.** A resource-scoped denial must be indistinguishable from an operation-level denial, and from a missing resource, in both status code and response body.
5. **The store's existing owner fences are not weakened.** The `principal_id` fences on executions, artifacts, and workspaces remain, and remain the backstop behind whatever the authorizer decides.
6. **The configuration loader still fails closed.** A malformed or unknown field in a grant remains a configuration error, and the node refuses to start.

Non-goals:

- inventing a general policy language, a rule engine, or a role system;
- adding a scheduler, a queue, or a placement concept;
- changing who may hold which *operation* — this milestone is about the resource scope within an operation, not about the operation grant itself;
- auditing the many other authorization boundaries the repository already records.

## 5. Required production changes — Branch A: implement resource-scoped grants

This branch is the work required **if** ADR-0007 selects implementation. It is written so that an agent can execute it without re-deriving the design.

### 5.1 The grant configuration shape

Extend `ClientGrant` in `crates/eggwork-server/src/operations.rs` with an optional resource-scoped grant list. The shape must:

- be additive — a grant without a resource list keeps today's operation-level meaning;
- name the **operation** the scope applies to, so a scope cannot widen an operation the principal does not hold;
- name the **resource kind** and the **resource identifier** — the identifier being the string form of the corresponding `ResourceId` variant;
- be bounded — a per-grant cap and a per-node cap on the total number of resource-scoped grants, both validated in `OperatorConfig::validate`, alongside the existing caps;
- be validated — an unknown operation name, an unknown resource kind, a malformed identifier, a resource scope for an operation the grant does not hold, and a duplicate scope must each be a configuration error, not a silently ignored entry;
- remain `#[serde(deny_unknown_fields)]`, so a typo in a scope is a refusal rather than a silently absent scope.

The identifier is the **string form** of the `ResourceId` variant, and the implementation must parse it back into the typed variant — `ExecutionId::new`, `BlobDigest::parse`, `WorkspaceId::new`, `ArtifactId::new` — so that the configuration and the wire path agree on what an identifier is. A scope that names an identifier the corresponding type rejects is a configuration error.

### 5.2 The owner fence

A resource-scoped grant must not be usable to reach a resource the principal does not own. Two different fences apply, and the implementation must distinguish them:

- **Execution, Workspace, Artifact** — these have an owner. The store already compares `principal_id` for them: `ExecutionStore::load_snapshot_for_principal` selects by `(execution_id, principal_id)`; the artifact store's `load` selects by `(artifact_id, principal_id)`; the workspace store's `lookup_manifest` selects by `(principal_id, manifest_digest)`. The authorizer's resource-scoped decision must therefore be **consistent with** the store's ownership, and the milestone must decide which of the two is authoritative when they disagree. The recommended disposition is that the **store remains authoritative** for ownership — the authorizer may narrow, never widen — so that a resource-scoped grant cannot be used to reach a foreign resource even if the grant list is misconfigured. The milestone must prove that property with a test that grants a scope and then attempts to reach a resource owned by a different principal.
- **Blob** — there is no owner. Blob storage is content-addressed and shared; the same digest is the same bytes for every principal. A blob scope must therefore be an **explicit digest list**, never an owner comparison, and the implementation must say so in the code and the documentation. This is the case the threat model's remaining-boundary column names.

### 5.3 The authorizer's decision

`GrantAuthorizer` must implement `authorize_request` so that the typed `ResourceId` is consulted. The decision rule must be:

- the operation grant must be present — a resource scope never *grants* an operation the principal does not hold;
- when a resource scope is configured for that operation and that resource kind, the resource must be within the granted set;
- when no resource scope is configured for that operation and that resource kind, the decision falls back to the operation-level grant, exactly as today — this preserves existing deployments' behaviour and is the compatibility contract;
- when the request carries no resource (`resource: None`), the decision falls back to the operation-level grant.

The implementation must keep the **existing** rule from `architecture/overview.md`'s positive notes: "Authorization strictly precedes resource resolution in `dispatch`, and id parsing touches only the path, so a denied caller cannot distinguish an existing resource from an absent one." A resource-scoped denial must not introduce a distinguishable failure.

### 5.4 The operations the resource scope applies to

The scope applies to the operations whose resources the six call sites already name:

| Operation | Resource kind | Call sites |
|---|---|---|
| `BlobRead` | `Blob` | the blob-find-missing handler |
| `BlobWrite` | `Blob` | the blob-prepare handler |
| `WorkspaceCreate` | `Workspace` | the workspace-create and workspace-derive handlers |
| `Execute` | `Execution` | the execute handler |
| `Execute` | `Workspace` | the execute handler, when the request names one |

`resource_for_path` additionally produces a resource for `Observe`, `Cancel`, `Renew`, `Events`, and `ArtifactRead`, and the `dispatch`-level call passes it. The milestone must decide, explicitly, whether the scope applies to those operations too. The recommended disposition is that it **does**, because `dispatch` already constructs the resource for them and the six handler-level call sites are the ones that would need additional coverage — leaving `dispatch` unscoped while scoping the handlers would produce an inconsistent boundary.

### 5.5 The configuration, operator, and documentation surface

- The node configuration gains the new field; `examples/eggworkd.example.json` must show it.
- `config print` must redact it consistently with the existing redactions, and must not print a private key path or a certificate fingerprint.
- `architecture/server-node.md` and `architecture/operations-cli.md` must describe the new decision rule.
- `plans/security/threat-model.md`'s remaining-boundary column must be updated to state what is now enforced.

## 6. Required production changes — Branch B: remove the hook and the plumbing

This branch is the work required **if** ADR-0007 selects removal. It is written so that an agent can execute it without re-deriving the design.

### 6.1 The removals

In `crates/eggwork-server/src/lib.rs`:

1. delete `pub enum ResourceId` and its four variants;
2. delete the `resource` field from `pub struct OperationRequest`, leaving `operation` as its only field;
3. delete `fn resource_for_path(operation, path) -> Option<ResourceId>`;
4. delete `fn authorize_resource(state, principal, operation, resource) -> bool`;
5. remove the `resource` argument from every `authorize_resource` call and collapse the six handler-level call sites to the operation-level `state.authorizer.authorize(...)` form — the blob-find-missing handler, the blob-prepare handler, the workspace-create handler, the workspace-derive handler, and the two execute-handler call sites;
6. change the `dispatch` function to build an `OperationRequest` with no resource, or to call the operation-level authorizer directly;
7. remove the `authorize_request` method from the public `Authorizer` trait, and remove its default implementation.

### 6.2 The test removal

Remove the unit test in `crates/eggwork-server/src/lib.rs`'s test module that asserts `resource_for_path` maps a path segment to a `ResourceId`. If the `dispatch` path's routing test asserts on the resolved resource, that assertion must be removed with it, and the routing test must be rewritten to assert on the *operation* rather than on the resource.

### 6.3 The compatibility note — this is a public API removal

`ResourceId`, `OperationRequest`, and the `Authorizer` trait are `pub` in `eggwork-server`, which is a published workspace crate. Removing `ResourceId` and the `authorize_request` method is a **breaking change to that crate's public API**. The milestone must:

- enumerate every in-tree consumer of the removed items and state that each was migrated;
- state whether any out-of-tree embedder is known to exist, and if the answer is unknown, say so explicitly rather than assuming the API is free to change;
- reflect the removal in the release version and notes, and not bundle it silently into a release that claims to be a qualification-only candidate;
- record in the closure record that the removal is the reason the divergence is closed: the mechanism no longer *looks* available.

### 6.4 The boundary that remains after removal

Removing the hook does **not** remove the boundary; it removes the pretence. The store's `principal_id` fences remain the enforcement for executions, artifacts, and workspaces. The blob boundary remains exactly what the threat model says it is: a principal holding `BlobRead` may read any known digest. The milestone must make sure the documentation says that plainly, and must not let the removal be described as if it closed the blob boundary.

### 6.5 The documentation surface

- `architecture/overview.md`'s divergence #4 is rewritten or closed: the row must no longer describe an inert hook as if it were a live mechanism. If the row is closed, it must say what now enforces the boundary and what residual boundary remains.
- `architecture/server-node.md` and `architecture/operations-cli.md` must stop describing a resource-aware hook.
- `plans/security/threat-model.md`'s remaining-boundary column must be updated to state the boundary as it is, without the "unless resource-scoped policy is configured" clause if no such policy can now be configured.

## 7. Failure, cancellation, restart, and contention semantics

- **Authorization precedes everything.** Both branches preserve the rule that authorization runs before resource resolution and before any store lookup that could disclose existence. A resource-scoped denial must be indistinguishable from an operation-level denial.
- **No state machine change.** Neither branch touches the execution state machine, leases, generations, or fencing.
- **No restart or recovery change.** Both branches are authorization-decision changes; neither alters what is persisted or how a node recovers.
- **Contention.** Neither branch adds a lock on the authorization path. The authorizer's decision must remain a bounded, in-memory decision over the configuration-derived grant tables.
- **Failure ordering.** A configuration error must be detected at load time, before the node accepts traffic, and must refuse the node — not degrade to a permissive decision.

## 8. Compatibility and migration

**Branch A.** Additive to the configuration surface; the default behaviour for a grant without a resource list is unchanged, so an existing deployment's behaviour is unchanged. The change to *decisions* applies only to principals that configure resource scopes. The public API changes are additive (`GrantAuthorizer` gains an implementation of an existing trait method; `ClientGrant` gains an optional field). `ClientGrant` is `pub` and serialized — adding an optional field is a compatible change, but `#[serde(deny_unknown_fields)]` means an **older** node reading a **newer** configuration with a resource scope will refuse to start. That refusal is correct and must be documented: a node must not silently ignore a scope it does not understand.

**Branch B.** A breaking public-API removal, as described in §6.3.

**Both branches.** No wire protocol change to request or response shapes is required by either branch as written. If the decided disposition requires one, the ADR must say so and the change must be coordinated with the control-plane shared-contract corrective, which owns `API_SCHEMA_VERSION`.

## 9. Required tests

Required, and each must be non-vacuous.

### Common to both branches

1. **The divergence table matches the code.** A test or review assertion that the disposition recorded in `architecture/overview.md` is the disposition the code implements. In Branch A, a test proving that `GrantAuthorizer::authorize_request` is reached and consults the resource — for instance, that two principals with the same operation grants but different resource scopes receive different answers for the same resource. In Branch B, an assertion that no `ResourceId` and no `authorize_request` remain, so the hook cannot be inert again.
2. **No regression in the existing boundary.** The existing server authorization tests — the operation-level grant matrix, the "request payload cannot supply authoritative principal" test, and the routing test — remain green and unchanged in their assertions.
3. **Authorization still precedes resource resolution.** A test proving that an unauthorized principal's request is refused at the same point and with the same response as at the reviewed baseline, and that the response does not disclose whether the named resource exists.

### Branch A only

4. **A resource scope actually narrows.** A test proving that a principal with the `BlobRead` operation grant and a scope naming one digest is refused a *different* digest, and permitted the named one. Without this test, Branch A could land as an inert hook again — which is the defect being corrected.
5. **The owner fence holds.** A test proving that a resource-scoped grant naming a resource owned by a different principal does not permit access to it, for each of `Execution`, `Workspace`, and `Artifact`.
6. **Blobs have no owner fence, and the scope says so.** A test proving that a blob scope is an explicit digest list and that a principal with a blob scope cannot read a digest outside it.
7. **Operation grants are never widened by a scope.** A test proving that a resource scope for an operation the principal does not hold grants nothing.
8. **No scope configured means today's behaviour.** A test proving that a principal with no resource scopes receives exactly the operation-level decision, so existing deployments are unaffected.
9. **Configuration failures are refused.** Tests proving that an unknown operation, an unknown resource kind, a malformed identifier, a scope for an unheld operation, a duplicate scope, and an over-cap grant count each cause the configuration to be rejected and the node to refuse to start.
10. **Redaction.** A test proving that `config print` does not print the resource scope identifiers in a way that leaks a private key path or a certificate fingerprint, consistent with the existing redactions.

### Branch B only

11. **The hook is gone.** An assertion that `ResourceId`, `OperationRequest.resource`, `resource_for_path`, `authorize_resource`, and the `authorize_request` trait method no longer exist — a test that fails if any of them is reintroduced.
12. **The six call sites are gone.** An assertion that no handler-level call constructs a typed resource, and that the six sites now authorize at the operation level.
13. **Behaviour is unchanged.** The existing authorization test matrix is green and unchanged, proving the removal did not change any decision.
14. **The residual boundary is stated, not hidden.** A documentation assertion — or an explicit closure-record statement — that the blob boundary remains exactly as the threat model describes it, and that the removal closed the *pretence*, not the boundary.

## 10. Required verification

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

The authorization path is exercised entirely on the host; no platform matrix is required for the decision logic itself. If either branch changes a request or response wire shape, the change must additionally be exercised across the release matrix, and the platform matrix actually exercised must be stated honestly.

## 11. Documentation updates

- `architecture/overview.md` — divergence #4 must end up reflecting the decided disposition. This is a required update in **both** branches, and it is the single most important documentation consequence of the milestone.
- `architecture/server-node.md` — the authorization decision path, whichever branch is decided.
- `architecture/operations-cli.md` — the grant configuration surface, in Branch A; the absence of a resource-scope surface, in Branch B.
- `architecture/protocol.md` — only if a wire shape changes.
- `plans/security/threat-model.md` — the "Authenticated unauthorized principal" row's remaining-boundary column.
- `examples/eggworkd.example.json` — Branch A only.
- `AGENTS.md` — the hard-invariant list does not need a new entry. If the "Node trust requires a verified mTLS client CA and explicit per-principal grants" line is extended, keep the wording in the same style and keep it true.
- `plans/subsystems/security-isolation-resource-roadmap.md` — add the M005c milestone entry and set the roadmap's status line to reflect it.
- `plans/registry.md` — updated by the registry owner when the milestone is registered; this plan does not edit it.

## 12. Acceptance criteria

Common to both branches:

1. The decided branch is recorded in `plans/adrs/ADR-0007-resource-scoped-authorization.md` as **accepted**, with the status changed from `proposed`.
2. `architecture/overview.md`'s divergence #4 reflects the decided disposition.
3. `plans/security/threat-model.md` states the boundary as it is after the change.
4. The node does not present a mechanism as available when it is inert: in Branch A the hook decides and a test proves it; in Branch B the hook and its plumbing are gone and a test proves they stay gone.
5. Authorization still precedes resource resolution, and an unauthorized caller cannot distinguish an existing resource from an absent one.
6. The store's existing owner fences are not weakened, and the existing authorization test matrix is green and unchanged.
7. The configuration loader still fails closed on a malformed or unknown field.
8. No wire protocol change, unless the ADR decided one and the change is coordinated with the control-plane shared-contract corrective.
9. The full pre-submit gate is green on the reviewed head.

Branch A additionally:

10. A resource scope measurably narrows an operation grant, proven by a test that fails if the scope is ignored.
11. The owner fence holds for `Execution`, `Workspace`, and `Artifact`, and blobs are governed by an explicit digest list.
12. A principal with no resource scopes receives exactly the operation-level decision.
13. Every configuration error class is refused at load time.

Branch B additionally:

14. `ResourceId`, `OperationRequest.resource`, `resource_for_path`, `authorize_resource`, the `authorize_request` trait method, the six resource-scoped call sites, and the unit test are all removed, and a test proves they stay removed.
15. The removal is recorded as a breaking public-API change, with the in-tree consumer enumeration, the out-of-tree position stated honestly, and the release version and notes updated.

## 13. Stop conditions

Stop and re-plan, recording the blocker, if any of the following occurs:

- the ADR is still `proposed` when implementation is proposed to begin — this is the milestone's own blocker, and per `plans/003-planning-process.md` §4 it must not be worked around;
- either branch would require weakening the store's `principal_id` fences, or would let a resource-scoped grant reach a resource owned by another principal;
- either branch would make a denied caller able to distinguish an existing resource from an absent one;
- either branch would require the authorizer to consult the store on the authorization path in a way that introduces a lock or an ordering dependency that authorization does not have today;
- either branch would change the wire protocol without a decision recorded in the ADR and coordinated with the control-plane shared-contract corrective;
- implementing Branch A would require a general policy language, a role system, or a rule engine — that is a different decision and must be escalated;
- the milestone cannot be completed without weakening an authentication, lease, generation, fencing, or digest bound.

## 14. Closure evidence required

Create:

- `plans/closure/security-isolation-resource/005c-status.md`

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- the **accepted** ADR's identity, its status, and the decision it records;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output;
- negative and adversarial evidence: in Branch A, a resource scope shown to refuse a resource outside the scope and a foreign-owned resource shown to remain refused; in Branch B, an assertion that the removed items cannot be reintroduced silently;
- compatibility review: for Branch A, the configuration compatibility position including the `deny_unknown_fields` refusal by an older node; for Branch B, the public-API removal, the in-tree consumer enumeration, the out-of-tree position stated honestly, and the release-visibility consequence;
- documentation status, including the divergences-table row, the threat-model row, and the deep dives;
- the residual boundary that remains after the change, stated plainly — including, in Branch B, that the blob boundary remains exactly as the threat model describes it;
- unresolved findings ranked by severity;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 15. Handoff notes

- **This plan is blocked and must not be handed off as ready.** The blocker is concrete and named: `plans/adrs/ADR-0007-resource-scoped-authorization.md`, which is **proposed, not accepted**. It is written now, per `plans/003-planning-process.md` §4, to preserve the design intent so that the implementation is mechanical once the decision is made.
- The plan is written so that **either** ADR outcome is executable without re-deriving the design. §5 is Branch A's work; §6 is Branch B's work; §4, §7, §8, §9, §10, §11, §12, and §14 hold in both.
- The decision is durable and needs a human's judgement, not an implementer's. The two branches differ enormously in cost and in what they promise a caller: one adds a real per-resource boundary, the other removes a false promise.
- The defect is **already recorded**. This milestone must not be reported as a newly discovered defect, and the closure record must say that the gap was known and is now decided.
- If the ADR defers with a third option — keeping the mechanism present while advertising that it is unavailable — that option must still satisfy the invariant in §4.1, and the ADR must say concretely how a caller learns that resource-scoped authorization is not in force. A comment in the source is not that.
- This milestone is independent of M005a and M005b and may be executed in parallel with them, with the foundation runner-request-validation corrective, with the control-plane shared-contract corrective, and with Operations M007.
